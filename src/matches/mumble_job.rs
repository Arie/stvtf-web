//! A Mumble channel wanted for a match: where the voice bot is to sit, asked for on the match's
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
}
