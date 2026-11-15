//! The routes: axum's, each answered by one function here or in its own module.

// Handlers have the shape the routes' adapters take (`server::to` and the like), whether or not
// one can fail or needs to own what was posted.
#![allow(clippy::unnecessary_wraps, clippy::needless_pass_by_value)]

use super::Answered;
use super::MAX_REQUEST;
use super::query_of;
use super::server::{bytes, form, json, json_if_any, to};
use super::visitor::{origin, return_to, set_cookie};
use super::{
    HTML, INDEX, JSON, MUMBLE, MUMBLE_JSON, Request, TEXT, USER_COOKIE, assets, back_with, events, landing, matches, not_found, post_watch,
    redirect, refuse, respond, rooms, start, start_view, token, unix_now, upload, visitor, voice_link, voice_mumble, voice_who_sync,
};
use crate::matches::sessions::Sessions;
use crate::{App, Matches};
use anyhow::{Result, anyhow};
use axum::Router;
use axum::body::Bytes;
use axum::extract::DefaultBodyLimit;
use axum::http::StatusCode;
use axum::routing::{any, get, post, put};
use std::net::IpAddr;
use std::sync::Arc;
use tower_http::limit::RequestBodyLimitLayer;

/// A request, as its route's function gets it: all of it but its body.
pub(super) struct Asked {
    pub(super) app: Arc<App>,
    pub(super) request: Request,
    pub(super) client: IpAddr,
}

impl Asked {
    /// The matches of a server of any number of them.
    pub(super) fn sessions(&self) -> Result<&Sessions> {
        match &self.app.matches {
            Matches::Many(sessions) => Ok(sessions),
            Matches::One(_) => Err(anyhow!("one match here")),
        }
    }
}

type Routes = Router<Arc<App>>;

/// The page and its overlay under `at`, each with a delay (path part `next`, or under the
/// overlay `after`) or without.
fn pages(router: Routes, at: &str, [next, after]: [&str; 2], page: fn(&Asked) -> Answered) -> Routes {
    let shapes = [format!("{at}/"), format!("{at}/{next}"), format!("{at}/overlay"), format!("{at}/overlay/{after}")];
    shapes.iter().fold(router, |r, shape| r.route(shape, get(to(page))))
}

/// The routes of a match's or a room's voice from outside, under `at`.
fn voice(router: Routes, at: &str) -> Routes {
    use super::voice_routes as v;
    router
        .route(&format!("{at}/voice"), post(json(v::voice)))
        .route(&format!("{at}/voice-mumble"), post(json(v::mumble)))
        .route(&format!("{at}/voice-status"), post(json(v::status)))
        .route(&format!("{at}/voice-match"), post(json(v::matched)))
        .route(&format!("{at}/voice-comms"), get(to(v::comms)))
        .route(&format!("{at}/voices"), get(to(v::voices)))
        .route(&format!("{at}/voice-invite"), get(to(v::invite)))
}

/// A match's routes under `at`, its id; `next` and `after` name the parts of the path after it.
fn of_match(router: Routes, at: &str, [next, after]: [&str; 2]) -> Routes {
    voice(pages(router, at, [next, after], matches::page), at)
        .route(&format!("{at}/demos"), get(to(matches::demos)))
        .route(&format!("{at}/demos/{after}"), get(to(matches::demo)))
        .route(&format!("{at}/watch/{after}"), get(to(matches::watch)))
        .route(&format!("{at}/room"), get(to(matches::room)).post(json_if_any(matches::room_opened)))
        .route(&format!("{at}/timeline"), get(to(matches::timeline)))
        .route(&format!("{at}/marks"), get(to(matches::timeline)))
        .route(&format!("{at}/rewind"), post(json(matches::rewound)))
        .route(&format!("{at}/events"), get(to(matches::events)))
}

