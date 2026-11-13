//! A match's parts: each map load of it that showed something, one after another in a recording.
//! A part begins at its load's first snapshot (see [`Loads`]); the first of a recording at its
//! first line. What comes between a part's last snapshot and the next part's first (the server
//! changing map, the next map's server and world lines, its signon again and again) is the part's
//! tail.

use super::{Inner, Mark, Recording};
use crate::lock;
use crate::matches::loads::Loads;
use serde::{Deserialize, Serialize};
use stvtf::Line;
use stvtf::output::Status;

/// Seconds of ticks a part plays before it has play (a frozen baseline frame has none), and
/// that it goes on past the server's word that it is changing map before that word is taken back.
const READY_S: f64 = 1.0;
const WENT_ON_S: f64 = 5.0;

/// A map load of the match.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Part {
    /// Its load's number in the match, dropped ones too.
    pub(crate) load: usize,
    /// Empty until a line says which map it is (a recording with no server line).
    pub(crate) map: String,
    /// Play time of its first snapshot (the recording's first line for its first part), and of the
    /// first line kept of it.
    pub(crate) origin: u64,
    pub(crate) start: u64,
    /// Play time of its last line that is not tail: where it ends for those watching it. For the
    /// latest part not `changing`: the recording's length.
    pub(crate) last: u64,
    /// Play time of the next part's first snapshot; None for the latest.
    pub(crate) end: Option<u64>,
    /// The server said it is changing map, at this play time, and the part's ticks have not gone
    /// on since.
    pub(crate) changing: Option<u64>,
    /// Play time of the first `server` line after `changing`: from there on its lines are tail.
    pub(crate) tail: Option<u64>,
    /// It has play: a snapshot at least a second of ticks past its first.
    pub(crate) ready: bool,
    /// The tick of its first snapshot.
    pub(crate) first_tick: Option<u32>,
}

impl Part {
    /// The server is changing map from it, or did.
    pub(crate) fn over(&self) -> bool {
        self.changing.is_some() || self.end.is_some()
    }
}

/// What a recording keeps of a part as it goes.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct PartState {
    #[serde(rename = "n")]
    load: usize,
    map: String,
    origin: u64,
    last: u64,
    changing: Option<u64>,
    /// The tick it had at `changing`.
    changing_tick: Option<u32>,
    tail: Option<u64>,
    ready: bool,
    first_tick: Option<u32>,
    /// Play time of its first snapshot: a seek there has it.
    first_at: Option<u64>,
    /// Play time and tick of the first snapshot of the run of ticks it is in, and of the last
    /// snapshot: when it turns ready, it begins at that run (not at a frozen frame before it).
    run: Option<(u64, u32)>,
    snap: Option<(u64, u32)>,
}

impl PartState {
    /// A snapshot at play time `at`, `tick`: a run of ticks breaks where they jump more than a
    /// second, or play time runs more than a second past them (a frozen frame, then the break).
    fn run(&mut self, at: u64, tick: u32, interval: f64) {
        let broke = self.snap.is_none_or(|(was_at, was)| {
            let ms = f64::from(tick.saturating_sub(was)) * interval * 1000.0;
            tick.saturating_sub(was) > ticks(READY_S, interval) || at.saturating_sub(was_at) as f64 > ms + READY_S * 1000.0
        });
        if broke || self.run.is_none() {
            self.run = Some((at, tick));
        }
        self.snap = Some((at, tick));
    }

    /// A later map turns ready: it begins at its run of play; its frozen first frame and the break
    /// after it go to the map before (its clock starts at play).
    fn begin_at_play(&mut self) {
        if let Some((at, _)) = self.run.filter(|(at, _)| *at > self.origin) {
            (self.origin, self.first_at) = (at, Some(at));
        }
    }
}

/// The parts as they are at a chunk's start, for a file that begins there.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct Seed {
    loads: Loads,
    part: Option<PartState>,
}

