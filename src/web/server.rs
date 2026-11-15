//! The server: hyper takes the connections and reads the requests, axum routes them with its
//! layers; each is answered on a blocking thread (see [`super::routes`]), and an event stream is
//! written by a thread of its own.

use super::Shared;
use anyhow::Result;
use axum::Router;
use axum::extract::{ConnectInfo, DefaultBodyLimit};
use axum::http::header::{self, HeaderValue};
use axum::http::{Method, StatusCode};
use hyper::server::conn::http1;
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::service::TowerToHyperService;
use std::net::SocketAddr;
use std::time::Duration;
use tower_http::cors::{self, CorsLayer};
use tower_http::csrf::CsrfLayer;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::TimeoutLayer;

/// A request's line and headers arrive in this time, and are no more than twice
/// [`super::MAX_REQUEST`] (what is read at once: them, and the start of a body).
const HEAD_WITHIN: Duration = Duration::from_secs(10);

/// Serves `app` on `listen` until the process ends.
pub(crate) fn run(listen: &str, app: Shared) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind(listen).await?;
        log!("http://{listen}/");
        let (router, http) = (router(app), http());
        loop {
            let Ok((tcp, peer)) = listener.accept().await else {
                // Out of file descriptors, say: not in a spin.
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            };
            let (router, http) = (router.clone(), http.clone());
            tokio::spawn(async move {
                let _ = connection(tcp, peer, router, &http).await;
            });
        }
    })
}

/// How connections are served: HTTP/1 (a proxy is in front), the request's head bounded in size
/// and time.
pub(crate) fn http() -> http1::Builder {
    let mut http = http1::Builder::new();
    http.timer(TokioTimer::new()).header_read_timeout(HEAD_WITHIN).max_buf_size(2 * super::MAX_REQUEST as usize);
    http
}

/// Serves the requests of one connection, from `peer`.
pub(crate) async fn connection(tcp: tokio::net::TcpStream, peer: SocketAddr, router: Router, http: &http1::Builder) -> Result<()> {
    // Every write is a whole response or a flushed batch of events: no waiting to fill a packet
    // (with delayed acks, up to 40 ms an event).
    tcp.set_nodelay(true)?;
    // The routes are shared by every connection: each request is told whose it is.
    let shared = TowerToHyperService::new(router);
    let service = hyper::service::service_fn(move |mut request: hyper::Request<hyper::body::Incoming>| {
        request.extensions_mut().insert(ConnectInfo(peer));
        hyper::service::Service::call(&shared, request)
    });
    Ok(http.serve_connection(TokioIo::new(tcp), service).await?)
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
}
