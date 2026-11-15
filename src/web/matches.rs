//! A match's page, timeline and private playbacks, and starting one from a site's link.

// Handlers have the shape the routes' adapters take (`server::to` and the like), whether or not
// one can fail or needs to own what was posted.
#![allow(clippy::unnecessary_wraps, clippy::needless_pass_by_value)]

use super::Asks;
use super::client::counted;
use super::events::{stream, too_many_streams, wants_detail, wants_voice};
use super::query_of;
use super::rooms::{Open, Where, open_room, opened};
use super::routes::Asked;
use super::token::Source;
use super::visitor::{Who, who};
use super::{Answered, Early};
use super::{
    HTML, INDEX, JSON, MAX_DELAY_S, Request, TEXT, landing, not_found, redirect, refuse, respond, respond_marks, respond_with, token,
    unix_now,
};
use crate::matches::sessions;
use crate::matches::sessions::{Session, Sessions, Viewer};
use crate::rooms::room::StartAt;
use crate::voice::who::WhoIsWho;
use crate::{App, is_name, lock};
use anyhow::{Result, anyhow};
use axum::http::StatusCode;
use axum::response::Response;
use cookie::{Cookie, SameSite};
use std::fmt::Write as _;
use std::net::IpAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const GONE: &str = "That match is no longer being watched; paste its connect string to watch it again.";

/// The match a request under `/w/` is for, and what follows its id.
fn asked(a: &Asked) -> (String, String) {
    let (id, rest) = sessions::split_id(a.request.path.strip_prefix("/w/").unwrap_or_default());
    (id.to_string(), rest.to_string())
}

/// The match asked for, a demo kept played again. Answered early otherwise: gone, or its page
/// elsewhere.
pub(super) fn found(a: &Asked) -> Result<Arc<Session>, Early> {
    let (app, sessions, request, client) = (&a.app, a.sessions()?, &a.request, a.client);
    let (id, rest) = asked(a);
    let (id, rest) = (id.as_str(), rest.as_str());
    let session = sessions.get(id).or_else(|| kept_demo(app, sessions, request, id, rest, client));
    if let Some(to) = moved_on(app, sessions, request, id, rest, session.as_ref()) {
        return Err(redirect(StatusCode::SEE_OTHER, &to).into());
    }
    // A demo with comms of which more is known since is another demo: its page is elsewhere.
    if let Some(m) = session.as_ref().filter(|m| m.id != id) {
        let at = moment(&request.query, "t").map_or_default(|t| format!("?t={t}"));
        return Err(redirect(StatusCode::SEE_OTHER, &format!("{}/w/{}/{at}", app.base, m.id)).into());
    }
    if let Some(m) = session {
        return Ok(m);
    }
    // Looked for only when a page of a match that is gone is asked for: it reads the store.
    let kept = if is_page(rest) { comms_demos(app, sessions, id) } else { Vec::new() };
    let ended_demo = is_page(rest) && app.home.is_some() && sessions::id_demo(id).is_some_and(|d| !sessions::is_own_demo(d));
    let gone = if !kept.is_empty() {
        // Gone, but with comms kept: what there is to download.
        respond(StatusCode::NOT_FOUND, HTML, gone_with_demos(&kept).as_bytes())
    } else if ended_demo && who(app, request).user.is_none() {
        // An ended demos.tf demo, nobody signed in: sign in, then back.
        let at = moment(&request.query, "t").map_or_default(|t| format!("?t={t}"));
        let back = format!("{}{}{at}", app.base, request.path);
        app.redirect(StatusCode::SEE_OTHER, &format!("/signin?{}", query_of(&[("return", &back)])))
    } else {
        respond(StatusCode::NOT_FOUND, HTML, &app.page(&landing(app, sessions, request, client, GONE)))
    };
    Err(gone.into())
}

/// `/w/<id>/`, with a delay or as an overlay: the page.
pub(super) fn page(a: &Asked) -> Answered {
    let (_, rest) = asked(a);
    let _playing = found(a)?;
    Ok(if rest.is_empty() && !a.request.path.ends_with('/') {
        a.app.redirect(StatusCode::PERMANENT_REDIRECT, &format!("{}/", a.request.path))
    } else if is_page(&rest) {
        respond(StatusCode::OK, HTML, &a.app.page(INDEX))
    } else {
        not_found()
    })
}

