//! Top-down geometry from a Source BSP: walkable floors and wall outlines.
//!
//! Control points, pickups and nearby spawns frame the playable area, which keeps
//! the 3D skybox out of the picture.

use anyhow::{Result, anyhow, bail};
use material::Kind;
use serde::Serialize;
use stvtf::output::{PickupItem, PickupSize, Team, round1};

mod displacement;
mod material;
mod track;

const LUMP_ENTITIES: usize = 0;
const LUMP_PLANES: usize = 1;
const LUMP_TEXDATA: usize = 2;
const LUMP_VERTEXES: usize = 3;
const LUMP_TEXINFO: usize = 6;
const LUMP_FACES: usize = 7;
const LUMP_EDGES: usize = 12;
const LUMP_SURFEDGES: usize = 13;
const LUMP_MODELS: usize = 14;
const LUMP_DISPINFO: usize = 26;
const LUMP_DISP_VERTS: usize = 33;
const LUMP_TEXDATA_STRING_DATA: usize = 43;
const LUMP_TEXDATA_STRING_TABLE: usize = 44;

/// `SURF_*` texture flags of faces that are not drawn as part of the world.
const SURF_SKY2D: i32 = 0x2;
const SURF_SKY: i32 = 0x4;
const SURF_TRIGGER: i32 = 0x40;
const SURF_NODRAW: i32 = 0x80;
const SURF_HINT: i32 = 0x100;
const SURF_SKIP: i32 = 0x200;
const SKIP_FLAGS: i32 = SURF_SKY2D | SURF_SKY | SURF_TRIGGER | SURF_NODRAW | SURF_HINT | SURF_SKIP;
/// Room around the points, packs and spawns that frame the playable area.
const PAD: f32 = 1536.0;
// The lumps read here are under 2 MB on real maps; a crafted one could otherwise claim 512 MB
// unpacked for each, from a few KB of LZMA.
const MAX_LUMP: usize = 16 << 20;
/// The lump directory follows the `VBSP` id and version: offset, length, version and id per lump.
const LUMP_DIRECTORY: usize = 8;
const LUMP_ENTRY: usize = 16;
const LUMP_OFFSET: usize = 0;
const LUMP_LENGTH: usize = 4;
/// Valve's LZMA lump header: `LZMA`, actual size (u32), packed size (u32), 5 property bytes.
const LZMA_MAGIC: &[u8] = b"LZMA";
const LZMA_ACTUAL_SIZE: usize = 4;
const LZMA_PROPERTIES: usize = 12;
const LZMA_HEADER: usize = 17;
/// The sizes of the fields read.
const F32_SIZE: usize = 4;
const U16_SIZE: usize = 2;
/// Record sizes and field offsets of the lumps read (`dplane_t`, `dface_t`, …).
const PLANE_SIZE: usize = 20;
const VERTEX_SIZE: usize = 12;
const EDGE_SIZE: usize = 4;
const SURFEDGE_SIZE: usize = 4;
const TEXINFO_SIZE: usize = 72;
const TEXINFO_FLAGS: usize = 64;
const TEXINFO_TEXDATA: usize = 68;
const TEXDATA_SIZE: usize = 32;
const TEXDATA_NAME: usize = 12;
const STRING_TABLE_ENTRY: usize = 4;
const FACE_SIZE: usize = 56;
const FACE_SIDE: usize = 2;
const FACE_FIRST_EDGE: usize = 4;
const FACE_NUM_EDGES: usize = 8;
const FACE_TEXINFO: usize = 10;
const FACE_DISPINFO: usize = 12;
const PLANE_DIST: usize = 12;
const MODEL_SIZE: usize = 48;
const MODEL_MINS: usize = 0;
const MODEL_MAXS: usize = 12;
const MODEL_FIRST_FACE: usize = 40;
const MODEL_NUM_FACES: usize = 44;
/// A face is a floor when its normal points up at least this much (within about 45°), a wall
/// when it is within this of horizontal (within about 17° of upright).
const FLOOR_MIN_NORMAL_Z: f32 = 0.7;
const WALL_MAX_NORMAL_Z: f32 = 0.3;
/// Walls shorter than this (in units) are left out.
const MIN_WALL_LENGTH: f32 = 2.0;
/// Spawn points stand up to this far above or below their room's box.
const SPAWN_SLACK: f64 = 64.0;
/// Spawns frame the map when they are this near the points and packs.
const SPAWN_REACH: f32 = 4096.0;
/// The margin around the floors when no points or packs frame the map.
const FLOOR_MARGIN: f32 = 256.0;
/// Texture names of tool brushes (clips, triggers, …), which are not drawn.
const TOOL_TEXTURES: &str = "tools/";
/// `TeamNum` of an entity on each team.
const TEAM_RED: &str = "2";
const TEAM_BLUE: &str = "3";

type Vec3 = [f32; 3];
/// A position as served, to a tenth of a unit.
type Position = [f64; 3];

