//! A match's page, timeline and private playbacks, and starting one from a site's link.

use super::asked::{Marks, MatchAt, Stream, Visit, answer, is_moment};
use super::client::counted;
use super::events::{stream, too_many_streams};
use super::pages::{self, Kept};
use super::rooms::{Open, Where, open_room, opened};
use super::token::Source;
use super::visitor::Who;
use super::{Answered, Early, MAX_DELAY_S, landing, marks_from, query_of, unix_now};
use crate::matches::sessions;
use crate::matches::sessions::{Session, Sessions, Viewer};
use crate::rooms::room::StartAt;
use crate::voice::who::WhoIsWho;
use crate::{App, is_name, lock};
use anyhow::{Result, anyhow};
use axum::Json;
use axum::extract::Query;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::CookieJar;
use cookie::{Cookie, SameSite};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const GONE: &str = "That match is no longer being watched; paste its connect string to watch it again.";

/// The match asked for, a demo kept played again. Answered early otherwise: gone, or its page
/// elsewhere.
pub(super) fn found(v: &Visit, at: &MatchAt) -> Result<Arc<Session>, Early> {
    let (app, sessions) = (&v.app, v.sessions()?);
    let (id, rest) = (at.id.as_str(), at.rest.as_str());
    let session = sessions.get(id).or_else(|| kept_demo(v, sessions, id, rest));
    if let Some(to) = moved_on(app, sessions, at, session.as_ref()) {
        return Err(Redirect::to(&to).into_response().into());
    }
    let moment = at.moment.t().map_or_default(|t| format!("?t={t}"));
    // A demo with comms of which more is known since is another demo: its page is elsewhere.
    if let Some(m) = session.as_ref().filter(|m| m.id != id) {
        return Err(Redirect::to(&format!("{}/w/{}/{moment}", app.base, m.id)).into_response().into());
    }
    if let Some(m) = session {
        return Ok(m);
    }
    // Looked for only when a page of a match that is gone is asked for: it reads the store.
    let kept = if is_page(rest) { comms_demos(app, sessions, id) } else { Vec::new() };
    let ended_demo = is_page(rest) && app.home.is_some() && sessions::id_demo(id).is_some_and(|d| !sessions::is_own_demo(d));
    let gone = if !kept.is_empty() {
        // Gone, but with comms kept: what there is to download.
        pages::with(StatusCode::NOT_FOUND, pages::gone(app, &kept))
    } else if ended_demo && v.who.user.is_none() {
        // An ended demos.tf demo, nobody signed in: sign in, then back.
        let back = format!("{}{}{moment}", app.base, v.path);
        Redirect::to(&format!("{}/signin?{}", app.base, query_of(&[("return", &back)]))).into_response()
    } else {
        pages::with(StatusCode::NOT_FOUND, landing(v, sessions, GONE))
    };
    Err(gone.into())
}

/// `/w/<id>/`, with a delay or as an overlay: the page.
pub(super) async fn show(v: Visit, at: MatchAt) -> Answered {
    answer(move || {
        found(&v, &at)?;
        Ok(if at.rest.is_empty() && !v.path.ends_with('/') {
            Redirect::permanent(&format!("{}{}/", v.app.base, v.path)).into_response()
        } else if is_page(&at.rest) {
            pages::viewer(&v.app)
        } else {
            StatusCode::NOT_FOUND.into_response()
        })
    })
    .await
}

/// `/w/<id>/demos`: its demos with its comms in them, kept after it too.
pub(super) async fn demos(v: Visit, at: MatchAt) -> Answered {
    answer(move || Ok(Json(comms_demos(&v.app, v.sessions()?, &at.id)).into_response())).await
}

/// `?built=<mark>`: a mark to set as a cookie once a demo is built.
#[derive(Deserialize)]
pub(super) struct BuiltMark {
    built: Option<String>,
}

/// `/w/<id>/demos/<file>`: one of them.
pub(super) async fn demo(v: Visit, at: MatchAt, Query(q): Query<BuiltMark>) -> Answered {
    answer(move || Ok(comms_demo(&v.app, v.sessions()?, q.built.as_deref(), &at.id, at.rest.strip_prefix("demos/").unwrap_or_default())))
        .await
}

/// `/w/<id>/watch/<file>`.
pub(super) async fn watch(v: Visit, at: MatchAt) -> Answered {
    answer(move || Ok(watch_comms_demo(&v, v.sessions()?, &at, at.rest.strip_prefix("watch/").unwrap_or_default()))).await
}

