//! A match as a room plays it: stvtf lines in gzipped chunks of play time, each starting from a
//! checkpoint of the hub, so a seek replays at most one chunk.

use crate::hub::{Hub, SharedHub, publish};
use crate::lock;
use crate::replay::{Clock, read_line};
use anyhow::{Result, bail};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::{Arc, Mutex};
use stvtf::Line;

/// Play time per chunk, in milliseconds.
const CHUNK_MS: u64 = 10_000;

pub(crate) struct Recording {
    /// Play time of the last line, in milliseconds.
    pub(crate) length: u64,
    /// Play times of the rounds' starts.
    pub(crate) rounds: Vec<u64>,
    chunks: Vec<Chunk>,
}

struct Chunk {
    /// Play time of its first line.
    start: u64,
    /// The hub just before it.
    hub: Hub,
    /// Gzipped `{at}\t{json}\n` lines.
    lines: Vec<u8>,
}

/// Lines to play, with their play times.
pub(crate) type Lines = Vec<(u64, Line)>;

impl Recording {
    /// Saved stvtf output (as `stvtf replay` writes it); lines it cannot read are skipped.
    pub(crate) fn read(reader: impl Read, name: &str) -> Result<Recording> {
        let scratch = SharedHub::default();
        let (mut clock, mut skipped) = (Clock::default(), 0);
        let (mut chunks, mut rounds, mut gz) = (Vec::<Chunk>::new(), Vec::new(), None::<GzEncoder<Vec<u8>>>);
        for (n, text) in BufReader::new(reader).lines().enumerate() {
            let text = text?;
            let Some(stamped) = read_line(&text, name, n, &mut skipped) else { continue };
            let at = clock.at(&stamped);
            if chunks.last().is_none_or(|c| at >= c.start + CHUNK_MS) {
                if let (Some(last), Some(done)) = (chunks.last_mut(), gz.take()) {
                    last.lines = done.finish()?;
                }
                chunks.push(Chunk { start: at, hub: lock(&scratch).checkpoint(), lines: Vec::new() });
                gz = Some(GzEncoder::new(Vec::new(), Compression::default()));
            }
            if matches!(stamped.line, Line::RoundStart(_)) {
                rounds.push(at);
            }
            writeln!(gz.as_mut().expect("a chunk is open"), "{at}\t{text}")?;
            publish(&scratch, &stamped.line);
        }
        if let (Some(last), Some(done)) = (chunks.last_mut(), gz) {
            last.lines = done.finish()?;
        }
        if !clock.timed {
            bail!("{name}: no line has a time or tick");
        }
        Ok(Recording { length: clock.play, rounds, chunks })
    }

    pub(crate) fn chunks(&self) -> usize {
        self.chunks.len()
    }

    /// Chunk `i`'s lines.
    pub(crate) fn chunk(&self, i: usize) -> Option<Lines> {
        let chunk = self.chunks.get(i)?;
        let mut lines = Vec::new();
        for text in BufReader::new(GzDecoder::new(chunk.lines.as_slice())).lines() {
            let text = text.expect("our own gzip reads");
            let (at, json) = text.split_once('\t').expect("our own lines");
            let line = serde_json::from_str::<stvtf::output::Stamped>(json).expect("read once already").line;
            lines.push((at.parse().expect("our own number"), line));
        }
        Some(lines)
    }

    /// The hub just before play time `at`, what is left of its chunk from there, and the chunk.
    pub(crate) fn seek(&self, at: u64) -> (Hub, Lines, usize) {
        let i = self.chunks.partition_point(|c| c.start <= at).saturating_sub(1);
        let Some(chunk) = self.chunks.get(i) else { return (Hub::default(), Vec::new(), 0) };
        let scratch = Arc::new(Mutex::new(chunk.hub.checkpoint()));
        let mut lines = self.chunk(i).unwrap_or_default();
        let played = lines.partition_point(|(t, _)| *t < at);
        for (_, line) in lines.drain(..played) {
            publish(&scratch, &line);
        }
        let hub = lock(&scratch).checkpoint();
        (hub, lines, i)
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
        assert_eq!(r.length, 24_000);
        assert_eq!(r.rounds, [13_500]);
        assert_eq!(r.chunks.iter().map(|c| c.start).collect::<Vec<_>>(), [0, 10_500, 21_000]);
        assert_eq!(r.chunk(1).unwrap().len(), 14);
    }

    #[test]
    fn seeking_matches_playing_through() {
        let r = Recording::read(script(1650).as_bytes(), "test").unwrap();
        let (hub, rest, i) = r.seek(15_000);
        assert_eq!(i, 1);
        // Lines at 0, 0.75, … 14.25 s are played: 20 of them.
        assert_eq!(hub.history.len(), 20);
        assert_eq!(rest.first().map(|(t, _)| *t), Some(15_000));
    }

    #[test]
    fn seeking_past_the_end_gives_nothing_to_play() {
        let r = Recording::read(script(1650).as_bytes(), "test").unwrap();
        let (hub, rest, i) = r.seek(99_000);
        assert_eq!((hub.history.len(), rest.len(), i), (33, 0, 2));
    }

    #[test]
    fn a_recording_needs_times() {
        assert!(Recording::read(r#"{"type":"status","state":"new"}"#.as_bytes(), "test").is_err());
    }
}
