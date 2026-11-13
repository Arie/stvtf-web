//! A room at the end of its map while the server changes map: whether it goes on with the match,
//! decided when the map ends, and the voice of the gap it hears while it waits.

use super::Room;
use super::play::{Cursor, IDLE_POLL, LIVE_POLL, State};
use crate::matches::hub::publish;
use std::time::Duration;
use stvtf::Line;
use stvtf::output::Status;

/// A room this close to its map's end when the map ends (in play time) is at live: it follows.
pub(super) const FOLLOW_WITHIN: u64 = 3000;

/// Whether `line` is the server saying it changes map: never played to a room (the map it ends
/// stays showing; a room goes to the next by a seek).
pub(super) fn changes_map(line: &Line) -> bool {
    matches!(line, Line::Status(Status::Changelevel | Status::MapChange))
}

/// Whether `line` is voice from outside the match, which a room waiting in the gap hears (what was
/// said in a pause neither its pages nor its relay take).
fn heard_in_the_gap(line: &Line) -> bool {
    matches!(line, Line::Voice(v) if v.injected)
}

impl Room {
    /// Looks whether the room's map ended: the first time it sees it over, it follows if it was at
    /// live then (playing, at 1×, near the end); a map that went on after all is not over, and
    /// that is decided afresh. A room following goes on: true when it did (the seek is due).
    pub(super) fn at_map_end(&self, st: &mut State) -> bool {
        let part = self.part(st);
        let over = self.end_of(&part).is_some();
        if over && st.over != Some(part.n) {
            let near = part.last.saturating_sub(st.at) <= FOLLOW_WITHIN;
            st.follows = !st.paused && (st.speed - 1.0).abs() < f64::EPSILON && near;
            st.over = Some(part.n);
        } else if !over && st.over.is_some() {
            (st.follows, st.over) = (false, None);
        }
        over && self.move_on(st)
    }

    /// The room waits at the end of its map: its tail is read as it comes and dropped, but for
    /// voice from outside the match, played at once to its pages and relay. What the tail had when
    /// the room got there is not: that was said before. How long to wait for more.
    pub(super) fn hear_the_gap(&self, st: &mut State, cur: &mut Cursor) -> Duration {
        let rec = &self.recording;
        let first = !cur.waiting;
        loop {
            cur.refill(rec);
            // Looked at after the lines: a late frame of the map's own moves its end, and plays.
            let part = self.part(st);
            let Some(end) = self.end_of(&part) else { break };
            let Some(t) = cur.lines.front().map(|l| l.0) else { break };
            if t <= end || part.end.is_some_and(|next| t >= next) {
                break;
            }
            let (_, line) = cur.lines.pop_front().expect("a front line");
            cur.done += 1;
            if !first && heard_in_the_gap(&line) {
                publish(&self.hub, &line);
                self.relay_line(&line);
            }
        }
        cur.waiting = true;
        let part = self.part(st);
        if part.end.is_some() || !rec.is_live() {
            if !rec.is_live() && part.end.is_none() && !st.paused {
                // A recording that ended in the gap: nothing more comes.
                st.paused = true;
                self.announce(st);
            }
            return IDLE_POLL;
        }
        LIVE_POLL
    }
}
