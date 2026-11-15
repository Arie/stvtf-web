//! The HTTP side: pages, files, event streams and starting matches.

use crate::App;
use crate::matches::recording::Mark;
use crate::matches::sessions;
use crate::matches::sessions::Sessions;
use anyhow::Result;
use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

mod asked;
mod assets;
mod client;
mod events;
mod matches;
mod mine;
mod pages;
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

pub(crate) use assets::app_version;
use client::counted;
use visitor::set_cookie;

#[cfg(test)]
const HTML: &str = "text/html; charset=utf-8";
#[cfg(test)]
const CSS: &str = "text/css; charset=utf-8";
const JS: &str = "text/javascript; charset=utf-8";
const JSON: &str = "application/json";
const WEBP: &str = "image/webp";
#[cfg(test)]
const SVG: &str = "image/svg+xml";

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

impl IntoResponse for Early {
    fn into_response(self) -> Response {
        *self.0
    }
}

/// The app, as every handler has it.
pub(crate) type Shared = Arc<App>;

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

/// `pairs` as a query string.
fn query_of(pairs: &[(&str, &str)]) -> String {
    serde_urlencoded::to_string(pairs).unwrap_or_default()
}

/// The marks from the one `asked` for on, and how many there are in all.
fn marks_from(asked: &asked::Marks, marks: impl FnOnce(usize) -> (Vec<Mark>, usize)) -> Response {
    let (marks, all) = marks(asked.from());
    Json(serde_json::json!({ "marks": marks, "all": all })).into_response()
}

/// `204` when `done` is, `400` with why not.
fn done(done: Result<()>) -> Response {
    match done {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, format!("{e:#}")).into_response(),
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
fn landing(v: &asked::Visit, sessions: &Sessions, error: &str) -> Response {
    let signed = v.app.keys.is_none() || v.who.user.is_some();
    pages::front(&v.app, signed, error, mine::mine(v, sessions))
}

#[cfg(test)]
mod tests;
