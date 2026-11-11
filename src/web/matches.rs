//! A match's page, timeline and private playbacks, and starting one from a site's link.

use super::client::counted;
use super::events::{stream, too_many_streams, wants_detail, wants_voice};
use super::rooms::{open_room, opened};
use super::token::Source;
use super::visitor::{Who, who};
use super::{
    HTML, INDEX, JSON, MAX_DELAY_S, Request, TEXT, json_body, landing, not_found, param, redirect, refuse, respond, respond_marks,
    respond_with, token, unix_now,
};
use crate::matches::sessions;
use crate::matches::sessions::{Session, Sessions, Viewer};
use crate::{App, lock};
use anyhow::{Result, anyhow};
use std::io::{BufReader, Take};
use std::net::{IpAddr, TcpStream};
use std::sync::Arc;

/// A match's page and event stream: `path` is what follows `/w/`.
pub(super) fn serve_match(
    mut s: TcpStream,
    reader: &mut BufReader<Take<TcpStream>>,
    app: &App,
    sessions: &Sessions,
    request: &Request,
    path: &str,
    client: IpAddr,
) -> Result<()> {
    let (id, rest) = path.split_once('/').unwrap_or((path, ""));
    match (sessions.get(id), rest) {
        // A demo not playing (now): the site plays it again.
        (None, rest)
            if is_page(rest)
                && let (Some(demo), Some(home)) = (sessions::id_demo(id), &app.home) =>
        {
            let at = moment(&request.query, "t").map(|t| format!("&at={t}")).unwrap_or_default();
            let to = format!("{home}?connect=https%3A%2F%2Fdemos.tf%2F{demo}{at}");
            redirect(&mut s, "303 See Other", &to)?;
        }
        (None, _) if app.keys.is_some() => {
            respond(&mut s, "404 Not Found", TEXT, b"That match is no longer being watched; open it again from the site.")?;
        }
        (None, _) => {
            let gone = landing("That match is no longer being watched; paste its connect string to watch it again.");
            respond(&mut s, "404 Not Found", HTML, &app.page(&gone))?;
        }
        (Some(_), "") if !path.ends_with('/') => {
            app.redirect(&mut s, "308 Permanent Redirect", &format!("{}/", request.path))?;
        }
        (Some(_), rest) if is_page(rest) => respond(&mut s, "200 OK", HTML, &app.page(INDEX))?,
        (Some(m), "room") => open_room(&mut s, app, sessions, &m, request, client)?,
        (Some(m), part @ ("timeline" | "marks")) => match_timeline(&mut s, &m, request, part)?,
        (Some(m), "rewind") if request.method == "POST" => rewind(&mut s, reader, app, &m, request, client)?,
        (Some(m), "voice-invite") => super::voice_in::invite(&mut s, app, Some(&m), &who(app, request))?,
        (Some(m), "voice") if request.method == "POST" => super::voice_in::voice_in(&mut s, reader, app, &m, request)?,
        (Some(m), "events") => match app.stream_slot(client) {
            Some(_slot) => {
                let _viewer = Viewer::new(m.clone());
                stream(s, &m.hub, request.gzip, wants_detail(&request.query), wants_voice(&request.query))?;
            }
            None => too_many_streams(&mut s)?,
        },
        _ => not_found(&mut s)?,
    }
    Ok(())
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
pub(super) fn rewind(
    s: &mut TcpStream,
    reader: &mut BufReader<Take<TcpStream>>,
    app: &App,
    m: &Arc<Session>,
    request: &Request,
    client: IpAddr,
) -> Result<()> {
    let Some(recording) = m.ready() else { return Ok(respond(s, "409 Conflict", TEXT, b"No timeline yet.")?) };
    let r = match json_body::<Rewind>(reader, request) {
        Ok(r) => r,
        Err(e) => return Ok(refuse(s, "400 Bad Request", &e)?),
    };
    let Who { key: guest, cookie, user } = who(app, request);
    let by = counted(user.as_ref(), None, client);
    match app.rooms.open_private(m, recording, &guest, r.at, r.speed.unwrap_or(1.0), r.paused, by.as_deref()) {
        Ok(room) => opened(s, app, &room.id, cookie.as_deref()),
        Err(e) => Ok(refuse(s, "503 Service Unavailable", &e)?),
    }
}

/// A match page's timeline: where its shared playback is (a live match at live, or at its end),
/// how long it is, its rounds and how many marks; or the marks from `?from=` on. While a demo is
/// read, `202 {}`.
pub(super) fn match_timeline(s: &mut TcpStream, m: &Arc<Session>, request: &Request, part: &str) -> std::io::Result<()> {
    let Some(r) = m.ready() else {
        m.prepare();
        return respond(s, "202 Accepted", JSON, b"{}");
    };
    if part == "marks" {
        return respond_marks(s, &request.query, |from| r.marks(from));
    }
    // A live match's is at its end, once it ended too: only a replay plays from the start.
    let at = if m.watches_server() { r.length() } else { lock(&m.hub).played.min(r.length()) };
    let (length, live, rounds, marks) = (r.length(), r.is_live(), r.rounds(), r.marks(usize::MAX).1);
    let (start, origin) = (r.start(), r.origin());
    let body =
        serde_json::json!({ "at": at, "length": length, "start": start, "origin": origin, "live": live, "rounds": rounds, "marks": marks });
    respond(s, "200 OK", JSON, body.to_string().as_bytes())
}

/// Every match, for the site in front, with a list token.
pub(super) fn list_matches(s: &mut TcpStream, keys: &token::Keys, sessions: &Sessions, request: &Request) -> Result<()> {
    if request.bearer.as_deref().is_none_or(|t| keys.open_list(t, unix_now()).is_err()) {
        respond_with(s, "401 Unauthorized", &[("Content-Type", TEXT), ("WWW-Authenticate", "Bearer")], b"needs a list token")?;
        return Ok(());
    }
    let body = serde_json::to_vec(&serde_json::json!({ "matches": sessions.list() }))?;
    respond_with(s, "200 OK", &[("Content-Type", JSON), ("Cache-Control", "no-store")], &body)?;
    Ok(())
}

/// The match a site's `/start?t=` token asks for.
pub(super) fn start(app: &App, sessions: &Sessions, request: &Request, client: IpAddr) -> Result<Arc<Session>> {
    let keys = app.keys.as_ref().ok_or_else(|| anyhow!("this server starts matches from its own page"))?;
    let token = param(&request.query, "t").unwrap_or_default();
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
pub(super) fn start_view(query: &str) -> String {
    let overlay = param(query, "view") == Some("overlay");
    let delay = param(query, "delay").unwrap_or_default();
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
            param(query, k)
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
pub(super) fn moment<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    let v = param(query, key)?;
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
