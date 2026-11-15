//! A room's page, event stream and controls, and opening one.

// Handlers have the shape the routes' adapters take (`server::to` and the like), whether or not
// one can fail or needs to own what was posted.
#![allow(clippy::unnecessary_wraps, clippy::needless_pass_by_value)]

use super::client::counted;
use super::events::{stream, too_many_streams, wants_detail, wants_voice};
use super::matches::is_page;
use super::routes::Asked;
use super::visitor::{Who, origin, who};
use super::{Answered, Early};
use super::{HTML, INDEX, JSON, Request, TEXT, done, not_found, refuse, respond, respond_marks, respond_with};
use crate::App;
use crate::matches::sessions::{Session, Sessions, logged};
use crate::rooms::room::{ArcDraw, CircleDraw, Control, Draw, LightDraw, MapChoice, NoteDraw, Room, StartAt, ViewPost};
use anyhow::Result;
use axum::http::StatusCode;
use axum::response::Response;
use std::net::IpAddr;
use std::sync::Arc;

/// The new room's page, for the browser that opened it.
pub(super) fn opened(app: &App, id: &str, cookie: Option<&str>) -> Answered {
    let body = serde_json::json!({ "page": format!("{}/r/{id}/", app.base) }).to_string();
    let mut headers = vec![("Content-Type", JSON)];
    if let Some(c) = cookie {
        headers.push(("Set-Cookie", c));
    }
    Ok(respond_with(StatusCode::CREATED, &headers, body.as_bytes()))
}

/// `GET` says whether a room can be opened on match `m`; `POST` opens one, owned by the browser.
pub(super) fn open_room(app: &App, sessions: &Sessions, m: &Arc<Session>, request: &Request, client: IpAddr, open: Open) -> Answered {
    if !m.recordable() && m.gave_up() {
        return Ok(respond(StatusCode::GONE, TEXT, b"This match has nothing to play."));
    }
    if !m.recordable() {
        return Ok(respond(StatusCode::CONFLICT, TEXT, b"This match has nothing to play (yet)."));
    }
    let Open::From(from) = open else {
        // Someone may open one: have it ready.
        m.prepare();
        return Ok(respond(StatusCode::OK, JSON, b"{}"));
    };
    // Where its opener was watching, if they say: it starts there.
    let start = from.and_then(|w| Some(StartAt { at: w.at?, speed: 1.0, paused: w.paused }));
    let Who { key: guest, cookie, user } = who(app, request);
    // One's room on it already: back to that.
    if let Some(room) = app.rooms.owned(m, &guest) {
        return opened(app, &room.id, cookie.as_deref());
    }
    if sessions.running() + app.rooms.shared_count() >= sessions.max {
        return Ok(respond(StatusCode::SERVICE_UNAVAILABLE, TEXT, b"This server is watching all it can."));
    }
    let recording = match m.recording() {
        Ok(r) => r,
        Err(e) => return Ok(refuse(StatusCode::CONFLICT, &e)),
    };
    let room = match app.rooms.open(m, recording, &guest, counted(user.as_ref(), None, client).as_deref(), start) {
        Ok(r) => r,
        Err(e) => return Ok(refuse(StatusCode::CONFLICT, &e)),
    };
    log!("room {} opened on match {}", logged(&room.id), logged(&m.id));
    opened(app, &room.id, cookie.as_deref())
}

/// What is asked of a match's `room`: whether one can be opened, or one, from where its opener
/// says they were watching.
pub(super) enum Open {
    Asked,
    From(Option<Where>),
}

/// Where the one opening a room was watching the match.
#[derive(serde::Deserialize, Default)]
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

/// The room a request under `/r/` is for, and what follows its id. Answered early that it has
/// closed.
pub(super) fn found(a: &Asked) -> Result<(Arc<Room>, String), Early> {
    let path = a.request.path.strip_prefix("/r/").unwrap_or_default();
    let (id, rest) = path.split_once('/').unwrap_or((path, ""));
    let Some(room) = a.app.rooms.get(id, a.sessions()?) else {
        return Err(respond(StatusCode::NOT_FOUND, TEXT, b"That room has closed.").into());
    };
    Ok((room, rest.to_string()))
}

/// `/r/<id>/`, with a delay or as an overlay: the page. Without a cookie (none yet, or refused),
/// a page is someone new each time.
pub(super) fn page(a: &Asked) -> Answered {
    let (_room, rest) = found(a)?;
    Ok(if rest.is_empty() && !a.request.path.ends_with('/') {
        a.app.redirect(StatusCode::PERMANENT_REDIRECT, &format!("{}/", a.request.path))
    } else if is_page(&rest) {
        let who = who(&a.app, &a.request);
        let mut headers = vec![("Content-Type", HTML), ("Cache-Control", "no-cache")];
        if let Some(c) = &who.cookie {
            headers.push(("Set-Cookie", c));
        }
        respond_with(StatusCode::OK, &headers, &a.app.page(INDEX))
    } else {
        not_found()
    })
}

