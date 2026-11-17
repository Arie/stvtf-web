//! Who is who in a channel brought into a match, and how that channel goes, for whoever has a say
//! in the match's voice (`incoming::has_say`): on a match's page and on a room's.
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
//!
//! A match's demo with its comms, watched here as a demo of its own, has the last two too, for
//! its voices that were no player's: whose each is said to be holds for that demo (built anew
//! with it), by its players (signed in, playing in it at the moment shown) and the streamers.

use super::incoming::{has_say, is_key};
use crate::matches::sessions::Session;
use crate::voice::feed::Put;
use crate::voice::ids::{is_voice_id, parse_steamid};
use crate::voice::who::Recalled;
use crate::voice::{ChannelState, DiscordGoing};
use crate::web::visitor::Who;
use crate::{App, lock};
use axum::Json;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use serde::Deserialize;
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
pub(in crate::web) struct Report {
    #[serde(default)]
    invite: Option<String>,
    kind: String,
    #[serde(default)]
    status: String,
    /// Where the bot is: `in`, `failed`, `left`; none from a bot before it said, and one not known
    /// here goes by the status.
    #[serde(default)]
    state: Option<String>,
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
pub(in crate::web) fn post_voice_status(app: &App, m: Option<&Session>, bearer: Option<&str>, said: Report) -> Response {
    let Some(key) = &app.voice.key else { return StatusCode::NOT_FOUND.into_response() };
    if !is_key(key, bearer) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(m) = m.filter(|m| m.is_live()) else { return (StatusCode::NOT_FOUND, "not a live match").into_response() };
    if !is_key(m.voice_invite(), said.invite.as_deref()) {
        return (StatusCode::FORBIDDEN, "not invited").into_response();
    }
    let text: String = said.status.chars().filter(|c| !c.is_control()).take(MAX_TEXT).collect();
    // Which channel: the bot's own name for it, else the one of its kind.
    let channel = said.channel.clone().filter(|c| is_voice_id(c)).unwrap_or_else(|| said.kind.clone());
    match said.kind.as_str() {
        "discord" => discord_status(m, &said, &channel, text),
        "mumble" => {
            if !mumble_status(m, &said, &channel, text) {
                return StatusCode::NO_CONTENT.into_response();
            }
        }
        _ => return (StatusCode::BAD_REQUEST, "Discord or Mumble").into_response(),
    }
    if said.left || said.people.is_some() {
        take_roster(app, m, &channel, said.people.unwrap_or_default());
    }
    StatusCode::NO_CONTENT.into_response()
}

/// How it goes in a Discord `channel` the bot is in for match `m`, or that it left.
fn discord_status(m: &Session, said: &Report, channel: &str, text: String) {
    let mut going = lock(&m.comms.discord);
    going.retain(|_, g| g.at.elapsed() < DISCORD_SAID_FOR);
    if said.left {
        going.remove(channel);
    } else if going.len() < MAX_CHANNELS || going.contains_key(channel) {
        let state = ChannelState::said(said.state.as_deref(), &text);
        going.insert(channel.to_string(), DiscordGoing { status: text, state, at: Instant::now() });
    }
}

/// How it goes in one of match `m`'s Mumble channels: by its number, or the only one there is.
/// Whether it was taken: not of a channel the bot was since asked out of.
fn mumble_status(m: &Session, said: &Report, channel: &str, text: String) -> bool {
    let mut jobs = lock(&m.comms.mumble);
    let only = jobs.len() == 1;
    let job = jobs.iter_mut().find(|j| said.job.map_or(only, |id| id == j.id));
    let Some(job) = job.filter(|j| said.asked.is_none_or(|a| a == j.asked)) else { return false };
    job.state = Some(ChannelState::said(said.state.as_deref(), &text));
    job.said = Some(crate::voice::mumble::Said(Instant::now()));
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
    m.comms.feed.set_roster(&m.hub, channel, people);
    recall(app, m);
}

/// Whom those in the match's channels were said to be before holds here too, until someone says
/// otherwise; a Discord user nothing is known of is asked about at the sites.
fn recall(app: &App, m: &Session) {
    for id in m.comms.feed.roster_ids() {
        match app.voice.who.recall(&id) {
            Some(Recalled::Player { steamid, sure }) => m.comms.feed.set_known_before(&id, Put::Player { steamid, confirmed: sure }),
            Some(Recalled::Nobody) => m.comms.feed.set_known_before(&id, Put::Nobody),
            None => {}
        }
        app.voice.who.look_up(&id);
    }
}

/// The match whose voice `who` has a say in, if it is one and they have.
fn theirs<'a>(app: &App, m: Option<&'a Session>, who: &Who) -> Option<&'a Session> {
    m.filter(|m| m.is_live() && app.voice.key.is_some() && has_say(app, m, who))
}