/// `/w/<id>/demos`: its demos with its comms in them, kept after it too.
pub(super) fn demos(a: &Asked) -> Answered {
    let listed = serde_json::json!(comms_demos(&a.app, a.sessions()?, &asked(a).0));
    Ok(respond(StatusCode::OK, JSON, listed.to_string().as_bytes()))
}

/// `/w/<id>/demos/<file>`: one of them.
pub(super) fn demo(a: &Asked) -> Answered {
    let (id, rest) = asked(a);
    Ok(comms_demo(&a.app, a.sessions()?, &a.request, &id, rest.strip_prefix("demos/").unwrap_or_default()))
}

/// `/w/<id>/watch/<file>`.
pub(super) fn watch(a: &Asked) -> Answered {
    let (id, rest) = asked(a);
    watch_comms_demo(&a.app, a.sessions()?, &a.request, &id, rest.strip_prefix("watch/").unwrap_or_default(), a.client)
}

/// `GET /w/<id>/room`: whether a room can be opened on it.
pub(super) fn room(a: &Asked) -> Answered {
    let m = found(a)?;
    open_room(&a.app, a.sessions()?, &m, &a.request, a.client, Open::Asked)
}

/// `POST /w/<id>/room`: one is, from where its opener says they were watching.
pub(super) fn room_opened(a: &Asked, from: Option<Where>) -> Answered {
    let m = found(a)?;
    open_room(&a.app, a.sessions()?, &m, &a.request, a.client, Open::From(from))
}

/// `/w/<id>/timeline` and `/marks`.
pub(super) fn timeline(a: &Asked) -> Answered {
    let m = found(a)?;
    Ok(match_timeline(&m, &a.request, &asked(a).1))
}

/// `POST /w/<id>/rewind`.
pub(super) fn rewound(a: &Asked, to: Rewind) -> Answered {
    let m = found(a)?;
    rewind(&a.app, &m, &a.request, a.client, &to)
}

/// `/w/<id>/events`.
pub(super) fn events(a: &Asked) -> Answered {
    let m = found(a)?;
    Ok(match a.app.stream_slot(a.client) {
        Some(slot) => {
            let held = (slot, Viewer::new(m.clone()));
            stream(&m.hub, a.request.gzip, wants_detail(&a.request.query), wants_voice(&a.request.query), held)
        }
        None => too_many_streams(),
    })
}

/// Where `?map=<load>&t=` leads once match `id` is on another map, or gone: that map's comms demo.
fn moved_on(app: &App, sessions: &Sessions, request: &Request, id: &str, rest: &str, session: Option<&Arc<Session>>) -> Option<String> {
    let load: usize = request.query.get("map").filter(|_| is_page(rest))?.parse().ok()?;
    let at = moment(&request.query, "t")?;
    if session.is_some_and(|m| m.demo_id().is_some() || m.ready().is_none_or(|r| r.watchable().load == load)) {
        return None;
    }
    let store = sessions.store.as_ref().filter(|_| sessions::is_id(id))?;
    let demo = crate::matches::live_demos::downloads(&store.dir, id).into_iter().find(|d| d.load() == Some(load))?;
    Some(format!("{}/w/{id}/watch/{}?t={at}", app.base, demo.file))
}

/// Demo `id`, not playing, played again: demos.tf's (behind a site, signed in), or one kept here.
fn kept_demo(app: &App, sessions: &Sessions, request: &Request, id: &str, rest: &str, client: IpAddr) -> Option<Arc<Session>> {
    let demo = sessions::id_demo(id).filter(|_| is_page(rest))?;
    let user = who(app, request).user;
    let by = counted(user.as_ref(), None, client);
    if !sessions::is_own_demo(demo) {
        let may = app.keys.is_none() || user.is_some();
        return may.then(|| sessions.open_demo_as(by.as_deref(), demo, user.as_ref().map(|u| u.uid.as_str())).ok()).flatten();
    }
    match sessions.open_upload_as(by.as_deref(), demo) {
        Some(opened) => opened.ok(),
        None => sessions.open_kept_comms_as(by.as_deref(), demo, app.voice.who.clone()),
    }
}

