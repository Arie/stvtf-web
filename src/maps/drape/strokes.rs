//! A stroke laid on the ground: stepped along, split where it leaves the floor.

use super::Ground;

/// The stroke is followed in steps this long, so lines follow the floor between its points.
pub(super) const STEP: f64 = 8.0;

/// A floor continues the line when this much higher or lower than it, plus a step's worth for
/// slopes (under 45°): over a gap too, so a line does not drop off a ledge onto the floor below.
pub(super) const STEP_HEIGHT: f64 = 18.0;

/// A line goes on over a gap in the floor up to this long.
pub(super) const MAX_GAP: f64 = 64.0;

/// Floors closer in height than this at a point are one (faces meeting, terrain over a brush).
pub(super) const SAME_FLOOR: f64 = 4.0;

/// Lines are drawn this far above the floor, clear of it.
pub(super) const LIFT: f64 = 8.0;

/// A line is made of as few straight pieces as keep it within this of its steps.
pub(super) const TOLERANCE: f64 = 6.0;

/// Lines shorter than this are left out: a stroke over a ledge or a crate's edge.
pub(super) const MIN_LINE: f64 = 32.0;

/// The most steps one piece of a stroke is followed in, whatever its points say.
pub(super) const MAX_STEPS: usize = 20_000;

/// The most lines a stroke has on the floors at once.
pub(super) const MAX_LINES: usize = 16;

/// A straight piece of a line in the world, and when its end was drawn (ms into the stroke).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Piece {
    pub(crate) from: [f64; 3],
    pub(crate) to: [f64; 3],
    pub(crate) ms: f64,
}

/// A line along one floor.
pub(super) struct Line {
    /// Where its last piece ended, and the steps since, not yet in a piece.
    pub(super) anchor: [f64; 3],
    pub(super) steps: Vec<([f64; 3], f64)>,
    /// How far the stroke has gone since this line was last on a floor.
    pub(super) gap: f64,
    /// How long the line is, and its pieces while it is too short to show.
    pub(super) length: f64,
    pub(super) held: Vec<Piece>,
}

impl Line {
    pub(super) fn new(at: [f64; 3]) -> Self {
        Line { anchor: at, steps: Vec::new(), gap: 0.0, length: 0.0, held: Vec::new() }
    }

    pub(super) fn last(&self) -> [f64; 3] {
        self.steps.last().map_or(self.anchor, |s| s.0)
    }

    /// On a floor again: `at`, drawn at `ms`. Pieces it makes go to `out`.
    pub(super) fn reach(&mut self, at: [f64; 3], ms: f64, out: &mut Vec<Piece>) {
        self.gap = 0.0;
        self.steps.push((at, ms));
        let a = self.anchor;
        if self.steps.iter().all(|(p, _)| off_line(a, at, *p) <= TOLERANCE) {
            return;
        }
        // Not straight any more: the steps up to the one before make a piece.
        let (end, end_ms) = self.steps[self.steps.len() - 2];
        self.piece(end, end_ms, out);
        self.steps.drain(..self.steps.len() - 1);
    }

    pub(super) fn piece(&mut self, to: [f64; 3], ms: f64, out: &mut Vec<Piece>) {
        let p = Piece { from: self.anchor, to, ms };
        self.anchor = to;
        self.length += dist(p.from, p.to);
        if self.length >= MIN_LINE {
            out.append(&mut self.held);
            out.push(p);
        } else {
            self.held.push(p);
        }
    }

    /// What is left of the line, as a piece.
    pub(super) fn flush(&mut self, out: &mut Vec<Piece>) {
        if let Some(&(end, ms)) = self.steps.last() {
            self.piece(end, ms, out);
            self.steps.clear();
        }
    }
}

/// A stroke being laid on the floors, a piece of it at a time.
pub(crate) struct Drape {
    pub(super) lines: Vec<Line>,
    /// The stroke's last point followed, and when it was drawn.
    pub(super) last: Option<([f64; 2], f64)>,
}

impl Drape {
    pub(crate) fn new() -> Self {
        Drape { lines: Vec::new(), last: None }
    }

    /// The next points of the stroke (`x, y, ms`): the new straight pieces of its lines.
    pub(crate) fn push(&mut self, ground: &Ground, points: &[[f64; 3]]) -> Vec<Piece> {
        let mut out = Vec::new();
        let mut steps_left = MAX_STEPS;
        for &[x, y, ms] in points {
            let Some((from, from_ms)) = self.last else {
                self.step(ground, [x, y], ms, 0.0, &mut out);
                self.last = Some(([x, y], ms));
                continue;
            };
            let d = (x - from[0]).hypot(y - from[1]);
            let n = ((d / STEP).ceil() as usize).clamp(1, steps_left.max(1));
            steps_left = steps_left.saturating_sub(n);
            for i in 1..=n {
                let t = i as f64 / n as f64;
                let at = [from[0] + (x - from[0]) * t, from[1] + (y - from[1]) * t];
                self.step(ground, at, from_ms + (ms - from_ms) * t, d / n as f64, &mut out);
            }
            self.last = Some(([x, y], ms));
        }
        for l in &mut self.lines {
            l.flush(&mut out);
        }
        out
    }

    /// One step of the stroke, `moved` from the last: each floor under it continues the nearest
    /// line in reach, or starts one.
    pub(super) fn step(&mut self, ground: &Ground, at: [f64; 2], ms: f64, moved: f64, out: &mut Vec<Piece>) {
        for l in &mut self.lines {
            l.gap += moved;
        }
        let mut taken = vec![false; self.lines.len()];
        for z in ground.heights(at[0], at[1]) {
            let p = [at[0], at[1], z + LIFT];
            let near = self
                .lines
                .iter()
                .enumerate()
                .filter(|(i, l)| !taken[*i] && (l.last()[2] - p[2]).abs() <= STEP_HEIGHT + STEP)
                .min_by(|a, b| (a.1.last()[2] - p[2]).abs().total_cmp(&(b.1.last()[2] - p[2]).abs()))
                .map(|(i, _)| i);
            match near {
                Some(i) => {
                    taken[i] = true;
                    self.lines[i].reach(p, ms, out);
                }
                None if self.lines.len() < MAX_LINES => {
                    self.lines.push(Line::new(p));
                    taken.push(true);
                }
                None => {}
            }
        }
        // Lines off the floor for too long end.
        let mut i = 0;
        self.lines.retain_mut(|l| {
            let keep = taken.get(i).copied().unwrap_or(false) || l.gap <= MAX_GAP;
            i += 1;
            if !keep {
                l.flush(out);
            }
            keep
        });
    }
}

/// How far apart `a` and `b` are.
pub(crate) fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f64>().sqrt()
}

/// How far `p` is from the segment `a`-`b`.
pub(super) fn off_line(a: [f64; 3], b: [f64; 3], p: [f64; 3]) -> f64 {
    let ab = [0, 1, 2].map(|k| b[k] - a[k]);
    let len2: f64 = ab.iter().map(|v| v * v).sum();
    let t = if len2 > 0.0 { ((0..3).map(|k| (p[k] - a[k]) * ab[k]).sum::<f64>() / len2).clamp(0.0, 1.0) } else { 0.0 };
    dist(p, [0, 1, 2].map(|k| a[k] + ab[k] * t))
}
