//! A match as a room plays it: stvtf lines in gzipped chunks of play time, each starting from a
//! checkpoint of the hub, so a seek replays at most one chunk. A live match's recording grows
//! as it is played: its last chunk stays open, as text, until its time is up, and each chunk
//! closed can go to a file (one gzip member after another), from which it resumes.

use crate::hub::{Hub, SharedHub, publish};
use crate::lock;
use crate::replay::{Clock, read_line};
use anyhow::{Result, bail};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use serde::Serialize;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use stvtf::Line;
use stvtf::output::{Stamped, Team};

/// Play time per chunk, in milliseconds.
const CHUNK_MS: u64 = 10_000;

/// Where a live recording's chunks go as they close: their gzipped lines.
pub(crate) type Sink = Box<dyn FnMut(&[u8]) + Send>;

#[derive(Default)]
pub(crate) struct Recording {
    inner: Mutex<Inner>,
    /// Still being recorded (a live match): playback waits at its end for more.
    live: AtomicBool,
    sink: Mutex<Option<Sink>>,
}

#[derive(Default)]
struct Inner {
    /// The match as it is at the latest line, for the next chunk's checkpoint.
    scratch: SharedHub,
    clock: Clock,
    chunks: Vec<Chunk>,
    /// The last chunk's lines while it is open: `{at}\t{json}`.
    open: Vec<String>,
    rounds: Vec<u64>,
    /// The next line starts a chunk: the last one was resumed, and is in its file already.
    fresh: bool,
    /// Lines in all.
    count: usize,
    /// What a timeline shows.
    marks: Vec<Mark>,
    /// The latest score, and when: a round won takes it if it came with it.
    scores: Option<(u64, [u16; 2])>,
}

/// A score and a round won this close together, in play time, go together (either comes first).
const SCORE_WITH_WIN_MS: u64 = 2000;

/// A moment a timeline marks: a round won (with the score after), an uber, a drop (a medic dying
/// with it ready), or a kill (for where the fights were), with the team and who, if anyone.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Mark {
    pub(crate) at: u64,
    pub(crate) kind: &'static str,
    pub(crate) team: Team,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) who: Option<String>,
    /// After a round won: blue's and red's score.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) score: Option<[u16; 2]>,
}

struct Chunk {
    /// Play time of its first line.
    start: u64,
    /// The hub just before it.
    hub: Hub,
    /// Gzipped `{at}\t{json}\n` lines, once closed.
    lines: Vec<u8>,
}

/// Lines to play, with their play times.
pub(crate) type Lines = Vec<(u64, Line)>;

impl Inner {
    /// Marks `line`, at play time `at`, if a timeline shows it.
    fn mark(&mut self, at: u64, line: &Line) {
        let mark = |kind, team, who: Option<&str>| Mark { at, kind, team, who: who.map(str::to_string), score: None };
        let h = lock(&self.scratch);
        let player = |id| h.seen(id);
        let new = match line {
            Line::RoundWin(w) => {
                let score = self.scores.filter(|(t, _)| at - t <= SCORE_WITH_WIN_MS).map(|(_, s)| s);
                Some(Mark { score, ..mark("win", w.team, None) })
            }
            Line::Uber(u) => player(u.medic).map(|(name, team)| mark("uber", team, Some(name))),
            Line::MedicDeath(d) if d.drop => player(d.medic).map(|(name, team)| mark("drop", team, Some(name))),
            Line::Kill(k) if k.attacker.is_some_and(|a| a != k.victim) => {
                k.attacker.and_then(player).map(|(_, team)| mark("kill", team, None))
            }
            Line::Scores(s) => {
                drop(h);
                self.scores = Some((at, [s.blue, s.red]));
                // The score after a round just won.
                let win = self.marks.iter_mut().rev().find(|m| m.kind == "win");
                if let Some(win) = win.filter(|m| m.score.is_none() && at - m.at <= SCORE_WITH_WIN_MS) {
                    win.score = Some([s.blue, s.red]);
                }
                return;
            }
            _ => None,
        };
        drop(h);
        self.marks.extend(new);
    }

    /// Gzips the open chunk's lines; true if there were any.
    fn close(&mut self) -> Result<bool> {
        let Some(last) = self.chunks.last_mut() else { return Ok(false) };
        if self.open.is_empty() {
            return Ok(false);
        }
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        for line in self.open.drain(..) {
            writeln!(gz, "{line}")?;
        }
        last.lines = gz.finish()?;
        Ok(true)
    }

