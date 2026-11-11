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
//! What someone says of who is who is remembered (`voice-who.json` in the store), and holds
//! when the same person is in a channel again.

use super::visitor::Who;
use super::voice_in::{has_say, is_key, is_voice_id};
use super::{JSON, Request, TEXT, json_body, respond};
use crate::maps::cache::write_atomic;
use crate::matches::sessions::Session;
use crate::{App, lock};
use serde::Deserialize;
use std::collections::HashMap;
use std::io::{BufReader, Take};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// What Steam's 64-bit ids of people start from.
const STEAM64_BASE: u64 = 76_561_197_960_265_728;
/// The most people a channel is said to have, and the longest a name or a status.
const MAX_PEOPLE: usize = 64;
const MAX_TEXT: usize = 100;
/// The bot says how Discord goes this often at least: past it, it is taken to be gone.
pub(crate) const DISCORD_SAID_FOR: Duration = Duration::from_secs(45);
const FILE: &str = "voice-who.json";

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

/// Whom people were said to be, from one match to the next: who they are where they talk, and
/// the `SteamID` someone put them on.
#[derive(Default)]
pub(crate) struct WhoHistory {
    file: Option<PathBuf>,
    known: Mutex<HashMap<String, String>>,
}

impl WhoHistory {
    pub(crate) fn new(dir: Option<&Path>) -> Self {
        let file = dir.map(|d| d.join(FILE));
        let known = file.as_ref().and_then(|f| std::fs::read(f).ok()).and_then(|d| serde_json::from_slice(&d).ok()).unwrap_or_default();
        Self { file, known: Mutex::new(known) }
    }

    fn get(&self, who: &str) -> Option<String> {
        lock(&self.known).get(who).cloned()
    }

    /// `who` is `steamid`, or with none is nobody any more.
    fn remember(&self, who: &str, steamid: Option<&str>) {
        let mut known = lock(&self.known);
        match steamid {
            Some(s) => known.insert(who.to_string(), s.to_string()),
            None => known.remove(who),
        };
        if let Some(file) = &self.file
            && let Err(e) = write_atomic(file, serde_json::json!(*known).to_string().as_bytes())
        {
            eprintln!("stvtf-web: who is who not kept: {e}");
        }
    }
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
}

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
    match said.kind.as_str() {
        "discord" => *lock(&m.discord) = (!said.left).then(|| (text, Instant::now())),
        "mumble" => {
            if let Some(job) = lock(&m.mumble).as_mut() {
                job.status = text;
            }
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
            if let Some(steamid) = app.voice_who.get(id) {
                m.voice.remembered(id, steamid);
            }
        }
        m.voice.set_roster(&m.hub, people);
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
    let discord = lock(&m.discord)
        .as_ref()
        .filter(|(_, at)| at.elapsed() < DISCORD_SAID_FOR)
        .map(|(status, _)| serde_json::json!({ "status": status }));
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
    let steamid = match (v.player, v.steamid.as_deref()) {
        (Some(p), _) => match lock(&m.hub).steamid_of(stvtf::output::PlayerId(p)).map(str::to_owned) {
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
    app.voice_who.remember(&v.discord, steamid.as_deref());
    respond(s, "204 No Content", TEXT, b"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

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

    #[test]
    fn who_is_who_is_kept_and_read_back() {
        let dir = TempDir::new("voicewho");
        let who = WhoHistory::new(Some(&dir));
        who.remember("mumble:x:user:1", Some("[U:1:5]"));
        who.remember("77", Some("[U:1:6]"));
        who.remember("77", None);
        let again = WhoHistory::new(Some(&dir));
        assert_eq!((again.get("mumble:x:user:1").as_deref(), again.get("77")), (Some("[U:1:5]"), None));
    }
}
