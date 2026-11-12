//! A match's parts: each map load of it, one after another in a recording.

use super::{Inner, Mark, Recording};
use crate::lock;
use std::fmt::Write as _;
use stvtf::Line;

/// A map load of the match.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Part {
    /// Its number in the match, counting from 0, dropped ones too.
    pub(crate) n: usize,
    pub(crate) map: String,
    /// How many earlier parts of the match (dropped ones too) were of this map.
    pub(crate) again: usize,
    /// Play time of its first line, and of the first line kept of it.
    pub(crate) origin: u64,
    pub(crate) start: u64,
    /// Play time of the next part's first line; None for the latest.
    pub(crate) end: Option<u64>,
    /// A snapshot of it came: it has play to show.
    pub(crate) ready: bool,
}

impl Part {
    /// Whether play time `at` is in it.
    fn holds(&self, at: u64) -> bool {
        at >= self.origin && self.end.is_none_or(|end| at < end)
    }
}

/// What a recording notes of a part as it starts.
pub(super) struct PartStart {
    map: String,
    again: usize,
    origin: u64,
    ready: bool,
}

impl Inner {
    /// Notes `line`, at play time `at`, in the parts; true if it starts another part than the
    /// first. A server line naming another map does, and so does a snapshot whose tick is lower
    /// than the last one's (the server's ticks started over on the same map).
    pub(super) fn begin_part(&mut self, at: u64, line: &Line) -> bool {
        let known = lock(&self.scratch).map.clone();
        let named = if let Line::Server(server) = line { Some(server.map.as_str()) } else { None };
        let first = self.parts.is_empty();
        let later = !first
            && (named.zip(known.as_deref()).is_some_and(|(named, known)| named != known)
                || matches!(line, Line::Snapshot(s) if self.last_tick.is_some_and(|tick| s.tick < tick)));
        if first || later {
            let map = named.map(str::to_string).or(known).unwrap_or_default();
            let again = self.load(&map);
            self.parts.push(PartStart { map, again, origin: at, ready: false });
            self.scores = None;
            self.last_tick = None;
        } else if let Some(map) = named
            && self.parts.last().is_some_and(|p| p.map.is_empty())
        {
            // The first line was not a server line: this one names its map.
            let again = self.load(map);
            if let Some(part) = self.parts.last_mut() {
                part.map = map.to_string();
                part.again = again;
            }
        }
        if let Line::Snapshot(snapshot) = line {
            self.last_tick = Some(snapshot.tick);
            if let Some(part) = self.parts.last_mut() {
                part.ready = true;
            }
        }
        later
    }

    /// Counts a load of `map`; how many there were before.
    fn load(&mut self, map: &str) -> usize {
        if map.is_empty() {
            return 0;
        }
        let loads = self.names.entry(map.to_string()).or_default();
        *loads += 1;
        *loads - 1
    }

    /// Drops the parts whose chunks are all gone, the latest never.
    pub(super) fn drop_parts(&mut self) {
        let first = self.chunks.first().map_or(0, |c| c.start);
        let gone = self.parts.windows(2).take_while(|w| w[1].origin <= first).count();
        self.parts.drain(..gone);
        self.parts_dropped += gone;
    }

    /// The file's header lines for the parts: how many are dropped, and the loads dropped of
    /// each map, so numbers and counts go on after a restart.
    pub(super) fn parts_header(&self) -> String {
        let mut out = format!("parts\t{}\n", self.parts_dropped);
        let mut loads: Vec<_> = self.names.iter().map(|(map, n)| (map, n.saturating_sub(self.parts.iter().filter(|p| p.map == *map).count()))).collect();
        loads.sort();
        for (map, n) in loads.into_iter().filter(|(_, n)| *n > 0) {
            writeln!(out, "loads\t{map}\t{n}").expect("a string takes it");
        }
        out
    }

    /// Reads a header line of the parts; a file without them is one part.
    pub(super) fn read_header(&mut self, key: &str, value: &str) -> anyhow::Result<()> {
        if key == "parts" {
            self.parts_dropped = value.parse()?;
        } else {
            let (map, n) = value.rsplit_once('\t').ok_or_else(|| anyhow::anyhow!("not a recording"))?;
            self.names.insert(map.to_string(), n.parse()?);
        }
        Ok(())
    }

    /// The parts kept, oldest first.
    fn part_list(&self) -> Vec<Part> {
        let ends = self.parts.iter().skip(1).map(|p| Some(p.origin)).chain([None]);
        let list = self.parts.iter().zip(ends).enumerate().filter_map(|(i, (p, end))| {
            let first = self.chunks.partition_point(|c| c.start < p.origin);
            let start = self.chunks.get(first)?.start;
            let part = Part { n: self.parts_dropped + i, map: p.map.clone(), again: p.again, origin: p.origin, start, end, ready: p.ready };
            end.is_none_or(|end| start < end).then_some(part)
        });
        list.collect()
    }

    fn latest_part(&self) -> Part {
        let nameless = Part { n: 0, map: String::new(), again: 0, origin: 0, start: 0, end: None, ready: false };
        self.part_list().pop().unwrap_or(nameless)
    }

    fn rounds_of(&self, part: &Part) -> Vec<u64> {
        self.rounds.iter().copied().filter(|t| part.holds(*t)).collect()
    }
}

#[allow(dead_code, reason = "the rooms use them once they show maps")]
impl Recording {
    /// The parts kept, oldest first.
    pub(crate) fn parts(&self) -> Vec<Part> {
        lock(&self.inner).part_list()
    }

    /// Part `n`; none once dropped.
    pub(crate) fn part(&self, n: usize) -> Option<Part> {
        self.parts().into_iter().find(|p| p.n == n)
    }

    /// The latest part; a recording with no server line yet has a nameless one.
    pub(crate) fn latest(&self) -> Part {
        lock(&self.inner).latest_part()
    }

    /// Play times of the rounds' starts in part `n`.
    pub(crate) fn rounds_in(&self, n: usize) -> Vec<u64> {
        let inner = lock(&self.inner);
        inner.part_list().iter().find(|p| p.n == n).map(|p| inner.rounds_of(p)).unwrap_or_default()
    }

    /// Play times of the rounds' starts in the latest part.
    pub(super) fn latest_rounds(&self) -> Vec<u64> {
        let inner = lock(&self.inner);
        inner.rounds_of(&inner.latest_part())
    }

    /// Marks of part `n` from the `from`th of the match on, and how many the match has in all.
    pub(crate) fn marks_in(&self, n: usize, from: usize) -> (Vec<Mark>, usize) {
        let inner = lock(&self.inner);
        let all = inner.marks_dropped + inner.marks.len();
        let Some(part) = inner.part_list().into_iter().find(|p| p.n == n) else { return (Vec::new(), all) };
        let marks = inner.marks.iter().enumerate().filter(|(i, m)| inner.marks_dropped + i >= from && part.holds(m.at));
        (marks.map(|(_, m)| m.clone()).collect(), all)
    }

    /// Play time of the latest part's first line.
    pub(super) fn latest_origin(&self) -> u64 {
        lock(&self.inner).latest_part().origin
    }
}

#[cfg(test)]
mod tests;
