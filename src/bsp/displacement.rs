//! Displacements (terrain): the grid of points each one bends its base face into, which is
//! where the ground really is.

use super::{
    FACE_SIZE, LUMP_DISP_VERTS, LUMP_DISPINFO, LUMP_MODELS, MODEL_FIRST_FACE, MODEL_NUM_FACES, SURFEDGE_SIZE, Vec3, World, f32_at, i32_at,
    index_at, lump, vec3_at,
};
use anyhow::{Result, anyhow, bail};

/// `ddispinfo_t`: its size and the fields read, and `dDispVert`: its size and its direction and distance.
const DISPINFO_SIZE: usize = 176;
const DISPINFO_VERTEX_START: usize = 12;
const DISPINFO_POWER: usize = 20;
const DISP_VERT_SIZE: usize = 20;
const DISP_VERT_DIST: usize = 12;
/// A displacement's grid is `2^power + 1` points wide, and its power is 2 to 4.
const MIN_POWER: usize = 2;
const MAX_POWER: usize = 4;
/// All the grid points read from one map: real maps have far fewer, and a crafted one could
/// otherwise make every face's displacement read the same points again and again.
const MAX_GRID_POINTS: usize = 1 << 20;
/// `samples` gives at most about this many points per displacement (a real 512-unit one at a
/// step of 8 gives 4225, a 1400-unit one 8400), coarsening all its cells alike when more would come.
const MAX_SAMPLES: usize = 16384;
/// Displacements read per map (real maps have up to about 1200): with `MAX_SAMPLES` a crafted
/// map's samples are then at most about 400 MB. The rest keep their base faces.
const MAX_DISPLACEMENTS: usize = 2048;
/// The most pieces a cell is cut into per side by `samples`.
const MAX_SAMPLE_SPLIT: usize = 64;

/// A displacement of the world: the plane of its base face (normal, distance, facing out), the
/// base face's corners, and the `n` by `n` grid of points (row-major) that is its real surface.
#[derive(Debug)]
pub(super) struct Displacement {
    pub plane: [f32; 4],
    pub base: [Vec3; 4],
    pub n: usize,
    pub grid: Vec<Vec3>,
    pub texture: usize,
    /// The base face's index in the faces lump.
    pub face: usize,
}

fn lerp(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t)
}

impl Displacement {
    /// The surface's cells: the four grid points around each, in order.
    pub fn cells(&self) -> impl Iterator<Item = [Vec3; 4]> + '_ {
        let (n, g) = (self.n, &self.grid);
        (0..n - 1).flat_map(move |i| (0..n - 1).map(move |j| [g[i * n + j], g[i * n + j + 1], g[(i + 1) * n + j + 1], g[(i + 1) * n + j]]))
    }

    /// Points on the surface that are no more than `step` apart (a step of 0 gives the grid's points).
    pub fn samples(&self, step: f32) -> Vec<Vec3> {
        if step <= 0.0 || step.is_nan() {
            return self.grid.clone();
        }
        let cells_per_side = self.n - 1;
        let splits = |scale: f32| -> Vec<usize> {
            self.cells()
                .map(|[near_left, near_right, far_right, far_left]| {
                    let longest = [(near_left, near_right), (near_right, far_right), (far_right, far_left), (far_left, near_left)]
                        .into_iter()
                        .map(|(p, q)| (0..3).map(|k| (p[k] - q[k]).powi(2)).sum::<f32>().sqrt())
                        .fold(0.0, f32::max);
                    ((longest / step).min(MAX_SAMPLE_SPLIT as f32) * scale).ceil().max(1.0) as usize
                })
                .collect()
        };
        // Points a split makes: the cell's far edges belong to the next cell, except at the surface's edge.
        let count = |splits: &[usize]| -> usize {
            splits
                .iter()
                .enumerate()
                .map(|(i, &m)| {
                    (m + usize::from(i / cells_per_side == cells_per_side - 1))
                        * (m + usize::from(i % cells_per_side == cells_per_side - 1))
                })
                .sum()
        };
        let mut splits_now = splits(1.0);
        let total = count(&splits_now);
        if total > MAX_SAMPLES {
            splits_now = splits((MAX_SAMPLES as f32 / total as f32).sqrt());
        }
        let mut out = Vec::new();
        for (index, ([near_left, near_right, far_right, far_left], split)) in self.cells().zip(splits_now).enumerate() {
            let (row, col) = (index / cells_per_side, index % cells_per_side);
            let (rows, cols) = (split + usize::from(row == cells_per_side - 1), split + usize::from(col == cells_per_side - 1));
            for r in 0..rows {
                for c in 0..cols {
                    let (along, across) = (r as f32 / split as f32, c as f32 / split as f32);
                    out.push(lerp(lerp(near_left, far_left, along), lerp(near_right, far_right, along), across));
                }
            }
        }
        out
    }
}

