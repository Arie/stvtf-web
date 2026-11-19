//! A BSP read into its geometry: faces sorted into floors and walls.

use anyhow::{Result, anyhow, bail};
use stvtf::output::round1;

use super::entities::{Entities, cap_areas, packs, playable_bounds, spawns};
use super::floors::{fill, outline_area, without_shallow_water};
use super::format::{
    EDGE_SIZE, FACE_DISPINFO, FACE_FIRST_EDGE, FACE_NUM_EDGES, FACE_SIDE, FACE_SIZE, FACE_TEXINFO, LUMP_EDGES, LUMP_FACES, LUMP_PLANES,
    LUMP_SURFEDGES, LUMP_TEXDATA, LUMP_TEXDATA_STRING_DATA, LUMP_TEXDATA_STRING_TABLE, LUMP_TEXINFO, LUMP_VERTEXES, MODEL_FIRST_FACE,
    MODEL_NUM_FACES, SKIP_FLAGS, STRING_TABLE_ENTRY, SURFEDGE_SIZE, TEXDATA_NAME, TEXDATA_SIZE, TEXINFO_FLAGS, TEXINFO_SIZE,
    TEXINFO_TEXDATA, U16_SIZE, VERTEX_SIZE, bytes, i16_at, i32_at, index_at, lump, plane_at, u16_at, vec3_at,
};
use super::geometry::{Position, Vec3};
use super::material::{Kind, Tint, blended_with};
use super::{CapArea, Floor, Geometry, PAD, Wall};

/// A face is a floor when its normal points up at least this much (within about 45°), a wall
/// when it is within this of horizontal (within about 17° of upright).
pub(super) const FLOOR_MIN_NORMAL_Z: f32 = 0.7;

pub(super) const WALL_MAX_NORMAL_Z: f32 = 0.3;

/// Walls shorter than this (in units) are left out.
pub(super) const MIN_WALL_LENGTH: f32 = 2.0;

/// Spawn points stand up to this far above or below their room's box.
pub(super) const SPAWN_SLACK: f64 = 64.0;

/// The margin around the floors when no points or packs frame the map.
pub(super) const FLOOR_MARGIN: f32 = 256.0;

/// Texture names of tool brushes (clips, triggers, …), which are not drawn.
pub(super) const TOOL_TEXTURES: &str = "tools/";

/// A box: its lowest and highest corner.
#[derive(Copy, Clone, Debug)]
pub(super) struct Bounds {
    pub(super) lo: Vec3,
    pub(super) hi: Vec3,
}

impl Bounds {
    /// The box around `pts`, `pad` wider on every side.
    pub(super) fn around(pts: &[Vec3], pad: f32) -> Self {
        let mut b = Bounds { lo: [f32::MAX; 3], hi: [f32::MIN; 3] };
        for p in pts {
            for (i, v) in p.iter().enumerate() {
                b.lo[i] = b.lo[i].min(v - pad);
                b.hi[i] = b.hi[i].max(v + pad);
            }
        }
        b
    }

    pub(super) fn contains(&self, p: &Vec3) -> bool {
        (0..3).all(|i| self.lo[i] <= p[i] && p[i] <= self.hi[i])
    }

    pub(super) fn rounded(&self) -> (Position, Position) {
        (self.lo.map(round1), self.hi.map(round1))
    }
}

/// Each texture's colour, in the texture table's order.
fn texture_tints(data: &[u8]) -> Result<Vec<Option<Tint>>> {
    let texdata = lump(data, LUMP_TEXDATA)?;
    Ok((0..texdata.len() / TEXDATA_SIZE).map(|i| Tint::of(vec3_at(&texdata, i * TEXDATA_SIZE).ok()?)).collect())
}

pub(super) fn texture_names(data: &[u8]) -> Result<Vec<String>> {
    let table = lump(data, LUMP_TEXDATA_STRING_TABLE)?;
    let strings = lump(data, LUMP_TEXDATA_STRING_DATA)?;
    let texdata = lump(data, LUMP_TEXDATA)?;
    (0..texdata.len() / TEXDATA_SIZE)
        .map(|i| {
            let id = index_at(&texdata, i * TEXDATA_SIZE + TEXDATA_NAME)?;
            // A name missing from the tables is only used to skip tool textures, so it is left
            // empty.
            let start = index_at(&table, id * STRING_TABLE_ENTRY).unwrap_or(0);
            let s = strings.get(start..).unwrap_or_default();
            let s = &s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())];
            Ok(String::from_utf8_lossy(s).to_lowercase())
        })
        .collect()
}

