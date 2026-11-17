//! A match's lines, kept in chunks of time: recorded, marked and read back.

use std::io::{BufRead, BufReader, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Result, bail};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use serde::Serialize;
use stvtf::Line;
use stvtf::output::{Stamped, Status, Team};

use crate::matches::hub::{Hub, SharedHub, publish};
use crate::matches::loads::Loads;
use crate::matches::replay::{Clock, read_line};
use crate::sync::lock;

/// Play time per chunk, in milliseconds.
pub(super) const CHUNK_MS: u64 = 10_000;

/// Play time a live recording keeps, in milliseconds, dropping more once over by [`TRIM_MS`].
pub(super) const KEEP_MS: u64 = 3 * 3600 * 1000;

pub(super) const TRIM_MS: u64 = 10 * 60 * 1000;

/// Where a live recording's chunks go as they close.
pub(crate) type Sink = Box<dyn FnMut(&Save) + Send>;

/// For a live recording's file: a chunk closed (its gzipped lines), or all the file is now.
pub(crate) enum Save {
    Add(Vec<u8>),
    All(Vec<u8>),
}

#[derive(Default)]
pub(crate) struct Recording {
    pub(super) inner: Mutex<Inner>,
    /// Still being recorded (a live match): playback waits at its end for more.
    pub(super) live: AtomicBool,
    pub(super) sink: Mutex<Option<Sink>>,
    /// Picked up from a file another process still writes: its end is not the match's latest yet.
    pub(super) catching_up: AtomicBool,
}

#[derive(Default)]
pub(super) struct Inner {
    /// The match as it is at the latest line, for the next chunk's checkpoint.
    pub(super) scratch: SharedHub,
    pub(super) clock: Clock,
    /// Play time of the first line kept (of the first chunk).
    pub(super) start: u64,
    pub(super) chunks: Vec<Chunk>,
    /// Chunks and marks dropped before these.
    pub(super) dropped: usize,
    pub(super) marks_dropped: usize,
    /// The last chunk's lines while it is open: `{at}\t{json}`.
    pub(super) open: Vec<String>,
    pub(super) rounds: Vec<u64>,
    /// The next line starts a chunk: the last one was resumed, and is in its file already.
    pub(super) fresh: bool,
    /// Lines in all.
    pub(super) count: usize,
    /// What a timeline shows.
    pub(super) marks: Vec<Mark>,
    /// The latest score and when, for the next round won.
    pub(super) scores: Option<(u64, [u16; 2])>,
    /// The match's map loads, and its parts kept.
    pub(super) parts: super::parts::Parts,
    /// The file is written whole at the next chunk closed: its header says what chunks alone do
    /// not (the first part's number), or it holds what this does not.
    pub(super) rewrite: bool,
    /// The header of the file it is read from, until that file's first line.
    pub(super) header: Option<super::parts::Header>,
    /// The tick of the latest line that has one.
    pub(super) last_tick: Option<u32>,
}

/// A score and a round won this close (ms) belong together, either first.
pub(super) const SCORE_WITH_WIN_MS: u64 = 2000;

/// A timeline mark: a round won (with the score), an uber, a drop, or a kill (for fight heat).
#[derive(Clone, Debug, PartialEq, Serialize)]
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

pub(super) struct Chunk {
    /// Play time of its first line.
    pub(super) start: u64,
    /// The hub just before it, and the parts.
    pub(super) hub: Hub,
    pub(super) seed: super::parts::Seed,
    /// Gzipped `{at}\t{json}\n` lines, once closed.
    pub(super) lines: Vec<u8>,
}

/// Lines to play, with their play times.
pub(crate) type Lines = Vec<(u64, Line)>;

