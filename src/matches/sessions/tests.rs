use super::ids::ID_BYTES;
use super::store::{CURRENT, dem_beside};
use super::*;
use crate::PathBuf;
use crate::testing::{TempDir, script, server};
use crate::web::token;
use std::time::Instant;

#[test]
fn a_fixed_matchs_demo_is_beside_its_recording() {
    assert_eq!(dem_beside(std::path::Path::new("/data/demo.jsonl.gz")), PathBuf::from("/data/demo.dem"));
    assert_eq!(dem_beside(std::path::Path::new("demos/gully.jsonl")), PathBuf::from("demos/gully.dem"));
}

#[test]
fn only_public_addresses_pass() {
    let ok = |s: &str| is_public(s.parse().unwrap());
    assert!(ok("141.94.96.119"));
    assert!(ok("104.128.58.43"));
    for internal in [
        "127.0.0.1",
        "10.1.2.3",
        "172.16.0.1",
        "192.168.1.12",
        "100.64.0.1",
        "100.100.100.100",
        "169.254.1.1",
        "0.0.0.0",
        "255.255.255.255",
        "224.0.0.1",
        "192.0.0.8",
        "198.18.0.1",
        "198.19.255.1",
        "::1",
    ] {
        assert!(!ok(internal), "{internal}");
    }
}

#[test]
fn public_sessions_refuse_internal_servers() {
    let s = Sessions::new(true, 4, 20.0);
    let err = s.open("connect 127.0.0.1:27020; password tv", None, None).err().unwrap().to_string();
    assert!(err.contains("not a public address"), "{err}");
    assert!(s.open("10.0.0.5 tv", None, None).is_err());
}

#[test]
fn a_user_may_watch_any_number_of_matches() {
    let s = Sessions::new(false, 8, 20.0);
    let a = s.open("192.0.2.1:27020 tv", Some("u1"), None).unwrap();
    let again = s.open("192.0.2.1:27020 tv", Some("u2"), None).unwrap();
    assert_eq!(a.id, again.id, "the same match is the same session");
    assert_eq!(again.owner.as_deref(), Some("u1"), "still the one who started it");
    let b = s.open("192.0.2.2:27020 tv", Some("u1"), None).unwrap();
    assert!(s.get(&a.id).is_some() && s.get(&b.id).is_some(), "starting another match stops none");
    s.stop_all();
}

#[test]
fn the_list_says_what_is_watched_and_never_the_password() {
    let s = Sessions::new(false, 8, 20.0);
    s.pin("demo", |_| {});
    let m = s.open("connect 192.0.2.1:27020; password \"hunter2\"", Some("u1"), Some(9)).unwrap();
    let _viewer = Viewer::new(m.clone());
    let list = s.list();
    let listed = list.iter().find(|l| l.id == m.id).unwrap();
    assert_eq!((listed.reservation, listed.owner.as_deref(), listed.viewers, listed.peak), (Some(9), Some("u1"), 1, 1));
    assert_eq!(listed.server, Some("192.0.2.1:27020".parse().unwrap()));
    let demo = list.iter().find(|l| l.id == "demo").unwrap();
    assert!(demo.pinned && demo.server.is_none());
    assert!(!serde_json::to_string(&list).unwrap().contains("hunter2"));
    s.stop_all();
}

#[test]
fn a_start_link_with_a_reservation_links_the_match() {
    let s = Sessions::new(false, 8, 20.0);
    let pasted = s.open("192.0.2.1:27020 tv", None, None).unwrap();
    assert_eq!(pasted.reservation(), None);
    s.open("192.0.2.1:27020 tv", None, Some(7)).unwrap();
    assert_eq!(pasted.reservation(), Some(7), "joining from the reservation links it");
    s.open("192.0.2.1:27020 tv", None, None).unwrap();
    assert_eq!(pasted.reservation(), Some(7), "a pasted link keeps the link");
    assert_eq!(s.open("192.0.2.2:27020 tv", None, Some(8)).unwrap().reservation(), Some(8));
    s.stop_all();
}

