//! A room and its relay. The relay process is a stand-in that keeps what it is asked.
use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Write as _};
use std::os::unix::net::UnixListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde_json::{Value, json};
use stvtf::Line;
use stvtf::output::{PlayerId, Voice};
use stvtf::relay::Want;

use super::drawing::Draw;
use super::light::LightDraw;
use super::note::NoteDraw;
use super::play::Control;
use super::relay::RELAY_RETRY;
use super::state::Room;
use super::tests::{arc_room, until};
use crate::matches::hub::subscribe;
use crate::matches::hub::tests::snapshot;
use crate::matches::recording::Recording;
use crate::relays::daemon::Remote;
use crate::relays::pool::{Offer, Pool, Relays, RoomRelay};
use crate::sync::lock;
use crate::testing::{TempDir, free_port};

/// Stands in for the relay process: a relay for every room, its TF2 clients and end as set.
struct RelayProcess {
    pool: Arc<Pool>,
    asked: Arc<Mutex<Vec<Value>>>,
    clients: Arc<AtomicUsize>,
    stopped: Arc<AtomicBool>,
    _dir: TempDir,
}

impl RelayProcess {
    fn new() -> RelayProcess {
        let dir = TempDir::new("relay-stand-in");
        let socket = dir.join("relay.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let (asked, clients, stopped): (Arc<Mutex<Vec<Value>>>, Arc<AtomicUsize>, Arc<AtomicBool>) = Default::default();
        let (log, tf2, gone) = (asked.clone(), clients.clone(), stopped.clone());
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                let (log, tf2, gone) = (log.clone(), tf2.clone(), gone.clone());
                std::thread::spawn(move || {
                    let mut out = conn.try_clone().unwrap();
                    let line = json!({ "type": "relay", "connect": "127.0.0.1:27100", "port": 27100, "password": "pw" });
                    for request in BufReader::new(conn).lines().map_while(Result::ok) {
                        let request: Value = serde_json::from_str(&request).unwrap();
                        let answer = match request["op"].as_str() {
                            Some("start") => json!({ "line": line }),
                            Some("status") => {
                                json!({ "clients": tf2.load(Ordering::Relaxed), "stopped": gone.load(Ordering::Relaxed), "line": line })
                            }
                            _ => json!({}),
                        };
                        lock(&log).push(request);
                        if writeln!(out, "{answer}").is_err() {
                            break;
                        }
                    }
                });
            }
        });
        RelayProcess { pool: Arc::new(Pool::Remote(Remote::new(&socket))), asked, clients, stopped, _dir: dir }
    }

    fn offer(&self) -> Offer {
        offer(self.pool.clone())
    }

    /// What it was asked to `op`, in order.
    fn asked(&self, op: &str) -> Vec<Value> {
        lock(&self.asked).iter().filter(|r| r["op"] == op).cloned().collect()
    }

    /// The first `op` it was asked that is `right`, once it is.
    fn hears(&self, op: &str, right: impl Fn(&Value) -> bool) -> Value {
        let found = || self.asked(op).into_iter().find(|r| right(r));
        until(&format!("the relay process asked to {op}"), || found().is_some());
        found().unwrap()
    }
}

/// Relays from `pool`, every demo at hand.
fn offer(pool: Arc<Pool>) -> Offer {
    Offer { relays: pool, demo_of: Arc::new(|_, _| Some("a.dem".into())) }
}

/// A room on a minute of snapshots, 4 ticks (60 ms) apart.
fn room_of_snapshots() -> Arc<Room> {
    let mut lines = String::new();
    for tick in (4..=4000).step_by(4) {
        writeln!(lines, "{}", serde_json::to_string(&snapshot(tick, None, Vec::new())).unwrap()).unwrap();
    }
    Room::start("r".into(), "me", Arc::new(Recording::read(lines.as_bytes(), "test").unwrap()))
}

/// Paused, half a minute in: tick 2000.
fn seek_to_half_a_minute(room: &Room) {
    room.control(&Control { paused: Some(true), ..Control::default() }).unwrap();
    room.control(&Control { seek: Some(30_000), ..Control::default() }).unwrap();
}

fn near_tick_2000(tick: u64) -> bool {
    tick.abs_diff(2000) <= 4
}

/// A line on the low floor of [`arc_room`], from x 16 to 496.
fn stroke(stroke: u32, y: f64) -> Draw {
    Draw { stroke, points: vec![[16.0, y, 0.0], [496.0, y, 16.0]], end: true }
}

