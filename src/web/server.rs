//! The server: axum's routes with tower-http layers, on hyper (`axum::serve` has no header
//! timeout); handlers work on blocking threads (`asked::answer`).

use super::Shared;
use anyhow::Result;
use axum::Router;
use axum::extract::{ConnectInfo, DefaultBodyLimit};
use axum::http::header::{self, HeaderValue};
use axum::http::{Method, StatusCode};
use hyper::server::conn::http1;
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::server::graceful::GracefulShutdown;
use hyper_util::service::TowerToHyperService;
use sentry::integrations::tower::{NewSentryLayer, SentryHttpLayer};
use std::future::Future;
use std::pin::pin;
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
    let served = runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind(listen).await?;
        tracing::info!("http://{listen}/");
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
    });
    // Handlers still at work on blocking threads are not waited for.
    runtime.shutdown_timeout(Duration::from_secs(1));
    served
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

/// A request's head arrives in this time, as does the next on a connection kept open.
const HEAD_WITHIN: Duration = Duration::from_secs(30);

/// Serves `app` on `listener` until `stopped`, then finishes what is under way.
pub(crate) async fn serve(
    listener: tokio::net::TcpListener,
    app: Shared,
    stopped: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let router = router(app);
    let mut http = http1::Builder::new();
    http.timer(TokioTimer::new()).header_read_timeout(HEAD_WITHIN);
    let graceful = GracefulShutdown::new();
    let mut stopped = pin!(stopped);
    loop {
        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
            () = &mut stopped => break,
        };
        let Ok((tcp, peer)) = accepted else {
            // Out of file descriptors, say: not in a spin.
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        };
        // No Nagle: it delays events by up to 40 ms.
        let _ = tcp.set_nodelay(true);
        let shared = TowerToHyperService::new(router.clone());
        let service = hyper::service::service_fn(move |mut request: hyper::Request<hyper::body::Incoming>| {
            request.extensions_mut().insert(ConnectInfo(peer));
            hyper::service::Service::call(&shared, request)
        });
        let (http, watcher) = (http.clone(), graceful.watcher());
        tokio::spawn(async move {
            // hyper's timer starts at the first byte: one that sends none is not kept either.
            if !matches!(tokio::time::timeout(HEAD_WITHIN, tcp.peek(&mut [0])).await, Ok(Ok(1..))) {
                return;
            }
            let _ = watcher.watch(http.serve_connection(TokioIo::new(tcp), service)).await;
        });
    }
    graceful.shutdown().await;
    Ok(())
}

/// A request is answered in this time, its body read: an upload's part on a slow line, at most.
const ANSWER_WITHIN: Duration = Duration::from_mins(5);

/// Gives a request's trace id to its log lines and its answer (`x-trace-id`), and tags it with its
/// room or match.
async fn traced(req: axum::extract::Request, next: axum::middleware::Next) -> axum::response::Response {
    let id = crate::report::trace_id();
    crate::report::tag_path(req.uri().path());
    let span = tracing::info_span!("trace", id = id.as_deref().unwrap_or_default());
    let mut res = tracing::Instrument::instrument(next.run(req), span).await;
    if let Some(v) = id.and_then(|i| HeaderValue::from_str(&i).ok()) {
        res.headers_mut().insert("x-trace-id", v);
    }
    res
}

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
        .layer(axum::middleware::from_fn(traced))
        // Each request, for whoever asks to see them (`RUST_LOG=tower_http=debug`).
        .layer(TraceLayer::new_for_http())
        // A trace per request, or the caller's (`sentry-trace`), in a Sentry scope of its own.
        .layer(SentryHttpLayer::new().enable_transaction())
        .layer(NewSentryLayer::<axum::extract::Request>::new_from_top())
        // Event streams compress themselves, flushing per batch (`events::stream`).
        .layer(CompressionLayer::new())
}
