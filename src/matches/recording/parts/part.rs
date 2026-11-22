//! The parts of a recording: each load of a map, from its first snapshot on.

use serde::{Deserialize, Serialize};
use stvtf::Line;
use stvtf::output::Status;

use crate::matches::hub::SharedHub;
use crate::matches::loads::Loads;
use crate::sync::lock;

/// Seconds of ticks past its first snapshot before a part counts as having play (a frozen
/// baseline frame has none), and seconds of ticks a part must go on after the server said it is
/// changing map before that is taken back.
pub(super) const READY_S: f64 = 1.0;

pub(super) const WENT_ON_S: f64 = 5.0;

/// A map load of the match.
#[derive(Clone, Debug, PartialEq)]
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
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(in crate::matches::recording) struct PartState {
    #[serde(rename = "n")]
    pub(super) load: usize,
    pub(super) map: String,
    pub(super) origin: u64,
    pub(super) last: u64,
    pub(super) changing: Option<u64>,
    /// The tick it had at `changing`.
    pub(super) changing_tick: Option<u32>,
    pub(super) tail: Option<u64>,
    pub(super) ready: bool,
    pub(super) first_tick: Option<u32>,
    /// Play time of its first snapshot: a seek there includes it.
    pub(super) first_at: Option<u64>,
    /// Play time and tick of the first snapshot of the run of ticks it is in, and of the last
    /// snapshot: when it turns ready, it begins at that run (not at a frozen frame before it).
    pub(super) run: Option<(u64, u32)>,
    pub(super) snap: Option<(u64, u32)>,
}

impl PartState {
    /// A snapshot at play time `at`, `tick`: a run of ticks breaks where they jump more than a
    /// second, or play time runs more than a second past them (a frozen frame, then the break).
    pub(super) fn run(&mut self, at: u64, tick: u32, interval: f64) {
        let broke = self.snap.is_none_or(|(was_at, was)| {
            let ms = f64::from(tick.saturating_sub(was)) * interval * 1000.0;
            tick.saturating_sub(was) > ticks(READY_S, interval) || at.saturating_sub(was_at) as f64 > ms + READY_S * 1000.0
        });
        if broke || self.run.is_none() {
            self.run = Some((at, tick));
        }
        self.snap = Some((at, tick));
    }

    /// A part beginning at the snapshot at play time `at`, `tick`, of load `load` of `map`.
    pub(super) fn begun(load: usize, map: String, at: u64, tick: u32) -> PartState {
        let run = Some((at, tick));
        PartState { load, map, origin: at, last: at, first_tick: Some(tick), first_at: Some(at), run, snap: run, ..PartState::default() }
    }

    /// The first part of a recording begins load `load` at the snapshot at `at`, `tick` (it
    /// began at the recording's first line); of `map`, if known.
    pub(super) fn begin_load(&mut self, load: usize, at: u64, tick: u32, map: Option<String>) {
        (self.load, self.first_tick, self.first_at) = (load, Some(tick), Some(at));
        (self.run, self.snap) = (Some((at, tick)), Some((at, tick)));
        if let Some(map) = map {
            self.map = map;
        }
    }

    /// A snapshot of its play at `at`, `tick`: it turns ready a second of ticks past its first (a
    /// `later` part then begins at its run of play). If the server said it was changing map but
    /// the ticks go on, the part was not over.
    pub(super) fn play(&mut self, at: u64, tick: u32, interval: f64, later: bool) {
        self.run(at, tick, interval);
        let ready = self.first_tick.is_some_and(|first| tick >= first.saturating_add(ticks(READY_S, interval)));
        if ready && !self.ready && later {
            self.begin_at_play();
        }
        self.ready |= ready;
        let went_on = self.changing_tick.is_some_and(|before| tick > before.saturating_add(ticks(WENT_ON_S, interval)));
        // Its ticks went on, whatever was announced meanwhile (a reconnect): it was not over.
        if went_on {
            (self.changing, self.changing_tick, self.tail) = (None, None, None);
        }
    }

