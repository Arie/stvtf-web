//! The playable area: the space players can get to, found by a flood fill of the map's space
//! cut into voxels, from the places players start from and go to.

use std::cell::Cell;
use std::collections::VecDeque;

use anyhow::{Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::Serialize;

use super::displacement::Displacement;
use super::{
    Bounds, FLOOR_MIN_NORMAL_Z, LUMP_BRUSHES, LUMP_BRUSHSIDES, LUMP_LEAFBRUSHES, LUMP_LEAFS, LUMP_MODELS, LUMP_NODES, LUMP_PLANES,
    PLANE_DIST, PLANE_SIZE, Vec3, crosses, f32_at, i32_at, index_at, lump, u16_at, vec3_at,
};

/// Where the BSP's version is, and the only one whose leaf layout is read here.
const BSP_VERSION: usize = 4;
const LEAF_VERSION: i32 = 20;
/// `dmodel_t`'s root node.
const MODEL_HEADNODE: usize = 36;
/// `dnode_t`: its size and its two children (a negative child is leaf `-1 - child`).
const NODE_SIZE: usize = 32;
const NODE_CHILDREN: usize = 4;
/// `dleaf_t` (version 20): its size and its range of the leaf brush list.
const LEAF_SIZE: usize = 32;
const LEAF_FIRST_LEAFBRUSH: usize = 24;
const LEAF_NUM_LEAFBRUSHES: usize = 26;
const LEAFBRUSH_SIZE: usize = 2;
/// `dbrush_t`: its size, its range of sides and its contents.
const BRUSH_SIZE: usize = 12;
const BRUSH_FIRST_SIDE: usize = 0;
const BRUSH_NUM_SIDES: usize = 4;
const BRUSH_CONTENTS: usize = 8;
/// `dbrushside_t`: its size and its plane.
const BRUSHSIDE_SIZE: usize = 8;
const BRUSHSIDE_PLANE: usize = 0;

const CONTENTS_SOLID: i32 = 0x1;
const CONTENTS_WINDOW: i32 = 0x2;
const CONTENTS_GRATE: i32 = 0x8;
const CONTENTS_PLAYERCLIP: i32 = 0x10000;
/// Brushes players cannot walk through.
const BLOCKING: i32 = CONTENTS_SOLID | CONTENTS_WINDOW | CONTENTS_GRATE | CONTENTS_PLAYERCLIP;

/// Twice the engine's limits on brushes and brush sides (8192 and 65536): more is a crafted map.
const MAX_BRUSHES: usize = 16384;
const MAX_BRUSH_SIDES: usize = 131_072;
/// Voxel columns times planes the brushes may be tested against, plus the terrain samples
/// they carry, all together. Real maps use under 10 million; a crafted map of huge brushes would
/// otherwise take minutes.
const MAX_BRUSH_WORK: usize = 1 << 28;

/// The voxel size, and the most voxels a grid may have: a huge map gets coarser voxels instead.
const VOXEL: f32 = 16.0;
const MAX_VOXELS: usize = 400_000_000;
/// How far the grid reaches past the playable bounds.
const MARGIN: f32 = 512.0;
/// How far apart terrain is sampled: half a voxel leaves no gaps.
const SAMPLE_STEP: f32 = 8.0;
/// The layer under upward terrain that blocks too, and the steps it is marked in.
const UNDER_TERRAIN: usize = 64;
const UNDER_STEP: usize = 8;
/// Two planes are the same when their normals are this close and their distances this close.
const SAME_NORMAL: f32 = 0.999;
const SAME_DIST: f32 = 0.5;
/// How far outside a brush's box a displacement's base centre may be and still be on it.
const BASE_SLACK: f32 = 1.0;
/// A plane is axial when its normal's other components are below this.
const AXIAL_SLACK: f32 = 1e-3;
/// How far a voxel must be behind a brush's plane to count as inside: a voxel touching the
/// brush blocks, one only touching its face does not.
const INSIDE_SLACK: f64 = 0.5;
/// Only space this high above something solid is filled: the clips around a map stop well
/// below the sky, and the fill would otherwise go over them into the scenery.
const NEAR_GROUND: f32 = 256.0;
/// Seeds are this high above their point, at the first free voxel of these offsets.
const SEED_UP: f32 = 40.0;
const SEED_TRIES: [isize; 4] = [0, 1, -1, 2];
/// Floors are sampled this far apart, and kept with reached space this high above.
const FLOOR_SAMPLE_STEP: f64 = 48.0;
/// The most samples along x or y of a floor: a bigger floor's are further apart. Real floors
/// have at most about 25.
const MAX_SAMPLES_ALONG: f64 = 64.0;
/// Samples times outline points the floors of a map may use together (real maps use about
/// 500 000): a crafted map's floors after that are kept.
const MAX_FLOOR_WORK: usize = 1 << 26;
const ABOVE_FLOOR: (f64, f64) = (8.0, 168.0);
/// How close to a floor's outline a sample may be outside it.
const OUTLINE_SLACK: f64 = 1.0;
/// How many cells the mask grows by around the reached space, so floors at its edge stay whole.
const MASK_GROW: usize = 2;

/// A bit per voxel.
struct Bits(Vec<u64>);

impl Bits {
    fn new(n: usize) -> Self {
        Bits(vec![0; n.div_ceil(64)])
    }
    fn get(&self, i: usize) -> bool {
        self.0[i / 64] >> (i % 64) & 1 == 1
    }
    fn set(&mut self, i: usize) {
        self.0[i / 64] |= 1 << (i % 64);
    }
    /// Sets bits `a..b`.
    fn set_range(&mut self, a: usize, b: usize) {
        let mut i = a;
        while i < b {
            let n = (b - i).min(64 - i % 64);
            self.0[i / 64] |= (u64::MAX >> (64 - n)) << (i % 64);
            i += n;
        }
    }
    /// `n` (1 to 64) bits from `i` on.
    fn word(&self, i: usize, n: usize) -> u64 {
        let (word, shift) = (i / 64, i % 64);
        let mut bits = self.0[word] >> shift;
        if shift > 0 && shift + n > 64 {
            bits |= self.0[word + 1] << (64 - shift);
        }
        bits & (u64::MAX >> (64 - n))
    }
    /// Sets the bits of `v` (`n` of them, 1 to 64) from `i` on.
    fn or_word(&mut self, i: usize, n: usize, bits: u64) {
        let (word, shift) = (i / 64, i % 64);
        let bits = bits & (u64::MAX >> (64 - n));
        self.0[word] |= bits << shift;
        if shift > 0 && shift + n > 64 {
            self.0[word + 1] |= bits >> (64 - shift);
        }
    }
    /// Whether any of bits `a..b` is set.
    fn any(&self, a: usize, b: usize) -> bool {
        (a..b).step_by(64).any(|i| self.word(i, (b - i).min(64)) != 0)
    }
}

/// The voxels: the lowest corner, the size of one and how many there are along x, y and z.
/// Voxel (x, y, z) is bit `(x * ny + y) * nz + z`: a column is a run of bits.
struct Grid {
    origin: [f64; 3],
    vox: f64,
    dims: [usize; 3],
}

impl Grid {
    fn len(&self) -> usize {
        self.dims.iter().product()
    }
    fn column(&self, x: usize, y: usize) -> usize {
        (x * self.dims[1] + y) * self.dims[2]
    }
    /// The voxel along `axis` that coordinate `v` is in, if it is in the grid.
    fn at(&self, axis: usize, v: f64) -> Option<usize> {
        let i = ((v - self.origin[axis]) / self.vox).floor();
        (i >= 0.0 && i < self.dims[axis] as f64).then_some(i as usize)
    }
    /// The voxel along `axis` that `v` is in, clamped to the grid (NaN gives 0).
    fn clamped(&self, axis: usize, v: f64) -> usize {
        ((v - self.origin[axis]) / self.vox).floor().clamp(0.0, (self.dims[axis] - 1) as f64) as usize
    }
    /// The lowest coordinate of voxel `i` along `axis`.
    fn corner(&self, axis: usize, i: usize) -> f64 {
        self.origin[axis] + i as f64 * self.vox
    }
}

/// The voxel size for `bounds` and the grid's size along x, y and z: 16-unit voxels over the
/// bounds plus a margin, doubled until the grid has at most `MAX_VOXELS`.
pub(super) fn grid_for(bounds: &Bounds) -> (f32, [usize; 3]) {
    let mut vox = VOXEL;
    loop {
        let dims =
            [0, 1, 2].map(|a| ((f64::from(bounds.hi[a] - bounds.lo[a] + 2.0 * MARGIN) / f64::from(vox)).ceil() as usize).saturating_add(1));
        let total = dims.iter().try_fold(1usize, |t, &d| t.checked_mul(d));
        if total.is_some_and(|t| t <= MAX_VOXELS) || !vox.is_finite() {
            return (vox, dims);
        }
        vox *= 2.0;
    }
}

/// The voxels players can get to.
pub(super) struct Reach {
    grid: Grid,
    reached: Bits,
    /// What is left of `MAX_FLOOR_WORK`.
    floor_work_left: Cell<usize>,
}

/// The playable area seen from above, for the client to clip the floors to: `w` by `h` cells of
/// `cell` units (the grid's voxel) from `origin` (the lowest x and y), row by row from the lowest
/// y; bit `i` is `bytes[i / 8] >> (i % 8) & 1` of `bits` (base64). A cell is set when some of the
/// space above it was reached, or that of a cell up to `MASK_GROW` cells away.
#[derive(Debug, Serialize)]
pub struct Mask {
    pub origin: [f64; 2],
    pub cell: f64,
    pub w: usize,
    pub h: usize,
    pub bits: String,
}

/// The lumps the world's brushes are read from.
struct Lumps {
    planes: Vec<u8>,
    nodes: Vec<u8>,
    leafs: Vec<u8>,
    leafbrushes: Vec<u8>,
    brushes: Vec<u8>,
    sides: Vec<u8>,
}

/// A brush of the world: its planes (normal and distance, facing out) and its contents.
struct Brush {
    planes: Vec<[f32; 4]>,
    contents: i32,
}

impl Lumps {
    fn read(data: &[u8]) -> Result<Self> {
        Ok(Lumps {
            planes: lump(data, LUMP_PLANES)?,
            nodes: lump(data, LUMP_NODES)?,
            leafs: lump(data, LUMP_LEAFS)?,
            leafbrushes: lump(data, LUMP_LEAFBRUSHES)?,
            brushes: lump(data, LUMP_BRUSHES)?,
            sides: lump(data, LUMP_BRUSHSIDES)?,
        })
    }

    /// The brushes in the leaves under `head`, each once: brush entities (doors, `func_brush`)
    /// are other models, so a closed spawn door walls nobody in. Every node and leaf is visited
    /// once, and the leaves may use the leaf brush list once over, as real maps do.
    fn world_brushes(&self, head: i32) -> Result<Vec<usize>> {
        let mut nodes_seen = vec![false; self.nodes.len() / NODE_SIZE];
        let mut leafs_seen = vec![false; self.leafs.len() / LEAF_SIZE];
        let mut brushes_seen = vec![false; self.brushes.len() / BRUSH_SIZE];
        let mut leafbrushes_left = self.leafbrushes.len() / LEAFBRUSH_SIZE;
        let (mut stack, mut out) = (vec![head], Vec::new());
        while let Some(n) = stack.pop() {
            if let Ok(node) = usize::try_from(n) {
                let seen = nodes_seen.get_mut(node).ok_or_else(|| anyhow!("node {node} out of range"))?;
                if !std::mem::replace(seen, true) {
                    stack.extend([
                        i32_at(&self.nodes, node * NODE_SIZE + NODE_CHILDREN)?,
                        i32_at(&self.nodes, node * NODE_SIZE + NODE_CHILDREN + 4)?,
                    ]);
                }
                continue;
            }
            let leaf = (-1 - i64::from(n)) as usize;
            let seen = leafs_seen.get_mut(leaf).ok_or_else(|| anyhow!("leaf {leaf} out of range"))?;
            if std::mem::replace(seen, true) {
                continue;
            }
            let first = usize::from(u16_at(&self.leafs, leaf * LEAF_SIZE + LEAF_FIRST_LEAFBRUSH)?);
            let num = usize::from(u16_at(&self.leafs, leaf * LEAF_SIZE + LEAF_NUM_LEAFBRUSHES)?);
            leafbrushes_left = leafbrushes_left.checked_sub(num).ok_or_else(|| anyhow!("leaves use more brushes than the map lists"))?;
            for i in first..first + num {
                let b = usize::from(u16_at(&self.leafbrushes, i * LEAFBRUSH_SIZE)?);
                let seen = brushes_seen.get_mut(b).ok_or_else(|| anyhow!("brush {b} out of range"))?;
                if !std::mem::replace(seen, true) {
                    out.push(b);
                }
            }
            if out.len() > MAX_BRUSHES {
                bail!("more than {MAX_BRUSHES} brushes");
            }
        }
        Ok(out)
    }

    /// Brush `b`, whose sides count against `sides_left`.
    fn brush(&self, b: usize, sides_left: &mut usize) -> Result<Brush> {
        let o = b * BRUSH_SIZE;
        let (first, num) = (index_at(&self.brushes, o + BRUSH_FIRST_SIDE)?, index_at(&self.brushes, o + BRUSH_NUM_SIDES)?);
        *sides_left = sides_left.checked_sub(num).ok_or_else(|| anyhow!("brushes have more than {MAX_BRUSH_SIDES} sides"))?;
        let planes = (first..first + num)
            .map(|s| {
                let p = usize::from(u16_at(&self.sides, s * BRUSHSIDE_SIZE + BRUSHSIDE_PLANE)?) * PLANE_SIZE;
                let n = vec3_at(&self.planes, p)?;
                Ok([n[0], n[1], n[2], f32_at(&self.planes, p + PLANE_DIST)?])
            })
            .collect::<Result<_>>()?;
        Ok(Brush { planes, contents: i32_at(&self.brushes, o + BRUSH_CONTENTS)? })
    }
}

impl Brush {
    /// The brush's box, from its axial sides (every brush has them), within `grid`'s; None
    /// when it is outside the grid.
    fn bounds(&self, grid: &Grid) -> Option<(Vec3, Vec3)> {
        let mut lo = grid.origin.map(|v| v as f32);
        let mut hi = [0, 1, 2].map(|a| grid.corner(a, grid.dims[a]) as f32);
        for p in &self.planes {
            for a in 0..3 {
                if p[a].abs() > SAME_NORMAL && (0..3).filter(|&b| b != a).all(|b| p[b].abs() < AXIAL_SLACK) {
                    if p[a] > 0.0 {
                        hi[a] = hi[a].min(p[3]);
                    } else {
                        lo[a] = lo[a].max(-p[3]);
                    }
                }
            }
        }
        let inside = |a: usize| {
            lo[a].partial_cmp(&hi[a]).is_some_and(std::cmp::Ordering::is_le)
                && f64::from(hi[a]) >= grid.origin[a]
                && f64::from(lo[a]) <= grid.corner(a, grid.dims[a])
        };
        (0..3).all(inside).then_some((lo, hi))
    }

    /// The voxels of column (x, y), from `bottom` to `roof`, that are behind every plane, as the
    /// lowest and highest: for the corner of a voxel nearest a plane's back,
    /// `c + z * dz <= dist - slack`, which is a range of z.
    fn column(&self, grid: &Grid, x: usize, y: usize, bottom: usize, roof: usize) -> Option<(usize, usize)> {
        let (mut lo, mut hi) = (bottom as f64, roof as f64);
        for plane in &self.planes {
            let n = plane.map(f64::from);
            let near = |a: usize, i: usize| (grid.corner(a, i) + if n[a] < 0.0 { grid.vox } else { 0.0 }) * n[a];
            let limit = n[3] - INSIDE_SLACK - (near(0, x) + near(1, y) + near(2, 0));
            let dz = grid.vox * n[2];
            if dz > 0.0 {
                hi = hi.min((limit / dz).floor());
            } else if dz < 0.0 {
                lo = lo.max((limit / dz).ceil());
            } else if limit.is_nan() || limit < 0.0 {
                return None;
            }
        }
        // NaN planes leave NaN, which is no range.
        (lo <= hi).then_some((lo as usize, hi as usize))
    }
}

/// The voxels the world's brushes and terrain block.
struct Blocker<'a> {
    grid: &'a Grid,
    disps: &'a [Displacement],
    blocked: Bits,
    work_left: usize,
}