/// The lumps the world's faces are read from.
pub(super) struct World {
    pub(super) planes: Vec<u8>,
    pub(super) verts: Vec<u8>,
    pub(super) texinfo: Vec<u8>,
    pub(super) faces: Vec<u8>,
    pub(super) edges: Vec<u8>,
    pub(super) surfedges: Vec<u8>,
    pub(super) textures: Vec<String>,
    pub(super) tints: Vec<Option<Tint>>,
}

/// A face of the world as seen from above: its corners, its plane (normal and distance, facing
/// out), its texture's index in the texture table and its displacement, if it has one.
pub(super) struct Face {
    pub(super) pts: Vec<Vec3>,
    pub(super) plane: [f32; 4],
    pub(super) texture: usize,
    pub(super) disp: Option<usize>,
}

impl World {
    pub(super) fn read(data: &[u8]) -> Result<Self> {
        Ok(World {
            planes: lump(data, LUMP_PLANES)?,
            verts: lump(data, LUMP_VERTEXES)?,
            texinfo: lump(data, LUMP_TEXINFO)?,
            faces: lump(data, LUMP_FACES)?,
            edges: lump(data, LUMP_EDGES)?,
            surfedges: lump(data, LUMP_SURFEDGES)?,
            textures: texture_names(data)?,
            tints: texture_tints(data)?,
        })
    }

    /// Face `f`, or None for one that is not drawn. `edges_left` is how many of the map's edges
    /// are not used yet: the world's faces each have their own, and a crafted map could reuse the
    /// same ones in every face to have this build gigabytes of points.
    pub(super) fn face(&self, f: usize, edges_left: &mut usize) -> Result<Option<Face>> {
        let o = f * FACE_SIZE;
        let (plane, [side]) = (usize::from(u16_at(&self.faces, o)?), bytes(&self.faces, o + FACE_SIDE)?);
        let first_edge = index_at(&self.faces, o + FACE_FIRST_EDGE)?;
        let num_edges = usize::from(u16_at(&self.faces, o + FACE_NUM_EDGES)?);
        let tex = i16_at(&self.faces, o + FACE_TEXINFO)?;
        *edges_left = edges_left.checked_sub(num_edges).ok_or_else(|| anyhow!("cannot place faces' edges: more than the map has"))?;
        let Ok(tex) = usize::try_from(tex) else { return Ok(None) };
        let t = tex * TEXINFO_SIZE;
        let (flags, texture) = (i32_at(&self.texinfo, t + TEXINFO_FLAGS)?, i32_at(&self.texinfo, t + TEXINFO_TEXDATA)? as usize);
        if flags & SKIP_FLAGS != 0 || self.textures.get(texture).is_some_and(|n| n.starts_with(TOOL_TEXTURES)) {
            return Ok(None);
        }
        let sign = if side == 0 { 1.0 } else { -1.0 };
        let plane = plane_at(&self.planes, plane)?.map(|v| sign * v);
        let disp = usize::try_from(i16_at(&self.faces, o + FACE_DISPINFO)?).ok();
        let pts = (first_edge..first_edge + num_edges)
            .map(|e| {
                let se = i32_at(&self.surfedges, e * SURFEDGE_SIZE)?;
                // A negative surfedge is the edge walked backwards: from its second vertex.
                let eo = se.unsigned_abs() as usize * EDGE_SIZE + if se >= 0 { 0 } else { U16_SIZE };
                vec3_at(&self.verts, usize::from(u16_at(&self.edges, eo)?) * VERTEX_SIZE)
            })
            .collect::<Result<_>>()?;
        Ok(Some(Face { pts, plane, texture, disp }))
    }

    /// The colour of texture `texture` where `blend` (0 to 1) of it is its second texture; None
    /// for one without, and for water and lava, which are drawn as what they are.
    pub(super) fn tint(&self, texture: usize, blend: f32) -> Option<Tint> {
        let first = self.tints.get(texture).copied().flatten().filter(|_| !matches!(self.kind(texture), Kind::Water | Kind::Lava))?;
        let second = self.textures.get(texture).and_then(|name| blended_with(name, first)).filter(|_| blend > 0.0);
        Some(second.map_or(first, |second| first.towards(second, blend.min(1.0))))
    }

    /// What texture `texture` is made of; Other for one not in the table.
    pub(super) fn kind(&self, texture: usize) -> Kind {
        self.textures.get(texture).map_or(Kind::Other, |t| super::material::kind(t))
    }
}

/// Points seen from above as an outline (`x, y, x, y, …`), to a tenth of a unit.
pub(super) fn outline(pts: &[Vec3]) -> Vec<f64> {
    pts.iter().flat_map(|p| [round1(p[0]), round1(p[1])]).collect()
}

