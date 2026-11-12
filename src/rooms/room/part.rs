//! The map of its match a room plays.

use super::Room;
use super::play::State;
use crate::matches::recording::Part;

impl Room {
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
            (st.part, st.paused, st.seek) = (part.n, true, Some(part.start));
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