/// A room's routes: its pages, what it shows, and what is posted to it.
fn of_room(router: Routes) -> Routes {
    let at = "/r/{id}";
    voice(pages(router, at, ["{delay}", "{delay}"], rooms::page), at)
        .route(at, get(to(rooms::page)))
        .route("/r/{id}/events", get(to(rooms::events)))
        .route("/r/{id}/me", get(to(rooms::mine)))
        .route("/r/{id}/demos", get(to(rooms::demos)))
        .route("/r/{id}/marks", get(to(rooms::marks)))
        .route("/r/{id}/claim", post(to(rooms::claim)))
        .route("/r/{id}/clear", post(to(rooms::clear)))
        .route("/r/{id}/name", post(json(rooms::name)))
        .route("/r/{id}/view", post(json(rooms::view)))
        .route("/r/{id}/ink", post(json(rooms::ink)))
        .route("/r/{id}/draw", post(json(rooms::draw)))
        .route("/r/{id}/arc", post(json(rooms::arc)))
        .route("/r/{id}/circle", post(json(rooms::circle)))
        .route("/r/{id}/light", post(json(rooms::light)))
        .route("/r/{id}/note", post(json(rooms::note)))
        .route("/r/{id}/control", post(json(rooms::control)))
        .route("/r/{id}/map", post(json(rooms::map)))
        .route("/r/{id}/admin", post(json(rooms::admin)))
}

/// The routes of the app: of one match at `/`, or of any number of them. Those in `acting` act
/// for whoever is signed in.
pub(super) fn routes(app: &App) -> (Routes, Routes) {
    let all = Router::new().route("/up", get(to(up))).fallback(get(to(file)));
    if matches!(app.matches, Matches::One(_)) {
        let one = pages(all, "", ["{delay}", "{delay}"], one_page).route("/index.html", get(to(one_page)));
        return (one.route("/events", get(to(one_events))), Router::new());
    }
    let acting = Router::new().route("/", get(to(front))).route("/watch", post(form(watch))).route("/mumble", get(to(mumble)));
    // A body is no bigger than the little a form or JSON takes; an upload's part may be.
    let little = RequestBodyLimitLayer::new(MAX_REQUEST as usize);
    let big = (DefaultBodyLimit::max(upload::MAX_PART), RequestBodyLimitLayer::new(upload::MAX_PART));
    let part = Router::new().route("/upload/{id}", put(bytes(upload::part)).layer(big));
    let all = all
        .route("/matches", get(to(list)))
        .route("/mumble.json", get(to(mumble_json)))
        .route("/upload", get(to(upload::form)).post(json(upload::begun)))
        .route("/upload/{id}", post(json_if_any(upload::finished)))
        .route("/signin", get(to(signin)))
        .route("/signed_in", get(to(signed_in)))
        .route("/signout", get(to(signout)))
        .route("/voice/link", get(to(|a| Ok(voice_link::link(&a.app)))))
        .route("/voice/linked", get(to(|a| Ok(voice_link::linked(&a.app, &a.request)))))
        .route("/voice/who", get(to(|a| Ok(voice_link::who(&a.app, &a.request)))))
        .route("/voice/mumble", get(to(|a| Ok(voice_mumble::get_mumble_jobs(&a.app, a.sessions()?, &a.request)))))
        .route(
            "/voice/known",
            get(to(|a| Ok(voice_who_sync::get_known(&a.app, &a.request))))
                .post(json(|a, said| Ok(voice_who_sync::post_known(&a.app, &a.request, &said)))),
        )
        .route("/start", any(to(started)))
        .route("/fault", post(bytes(fault)));
    // A match's id is one part of the path, or of a demo of ours two (`demo/1a2b…`): the routes
    // are for both, and each handler reads the id off the path (`sessions::split_id`).
    let all = of_match(of_room(all).route("/w/{a}", get(to(matches::page))), "/w/{a}", ["{b}", "{c}"]);
    (of_match(all, "/w/{a}/{b}", ["{c}", "{d}"]).layer(little).merge(part), acting.layer(little))
}

fn up(_: &Asked) -> Answered {
    Ok(respond(StatusCode::OK, TEXT, b"ok"))
}

fn file(a: &Asked) -> Answered {
    Ok(assets::serve_file(&a.app, &a.request.path, a.request.gzip))
}

