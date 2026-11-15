//! What handlers take of a request: extractors of our own, and the queries they read.

use super::client::client_of;
use super::visitor::Who;
use super::{Answered, Early, GUEST_COOKIE, Shared, USER_COOKIE, unix_now};
use crate::Matches;
use crate::matches::sessions::{self, Sessions};
use crate::rooms::people::steam_key;
use anyhow::{Result, anyhow};
use axum::extract::{ConnectInfo, FromRequestParts, Query};
use axum::http::header::{self, HeaderName};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum_extra::extract::CookieJar;
use serde::Deserialize;
use std::net::{IpAddr, SocketAddr};

/// A request's visitor and where it is from: what most handlers go by.
pub(super) struct Visit {
    pub(super) app: Shared,
    pub(super) who: Who,
    pub(super) client: IpAddr,
    /// The path asked for.
    pub(super) path: String,
    /// An `Authorization: Bearer` token.
    pub(super) bearer: Option<String>,
    /// Whether the answer may be gzipped.
    pub(super) gzip: bool,
}

impl Visit {
    /// The matches of a server of any number of them.
    pub(super) fn sessions(&self) -> Result<&Sessions> {
        match &self.app.matches {
            Matches::Many(sessions) => Ok(sessions),
            Matches::One(_) => Err(anyhow!("one match here")),
        }
    }
}

fn one(headers: &HeaderMap, name: HeaderName) -> Option<&str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

fn all(headers: &HeaderMap, name: HeaderName) -> String {
    let values: Vec<&str> = headers.get_all(name).iter().filter_map(|v| v.to_str().ok()).collect();
    values.join(", ")
}

/// `X-Forwarded-For`'s addresses, nearest hop last; none if any of it is not an address.
pub(super) fn forwarded(value: &str) -> Vec<IpAddr> {
    value.split(',').filter(|a| !a.trim().is_empty()).map(|a| a.trim().parse()).collect::<Result<_, _>>().unwrap_or_default()
}

impl FromRequestParts<Shared> for Visit {
    type Rejection = StatusCode;

    async fn from_request_parts(parts: &mut Parts, app: &Shared) -> Result<Self, StatusCode> {
        let peer = parts.extensions.get::<ConnectInfo<SocketAddr>>().ok_or(StatusCode::INTERNAL_SERVER_ERROR)?.0.ip();
        let who = Who::from_request_parts(parts, app).await?;
        let accepts = all(&parts.headers, header::ACCEPT_ENCODING);
        Ok(Visit {
            app: app.clone(),
            who,
            client: client_of(&forwarded(&all(&parts.headers, HeaderName::from_static("x-forwarded-for"))), peer),
            path: parts.uri.path().to_string(),
            bearer: one(&parts.headers, header::AUTHORIZATION).and_then(|a| a.trim().strip_prefix("Bearer ")).map(|t| t.trim().to_string()),
            gzip: accepts.split(',').any(|e| e.split(';').next().is_some_and(|e| e.trim().eq_ignore_ascii_case("gzip"))),
        })
    }
}

impl FromRequestParts<Shared> for Who {
    type Rejection = StatusCode;

    /// The user signed in with the site, by their Steam id, or the browser by its guest cookie:
    /// a new one, to set, if it had none.
    async fn from_request_parts(parts: &mut Parts, app: &Shared) -> Result<Self, StatusCode> {
        let Ok(jar) = CookieJar::from_request_parts(parts, app).await;
        let user = app.keys.as_ref().zip(jar.get(USER_COOKIE)).and_then(|(k, c)| k.kept(c.value(), unix_now()));
        if let Some(user) = user {
            return Ok(Who { key: steam_key(&user.uid), cookie: None, user: Some(user) });
        }
        if let Some(g) = jar.get(GUEST_COOKIE).map(cookie::Cookie::value).filter(|g| sessions::is_id(g)) {
            return Ok(Who { key: g.to_string(), cookie: None, user: None });
        }
        Ok(Who::new_guest(app))
    }
}

/// The match a path under `/w/` is of, and what follows its id: an id is one part of the path,
/// or of a demo of ours two (`demo/1a2b…`), which no route's parameters tell apart.
pub(super) struct MatchAt {
    pub(super) id: String,
    pub(super) rest: String,
    pub(super) moment: Moment,
}

/// A moment a page's link asks for: `t` (`12:34`), on the map of load `map`.
#[derive(Deserialize, Default)]
pub(super) struct Moment {
    t: Option<String>,
    map: Option<String>,
}

impl Moment {
    /// `t`, if it is a moment as a page's clock writes it: `12:34`, `1:02:03`, `90`.
    pub(super) fn t(&self) -> Option<&str> {
        self.t.as_deref().filter(|t| is_moment(t))
    }

    pub(super) fn map(&self) -> Option<usize> {
        self.map.as_deref()?.parse().ok()
    }
}

/// Whether `v` is a moment as a page's clock writes it.
pub(super) fn is_moment(v: &str) -> bool {
    let parts: Vec<&str> = v.split(':').collect();
    let digits = |p: &str| !p.is_empty() && p.len() <= 6 && p.bytes().all(|b| b.is_ascii_digit());
    parts.len() <= 3 && parts.iter().all(|p| digits(p))
}

impl FromRequestParts<Shared> for MatchAt {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, app: &Shared) -> Result<Self, Response> {
        let (id, rest) = sessions::split_id(parts.uri.path().strip_prefix("/w/").unwrap_or_default());
        let (id, rest) = (id.to_string(), rest.to_string());
        let moment = Query::<Moment>::from_request_parts(parts, app).await.map_or_else(|_| Moment::default(), |Query(m)| m);
        Ok(MatchAt { id, rest, moment })
    }
}

/// An event stream's query.
#[derive(Deserialize, Default)]
pub(super) struct Stream {
    detail: Option<String>,
    voice: Option<String>,
    /// Who the viewer says they are, in a room.
    pub(super) name: Option<String>,
    overlay: Option<String>,
}

impl Stream {
    pub(super) fn detail(&self) -> bool {
        self.detail.as_deref() == Some("1")
    }

    pub(super) fn voice(&self) -> bool {
        self.voice.as_deref() == Some("1")
    }

    pub(super) fn overlay(&self) -> bool {
        self.overlay.as_deref() == Some("1")
    }
}

/// `?from=`: the marks from that one on.
#[derive(Deserialize, Default)]
pub(super) struct Marks {
    from: Option<String>,
}

impl Marks {
    pub(super) fn from(&self) -> usize {
        self.from.as_deref().and_then(|f| f.parse().ok()).unwrap_or(0)
    }
}

/// `?since=`: what changed since that version.
#[derive(Deserialize, Default)]
pub(super) struct Since {
    since: Option<String>,
}

impl Since {
    pub(super) fn since(&self) -> Option<u64> {
        self.since.as_deref()?.parse().ok()
    }
}

/// `?return=`: a page of ours to go back to.
#[derive(Deserialize, Default)]
pub(super) struct Back {
    #[serde(rename = "return")]
    pub(super) to: Option<String>,
    /// A sign-in's token.
    pub(super) t: Option<String>,
}

/// Work that may block (locks, files, a wait), off the threads that serve everyone: its answer.
pub(super) async fn answer(work: impl FnOnce() -> Answered + Send + 'static) -> Answered {
    match tokio::task::spawn_blocking(work).await {
        Ok(answered) => answered,
        Err(_) => Err(Early::from(StatusCode::INTERNAL_SERVER_ERROR.into_response())),
    }
}
