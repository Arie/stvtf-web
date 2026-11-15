//! The server: axum serves the routes with their layers; each request's work is done on a
//! blocking thread (see `asked::answer`), and an event stream is written by a thread of its own.

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

/// Serves `app` on `listen` until `stop`: requests under way are then finished, and event
/// streams end (see `events::stream`).
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
        // What is under way gets a while to finish: a client that will not let go does not hold
        // the process up.
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
    // Every write is a whole response or a flushed batch of events: no waiting to fill a packet
    // (with delayed acks, up to 40 ms an event).
    let listener = listener.tap_io(|tcp| {
        let _ = tcp.set_nodelay(true);
    });
    let service = router(app).into_make_service_with_connect_info::<SocketAddr>();
    Ok(axum::serve(listener, service).with_graceful_shutdown(stopped).await?)
}

/// A request is answered in this time, its body read: an upload's part on a slow line, at most.
const ANSWER_WITHIN: Duration = Duration::from_mins(5);

/// The app's routes (see [`super::routes`]). Every response says `nosniff`, and what is read
/// with GET may be read by any site (it is public: `<stv-map>` on other sites reads it all,
/// without cookies). Nothing is changed at another site's asking (a browser says where a request
/// is from), and the pages that act for whoever is signed in are not shown in another site's
/// frame.
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
        // What compresses well is compressed for whoever takes it; an event stream compresses
        // itself, flushing each batch (`events::stream`).
        .layer(CompressionLayer::new())
}
