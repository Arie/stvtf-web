//! A room's page, event stream and controls, and opening one.

use super::Out;
use super::client::counted;
use super::events::{stream, too_many_streams, wants_detail, wants_voice};
use super::matches::is_page;
use super::routes::Asked;
use super::visitor::{Who, origin, who};
use super::{
    HTML, INDEX, JSON, Request, TEXT, done, form_decode, json_body, not_found, param, refuse, respond, respond_marks, respond_with,
};
use crate::App;
use crate::matches::sessions::{Session, Sessions, logged};
use crate::rooms::room::{ArcDraw, CircleDraw, Control, Draw, LightDraw, MapChoice, NoteDraw, Room, StartAt, ViewPost};
use anyhow::Result;
use std::net::IpAddr;
use std::sync::Arc;

/// The new room's page, for the browser that opened it.
pub(super) fn opened(s: &mut Out, app: &App, id: &str, cookie: Option<&str>) -> Result<()> {
    let body = serde_json::json!({ "page": format!("{}/r/{id}/", app.base) }).to_string();
    let mut headers = vec![("Content-Type", JSON)];
    if let Some(c) = cookie {
        headers.push(("Set-Cookie", c));
    }
    Ok(respond_with(s, "201 Created", &headers, body.as_bytes())?)
}

/// `GET` says whether a room can be opened on match `m`; `POST` opens one, owned by the browser.
pub(super) fn open_room(s: &mut Out, app: &App, sessions: &Sessions, m: &Arc<Session>, request: &Request, client: IpAddr) -> Result<()> {
    if !m.recordable() && m.gave_up() {
        return Ok(respond(s, "410 Gone", TEXT, b"This match has nothing to play.")?);
    }
    if !m.recordable() {
        return Ok(respond(s, "409 Conflict", TEXT, b"This match has nothing to play (yet).")?);
    }
    if request.method != "POST" {
        // Someone may open one: have it ready.
        m.prepare();
        return Ok(respond(s, "200 OK", JSON, b"{}")?);
    }
    // Where its opener was watching, if they say: it starts there.
    let start = match (request.length > 0).then(|| json_body::<Where>(request)).transpose() {
        Ok(w) => w.and_then(|w| Some(StartAt { at: w.at?, speed: 1.0, paused: w.paused })),
        Err(e) => return Ok(refuse(s, "400 Bad Request", &e)?),
    };
    let Who { key: guest, cookie, user } = who(app, request);
    // One's room on it already: back to that.
    if let Some(room) = app.rooms.owned(m, &guest) {
        return opened(s, app, &room.id, cookie.as_deref());
    }
    if sessions.running() + app.rooms.shared_count() >= sessions.max {
        return Ok(respond(s, "503 Service Unavailable", TEXT, b"This server is watching all it can.")?);
    }
    let recording = match m.recording() {
        Ok(r) => r,
        Err(e) => return Ok(refuse(s, "409 Conflict", &e)?),
    };
    let room = match app.rooms.open(m, recording, &guest, counted(user.as_ref(), None, client).as_deref(), start) {
        Ok(r) => r,
        Err(e) => return Ok(refuse(s, "409 Conflict", &e)?),
    };
    log!("room {} opened on match {}", logged(&room.id), logged(&m.id));
    opened(s, app, &room.id, cookie.as_deref())
}

