//! The HTTP side: pages, files, event streams and starting matches.

use crate::App;
use crate::matches::recording::Mark;
use crate::matches::sessions;
use crate::matches::sessions::Sessions;
use anyhow::Result;
use axum::body::Body;
use axum::http::StatusCode;
use axum::http::header::{HeaderName, HeaderValue};
use axum::response::{IntoResponse, Response};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

mod assets;
mod client;
mod events;
mod matches;
mod mine;
mod rooms;
mod routes;
pub(crate) mod server;
pub(crate) mod token;
mod unpack;
mod upload;
mod visitor;
mod voice_in;
pub(crate) mod voice_link;
pub(crate) mod voice_mumble;
pub(crate) mod voice_routes;
pub(crate) mod voice_who;
pub(crate) mod voice_who_sync;

pub(crate) use assets::{app_version, module_preloads};
use client::{client_of, counted};
use matches::{start, start_view};
use visitor::{return_to, set_cookie};

const INDEX: &str = include_str!("../app/pages/index.html");
const LANDING: &str = include_str!("../app/pages/landing.html");
/// The voice bot's Mumble certificates: a page, and JSON.
const MUMBLE: &str = include_str!("../app/pages/mumble.html");
const MUMBLE_JSON: &str = include_str!("../app/pages/mumble.json");
const HTML: &str = "text/html; charset=utf-8";
const CSS: &str = "text/css; charset=utf-8";
const JS: &str = "text/javascript; charset=utf-8";
const TEXT: &str = "text/plain";
const JSON: &str = "application/json";
const WEBP: &str = "image/webp";
const SVG: &str = "image/svg+xml";
const WOFF2: &str = "font/woff2";

/// Who the browser is, to the rooms; and who signed in with the site, still sealed.
const GUEST_COOKIE: &str = "stvtf_guest";
const USER_COOKIE: &str = "stvtf_user";

/// Request line and headers together; the pages send far less.
const MAX_REQUEST: u64 = 16 << 10;

/// Event streams from one address, so a single client cannot take all the connections. A LAN
/// event or an office behind one address stays well under it.
pub(crate) const MAX_STREAMS_PER_CLIENT: usize = 32;

/// The longest delay a page can ask for after its last `/`, to line up with a stream of the match.
const MAX_DELAY_S: f64 = 60.0;

/// How long a browser keeps a file that does not change for a given build.
const CACHE_FOR: Duration = Duration::from_hours(24);

/// What a handler answers with; or with what it answered early: a refusal, a redirect, a fault.
pub(crate) type Answered = Result<Response, Early>;

pub(crate) struct Early(pub(crate) Box<Response>);

impl From<Response> for Early {
    fn from(response: Response) -> Self {
        Early(Box::new(response))
    }
}

impl From<anyhow::Error> for Early {
    fn from(_: anyhow::Error) -> Self {
        StatusCode::INTERNAL_SERVER_ERROR.into_response().into()
    }
}

impl From<std::io::Error> for Early {
    fn from(_: std::io::Error) -> Self {
        StatusCode::INTERNAL_SERVER_ERROR.into_response().into()
    }
}

/// A query's values by name, decoded.
#[derive(Default)]
pub(crate) struct Asks(pub(crate) HashMap<String, String>);

impl Asks {
    pub(crate) fn get(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str)
    }
}

/// What the handlers need of a request, apart from its body.
pub(crate) struct Request {
    path: String,
    query: Asks,
    gzip: bool,
    /// An `Authorization: Bearer` token.
    bearer: Option<String>,
    /// The [`GUEST_COOKIE`].
    guest: Option<String>,
    /// The [`USER_COOKIE`].
    user: Option<String>,
    /// The cookie of a Discord sign-in begun ([`voice_link::STATE_COOKIE`]).
    voice_link: Option<String>,
    /// `User-Agent`.
    agent: String,
}

/// `pairs` as a query string.
fn query_of(pairs: &[(&str, &str)]) -> String {
    serde_urlencoded::to_string(pairs).unwrap_or_default()
}

/// A response of `body` with these headers.
fn respond_with(status: StatusCode, headers: &[(&str, &str)], body: &[u8]) -> Response {
    let mut response = Response::new(Body::from(body.to_vec()));
    *response.status_mut() = status;
    for (name, value) in headers {
        if let (Ok(name), Ok(value)) = (HeaderName::try_from(*name), HeaderValue::try_from(*value)) {
            response.headers_mut().append(name, value);
        }
    }
    response
}

