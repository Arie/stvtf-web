//! Who is who in a channel brought into a match, and how that channel goes, for whoever has a say
//! in the match's voice (`voice_in::has_say`): on a match's page and on a room's.
//!
//! - `POST <page>voice-status`, the bot's (its key and the match's invite): how it goes in the
//!   channel it is in (`kind`: `discord` or `mumble`; `status`), who is in it (`people`), or
//!   that it left.
//! - `GET <page>voice-comms`: where the bot is, and how it goes.
//! - `GET <page>voices`: those in the channel and those heard, and whom each is put on.
//! - `POST <page>voice-match`: someone is a player of the match, a `SteamID`, or nobody.
//!
//! What someone says of who is who is remembered (`voice_known`), and holds when the same
//! person is in a channel again, in any region.

use super::visitor::Who;
use super::voice_in::{has_say, is_key, is_voice_id};
use super::voice_known::Recalled;
use super::{JSON, Request, TEXT, json_body, respond};
use crate::matches::sessions::Session;
use crate::matches::voice_feed::Put;
use crate::{App, lock};
use serde::Deserialize;
use std::io::{BufReader, Take};
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// What Steam's 64-bit ids of people start from.
const STEAM64_BASE: u64 = 76_561_197_960_265_728;
/// The most people a channel is said to have, and the longest a name or a status.
const MAX_PEOPLE: usize = 64;
const MAX_TEXT: usize = 100;
/// The bot says how Discord goes this often at least: past it, it is taken to be gone.
pub(crate) const DISCORD_SAID_FOR: Duration = Duration::from_secs(45);

/// A `SteamID` as people write them, as `[U:1:N]`: that itself, `STEAM_X:Y:Z`, the 64-bit number,
/// or a profile link with that number in it.
pub(crate) fn steamid_of(text: &str) -> Option<String> {
    let t = text.trim().trim_end_matches('/');
    let account = if let Some(n) = t.strip_prefix("[U:1:").and_then(|r| r.strip_suffix(']')).or_else(|| t.strip_prefix("U:1:")) {
        n.parse::<u32>().ok()?
    } else if let Some(rest) = t.to_ascii_uppercase().strip_prefix("STEAM_") {
        let mut parts = rest.split(':').map(str::parse::<u32>);
        let (_, y, z) = (parts.next()?.ok()?, parts.next()?.ok()?, parts.next()?.ok()?);
        if y > 1 || parts.next().is_some() {
            return None;
        }
        z.checked_mul(2)?.checked_add(y)?
    } else {
        let digits = t.rsplit('/').next()?;
        u32::try_from(digits.parse::<u64>().ok()?.checked_sub(STEAM64_BASE)?).ok()?
    };
    (account > 0).then(|| format!("[U:1:{account}]"))
}

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
    /// For Mumble: which asking for a channel (`MumbleJob::asked`) this answers.
    #[serde(default)]
    asked: Option<u64>,
}

/// The most channels a match keeps the people and the going of.
const MAX_CHANNELS: usize = 8;

/// `POST <page>voice-status`, the bot's.
pub(super) fn status(
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
    let channel = said.channel.filter(|c| is_voice_id(c)).unwrap_or_else(|| said.kind.clone());
    match said.kind.as_str() {
        "discord" => {
            let mut going = lock(&m.discord);
            going.retain(|_, (_, at)| at.elapsed() < DISCORD_SAID_FOR);
            if said.left {
                going.remove(&channel);
            } else if going.len() < MAX_CHANNELS || going.contains_key(&channel) {
                going.insert(channel.clone(), (text, Instant::now()));
            }
        }
        "mumble" => {
            let mut job = lock(&m.mumble);
            // Of a channel it was since asked out of: nothing of it stands.
            let Some(job) = job.as_mut().filter(|j| said.asked.is_none_or(|a| a == j.asked)) else {
                return respond(s, "204 No Content", TEXT, b"");
            };
            job.status = text;
        }
        _ => return respond(s, "400 Bad Request", TEXT, b"Discord or Mumble"),
    }
    if said.left || said.people.is_some() {
        let people: Vec<(String, String)> = said
            .people
            .unwrap_or_default()
            .into_iter()
            .filter(|p| is_voice_id(&p.id))
            .take(MAX_PEOPLE)
            .map(|p| (p.id, p.name.chars().filter(|c| !c.is_control()).take(MAX_TEXT).collect()))
            .collect();
        // Whom they were said to be before holds here too, until someone says otherwise.
        for (id, _) in &people {
            match app.voice_who.recall(id) {
                Some(Recalled::Player { steamid, sure }) => m.voice.remembered(id, Put::Player { steamid, confirmed: sure }),
                Some(Recalled::Nobody) => m.voice.remembered(id, Put::Nobody),
                None => {}
            }
        }
        m.voice.set_roster(&m.hub, &channel, people);
    }
    respond(s, "204 No Content", TEXT, b"")
}

