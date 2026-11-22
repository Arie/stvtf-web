//! A room's page, event stream and controls, and opening one.

use std::sync::Arc;

use anyhow::Result;
use axum::Json;
use axum::extract::{Path, Query};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use super::answer::{done, marks_from};
use super::asked::{Marks, Stream, Visit, answer};
use super::client::counted;
use super::events::{stream, too_many_streams};
use super::matches::is_page;
use super::visitor::{Who, origin};
use super::{Answered, Early, pages};
use crate::app::App;
use crate::cameras::Change;
use crate::leagues::{Asked, MAX_PLAYERS};
use crate::matches::sessions::{Session, Sessions, logged};
use crate::rooms::room::{
    ArcDraw, CircleDraw, Control, Draw, LightDraw, MapChoice, Names, NoteDraw, Room, ScoreEdit, SpotPost, StartAt, ViewPost,
};

/// The new room's page, for the browser that opened it.
pub(super) fn opened(app: &App, id: &str, cookie: Option<&str>) -> Response {
    let page = Json(serde_json::json!({ "page": format!("{}/r/{id}/", app.base) }));
    (StatusCode::CREATED, with_cookie(cookie), page).into_response()
}

/// A `Set-Cookie` of `cookie`, if there is one to set.
fn with_cookie(cookie: Option<&str>) -> Option<[(header::HeaderName, String); 1]> {
    cookie.map(|c| [(header::SET_COOKIE, c.to_string())])
}

/// Answers whether a room can be opened on match `m`; with `Open::From`, opens one, owned by the
/// browser.
pub(super) fn open_room(v: &Visit, sessions: &Sessions, m: &Arc<Session>, open: Open) -> Response {
    let app = &v.app;
    if !m.recordable() && m.gave_up() {
        return (StatusCode::GONE, "This match has nothing to play.").into_response();
    }
    if !m.recordable() {
        return (StatusCode::CONFLICT, "This match has nothing to play (yet).").into_response();
    }
    let Open::From(from) = open else {
        // Someone may open one: have it ready.
        m.prepare();
        return Json(serde_json::json!({})).into_response();
    };
    // Where its opener was watching, if they say: it starts there.
    let start = from.and_then(|w| Some(StartAt { at: w.at?, speed: 1.0, paused: w.paused }));
    let Who { key: guest, cookie, user } = v.who.clone();
    // The visitor already has a room on it: back to that.
    if let Some(room) = app.rooms.owned(m, &guest) {
        return opened(app, &room.id, cookie.as_deref());
    }
    if sessions.running() + app.rooms.shared_count() >= sessions.max {
        return (StatusCode::SERVICE_UNAVAILABLE, "This server is watching all it can.").into_response();
    }
    let recording = match m.recording() {
        Ok(r) => r,
        Err(e) => return (StatusCode::CONFLICT, format!("{e:#}")).into_response(),
    };
    let room = match app.rooms.open(m, recording, &guest, counted(user.as_ref(), None, v.client).as_deref(), start) {
        Ok(r) => r,
        Err(e) => return (StatusCode::CONFLICT, format!("{e:#}")).into_response(),
    };
    tracing::info!("room {} opened on match {}", logged(&room.id), logged(&m.id));
    opened(app, &room.id, cookie.as_deref())
}

/// A match's `room`: whether one can open, or opening one at the opener's moment.
pub(super) enum Open {
    Asked,
    From(Option<Where>),
}

