//! A room's page, event stream and controls, and opening one.

use super::client::counted;
use super::events::{stream, too_many_streams, wants_detail};
use super::matches::is_page;
use super::visitor::{Who, origin, who};
use super::{HTML, INDEX, JSON, Request, TEXT, done, form_decode, json_body, not_found, param, respond, respond_with};
use crate::App;
use crate::matches::sessions::{Session, Sessions};
use crate::rooms::room::{Control, Draw, Room, ViewPost};
use anyhow::Result;
use std::io::{BufReader, Take};
use std::net::{IpAddr, TcpStream};
use std::sync::Arc;

/// The new room's page, for the browser that opened it.
pub(super) fn opened(s: &mut TcpStream, app: &App, id: &str, cookie: Option<&str>) -> Result<()> {
    let body = serde_json::json!({ "page": format!("{}/r/{id}/", app.base) }).to_string();
    let mut headers = vec![("Content-Type", JSON)];
    if let Some(c) = cookie {
        headers.push(("Set-Cookie", c));
    }
    Ok(respond_with(s, "201 Created", &headers, body.as_bytes())?)
}

/// `GET` says whether a room can be opened on match `m`; `POST` opens one, owned by the browser.
pub(super) fn open_room(
    s: &mut TcpStream,
    app: &App,
    sessions: &Sessions,
    m: &Arc<Session>,
    request: &Request,
    client: IpAddr,
) -> Result<()> {
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
        Err(e) => return Ok(respond(s, "409 Conflict", TEXT, format!("{e:#}").as_bytes())?),
    };
    let room = match app.rooms.open(m, recording, &guest, counted(user.as_ref(), None, client).as_deref()) {
        Ok(r) => r,
        Err(e) => return Ok(respond(s, "409 Conflict", TEXT, format!("{e:#}").as_bytes())?),
    };
    eprintln!("stvtf-web: room {} opened on match {}", &room.id[..8], &m.id[..m.id.len().min(8)]);
    opened(s, app, &room.id, cookie.as_deref())
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

/// A room's page, event stream and controls: `path` is what follows `/r/`.
pub(super) fn serve_room(
    mut s: TcpStream,
    reader: &mut BufReader<Take<TcpStream>>,
    app: &App,
    sessions: &Sessions,
    request: &Request,
    path: &str,
    client: IpAddr,
) -> Result<()> {
    let (id, rest) = path.split_once('/').unwrap_or((path, ""));
    let Some(room) = app.rooms.get(id, sessions) else {
        return Ok(respond(&mut s, "404 Not Found", TEXT, b"That room has closed.")?);
    };
    // Without a cookie (none yet, or refused), a page is someone new each time.
    let who = who(app, request);
    match (rest, request.method.as_str()) {
        ("", _) if !path.ends_with('/') => app.redirect(&mut s, "308 Permanent Redirect", &format!("{}/", request.path))?,
        (rest, "GET") if is_page(rest) => {
            let mut headers = vec![("Content-Type", HTML), ("Cache-Control", "no-cache")];
            if let Some(c) = &who.cookie {
                headers.push(("Set-Cookie", c));
            }
            respond_with(&mut s, "200 OK", &headers, &app.page(INDEX))?;
        }
        ("events", "GET") => match app.stream_slot(client) {
            Some(_slot) => {
                let name = param(&request.query, "name").map(form_decode);
                let listed = param(&request.query, "overlay") != Some("1");
                let site = who.user.as_ref().map(|u| u.name.as_str());
                let _watching = room.join(&who.key, name.as_deref(), site, listed);
                stream(s, &room.hub, request.gzip, wants_detail(&request.query))?;
            }
            None => too_many_streams(&mut s)?,
        },
        ("me", "GET") => me(&mut s, app, &room, &who)?,
        ("marks", "GET") => {
            let from = param(&request.query, "from").and_then(|f| f.parse().ok()).unwrap_or(0);
            let (marks, all) = room.marks(from);
            respond(&mut s, "200 OK", JSON, serde_json::json!({ "marks": marks, "all": all }).to_string().as_bytes())?;
        }
        (part, "POST") => change(&mut s, reader, request, &room, &who, part)?,
        _ => not_found(&mut s)?,
    }
    Ok(())
}

/// Who the visitor is in `room`, and what they may do.
fn me(s: &mut TcpStream, app: &App, room: &Room, who: &Who) -> std::io::Result<()> {
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
fn change(
    s: &mut TcpStream,
    reader: &mut BufReader<Take<TcpStream>>,
    request: &Request,
    room: &Room,
    who: &Who,
    part: &str,
) -> std::io::Result<()> {
    let guest = &who.key;
    match part {
        "claim" if who.user.as_ref().is_some_and(|u| u.streamer) => {
            room.make_admin(guest);
            respond(s, "204 No Content", TEXT, b"")
        }
        "claim" => respond(s, "403 Forbidden", TEXT, b"Only the site's streamers make themselves admin."),
        "name" => done(s, json_body::<Name>(reader, request).map(|n| room.rename(guest, &n.name))),
        "control" | "draw" | "clear" | "admin" | "view" | "ink" if !room.is_admin(guest) => {
            respond(s, "403 Forbidden", TEXT, b"Only the room's admins do that.")
        }
        "view" => done(s, json_body::<ViewPost>(reader, request).and_then(|v| room.set_view(&v, guest))),
        "ink" => done(s, json_body::<Ink>(reader, request).and_then(|i| room.set_ink(guest, &i.ink))),
        "draw" => done(s, json_body::<Draw>(reader, request).and_then(|d| room.draw(&d, guest))),
        "clear" => {
            room.clear(guest);
            respond(s, "204 No Content", TEXT, b"")
        }
        "control" => done(s, json_body::<Control>(reader, request).and_then(|c| room.control(&c))),
        "admin" => done(s, json_body::<Admin>(reader, request).and_then(|a| room.set_admin(guest, a.person, a.admin))),
        _ => not_found(s),
    }
}
