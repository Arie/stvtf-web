//! Voice from outside a match (a team's Discord, through the bot): `POST /w/<id>/voice` with the
//! bot's key. Off without `STVTF_VOICE_KEY`.

use super::{Request, TEXT, json_body, respond};
use crate::App;
use crate::matches::sessions::Session;
use crate::matches::voice_feed::{MAX_DELAY, Speaker};
use crate::matches::voice_hold::Took;
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
fn is_key(key: &str, given: Option<&str>) -> bool {
    let Some(given) = given else { return false };
    key.len() == given.len() && key.bytes().zip(given.bytes()).fold(0, |d, (a, b)| d | (a ^ b)) == 0
}

/// `[U:1:<account>]`.
fn is_steamid(s: &str) -> bool {
    s.strip_prefix("[U:1:")
        .and_then(|r| r.strip_suffix(']'))
        .is_some_and(|n| !n.is_empty() && n.len() <= 10 && n.bytes().all(|b| b.is_ascii_digit()))
}

/// A Discord id: digits.
fn is_snowflake(s: &str) -> bool {
    !s.is_empty() && s.len() <= 20 && s.bytes().all(|b| b.is_ascii_digit())
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
    if said.frames.len() > MAX_FRAMES {
        return respond(s, "413 Content Too Large", TEXT, b"too many frames");
    }
    let b64 = base64::engine::general_purpose::STANDARD;
    let sound = !said.frames.is_empty() && said.frames.iter().all(|f| f.len() <= MAX_FRAME && b64.decode(f).is_ok_and(|d| !d.is_empty()));
    let known = match (&said.steamid, &said.discord) {
        (Some(steamid), _) => is_steamid(steamid),
        (None, Some(discord)) => is_snowflake(discord),
        (None, None) => false,
    };
    if !sound || !known || said.delay.is_some_and(|d| !(0.0..=MAX_DELAY).contains(&d)) {
        return respond(s, "400 Bad Request", TEXT, b"not voice");
    }
    let who = Speaker {
        steamid: said.steamid,
        discord: said.discord.filter(|d| is_snowflake(d)),
        name: said.name.map(|n| n.chars().filter(|c| !c.is_control()).take(MAX_NAME).collect()),
        channel: said.channel.filter(|c| is_snowflake(c)),
    };
    match m.voice_said(&who, said.frames, said.delay) {
        None => respond(s, "404 Not Found", TEXT, b"not a live match"),
        Some(Took::Held { due }) => respond(s, "200 OK", TEXT, format!("held until tick {due}").as_bytes()),
        Some(Took::PauseTalk) => respond(s, "200 OK", TEXT, b"pause talk"),
        Some(Took::Live) => respond(s, "200 OK", TEXT, b"live"),
        Some(Took::NoStream) => respond(s, "409 Conflict", TEXT, b"no stream yet"),
        Some(Took::Full) => respond(s, "429 Too Many Requests", TEXT, b"too many speakers"),
    }
}