impl Seed {
    /// The first load is `n`, if none began (see [`Loads::first_is`]).
    pub(super) fn first_load_is(&mut self, n: usize) {
        self.loads.first_is(n);
    }
}

/// Seconds `s` in ticks of `interval` seconds.
fn ticks(s: f64, interval: f64) -> u32 {
    (s / interval).round() as u32
}

/// Whether `line` is play of the part it is in, not tail: a snapshot or an event.
fn is_play(line: &Line) -> bool {
    match line {
        Line::Status(_) | Line::Server(_) | Line::World(_) => false,
        Line::Voice(v) => !v.injected,
        _ => true,
    }
}

impl Inner {
    /// Whether `line` would start a part: a snapshot of a new load, once the latest part showed
    /// something (the first part of all begins at the first line).
    pub(super) fn starts_part(&self, line: &Line) -> bool {
        let snapped = self.parts.last().is_some_and(|p| p.first_tick.is_some());
        matches!(line, Line::Snapshot(s) if snapped && self.loads.starts(s.tick))
    }

    /// Notes `line`, at play time `at`, in the parts.
    pub(super) fn note_line(&mut self, at: u64, line: &Line) {
        if self.parts.is_empty() {
            let map = lock(&self.scratch).map.clone().unwrap_or_default();
            self.parts.push(PartState { load: self.loads.latest(), map, origin: at, last: at, ..PartState::default() });
        }
        match line {
            Line::Server(server) => self.on_server(at, &server.map),
            Line::Status(Status::Changelevel | Status::MapChange) => self.on_changelevel(at),
            Line::Snapshot(snapshot) => self.on_snapshot(at, snapshot.tick),
            _ => {}
        }
        let latest = self.parts.last_mut().expect("a part");
        if is_play(line) && latest.tail.is_none() {
            latest.last = at;
        }
    }

    /// A server line naming `map`: the tail of a part the server is changing from begins. A part
    /// with no map yet (a recording begun mid-stream) is of `map`: no new load.
    fn on_server(&mut self, at: u64, map: &str) {
        let latest = self.parts.last_mut().expect("a part");
        if latest.changing.is_some() {
            latest.tail.get_or_insert(at);
        }
        if latest.first_tick.is_some() && latest.map.is_empty() {
            self.loads = Loads::from(self.loads.latest(), Some(map.to_string()), self.loads.last_tick());
        }
        if latest.first_tick.is_none() || latest.map.is_empty() {
            latest.map = map.to_string();
        }
        self.loads.server(map);
    }

    /// The server says it is changing map: a part with play is over, unless it goes on.
    fn on_changelevel(&mut self, at: u64) {
        let tick = self.loads.last_tick();
        let latest = self.parts.last_mut().expect("a part");
        if latest.ready && latest.changing.is_none() {
            (latest.changing, latest.changing_tick) = (Some(at), tick);
        }
    }

    /// A snapshot at `tick`: a new part if its load starts here, else play of the latest.
    fn on_snapshot(&mut self, at: u64, tick: u32) {
        let interval = lock(&self.scratch).tick_interval();
        let snapped = self.parts.last().is_some_and(|p| p.first_tick.is_some());
        let (n, starts) = self.loads.snapshot(tick);
        let map = self.loads.map().map(str::to_string);
        if starts && snapped {
            let map = map.unwrap_or_default();
            let (first_tick, first_at, run) = (Some(tick), Some(at), Some((at, tick)));
            self.parts.push(PartState { load: n, map, origin: at, last: at, first_tick, first_at, run, snap: run, ..PartState::default() });
            self.scores = None;
            return;
        }
        let later = self.parts.len() > 1;
        let latest = self.parts.last_mut().expect("a part");
        if starts {
            (latest.load, latest.first_tick, latest.first_at) = (n, Some(tick), Some(at));
            (latest.run, latest.snap) = (Some((at, tick)), Some((at, tick)));
            latest.map = map.unwrap_or_else(|| latest.map.clone());
            return;
        }
        latest.run(at, tick, interval);
        let ready = latest.first_tick.is_some_and(|first| tick >= first.saturating_add(ticks(READY_S, interval)));
        if ready && !latest.ready && later {
            latest.begin_at_play();
        }
        latest.ready |= ready;
        let went_on = latest.changing_tick.is_some_and(|before| tick > before.saturating_add(ticks(WENT_ON_S, interval)));
        // Its ticks went on, whatever was announced meanwhile (a reconnect): it was not over.
        if went_on {
            (latest.changing, latest.changing_tick, latest.tail) = (None, None, None);
        }
    }

