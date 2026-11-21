use std::time::{Duration, Instant};

use stvtf::relay::{Beam, Camera, Path as CameraPath, Step, VoiceIn, Want};

use super::wire::*;
use super::*;
use crate::relays::pool::tests::want;
use crate::testing::{TempDir, free_port};

/// A path reaches the relay process as it is; an old stv.tf's spot is one too.
#[test]
fn a_cameras_path_goes_to_the_relay_process_and_an_old_spot_is_one_too() {
    let step = |secs, x| Step { secs, pos: [x, 0.0, 900.0], aim: [0.0, 0.0, 300.0] };
    let began = Instant::now().checked_sub(Duration::from_secs(2)).unwrap();
    let path = Arc::new(CameraPath { id: 7, began, steps: vec![step(0.0, 900.0), step(4.0, 0.5)] });
    let sent = serde_json::to_string(&WantLine::of(&Want { camera: Some(Camera::Along(path.clone())), ..want() })).unwrap();
    let Some(Camera::Along(got)) = serde_json::from_str::<WantLine>(&sent).unwrap().want().camera else { panic!() };
    assert_eq!((got.id, &got.steps), (7, &path.steps));
    let age = got.began.elapsed();
    assert!(age >= Duration::from_secs(2) && age < Duration::from_secs(3), "{age:?}");
    // As a stv.tf from before paths says a spot: where, pitch and yaw.
    let old = |fixed: &str| {
        let line = sent.split_once(r#","path""#).unwrap().0.to_string() + &format!(r#","fixed":{fixed}}}"#);
        let Some(Camera::Along(got)) = serde_json::from_str::<WantLine>(&line).unwrap().want().camera else { panic!() };
        got
    };
    let (down, again, other) = (old("[[0,0,1500],90,0]"), old("[[0,0,1500],90,0]"), old("[[10,0,1500],90,0]"));
    assert_eq!((down.steps.len(), down.steps[0].pos), (1, [0.0, 0.0, 1500.0]));
    assert!((down.steps[0].aim[2] - 500.0).abs() < 0.01, "{:?}", down.steps[0].aim);
    assert!(down.id == again.id && down.id != other.id);
}

/// A request asked while answering one of stv.tf's goes in its trace.
#[test]
fn names_go_to_the_relay_process_as_they_were() {
    let names =
        Names { players: [("[U:1:5]".to_string(), "Five".to_string())].into(), teams: [("red".to_string(), "Reds".to_string())].into() };
    let line = json!({ "op": "names", "room": "r", "names": names });
    let Ok(Request::Names { room, names: asked }) = serde_json::from_value(line) else { panic!("not a names request") };
    assert_eq!((room.as_str(), asked), ("r", names));
}

#[test]
fn a_request_carries_the_trace_it_was_asked_in() {
    crate::report::on_for_tests();
    let dir = TempDir::new("trace");
    let socket = dir.join("r.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || {
        let (conn, _) = listener.accept().unwrap();
        let mut out = conn.try_clone().unwrap();
        for line in BufReader::new(conn).lines() {
            writeln!(out, "{}", json!({ "asked": serde_json::from_str::<Value>(&line.unwrap()).unwrap() })).unwrap();
        }
    });
    let remote = Remote::new(&socket);
    let header = "0123456789abcdef0123456789abcdef-0123456789abcdef-0";
    let asked = crate::report::continued("t", Some(header), &[], || remote.call(&json!({ "op": "status" }))).unwrap();
    assert!(asked["asked"]["trace"].as_str().is_some_and(|t| t.starts_with("0123456789abcdef0123456789abcdef-")), "{asked}");
    assert!(remote.call(&json!({ "op": "status" })).unwrap()["asked"].get("trace").is_none());
}

#[test]
fn a_beam_goes_to_the_relay_process_as_it_was() {
    let b = Beam { from: [1.5, -2.0, 3.0], to: [4.0, 5.0, -6.25], rgb: [255, 128, 0], life: 4.0, width: 3.0, speed: 20 };
    let line: BeamLine = serde_json::from_value(serde_json::to_value(beam_line(&b)).unwrap()).unwrap();
    assert_eq!(beam(&line), b);
}

