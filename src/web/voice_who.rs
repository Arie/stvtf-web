//! Who is who in a channel brought into a match, and how that channel goes, for whoever has a say
//! in the match's voice (`voice_in::has_say`): on a match's page and on a room's.
//!
//! - `POST <page>voice-status`, the bot's (its key and the match's invite): how it goes in the
//!   channel it is in (`kind`: `discord` or `mumble`; `status`), who is in it (`people`), or
//!   that it left.
//! - `GET <page>voice-comms`: where the bot is (in Discord, and each Mumble channel), and how
//!   it goes.
//! - `GET <page>voices`: those in the channel and those heard, and whom each is put on.
//! - `POST <page>voice-match`: someone is a player of the match, a `SteamID`, or nobody.
//!
//! What someone says of who is who is remembered (`voice::who`), and holds when the same
//! person is in a channel again, in any region.

use super::visitor::Who;
use super::voice_in::{has_say, is_key};
use super::{JSON, Request, TEXT, json_body, respond};
use crate::matches::sessions::Session;
use crate::voice::feed::Put;
use crate::voice::ids::{is_voice_id, parse_steamid};
use crate::voice::who::Recalled;
use crate::{App, lock};
use serde::Deserialize;
use std::io::{BufReader, Take};
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// The most people a channel is said to have, and the longest a name or a status.
const MAX_PEOPLE: usize = 64;
const MAX_TEXT: usize = 100;
/// The bot says how Discord goes this often at least: past it, it is taken to be gone.
pub(crate) const DISCORD_SAID_FOR: Duration = Duration::from_secs(45);

#[derive(Deserialize)]
struct Member {
    id: String,
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct Report {
    #[serde(default)]
    invite: Option<String>,
    kind: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    people: Option<Vec<Member>>,
    #[serde(default)]
    left: bool,
    /// The channel this is of: a match may have more than one (each team's).
    #[serde(default)]
    channel: Option<String>,
    /// For Mumble: which asking for a channel (`MumbleJob::asked`) this answers, and which of
    /// the match's channels (`MumbleJob::id`) it is of.
    #[serde(default)]
    asked: Option<u64>,
    #[serde(default)]
    job: Option<u32>,
}

/// The most channels a match keeps the people and the going of.
const MAX_CHANNELS: usize = 8;

/// `POST <page>voice-status`, the bot's.
pub(super) fn post_voice_status(
    s: &mut TcpStream,
    reader: &mut BufReader<Take<TcpStream>>,
    app: &App,
    m: Option<&Session>,
    request: &Request,
) -> std::io::Result<()> {
    let Some(key) = &app.voice_key else { return respond(s, "404 Not Found", TEXT, b"") };
    if !is_key(key, request.bearer.as_deref()) {
        return respond(s, "401 Unauthorized", TEXT, b"");
    }
    let Some(m) = m.filter(|m| m.is_live()) else { return respond(s, "404 Not Found", TEXT, b"not a live match") };
    let Ok(said) = json_body::<Report>(reader, request) else { return respond(s, "400 Bad Request", TEXT, b"how it goes") };
    if !is_key(m.voice_invite(), said.invite.as_deref()) {
        return respond(s, "403 Forbidden", TEXT, b"not invited");
    }
    let text: String = said.status.chars().filter(|c| !c.is_control()).take(MAX_TEXT).collect();
    // Which channel: the bot's own name for it, else the one of its kind.
    let channel = said.channel.clone().filter(|c| is_voice_id(c)).unwrap_or_else(|| said.kind.clone());
    match said.kind.as_str() {
        "discord" => discord_status(m, &said, &channel, text),
        "mumble" => {
            if !mumble_status(m, &said, &channel, text) {
                return respond(s, "204 No Content", TEXT, b"");
            }
        }
        _ => return respond(s, "400 Bad Request", TEXT, b"Discord or Mumble"),
    }
    if said.left || said.people.is_some() {
        take_roster(app, m, &channel, said.people.unwrap_or_default());
    }
    respond(s, "204 No Content", TEXT, b"")
}

/// How it goes in a Discord `channel` the bot is in for match `m`, or that it left.
fn discord_status(m: &Session, said: &Report, channel: &str, text: String) {
    let mut going = lock(&m.discord);
    going.retain(|_, (_, at)| at.elapsed() < DISCORD_SAID_FOR);
    if said.left {
        going.remove(channel);
    } else if going.len() < MAX_CHANNELS || going.contains_key(channel) {
        going.insert(channel.to_string(), (text, Instant::now()));
    }
}

/// How it goes in one of match `m`'s Mumble channels: by its number, or the only one there is.
/// Whether it was taken: not of a channel the bot was since asked out of.
fn mumble_status(m: &Session, said: &Report, channel: &str, text: String) -> bool {
    let mut jobs = lock(&m.mumble);
    let only = jobs.len() == 1;
    let job = jobs.iter_mut().find(|j| said.job.map_or(only, |id| id == j.id));
    let Some(job) = job.filter(|j| said.asked.is_none_or(|a| a == j.asked)) else { return false };
    job.status = text;
    if said.people.is_some() {
        job.roster = Some(channel.to_string());
    }
    true
}

/// Those the bot says are in `channel` now (none: it left), and whom they were said to be before.
fn take_roster(app: &App, m: &Session, channel: &str, people: Vec<Member>) {
    let people = people
        .into_iter()
        .filter(|p| is_voice_id(&p.id))
        .take(MAX_PEOPLE)
        .map(|p| (p.id, p.name.chars().filter(|c| !c.is_control()).take(MAX_TEXT).collect::<String>()));
    m.voice.set_roster(&m.hub, channel, people);
    recall(app, m);
}

/// Whom those in the match's channels were said to be before holds here too, until someone says
/// otherwise; a Discord user nothing is known of is asked about at the sites.
fn recall(app: &App, m: &Session) {
    for id in m.voice.roster_ids() {
        match app.voice_who.recall(&id) {
            Some(Recalled::Player { steamid, sure }) => m.voice.set_known_before(&id, Put::Player { steamid, confirmed: sure }),
            Some(Recalled::Nobody) => m.voice.set_known_before(&id, Put::Nobody),
            None => {}
        }
        app.voice_who.look_up(&id);
    }
}

/// The match whose voice `who` has a say in, if it is one and they have.
fn theirs<'a>(app: &App, m: Option<&'a Session>, who: &Who) -> Option<&'a Session> {
    m.filter(|m| m.is_live() && app.voice_key.is_some() && has_say(app, m, who))
}

