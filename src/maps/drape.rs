//! A stroke drawn on the map from above, laid on every floor under it, as lines in the world:
//! what a TF2 client in a room's relay is shown.
//!
//! The overview cannot tell a balcony from the route under it, so a stroke goes on both. Along
//! the stroke each floor under it continues the line on the floor before when it is within a step
//! or a slope of it; a line goes on over a short gap (a wall, a railing) and ends after a longer one.

use crate::lock;
use anyhow::Result;
use serde::Deserialize;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// The stroke is followed in steps this long, so lines follow the floor between its points.
const STEP: f64 = 8.0;
/// A floor continues the line when this much higher or lower than it, plus a step's worth for
/// slopes (under 45°): over a gap too, so a line does not drop off a ledge onto the floor below.
const STEP_HEIGHT: f64 = 18.0;
/// A line goes on over a gap in the floor up to this long.
const MAX_GAP: f64 = 64.0;
/// Floors closer in height than this at a point are one (faces meeting, terrain over a brush).
const SAME_FLOOR: f64 = 4.0;
/// Lines are drawn this far above the floor, clear of it.
const LIFT: f64 = 8.0;
/// A line is made of as few straight pieces as keep it within this of its steps.
const TOLERANCE: f64 = 2.0;
/// Lines shorter than this are left out: a stroke over a ledge or a crate's edge.
const MIN_LINE: f64 = 32.0;
/// Floors are found through a grid of cells this big.
const CELL: f64 = 256.0;
/// The most steps one piece of a stroke is followed in, whatever its points say.
const MAX_STEPS: usize = 20_000;
/// The most lines a stroke has on the floors at once.
const MAX_LINES: usize = 16;

/// The most maps whose floors are kept at once.
const MAX_GROUNDS: usize = 8;

/// The floors of `map`, from its geometry in `cache`, kept for the next stroke; None while the map
/// has not been built.
pub(crate) fn ground(cache: &Path, map: &str) -> Option<Arc<Ground>> {
    static KEPT: Mutex<Vec<(String, Arc<Ground>)>> = Mutex::new(Vec::new());
    if let Some((_, g)) = lock(&KEPT).iter().find(|(m, _)| m == map) {
        return Some(g.clone());
    }
    let json = super::cache::map_geometry(cache, map, false).ok()?;
    let g = Arc::new(Ground::from_json(&json).ok()?);
    let mut kept = lock(&KEPT);
    if kept.len() >= MAX_GROUNDS {
        kept.remove(0);
    }
    kept.push((map.to_string(), g.clone()));
    Some(g)
}

/// A floor as the map geometry serves it.
#[derive(Debug, Deserialize)]
struct Floor {
    z: f64,
    p: Vec<f64>,
    #[serde(default)]
    g: Option<[f64; 2]>,
}

#[derive(Deserialize)]
struct Geometry {
    #[serde(default)]
    floors: Vec<Floor>,
    /// The tops of solid static props.
    #[serde(default)]
    props: Vec<Floor>,
}

/// One floor: its outline, its box, and its height as a plane.
struct Face {
    p: Vec<f64>,
    min: [f64; 2],
    max: [f64; 2],
    /// The height at `at`, rising by `g` per unit.
    z: f64,
    at: [f64; 2],
    g: [f64; 2],
}

impl Face {
    fn new(f: Floor) -> Option<Self> {
        let n = f.p.len() / 2;
        if n < 3 {
            return None;
        }
        let xs = || f.p.iter().step_by(2);
        let ys = || f.p.iter().skip(1).step_by(2);
        let at = [xs().sum::<f64>() / n as f64, ys().sum::<f64>() / n as f64];
        let min = [xs().copied().fold(f64::INFINITY, f64::min), ys().copied().fold(f64::INFINITY, f64::min)];
        let max = [xs().copied().fold(f64::NEG_INFINITY, f64::max), ys().copied().fold(f64::NEG_INFINITY, f64::max)];
        Some(Face { p: f.p, min, max, z: f.z, at, g: f.g.unwrap_or_default() })
    }

    fn height(&self, x: f64, y: f64) -> f64 {
        self.z + self.g[0] * (x - self.at[0]) + self.g[1] * (y - self.at[1])
    }

    fn holds(&self, x: f64, y: f64) -> bool {
        x >= self.min[0] && x <= self.max[0] && y >= self.min[1] && y <= self.max[1] && super::bsp::contains_point(&self.p, x, y)
    }
}

/// A map's floors, found by where they are seen from above.
pub(crate) struct Ground {
    faces: Vec<Face>,
    origin: [f64; 2],
    w: usize,
    h: usize,
    /// The faces whose box touches each cell, row by row.
    cells: Vec<Vec<u32>>,
}

/// The most cells the grid has: a crafted map's huge floors get bigger cells' worth of faces.
const MAX_CELLS: usize = 1 << 16;
/// The most faces in cells, all together (real maps use under 200 000): a crafted map's floors
/// after that are left out.
const MAX_IN_CELLS: usize = 1 << 22;