/// A map's top-down geometry, as `/map/NAME` serves it.
#[derive(Debug, Serialize)]
pub struct Geometry {
    pub spawns: Spawns,
    pub packs: Vec<Pack>,
    pub cap_areas: Vec<CapArea>,
    /// The playable area: lowest and highest corner, unrounded (widened from f32, as served before).
    pub bounds: Option<[[f64; 3]; 2]>,
    /// Upward faces, from low to high.
    pub floors: Vec<Floor>,
    pub walls: Vec<Wall>,
    /// The payload track's segments (`x1, y1, z1, x2, y2, z2`); empty without a payload cart.
    pub track: Vec<[f64; 6]>,
}

/// An upward face: its height, outline (`x, y, x, y, …`) and what it is made of.
#[derive(Debug, Serialize)]
pub struct Floor {
    pub z: f64,
    pub p: Vec<f64>,
    pub k: Kind,
    /// A cell of a displacement (terrain), drawn at its real height.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub d: bool,
}

/// A wall seen from above: its top and its two ends (`x1, y1, x2, y2`).
#[derive(Debug, Serialize)]
pub struct Wall {
    pub z: f64,
    pub p: [f64; 4],
}

/// A health or ammo pack.
#[derive(Debug, Serialize)]
pub struct Pack {
    pub kind: PickupItem,
    pub size: PickupSize,
    pub pos: Position,
}

/// A capture area's bounds, and the point it captures (by the game's index).
#[derive(Debug, Serialize)]
pub struct CapArea {
    pub point: Option<i64>,
    pub min: Position,
    pub max: Position,
}

#[derive(Debug, Serialize)]
pub struct Spawns {
    pub rooms: Vec<SpawnRoom>,
    pub points: Vec<SpawnPoint>,
}

/// A spawn room (`func_respawnroom`).
#[derive(Debug, Serialize)]
pub struct SpawnRoom {
    pub name: String,
    pub team: Team,
    pub min: Position,
    pub max: Position,
    pub disabled: bool,
}

impl SpawnRoom {
    /// Whether `p` is in the room, with some room above and below for where spawn points stand.
    fn holds(&self, p: Position) -> bool {
        (0..3).all(|k| {
            let slack = if k == 2 { SPAWN_SLACK } else { 0.0 };
            p[k] >= self.min[k] - slack && p[k] <= self.max[k] + slack
        })
    }

    fn centre(&self) -> Position {
        [0, 1, 2].map(|k| f64::midpoint(self.min[k], self.max[k]))
    }
}

/// A spawn point (`info_player_teamspawn`), with the control point it belongs to.
#[derive(Debug, Serialize)]
pub struct SpawnPoint {
    pub team: Team,
    pub pos: Position,
    pub point: Option<i64>,
    pub disabled: bool,
}

/// A box: its lowest and highest corner.
#[derive(Debug, Clone, Copy)]
struct Bounds {
    lo: Vec3,
    hi: Vec3,
}

impl Bounds {
    /// The box around `pts`, `pad` wider on every side.
    fn around(pts: &[Vec3], pad: f32) -> Self {
        let mut b = Bounds { lo: [f32::MAX; 3], hi: [f32::MIN; 3] };
        for p in pts {
            for (i, v) in p.iter().enumerate() {
                b.lo[i] = b.lo[i].min(v - pad);
                b.hi[i] = b.hi[i].max(v + pad);
            }
        }
        b
    }

    fn contains(&self, p: &Vec3) -> bool {
        (0..3).all(|i| self.lo[i] <= p[i] && p[i] <= self.hi[i])
    }

    fn rounded(&self) -> (Position, Position) {
        (self.lo.map(round1), self.hi.map(round1))
    }
}

// Maps come from fastdl and can be broken, so every read is checked: a panic would take down
// the request and poison the lock around map loading.
fn bytes<const N: usize>(b: &[u8], o: usize) -> Result<[u8; N]> {
    o.checked_add(N).and_then(|end| b.get(o..end)).and_then(|s| s.try_into().ok()).ok_or_else(|| anyhow!("BSP truncated at {o}"))
}
fn u16_at(b: &[u8], o: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(bytes(b, o)?))
}
fn i16_at(b: &[u8], o: usize) -> Result<i16> {
    Ok(i16::from_le_bytes(bytes(b, o)?))
}
fn i32_at(b: &[u8], o: usize) -> Result<i32> {
    Ok(i32::from_le_bytes(bytes(b, o)?))
}
fn f32_at(b: &[u8], o: usize) -> Result<f32> {
    Ok(f32::from_le_bytes(bytes(b, o)?))
}
/// Three floats in a row: a point or a direction.
fn vec3_at(b: &[u8], o: usize) -> Result<Vec3> {
    Ok([f32_at(b, o)?, f32_at(b, o + F32_SIZE)?, f32_at(b, o + 2 * F32_SIZE)?])
}
/// An offset, count or index stored as an i32; negative ones are an error.
fn index_at(b: &[u8], o: usize) -> Result<usize> {
    let v = i32_at(b, o)?;
    usize::try_from(v).map_err(|_| anyhow!("negative index {v} at {o}"))
}

/// Where lump `index`'s entry is in the lump directory.
fn lump_entry(index: usize) -> usize {
    LUMP_DIRECTORY + LUMP_ENTRY * index
}

