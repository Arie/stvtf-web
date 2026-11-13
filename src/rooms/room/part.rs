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
    /// An admin moves the room to map `m.map`: at live on the latest, at its first snapshot on an
    /// earlier one; its own map, to its end or live, following again (the page's Live).
    pub(crate) fn go(&self, m: &MapPost) -> Result<()> {
        let mut st = lock(&self.state);
        let own = m.map == self.part(&st).n;
        let Some(part) = self.recording.part(m.map).filter(|p| p.ready || own) else { bail!("Map gone") };
        let latest = part.n == self.recording.latest().n;
        if own || latest {
            let over = self.end_of(&part).is_some();
            (st.paused, st.speed, st.seek) = (false, 1.0, Some(self.end_of(&part).unwrap_or_else(|| self.recording.length())));
            // Its own map over: it goes on when the next plays. The latest: decided when it ends.
            st.follows = own && over;
            st.over = (own && over).then_some(part.n);
        } else {
            // Where its map shows: its first snapshot, nothing of the map before. It waits there.
            (st.seek, st.follows, st.over) = (Some(part.origin.max(part.start)), false, Some(part.n));
        }
        st.part = part.n;
        // Following again with a later map playing: straight there, not to this one's end first.
        self.move_on(&mut st);
        self.wake.notify_all();
        Ok(())
    }

    /// Someone did something in the room: it no longer moves by itself.
    pub(super) fn touched(&self) {
        lock(&self.state).follows = false;
    }

    /// A room following goes on to the newest map with play (past any that never had), at live,
    /// at 1x: true when it did (the seek there is due).
    pub(super) fn move_on(&self, st: &mut State) -> bool {
        if !st.follows {
            return false;
        }
        let Some(next) = self.recording.parts().into_iter().rfind(|p| p.n > st.part && p.ready) else { return false };
        let at = self.end_of(&next).unwrap_or_else(|| self.recording.length());
        (st.part, st.paused, st.speed, st.seek, st.follows, st.over) = (next.n, false, 1.0, Some(at), false, None);
        true
    }

    /// The part the room is on, or the earliest kept if that was dropped.
    pub(super) fn part(&self, st: &State) -> Part {
        self.placed(st).0
    }

    /// Whether the room waits at the end of its part: one that has a successor, or the server is
    /// changing from while the match goes on (or did as it ended).
    pub(super) fn part_over(&self, st: &State) -> bool {
        self.end_of(&self.part(st)).is_some()
    }

    /// Where `part` ends for a room on it: its last line not tail, once over. A recording that
    /// ends as the server said it changes map, and no more, ends as one without it.
    pub(super) fn end_of(&self, part: &Part) -> Option<u64> {
        let ended = part.end.is_some() || part.tail.is_some() || self.recording.is_live();
        (part.over() && ended).then_some(part.last)
    }

    /// Where the room may be, from where its part shows (its first snapshot, for one after
    /// another: not the map before) to its end: none while it goes on (it goes as far as the
    /// recording does).
    pub(super) fn reach(&self, st: &State) -> (u64, Option<u64>) {
        let part = self.part(st);
        (part.start, self.end_of(&part))
    }

    /// A room whose part was dropped (the trim) goes to the earliest kept, where it shows, paused:
    /// it does not play another map by itself.
    pub(super) fn rehome(&self, st: &mut State) {
        let (part, dropped) = self.placed(st);
        if dropped {
            (st.part, st.paused, st.seek, st.follows, st.over) = (part.n, true, Some(part.start), false, Some(part.n));
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

    /// The map the room is on: its geometry may be built.
    pub(crate) fn map(&self) -> String {
        self.part(&lock(&self.state)).map
    }
}