    /// What a chunk starting now notes of the parts.
    pub(super) fn seed(&self) -> Seed {
        Seed { loads: self.loads.clone(), part: self.parts.last().cloned() }
    }

    /// Whether play time `at` is a part's first snapshot: a seek there has it.
    pub(super) fn first_snapshot_at(&self, at: u64) -> bool {
        self.parts.iter().any(|p| p.first_at == Some(at))
    }

    /// Play time of the first chunk kept.
    fn first_kept(&self) -> u64 {
        self.chunks.first().map_or(self.start, |c| c.start)
    }

    /// How many parts at the front have nothing left: their last line is before the first chunk
    /// kept. The latest never.
    fn gone(&self) -> usize {
        let first = self.first_kept();
        self.parts.iter().take(self.parts.len().saturating_sub(1)).take_while(|p| p.last < first).count()
    }

    /// Drops the parts with nothing left.
    pub(super) fn drop_parts(&mut self) {
        self.parts.drain(..self.gone());
    }

    /// The parts kept, oldest first.
    fn part_list(&self) -> Vec<Part> {
        let first = self.first_kept();
        let kept = &self.parts[self.gone()..];
        let ends = kept.iter().skip(1).map(|p| Some(p.origin)).chain([None]);
        let list = kept.iter().zip(ends).map(|(p, end)| {
            let last = if end.is_none() && p.changing.is_none() { self.clock.play } else { p.last };
            let (n, map, origin, start) = (p.load, p.map.clone(), p.origin, p.origin.max(first));
            let (changing, tail, ready, first_tick) = (p.changing, p.tail, p.ready, p.first_tick);
            Part { load: n, map, origin, start, last, end, changing, tail, ready, first_tick }
        });
        list.collect()
    }

    fn latest_part(&self) -> Part {
        self.part_list().pop().unwrap_or(Part {
            load: self.loads.latest(),
            map: String::new(),
            origin: 0,
            start: 0,
            last: 0,
            end: None,
            changing: None,
            tail: None,
            ready: false,
            first_tick: None,
        })
    }

    /// Play times part `n` holds, from where it begins to where it ends, if kept: a part's tail is
    /// the next part's (the next map's round can start before its first snapshot), and is no
    /// part's till that exists.
    fn span(&self, n: usize) -> Option<(u64, Option<u64>)> {
        let parts = self.part_list();
        let i = parts.iter().position(|p| p.load == n)?;
        let from = i.checked_sub(1).and_then(|before| parts[before].tail).unwrap_or(parts[i].origin);
        Some((from, parts[i].tail.or(parts[i].end)))
    }

    fn rounds_of(&self, n: usize) -> Vec<u64> {
        let Some((from, to)) = self.span(n) else { return Vec::new() };
        self.rounds.iter().copied().filter(|t| *t >= from && to.is_none_or(|to| *t < to)).collect()
    }
}

/// The parts' header lines of a file, as read.
#[derive(Default)]
pub(super) struct Header {
    origin: Option<u64>,
    parts: usize,
    seed: Option<Seed>,
}

