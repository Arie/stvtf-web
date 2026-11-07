//! A match as a room plays it: stvtf lines in gzipped chunks of play time, each starting from a
//! checkpoint of the hub, so a seek replays at most one chunk. A live match's recording grows
//! as it is played: its last chunk stays open, as text, until its time is up.

use crate::hub::{Hub, SharedHub, publish};
use crate::lock;
use crate::replay::{Clock, read_line};
use anyhow::{Result, bail};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use stvtf::Line;
use stvtf::output::Stamped;

/// Play time per chunk, in milliseconds.
const CHUNK_MS: u64 = 10_000;

#[derive(Default)]
pub(crate) struct Recording {
    inner: Mutex<Inner>,
    /// Still being recorded (a live match): playback waits at its end for more.
    live: AtomicBool,
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
    /// Gzips the open chunk's lines.
    fn close(&mut self) -> Result<()> {
        let Some(last) = self.chunks.last_mut() else { return Ok(()) };
        if self.open.is_empty() {
            return Ok(());
        }
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        for line in self.open.drain(..) {
            writeln!(gz, "{line}")?;
        }
        last.lines = gz.finish()?;
        Ok(())
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

    /// The next line, `text` as read into `stamped`.
    pub(crate) fn push(&self, stamped: &Stamped, text: &str) -> Result<()> {
        let mut inner = lock(&self.inner);
        let at = inner.clock.at(stamped);
        if inner.chunks.last().is_none_or(|c| at >= c.start + CHUNK_MS) {
            inner.close()?;
            let hub = lock(&inner.scratch).checkpoint();
            inner.chunks.push(Chunk { start: at, hub, lines: Vec::new() });
        }
        if matches!(stamped.line, Line::RoundStart(_)) {
            inner.rounds.push(at);
        }
        inner.open.push(format!("{at}\t{text}"));
        publish(&inner.scratch, &stamped.line);
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

    #[test]
    fn a_recording_needs_times() {
        assert!(Recording::read(r#"{"type":"status","state":"new"}"#.as_bytes(), "test").is_err());
    }
}