/// `GET <page>voice-comms`: where the bot is, in Discord or Mumble, and how it goes. No content
/// for someone with no say.
pub(super) fn get_voice_comms(s: &mut TcpStream, app: &App, m: Option<&Session>, who: &Who) -> std::io::Result<()> {
    let Some(m) = theirs(app, m, who) else { return respond(s, "204 No Content", TEXT, b"") };
    // Each Discord channel's going, one after the other.
    let going: Vec<String> =
        lock(&m.discord).values().filter(|(_, at)| at.elapsed() < DISCORD_SAID_FOR).map(|(status, _)| status.clone()).collect();
    let discord = (!going.is_empty()).then(|| serde_json::json!({ "status": going.join(" + ") }));
    let mumble: Vec<_> = lock(&m.mumble)
        .iter()
        .map(|j| serde_json::json!({ "id": j.id, "host": j.host, "port": j.port, "channel": j.channel, "delay": j.delay, "status": j.status }))
        .collect();
    respond(s, "200 OK", JSON, serde_json::json!({ "discord": discord, "mumble": mumble }).to_string().as_bytes())
}

/// `GET <page>voices`: those in the channel and those heard, and whom each is put on.
pub(super) fn get_voices(s: &mut TcpStream, app: &App, m: Option<&Session>, who: &Who) -> std::io::Result<()> {
    let Some(m) = theirs(app, m, who) else { return respond(s, "204 No Content", TEXT, b"") };
    // What the sites said since of those in the channel shows at once.
    recall(app, m);
    respond(s, "200 OK", JSON, serde_json::json!(m.voice.voices(&m.hub)).to_string().as_bytes())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Match {
    #[serde(rename = "discord")]
    voice_id: String,
    #[serde(default)]
    player: Option<u16>,
    #[serde(default)]
    steamid: Option<String>,
}

/// `POST <page>voice-match`: `discord` is the match's `player`, or `steamid` (as people write
/// them), or with neither nobody.
pub(super) fn post_voice_match(
    s: &mut TcpStream,
    reader: &mut BufReader<Take<TcpStream>>,
    app: &App,
    m: Option<&Session>,
    request: &Request,
    who: &Who,
) -> std::io::Result<()> {
    let Some(m) = m.filter(|m| m.is_live() && app.voice_key.is_some()) else {
        return respond(s, "409 Conflict", TEXT, b"No voice here");
    };
    if !has_say(app, m, who) {
        return respond(s, "403 Forbidden", TEXT, b"Not allowed");
    }
    let Ok(v) = json_body::<Match>(reader, request) else {
        return respond(s, "400 Bad Request", TEXT, b"Invalid");
    };
    // Looked up before anything is answered: the match is not held meanwhile.
    let player = v.player.and_then(|p| lock(&m.hub).steamid_of_player(stvtf::output::PlayerId(p)).map(str::to_owned));
    let steamid = match (v.player, v.steamid.as_deref()) {
        (Some(_), _) => match player {
            Some(steamid) => Some(steamid),
            None => return respond(s, "400 Bad Request", TEXT, b"No such player"),
        },
        (None, Some(text)) => match parse_steamid(text) {
            Some(steamid) => Some(steamid),
            None => {
                return respond(s, "400 Bad Request", TEXT, b"Invalid Steam ID");
            }
        },
        (None, None) => None,
    };
    if !m.voice.put_on(&v.voice_id, steamid.clone()) {
        return respond(s, "400 Bad Request", TEXT, b"Not found");
    }
    let caster = who.user.as_ref().is_some_and(|u| u.streamer);
    app.voice_who.remember(&v.voice_id, steamid.as_deref(), &m.voice.name_of(&v.voice_id).unwrap_or_default(), caster);
    respond(s, "204 No Content", TEXT, b"")
}
