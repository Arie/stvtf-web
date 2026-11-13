//! A match's parts: each map load of it, one after another in a recording.

use super::{Inner, Mark, Recording};
use crate::lock;
use std::collections::BTreeSet;
use std::fmt::Write as _;
use stvtf::Line;
use stvtf::output::Status;

/// A map load of the match.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Part {
    /// Its number in the match, counting from 0, dropped ones too.
    pub(crate) n: usize,
    /// Empty until a line says which map it is.
    pub(crate) map: String,
    /// How many earlier parts of the match (dropped ones too) were of this map.
    pub(crate) again: usize,
    /// Play time of its first line, and of the first line kept of it.
    pub(crate) origin: u64,
    pub(crate) start: u64,
    /// Play time it shows its map from (see `PartStart`), its first line kept if later.
    pub(crate) shown: u64,
    /// Play time of the next part's first line; None for the latest.
    pub(crate) end: Option<u64>,
    /// A snapshot of it came: it has play to show.
    pub(crate) ready: bool,
}

/// Whether `status` says the server is changing map: where a map ends. The live hub forgets the
/// map only at `map_change` (signon starting over), which can come without a `changelevel`.
fn changes_map(status: &Status) -> bool {
    matches!(status, Status::Changelevel | Status::MapChange)
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
    /// Play time it shows from: its first line, or for one the server said it is changing to, its
    /// first snapshot.
    shown: Option<u64>,
    /// It began after another part: till its first snapshot (`played`), the hub may hold the map
    /// before (its world, its snapshot, its tick).
    after: bool,
    played: Option<u64>,
    /// Named after the map before it, with that load's last tick: its first snapshot says whether
    /// its ticks go on from there (the same load) or started over (another).
    goes_on: Option<u32>,
}

impl Inner {
    /// Notes `line`, at play time `at`, in the parts; true if it starts another part than the
    /// first (see `ends_part`). A part the server is changing to has no name until its server
    /// line, or a snapshot, says which map it is.
    pub(super) fn begin_part(&mut self, at: u64, line: &Line) -> bool {
        let (known, tick) = {
            let h = lock(&self.scratch);
            (h.map.clone(), h.tick())
        };
        let named = if let Line::Server(server) = line { Some(server.map.as_str()) } else { None };
        let later = self.ends_part(line, named, known.as_deref());
        if self.parts.is_empty() || later {
            let changing = matches!(line, Line::Status(status) if changes_map(status));
            let map = named.map(str::to_string).or(known.filter(|_| !changing)).unwrap_or_default();
            let again = self.load(&map);
            let (shown, after) = ((!changing).then_some(at), !self.parts.is_empty());
            self.parts.push(PartStart { map, again, origin: at, ready: false, shown, after, played: None, goes_on: None });
            self.scores = None;
            self.last_tick = None;
        } else if !self.take_back(line, tick)
            && let Some(map) = named.map(str::to_string).or(known.clone().filter(|_| matches!(line, Line::Snapshot(_))))
            && self.parts.last().is_some_and(|p| p.map.is_empty())
        {
            let before = tick.filter(|_| known.as_deref() == Some(map.as_str()));
            self.name_part(map, before);
        }
        if let Line::Snapshot(snapshot) = line {
            self.first_snapshot(at, snapshot.tick);
        }
        later
    }

    /// A snapshot in a part after another that no line named yet, its ticks going on from the
    /// map before (`tick`): that map still plays, late behind the server's word that it changes,
    /// so the part never was. True if it is taken back.
    fn take_back(&mut self, line: &Line, tick: Option<u32>) -> bool {
        let Line::Snapshot(snapshot) = line else { return false };
        // The map before still kept: one trimmed away is not taken back to.
        let nameless = self.parts.len() > 1 && self.parts.last().is_some_and(|p| p.after && p.map.is_empty());
        let Some(before) = tick.filter(|before| nameless && snapshot.tick >= *before) else { return false };
        self.parts.pop();
        self.last_tick = Some(before);
        true
    }

    /// The latest part, nameless till now, is of `map`: another load of it, unless the map before
    /// was `map` too, which got to tick `before`; its first snapshot then says.
    fn name_part(&mut self, map: String, before: Option<u32>) {
        let last_load = self.names.get(&map).and_then(|n| n.checked_sub(1));
        let (again, goes_on) = match (before, last_load) {
            (Some(tick), Some(again)) => (again, Some(tick)),
            _ => (self.load(&map), None),
        };
        if let Some(part) = self.parts.last_mut() {
            (part.map, part.again, part.goes_on) = (map, again, goes_on);
        }
    }

    /// A snapshot at play time `at`, of `tick`: the latest part has play, and one named after the
    /// map before it is another load of that if its ticks started over.
    fn first_snapshot(&mut self, at: u64, tick: u32) {
        self.last_tick = Some(tick);
        // One after another with no name has nothing to show yet.
        let Some(part) = self.parts.last_mut().filter(|p| !(p.after && p.map.is_empty())) else { return };
        part.ready = true;
        part.shown.get_or_insert(at);
        part.played.get_or_insert(at);
        if part.goes_on.take().is_some_and(|before| tick < before) {
            let map = part.map.clone();
            let again = self.load(&map);
            if let Some(part) = self.parts.last_mut() {
                part.again = again;
            }
        }
    }

