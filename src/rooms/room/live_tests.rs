//! A room at live keeps up with its recording: through a file read on in steps while a deploy's
//! next process picks the match up, and where the room is picked up itself.

use super::move_tests::live_room_at_live_on;
use super::part_tests::push;
use super::play::LIVE_SLACK;
use super::stream_tests::at_live;
use super::tests::until;
use super::*;
use crate::matches::hub::tests::snapshot;
use crate::matches::recording::Save;
use crate::testing::server;

/// The match goes on in `rec` for `ms`: a snapshot every 100 ms.
fn plays_on(rec: &Recording, ms: u64) {
    let from = rec.length();
    for at in (from + 100..=from + ms).step_by(100) {
        push(rec, &snapshot(1000 + (at / 15) as u32, None, Vec::new()), at);
    }
}

#[test]
fn a_room_at_live_is_at_live_while_its_recording_catches_up_with_its_file_and_after() {
    // The process that writes the file, and the one that picked the match up and reads it on.
    let file: Arc<Mutex<Vec<u8>>> = Arc::default();
    let to = file.clone();
    let old = Recording::live_to(Box::new(move |save: &Save| match save {
        Save::Add(gz) => lock(&to).extend_from_slice(gz),
        Save::All(gz) => lock(&to).clone_from(gz),
    }));
    push(&old, &server("cp_a"), 0);
    plays_on(&old, 1000);
    old.flush();
    let rec = Arc::new(Recording::live());
    let mut read = rec.read_on(&lock(&file)).unwrap();
    rec.set_catching_up(true);
    let room = Room::start("r".into(), "me", rec.clone());
    room.viewers.fetch_add(1, Ordering::Relaxed);
    at_live(&room);
    let mut step = |ms| {
        plays_on(&old, ms);
        old.flush();
        read += rec.read_on(&lock(&file)[read..]).unwrap();
    };
    // The chunk its writer had open, seconds of it at once: not played through.
    step(8000);
    let came = Instant::now();
    until("the room near its end", || rec.length() - lock(&room.state).at <= LIVE_SLACK);
    assert!(came.elapsed() < Duration::from_secs(2), "{:?}", came.elapsed());
    // A step is played as it comes; the file taken, the room is at the match's latest.
    step(2500);
    rec.set_catching_up(false);
    let taken = Instant::now();
    at_live(&room);
    assert!(taken.elapsed() < Duration::from_secs(1), "{:?}", taken.elapsed());
    room.stop();
}

#[test]
fn a_room_saved_at_live_is_at_live_where_it_is_picked_up_and_one_moved_is_where_it_was() {
    let (room, rec) = live_room_at_live_on("cp_a");
    let live = room.saved();
    room.control(&Control { paused: Some(true), seek: Some(100), ..Control::default() }).unwrap();
    until("the seek", || lock(&room.state).seek.is_none());
    let moved = room.saved();
    room.stop();
    // The match went on before the next process picked the room up.
    plays_on(&rec, 5000);
    let again = Room::restore("r".into(), live, rec.clone());
    again.viewers.fetch_add(1, Ordering::Relaxed);
    let picked_up = Instant::now();
    at_live(&again);
    assert!(picked_up.elapsed() < Duration::from_secs(1), "{:?}", picked_up.elapsed());
    again.stop();
    let again = Room::restore("r".into(), moved, rec.clone());
    until("the seek", || lock(&again.state).seek.is_none());
    let st = lock(&again.state);
    assert_eq!((st.at, st.paused), (100, true));
    drop(st);
    again.stop();
}
