//! A match as a room plays it: stvtf lines in gzipped chunks of play time, each starting from a
//! checkpoint of the hub, so a seek replays at most one chunk. A live match's recording grows
//! as it is played: its last chunk stays open, as text, until its time is up, and each chunk
//! closed can go to a file (one gzip member after another), from which it resumes. A new map
//! starts a part, and nothing before it is dropped for that; what is more than [`KEEP_MS`] old is,
//! though chunks and marks keep their numbers.

mod parts;

#[allow(unused_imports, reason = "the rooms use it once they show maps")]
pub(crate) use parts::Part;

use crate::lock;
use crate::matches::hub::{Hub, SharedHub, publish};
use crate::matches::replay::{Clock, read_line};
use anyhow::{Result, bail};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use serde::Serialize;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use stvtf::Line;
use stvtf::output::{Stamped, Team};

/// Play time per chunk, in milliseconds.
const CHUNK_MS: u64 = 10_000;

/// Play time a live recording keeps, in milliseconds, dropping more once over by [`TRIM_MS`].
const KEEP_MS: u64 = 3 * 3600 * 1000;
const TRIM_MS: u64 = 10 * 60 * 1000;

/// Where a live recording's chunks go as they close.
pub(crate) type Sink = Box<dyn FnMut(&Save) + Send>;

/// For a live recording's file: a chunk closed (its gzipped lines), or all the file is now.
pub(crate) enum Save {
    Add(Vec<u8>),
    All(Vec<u8>),
}

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
    /// Play time of the first line kept.
    start: u64,
    chunks: Vec<Chunk>,
    /// Chunks and marks dropped before these.
    dropped: usize,
    marks_dropped: usize,
    /// The last chunk's lines while it is open: `{at}\t{json}`.
    open: Vec<String>,
    rounds: Vec<u64>,
    /// The next line starts a chunk: the last one was resumed, and is in its file already.
    fresh: bool,
    /// Lines in all.
    count: usize,
    /// What a timeline shows.
    marks: Vec<Mark>,
    /// The latest score and when, for the next round won.
    scores: Option<(u64, [u16; 2])>,
    /// The match's map loads, those kept, oldest first; dropped ones before are counted.
    parts: Vec<parts::PartStart>,
    parts_dropped: usize,
    /// Loads so far of each map, dropped parts too.
    names: HashMap<String, usize>,
    /// The tick of the latest snapshot of the part going on.
    last_tick: Option<u32>,
}

/// A score and a round won this close (ms) belong together, either first.
const SCORE_WITH_WIN_MS: u64 = 2000;

