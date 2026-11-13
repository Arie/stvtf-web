//! A live match's loads: its recording's parts and its demos' names by the same numbers.

use super::session::Session;
use super::*;
use crate::matches::hub::publish;
use crate::matches::hub::tests::snapshot;
use crate::matches::recording::Recording;
use crate::testing::{TempDir, server};
use std::time::Instant;
use stvtf::Line;
use stvtf::demo::Files;

/// A live match not watched yet, its demos kept in `store`.
fn live_match(store: &Store, id: &str) -> Arc<Session> {
    let s = Session::new(id.to_string(), SharedHub::default(), "192.0.2.1:27020".parse().unwrap(), "tv".into(), None, false);
    let s = Arc::new(s);
    let _ = s.demos.set(LiveDemos::new(&store.dir, id));
    s
}

/// The match's watcher getting `map`'s first frame at `tick`: its demo made, then the frame out.
fn connect(s: &Session, map: &str, tick: u32) {
    publish(&s.hub, &server(map));
    s.demos.get().unwrap().create(map, tick).unwrap();
    publish(&s.hub, &snapshot(tick, None, Vec::new()));
}

fn until(what: &str, done: impl Fn() -> bool) {
    let t = Instant::now();
    while !done() {
        assert!(t.elapsed() < Duration::from_secs(5), "waited for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_recording_made_after_two_loads_starts_at_part_2() {
    let dir = TempDir::new("loads");
    let store = Store { dir: dir.to_path_buf(), keys: None };
    let s = live_match(&store, &random_id());
    let demos = s.demos.get().unwrap().clone();
    // Its demos were recorded before: the recording was gone at a restart, say.
    for map in ["cp_a", "cp_b"] {
        demos.note_line(&server(map));
        demos.create(map, 50).unwrap();
        demos.note_line(&snapshot(50, None, Vec::new()));
        demos.note_line(&snapshot(9000, None, Vec::new()));
    }
    record(&s, Some(&store), false);
    connect(&s, "cp_c", 10);
    let rec = s.ready().unwrap();
    until("its first snapshot", || rec.latest().first_tick.is_some());
    assert_eq!(rec.parts().iter().map(|p| (p.load, p.map.as_str())).collect::<Vec<_>>(), [(2, "cp_c")]);
    assert_eq!(demos.at(2, 10), Some(dir.join(format!("live-{}-2-10.dem", s.id))));
    s.stop.store(true, Ordering::Relaxed);
}

const T0: u64 = 1_700_000_000_000;

/// Process before: `cp_a` then `cp_b`, its recording in the store, a demo of each.
fn before_a_restart(store: &Store, id: &str) {
    let demos = LiveDemos::new(&store.dir, id);
    let rec = Recording::live_to(store::to_file(store, &live_file(id)));
    let take = |line: &Line, at: u64| {
        demos.note_line(line);
        let text = serde_json::to_string(line).unwrap();
        let mut stamped: stvtf::output::Stamped = serde_json::from_str(&text).unwrap();
        stamped.time = Some(T0 + at);
        rec.push(&stamped, &text).unwrap();
    };
    for (i, map) in ["cp_a", "cp_b"].into_iter().enumerate() {
        let at = i as u64 * 30_000;
        take(&server(map), at);
        demos.create(map, 50).unwrap();
        for s in 0..=20 {
            take(&snapshot(50 + s * 67, None, Vec::new()), at + u64::from(s) * 1000);
        }
    }
    // Closes `cp_b`'s chunk: what the file has.
    take(&snapshot(5000, None, Vec::new()), 75_000);
    assert_eq!(rec.parts().iter().map(|p| p.load).collect::<Vec<_>>(), [0, 1]);
}

#[test]
fn after_a_restart_the_next_load_has_the_next_number_on_both_sides() {
    let dir = TempDir::new("loads");
    let store = Store { dir: dir.to_path_buf(), keys: None };
    let id = random_id();
    before_a_restart(&store, &id);
    let s = live_match(&store, &id);
    record(&s, Some(&store), true);
    let rec = s.ready().unwrap();
    // `cp_b` again, its first tick below the last the recording has (not below its demo's first).
    connect(&s, "cp_b", 1000);
    publish(&s.hub, &snapshot(1100, None, Vec::new()));
    until("cp_b again", || rec.latest().first_tick == Some(1000));
    assert_eq!(rec.parts().iter().map(|p| (p.load, p.map.as_str())).collect::<Vec<_>>(), [(0, "cp_a"), (1, "cp_b"), (2, "cp_b")]);
    assert_eq!(s.demos.get().unwrap().at(2, 1000), Some(dir.join(format!("live-{id}-2-1000.dem"))));
    s.stop.store(true, Ordering::Relaxed);
}

#[test]
fn a_match_given_up_keeps_its_demos_from_the_sweep_till_it_ends() {
    let dir = TempDir::new("loads");
    let mut sessions = Sessions::new(false, 4, 20.0);
    (sessions.store, sessions.demos) = (Some(Store { dir: dir.to_path_buf(), keys: None }), true);
    let m = sessions.open("192.0.2.1:27020 tv", None, None).unwrap();
    m.done.store(true, Ordering::Relaxed);
    assert!(!m.is_live() && sessions.with_demos().contains(&m.id), "rooms may still be on it");
    sessions.reap(Duration::ZERO);
    assert!(sessions.with_demos().is_empty());
    sessions.stop_all();
}