fn lump(data: &[u8], index: usize) -> Result<Vec<u8>> {
    let h = lump_entry(index);
    let (ofs, len) = (index_at(data, h + LUMP_OFFSET)?, index_at(data, h + LUMP_LENGTH)?);
    let Some(raw) = data.get(ofs..ofs + len) else { bail!("lump {index} out of range") };
    if len > MAX_LUMP {
        bail!("lump {index}: {len} bytes");
    }
    if !raw.starts_with(LZMA_MAGIC) {
        return Ok(raw.to_vec());
    }
    let actual = u32::from_le_bytes(bytes(raw, LZMA_ACTUAL_SIZE)?) as usize;
    if actual > MAX_LUMP {
        bail!("lump {index}: {actual} bytes unpacked");
    }
    let props: [u8; 5] = bytes(raw, LZMA_PROPERTIES)?;
    let mut alone = props.to_vec();
    alone.extend_from_slice(&(actual as u64).to_le_bytes());
    alone.extend_from_slice(&raw[LZMA_HEADER..]);
    let mut out = Vec::with_capacity(actual);
    lzma_rs::lzma_decompress(&mut &alone[..], &mut out).map_err(|e| anyhow!("lump {index}: {e:?}"))?;
    out.truncate(actual);
    Ok(out)
}

/// One entity of the entity lump: its keys and values.
struct Entity(Vec<(String, String)>);

impl Entity {
    /// The value of `key`; Hammer keys are not case-sensitive.
    fn get(&self, key: &str) -> Option<&str> {
        self.0.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())
    }

    fn class(&self) -> &str {
        self.get("classname").unwrap_or_default()
    }

    fn origin(&self) -> Option<Vec3> {
        let v: Vec<f32> = self.get("origin")?.split_whitespace().filter_map(|s| s.parse().ok()).collect();
        <Vec3>::try_from(v.as_slice()).ok()
    }

    fn team(&self) -> Team {
        match self.get("TeamNum") {
            Some(TEAM_RED) => Team::Red,
            Some(TEAM_BLUE) => Team::Blue,
            _ => Team::Unassigned,
        }
    }

    fn start_disabled(&self) -> bool {
        self.get("StartDisabled") == Some("1")
    }
}

fn entities(data: &[u8]) -> Result<Vec<Entity>> {
    let text = String::from_utf8_lossy(&lump(data, LUMP_ENTITIES)?).into_owned();
    let mut out = Vec::new();
    for block in text.split('{').skip(1) {
        let body = block.split('}').next().unwrap_or("");
        let quoted: Vec<&str> = body.split('"').skip(1).step_by(2).collect();
        out.push(Entity(quoted.chunks(2).filter(|kv| kv.len() == 2).map(|kv| (kv[0].to_string(), kv[1].to_string())).collect()));
    }
    Ok(out)
}

/// What the spawns, packs and capture areas come from: the entities, the brush models some of
/// them are, and the game's index of each control point.
struct Entities {
    list: Vec<Entity>,
    models: Vec<u8>,
    /// `team_control_point` targetname -> the point's index in the game, which is its rank by
    /// `point_index` (`cp_gullywash` numbers its points from 1).
    points: Vec<(String, i64)>,
}

impl Entities {
    fn read(data: &[u8]) -> Result<Self> {
        let list = entities(data)?;
        let mut points: Vec<(String, i64)> = list
            .iter()
            .filter(|e| e.class() == "team_control_point")
            .filter_map(|e| Some((e.get("targetname")?.to_string(), e.get("point_index")?.parse().ok()?)))
            .collect();
        points.sort_by_key(|p| p.1);
        let mut ranks: Vec<i64> = points.iter().map(|p| p.1).collect();
        ranks.dedup();
        let points = points.into_iter().map(|(name, i)| (name, ranks.iter().position(|&r| r == i).unwrap_or_default() as i64)).collect();
        Ok(Entities { list, models: lump(data, LUMP_MODELS)?, points })
    }

    fn of_class<'a>(&'a self, class: &'a str) -> impl Iterator<Item = &'a Entity> {
        self.list.iter().filter(move |e| e.class() == class)
    }

    /// The control point a key of `e` names, by the game's index.
    fn point(&self, e: &Entity, key: &str) -> Option<i64> {
        let target = e.get(key)?;
        self.points.iter().find(|(name, _)| name.eq_ignore_ascii_case(target)).map(|p| p.1)
    }

    /// A brush entity's bounds: its model's box moved by its origin.
    fn brush_bounds(&self, e: &Entity) -> Option<Bounds> {
        let m = usize::from(e.get("model")?.strip_prefix('*')?.parse::<u16>().ok()?);
        let model = self.models.get(m * MODEL_SIZE..(m + 1) * MODEL_SIZE)?;
        let origin = e.origin().unwrap_or([0.0; 3]);
        let corner = |o: usize| -> Option<Vec3> {
            let v = vec3_at(model, o).ok()?;
            Some([0, 1, 2].map(|i| origin[i] + v[i]))
        };
        Some(Bounds { lo: corner(MODEL_MINS)?, hi: corner(MODEL_MAXS)? })
    }
}