/// A timeline mark: a round won (with the score), an uber, a drop, or a kill (for fight heat).
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

    /// Drops the chunks before play time `before`, and the marks and rounds before what is left;
    /// the round going on stays.
    fn trim(&mut self, before: u64) {
        let n = self.chunks.partition_point(|c| c.start < before).min(self.chunks.len().saturating_sub(1));
        self.chunks.drain(..n);
        self.dropped += n;
        self.start = self.chunks.first().map_or(self.start, |c| c.start);
        let n = self.marks.partition_point(|m| m.at < self.start);
        self.marks.drain(..n);
        self.marks_dropped += n;
        let n = self.rounds.partition_point(|r| *r < self.start).saturating_sub(1);
        self.rounds.drain(..n);
        self.drop_parts();
    }

    /// The file: a header (with the match as it was at the first chunk), then the closed chunks.
    fn file(&self) -> Result<Vec<u8>> {
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        writeln!(gz, "origin\t{}\nmarks\t{}", self.start, self.marks_dropped)?;
        for line in self.chunks.first().into_iter().flat_map(|c| c.hub.state()) {
            writeln!(gz, "hub\t{line}")?;
        }
        let mut file = gz.finish()?;
        let closed = if self.open.is_empty() { &self.chunks[..] } else { &self.chunks[..self.chunks.len() - 1] };
        for c in closed {
            file.extend_from_slice(&c.lines);
        }
        Ok(file)
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

    /// Line `text`, read as `stamped`, at play time `at`; what the file gets, if anything.
    fn add(&mut self, at: u64, stamped: &Stamped, text: &str) -> Result<Option<Save>> {
        let mut save = None;
        let new_part = self.begin_part(at, &stamped.line);
        // A new map's chunk starts from its server line, so a seek to its origin lands on the map.
        let published = new_part && matches!(stamped.line, Line::Server(_));
        if published {
            publish(&self.scratch, &stamped.line);
        }
        if self.fresh || new_part || self.chunks.last().is_none_or(|c| at >= c.start + CHUNK_MS) {
            if self.close()? {
                save = self.chunks.last().map(|c| Save::Add(c.lines.clone()));
            }
            let hub = lock(&self.scratch).checkpoint();
            self.chunks.push(Chunk { start: at, hub, lines: Vec::new() });
            self.fresh = false;
            if at.saturating_sub(self.start) > KEEP_MS + TRIM_MS {
                self.trim(at - KEEP_MS);
                save = Some(Save::All(self.file()?));
            }
        }
        if matches!(stamped.line, Line::RoundStart(_)) {
            self.rounds.push(at);
        }
        self.open.push(format!("{at}\t{text}"));
        self.count += 1;
        self.mark(at, &stamped.line);
        if !published {
            publish(&self.scratch, &stamped.line);
        }
        Ok(save)
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
    pub(crate) fn resume(saved: impl Read, mut sink: Sink) -> Result<Recording> {
        let r = Recording::live();
        {
            let mut inner = lock(&r.inner);
            let (mut at, mut cut) = (0, false);
            for text in BufReader::new(flate2::read::MultiGzDecoder::new(saved)).lines() {
                let text = text?;
                let Some((key, value)) = text.split_once('\t') else { bail!("not a recording") };
                match key {
                    "origin" => inner.start = value.parse()?,
                    "marks" => inner.marks_dropped = value.parse()?,
                    "hub" => publish(&inner.scratch, &serde_json::from_str(value)?),
                    _ => {
                        at = key.parse()?;
                        cut |= matches!(inner.add(at, &serde_json::from_str(value)?, value)?, Some(Save::All(_)));
                    }
                }
            }
            inner.close()?;
            inner.clock.resume(at);
            inner.start = inner.chunks.first().map_or(inner.start, |c| c.start);
            inner.fresh = true;
            // A file trimmed as it was read: its remains are the file.
            if cut {
                sink(&Save::All(inner.file()?));
            }
        }
        *lock(&r.sink) = Some(sink);
        Ok(r)
    }

    /// The next line, `text` as read into `stamped`.
    pub(crate) fn push(&self, stamped: &Stamped, text: &str) -> Result<()> {
        let save = {
            let mut inner = lock(&self.inner);
            let at = inner.clock.at(stamped);
            inner.add(at, stamped, text)?
        };
        if let (Some(save), Some(sink)) = (save, lock(&self.sink).as_mut()) {
            sink(&save);
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

    /// Play time of the first line kept.
    pub(crate) fn start(&self) -> u64 {
        lock(&self.inner).start
    }

    /// Play time of the latest part's first line.
    pub(crate) fn origin(&self) -> u64 {
        self.latest_origin()
    }

    /// Play times of the latest part's rounds' starts.
    pub(crate) fn rounds(&self) -> Vec<u64> {
        self.latest_rounds()
    }

    /// What a timeline shows, from the `from`th on, and how many there are in all, the dropped
    /// ones too.
    pub(crate) fn marks(&self, from: usize) -> (Vec<Mark>, usize) {
        let inner = lock(&self.inner);
        let kept = from.saturating_sub(inner.marks_dropped);
        (inner.marks.get(kept..).unwrap_or_default().to_vec(), inner.marks_dropped + inner.marks.len())
    }

    /// Lines in all: more of a live match's means more to play, even at the same play time.
    pub(crate) fn count(&self) -> usize {
        lock(&self.inner).count
    }

    /// Chunks in all, the dropped ones too.
    pub(crate) fn chunks(&self) -> usize {
        let inner = lock(&self.inner);
        inner.dropped + inner.chunks.len()
    }

    /// Chunk `i`'s lines, so far if it is still open; none if dropped.
    pub(crate) fn chunk(&self, i: usize) -> Option<Lines> {
        enum Text {
            Gz(Vec<u8>),
            Open(Vec<String>),
        }
        let text = {
            let inner = lock(&self.inner);
            let i = i.checked_sub(inner.dropped)?;
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

    /// The hub just before play time `at` (the start, if before), the chunk it is in, and how
    /// many of its lines are before it.
    pub(crate) fn seek(&self, at: u64) -> (Hub, usize, usize) {
        let (i, hub, at) = {
            let inner = lock(&self.inner);
            let at = at.max(inner.start);
            let i = inner.chunks.partition_point(|c| c.start <= at).saturating_sub(1);
            let Some(chunk) = inner.chunks.get(i) else { return (Hub::default(), inner.dropped, 0) };
            (inner.dropped + i, chunk.hub.checkpoint(), at)
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
        live_lines(&r, 30, 1_700_000_000_000);
        // 0 to 14.5 s: a closed chunk, and an open one of 10 lines (10 to 14.5 s) so far.
        assert_eq!((r.length(), r.chunks(), r.is_live()), (14_500, 2, true));
        assert_eq!(r.chunk(1).unwrap().len(), 10);
        let (hub, i, played) = r.seek(12_000);
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
        let (hub, _, _) = again.seek(31_500);
        assert_eq!(hub.history.len(), 40 + 24, "every line once");
        let resumed = Recording::resume(lock(&saved).as_slice(), Box::new(|_: &Save| {})).unwrap();
        assert_eq!(resumed.chunks(), 3, "the chunks saved before, and the one closed after");
    }

    /// Line `line` at `time` into `r`.
    pub(super) fn push_at(r: &Recording, line: &Line, time: u64) {
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
        assert_eq!(r.origin(), 0, "the map began where it did");
        assert!(r.chunk(0).is_none());
        let (hub, _, _) = r.seek(0);
        assert_eq!(hub.map.as_deref(), Some("cp_long"), "the match as it was at the start");
        let again = Recording::resume(lock(&saved).as_slice(), to(&saved)).unwrap();
        assert_eq!(again.start(), start, "the file was trimmed too");
        let (hub, _, _) = again.seek(0);
        assert_eq!(hub.map.as_deref(), Some("cp_long"), "and keeps the match as it was");
    }

    #[test]
    fn a_recording_marks_rounds_won_ubers_drops_and_kills() {
        use crate::matches::hub::tests::{player, snapshot};
        // Who the lines' players are: 3 and 5 blue (5 the medic), 4 red.
        let who = vec![player(3, "scout", Team::Blue), player(4, "soldier", Team::Red), player(5, "medic", Team::Blue)];
        let mut lines = serde_json::to_string(&snapshot(100, None, who)).unwrap() + "\n";
        lines += include_str!("../../testdata/lines.jsonl");
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
