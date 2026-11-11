//! Mumble for a match: `POST /w/<id>/voice-mumble` (a room's too) asks the voice bot into a
//! channel of a Mumble server (again, to move it: with no password, the one given before), or
//! sends it away (`{"leave": true}`). For whoever has a say in the match's voice
//! (`voice_in::has_say`). The bot asks all that is wanted (`GET /voice/mumble`, with its key);
//! how each goes it says to `voice_who`.

use super::visitor::Who;
use super::voice_in::{has_say, is_key};
use super::{JSON, Request, TEXT, json_body, respond};
use crate::matches::mumble_job::{DEFAULT_PORT, MumbleJob};
use crate::matches::sessions::{Session, Sessions};
use crate::matches::voice_feed::{DEFAULT_DELAY, MAX_DELAY};
use crate::{App, lock};
use serde::Deserialize;
use std::io::{BufReader, Take};
use std::net::TcpStream;
use std::sync::{Condvar, Mutex};
use std::time::Duration;

/// How long the bot is kept waiting for a change before it is answered anyway.
const WAIT: Duration = Duration::from_secs(25);

/// How often what is wanted changed, and the bell that says so: the bot waits on it, so it hears
/// of a channel asked for at once.
#[derive(Default)]
pub(crate) struct Changes(Mutex<u64>, Condvar);

impl Changes {
    fn changed(&self) {
        *lock(&self.0) += 1;
        self.1.notify_all();
    }

    /// How often it changed, once that is not `since` (or the wait is over).
    fn wait(&self, since: Option<u64>) -> u64 {
        let waited = self.1.wait_timeout_while(lock(&self.0), WAIT, |v| Some(*v) == since);
        *waited.unwrap_or_else(std::sync::PoisonError::into_inner).0
    }
}

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
}

/// A server's name or address: nothing of a link, a path or a port.
fn is_host(host: &str) -> bool {
    !host.is_empty() && host.len() <= 253 && host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
}

/// `POST /w/<id>/voice-mumble`.
pub(super) fn ask(
    s: &mut TcpStream,
    reader: &mut BufReader<Take<TcpStream>>,
    app: &App,
    m: Option<&Session>,
    request: &Request,
    who: &Who,
) -> std::io::Result<()> {
    let Some(m) = m.filter(|m| m.is_live() && app.voice_key.is_some()) else {
        return respond(s, "404 Not Found", TEXT, b"No voice comes into this match.");
    };
    if !has_say(app, m, who) {
        return respond(s, "403 Forbidden", TEXT, b"Only whoever started the match, and streamers, bring a Mumble channel in.");
    }
    let Ok(ask) = json_body::<Ask>(reader, request) else { return respond(s, "400 Bad Request", TEXT, b"a server and a channel") };
    if ask.leave {
        *lock(&m.mumble) = None;
        app.mumble_changes.changed();
        return respond(s, "204 No Content", TEXT, b"");
    }
    let (port, delay) = (ask.port.unwrap_or(DEFAULT_PORT), ask.delay.unwrap_or(DEFAULT_DELAY));
    let channel = ask.channel.trim();
    let fits = |t: &str| t.chars().count() <= MAX_TEXT && !t.chars().any(char::is_control);
    let before = lock(&m.mumble).as_ref().filter(|j| j.host == ask.host && j.port == port).map(|j| j.password.clone());
    let password = ask.password.or(before).unwrap_or_default();
    if !is_host(&ask.host) || port == 0 || channel.is_empty() || !fits(channel) || !fits(&password) || !(0.0..=MAX_DELAY).contains(&delay) {
        return respond(s, "400 Bad Request", TEXT, b"a server's name, its port, a channel, and a delay of 0 to 300 s");
    }
    let job = MumbleJob { host: ask.host, port, password, channel: channel.to_string(), delay, status: "on its way".into() };
    *lock(&m.mumble) = Some(job);
    app.mumble_changes.changed();
    respond(s, "204 No Content", TEXT, b"")
}

/// `GET /voice/mumble`, the bot's: every channel wanted, with what it takes to sit in it and to
/// post its voice, and the `version` of that. Asked with `?since=<version>` it waits (a while)
/// until something changed since.
pub(super) fn wanted(s: &mut TcpStream, app: &App, sessions: &Sessions, request: &Request) -> std::io::Result<()> {
    let Some(key) = &app.voice_key else { return respond(s, "404 Not Found", TEXT, b"") };
    if !is_key(key, request.bearer.as_deref()) {
        return respond(s, "401 Unauthorized", TEXT, b"");
    }
    let since = super::param(&request.query, "since").and_then(|v| v.parse::<u64>().ok());
    let version = app.mumble_changes.wait(since);
    let jobs: Vec<serde_json::Value> = sessions
        .live()
        .iter()
        .filter_map(|m| {
            let job = lock(&m.mumble).clone()?;
            Some(serde_json::json!({
                "match": m.id, "invite": m.voice_invite(),
                "host": job.host, "port": job.port, "password": job.password, "channel": job.channel, "delay": job.delay,
            }))
        })
        .collect();
    respond(s, "200 OK", JSON, serde_json::json!({ "jobs": jobs, "version": version }).to_string().as_bytes())
}
