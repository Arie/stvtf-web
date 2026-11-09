use super::drawing::MAX_KEPT_POINTS;
use super::play::wait;
use super::relay::on_course;
use super::view::View;
use super::*;
use crate::matches::hub::subscribe;
use std::fmt::Write as _;
use stvtf::Line;

/// `n` feed lines, a tick (15 ms) apart.
fn room(n: u32) -> Arc<Room> {
    let mut lines = String::new();
    for tick in 1..=n {
        writeln!(lines, "{{\"type\":\"stalemate\",\"tick\":{tick}}}").unwrap();
    }
    Room::start("r".into(), "me", Arc::new(Recording::read(lines.as_bytes(), "test").unwrap()))
}

fn until(what: &str, done: impl Fn() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(started.elapsed() < Duration::from_secs(5), "waited for {what}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn wait_follows_the_speed() {
    assert_eq!(wait(1000, 2.0), Duration::from_millis(500));
    assert_eq!(wait(1000, 0.25), Duration::from_secs(4));
}

#[test]
fn a_room_plays_its_recording_while_watched() {
    let r = room(10);
    std::thread::sleep(Duration::from_millis(50));
    assert!(r.hub.lock().unwrap().history.is_empty(), "nobody watches yet");
    let _w = r.join("me", None, None, true);
    until("every line", || r.hub.lock().unwrap().history.len() == 10);
    r.stop();
}

#[test]
fn a_paused_room_publishes_nothing() {
    let r = room(10);
    r.control(&Control { paused: Some(true), ..Control::default() }).unwrap();
    let _w = r.join("me", None, None, true);
    std::thread::sleep(Duration::from_millis(100));
    assert!(r.hub.lock().unwrap().history.is_empty());
    r.stop();
}

#[test]
fn a_seek_reprimes_the_pages() {
    let r = room(10);
    r.control(&Control { paused: Some(true), ..Control::default() }).unwrap();
    let page = subscribe(&r.hub, false);
    r.control(&Control { seek: Some(60), ..Control::default() }).unwrap();
    until("the seek", || page.try_iter().any(|l| l.contains(r#""type":"seek""#)));
    // Ticks 1 to 4 (0 to 45 ms) are before 60 ms.
    assert_eq!(r.hub.lock().unwrap().history.len(), 4);
    r.stop();
}

#[test]
fn the_end_pauses_at_the_length() {
    let r = room(3);
    let _w = r.join("me", None, None, true);
    until("the end", || lock(&r.state).paused);
    assert_eq!(lock(&r.state).at, 30);
    r.stop();
}

#[test]
fn strokes_reach_every_page_and_stay_on_the_map() {
    let r = room(1);
    let page = subscribe(&r.hub, false);
    r.draw(&Draw { stroke: 7, points: vec![[1.0, 2.0, 0.0], [3.5, -4.0, 16.0]], keep: false }, "me").unwrap();
    let line = page.try_iter().find(|l| l.contains(r#""type":"draw""#)).unwrap();
    assert_eq!(&*line, r#"{"by":1,"points":[[1.0,2.0,0.0],[3.5,-4.0,16.0]],"stroke":7,"type":"draw"}"#);
    assert!(r.draw(&Draw { stroke: 7, points: vec![], keep: false }, "me").is_err());
    assert!(r.draw(&Draw { stroke: 7, points: vec![[1e9, 0.0, 0.0]], keep: false }, "me").is_err());
    assert!(r.draw(&Draw { stroke: 7, points: vec![[0.0, 0.0, -1.0]], keep: false }, "me").is_err());
    assert!(r.draw(&Draw { stroke: 7, points: vec![[0.0, 0.0, 0.0]; 257], keep: false }, "me").is_err());
    r.stop();
}

#[test]
fn marker_stays_for_pages_that_join_until_cleared() {
    let r = room(1);
    let marker = |stroke, x| Draw { stroke, points: vec![[x, 0.0, 0.0]], keep: true };
    r.draw(&marker(1, 1.0), "me").unwrap();
    r.draw(&marker(1, 2.0), "me").unwrap();
    r.draw(&Draw { stroke: 2, points: vec![[9.0, 9.0, 0.0]], keep: false }, "me").unwrap();
    let joined = |r: &Room| subscribe(&r.hub, false).try_iter().filter(|l| l.contains(r#""type":"marker""#)).collect::<Vec<_>>();
    let kept = joined(&r);
    assert_eq!(&*kept[0], r#"{"max":8000,"strokes":[{"by":1,"points":[[1.0,0.0,0.0],[2.0,0.0,0.0]],"stroke":1}],"type":"marker"}"#);
    // Picked up again after a restart.
    let saved: Saved = serde_json::from_str(&serde_json::to_string(&r.saved()).unwrap()).unwrap();
    let again = Room::restore("r2".into(), saved, r.recording.clone());
    assert_eq!(joined(&again), kept);
    let page = subscribe(&r.hub, false);
    r.clear("me");
    assert!(page.try_iter().any(|l| &*l == r#"{"by":1,"type":"clear"}"#));
    assert_eq!(&*joined(&r)[0], r#"{"max":8000,"strokes":[],"type":"marker"}"#, "nothing kept, said so");
    assert_eq!(r.saved().marker.len(), 0);
    let full = Draw { stroke: 3, points: vec![[0.0, 0.0, 0.0]; 200], keep: true };
    let mut drawn = 0;
    while r.draw(&full, "me").is_ok() {
        drawn += 200;
    }
    assert_eq!(drawn, MAX_KEPT_POINTS);
    r.stop();
    again.stop();
}

#[test]
fn pages_hear_who_is_in_the_room() {
    let r = room(1);
    let page = subscribe(&r.hub, false);
    let w = r.join("me", Some("coach"), None, true);
    let _v = r.join("you", None, None, true);
    let _overlay = r.join("cast", None, None, false);
    r.set_admin("me", 2, true).unwrap();
    drop(w);
    let people: Vec<_> = page.try_iter().filter(|l| l.contains(r#""type":"people""#)).collect();
    assert_eq!(
        &*people[people.len() - 1],
        r#"{"people":[{"admin":true,"id":2,"name":"Guest 2","owner":false,"signed":false}],"type":"people"}"#
    );
    assert!(r.is_admin("you"));
    r.stop();
}

#[test]
fn the_view_reaches_pages_that_join_later() {
    let r = room(1);
    r.set_view(&ViewPost { view: View::Follow { player: 4, w: None, h: None }, take: true, page: "a".into() }, "me").unwrap();
    let later: Vec<_> = subscribe(&r.hub, false).try_iter().filter(|l| l.contains(r#""type":"view""#)).collect();
    assert_eq!(&*later[0], r#"{"by":1,"mode":"follow","page":"a","player":4,"type":"view"}"#);
    let zoomed = View::Follow { player: 4, w: Some(3000.0), h: Some(2000.0) };
    r.set_view(&ViewPost { view: zoomed, take: false, page: "a".into() }, "me").unwrap();
    let later: Vec<_> = subscribe(&r.hub, false).try_iter().filter(|l| l.contains(r#""type":"view""#)).collect();
    assert_eq!(&*later[0], r#"{"by":1,"h":2000.0,"mode":"follow","page":"a","player":4,"type":"view","w":3000.0}"#);
    let half = View::Follow { player: 4, w: Some(3000.0), h: None };
    assert!(r.set_view(&ViewPost { view: half, take: true, page: String::new() }, "me").is_err());
    let flat = View::Admin { cx: 0.0, cy: 0.0, w: 0.0, h: 10.0, rot: 0.0 };
    assert!(r.set_view(&ViewPost { view: flat, take: true, page: String::new() }, "me").is_err());
    r.stop();
}

#[test]
fn a_camera_moving_counts_only_while_the_view_is_of_its_kind() {
    let r = room(1);
    let post = |view, take, page: &str| ViewPost { view, take, page: page.into() };
    let mine = |x: f64| View::Admin { cx: x, cy: 0.0, w: 10.0, h: 10.0, rot: 0.0 };
    let latest = || subscribe(&r.hub, false).try_iter().find(|l| l.contains(r#""type":"view""#));
    r.set_view(&post(mine(1.0), false, "a"), "me").unwrap();
    assert!(latest().is_none(), "nobody chose a view yet");
    r.set_view(&post(mine(1.0), true, "a"), "me").unwrap();
    // Any admin's page moving it leads it from then on.
    r.set_view(&post(mine(2.0), false, "c"), "you").unwrap();
    assert!(latest().unwrap().contains(r#""page":"c""#));
    // A move sent before auto zoom was chosen does not bring the admin's view back.
    r.set_view(&post(View::Auto, true, "a"), "me").unwrap();
    r.set_view(&post(mine(3.0), false, "c"), "you").unwrap();
    assert_eq!(&*latest().unwrap(), r#"{"by":1,"mode":"auto","page":"a","type":"view"}"#);
    r.stop();
}

#[test]
fn a_room_plays_on_from_one_chunk_into_the_next() {
    // 0 to 15 s of lines, 0.5 s apart: two chunks, the second from 10 s.
    let mut lines = String::new();
    for tick in (0..=1000).step_by(33) {
        writeln!(lines, "{{\"type\":\"stalemate\",\"tick\":{}}}", tick + 1).unwrap();
    }
    let rec = Arc::new(Recording::read(lines.as_bytes(), "test").unwrap());
    assert!(rec.chunks() > 1);
    let r = Room::start("r".into(), "me", rec);
    let _w = r.join("me", None, None, true);
    r.control(&Control { seek: Some(9_600), speed: Some(4.0), ..Control::default() }).unwrap();
    until("the next chunk", || lock(&r.state).at > 10_500);
    assert!(!lock(&r.state).paused, "still playing");
    r.stop();
}

#[test]
fn catching_up_with_a_live_match_plays_on_at_1x() {
    let rec = Arc::new(Recording::live());
    let mut time = 1_700_000_000_000;
    for _ in 0..10 {
        let text = r#"{"type":"stalemate","tick":5}"#;
        let mut stamped: stvtf::output::Stamped = serde_json::from_str(text).unwrap();
        stamped.time = Some(time);
        rec.push(&stamped, text).unwrap();
        time += 100;
    }
    let r = Room::start("r".into(), "me", rec);
    let _w = r.join("me", None, None, true);
    r.control(&Control { seek: Some(0), speed: Some(4.0), ..Control::default() }).unwrap();
    until("live again", || lock(&r.state).at >= 900);
    until("1x", || (lock(&r.state).speed - 1.0).abs() < f64::EPSILON);
    r.stop();
}

#[test]
fn a_room_behind_on_the_old_map_goes_on_to_the_new() {
    let rec = Arc::new(Recording::live());
    let push = |line: &Line, time: u64| {
        let text = serde_json::to_string(line).unwrap();
        let mut stamped: stvtf::output::Stamped = serde_json::from_str(&text).unwrap();
        stamped.time = Some(time);
        rec.push(&stamped, &text).unwrap();
    };
    let t0 = 1_700_000_000_000;
    push(&crate::testing::server("cp_one"), t0);
    let r = Room::start_at("r".into(), "me", rec.clone(), 0, 1.0, true);
    push(&crate::testing::server("cp_two"), t0 + 500);
    until("the new map's start", || lock(&r.state).at == 500);
    let _w = r.join("me", None, None, true);
    r.control(&Control { paused: Some(false), ..Control::default() }).unwrap();
    until("the new map", || r.hub.lock().unwrap().map.as_deref() == Some("cp_two"));
    r.stop();
}

#[test]
fn a_relay_is_told_again_only_when_the_room_changes_course() {
    use stvtf::relay::{Camera, Want};
    let t0 = Instant::now();
    let at = |ms: u64| t0 + Duration::from_millis(ms);
    let sent = Want { demo: "a.dem".into(), tick: 1000, at: t0, paused: false, speed: 2.0, from_start: false, camera: None };
    // A second on at 2x, 15 ms a tick: 133 ticks on.
    let now = |tick, ms| Want { tick, at: at(ms), ..sent.clone() };
    assert!(on_course(&sent, &now(1133, 1000), 0.015));
    assert!(on_course(&sent, &now(1140, 1000), 0.015), "a snapshot off");
    assert!(!on_course(&sent, &now(1000, 1000), 0.015), "held still (a live match waiting)");
    assert!(!on_course(&sent, &now(5000, 1000), 0.015), "a seek");
    assert!(!on_course(&sent, &Want { paused: true, ..now(1133, 1000) }, 0.015));
    assert!(!on_course(&sent, &Want { camera: Some(Camera::Follow { player: 3 }), ..now(1133, 1000) }, 0.015));
    assert!(!on_course(&sent, &Want { demo: "b.dem".into(), ..now(1133, 1000) }, 0.015));
    let paused = Want { paused: true, ..sent.clone() };
    assert!(on_course(&paused, &Want { at: at(5000), ..paused.clone() }, 0.015));
}

/// A paused room plays no snapshots: the tick its relay follows comes from where it went.
#[test]
fn a_seek_while_paused_moves_what_the_relay_follows() {
    let mut lines = String::new();
    for tick in (4..=4000).step_by(4) {
        writeln!(lines, "{}", serde_json::to_string(&crate::matches::hub::tests::snapshot(tick, None, Vec::new())).unwrap()).unwrap();
    }
    let room = Room::start("r".into(), "me", Arc::new(Recording::read(lines.as_bytes(), "test").unwrap()));
    room.control(&Control { paused: Some(true), ..Control::default() }).unwrap();
    room.control(&Control { seek: Some(30_000), ..Control::default() }).unwrap();
    // 30 s at 15 ms a tick.
    until("the tick of the moment sought", || lock(&room.state).tick.is_some_and(|t| t.abs_diff(2000) <= 4));
    room.stop();
}

#[test]
fn only_listed_speeds_pass() {
    let r = room(1);
    assert!(r.control(&Control { speed: Some(3.0), ..Control::default() }).is_err());
    r.control(&Control { speed: Some(0.25), ..Control::default() }).unwrap();
    r.stop();
}

#[test]
fn a_room_behind_a_live_match_that_ended_skips_to_its_end() {
    let rec = Arc::new(Recording::live());
    let mut time = 1_700_000_000_000;
    for _ in 0..300 {
        let text = r#"{"type":"stalemate","tick":5}"#;
        let mut stamped: stvtf::output::Stamped = serde_json::from_str(text).unwrap();
        stamped.time = Some(time);
        rec.push(&stamped, text).unwrap();
        time += 100;
    }
    let r = Room::start("r".into(), "me", rec.clone());
    let _w = r.join("me", None, None, true);
    r.control(&Control { seek: Some(1_000), ..Control::default() }).unwrap();
    until("rewound", || lock(&r.state).at < 2_000 && lock(&r.state).seek.is_none());
    rec.end();
    r.control(&Control { seek: Some(rec.length()), paused: Some(false), ..Control::default() }).unwrap();
    until("the end", || lock(&r.state).at == rec.length());
    r.stop();
}