/// How far up a cell of a displacement faces, as a unit normal's z, turned to the same side as
/// the base face's `plane`.
pub(super) fn cell_normal_z(cell: &[Vec3; 4], plane: &[f32; 4]) -> f32 {
    let n = cell_normal(cell);
    let len = n.iter().map(|c| c * c).sum::<f32>().sqrt();
    let side = if n[0] * plane[0] + n[1] * plane[1] + n[2] * plane[2] < 0.0 { -1.0 } else { 1.0 };
    side * n[2] / len
}

/// A cell's normal, unnormalised and facing either way: across its diagonals.
pub(super) fn cell_normal(cell: &[Vec3; 4]) -> Vec3 {
    let d = |a: usize, b: usize| [0, 1, 2].map(|i| cell[a][i] - cell[b][i]);
    cross(d(2, 0), d(3, 1))
}

/// `u` × `v`.
pub(in crate::maps) fn cross(u: Vec3, v: Vec3) -> Vec3 {
    [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]]
}

/// Gradients are kept to this many places: a unit off over 10 000 units.
pub(super) const GRADIENT_PLACES: f64 = 1e4;

/// How much a surface with normal `n` rises per unit along x and y, or None when it is flat.
pub(super) fn gradient(n: Vec3) -> Option<[f64; 2]> {
    let g = [0, 1].map(|i| (-f64::from(n[i]) / f64::from(n[2]) * GRADIENT_PLACES).round() / GRADIENT_PLACES);
    (g != [0.0, 0.0] && g.iter().all(|v| v.is_finite())).then_some(g)
}

/// A wall's top and ends seen from above, or None for one too short to draw.
pub(super) fn wall(pts: &[Vec3], c: Option<Tint>) -> Option<Wall> {
    // total_cmp: a broken map can have NaN coordinates.
    let key = |a: &&Vec3, b: &&Vec3| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1]));
    let lo = pts.iter().min_by(key)?;
    let hi = pts.iter().max_by(key)?;
    if (lo[0] - hi[0]).powi(2) + (lo[1] - hi[1]).powi(2) <= MIN_WALL_LENGTH.powi(2) {
        return None;
    }
    let top = pts.iter().map(|p| p[2]).fold(f32::MIN, f32::max);
    Some(Wall { z: round1(top), p: [round1(lo[0]), round1(lo[1]), round1(hi[0]), round1(hi[1])], c })
}

/// The map's spawns, packs, capture areas, playable bounds, floors (sorted from low to high) and
/// walls.
pub fn geometry(data: &[u8]) -> Result<Geometry> {
    geometry_counted(data).map(|g| g.0)
}

/// What blocks a camera's sight on the map (see [`super::reach::sight`]): of one with points or
/// pickups to frame it.
pub fn sight(data: &[u8]) -> Result<Vec<u8>> {
    if !data.starts_with(b"VBSP") {
        bail!("cannot read a file that is not VBSP");
    }
    let world = World::read(data)?;
    let Some(bounds) = playable_bounds(&Entities::read(data)?.list) else { bail!("cannot frame a map without points or pickups") };
    let disps = super::displacement::displacements(data, &world).unwrap_or_default();
    super::reach::sight(data, &disps, &bounds)
}

/// `geometry`, and how many floors there were, and their area, before the playable-area filter.
pub(super) fn geometry_counted(data: &[u8]) -> Result<(Geometry, (usize, f64))> {
    if !data.starts_with(b"VBSP") {
        bail!("cannot read a file that is not VBSP");
    }
    let world = World::read(data)?;
    let map = Entities::read(data)?;
    let bounds = playable_bounds(&map.list);
    let (spawns, packs, cap_areas, track) = (spawns(&map), packs(&map.list), cap_areas(&map), super::track::track(&map));

    // A displacement that cannot be read leaves its base face as it is.
    let disps = super::displacement::displacements(data, &world).unwrap_or_default();
    let Faces { floors, floor_pts, walls } = faces(&world, &map, bounds, &disps)?;
    // Where players stand: spawn points and packs.
    let stands: Vec<Position> = spawns.points.iter().map(|s| s.pos).chain(packs.iter().map(|p| p.pos)).collect();
    // Only maps with points or pickups: on the others (MGE, jump maps) the spawns are not where
    // the play is, so every floor is kept.
    let reach = bounds.and_then(|b| {
        // A map whose brushes cannot be read keeps every floor.
        super::reach::reach(data, &disps, &b, &reach_seeds(&stands, &cap_areas, &track)).unwrap_or_else(|e| {
            tracing::info!("playable area left out: {e:#}");
            None
        })
    });
    let mut floors: Vec<Floor> = floors.into_iter().map(|f| f.1).collect();
    let before_fill = (floors.len(), floors.iter().map(|f| outline_area(&f.p)).sum());
    let reach = reach.filter(|r| fill(&mut floors, |f| r.keeps(&f.p, f.z), &stands, before_fill.1));
    let floors = without_shallow_water(floors);
    let props = super::props::prop_floors(data, bounds, |x, y| reach.as_ref().is_none_or(|r| r.over(x, y)));
    // Without points or pickups, the floors frame the map; the top keeps the PAD above the
    // highest floor that the viewer's roof cut expects.
    let bounds = bounds.or_else(|| {
        let mut b = Bounds::around(&floor_pts, FLOOR_MARGIN);
        b.hi[2] += PAD - FLOOR_MARGIN;
        (!floor_pts.is_empty()).then_some(b)
    });
    let geometry = Geometry {
        spawns,
        packs,
        cap_areas,
        bounds: bounds.map(|b| [b.lo.map(f64::from), b.hi.map(f64::from)]),
        floors,
        props,
        walls,
        track: track.into_iter().map(|[a, b]| [a[0], a[1], a[2], b[0], b[1], b[2]].map(round1)).collect(),
        playable: reach.map(|r| r.mask()),
    };
    Ok((geometry, before_fill))
}