impl Header {
    /// Reads `key`'s `value`, if it is one of the parts'.
    pub(super) fn read(&mut self, key: &str, value: &str) -> anyhow::Result<bool> {
        match key {
            "origin" => self.origin = Some(value.parse()?),
            "parts" => self.parts = value.parse()?,
            "seed" => self.seed = Some(serde_json::from_str(value)?),
            // Round 1's loads of each map: the rule needs none.
            "loads" => {}
            _ => return Ok(false),
        }
        Ok(true)
    }
}

impl Inner {
    /// The file's header lines of the parts: where the first kept began, how many are dropped,
    /// and the parts as they were at the first chunk, which the file's lines go on from.
    pub(super) fn parts_header(&self) -> anyhow::Result<String> {
        let first = self.part_list().into_iter().next();
        let (origin, n) = first.map_or((self.start, self.loads.latest()), |p| (p.origin, p.load));
        let seed = self.chunks.first().map(|c| &c.seed);
        Ok(format!("origin\t{origin}\nparts\t{n}\nseed\t{}\n", serde_json::to_string(&seed)?))
    }

    /// The parts as a file's `header` says they were at its first line, at play time `at`. A file
    /// with no seed goes on from the map its hub holds.
    pub(super) fn seed_from(&mut self, header: Header, at: u64) {
        if let Some(Seed { loads, part }) = header.seed {
            self.loads = loads;
            self.parts.extend(part);
            return;
        }
        let (map, tick) = {
            let h = lock(&self.scratch);
            (h.map.clone(), h.tick())
        };
        let n = header.parts;
        let origin = header.origin.unwrap_or(at);
        let part = PartState {
            load: n,
            map: map.clone().unwrap_or_default(),
            origin,
            last: origin,
            ready: tick.is_some(),
            first_tick: tick,
            ..PartState::default()
        };
        if tick.is_some() {
            self.loads = Loads::from(n, map, tick);
        } else if let Some(map) = map {
            // No snapshot yet: its first starts the load.
            self.loads.server(&map);
        }
        self.parts.push(part);
    }
}

impl Recording {
    /// The latest part; nameless before a server line (a recording with none yet).
    pub(crate) fn latest(&self) -> Part {
        lock(&self.inner).latest_part()
    }

    /// The newest part that has play; the earliest kept if none has: what a room opened now is on.
    pub(crate) fn watchable(&self) -> Part {
        let inner = lock(&self.inner);
        let parts = inner.part_list();
        parts.iter().rfind(|p| p.ready).or(parts.first()).cloned().unwrap_or_else(|| inner.latest_part())
    }

    /// The parts kept, oldest first.
    pub(crate) fn parts(&self) -> Vec<Part> {
        lock(&self.inner).part_list()
    }

    /// Part `n`; none once dropped.
    pub(crate) fn part(&self, n: usize) -> Option<Part> {
        self.parts().into_iter().find(|p| p.load == n)
    }

    /// Play times of the rounds' starts in part `n` (see `span`).
    pub(crate) fn rounds_in(&self, n: usize) -> Vec<u64> {
        lock(&self.inner).rounds_of(n)
    }

    /// Marks of part `n` (see `span`) from the `from`th of the match on, and how many the match
    /// has in all.
    pub(crate) fn marks_in(&self, n: usize, from: usize) -> (Vec<Mark>, usize) {
        let inner = lock(&self.inner);
        let all = inner.marks_dropped + inner.marks.len();
        let Some((start, to)) = inner.span(n) else { return (Vec::new(), all) };
        let holds = |at: u64| at >= start && to.is_none_or(|to| at < to);
        let marks = inner.marks.iter().enumerate().filter(|(i, m)| inner.marks_dropped + i >= from && holds(m.at));
        (marks.map(|(_, m)| m.clone()).collect(), all)
    }
}

#[cfg(test)]
mod file_tests;
#[cfg(test)]
pub(crate) mod real_tests;
#[cfg(test)]
pub(crate) mod tests;
