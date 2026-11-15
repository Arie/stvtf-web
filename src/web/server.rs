//! The server: hyper takes the connections and reads the requests, axum routes them with its
//! layers; each is answered on a blocking thread (see [`super::answer`]), and an event stream is
//! written by a thread of its own.

use super::{Body as Answer, Events, Out, Reply, Request, client_of, cookie, voice_link};
use super::{GUEST_COOKIE, USER_COOKIE};
use crate::App;
use crate::matches::sessions;
use anyhow::Result;
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, State};
use axum::http::header::{self, HeaderName, HeaderValue};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use hyper::server::conn::http1;
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::service::TowerToHyperService;
use std::io::Write;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::error::TrySendError;
use tower_http::set_header::SetResponseHeaderLayer;

/// Event streams open at once; more are refused.
pub(crate) const MAX_STREAMS: usize = 512;
/// Flushes of events that may wait for a slow client, and how long one waits before it is gone.
const QUEUED: usize = 64;
const SLOW_FOR: Duration = Duration::from_secs(30);
const SLOW_STEP: Duration = Duration::from_millis(20);

/// A request's line and headers arrive in this time, and are no more than [`super::MAX_REQUEST`].
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
    http.timer(TokioTimer::new()).header_read_timeout(HEAD_WITHIN).max_buf_size(super::MAX_REQUEST as usize);
    http
}

/// Serves the requests of one connection, from `peer`.
pub(crate) async fn connection(tcp: tokio::net::TcpStream, peer: SocketAddr, router: Router, http: &http1::Builder) -> Result<()> {
    // Every write is a whole response or a flushed batch of events: no waiting to fill a packet
    // (with delayed acks, up to 40 ms an event).
    tcp.set_nodelay(true)?;
    let service = TowerToHyperService::new(router.layer(axum::Extension(ConnectInfo(peer))));
    Ok(http.serve_connection(TokioIo::new(tcp), service).await?)
}

