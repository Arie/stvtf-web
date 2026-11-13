//! An admin's moves, and following again: the page's Live on a map that is over.

use super::move_tests::{decided, live_room_at_live_on, on_map, push_server, push_snapshot};
use super::part::MapPost;
use super::part_tests::{OLD_SAVED_ROOM, playback, push, settle};
use super::stream_tests::a_pass;
use super::tests::until;
use super::view::View;
use super::*;
use crate::matches::hub::{subscribe, tests::snapshot};
use serde_json::Value;

#[test]
fn live_on_a_map_over_follows_again_and_the_room_goes_on_when_the_next_plays() {
    let (room, rec) = live_room_at_live_on("cp_a");
    push_server(&rec, "cp_b");
    assert!(decided(&room, 0));
    room.control(&Control { paused: Some(true), speed: Some(2.0), seek: Some(100) }).unwrap();
    assert!(!lock(&room.state).follows, "touched");
    room.go(&MapPost { map: 0 }).unwrap();
    let last = rec.part(0).unwrap().last;
    until("at the end", || {
        let st = lock(&room.state);
        st.seek.is_none() && st.at == last
    });
    let st = lock(&room.state);
    assert_eq!((st.paused, st.speed, st.follows, st.part), (false, 1.0, true, 0));
    drop(st);
    push_snapshot(&rec, 50);
    on_map(&room, 1);
    room.stop();
}

#[test]
fn live_on_a_map_going_on_is_live() {
    let (room, rec) = live_room_at_live_on("cp_a");
    room.control(&Control { paused: Some(true), speed: Some(2.0), seek: Some(50) }).unwrap();
    room.go(&MapPost { map: 0 }).unwrap();
    until("at live", || {
        let st = lock(&room.state);
        st.seek.is_none() && st.at == rec.length()
    });
    let st = lock(&room.state);
    assert_eq!((st.paused, st.speed, st.follows, st.part), (false, 1.0, false, 0));
    drop(st);
    room.stop();
}

#[test]
fn a_map_with_no_play_is_no_map_to_go_to() {
    let (room, rec) = live_room_at_live_on("cp_a");
    push_server(&rec, "cp_b");
    push(&rec, &snapshot(68, None, Vec::new()), rec.length() + 50);
    assert!(!rec.part(1).unwrap().ready);
    assert_eq!(room.go(&MapPost { map: 1 }).unwrap_err().to_string(), "Map gone");
    assert_eq!(lock(&room.state).part, 0);
    room.stop();
}

/// The last view line the room told its pages.
fn view_line(room: &Room) -> Value {
    let lines: Vec<Value> = subscribe(&room.hub, false).try_iter().map(|l| serde_json::from_str(&l).unwrap()).collect();
    lines.into_iter().rfind(|l| l["type"] == "view").expect("a view line")
}

#[test]
fn a_room_that_moves_stops_following_a_player_and_goes_back_to_the_view_before() {
    let (room, rec) = live_room_at_live_on("cp_a");
    room.set_view(&ViewPost { view: View::Auto, take: true, page: "a".into() }, "me").unwrap();
    room.set_view(&ViewPost { view: View::Follow { player: 4, w: None, h: None }, take: true, page: "a".into() }, "me").unwrap();
    assert_eq!(*lock(&room.camera), Some(4));
    push_server(&rec, "cp_b");
    assert!(decided(&room, 0), "a view chosen before the map ended does not count");
    push_snapshot(&rec, 50);
    on_map(&room, 1);
    settle(&room);
    let v = view_line(&room);
    assert_eq!((v["mode"].as_str(), v["page"].as_str()), (Some("auto"), Some("a")));
    assert_eq!(*lock(&room.camera), None);
    // An admin's move too; none chosen before: free.
    room.set_view(&ViewPost { view: View::Follow { player: 4, w: None, h: None }, take: true, page: "b".into() }, "me").unwrap();
    room.set_view(&ViewPost { view: View::Free, take: true, page: "b".into() }, "me").unwrap();
    room.set_view(&ViewPost { view: View::Follow { player: 5, w: None, h: None }, take: true, page: "b".into() }, "me").unwrap();
    room.go(&MapPost { map: 0 }).unwrap();
    on_map(&room, 0);
    settle(&room);
    assert_eq!((view_line(&room)["mode"].as_str(), *lock(&room.camera)), (Some("free"), None));
    room.stop();
}

/// A room saved at the end of a map over by the version before (following, and nothing of
/// whether that was decided), picked up in the gap.
#[test]
fn a_room_saved_in_the_gap_by_the_version_before_opens_and_decides_where_it_is() {
    let (room, rec) = live_room_at_live_on("cp_a");
    push_server(&rec, "cp_b");
    room.stop();
    let last = rec.part(0).unwrap().last;
    let mut saved: Value = serde_json::from_str(OLD_SAVED_ROOM).unwrap();
    (saved["part"], saved["at"], saved["paused"], saved["follows"]) = (0.into(), last.into(), false.into(), true.into());
    let again = Room::restore("r".into(), serde_json::from_value(saved).unwrap(), rec.clone());
    assert!(decided(&again, 0), "at the end, playing: it follows");
    a_pass(&again);
    assert_eq!(playback(&again)["map"].as_u64(), Some(0));
    push_snapshot(&rec, 50);
    on_map(&again, 1);
    again.stop();
}

#[test]
fn live_on_a_map_over_with_the_next_playing_goes_there_at_once() {
    let (room, rec) = live_room_at_live_on("cp_a");
    push_server(&rec, "cp_b");
    assert!(decided(&room, 0));
    room.clear("me");
    push_snapshot(&rec, 50);
    a_pass(&room);
    assert_eq!(lock(&room.state).part, 0, "touched: it waits");
    let page = subscribe(&room.hub, false);
    let _joined: Vec<_> = page.try_iter().collect();
    room.go(&MapPost { map: 0 }).unwrap();
    on_map(&room, 1);
    settle(&room);
    let seeks = page.try_iter().filter(|l| l.contains(r#""type":"seek""#)).count();
    assert_eq!(seeks, 1, "one move, not to the old map's end first");
    room.stop();
}
