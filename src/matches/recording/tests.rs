use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use stvtf::Line;
use stvtf::output::{Stamped, Team};

use super::lines::{KEEP_MS, Recording, Save, Sink, TRIM_MS};
use crate::sync::lock;

/// A line every 50 ticks (0.75 s) up to `ticks`, with a round start at tick 950.
pub(super) fn script(ticks: u32) -> String {
    let mut out = String::new();
    for tick in (50..=ticks).step_by(50) {
        let kind = if tick == 950 { r#""round_start","full_reset":false"# } else { r#""stalemate""# };
        writeln!(out, "{{\"type\":{kind},\"tick\":{tick}}}").unwrap();
    }
    out
}

#[test]
fn a_recording_is_chunked_by_play_time() {
    // 50..=1650 ticks: 0 to 24 s of play.
    let r = Recording::read(script(1650).as_bytes(), "test").unwrap();
    assert_eq!(r.length(), 24_000);
    assert_eq!(r.rounds_in(0), [13_500]);
    assert_eq!(lock(&r.inner).chunks.iter().map(|c| c.start).collect::<Vec<_>>(), [0, 10_500, 21_000]);
    assert_eq!(r.chunk(1).unwrap().len(), 14);
}

#[test]
fn seeking_matches_playing_through() {
    let r = Recording::read(script(1650).as_bytes(), "test").unwrap();
    let (hub, i, played) = r.seek(15_000, None);
    assert_eq!(i, 1);
    // Lines at 0, 0.75, … 14.25 s are played: 20 of them.
    assert_eq!(hub.history.len(), 20);
    assert_eq!(r.chunk(i).unwrap()[played].0, 15_000);
}

#[test]
fn seeking_past_the_end_gives_nothing_to_play() {
    let r = Recording::read(script(1650).as_bytes(), "test").unwrap();
    let (hub, i, played) = r.seek(99_000, None);
    assert_eq!((hub.history.len(), i, played), (33, 2, r.chunk(2).unwrap().len()));
}

#[test]
fn a_live_recording_grows_and_its_open_chunk_plays() {
    let r = Recording::live();
    live_lines(&r, 30, 1_700_000_000_000);
    // 0 to 14.5 s: a closed chunk, and an open one of 10 lines (10 to 14.5 s) so far.
    assert_eq!((r.length(), r.chunks(), r.is_live()), (14_500, 2, true));
    assert_eq!(r.chunk(1).unwrap().len(), 10);
    let (hub, i, played) = r.seek(12_000, None);
    assert_eq!((hub.history.len(), i, played), (24, 1, 4));
    r.end();
    assert!(!r.is_live());
}

/// `n` stalemates half a second apart from `time`, into `r`; the time after.
pub(super) fn live_lines(r: &Recording, n: usize, mut time: u64) -> u64 {
    for _ in 0..n {
        let text = r#"{"type":"stalemate","tick":5}"#;
        let mut stamped: Stamped = serde_json::from_str(text).unwrap();
        stamped.time = Some(time);
        r.push(&stamped, text).unwrap();
        time += 500;
    }
    time
}

/// A sink that keeps what it gets in `saved`, as the file would.
pub(super) fn to(saved: &Arc<Mutex<Vec<u8>>>) -> Sink {
    let saved = saved.clone();
    Box::new(move |save: &Save| match save {
        Save::Add(gz) => lock(&saved).extend_from_slice(gz),
        Save::All(gz) => *lock(&saved) = gz.clone(),
    })
}

#[test]
fn a_live_recording_saves_its_chunks_and_goes_on_from_them() {
    let saved = Arc::new(Mutex::new(Vec::new()));
    let r = Recording::live_to(to(&saved));
    let time = live_lines(&r, 45, 1_700_000_000_000);
    // 0 to 22 s: two chunks closed and saved, the third open.
    assert_eq!((r.chunks(), r.length()), (3, 22_000));
    let file = lock(&saved).clone();
    let again = Recording::resume(file.as_slice(), to(&saved)).unwrap();
    assert_eq!((again.chunks(), again.length(), again.is_live()), (2, 19_500, true));
    // On after a pause (a deploy): from where it was, in a chunk of its own.
    live_lines(&again, 25, time + 60_000);
    assert_eq!((again.chunks(), again.length()), (4, 31_500));
    assert_eq!(again.chunk(2).unwrap().first().map(|(t, _)| *t), Some(19_500));
    let (hub, _, _) = again.seek(31_500, None);
    assert_eq!(hub.history.len(), 40 + 24, "every line once");
    let resumed = Recording::resume(lock(&saved).as_slice(), Box::new(|_: &Save| {})).unwrap();
    assert_eq!(resumed.chunks(), 3, "the chunks saved before, and the one closed after");
}

/// Line `line` at `time` into `r`.
pub(crate) fn push_at(r: &Recording, line: &Line, time: u64) {
    let text = serde_json::to_string(line).unwrap();
    let mut stamped: Stamped = serde_json::from_str(&text).unwrap();
    stamped.time = Some(time);
    r.push(&stamped, &text).unwrap();
}

#[test]
fn a_live_recording_keeps_the_last_hours() {
    use crate::testing::server;
    let saved = Arc::new(Mutex::new(Vec::new()));
    let r = Recording::live_to(to(&saved));
    let t0 = 1_700_000_000_000;
    push_at(&r, &server("cp_long"), t0);
    // A line a second, up to just past what is kept and trimmed at once.
    let last = (KEEP_MS + TRIM_MS) / 1000 + 10;
    let stalemate: Line = serde_json::from_str(r#"{"type":"stalemate","tick":5}"#).unwrap();
    for s in 1..=last {
        push_at(&r, &stalemate, t0 + s * 1000);
    }
    let start = r.start();
    assert!(start > 0 && last * 1000 - start <= KEEP_MS, "{start}");
    assert_eq!(r.latest().origin, 0, "the map began where it did");
    assert!(r.chunk(0).is_none());
    let (hub, _, _) = r.seek(0, None);
    assert_eq!(hub.map.as_deref(), Some("cp_long"), "the match as it was at the start");
    let again = Recording::resume(lock(&saved).as_slice(), to(&saved)).unwrap();
    assert_eq!(again.start(), start, "the file was trimmed too");
    let (hub, _, _) = again.seek(0, None);
    assert_eq!(hub.map.as_deref(), Some("cp_long"), "and keeps the match as it was");
}

#[test]
fn a_recording_marks_rounds_won_ubers_drops_and_kills() {
    use crate::matches::hub::tests::{player, snapshot};
    // Who the lines' players are: 3 and 5 blue (5 the medic), 4 red.
    let who = vec![player(3, "scout", Team::Blue), player(4, "soldier", Team::Red), player(5, "medic", Team::Blue)];
    let mut lines = serde_json::to_string(&snapshot(100, None, who)).unwrap() + "\n";
    lines += include_str!("../../../testdata/lines.jsonl");
    let r = Recording::read(lines.as_bytes(), "lines").unwrap();
    let (marks, all) = r.marks(0);
    assert_eq!(all, marks.len());
    let win = marks.iter().find(|m| m.kind == "win").unwrap();
    assert_eq!((win.team, win.score), (Team::Red, Some([2, 3])), "the score after it, blue's first");
    let uber = marks.iter().find(|m| m.kind == "uber").unwrap();
    assert_eq!((uber.team, uber.who.as_deref()), (Team::Blue, Some("medic")));
    assert!(marks.iter().any(|m| m.kind == "drop" && m.team == Team::Blue));
    assert!(marks.iter().any(|m| m.kind == "kill"));
    assert_eq!(r.marks(all).0, [], "none new");
}

#[test]
fn a_score_just_before_a_round_won_is_its_score() {
    let lines = r#"{"type":"scores","tick":5,"red":2,"blue":4,"game_over":false}
{"type":"round_win","tick":5,"team":"red","reason":1,"round_time":532.4}
{"type":"scores","tick":900,"red":3,"blue":4,"game_over":false}
{"type":"stalemate","tick":1200}
{"type":"stalemate","tick":1500}
{"type":"stalemate","tick":1800}
{"type":"round_win","tick":2000,"team":"red","reason":1,"round_time":9.0}
"#;
    let marks = Recording::read(lines.as_bytes(), "test").unwrap().marks(0).0;
    assert_eq!(marks.iter().map(|m| m.score).collect::<Vec<_>>(), [Some([4, 2]), None], "a score long before is not");
}

#[test]
fn a_recording_needs_times() {
    assert!(Recording::read(r#"{"type":"status","state":"new"}"#.as_bytes(), "test").is_err());
}