/// The world's faces sorted out: floors (with their height, from low to high), the floors'
/// points when there are no playable bounds to frame the map, and walls.
pub(super) struct Faces {
    pub(super) floors: Vec<(f32, Floor)>,
    pub(super) floor_pts: Vec<Vec3>,
    pub(super) walls: Vec<Wall>,
}

/// The world model's faces inside `bounds`: those facing up are floors, upright ones walls, and a
/// displacement's cells are each one or the other.
pub(super) fn faces(world: &World, map: &Entities, bounds: Option<Bounds>, disps: &[super::displacement::Displacement]) -> Result<Faces> {
    // Only the world model: brush entities (doors, triggers, func_brush) are separate models.
    let (first, count) = (index_at(&map.models, MODEL_FIRST_FACE)?, index_at(&map.models, MODEL_NUM_FACES)?);
    let mut floors: Vec<(f32, Floor)> = Vec::new();
    let mut floor_pts = Vec::new();
    let mut walls = Vec::new();
    let mut edges_left = world.surfedges.len() / SURFEDGE_SIZE;
    for f in first..first + count {
        let Some(face) = world.face(f, &mut edges_left)? else { continue };
        if face.pts.len() < 3 {
            continue;
        }
        let n = face.pts.len() as f32;
        let centre = [0, 1, 2].map(|i| face.pts.iter().map(|p| p[i]).sum::<f32>() / n);
        if bounds.is_some_and(|b| !b.contains(&centre)) {
            continue;
        }
        if let Some(d) = disps.iter().find(|d| d.face == f) {
            let k = world.kind(d.texture);
            for (cell, blend) in d.cells().zip(d.blends()) {
                let tint = world.tint(d.texture, blend);
                match cell_normal_z(&cell, &d.plane) {
                    z if z > FLOOR_MIN_NORMAL_Z => {
                        let height = cell.map(|p| p[2]).iter().sum::<f32>() / 4.0;
                        let slope = gradient(cell_normal(&cell));
                        floors.push((height, Floor { z: round1(height), p: outline(&cell), k, d: true, g: slope, c: tint }));
                        if bounds.is_none() {
                            floor_pts.extend(cell);
                        }
                    }
                    z if z.abs() < WALL_MAX_NORMAL_Z => walls.extend(wall(&cell, tint)),
                    _ => {}
                }
            }
        } else if face.plane[2] > FLOOR_MIN_NORMAL_Z {
            let g = gradient([face.plane[0], face.plane[1], face.plane[2]]);
            let (kind, tint) = (world.kind(face.texture), world.tint(face.texture, 0.0));
            floors.push((centre[2], Floor { z: round1(centre[2]), p: outline(&face.pts), k: kind, d: false, g, c: tint }));
            if bounds.is_none() {
                floor_pts.extend(face.pts);
            }
        } else if face.plane[2].abs() < WALL_MAX_NORMAL_Z {
            walls.extend(wall(&face.pts, world.tint(face.texture, 0.0)));
        }
    }
    floors.sort_by(|a, b| a.0.total_cmp(&b.0));
    Ok(Faces { floors, floor_pts, walls })
}

/// Where the flood fill of the playable area starts: where players stand, the middle of each
/// capture area's floor and the cart track.
pub(super) fn reach_seeds(stands: &[Position], cap_areas: &[CapArea], track: &[[Vec3; 2]]) -> Vec<Vec3> {
    let mut seeds: Vec<Vec3> = stands.iter().map(|p| p.map(|v| v as f32)).collect();
    seeds.extend(
        cap_areas.iter().map(|a| [f64::midpoint(a.min[0], a.max[0]), f64::midpoint(a.min[1], a.max[1]), a.min[2]].map(|v| v as f32)),
    );
    seeds.extend(track.iter().flatten());
    seeds
}