/// Where a viewer of a match page goes back to, and how.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Rewind {
    at: u64,
    #[serde(default)]
    speed: Option<f64>,
    #[serde(default)]
    paused: bool,
}

/// A viewer of match `m` goes back in time: a private playback, theirs alone, from where they say.
pub(super) fn rewind(app: &App, m: &Arc<Session>, request: &Request, client: IpAddr, r: &Rewind) -> Answered {
    let Some(recording) = m.ready() else { return Ok(respond(StatusCode::CONFLICT, TEXT, b"No timeline yet.")) };
    let Who { key: guest, cookie, user } = who(app, request);
    let by = counted(user.as_ref(), None, client);
    match app.rooms.open_private(m, recording, &guest, StartAt { at: r.at, speed: r.speed.unwrap_or(1.0), paused: r.paused }, by.as_deref())
    {
        Ok(room) => opened(app, &room.id, cookie.as_deref()),
        Err(e) => Ok(refuse(StatusCode::SERVICE_UNAVAILABLE, &e)),
    }
}

/// A match page's timeline, of the map a room opened now is on: where its shared playback is (a
/// live match at live, or at its end), how long it is, its rounds and how many marks; or the
/// marks from `?from=` on. While a demo is read, `202 {}`.
pub(super) fn match_timeline(m: &Arc<Session>, request: &Request, part: &str) -> Response {
    let Some(r) = m.ready() else {
        m.prepare();
        return respond(StatusCode::ACCEPTED, JSON, b"{}");
    };
    if part == "marks" {
        return respond_marks(&request.query, |from| r.marks_in(r.watchable().load, from));
    }
    let part = r.watchable();
    // While the server changes map, the one just played, at its end; the match is live all the
    // same.
    let length = part.last;
    // A live match's is at its end, once it ended too: only a replay plays from the start.
    let at = if m.watches_server() { length } else { lock(&m.hub).played.min(length) };
    let (live, rounds, marks) = (r.is_live(), r.rounds_in(part.load), r.marks(usize::MAX).1);
    let (start, origin) = (part.start, part.origin);
    let mut body =
        serde_json::json!({ "at": at, "length": length, "start": start, "origin": origin, "live": live, "rounds": rounds, "marks": marks });
    // For links to a moment.
    if m.demo_id().is_none() {
        body["load"] = part.load.into();
    }
    respond(StatusCode::OK, JSON, body.to_string().as_bytes())
}

/// Every match, for the site in front, with a list token.
pub(super) fn list_matches(keys: &token::Keys, sessions: &Sessions, request: &Request) -> Answered {
    if request.bearer.as_deref().is_none_or(|t| keys.open_list(t, unix_now()).is_err()) {
        return Ok(respond_with(
            StatusCode::UNAUTHORIZED,
            &[("Content-Type", TEXT), ("WWW-Authenticate", "Bearer")],
            b"needs a list token",
        ));
    }
    let body = serde_json::to_vec(&serde_json::json!({ "matches": sessions.list() })).map_err(anyhow::Error::from)?;
    Ok(respond_with(StatusCode::OK, &[("Content-Type", JSON), ("Cache-Control", "no-store")], &body))
}

/// The match a site's `/start?t=` token asks for.
pub(super) fn start(app: &App, sessions: &Sessions, request: &Request, client: IpAddr) -> Result<Arc<Session>> {
    let keys = app.keys.as_ref().ok_or_else(|| anyhow!("this server starts matches from its own page"))?;
    let token = request.query.get("t").unwrap_or_default();
    let t = keys.open(token, unix_now())?;
    let user = request.user.as_deref().and_then(|c| keys.kept(c, unix_now()));
    let by = counted(user.as_ref(), t.owner.as_deref(), client);
    match t.source {
        Source::Connect(connect) => sessions.open_as(by.as_deref(), &connect, t.owner.as_deref(), t.reservation),
        Source::Demo(demo) => sessions.open_demo_as(by.as_deref(), demo, t.owner.as_deref()),
    }
}

