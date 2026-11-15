//! The routes: axum's, each answered by one function here or in its own module.

use super::server::to;
use super::visitor::{origin, return_to, set_cookie, url_encode};
use super::{
    HTML, INDEX, JSON, MAX_REQUEST, MUMBLE, MUMBLE_JSON, Out, Request, TEXT, USER_COOKIE, assets, back_with, events, landing, matches,
    not_found, param, post_watch, read_body, redirect, refuse, respond, rooms, start, start_view, token, unix_now, upload, visitor,
    voice_link, voice_mumble, voice_who_sync,
};
use crate::matches::sessions::Sessions;
use crate::{App, Matches};
use anyhow::{Result, anyhow};
use axum::Router;
use axum::routing::{any, get, post, put};
use std::net::IpAddr;
use std::sync::Arc;

/// A request, as its route's function gets it.
pub(super) struct Asked<'a> {
    pub(super) app: &'a App,
    pub(super) request: &'a Request,
    pub(super) client: IpAddr,
}

impl Asked<'_> {
    /// The matches of a server of any number of them.
    pub(super) fn sessions(&self) -> Result<&Sessions> {
        match &self.app.matches {
            Matches::Many(sessions) => Ok(sessions),
            Matches::One(_) => Err(anyhow!("one match here")),
        }
    }
}

/// What answers a request.
pub(super) type Answer = fn(&mut Out, &Asked) -> Result<()>;

/// The page and its overlay under `at`, each with a delay (path part `next`, or under the
/// overlay `after`) or without.
fn pages(router: Router<Arc<App>>, at: &str, [next, after]: [&str; 2], page: Answer) -> Router<Arc<App>> {
    let shapes = [format!("{at}/"), format!("{at}/{next}"), format!("{at}/overlay"), format!("{at}/overlay/{after}")];
    shapes.iter().fold(router, |r, shape| r.route(shape, get(to(page))))
}

/// The routes of a match's or a room's voice from outside, under `at`.
fn voice(router: Router<Arc<App>>, at: &str, answer: Answer) -> Router<Arc<App>> {
    let posted = ["voice", "voice-mumble", "voice-status", "voice-match"];
    let got = ["voice-comms", "voices", "voice-invite"];
    let router = posted.iter().fold(router, |r, part| r.route(&format!("{at}/{part}"), post(to(answer))));
    got.iter().fold(router, |r, part| r.route(&format!("{at}/{part}"), get(to(answer))))
}

/// A match's routes under `at`, its id; `next` and `after` name the parts of the path after it.
fn of_match(router: Router<Arc<App>>, at: &str, [next, after]: [&str; 2]) -> Router<Arc<App>> {
    voice(pages(router, at, [next, after], matches::page), at, matches::voice)
        .route(&format!("{at}/demos"), get(to(matches::demos)))
        .route(&format!("{at}/demos/{after}"), get(to(matches::demo)))
        .route(&format!("{at}/watch/{after}"), get(to(matches::watch)))
        .route(&format!("{at}/room"), get(to(matches::room)).post(to(matches::room)))
        .route(&format!("{at}/timeline"), get(to(matches::timeline)))
        .route(&format!("{at}/marks"), get(to(matches::timeline)))
        .route(&format!("{at}/rewind"), post(to(matches::rewound)))
        .route(&format!("{at}/events"), get(to(matches::events)))
}

/// The routes of the app: of one match at `/`, or of any number of them. Those in `acting` act
/// for whoever is signed in.
pub(super) fn routes(app: &App) -> (Router<Arc<App>>, Router<Arc<App>>) {
    let all = Router::new().route("/up", get(to(up))).fallback(get(to(file)));
    if matches!(app.matches, Matches::One(_)) {
        let one = pages(all, "", ["{delay}", "{delay}"], one_page).route("/index.html", get(to(one_page)));
        return (one.route("/events", get(to(one_events))), Router::new());
    }
    let acting = Router::new().route("/", get(to(front))).route("/watch", post(to(watch))).route("/mumble", get(to(mumble)));
    let mut all = all
        .route("/matches", get(to(list)))
        .route("/mumble.json", get(to(mumble_json)))
        .route("/upload", get(to(upload::form)).post(to(upload::begun)))
        .route("/upload/{id}", put(to(upload::part)).post(to(upload::finished)))
        .route("/signin", get(to(signin)))
        .route("/signed_in", get(to(signed_in)))
        .route("/signout", get(to(signout)))
        .route("/voice/link", get(to(|s, a| Ok(voice_link::link(s, a.app)?))))
        .route("/voice/linked", get(to(|s, a| Ok(voice_link::linked(s, a.app, a.request)?))))
        .route("/voice/who", get(to(|s, a| Ok(voice_link::who(s, a.app, a.request)?))))
        .route("/voice/mumble", get(to(|s, a| Ok(voice_mumble::get_mumble_jobs(s, a.app, a.sessions()?, a.request)?))))
        .route(
            "/voice/known",
            get(to(|s, a| Ok(voice_who_sync::get_known(s, a.app, a.request)?)))
                .post(to(|s, a| Ok(voice_who_sync::post_known(s, a.app, a.request)?))),
        )
        .route("/start", any(to(started)))
        .route("/fault", post(to(fault)))
        .route("/r/{id}/{part}", post(to(rooms::changed)))
        .route("/r/{id}/events", get(to(rooms::events)))
        .route("/r/{id}/me", get(to(rooms::mine)))
        .route("/r/{id}/demos", get(to(rooms::demos)))
        .route("/r/{id}/marks", get(to(rooms::marks)));
    all = voice(pages(all, "/r/{id}", ["{part}", "{delay}"], rooms::page), "/r/{id}", rooms::voice).route("/r/{id}", get(to(rooms::page)));
    // A match's id is one part of the path, or of a demo of ours two (`demo/1a2b…`): the routes
    // are for both, and each handler reads the id off the path (`sessions::split_id`).
    all = of_match(all.route("/w/{a}", get(to(matches::page))), "/w/{a}", ["{b}", "{c}"]);
    all = of_match(all, "/w/{a}/{b}", ["{c}", "{d}"]);
    (all, acting)
}

