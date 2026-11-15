//! The routes, and the handlers of those that belong to no part of the site in particular.

use super::asked::{Back, Stream, Visit, answer};
use super::visitor::{origin, return_to, set_cookie};
use super::{
    Answered, MAX_REQUEST, Shared, USER_COOKIE, assets, events, landing, matches, pages, query_of, rooms, token, unix_now, upload,
    voice_routes as voice, watch,
};
use crate::{App, Matches};
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{any, get, post, put};
use axum::{Form, Json, Router};
use axum_extra::extract::CookieJar;
use cookie::Cookie;
use serde::Deserialize;
use std::collections::HashMap;
use tower::ServiceBuilder;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;

type Routes = Router<Shared>;

/// The routes of a match's or a room's voice from outside, under `at`.
fn voiced(router: Routes, at: &str) -> Routes {
    router
        .route(&format!("{at}/voice"), post(voice::voice))
        .route(&format!("{at}/voice-mumble"), post(voice::mumble))
        .route(&format!("{at}/voice-status"), post(voice::status))
        .route(&format!("{at}/voice-match"), post(voice::matched))
        .route(&format!("{at}/voice-comms"), get(voice::comms))
        .route(&format!("{at}/voices"), get(voice::voices))
        .route(&format!("{at}/voice-invite"), get(voice::invite))
}

/// A match's routes under `at` (its id); `next` and `after` name the segments after it.
fn of_match(router: Routes, at: &str, [next, after]: [&str; 2]) -> Routes {
    voiced(router, at)
        .route(&format!("{at}/"), get(matches::show))
        .route(&format!("{at}/{next}"), get(matches::show))
        .route(&format!("{at}/overlay"), get(matches::show))
        .route(&format!("{at}/overlay/{after}"), get(matches::show))
        .route(&format!("{at}/demos"), get(matches::demos))
        .route(&format!("{at}/demos/{after}"), get(matches::demo))
        .route(&format!("{at}/watch/{after}"), get(matches::watch))
        .route(&format!("{at}/room"), get(matches::room).post(matches::room_opened))
        .route(&format!("{at}/timeline"), get(matches::timeline))
        .route(&format!("{at}/marks"), get(matches::timeline))
        .route(&format!("{at}/rewind"), post(matches::rewound))
        .route(&format!("{at}/events"), get(matches::events))
}

/// A room's routes: its pages, what it shows, and what is posted to it.
fn of_room(router: Routes) -> Routes {
    voiced(router, "/r/{id}")
        .route("/r/{id}", get(rooms::show))
        .route("/r/{id}/", get(rooms::show))
        .route("/r/{id}/{delay}", get(rooms::show))
        .route("/r/{id}/overlay", get(rooms::show))
        .route("/r/{id}/overlay/{delay}", get(rooms::show))
        .route("/r/{id}/events", get(rooms::events))
        .route("/r/{id}/me", get(rooms::mine))
        .route("/r/{id}/demos", get(rooms::demos))
        .route("/r/{id}/marks", get(rooms::marks))
        .route("/r/{id}/claim", post(rooms::claim))
        .route("/r/{id}/clear", post(rooms::clear))
        .route("/r/{id}/name", post(rooms::name))
        .route("/r/{id}/view", post(rooms::view))
        .route("/r/{id}/ink", post(rooms::ink))
        .route("/r/{id}/draw", post(rooms::draw))
        .route("/r/{id}/arc", post(rooms::arc))
        .route("/r/{id}/circle", post(rooms::circle))
        .route("/r/{id}/light", post(rooms::light))
        .route("/r/{id}/note", post(rooms::note))
        .route("/r/{id}/control", post(rooms::control))
        .route("/r/{id}/map", post(rooms::map))
        .route("/r/{id}/admin", post(rooms::admin))
}