#[test]
fn the_light_goes_to_the_relay_process_as_it_was_and_none_puts_it_out() {
    let light = Light { at: [1.0, 2.0, 3.0], rgb: [255, 128, 0], radius: 500.0, exponent: 5 };
    let line = json!({ "op": "light", "room": "r", "light": light });
    let Ok(Request::Light { room, light: got }) = serde_json::from_value(line) else { panic!("not a light request") };
    assert_eq!((room.as_str(), got), ("r", Some(light)));
    let out = json!({ "op": "light", "room": "r", "light": null });
    assert!(matches!(serde_json::from_value(out), Ok(Request::Light { light: None, .. })));
}

#[test]
fn notes_go_to_the_relay_process_as_they_were() {
    let note = Note { id: 3, text: "lobby".into(), at: [1.0, 2.0, 3.0], player: Some(7) };
    let line = json!({ "op": "notes", "room": "r", "notes": [note.clone()] });
    let Ok(Request::Notes { room, notes }) = serde_json::from_value(line) else { panic!("not a notes request") };
    assert_eq!((room.as_str(), notes), ("r", vec![note]));
}

#[test]
fn voice_goes_to_the_relay_process_as_it_was() {
    let v = VoiceIn { user: Some(7), steamid: "[U:1:5]".into(), rate: 24000, frames: vec![vec![0x68, 1, 2], vec![3]] };
    let line = json!({ "op": "voice", "room": "r", "voice": [VoiceLine::of(&v)] });
    let Ok(Request::Voice { room, voice }) = serde_json::from_value(line) else { panic!("not a voice request") };
    assert_eq!((room.as_str(), voice.iter().filter_map(VoiceLine::voice).collect::<Vec<_>>()), ("r", vec![v]));
}

/// A relay process with one port, serving on a socket in `dir` once this returns.
fn serving(dir: &TempDir, lease: Duration) -> (PathBuf, u16) {
    static NEVER: AtomicBool = AtomicBool::new(false);
    let socket = dir.join("relay.sock");
    let port = free_port();
    let relays = Relays::new("127.0.0.1", [port]);
    let s = socket.clone();
    std::thread::spawn(move || serve(&s, &relays, lease, &NEVER));
    let until = Instant::now() + Duration::from_secs(5);
    while !socket.exists() {
        assert!(Instant::now() < until, "no socket");
        std::thread::sleep(Duration::from_millis(10));
    }
    (socket, port)
}

/// Room `room`'s relay, once a port is free again.
fn start_when_free(remote: &Arc<Remote>, room: &str, was: Option<&Was>) -> RemoteRelay {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(r) = remote.start(room, &want(), was, true).unwrap() {
            return r;
        }
        assert!(Instant::now() < until, "the port never came back");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_relay_process_that_does_not_answer_is_left_alone_a_while() {
    let dir = TempDir::new("relay-away");
    let socket = dir.join("relay.sock");
    // Never answers.
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || {
        let held: Vec<UnixStream> = listener.incoming().flatten().collect();
        drop(held);
    });
    let remote = Remote::new(&socket);
    let first = Instant::now();
    assert!(remote.start("r", &want(), None, true).is_err());
    assert!(first.elapsed() >= ANSWER, "waited for it once");
    let again = Instant::now();
    assert!(remote.start("r", &want(), None, true).is_err());
    assert!(again.elapsed() < Duration::from_millis(50), "not again: {:?}", again.elapsed());
}

#[test]
fn nothing_waits_on_a_relay_in_the_relay_process() {
    let dir = TempDir::new("relay-nowait");
    let (socket, _) = serving(&dir, LEASE);
    let remote = Remote::new(&socket);
    let r = remote.start("room", &want(), None, true).unwrap().unwrap();
    // As a hung relay process would.
    let held = lock(&remote.conn);
    let t = Instant::now();
    for _ in 0..1000 {
        r.follow(&want());
        let _ = (r.clients(), r.stopped(), r.line());
    }
    assert!(t.elapsed() < Duration::from_millis(200), "{:?}", t.elapsed());
    drop(held);
    r.end();
}

#[test]
fn the_relay_process_says_how_many_tf2_clients_it_has() {
    let dir = TempDir::new("stvtf-relay-clients");
    let (socket, _) = serving(&dir, LEASE);
    assert_eq!(clients(&socket).unwrap(), json!({ "clients": 0 }));
    let _relay = start_when_free(&Remote::new(&socket), "r", None);
    assert_eq!(clients(&socket).unwrap(), json!({ "clients": 0 }));
}

