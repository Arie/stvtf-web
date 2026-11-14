//! A recording's parts through its file: written, trimmed, resumed; and files of the versions
//! before.

use super::super::tests::{push_at, to};
use super::super::*;
use super::real_tests::viaduct;
use super::tests::{Stream, T0, line, snapshots};
use super::{PartState, Seed};
use crate::testing::server;

/// What the file holds now.
fn file_of(r: &Recording) -> Vec<u8> {
    lock(&r.inner).file().unwrap()
}

fn resumed(file: &[u8]) -> Recording {
    Recording::resume(file, Box::new(|_: &Save| {})).unwrap()
}

/// `again`, resumed from `r`'s file, has `r`'s parts, every field: only the latest's length is
/// less (the open chunk is not in the file).
fn same_parts(r: &Recording, again: &Recording) {
    let mut want = r.parts();
    let latest = want.last_mut().unwrap();
    if latest.changing.is_none() {
        latest.last = again.length();
    }
    assert_eq!(again.parts(), want);
    assert_eq!((again.start(), again.latest().load), (r.start(), r.latest().load));
}

/// `cp_a` for `before` seconds of play; the server changing map, `tail` seconds of signon lines
/// with `cp_b`'s server line the `named`th; the frozen frame; `cp_b`'s play till the trim cut,
/// and 20 s more.
fn trimmed(before: u32, tail: u32, named: u32) -> Recording {
    let r = Recording::live();
    let mut s = Stream { r: &r, s: 0.0 };
    s.server(0.0, "cp_a");
    s.play(1000, before);
    s.status(1.0, "changelevel");
    let connecting = line(r#"{"type":"status","state":"connecting","server":"tv","build":"1"}"#);
    for i in 0..tail {
        if i == named {
            s.server(1.0, "cp_b")
        } else {
            s.push(1.0, &connecting)
        };
    }
    s.snapshot(1.0, 68);
    s.status(1.0, "map_change");
    s.server(1.0, "cp_b");
    let mut tick = 244;
    while r.start() == 0 {
        s.snapshot(1.0, tick);
        tick += 67;
    }
    s.play(tick, 20);
    r
}

/// After `again` resumed: the next map is the next number.
fn numbers_go_on(again: &Recording) {
    let n = again.latest().load;
    push_at_end(again, &server("cp_c"));
    push_at_end(again, &crate::matches::hub::tests::snapshot(50, None, Vec::new()));
    assert_eq!(again.latest().load, n + 1);
}

fn push_at_end(r: &Recording, line: &Line) {
    push_at(r, line, T0 + 99_000_000 + r.length());
}

#[test]
fn a_real_change_resumes_with_the_same_parts() {
    let r = Recording::live();
    let mut s = Stream { r: &r, s: 0.0 };
    viaduct(&mut s);
    s.change("koth_nucleus", 244, 20);
    let again = resumed(&file_of(&r));
    assert_eq!(again.parts().len(), 2);
    same_parts(&r, &again);
    numbers_go_on(&again);
}

#[test]
fn a_change_with_one_signon_resumes_with_the_same_parts() {
    let r = Recording::live();
    let mut s = Stream { r: &r, s: 0.0 };
    viaduct(&mut s);
    s.fixed_change("koth_nucleus", 250, 20);
    let again = resumed(&file_of(&r));
    assert_eq!(again.parts().len(), 2);
    same_parts(&r, &again);
}

#[test]
fn a_trim_inside_a_maps_play_keeps_it_from_there() {
    let r = trimmed(700, 40, 2);
    let parts = r.parts();
    assert_eq!(parts.iter().map(|p| p.load).collect::<Vec<_>>(), [0, 1]);
    assert!(r.start() > 0 && r.start() < parts[0].last);
    assert_eq!((parts[0].origin, parts[0].start), (0, r.start()));
    same_parts(&r, &resumed(&file_of(&r)));
}

#[test]
fn a_trim_inside_a_maps_tail_drops_it() {
    for named in [2, 38] {
        let r = trimmed(590, 40, named);
        let parts = r.parts();
        assert_eq!(parts.iter().map(|p| p.load).collect::<Vec<_>>(), [1], "named {named}");
        assert!(r.start() < parts[0].origin, "the file begins in the tail before it");
        assert_eq!(parts[0].start, parts[0].origin);
        let again = resumed(&file_of(&r));
        same_parts(&r, &again);
        numbers_go_on(&again);
    }
}

#[test]
fn a_trim_inside_the_next_map_keeps_it_from_there() {
    let r = trimmed(300, 40, 2);
    let parts = r.parts();
    assert_eq!(parts.iter().map(|p| p.load).collect::<Vec<_>>(), [1]);
    assert!(parts[0].origin < r.start());
    assert_eq!(parts[0].start, r.start());
    let again = resumed(&file_of(&r));
    same_parts(&r, &again);
    numbers_go_on(&again);
}

/// A file with no seed: `origin`, `marks` and `hub` in the header, from chunk `from` on, and
/// `extra` header lines.
fn old_file(r: &Recording, from: usize, extra: &str) -> Vec<u8> {
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    let inner = lock(&r.inner);
    write!(gz, "origin\t0\nmarks\t0\n{extra}").unwrap();
    for state in inner.chunks[from].hub.state() {
        writeln!(gz, "hub\t{state}").unwrap();
    }
    let mut file = gz.finish().unwrap();
    for c in &inner.chunks[from..inner.chunks.len() - 1] {
        file.extend_from_slice(&c.lines);
    }
    file
}

/// `cp_a` played, then loaded again (its ticks start over, no server line: a file with no
/// seed goes on through that), played on.
fn reloaded() -> Recording {
    let r = Recording::live();
    push_at(&r, &server("cp_a"), T0);
    let time = snapshots(&r, 50, 1650, T0 + 500);
    let time = snapshots(&r, 50, 1650, time);
    snapshots(&r, 1700, 2200, time);
    r
}

#[test]
fn a_file_with_no_seed_has_parts_by_the_rule() {
    let r = reloaded();
    let again = resumed(&old_file(&r, 0, ""));
    let parts = again.parts();
    assert_eq!(parts.iter().map(|p| (p.load, p.map.as_str(), p.ready)).collect::<Vec<_>>(), [(0, "cp_a", true), (1, "cp_a", true)]);
    assert_eq!(parts[1].origin, r.parts()[1].origin);
    // Trimmed inside the map: the hub there says which, and that it played.
    let again = resumed(&old_file(&r, 1, ""));
    let parts = again.parts();
    assert_eq!(parts.iter().map(|p| (p.load, p.map.as_str(), p.ready)).collect::<Vec<_>>(), [(0, "cp_a", true), (1, "cp_a", true)]);
    assert_eq!((parts[0].origin, parts[0].start), (0, again.start()));
}

#[test]
fn a_file_with_parts_and_loads_but_no_seed_opens() {
    let r = reloaded();
    let again = resumed(&old_file(&r, 1, "parts\t1\nloads\tcp_a\t1\n"));
    assert_eq!(again.parts().iter().map(|p| p.map.as_str()).collect::<Vec<_>>(), ["cp_a", "cp_a"]);
}

#[test]
fn a_seed_saved_before_runs_of_play_reads() {
    let mut seed = serde_json::to_value(Seed { loads: Loads::default(), part: Some(PartState::default()) }).unwrap();
    let part = seed["part"].as_object_mut().unwrap();
    assert!(part.remove("run").is_some() && part.remove("snap").is_some());
    let read: Seed = serde_json::from_value(seed).unwrap();
    assert!(read.part.is_some_and(|p| p.run.is_none() && p.snap.is_none()));
}

#[test]
fn a_recording_begun_after_loads_of_its_match_resumes_with_their_numbers() {
    // A file as the sink writes it: chunk by chunk, short of any trim.
    let saved = Arc::new(Mutex::new(Vec::new()));
    let r = Recording::live_to(to(&saved));
    let mut s = Stream { r: &r, s: 0.0 };
    s.signon(0.0, "cp_a");
    r.first_load_is(2);
    s.play(50, 25);
    assert_eq!(r.latest().load, 2);
    let again = resumed(&lock(&saved));
    assert_eq!(again.parts().iter().map(|p| p.load).collect::<Vec<_>>(), [2]);
    numbers_go_on(&again);
}

/// The ticks of `r`'s snapshots, in the order it plays them.
fn snapshot_ticks(r: &Recording) -> Vec<u32> {
    let lines = (0..r.chunks()).flat_map(|i| r.chunk(i).unwrap_or_default());
    lines.filter_map(|(_, line)| matches!(line, Line::Snapshot(_)).then(|| line.tick()).flatten()).collect()
}

/// A release that goes on closing chunks into the file after the next one resumed from it, as
/// both did at a deploy before a match was handed over: the file, and the tick it reaches.
fn written_by_two_at_once() -> (Vec<u8>, u32) {
    let saved = Arc::new(Mutex::new(Vec::new()));
    let old = Recording::live_to(to(&saved));
    let mut o = Stream { r: &old, s: 0.0 };
    o.server(0.0, "cp_a");
    o.play(1000, 25);
    let file = lock(&saved).clone();
    let new = Recording::resume(file.as_slice(), to(&saved)).unwrap();
    // The old one's next chunk goes in first, 30 s of the match; the new one's begin at 25 s.
    o.play(1000 + 25 * 67, 10);
    let mut n = Stream { r: &new, s: 22.0 };
    n.push(0.0, &line(r#"{"type":"status","state":"connecting","server":"tv","build":"1"}"#));
    n.signon(0.5, "cp_a");
    n.play(1000 + 25 * 67, 25);
    let file = lock(&saved).clone();
    (file, 1000 + 44 * 67)
}

#[test]
fn a_stretch_two_releases_both_wrote_resumes_as_one_load_its_ticks_going_on() {
    let (file, reached) = written_by_two_at_once();
    let saved = Arc::new(Mutex::new(Vec::new()));
    let again = Recording::resume(file.as_slice(), to(&saved)).unwrap();
    assert_eq!(again.parts().iter().map(|p| (p.load, p.map.as_str())).collect::<Vec<_>>(), [(0, "cp_a")]);
    let ticks = snapshot_ticks(&again);
    assert!(ticks.is_sorted(), "{ticks:?}");
    assert!(ticks.last().is_some_and(|t| *t >= reached - 10 * 67), "{ticks:?}");
    let clean = lock(&saved).clone();
    assert_eq!(snapshot_ticks(&resumed(&clean)), ticks, "the file is written anew without what was skipped");
}

#[test]
fn a_chunk_written_after_the_next_release_went_on_from_the_file_is_skipped_whole() {
    let saved = Arc::new(Mutex::new(Vec::new()));
    let old = Recording::live_to(to(&saved));
    let mut o = Stream { r: &old, s: 0.0 };
    o.server(0.0, "cp_a");
    o.play(1000, 25);
    // The new release goes on at the tick after the file's last, and closes a chunk into it;
    // then the old one, a version that does not give its file, closes its own of that stretch.
    let file = lock(&saved).clone();
    let new = Recording::resume(file.as_slice(), to(&saved)).unwrap();
    let mut n = Stream { r: &new, s: 19.0 };
    n.play(1000 + 19 * 67, 11);
    o.play(1000 + 25 * 67, 6);
    n.play(1000 + 30 * 67, 12);
    let file = lock(&saved).clone();
    let again = resumed(&file);
    let ticks = snapshot_ticks(&again);
    assert!(ticks.windows(2).all(|t| t[1] == t[0] + 67), "every second once: {ticks:?}");
    assert_eq!((ticks.len(), again.parts().len()), (39, 1));
}

#[test]
fn a_file_torn_at_its_end_resumes_up_to_its_last_whole_chunk() {
    let saved = Arc::new(Mutex::new(Vec::new()));
    let r = Recording::live_to(to(&saved));
    let mut s = Stream { r: &r, s: 0.0 };
    s.server(0.0, "cp_a");
    s.play(1000, 35);
    // Three chunks, the process killed as it wrote the third.
    let whole = lock(&saved).clone();
    let again = Recording::resume(&whole[..whole.len() - 20], to(&saved)).unwrap();
    assert_eq!((again.chunks(), again.length()), (2, 19_000));
    let file = lock(&saved).clone();
    assert_eq!(resumed(&file).chunks(), 2, "the file is written anew without the torn chunk");
    assert!(Recording::resume(&whole[5..], to(&saved)).is_err(), "nothing of it reads: not a recording");
}
