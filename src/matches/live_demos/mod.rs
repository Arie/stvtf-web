//! A live match's `.dem` files, recorded as it is watched so rooms on it can be relayed to TF2
//! clients: one per connection to its server (a reconnect, a new map, a deploy starts another),
//! `live-<match>-<load>-<tick>.dem`: the server tick of its first frame, and the load it is of,
//! its part's number in the recording (one rule, [`Loads`](crate::matches::loads::Loads),
//! fed the lines the recording gets).
//! Those of earlier loads stay, so rooms on them can be relayed, till the recording drops their
//! part or the match ends. Names without a load, `live-<match>-<tick>.dem`, are given loads by
//! their order in time.
//!
//! Voice from outside the match that its players said (a team's Discord) is kept beside the demo
//! it was heard in (`<demo>.voice`, a line each). A demo with any is not dropped but kept
//! (`kept-…`, once no room can be on it: its part dropped, or the match ended), for a while and
//! within a budget, to download with the comms written into it:
//! [`downloads()`](self::downloads::downloads), [`build`],
//! [`sweep_kept`]. Till then it is downloaded under its `live-` name.

mod comms;
mod downloads;
mod store;
mod sweep;

pub(crate) use self::comms::{By, Spoken, build, comms_key, pick, picks, voices_in};
pub(crate) use self::downloads::downloads;
pub(crate) use self::store::LiveDemos;
pub(crate) use self::sweep::{is_live_demo, sweep_kept, sweep_live};

#[cfg(test)]
mod tests;
