//! A room going on to another map: by an admin, or by itself when it follows live.

use super::part::MapPost;
use super::part_tests::{playback, push, settle, stalemate};
use super::tests::until;
use super::view::View;
use super::*;
use crate::matches::hub::{subscribe, tests::snapshot};
use crate::testing::server;
use serde_json::Value;

/// A watched room on a live match on `map`, resting at live: it follows.
fn live_room_at_live_on(map: &str) -> (Arc<Room>, Arc<Recording>) {
    let rec = Arc::new(Recording::live());
    push(&rec, &server(map), 0);
    push(&rec, &snapshot(50, None, Vec::new()), 50);
    push(&rec, &stalemate(), 200);
    let room = Room::start("r".into(), "me", rec.clone());
    room.viewers.fetch_add(1, Ordering::Relaxed);
    until("the room to follow live", || lock(&room.state).follows);
    (room, rec)
}

/// The server changes map to `map`: no play of it yet.
fn push_server(rec: &Recording, map: &str) {
    push(rec, &server(map), rec.length() + 100);
}

/// A snapshot at `tick`: the latest map has play.
fn push_snapshot(rec: &Recording, tick: u32) {
    push(rec, &snapshot(tick, None, Vec::new()), rec.length() + 50);
}

fn new_map_with_play(rec: &Recording, map: &str) {
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

fn on_map(room: &Room, n: usize) {
    until("the room on its map", || {
        let st = lock(&room.state);
        st.part == n && st.seek.is_none()
    });
}

/// Something someone does in a room.
#[derive(Debug, Clone, Copy)]
enum Touch {
    Pause,
    Draw,
    Note,
    Clear,
    View,
}

impl Touch {
    fn on(self, room: &Room) {
        match self {
            Touch::Pause => room.control(&Control { paused: Some(true), ..Control::default() }).unwrap(),
            Touch::Draw => room.draw(&Draw { stroke: 1, points: vec![[1.0, 2.0, 0.0]], end: true }, "me").unwrap(),
            Touch::Note => room.note(&NoteDraw { text: "here".into(), at: None, player: Some(2) }, "me").unwrap(),
            Touch::Clear => room.clear("me"),
            Touch::View => room.set_view(&ViewPost { view: View::Auto, take: true, page: String::new() }, "me").unwrap(),
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

#[test]
fn a_room_behind_live_when_its_map_ended_stays() {
    let (room, rec) = live_room_at_live_on("cp_a");
    room.control(&Control { seek: Some(rec.length() - 150), ..Control::default() }).unwrap();
    new_map_with_play(&rec, "cp_b");
    settle_past(&room, rec.length());
    let p = playback(&room);
    assert_eq!((p["map"].as_u64(), p["over"].as_bool()), (Some(0), Some(true)), "the replay ran to the end, and waits");
    room.stop();
}

#[test]
fn a_room_touched_after_its_map_ended_stays() {
    std::thread::scope(|s| {
        for touch in [Touch::Pause, Touch::Draw, Touch::Note, Touch::Clear, Touch::View] {
            s.spawn(move || {
                let (room, rec) = live_room_at_live_on("cp_a");
                push_server(&rec, "cp_b");
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
    new_map_with_play(&rec, "cp_b");
    let at = rec.latest().origin;
    room.go(&MapPost { map: 1 }).unwrap();
    on_map(&room, 1);
    settle(&room);
    assert_eq!((playback(&room)["map"].as_u64(), playback(&room)["live"].as_bool()), (Some(1), Some(true)));
    let page = subscribe(&room.hub, false);
    room.go(&MapPost { map: 0 }).unwrap();
    on_map(&room, 0);
    settle(&room);
    let lines: Vec<Value> = page.try_iter().map(|l| serde_json::from_str(&l).unwrap()).collect();
    let p = lines.iter().find(|l| l["type"] == "playback" && l["map"] == 0).unwrap();
    assert_eq!((p["map"].as_u64(), p["at"].as_u64(), p["length"].as_u64()), (Some(0), Some(0), Some(at)));
    settle_past(&room, at + 100);
    assert_eq!(playback(&room)["map"].as_u64(), Some(0), "at its end it waits: it was moved here");
    assert_eq!(room.go(&MapPost { map: 7 }).unwrap_err().to_string(), "That map is gone.");
    room.stop();
}

#[test]
fn a_restart_in_the_break_keeps_whether_the_room_moves_by_itself() {
    let (room, rec) = live_room_at_live_on("cp_a");
    push_server(&rec, "cp_b");
    settle(&room);
    let saved = serde_json::to_string(&room.saved()).unwrap();
    room.stop();
    let again = Room::restore("r".into(), serde_json::from_str(&saved).unwrap(), rec.clone());
    again.viewers.fetch_add(1, Ordering::Relaxed);
    settle(&again);
    push_snapshot(&rec, 50);
    on_map(&again, 1);
    settle(&again);
    assert_eq!(playback(&again)["map"].as_u64(), Some(1));
    again.stop();
}