impl Blocker<'_> {
    /// Marks the voxels `brush` overlaps. One carrying upward terrain (a side holds its base
    /// face) is solid only from its bottom up to the terrain, also where a hill rises above its
    /// top: players stand on the terrain, which can sit well below the brush's top.
    fn brush(&mut self, brush: &Brush) -> Result<()> {
        let grid = self.grid;
        let Some((lo, hi)) = brush.bounds(grid) else { return Ok(()) };
        let first = [0, 1, 2].map(|a| grid.clamped(a, f64::from(lo[a])));
        let last = [0, 1, 2].map(|a| grid.clamped(a, f64::from(hi[a])));
        let (wide, deep) = (last[0] - first[0] + 1, last[1] - first[1] + 1);
        let work = wide * deep * (brush.planes.len() + 1);
        self.work_left = self.work_left.checked_sub(work).ok_or_else(|| anyhow!("brushes too big to voxelise"))?;

        let tops: Vec<&Displacement> = if brush.contents & CONTENTS_PLAYERCLIP == 0 {
            self.disps.iter().filter(|d| d.plane[2] > FLOOR_MIN_NORMAL_Z && on(d, brush, lo, hi)).collect()
        } else {
            Vec::new()
        };
        // The highest terrain in each voxel column of the brush's box.
        let mut surface = vec![f32::NEG_INFINITY; if tops.is_empty() { 0 } else { wide * deep }];
        for d in &tops {
            let samples = d.samples(SAMPLE_STEP);
            self.work_left = self.work_left.checked_sub(samples.len()).ok_or_else(|| anyhow!("brushes carry too much terrain"))?;
            for p in samples {
                let (Some(x), Some(y)) = (grid.at(0, f64::from(p[0])), grid.at(1, f64::from(p[1]))) else { continue };
                if (first[0]..=last[0]).contains(&x) && (first[1]..=last[1]).contains(&y) {
                    let z = &mut surface[(x - first[0]) * deep + y - first[1]];
                    *z = z.max(p[2]);
                }
            }
        }
        let highest = surface.iter().map(|&z| if z.is_finite() { z } else { hi[2] }).fold(f32::NEG_INFINITY, f32::max);
        let top = last[2].max(grid.clamped(2, f64::from(highest)));

        for x in first[0]..=last[0] {
            for y in first[1]..=last[1] {
                let Some((bottom, roof)) = brush.column(grid, x, y, first[2], last[2]) else { continue };
                let col = grid.column(x, y);
                let ground = surface.get((x - first[0]) * deep + y - first[1]).copied().unwrap_or(f32::NEG_INFINITY);
                if ground.is_finite() {
                    // From the brush's bottom up to the last voxel starting below the terrain.
                    let below = ((f64::from(ground) - grid.origin[2]) / grid.vox).ceil() - 1.0;
                    let end = below.min(top as f64);
                    if end >= first[2] as f64 {
                        self.blocked.set_range(col + first[2], col + end as usize + 1);
                    }
                } else {
                    self.blocked.set_range(col + bottom, col + roof + 1);
                }
            }
        }
        Ok(())
    }

    /// Marks the terrain itself, and a layer under upward terrain so seams at its edges do not
    /// let the fill into the hollows under it.
    fn terrain(&mut self) {
        let g = self.grid;
        for d in self.disps {
            let under = if d.plane[2] > FLOOR_MIN_NORMAL_Z { UNDER_TERRAIN } else { 0 };
            for s in d.samples(SAMPLE_STEP) {
                let (Some(x), Some(y)) = (g.at(0, f64::from(s[0])), g.at(1, f64::from(s[1]))) else { continue };
                for dz in (0..=under).step_by(UNDER_STEP) {
                    if let Some(z) = g.at(2, f64::from(s[2]) - dz as f64) {
                        self.blocked.set(g.column(x, y) + z);
                    }
                }
            }
        }
    }
}