    /// Whether `line` ends the latest part: the server saying it is changing map, once the part
    /// has a name and play; a server line naming another map than `known`, when none said so
    /// first; a snapshot whose tick is lower than the last one's (the server's ticks started over).
    fn ends_part(&self, line: &Line, named: Option<&str>, known: Option<&str>) -> bool {
        let Some(latest) = self.parts.last() else { return false };
        let named_part = !latest.map.is_empty();
        match line {
            Line::Status(status) => changes_map(status) && named_part && latest.ready,
            Line::Server(_) => named_part && named.zip(known).is_some_and(|(named, known)| named != known),
            Line::Snapshot(s) => self.last_tick.is_some_and(|tick| s.tick < tick),
            _ => false,
        }
    }

    /// Where a trim may cut, from chunk `n` on: not inside a part that shows nothing yet (the
    /// server changing map), so a file resumes knowing what came before it; unless that would
    /// cut nothing (a part that never shows anything is trimmed too).
    pub(super) fn cut_at(&self, n: usize) -> usize {
        let Some(at) = self.chunks.get(n).map(|c| c.start) else { return n };
        match self.parts.iter().rfind(|p| p.origin <= at) {
            Some(part) if part.shown.is_none_or(|shown| shown > at) => {
                Some(self.chunks.partition_point(|c| c.start < part.origin)).filter(|back| *back > 0).unwrap_or(n)
            }
            _ => n,
        }
    }

    /// Whether play time `at` is the first snapshot of a part after another: a seek there has it.
    pub(super) fn first_snapshot_at(&self, at: u64) -> bool {
        self.parts.iter().any(|p| p.after && p.played == Some(at))
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

    /// The first part kept began at play time `origin`, before the first chunk kept maybe.
    pub(super) fn begin_at(&mut self, origin: u64) {
        if let Some(part) = self.parts.first_mut() {
            part.origin = origin;
        }
    }

    /// Play time the first part kept began at.
    pub(super) fn first_origin(&self) -> u64 {
        self.parts.first().map_or(self.start, |p| p.origin)
    }

    /// The file's header lines for the parts: how many are dropped, and the loads dropped of
    /// each map, so numbers and counts go on after a restart.
    pub(super) fn parts_header(&self) -> String {
        let mut out = format!("parts\t{}\n", self.parts_dropped);
        // Two parts can be one load: a map that went on through a map change.
        let kept = |map: &str| self.parts.iter().filter(|p| p.map == map).map(|p| p.again).collect::<BTreeSet<_>>().len();
        let mut loads: Vec<_> = self.names.iter().map(|(map, n)| (map, n.saturating_sub(kept(map)))).collect();
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
            // One after another shows from its first snapshot: not the map before.
            let shown = p.played.filter(|_| p.after).or(p.shown).unwrap_or(p.origin).max(start);
            let (n, map, again, origin, ready) = (self.parts_dropped + i, p.map.clone(), p.again, p.origin, p.ready);
            let part = Part { n, map, again, origin, start, shown, end, ready };
            end.is_none_or(|end| start < end).then_some(part)
        });
        list.collect()
    }

    fn latest_part(&self) -> Part {
        let nameless = Part { n: 0, map: String::new(), again: 0, origin: 0, start: 0, shown: 0, end: None, ready: false };
        self.part_list().pop().unwrap_or(nameless)
    }

    fn rounds_of(&self, part: &Part) -> Vec<u64> {
        self.rounds.iter().copied().filter(|t| part.holds(*t)).collect()
    }
}

impl Recording {
    /// The latest part; nameless before its server line (a recording with none yet, or the
    /// server changing map).
    pub(crate) fn latest(&self) -> Part {
        lock(&self.inner).latest_part()
    }

    /// Whether the match went on from `part` to another map: not when all after it is a map
    /// change that a recording which ended was cut at (stvtf output saved then), which is no map.
    pub(crate) fn went_on_from(&self, part: &Part) -> bool {
        let latest = self.latest();
        part.end.is_some() && (self.is_live() || latest.n != part.n + 1 || latest.ready || !latest.map.is_empty())
    }

    /// The latest part with play to show, or the latest if none has: what a room opened now is on.
    pub(crate) fn watchable(&self) -> Part {
        let inner = lock(&self.inner);
        inner.part_list().into_iter().rfind(|p| p.ready).unwrap_or_else(|| inner.latest_part())
    }
}

impl Recording {
    /// The parts kept, oldest first.
    pub(crate) fn parts(&self) -> Vec<Part> {
        lock(&self.inner).part_list()
    }

    /// Part `n`; none once dropped.
    pub(crate) fn part(&self, n: usize) -> Option<Part> {
        self.parts().into_iter().find(|p| p.n == n)
    }

    /// Play times of the rounds' starts in part `n`.
    pub(crate) fn rounds_in(&self, n: usize) -> Vec<u64> {
        let inner = lock(&self.inner);
        inner.part_list().iter().find(|p| p.n == n).map(|p| inner.rounds_of(p)).unwrap_or_default()
    }

    /// Marks of part `n` from the `from`th of the match on, and how many the match has in all.
    pub(crate) fn marks_in(&self, n: usize, from: usize) -> (Vec<Mark>, usize) {
        let inner = lock(&self.inner);
        let all = inner.marks_dropped + inner.marks.len();
        let Some(part) = inner.part_list().into_iter().find(|p| p.n == n) else { return (Vec::new(), all) };
        let marks = inner.marks.iter().enumerate().filter(|(i, m)| inner.marks_dropped + i >= from && part.holds(m.at));
        (marks.map(|(_, m)| m.clone()).collect(), all)
    }
}

#[cfg(test)]
mod gap_tests;
#[cfg(test)]
mod tests;