    /// A later map turns ready: it begins at its run of play; its frozen first frame and the break
    /// after it go to the map before (its clock starts at play).
    pub(super) fn begin_at_play(&mut self) {
        if let Some((at, _)) = self.run.filter(|(at, _)| *at > self.origin) {
            (self.origin, self.first_at) = (at, Some(at));
        }
    }
}

/// The parts as they are at a chunk's start, for a file that begins there.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(in crate::matches::recording) struct Seed {
    pub(super) loads: Loads,
    pub(super) part: Option<PartState>,
}

impl Seed {
    /// The first load is `n`, if none began (see [`Loads::first_is`]).
    pub(in crate::matches::recording) fn first_load_is(&mut self, n: usize) {
        self.loads.first_is(n);
    }
}

/// Seconds `s` in ticks of `interval` seconds.
pub(super) fn ticks(s: f64, interval: f64) -> u32 {
    (s / interval).round() as u32
}

/// The latest part: there is one once a recording noted a line.
pub(super) fn latest(parts: &mut [PartState]) -> &mut PartState {
    parts.last_mut().expect("internal error: no part though a line came")
}

/// Whether `line` is play of the part it is in, not tail: a snapshot or an event.
pub(super) fn is_play(line: &Line) -> bool {
    match line {
        Line::Status(_) | Line::Server(_) | Line::World(_) => false,
        Line::Voice(v) => !v.injected,
        _ => true,
    }
}

/// A recording's parts, and the rule its loads follow.
#[derive(Debug, Default)]
pub(in crate::matches::recording) struct Parts {
    pub(in crate::matches::recording) loads: Loads,
    /// Oldest first; the first from the recording's first line on.
    pub(super) list: Vec<PartState>,
}

impl Parts {
    /// Whether `line` would start a part: a snapshot of a new load, once the latest part showed
    /// something (the first part of all begins at the first line).
    pub(in crate::matches::recording) fn starts_part(&self, line: &Line) -> bool {
        let snapped = self.list.last().is_some_and(|p| p.first_tick.is_some());
        matches!(line, Line::Snapshot(s) if snapped && self.loads.starts(s.tick))
    }

    /// Notes `line`, at play time `at`, with the match as `scratch` holds it. Whether a part
    /// began.
    pub(in crate::matches::recording) fn note_line(&mut self, at: u64, line: &Line, scratch: &SharedHub) -> bool {
        if self.list.is_empty() {
            let map = lock(scratch).map.clone().unwrap_or_default();
            self.list.push(PartState { load: self.loads.latest(), map, origin: at, last: at, ..PartState::default() });
        }
        let began = match line {
            Line::Server(server) => {
                self.on_server(at, &server.map);
                false
            }
            Line::Status(Status::Changelevel | Status::MapChange) => {
                self.on_changelevel(at);
                false
            }
            Line::Snapshot(snapshot) => self.on_snapshot(at, snapshot.tick, lock(scratch).tick_interval()),
            _ => false,
        };
        let latest = latest(&mut self.list);
        if is_play(line) && latest.tail.is_none() {
            latest.last = at;
        }
        began
    }

    /// A server line naming `map`: the tail of a part the server is changing from begins. A part
    /// with no map yet (a recording begun mid-stream) is of `map`: no new load.
    pub(super) fn on_server(&mut self, at: u64, map: &str) {
        let latest = latest(&mut self.list);
        if latest.changing.is_some() {
            latest.tail.get_or_insert(at);
        }
        if latest.first_tick.is_some() && latest.map.is_empty() {
            self.loads = Loads::resumed(self.loads.latest(), Some(map.to_string()), self.loads.last_tick());
        }
        if latest.first_tick.is_none() || latest.map.is_empty() {
            latest.map = map.to_string();
        }
        self.loads.server(map);
    }

    /// The server says it is changing map: a part with play is over, unless it goes on.
    pub(super) fn on_changelevel(&mut self, at: u64) {
        let tick = self.loads.last_tick();
        let latest = latest(&mut self.list);
        if latest.ready && latest.changing.is_none() {
            (latest.changing, latest.changing_tick) = (Some(at), tick);
        }
    }

