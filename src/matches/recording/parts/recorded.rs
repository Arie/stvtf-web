//! A recording's parts, as it records and as it is asked for them.

use stvtf::Line;

use super::super::lines::Inner;
use super::super::{Mark, Recording};
use super::Part;
use crate::sync::lock;

impl Inner {
    /// Play time of the first chunk kept.
    pub(super) fn first_kept(&self) -> u64 {
        self.chunks.first().map_or(self.start, |c| c.start)
    }

    /// Notes `line`, at play time `at`, in the parts: a new one has no score yet.
    pub(in crate::matches::recording) fn note_part_line(&mut self, at: u64, line: &Line) {
        if self.parts.note_line(at, line, &self.scratch) {
            self.scores = None;
        }
    }

    /// Drops the parts with nothing left.
    pub(in crate::matches::recording) fn drop_parts(&mut self) {
        let first = self.first_kept();
        self.parts.drop_before(first);
    }

    /// The parts kept, oldest first.
    pub(super) fn part_list(&self) -> Vec<Part> {
        self.parts.kept(self.first_kept(), self.clock.play)
    }

    pub(super) fn latest_part(&self) -> Part {
        self.part_list().pop().unwrap_or_else(|| self.parts.none_yet())
    }

    /// Play times part `n` holds, from where it begins to where it ends, if kept: a part's tail is
    /// the next part's (the next map's round can start before its first snapshot), and is no
    /// part's till that exists.
    pub(super) fn span(&self, n: usize) -> Option<(u64, Option<u64>)> {
        let parts = self.part_list();
        let i = parts.iter().position(|p| p.load == n)?;
        let from = i.checked_sub(1).and_then(|before| parts[before].tail).unwrap_or(parts[i].origin);
        Some((from, parts[i].tail.or(parts[i].end)))
    }

    pub(super) fn rounds_of(&self, n: usize) -> Vec<u64> {
        let Some((from, to)) = self.span(n) else { return Vec::new() };
        self.rounds.iter().copied().filter(|t| *t >= from && to.is_none_or(|to| *t < to)).collect()
    }
}

impl Inner {
    /// The file's header lines of the parts: where the first kept began, how many are dropped,
    /// and the parts as they were at the first chunk, which the file's lines go on from.
    pub(in crate::matches::recording) fn parts_header(&self) -> anyhow::Result<String> {
        let first = self.part_list().into_iter().next();
        let (origin, n) = first.map_or((self.start, self.parts.loads.latest()), |p| (p.origin, p.load));
        let seed = self.chunks.first().map(|c| &c.seed);
        Ok(format!("origin\t{origin}\nparts\t{n}\nseed\t{}\n", serde_json::to_string(&seed)?))
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
