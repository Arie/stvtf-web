//! A room while the server changes map: the map just played stays showing.

use super::move_tests::{decided, live_on, live_room_at_live_on, new_map_with_play, on_map, push_server};
use super::part::MapPost;
use super::part_tests::{OLD_SAVED_ROOM, playback, push, settle};
use super::stream_tests::a_pass;
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
    assert_eq!(p["maps"].as_array().unwrap().len(), 1, "a map with nothing shown yet is not one");
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
    // A frozen frame of it, and nothing more.
    push(&rec, &snapshot(68, None, Vec::new()), rec.length() + 50);
    push_changing(&rec);
    new_map_with_play(&rec, "cp_c");
    assert!(!rec.part(1).unwrap().ready && rec.part(2).unwrap().ready);
    on_map(&room, 2);
    settle(&room);
    assert_eq!(lock(&room.hub).map.as_deref(), Some("cp_c"));
    room.stop();
}

#[test]
fn a_map_that_showed_nothing_yet_is_no_map_to_go_to() {
    let rec = live_on("cp_a");
    push_changing(&rec);
    push_server(&rec, "cp_b");
    let room = Room::start("r".into(), "me", rec.clone());
    assert_eq!(rec.parts().len(), 1);
    assert_eq!(room.go(&MapPost { map: 1 }).unwrap_err().to_string(), "Map gone");
    assert_eq!(lock(&room.state).part, 0);
    room.stop();
}

#[test]
fn a_room_following_live_stays_on_its_map_through_a_snapshot_late_behind_the_change() {
    let (room, rec) = live_room_at_live_on("cp_a");
    let page = subscribe(&room.hub, false);
    let stroke = || read_marker(br#"[{"stroke":1,"by":1,"points":[[0.0,0.0,0.0]]}]"#, 0).remove(&0).unwrap();
    lock(&room.marker).insert(0, stroke());
    push(&rec, &status("changelevel"), rec.length() + 100);
    push(&rec, &snapshot(130, None, Vec::new()), rec.length() + 86);
    push(&rec, &status("map_change"), rec.length() + 14);
    until("the room to say its map is over", || playback(&room)["over"] == true);
    new_map_with_play(&rec, "cp_b");
    on_map(&room, 1);
    let lines: Vec<Value> = page.try_iter().map(|l| serde_json::from_str(&l).unwrap()).collect();
    let before: Vec<_> = lines.iter().take_while(|l| l["type"] != "seek").collect();
    assert!(before.iter().any(|l| l["type"] == "snapshot" && l["tick"] == 130), "the late frame plays");
    assert!(!before.iter().any(|l| l["type"] == "status"), "the word of the change does not");
    for p in lines.iter().filter(|l| l["type"] == "playback") {
        let maps = p["maps"].as_array().unwrap();
        assert!(maps.iter().all(|m| m["map"] != "cp_a" || m["n"] == 0), "{p}");
        assert!(p["map"] == 0 || maps.iter().any(|m| m["n"] == p["map"] && m["map"] == "cp_b" && m["ready"] == true), "{p}");
    }
    assert_eq!(lock(&room.hub).map.as_deref(), Some("cp_b"));
    assert!(lock(&room.marker).contains_key(&0), "its marker is still its map's");
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
    let end = rec.parts()[0].last;
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
    assert_eq!(part.start, part.origin, "its first snapshot");
    // Paused: it stays where it lands.
    let room = Room::start_at("r".into(), "me", rec.clone(), rec.length(), 1.0, true);
    room.go(&MapPost { map: 1 }).unwrap();
    on_map(&room, 1);
    settle(&room);
    assert_eq!(playback(&room)["at"].as_u64(), Some(part.start));
    assert!(lock(&room.hub).has_snapshot() && lock(&room.hub).map.as_deref() == Some("cp_b"), "its own map's first moment");
    assert_eq!(lock(&room.state).tick, Some(50), "and that map's tick");
    room.stop();
}

#[test]
fn stvtf_output_saved_as_the_server_changed_map_ends_as_one_without_it() {
    let mut lines = String::new();
    for line in
        [server("cp_a"), snapshot(50, None, Vec::new()), snapshot(117, None, Vec::new()), status("changelevel"), status("map_change")]
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
    assert_eq!((p["maps"].as_array().unwrap().len(), p["at"].as_u64()), (1, Some(rec.length())));
    room.stop();
}

#[test]
fn a_map_that_goes_on_after_all_plays_on_with_what_came_while_it_seemed_over() {
    let (room, rec) = live_room_at_live_on("cp_a");
    let page = subscribe(&room.hub, false);
    // Announced, then the same map again (a reconnect): over, its frames tail.
    push_server(&rec, "cp_a");
    assert!(decided(&room, 0));
    for tick in [150, 200] {
        push(&rec, &snapshot(tick, None, Vec::new()), rec.length() + 50);
    }
    a_pass(&room);
    assert!(rec.latest().over(), "frames within 5 s of ticks: still over");
    // Past 5 s of ticks: it went on.
    push(&rec, &snapshot(500, None, Vec::new()), rec.length() + 50);
    until("the room to play on", || lock(&room.state).tick == Some(500));
    let ticks: Vec<_> = page
        .try_iter()
        .map(|l| serde_json::from_str::<Value>(&l).unwrap())
        .filter(|l| l["type"] == "snapshot")
        .map(|l| l["tick"].clone())
        .collect();
    assert!(ticks.contains(&150.into()) && ticks.contains(&200.into()), "{ticks:?}");
    assert!(!lock(&room.state).follows);
    room.stop();
}
