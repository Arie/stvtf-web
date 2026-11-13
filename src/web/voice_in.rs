//! Voice from outside a match (a team's Discord, through the bot): `POST /w/<id>/voice` with the
//! bot's key. Off without `STVTF_VOICE_KEY`.

use super::{Request, TEXT, json_body, respond};
use crate::App;
use crate::matches::sessions::Session;
use crate::voice::feed::{MAX_DELAY, Speaker};
use crate::voice::hold::Took;
use crate::voice::ids::is_voice_id;
use base64::Engine;
use serde::Deserialize;
use std::io::{BufReader, Take};
use std::net::TcpStream;
use std::sync::Arc;

/// The most frames in a post (0.64 s), and the longest a frame is in base64.
const MAX_FRAMES: usize = 32;
const MAX_FRAME: usize = 2000;

#[derive(Deserialize)]
struct Said {
    /// The match's voice invite, from the link the bot was given.
    #[serde(default)]
    invite: Option<String>,
    /// The speaker's `SteamID`, when the bot knows their link; else who they are in Discord.
    #[serde(default)]
    steamid: Option<String>,
    #[serde(default)]
    discord: Option<String>,
    #[serde(default)]
    name: Option<String>,
    /// The Discord voice channel they are in: those in one are taken for a team.
    #[serde(default)]
    channel: Option<String>,
    frames: Vec<String>,
    /// How far the match's stream is behind its game, in seconds, when the bot was told.
    #[serde(default)]
    delay: Option<f64>,
}

/// Whether `given` is the key, compared without stopping at the first difference.
pub(super) fn is_key(key: &str, given: Option<&str>) -> bool {
    let Some(given) = given else { return false };
    key.len() == given.len() && key.bytes().zip(given.bytes()).fold(0, |d, (a, b)| d | (a ^ b)) == 0
}

/// `[U:1:<account>]`.
fn is_steamid(s: &str) -> bool {
    s.strip_prefix("[U:1:")
        .and_then(|r| r.strip_suffix(']'))
        .is_some_and(|n| !n.is_empty() && n.len() <= 10 && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The most characters of a speaker's name kept.
const MAX_NAME: usize = 40;

pub(super) fn voice_in(
    s: &mut TcpStream,
    reader: &mut BufReader<Take<TcpStream>>,
    app: &App,
    m: &Arc<Session>,
    request: &Request,
) -> std::io::Result<()> {
    let Some(key) = &app.voice_key else { return respond(s, "404 Not Found", TEXT, b"") };
    if !is_key(key, request.bearer.as_deref()) {
        return respond(s, "401 Unauthorized", TEXT, b"");
    }
    let Ok(said) = json_body::<Said>(reader, request) else { return respond(s, "400 Bad Request", TEXT, b"not voice") };
    // The key says it is our bot; the invite, that whoever brought it in was let into this match.
    if !is_key(m.voice_invite(), said.invite.as_deref()) {
        return respond(s, "403 Forbidden", TEXT, b"not invited");
    }
    if said.frames.len() > MAX_FRAMES {
        return respond(s, "413 Content Too Large", TEXT, b"too many frames");
    }
    let b64 = base64::engine::general_purpose::STANDARD;
    let sound = !said.frames.is_empty() && said.frames.iter().all(|f| f.len() <= MAX_FRAME && b64.decode(f).is_ok_and(|d| !d.is_empty()));
    let known = match (&said.steamid, &said.discord) {
        (Some(steamid), _) => is_steamid(steamid),
        (None, Some(discord)) => is_voice_id(discord),
        (None, None) => false,
    };
    if !sound || !known || said.delay.is_some_and(|d| !(0.0..=MAX_DELAY).contains(&d)) {
        return respond(s, "400 Bad Request", TEXT, b"not voice");
    }
    let who = Speaker {
        steamid: said.steamid,
        discord: said.discord.filter(|d| is_voice_id(d)),
        name: said.name.map(|n| n.chars().filter(|c| !c.is_control()).take(MAX_NAME).collect()),
        channel: said.channel.filter(|c| is_voice_id(c)),
    };
    match m.take_in_voice(&who, said.frames, said.delay) {
        None => respond(s, "404 Not Found", TEXT, b"not a live match"),
        Some(Took::Held { due }) => respond(s, "200 OK", TEXT, format!("held until tick {due}").as_bytes()),
        Some(Took::PauseTalk) => respond(s, "200 OK", TEXT, b"pause talk"),
        Some(Took::Live) => respond(s, "200 OK", TEXT, b"live"),
        Some(Took::NoStream) => respond(s, "409 Conflict", TEXT, b"no stream yet"),
        Some(Took::Full) => respond(s, "429 Too Many Requests", TEXT, b"too many speakers"),
    }
}

/// Whether `who` gets a match's voice invite, and says whose voice is whose: whoever started it,
/// and the site's streamers; where nobody signs in (no site in front), anyone.
pub(super) fn has_say(app: &App, m: &Session, who: &super::visitor::Who) -> bool {
    app.keys.is_none() || m.started_by(&who.key) || who.user.as_ref().is_some_and(|u| u.streamer)
}

/// `GET /w/<id>/voice-invite` (and a room's): the invite, and where to add the bot to a Discord
/// server (`STVTF_VOICE_BOT`), to those with a say. Every page asks: no content for the rest, and
/// where no voice comes in, rather than an error in everyone's console.
pub(super) fn invite(s: &mut TcpStream, app: &App, m: Option<&Session>, who: &super::visitor::Who) -> std::io::Result<()> {
    let Some(m) = m.filter(|m| m.is_live() && app.voice_key.is_some() && has_say(app, m, who)) else {
        return respond(s, "204 No Content", TEXT, b"");
    };
    let invite = serde_json::json!({ "invite": m.voice_invite(), "bot": app.voice_bot });
    respond(s, "200 OK", super::JSON, invite.to_string().as_bytes())
}