/// Whether displacement `d` lies on `brush` (box `lo`, `hi`): its base's centre is in the box
/// and one of the brush's sides has the base face's plane.
fn on(d: &Displacement, brush: &Brush, lo: Vec3, hi: Vec3) -> bool {
    let centre = [0, 1, 2].map(|a| d.base.iter().map(|p| p[a]).sum::<f32>() / 4.0);
    (0..3).all(|a| lo[a] - BASE_SLACK <= centre[a] && centre[a] <= hi[a] + BASE_SLACK)
        && brush
            .planes
            .iter()
            .any(|p| p[0] * d.plane[0] + p[1] * d.plane[1] + p[2] * d.plane[2] > SAME_NORMAL && (p[3] - d.plane[3]).abs() < SAME_DIST)
}

/// The space players can get to from `seeds` (spawn points, packs, capture areas, the track):
/// a flood fill of the voxels the world's brushes and terrain leave free, within `bounds` and
/// a margin. None when there is nothing to start from or the BSP's leaves are of another
/// version: then every floor is kept.
pub(super) fn reach(data: &[u8], disps: &[Displacement], bounds: &Bounds, seeds: &[Vec3]) -> Result<Option<Reach>> {
    if seeds.is_empty() || i32_at(data, BSP_VERSION)? != LEAF_VERSION {
        return Ok(None);
    }
    if !bounds.lo.iter().chain(&bounds.hi).all(|v| v.is_finite()) {
        bail!("bounds {bounds:?}");
    }
    let (vox, dims) = grid_for(bounds);
    if dims.iter().product::<usize>() > MAX_VOXELS {
        bail!("grid of {dims:?} voxels");
    }
    let grid = Grid { origin: bounds.lo.map(|v| f64::from(v - MARGIN)), vox: f64::from(vox), dims };
    let lumps = Lumps::read(data)?;
    let head = i32_at(&lump(data, LUMP_MODELS)?, MODEL_HEADNODE)?;
    let mut marks = Blocker { grid: &grid, disps, blocked: Bits::new(grid.len()), work_left: MAX_BRUSH_WORK };
    let mut sides_left = MAX_BRUSH_SIDES;
    for b in lumps.world_brushes(head)? {
        let brush = lumps.brush(b, &mut sides_left)?;
        if brush.contents & BLOCKING != 0 {
            marks.brush(&brush)?;
        }
    }
    marks.terrain();
    let blocked = marks.blocked;

    let [nx, ny, nz] = dims;
    // Space at most NEAR_GROUND above a blocked voxel.
    let mut near = Bits::new(grid.len());
    let height = (NEAR_GROUND / vox) as usize;
    for col in (0..nx * ny).map(|c| c * nz) {
        let mut run = usize::MAX;
        for z in 0..nz {
            run = if blocked.get(col + z) { 0 } else { run.saturating_add(1) };
            if run <= height {
                near.set(col + z);
            }
        }
    }
    let mut start = Vec::new();
    for p in seeds {
        let at = [0, 1, 2].map(|a| grid.at(a, f64::from(p[a]) + if a == 2 { f64::from(SEED_UP) } else { 0.0 }));
        let [Some(x), Some(y), Some(z)] = at else { continue };
        // The first free voxel near it: a seed can sit just inside a brush.
        let free = SEED_TRIES
            .iter()
            .filter_map(|dz| z.checked_add_signed(*dz).filter(|&z| z < nz))
            .map(|z| grid.column(x, y) + z)
            .find(|&i| !blocked.get(i));
        start.extend(free.filter(|&i| near.get(i)));
    }
    // Players are 48 units wide: space beside a wall, closer than half that, is not somewhere one fits.
    let mut free = Bits::new(grid.len());
    for x in 0..nx {
        for y in 0..ny {
            let col = grid.column(x, y);
            for (ax, ay) in
                (x.saturating_sub(1)..=(x + 1).min(nx - 1)).flat_map(|a| (y.saturating_sub(1)..=(y + 1).min(ny - 1)).map(move |b| (a, b)))
            {
                let other = grid.column(ax, ay);
                for z in (0..nz).step_by(64) {
                    let n = (nz - z).min(64);
                    free.or_word(col + z, n, blocked.word(other + z, n));
                }
            }
        }
    }
    drop(blocked);
    for (f, n) in free.0.iter_mut().zip(&near.0) {
        *f = !*f & n;
    }
    drop(near);

    // A flood fill, 6-connected; the seeds count as reached even where they are now beside a wall.
    let mut reached = Bits::new(grid.len());
    let mut queue: VecDeque<u32> = VecDeque::new();
    for i in start {
        if !reached.get(i) {
            reached.set(i);
            queue.push_back(i as u32);
        }
    }
    if queue.is_empty() {
        return Ok(None);
    }
    let (sy, sx) = (nz, ny * nz);
    while let Some(i) = queue.pop_front() {
        let i = i as usize;
        let (x, y, z) = (i / sx, i / sy % ny, i % nz);
        let steps = [
            (x > 0, i.wrapping_sub(sx)),
            (x + 1 < nx, i + sx),
            (y > 0, i.wrapping_sub(sy)),
            (y + 1 < ny, i + sy),
            (z > 0, i.wrapping_sub(1)),
            (z + 1 < nz, i + 1),
        ];
        for (ok, j) in steps {
            if ok && free.get(j) && !reached.get(j) {
                reached.set(j);
                queue.push_back(j as u32);
            }
        }
    }
    Ok(Some(Reach { grid, reached, floor_work_left: Cell::new(MAX_FLOOR_WORK) }))
}