/// What a process told to stop left: a room's relay comes back on its port and password,
/// showing what it showed. Too long ago, or not readable, it is left.
#[test]
fn a_relay_handed_over_is_taken_in_on_its_port_by_the_next_process() {
    let dir = TempDir::new("stvtf-relay-handed");
    let port = free_port();
    let was = Was { port, password: "abc123".into() };
    let handed = |at_ms| Handed {
        at_ms,
        rooms: vec![HandedRoom {
            room: "r".into(),
            was: was.clone(),
            want: WantLine::of(&want()),
            beams: vec![[0.0; 12]],
            notes: Vec::new(),
            light: None,
            names: Names::default(),
            score: [0, 0],
            clients: Handover::default(),
        }],
    };
    let file = dir.join("relay.handover");
    let relays = Relays::new("127.0.0.1", [port]);
    for (left, taken) in [
        (serde_json::to_vec(&handed(now_ms())).unwrap(), true),
        (serde_json::to_vec(&handed(now_ms() - 60_000)).unwrap(), false),
        (b"{".to_vec(), false),
    ] {
        std::fs::write(&file, left).unwrap();
        let rooms: Rooms = Arc::default();
        take_over(&rooms, &relays, &file);
        assert!(!file.exists(), "read once");
        let all = lock(&rooms);
        assert_eq!(all.get("r").map(|l| (l.relay.was(), l.shows.beams.len())), taken.then(|| (was.clone(), 1)));
    }
}

#[test]
fn a_relay_process_told_to_stop_leaves_nothing_of_rooms_without_tf2_clients() {
    let dir = TempDir::new("stvtf-relay-none");
    let relays = Relays::new("127.0.0.1", [free_port()]);
    let rooms: Rooms = Arc::default();
    let relay = relays.start(want(), None).unwrap().unwrap();
    lock(&rooms).insert("r".into(), Leased { relay, seen: Instant::now(), shows: Shows::of(want()) });
    let file = dir.join("relay.handover");
    assert_eq!(hand_over(&rooms, &file).unwrap(), 0);
    let left: Handed = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert!(left.rooms.is_empty() && lock(&rooms).is_empty());
}

/// The next stv.tf, picking a room up before it knows where the room is, leaves its relay
/// playing where it is.
#[test]
fn a_relay_found_again_plays_on_where_it_is_until_the_room_knows_its_place() {
    let relays = Relays::new("127.0.0.1", [free_port()]);
    let rooms: Rooms = Arc::default();
    let at = |tick| WantLine::of(&Want { tick, ..want() });
    let wanted = || lock(&rooms).get("r").map(|l| l.shows.want.tick);
    start("r".into(), at(500), true, None, &rooms, &relays);
    assert_eq!(wanted(), Some(500));
    start("r".into(), at(0), false, None, &rooms, &relays);
    assert_eq!(wanted(), Some(500), "a stand-in");
    start("r".into(), at(700), true, None, &rooms, &relays);
    assert_eq!(wanted(), Some(700));
}

#[test]
fn a_room_finds_its_relay_again_and_ends_it() {
    let dir = TempDir::new("relay-daemon");
    let (socket, port) = serving(&dir, Duration::from_secs(1));
    let remote = Remote::new(&socket);
    let a = remote.start("room-a", &want(), None, true).unwrap().unwrap();
    assert_eq!(a.line()["connect"], format!("127.0.0.1:{port}"));
    // The next stv.tf (another connection) finds the same relay, password and all.
    let next = Remote::new(&socket);
    let again = next.start("room-a", &want(), None, true).unwrap().unwrap();
    assert_eq!(again.line()["password"], a.line()["password"]);
    assert!(next.start("room-b", &want(), None, true).unwrap().is_none(), "the only port is taken");
    a.follow(&want());
    assert_eq!(a.clients(), 0);
    // Ended, its port is free for another room.
    again.end();
    let b = start_when_free(&next, "room-b", None);
    // Started again with what it was on, as after a restart of the relay process.
    let was = Was::of_line(&b.line()).unwrap();
    b.end();
    let b = start_when_free(&next, "room-b2", Some(&was));
    assert_eq!(Was::of_line(&b.line()), Some(was));
    // Not asked about for a lease, it goes.
    drop(b);
    std::thread::sleep(Duration::from_millis(2500));
    assert!(next.start("room-c", &want(), None, true).unwrap().is_some(), "the lease ran out");
}
