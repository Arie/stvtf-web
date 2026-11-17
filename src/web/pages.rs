//! The pages: templates in `templates/`, filled in and escaped by askama.

use std::sync::OnceLock;

use askama::Template;
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use serde::Deserialize;

use super::app_version;
use crate::app::App;

/// The viewer page of a match or room; preloads all modules (the import chain is 15 deep).
#[derive(Template)]
#[template(path = "index.html")]
pub(super) struct Viewer<'a> {
    base: &'a str,
    version: &'static str,
    modules: &'static [String],
}

/// The front page, with the last attempt's error and the visitor's own list.
#[derive(Template)]
#[template(path = "landing.html")]
pub(super) struct Front<'a> {
    base: &'a str,
    version: &'static str,
    signed: bool,
    max: u64,
    error: &'a str,
    yours: Vec<Yours>,
}

/// Something of the visitor's: a demo, a match or a room, and its page.
pub(super) struct Yours {
    pub(super) kind: &'static str,
    pub(super) page: String,
    pub(super) what: String,
}

/// The voice bot's Mumble certificates.
#[derive(Template)]
#[template(path = "mumble.html")]
struct Bots<'a> {
    base: &'a str,
    version: &'static str,
    listed: &'static [Bot],
}

#[derive(Deserialize)]
struct Bot {
    name: String,
    sha1: String,
}

/// A match's comms demo: map, MB, download (`href`) and replay (`watch`) links.
#[derive(serde::Serialize)]
pub(super) struct Kept {
    pub(super) map: String,
    pub(super) mb: u64,
    pub(super) href: String,
    pub(super) watch: String,
}

/// A match that is gone, with its demos to download.
#[derive(Template)]
#[template(path = "gone.html")]
struct Gone<'a> {
    base: &'a str,
    version: &'static str,
    demos: &'a [Kept],
}

/// A page: no-cache, so a reload gets a new build (it names the build's files).
fn page(template: &impl Template) -> Response {
    match template.render() {
        Ok(html) => ([(header::CACHE_CONTROL, "no-cache")], Html(html)).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub(super) fn viewer(app: &App) -> Response {
    page(&Viewer { base: &app.base, version: app_version(), modules: super::assets::page_modules() })
}

pub(super) fn front(app: &App, signed: bool, error: &str, yours: Vec<Yours>) -> Response {
    page(&Front { base: &app.base, version: app_version(), signed, max: super::upload::MAX_DEMO >> 20, error, yours })
}

pub(super) fn bots(app: &App) -> Response {
    static BOTS: OnceLock<Vec<Bot>> = OnceLock::new();
    let bots =
        BOTS.get_or_init(|| super::assets::mumble_bots().and_then(|listed| serde_json::from_slice(&listed).ok()).unwrap_or_default());
    page(&Bots { base: &app.base, version: app_version(), listed: bots })
}

pub(super) fn gone(app: &App, demos: &[Kept]) -> Response {
    page(&Gone { base: &app.base, version: app_version(), demos })
}

/// `status`, with `page` as its body.
pub(super) fn with(status: StatusCode, mut page: Response) -> Response {
    *page.status_mut() = status;
    page
}