impl Reach {
    /// Whether a voxel above (x, y) at height `z_lo`, `z_lo` + a voxel, … below `z_hi` was reached.
    pub(super) fn above(&self, x: f64, y: f64, z_lo: f64, z_hi: f64) -> bool {
        let g = &self.grid;
        let (Some(x), Some(y)) = (g.at(0, x), g.at(1, y)) else { return false };
        let col = g.column(x, y);
        let mut z = z_lo;
        while z < z_hi {
            if g.at(2, z).is_some_and(|k| self.reached.get(col + k)) {
                return true;
            }
            z += g.vox;
        }
        false
    }

    /// Whether players can get to the space just above the floor `p` (`x, y, x, y, …`) at height `z`.
    pub(super) fn keeps(&self, p: &[f64], z: f64) -> bool {
        let outline = p.as_chunks::<2>().0;
        let (_, _, [along_x, along_y]) = sample_grid(outline);
        let work = outline.len() * (along_x * along_y + 1);
        let Some(left) = self.floor_work_left.get().checked_sub(work) else { return true };
        self.floor_work_left.set(left);
        floor_samples(p).into_iter().any(|[x, y]| self.above(x, y, z + ABOVE_FLOOR.0, z + ABOVE_FLOOR.1))
    }

    /// The reached space seen from above.
    pub(super) fn mask(&self) -> Mask {
        let g = &self.grid;
        let [w, h, nz] = g.dims;
        let mut cells: Vec<bool> = (0..w * h)
            .map(|i| {
                let col = g.column(i % w, i / w);
                self.reached.any(col, col + nz)
            })
            .collect();
        for _ in 0..MASK_GROW {
            cells = (0..w * h)
                .map(|i| {
                    let (cx, cy) = (i % w, i / w);
                    (cx.saturating_sub(1)..=(cx + 1).min(w - 1))
                        .any(|x| (cy.saturating_sub(1)..=(cy + 1).min(h - 1)).any(|y| cells[y * w + x]))
                })
                .collect();
        }
        let mut bytes = vec![0u8; (w * h).div_ceil(8)];
        for (i, _) in cells.iter().enumerate().filter(|&(_, &set)| set) {
            bytes[i / 8] |= 1 << (i % 8);
        }
        Mask { origin: [g.origin[0], g.origin[1]], cell: g.vox, w, h, bits: STANDARD.encode(bytes) }
    }
}

