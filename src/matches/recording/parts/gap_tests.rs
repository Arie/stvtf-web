//! A map change as the server tells it: two status lines, a long silence, then the new map.

use super::super::tests::{push_at, to};
use super::super::*;
use super::tests::{T0, file_of, line, snapshots};
use crate::matches::hub::tests::snapshot;
use crate::testing::server;

fn status(state: &str) -> Line {
    line(&format!(r#"{{"type":"status","state":"{state}"}}"#))
}

/// `cp_a` with play, then the server says it is changing map; the play time it says so at.
fn cp_a_then_changing(r: &Recording) -> u64 {
    push_at(r, &server("cp_a"), T0);
    let time = snapshots(r, 50, 1650, T0 + 500);
    push_at(r, &status("changelevel"), time);
    push_at(r, &status("map_change"), time);
    time - T0
}

/// After the silence that follows the change at `t` (a second of play, as any gap is): signon,
/// `map`'s server line and its snapshots from tick `from` on.
fn then_on(r: &Recording, map: &str, from: u32, t: u64) {
    let time = T0 + t + 60_000;
    push_at(r, &status("new"), time);
    push_at(r, &line(&format!(r#"{{"type":"status","state":"full","map":"{map}"}}"#)), time);
    push_at(r, &server(map), time);
    snapshots(r, from, from + 1000, time + 500);
}

fn maps(r: &Recording) -> Vec<(String, usize)> {
    r.parts().into_iter().map(|p| (p.map, p.again)).collect()
}

#[test]
fn a_map_ends_where_the_server_says_it_changes() {
    let r = Recording::live();
    let t = cp_a_then_changing(&r);
    then_on(&r, "cp_b", 50, t);
    let parts = r.parts();
    assert_eq!(parts.iter().map(|p| (p.n, p.map.as_str(), p.again)).collect::<Vec<_>>(), [(0, "cp_a", 0), (1, "cp_b", 0)]);
    assert_eq!((parts[0].end, parts[1].origin), (Some(t), t));
    let (hub, _, _) = r.seek(t - 1);
    assert!(hub.has_snapshot() && hub.map.as_deref() == Some("cp_a"), "the map's last moment");
}

#[test]
fn the_gap_has_a_nameless_part_that_is_not_ready() {
    let r = Recording::live();
    let t = cp_a_then_changing(&r);
    let latest = r.latest();
    assert_eq!((latest.map.as_str(), latest.ready, r.parts()[0].end), ("", false, Some(t)));
    assert_eq!(r.watchable().map, "cp_a");
    push_at(&r, &server("cp_b"), T0 + t + 60_000);
    push_at(&r, &snapshot(50, None, Vec::new()), T0 + t + 60_500);
    let latest = r.latest();
    assert_eq!((latest.map.as_str(), latest.ready, r.watchable().n), ("cp_b", true, 1));
}

#[test]
fn a_map_change_to_the_same_map_is_one_new_part() {
    let r = Recording::live();
    let t = cp_a_then_changing(&r);
    then_on(&r, "cp_a", 50, t);
    assert_eq!(maps(&r), [("cp_a".into(), 0), ("cp_a".into(), 1)]);
}

#[test]
fn a_status_of_a_map_change_before_any_play_starts_nothing() {
    let r = Recording::live();
    push_at(&r, &status("changelevel"), T0);
    push_at(&r, &status("map_change"), T0);
    push_at(&r, &server("cp_a"), T0 + 1000);
    snapshots(&r, 50, 500, T0 + 1500);
    assert_eq!(maps(&r), [("cp_a".into(), 0)], "as the very first lines");
    let r = Recording::live();
    let t = cp_a_then_changing(&r);
    push_at(&r, &status("changelevel"), T0 + t + 1000);
    push_at(&r, &status("map_change"), T0 + t + 1000);
    then_on(&r, "cp_b", 50, t);
    assert_eq!(maps(&r), [("cp_a".into(), 0), ("cp_b".into(), 0)], "twice in a row");
}

#[test]
fn a_server_line_without_a_status_still_starts_a_part() {
    let r = Recording::live();
    push_at(&r, &server("cp_a"), T0);
    let time = snapshots(&r, 50, 500, T0 + 500);
    push_at(&r, &server("cp_b"), time);
    assert_eq!(maps(&r), [("cp_a".into(), 0), ("cp_b".into(), 0)]);
    assert_eq!(r.latest().origin, time - T0);
}

#[test]
fn a_change_the_same_map_goes_on_from_is_another_load_of_it() {
    // The connection drops and comes back to the map, its ticks going on.
    let r = Recording::live();
    let t = cp_a_then_changing(&r);
    then_on(&r, "cp_a", 1700, t);
    assert_eq!(maps(&r), [("cp_a".into(), 0), ("cp_a".into(), 1)]);
    // Play with no server line: the map it was.
    let r = Recording::live();
    let t = cp_a_then_changing(&r);
    snapshots(&r, 1700, 2200, T0 + t + 1000);
    assert_eq!(maps(&r), [("cp_a".into(), 0), ("cp_a".into(), 1)]);
    assert!(r.latest().ready);
}

#[test]
fn a_resumed_recording_has_the_parts_of_a_map_change() {
    let saved = Arc::default();
    let r = Recording::live_to(to(&saved));
    let t = cp_a_then_changing(&r);
    then_on(&r, "cp_b", 50, t);
    let again = Recording::resume(lock(&saved).as_slice(), to(&saved)).unwrap();
    assert_eq!(again.parts().len(), 2);
    assert_eq!(again.parts(), r.parts_closed());
}

#[test]
fn a_file_trimmed_in_the_gap_resumes_nameless_until_its_server_line() {
    let r = Recording::live();
    push_at(&r, &server("cp_a"), T0);
    push_at(&r, &snapshot(50, None, Vec::new()), T0 + 500);
    let stalemate = line(r#"{"type":"stalemate","tick":5}"#);
    for s in 1..600 {
        push_at(&r, &stalemate, T0 + s * 1000);
    }
    push_at(&r, &status("changelevel"), T0 + 600_000);
    push_at(&r, &status("map_change"), T0 + 600_000);
    // Tries to connect again while the server loads: a chunk starts among them, which the trim keeps.
    let connecting = line(r#"{"type":"status","state":"connecting","server":"tv","build":"1"}"#);
    for s in 601..=615 {
        push_at(&r, &connecting, T0 + s * 1000);
    }
    push_at(&r, &status("new"), T0 + 616_000);
    push_at(&r, &server("cp_b"), T0 + 616_000);
    push_at(&r, &snapshot(50, None, Vec::new()), T0 + 616_500);
    let mut s = 617;
    while r.start() == 0 {
        push_at(&r, &stalemate, T0 + s * 1000);
        s += 1;
    }
    assert_eq!(r.start(), 610_000, "the trim cut in the gap");
    let again = Recording::resume(file_of(&r).as_slice(), Box::new(|_: &Save| {})).unwrap();
    assert_eq!(again.parts(), r.parts_closed());
    assert_eq!((again.latest().n, again.latest().map.as_str(), again.latest().again), (1, "cp_b", 0));
}
