//! A room going on to another map: by an admin, or by itself when it follows live.

use super::markers::{Markers, read_marker};
use super::part::MapPost;
use super::part_tests::{live_room_then_new_map, playback, push, room_on_a_part_then_trimmed, settle, stalemate};
use super::stream_tests::a_pass;
use super::tests::until;
use super::view::View;
use super::*;
use crate::matches::hub::{subscribe, tests::snapshot};
use crate::testing::server;
use serde_json::Value;

/// A live match on `map`, 200 ms of it so far, with play.
pub(super) fn live_on(map: &str) -> Arc<Recording> {
    let rec = Arc::new(Recording::live());
    push(&rec, &server(map), 0);
    push(&rec, &snapshot(50, None, Vec::new()), 50);
    push(&rec, &snapshot(117, None, Vec::new()), 100);
    push(&rec, &stalemate(), 200);
    rec
}

/// A watched room on a live match on `map`, resting at live.
pub(super) fn live_room_at_live_on(map: &str) -> (Arc<Room>, Arc<Recording>) {
    let rec = live_on(map);
    let room = Room::start("r".into(), "me", rec.clone());
    room.viewers.fetch_add(1, Ordering::Relaxed);
    until("the room at live", || {
        let st = lock(&room.state);
        st.seek.is_none() && st.at >= rec.length()
    });
    (room, rec)
}

/// Until the room saw its map `n` over, and decided whether it follows.
pub(super) fn decided(room: &Room, n: usize) -> bool {
    until("the room to see its map over", || lock(&room.state).over == Some(n));
    lock(&room.state).follows_live
}