/// Points of the floor `p` (`x, y, x, y, …`) to look above: its centroid (of its corners) and
/// a grid `FLOOR_SAMPLE_STEP` apart from its lowest corner, those within `OUTLINE_SLACK` of
/// the outline (just the centroid when none is).
pub(super) fn floor_samples(p: &[f64]) -> Vec<[f64; 2]> {
    let outline = p.as_chunks::<2>().0;
    if outline.is_empty() {
        return Vec::new();
    }
    let count = outline.len() as f64;
    let centroid = [0, 1].map(|a| outline.iter().map(|q| q[a]).sum::<f64>() / count);
    let (lo, step, [along_x, along_y]) = sample_grid(outline);
    let mut out: Vec<[f64; 2]> =
        (0..along_x).flat_map(|i| (0..along_y).map(move |j| [lo[0] + i as f64 * step, lo[1] + j as f64 * step])).collect();
    out.push(centroid);
    if outline.len() >= 3 {
        out.retain(|q| within(outline, *q));
        if out.is_empty() {
            out.push(centroid);
        }
    }
    out
}

/// The grid a floor is sampled on: its lowest corner, its step and how many points it has
/// along x and y (up to a unit past the highest corner). At most `MAX_SAMPLES_ALONG` along
/// either, and none for NaN or infinite corners.
fn sample_grid(outline: &[[f64; 2]]) -> ([f64; 2], f64, [usize; 2]) {
    let lo = [0, 1].map(|a| outline.iter().map(|q| q[a]).fold(f64::INFINITY, f64::min));
    let hi = [0, 1].map(|a| outline.iter().map(|q| q[a]).fold(f64::NEG_INFINITY, f64::max));
    let extent = [0, 1].map(|a| hi[a] + 1.0 - lo[a]);
    let step = FLOOR_SAMPLE_STEP.max(extent[0] / MAX_SAMPLES_ALONG).max(extent[1] / MAX_SAMPLES_ALONG);
    (lo, step, extent.map(|e| (e / step).ceil().min(MAX_SAMPLES_ALONG) as usize))
}

