//! The pages: templates in `templates/`, filled in and escaped by askama.

use std::sync::OnceLock;
use std::time::SystemTime;

use askama::Template;
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use serde::Deserialize;

use super::app_version;
use crate::app::App;
use crate::rooms::registry::COMMS_DEMOS_KEPT;

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

/// How the voice bot is brought into a Discord server and a match; `bot` is where it is added.
#[derive(Template)]
#[template(path = "discord.html")]
struct Discord<'a> {
    base: &'a str,
    version: &'static str,
    bot: &'a str,
}

#[derive(Deserialize)]
struct Bot {
    name: String,
    sha1: String,
}

/// A match's comms demo: map, MB, download (`href`) and replay (`watch`) links. For the page of
/// a match that is gone, also how many minutes it plays (0 when its header does not say) and
/// when it was last written.
#[derive(serde::Serialize)]
pub(super) struct Kept {
    pub(super) map: String,
    pub(super) mb: u64,
    pub(super) href: String,
    pub(super) watch: String,
    #[serde(skip)]
    pub(super) min: u32,
    #[serde(skip)]
    pub(super) written: Option<SystemTime>,
}

/// A match that is gone, with its demos to download.
#[derive(Template)]
#[template(path = "gone.html")]
struct Gone<'a> {
    base: &'a str,
    version: &'static str,
    demos: &'a [Kept],
    /// The day it was played and the day its demos go, as [`day`] says them: empty when unknown.
    date: String,
    until: String,
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

/// `/discord`: not found where no bot can be added (`STVTF_VOICE_BOT`).
pub(super) fn discord(app: &App) -> Response {
    let Some(bot) = app.voice.bot.as_deref() else { return StatusCode::NOT_FOUND.into_response() };
    page(&Discord { base: &app.base, version: app_version(), bot })
}

pub(super) fn gone(app: &App, demos: &[Kept]) -> Response {
    // The first of its demos to go is the first written.
    let first = demos.iter().filter_map(|d| d.written).min();
    let (date, until) = (first.map_or_default(day), first.map_or_default(|at| day(at + COMMS_DEMOS_KEPT)));
    page(&Gone { base: &app.base, version: app_version(), demos, date, until })
}

/// A day as a page says it, in UTC: `24 Nov`.
fn day(at: SystemTime) -> String {
    let at = cookie::time::OffsetDateTime::from(at);
    let month = at.month().to_string();
    format!("{} {}", at.day(), &month[..3])
}

/// `status`, with `page` as its body.
pub(super) fn with(status: StatusCode, mut page: Response) -> Response {
    *page.status_mut() = status;
    page
}