/// Display options a start link can carry over to the page.
pub(super) const VIEW_OPTIONS: [&str; 26] = [
    "names",
    "score",
    "panel",
    "rotate",
    "dot",
    "ui",
    "zoom",
    "side",
    "bg",
    "bgopacity",
    "health",
    "projectiles",
    "beams",
    "buildings",
    "shots",
    "deaths",
    "caps",
    "heals",
    "hits",
    "marks",
    "jumps",
    "pickups",
    "points",
    "flow",
    "loadout",
    "effects",
];

/// Where a start link lands below `/w/<id>/`: `view=overlay`, `delay=<s>` and the display options
/// become `overlay/4.5?names=1`. Anything else, or a delay out of range, is left out.
pub(super) fn start_view(query: &Asks) -> String {
    let overlay = query.get("view") == Some("overlay");
    let delay = query.get("delay").unwrap_or_default();
    let mut path = match (overlay, delay) {
        (true, "") => "overlay".to_string(),
        (true, d) => format!("overlay/{d}"),
        (false, d) => d.to_string(),
    };
    if !is_page(&path) {
        path = if overlay { "overlay".into() } else { String::new() };
    }
    let mut options: Vec<String> = VIEW_OPTIONS
        .iter()
        .filter_map(|k| {
            query
                .get(k)
                .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-'))
                .map(|v| format!("{k}={v}"))
        })
        .collect();
    // A moment to watch from: `t` on the page (`t` here being the token).
    options.extend(moment(query, "at").map(|at| format!("t={at}")));
    if !options.is_empty() {
        path = format!("{path}?{}", options.join("&"));
    }
    path
}

/// Query parameter `key` if it is a moment, as a page's clock writes it: `12:34`, `1:02:03`, `90`.
pub(super) fn moment<'a>(query: &'a Asks, key: &str) -> Option<&'a str> {
    let v = query.get(key)?;
    let parts: Vec<&str> = v.split(':').collect();
    let digits = |p: &str| !p.is_empty() && p.len() <= 6 && p.bytes().all(|b| b.is_ascii_digit());
    (parts.len() <= 3 && parts.iter().all(|p| digits(p))).then_some(v)
}

/// The page or its overlay, either with a delay in seconds after it: `5`, `overlay/4.5`.
pub(super) fn is_page(rest: &str) -> bool {
    let delay =
        |d: &str| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit() || b == b'.') && d.parse::<f64>().is_ok_and(|s| s <= MAX_DELAY_S);
    match rest.split_once('/') {
        None => matches!(rest, "" | "overlay") || delay(rest),
        Some(("overlay", d)) => delay(d),
        _ => false,
    }
}