/// Points, pickups and the spawns near them; maps also hide unused spawns far away. None for a
/// map without points or pickups (MGE, jump maps): its spawns frame no more than a spawn room.
fn playable_bounds(ents: &[Entity]) -> Option<Bounds> {
    let (mut core, mut spawns) = (Vec::new(), Vec::new());
    for ent in ents {
        let Some(p) = ent.origin() else { continue };
        let class = ent.class();
        if class == "team_control_point" || pack_of(class).is_some() {
            core.push(p);
        } else if class == "info_player_teamspawn" {
            spawns.push(p);
        }
    }
    if core.is_empty() {
        return None;
    }
    let near = Bounds::around(&core, SPAWN_REACH);
    core.extend(spawns.into_iter().filter(|s| near.contains(s)));
    Some(Bounds::around(&core, PAD))
}

/// A health or ammo pack's kind and its size's name, from its class.
fn pack_of(class: &str) -> Option<(PickupItem, &str)> {
    class
        .strip_prefix("item_healthkit_")
        .map(|s| (PickupItem::Health, s))
        .or_else(|| class.strip_prefix("item_ammopack_").map(|s| (PickupItem::Ammo, s)))
}

/// Health and ammo packs by size.
fn packs(ents: &[Entity]) -> Vec<Pack> {
    ents.iter()
        .filter_map(|e| {
            let (kind, size) = pack_of(e.class())?;
            let size = match size {
                "small" => PickupSize::Small,
                "medium" => PickupSize::Medium,
                "full" => PickupSize::Full,
                _ => return None,
            };
            Some(Pack { kind, size, pos: e.origin()?.map(round1) })
        })
        .collect()
}

/// Capture areas (`trigger_capture_area`: their brush model's bounds) and the point each caps.
/// Payload ones are parented to the cart and move with it, so their bounds are just its start.
fn cap_areas(map: &Entities) -> Vec<CapArea> {
    map.of_class("trigger_capture_area")
        .filter(|e| e.get("parentname").is_none_or(str::is_empty))
        .filter_map(|e| {
            let (min, max) = map.brush_bounds(e)?.rounded();
            Some(CapArea { point: map.point(e, "area_cap_point"), min, max })
        })
        .collect()
}

/// Spawn rooms (`func_respawnroom`: their brush model's bounds) and spawn points
/// (`info_player_teamspawn`), with the control point each spawn point belongs to: TF2 enables
/// such a spawn point for the team that owns that point.
fn spawns(map: &Entities) -> Spawns {
    let mut rooms: Vec<SpawnRoom> = map
        .of_class("func_respawnroom")
        .filter_map(|e| {
            let (min, max) = map.brush_bounds(e)?.rounded();
            let name = e.get("targetname").unwrap_or_default().to_string();
            Some(SpawnRoom { name, team: e.team(), min, max, disabled: e.start_disabled() })
        })
        .collect();
    let points: Vec<SpawnPoint> = map
        .of_class("info_player_teamspawn")
        // Team-less spawn points are unused leftovers (on cp_process far outside the map).
        .filter(|e| e.team() != Team::Unassigned)
        .filter_map(|e| {
            Some(SpawnPoint {
                team: e.team(),
                pos: e.origin()?.map(round1),
                point: map.point(e, "controlpoint"),
                disabled: e.start_disabled(),
            })
        })
        .collect();
    give_rooms_teams(&mut rooms, &points);
    Spawns { rooms, points }
}

/// Some maps (`pl_swiftwater`) give a room its team by input: take the team of the spawn points
/// inside, else of the room around its centre.
fn give_rooms_teams(rooms: &mut [SpawnRoom], points: &[SpawnPoint]) {
    for by_points in [true, false] {
        for i in 0..rooms.len() {
            if rooms[i].team != Team::Unassigned {
                continue;
            }
            let found = if by_points {
                let teams: Vec<Team> = points.iter().filter(|p| rooms[i].holds(p.pos)).map(|p| p.team).collect();
                teams.first().copied().filter(|t| teams.iter().all(|u| u == t))
            } else {
                let centre = rooms[i].centre();
                rooms.iter().find(|o| o.team != Team::Unassigned && o.holds(centre)).map(|o| o.team)
            };
            if let Some(team) = found {
                rooms[i].team = team;
            }
        }
    }
}

fn texture_names(data: &[u8]) -> Result<Vec<String>> {
    let table = lump(data, LUMP_TEXDATA_STRING_TABLE)?;
    let strings = lump(data, LUMP_TEXDATA_STRING_DATA)?;
    let texdata = lump(data, LUMP_TEXDATA)?;
    (0..texdata.len() / TEXDATA_SIZE)
        .map(|i| {
            let id = index_at(&texdata, i * TEXDATA_SIZE + TEXDATA_NAME)?;
            // A name missing from the tables is only used to skip tool textures, so it is left empty.
            let start = index_at(&table, id * STRING_TABLE_ENTRY).unwrap_or(0);
            let s = strings.get(start..).unwrap_or_default();
            let s = &s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())];
            Ok(String::from_utf8_lossy(s).to_lowercase())
        })
        .collect()
}

/// The lumps the world's faces are read from.
struct World {
    planes: Vec<u8>,
    verts: Vec<u8>,
    texinfo: Vec<u8>,
    faces: Vec<u8>,
    edges: Vec<u8>,
    surfedges: Vec<u8>,
    textures: Vec<String>,
}