/// The one match's page, at `/`.
fn one_page(a: &Asked) -> Answered {
    let path = &a.request.path;
    if path == "/index.html" || path.strip_prefix('/').is_some_and(matches::is_page) {
        return Ok(respond(StatusCode::OK, HTML, &a.app.page(INDEX)));
    }
    file(a)
}

fn one_events(a: &Asked) -> Answered {
    let Matches::One(hub) = &a.app.matches else { return Ok(not_found()) };
    match a.app.stream_slot(a.client) {
        Some(slot) => {
            Ok(events::stream(hub, a.request.gzip, events::wants_detail(&a.request.query), events::wants_voice(&a.request.query), slot))
        }
        None => Ok(events::too_many_streams()),
    }
}

fn front(a: &Asked) -> Answered {
    Ok(respond(StatusCode::OK, HTML, &a.app.page(&landing(&a.app, a.sessions()?, &a.request, a.client, ""))))
}

/// The front page's form.
#[derive(serde::Deserialize)]
struct Watch {
    #[serde(default)]
    connect: String,
}

fn watch(a: &Asked, form: Watch) -> Answered {
    Ok(post_watch(&a.app, a.sessions()?, &a.request, a.client, &form.connect))
}

fn mumble(a: &Asked) -> Answered {
    Ok(respond(StatusCode::OK, HTML, &a.app.page(MUMBLE)))
}

fn mumble_json(_: &Asked) -> Answered {
    Ok(respond(StatusCode::OK, JSON, MUMBLE_JSON.as_bytes()))
}

/// `/matches`: every match, for the site in front.
fn list(a: &Asked) -> Answered {
    let Some(keys) = &a.app.keys else { return Ok(not_found()) };
    matches::list_matches(keys, a.sessions()?, &a.request)
}

fn signin(a: &Asked) -> Answered {
    let Some(site) = a.app.home.as_deref().and_then(origin) else { return Ok(not_found()) };
    let back = return_to(&a.app, &a.request);
    Ok(redirect(StatusCode::FOUND, &format!("{site}/stv/sign_in?{}", query_of(&[("return", &back)]))))
}

fn signed_in(a: &Asked) -> Answered {
    let Some(keys) = &a.app.keys else { return Ok(not_found()) };
    Ok(match keys.open_signin(a.request.query.get("t").unwrap_or_default(), unix_now()) {
        Ok(user) => {
            let kept = set_cookie(&a.app, USER_COOKIE, &keys.keep(&user, unix_now()), token::SIGNED_IN_FOR);
            back_with(&a.app, &a.request, &kept)
        }
        Err(e) => refuse(StatusCode::BAD_REQUEST, &e),
    })
}

fn signout(a: &Asked) -> Answered {
    Ok(back_with(&a.app, &a.request, &set_cookie(&a.app, USER_COOKIE, "", 0)))
}

/// `/start`: the match a site's token asks for.
fn started(a: &Asked) -> Answered {
    Ok(match start(&a.app, a.sessions()?, &a.request, a.client) {
        // <stv-map> asks where the match is instead of following the redirect.
        Ok(m) if a.request.query.get("format") == Some("json") => {
            let page = format!("{}/w/{}/", a.app.base, m.id);
            respond(StatusCode::OK, JSON, serde_json::json!({ "page": page }).to_string().as_bytes())
        }
        Ok(m) => a.app.redirect(StatusCode::SEE_OTHER, &format!("/w/{}/{}", m.id, start_view(&a.request.query))),
        Err(e) => refuse(StatusCode::BAD_REQUEST, &e),
    })
}

/// `/fault`: what a page says went wrong in it (see `report`): taken or not, it is not told.
fn fault(a: &Asked, said: Bytes) -> Answered {
    let who = visitor::who(&a.app, &a.request).user;
    crate::report::page_fault(&said, &a.request.agent, who.as_ref().map(|u| u.uid.as_str()));
    Ok(respond(StatusCode::NO_CONTENT, TEXT, b""))
}