fn respond(status: StatusCode, ctype: &str, body: &[u8]) -> Response {
    // Pages name the build's files: no-cache, so a reload gets a new build.
    if ctype == HTML {
        return respond_with(status, &[("Content-Type", ctype), ("Cache-Control", "no-cache")], body);
    }
    respond_with(status, &[("Content-Type", ctype)], body)
}

/// A file that never changes for a given build, so the browser keeps it.
fn respond_cached(ctype: &str, body: &[u8]) -> Response {
    let max_age = format!("max-age={}", CACHE_FOR.as_secs());
    respond_with(StatusCode::OK, &[("Content-Type", ctype), ("Cache-Control", &max_age)], body)
}

/// `status`, with why as text.
fn refuse(status: StatusCode, why: &anyhow::Error) -> Response {
    respond(status, TEXT, format!("{why:#}").as_bytes())
}

fn not_found() -> Response {
    respond(StatusCode::NOT_FOUND, TEXT, b"not found")
}

pub(crate) fn redirect(status: StatusCode, location: &str) -> Response {
    respond_with(status, &[("Location", location)], b"")
}

/// Back to the page `?return=` names, setting `cookie`.
fn back_with(app: &App, request: &Request, cookie: &str) -> Response {
    respond_with(StatusCode::SEE_OTHER, &[("Location", &return_to(app, request)), ("Set-Cookie", cookie)], b"")
}

/// The marks from `?from=` on, and how many there are in all.
fn respond_marks(query: &Asks, marks: impl FnOnce(usize) -> (Vec<Mark>, usize)) -> Response {
    let from = query.get("from").and_then(|f| f.parse().ok()).unwrap_or(0);
    let (marks, all) = marks(from);
    respond(StatusCode::OK, JSON, serde_json::json!({ "marks": marks, "all": all }).to_string().as_bytes())
}

/// `204` when `done` is, `400` with why not.
fn done(done: Result<()>) -> Response {
    match done {
        Ok(()) => respond(StatusCode::NO_CONTENT, TEXT, b""),
        Err(e) => refuse(StatusCode::BAD_REQUEST, &e),
    }
}

/// `POST /watch`: the front page's form.
fn post_watch(app: &App, sessions: &Sessions, request: &Request, client: IpAddr, line: &str) -> Response {
    let user = visitor::who(app, request).user;
    match watch(app, sessions, user.as_ref(), line, client) {
        Ok(m) => app.redirect(StatusCode::SEE_OTHER, &format!("/w/{}/", m.id)),
        Err(e) => {
            let status = if app.keys.is_some() && user.is_none() { StatusCode::FORBIDDEN } else { StatusCode::BAD_REQUEST };
            respond(status, HTML, &app.page(&landing(app, sessions, request, client, &format!("{e:#}"))))
        }
    }
}

/// The match `line` asks for: a connect string, or a demos.tf link. Behind a site, signed in.
fn watch(app: &App, sessions: &Sessions, user: Option<&token::User>, line: &str, client: IpAddr) -> Result<Arc<sessions::Session>> {
    if app.keys.is_some() && user.is_none() {
        anyhow::bail!("Sign in first");
    }
    let by = counted(user, None, client);
    let owner = user.map(|u| u.uid.as_str());
    match demostf_link(line) {
        Some(demo) => sessions.open_demo_as(by.as_deref(), demo, owner),
        None => sessions.open_as(by.as_deref(), line, owner, None),
    }
}

/// The demo of a demos.tf link.
fn demostf_link(line: &str) -> Option<u64> {
    let rest = line.trim().trim_start_matches("https://").trim_start_matches("www.").strip_prefix("demos.tf/")?;
    rest.trim_end_matches('/').parse().ok().filter(|d| !sessions::is_own_demo(*d))
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The front page, with `error` if any.
fn landing(app: &App, sessions: &Sessions, request: &Request, client: IpAddr, error: &str) -> String {
    let signed = app.keys.is_none() || visitor::who(app, request).user.is_some();
    LANDING
        .replace("{mine}", &mine::mine(app, sessions, request, client))
        .replace("{signed}", if signed { "1" } else { "" })
        .replace("{max}", &(upload::MAX_DEMO >> 20).to_string())
        .replace("{base}", &app.base)
        .replace("{error}", &escape(error))
}

/// `text` for HTML.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests;
