//! A room waiting at the end of its map hears the voice said in the gap, as it is said.

use super::move_tests::on_map;
use super::move_tests::{decided, live_room_at_live_on, push_server};
use super::part_tests::push;
use super::stream_tests::{a_pass, at_live};
use super::tests::until;
use super::*;
use crate::matches::hub::{subscribe_to, tests::snapshot};
use crate::matches::recording::part_tests::Stream;
use crate::matches::recording::real_part_tests::viaduct;
use base64::Engine;
use serde_json::Value;
use std::sync::mpsc::Receiver;
use stvtf::Line;
use stvtf::output::{PlayerId, Voice};

/// Voice from outside the match, its one frame `n`; said in a pause or not.
fn said(n: u8, pause: bool) -> Line {
    let frame = base64::engine::general_purpose::STANDARD.encode([n]);
    Line::Voice(Voice {
        tick: 1,
        player: Some(PlayerId(7)),
        steamid: "[U:1:5]".into(),
        rate: 24000,
        frames: vec![frame],
        injected: true,
        pause,
        team: None,
        name: None,
    })
}

/// Voice `n` said now in `rec`.
fn say(rec: &Recording, n: u8, pause: bool) {
    push(rec, &said(n, pause), rec.length() + 20);
}

/// A page of `room` that hears voice, past what it joined with.
fn listener(room: &Room) -> Receiver<Arc<str>> {
    let page = subscribe_to(&room.hub, false, true);
    let _joined: Vec<_> = page.try_iter().collect();
    page
}

/// The frames of the voice `page` heard since last asked.
fn heard(page: &Receiver<Arc<str>>) -> Vec<u8> {
    let lines = page.try_iter().map(|l| serde_json::from_str::<Value>(&l).unwrap()).filter(|l| l["type"] == "voice");
    let b64 = base64::engine::general_purpose::STANDARD;
    lines.map(|l| b64.decode(l["frames"][0].as_str().unwrap()).unwrap()[0]).collect()
}

/// The frames of the voice the room gave its relay.
fn relayed(room: &Room) -> Vec<u8> {
    lock(&room.relayed).iter().map(|v| v.frames[0][0]).collect()
}

/// A watched room at live on `cp_a` when the server changes map to `cp_b`: it waits at the end,
/// having read the tail so far.
fn waiting_in_the_gap() -> (Arc<Room>, Arc<Recording>) {
    let (room, rec) = live_room_at_live_on("cp_a");
    push_server(&rec, "cp_b");
    assert!(decided(&room, 0));
    a_pass(&room);
    (room, rec)
}

#[test]
fn a_room_waiting_in_the_gap_hears_its_voice_as_it_is_said_but_not_in_a_pause() {
    let (room, rec) = waiting_in_the_gap();
    let page = listener(&room);
    say(&rec, 1, false);
    say(&rec, 2, true);
    say(&rec, 3, false);
    say(&rec, 4, false);
    until("three said", || relayed(&room).len() == 3);
    assert_eq!(relayed(&room), [1, 3, 4]);
    assert_eq!(heard(&page), [1, 3, 4]);
    assert_eq!(lock(&room.state).at, rec.part(0).unwrap().last, "still at the end");
    room.stop();
}

#[test]
fn a_room_back_at_the_end_hears_only_what_is_said_after() {
    let (room, rec) = waiting_in_the_gap();
    let last = rec.part(0).unwrap().last;
    room.control(&Control { paused: Some(true), seek: Some(last - 100), ..Control::default() }).unwrap();
    until("rewound", || {
        let st = lock(&room.state);
        st.seek.is_none() && st.at == last - 100
    });
    say(&rec, 1, false);
    room.control(&Control { paused: Some(false), seek: Some(u64::MAX), ..Control::default() }).unwrap();
    until("at the end again", || {
        let st = lock(&room.state);
        st.seek.is_none() && st.at == last
    });
    a_pass(&room);
    let page = listener(&room);
    say(&rec, 2, false);
    until("said after", || !relayed(&room).is_empty());
    a_pass(&room);
    assert_eq!((relayed(&room), heard(&page)), (vec![2], vec![2]));
    room.stop();
}

#[test]
fn a_room_on_a_map_going_on_hears_voice_once_as_it_plays() {
    let (room, rec) = live_room_at_live_on("cp_a");
    let page = listener(&room);
    say(&rec, 1, false);
    until("said", || !relayed(&room).is_empty());
    a_pass(&room);
    assert_eq!((relayed(&room), heard(&page)), (vec![1], vec![1]));
    room.stop();
}

#[test]
fn a_room_waiting_hears_the_gap_past_the_next_maps_frozen_frame_until_it_plays() {
    let rec = Arc::new(Recording::live());
    let mut s = Stream { r: &rec, s: 0.0 };
    viaduct(&mut s);
    let room = Room::start("r".into(), "me", rec.clone());
    room.viewers.fetch_add(1, Ordering::Relaxed);
    at_live(&room);
    // `fixed_change`, the frozen frame apart from the play.
    s.status(1.0, "changelevel");
    s.status(5.0, "map_change");
    s.signon(7.0, "koth_nucleus");
    s.push(15.0, &snapshot(68, None, Vec::new()));
    assert!(decided(&room, 0));
    a_pass(&room);
    let page = listener(&room);
    s.push(20.0, &said(1, false));
    s.push(20.0, &said(2, false));
    until("said past the frozen frame", || relayed(&room).len() == 2);
    s.snapshot(40.0, 250);
    s.play(317, 2);
    on_map(&room, 1);
    a_pass(&room);
    assert_eq!((relayed(&room), heard(&page)), (vec![1, 2], vec![1, 2]), "once each");
    room.stop();
}