/// The server changes map to `map`: no play of it yet.
pub(super) fn push_server(rec: &Recording, map: &str) {
    let at = rec.length() + 100;
    for state in ["changelevel", "map_change"] {
        push(rec, &serde_json::from_str(&format!(r#"{{"type":"status","state":"{state}"}}"#)).unwrap(), at);
    }
    push(rec, &server(map), at);
}

/// A snapshot at `tick`, and one a second of ticks on: the latest map has play.
pub(super) fn push_snapshot(rec: &Recording, tick: u32) {
    push(rec, &snapshot(tick, None, Vec::new()), rec.length() + 50);
    push(rec, &snapshot(tick + 67, None, Vec::new()), rec.length() + 50);
}

pub(super) fn new_map_with_play(rec: &Recording, map: &str) {
    push_server(rec, map);
    push_snapshot(rec, 50);
}

/// Until the room's play time would have passed `ms` if it were going to: it is past it, or it
/// rests at the end of a map the match went on from (where it chose whether to go on).
fn settle_past(room: &Room, ms: u64) {
    until("the room to pass the time", || {
        let st = lock(&room.state);
        let at_end = room.reach(&st).1.is_some_and(|end| st.at >= end);
        st.seek.is_none() && (st.at >= ms || at_end)
    });
}

/// The room's map once the player looked at it after each map kept had play: it says so to its
/// pages each second, then goes on (or not) before it lets go of the room.
fn looked(room: &Room) -> usize {
    until("a look at the room", || {
        let p = playback(room);
        let maps = p["maps"].as_array().unwrap();
        p["map"].as_u64() != Some(0) || (maps.len() > 1 && maps.iter().all(|m| m["ready"] == true))
    });
    lock(&room.state).part
}

pub(super) fn on_map(room: &Room, n: usize) {
    until("the room on its map", || {
        let st = lock(&room.state);
        st.part == n && st.seek.is_none()
    });
}

/// Something someone does in a room.
#[derive(Debug, Clone, Copy)]
enum Touch {
    Pause,
    Speed,
    Seek,
    Draw,
    Note,
    Clear,
    View,
    HandView,
}

fn admin_view(take: bool) -> ViewPost {
    ViewPost { view: View::Admin { cx: 1.0, cy: 2.0, w: 300.0, h: 200.0, rot: 0.0 }, take, page: String::new() }
}

impl Touch {
    /// Before the map ends (which does not count): a camera is moved by hand only in a view
    /// chosen first.
    fn ready(self, room: &Room) {
        if let Touch::HandView = self {
            room.set_view(&admin_view(true), "me").unwrap();
        }
    }

    fn on(self, room: &Room) {
        match self {
            Touch::Pause => room.control(&Control { paused: Some(true), ..Control::default() }).unwrap(),
            Touch::Speed => room.control(&Control { speed: Some(2.0), ..Control::default() }).unwrap(),
            // To its map's end, where it would go on: not back into it, which it would play first.
            Touch::Seek => room.control(&Control { seek: Some(u64::MAX), ..Control::default() }).unwrap(),
            Touch::Draw => room.draw(&Draw { stroke: 1, points: vec![[1.0, 2.0, 0.0]], end: true }, "me").unwrap(),
            Touch::Note => room.note(&NoteDraw { text: "here".into(), at: None, player: Some(2) }, "me").unwrap(),
            Touch::Clear => room.clear("me"),
            Touch::View => room.set_view(&ViewPost { view: View::Auto, take: true, page: String::new() }, "me").unwrap(),
            Touch::HandView => room.set_view(&admin_view(false), "me").unwrap(),
        }
    }
}

#[test]
fn a_room_following_live_moves_to_the_next_map_once_it_has_play() {
    let (room, rec) = live_room_at_live_on("cp_a");
    push_server(&rec, "cp_b");
    settle(&room);
    assert_eq!(playback(&room)["map"].as_u64(), Some(0), "announced, no play yet: it waits");
    push_snapshot(&rec, 50);
    on_map(&room, 1);
    settle(&room);
    let p = playback(&room);
    assert_eq!((p["map"].as_u64(), p["live"].as_bool(), p["paused"].as_bool()), (Some(1), Some(true), Some(false)));
    assert_eq!(lock(&room.hub).map.as_deref(), Some("cp_b"));
    room.stop();
}

/// `ms` more of the match, a line each half second.
fn play_on(rec: &Recording, ms: u64) {
    for _ in 0..ms / 500 {
        push(rec, &stalemate(), rec.length() + 500);
    }
}

/// A room 10 s behind live when its map ends (unwatched: it plays nothing till then) plays on to
/// the end, at 4× to be quick, and stays there.
fn behind_live_plays_to_the_end_and_stays(room: &Room, rec: &Recording) {
    new_map_with_play(rec, "cp_b");
    assert!(!decided(room, 0), "10 s behind is not at live");
    room.control(&Control { speed: Some(4.0), ..Control::default() }).unwrap();
    room.viewers.fetch_add(1, Ordering::Relaxed);
    let last = rec.part(0).unwrap().last;
    until("the end", || playback(room)["at"].as_u64() == Some(last));
    a_pass(room);
    let p = playback(room);
    assert_eq!((p["map"].as_u64(), p["over"].as_bool()), (Some(0), Some(true)), "it played to the end, and waits");
}

#[test]
fn a_room_behind_live_when_its_map_ended_stays() {
    let (room, rec) = live_room_at_live_on("cp_a");
    room.viewers.fetch_sub(1, Ordering::Relaxed);
    play_on(&rec, 10_000);
    room.control(&Control { seek: Some(200), ..Control::default() }).unwrap();
    settle(&room);
    behind_live_plays_to_the_end_and_stays(&room, &rec);
    room.stop();
}

#[test]
fn a_room_that_never_followed_plays_to_its_maps_end_and_stays() {
    let rec = live_on("cp_a");
    play_on(&rec, 10_000);
    let room = Room::start_at("r".into(), "me", rec.clone(), 50, 1.0, false);
    behind_live_plays_to_the_end_and_stays(&room, &rec);
    room.stop();
}

#[test]
fn a_room_touched_after_its_map_ended_stays() {
    std::thread::scope(|s| {
        for touch in [Touch::Pause, Touch::Speed, Touch::Seek, Touch::Draw, Touch::Note, Touch::Clear, Touch::View, Touch::HandView] {
            s.spawn(move || {
                let (room, rec) = live_room_at_live_on("cp_a");
                touch.ready(&room);
                push_server(&rec, "cp_b");
                assert!(decided(&room, 0), "at live as its map ended: {touch:?}");
                touch.on(&room);
                push_snapshot(&rec, 50);
                assert_eq!(looked(&room), 0, "{touch:?}");
                room.stop();
            });
        }
    });
}

#[test]
fn an_admin_moves_the_room_to_the_latest_map_at_live_and_to_an_earlier_one_at_its_start() {
    let (room, rec) = live_room_at_live_on("cp_a");
    // Paused as its map ends: it waits there, paused.
    room.control(&Control { paused: Some(true), ..Control::default() }).unwrap();
    push_server(&rec, "cp_b");
    assert!(!decided(&room, 0));
    push_snapshot(&rec, 50);
    assert_eq!(looked(&room), 0);
    let at = rec.part(0).unwrap().last;
    room.go(&MapPost { map: 1 }).unwrap();
    on_map(&room, 1);
    settle(&room);
    let p = playback(&room);
    assert_eq!((p["map"].as_u64(), p["live"].as_bool(), p["paused"].as_bool()), (Some(1), Some(true), Some(false)));
    let page = subscribe(&room.hub, false);
    room.go(&MapPost { map: 0 }).unwrap();
    on_map(&room, 0);
    settle(&room);
    let lines: Vec<Value> = page.try_iter().map(|l| serde_json::from_str(&l).unwrap()).collect();
    let p = lines.iter().find(|l| l["type"] == "playback" && l["map"] == 0).unwrap();
    assert_eq!((p["map"].as_u64(), p["at"].as_u64(), p["length"].as_u64()), (Some(0), Some(0), Some(at)));
    settle_past(&room, at + 100);
    assert_eq!(playback(&room)["map"].as_u64(), Some(0), "at its end it waits: it was moved here");
    assert_eq!(room.go(&MapPost { map: 7 }).unwrap_err().to_string(), "Map gone");
    room.stop();
}

/// The map a room restarted in the break, after `touch` (none: left alone), is on once the next
/// map has play.
fn restarted_in_the_break(touch: Option<Touch>) -> usize {
    let (room, rec) = live_room_at_live_on("cp_a");
    push_server(&rec, "cp_b");
    settle(&room);
    assert!(decided(&room, 0));
    if let Some(touch) = touch {
        touch.on(&room);
    }
    let saved = serde_json::to_string(&room.saved()).unwrap();
    room.stop();
    let again = Room::restore("r".into(), serde_json::from_str(&saved).unwrap(), rec.clone());
    again.viewers.fetch_add(1, Ordering::Relaxed);
    settle(&again);
    push_snapshot(&rec, 50);
    let map = looked(&again);
    again.stop();
    map
}

#[test]
fn a_restart_in_the_break_keeps_whether_the_room_moves_by_itself() {
    assert_eq!(restarted_in_the_break(None), 1, "it followed: it goes on");
    assert_eq!(restarted_in_the_break(Some(Touch::Clear)), 0, "it did not: it stays");
}

/// The last `marker` line the room announced: what a page joining now is told.
fn marker_line(room: &Room) -> Value {
    let lines: Vec<Value> = subscribe(&room.hub, false).try_iter().map(|l| serde_json::from_str(&l).unwrap()).collect();
    lines.into_iter().rfind(|l| l["type"] == "marker").expect("a marker line")
}

#[test]
fn drawings_stay_with_the_map_they_were_made_on() {
    let (room, _, _) = live_room_then_new_map();
    room.note(&NoteDraw { text: "here".into(), at: None, player: Some(3) }, "me").unwrap();
    room.go(&MapPost { map: 1 }).unwrap();
    settle(&room);
    assert_eq!(marker_line(&room)["strokes"].as_array().unwrap().len(), 0, "none on the next map");
    room.go(&MapPost { map: 0 }).unwrap();
    settle(&room);
    assert_eq!(marker_line(&room)["strokes"][0]["note"]["text"], "here");
    room.stop();
}

#[test]
fn a_marker_saved_before_parts_is_the_latest_maps() {
    let marker: Markers = read_marker(br#"[{"stroke":1,"by":1,"points":[[0.0,0.0,0.0]]}]"#, 1);
    assert_eq!(marker.get(&1).map(Vec::len), Some(1));
}

#[test]
fn a_map_the_trim_dropped_takes_its_marker_with_it_saved_or_not() {
    let (room, _) = room_on_a_part_then_trimmed();
    let stroke = || read_marker(br#"[{"stroke":1,"by":1,"points":[[0.0,0.0,0.0]]}]"#, 0).remove(&0).unwrap();
    // Part 0 is trimmed, part 1 is kept.
    lock(&room.marker).extend([(0, stroke()), (1, stroke())]);
    until("the trimmed map's marker to go", || !lock(&room.marker).contains_key(&0));
    assert!(lock(&room.marker).contains_key(&1), "the map kept keeps its marker");
    room.stop();
}

/// A room file from before maps, with `marker` inline, on a live match of two maps.
#[test]
fn an_inline_marker_from_before_maps_is_the_latest_maps() {
    let mut saved: Value = serde_json::from_str(part_tests::OLD_SAVED_ROOM).unwrap();
    saved["marker"] = serde_json::json!([{"stroke":1,"by":1,"points":[[0.0,0.0,0.0]]}]);
    let rec = live_on("cp_a");
    push_server(&rec, "cp_b");
    push_snapshot(&rec, 50);
    let room = Room::restore("r".into(), serde_json::from_value(saved).unwrap(), rec.clone());
    assert_eq!(room.saved().markers.keys().collect::<Vec<_>>(), [&rec.latest().n]);
    room.stop();
}
