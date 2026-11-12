//! A room on one map of its match.

use super::tests::until;
use super::*;
use crate::matches::hub::{subscribe, tests::snapshot};
use crate::relays::pool::{DemoOf, Offer, Pool, Relays};
use crate::testing::server;
use serde_json::Value;
use stvtf::Line;

const T0: u64 = 1_700_000_000_000;

fn push(rec: &Recording, line: &Line, at: u64) {
    let text = serde_json::to_string(line).unwrap();
    let mut stamped: stvtf::output::Stamped = serde_json::from_str(&text).unwrap();
    stamped.time = Some(T0 + at);
    rec.push(&stamped, &text).unwrap();
}

fn stalemate() -> Line {
    serde_json::from_str(r#"{"type":"stalemate","tick":5}"#).unwrap()
}

/// The room's player has nothing due: it is inside its part (a seek is done), and one that is
/// watched has played what its part has.
fn settle(room: &Room) {
    until("the player to rest", || {
        let st = lock(&room.state);
        let (start, end) = room.reach(&st);
        let end = end.unwrap_or_else(|| room.recording.length());
        st.seek.is_none() && st.at >= start && (room.viewers.load(Ordering::Relaxed) == 0 || st.at >= end)
    });
}

/// The last playback line the room told its pages.
fn playback(room: &Room) -> Value {
    let page = subscribe(&room.hub, false);
    let lines: Vec<Value> = page.try_iter().map(|l| serde_json::from_str(&l).unwrap()).collect();
    lines.into_iter().rfind(|l| l["type"] == "playback").expect("a playback line")
}

/// A watched room on a live match, opened on `cp_a`; then `cp_b` comes, whose play time is returned.
fn live_room_then_new_map() -> (Arc<Room>, Arc<Recording>, u64) {
    let rec = Arc::new(Recording::live());
    push(&rec, &server("cp_a"), 0);
    push(&rec, &snapshot(50, None, Vec::new()), 500);
    push(&rec, &stalemate(), 2000);
    let room = Room::start("r".into(), "me", rec.clone());
    // Someone watches, without a page to drop.
    room.viewers.fetch_add(1, Ordering::Relaxed);
    push(&rec, &server("cp_b"), 2500);
    push(&rec, &snapshot(50, None, Vec::new()), 3000);
    let origin = rec.latest().origin;
    (room, rec, origin)
}

/// `cp_a` for a minute, `cp_b` for hours (the trim drops `cp_a`), then `cp_c`; a room opened on `cp_a`.
fn room_on_a_part_then_trimmed() -> (Arc<Room>, Arc<Recording>) {
    let rec = Arc::new(Recording::live());
    push(&rec, &server("cp_a"), 0);
    push(&rec, &stalemate(), 60_000);
    let room = Room::start("r".into(), "me", rec.clone());
    push(&rec, &server("cp_b"), 61_000);
    for s in 62..=4 * 3600 {
        push(&rec, &stalemate(), s * 1000);
    }
    push(&rec, &server("cp_c"), 4 * 3600 * 1000 + 1000);
    (room, rec)
}

/// `cp_a`, `cp_b`, `cp_a` again, a snapshot in each.
fn two_maps_recording() -> Arc<Recording> {
    let rec = Arc::new(Recording::live());
    for (i, map) in ["cp_a", "cp_b", "cp_a"].into_iter().enumerate() {
        push(&rec, &server(map), i as u64 * 1000);
        push(&rec, &snapshot(50, None, Vec::new()), i as u64 * 1000 + 500);
    }
    rec
}

/// A room as the version before parts saved it.
const OLD_SAVED_ROOM: &str =
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
    let (room, asked) = room_on_part_with_demo_of(2);
    let st = lock(&room.state);
    room.want(&st, room.offer.get().unwrap(), true);
    assert_eq!(lock(&asked).last().map(|(map, again, _)| (map.as_str(), *again)), Some(("cp_a", 1)));
}