/// A face of the world as seen from above: its corners, its plane (normal and distance, facing
/// out), how far its normal points up, its texture's index in the texture table and its
/// displacement, if it has one.
struct Face {
    pts: Vec<Vec3>,
    plane: [f32; 4],
    normal_z: f32,
    texture: usize,
    disp: Option<usize>,
}

impl World {
    fn read(data: &[u8]) -> Result<Self> {
        Ok(World {
            planes: lump(data, LUMP_PLANES)?,
            verts: lump(data, LUMP_VERTEXES)?,
            texinfo: lump(data, LUMP_TEXINFO)?,
            faces: lump(data, LUMP_FACES)?,
            edges: lump(data, LUMP_EDGES)?,
            surfedges: lump(data, LUMP_SURFEDGES)?,
            textures: texture_names(data)?,
        })
    }

    /// Face `f`, or None for one that is not drawn. `edges_left` is how many of the map's edges
    /// are not used yet: the world's faces each have their own, and a crafted map could reuse the
    /// same ones in every face to have this build gigabytes of points.
    fn face(&self, f: usize, edges_left: &mut usize) -> Result<Option<Face>> {
        let o = f * FACE_SIZE;
        let (plane, [side]) = (usize::from(u16_at(&self.faces, o)?), bytes(&self.faces, o + FACE_SIDE)?);
        let first_edge = index_at(&self.faces, o + FACE_FIRST_EDGE)?;
        let num_edges = usize::from(u16_at(&self.faces, o + FACE_NUM_EDGES)?);
        let tex = i16_at(&self.faces, o + FACE_TEXINFO)?;
        *edges_left = edges_left.checked_sub(num_edges).ok_or_else(|| anyhow!("faces use more edges than the map has"))?;
        let Ok(tex) = usize::try_from(tex) else { return Ok(None) };
        let t = tex * TEXINFO_SIZE;
        let (flags, texture) = (i32_at(&self.texinfo, t + TEXINFO_FLAGS)?, i32_at(&self.texinfo, t + TEXINFO_TEXDATA)? as usize);
        if flags & SKIP_FLAGS != 0 || self.textures.get(texture).is_some_and(|n| n.starts_with(TOOL_TEXTURES)) {
            return Ok(None);
        }
        let sign = if side == 0 { 1.0 } else { -1.0 };
        let normal = vec3_at(&self.planes, plane * PLANE_SIZE)?;
        let plane = [sign * normal[0], sign * normal[1], sign * normal[2], sign * f32_at(&self.planes, plane * PLANE_SIZE + PLANE_DIST)?];
        let normal_z = plane[2];
        let disp = usize::try_from(i16_at(&self.faces, o + FACE_DISPINFO)?).ok();
        let pts = (first_edge..first_edge + num_edges)
            .map(|e| {
                let se = i32_at(&self.surfedges, e * SURFEDGE_SIZE)?;
                // A negative surfedge is the edge walked backwards: from its second vertex.
                let eo = se.unsigned_abs() as usize * EDGE_SIZE + if se >= 0 { 0 } else { U16_SIZE };
                vec3_at(&self.verts, usize::from(u16_at(&self.edges, eo)?) * VERTEX_SIZE)
            })
            .collect::<Result<_>>()?;
        Ok(Some(Face { pts, plane, normal_z, texture, disp }))
    }
}

/// How far up a cell of a displacement faces, as a unit normal's z, turned to the same side as
/// the base face's `plane`.
fn cell_normal_z(cell: &[Vec3; 4], plane: &[f32; 4]) -> f32 {
    let d = |a: usize, b: usize| [0, 1, 2].map(|i| cell[a][i] - cell[b][i]);
    let (u, v) = (d(2, 0), d(3, 1));
    let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
    let len = n.iter().map(|c| c * c).sum::<f32>().sqrt();
    let side = if n[0] * plane[0] + n[1] * plane[1] + n[2] * plane[2] < 0.0 { -1.0 } else { 1.0 };
    side * n[2] / len
}

/// A wall's top and ends seen from above, or None for one too short to draw.
fn wall(pts: &[Vec3]) -> Option<Wall> {
    // total_cmp: a broken map can have NaN coordinates.
    let key = |a: &&Vec3, b: &&Vec3| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1]));
    let lo = pts.iter().min_by(key)?;
    let hi = pts.iter().max_by(key)?;
    if (lo[0] - hi[0]).powi(2) + (lo[1] - hi[1]).powi(2) <= MIN_WALL_LENGTH.powi(2) {
        return None;
    }
    let top = pts.iter().map(|p| p[2]).fold(f32::MIN, f32::max);
    Some(Wall { z: round1(top), p: [round1(lo[0]), round1(lo[1]), round1(hi[0]), round1(hi[1])] })
}