/// `/r/<id>/events`.
pub(super) fn events(a: &Asked) -> Answered {
    let (room, _) = found(a)?;
    let (request, who) = (&a.request, who(&a.app, &a.request));
    Ok(match a.app.stream_slot(a.client) {
        Some(slot) => {
            let name = request.query.get("name").map(str::to_string);
            let listed = request.query.get("overlay") != Some("1");
            let site = who.user.as_ref().map(|u| u.name.as_str());
            let watching = room.join(&who.key, name.as_deref(), site, listed);
            // By Steam id when signed in: whose room it was, when something is asked of it later.
            log!("room {} joined by {}", logged(&room.id), who.user.as_ref().map_or("a guest", |u| u.uid.as_str()));
            stream(&room.hub, request.gzip, wants_detail(&request.query), wants_voice(&request.query), (slot, watching))
        }
        None => too_many_streams(),
    })
}

/// `/r/<id>/me`.
pub(super) fn mine(a: &Asked) -> Answered {
    let (room, _) = found(a)?;
    Ok(me(&a.app, &room, &who(&a.app, &a.request)))
}

/// `/r/<id>/demos`: its match's demos with comms in them.
pub(super) fn demos(a: &Asked) -> Answered {
    let (room, _) = found(a)?;
    let sessions = a.sessions()?;
    let demos = a.app.rooms.live_match(&room.id, sessions).map_or_default(|m| super::matches::comms_demos(&a.app, sessions, &m.id));
    Ok(respond(StatusCode::OK, JSON, serde_json::json!(demos).to_string().as_bytes()))
}

/// `/r/<id>/marks`.
pub(super) fn marks(a: &Asked) -> Answered {
    let (room, _) = found(a)?;
    Ok(respond_marks(&a.request.query, |from| room.marks(from)))
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
        "signin": app.keys.is_some() && app.home.as_deref().and_then(origin).is_some(),
    });
    respond(StatusCode::OK, JSON, me.to_string().as_bytes())
}

/// The room a change is posted to, and who posts it. Answered early that only its admins do
/// that, when `admins` and they are none.
fn changing(a: &Asked, admins: bool) -> Result<(Arc<Room>, Who), Early> {
    let ((room, _), who) = (found(a)?, who(&a.app, &a.request));
    if admins && !room.is_admin(&who.key) {
        return Err(respond(StatusCode::FORBIDDEN, TEXT, b"Only the room's admins do that.").into());
    }
    Ok((room, who))
}

/// `POST /r/<id>/claim`: a streamer makes themselves admin.
pub(super) fn claim(a: &Asked) -> Answered {
    let (room, who) = changing(a, false)?;
    if !who.user.as_ref().is_some_and(|u| u.streamer) {
        return Ok(respond(StatusCode::FORBIDDEN, TEXT, b"Only the site's streamers make themselves admin."));
    }
    room.make_admin(&who.key);
    Ok(respond(StatusCode::NO_CONTENT, TEXT, b""))
}

/// `POST /r/<id>/name`: one's own name, by anyone.
pub(super) fn name(a: &Asked, n: Name) -> Answered {
    let (room, who) = changing(a, false)?;
    room.rename(&who.key, &n.name);
    Ok(respond(StatusCode::NO_CONTENT, TEXT, b""))
}

/// `POST /r/<id>/clear`.
pub(super) fn clear(a: &Asked) -> Answered {
    let (room, who) = changing(a, true)?;
    room.clear(&who.key);
    Ok(respond(StatusCode::NO_CONTENT, TEXT, b""))
}

/// The rest of what a room's admins post: each a route, and what it does with what was posted.
macro_rules! by_admins {
    ($($(#[$doc:meta])* $route:ident($body:ty) => |$room:ident, $guest:ident, $posted:ident| $does:expr;)*) => {$(
        $(#[$doc])*
        pub(super) fn $route(a: &Asked, $posted: $body) -> Answered {
            let ($room, who) = changing(a, true)?;
            let $guest = &who.key;
            Ok(done($does))
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
    /// `POST /r/<id>/control`.
    control(Control) => |room, _guest, c| room.control(&c);
    /// `POST /r/<id>/map`.
    map(MapChoice) => |room, _guest, m| room.move_to_map(&m);
    /// `POST /r/<id>/admin`.
    admin(Admin) => |room, guest, a| room.set_admin(guest, a.person, a.admin);
}