#[test]
fn a_demo_plays_once_for_everyone_who_opens_it() {
    // Held before they fetch, the demos never reach demos.tf.
    let fetching = lock(&demostf::HELD);
    let dir = TempDir::new("demo-sessions");
    let mut s = Sessions::new(false, 3, 20.0);
    s.store = Some(Store { dir: dir.to_path_buf(), keys: None });
    let a = s.open_demo_as(None, 1, Some("u1")).unwrap();
    assert_eq!(s.open_demo_as(None, 1, Some("u2")).unwrap().id, a.id, "joined where it is");
    let server = s.open("192.0.2.1:27020 tv", None, None).unwrap();
    assert_ne!(server.id, a.id);
    s.open_demo_as(None, 2, Some("u3")).unwrap();
    let full = s.open_demo_as(None, 3, Some("u3")).err().unwrap().to_string();
    assert!(full.contains("already watching 3"), "demos count towards the limit: {full}");
    s.persist();
    assert!(!dir.join(&a.id).exists(), "the next process could not pick a demo up where it is");
    for m in s.lock().values() {
        m.stop.store(true, Ordering::Relaxed);
    }
    drop(fetching);
    s.stop_all();
}

#[test]
fn a_session_knows_whether_it_is_watched() {
    let s = Sessions::new(false, 1, 20.0);
    s.pin("demo", |_| {});
    let demo = s.get("demo").unwrap();
    assert!(!demo.watched());
    let viewer = Viewer::new(demo.clone());
    assert!(demo.watched());
    drop(viewer);
    assert!(!demo.watched());
}