/// Every request goes to [`handle`]; every response says `nosniff` and may be read by any site
/// (everything here is public: `<stv-map>` on other sites reads it all).
pub(crate) fn router(app: Arc<App>) -> Router {
    Router::new()
        .fallback(handle)
        .with_state(app)
        .layer(SetResponseHeaderLayer::overriding(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")))
        .layer(SetResponseHeaderLayer::overriding(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*")))
}

async fn handle(State(app): State<Arc<App>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, request: axum::extract::Request) -> Response {
    let (request, client) = match request_of(peer.ip(), request).await {
        Ok(asked) => asked,
        Err(refused) => return refused.into_response(),
    };
    let answered = tokio::task::spawn_blocking(move || {
        let out = Out::default();
        super::answer(out.clone(), &app, &request, client).ok().and_then(|()| out.take())
    });
    response_of(answered.await.ok().flatten())
}

/// What the handlers need of a request, its body read; and who asks. Refused: a body over the
/// most it may be that does not say so itself.
async fn request_of(peer: IpAddr, request: axum::extract::Request) -> Result<(Request, IpAddr), StatusCode> {
    let (parts, body) = request.into_parts();
    let all = |name: HeaderName| -> String {
        let values: Vec<&str> = parts.headers.get_all(name).iter().filter_map(|v| v.to_str().ok()).collect();
        values.join(", ")
    };
    let cookies = all(header::COOKIE).replace(", ", "; ");
    let mut asked = Request {
        method: parts.method.as_str().to_string(),
        path: parts.uri.path().to_string(),
        query: parts.uri.query().unwrap_or_default().to_string(),
        length: one(&parts.headers, header::CONTENT_LENGTH).and_then(|l| l.trim().parse().ok()).unwrap_or(0),
        gzip: all(header::ACCEPT_ENCODING).split(',').any(|e| e.split(';').next().is_some_and(|e| e.trim().eq_ignore_ascii_case("gzip"))),
        forwarded: forwarded(&all(HeaderName::from_static("x-forwarded-for"))),
        bearer: one(&parts.headers, header::AUTHORIZATION).and_then(|a| a.trim().strip_prefix("Bearer ")).map(|t| t.trim().to_string()),
        guest: cookie(&cookies, GUEST_COOKIE).filter(|g| sessions::is_id(g)).map(str::to_string),
        user: cookie(&cookies, USER_COOKIE).map(str::to_string),
        voice_link: cookie(&cookies, voice_link::STATE_COOKIE).filter(|g| sessions::is_id(g)).map(str::to_string),
        agent: one(&parts.headers, header::USER_AGENT).unwrap_or_default().trim().to_string(),
        body: Vec::new(),
    };
    let limit = super::body_limit(&asked);
    // One that says it is over the most: its handler says so, the body unread.
    if asked.length <= limit {
        let Ok(read) = axum::body::to_bytes(body, limit).await else {
            return Err(StatusCode::PAYLOAD_TOO_LARGE);
        };
        asked.length = read.len();
        asked.body = read.to_vec();
    }
    let client = client_of(&asked.forwarded, peer);
    Ok((asked, client))
}

/// `X-Forwarded-For`'s addresses, nearest hop last; none if any of it is not an address.
pub(super) fn forwarded(value: &str) -> Vec<IpAddr> {
    value.split(',').filter(|a| !a.trim().is_empty()).map(|a| a.trim().parse()).collect::<Result<_, _>>().unwrap_or_default()
}

fn one(headers: &HeaderMap, name: HeaderName) -> Option<&str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// A handler's reply as the framework sends it: none is a fault.
fn response_of(reply: Option<Reply>) -> Response {
    let Some(reply) = reply else { return StatusCode::INTERNAL_SERVER_ERROR.into_response() };
    let body = match reply.body {
        Answer::Whole(body) => Body::from(body),
        Answer::Events(events) => match events_body(events) {
            Some(body) => body,
            None => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        },
    };
    let mut response = Response::new(body);
    let status = reply.status.get(..3).and_then(|code| code.parse::<u16>().ok()).and_then(|code| StatusCode::from_u16(code).ok());
    *response.status_mut() = status.unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    for (name, value) in reply.headers {
        if let (Ok(name), Ok(value)) = (HeaderName::try_from(name), HeaderValue::try_from(value)) {
            response.headers_mut().append(name, value);
        }
    }
    response
}

static STREAMS: AtomicUsize = AtomicUsize::new(0);

/// Counts an event stream until dropped.
struct Streaming;

impl Streaming {
    fn open() -> Option<Self> {
        if STREAMS.fetch_add(1, Ordering::Relaxed) >= MAX_STREAMS {
            STREAMS.fetch_sub(1, Ordering::Relaxed);
            return None;
        }
        Some(Streaming)
    }
}

impl Drop for Streaming {
    fn drop(&mut self) {
        STREAMS.fetch_sub(1, Ordering::Relaxed);
    }
}

/// A body of what `events` writes, each flush a chunk, on a thread of its own until the client
/// is gone. None when too many are open.
fn events_body(events: Events) -> Option<Body> {
    let open = Streaming::open()?;
    let (tx, rx) = tokio::sync::mpsc::channel(QUEUED);
    std::thread::spawn(move || {
        let _open = open;
        let _ = events(&mut Chunks { tx, held: Vec::new() });
    });
    Some(Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(rx)))
}

/// What is written, sent on as a chunk at each flush.
struct Chunks {
    tx: tokio::sync::mpsc::Sender<std::io::Result<Bytes>>,
    held: Vec<u8>,
}

impl Write for Chunks {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.held.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    /// Fails once the client is gone, or has taken nothing for [`SLOW_FOR`].
    fn flush(&mut self) -> std::io::Result<()> {
        if self.held.is_empty() {
            return Ok(());
        }
        let mut chunk = Ok(Bytes::from(std::mem::take(&mut self.held)));
        let since = Instant::now();
        loop {
            match self.tx.try_send(chunk) {
                Ok(()) => return Ok(()),
                Err(TrySendError::Closed(_)) => return Err(std::io::ErrorKind::BrokenPipe.into()),
                Err(TrySendError::Full(_)) if since.elapsed() > SLOW_FOR => return Err(std::io::ErrorKind::TimedOut.into()),
                Err(TrySendError::Full(back)) => chunk = back,
            }
            std::thread::sleep(SLOW_STEP);
        }
    }
}