/// `GET /w/<id>/room`: whether a room can be opened on it.
pub(super) async fn room(v: Visit, at: MatchAt) -> Answered {
    answer(move || Ok(open_room(&v, v.sessions()?, &found(&v, &at)?, Open::Asked))).await
}

/// `POST /w/<id>/room`: one is, from where its opener says they were watching.
pub(super) async fn room_opened(v: Visit, at: MatchAt, from: Option<Json<Where>>) -> Answered {
    answer(move || Ok(open_room(&v, v.sessions()?, &found(&v, &at)?, Open::From(from.map(|Json(from)| from))))).await
}

/// `/w/<id>/timeline` and `/marks`.
pub(super) async fn timeline(v: Visit, at: MatchAt, Query(marks): Query<Marks>) -> Answered {
    answer(move || Ok(match_timeline(&found(&v, &at)?, &marks, &at.rest))).await
}

/// `POST /w/<id>/rewind`.
pub(super) async fn rewound(v: Visit, at: MatchAt, Json(to): Json<Rewind>) -> Answered {
    answer(move || Ok(rewind(&v, &found(&v, &at)?, &to))).await
}

/// `/w/<id>/events`.
pub(super) async fn events(v: Visit, at: MatchAt, Query(q): Query<Stream>) -> Answered {
    answer(move || {
        let m = found(&v, &at)?;
        Ok(match v.app.stream_slot(v.client) {
            Some(slot) => stream(&m.hub, v.gzip, &q, (slot, Viewer::new(m.clone()))),
            None => too_many_streams(),
        })
    })
    .await
}

/// Where `?map=<load>&t=` leads once match `id` is on another map, or gone: that map's comms demo.
fn moved_on(app: &App, sessions: &Sessions, at: &MatchAt, session: Option<&Arc<Session>>) -> Option<String> {
    let id = at.id.as_str();
    let load = at.moment.map().filter(|_| is_page(&at.rest))?;
    let at = at.moment.t()?;
    if session.is_some_and(|m| m.demo_id().is_some() || m.ready().is_none_or(|r| r.watchable().load == load)) {
        return None;
    }
    let store = sessions.store.as_ref().filter(|_| sessions::is_id(id))?;
    let demo = crate::matches::live_demos::downloads(&store.dir, id).into_iter().find(|d| d.load() == Some(load))?;
    Some(format!("{}/w/{id}/watch/{}?t={at}", app.base, demo.file))
}

