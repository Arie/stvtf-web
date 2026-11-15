//! The server: axum with tower-http layers; handlers work on blocking threads (`asked::answer`).

use super::Shared;
use anyhow::Result;
use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::header::{self, HeaderValue};
use axum::http::{Method, StatusCode};
use axum::serve::ListenerExt;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tower_http::compression::CompressionLayer;
use tower_http::cors::{self, CorsLayer};
use tower_http::csrf::CsrfLayer;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

/// Serves `app` on `listen` until `stop`, then finishes requests and ends event streams.
pub(crate) fn run(listen: &str, app: Shared, stop: &'static AtomicBool) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind(listen).await?;
        log!("http://{listen}/");
        let stopped = async move {
            // The flag is the watchers' too, which are no tasks: looked at now and then.
            while !stop.load(Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            super::events::end_all();
        };
        // A grace period: a stuck client does not hold the process up.
        tokio::select! {
            served = serve(listener, app, stopped) => served?,
            () = stopped_for(stop, FINISH_WITHIN) => {}
        }
        Ok(())
    })
}

/// How long requests under way have to finish once the server is told to stop.
const FINISH_WITHIN: Duration = Duration::from_secs(5);

/// Once `stop` has been set for `long`.
async fn stopped_for(stop: &'static AtomicBool, long: Duration) {
    while !stop.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(long).await;
}

/// Serves `app` on `listener` until `stopped`, then finishes what is under way.
pub(crate) async fn serve(
    listener: tokio::net::TcpListener,
    app: Shared,
    stopped: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    // No Nagle: it delays events by up to 40 ms.
    let listener = listener.tap_io(|tcp| {
        let _ = tcp.set_nodelay(true);
    });
    let service = router(app).into_make_service_with_connect_info::<SocketAddr>();
    Ok(axum::serve(listener, service).with_graceful_shutdown(stopped).await?)
}

/// A request is answered in this time, its body read: an upload's part on a slow line, at most.
const ANSWER_WITHIN: Duration = Duration::from_mins(5);

/// The routes with their layers: `nosniff`, GET readable by any site (`<stv-map>` embeds),
/// cross-site writes refused, signed-in pages not framed.
pub(crate) fn router(app: Shared) -> Router {
    let (all, acting) = super::routes::routes(&app);
    let unframed = SetResponseHeaderLayer::overriding(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    all.merge(acting.layer(unframed))
        .with_state(app)
        .layer(DefaultBodyLimit::max(super::MAX_REQUEST as usize))
        .layer(CsrfLayer::new())
        .layer(SetResponseHeaderLayer::overriding(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")))
        .layer(CorsLayer::new().allow_origin(cors::Any).allow_methods([Method::GET]))
        .layer(TimeoutLayer::with_status_code(StatusCode::REQUEST_TIMEOUT, ANSWER_WITHIN))
        // Each request, for whoever asks to see them (`RUST_LOG=tower_http=debug`).
        .layer(TraceLayer::new_for_http())
        // Event streams compress themselves, flushing per batch (`events::stream`).
        .layer(CompressionLayer::new())
}
