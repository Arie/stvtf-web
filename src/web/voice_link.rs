//! Who a Discord user is on Steam, for voice from Discord: `GET /voice/link` signs them in with
//! Discord, which says which Steam account they connected to it (`/voice/linked`); kept, and told
//! to the voice bot (`GET /voice/who?discord=<id>`, with its key). Linking is off without a
//! Discord application (`STVTF_DISCORD_CLIENT_ID`, `_SECRET` and `_REDIRECT`).

use super::voice_in::is_key;
use super::{query_of, set_cookie};
use crate::App;
use crate::matches::sessions::random_id;
use crate::voice::ids::is_snowflake;
use crate::voice::links::ask_discord;
use axum::Json;
use axum::http::StatusCode;
use axum::http::header;
use axum::response::IntoResponse;
use axum::response::Redirect;
use axum::response::Response;

/// The cookie that says which sign-in a browser began, and how long it has to end it.
pub(super) const STATE_COOKIE: &str = "voice_link";
const STATE_FOR: u64 = 600;

/// `/voice/link`: off to Discord, to come back to `/voice/linked`.
pub(super) fn link(app: &App) -> Response {
    let Some(d) = &app.voice.links.discord else { return (StatusCode::NOT_FOUND, "No linking here").into_response() };
    let state = random_id();
    let asked = [
        ("response_type", "code"),
        ("client_id", d.client_id.as_str()),
        ("scope", "identify connections"),
        ("redirect_uri", d.redirect.as_str()),
        ("state", state.as_str()),
    ];
    let to = format!("{}/oauth2/authorize?{}", d.api, query_of(&asked));
    ([(header::SET_COOKIE, set_cookie(app, STATE_COOKIE, &state, STATE_FOR))], Redirect::to(&to)).into_response()
}

/// `/voice/linked`: back from Discord with a code, in the browser that set out.
pub(super) fn linked(app: &App, state: &str, code: &str, began: Option<&str>) -> Response {
    let Some(d) = &app.voice.links.discord else { return (StatusCode::NOT_FOUND, "No linking here").into_response() };
    if state.is_empty() || code.is_empty() || began != Some(state) {
        return (StatusCode::BAD_REQUEST, "Expired. Try again.").into_response();
    }
    match ask_discord(d, code) {
        Ok((discord, Some(steamid))) => {
            let done = format!("Linked to {steamid}.");
            app.voice.links.link(discord, steamid);
            done.into_response()
        }
        Ok((_, None)) => (StatusCode::CONFLICT, "No Steam on your Discord. Add it under Connections.").into_response(),
        Err(e) => {
            log!("voice link failed: {e:#}");
            (StatusCode::BAD_GATEWAY, "Discord error. Try again.").into_response()
        }
    }
}

/// `/voice/who?discord=<id>`: the Steam id linked to a Discord user, to the bot.
pub(super) fn who(app: &App, bearer: Option<&str>, discord: Option<&str>) -> Response {
    let Some(key) = &app.voice.key else { return StatusCode::NOT_FOUND.into_response() };
    if !is_key(key, bearer) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(discord) = discord.filter(|d| is_snowflake(d)) else {
        return (StatusCode::BAD_REQUEST, "a Discord id").into_response();
    };
    let who = serde_json::json!({ "steamid": app.voice.links.steamid(discord) });
    Json(who).into_response()
}