    /// A snapshot at `tick`, of a server ticking every `interval` seconds: a new part if its load
    /// starts here (true), else play of the latest.
    pub(super) fn on_snapshot(&mut self, at: u64, tick: u32, interval: f64) -> bool {
        let snapped = self.list.last().is_some_and(|p| p.first_tick.is_some());
        let (n, starts) = self.loads.snapshot(tick);
        let map = self.loads.map().map(str::to_string);
        if starts && snapped {
            self.list.push(PartState::begun(n, map.unwrap_or_default(), at, tick));
            return true;
        }
        if starts {
            latest(&mut self.list).begin_load(n, at, tick, map);
        } else {
            let later = self.list.len() > 1;
            latest(&mut self.list).play(at, tick, interval, later);
        }
        false
    }

    /// What a chunk starting now notes of the parts.
    pub(in crate::matches::recording) fn seed(&self) -> Seed {
        Seed { loads: self.loads.clone(), part: self.list.last().cloned() }
    }

    /// Whether play time `at` is a part's first snapshot: a seek there includes it.
    pub(in crate::matches::recording) fn first_snapshot_at(&self, at: u64) -> bool {
        self.list.iter().any(|p| p.first_at == Some(at))
    }

    /// How many parts at the front have nothing left: their last line is before `first_kept`, the
    /// play time of the first chunk kept. The latest never.
    pub(super) fn gone(&self, first_kept: u64) -> usize {
        self.list.iter().take(self.list.len().saturating_sub(1)).take_while(|p| p.last < first_kept).count()
    }

    /// Drops the parts with nothing left from `first_kept` on.
    pub(in crate::matches::recording) fn drop_before(&mut self, first_kept: u64) {
        self.list.drain(..self.gone(first_kept));
    }

    /// The parts kept from `first_kept` on, oldest first, the latest going as far as `play`.
    pub(super) fn kept(&self, first_kept: u64, play: u64) -> Vec<Part> {
        let kept = &self.list[self.gone(first_kept)..];
        let ends = kept.iter().skip(1).map(|p| Some(p.origin)).chain([None]);
        let list = kept.iter().zip(ends).map(|(p, end)| {
            let last = if end.is_none() && p.changing.is_none() { play } else { p.last };
            let (load, map, origin, start) = (p.load, p.map.clone(), p.origin, p.origin.max(first_kept));
            let (changing, tail, ready, first_tick) = (p.changing, p.tail, p.ready, p.first_tick);
            Part { load, map, origin, start, last, end, changing, tail, ready, first_tick }
        });
        list.collect()
    }

    /// A part of the latest load, with nothing of it known yet.
    pub(super) fn none_yet(&self) -> Part {
        Part {
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
        }
    }

    /// The parts as a file's `header` says they were at its first line, at play time `at`. A file
    /// with no seed goes on from the map `scratch` holds.
    pub(in crate::matches::recording) fn seed_from(&mut self, header: Header, at: u64, scratch: &SharedHub) {
        if let Some(Seed { loads, part }) = header.seed {
            self.loads = loads;
            self.list.extend(part);
            return;
        }
        let (map, tick) = {
            let h = lock(scratch);
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
            self.loads = Loads::resumed(n, map, tick);
        } else if let Some(map) = map {
            // No snapshot yet: its first starts the load.
            self.loads.server(&map);
        }
        self.list.push(part);
    }
}

/// The parts' header lines of a file, as read.
#[derive(Default)]
pub(in crate::matches::recording) struct Header {
    pub(super) origin: Option<u64>,
    pub(super) parts: usize,
    pub(super) seed: Option<Seed>,
}

impl Header {
    /// Whether `key` is the key of a header line, one of the parts' or any other.
    pub(in crate::matches::recording) fn has(key: &str) -> bool {
        matches!(key, "marks" | "hub" | "origin" | "parts" | "seed" | "loads")
    }

    /// Reads `key`'s `value`, if it is one of the parts'.
    pub(in crate::matches::recording) fn read(&mut self, key: &str, value: &str) -> anyhow::Result<bool> {
        match key {
            "origin" => self.origin = Some(value.parse()?),
            "parts" => self.parts = value.parse()?,
            "seed" => self.seed = Some(serde_json::from_str(value)?),
            // An older version's count of loads per map: the rule needs none.
            "loads" => {}
            _ => return Ok(false),
        }
        Ok(true)
    }
}
