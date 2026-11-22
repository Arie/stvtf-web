//! The list of who is who between regions (`voice::who`): its routes, and asking a site.

use std::sync::Arc;

use anyhow::Context;
use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use super::incoming::is_key;
use crate::app::App;
use crate::voice::ids::{is_voice_id, parse_steamid};
use crate::voice::who::{CASTER, MAX_NAME, RegionEntry, Site, TIMEOUT};
use crate::web::token::Keys;

/// The site at `home` (as a stv.tf's `--home`), asked with tokens sealed with `keys`.
pub(crate) fn site(home: &str, keys: Arc<Keys>) -> Site {
    let url = format!("{}/discord_links/", home.trim_end_matches('/'));
    let name = home.split("://").last().unwrap_or(home).split('/').next().unwrap_or_default().to_string();
    let agent = ureq::AgentBuilder::new().timeout(TIMEOUT).redirects(0).build();
    let ask = move |discord: &str| {
        let bearer = format!("Bearer {}", keys.link_token(discord, crate::web::answer::unix_now()));
        match agent.get(&format!("{url}{discord}")).set("Authorization", &bearer).call() {
            Ok(answer) => {
                let said: serde_json::Value = serde_json::from_str(&answer.into_string()?)?;
                Ok(said["steam"].as_str().map(str::to_string))
            }
            Err(ureq::Error::Status(404, _)) => Ok(None),
            Err(e) => Err(e).context("cannot ask a site for a Discord link"),
        }
    };
    Site { name, ask: Box::new(ask) }
}

/// `GET /voice/known?since=<version>`, for another stv.tf: what changed since.
pub(in crate::web) fn get_known(app: &App, bearer: Option<&str>, since: u64) -> Response {
    let Some(key) = &app.voice.key else { return StatusCode::NOT_FOUND.into_response() };
    if !is_key(key, bearer) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let (version, known) = app.voice.who.since(since);
    Json(serde_json::json!({ "version": version, "known": known })).into_response()
}

/// `POST /voice/known`, from another stv.tf: what was said on a page there.
pub(in crate::web) fn post_known(app: &App, bearer: Option<&str>, said: &RegionEntry) -> Response {
    let Some(key) = &app.voice.key else { return StatusCode::NOT_FOUND.into_response() };
    if !is_key(key, bearer) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let steamid = said.steamid.as_deref().map(parse_steamid);
    if !is_voice_id(&said.id) || steamid == Some(None) {
        return (StatusCode::BAD_REQUEST, "Invalid").into_response();
    }
    let name: String = said.name.chars().filter(|c| !c.is_control()).collect();
    if said.src.len() > MAX_NAME || !said.src.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':')) {
        return (StatusCode::BAD_REQUEST, "Invalid").into_response();
    }
    if !matches!(said.by.as_str(), "" | CASTER) {
        return (StatusCode::BAD_REQUEST, "Invalid").into_response();
    }
    app.voice.who.take_from_region(&said.id, steamid.flatten().as_deref(), &name, &said.src, &said.by);
    StatusCode::NO_CONTENT.into_response()
}