/// Whether `q` is inside the outline `pts`, or within `OUTLINE_SLACK` of it.
fn within(pts: &[[f64; 2]], q: [f64; 2]) -> bool {
    let edges = || pts.iter().zip(pts.iter().cycle().skip(1));
    let inside = edges().fold(false, |inside, (a, b)| inside ^ crosses(*a, *b, q));
    inside
        || edges().any(|(a, b)| {
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let len2 = dx * dx + dy * dy;
            let t = if len2 > 0.0 { (((q[0] - a[0]) * dx + (q[1] - a[1]) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
            (q[0] - a[0] - t * dx).hypot(q[1] - a[1] - t * dy) <= OUTLINE_SLACK
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bsp::tests::{map, process, swiftwater};
    use crate::bsp::{Bounds, LUMP_LENGTH, contains_point as contains, geometry, lump_entry};

    #[test]
    fn process_keeps_the_playable_floors_only() {
        let Some(data) = process() else { return };
        let g = geometry(&data).unwrap();
        // Every spawn point and pack has a kept floor within 64 units under it.
        for p in g.spawns.points.iter().map(|s| s.pos).chain(g.packs.iter().map(|p| p.pos)) {
            assert!(g.floors.iter().any(|f| contains(&f.p, p[0], p[1]) && (p[2] - f.z).abs() < 64.0), "{p:?}");
        }
        // The mask is at the voxel's size and covers them all.
        let m = g.playable.unwrap();
        let bytes = STANDARD.decode(&m.bits).unwrap();
        assert_eq!(m.cell, 16.0);
        for p in g.spawns.points.iter().map(|s| s.pos).chain(g.packs.iter().map(|p| p.pos)) {
            let at = [0, 1].map(|a| ((p[a] - m.origin[a]) / m.cell) as usize);
            let i = at[1] * m.w + at[0];
            assert!(bytes[i / 8] >> (i % 8) & 1 == 1, "{p:?}");
        }
    }

    #[test]
    fn sunshine_fields_beyond_the_clips_are_left_out() {
        let Some(data) = map("cp_sunshine") else { return };
        let g = geometry(&data).unwrap();
        // The field beyond the clips (the fill leaked into it over them before the 256-unit rule).
        for (x, y) in [(-6100.0, 8715.0), (-6000.0, 8600.0)] {
            assert!(!g.floors.iter().any(|f| contains(&f.p, x, y)), "({x}, {y})");
        }
    }

    #[test]
    fn swiftwater_cave_and_track_loop_grass_are_kept() {
        let Some(data) = swiftwater() else { return };
        let t = std::time::Instant::now();
        let g = geometry(&data).unwrap();
        eprintln!("pl_swiftwater_final1 built in {:.2?}", t.elapsed());
        // The cave floor and the grass inside the track loop (both lost before the terrain rule).
        for (x, y) in [(4263.0, -2873.0), (1617.0, -2993.0)] {
            assert!(g.floors.iter().any(|f| contains(&f.p, x, y)), "({x}, {y})");
        }
        // Every pack has a kept floor 0 to 64 units under it (two at (2036, 1496/1544, -1127) were
        // on floors the fill missed).
        for p in g.packs.iter().map(|p| p.pos) {
            assert!(g.floors.iter().any(|f| contains(&f.p, p[0], p[1]) && (0.0..=64.0).contains(&(p[2] - f.z))), "{p:?}");
        }
    }

    #[test]
    fn maps_with_nothing_to_seed_keep_every_floor() {
        // Review focus 1: a map with no spawns, packs, points or track.
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/cache/mge_training_v8_beta4b.bsp");
        let Ok(data) = std::fs::read(path) else { return eprintln!("{path} not downloaded; skipped") };
        let g = geometry(&data).unwrap();
        assert!(g.floors.len() > 1000);
    }

    #[test]
    fn brushes_as_big_as_the_map_are_bounded() {
        let Some(mut data) = process() else { return };
        // Every brush side gets the same plane, so no brush has a box: each fills the grid.
        let mut sides = lump(&data, LUMP_BRUSHSIDES).unwrap();
        for side in sides.as_chunks_mut::<BRUSHSIDE_SIZE>().0 {
            side[BRUSHSIDE_PLANE..BRUSHSIDE_PLANE + 2].fill(0);
        }
        let h = lump_entry(LUMP_BRUSHSIDES);
        let (ofs, len) = (data.len() as i32, sides.len() as i32);
        data.extend_from_slice(&sides);
        data[h..h + 4].copy_from_slice(&ofs.to_le_bytes());
        data[h + LUMP_LENGTH..h + LUMP_LENGTH + 4].copy_from_slice(&len.to_le_bytes());
        let t = std::time::Instant::now();
        let g = geometry(&data).unwrap();
        eprintln!("{:.2?}, {} floors", t.elapsed(), g.floors.len());
        assert!(g.playable.is_none() && g.floors.len() > 1000);
    }

    #[test]
    fn floors_are_sampled_inside_their_outline() {
        // A 100-unit square: its centre and a 48-unit grid from its corner, all inside or on it.
        let s = floor_samples(&[0.0, 0.0, 100.0, 0.0, 100.0, 100.0, 0.0, 100.0]);
        assert_eq!(s.len(), 10);
        assert!(s.contains(&[50.0, 50.0]) && s.contains(&[96.0, 96.0]));
        // A sliver thinner than a step: its corner (on the outline) and its centroid.
        assert_eq!(floor_samples(&[10.0, 10.0, 20.0, 10.0, 20.0, 10.5]).len(), 2);
    }

    #[test]
    fn huge_bounds_coarsen_the_voxels() {
        // Review focus 2: 40 000 x 40 000 x 8 000 units stays under the voxel cap.
        let (vox, dims) = grid_for(&Bounds { lo: [-20000.0; 3], hi: [20000.0, 20000.0, 8000.0] });
        assert!(vox > 16.0 && dims.iter().product::<usize>() <= 400_000_000);
    }
}