/// Demo `id`, not playing, played again: demos.tf's (behind a site, signed in), or one kept here.
fn kept_demo(v: &Visit, sessions: &Sessions, id: &str, rest: &str) -> Option<Arc<Session>> {
    let demo = sessions::id_demo(id).filter(|_| is_page(rest))?;
    let (app, user) = (&v.app, &v.who.user);
    let by = counted(user.as_ref(), None, v.client);
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
pub(super) fn rewind(v: &Visit, m: &Arc<Session>, r: &Rewind) -> Response {
    let Some(recording) = m.ready() else { return (StatusCode::CONFLICT, "No timeline yet.").into_response() };
    let (app, Who { key: guest, cookie, user }) = (&v.app, v.who.clone());
    let by = counted(user.as_ref(), None, v.client);
    match app.rooms.open_private(m, recording, &guest, StartAt { at: r.at, speed: r.speed.unwrap_or(1.0), paused: r.paused }, by.as_deref())
    {
        Ok(room) => opened(app, &room.id, cookie.as_deref()),
        Err(e) => (StatusCode::SERVICE_UNAVAILABLE, format!("{e:#}")).into_response(),
    }
}

/// A match page's timeline, of the map a room opened now is on: where its shared playback is (a
/// live match at live, or at its end), how long it is, its rounds and how many marks; or the
/// marks from `?from=` on. While a demo is read, `202 {}`.
pub(super) fn match_timeline(m: &Arc<Session>, asked: &Marks, part: &str) -> Response {
    let Some(r) = m.ready() else {
        m.prepare();
        return (StatusCode::ACCEPTED, Json(serde_json::json!({}))).into_response();
    };
    if part == "marks" {
        return marks_from(asked, |from| r.marks_in(r.watchable().load, from));
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
    Json(body).into_response()
}

/// `/matches`: every match, for the site in front, with a list token.
pub(super) async fn list(v: Visit) -> Answered {
    answer(move || {
        let Some(keys) = &v.app.keys else { return Ok(StatusCode::NOT_FOUND.into_response()) };
        if v.bearer.as_deref().is_none_or(|t| keys.open_list(t, unix_now()).is_err()) {
            return Ok((StatusCode::UNAUTHORIZED, [(header::WWW_AUTHENTICATE, "Bearer")], "needs a list token").into_response());
        }
        let listed = serde_json::json!({ "matches": v.sessions()?.list() });
        Ok(([(header::CACHE_CONTROL, "no-store")], Json(listed)).into_response())
    })
    .await
}

/// The match a site's `/start?t=` token asks for.
pub(super) fn start(v: &Visit, sessions: &Sessions, token: &str) -> Result<Arc<Session>> {
    let keys = v.app.keys.as_ref().ok_or_else(|| anyhow!("this server starts matches from its own page"))?;
    let t = keys.open(token, unix_now())?;
    let by = counted(v.who.user.as_ref(), t.owner.as_deref(), v.client);
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
pub(super) fn start_view(query: &HashMap<String, String>) -> String {
    let query = |name: &str| query.get(name).map(String::as_str);
    let overlay = query("view") == Some("overlay");
    let delay = query("delay").unwrap_or_default();
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
            query(k)
                .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-'))
                .map(|v| format!("{k}={v}"))
        })
        .collect();
    // A moment to watch from: `t` on the page (`t` here being the token).
    options.extend(query("at").filter(|at| is_moment(at)).map(|at| format!("t={at}")));
    if !options.is_empty() {
        path = format!("{path}?{}", options.join("&"));
    }
    path
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

/// Match `id`'s demos with comms in them: per demo its map, its size in MB, where it is to
/// download (`href`) and what plays it here (`watch`). None without a store, or for an id that is none.
pub(super) fn comms_demos(app: &App, sessions: &Sessions, id: &str) -> Vec<Kept> {
    let Some(store) = sessions.store.as_ref().filter(|_| sessions::is_id(id)) else { return Vec::new() };
    crate::matches::live_demos::downloads(&store.dir, id)
        .into_iter()
        .filter(|d| is_name(&d.map, b"_-"))
        .map(|d| {
            let href = format!("{}/w/{id}/demos/{}.dem", app.base, d.file);
            let watch = format!("{}/w/{id}/watch/{}", app.base, d.file);
            Kept { map: d.map, mb: d.bytes.div_ceil(1 << 20), href, watch }
        })
        .collect()
}

/// `GET /w/<id>/watch/<file>`: plays that demo with its comms as a match of its own, and sends
/// the visitor to its page.
fn watch_comms_demo(v: &Visit, sessions: &Sessions, at: &MatchAt, file: &str) -> Response {
    let (app, by) = (&v.app, counted(v.who.user.as_ref(), None, v.client));
    match sessions.open_comms_demo_as(by.as_deref(), &at.id, file, app.voice.who.clone()) {
        Ok(m) => {
            // The moment asked for goes along: whoever said whose voice is whose watches on there.
            let moment = at.moment.t().map_or_default(|t| format!("?t={t}"));
            Redirect::to(&format!("{}/w/{}/{moment}", app.base, m.id)).into_response()
        }
        Err(e) => (StatusCode::NOT_FOUND, format!("{e:#}")).into_response(),
    }
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
fn comms_demo(app: &App, sessions: &Sessions, mark: Option<&str>, id: &str, file: &str) -> Response {
    let built = sessions
        .store
        .as_ref()
        .filter(|_| sessions::is_id(id))
        .zip(file.strip_suffix(".dem"))
        .and_then(|(store, file)| build_comms_demo(&store.dir, id, file, &app.voice.who));
    let Some(built) = built else { return (StatusCode::NOT_FOUND, "No such demo.").into_response() };
    let attachment = format!(r#"attachment; filename="{}""#, built.name);
    let headers = [(header::CONTENT_TYPE, "application/octet-stream".to_string()), (header::CONTENT_DISPOSITION, attachment)];
    let mark = mark.filter(|m| m.len() <= 16 && m.bytes().all(|b| b.is_ascii_alphanumeric()));
    let marked =
        |m: &str| Cookie::build(("built", m.to_string())).path("/").max_age(cookie::time::Duration::seconds(60)).same_site(SameSite::Lax);
    let jar = mark.map_or_else(CookieJar::new, |m| CookieJar::new().add(marked(m)));
    (jar, headers, built.bytes.clone()).into_response()
}
