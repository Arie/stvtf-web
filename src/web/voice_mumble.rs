//! Mumble for a match: `POST /w/<id>/voice-mumble` (a room's too) asks the voice bot into a channel
//! of a Mumble server: one more (each team's, on one server or on several), or with the `id` of one
//! it is in, to move that (with no password, the one given before for the server). `{"leave":
//! true}` sends it away from all, with an `id` from that one. For whoever has a say in the match's
//! voice (`voice_in::has_say`). The bot asks all that is wanted (`GET /voice/mumble`, with its
//! key); how each goes it says to `voice_who`.

use super::visitor::Who;
use super::voice_in::{has_say, is_key};
use super::{JSON, Request, TEXT, json_body, respond};
use crate::matches::sessions::{Session, Sessions};
use crate::voice::feed::{DEFAULT_DELAY, MAX_DELAY};
use crate::voice::mumble::{DEFAULT_PORT, MumbleJob, is_host, place};
use crate::{App, lock};
use serde::Deserialize;
use std::io::{BufReader, Take};
use std::net::TcpStream;

/// The longest a channel's name, a password or a status is, in characters.
const MAX_TEXT: usize = 100;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ask {
    #[serde(default)]
    host: String,
    #[serde(default)]
    port: Option<u16>,
    /// With none, the one given before for this server.
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    channel: String,
    #[serde(default)]
    delay: Option<f64>,
    #[serde(default)]
    leave: bool,
    /// Which of the match's channels: none for one more.
    #[serde(default)]
    id: Option<u32>,
}

/// `POST /w/<id>/voice-mumble`.
pub(super) fn post_mumble(
    s: &mut TcpStream,
    reader: &mut BufReader<Take<TcpStream>>,
    app: &App,
    m: Option<&Session>,
    request: &Request,
    who: &Who,
) -> std::io::Result<()> {
    let Some(m) = m.filter(|m| m.is_live() && app.voice_key.is_some()) else {
        return respond(s, "404 Not Found", TEXT, b"No voice here");
    };
    if !has_say(app, m, who) {
        return respond(s, "403 Forbidden", TEXT, b"Not allowed");
    }
    let Ok(ask) = json_body::<Ask>(reader, request) else { return respond(s, "400 Bad Request", TEXT, b"Invalid") };
    if ask.leave {
        leave_mumble(m, ask.id);
        app.mumble_changes.changed();
        return respond(s, "204 No Content", TEXT, b"");
    }
    // Decided with the match held, answered once it is not.
    if let Err(why) = want(app, m, ask) {
        return respond(s, "400 Bad Request", TEXT, why.as_bytes());
    }
    app.mumble_changes.changed();
    respond(s, "204 No Content", TEXT, b"")
}

/// The bot leaves match `m`'s Mumble channel `id`, or with none all of them.
fn leave_mumble(m: &Session, id: Option<u32>) {
    let left: Vec<MumbleJob> = {
        let mut wanted = lock(&m.mumble);
        let (left, stay) = wanted.drain(..).partition(|j| id.is_none_or(|id| id == j.id));
        *wanted = stay;
        left
    };
    for j in &left {
        drop_roster(m, j);
    }
}

/// Those the bot said are in `job`'s channel go with it.
fn drop_roster(m: &Session, job: &MumbleJob) {
    if let Some(roster) = &job.roster {
        m.voice.set_roster(&m.hub, roster, []);
    }
}

/// The channel `ask` names wanted for match `m`: the one named, else the one in that channel
/// already, else one more. Why not, when it will not do.
fn want(app: &App, m: &Session, ask: Ask) -> Result<(), &'static str> {
    let (port, delay) = (ask.port.unwrap_or(DEFAULT_PORT), ask.delay.unwrap_or(DEFAULT_DELAY));
    let channel = ask.channel.trim();
    let fits = |t: &str| t.chars().count() <= MAX_TEXT && !t.chars().any(char::is_control);
    let mut wanted = lock(&m.mumble);
    let at = wanted.iter().position(|j| match ask.id {
        Some(id) => id == j.id,
        None => (j.host.as_str(), j.port, j.channel.as_str()) == (ask.host.as_str(), port, channel),
    });
    let on_server = |j: &&MumbleJob| j.host == ask.host && j.port == port;
    let before = at.map(|at| &wanted[at]).filter(on_server).or_else(|| wanted.iter().find(on_server)).map(|j| j.password.clone());
    let password = ask.password.or(before).unwrap_or_default();
    let bad = !is_host(&ask.host, app.mumble_anywhere) || port == 0 || channel.is_empty() || !fits(channel) || !fits(&password);
    if bad || !(0.0..=MAX_DELAY).contains(&delay) || (ask.id.is_some() && at.is_none()) {
        return Err("Invalid server, channel or delay");
    }
    let job = MumbleJob {
        host: ask.host,
        port,
        password,
        channel: channel.to_string(),
        delay,
        status: "connecting".into(),
        asked: 1,
        id: wanted.iter().map(|j| j.id).max().unwrap_or(0) + 1,
        roster: None,
    };
    if !place(&mut wanted, at, job, &|j| drop_roster(m, j)) {
        return Err("Too many channels");
    }
    Ok(())
}

/// `GET /voice/mumble`, the bot's: every channel wanted, with what it takes to sit in it and to
/// post its voice, and the `version` of that. Asked with `?since=<version>` it waits (a while)
/// until something changed since.
pub(super) fn get_mumble_jobs(s: &mut TcpStream, app: &App, sessions: &Sessions, request: &Request) -> std::io::Result<()> {
    let Some(key) = &app.voice_key else { return respond(s, "404 Not Found", TEXT, b"") };
    if !is_key(key, request.bearer.as_deref()) {
        return respond(s, "401 Unauthorized", TEXT, b"");
    }
    let since = super::param(&request.query, "since").and_then(|v| v.parse::<u64>().ok());
    let version = app.mumble_changes.wait(since);
    let jobs: Vec<serde_json::Value> = sessions
        .live()
        .iter()
        .flat_map(|m| {
            let jobs = lock(&m.mumble).clone();
            jobs.into_iter()
                .map(|job| {
                    serde_json::json!({
                        "match": m.id, "invite": m.voice_invite(), "job": job.id,
                        "host": job.host, "port": job.port, "password": job.password, "channel": job.channel, "delay": job.delay, "asked": job.asked,
                    })
                })
                .collect::<Vec<_>>()
        })
        .collect();
    respond(s, "200 OK", JSON, serde_json::json!({ "jobs": jobs, "version": version }).to_string().as_bytes())
}
