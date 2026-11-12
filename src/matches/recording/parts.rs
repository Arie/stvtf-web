//! A match's parts: each map load of it, one after another in a recording.

use super::{Inner, Mark, Recording};
use crate::lock;
use stvtf::Line;

/// A map load of the match.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Part {
    /// Its number in the match, counting from 0, dropped ones too.
    pub(crate) n: usize,
    pub(crate) map: String,
    /// How many earlier parts of the match (dropped ones too) were of this map.
    pub(crate) again: usize,
    /// Play time of its first line, and of the first line kept of it.
    pub(crate) origin: u64,
    pub(crate) start: u64,
    /// Play time of the next part's first line; None for the latest.
    pub(crate) end: Option<u64>,
    /// A snapshot of it came: it has play to show.
    pub(crate) ready: bool,
}

impl Part {
    /// Whether play time `at` is in it.
    fn holds(&self, at: u64) -> bool {
        at >= self.origin && self.end.is_none_or(|end| at < end)
    }
}

/// What a recording notes of a part as it starts.
pub(super) struct PartStart {
    map: String,
    again: usize,
    origin: u64,
    ready: bool,
}

impl Inner {
    /// Notes `line`, at play time `at`, in the parts; true if it starts another part than the
    /// first. A server line naming another map does, and so does a snapshot whose tick is lower
    /// than the last one's (the server's ticks started over on the same map).
    pub(super) fn begin_part(&mut self, at: u64, line: &Line) -> bool {
        let known = lock(&self.scratch).map.clone();
        let named = if let Line::Server(server) = line { Some(server.map.as_str()) } else { None };
        let first = self.parts.is_empty();
        let later = !first
            && (named.zip(known.as_deref()).is_some_and(|(named, known)| named != known)
                || matches!(line, Line::Snapshot(s) if self.last_tick.is_some_and(|tick| s.tick < tick)));
        if first || later {
            let map = named.map(str::to_string).or(known).unwrap_or_default();
            let again = self.load(&map);
            self.parts.push(PartStart { map, again, origin: at, ready: false });
            self.scores = None;
            self.last_tick = None;
        } else if let Some(map) = named
            && self.parts.last().is_some_and(|p| p.map.is_empty())
        {
            // The first line was not a server line: this one names its map.
            let again = self.load(map);
            if let Some(part) = self.parts.last_mut() {
                part.map = map.to_string();
                part.again = again;
            }
        }
        if let Line::Snapshot(snapshot) = line {
            self.last_tick = Some(snapshot.tick);
            if let Some(part) = self.parts.last_mut() {
                part.ready = true;
            }
        }
        later
    }

    /// Counts a load of `map`; how many there were before.
    fn load(&mut self, map: &str) -> usize {
        if map.is_empty() {
            return 0;
        }
        let loads = self.names.entry(map.to_string()).or_default();
        *loads += 1;
        *loads - 1
    }

    /// Drops the parts whose chunks are all gone, the latest never.
    pub(super) fn drop_parts(&mut self) {
        let first = self.chunks.first().map_or(0, |c| c.start);
        let gone = self.parts.windows(2).take_while(|w| w[1].origin <= first).count();
        self.parts.drain(..gone);
        self.parts_dropped += gone;
    }

    /// The parts kept, oldest first.
    fn part_list(&self) -> Vec<Part> {
        let ends = self.parts.iter().skip(1).map(|p| Some(p.origin)).chain([None]);
        let list = self.parts.iter().zip(ends).enumerate().filter_map(|(i, (p, end))| {
            let first = self.chunks.partition_point(|c| c.start < p.origin);
            let start = self.chunks.get(first)?.start;
            let part = Part { n: self.parts_dropped + i, map: p.map.clone(), again: p.again, origin: p.origin, start, end, ready: p.ready };
            end.is_none_or(|end| start < end).then_some(part)
        });
        list.collect()
    }

    fn latest_part(&self) -> Part {
        let nameless = Part { n: 0, map: String::new(), again: 0, origin: 0, start: 0, end: None, ready: false };
        self.part_list().pop().unwrap_or(nameless)
    }

    fn rounds_of(&self, part: &Part) -> Vec<u64> {
        self.rounds.iter().copied().filter(|t| part.holds(*t)).collect()
    }
}

#[allow(dead_code, reason = "the rooms use them once they show maps")]
impl Recording {
    /// The parts kept, oldest first.
    pub(crate) fn parts(&self) -> Vec<Part> {
        lock(&self.inner).part_list()
    }

