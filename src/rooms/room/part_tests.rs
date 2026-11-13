//! A room on one map of its match.

use super::part::MapPost;
use super::tests::until;
use super::*;
use crate::matches::hub::{subscribe, tests::snapshot};
use crate::relays::pool::{DemoOf, Offer, Pool, Relays};
use crate::testing::server;
use serde_json::Value;
use std::fmt::Write as _;
use stvtf::Line;

const T0: u64 = 1_700_000_000_000;

pub(super) fn push(rec: &Recording, line: &Line, at: u64) {
    let text = serde_json::to_string(line).unwrap();
    let mut stamped: stvtf::output::Stamped = serde_json::from_str(&text).unwrap();
    stamped.time = Some(T0 + at);
    rec.push(&stamped, &text).unwrap();
}

pub(super) fn stalemate() -> Line {
    serde_json::from_str(r#"{"type":"stalemate","tick":5}"#).unwrap()
}

/// The room's player has nothing due: it is inside its part (a seek is done), and one that is
/// watched has played what its part has.
pub(super) fn settle(room: &Room) {
    until("the player to rest", || {
        let st = lock(&room.state);
        let (start, end) = room.reach(&st);
        let end = end.unwrap_or_else(|| room.recording.length());
        st.seek.is_none() && st.at >= start && (room.viewers.load(Ordering::Relaxed) == 0 || st.at >= end)
    });
}

/// The last playback line the room told its pages (once it told any).
pub(super) fn playback(room: &Room) -> Value {
    let last = || {
        let page = subscribe(&room.hub, false);
        let lines: Vec<Value> = page.try_iter().map(|l| serde_json::from_str(&l).unwrap()).collect();
        lines.into_iter().rfind(|l| l["type"] == "playback")
    };
    until("a playback line", || last().is_some());
    last().unwrap()
}

/// A watched room on a live match, opened on `cp_a`; then `cp_b` comes. The play time `cp_a` ends
/// at for the room (its last line not tail) is returned.
pub(super) fn live_room_then_new_map() -> (Arc<Room>, Arc<Recording>, u64) {
    let rec = Arc::new(Recording::live());
    push(&rec, &server("cp_a"), 0);
    push(&rec, &snapshot(50, None, Vec::new()), 500);
    push(&rec, &snapshot(117, None, Vec::new()), 1000);
    push(&rec, &stalemate(), 2000);
    let room = Room::start("r".into(), "me", rec.clone());
    // Someone watches, without a page to drop.
    room.viewers.fetch_add(1, Ordering::Relaxed);
    push(&rec, &server("cp_b"), 2500);
    push(&rec, &snapshot(50, None, Vec::new()), 3000);
    push(&rec, &snapshot(117, None, Vec::new()), 3500);
    let last = rec.part(0).unwrap().last;
    (room, rec, last)
}

/// `cp_a` for a minute, `cp_b` for hours (the trim drops `cp_a`), then `cp_c`; a room opened on
/// `cp_a`, following live.
pub(super) fn room_on_a_part_then_trimmed() -> (Arc<Room>, Arc<Recording>) {
    let rec = Arc::new(Recording::live());
    push(&rec, &server("cp_a"), 0);
    push(&rec, &snapshot(50, None, Vec::new()), 500);
    push(&rec, &snapshot(117, None, Vec::new()), 1000);
    push(&rec, &stalemate(), 60_000);
    let room = Room::start("r".into(), "me", rec.clone());
    // As a watched one at live would: unwatched, it plays nothing.
    lock(&room.state).follows = true;
    push(&rec, &server("cp_b"), 61_000);
    // A frame of it, no play: a room following stays on `cp_a` till the trim drops that.
    push(&rec, &snapshot(50, None, Vec::new()), 61_500);
    for s in 62..=4 * 3600 {
        push(&rec, &stalemate(), s * 1000);
    }
    push(&rec, &server("cp_c"), 4 * 3600 * 1000 + 1000);
    push(&rec, &snapshot(50, None, Vec::new()), 4 * 3600 * 1000 + 1500);
    push(&rec, &snapshot(117, None, Vec::new()), 4 * 3600 * 1000 + 2000);
    (room, rec)
}

/// `cp_a`, `cp_b`, `cp_a` again, play in each.
fn two_maps_recording() -> Arc<Recording> {
    let rec = Arc::new(Recording::live());
    for (i, map) in ["cp_a", "cp_b", "cp_a"].into_iter().enumerate() {
        push(&rec, &server(map), i as u64 * 1000);
        push(&rec, &snapshot(50, None, Vec::new()), i as u64 * 1000 + 500);
        push(&rec, &snapshot(117, None, Vec::new()), i as u64 * 1000 + 700);
    }
    rec
}

/// A room as the version before parts saved it.
pub(super) const OLD_SAVED_ROOM: &str =
    r#"{"paused":true,"speed":1.0,"at":600,"people":{"owner":"me","by_guest":{},"admins":[],"next":1},"leader":null,"view":null}"#;

/// What a `demo_of` was asked: the map, which load of it, the tick.
type Asked = Arc<Mutex<Vec<(String, usize, u32)>>>;

/// A room on part `n` of `two_maps_recording`, and what its relay's `demo_of` is asked.
fn room_on_part_with_demo_of(n: usize) -> (Arc<Room>, Asked) {
    let rec = two_maps_recording();
    let saved: Saved = serde_json::from_str(OLD_SAVED_ROOM).unwrap();
    let room = Room::restore("r".into(), Saved { part: Some(n), ..saved }, rec);
    let asked = Arc::new(Mutex::new(Vec::new()));
    let log = asked.clone();
    let demo_of: DemoOf = Arc::new(move |map, again, tick| {
        lock(&log).push((map.to_string(), again, tick));
        Some("a.dem".into())
    });
    let relays = Arc::new(Pool::Local(Relays::new("127.0.0.1", [])));
    room.offer_relay(Offer { relays, demo_of });
    (room, asked)
}

#[test]
fn a_room_stays_on_its_map_when_the_next_starts() {
    let (room, rec, at) = live_room_then_new_map();
    settle(&room);
    let p = playback(&room);
    assert_eq!((p["map"].as_u64(), p["over"].as_bool(), p["live"].as_bool()), (Some(0), Some(true), Some(false)));
    assert_eq!((p["origin"].as_u64(), p["length"].as_u64()), (Some(0), Some(at)));
    assert_eq!(p["maps"].as_array().unwrap().len(), 2);
    assert_eq!(lock(&room.hub).map.as_deref(), Some("cp_a"), "no line of the next map is played");
    assert!(rec.latest().ready);
    room.stop();
}

#[test]
fn a_seek_is_held_inside_the_rooms_map() {
    let (room, _, at) = live_room_then_new_map();
    room.control(&Control { seek: Some(at + 5000), ..Control::default() }).unwrap();
    settle(&room);
    assert_eq!(playback(&room)["at"].as_u64(), Some(at));
    room.stop();
}

#[test]
fn a_room_whose_map_was_trimmed_goes_to_the_earliest_kept_and_waits() {
    let (room, _) = room_on_a_part_then_trimmed();
    settle(&room);
    let p = playback(&room);
    assert_eq!((p["map"].as_u64(), p["at"].as_u64()), (Some(1), p["start"].as_u64()));
    assert_eq!((p["over"].as_bool(), p["live"].as_bool(), p["paused"].as_bool()), (Some(true), Some(false), Some(true)));
    assert!(!lock(&room.state).follows, "put on another map: it does not go on by itself");
    let inside = p["start"].as_u64().unwrap() + 5000;
    room.control(&Control { seek: Some(inside), ..Control::default() }).unwrap();
    settle(&room);
    assert_eq!(playback(&room)["at"].as_u64(), Some(inside), "it is on that map now");
    room.stop();
}

#[test]
fn a_room_saved_before_parts_opens_on_the_latest_map() {
    let saved: Saved = serde_json::from_str(OLD_SAVED_ROOM).unwrap();
    let room = Room::restore("r".into(), saved, two_maps_recording());
    settle(&room);
    assert_eq!(playback(&room)["map"].as_u64(), Some(2));
    room.stop();
}

#[test]
fn the_relay_is_pointed_at_the_demo_of_the_rooms_load_of_the_map() {
    let (room, asked) = room_on_part_with_demo_of(0);
    let st = lock(&room.state);
    room.want(&st, room.offer.get().unwrap(), true);
    assert_eq!(lock(&asked).last().map(|(map, again, _)| (map.as_str(), *again)), Some(("cp_a", 0)));
    drop(st);
    room.stop();
    let (room, asked) = room_on_part_with_demo_of(2);
    let st = lock(&room.state);
    room.want(&st, room.offer.get().unwrap(), true);
    // The load within its name is not asked yet: the next step asks for the load's number.
    assert_eq!(lock(&asked).last().map(|(map, again, _)| (map.as_str(), *again)), Some(("cp_a", 0)));
    drop(st);
    room.stop();
}

/// The room shows `map`'s first snapshot, and the relay would be pointed at that load's demo
/// at its tick, paused or not.
fn shows_the_first_of(room: &Room, asked: &Asked, map: &str, again: usize) {
    assert!(lock(&room.hub).has_snapshot() && lock(&room.hub).map.as_deref() == Some(map));
    let st = lock(&room.state);
    assert!(room.want(&st, room.offer.get().unwrap(), false).is_some(), "it has a tick to give");
    assert_eq!(lock(asked).last().cloned(), Some((map.to_string(), again, 50)));
}

#[test]
fn a_seek_to_the_start_of_a_later_map_lands_where_it_shows() {
    let (room, asked) = room_on_part_with_demo_of(2);
    room.control(&Control { seek: Some(0), ..Control::default() }).unwrap();
    settle(&room);
    let shown = room.recording.part(2).unwrap().start;
    let p = playback(&room);
    assert_eq!((p["at"].as_u64(), p["start"].as_u64(), p["paused"].as_bool()), (Some(shown), Some(shown), Some(true)));
    shows_the_first_of(&room, &asked, "cp_a", 0);
    room.stop();
}

#[test]
fn a_room_moved_while_paused_has_the_other_maps_first_moment_in_tf2_too() {
    let (room, asked) = room_on_part_with_demo_of(2);
    settle(&room);
    room.go(&MapPost { map: 1 }).unwrap();
    until("the room on map 1", || {
        let st = lock(&room.state);
        st.part == 1 && st.seek.is_none()
    });
    settle(&room);
    assert_eq!(playback(&room)["paused"].as_bool(), Some(true));
    shows_the_first_of(&room, &asked, "cp_b", 0);
    room.stop();
}

#[test]
fn a_demo_is_announced_as_before() {
    let mut lines = String::new();
    for tick in 1..=40 {
        writeln!(lines, "{{\"type\":\"stalemate\",\"tick\":{tick}}}").unwrap();
        if tick == 10 {
            writeln!(lines, r#"{{"type":"round_start","full_reset":false,"tick":{tick}}}"#).unwrap();
        }
    }
    let rec = Arc::new(Recording::read(lines.as_bytes(), "test").unwrap());
    let room = Room::start("r".into(), "me", rec.clone());
    settle(&room);
    let p = playback(&room);
    assert_eq!(p["start"].as_u64(), Some(rec.start()));
    assert_eq!(p["origin"].as_u64(), Some(rec.latest().origin));
    assert_eq!(p["length"].as_u64(), Some(rec.length()));
    assert_eq!(p["rounds"], serde_json::json!(rec.rounds_in(0)));
    assert_eq!(p["live"].as_bool(), Some(rec.is_live()));
    assert!(!rec.rounds_in(0).is_empty() && rec.length() > 0);
    room.stop();
}