/// `GET <page>voice-comms`: where the bot is, in Discord or Mumble, and how it goes. No content
/// for someone with no say.
pub(in crate::web) fn get_voice_comms(app: &App, m: Option<&Session>, who: &Who) -> Response {
    let Some(m) = theirs(app, m, who) else { return StatusCode::NO_CONTENT.into_response() };
    // Each Discord channel's going, one after the other; in, unless one is not.
    let going: Vec<DiscordGoing> = lock(&m.comms.discord).values().filter(|g| g.at.elapsed() < DISCORD_SAID_FOR).cloned().collect();
    let state = going.iter().map(|g| g.state).find(|s| *s != ChannelState::In).unwrap_or(ChannelState::In);
    let status = going.iter().map(|g| g.status.as_str()).collect::<Vec<_>>().join(" + ");
    let discord = (!going.is_empty()).then(|| serde_json::json!({ "status": status, "state": state }));
    let mumble: Vec<_> = lock(&m.comms.mumble)
        .iter()
        .map(|j| {
            let state = j.state();
            serde_json::json!({ "id": j.id, "host": j.host, "port": j.port, "channel": j.channel, "delay": j.delay, "status": j.status, "state": state })
        })
        .collect();
    Json(serde_json::json!({ "discord": discord, "mumble": mumble })).into_response()
}

/// Where the demos are kept.
fn demos_dir(app: &App) -> Option<&std::path::Path> {
    match &app.matches {
        crate::Matches::Many(sessions) => sessions.store.as_ref().map(|s| s.dir.as_path()),
        crate::Matches::One(_) => None,
    }
}

/// The match and demo `m`, a demo with comms watched here, is of, and where the demos are, if
/// `who` may say whose its voices are: a player in it now, signed in, or a streamer; where nobody
/// signs in, anyone.
pub(in crate::web) fn reviewed<'a>(app: &'a App, m: Option<&'a Session>, who: &Who) -> Option<(&'a std::path::Path, &'a str, &'a str)> {
    let m = m?;
    let (id, file) = m.comms_of()?;
    let plays = |uid: &str| parse_steamid(uid).is_some_and(|steamid| lock(&m.hub).player_with(&steamid).is_some());
    let may = app.keys.is_none() || who.user.as_ref().is_some_and(|u| u.streamer || plays(&u.uid));
    may.then_some((demos_dir(app)?, id, file))
}

/// Who they are where they talk, for one a demo's lines name `discord:<that>`.
fn voice_id(key: &str) -> Option<&str> {
    key.strip_prefix(crate::voice::feed::UNLINKED).filter(|id| is_voice_id(id))
}

/// A demo's voices that were no player's, and whom each is said to be: for that demo, else as
/// known from before.
fn review_voices(app: &App, dir: &std::path::Path, id: &str, file: &str) -> Vec<crate::voice::feed::Person> {
    let picks = crate::matches::live_demos::picks(dir, id, file);
    let person = |(key, name): (String, String)| {
        let voice_id = voice_id(&key)?.to_string();
        let (steamid, confirmed) = match (picks.get(&key), app.voice.who.recall(&voice_id)) {
            (Some(steamid), _) => (steamid.clone(), true),
            (None, Some(Recalled::Player { steamid, sure })) => (Some(steamid), sure),
            (None, Some(Recalled::Nobody)) => (None, true),
            (None, None) => (None, false),
        };
        let name = if name.is_empty() { voice_id.clone() } else { name };
        Some(crate::voice::feed::Person { voice_id, name, steamid, linked: None, heard: true, confirmed })
    };
    crate::matches::live_demos::voices_in(dir, id, file).into_iter().filter_map(person).collect()
}