/// Where the one opening a room was watching the match.
#[derive(Default, serde::Deserialize)]
#[serde(deny_unknown_fields, default)]
pub(super) struct Where {
    at: Option<u64>,
    paused: bool,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Name {
    name: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Ink {
    ink: String,
}

/// Person `person` made an admin, or not.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Admin {
    person: u32,
    admin: bool,
}

/// The id in a room's path.
#[derive(Deserialize)]
pub(super) struct RoomAt {
    id: String,
}

/// Room `id`, if it is still open; else an early answer that it has closed.
pub(super) fn found(v: &Visit, id: &str) -> Result<Arc<Room>, Early> {
    match v.app.rooms.get(id, v.sessions()?) {
        Some(room) => Ok(room),
        None => Err((StatusCode::NOT_FOUND, "That room has closed.").into_response().into()),
    }
}

/// `/r/<id>/`, also delayed or as overlay: the page. Sets a new guest's cookie, without which the
/// browser would be a new guest each time.
pub(super) async fn show(v: Visit, Path(at): Path<RoomAt>) -> Answered {
    answer(move || {
        found(&v, &at.id)?;
        let rest = v.path.strip_prefix("/r/").and_then(|p| p.split_once('/')).map_or("", |(_, rest)| rest);
        Ok(if rest.is_empty() && !v.path.ends_with('/') {
            Redirect::permanent(&format!("{}{}/", v.app.base, v.path)).into_response()
        } else if is_page(rest) {
            (with_cookie(v.who.cookie.as_deref()), pages::viewer(&v.app)).into_response()
        } else {
            StatusCode::NOT_FOUND.into_response()
        })
    })
    .await
}

/// `/r/<id>/events`.
pub(super) async fn events(v: Visit, Path(at): Path<RoomAt>, Query(q): Query<Stream>) -> Answered {
    answer(move || {
        let (room, who) = (found(&v, &at.id)?, &v.who);
        let Some(slot) = v.app.stream_slot(v.client) else { return Ok(too_many_streams()) };
        let site = who.user.as_ref().map(|u| u.name.as_str());
        let watching = room.join(&who.key, q.name.as_deref(), site, !q.overlay());
        // By Steam id when signed in, to tell later who was in the room.
        tracing::info!("room {} joined by {}", logged(&room.id), who.user.as_ref().map_or("a guest", |u| u.uid.as_str()));
        Ok(stream(&room.hub, v.gzip, &q, (slot, watching)))
    })
    .await
}

/// `/r/<id>/me`.
pub(super) async fn mine(v: Visit, Path(at): Path<RoomAt>) -> Answered {
    answer(move || Ok(me(&v.app, &*found(&v, &at.id)?, &v.who))).await
}

/// `/r/<id>/demos`: its match's demos with comms in them.
pub(super) async fn demos(v: Visit, Path(at): Path<RoomAt>) -> Answered {
    answer(move || {
        let (room, sessions) = (found(&v, &at.id)?, v.sessions()?);
        let demos = v.app.rooms.live_match(&room.id, sessions).map_or_default(|m| super::matches::comms_demos(&v.app, sessions, &m.id));
        Ok(Json(demos).into_response())
    })
    .await
}

/// `/r/<id>/marks`.
pub(super) async fn marks(v: Visit, Path(at): Path<RoomAt>, Query(asked): Query<Marks>) -> Answered {
    answer(move || {
        let room = found(&v, &at.id)?;
        Ok(marks_from(&asked, |from| room.marks(from)))
    })
    .await
}

/// The live match `room` is on, if it is on one.
pub(super) fn live(app: &App, sessions: &Sessions, room: &Room) -> Option<Arc<Session>> {
    app.rooms.live_match(&room.id, sessions)
}

/// Who the visitor is in `room`, and what they may do.
fn me(app: &App, room: &Room, who: &Who) -> Response {
    let (id, admin) = room.me(&who.key);
    let me = serde_json::json!({
        "id": id,
        "admin": admin,
        "signed": who.user.is_some(),
        "streamer": who.user.as_ref().is_some_and(|u| u.streamer),
        "cameras": app.keys.is_none() || who.user.as_ref().is_some_and(|u| u.streamer),
        "signin": app.keys.is_some() && app.home.as_deref().and_then(origin).is_some(),
    });
    Json(me).into_response()
}

/// The room a change is for; refused to non-admins when `admins`.
fn changing(v: &Visit, id: &str, admins: bool) -> Result<Arc<Room>, Early> {
    let room = found(v, id)?;
    if admins && !room.is_admin(&v.who.key) {
        return Err((StatusCode::FORBIDDEN, "Only the room's admins do that.").into_response().into());
    }
    Ok(room)
}

/// `POST /r/<id>/claim`: a streamer makes themselves admin.
pub(super) async fn claim(v: Visit, Path(at): Path<RoomAt>) -> Answered {
    answer(move || {
        let room = changing(&v, &at.id, false)?;
        if !v.who.user.as_ref().is_some_and(|u| u.streamer) {
            return Ok((StatusCode::FORBIDDEN, "Only the site's streamers make themselves admin.").into_response());
        }
        room.make_admin(&v.who.key);
        Ok(StatusCode::NO_CONTENT.into_response())
    })
    .await
}

/// `POST /r/<id>/name`: one's own name, by anyone.
pub(super) async fn name(v: Visit, Path(at): Path<RoomAt>, Json(n): Json<Name>) -> Answered {
    answer(move || {
        changing(&v, &at.id, false)?.rename(&v.who.key, &n.name);
        Ok(StatusCode::NO_CONTENT.into_response())
    })
    .await
}

/// The match's players, as a page sees them.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Players {
    players: Vec<Asked>,
}

