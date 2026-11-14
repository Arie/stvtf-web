//! A live match's recording from one process to the next: its file has one writer, and the next
//! goes on at a tick.

use super::record::{GIVE_BY, GIVE_WAIT, given_file, take_file};
use super::tests::start_on;
use super::*;
use crate::matches::hub::tests::snapshot;
use crate::matches::recording::{Recording, Save};
use crate::testing::{TempDir, script, server};
use std::time::Instant;
use stvtf::Line;
use stvtf::output::Status;

/// A stream of `map`: its server line, and a snapshot every 100 ticks of `ticks`.
fn stream(map: &str, ticks: std::ops::RangeInclusive<u32>) -> Vec<Line> {
    [server(map)].into_iter().chain(ticks.step_by(100).map(|tick| snapshot(tick, None, Vec::new()))).collect()
}

/// The ticks of `r`'s snapshots, in the order it plays them.
fn ticks_of(r: &Recording) -> Vec<u32> {
    let lines = (0..r.chunks()).flat_map(|i| r.chunk(i).unwrap_or_default());
    lines.filter_map(|(_, line)| matches!(line, Line::Snapshot(_)).then(|| line.tick()).flatten()).collect()
}

fn every_100(ticks: std::ops::RangeInclusive<u32>) -> Vec<u32> {
    ticks.step_by(100).collect()
}