/// `GET <page>voices`: those in the channel and those heard, and whom each is put on.
pub(in crate::web) fn get_voices(app: &App, m: Option<&Session>, who: &Who) -> Response {
    if let Some((dir, id, file)) = reviewed(app, m, who) {
        return Json(serde_json::json!(review_voices(app, dir, id, file))).into_response();
    }
    let Some(m) = theirs(app, m, who) else { return StatusCode::NO_CONTENT.into_response() };
    // What the sites said since of those in the channel shows at once.
    recall(app, m);
    Json(serde_json::json!(m.comms.feed.voices(&m.hub))).into_response()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::web) struct Match {
    #[serde(rename = "discord")]
    voice_id: String,
    #[serde(default)]
    player: Option<u16>,
    #[serde(default)]
    steamid: Option<String>,
}

/// `POST <page>voice-match`: `discord` is the match's `player`, or `steamid` (as people write
/// them), or with neither nobody.
pub(in crate::web) fn post_voice_match(app: &App, m: Option<&Session>, v: &Match, who: &Who) -> Response {
    if m.is_some_and(|m| m.comms_of().is_some()) {
        return post_review_match(app, m, v, who);
    }
    let Some(m) = m.filter(|m| m.is_live() && app.voice.key.is_some()) else {
        return (StatusCode::CONFLICT, "No voice here").into_response();
    };
    if !has_say(app, m, who) {
        return (StatusCode::FORBIDDEN, "Not allowed").into_response();
    }
    // Looked up before anything is answered: the match is not held meanwhile.
    let player = v.player.and_then(|p| lock(&m.hub).steamid_of_player(stvtf::output::PlayerId(p)).map(str::to_owned));
    let steamid = match (v.player, v.steamid.as_deref()) {
        (Some(_), _) => match player {
            Some(steamid) => Some(steamid),
            None => return (StatusCode::BAD_REQUEST, "No such player").into_response(),
        },
        (None, Some(text)) => match parse_steamid(text) {
            Some(steamid) => Some(steamid),
            None => {
                return (StatusCode::BAD_REQUEST, "Invalid Steam ID").into_response();
            }
        },
        (None, None) => None,
    };
    if !m.comms.feed.put_on(&v.voice_id, steamid.clone()) {
        return (StatusCode::BAD_REQUEST, "Not found").into_response();
    }
    let caster = who.user.as_ref().is_some_and(|u| u.streamer);
    app.voice.who.remember(&v.voice_id, steamid.as_deref(), &m.comms.feed.name_of(&v.voice_id).unwrap_or_default(), caster);
    StatusCode::NO_CONTENT.into_response()
}

/// `POST <page>voice-match` on a demo with comms watched here: one of its voices that was no
/// player's is a player of it, a `SteamID`, or nobody, for this demo. The answer says where it
/// plays with that (`watch`): built anew.
fn post_review_match(app: &App, m: Option<&Session>, v: &Match, who: &Who) -> Response {
    let Some((dir, id, file)) = reviewed(app, m, who) else { return (StatusCode::FORBIDDEN, "Not allowed").into_response() };
    let player = v.player.and_then(|p| lock(&m?.hub).steamid_of_player(stvtf::output::PlayerId(p)).map(str::to_owned));
    let steamid = match (v.player, v.steamid.as_deref()) {
        (Some(_), _) if player.is_none() => return (StatusCode::BAD_REQUEST, "No such player").into_response(),
        (Some(_), _) => player,
        (None, Some(text)) => match parse_steamid(text) {
            Some(steamid) => Some(steamid),
            None => return (StatusCode::BAD_REQUEST, "Invalid Steam ID").into_response(),
        },
        (None, None) => None,
    };
    let key = format!("{}{}", crate::voice::feed::UNLINKED, v.voice_id);
    let Some((_, name)) = crate::matches::live_demos::voices_in(dir, id, file).into_iter().find(|(k, _)| *k == key) else {
        return (StatusCode::BAD_REQUEST, "Not found").into_response();
    };
    if crate::matches::live_demos::pick(dir, id, file, &key, steamid.as_deref()).is_err() {
        return (StatusCode::INTERNAL_SERVER_ERROR, "Not kept").into_response();
    }
    // For the next match they talk in too: a streamer's word for sure, a player's a suggestion.
    let caster = who.user.as_ref().is_some_and(|u| u.streamer);
    app.voice.who.remember(&v.voice_id, steamid.as_deref(), &name, caster);
    tracing::info!("demo {id} {file}: {} said to be {}", v.voice_id, steamid.as_deref().unwrap_or("nobody"));
    let watch = format!("{}/w/{id}/watch/{file}", app.base);
    Json(serde_json::json!({ "watch": watch })).into_response()
}