    /// Part `n`; none once dropped.
    pub(crate) fn part(&self, n: usize) -> Option<Part> {
        self.parts().into_iter().find(|p| p.n == n)
    }

    /// The latest part; a recording with no server line yet has a nameless one.
    pub(crate) fn latest(&self) -> Part {
        lock(&self.inner).latest_part()
    }

    /// Play times of the rounds' starts in part `n`.
    pub(crate) fn rounds_in(&self, n: usize) -> Vec<u64> {
        let inner = lock(&self.inner);
        inner.part_list().iter().find(|p| p.n == n).map(|p| inner.rounds_of(p)).unwrap_or_default()
    }

    /// Play times of the rounds' starts in the latest part.
    pub(super) fn latest_rounds(&self) -> Vec<u64> {
        let inner = lock(&self.inner);
        inner.rounds_of(&inner.latest_part())
    }

    /// Marks of part `n` from the `from`th of the match on, and how many the match has in all.
    pub(crate) fn marks_in(&self, n: usize, from: usize) -> (Vec<Mark>, usize) {
        let inner = lock(&self.inner);
        let all = inner.marks_dropped + inner.marks.len();
        let Some(part) = inner.part_list().into_iter().find(|p| p.n == n) else { return (Vec::new(), all) };
        let marks = inner.marks.iter().enumerate().filter(|(i, m)| inner.marks_dropped + i >= from && part.holds(m.at));
        (marks.map(|(_, m)| m.clone()).collect(), all)
    }