/// Where the one opening a room was watching the match.
#[derive(serde::Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
struct Where {
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

/// The room a request under `/r/` is for, and what follows its id. None once answered that it
/// has closed.
fn found(s: &mut Out, a: &Asked) -> Result<Option<(Arc<Room>, String)>> {
    let path = a.request.path.strip_prefix("/r/").unwrap_or_default();
    let (id, rest) = path.split_once('/').unwrap_or((path, ""));
    let Some(room) = a.app.rooms.get(id, a.sessions()?) else {
        respond(s, "404 Not Found", TEXT, b"That room has closed.")?;
        return Ok(None);
    };
    Ok(Some((room, rest.to_string())))
}

/// `/r/<id>/`, with a delay or as an overlay: the page. Without a cookie (none yet, or refused),
/// a page is someone new each time.
pub(super) fn page(s: &mut Out, a: &Asked) -> Result<()> {
    let Some((_room, rest)) = found(s, a)? else { return Ok(()) };
    if rest.is_empty() && !a.request.path.ends_with('/') {
        a.app.redirect(s, "308 Permanent Redirect", &format!("{}/", a.request.path))?;
    } else if is_page(&rest) {
        let who = who(a.app, a.request);
        let mut headers = vec![("Content-Type", HTML), ("Cache-Control", "no-cache")];
        if let Some(c) = &who.cookie {
            headers.push(("Set-Cookie", c));
        }
        respond_with(s, "200 OK", &headers, &a.app.page(INDEX))?;
    } else {
        not_found(s)?;
    }
    Ok(())
}

/// `/r/<id>/voice…`: its match's voice from outside, as on the match's own page.
pub(super) fn voice(s: &mut Out, a: &Asked) -> Result<()> {
    let Some((room, rest)) = found(s, a)? else { return Ok(()) };
    let on = live(a.app, a.sessions()?, &room);
    match super::voice_routes::voice_route(s, a.app, on.as_ref(), a.request, &rest, &who(a.app, a.request)) {
        Some(answered) => answered?,
        None => not_found(s)?,
    }
    Ok(())
}

/// `/r/<id>/events`.
pub(super) fn events(s: &mut Out, a: &Asked) -> Result<()> {
    let Some((room, _)) = found(s, a)? else { return Ok(()) };
    let (request, who) = (a.request, who(a.app, a.request));
    match a.app.stream_slot(a.client) {
        Some(slot) => {
            let name = param(&request.query, "name").map(form_decode);
            let listed = param(&request.query, "overlay") != Some("1");
            let site = who.user.as_ref().map(|u| u.name.as_str());
            let watching = room.join(&who.key, name.as_deref(), site, listed);
            // By Steam id when signed in: whose room it was, when something is asked of it later.
            log!("room {} joined by {}", logged(&room.id), who.user.as_ref().map_or("a guest", |u| u.uid.as_str()));
            stream(s, &room.hub, request.gzip, wants_detail(&request.query), wants_voice(&request.query), (slot, watching))?;
        }
        None => too_many_streams(s)?,
    }
    Ok(())
}

/// `/r/<id>/me`.
pub(super) fn mine(s: &mut Out, a: &Asked) -> Result<()> {
    let Some((room, _)) = found(s, a)? else { return Ok(()) };
    Ok(me(s, a.app, &room, &who(a.app, a.request))?)
}

/// `/r/<id>/demos`: its match's demos with comms in them.
pub(super) fn demos(s: &mut Out, a: &Asked) -> Result<()> {
    let Some((room, _)) = found(s, a)? else { return Ok(()) };
    let sessions = a.sessions()?;
    let demos = a.app.rooms.live_match(&room.id, sessions).map_or_default(|m| super::matches::comms_demos(a.app, sessions, &m.id));
    Ok(respond(s, "200 OK", JSON, serde_json::json!(demos).to_string().as_bytes())?)
}

/// `/r/<id>/marks`.
pub(super) fn marks(s: &mut Out, a: &Asked) -> Result<()> {
    let Some((room, _)) = found(s, a)? else { return Ok(()) };
    Ok(respond_marks(s, &a.request.query, |from| room.marks(from))?)
}

/// `POST /r/<id>/<part>`: a change to the room.
pub(super) fn changed(s: &mut Out, a: &Asked) -> Result<()> {
    let Some((room, part)) = found(s, a)? else { return Ok(()) };
    Ok(change(s, a.request, &room, &who(a.app, a.request), &part)?)
}

/// The live match `room` is on, if it is on one.
fn live(app: &App, sessions: &Sessions, room: &Room) -> Option<Arc<Session>> {
    app.rooms.live_match(&room.id, sessions)
}

/// Who the visitor is in `room`, and what they may do.
fn me(s: &mut Out, app: &App, room: &Room, who: &Who) -> std::io::Result<()> {
    let (id, admin) = room.me(&who.key);
    let me = serde_json::json!({
        "id": id,
        "admin": admin,
        "signed": who.user.is_some(),
        "streamer": who.user.as_ref().is_some_and(|u| u.streamer),
        "signin": app.keys.is_some() && app.home.as_deref().and_then(origin).is_some(),
    });
    respond(s, "200 OK", JSON, me.to_string().as_bytes())
}

/// A change to `room` (`part`, posted): one's own name and ink by anyone, the rest by its admins.
fn change(s: &mut Out, request: &Request, room: &Room, who: &Who, part: &str) -> std::io::Result<()> {
    let guest = &who.key;
    match part {
        "claim" if who.user.as_ref().is_some_and(|u| u.streamer) => {
            room.make_admin(guest);
            respond(s, "204 No Content", TEXT, b"")
        }
        "claim" => respond(s, "403 Forbidden", TEXT, b"Only the site's streamers make themselves admin."),
        "name" => done(s, json_body::<Name>(request).map(|n| room.rename(guest, &n.name))),
        "control" | "draw" | "arc" | "circle" | "light" | "note" | "clear" | "admin" | "view" | "ink" | "map" if !room.is_admin(guest) => {
            respond(s, "403 Forbidden", TEXT, b"Only the room's admins do that.")
        }
        "view" => done(s, json_body::<ViewPost>(request).and_then(|v| room.set_view(&v, guest))),
        "ink" => done(s, json_body::<Ink>(request).and_then(|i| room.set_ink(guest, &i.ink))),
        "draw" => done(s, json_body::<Draw>(request).and_then(|d| room.draw(&d, guest))),
        "arc" => done(s, json_body::<ArcDraw>(request).and_then(|a| room.arc(&a, guest))),
        "circle" => done(s, json_body::<CircleDraw>(request).and_then(|c| room.circle(&c, guest))),
        "light" => done(s, json_body::<LightDraw>(request).and_then(|l| room.light(&l, guest))),
        "note" => done(s, json_body::<NoteDraw>(request).and_then(|n| room.note(&n, guest))),
        "clear" => {
            room.clear(guest);
            respond(s, "204 No Content", TEXT, b"")
        }
        "control" => done(s, json_body::<Control>(request).and_then(|c| room.control(&c))),
        "map" => done(s, json_body::<MapChoice>(request).and_then(|m| room.move_to_map(&m))),
        "admin" => done(s, json_body::<Admin>(request).and_then(|a| room.set_admin(guest, a.person, a.admin))),
        _ => not_found(s),
    }
}
