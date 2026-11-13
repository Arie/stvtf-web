//! A room while the server changes map: the map just played stays showing.

use super::move_tests::{live_on, live_room_at_live_on, new_map_with_play, on_map, push_server};
use super::part::MapPost;
use super::part_tests::{OLD_SAVED_ROOM, playback, push, settle};
use super::tests::until;
use super::*;
use crate::matches::hub::{subscribe, tests::snapshot};
use crate::testing::server;
use serde_json::Value;

fn status(state: &str) -> stvtf::Line {
    serde_json::from_str(&format!(r#"{{"type":"status","state":"{state}"}}"#)).unwrap()
}

/// The server says it is changing map: its two status lines, then nothing for a while.
fn push_changing(rec: &Recording) {
    let at = rec.length() + 100;
    for state in ["changelevel", "map_change"] {
        push(rec, &status(state), at);
    }
}

/// The kinds of line a page joining the room now is given.
fn joined_with(room: &Room) -> Vec<String> {
    let page = subscribe(&room.hub, false);
    page.try_iter()
        .map(|l| serde_json::from_str::<Value>(&l).unwrap())
        .map(|l| l["type"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The room shows the moment just before the server said it changes map: play, and no word of it.
fn shows_the_last_moment(room: &Room) {
    let kinds = joined_with(room);
    assert!(kinds.iter().any(|k| k == "snapshot") && !kinds.iter().any(|k| k == "status"), "{kinds:?}");
    assert!(lock(&room.hub).has_snapshot() && lock(&room.hub).map.as_deref() == Some("cp_a"));
}

#[test]
fn a_room_keeps_the_last_moment_showing_through_the_gap() {
    let (room, rec) = live_room_at_live_on("cp_a");
    push_changing(&rec);
    until("the room to say its map is over", || playback(&room)["over"] == true);
    let p = playback(&room);
    assert_eq!((p["map"].as_u64(), p["live"].as_bool()), (Some(0), Some(false)));
    assert_eq!(p["maps"].as_array().unwrap().len(), 1, "a map with no name yet is not listed");
    shows_the_last_moment(&room);
    new_map_with_play(&rec, "cp_b");
    on_map(&room, 1);
    settle(&room);
    assert_eq!(playback(&room)["live"].as_bool(), Some(true));
    room.stop();
}

#[test]
fn a_room_following_live_goes_past_a_map_that_never_had_play() {
    let (room, rec) = live_room_at_live_on("cp_a");
    push_changing(&rec);
    push_server(&rec, "cp_b");
    push_changing(&rec);
    new_map_with_play(&rec, "cp_c");
    assert!(!rec.part(1).unwrap().ready && rec.part(2).unwrap().ready);
    on_map(&room, 2);
    settle(&room);
    assert_eq!(lock(&room.hub).map.as_deref(), Some("cp_c"));
    room.stop();
}

#[test]
fn a_map_with_no_name_yet_is_no_map_to_go_to() {
    let rec = live_on("cp_a");
    push_changing(&rec);
    let room = Room::start("r".into(), "me", rec.clone());
    assert_eq!(rec.latest().map, "");
    assert_eq!(room.go(&MapPost { map: 1 }).unwrap_err().to_string(), "Map gone");
    assert_eq!(lock(&room.state).part, 0);
    room.stop();
}

#[test]
fn a_room_opened_in_the_gap_is_on_the_map_just_played() {
    let rec = live_on("cp_a");
    push_changing(&rec);
    let room = Room::start("r".into(), "me", rec.clone());
    room.viewers.fetch_add(1, Ordering::Relaxed);
    settle(&room);
    let p = playback(&room);
    assert_eq!((p["map"].as_u64(), p["over"].as_bool()), (Some(0), Some(true)));
    shows_the_last_moment(&room);
    // Opened at live: it goes on with the match.
    new_map_with_play(&rec, "cp_b");
    on_map(&room, 1);
    room.stop();
}

#[test]
fn a_room_rewound_to_the_end_of_the_map_just_played_shows_its_last_moment() {
    let rec = live_on("cp_a");
    push_changing(&rec);
    let end = rec.parts()[0].end.unwrap();
    for at in [end, u64::MAX] {
        let room = Room::start_at("r".into(), "me", rec.clone(), at, 1.0, true);
        settle(&room);
        assert_eq!(playback(&room)["at"].as_u64(), Some(end));
        shows_the_last_moment(&room);
        room.stop();
    }
}

#[test]
fn a_room_restored_in_the_gap_is_on_the_map_just_played() {
    let rec = live_on("cp_a");
    push_changing(&rec);
    let room = Room::restore("r".into(), serde_json::from_str(OLD_SAVED_ROOM).unwrap(), rec.clone());
    settle(&room);
    assert_eq!(playback(&room)["map"].as_u64(), Some(0));
    room.stop();
}

#[test]
fn an_admin_moves_the_room_to_where_a_map_after_a_change_shows() {
    let rec = live_on("cp_a");
    push_changing(&rec);
    new_map_with_play(&rec, "cp_b");
    push_changing(&rec);
    new_map_with_play(&rec, "cp_c");
    let part = rec.part(1).unwrap();
    assert!(part.shown > part.origin, "{part:?}");
    // Paused: it stays where it lands.
    let room = Room::start_at("r".into(), "me", rec.clone(), rec.length(), 1.0, true);
    room.go(&MapPost { map: 1 }).unwrap();
    on_map(&room, 1);
    settle(&room);
    assert_eq!(playback(&room)["at"].as_u64(), Some(part.shown));
    assert!(lock(&room.hub).has_snapshot() && lock(&room.hub).map.as_deref() == Some("cp_b"), "its own map's first moment");
    assert_eq!(lock(&room.state).tick, Some(50), "and that map's tick");
    room.stop();
}

#[test]
fn stvtf_output_saved_as_the_server_changed_map_ends_as_one_without_it() {
    let mut lines = String::new();
    for line in
        [server("cp_a"), snapshot(50, None, Vec::new()), snapshot(60, None, Vec::new()), status("changelevel"), status("map_change")]
    {
        lines += &serde_json::to_string(&line).unwrap();
        lines += "\n";
    }
    let rec = Arc::new(Recording::read(lines.as_bytes(), "test").unwrap());
    let room = Room::start("r".into(), "me", rec.clone());
    room.viewers.fetch_add(1, Ordering::Relaxed);
    until("the room to pause at the end", || playback(&room)["paused"] == true);
    let p = playback(&room);
    assert_eq!((p["map"].as_u64(), p["over"].as_bool(), p["live"].as_bool()), (Some(0), Some(false), Some(false)));
    assert_eq!((p["maps"].as_array().unwrap().len(), p["at"].as_u64()), (1, rec.parts()[0].end));
    room.stop();
}