/// `POST /r/<id>/leagues`: how the leagues know `players`, for an admin to name them by.
pub(super) async fn leagues(v: Visit, Path(at): Path<RoomAt>, Json(p): Json<Players>) -> Answered {
    answer(move || {
        changing(&v, &at.id, true)?;
        if p.players.len() > MAX_PLAYERS {
            return Ok((StatusCode::BAD_REQUEST, format!("At most {MAX_PLAYERS} players.")).into_response());
        }
        Ok(Json(serde_json::json!({ "leagues": v.app.leagues.look_up(&p.players) })).into_response())
    })
    .await
}

/// `POST /r/<id>/clear`.
pub(super) async fn clear(v: Visit, Path(at): Path<RoomAt>) -> Answered {
    answer(move || {
        changing(&v, &at.id, true)?.clear(&v.who.key);
        Ok(StatusCode::NO_CONTENT.into_response())
    })
    .await
}

/// The rest of what a room's admins post: each a route, and what it does with what was posted.
macro_rules! by_admins {
    ($($(#[$doc:meta])* $route:ident($body:ty) => |$room:ident, $guest:ident, $posted:ident| $does:expr;)*) => {$(
        $(#[$doc])*
        pub(super) async fn $route(v: Visit, Path(at): Path<RoomAt>, Json($posted): Json<$body>) -> Answered {
            answer(move || {
                let ($room, $guest) = (changing(&v, &at.id, true)?, &v.who.key);
                Ok(done($does))
            })
            .await
        }
    )*};
}

by_admins! {
    /// `POST /r/<id>/view`.
    view(ViewPost) => |room, guest, v| room.set_view(&v, guest);
    /// `POST /r/<id>/ink`.
    ink(Ink) => |room, guest, i| room.set_ink(guest, &i.ink);
    /// `POST /r/<id>/draw`.
    draw(Draw) => |room, guest, d| room.draw(&d, guest);
    /// `POST /r/<id>/arc`.
    arc(ArcDraw) => |room, guest, a| room.arc(&a, guest);
    /// `POST /r/<id>/circle`.
    circle(CircleDraw) => |room, guest, c| room.circle(&c, guest);
    /// `POST /r/<id>/light`.
    light(LightDraw) => |room, guest, l| room.light(&l, guest);
    /// `POST /r/<id>/note`.
    note(NoteDraw) => |room, guest, n| room.note(&n, guest);
    /// `POST /r/<id>/names`.
    names(Names) => |room, _guest, n| room.set_names(&n);
    /// `POST /r/<id>/score`.
    score(ScoreEdit) => |room, _guest, s| room.set_score(&s);
    /// `POST /r/<id>/camera`.
    camera(SpotPost) => |room, _guest, s| room.set_spot(&s);
    /// `POST /r/<id>/cameras`.
    cameras(Change) => |room, _guest, c| room.change_cameras(&c);
    /// `POST /r/<id>/control`.
    control(Control) => |room, _guest, c| room.control(&c);
    /// `POST /r/<id>/map`.
    map(MapChoice) => |room, _guest, m| room.move_to_map(&m);
    /// `POST /r/<id>/admin`.
    admin(Admin) => |room, guest, a| room.set_admin(guest, a.person, a.admin);
}