    /// Line `text`, read as `stamped`, at play time `at`; the gzipped chunk it closed, if any.
    fn add(&mut self, at: u64, stamped: &Stamped, text: &str) -> Result<Option<Vec<u8>>> {
        let mut closed = None;
        if self.fresh || self.chunks.last().is_none_or(|c| at >= c.start + CHUNK_MS) {
            if self.close()? {
                closed = self.chunks.last().map(|c| c.lines.clone());
            }
            let hub = lock(&self.scratch).checkpoint();
            self.chunks.push(Chunk { start: at, hub, lines: Vec::new() });
            self.fresh = false;
        }
        if matches!(stamped.line, Line::RoundStart(_)) {
            self.rounds.push(at);
        }
        self.open.push(format!("{at}\t{text}"));
        self.count += 1;
        self.mark(at, &stamped.line);
        publish(&self.scratch, &stamped.line);
        Ok(closed)
    }
}

impl Recording {
    /// Saved stvtf output (as `stvtf replay` writes it); lines it cannot read are skipped.
    pub(crate) fn read(reader: impl Read, name: &str) -> Result<Recording> {
        let r = Recording::default();
        let mut skipped = 0;
        for (n, text) in BufReader::new(reader).lines().enumerate() {
            let text = text?;
            if let Some(stamped) = read_line(&text, name, n, &mut skipped) {
                r.push(&stamped, &text)?;
            }
        }
        let mut inner = lock(&r.inner);
        inner.close()?;
        if !inner.clock.timed {
            bail!("{name}: no line has a time or tick");
        }
        drop(inner);
        Ok(r)
    }

    /// A recording of a live match, from now on.
    pub(crate) fn live() -> Recording {
        let r = Recording::default();
        r.live.store(true, Ordering::Relaxed);
        r
    }

    /// A live recording whose chunks go to `sink` as they close.
    pub(crate) fn live_to(sink: Sink) -> Recording {
        let r = Recording::live();
        *lock(&r.sink) = Some(sink);
        r
    }

    /// A live recording going on from the chunks `saved` (as a sink got them), to `sink`.
    pub(crate) fn resume(saved: impl Read, sink: Sink) -> Result<Recording> {
        let r = Recording::live();
        {
            let mut inner = lock(&r.inner);
            let mut at = 0;
            for text in BufReader::new(flate2::read::MultiGzDecoder::new(saved)).lines() {
                let text = text?;
                let Some((t, json)) = text.split_once('\t') else { bail!("not a recording") };
                at = t.parse()?;
                inner.add(at, &serde_json::from_str(json)?, json)?;
            }
            inner.close()?;
            inner.clock.resume(at);
            inner.fresh = true;
        }
        *lock(&r.sink) = Some(sink);
        Ok(r)
    }

    /// The next line, `text` as read into `stamped`.
    pub(crate) fn push(&self, stamped: &Stamped, text: &str) -> Result<()> {
        let closed = {
            let mut inner = lock(&self.inner);
            let at = inner.clock.at(stamped);
            inner.add(at, stamped, text)?
        };
        if let (Some(gz), Some(sink)) = (closed, lock(&self.sink).as_mut()) {
            sink(&gz);
        }
        Ok(())
    }

    pub(crate) fn is_live(&self) -> bool {
        self.live.load(Ordering::Relaxed)
    }

    /// No more lines come: the match ended, or the room did.
    pub(crate) fn end(&self) {
        self.live.store(false, Ordering::Relaxed);
    }

    /// Play time of the last line, in milliseconds.
    pub(crate) fn length(&self) -> u64 {
        lock(&self.inner).clock.play
    }

    /// Play times of the rounds' starts.
    pub(crate) fn rounds(&self) -> Vec<u64> {
        lock(&self.inner).rounds.clone()
    }

    /// What a timeline shows, from the `from`th on, and how many there are in all.
    pub(crate) fn marks(&self, from: usize) -> (Vec<Mark>, usize) {
        let inner = lock(&self.inner);
        (inner.marks.get(from..).unwrap_or_default().to_vec(), inner.marks.len())
    }

    /// Lines in all: more of a live match's means more to play, even at the same play time.
    pub(crate) fn count(&self) -> usize {
        lock(&self.inner).count
    }

    pub(crate) fn chunks(&self) -> usize {
        lock(&self.inner).chunks.len()
    }

    /// Chunk `i`'s lines, so far if it is still open.
    pub(crate) fn chunk(&self, i: usize) -> Option<Lines> {
        enum Text {
            Gz(Vec<u8>),
            Open(Vec<String>),
        }
        let text = {
            let inner = lock(&self.inner);
            let chunk = inner.chunks.get(i)?;
            if i + 1 == inner.chunks.len() && !inner.open.is_empty() {
                Text::Open(inner.open.clone())
            } else {
                Text::Gz(chunk.lines.clone())
            }
        };
        let read = |text: &str| {
            let (at, json) = text.split_once('\t').expect("our own lines");
            let line = serde_json::from_str::<Stamped>(json).expect("read once already").line;
            (at.parse().expect("our own number"), line)
        };
        Some(match text {
            Text::Open(lines) => lines.iter().map(|l| read(l)).collect(),
            Text::Gz(gz) => BufReader::new(GzDecoder::new(gz.as_slice())).lines().map(|l| read(&l.expect("our own gzip reads"))).collect(),
        })
    }

