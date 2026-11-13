use super::*;
use crate::matches::hub::tests::snapshot;
use crate::testing::{TempDir, server};
use std::time::Duration;
use stvtf::demo::Files;

fn header(map: &str) -> Vec<u8> {
    stvtf::demo::Header {
        protocol: 24,
        server: String::new(),
        client: String::new(),
        map: map.into(),
        game: "tf".into(),
        seconds: 0.0,
        ticks: 0,
        frames: 0,
        signon_bytes: 0,
    }
    .bytes()
}

/// A connection's first frame, of `map` at `tick`, as a match's are: the server line, the demo
/// made, then the frame reaching the match's loads.
fn connect(demos: &LiveDemos, map: &str, tick: u32) {
    demos.note_line(&server(map));
    Write::write_all(&mut demos.create(map, tick).unwrap(), &header(map)).unwrap();
    demos.note_line(&snapshot(tick, None, Vec::new()));
}

/// Play up to `tick`.
fn play(demos: &LiveDemos, tick: u32) {
    demos.note_line(&snapshot(tick, None, Vec::new()));
}

#[test]
fn a_tick_is_found_in_the_demo_it_falls_in() {
    let dir = TempDir::new("live-demos");
    let demos = LiveDemos::new(&dir, "m");
    assert_eq!(demos.at(0, 5), None);
    connect(&demos, "cp_a", 100);
    play(&demos, 400);
    connect(&demos, "cp_a", 500);
    let (first, second) = (dir.join("live-m-0-100.dem"), dir.join("live-m-0-500.dem"));
    assert_eq!(demos.at(0, 50), Some(first.clone()), "before them all: the first");
    assert_eq!(demos.at(0, 499), Some(first.clone()));
    assert_eq!(demos.at(0, 500), Some(second.clone()));
    assert_eq!(demos.at(0, 9000), Some(second.clone()));
    assert_eq!(demos.at(1, 9000), None);
    // The next process finds them.
    assert_eq!(LiveDemos::new(&dir, "m").at(0, 9000), Some(second));
}

#[test]
fn each_load_has_its_demos_by_its_number() {
    let dir = TempDir::new("live-demos");
    let demos = LiveDemos::new(&dir, "m");
    connect(&demos, "cp_a", 100);
    play(&demos, 9000);
    connect(&demos, "cp_b", 10);
    play(&demos, 9000);
    connect(&demos, "cp_a", 20);
    assert_eq!(demos.at(0, 9000), Some(dir.join("live-m-0-100.dem")));
    assert_eq!(demos.at(1, 9000), Some(dir.join("live-m-1-10.dem")), "another map");
    assert_eq!(demos.at(2, 9000), Some(dir.join("live-m-2-20.dem")), "the same map again: its own");
    assert_eq!(demos.loads().latest(), 2);
    let next = LiveDemos::new(&dir, "m");
    assert_eq!((next.at(1, 25), next.at(2, 25)), (Some(dir.join("live-m-1-10.dem")), Some(dir.join("live-m-2-20.dem"))));
    assert_eq!(next.loads().would_be("cp_a", 30), 2, "the next process goes on from the latest");
    assert_eq!(next.loads().would_be("cp_b", 30), 3);
    demos.forget();
    assert_eq!(std::fs::read_dir(&*dir).unwrap().count(), 0);
}

#[test]
fn another_map_with_its_ticks_going_on_is_another_load() {
    let dir = TempDir::new("live-demos");
    let demos = LiveDemos::new(&dir, "m");
    connect(&demos, "cp_a", 100);
    play(&demos, 9000);
    connect(&demos, "cp_b", 9100);
    assert_eq!(demos.at(1, 9100), Some(dir.join("live-m-1-9100.dem")));
}

#[test]
fn the_same_map_loaded_again_right_away_has_demos_of_its_own() {
    let dir = TempDir::new("live-demos");
    let demos = LiveDemos::new(&dir, "m");
    connect(&demos, "cp_a", 68);
    play(&demos, 9000);
    connect(&demos, "cp_a", 68);
    assert_eq!(demos.at(0, 100), Some(dir.join("live-m-0-68.dem")));
    assert_eq!(demos.at(1, 100), Some(dir.join("live-m-1-68.dem")), "ticks started over: another load");
    assert_eq!(std::fs::read(dir.join("live-m-0-68.dem")).unwrap(), header("cp_a"), "neither overwritten");
    play(&demos, 300);
    connect(&demos, "cp_a", 400);
    assert_eq!(demos.at(1, 500), Some(dir.join("live-m-1-400.dem")), "a reconnect: the same load");
    assert_eq!(demos.at(2, 500), None);
}