/// The page of a match that is gone, with its demos to download.
fn gone_with_demos(kept: &[serde_json::Value]) -> String {
    let mut links = String::new();
    for d in kept {
        let (href, map) = (d["href"].as_str().unwrap_or_default(), d["map"].as_str().unwrap_or_default());
        let watch = d["watch"].as_str().unwrap_or_default();
        let _ = write!(links, r#"<li><a href="{watch}">{map}</a> · <a href="{href}" download>{} MB ↓</a></li>"#, d["mb"]);
    }
    format!(
        "<!doctype html><meta charset=utf-8><meta name=viewport content=\"width=device-width\"><title>stv.tf</title>\
         <body style=\"font:16px system-ui;background:#16171a;color:#ddd;max-width:32em;margin:3em auto;padding:0 1em\">\
         <p>Match ended. With comms:</p>\
         <ul>{links}</ul>"
    )
}

/// Match `id`'s demos with comms in them: per demo its map, its size in MB, where it is to
/// download (`href`) and what plays it here (`watch`). None without a store, or for an id that is none.
pub(super) fn comms_demos(app: &App, sessions: &Sessions, id: &str) -> Vec<serde_json::Value> {
    let Some(store) = sessions.store.as_ref().filter(|_| sessions::is_id(id)) else { return Vec::new() };
    crate::matches::live_demos::downloads(&store.dir, id)
        .into_iter()
        .filter(|d| is_name(&d.map, b"_-"))
        .map(|d| {
            let href = format!("{}/w/{id}/demos/{}.dem", app.base, d.file);
            let watch = format!("{}/w/{id}/watch/{}", app.base, d.file);
            serde_json::json!({ "map": d.map, "mb": d.bytes.div_ceil(1 << 20), "href": href, "watch": watch })
        })
        .collect()
}

/// `GET /w/<id>/watch/<file>`: plays that demo with its comms as a match of its own, and sends
/// the visitor to its page.
fn watch_comms_demo(app: &App, sessions: &Sessions, request: &Request, id: &str, file: &str, client: IpAddr) -> Answered {
    let by = counted(who(app, request).user.as_ref(), None, client);
    Ok(match sessions.open_comms_demo_as(by.as_deref(), id, file, app.voice.who.clone()) {
        Ok(m) => {
            // The moment asked for goes along: whoever said whose voice is whose watches on there.
            let at = moment(&request.query, "t").map_or_default(|t| format!("?t={t}"));
            redirect(StatusCode::SEE_OTHER, &format!("{}/w/{}/{at}", app.base, m.id))
        }
        Err(e) => refuse(StatusCode::NOT_FOUND, &e),
    })
}

/// How long a demo built stays at hand for the next to ask for it.
const BUILT_FOR: Duration = Duration::from_secs(60);

/// A demo with its comms written in: what to call it, and the demo.
pub(super) struct CommsDemo {
    pub(super) name: String,
    pub(super) bytes: Vec<u8>,
}

/// The demo last built (by match and file), and when: whoever asks for it again meanwhile gets
/// the same.
struct Built {
    key: String,
    at: Instant,
    demo: Arc<CommsDemo>,
}

static BUILT: Mutex<Option<Built>> = Mutex::new(None);

/// The demo `file` of match `id` with its comms written in: built once for all who ask at a
/// time or soon after. One at a time: a demo is built in memory.
pub(super) fn build_comms_demo(dir: &Path, id: &str, file: &str, who: &WhoIsWho) -> Option<Arc<CommsDemo>> {
    let key = format!("{id}/{file}");
    let mut last = lock(&BUILT);
    if let Some(built) = last.as_ref().filter(|b| b.key == key && b.at.elapsed() < BUILT_FOR) {
        return Some(built.demo.clone());
    }
    *last = None;
    let built = crate::matches::live_demos::build(dir, id, file, &|key| who.account_now(key)).ok()?;
    let (name, bytes) = (built.name, built.demo);
    let demo = Arc::new(CommsDemo { name, bytes });
    let at = Instant::now();
    *last = Some(Built { key, at, demo: demo.clone() });
    // Not kept in memory once nobody asks any more.
    std::thread::spawn(move || {
        std::thread::sleep(BUILT_FOR);
        let mut last = lock(&BUILT);
        if last.as_ref().is_some_and(|b| b.at == at) {
            *last = None;
        }
    });
    Some(demo)
}

/// `GET /w/<id>/demos/<file>.dem`: that demo as TF2 plays it, the comms written into it. Asked
/// with `?built=<mark>`, the answer sets that mark as a cookie: the page that waits for it then
/// knows the demo is built and on its way.
fn comms_demo(app: &App, sessions: &Sessions, request: &Request, id: &str, file: &str) -> Response {
    let built = sessions
        .store
        .as_ref()
        .filter(|_| sessions::is_id(id))
        .zip(file.strip_suffix(".dem"))
        .and_then(|(store, file)| build_comms_demo(&store.dir, id, file, &app.voice.who));
    let Some(built) = built else { return respond(StatusCode::NOT_FOUND, TEXT, b"No such demo.") };
    let attachment = format!(r#"attachment; filename="{}""#, built.name);
    let mut headers = vec![("Content-Type", "application/octet-stream"), ("Content-Disposition", attachment.as_str())];
    let mark = request.query.get("built").filter(|m| m.len() <= 16 && m.bytes().all(|b| b.is_ascii_alphanumeric()));
    let marked = |m| Cookie::build(("built", m)).path("/").max_age(cookie::time::Duration::seconds(60)).same_site(SameSite::Lax);
    let cookie = mark.map(|m| marked(m).build().to_string());
    if let Some(cookie) = &cookie {
        headers.push(("Set-Cookie", cookie));
    }
    respond_with(StatusCode::OK, &headers, &built.bytes)
}