    /// The hub just before play time `at`, the chunk it is in, and how many of its lines are
    /// before it.
    pub(crate) fn seek(&self, at: u64) -> (Hub, usize, usize) {
        let (i, hub) = {
            let inner = lock(&self.inner);
            let i = inner.chunks.partition_point(|c| c.start <= at).saturating_sub(1);
            let Some(chunk) = inner.chunks.get(i) else { return (Hub::default(), 0, 0) };
            (i, chunk.hub.checkpoint())
        };
        let scratch = Arc::new(Mutex::new(hub));
        let lines = self.chunk(i).unwrap_or_default();
        let played = lines.partition_point(|(t, _)| *t < at);
        for (_, line) in &lines[..played] {
            publish(&scratch, line);
        }
        let hub = lock(&scratch).checkpoint();
        (hub, i, played)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;

    /// A line every 50 ticks (0.75 s) up to `ticks`, with a round start at tick 950.
    fn script(ticks: u32) -> String {
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
        assert_eq!(r.rounds(), [13_500]);
        assert_eq!(lock(&r.inner).chunks.iter().map(|c| c.start).collect::<Vec<_>>(), [0, 10_500, 21_000]);
        assert_eq!(r.chunk(1).unwrap().len(), 14);
    }

    #[test]
    fn seeking_matches_playing_through() {
        let r = Recording::read(script(1650).as_bytes(), "test").unwrap();
        let (hub, i, played) = r.seek(15_000);
        assert_eq!(i, 1);
        // Lines at 0, 0.75, … 14.25 s are played: 20 of them.
        assert_eq!(hub.history.len(), 20);
        assert_eq!(r.chunk(i).unwrap()[played].0, 15_000);
    }

    #[test]
    fn seeking_past_the_end_gives_nothing_to_play() {
        let r = Recording::read(script(1650).as_bytes(), "test").unwrap();
        let (hub, i, played) = r.seek(99_000);
        assert_eq!((hub.history.len(), i, played), (33, 2, r.chunk(2).unwrap().len()));
    }

    #[test]
    fn a_live_recording_grows_and_its_open_chunk_plays() {
        let r = Recording::live();
        let mut time = 1_700_000_000_000;
        for _ in 0..30 {
            let text = r#"{"type":"stalemate","tick":5}"#;
            let mut stamped: Stamped = serde_json::from_str(text).unwrap();
            stamped.time = Some(time);
            r.push(&stamped, text).unwrap();
            time += 500;
        }
        // 0 to 14.5 s: a closed chunk, and an open one of 10 lines (10 to 14.5 s) so far.
        assert_eq!((r.length(), r.chunks(), r.is_live()), (14_500, 2, true));
        assert_eq!(r.chunk(1).unwrap().len(), 10);
        let (hub, i, played) = r.seek(12_000);
        assert_eq!((hub.history.len(), i, played), (24, 1, 4));
        r.end();
        assert!(!r.is_live());
    }

    /// `n` stalemates half a second apart from `time`, into `r`; the time after.
    fn live_lines(r: &Recording, n: usize, mut time: u64) -> u64 {
        for _ in 0..n {
            let text = r#"{"type":"stalemate","tick":5}"#;
            let mut stamped: Stamped = serde_json::from_str(text).unwrap();
            stamped.time = Some(time);
            r.push(&stamped, text).unwrap();
            time += 500;
        }
        time
    }

    #[test]
    fn a_live_recording_saves_its_chunks_and_goes_on_from_them() {
        let saved = Arc::new(Mutex::new(Vec::new()));
        let to = |saved: &Arc<Mutex<Vec<u8>>>| -> Sink {
            let saved = saved.clone();
            Box::new(move |gz: &[u8]| lock(&saved).extend_from_slice(gz))
        };
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
        let (hub, _, _) = again.seek(31_500);
        assert_eq!(hub.history.len(), 40 + 24, "every line once");
        let resumed = Recording::resume(lock(&saved).as_slice(), Box::new(|_: &[u8]| {})).unwrap();
        assert_eq!(resumed.chunks(), 3, "the chunks saved before, and the one closed after");
    }

    #[test]
    fn a_recording_marks_rounds_won_ubers_drops_and_kills() {
        use crate::hub::tests::{player, snapshot};
        // Who the lines' players are: 3 and 5 blue (5 the medic), 4 red.
        let who = vec![player(3, "scout", Team::Blue), player(4, "soldier", Team::Red), player(5, "medic", Team::Blue)];
        let mut lines = serde_json::to_string(&snapshot(100, None, who)).unwrap() + "\n";
        lines += include_str!("../testdata/lines.jsonl");
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
}
