use super::super::tests::{push_at, script, to};
use super::super::*;
use crate::matches::hub::tests::snapshot;
use crate::testing::server;

pub(crate) const T0: u64 = 1_700_000_000_000;

pub(crate) fn line(json: &str) -> Line {
    serde_json::from_str(json).unwrap()
}

pub(crate) fn status(state: &str) -> Line {
    line(&format!(r#"{{"type":"status","state":"{state}"}}"#))
}

/// Lines pushed at wall-clock times, seconds from [`T0`]: what the stream sends, when.
pub(crate) struct Stream<'a> {
    pub(crate) r: &'a Recording,
    pub(crate) s: f64,
}

impl Stream<'_> {
    /// `line`, `after` seconds on; its play time.
    pub(crate) fn push(&mut self, after: f64, line: &Line) -> u64 {
        self.s += after;
        push_at(self.r, line, T0 + (self.s * 1000.0).round() as u64);
        self.r.length()
    }

    pub(crate) fn status(&mut self, after: f64, state: &str) -> u64 {
        self.push(after, &status(state))
    }

    pub(crate) fn server(&mut self, after: f64, map: &str) -> u64 {
        self.push(after, &server(map))
    }

    pub(crate) fn snapshot(&mut self, after: f64, tick: u32) -> u64 {
        self.push(after, &snapshot(tick, None, Vec::new()))
    }

    /// Play: a snapshot a second, 67 ticks apart, from `from` for `seconds`; the last's play time.
    pub(crate) fn play(&mut self, from: u32, seconds: u32) -> u64 {
        let mut at = self.r.length();
        for s in 0..seconds {
            at = self.snapshot(1.0, from + s * 67);
        }
        at
    }

    /// Signon to `map`: `new`, its server line, `prespawn`, `spawn`, `full`; the server line's
    /// play time.
    pub(crate) fn signon(&mut self, after: f64, map: &str) -> u64 {
        self.status(after, "new");
        let at = self.server(0.0, map);
        self.status(1.9, "prespawn");
        self.status(0.05, "spawn");
        self.push(0.02, &line(&format!(r#"{{"type":"status","state":"full","map":"{map}"}}"#)));
        at
    }
}

/// `cp_a` for 24 s of play, then `cp_b`'s server line; the play time of that line.
fn two_maps() -> (Recording, u64) {
    let r = Recording::live();
    push_at(&r, &server("cp_a"), T0);
    let time = snapshots(&r, 50, 2350, T0 + 500);
    push_at(&r, &server("cp_b"), time);
    (r, time - T0)
}

/// Snapshots of ticks `from` to `to`, half a second apart from `time`; the time after.
pub(super) fn snapshots(r: &Recording, from: u32, to: u32, mut time: u64) -> u64 {
    for tick in (from..=to).step_by(50) {
        push_at(r, &snapshot(tick, None, Vec::new()), time);
        time += 500;
    }
    time
}

/// `cp_a`, then `cp_b` from a server line on, each with play and a round started and won.
fn two_maps_with_a_round_each() -> (Recording, u64) {
    let r = Recording::live();
    push_at(&r, &server("cp_a"), T0);
    let time = snapshots(&r, 50, 500, T0 + 500);
    a_round(&r, time);
    push_at(&r, &server("cp_b"), time + 2000);
    let time = snapshots(&r, 50, 500, time + 2500);
    a_round(&r, time);
    let origin = r.latest().origin;
    (r, origin)
}

/// A round started and won, a second apart, from `time`.
fn a_round(r: &Recording, time: u64) {
    push_at(r, &line(r#"{"type":"round_start","full_reset":false,"tick":5}"#), time);
    push_at(r, &line(r#"{"type":"round_win","tick":5,"team":"red","reason":1,"round_time":9.0}"#), time + 1000);
}

#[test]
fn a_recording_of_plain_lines_is_one_part() {
    let r = Recording::read(script(1650).as_bytes(), "test").unwrap();
    let part = r.latest();
    assert_eq!(r.parts(), std::slice::from_ref(&part));
    assert_eq!((part.load, part.origin, part.start, part.last), (0, 0, 0, r.length()));
    assert!(!part.over() && part.map.is_empty());
    assert_eq!(
        Recording::default().latest(),
        Part {
            load: 0,
            map: String::new(),
            origin: 0,
            start: 0,
            last: 0,
            end: None,
            changing: None,
            tail: None,
            ready: false,
            first_tick: None
        }
    );
}

#[test]
fn a_new_map_starts_a_part_at_its_first_snapshot() {
    let (r, at) = two_maps();
    assert_eq!(r.parts().len(), 1, "a server line alone shows nothing of it");
    assert_eq!(r.latest().map, "cp_a");
    push_at(&r, &snapshot(50, None, Vec::new()), T0 + at + 500);
    let parts = r.parts();
    assert_eq!(parts.iter().map(|p| (p.load, p.map.as_str())).collect::<Vec<_>>(), [(0, "cp_a"), (1, "cp_b")]);
    assert_eq!((parts[0].end, parts[1].origin, parts[1].first_tick), (Some(at + 500), at + 500, Some(50)));
    assert_eq!(parts[0].last, at - 500, "its last line that is play: the server line is the next map's");
    assert_eq!(r.start(), 0, "nothing is dropped");
    let (hub, ..) = r.seek(at + 500, None);
    assert_eq!((hub.map.as_deref(), hub.tick()), (Some("cp_b"), Some(50)), "a seek to its origin has its first snapshot");
}

#[test]
fn a_part_is_ready_a_second_of_ticks_past_its_first_snapshot() {
    let r = Recording::live();
    push_at(&r, &server("cp_a"), T0);
    push_at(&r, &snapshot(50, None, Vec::new()), T0 + 500);
    push_at(&r, &snapshot(100, None, Vec::new()), T0 + 1000);
    assert!(!r.latest().ready);
    push_at(&r, &snapshot(117, None, Vec::new()), T0 + 1500);
    assert!(r.latest().ready);
}

#[test]
fn rounds_and_marks_are_their_parts() {
    let (r, at) = two_maps_with_a_round_each();
    assert_eq!((r.rounds_in(0).len(), r.rounds_in(1).len()), (1, 1));
    assert!(r.rounds_in(0).iter().all(|t| *t < at) && r.rounds_in(1).iter().all(|t| *t >= at));
    let (first, all) = r.marks_in(0, 0);
    assert!(!first.is_empty() && first.iter().all(|m| m.at < at) && all == r.marks(0).1);
    let (second, _) = r.marks_in(1, 0);
    assert!(!second.is_empty() && second.iter().all(|m| m.at >= at));
    assert_eq!(r.marks_in(1, all).0, [], "none new");
}

#[test]
fn a_round_in_a_tail_is_the_next_maps() {
    // No frozen frame: the next map's round starts before its first snapshot.
    let r = Recording::live();
    let mut s = Stream { r: &r, s: 0.0 };
    s.server(0.0, "cp_a");
    s.play(50, 5);
    s.status(1.0, "changelevel");
    s.signon(5.0, "cp_b");
    let round = s.push(1.0, &line(r#"{"type":"round_start","full_reset":false,"tick":5}"#));
    s.push(0.0, &line(r#"{"type":"round_win","tick":5,"team":"red","reason":1,"round_time":9.0}"#));
    assert_eq!((r.rounds_in(0), r.parts().len()), (vec![], 1), "no part's till the next map shows");
    s.play(70, 3);
    assert_eq!(r.parts().len(), 2);
    assert!(r.part(0).unwrap().tail.is_some_and(|tail| tail < round) && r.latest().origin > round);
    assert_eq!((r.rounds_in(0), r.rounds_in(1)), (vec![], vec![round]));
    assert!(r.marks_in(0, 0).0.is_empty() && r.marks_in(1, 0).0.iter().any(|m| m.kind == "win" && m.at == round));
    assert!(r.part(0).unwrap().last < r.part(0).unwrap().tail.unwrap(), "an event in its tail is not where it ends");
}

#[test]
fn a_part_with_only_a_frozen_frame_holds_all_up_to_the_next() {
    let r = Recording::live();
    let mut s = Stream { r: &r, s: 0.0 };
    s.server(0.0, "cp_a");
    s.play(50, 5);
    s.status(1.0, "changelevel");
    s.signon(5.0, "cp_b");
    s.snapshot(6.0, 68);
    s.status(1.0, "changelevel");
    s.signon(5.0, "cp_c");
    let round = s.push(1.0, &line(r#"{"type":"round_start","full_reset":false,"tick":5}"#));
    s.play(70, 3);
    let frozen = r.part(1).unwrap();
    assert_eq!((r.parts().len(), frozen.ready, frozen.changing, frozen.tail), (3, false, None, None));
    assert_eq!((r.rounds_in(1), r.rounds_in(2)), (vec![round], vec![]), "never changing: no tail to hand on");
}

#[test]
fn a_recording_begun_mid_stream_takes_the_map_of_its_first_server_line() {
    let r = Recording::live();
    let mut s = Stream { r: &r, s: 0.0 };
    s.play(1000, 5);
    assert_eq!(r.latest().map, "");
    s.signon(1.0, "cp_a");
    s.play(1000 + 5 * 67, 5);
    let parts = r.parts();
    assert_eq!(parts.iter().map(|p| (p.load, p.map.as_str())).collect::<Vec<_>>(), [(0, "cp_a")], "a reconnect: no new load");
}

#[test]
fn ticks_starting_over_on_the_same_map_start_a_part_and_a_chunk() {
    let r = Recording::live();
    push_at(&r, &server("cp_a"), T0);
    let time = snapshots(&r, 50, 1650, T0 + 500);
    snapshots(&r, 50, 500, time);
    let part = r.latest();
    assert_eq!((r.parts().len(), part.load, part.map.as_str(), part.first_tick), (2, 1, "cp_a", Some(50)));
    assert_eq!(r.part(0).unwrap().tail, None, "no server line: no tail");
    let (hub, i, played) = r.seek(part.origin, None);
    assert_eq!((played, r.chunk(i).unwrap()[0].0), (1, part.origin), "its origin is the start of a chunk");
    assert_eq!(hub.tick(), Some(50), "and a seek there has its first snapshot, not the one before");
}

#[test]
fn a_part_trimmed_away_is_dropped_and_its_number_stays_used() {
    let r = Recording::live();
    push_at(&r, &server("cp_a"), T0);
    push_at(&r, &snapshot(50, None, Vec::new()), T0 + 500);
    let stalemate = line(r#"{"type":"stalemate","tick":5}"#);
    for s in 1..=60 {
        push_at(&r, &stalemate, T0 + s * 1000);
    }
    push_at(&r, &server("cp_b"), T0 + 61_000);
    push_at(&r, &snapshot(50, None, Vec::new()), T0 + 61_500);
    for s in 62..=(KEEP_MS + TRIM_MS) / 1000 + 100 {
        push_at(&r, &stalemate, T0 + s * 1000);
    }
    assert_eq!(r.parts().iter().map(|p| p.load).collect::<Vec<_>>(), [1]);
    assert!(r.part(0).is_none() && r.part(1).is_some());
    assert_eq!(r.part(1).map(|p| (p.origin, p.start)), Some((61_500, r.start())), "its first line kept is the recording's");
    push_at(&r, &server("cp_c"), T0 + 99_999_000);
    push_at(&r, &snapshot(50, None, Vec::new()), T0 + 99_999_500);
    assert_eq!(r.latest().load, 2);
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
    let time = snapshots(&r, 50, 1650, T0 + 500);
    push_at(&r, &server("cp_b"), time);
    snapshots(&r, 50, 1650, time + 500);
    assert_eq!((r.parts().len(), *lock(&rewrites)), (2, 0));
}