#[test]
fn a_frozen_frame_then_a_reconnect_is_one_load() {
    let dir = TempDir::new("live-demos");
    let demos = LiveDemos::new(&dir, "m");
    connect(&demos, "koth_a", 100);
    play(&demos, 35084);
    // The new map's first frame, frozen; the client drops and connects again.
    connect(&demos, "koth_a", 68);
    connect(&demos, "koth_a", 244);
    assert_eq!(demos.at(1, 100), Some(dir.join("live-m-1-68.dem")));
    assert_eq!(demos.at(1, 300), Some(dir.join("live-m-1-244.dem")));
    assert_eq!(demos.at(2, 300), None);
}

#[test]
fn a_demo_named_by_the_version_before_is_the_first_load() {
    let dir = TempDir::new("live-demos");
    std::fs::write(dir.join("live-m-100.dem"), header("cp_a")).unwrap();
    assert_eq!(LiveDemos::new(&dir, "m").at(0, 150), Some(dir.join("live-m-100.dem")));
}

/// Demos on disk, some named without a load, in this order in time: (name, map).
fn on_disk(dir: &Path, demos: &[(&str, &str)]) {
    let now = SystemTime::now();
    for (i, (name, map)) in demos.iter().enumerate() {
        let path = dir.join(name);
        std::fs::write(&path, header(map)).unwrap();
        File::options().write(true).open(&path).unwrap().set_modified(now - Duration::from_secs(300 - i as u64 * 10)).unwrap();
    }
}

/// The match's recording, resumed, is at load `n` of `map`.
fn resumed_at(n: usize, map: &str) -> Loads {
    Loads::resumed(n, Some(map.into()), Some(5000))
}

#[test]
fn demos_named_by_the_version_before_are_given_loads_by_their_order_in_time() {
    let dir = TempDir::new("live-demos");
    on_disk(&dir, &[("live-m-193.dem", "cp_a"), ("live-m-210.dem", "cp_a"), ("live-m-69.dem", "cp_a")]);
    let demos = LiveDemos::new(&dir, "m");
    assert_eq!(demos.at(0, 205), Some(dir.join("live-m-193.dem")));
    assert_eq!(demos.at(0, 9000), Some(dir.join("live-m-210.dem")), "a reconnect: the same load");
    assert_eq!(demos.at(1, 9000), Some(dir.join("live-m-69.dem")), "a lower first tick: the next");
    assert_eq!(demos.loads().would_be("cp_a", 100), 1);
    // The recording resumed is at load 4: the newest are of it, those before of the one before.
    demos.go_on_from(resumed_at(4, "cp_a"));
    assert_eq!((demos.at(3, 9000), demos.at(4, 9000)), (Some(dir.join("live-m-210.dem")), Some(dir.join("live-m-69.dem"))));
    assert_eq!(demos.loads().latest(), 4);
}

#[test]
fn a_match_on_its_second_map_at_the_deploy_has_that_maps_demo_only() {
    let dir = TempDir::new("live-demos");
    on_disk(&dir, &[("live-m-250.dem", "cp_b")]);
    let demos = LiveDemos::new(&dir, "m");
    demos.go_on_from(resumed_at(1, "cp_b"));
    assert_eq!((demos.at(1, 300), demos.at(0, 300)), (Some(dir.join("live-m-250.dem")), None));
}