/// The match whose voice `who` has a say in, if it is one and they have.
fn theirs<'a>(app: &App, m: Option<&'a Session>, who: &Who) -> Option<&'a Session> {
    m.filter(|m| m.is_live() && app.voice_key.is_some() && has_say(app, m, who))
}

/// `GET <page>voice-comms`: where the bot is, in Discord or Mumble, and how it goes. No content
/// for someone with no say.
pub(super) fn comms(s: &mut TcpStream, app: &App, m: Option<&Session>, who: &Who) -> std::io::Result<()> {
    let Some(m) = theirs(app, m, who) else { return respond(s, "204 No Content", TEXT, b"") };
    // Each Discord channel's going, one after the other.
    let going: Vec<String> =
        lock(&m.discord).values().filter(|(_, at)| at.elapsed() < DISCORD_SAID_FOR).map(|(status, _)| status.clone()).collect();
    let discord = (!going.is_empty()).then(|| serde_json::json!({ "status": going.join(" + ") }));
    let mumble = lock(&m.mumble)
        .as_ref()
        .map(|j| serde_json::json!({ "host": j.host, "port": j.port, "channel": j.channel, "delay": j.delay, "status": j.status }));
    respond(s, "200 OK", JSON, serde_json::json!({ "discord": discord, "mumble": mumble }).to_string().as_bytes())
}

/// `GET <page>voices`: those in the channel and those heard, and whom each is put on.
pub(super) fn voices(s: &mut TcpStream, app: &App, m: Option<&Session>, who: &Who) -> std::io::Result<()> {
    let Some(m) = theirs(app, m, who) else { return respond(s, "204 No Content", TEXT, b"") };
    respond(s, "200 OK", JSON, serde_json::json!(m.voice.voices(&m.hub)).to_string().as_bytes())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Match {
    discord: String,
    #[serde(default)]
    player: Option<u16>,
    #[serde(default)]
    steamid: Option<String>,
}

/// `POST <page>voice-match`: `discord` is the match's `player`, or `steamid` (as people write
/// them), or with neither nobody.
pub(super) fn voice_match(
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
    let player = v.player.and_then(|p| lock(&m.hub).steamid_of(stvtf::output::PlayerId(p)).map(str::to_owned));
    let steamid = match (v.player, v.steamid.as_deref()) {
        (Some(_), _) => match player {
            Some(steamid) => Some(steamid),
            None => return respond(s, "400 Bad Request", TEXT, b"No such player"),
        },
        (None, Some(text)) => match steamid_of(text) {
            Some(steamid) => Some(steamid),
            None => {
                return respond(s, "400 Bad Request", TEXT, b"Invalid Steam ID");
            }
        },
        (None, None) => None,
    };
    if !m.voice.set_match(&v.discord, steamid.clone()) {
        return respond(s, "400 Bad Request", TEXT, b"Not found");
    }
    let caster = who.user.as_ref().is_some_and(|u| u.streamer);
    app.voice_who.remember(&v.discord, steamid.as_deref(), &m.voice.name_of(&v.discord).unwrap_or_default(), caster);
    respond(s, "204 No Content", TEXT, b"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_steam_id_is_read_as_people_write_them() {
        for same in [
            "[U:1:9]",
            "U:1:9",
            "STEAM_0:1:4",
            "steam_1:1:4",
            " 76561197960265737 ",
            "https://steamcommunity.com/profiles/76561197960265737/",
        ] {
            assert_eq!(steamid_of(same).as_deref(), Some("[U:1:9]"), "{same}");
        }
        for none in [
            "",
            "gaben",
            "https://steamcommunity.com/id/gaben",
            "STEAM_0:2:4",
            "STEAM_0:1",
            "[U:1:x]",
            "76561197960265728",
            "123",
            "[U:1:0]",
        ] {
            assert_eq!(steamid_of(none), None, "{none}");
        }
    }
}
