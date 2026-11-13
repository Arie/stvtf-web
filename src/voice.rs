//! Team voice comms: a team's Discord or Mumble brought into a match, and who is who in it.

pub(crate) mod feed;
pub(crate) mod hold;
pub(crate) mod ids;
pub(crate) mod links;
pub(crate) mod mumble;
pub(crate) mod who;

use crate::voice::feed::VoiceFeed;
use crate::voice::mumble::MumbleJob;
use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// A match's team voice comms.
#[derive(Default)]
pub(crate) struct MatchComms {
    /// Voice from outside the match (a team's Discord, Mumble), and whether a thread tends it.
    pub(crate) feed: VoiceFeed,
    pub(crate) tended: AtomicBool,
    /// What a Discord channel's voice comes in with: given to whoever started the match, and to
    /// the site's streamers, to pass on with the bot's command.
    pub(crate) invite: OnceLock<String>,
    /// The Mumble channels the voice bot is wanted in for it.
    pub(crate) mumble: Mutex<Vec<MumbleJob>>,
    /// How it goes in each Discord channel the voice bot is in for it, and when it last said.
    pub(crate) discord: Mutex<BTreeMap<String, (String, Instant)>>,
}

/// What the server's voice comms are set up with.
#[derive(Default)]
pub(crate) struct VoiceSetup {
    /// The key the voice bot posts with (`STVTF_VOICE_KEY`); without it no voice is taken in.
    pub(crate) key: Option<String>,
    /// Discord users and the Steam accounts linked to them, for voice from Discord.
    pub(crate) links: links::Links,
    /// Where the voice bot is added to a Discord server (`STVTF_VOICE_BOT`), for the invite.
    pub(crate) bot: Option<String>,
    /// The bell the voice bot waits on for Mumble channels asked for.
    pub(crate) mumble_changes: mumble::Changes,
    /// Whether a Mumble server may be on this machine or its network (`STVTF_MUMBLE_ANYWHERE=1`):
    /// at a LAN.
    pub(crate) mumble_anywhere: bool,
    /// Whom people in voice channels were said to be, from one match to the next.
    pub(crate) who: who::WhoIsWho,
}
