//! Rooms through map changes as a real server sends them (the recording's `Stream`, at
//! `tv_delay 90`): whether a room goes on with the match is decided where the map ends.

use super::move_tests::{decided, on_map};
use super::part_tests::{OLD_SAVED_ROOM, playback};
use super::tests::until;
use super::*;
use crate::matches::hub::{subscribe, tests::snapshot};
use crate::matches::recording::part_tests::Stream;
use crate::matches::recording::real_part_tests::viaduct;
use serde_json::Value;

/// Until the player has looked at the room since now: a pass that told its pages where it is (each
/// second while live) is done.
pub(super) fn a_pass(room: &Room) {
    let page = subscribe(&room.hub, false);
    let _joined: Vec<_> = page.try_iter().collect();
    until("a pass", || page.try_iter().any(|l| l.contains(r#""type":"playback""#)));
    drop(lock(&room.state));
}

/// Until the room rests at live.
pub(super) fn at_live(room: &Room) {
    until("the room at live", || {
        let st = lock(&room.state);
        st.seek.is_none() && st.at >= room.recording.length()
    });
}

/// `koth_viaduct` played up to tick 35084 on a live match; a room `open`ed on it once it has; then
/// the server changes map to `koth_nucleus` (the client's fault: a frozen frame, the map announced
/// again and again, then play). Whether the room followed when the map ended, and the map it is
/// on once the next had play.
fn through_a_change(open: impl FnOnce(&Arc<Recording>, u64) -> Arc<Room>) -> (bool, usize) {
    let rec = Arc::new(Recording::live());
    let mut s = Stream { r: &rec, s: 0.0 };
    let last = viaduct(&mut s);
    let room = open(&rec, last);
    let mut change = s.change_to_frozen("koth_nucleus");
    let follows = decided(&room, 0);
    s.after_frozen(&mut change, "koth_nucleus", 3, 250, 5);
    assert!(rec.part(1).unwrap().ready);
    if follows {
        on_map(&room, 1);
    }
    a_pass(&room);
    let part = lock(&room.state).part;
    room.stop();
    (follows, part)
}

#[test]
fn a_room_at_live_through_a_stream_that_never_stops_goes_on_with_the_match() {
    let rec = Arc::new(Recording::live());
    let mut s = Stream { r: &rec, s: 0.0 };
    s.signon(0.0, "cp_a");
    s.play(50, 2);
    let room = Room::start("r".into(), "me", rec.clone());
    room.viewers.fetch_add(1, Ordering::Relaxed);
    at_live(&room);
    // Frames 15 to 50 ms apart as they come, for 3 s; someone draws midway.
    let (started, from) = (Instant::now(), s.s);
    let mut tick = 200;
    for i in 0u64.. {
        std::thread::sleep(Duration::from_millis(15 + i * 7 % 36));
        tick += 2;
        let now = from + started.elapsed().as_secs_f64();
        s.push(now - s.s, &snapshot(tick, None, Vec::new()));
        if i == 40 {
            room.draw(&Draw { stroke: 1, points: vec![[1.0, 2.0, 0.0]], end: true }, "me").unwrap();
        }
        if started.elapsed() > Duration::from_secs(3) {
            break;
        }
    }
    assert!(!lock(&room.state).follows, "drawn in: nothing decided before the map ends");
    s.change("cp_b", 250, 5);
    on_map(&room, 1);
    let p = playback(&room);
    assert_eq!((p["live"].as_bool(), p["paused"].as_bool()), (Some(true), Some(false)));
    assert_eq!(lock(&room.hub).map.as_deref(), Some("cp_b"));
    room.stop();
}

#[test]
fn a_room_at_live_when_its_map_ends_goes_on_and_one_away_from_it_stays() {
    let live = |rec: &Arc<Recording>, _| {
        let room = Room::start("r".into(), "me", rec.clone());
        at_live(&room);
        room
    };
    assert_eq!(through_a_change(live), (true, 1));
    // Unwatched, these play nothing: they are where they were put when the map ends.
    let at = |behind: u64, speed| {
        move |rec: &Arc<Recording>, last: u64| Room::start_at("r".into(), "me", rec.clone(), last - behind, speed, false)
    };
    assert_eq!(through_a_change(at(2000, 1.0)), (true, 1), "2 s behind is at live");
    assert_eq!(through_a_change(at(10_000, 1.0)), (false, 0), "10 s behind");
    assert_eq!(through_a_change(at(2000, 2.0)), (false, 0), "at 2x");
    let paused = |rec: &Arc<Recording>, last| {
        let room = live(rec, last);
        room.control(&Control { paused: Some(true), ..Control::default() }).unwrap();
        room
    };
    assert_eq!(through_a_change(paused), (false, 0), "paused at live");
}

/// The kinds of line `page` was sent before the room's next seek (a move).
fn sent_before_a_seek(page: &std::sync::mpsc::Receiver<Arc<str>>) -> Vec<String> {
    let lines = page.try_iter().map(|l| serde_json::from_str::<Value>(&l).unwrap());
    lines.map(|l| l["type"].as_str().unwrap_or_default().to_string()).take_while(|t| t != "seek").collect()
}

#[test]
fn a_room_plays_nothing_of_its_maps_tail_and_moves_after_the_frozen_frame_once_there_is_play() {
    let rec = Arc::new(Recording::live());
    let mut s = Stream { r: &rec, s: 0.0 };
    viaduct(&mut s);
    let room = Room::start("r".into(), "me", rec.clone());
    room.viewers.fetch_add(1, Ordering::Relaxed);
    at_live(&room);
    let page = subscribe(&room.hub, false);
    let _joined: Vec<_> = page.try_iter().collect();
    let mut change = s.change_to_frozen("koth_nucleus");
    assert!(decided(&room, 0));
    // It read the tail up to the frozen frame, and stays.
    a_pass(&room);
    assert_eq!(lock(&room.state).part, 0, "a frozen frame is not play");
    assert_eq!(playback(&room)["maps"].as_array().unwrap().len(), 1, "nor a map to go to");
    let h = lock(&room.hub);
    assert!(h.map.as_deref() == Some("koth_viaduct") && h.has_snapshot() && h.tick() == Some(35084), "the last moment");
    drop(h);
    s.after_frozen(&mut change, "koth_nucleus", 3, 250, 5);
    on_map(&room, 1);
    let sent = sent_before_a_seek(&page);
    assert!(!sent.iter().any(|t| ["status", "server", "world", "snapshot"].contains(&t.as_str())), "{sent:?}");
    assert_eq!(lock(&room.hub).map.as_deref(), Some("koth_nucleus"));
    room.stop();
}

#[test]
fn with_one_signon_a_room_waits_out_the_frozen_frame_and_goes_on_once_there_is_play() {
    let rec = Arc::new(Recording::live());
    let mut s = Stream { r: &rec, s: 0.0 };
    viaduct(&mut s);
    let room = Room::start("r".into(), "me", rec.clone());
    room.viewers.fetch_add(1, Ordering::Relaxed);
    at_live(&room);
    let page = subscribe(&room.hub, false);
    let _joined: Vec<_> = page.try_iter().collect();
    // `fixed_change`, the frozen frame apart from the play.
    s.status(1.0, "changelevel");
    s.status(5.0, "map_change");
    s.signon(7.0, "koth_nucleus");
    s.push(15.0, &snapshot(68, None, Vec::new()));
    assert!(decided(&room, 0));
    a_pass(&room);
    assert_eq!(lock(&room.state).part, 0, "a frozen frame is not play");
    s.snapshot(80.0, 250);
    s.play(317, 2);
    on_map(&room, 1);
    let sent = sent_before_a_seek(&page);
    assert!(!sent.iter().any(|t| ["status", "server", "world", "snapshot"].contains(&t.as_str())), "{sent:?}");
    let (h, st) = (lock(&room.hub), lock(&room.state));
    assert_eq!((h.map.as_deref(), h.tick(), st.tick), (Some("koth_nucleus"), Some(317 + 67), Some(317 + 67)), "at live");
    drop((h, st));
    room.stop();
}

#[test]
fn a_room_restarted_following_goes_to_the_newest_map_with_play() {
    let rec = Arc::new(Recording::live());
    let mut s = Stream { r: &rec, s: 0.0 };
    let last = viaduct(&mut s);
    s.change("koth_nucleus", 250, 5);
    // A load that never plays, then one that does.
    s.change_to_frozen("koth_viaduct");
    s.change("cp_process_final", 250, 5);
    let ns: Vec<_> = rec.parts().iter().map(|p| (p.n, p.ready)).collect();
    assert_eq!(ns, [(0, true), (1, true), (2, false), (3, true)]);
    let mut saved: Value = serde_json::from_str(OLD_SAVED_ROOM).unwrap();
    (saved["part"], saved["at"], saved["paused"]) = (0.into(), last.into(), false.into());
    (saved["follows"], saved["decided"]) = (true.into(), true.into());
    let room = Room::restore("r".into(), serde_json::from_value(saved).unwrap(), rec.clone());
    on_map(&room, 3);
    assert_eq!(lock(&room.hub).map.as_deref(), Some("cp_process_final"));
    room.stop();
}