    /// Play time of the latest part's first line.
    pub(super) fn latest_origin(&self) -> u64 {
        lock(&self.inner).latest_part().origin
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{live_lines, push_at, to};
    use super::super::*;
    use crate::matches::hub::tests::snapshot;
    use crate::testing::server;

    const T0: u64 = 1_700_000_000_000;

    fn line(json: &str) -> Line {
        serde_json::from_str(json).unwrap()
    }

    /// `cp_a` for 24 s of play, then `cp_b`; the play time of the switch.
    fn two_maps() -> (Recording, u64) {
        let r = Recording::live();
        push_at(&r, &server("cp_a"), T0);
        let time = live_lines(&r, 47, T0 + 500);
        push_at(&r, &server("cp_b"), time);
        (r, time - T0)
    }

    /// A round started and won, a second apart, from `time`.
    fn a_round(r: &Recording, time: u64) {
        push_at(r, &line(r#"{"type":"round_start","full_reset":false,"tick":5}"#), time);
        push_at(r, &line(r#"{"type":"round_win","tick":5,"team":"red","reason":1,"round_time":9.0}"#), time + 1000);
    }

    fn two_maps_with_a_round_and_a_win_each() -> (Recording, u64) {
        let r = Recording::live();
        push_at(&r, &server("cp_a"), T0);
        a_round(&r, T0 + 1000);
        push_at(&r, &server("cp_b"), T0 + 3000);
        a_round(&r, T0 + 4000);
        (r, 3000)
    }

    /// Snapshots of ticks `from` to `to`, half a second apart from `time`; the time after.
    fn snapshots(r: &Recording, from: u32, to: u32, mut time: u64) -> u64 {
        for tick in (from..=to).step_by(50) {
            push_at(r, &snapshot(tick, None, Vec::new()), time);
            time += 500;
        }
        time
    }

    /// `cp_a` ticks 50..=1650, a Server line for `cp_a`, ticks going on from `again`.
    fn cp_a_twice(again: u32) -> Recording {
        let r = Recording::live();
        push_at(&r, &server("cp_a"), T0);
        let time = snapshots(&r, 50, 1650, T0 + 500);
        push_at(&r, &server("cp_a"), time);
        snapshots(&r, again, again + 500, time + 500);
        r
    }

    fn reconnected() -> Recording {
        cp_a_twice(1700)
    }

    fn reloaded() -> Recording {
        cp_a_twice(50)
    }

    fn two_maps_the_second_with_only_its_server_line() -> Recording {
        two_maps().0
    }

    fn push_snapshot(r: &Recording, tick: u32) {
        push_at(r, &snapshot(tick, None, Vec::new()), T0 + r.length() + 500);
    }

    #[test]
    fn a_new_map_starts_a_part_and_keeps_the_one_before() {
        let (r, at) = two_maps();
        let parts = r.parts();
        assert_eq!(parts.iter().map(|p| (p.n, p.map.as_str(), p.again)).collect::<Vec<_>>(), [(0, "cp_a", 0), (1, "cp_b", 0)]);
        assert_eq!((parts[0].origin, parts[0].end), (0, Some(at)));
        assert_eq!((parts[1].origin, parts[1].end), (at, None));
        assert_eq!(r.start(), 0, "nothing is dropped");
        assert_eq!(r.origin(), at, "the latest map's");
        assert!(r.seek(at - 1).0.map.as_deref() == Some("cp_a") && r.seek(at).0.map.as_deref() == Some("cp_b"));
    }

    #[test]
    fn rounds_and_marks_are_their_parts() {
        let (r, at) = two_maps_with_a_round_and_a_win_each();
        assert_eq!((r.rounds_in(0).len(), r.rounds_in(1).len()), (1, 1));
        assert!(r.rounds_in(0).iter().all(|t| *t < at) && r.rounds_in(1).iter().all(|t| *t >= at));
        assert_eq!(r.rounds(), r.rounds_in(1), "the latest part's");
        let (first, all) = r.marks_in(0, 0);
        assert!(!first.is_empty() && first.iter().all(|m| m.at < at) && all == r.marks(0).1);
        let (second, _) = r.marks_in(1, 0);
        assert!(!second.is_empty() && second.iter().all(|m| m.at >= at));
        assert_eq!(r.marks_in(1, all).0, [], "none new");
    }

    #[test]
    fn the_same_map_loaded_again_is_another_part_and_a_reconnect_is_not() {
        assert_eq!(reconnected().parts().len(), 1);
        let parts = reloaded().parts();
        assert_eq!(parts.iter().map(|p| (p.map.as_str(), p.again)).collect::<Vec<_>>(), [("cp_a", 0), ("cp_a", 1)]);
    }

    #[test]
    fn a_part_is_ready_at_its_first_snapshot() {
        let r = two_maps_the_second_with_only_its_server_line();
        assert!(!r.latest().ready);
        push_snapshot(&r, 50);
        assert!(r.latest().ready);
        assert_eq!(r.parts().len(), 2, "the new map's ticks start over without starting a part");
    }

    #[test]
    fn a_recording_without_a_server_line_has_a_nameless_part() {
        let r = Recording::read(r#"{"type":"stalemate","tick":50}"#.as_bytes(), "test").unwrap();
        assert_eq!((r.latest().n, r.latest().map.as_str()), (0, ""));
        assert_eq!(
            Recording::default().latest(),
            Part { n: 0, map: String::new(), again: 0, origin: 0, start: 0, end: None, ready: false }
        );
    }

    #[test]
    fn a_part_trimmed_away_is_dropped_and_its_number_stays_used() {
        let r = Recording::live();
        push_at(&r, &server("cp_a"), T0);
        let stalemate = line(r#"{"type":"stalemate","tick":5}"#);
        for s in 1..=60 {
            push_at(&r, &stalemate, T0 + s * 1000);
        }
        push_at(&r, &server("cp_b"), T0 + 61_000);
        for s in 62..=(KEEP_MS + TRIM_MS) / 1000 + 100 {
            push_at(&r, &stalemate, T0 + s * 1000);
        }
        assert_eq!(r.parts().iter().map(|p| p.n).collect::<Vec<_>>(), [1]);
        assert!(r.part(0).is_none() && r.part(1).is_some());
        assert_eq!(r.part(1).map(|p| (p.origin, p.start)), Some((61_000, r.start())), "its first line kept is the recording's");
        push_at(&r, &server("cp_c"), T0 + 99_999_000);
        assert_eq!(r.latest().n, 2);
    }

    #[test]
    fn a_new_part_does_not_rewrite_the_file() {
        let saved = Arc::new(Mutex::new(Vec::new()));
        let rewrites = Arc::new(Mutex::new(0));
        let mut file = to(&saved);
        let counted = rewrites.clone();
        let r = Recording::live_to(Box::new(move |save: &Save| {
            *lock(&counted) += usize::from(matches!(save, Save::All(_)));
            file(save);
        }));
        push_at(&r, &server("cp_a"), T0);
        let time = live_lines(&r, 45, T0 + 500);
        push_at(&r, &server("cp_b"), time);
        live_lines(&r, 45, time + 500);
        assert_eq!((r.parts().len(), *lock(&rewrites)), (2, 0));
    }
}
