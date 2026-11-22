//! The ground strokes are laid on: a map's floors, in cells, kept for a while.

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use serde::Deserialize;

use super::strokes::SAME_FLOOR;
use crate::sync::lock;

/// Floors are found through a grid of cells this big.
pub(super) const CELL: f64 = 256.0;

/// The most maps whose floors are kept at once.
pub(super) const MAX_GROUNDS: usize = 8;

/// The floors of `map`, from its geometry in `cache`, kept for the next stroke; None while the map
/// has not been built.
pub(crate) fn ground(cache: &Path, map: &str) -> Option<Arc<Ground>> {
    static KEPT: Mutex<Vec<(String, Arc<Ground>)>> = Mutex::new(Vec::new());
    if let Some((_, g)) = lock(&KEPT).iter().find(|(m, _)| m == map) {
        return Some(g.clone());
    }
    let json = super::super::cache::map_geometry(cache, map, false).ok()?;
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
pub(super) struct Floor {
    pub(super) z: f64,
    pub(super) p: Vec<f64>,
    #[serde(default)]
    pub(super) g: Option<[f64; 2]>,
}

#[derive(Deserialize)]
pub(super) struct Geometry {
    #[serde(default)]
    pub(super) floors: Vec<Floor>,
    /// The tops of solid static props.
    #[serde(default)]
    pub(super) props: Vec<Floor>,
    /// The playable area's lowest and highest corner.
    #[serde(default)]
    pub(super) bounds: Option<[[f64; 3]; 2]>,
}

/// The overview leaves out floors higher than the top of `bounds` minus the geometry's pad plus
/// this (the same as `ROOF_ABOVE` in `app/map/geometry.js`).
pub(super) const ROOF_ABOVE: f64 = 400.0;

/// One floor: its outline, its box, and its height as a plane.
pub(super) struct Face {
    pub(super) p: Vec<f64>,
    pub(super) min: [f64; 2],
    pub(super) max: [f64; 2],
    /// The height at `at`, rising by `g` per unit.
    pub(super) z: f64,
    pub(super) at: [f64; 2],
    pub(super) g: [f64; 2],
}

impl Face {
    pub(super) fn new(f: Floor) -> Option<Self> {
        let n = f.p.len() / 2;
        if n < 3 {
            return None;
        }
        let at = [0, 1].map(|k| f.p.iter().skip(k).step_by(2).sum::<f64>() / n as f64);
        let [x0, y0, x1, y1] = super::super::bsp::outline_box(&f.p);
        Some(Face { p: f.p, min: [x0, y0], max: [x1, y1], z: f.z, at, g: f.g.unwrap_or_default() })
    }

    pub(super) fn height(&self, x: f64, y: f64) -> f64 {
        self.z + self.g[0] * (x - self.at[0]) + self.g[1] * (y - self.at[1])
    }

    pub(super) fn holds(&self, x: f64, y: f64) -> bool {
        x >= self.min[0] && x <= self.max[0] && y >= self.min[1] && y <= self.max[1] && super::super::bsp::contains_point(&self.p, x, y)
    }
}

/// A map's floors, found by where they are seen from above.
pub(crate) struct Ground {
    pub(super) faces: Vec<Face>,
    pub(super) origin: [f64; 2],
    pub(super) w: usize,
    pub(super) h: usize,
    /// The faces whose box touches each cell, row by row.
    pub(super) cells: Vec<Vec<u32>>,
    /// Floors above this are roofs the overview leaves out.
    pub(super) roof: f64,
}

/// The most cells the grid has: on a crafted map with huge floors, the last row and column of
/// cells also take the faces that lie past them.
pub(super) const MAX_CELLS: usize = 1 << 16;

/// The most faces in cells, all together (real maps use under 200 000): a crafted map's floors
/// after that are left out.
pub(super) const MAX_IN_CELLS: usize = 1 << 22;

impl Ground {
    /// The floors of the map geometry JSON served on `/map/NAME`.
    pub(crate) fn from_json(json: &[u8]) -> Result<Self> {
        let g: Geometry = serde_json::from_slice(json)?;
        let roof = g.bounds.map_or(f64::INFINITY, |[_, hi]| hi[2] - f64::from(super::super::bsp::PAD) + ROOF_ABOVE);
        Ok(Self::new(g.floors.into_iter().chain(g.props).collect(), roof))
    }

    pub(super) fn new(floors: Vec<Floor>, roof: f64) -> Self {
        let faces: Vec<Face> = floors.into_iter().filter_map(Face::new).collect();
        let lo = |k: usize| faces.iter().map(|f| f.min[k]).fold(f64::INFINITY, f64::min);
        let hi = |k: usize| faces.iter().map(|f| f.max[k]).fold(f64::NEG_INFINITY, f64::max);
        if faces.is_empty() {
            return Ground { faces, origin: [0.0, 0.0], w: 0, h: 0, cells: Vec::new(), roof };
        }
        let origin = [lo(0), lo(1)];
        let along = |k: usize| (((hi(k) - origin[k]) / CELL).floor() as usize + 1).min(MAX_CELLS);
        let (w, h) = (along(0), along(1).min(MAX_CELLS / along(0)).max(1));
        let mut cells = vec![Vec::new(); w * h];
        let mut g = Ground { faces: Vec::new(), origin, w, h, cells: Vec::new(), roof };
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

    pub(super) fn cell(&self, p: [f64; 2]) -> [usize; 2] {
        let at = |k: usize, n: usize| (((p[k] - self.origin[k]) / CELL).floor().max(0.0) as usize).min(n.saturating_sub(1));
        [at(0, self.w), at(1, self.h)]
    }

    /// The highest floor at (x, y) the overview shows (under its roof cut), if there is one.
    pub(crate) fn top(&self, x: f64, y: f64) -> Option<f64> {
        self.heights(x, y).into_iter().rev().find(|z| *z <= self.roof)
    }

    /// The heights of the floors at (x, y), low to high; floors within `SAME_FLOOR` of one another
    /// count once.
    pub(super) fn heights(&self, x: f64, y: f64) -> Vec<f64> {
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
