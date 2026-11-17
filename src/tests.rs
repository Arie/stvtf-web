use std::net::IpAddr;
use std::time::Duration;

use super::*;
use crate::app::Matches;
use crate::cli::Options;
use crate::matches::hub::{SharedHub, publish};
use crate::start::{demo_files, many};
use crate::testing::{TempDir, app, server};
use crate::web::MAX_STREAMS_PER_CLIENT;

#[test]
fn a_demo_folder_gives_one_match_per_recording() {
    let dir = TempDir::new("demos");
    for name in ["pass_time.jsonl.gz", "rd.jsonl", "notes.txt", "bad name.jsonl", ".jsonl", "mvm-tank.jsonl"] {
        std::fs::write(dir.join(name), "").unwrap();
    }
    let found: Vec<(String, String)> =
        demo_files(&dir).unwrap().into_iter().map(|(id, path)| (id, path.file_name().unwrap().to_string_lossy().into_owned())).collect();
    let expected = [("mvm-tank", "mvm-tank.jsonl"), ("pass_time", "pass_time.jsonl.gz"), ("rd", "rd.jsonl")];
    assert_eq!(found, expected.map(|(a, b)| (a.to_string(), b.to_string())));
    assert!(demo_files(&dir.join("missing")).is_err(), "a missing folder is an error, not no demos");
}

#[test]
fn a_server_knows_which_maps_are_watched() {
    let hub = SharedHub::default();
    let app = app(Matches::One(hub.clone()));
    assert!(!app.on_map("cp_process_f12"));
    publish(&hub, &server("cp_process_f12"));
    assert!(app.on_map("cp_process_f12") && !app.on_map("cp_badlands"));
}

#[test]
fn event_streams_are_capped_per_client() {
    let app = app(Matches::One(SharedHub::default()));
    let (a, b): (IpAddr, IpAddr) = ("203.0.113.1".parse().unwrap(), "203.0.113.2".parse().unwrap());
    let open: Vec<_> = (0..MAX_STREAMS_PER_CLIENT).map(|_| app.stream_slot(a).unwrap()).collect();
    assert!(app.stream_slot(a).is_none());
    assert!(app.stream_slot(b).is_some(), "other clients still get in");
    drop(open);
    assert!(app.stream_slot(a).is_some());
    assert!(app.streams.lock().unwrap().is_empty(), "closed streams are forgotten");
}

#[test]
fn a_server_of_many_matches_starts_with_its_demos_and_the_saved_matches() {
    let (demos, store) = (TempDir::new("manydemos"), TempDir::new("manystore"));
    std::fs::write(demos.join("rd.jsonl"), "").unwrap();
    std::fs::write(demos.join("notes.txt"), "").unwrap();
    let demo = demos.join("rd.jsonl");
    let args = ["--demos", demos.to_str().unwrap(), "--demo", demo.to_str().unwrap(), "--sessions", store.to_str().unwrap()];
    let options = Options::parse_from(args.map(String::from)).unwrap();

    let before = many(&options, None).unwrap();
    testing::script("192.0.2.77:27020", vec![server("cp_restored")], None);
    let running = before.open("192.0.2.77:27020 tv", None, None).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !before.on_map("cp_restored") {
        assert!(std::time::Instant::now() < deadline, "the watcher should have said its map");
        std::thread::sleep(Duration::from_millis(1));
    }
    before.persist();
    before.stop_all();

    let after = many(&options, None).unwrap();
    // Before anyone asks for it by id, which would pick it up anyway.
    assert!(after.on_map("cp_restored"), "picked up");
    assert!(after.get(&running.id).is_some());
    assert!(after.get("rd").is_some() && after.get("demo").is_some(), "the demos are there");
    assert!(after.get("notes").is_none());
    after.stop_all();
}

#[test]
fn a_missing_demo_folder_stops_the_server_from_starting() {
    let dir = TempDir::new("nodemos");
    let missing = dir.join("nothing");
    let options = Options::parse_from(["--demos".to_string(), missing.to_str().unwrap().to_string()]).unwrap();
    assert!(many(&options, None).is_err());
}
