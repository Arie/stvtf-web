//! A Mumble channel wanted for a match (there may be a few: each team's): where the voice bot is to sit, asked for on the match's
//! page by whoever has a say (see `web::voice_mumble`). The bot asks what is wanted and says
//! how it goes. Kept with the match, so a restart does not send the bot away.

use serde::{Deserialize, Serialize};

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