fn until(what: &str, done: impl Fn() -> bool) {
    let t = Instant::now();
    while !done() {
        assert!(t.elapsed() < Duration::from_secs(5), "waited for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// What a process starting now reads from match `id`'s file in `dir`.
fn in_file(dir: &TempDir, id: &str) -> Recording {
    let file = std::fs::read(dir.join(live_file(id))).unwrap();
    Recording::resume(file.as_slice(), Box::new(|_: &Save| {})).unwrap()
}

#[test]
fn a_recording_goes_to_the_next_release_at_a_tick_with_every_line_once() {
    let dir = TempDir::new("handover");
    let target = "192.0.2.41:27020";
    script(target, stream("cp_a", 1000..=1700), None);
    let (old, _) = start_on(&dir);
    let m = old.open(&format!("{target} tv"), None, None).unwrap();
    let before = m.ready().unwrap();
    until("the old release's lines", || ticks_of(&before).last() == Some(&1700));
    // The new release's stream begins while the old one's goes on, and goes on after it.
    script(target, stream("cp_a", 1500..=2000), None);
    let (new, _) = start_on(&dir);
    let rec = new.get(&m.id).unwrap().ready().unwrap();
    assert!(before.is_written() && !rec.is_written(), "the old release writes the file till it is asked for");
    until("the file to be the new release's", || rec.is_written());
    assert!(!before.is_written(), "one writer");
    until("its own lines", || ticks_of(&rec).last() == Some(&2000));
    assert_eq!(ticks_of(&rec), every_100(1000..=2000), "none missing, none twice");
    assert_eq!(rec.parts().len(), 1);
    let servers =
        |r: &Recording| (0..r.chunks()).flat_map(|i| r.chunk(i).unwrap_or_default()).filter(|(_, l)| matches!(l, Line::Server(_))).count();
    assert_eq!(servers(&rec), 1, "the next release's server line is not the match's");
    old.stop_all();
    new.stop_all();
    let read = in_file(&dir, &m.id);
    assert_eq!((ticks_of(&read), read.parts().len()), (every_100(1000..=2000), 1), "and so the file");
}

#[test]
fn a_release_that_stops_closes_its_recordings_into_their_files() {
    let dir = TempDir::new("handover");
    let target = "192.0.2.42:27020";
    script(target, stream("cp_a", 1000..=1700), None);
    let (old, _) = start_on(&dir);
    let m = old.open(&format!("{target} tv"), None, None).unwrap();
    until("its lines", || ticks_of(&m.ready().unwrap()).last() == Some(&1700));
    old.stop_all();
    assert_eq!(ticks_of(&in_file(&dir, &m.id)), every_100(1000..=1700), "the chunk that was open too");
    assert!(dir.join(given_file(&m.id)).exists());
    // The next process has it all from the start, and the file with no stream of its own yet.
    let (new, _) = start_on(&dir);
    let rec = new.get(&m.id).unwrap().ready().unwrap();
    assert_eq!(ticks_of(&rec), every_100(1000..=1700));
    until("the file to be the new process's", || rec.is_written());
    assert!(!dir.join(given_file(&m.id)).exists());
    new.reap(Duration::ZERO);
    assert!(!dir.join(take_file(&m.id)).exists(), "a match that ended leaves nothing");
    new.stop_all();
}

/// A match on `target` saved in `dir` with a recording of `cp_a`, ticks 1000 to 3000 in its
/// file, by a process that is gone; its id.
fn left_by_a_crash(dir: &TempDir, target: &str) -> String {
    let id = random_id();
    let store = Store { dir: dir.to_path_buf(), keys: None };
    let saved = format!(r#"{{"server":"{target}","password":"tv","owner":null,"map":"cp_a","history":[]}}"#);
    std::fs::write(dir.join(&id), saved).unwrap();
    let rec = Recording::live_to(store::to_file(&store, &live_file(&id)));
    let lines = stream("cp_a", 1000..=3100);
    for (i, line) in lines.iter().enumerate() {
        crate::matches::recording::tests::push_at(&rec, line, 1_700_000_000_000 + i as u64 * 1500);
    }
    id
}

#[test]
fn a_release_gone_without_a_word_leaves_the_next_its_file_as_it_is() {
    let dir = TempDir::new("handover");
    let target = "192.0.2.43:27020";
    let id = left_by_a_crash(&dir, target);
    let had = ticks_of(&in_file(&dir, &id));
    assert!(had.len() > 10 && had.len() < 22, "its open chunk is lost: {had:?}");
    script(target, stream("cp_a", 3500..=3800), None);
    let (new, _) = start_on(&dir);
    let rec = new.get(&id).unwrap().ready().unwrap();
    assert_eq!(ticks_of(&rec), had, "what the file has, at once");
    let asked = Instant::now();
    until("the file to be the new process's", || rec.is_written());
    assert!(asked.elapsed() >= GIVE_WAIT / 2, "it waited for the old one to give it");
    until("its own lines", || ticks_of(&rec).last() == Some(&3800));
    assert_eq!(ticks_of(&rec), [had, every_100(3500..=3800)].concat(), "a hole, and nothing twice");
    assert_eq!(rec.parts().len(), 1);
    new.stop_all();
}

#[test]
fn a_map_loaded_again_meanwhile_is_a_new_load_with_all_its_lines() {
    let dir = TempDir::new("handover");
    let target = "192.0.2.44:27020";
    let id = left_by_a_crash(&dir, target);
    let had = ticks_of(&in_file(&dir, &id));
    // Its ticks started over: lower than the file's last, and no stretch of it.
    script(target, stream("cp_a", 100..=400), None);
    let (new, _) = start_on(&dir);
    let rec = new.get(&id).unwrap().ready().unwrap();
    until("its own lines", || ticks_of(&rec).last() == Some(&400));
    assert_eq!(ticks_of(&rec), [had, every_100(100..=400)].concat());
    assert_eq!(rec.parts().iter().map(|p| p.load).collect::<Vec<_>>(), [0, 1]);
    new.stop_all();
}

#[test]
fn a_release_writes_its_recording_again_when_the_one_that_took_it_stops() {
    let dir = TempDir::new("handover");
    let target = "192.0.2.45:27020";
    script(target, stream("cp_a", 1000..=1700), None);
    let (old, _) = start_on(&dir);
    let m = old.open(&format!("{target} tv"), None, None).unwrap();
    let before = m.ready().unwrap();
    until("the old release's lines", || ticks_of(&before).last() == Some(&1700));
    script(target, stream("cp_a", 1500..=2000), None);
    let (new, _) = start_on(&dir);
    let rec = new.get(&m.id).unwrap().ready().unwrap();
    until("the file to be the new release's", || rec.is_written());
    // The old release's stream went on too; the new one is taken down (a deploy that failed).
    for tick in every_100(1800..=2200) {
        crate::matches::hub::publish(&m.hub, &snapshot(tick, None, Vec::new()));
    }
    new.stop_all();
    until("the file to be the old release's again", || before.is_written());
    until("its lines", || ticks_of(&before).last() == Some(&2200));
    old.stop_all();
    assert_eq!(ticks_of(&in_file(&dir, &m.id)), every_100(1000..=2200), "all it has, once");
}

#[test]
fn a_release_writes_its_recording_again_when_the_one_that_took_it_is_gone_without_a_word() {
    let dir = TempDir::new("handover");
    let target = "192.0.2.46:27020";
    script(target, stream("cp_a", 1000..=1700), None);
    let (old, _) = start_on(&dir);
    let m = old.open(&format!("{target} tv"), None, None).unwrap();
    let before = m.ready().unwrap();
    until("the old release's lines", || ticks_of(&before).last() == Some(&1700));
    script(target, stream("cp_a", 1500..=2000), None);
    let (new, _) = start_on(&dir);
    let rec = new.get(&m.id).unwrap().ready().unwrap();
    until("the file to be the new release's", || rec.is_written());
    std::thread::sleep(GIVE_WAIT / 4);
    assert!(!before.is_written(), "while the new one says it writes it");
    // The new release crashes: its recorders stop, and nothing is said.
    for s in new.lock().values() {
        s.stop.store(true, Ordering::Relaxed);
    }
    goes_on(&m.hub, 1800..=2200);
    until("the file to be the old release's again", || before.is_written());
    until("its lines", || ticks_of(&before).last() == Some(&2200));
    old.stop_all();
    assert_eq!(ticks_of(&in_file(&dir, &m.id)), every_100(1000..=2200), "all it has, once");
}

/// Snapshots at `ticks`, every 100, as `hub`'s stream goes on.
fn goes_on(hub: &SharedHub, ticks: std::ops::RangeInclusive<u32>) {
    for tick in every_100(ticks) {
        crate::matches::hub::publish(hub, &snapshot(tick, None, Vec::new()));
    }
}

/// A match on `target` an old release records up to tick 1700, and a new release whose stream
/// begins with a frame at `first`: a real `SourceTV` sends one who just connected a frame some
/// 11 s ahead of its stream, then nothing until the stream is there.
fn asked_for_at(dir: &TempDir, target: &str, first: u32) -> (Sessions, Arc<Session>, Sessions, Arc<Session>) {
    script(target, stream("cp_a", 1000..=1700), None);
    let (old, _) = start_on(dir);
    let m = old.open(&format!("{target} tv"), None, None).unwrap();
    until("the old release's lines", || ticks_of(&m.ready().unwrap()).last() == Some(&1700));
    script(target, stream("cp_a", first..=first), None);
    let (new, _) = start_on(dir);
    let n = new.get(&m.id).unwrap();
    (old, m, new, n)
}

#[test]
fn a_recording_asked_for_ahead_of_its_stream_is_written_on_to_the_tick_asked_at() {
    let dir = TempDir::new("handover");
    let (old, m, new, n) = asked_for_at(&dir, "192.0.2.47:27020", 2500);
    let (before, rec) = (m.ready().unwrap(), n.ready().unwrap());
    std::thread::sleep(GIVE_WAIT * 2);
    assert!(before.is_written() && !rec.is_written(), "the old release writes on: its stream is not there yet");
    assert!(rec.is_catching_up());
    goes_on(&m.hub, 1800..=2500);
    until("the file to be the new release's", || rec.is_written());
    assert!(!before.is_written(), "one writer");
    goes_on(&n.hub, 2600..=2800);
    until("its own lines", || ticks_of(&rec).last() == Some(&2800));
    assert_eq!(ticks_of(&rec), every_100(1000..=2800), "no hole, none twice");
    old.stop_all();
    new.stop_all();
    assert_eq!(ticks_of(&in_file(&dir, &m.id)), every_100(1000..=2800), "and so the file");
}

#[test]
fn a_recording_whose_stream_does_not_get_to_the_tick_asked_at_is_given_as_it_is() {
    let dir = TempDir::new("handover");
    let (old, m, new, n) = asked_for_at(&dir, "192.0.2.48:27020", 9000);
    let (before, rec) = (m.ready().unwrap(), n.ready().unwrap());
    let asked = Instant::now();
    goes_on(&m.hub, 1800..=2000);
    until("the file to be the new release's", || rec.is_written());
    assert!(asked.elapsed() >= GIVE_BY / 2 && !before.is_written(), "written on for a while, not for ever");
    until("its own line", || ticks_of(&rec).last() == Some(&9000));
    assert_eq!(ticks_of(&rec), [every_100(1000..=2000), vec![9000]].concat(), "all the old one had, a hole, none twice");
    old.stop_all();
    new.stop_all();
}

#[test]
fn a_recording_asked_for_at_no_tick_is_given_at_once() {
    let dir = TempDir::new("handover");
    let target = "192.0.2.49:27020";
    script(target, stream("cp_a", 1000..=1700), None);
    let (old, _) = start_on(&dir);
    let m = old.open(&format!("{target} tv"), None, None).unwrap();
    let before = m.ready().unwrap();
    until("the old release's lines", || ticks_of(&before).last() == Some(&1700));
    // As a version before asked: its mark, and no more.
    std::fs::write(dir.join(take_file(&m.id)), random_id()).unwrap();
    until("the file given", || dir.join(given_file(&m.id)).exists());
    assert!(!before.is_written());
    assert_eq!(ticks_of(&in_file(&dir, &m.id)), every_100(1000..=1700));
    old.stop_all();
}

#[test]
fn a_recording_picked_up_goes_on_with_its_file_while_it_waits_for_it() {
    let dir = TempDir::new("handover");
    let (old, m, new, n) = asked_for_at(&dir, "192.0.2.50:27020", 2500);
    let rec = n.ready().unwrap();
    for upto in [2000, 2300] {
        goes_on(&m.hub, upto - 200..=upto);
        until("what the old release wrote meanwhile", || ticks_of(&rec).last() == Some(&upto));
        assert!(rec.is_catching_up() && !rec.is_written(), "the file is the old release's still");
    }
    goes_on(&m.hub, 2400..=2500);
    until("the file to be the new release's", || rec.is_written());
    assert!(!rec.is_catching_up());
    assert_eq!(ticks_of(&rec), every_100(1000..=2500), "none missing, none twice");
    old.stop_all();
    new.stop_all();
}

#[test]
fn a_release_told_to_stop_records_on_until_the_recording_asked_for_is_at_the_tick_asked_at() {
    let dir = TempDir::new("handover");
    let (old, m, new, n) = asked_for_at(&dir, "192.0.2.51:27020", 2500);
    let (before, rec) = (m.ready().unwrap(), n.ready().unwrap());
    until("the old release to hear", || m.giving.load(Ordering::Relaxed));
    std::thread::scope(|scope| {
        let stopping = scope.spawn(|| old.stop_all());
        std::thread::sleep(STOP_WAIT / 4);
        assert!(!stopping.is_finished() && before.is_written(), "its match is not cut short");
        goes_on(&m.hub, 1800..=2500);
        stopping.join().unwrap();
    });
    until("the file to be the new release's", || rec.is_written());
    assert_eq!(ticks_of(&rec), every_100(1000..=2500), "no hole");
    new.stop_all();
}

#[test]
fn a_release_told_to_stop_does_not_wait_long_for_a_tick_asked_at() {
    let dir = TempDir::new("handover");
    let (old, m, new, _) = asked_for_at(&dir, "192.0.2.52:27020", 9000);
    until("the old release to hear", || m.giving.load(Ordering::Relaxed));
    let told = Instant::now();
    old.stop_all();
    assert!(told.elapsed() >= STOP_WAIT / 2 && told.elapsed() < GIVE_BY, "{:?}", told.elapsed());
    assert_eq!(ticks_of(&in_file(&dir, &m.id)), every_100(1000..=1700), "given as it is");
    new.stop_all();
}

/// `stream` as one who connects gets it: after the statuses of its signon.
fn connecting(stream: Vec<Line>) -> Vec<Line> {
    let connecting = Status::Connecting { server: "192.0.2.1:27020".into(), build: "1".into() };
    let signon = [connecting, Status::New, Status::Prespawn, Status::Spawn, Status::Full { map: None }];
    signon.into_iter().map(Line::Status).chain(stream).collect()
}

/// How many of `r`'s lines are statuses.
fn statuses_in(r: &Recording) -> usize {
    (0..r.chunks()).flat_map(|i| r.chunk(i).unwrap_or_default()).filter(|(_, line)| matches!(line, Line::Status(_))).count()
}

#[test]
fn a_recording_taken_has_none_of_the_next_release_s_signon() {
    let dir = TempDir::new("handover");
    // Its stream goes on from the old release's.
    let target = "192.0.2.53:27020";
    script(target, stream("cp_a", 1000..=1700), None);
    let (old, _) = start_on(&dir);
    let m = old.open(&format!("{target} tv"), None, None).unwrap();
    until("the old release's lines", || ticks_of(&m.ready().unwrap()).last() == Some(&1700));
    script(target, connecting(stream("cp_a", 1500..=2000)), None);
    let (new, _) = start_on(&dir);
    let rec = new.get(&m.id).unwrap().ready().unwrap();
    until("its own lines", || rec.is_written() && ticks_of(&rec).last() == Some(&2000));
    assert_eq!(statuses_in(&rec), 0);
    old.stop_all();
    new.stop_all();
    // The old release is gone, and left a hole.
    let target = "192.0.2.54:27020";
    let id = left_by_a_crash(&dir, target);
    script(target, connecting(stream("cp_a", 3500..=3800)), None);
    let (new, _) = start_on(&dir);
    let rec = new.get(&id).unwrap().ready().unwrap();
    until("its own lines", || rec.is_written() && ticks_of(&rec).last() == Some(&3800));
    assert_eq!(statuses_in(&rec), 0, "the match did not connect again");
    new.stop_all();
}

#[test]
fn a_release_taken_over_closes_its_recording_into_the_file_as_it_goes_for_the_next_to_read_on() {
    let dir = TempDir::new("handover");
    let target = "192.0.2.55:27020";
    script(target, stream("cp_a", 1000..=1700), None);
    let (old, _) = start_on(&dir);
    let m = old.open(&format!("{target} tv"), None, None).unwrap();
    until("the old release's lines", || ticks_of(&m.ready().unwrap()).last() == Some(&1700));
    // The new release is still signing on: it has no stream, and has not asked.
    let (new, _) = start_on(&dir);
    let rec = new.get(&m.id).unwrap().ready().unwrap();
    goes_on(&m.hub, 1800..=2000);
    until("what the old release recorded since", || ticks_of(&rec).last() == Some(&2000));
    assert!(m.ready().unwrap().is_written() && !rec.is_written(), "the file is the old release's still");
    old.stop_all();
    new.stop_all();
}

#[test]
fn a_release_gone_while_it_writes_a_file_on_is_not_waited_for() {
    let dir = TempDir::new("handover");
    let (old, m, new, n) = asked_for_at(&dir, "192.0.2.56:27020", 9000);
    let rec = n.ready().unwrap();
    until("the old release to hear", || m.giving.load(Ordering::Relaxed));
    // The old release crashes: its recorders stop, and nothing is said.
    for s in old.lock().values() {
        s.stop.store(true, Ordering::Relaxed);
    }
    let gone = Instant::now();
    until("the file to be the new release's", || rec.is_written());
    assert!(gone.elapsed() >= GIVE_WAIT / 2 && gone.elapsed() < GIVE_BY / 2, "{:?}", gone.elapsed());
    until("its own line", || ticks_of(&rec).last() == Some(&9000));
    assert_eq!(ticks_of(&rec), [every_100(1000..=1700), vec![9000]].concat(), "what the file had, and its own");
    old.stop_all();
    new.stop_all();
}

/// A deploy stops the old release before the new one's stream has a tick: it waits to be asked.
#[test]
fn a_release_stopped_before_it_is_asked_waits_for_the_next_one_s_stream() {
    let dir = TempDir::new("handover");
    let target = "192.0.2.47:27020";
    script(target, stream("cp_a", 1000..=1700), None);
    let (old, _) = start_on(&dir);
    let m = old.open(&format!("{target} tv"), None, None).unwrap();
    let before = m.ready().unwrap();
    until("the old release's lines", || ticks_of(&before).last() == Some(&1700));
    // The new release is still in its signon when the old one is told to stop.
    let (new, _) = start_on(&dir);
    let picked = new.get(&m.id).unwrap();
    let rec = picked.ready().unwrap();
    let stopping = std::thread::spawn(move || old.stop_all());
    std::thread::sleep(Duration::from_millis(100));
    assert!(before.is_written() && !stopping.is_finished(), "not given yet: nobody asked");
    for line in stream("cp_a", 1500..=2000) {
        crate::matches::hub::publish(&picked.hub, &line);
    }
    until("the file to be the new release's", || rec.is_written());
    stopping.join().unwrap();
    until("its own lines", || ticks_of(&rec).last() == Some(&2000));
    assert_eq!(ticks_of(&rec), every_100(1000..=2000), "none missing, none twice");
    new.stop_all();
}
