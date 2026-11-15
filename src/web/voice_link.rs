//! Who a Discord user is on Steam, for voice from Discord: `GET /voice/link` signs them in with
//! Discord, which says which Steam account they connected to it (`/voice/linked`); kept, and told
//! to the voice bot (`GET /voice/who?discord=<id>`, with its key). Linking is off without a
//! Discord application (`STVTF_DISCORD_CLIENT_ID`, `_SECRET` and `_REDIRECT`).

use super::voice_in::is_key;
use super::{JSON, Request, TEXT, query_of, respond, respond_with, set_cookie};
use crate::App;
use crate::matches::sessions::random_id;
use crate::voice::ids::is_snowflake;
use crate::voice::links::ask_discord;
use axum::http::StatusCode;
use axum::response::Response;

/// The cookie that says which sign-in a browser began, and how long it has to end it.
pub(super) const STATE_COOKIE: &str = "voice_link";
const STATE_FOR: u64 = 600;

/// `/voice/link`: off to Discord, to come back to `/voice/linked`.
pub(super) fn link(app: &App) -> Response {
    let Some(d) = &app.voice.links.discord else { return respond(StatusCode::NOT_FOUND, TEXT, b"No linking here") };
    let state = random_id();
    let asked = [
        ("response_type", "code"),
        ("client_id", d.client_id.as_str()),
        ("scope", "identify connections"),
        ("redirect_uri", d.redirect.as_str()),
        ("state", state.as_str()),
    ];
    let to = format!("{}/oauth2/authorize?{}", d.api, query_of(&asked));
    respond_with(StatusCode::FOUND, &[("Location", &to), ("Set-Cookie", &set_cookie(app, STATE_COOKIE, &state, STATE_FOR))], b"")
}

/// `/voice/linked`: back from Discord with a code, in the browser that set out.
pub(super) fn linked(app: &App, request: &Request) -> Response {
    let Some(d) = &app.voice.links.discord else { return respond(StatusCode::NOT_FOUND, TEXT, b"No linking here") };
    let state = request.query.get("state").unwrap_or_default();
    let code = request.query.get("code").map_or_default(str::to_string);
    if state.is_empty() || code.is_empty() || request.voice_link.as_deref() != Some(state) {
        return respond(StatusCode::BAD_REQUEST, TEXT, b"Expired. Try again.");
    }
    match ask_discord(d, &code) {
        Ok((discord, Some(steamid))) => {
            let done = format!("Linked to {steamid}.");
            app.voice.links.link(discord, steamid);
            respond(StatusCode::OK, TEXT, done.as_bytes())
        }
        Ok((_, None)) => respond(StatusCode::CONFLICT, TEXT, b"No Steam on your Discord. Add it under Connections."),
        Err(e) => {
            log!("voice link failed: {e:#}");
            respond(StatusCode::BAD_GATEWAY, TEXT, b"Discord error. Try again.")
        }
    }
}

/// `/voice/who?discord=<id>`: the Steam id linked to a Discord user, to the bot.
pub(super) fn who(app: &App, request: &Request) -> Response {
    let Some(key) = &app.voice.key else { return respond(StatusCode::NOT_FOUND, TEXT, b"") };
    if !is_key(key, request.bearer.as_deref()) {
        return respond(StatusCode::UNAUTHORIZED, TEXT, b"");
    }
    let Some(discord) = request.query.get("discord").filter(|d| is_snowflake(d)) else {
        return respond(StatusCode::BAD_REQUEST, TEXT, b"a Discord id");
    };
    let who = serde_json::json!({ "steamid": app.voice.links.steamid(discord) });
    respond(StatusCode::OK, JSON, who.to_string().as_bytes())
}