/// The map's spawns, packs, capture areas, playable bounds, floors (sorted from low to high) and
/// walls.
pub fn geometry(data: &[u8]) -> Result<Geometry> {
    if !data.starts_with(b"VBSP") {
        bail!("not a VBSP file");
    }
    let world = World::read(data)?;
    let map = Entities::read(data)?;
    let bounds = playable_bounds(&map.list);

    // Only the world model: brush entities (doors, triggers, func_brush) are separate models.
    let (first, count) = (index_at(&map.models, MODEL_FIRST_FACE)?, index_at(&map.models, MODEL_NUM_FACES)?);
    let mut floors: Vec<(f32, Floor)> = Vec::new();
    let mut floor_pts = Vec::new();
    let mut walls = Vec::new();
    let mut edges_left = world.surfedges.len() / SURFEDGE_SIZE;
    // A displacement that cannot be read leaves its base face as it is.
    let disps = displacement::displacements(data, &world).unwrap_or_default();
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
            for cell in d.cells() {
                let kind = world.textures.get(d.texture).map_or(Kind::Other, |t| material::kind(t));
                let c = cell.map(|p| p[2]).iter().sum::<f32>() / 4.0;
                match cell_normal_z(&cell, &d.plane) {
                    z if z > FLOOR_MIN_NORMAL_Z => {
                        let p: Vec<f64> = cell.iter().flat_map(|p| [round1(p[0]), round1(p[1])]).collect();
                        floors.push((c, Floor { z: round1(c), p, k: kind, d: true }));
                        if bounds.is_none() {
                            floor_pts.extend(cell);
                        }
                    }
                    z if z.abs() < WALL_MAX_NORMAL_Z => walls.extend(wall(&cell)),
                    _ => {}
                }
            }
        } else if face.normal_z > FLOOR_MIN_NORMAL_Z {
            let p: Vec<f64> = face.pts.iter().flat_map(|p| [round1(p[0]), round1(p[1])]).collect();
            let k = world.textures.get(face.texture).map_or(Kind::Other, |t| material::kind(t));
            floors.push((centre[2], Floor { z: round1(centre[2]), p, k, d: false }));
            if bounds.is_none() {
                floor_pts.extend(face.pts);
            }
        } else if face.normal_z.abs() < WALL_MAX_NORMAL_Z {
            walls.extend(wall(&face.pts));
        }
    }
    floors.sort_by(|a, b| a.0.total_cmp(&b.0));
    // Without points or pickups, the floors frame the map; the top keeps the PAD above the
    // highest floor that the viewer's roof cut expects.
    let bounds = bounds.or_else(|| {
        let mut b = Bounds::around(&floor_pts, FLOOR_MARGIN);
        b.hi[2] += PAD - FLOOR_MARGIN;
        (!floor_pts.is_empty()).then_some(b)
    });
    Ok(Geometry {
        spawns: spawns(&map),
        packs: packs(&map.list),
        cap_areas: cap_areas(&map),
        bounds: bounds.map(|b| [b.lo.map(f64::from), b.hi.map(f64::from)]),
        floors: floors.into_iter().map(|f| f.1).collect(),
        walls,
        track: track::track(&map).into_iter().map(|[a, b]| [a[0], a[1], a[2], b[0], b[1], b[2]].map(round1)).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The map `name` from the cache, or None (said on stderr) when it is not there.
    pub(super) fn map(name: &str) -> Option<Vec<u8>> {
        let data = std::fs::read(format!("{}/cache/{name}.bsp", env!("CARGO_MANIFEST_DIR"))).ok();
        if data.is_none() {
            eprintln!("skipping: cache/{name}.bsp is missing");
        }
        data
    }

    pub(super) fn process() -> Option<Vec<u8>> {
        map("cp_process_f12")
    }

    pub(super) fn swiftwater() -> Option<Vec<u8>> {
        map("pl_swiftwater_final1")
    }

    /// Whether (x, y) is inside the outline `p` (`x, y, x, y, …`).
    pub(super) fn contains(p: &[f64], x: f64, y: f64) -> bool {
        let n = p.len() / 2;
        let at = |i: usize| (p[i % n * 2], p[i % n * 2 + 1]);
        (0..n).fold(false, |inside, i| {
            let ((x1, y1), (x2, y2)) = (at(i), at(i + 1));
            inside ^ ((y1 > y) != (y2 > y) && x < (x2 - x1) * (y - y1) / (y2 - y1) + x1)
        })
    }

    #[test]
    fn rejects_non_bsp() {
        assert!(geometry(b"not a map").is_err());
    }

    /// The lumps a VBSP header has.
    const LUMPS: usize = 64;

    /// A VBSP header with every lump empty.
    fn empty_bsp() -> Vec<u8> {
        let mut b = b"VBSP".to_vec();
        b.extend_from_slice(&20i32.to_le_bytes());
        b.resize(lump_entry(LUMPS) + 4, 0);
        b
    }

    fn set_i32(b: &mut [u8], o: usize, v: i32) {
        b[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }

    // Broken maps must be an error, not a panic: a panic while serving `/map/NAME` used to
    // poison the lock around map loading and break every later map request.
    #[test]
    fn truncated_header_is_an_error() {
        assert!(geometry(b"VBSP").is_err());
        assert!(geometry(&empty_bsp()[..100]).is_err());
    }

    #[test]
    fn empty_lumps_are_an_error() {
        assert!(geometry(&empty_bsp()).is_err());
    }

    #[test]
    fn lump_out_of_range_is_an_error() {
        for (ofs, len) in [(-1, 4), (4, -1), (i32::MAX, i32::MAX), (1000, 1_000_000)] {
            let mut b = empty_bsp();
            set_i32(&mut b, lump_entry(LUMP_MODELS), ofs);
            set_i32(&mut b, lump_entry(LUMP_MODELS) + LUMP_LENGTH, len);
            assert!(geometry(&b).is_err(), "ofs {ofs} len {len}");
        }
    }

    #[test]
    fn lzma_lump_with_huge_size_is_an_error() {
        let mut b = empty_bsp();
        let ofs = b.len();
        b.extend_from_slice(b"LZMA");
        b.extend_from_slice(&u32::MAX.to_le_bytes());
        b.extend_from_slice(&[0; 9]);
        set_i32(&mut b, lump_entry(LUMP_PLANES), ofs as i32);
        set_i32(&mut b, lump_entry(LUMP_PLANES) + LUMP_LENGTH, LZMA_HEADER as i32);
        assert!(geometry(&b).is_err());
    }

    #[test]
    fn lump_over_the_limit_is_an_error() {
        let mut b = empty_bsp();
        let ofs = b.len();
        b.resize(ofs + MAX_LUMP + 1, 0);
        set_i32(&mut b, lump_entry(LUMP_PLANES), ofs as i32);
        set_i32(&mut b, lump_entry(LUMP_PLANES) + LUMP_LENGTH, (MAX_LUMP + 1) as i32);
        assert!(lump(&b, LUMP_PLANES).unwrap_err().to_string().contains("bytes"));
    }

    #[test]
    fn faces_sharing_edges_are_an_error() {
        let Some(mut data) = process() else { return };
        // Every face claims the same 65535 edges; the rewritten lump goes, unpacked, at the end.
        let mut faces = lump(&data, LUMP_FACES).unwrap();
        for face in faces.as_chunks_mut::<FACE_SIZE>().0 {
            face[FACE_FIRST_EDGE..FACE_NUM_EDGES].fill(0);
            face[FACE_NUM_EDGES..FACE_TEXINFO].copy_from_slice(&u16::MAX.to_le_bytes());
        }
        let h = lump_entry(LUMP_FACES);
        let (ofs, len) = (data.len() as i32, faces.len() as i32);
        data.extend_from_slice(&faces);
        set_i32(&mut data, h, ofs);
        set_i32(&mut data, h + LUMP_LENGTH, len);
        assert!(geometry(&data).unwrap_err().to_string().contains("more edges"));
    }

    #[test]
    fn process_truncated_or_corrupted_is_not_a_panic() {
        let Some(data) = process() else { return };
        for n in [9, 500, 1036, data.len() / 3, data.len() / 2, data.len() - 1] {
            let _ = geometry(&data[..n]);
        }
        for index in [
            LUMP_PLANES,
            LUMP_TEXDATA,
            LUMP_VERTEXES,
            LUMP_TEXINFO,
            LUMP_FACES,
            LUMP_EDGES,
            LUMP_SURFEDGES,
            LUMP_MODELS,
            LUMP_DISPINFO,
            LUMP_DISP_VERTS,
        ] {
            let h = lump_entry(index);
            let (ofs, len) = (index_at(&data, h + LUMP_OFFSET).unwrap(), index_at(&data, h + LUMP_LENGTH).unwrap());
            // Too short, and the contents overwritten (out of range indices, NaN coordinates).
            let mut b = data.clone();
            set_i32(&mut b, h + LUMP_LENGTH, (len / 2) as i32);
            let _ = geometry(&b);
            let mut b = data.clone();
            let start = ofs + if b[ofs..].starts_with(LZMA_MAGIC) { LZMA_HEADER } else { 0 };
            b[start..(start + 4096).min(ofs + len)].fill(0xFF);
            let _ = geometry(&b);
        }
    }

    #[test]
    fn process_floors_within_the_playable_area() {
        let Some(data) = process() else { return eprintln!("cache/cp_process_f12.bsp not downloaded; skipped") };
        let g = geometry(&data).unwrap();
        let [lo, hi] = g.bounds.unwrap();
        // cp_process is symmetric around the middle point at the origin.
        assert!(lo[0] < -5000.0 && hi[0] > 5000.0, "{lo:?} {hi:?}");
        assert!(lo[1] > -6000.0 && hi[1] < 6000.0, "{lo:?} {hi:?}");
        assert!(g.floors.len() > 1000, "{} floors", g.floors.len());
        assert!(g.floors.windows(2).all(|w| w[0].z <= w[1].z), "floors sorted by height");
        assert!(g.walls.len() > 1000);
    }

    #[test]
    fn process_floors_have_material_kinds() {
        let Some(data) = process() else { return };
        let g = geometry(&data).unwrap();
        let of = |k| g.floors.iter().filter(|f| f.k == k).count();
        assert!(of(Kind::Concrete) + of(Kind::Metal) > 100, "{} concrete, {} metal", of(Kind::Concrete), of(Kind::Metal));
        assert!(of(Kind::Other) < g.floors.len(), "every floor is other");
        // A (convex) floor contains its own middle.
        let f = g.floors.iter().find(|f| f.k == Kind::Metal).unwrap();
        let n = (f.p.len() / 2) as f64;
        let mid = |i: usize| f.p.iter().skip(i).step_by(2).sum::<f64>() / n;
        assert!(contains(&f.p, mid(0), mid(1)), "{:?}", f.p);
    }

    #[test]
    fn swiftwater_has_water_floors() {
        let Some(data) = swiftwater() else { return };
        let g = geometry(&data).unwrap();
        assert!(g.floors.iter().any(|f| f.k == Kind::Water));
    }

    #[test]
    fn maps_without_points_or_pickups_are_framed_by_their_floors() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/cache/mge_training_v8_beta4b.bsp");
        let Ok(data) = std::fs::read(path) else { return eprintln!("{path} not downloaded; skipped") };
        let g = geometry(&data).unwrap();
        let [lo, hi] = g.bounds.unwrap();
        // Players fighting in an arena, far from the one spawn point.
        for p in [[2609.0, -10118.0, -473.0], [2061.0, -9786.0, -862.0]] {
            assert!((0..3).all(|i| lo[i] <= p[i] && p[i] <= hi[i]), "{lo:?} {hi:?}");
        }
        assert!(g.floors.len() > 1000);
        assert!(g.packs.is_empty(), "{:?}", g.packs);
        assert!(g.cap_areas.is_empty(), "{:?}", g.cap_areas);
    }

    #[test]
    fn process_packs_and_capture_areas() {
        let Some(data) = process() else { return eprintln!("cache/cp_process_f12.bsp not downloaded; skipped") };
        let g = geometry(&data).unwrap();
        for i in 0..5 {
            let n = g.cap_areas.iter().filter(|a| a.point == Some(i)).count();
            eprintln!("cap areas point {i}: {n}");
            assert!(n >= 1, "point {i}");
        }
        assert!(g.cap_areas.iter().all(|a| (0..3).all(|i| a.min[i] < a.max[i])), "{:?}", g.cap_areas);
        let count = |k: PickupItem| g.packs.iter().filter(|p| p.kind == k).count();
        eprintln!("packs health {} ammo {}", count(PickupItem::Health), count(PickupItem::Ammo));
        assert!(count(PickupItem::Health) > 10 && count(PickupItem::Ammo) > 10, "{:?}", g.packs);
    }

    #[test]
    fn payload_capture_areas_follow_the_cart_and_are_skipped() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/cache/pl_upward_f12.bsp");
        let Ok(data) = std::fs::read(path) else { return eprintln!("{path} not downloaded; skipped") };
        assert!(geometry(&data).unwrap().cap_areas.is_empty());
    }

    #[test]
    fn process_spawn_rooms_and_points() {
        let Some(data) = process() else { return };
        let s = spawns(&Entities::read(&data).unwrap());
        assert_eq!(s.rooms.len(), 6, "three spawn rooms per team");
        assert_eq!(s.rooms.iter().filter(|r| r.team == Team::Red).count(), 3);
        // cp_process is mirrored through the middle point at the origin.
        let blue2 = s.rooms.iter().find(|r| r.name == "bluespawnroom2").unwrap();
        assert_eq!(blue2.min, [-2400.0, -3136.0, 680.0]);
        assert_eq!(s.points.len(), 44, "56 minus 12 unused team-less ones");
        let count = |team: Team, point: i64| s.points.iter().filter(|p| p.team == team && p.point == Some(point)).count();
        // Last spawns belong to the team's own last point, forward spawns to mid and the enemy second.
        assert_eq!(count(Team::Red, 4), 16);
        assert_eq!(count(Team::Blue, 0), 16);
        assert_eq!((count(Team::Red, 2), count(Team::Red, 1)), (3, 3));
        assert_eq!((count(Team::Blue, 2), count(Team::Blue, 3)), (3, 3));
    }

    #[test]
    fn gullywash_points_numbered_from_one_map_to_the_games_indices() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/cache/cp_gullywash_f9.bsp");
        let Ok(data) = std::fs::read(path) else { return eprintln!("{path} not downloaded; skipped") };
        let g = geometry(&data).unwrap();
        // Blu's last point is index 0, at x 3768; red's last is 4, at x -3766.
        let x = |i: i64| g.cap_areas.iter().find(|a| a.point == Some(i)).map(|a| a.min[0]);
        assert!(x(0).unwrap() > 3000.0 && x(4).unwrap() < -3000.0);
        assert!(spawns(&Entities::read(&data).unwrap()).points.iter().all(|p| p.point.is_none_or(|i| (0..5).contains(&i))));
    }

    #[test]
    fn swiftwater_teamless_spawn_rooms_take_their_spawn_points_team() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/cache/pl_swiftwater_final1.bsp");
        let Ok(data) = std::fs::read(path) else { return eprintln!("{path} not downloaded; skipped") };
        let teams: Vec<Team> = spawns(&Entities::read(&data).unwrap()).rooms.iter().map(|r| r.team).collect();
        let (b, r) = (Team::Blue, Team::Red);
        assert_eq!(teams, [b, b, r, r, b, r]);
    }

    #[test]
    fn process_spawns_far_away_are_ignored() {
        let Some(data) = process() else { return };
        let Bounds { lo, hi } = playable_bounds(&Entities::read(&data).unwrap().list).unwrap();
        assert!(lo[0] > -8000.0 && hi[1] < 8000.0, "{lo:?} {hi:?}");
    }
}
