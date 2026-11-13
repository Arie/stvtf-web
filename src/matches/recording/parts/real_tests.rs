//! Map changes as a real server sent them at `tv_delay 90` (the rig's capture, `probe-tv90.log`):
//! the server's word, a long silence, one frozen frame of the new map, a disconnect, the map
//! announced again and again, then play.

use super::super::*;
use super::tests::{Stream, line};
use crate::matches::hub::tests::{player, snapshot};
use stvtf::output::Team;

/// Play times of a change's lines.
pub(crate) struct Change {
    pub(crate) changelevel: u64,
    /// The first server line naming the next map.
    pub(crate) server: u64,
    /// The frozen frame (tick 68), and the first snapshot of play.
    pub(crate) frozen: u64,
    pub(crate) play: u64,
}

impl Stream<'_> {
    /// A change to `map` as the capture has it, up to the frozen frame; the play times so far.
    pub(crate) fn change_to_frozen(&mut self, map: &str) -> Change {
        let changelevel = self.status(1.0, "changelevel");
        self.status(5.4, "map_change");
        let server = self.signon(17.8, map);
        let frozen = self.snapshot(6.0, 68);
        Change { changelevel, server, frozen, play: 0 }
    }

    /// From the frozen frame: the disconnect, the map announced `repeats` + 2 times, the round
    /// starting, play from `tick` for `seconds`.
    pub(crate) fn after_frozen(&mut self, change: &mut Change, map: &str, repeats: usize, tick: u32, seconds: u32) {
        let disconnected = r#"{"type":"status","state":"disconnected","reason":"disconnected by server: Connection error","decode_errors":0,"packets":1498,"lost":0}"#;
        self.push(4.2, &line(disconnected));
        self.push(1.9, &line(r#"{"type":"status","state":"connecting","server":"127.0.0.1:27020","build":"11191711"}"#));
        for _ in 0..=repeats {
            self.signon(17.8, map);
            self.status(0.02, "map_change");
        }
        self.signon(17.7, map);
        self.push(2.2, &line(r#"{"type":"round_start","full_reset":false,"tick":250}"#));
        change.play = self.snapshot(0.0, tick);
        self.play(tick + 67, seconds);
    }

    /// A change to `map` as the stream is with the client's fault fixed: one signon, the frozen
    /// frame (one player), 80 s of nothing, play from `tick` for `seconds`.
    pub(crate) fn fixed_change(&mut self, map: &str, tick: u32, seconds: u32) -> Change {
        let changelevel = self.status(1.0, "changelevel");
        self.status(5.0, "map_change");
        let server = self.signon(7.0, map);
        let one = vec![player(2, "scout", Team::Blue)];
        let frozen = self.push(15.0, &snapshot(68, None, one));
        let play = self.snapshot(80.0, tick);
        self.play(tick + 67, seconds);
        Change { changelevel, server, frozen, play }
    }

    /// A whole change to `map`, its play from `tick` for `seconds`.
    pub(crate) fn change(&mut self, map: &str, tick: u32, seconds: u32) -> Change {
        let mut change = self.change_to_frozen(map);
        self.after_frozen(&mut change, map, 3, tick, seconds);
        change
    }
}

/// `koth_viaduct` from its signon, its play up to tick 35084; the last snapshot's play time.
pub(crate) fn viaduct(s: &mut Stream) -> u64 {
    s.push(0.0, &line(r#"{"type":"status","state":"connecting","server":"127.0.0.1:27020","build":"11191711"}"#));
    s.signon(18.8, "koth_viaduct");
    s.play(35084 - 156 * 67, 157)
}

#[test]
fn the_real_map_change_is_two_parts() {
    let r = Recording::live();
    let mut s = Stream { r: &r, s: 0.0 };
    let last = viaduct(&mut s);
    let mut change = s.change_to_frozen("koth_nucleus");
    assert!(!r.latest().ready && r.latest().load == 1);
    s.after_frozen(&mut change, "koth_nucleus", 3, 244, 0);
    assert!(r.latest().ready, "at its first snapshot of play");
    let parts = r.parts();
    assert_eq!(parts.iter().map(|p| (p.load, p.map.as_str())).collect::<Vec<_>>(), [(0, "koth_viaduct"), (1, "koth_nucleus")]);
    let (old, new) = (&parts[0], &parts[1]);
    assert_eq!((old.changing, old.tail, old.last, old.end), (Some(change.changelevel), Some(change.server), last, Some(new.origin)));
    // It begins at its play: the frozen frame and the break are the map before's.
    assert_eq!((new.origin, new.first_tick, new.ready), (change.play, Some(68), true));
    assert!(old.last < change.frozen && change.frozen < new.origin, "the frozen frame falls between");
    assert!(new.changing.is_none() && !new.over(), "the announcements again were no change of its own");
    assert_eq!(new.last, r.length());
    let (hub, ..) = r.seek(new.origin, None);
    assert_eq!((hub.map.as_deref(), hub.tick(), hub.has_snapshot()), (Some("koth_nucleus"), Some(244), true));
    let (hub, ..) = r.seek(old.end.unwrap(), Some(old.last));
    assert_eq!((hub.map.as_deref(), hub.tick()), (Some("koth_viaduct"), Some(35084)), "its last moment");
}

#[test]
fn the_same_map_again_is_two_parts() {
    let r = Recording::live();
    let mut s = Stream { r: &r, s: 0.0 };
    s.signon(0.0, "koth_nucleus");
    let last = s.play(9716 - 59 * 67, 60);
    let change = s.change("koth_nucleus", 272, 3);
    let parts = r.parts();
    assert_eq!(parts.iter().map(|p| (p.load, p.map.as_str())).collect::<Vec<_>>(), [(0, "koth_nucleus"), (1, "koth_nucleus")]);
    assert_eq!((parts[0].last, parts[0].tail, parts[0].end), (last, Some(change.server), Some(change.play)));
    assert!(parts[1].ready && !parts[1].over());
    assert_eq!(r.rounds_in(1), [change.play], "the round starting before its play is its own");
}

#[test]
fn a_late_snapshot_keeps_the_map_over() {
    let r = Recording::live();
    let mut s = Stream { r: &r, s: 0.0 };
    s.signon(0.0, "koth_viaduct");
    s.play(1000, 60);
    let changelevel = s.status(1.0, "changelevel");
    let late = s.snapshot(0.1, 1000 + 59 * 67 + 6);
    assert_eq!((r.latest().changing, r.latest().last), (Some(changelevel), late));
    s.status(4.5, "map_change");
    assert_eq!(r.latest().changing, Some(changelevel), "the silence keeps it so");
    assert_eq!((r.parts().len(), r.latest().last), (1, late));
    s.signon(17.8, "koth_nucleus");
    assert_eq!(r.parts().len(), 1, "one part till the next map's first snapshot");
    s.snapshot(6.0, 68);
    assert_eq!(r.parts().len(), 2);
    assert_eq!(r.part(0).unwrap().last, late);
}

#[test]
fn a_map_that_goes_on_is_not_over() {
    let r = Recording::live();
    let mut s = Stream { r: &r, s: 0.0 };
    s.signon(0.0, "koth_viaduct");
    let tick = 1000 + 59 * 67;
    s.play(1000, 60);
    let changelevel = s.status(1.0, "changelevel");
    s.play(tick + 67, 4);
    assert_eq!(r.latest().changing, Some(changelevel), "four seconds of ticks on: still changing");
    s.play(tick + 5 * 67, 3);
    let part = r.latest();
    assert_eq!((r.parts().len(), part.changing, part.over(), part.last), (1, None, false, r.length()));
}

#[test]
fn a_reconnect_is_no_new_part() {
    let r = Recording::live();
    let mut s = Stream { r: &r, s: 0.0 };
    s.signon(0.0, "koth_viaduct");
    s.play(1000, 60);
    s.push(1.0, &line(r#"{"type":"status","state":"disconnected","reason":"timed out","decode_errors":0,"packets":9,"lost":0}"#));
    s.push(1.9, &line(r#"{"type":"status","state":"connecting","server":"127.0.0.1:27020","build":"11191711"}"#));
    s.signon(2.0, "koth_viaduct");
    s.play(1000 + 70 * 67, 5);
    let part = r.latest();
    assert_eq!((r.parts().len(), part.load, part.changing, part.tail, part.over()), (1, 0, None, None, false));
}

#[test]
fn a_frozen_frame_is_not_play() {
    let r = Recording::live();
    let mut s = Stream { r: &r, s: 0.0 };
    viaduct(&mut s);
    let mut change = s.change_to_frozen("koth_nucleus");
    let latest = r.latest();
    assert_eq!((latest.load, latest.ready, latest.first_tick), (1, false, Some(68)));
    assert_eq!(r.watchable().load, 0);
    s.after_frozen(&mut change, "koth_nucleus", 0, 244, 1);
    assert_eq!(r.watchable().load, 1);
}

#[test]
fn a_new_load_that_never_plays() {
    let r = Recording::live();
    let mut s = Stream { r: &r, s: 0.0 };
    s.signon(0.0, "cp_a");
    s.play(1000, 30);
    s.change_to_frozen("cp_b");
    s.change_to_frozen("cp_c");
    s.snapshot(1.0, 300);
    s.play(367, 3);
    let parts = r.parts();
    let seen: Vec<_> = parts.iter().map(|p| (p.load, p.map.as_str(), p.ready)).collect();
    assert_eq!(seen, [(0, "cp_a", true), (1, "cp_b", false), (2, "cp_c", true)]);
    assert!(parts[1].over() && parts[1].changing.is_none(), "over by its successor: one with no play is not changing");
    assert_eq!(r.watchable().load, 2);
}

#[test]
fn a_map_change_with_one_signon_is_two_parts() {
    for map in ["koth_nucleus", "koth_viaduct"] {
        let r = Recording::live();
        let mut s = Stream { r: &r, s: 0.0 };
        let last = viaduct(&mut s);
        let change = s.fixed_change(map, 250, 0);
        let parts = r.parts();
        assert_eq!(parts.iter().map(|p| (p.load, p.map.as_str())).collect::<Vec<_>>(), [(0, "koth_viaduct"), (1, map)]);
        let (old, new) = (&parts[0], &parts[1]);
        assert_eq!((old.changing, old.tail, old.last, old.end), (Some(change.changelevel), Some(change.server), last, Some(change.play)));
        assert_eq!((new.origin, new.first_tick, new.ready, new.over()), (change.play, Some(68), true, false));
        let (hub, ..) = r.seek(new.origin, None);
        assert_eq!((hub.map.as_deref(), hub.tick()), (Some(map), Some(250)));
    }
}

#[test]
fn the_frozen_frame_alone_is_not_play_with_one_signon_either() {
    let r = Recording::live();
    let mut s = Stream { r: &r, s: 0.0 };
    viaduct(&mut s);
    s.status(1.0, "changelevel");
    s.status(5.0, "map_change");
    s.signon(7.0, "koth_nucleus");
    s.snapshot(15.0, 68);
    assert_eq!((r.latest().load, r.latest().ready, r.watchable().load), (1, false, 0));
    s.snapshot(80.0, 250);
    assert_eq!((r.latest().ready, r.watchable().load), (true, 1));
}

#[test]
fn a_map_that_goes_on_through_announcements_is_not_over() {
    // The faulty client reconnecting mid-map: the server announces it again before play goes on.
    let r = Recording::live();
    let mut s = Stream { r: &r, s: 0.0 };
    s.signon(0.0, "koth_viaduct");
    let tick = 1000 + 59 * 67;
    s.play(1000, 60);
    s.status(1.0, "map_change");
    s.signon(2.0, "koth_viaduct");
    s.signon(2.0, "koth_viaduct");
    assert!(r.latest().tail.is_some() && r.latest().over());
    s.play(tick + 67, 4);
    assert!(r.latest().over(), "four seconds of ticks on: still changing");
    s.play(tick + 5 * 67, 3);
    let part = r.latest();
    assert_eq!((r.parts().len(), part.changing, part.tail, part.over(), part.last), (1, None, None, false, r.length()));
    let length = r.length();
    assert_eq!(lock(&r.inner).parts.list[0].last, length, "it ends at its latest play again");
}

/// Voice from outside the match (no-delay commentary): recorded at its real time.
fn commentary() -> Line {
    line(
        r#"{"type":"voice","tick":1,"player":null,"steamid":"[U:1:5]","rate":24000,"frames":["aAEC"],"injected":true,"pause":false,"team":null,"name":null}"#,
    )
}

#[test]
fn a_later_map_begins_at_its_first_frame_of_play_not_its_frozen_one() {
    let r = Recording::live();
    let mut s = Stream { r: &r, s: 0.0 };
    viaduct(&mut s);
    s.status(1.0, "changelevel");
    s.status(5.0, "map_change");
    s.signon(7.0, "koth_nucleus");
    let frozen = s.push(15.0, &snapshot(68, None, vec![player(2, "scout", Team::Blue)]));
    // The casters talk through the break.
    for _ in 0..80 {
        s.push(0.9, &commentary());
    }
    s.push(0.5, &line(r#"{"type":"round_start","full_reset":false,"tick":250}"#));
    let play = s.snapshot(0.5, 250);
    s.play(317, 3);
    assert!(play - frozen > 60_000, "the talk kept the break's time");
    let p = r.part(1).unwrap();
    assert_eq!((p.load, p.map.as_str(), p.ready), (1, "koth_nucleus", true));
    assert_eq!((p.origin, p.start), (play, play), "its clock starts at play");
    assert_eq!(r.rounds_in(1).len(), 1, "its round");
    let (hub, _, _) = r.seek(p.origin, None);
    assert!(hub.has_snapshot() && hub.map.as_deref() == Some("koth_nucleus") && hub.tick() == Some(250), "its first moment of play");
}