#[test]
fn a_pinned_demo_can_be_recorded() {
    let dir = TempDir::new("recorded");
    let path = dir.join("demo.jsonl");
    std::fs::write(&path, "{\"type\":\"stalemate\",\"tick\":5}\n{\"type\":\"stalemate\",\"tick\":55}\n").unwrap();
    let s = Sessions::new(false, 1, 20.0);
    s.pin("demo", move |s| s.set_file(path));
    let demo = s.get("demo").unwrap();
    while !demo.recordable() {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(demo.recording().unwrap().length(), 750);
    assert!(Arc::ptr_eq(&demo.recording().unwrap(), &demo.recording().unwrap()), "read once");
}

#[test]
fn a_demo_s_recording_is_prepared_in_the_background() {
    let dir = TempDir::new("prepared");
    let path = dir.join("demo.jsonl");
    std::fs::write(&path, "{\"type\":\"stalemate\",\"tick\":5}\n{\"type\":\"stalemate\",\"tick\":55}\n").unwrap();
    let s = Sessions::new(false, 1, 20.0);
    s.pin("demo", move |s| s.set_file(path));
    let demo = s.get("demo").unwrap();
    while !demo.recordable() {
        std::thread::sleep(Duration::from_millis(1));
    }
    demo.prepare();
    let started = Instant::now();
    while demo.recording.get().is_none() {
        assert!(started.elapsed() < Duration::from_secs(5), "prepared");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(Arc::ptr_eq(&demo.recording().unwrap(), demo.recording.get().unwrap()), "the room plays what was prepared");
}

#[test]
fn a_live_match_is_recorded_from_the_start() {
    let s = Sessions::new(false, 1, 20.0);
    let m = s.open("192.0.2.1:27020 tv", None, None).unwrap();
    assert!(m.recordable());
    assert!(m.recording().unwrap().is_live());
    s.stop_all();
}

#[test]
fn a_pinned_session_stays_and_does_not_count() {
    let s = Sessions::new(false, 1, 20.0);
    s.pin("demo", |_| {});
    s.reap(Duration::ZERO);
    assert!(s.get("demo").is_some());
    let m = s.open("192.0.2.1:27020 tv", None, None).unwrap();
    assert!(s.get(&m.id).is_some(), "the demo is outside the match limit");
    s.stop_all();
}

#[test]
fn matches_survive_into_the_next_process() {
    let dir = TempDir::new("sessions");
    let keys = Arc::new(token::Keys::from_hex(&"42".repeat(32)).unwrap());
    let store = || Some(Store { dir: dir.to_path_buf(), keys: Some(keys.clone()) });
    let mut before = Sessions::new(false, 4, 20.0);
    before.store = store();
    let m = before.open("connect 192.0.2.1:27020; password \"hunter2\"", Some("u1"), Some(42)).unwrap();
    {
        let mut h = m.hub.lock().unwrap();
        h.history.push_back(r#"{"type":"kill","history":true}"#.into());
        h.changes += 1;
    }
    before.persist();
    let file = std::fs::read(dir.join(&m.id)).unwrap();
    assert!(!String::from_utf8_lossy(&file).contains("hunter2"), "saved encrypted");
    before.stop_all();

    let mut after = Sessions::new(false, 4, 20.0);
    after.store = store();
    assert_eq!(after.restore(), 1);
    let again = after.get(&m.id).expect("same link");
    assert_eq!((again.server, again.password.as_str(), again.owner.as_deref()), (m.server, "hunter2", Some("u1")));
    assert_eq!(again.hub.lock().unwrap().history.len(), 1, "feed so far");
    assert_eq!(again.reservation(), Some(42));
    after.reap(Duration::ZERO);
    assert!(!dir.join(&m.id).exists(), "a dropped match is forgotten");
    after.stop_all();
}

#[test]
fn only_changed_matches_are_saved_again() {
    let dir = TempDir::new("sessions");
    let mut s = Sessions::new(false, 4, 20.0);
    s.store = Some(Store { dir: dir.to_path_buf(), keys: None });
    let m = s.open("192.0.2.1:27020 tv", None, None).unwrap();
    std::fs::remove_file(dir.join(&m.id)).unwrap();
    s.persist();
    assert!(!dir.join(&m.id).exists(), "unchanged since it was saved");
    m.hub.lock().unwrap().changes += 1;
    s.persist();
    assert!(dir.join(&m.id).exists());
    s.stop_all();
}

#[test]
fn an_ended_match_is_not_saved_again() {
    let dir = TempDir::new("sessions");
    let mut s = Sessions::new(false, 4, 20.0);
    s.store = Some(Store { dir: dir.to_path_buf(), keys: None });
    let a = s.open("192.0.2.1:27020 tv", Some("u1"), None).unwrap();
    assert!(dir.join(&a.id).exists());
    // persist() took `a` from the list just before it was reaped.
    s.reap(Duration::ZERO);
    assert!(!dir.join(&a.id).exists(), "the ended match is forgotten");
    s.store.as_ref().unwrap().save(&a).unwrap();
    assert!(!dir.join(&a.id).exists(), "and stays forgotten");
    s.stop_all();
}

#[test]
fn a_match_saved_by_the_running_release_is_picked_up() {
    let dir = TempDir::new("sessions");
    let (id, broken) = (random_id(), random_id());
    let saved = r#"{"server":"192.0.2.1:27020","password":"tv","owner":null,"map":"cp_badlands","history":["{\"type\":\"kill\"}"]}"#;
    std::fs::write(dir.join(&id), saved).unwrap();
    std::fs::write(dir.join(&broken), r#"{"server":"192.0.2.1:27020"}"#).unwrap();
    let mut s = Sessions::new(false, 4, 20.0);
    s.store = Some(Store { dir: dir.to_path_buf(), keys: None });
    assert_eq!(s.restore(), 1);
    let m = s.get(&id).expect("same link");
    assert_eq!((m.password.as_str(), m.owner.as_deref()), ("tv", None));
    let hub = m.hub.lock().unwrap();
    assert_eq!(hub.map.as_deref(), Some("cp_badlands"));
    assert_eq!(hub.history.iter().map(ToString::to_string).collect::<Vec<_>>(), [r#"{"type":"kill"}"#]);
    drop(hub);
    assert!(!dir.join(&broken).exists(), "an unreadable one is dropped");
    s.stop_all();
}

/// A process starting on `dir`, as a deploy's new release does, and how many matches it took over.
fn start_on(dir: &TempDir) -> (Sessions, usize) {
    let mut s = Sessions::new(false, 4, 20.0);
    s.store = Some(Store { dir: dir.to_path_buf(), keys: None });
    let restored = s.restore();
    (s, restored)
}

#[test]
fn a_match_started_by_the_old_release_is_picked_up_when_asked_for() {
    let dir = TempDir::new("sessions");
    let ((old, _), (new, _)) = (start_on(&dir), start_on(&dir));
    // Started after the new release read the folder, before the proxy switched to it.
    let m = old.open("192.0.2.1:27020 tv", Some("u1"), None).unwrap();
    let again = new.get(&m.id).expect("the page reconnecting to the new release finds it");
    assert_eq!((again.server, again.owner.as_deref()), (m.server, Some("u1")));
    assert!(new.get(&random_id()).is_none(), "a match nobody saved is still unknown");
    assert!(new.get("../current").is_none(), "only match ids are looked up");
    old.stop_all();
    new.stop_all();
}

#[test]
fn a_release_taken_over_leaves_the_saved_matches_alone() {
    let dir = TempDir::new("sessions");
    let (old, _) = start_on(&dir);
    let m = old.open("192.0.2.1:27020 tv", Some("u1"), None).unwrap();
    let (new, restored) = start_on(&dir);
    assert_eq!(restored, 1);
    // Nobody watches it once the proxy switched: the new release ends it.
    new.reap(Duration::ZERO);
    assert!(!dir.join(&m.id).exists());
    // The old release still has it, with lines it got since its last save, and saves on SIGTERM.
    m.hub.lock().unwrap().changes += 1;
    old.persist();
    assert!(!dir.join(&m.id).exists(), "not brought back for the next deploy");
    old.stop_all();
    new.stop_all();
}

#[test]
fn ids_are_long_and_differ() {
    let (a, b) = (random_id(), random_id());
    assert_eq!(a.len(), 32);
    assert_ne!(a, b);
}

#[test]
fn a_match_whose_watcher_gave_up_is_started_again() {
    let s = Sessions::new(false, 8, 20.0);
    script("192.0.2.200:27020", vec![server("cp_process_f12")], Some("wrong password"));
    let first = s.open("192.0.2.200:27020 tv", None, None).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !first.done.load(Ordering::Relaxed) {
        assert!(Instant::now() < deadline, "the watcher should have given up");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(s.on_map("cp_process_f12"), "what it saw before giving up is kept");
    let second = s.open("192.0.2.200:27020 tv", None, None).unwrap();
    assert_ne!(first.id, second.id);
    s.stop_all();
}

#[test]
fn a_match_is_idle_only_once_its_last_viewer_left() {
    let s = Sessions::new(false, 8, 20.0);
    let m = s.open("192.0.2.50:27020 tv", None, None).unwrap();
    let (a, b) = (Viewer::new(m.clone()), Viewer::new(m.clone()));
    s.reap(Duration::ZERO);
    assert!(s.get(&m.id).is_some(), "two are watching");
    drop(a);
    s.reap(Duration::ZERO);
    assert!(s.get(&m.id).is_some(), "one is still watching");
    drop(b);
    s.reap(Duration::from_hours(1));
    assert!(s.get(&m.id).is_some(), "it lingers");
    s.reap(Duration::ZERO);
    assert!(s.get(&m.id).is_none());
}

#[test]
fn a_match_in_play_stays_watched_with_nobody_watching() {
    use crate::matches::hub::tests::{player, snapshot};
    use stvtf::output::Team;
    let s = Sessions::new(false, 8, 20.0);
    let m = s.open("192.0.2.51:27020 tv", None, None).unwrap();
    drop(Viewer::new(m.clone()));
    let players = (1..=4).map(|i| player(i, "p", if i % 2 == 0 { Team::Red } else { Team::Blue })).collect();
    crate::matches::hub::publish(&m.hub, &snapshot(100, None, players));
    s.reap(Duration::ZERO);
    assert!(s.get(&m.id).is_some(), "players on it: it stays");
    // Everyone gone to spectate, or left: it ends as any unwatched match.
    crate::matches::hub::publish(&m.hub, &snapshot(200, None, vec![player(1, "p", Team::Spectator)]));
    s.reap(Duration::ZERO);
    assert!(s.get(&m.id).is_none());
}

#[test]
fn one_person_s_fifth_match_ends_their_longest_unwatched() {
    let s = Sessions::new(false, 20, 20.0);
    let open = |n: u8, by| s.open_as(Some(by), &format!("192.0.2.{n}:27020 tv"), None, None);
    let theirs: Vec<_> = (70..74).map(|n| open(n, "steam:1").unwrap()).collect();
    // All four just started: none goes.
    assert!(open(74, "steam:1").err().unwrap().to_string().contains("close one first"));
    assert!(open(74, "ip:192.0.2.9").is_ok(), "someone else");
    assert!(s.open("192.0.2.75:27020 tv", None, None).is_ok(), "not counted");
    // One of theirs someone else is on still counts, but stays.
    assert!(s.open_as(Some("steam:2"), "192.0.2.70:27020 tv", None, None).is_ok(), "joining starts nothing");
    let watching = Viewer::new(theirs[0].clone());
    for m in &theirs {
        let mut idle = lock(&m.idle_since);
        *idle = idle.map(|_| Instant::now().checked_sub(MAKES_ROOM).unwrap());
    }
    *lock(&theirs[2].idle_since) = Some(Instant::now().checked_sub(MAKES_ROOM * 2).unwrap());
    let fifth = open(76, "steam:1").unwrap();
    assert!(s.get(&theirs[2].id).is_none(), "the longest unwatched went");
    assert!(s.get(&theirs[0].id).is_some() && s.get(&fifth.id).is_some());
    drop(watching);
    s.stop_all();
}

#[test]
fn a_full_server_makes_room_by_the_longest_unwatched_match() {
    let s = Sessions::new(false, 1, 20.0);
    let a = s.open("192.0.2.52:27020 tv", None, None).unwrap();
    // Just started, its page not in yet: it keeps its place.
    assert!(s.open("192.0.2.53:27020 tv", None, None).is_err());
    *lock(&a.idle_since) = Instant::now().checked_sub(MAKES_ROOM);
    let b = s.open("192.0.2.53:27020 tv", None, None).unwrap();
    assert!(s.get(&a.id).is_none() && s.get(&b.id).is_some());
    s.stop_all();
}

#[test]
fn only_match_ids_are_looked_up_in_the_store() {
    let root = TempDir::new("storeids");
    let dir = root.join("store");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join(CURRENT), "mark").unwrap();
    std::fs::write(root.join("outside"), "x").unwrap();
    let mut s = Sessions::new(false, 8, 20.0);
    s.store = Some(Store { dir: dir.clone(), keys: None });
    for name in [CURRENT, "../outside"] {
        assert!(s.get(name).is_none(), "{name}");
    }
    // Anything picked up and found unreadable would have been deleted.
    assert_eq!(std::fs::read(dir.join(CURRENT)).unwrap(), b"mark");
    assert!(root.join("outside").exists());
    let unreadable = "0".repeat(2 * ID_BYTES);
    std::fs::write(dir.join(&unreadable), "not a match").unwrap();
    assert!(s.get(&unreadable).is_none());
    assert!(!dir.join(&unreadable).exists(), "a saved match that cannot be read is dropped");
}

#[test]
fn a_match_id_is_exactly_as_long_as_they_are_made_and_hex() {
    assert!(is_id(&random_id()));
    assert_eq!(id_demo(&demo_id(1_531_482)), Some(1_531_482));
    assert_eq!(id_demo("d"), None);
    for bad in ["", "abc", &"a".repeat(31), &"a".repeat(33), &"g".repeat(32), &format!("../{}", "a".repeat(29))] {
        assert!(!is_id(bad), "{bad}");
    }
}

#[test]
fn documentation_and_broadcast_addresses_are_not_public() {
    for ip in ["192.0.2.1", "198.51.100.1", "203.0.113.1", "255.255.255.255"] {
        assert!(!is_public(ip.parse().unwrap()), "{ip}");
    }
}

#[test]
fn stopping_all_waits_for_every_watcher_to_finish() {
    let s = Sessions::new(false, 8, 20.0);
    let (a, b) = (s.open("192.0.2.60:27020 tv", None, None).unwrap(), s.open("192.0.2.61:27020 tv", None, None).unwrap());
    s.stop_all();
    assert!(a.done.load(Ordering::Relaxed) && b.done.load(Ordering::Relaxed));
    assert!(s.get(&a.id).is_none() && s.get(&b.id).is_none());
}
