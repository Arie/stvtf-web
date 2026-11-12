//! Who a Discord user is on Steam, for voice from Discord: `GET /voice/link` signs them in with
//! Discord, which says which Steam account they connected to it (`/voice/linked`); kept, and told
//! to the voice bot (`GET /voice/who?discord=<id>`, with its key). Linking is off without a
//! Discord application (`STVTF_DISCORD_CLIENT_ID`, `_SECRET` and `_REDIRECT`).

use super::voice_in::is_key;
use super::{JSON, Request, TEXT, form_decode, param, respond, respond_with, set_cookie, url_encode};
use crate::App;
use crate::matches::sessions::random_id;
use crate::voice::ids::is_snowflake;
use crate::voice::links::ask_discord;
use std::net::TcpStream;

/// The cookie that says which sign-in a browser began, and how long it has to end it.
pub(super) const STATE_COOKIE: &str = "voice_link";
const STATE_FOR: u64 = 600;

/// `/voice/link`: off to Discord, to come back to `/voice/linked`.
pub(super) fn link(s: &mut TcpStream, app: &App) -> std::io::Result<()> {
    let Some(d) = &app.voice_links.discord else { return respond(s, "404 Not Found", TEXT, b"No linking here") };
    let state = random_id();
    let to = format!(
        "{}/oauth2/authorize?response_type=code&client_id={}&scope=identify+connections&redirect_uri={}&state={state}",
        d.api,
        url_encode(&d.client_id),
        url_encode(&d.redirect),
    );
    respond_with(s, "302 Found", &[("Location", &to), ("Set-Cookie", &set_cookie(app, STATE_COOKIE, &state, STATE_FOR))], b"")
}

/// `/voice/linked`: back from Discord with a code, in the browser that set out.
pub(super) fn linked(s: &mut TcpStream, app: &App, request: &Request) -> std::io::Result<()> {
    let Some(d) = &app.voice_links.discord else { return respond(s, "404 Not Found", TEXT, b"No linking here") };
    let state = param(&request.query, "state").unwrap_or_default();
    let code = param(&request.query, "code").map(form_decode).unwrap_or_default();
    if state.is_empty() || code.is_empty() || request.voice_link.as_deref() != Some(state) {
        return respond(s, "400 Bad Request", TEXT, b"Expired. Try again.");
    }
    match ask_discord(d, &code) {
        Ok((discord, Some(steamid))) => {
            let done = format!("Linked to {steamid}.");
            app.voice_links.link(discord, steamid);
            respond(s, "200 OK", TEXT, done.as_bytes())
        }
        Ok((_, None)) => respond(s, "409 Conflict", TEXT, b"No Steam on your Discord. Add it under Connections."),
        Err(e) => {
            log!("voice link failed: {e:#}");
            respond(s, "502 Bad Gateway", TEXT, b"Discord error. Try again.")
        }
    }
}

/// `/voice/who?discord=<id>`: the Steam id linked to a Discord user, to the bot.
pub(super) fn who(s: &mut TcpStream, app: &App, request: &Request) -> std::io::Result<()> {
    let Some(key) = &app.voice_key else { return respond(s, "404 Not Found", TEXT, b"") };
    if !is_key(key, request.bearer.as_deref()) {
        return respond(s, "401 Unauthorized", TEXT, b"");
    }
    let Some(discord) = param(&request.query, "discord").filter(|d| is_snowflake(d)) else {
        return respond(s, "400 Bad Request", TEXT, b"a Discord id");
    };
    let who = serde_json::json!({ "steamid": app.voice_links.steamid(discord) });
    respond(s, "200 OK", JSON, who.to_string().as_bytes())
}