fn up(s: &mut Out, _: &Asked) -> Result<()> {
    Ok(respond(s, "200 OK", TEXT, b"ok")?)
}

fn file(s: &mut Out, a: &Asked) -> Result<()> {
    Ok(assets::serve_file(s, a.app, &a.request.path, a.request.gzip)?)
}

/// The one match's page, at `/`.
fn one_page(s: &mut Out, a: &Asked) -> Result<()> {
    let path = &a.request.path;
    if path == "/index.html" || path.strip_prefix('/').is_some_and(matches::is_page) {
        return Ok(respond(s, "200 OK", HTML, &a.app.page(INDEX))?);
    }
    file(s, a)
}

fn one_events(s: &mut Out, a: &Asked) -> Result<()> {
    let Matches::One(hub) = &a.app.matches else { return Ok(not_found(s)?) };
    match a.app.stream_slot(a.client) {
        Some(slot) => {
            events::stream(s, hub, a.request.gzip, events::wants_detail(&a.request.query), events::wants_voice(&a.request.query), slot)
        }
        None => Ok(events::too_many_streams(s)?),
    }
}

fn front(s: &mut Out, a: &Asked) -> Result<()> {
    Ok(respond(s, "200 OK", HTML, &a.app.page(&landing(a.app, a.sessions()?, a.request, a.client, "")))?)
}

fn watch(s: &mut Out, a: &Asked) -> Result<()> {
    if a.request.length as u64 > MAX_REQUEST {
        return Ok(respond(s, "413 Content Too Large", TEXT, format!("over {MAX_REQUEST} bytes").as_bytes())?);
    }
    post_watch(s, a.app, a.sessions()?, a.request, a.client)
}

fn mumble(s: &mut Out, a: &Asked) -> Result<()> {
    Ok(respond(s, "200 OK", HTML, &a.app.page(MUMBLE))?)
}

fn mumble_json(s: &mut Out, _: &Asked) -> Result<()> {
    Ok(respond(s, "200 OK", JSON, MUMBLE_JSON.as_bytes())?)
}

/// `/matches`: every match, for the site in front.
fn list(s: &mut Out, a: &Asked) -> Result<()> {
    let Some(keys) = &a.app.keys else { return Ok(not_found(s)?) };
    matches::list_matches(s, keys, a.sessions()?, a.request)
}

fn signin(s: &mut Out, a: &Asked) -> Result<()> {
    let Some(site) = a.app.home.as_deref().and_then(origin) else { return Ok(not_found(s)?) };
    Ok(redirect(s, "302 Found", &format!("{site}/stv/sign_in?return={}", url_encode(&return_to(a.app, a.request))))?)
}

fn signed_in(s: &mut Out, a: &Asked) -> Result<()> {
    let Some(keys) = &a.app.keys else { return Ok(not_found(s)?) };
    match keys.open_signin(param(&a.request.query, "t").unwrap_or_default(), unix_now()) {
        Ok(user) => {
            let kept = set_cookie(a.app, USER_COOKIE, &keys.keep(&user, unix_now()), token::SIGNED_IN_FOR);
            back_with(s, a.app, a.request, &kept)?;
        }
        Err(e) => refuse(s, "400 Bad Request", &e)?,
    }
    Ok(())
}

fn signout(s: &mut Out, a: &Asked) -> Result<()> {
    Ok(back_with(s, a.app, a.request, &set_cookie(a.app, USER_COOKIE, "", 0))?)
}

/// `/start`: the match a site's token asks for.
fn started(s: &mut Out, a: &Asked) -> Result<()> {
    match start(a.app, a.sessions()?, a.request, a.client) {
        // <stv-map> asks where the match is instead of following the redirect.
        Ok(m) if param(&a.request.query, "format") == Some("json") => {
            let page = format!("{}/w/{}/", a.app.base, m.id);
            respond(s, "200 OK", JSON, serde_json::json!({ "page": page }).to_string().as_bytes())?;
        }
        Ok(m) => a.app.redirect(s, "303 See Other", &format!("/w/{}/{}", m.id, start_view(&a.request.query)))?,
        Err(e) => refuse(s, "400 Bad Request", &e)?,
    }
    Ok(())
}

/// `/fault`: what a page says went wrong in it (see `report`): taken or not, it is not told.
fn fault(s: &mut Out, a: &Asked) -> Result<()> {
    let who = visitor::who(a.app, a.request).user;
    crate::report::page_fault(&read_body(a.request)?, &a.request.agent, who.as_ref().map(|u| u.uid.as_str()));
    Ok(respond(s, "204 No Content", TEXT, b"")?)
}
