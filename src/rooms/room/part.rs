//! The map of its match a room plays.

use super::Room;
use super::play::State;
use crate::lock;
use crate::matches::recording::Part;
use anyhow::{Result, bail};
use serde::Deserialize;

/// The map an admin takes the room to, by number.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MapPost {
    pub(crate) map: usize,
}

impl Room {
    /// An admin moves the room to map `m.map`: at live on the latest, at its start on another.
    pub(crate) fn go(&self, m: &MapPost) -> Result<()> {
        let Some(part) = self.recording.part(m.map) else { bail!("That map is gone.") };
        let mut st = lock(&self.state);
        // At live it follows again (`at_end`); anywhere else it waits at its map's end.
        (st.part, st.follows) = (part.n, false);
        if self.recording.is_live() && part.end.is_none() {
            (st.paused, st.seek) = (false, Some(self.recording.length()));
        } else {
            st.seek = Some(part.start);
        }
        self.wake.notify_all();
        Ok(())
    }

    /// Someone did something in the room: it no longer moves by itself.
    pub(super) fn touched(&self) {
        lock(&self.state).follows = false;
    }

    /// A room following live goes on to the next map once that has play, at live, at 1x: true
    /// when it did (the seek there is due).
    pub(super) fn move_on(&self, st: &mut State) -> bool {
        let Some(next) = self.recording.part(st.part + 1).filter(|p| st.follows && p.ready) else { return false };
        let at = next.end.unwrap_or_else(|| self.recording.length());
        (st.part, st.paused, st.speed, st.seek) = (next.n, false, 1.0, Some(at));
        true
    }

    /// The part the room is on, or the earliest kept if that was dropped.
    pub(super) fn part(&self, st: &State) -> Part {
        self.placed(st).0
    }

    /// Whether the room waits at the end of a part that has a successor.
    pub(super) fn part_over(&self, st: &State) -> bool {
        self.reach(st).1.is_some()
    }

    /// Where the room may be, from the start of its part to its end: none while it is the latest
    /// (it goes as far as the recording does).
    pub(super) fn reach(&self, st: &State) -> (u64, Option<u64>) {
        let part = self.part(st);
        (part.start, part.end)
    }

    /// A room whose part was dropped (the trim) goes to the earliest kept, at its start, paused:
    /// it does not play another map by itself.
    pub(super) fn rehome(&self, st: &mut State) {
        let (part, dropped) = self.placed(st);
        if dropped {
            (st.part, st.paused, st.seek, st.follows) = (part.n, true, Some(part.start), false);
        }
    }

    /// The part the room is on, or the earliest kept (true) if that was dropped.
    fn placed(&self, st: &State) -> (Part, bool) {
        if let Some(part) = self.recording.part(st.part) {
            return (part, false);
        }
        self.recording.parts().into_iter().next().map_or_else(|| (self.recording.latest(), false), |p| (p, true))
    }

    /// `at`, held inside where the room may be.
    pub(super) fn held(&self, st: &State, at: u64) -> u64 {
        let (start, end) = self.reach(st);
        at.clamp(start, end.unwrap_or_else(|| self.recording.length().max(start)))
    }
}