impl Inner {
    /// Marks `line`, at play time `at`, if a timeline shows it.
    pub(super) fn mark(&mut self, at: u64, line: &Line) {
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
    pub(super) fn trim(&mut self, before: u64) {
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
    pub(super) fn file(&self) -> Result<Vec<u8>> {
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        writeln!(gz, "marks\t{}", self.marks_dropped)?;
        gz.write_all(self.parts_header()?.as_bytes())?;
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
    pub(super) fn close(&mut self) -> Result<bool> {
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

    /// Closes the open chunk; what the file gets for it, if it had lines.
    pub(super) fn closed(&mut self) -> Result<Option<Save>> {
        if !self.close()? {
            return Ok(None);
        }
        if std::mem::take(&mut self.rewrite) {
            return Ok(Some(Save::All(self.file()?)));
        }
        Ok(self.chunks.last().map(|c| Save::Add(c.lines.clone())))
    }

    /// The lines of `text`, a gzip member of a file: its header, if this is the file's start,
    /// and chunks as a sink got them.
    pub(super) fn read_lines(&mut self, text: &str) -> Result<()> {
        for text in text.lines() {
            let Some((key, value)) = text.split_once('\t') else { bail!("cannot read a file that is not a recording") };
            let heads = self.header.is_some();
            match key {
                "marks" if heads => self.marks_dropped = value.parse()?,
                "hub" if heads => publish(&self.scratch, &serde_json::from_str(value)?),
                _ if self.header.as_mut().map(|h| h.read(key, value)).transpose()?.unwrap_or(false) => {}
                // The header of a file written anew since its start was read.
                _ if super::parts::Header::has(key) => {}
                _ => {
                    // Play time only goes on in a file: a chunk that begins before the latest
                    // line is of a stretch another process wrote too, and skipped whole.
                    let at = key.parse()?;
                    if at < self.clock.play {
                        self.rewrite = true;
                        return Ok(());
                    }
                    if let Some(header) = self.header.take() {
                        self.start = at;
                        let scratch = self.scratch.clone();
                        self.parts.seed_from(header, at, &scratch);
                    }
                    // A file trimmed as it is read: its remains are the file.
                    self.rewrite |= matches!(self.add(at, &serde_json::from_str(value)?, value)?, Some(Save::All(_)));
                    self.clock.play = at;
                }
            }
        }
        Ok(())
    }

    /// Line `text`, read as `stamped`, at play time `at`; what the file gets, if anything.
    pub(super) fn add(&mut self, at: u64, stamped: &Stamped, text: &str) -> Result<Option<Save>> {
        let mut save = None;
        // A part's chunk starts at its first snapshot: its checkpoint holds its map as announced.
        let new_part = self.parts.starts_part(&stamped.line);
        if self.fresh || new_part || self.chunks.last().is_none_or(|c| at >= c.start + CHUNK_MS) {
            save = self.closed()?;
            let hub = lock(&self.scratch).checkpoint();
            self.chunks.push(Chunk { start: at, hub, seed: self.parts.seed(), lines: Vec::new() });
            self.fresh = false;
            if at.saturating_sub(self.start) > KEEP_MS + TRIM_MS {
                self.trim(at - KEEP_MS);
                save = Some(Save::All(self.file()?));
            }
        }
        self.note_part_line(at, &stamped.line);
        if matches!(stamped.line, Line::RoundStart(_)) {
            self.rounds.push(at);
        }
        self.open.push(format!("{at}\t{text}"));
        self.count += 1;
        self.last_tick = stamped.line.tick().filter(|t| *t > 0).or(self.last_tick);
        self.mark(at, &stamped.line);
        publish(&self.scratch, &stamped.line);
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
            bail!("cannot play {name}: no line has a time or tick");
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

    /// A live recording going on from the chunks `saved` (as a sink got them), to `sink`: what a
    /// process alone on the file makes of it.
    #[cfg(test)]
    pub(crate) fn resume(mut saved: impl Read, mut sink: Sink) -> Result<Recording> {
        let r = Recording::live();
        let mut file = Vec::new();
        saved.read_to_end(&mut file)?;
        let whole = r.read_on(&file)? == file.len();
        {
            let mut inner = lock(&r.inner);
            if inner.count == 0 && !file.is_empty() {
                bail!("cannot read a file that is not a recording");
            }
            // A file trimmed as it was read, with lines skipped, or torn: its remains are the file.
            if std::mem::take(&mut inner.rewrite) || !whole {
                sink(&Save::All(inner.file()?));
            }
        }
        *lock(&r.sink) = Some(sink);
        Ok(r)
    }

    /// Takes in `saved`, the next bytes of the file a recording not pushed to yet goes on from:
    /// its whole gzip members, up to one torn or unreadable. How many bytes that was. What is
    /// before its latest line in play time is skipped, and the file is then to be written anew.
    pub(crate) fn read_on(&self, saved: &[u8]) -> Result<usize> {
        let mut inner = lock(&self.inner);
        if inner.count == 0 && inner.header.is_none() {
            inner.header = Some(super::parts::Header::default());
        }
        let mut taken = 0;
        while taken < saved.len() {
            let mut member = flate2::bufread::GzDecoder::new(&saved[taken..]);
            let mut text = String::new();
            if member.read_to_string(&mut text).is_err() {
                break;
            }
            inner.read_lines(&text)?;
            taken = saved.len() - member.into_inner().len();
        }
        inner.close()?;
        let at = inner.clock.play;
        inner.clock.resume(at);
        inner.drop_parts();
        inner.start = inner.chunks.first().map_or(inner.start, |c| c.start);
        inner.fresh = true;
        Ok(taken)
    }

    /// Its chunks go to `sink` from now on.
    pub(crate) fn write_to(&self, sink: Sink) {
        *lock(&self.sink) = Some(sink);
    }

    /// Its end is not the match's latest until the file another process writes is taken.
    pub(crate) fn set_catching_up(&self, catching_up: bool) {
        self.catching_up.store(catching_up, Ordering::Relaxed);
    }

    pub(crate) fn is_catching_up(&self) -> bool {
        self.catching_up.load(Ordering::Relaxed)
    }

    /// Whether its chunks go to a file.
    pub(crate) fn is_written(&self) -> bool {
        lock(&self.sink).is_some()
    }

    /// The file is written whole at the next chunk closed: it holds what this does not.
    pub(crate) fn rewrite(&self) {
        lock(&self.inner).rewrite = true;
    }

    /// Closes the open chunk into the file, which gets no more: another process goes on from
    /// it. False if nothing wrote it.
    pub(crate) fn give(&self) -> bool {
        let written = self.flush();
        *lock(&self.sink) = None;
        written
    }

    /// Closes the open chunk into the file, for another process to read on from. False if
    /// nothing writes it.
    pub(crate) fn flush(&self) -> bool {
        let mut sink = lock(&self.sink);
        let Some(write) = sink.as_mut() else { return false };
        let mut inner = lock(&self.inner);
        if let Ok(Some(save)) = inner.closed() {
            write(&save);
        }
        // What comes after is a chunk of its own: this one is in the file.
        inner.fresh = true;
        true
    }

    /// Writes its file no more, as it is: another process took it.
    pub(crate) fn let_go(&self) {
        *lock(&self.sink) = None;
    }

    /// The tick of its latest line that has one.
    pub(crate) fn last_tick(&self) -> Option<u32> {
        lock(&self.inner).last_tick
    }

    /// The next line, `text` as read into `stamped`.
    pub(crate) fn push(&self, stamped: &Stamped, text: &str) -> Result<()> {
        // Held from the line to its chunk in the file: none goes in after the file is given.
        let mut sink = lock(&self.sink);
        let save = {
            let mut inner = lock(&self.inner);
            let at = inner.clock.at(stamped);
            inner.add(at, stamped, text)?
        };
        if let (Some(save), Some(sink)) = (save, sink.as_mut()) {
            sink(&save);
        }
        Ok(())
    }

    pub(crate) fn is_live(&self) -> bool {
        self.live.load(Ordering::Relaxed)
    }

    /// The match's map loads, as its lines told them.
    pub(crate) fn loads(&self) -> Loads {
        lock(&self.inner).parts.loads.clone()
    }

    /// Its first part is the match's `n`th load, if it has shown nothing yet: the loads before it
    /// were counted by the demos' recorder.
    pub(crate) fn first_load_is(&self, n: usize) {
        let mut inner = lock(&self.inner);
        inner.parts.loads.first_is(n);
        // A file must say so: read alone, its lines would number it 0.
        for c in &mut inner.chunks {
            c.seed.first_load_is(n);
        }
        inner.rewrite |= n != 0;
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
            let (at, json) = text.split_once('\t').expect("internal error: a kept line has no time");
            let line = serde_json::from_str::<Stamped>(json).expect("internal error: a kept line does not read back").line;
            (at.parse().expect("internal error: a kept line's time is not a number"), line)
        };
        Some(match text {
            Text::Open(lines) => lines.iter().map(|l| read(l)).collect(),
            Text::Gz(gz) => BufReader::new(GzDecoder::new(gz.as_slice()))
                .lines()
                .map(|l| read(&l.expect("internal error: a kept chunk does not unzip")))
                .collect(),
        })
    }

    /// The hub just before play time `at` (the start, if before), the chunk it is in, and how
    /// many of its lines are before it; in a map over whose last line not tail is at `last`, at
    /// or past it, the map's last moment, with that line. At a part's first snapshot, with it.
    /// Never the server's word that it changes map: a room shows the map it is on to its end.
    pub(crate) fn seek(&self, at: u64, last: Option<u64>) -> (Hub, usize, usize) {
        let (i, hub, at, with_at) = {
            let inner = lock(&self.inner);
            let at = at.max(inner.start);
            let (at, with_at) = match last {
                Some(last) if at >= last => (last, true),
                _ => (at, inner.parts.first_snapshot_at(at)),
            };
            let i = inner.chunks.partition_point(|c| c.start <= at).saturating_sub(1);
            let Some(chunk) = inner.chunks.get(i) else { return (Hub::default(), inner.dropped, 0) };
            (inner.dropped + i, chunk.hub.checkpoint(), at, with_at)
        };
        let scratch = Arc::new(Mutex::new(hub));
        let lines = self.chunk(i).unwrap_or_default();
        let played = lines.partition_point(|(t, _)| *t < at || (with_at && *t == at));
        let changes_map = |line: &Line| matches!(line, Line::Status(Status::Changelevel | Status::MapChange));
        for (_, line) in lines[..played].iter().filter(|(_, line)| !changes_map(line)) {
            publish(&scratch, line);
        }
        let hub = lock(&scratch).checkpoint();
        (hub, i, played)
    }
}