/// The displacements of the world model's faces (those with 4 corners). One that cannot be read
/// (a power out of range, points past the lump) is left out.
pub(super) fn displacements(data: &[u8], world: &World) -> Result<Vec<Displacement>> {
    let models = lump(data, LUMP_MODELS)?;
    let (first, count) = (index_at(&models, MODEL_FIRST_FACE)?, index_at(&models, MODEL_NUM_FACES)?);
    let (info, verts) = (lump(data, LUMP_DISPINFO)?, lump(data, LUMP_DISP_VERTS)?);
    let mut edges_left = world.surfedges.len() / SURFEDGE_SIZE;
    let mut points_left = MAX_GRID_POINTS;
    let mut out = Vec::new();
    for f in first..first.saturating_add(count).min(world.faces.len() / FACE_SIZE) {
        let Some(face) = world.face(f, &mut edges_left)? else { continue };
        let (Some(d), Ok(base)) = (face.disp, <[Vec3; 4]>::try_from(face.pts.as_slice())) else { continue };
        if out.len() >= MAX_DISPLACEMENTS {
            break;
        }
        if let Ok(disp) = read(&info, &verts, d, &face, base, &mut points_left) {
            out.push(Displacement { face: f, ..disp });
        }
    }
    Ok(out)
}

/// Displacement `d` of `face`, whose grid points count against `points_left`.
fn read(info: &[u8], verts: &[u8], disp: usize, face: &super::Face, base: [Vec3; 4], points_left: &mut usize) -> Result<Displacement> {
    let o = disp * DISPINFO_SIZE;
    let start = vec3_at(info, o)?;
    let first = index_at(info, o + DISPINFO_VERTEX_START)?;
    let power = usize::try_from(i32_at(info, o + DISPINFO_POWER)?).unwrap_or(0);
    if !(MIN_POWER..=MAX_POWER).contains(&power) {
        bail!("displacement power {power}");
    }
    let n = (1 << power) + 1;
    *points_left = points_left.checked_sub(n * n).ok_or_else(|| anyhow!("displacements use more points than allowed"))?;
    // The grid starts at the corner nearest `startPosition`, and runs on in the face's order.
    let dist2 = |p: &Vec3| (0..3).map(|k| (p[k] - start[k]).powi(2)).sum::<f32>();
    let nearest = (0..4).min_by(|&a, &b| dist2(&base[a]).total_cmp(&dist2(&base[b]))).unwrap_or(0);
    let [c0, c3, c2, c1] = [0, 1, 2, 3].map(|i| base[(nearest + i) % 4]);
    let mut grid = Vec::with_capacity(n * n);
    for i in 0..n {
        let s = i as f32 / (n - 1) as f32;
        let (from, to) = (lerp(c0, c3, s), lerp(c1, c2, s));
        for j in 0..n {
            let v = (first + i * n + j) * DISP_VERT_SIZE;
            let (dir, height) = (vec3_at(verts, v)?, f32_at(verts, v + DISP_VERT_DIST)?);
            let flat = lerp(from, to, j as f32 / (n - 1) as f32);
            grid.push([0, 1, 2].map(|c| flat[c] + dir[c] * height));
        }
    }
    Ok(Displacement { plane: face.plane, base, n, grid, texture: face.texture, face: 0 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bsp::contains_point as contains;
    use crate::bsp::tests::swiftwater;
    use crate::bsp::{LUMP_LENGTH, geometry, lump_entry};
    use std::collections::HashMap;

    /// The share of boundary grid points that other displacements share (same x, y to the unit)
    /// whose heights are more than 2 units apart.
    fn edge_mismatch(ds: &[Displacement]) -> f64 {
        let mut at: HashMap<(i64, i64), Vec<f32>> = HashMap::new();
        for d in ds {
            for (k, p) in d.grid.iter().enumerate() {
                let (i, j) = (k / d.n, k % d.n);
                if [i, j].iter().any(|&v| v == 0 || v == d.n - 1) {
                    at.entry((p[0].round() as i64, p[1].round() as i64)).or_default().push(p[2]);
                }
            }
        }
        let shared: Vec<_> = at.values().filter(|z| z.len() > 1).collect();
        let off =
            shared.iter().filter(|z| z.iter().copied().fold(f32::MIN, f32::max) - z.iter().copied().fold(f32::MAX, f32::min) > 2.0).count();
        off as f64 / shared.len().max(1) as f64
    }

    #[test]
    fn swiftwater_terrain_is_at_its_real_height() {
        let Some(data) = swiftwater() else { return };
        // Terrain near (2260, -100), inside a cell rather than on its edge, sits at about -708; its base faces are at -548 and -480.
        // It is out of the players' reach, so its cells are looked at before the floors are filtered.
        let ds = displacements(&data, &World::read(&data).unwrap()).unwrap();
        let cells = ds.iter().flat_map(Displacement::cells);
        let zs: Vec<f32> = cells
            .filter(|c| contains(&c.iter().flat_map(|p| [f64::from(p[0]), f64::from(p[1])]).collect::<Vec<_>>(), 2260.0, -100.0))
            .map(|c| c.iter().map(|p| p[2]).sum::<f32>() / 4.0)
            .collect();
        assert!(!zs.is_empty() && zs.iter().all(|z| *z < -650.0), "{zs:?}");
        // Kept terrain near (3874, 378) sits at about -949; its base face is at -1028.
        let g = geometry(&data).unwrap();
        let zs: Vec<f64> = g.floors.iter().filter(|f| f.d && contains(&f.p, 3874.0, 378.0)).map(|f| f.z).collect();
        assert!(!zs.is_empty() && zs.iter().all(|z| *z > -990.0), "{zs:?}");
    }

    #[test]
    fn swiftwater_neighbouring_terrain_lines_up() {
        let Some(data) = swiftwater() else { return };
        let ds = displacements(&data, &World::read(&data).unwrap()).unwrap();
        // Grid edge points shared by two displacements mostly have the same height; the wrong
        // row/column orientation got ~51% mismatched, the right one ~26%.
        let share = edge_mismatch(&ds);
        eprintln!("edge mismatch {share}");
        assert!(share < 0.35, "{share}");
    }

    #[test]
    fn samples_are_no_further_apart_than_the_step() {
        let Some(data) = swiftwater() else { return };
        let ds = displacements(&data, &World::read(&data).unwrap()).unwrap();
        let d = ds.iter().max_by_key(|d| d.n).unwrap();
        assert!(d.samples(16.0).len() > d.grid.len());
        assert_eq!(d.samples(0.0).len(), d.grid.len());
    }

    #[test]
    fn samples_of_absurd_extents_are_bounded() {
        let n = 17;
        let grid = (0..n * n).map(|k| [(k / n) as f32 * 1e9, (k % n) as f32 * 1e9, 0.0]).collect::<Vec<_>>();
        let d = Displacement { plane: [0.0, 0.0, 1.0, 0.0], base: [[0.0; 3]; 4], n, grid, texture: 0, face: 0 };
        let len = d.samples(1.0).len();
        assert!(len <= 2 * MAX_SAMPLES, "{len}");
    }

    #[test]
    fn swiftwater_samples_leave_no_gaps_at_a_step_of_8() {
        let Some(data) = swiftwater() else { return };
        let ds = displacements(&data, &World::read(&data).unwrap()).unwrap();
        let reach = 8.0 * 2f32.sqrt() * 1.1;
        for d in &ds {
            let samples = d.samples(8.0);
            // Every sample has another within a diagonal step: look in the 3x3 buckets around it.
            let bucket = |p: &Vec3| [0, 1].map(|k| (p[k] / reach).floor() as i64);
            let mut at: HashMap<[i64; 2], Vec<usize>> = HashMap::new();
            for (i, p) in samples.iter().enumerate() {
                at.entry(bucket(p)).or_default().push(i);
            }
            for (i, p) in samples.iter().enumerate() {
                let [bx, by] = bucket(p);
                let near = (-1..=1).flat_map(|dx| (-1..=1).map(move |dy| [bx + dx, by + dy])).filter_map(|b| at.get(&b)).flatten();
                let nearest = near
                    .filter(|&&j| j != i)
                    .map(|&j| (0..3).map(|k| (p[k] - samples[j][k]).powi(2)).sum::<f32>().sqrt())
                    .fold(f32::MAX, f32::min);
                assert!(nearest <= reach, "face {}: {nearest} from {p:?}", d.face);
            }
        }
    }

    #[test]
    fn absurd_displacement_power_is_skipped_not_a_panic() {
        let Some(mut data) = swiftwater() else { return };
        // Every dispinfo gets power 30; the rewritten lump goes, unpacked, at the end.
        let mut info = lump(&data, LUMP_DISPINFO).unwrap();
        for rec in info.as_chunks_mut::<DISPINFO_SIZE>().0 {
            rec[DISPINFO_POWER..DISPINFO_POWER + 4].copy_from_slice(&30i32.to_le_bytes());
        }
        let h = lump_entry(LUMP_DISPINFO);
        let (ofs, len) = (data.len() as i32, info.len() as i32);
        data.extend_from_slice(&info);
        data[h..h + 4].copy_from_slice(&ofs.to_le_bytes());
        data[h + LUMP_LENGTH..h + LUMP_LENGTH + 4].copy_from_slice(&len.to_le_bytes());
        assert!(displacements(&data, &World::read(&data).unwrap()).unwrap().is_empty());
        assert!(geometry(&data).is_ok());
    }
}