impl Ground {
    /// The floors of the map geometry JSON served on `/map/NAME`.
    pub(crate) fn from_json(json: &[u8]) -> Result<Self> {
        let g: Geometry = serde_json::from_slice(json)?;
        Ok(Self::new(g.floors.into_iter().chain(g.props).collect()))
    }

    fn new(floors: Vec<Floor>) -> Self {
        let faces: Vec<Face> = floors.into_iter().filter_map(Face::new).collect();
        let lo = |k: usize| faces.iter().map(|f| f.min[k]).fold(f64::INFINITY, f64::min);
        let hi = |k: usize| faces.iter().map(|f| f.max[k]).fold(f64::NEG_INFINITY, f64::max);
        if faces.is_empty() {
            return Ground { faces, origin: [0.0, 0.0], w: 0, h: 0, cells: Vec::new() };
        }
        let origin = [lo(0), lo(1)];
        let along = |k: usize| (((hi(k) - origin[k]) / CELL).floor() as usize + 1).min(MAX_CELLS);
        let (w, h) = (along(0), along(1).min(MAX_CELLS / along(0)).max(1));
        let mut cells = vec![Vec::new(); w * h];
        let mut g = Ground { faces: Vec::new(), origin, w, h, cells: Vec::new() };
        let mut left = MAX_IN_CELLS;
        for (i, f) in faces.iter().enumerate() {
            let ([x0, y0], [x1, y1]) = (g.cell(f.min), g.cell(f.max));
            let Some(l) = left.checked_sub((x1 - x0 + 1) * (y1 - y0 + 1)) else { break };
            left = l;
            for y in y0..=y1 {
                for x in x0..=x1 {
                    cells[y * w + x].push(i as u32);
                }
            }
        }
        g.faces = faces;
        g.cells = cells;
        g
    }

    fn cell(&self, p: [f64; 2]) -> [usize; 2] {
        let at = |k: usize, n: usize| (((p[k] - self.origin[k]) / CELL).floor().max(0.0) as usize).min(n.saturating_sub(1));
        [at(0, self.w), at(1, self.h)]
    }

    /// The heights of the floors at (x, y), low to high, those about as high as one another once.
    fn heights(&self, x: f64, y: f64) -> Vec<f64> {
        if self.w == 0 {
            return Vec::new();
        }
        let [cx, cy] = self.cell([x, y]);
        let mut zs: Vec<f64> = self.cells[cy * self.w + cx]
            .iter()
            .map(|&i| &self.faces[i as usize])
            .filter(|f| f.holds(x, y))
            .map(|f| f.height(x, y))
            .collect();
        zs.sort_by(f64::total_cmp);
        zs.dedup_by(|a, b| *a - *b < SAME_FLOOR);
        zs
    }
}

/// A straight piece of a line in the world, and when its end was drawn (ms into the stroke).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Piece {
    pub(crate) from: [f64; 3],
    pub(crate) to: [f64; 3],
    pub(crate) ms: f64,
}

/// A line along one floor.
struct Line {
    /// Where its last piece ended, and the steps since, not yet in a piece.
    anchor: [f64; 3],
    steps: Vec<([f64; 3], f64)>,
    /// How far the stroke has gone since this line was last on a floor.
    gap: f64,
    /// How long the line is, and its pieces while it is too short to show.
    length: f64,
    held: Vec<Piece>,
}

impl Line {
    fn new(at: [f64; 3]) -> Self {
        Line { anchor: at, steps: Vec::new(), gap: 0.0, length: 0.0, held: Vec::new() }
    }

    fn last(&self) -> [f64; 3] {
        self.steps.last().map_or(self.anchor, |s| s.0)
    }

    /// On a floor again: `at`, drawn at `ms`. Pieces it makes go to `out`.
    fn reach(&mut self, at: [f64; 3], ms: f64, out: &mut Vec<Piece>) {
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

    fn piece(&mut self, to: [f64; 3], ms: f64, out: &mut Vec<Piece>) {
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
    fn flush(&mut self, out: &mut Vec<Piece>) {
        if let Some(&(end, ms)) = self.steps.last() {
            self.piece(end, ms, out);
            self.steps.clear();
        }
    }
}

/// A stroke being laid on the floors, a piece of it at a time.
pub(crate) struct Drape {
    lines: Vec<Line>,
    /// The stroke's last point followed, and when it was drawn.
    last: Option<([f64; 2], f64)>,
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
    fn step(&mut self, ground: &Ground, at: [f64; 2], ms: f64, moved: f64, out: &mut Vec<Piece>) {
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

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f64>().sqrt()
}

/// How far `p` is from the segment `a`-`b`.
fn off_line(a: [f64; 3], b: [f64; 3], p: [f64; 3]) -> f64 {
    let ab = [0, 1, 2].map(|k| b[k] - a[k]);
    let len2: f64 = ab.iter().map(|v| v * v).sum();
    let t = if len2 > 0.0 { ((0..3).map(|k| (p[k] - a[k]) * ab[k]).sum::<f64>() / len2).clamp(0.0, 1.0) } else { 0.0 };
    dist(p, [0, 1, 2].map(|k| a[k] + ab[k] * t))
}

#[cfg(test)]
mod tests;