#[test]
fn demos_of_earlier_maps_from_the_version_before_are_not_relayed() {
    let dir = TempDir::new("live-demos");
    on_disk(&dir, &[("live-m-past-cp_a-68.dem", "cp_a"), ("kept-m-193.dem", "cp_a"), ("live-m-250.dem", "cp_b")]);
    std::fs::write(dir.join("kept-m-193.dem.voice"), "said\n").unwrap();
    let demos = LiveDemos::new(&dir, "m");
    demos.go_on_from(resumed_at(1, "cp_b"));
    assert_eq!((demos.at(1, 300), demos.at(0, 300)), (Some(dir.join("live-m-250.dem")), None));
    demos.forget();
    let mut left: Vec<String> = std::fs::read_dir(&*dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    left.sort();
    assert_eq!(left, ["kept-m-193.dem", "kept-m-193.dem.voice"], "the comms stay for downloading");
}

#[test]
fn a_reconnect_from_the_version_before_is_one_load() {
    let dir = TempDir::new("live-demos");
    on_disk(&dir, &[("live-m-68.dem", "cp_b"), ("live-m-250.dem", "cp_b")]);
    let demos = LiveDemos::new(&dir, "m");
    demos.go_on_from(resumed_at(1, "cp_b"));
    assert_eq!((demos.at(1, 100), demos.at(1, 300)), (Some(dir.join("live-m-68.dem")), Some(dir.join("live-m-250.dem"))));
    assert_eq!(demos.at(0, 300), None);
}

#[test]
fn ticks_starting_over_before_the_deploy_are_the_load_before() {
    let dir = TempDir::new("live-demos");
    on_disk(&dir, &[("live-m-250.dem", "cp_b"), ("live-m-68.dem", "cp_b")]);
    let demos = LiveDemos::new(&dir, "m");
    demos.go_on_from(resumed_at(2, "cp_b"));
    assert_eq!((demos.at(1, 300), demos.at(2, 300)), (Some(dir.join("live-m-250.dem")), Some(dir.join("live-m-68.dem"))));
}

#[test]
fn the_version_befores_demos_are_numbered_from_the_reconnect_after_the_deploy() {
    let dir = TempDir::new("live-demos");
    on_disk(&dir, &[("live-m-250.dem", "cp_b"), ("live-m-1-9000.dem", "cp_b"), ("live-m-2-50.dem", "cp_c")]);
    let demos = LiveDemos::new(&dir, "m");
    assert_eq!(demos.at(1, 300), Some(dir.join("live-m-250.dem")), "a second restart");
    demos.go_on_from(resumed_at(2, "cp_c"));
    assert_eq!(demos.at(1, 300), Some(dir.join("live-m-250.dem")));
    assert_eq!(demos.at(2, 300), Some(dir.join("live-m-2-50.dem")));
}

#[test]
fn the_version_befores_demos_of_another_map_than_the_recordings_are_not_relayed() {
    let dir = TempDir::new("live-demos");
    on_disk(&dir, &[("live-m-250.dem", "cp_a")]);
    let demos = LiveDemos::new(&dir, "m");
    demos.go_on_from(resumed_at(1, "cp_b"));
    assert_eq!((demos.at(0, 300), demos.at(1, 300)), (None, None));
    demos.forget();
    assert!(!dir.join("live-m-250.dem").exists());
}

/// A demo of `map` with frames at these server ticks.
fn write_demo(file: File, map: &str, ticks: &[i32]) {
    let mut w = stvtf::demo::Writer::new(std::io::BufWriter::new(file), stvtf::demo::Header::parse(&header(map)).unwrap(), 0.015).unwrap();
    w.signon(b"signon").unwrap();
    for t in ticks {
        w.message(*t, b"game").unwrap();
    }
    w.finish().unwrap();
}

fn spoken(tick: u32) -> Spoken {
    Spoken { tick, slot: 3, account: 5, frames: vec!["aA==".into()] }
}

#[test]
fn a_map_with_comms_is_kept_for_downloading_and_one_without_is_not() {
    let dir = TempDir::new("kept-demos");
    let demos = LiveDemos::new(&dir, "m");
    assert_eq!(downloads(&dir, "m"), []);
    demos.note_line(&server("cp_a"));
    write_demo(demos.create("cp_a", 100).unwrap(), "cp_a", &[100, 104, 110]);
    play(&demos, 100);
    demos.append_voice(&spoken(103));
    demos.append_voice(&spoken(108));
    let live = Download { file: "0-100".into(), map: "cp_a".into(), bytes: std::fs::metadata(dir.join("live-m-0-100.dem")).unwrap().len() };
    assert_eq!(downloads(&dir, "m"), std::slice::from_ref(&live), "while it is played, too");
    // The demo as TF2 plays it, the comms in it at their ticks.
    let (name, demo) = build(&dir, "m", "0-100").unwrap();
    assert_eq!(name, "cp_a-comms-0-100.dem");
    let mut r = stvtf::demo::Reader::new(std::io::Cursor::new(&demo)).unwrap();
    assert_eq!((r.header.map.as_str(), r.header.frames), ("cp_a", 5), "three frames and two of voice");
    let mut ticks = Vec::new();
    while let stvtf::demo::Next::Packet(p) = r.next_packet().unwrap() {
        if let stvtf::demo::Packet::Message { tick, .. } = p {
            ticks.push(tick);
        }
    }
    assert_eq!(ticks, [0, 3, 4, 8, 10]);
    // The next load: this one's stays as it is, so a relay on it is not moved to another file.
    play(&demos, 110);
    demos.note_line(&server("cp_b"));
    write_demo(demos.create("cp_b", 10).unwrap(), "cp_b", &[10, 14]);
    play(&demos, 10);
    assert_eq!(demos.at(0, 105), Some(dir.join("live-m-0-100.dem")));
    assert_eq!(downloads(&dir, "m"), std::slice::from_ref(&live), "still there");
    assert!(build(&dir, "m", "0-100").is_ok());
    // The match gone, those with comms stay under another name; a demo nobody talked in does not.
    demos.forget();
    let kept = dir.join("kept-m-0-100.dem");
    assert!(kept.exists() && voice_of(&kept).exists() && !dir.join("live-m-0-100.dem").exists());
    assert_eq!(downloads(&dir, "m"), [live], "the match gone, its comms stay");
    assert!(!dir.join("live-m-1-10.dem").exists(), "the map nobody talked in is gone");
    assert!(build(&dir, "m", "1-10").is_err() && build(&dir, "other", "0-100").is_err() && build(&dir, "m", "../x").is_err());
}

#[test]
fn the_loads_dropped_by_the_recording_lose_their_demos_but_for_comms() {
    let dir = TempDir::new("kept-demos");
    let demos = LiveDemos::new(&dir, "m");
    connect(&demos, "cp_a", 100);
    demos.append_voice(&spoken(103));
    play(&demos, 9000);
    connect(&demos, "cp_b", 10);
    play(&demos, 9000);
    connect(&demos, "cp_c", 10);
    play(&demos, 9000);
    connect(&demos, "cp_c", 9100);
    let relayed = (demos.at(0, 100), demos.at(1, 100));
    assert_eq!(relayed, (Some(dir.join("live-m-0-100.dem")), Some(dir.join("live-m-1-10.dem"))), "the same files at the next load");
    demos.keep_from(2);
    let mut left: Vec<String> = std::fs::read_dir(&*dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    left.sort();
    assert_eq!(left, ["kept-m-0-100.dem", "kept-m-0-100.dem.voice", "live-m-2-10.dem", "live-m-2-9100.dem"]);
    assert_eq!((demos.at(0, 100), demos.at(1, 100)), (None, None));
    assert_eq!(demos.at(2, 9200), Some(dir.join("live-m-2-9100.dem")));
    // A restart finds the kept one as its load's demo again; the recording says it is gone.
    let next = LiveDemos::new(&dir, "m");
    assert_eq!(next.at(0, 100), Some(dir.join("kept-m-0-100.dem")));
    next.keep_from(2);
    assert!(dir.join("kept-m-0-100.dem").exists() && next.at(0, 100).is_none());
}

#[test]
fn kept_demos_go_when_old_or_over_the_budget_the_oldest_first() {
    let dir = TempDir::new("kept-sweep");
    let day = Duration::from_hours(24);
    let now = SystemTime::now();
    for (name, age) in [("kept-a-1.dem", 20), ("kept-b-1.dem", 3), ("kept-c-1.dem", 2), ("kept-d-1.dem", 1)] {
        for path in [dir.join(name), dir.join(format!("{name}.voice"))] {
            std::fs::write(&path, vec![0u8; 1000]).unwrap();
            File::options().write(true).open(&path).unwrap().set_modified(now - day * age).unwrap();
        }
    }
    std::fs::write(dir.join("live-x-1.dem"), b"not ours to sweep").unwrap();
    // Two weeks, and room for two demos with their comms.
    sweep_kept(&dir, day * 14, 4000);
    let mut left: Vec<String> = std::fs::read_dir(&*dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    left.sort();
    assert_eq!(left, ["kept-c-1.dem", "kept-c-1.dem.voice", "kept-d-1.dem", "kept-d-1.dem.voice", "live-x-1.dem"]);
}
