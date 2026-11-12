//! A Mumble channel wanted for a match (there may be a few: each team's): where the voice bot is to sit, asked for on the match's
//! page by whoever has a say (see `web::voice_mumble`). The bot asks what is wanted and says
//! how it goes. Kept with the match, so a restart does not send the bot away.

use crate::lock;
use serde::{Deserialize, Serialize};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

/// Mumble's own port.
pub(crate) const DEFAULT_PORT: u16 = 64738;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct MumbleJob {
    pub(crate) host: String,
    pub(crate) port: u16,
    /// The server's password, if it has one: for the bot only.
    pub(crate) password: String,
    /// The channel's name.
    pub(crate) channel: String,
    /// How far the match's stream is behind the game, in seconds.
    pub(crate) delay: f64,
    /// How it goes, as the bot last said.
    #[serde(default)]
    pub(crate) status: String,
    /// Counts each time the bot is asked to join: a new number, and it joins anew.
    #[serde(default)]
    pub(crate) asked: u64,
    /// Which of the match's it is: its number, from 1, kept while it is moved.
    #[serde(default)]
    pub(crate) id: u32,
    /// What the bot calls its channel when it says who is there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) roster: Option<String>,
}

/// The most channels wanted for one match.
pub(crate) const MAX_JOBS: usize = 4;

/// A match's channels as they were saved: a list, or as it was before, one or none.
pub(crate) fn saved<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<MumbleJob>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Saved {
        Many(Vec<MumbleJob>),
        One(Option<MumbleJob>),
    }
    let mut jobs = match Saved::deserialize(d)? {
        Saved::Many(jobs) => jobs,
        Saved::One(job) => job.into_iter().collect(),
    };
    for (n, job) in jobs.iter_mut().enumerate() {
        if job.id == 0 {
            job.id = n as u32 + 1;
        }
    }
    Ok(jobs)
}

impl MumbleJob {
    /// Whether the bot sits in the channel, as it last said.
    pub(crate) fn is_in(&self) -> bool {
        !(self.status == "connecting" || self.status.starts_with("Could not join") || self.status.starts_with("Left"))
    }
}

/// How long the bot is kept waiting for a change before it is answered anyway.
const WAIT: Duration = Duration::from_secs(25);

/// How often what is wanted changed, and the bell that says so: the bot waits on it, so it hears
/// of a channel asked for at once.
#[derive(Default)]
pub(crate) struct Changes(Mutex<u64>, Condvar);

impl Changes {
    pub(crate) fn changed(&self) {
        *lock(&self.0) += 1;
        self.1.notify_all();
    }

    /// How often it changed, once that is not `since` (or the wait is over).
    pub(crate) fn wait(&self, since: Option<u64>) -> u64 {
        let waited = self.1.wait_timeout_while(lock(&self.0), WAIT, |v| Some(*v) == since);
        *waited.unwrap_or_else(std::sync::PoisonError::into_inner).0
    }
}

/// A server's name or address, out on the internet: nothing of a link, a path or a port, and
/// not this machine or a network of its own (the bot looks again at what a name leads to).
/// `anywhere`, on that network too: for a stv.tf and its bot run at a LAN.
pub(crate) fn is_host(host: &str, anywhere: bool) -> bool {
    if host.is_empty() || host.len() > 253 || !host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-') {
        return false;
    }
    if anywhere {
        return true;
    }
    if let Ok(ip) = host.parse::<std::net::Ipv4Addr>() {
        let [a, b, ..] = ip.octets();
        let own =
            ip.is_private() || ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() || ip.is_broadcast() || ip.is_multicast();
        return !(own || (a == 100 && (64..128).contains(&b)) || (a == 198 && (18..20).contains(&b)) || a == 0 || a >= 240);
    }
    let name = host.to_ascii_lowercase();
    name.contains('.') && !name.ends_with('.') && ![".local", ".internal", ".localhost", ".lan"].iter().any(|own| name.ends_with(own))
}

/// Puts `job` among those `wanted`: in the place of the one `at`, moved (those said to be in its
/// channel are `gone`; only a new delay for where it sits is no joining anew), or as one more.
/// Not when there are as many as a match may have.
pub(crate) fn place(wanted: &mut Vec<MumbleJob>, at: Option<usize>, mut job: MumbleJob, gone: &dyn Fn(&MumbleJob)) -> bool {
    let Some(at) = at else {
        if wanted.len() >= MAX_JOBS {
            return false;
        }
        wanted.push(job);
        return true;
    };
    let had = &wanted[at];
    let there = (&had.host, had.port, &had.password, &had.channel) == (&job.host, job.port, &job.password, &job.channel);
    if there && had.is_in() && had.delay.total_cmp(&job.delay).is_ne() {
        job = MumbleJob { delay: job.delay, ..had.clone() };
    } else {
        // Joined anew: who was there before is told again once it is in.
        gone(had);
        (job.id, job.asked) = (had.id, had.asked + 1);
    }
    wanted[at] = job;
    true
}
