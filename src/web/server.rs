//! The server: hyper takes the connections and reads the requests, axum routes them with its
//! layers; each is answered on a blocking thread (see [`super::routes`]), and an event stream is
//! written by a thread of its own.

use super::routes::Asked;
use super::{Answered, Asks, GUEST_COOKIE, Request, USER_COOKIE, client_of, voice_link};
use crate::App;
use crate::matches::sessions;
use anyhow::Result;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, DefaultBodyLimit, FromRequestParts, Query};
use axum::http::header::{self, HeaderName, HeaderValue};
use axum::http::request::Parts;
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Form, Json};
use axum_extra::extract::CookieJar;
use hyper::server::conn::http1;
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::service::TowerToHyperService;
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tower_http::cors::{self, CorsLayer};
use tower_http::csrf::CsrfLayer;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::TimeoutLayer;

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A request's line and headers arrive in this time, and are no more than twice
/// [`super::MAX_REQUEST`] (what is read at once: them, and the start of a body).
const HEAD_WITHIN: Duration = Duration::from_secs(10);

/// Serves `app` on `listen` until the process ends.
pub(crate) fn run(listen: &str, app: Arc<App>) -> Result<()> {
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
pub(crate) fn router(app: Arc<App>) -> Router {
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

/// `work`, which may block, on a thread where it may; a fault if it does not end well.
async fn off_thread(work: impl FnOnce() -> Answered + Send + 'static) -> Response {
    match tokio::task::spawn_blocking(work).await {
        Ok(answered) => answered.unwrap_or_else(|early| *early.0),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

type Answering = BoxFuture<'static, Response>;

/// A route's handler: `answer` to a request without a body.
pub(super) fn to(answer: fn(&Asked) -> Answered) -> impl Fn(Asked) -> Answering + Clone + Send + Sync + 'static {
    move |asked| Box::pin(off_thread(move || answer(&asked)))
}

/// To a request with a JSON body, read as `T`.
pub(super) fn json<T: DeserializeOwned + Send + 'static>(
    answer: fn(&Asked, T) -> Answered,
) -> impl Fn(Asked, Json<T>) -> Answering + Clone + Send + Sync + 'static {
    move |asked, Json(body)| Box::pin(off_thread(move || answer(&asked, body)))
}

/// To a request that may have a JSON body.
pub(super) fn json_if_any<T: DeserializeOwned + Send + 'static>(
    answer: fn(&Asked, Option<T>) -> Answered,
) -> impl Fn(Asked, Option<Json<T>>) -> Answering + Clone + Send + Sync + 'static {
    move |asked, body| Box::pin(off_thread(move || answer(&asked, body.map(|Json(body)| body))))
}

/// To a form.
pub(super) fn form<T: DeserializeOwned + Send + 'static>(
    answer: fn(&Asked, T) -> Answered,
) -> impl Fn(Asked, Form<T>) -> Answering + Clone + Send + Sync + 'static {
    move |asked, Form(body)| Box::pin(off_thread(move || answer(&asked, body)))
}

/// To a request whose body is taken as it is.
pub(super) fn bytes(answer: fn(&Asked, Bytes) -> Answered) -> impl Fn(Asked, Bytes) -> Answering + Clone + Send + Sync + 'static {
    move |asked, body| Box::pin(off_thread(move || answer(&asked, body)))
}

impl FromRequestParts<Arc<App>> for Asked {
    type Rejection = StatusCode;

    /// What the handlers need of a request, and who asks. A query that is none is taken as empty.
    async fn from_request_parts(parts: &mut Parts, app: &Arc<App>) -> Result<Self, StatusCode> {
        let peer = parts.extensions.get::<ConnectInfo<SocketAddr>>().ok_or(StatusCode::INTERNAL_SERVER_ERROR)?.0.ip();
        let query = Query::<HashMap<String, String>>::from_request_parts(parts, app).await.map_or_else(|_| HashMap::new(), |Query(q)| q);
        let all = |name: HeaderName| -> String {
            let values: Vec<&str> = parts.headers.get_all(name).iter().filter_map(|v| v.to_str().ok()).collect();
            values.join(", ")
        };
        let jar = CookieJar::from_headers(&parts.headers);
        let cookie = |name: &str| jar.get(name).map(|c| c.value().to_string());
        let request = Request {
            path: parts.uri.path().to_string(),
            query: Asks(query),
            gzip: all(header::ACCEPT_ENCODING)
                .split(',')
                .any(|e| e.split(';').next().is_some_and(|e| e.trim().eq_ignore_ascii_case("gzip"))),
            bearer: one(&parts.headers, header::AUTHORIZATION).and_then(|a| a.trim().strip_prefix("Bearer ")).map(|t| t.trim().to_string()),
            guest: cookie(GUEST_COOKIE).filter(|g| sessions::is_id(g)),
            user: cookie(USER_COOKIE),
            voice_link: cookie(voice_link::STATE_COOKIE).filter(|g| sessions::is_id(g)),
            agent: one(&parts.headers, header::USER_AGENT).unwrap_or_default().trim().to_string(),
        };
        let client = client_of(&forwarded(&all(HeaderName::from_static("x-forwarded-for"))), peer);
        Ok(Asked { app: app.clone(), request, client })
    }
}

/// `X-Forwarded-For`'s addresses, nearest hop last; none if any of it is not an address.
pub(super) fn forwarded(value: &str) -> Vec<IpAddr> {
    value.split(',').filter(|a| !a.trim().is_empty()).map(|a| a.trim().parse()).collect::<Result<_, _>>().unwrap_or_default()
}

fn one(headers: &HeaderMap, name: HeaderName) -> Option<&str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}