/// A room on `cp_test` with someone on a page and a relay in `relays`.
fn relayed_room(relays: &RelayProcess) -> (Arc<Room>, super::state::Watching, TempDir) {
    let (room, dir) = arc_room();
    room.offer_relay(relays.offer());
    let page = room.join("me", None, None, true);
    assert!(lock(&room.relay.current).is_some());
    (room, page, dir)
}

/// The room hears of `n` TF2 clients in its relay.
fn tf2_clients(relays: &RelayProcess, room: &Room, n: usize) {
    relays.clients.store(n, Ordering::Relaxed);
    until("the room to hear of the TF2 clients", || room.relay_clients() == n);
}

#[test]
fn a_room_gets_a_relay_when_someone_is_on_a_page_and_tells_it_where_it_goes() {
    let port = free_port();
    let room = room_of_snapshots();
    let page = subscribe(&room.hub, false);
    room.offer_relay(offer(Arc::new(Pool::Local(Relays::new("127.0.0.1", [port])))));
    let _overlay = room.join("me", None, None, false);
    assert!(lock(&room.relay.current).is_none(), "nobody to connect from TF2");
    let _page = room.join("me", None, None, true);
    let line = page.try_iter().filter(|l| l.contains(r#""type":"relay""#)).last().expect("the pages are told");
    let line: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(line["connect"], format!("127.0.0.1:{port}"));
    assert_eq!(room.saved().relay.map(|was| was.port), Some(port), "kept for the next stv.tf");
    seek_to_half_a_minute(&room);
    until("the relay told of the seek", || lock(&room.relay.sent).as_ref().is_some_and(|w| w.paused && near_tick_2000(w.tick.into())));
    room.stop();
    assert!(lock(&room.relay.current).is_none());
}

#[test]
fn a_relay_that_cannot_start_is_tried_again_after_a_while() {
    let taken = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
    let port = taken.local_addr().unwrap().port();
    let room = room_of_snapshots();
    room.offer_relay(offer(Arc::new(Pool::Local(Relays::new("127.0.0.1", [port])))));
    let _page = room.join("me", None, None, true);
    let failed = lock(&room.relay.failed).clone().expect("the port is taken");
    assert!(failed.1.contains(&port.to_string()), "{}", failed.1);
    drop(taken);
    room.tend_relay();
    assert!(lock(&room.relay.current).is_none(), "not tried again yet");
    lock(&room.relay.failed).as_mut().unwrap().0 = Instant::now().checked_sub(RELAY_RETRY).unwrap();
    room.tend_relay();
    assert!(lock(&room.relay.current).is_some());
    assert!(lock(&room.relay.failed).is_none());
    room.stop();
}

#[test]
fn a_room_with_every_port_taken_gets_its_relay_once_one_is_free() {
    let relays = Relays::new("127.0.0.1", [free_port()]);
    let want = Want { demo: "a.dem".into(), tick: 0, at: Instant::now(), paused: false, speed: 1.0, from_start: false, camera: None };
    let other = relays.start(want, None).unwrap().unwrap();
    let room = room_of_snapshots();
    room.offer_relay(offer(Arc::new(Pool::Local(relays))));
    let _page = room.join("me", None, None, true);
    assert!(lock(&room.relay.current).is_none() && lock(&room.relay.failed).is_none());
    drop(other);
    until("the port to come back", || {
        room.tend_relay();
        lock(&room.relay.current).is_some()
    });
    room.stop();
}

#[test]
fn a_room_tells_the_relay_process_where_it_goes_and_ends_its_relay_with_itself() {
    let relays = RelayProcess::new();
    let room = room_of_snapshots();
    room.offer_relay(relays.offer());
    let _page = room.join("me", None, None, true);
    let started = relays.hears("start", |_| true);
    assert_eq!((started["room"].as_str(), started["want"]["demo"].as_str()), (Some("r"), Some("a.dem")));
    assert_eq!(room.saved().relay.map(|was| (was.port, was.password)), Some((27100, "pw".into())));
    seek_to_half_a_minute(&room);
    relays.hears("follow", |f| f["want"]["paused"] == true && f["want"]["tick"].as_u64().is_some_and(near_tick_2000));
    room.stop();
    relays.hears("stop", |s| s["room"] == "r");
}

#[test]
fn a_relay_that_stopped_is_ended_and_another_started() {
    let relays = RelayProcess::new();
    let (room, _page, _dir) = relayed_room(&relays);
    relays.stopped.store(true, Ordering::Relaxed);
    until("the room to hear its relay stopped", || lock(&room.relay.current).as_ref().is_some_and(RoomRelay::stopped));
    relays.stopped.store(false, Ordering::Relaxed);
    room.tend_relay();
    assert_eq!(relays.asked("start").len(), 2);
    assert!(lock(&room.relay.current).as_ref().is_some_and(|r| !r.stopped()));
    relays.hears("stop", |_| true);
    room.stop();
}

#[test]
fn the_marker_waits_for_tf2_clients_and_then_goes_to_the_relay_as_it_changes() {
    let relays = RelayProcess::new();
    let (room, _page, _dir) = relayed_room(&relays);
    room.set_ink("me", "#ff0000").unwrap();
    room.draw(&stroke(1, 100.0), "me").unwrap();
    assert!(room.marker.dirty.load(Ordering::Relaxed), "nobody in TF2 to show it to");
    tf2_clients(&relays, &room, 1);
    room.tend_relay();
    assert!(!room.marker.dirty.load(Ordering::Relaxed));
    let beams = |keep: &Value| keep["beams"].as_array().unwrap().clone();
    let first = beams(&relays.hears("keep", |_| true));
    assert_ne!(first, Vec::<Value>::new());
    assert!(first.iter().all(|b| b[6] == 255.0 && b[7] == 0.0 && b[8] == 0.0), "in the ink it was drawn in");
    room.draw(&stroke(2, 200.0), "me").unwrap();
    relays.hears("keep", |k| beams(k).len() > first.len());
    room.note(&NoteDraw { text: "lobby".into(), at: Some([100.0, 300.0]), player: None }, "me").unwrap();
    relays.hears("notes", |n| n["notes"][0]["text"] == "lobby");
    room.light(&LightDraw { at: [100.0, 400.0] }, "me").unwrap();
    relays.hears("light", |l| l["light"]["at"][0] == 100.0);
    room.clear("me");
    until("the cleared marker", || relays.asked("keep").last().is_some_and(|k| beams(k).is_empty()));
    until("the light put out", || relays.asked("light").last().is_some_and(|l| l["light"].is_null()));
    room.stop();
}

#[test]
fn a_marker_of_more_than_tf2_shows_is_refused_while_tf2_clients_are_in() {
    let relays = RelayProcess::new();
    let (room, _page, _dir) = relayed_room(&relays);
    tf2_clients(&relays, &room, 1);
    // Back and forth over the floor, 50 lines a piece.
    let piece = |n: u32| Draw {
        stroke: n,
        points: (0..50).map(|i| [if i % 2 == 0 { 16.0 } else { 496.0 }, 16.0 + f64::from(i) * 8.0, f64::from(i)]).collect(),
        end: true,
    };
    let refused = (1..=20).find_map(|n| room.draw(&piece(n), "me").err().map(|e| (n, e.to_string()))).expect("refused in the end");
    assert_eq!(refused.1, "TF2 cannot show more marker: clear it first.");
    assert_eq!(room.current_marker().len(), refused.0 as usize - 1, "the marker as it was");
    room.stop();
}

#[test]
fn voice_from_outside_the_match_goes_to_the_relay_process() {
    let relays = RelayProcess::new();
    let (room, _page, _dir) = relayed_room(&relays);
    room.relay_line(&Line::Voice(Voice {
        tick: 1,
        player: Some(PlayerId(7)),
        steamid: "[U:1:5]".into(),
        rate: 24000,
        frames: vec!["aAEC".into()],
        injected: true,
        pause: false,
        team: None,
        name: None,
    }));
    let said = relays.hears("voice", |_| true);
    assert_eq!(said["voice"], json!([{ "user": 7, "steamid": "[U:1:5]", "rate": 24000, "frames": ["aAEC"] }]));
    room.stop();
}

#[test]
fn the_rooms_names_go_to_the_relay_process_as_they_change_and_to_a_relay_started_later() {
    let relays = RelayProcess::new();
    let (room, _page, _dir) = relayed_room(&relays);
    let names = |player: &str| super::names::Names {
        players: [("[U:1:5]".to_string(), player.to_string())].into(),
        teams: [("red".to_string(), "Reds".to_string())].into(),
    };
    room.set_names(&names("Five")).unwrap();
    let said = relays.hears("names", |r| r["names"]["players"]["[U:1:5]"] == "Five");
    assert_eq!(said["names"]["teams"], json!({ "red": "Reds" }));
    // Its relay stopped: the next one is told the names as it starts.
    relays.stopped.store(true, Ordering::Relaxed);
    until("the room to hear its relay stopped", || lock(&room.relay.current).as_ref().is_some_and(RoomRelay::stopped));
    relays.stopped.store(false, Ordering::Relaxed);
    room.tend_relay();
    until("the names again", || relays.asked("names").len() == 2);
    room.stop();
}