/// The app's routes; those in `acting` act for the signed-in user.
pub(super) fn routes(app: &App) -> (Routes, Routes) {
    // Backpack pictures are files on disk; other static files are embedded.
    let day = SetResponseHeaderLayer::if_not_present(header::CACHE_CONTROL, HeaderValue::from_static("max-age=86400"));
    let all = Router::new()
        .route("/up", get(up))
        .route("/embed/v1/stv-map.js", get(|| async { assets::embed_loader() }))
        .route("/overview/{file}", get(overview))
        .route("/map/{name}", get(map))
        .nest_service("/backpack", ServiceBuilder::new().layer(day).service(ServeDir::new(&app.backpack)))
        .fallback(get(file));
    if matches!(app.matches, Matches::One(_)) {
        let one = all
            .route("/", get(one_page))
            .route("/index.html", get(one_page))
            .route("/{delay}", get(one_page))
            .route("/overlay", get(one_page))
            .route("/overlay/{delay}", get(one_page));
        return (one.route("/events", get(one_events)), Router::new());
    }
    // A body is no bigger than the little a form or JSON takes; an upload's part may be.
    let little = RequestBodyLimitLayer::new(MAX_REQUEST as usize);
    let big = (DefaultBodyLimit::max(upload::MAX_PART), RequestBodyLimitLayer::new(upload::MAX_PART));
    let part = Router::new().route("/upload/{id}", put(upload::part).layer(big));
    let acting = Router::new().route("/", get(front)).route("/watch", post(watched)).route("/mumble", get(mumble));
    let all = all
        .route("/matches", get(matches::list))
        .route("/mumble.json", get(mumble_json))
        .route("/upload", get(upload::form).post(upload::begun))
        .route("/upload/{id}", post(upload::finished))
        .route("/signin", get(signin))
        .route("/signed_in", get(signed_in))
        .route("/signout", post(signout))
        .route("/voice/link", get(voice::link))
        .route("/voice/linked", get(voice::linked))
        .route("/voice/who", get(voice::who))
        .route("/voice/mumble", get(voice::mumble_jobs))
        .route("/voice/known", get(voice::known).post(voice::told))
        .route("/start", any(started))
        .route("/fault", post(fault));
    // Ids are one or two segments: routes cover both, `MatchAt` reads the id.
    let all = of_match(of_room(all).route("/w/{a}", get(matches::show)), "/w/{a}", ["{b}", "{c}"]);
    (of_match(all, "/w/{a}/{b}", ["{c}", "{d}"]).layer(little).merge(part), acting.layer(little))
}

async fn up() -> &'static str {
    "ok"
}

async fn file(uri: Uri) -> Response {
    assets::site_file(uri.path())
}

/// `/overview/<file>`: fetched on a miss, which may take a while.
async fn overview(State(app): State<Shared>, Path(file): Path<String>) -> Answered {
    answer(move || Ok(assets::cached_overview(&app, &file))).await
}

/// `/map/<name>`: built on a miss.
async fn map(v: Visit, Path(name): Path<String>) -> Answered {
    answer(move || Ok(assets::map(&v.app, &name, v.gzip))).await
}

/// The one match's page, at `/`.
async fn one_page(v: Visit) -> Answered {
    if v.path == "/index.html" || v.path.strip_prefix('/').is_some_and(matches::is_page) {
        return Ok(pages::viewer(&v.app));
    }
    Ok(assets::site_file(&v.path))
}

async fn one_events(v: Visit, Query(q): Query<Stream>) -> Response {
    let Matches::One(hub) = &v.app.matches else { return StatusCode::NOT_FOUND.into_response() };
    match v.app.stream_slot(v.client) {
        Some(slot) => events::stream(hub, v.gzip, &q, slot),
        None => events::too_many_streams(),
    }
}

async fn front(v: Visit) -> Answered {
    answer(move || Ok(landing(&v, v.sessions()?, ""))).await
}

/// The front page's form.
#[derive(Deserialize)]
struct Watch {
    #[serde(default)]
    connect: String,
}

/// `POST /watch`: the match the form asks for, or the page again with why not.
async fn watched(v: Visit, Form(form): Form<Watch>) -> Answered {
    answer(move || {
        let sessions = v.sessions()?;
        Ok(match watch(&v.app, sessions, v.who.user.as_ref(), &form.connect, v.client) {
            Ok(m) => Redirect::to(&format!("{}/w/{}/", v.app.base, m.id)).into_response(),
            Err(e) => {
                let status = if v.app.keys.is_some() && v.who.user.is_none() { StatusCode::FORBIDDEN } else { StatusCode::BAD_REQUEST };
                pages::with(status, landing(&v, sessions, &format!("{e:#}")))
            }
        })
    })
    .await
}

