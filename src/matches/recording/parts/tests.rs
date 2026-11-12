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
    two_maps_to(Box::new(|_: &Save| {}))
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
    assert_eq!(Recording::default().latest(), Part { n: 0, map: String::new(), again: 0, origin: 0, start: 0, end: None, ready: false });
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

impl Recording {
    /// The parts as a resume can know them: none begun in the open chunk, the last one unended.
    fn parts_closed(&self) -> Vec<Part> {
        let inner = lock(&self.inner);
        let open = if inner.open.is_empty() { u64::MAX } else { inner.chunks.last().map_or(u64::MAX, |c| c.start) };
        let mut parts: Vec<Part> = inner.part_list().into_iter().filter(|p| p.origin < open).collect();
        if let Some(p) = parts.last_mut() {
            p.end = None;
        }
        parts
    }
}

/// What the file holds now.
fn file_of(r: &Recording) -> Vec<u8> {
    lock(&r.inner).file().unwrap()
}

/// A file as the version before parts wrote it: only `origin`, `marks` and `hub` in the header.
fn old_file() -> Vec<u8> {
    let r = Recording::live();
    push_at(&r, &server("cp_a"), T0);
    live_lines(&r, 45, T0 + 500);
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    let inner = lock(&r.inner);
    let hub: Vec<_> = inner.chunks[0].hub.state().collect();
    write!(gz, "origin\t0\nmarks\t0\n").unwrap();
    for state in hub {
        writeln!(gz, "hub\t{state}").unwrap();
    }
    let mut file = gz.finish().unwrap();
    file.extend_from_slice(&inner.chunks[0].lines);
    file
}

fn two_maps_to(sink: Sink) -> (Recording, u64) {
    let r = Recording::live_to(sink);
    push_at(&r, &server("cp_a"), T0);
    let time = live_lines(&r, 47, T0 + 500);
    push_at(&r, &server("cp_b"), time);
    (r, time - T0)
}

/// Maps `(name, minutes in it)` one after another, a stalemate a second, till 3 h 11 min of play.
fn long(maps: &[(&str, u64)]) -> Recording {
    let r = Recording::live();
    let stalemate = line(r#"{"type":"stalemate","tick":5}"#);
    let mut at = 0;
    for (map, minutes) in maps {
        push_at(&r, &server(map), T0 + at);
        let end = at + minutes * 60_000;
        for s in (at / 1000 + 1)..=(end / 1000) {
            push_at(&r, &stalemate, T0 + s * 1000);
        }
        at = end;
    }
    r
}

/// `cp_a` for 10 minutes, `cp_b` until 3 h 11 min of play in all.
fn long_two_maps() -> Recording {
    long(&[("cp_a", 10), ("cp_b", 181)])
}

#[test]
fn a_resumed_recording_has_the_parts_it_had() {
    let saved = Arc::default();
    let (r, _) = two_maps_to(to(&saved));
    let again = Recording::resume(lock(&saved).as_slice(), to(&saved)).unwrap();
    assert_eq!(again.parts(), r.parts_closed(), "all but what the open chunk held");
    assert_eq!(again.marks(0).1, r.marks(0).1);
}

#[test]
fn the_trim_drops_a_part_with_nothing_left_and_its_number_stays_gone() {
    let r = long_two_maps();
    let parts = r.parts();
    assert_eq!(parts.iter().map(|p| p.n).collect::<Vec<_>>(), [1]);
    assert_eq!(r.part(0), None);
    assert!(parts[0].start > parts[0].origin, "its own start is trimmed too");
    let again = Recording::resume(file_of(&r).as_slice(), Box::new(|_: &Save| {})).unwrap();
    assert_eq!(again.latest().n, 1, "numbers go on past the dropped");
    assert_eq!(again.latest().origin, parts[0].origin, "and so does where the map began");
    assert_eq!(again.latest().start, parts[0].start);
}

#[test]
fn loads_dropped_still_count_after_a_restart() {
    let r = long(&[("cp_a", 5), ("cp_b", 4), ("cp_a", 182)]);
    assert_eq!(r.parts().iter().map(|p| (p.n, p.again)).collect::<Vec<_>>(), [(2, 1)]);
    let again = Recording::resume(file_of(&r).as_slice(), Box::new(|_: &Save| {})).unwrap();
    assert_eq!((again.latest().n, again.latest().map.as_str(), again.latest().again), (2, "cp_a", 1));
    let t = T0 + again.length() + 1000;
    push_at(&again, &server("cp_b"), t);
    assert_eq!((again.latest().n, again.latest().again), (3, 1));
}

#[test]
fn a_file_from_before_parts_is_one_part() {
    let again = Recording::resume(old_file().as_slice(), Box::new(|_: &Save| {})).unwrap();
    assert_eq!(again.parts().len(), 1);
    assert_eq!((again.latest().n, again.latest().map.as_str()), (0, "cp_a"));
}

#[test]
fn ticks_starting_over_on_the_same_map_start_a_chunk_of_their_own() {
    let r = Recording::live();
    push_at(&r, &server("cp_a"), T0);
    let time = snapshots(&r, 50, 1650, T0 + 500);
    snapshots(&r, 50, 500, time);
    let part = r.latest();
    assert_eq!((r.parts().len(), part.n), (2, 1));
    let (_, i, played) = r.seek(part.origin);
    assert_eq!((played, r.chunk(i).unwrap()[0].0), (0, part.origin), "its origin is the start of a chunk");
}