async fn mumble(State(app): State<Shared>) -> Response {
    pages::bots(&app)
}

async fn mumble_json() -> Response {
    assets::site_file("mumble.json")
}

/// `/signin`: to the site in front, to come back to `?return=`.
async fn signin(State(app): State<Shared>, Query(back): Query<Back>) -> Response {
    let Some(site) = app.home.as_deref().and_then(origin) else { return StatusCode::NOT_FOUND.into_response() };
    // The site hands the return path back as it is: with it, what this browser set out with.
    let state = crate::matches::sessions::random_id();
    let back = return_to(&app, back.to.as_deref());
    let back = format!("{back}{}{SIGNIN_STATE}{state}", if back.contains('?') { '&' } else { '?' });
    let began = set_cookie(&app, SIGNIN_COOKIE, &state, SIGNIN_FOR);
    ([(header::SET_COOKIE, began)], Redirect::to(&format!("{site}/stv/sign_in?{}", query_of(&[("return", &back)])))).into_response()
}

/// A sign-in under way: its cookie, how long it has, and what ends its return path.
const SIGNIN_COOKIE: &str = "stvtf_signin";
const SIGNIN_FOR: u64 = 600;
const SIGNIN_STATE: &str = "signin=";

/// Back to the page `back` names, setting `cookie`.
fn back_with(app: &App, back: &Back, cookie: String) -> Response {
    ([(header::SET_COOKIE, cookie)], Redirect::to(&return_to(app, back.to.as_deref()))).into_response()
}

/// `/signed_in`: back from the site, with who signed in, in the browser that set out.
async fn signed_in(State(app): State<Shared>, cookies: CookieJar, Query(mut back): Query<Back>) -> Response {
    let Some(keys) = &app.keys else { return StatusCode::NOT_FOUND.into_response() };
    let to = back.to.take().unwrap_or_default();
    let (to, state) = to.rsplit_once(SIGNIN_STATE).unwrap_or((&to, ""));
    if state.is_empty() || cookies.get(SIGNIN_COOKIE).map(Cookie::value) != Some(state) {
        return (StatusCode::BAD_REQUEST, "Sign in again").into_response();
    }
    back.to = Some(to.trim_end_matches(['?', '&']).to_string());
    match keys.open_signin(back.t.as_deref().unwrap_or_default(), unix_now()) {
        Ok(user) => {
            let user = set_cookie(&app, USER_COOKIE, &keys.keep(&user, unix_now()), token::SIGNED_IN_FOR);
            let mut signed = back_with(&app, &back, user);
            if let Ok(done) = HeaderValue::from_str(&set_cookie(&app, SIGNIN_COOKIE, "", 0)) {
                signed.headers_mut().append(header::SET_COOKIE, done);
            }
            signed
        }
        Err(e) => (StatusCode::BAD_REQUEST, format!("{e:#}")).into_response(),
    }
}

/// `POST /signout`.
async fn signout(State(app): State<Shared>, Query(back): Query<Back>) -> Response {
    back_with(&app, &back, set_cookie(&app, USER_COOKIE, "", 0))
}

/// `/start`: the match a site's token asks for; `format=json` answers its address.
async fn started(v: Visit, Query(q): Query<HashMap<String, String>>) -> Answered {
    answer(move || {
        let token = q.get("t").map_or("", String::as_str);
        Ok(match matches::start(&v, v.sessions()?, token) {
            Ok(m) if q.get("format").map(String::as_str) == Some("json") => {
                Json(serde_json::json!({ "page": format!("{}/w/{}/", v.app.base, m.id) })).into_response()
            }
            Ok(m) => Redirect::to(&format!("{}/w/{}/{}", v.app.base, m.id, matches::start_view(&q))).into_response(),
            Err(e) => (StatusCode::BAD_REQUEST, format!("{e:#}")).into_response(),
        })
    })
    .await
}

/// `/fault`: what a page says went wrong in it (see `report`): taken or not, it is not told.
async fn fault(v: Visit, headers: HeaderMap, said: Bytes) -> StatusCode {
    let agent = headers.get(header::USER_AGENT).and_then(|a| a.to_str().ok()).unwrap_or_default();
    crate::report::page_fault(&said, agent.trim(), v.who.user.as_ref().map(|u| u.uid.as_str()));
    StatusCode::NO_CONTENT
}
