//! Top-down geometry from a Source BSP: walkable floors and wall outlines.
//!
//! Control points, pickups and nearby spawns frame the playable area, which keeps
//! the 3D skybox out of the picture.

use anyhow::{Result, anyhow, bail};
use serde::Serialize;
use stvtf::output::{PickupItem, PickupSize, Team};

const LUMP_ENTITIES: usize = 0;
const LUMP_PLANES: usize = 1;
const LUMP_TEXDATA: usize = 2;
const LUMP_VERTEXES: usize = 3;
const LUMP_TEXINFO: usize = 6;
const LUMP_FACES: usize = 7;
const LUMP_EDGES: usize = 12;
const LUMP_SURFEDGES: usize = 13;
const LUMP_MODELS: usize = 14;
const LUMP_TEXDATA_STRING_DATA: usize = 43;
const LUMP_TEXDATA_STRING_TABLE: usize = 44;

// sky2d, sky, trigger, nodraw, hint, skip
const SKIP_FLAGS: i32 = 0x2 | 0x4 | 0x40 | 0x80 | 0x100 | 0x200;
const PAD: f32 = 1536.0;
// The lumps read here are under 2 MB on real maps; a crafted one could otherwise claim 512 MB
// unpacked for each, from a few KB of LZMA.
const MAX_LUMP: usize = 16 << 20;
/// The lump directory follows the `VBSP` id and version: offset, length, version and id per lump.
const LUMP_DIRECTORY: usize = 8;
const LUMP_ENTRY: usize = 16;
/// Valve's LZMA lump header: `LZMA`, actual size (u32), packed size (u32), 5 property bytes.
const LZMA_ACTUAL_SIZE: usize = 4;
const LZMA_PROPERTIES: usize = 12;
const LZMA_HEADER: usize = 17;
/// Record sizes and field offsets of the lumps read (`dplane_t`, `dface_t`, …).
const PLANE_SIZE: usize = 20;
const PLANE_NORMAL_Z: usize = 8;
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
}

/// An upward face: its height and outline (`x, y, x, y, …`).
#[derive(Debug, Serialize)]
pub struct Floor {
    pub z: f64,
    pub p: Vec<f64>,
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
/// An offset, count or index stored as an i32; negative ones are an error.
fn index_at(b: &[u8], o: usize) -> Result<usize> {
    let v = i32_at(b, o)?;
    usize::try_from(v).map_err(|_| anyhow!("negative index {v} at {o}"))
}

fn lump(data: &[u8], index: usize) -> Result<Vec<u8>> {
    let h = LUMP_DIRECTORY + LUMP_ENTRY * index;
    let (ofs, len) = (index_at(data, h)?, index_at(data, h + 4)?);
    let Some(raw) = data.get(ofs..ofs + len) else { bail!("lump {index} out of range") };
    if len > MAX_LUMP {
        bail!("lump {index}: {len} bytes");
    }
    if !raw.starts_with(b"LZMA") {
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

pub fn entities(data: &[u8]) -> Result<Vec<Vec<(String, String)>>> {
    let text = String::from_utf8_lossy(&lump(data, LUMP_ENTITIES)?).into_owned();
    let mut out = Vec::new();
    for block in text.split('{').skip(1) {
        let body = block.split('}').next().unwrap_or("");
        let quoted: Vec<&str> = body.split('"').skip(1).step_by(2).collect();
        out.push(quoted.chunks(2).filter(|kv| kv.len() == 2).map(|kv| (kv[0].to_string(), kv[1].to_string())).collect());
    }
    Ok(out)
}

fn bounds_of(pts: &[Vec3], pad: f32) -> (Vec3, Vec3) {
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for p in pts {
        for i in 0..3 {
            lo[i] = lo[i].min(p[i] - pad);
            hi[i] = hi[i].max(p[i] + pad);
        }
    }
    (lo, hi)
}

fn inside(p: &Vec3, b: &(Vec3, Vec3)) -> bool {
    (0..3).all(|i| b.0[i] <= p[i] && p[i] <= b.1[i])
}

/// Points, pickups and the spawns near them; maps also hide unused spawns far away. None for a
/// map without points or pickups (MGE, jump maps): its spawns frame no more than a spawn room.
pub fn playable_bounds(data: &[u8]) -> Result<Option<(Vec3, Vec3)>> {
    let (mut core, mut spawns) = (Vec::new(), Vec::new());
    for ent in entities(data)? {
        let get = |k: &str| ent.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str());
        let Some(origin) = get("origin") else { continue };
        let v: Vec<f32> = origin.split_whitespace().filter_map(|s| s.parse().ok()).collect();
        let Ok(p) = <Vec3>::try_from(v.as_slice()) else { continue };
        let class = get("classname").unwrap_or("");
        if class == "team_control_point" || class.starts_with("item_healthkit_") || class.starts_with("item_ammopack_") {
            core.push(p);
        } else if class == "info_player_teamspawn" {
            spawns.push(p);
        }
    }
    if core.is_empty() {
        return Ok(None);
    }
    let near = bounds_of(&core, SPAWN_REACH);
    core.extend(spawns.into_iter().filter(|s| inside(s, &near)));
    Ok(Some(bounds_of(&core, PAD)))
}

fn team(v: Option<&str>) -> Team {
    match v {
        Some("2") => Team::Red,
        Some("3") => Team::Blue,
        _ => Team::Unassigned,
    }
}

fn vec3(v: Option<&str>) -> Option<Vec3> {
    let v: Vec<f32> = v?.split_whitespace().filter_map(|s| s.parse().ok()).collect();
    <Vec3>::try_from(v.as_slice()).ok()
}

fn get(e: &[(String, String)], k: &str) -> Option<String> {
    e.iter().find(|(key, _)| key.eq_ignore_ascii_case(k)).map(|(_, v)| v.clone())
}

/// A brush entity's bounds: its model's box moved by its origin.
fn brush_bounds(models: &[u8], e: &[(String, String)]) -> Option<(Vec3, Vec3)> {
    let m = usize::from(get(e, "model")?.strip_prefix('*')?.parse::<u16>().ok()?);
    if (m + 1) * MODEL_SIZE > models.len() {
        return None;
    }
    let origin = vec3(get(e, "origin").as_deref()).unwrap_or([0.0; 3]);
    let corner = |o: usize| -> Option<Vec3> {
        let mut c = origin;
        for (i, v) in c.iter_mut().enumerate() {
            *v += f32_at(models, m * MODEL_SIZE + o + i * 4).ok()?;
        }
        Some(c)
    };
    Some((corner(MODEL_MINS)?, corner(MODEL_MAXS)?))
}

/// Health and ammo packs by size.
fn packs(ents: &[Vec<(String, String)>]) -> Vec<Pack> {
    ents.iter()
        .filter_map(|e| {
            let class = e.iter().find(|(k, _)| k.eq_ignore_ascii_case("classname"))?.1.as_str();
            let (kind, size) = class
                .strip_prefix("item_healthkit_")
                .map(|s| (PickupItem::Health, s))
                .or_else(|| class.strip_prefix("item_ammopack_").map(|s| (PickupItem::Ammo, s)))?;
            let size = match size {
                "small" => PickupSize::Small,
                "medium" => PickupSize::Medium,
                "full" => PickupSize::Full,
                _ => return None,
            };
            let pos = vec3(e.iter().find(|(k, _)| k.eq_ignore_ascii_case("origin"))?.1.as_str().into())?;
            Some(Pack { kind, size, pos: pos.map(r1) })
        })
        .collect()
}

/// `team_control_point` targetname -> the point's index in the game, which is its rank by
/// `point_index` (`cp_gullywash` numbers its points from 1).
fn point_indices(ents: &[Vec<(String, String)>]) -> Vec<(String, i64)> {
    let mut points: Vec<(String, i64)> = ents
        .iter()
        .filter(|e| get(e, "classname").as_deref() == Some("team_control_point"))
        .filter_map(|e| Some((get(e, "targetname")?, get(e, "point_index")?.parse().ok()?)))
        .collect();
    points.sort_by_key(|p| p.1);
    let mut ranks: Vec<i64> = points.iter().map(|p| p.1).collect();
    ranks.dedup();
    points.into_iter().map(|(name, i)| (name, ranks.iter().position(|&r| r == i).unwrap_or_default() as i64)).collect()
}

/// Capture areas (`trigger_capture_area`: their brush model's bounds) and the point each caps.
/// Payload ones are parented to the cart and move with it, so their bounds are just its start.
fn cap_areas(data: &[u8]) -> Result<Vec<CapArea>> {
    let ents = entities(data)?;
    let models = lump(data, LUMP_MODELS)?;
    let points = point_indices(&ents);
    Ok(ents
        .iter()
        .filter(|e| get(e, "classname").as_deref() == Some("trigger_capture_area"))
        .filter(|e| get(e, "parentname").is_none_or(|p| p.is_empty()))
        .filter_map(|e| {
            let (min, max) = brush_bounds(&models, e)?;
            let point = get(e, "area_cap_point").and_then(|cp| points.iter().find(|(name, _)| name.eq_ignore_ascii_case(&cp)).map(|p| p.1));
            Some(CapArea { point, min: min.map(r1), max: max.map(r1) })
        })
        .collect())
}

/// Spawn rooms (`func_respawnroom`: their brush model's bounds) and spawn points
/// (`info_player_teamspawn`), with the control point each spawn point belongs to: TF2 enables
/// such a spawn point for the team that owns that point.
pub fn spawns(data: &[u8]) -> Result<Spawns> {
    let ents = entities(data)?;
    let models = lump(data, LUMP_MODELS)?;
    let points = point_indices(&ents);
    let mut rooms = Vec::new();
    let mut spawn_points = Vec::new();
    for e in &ents {
        match get(e, "classname").as_deref() {
            Some("func_respawnroom") => {
                let Some((min, max)) = brush_bounds(&models, e) else { continue };
                rooms.push(SpawnRoom {
                    name: get(e, "targetname").unwrap_or_default(),
                    team: team(get(e, "TeamNum").as_deref()),
                    min: min.map(r1),
                    max: max.map(r1),
                    disabled: get(e, "StartDisabled").as_deref() == Some("1"),
                });
            }
            Some("info_player_teamspawn") => {
                let Some(pos) = vec3(get(e, "origin").as_deref()) else { continue };
                // Team-less spawn points are unused leftovers (on cp_process far outside the map).
                if team(get(e, "TeamNum").as_deref()) == Team::Unassigned {
                    continue;
                }
                let point =
                    get(e, "controlpoint").and_then(|cp| points.iter().find(|(name, _)| name.eq_ignore_ascii_case(&cp)).map(|p| p.1));
                spawn_points.push(SpawnPoint {
                    team: team(get(e, "TeamNum").as_deref()),
                    pos: pos.map(r1),
                    point,
                    disabled: get(e, "StartDisabled").as_deref() == Some("1"),
                });
            }
            _ => {}
        }
    }
    // Some maps (pl_swiftwater) give a room its team by input: take the team of the spawn points
    // inside, else of the room around its centre.
    for by_points in [true, false] {
        for i in 0..rooms.len() {
            if rooms[i].team != Team::Unassigned {
                continue;
            }
            let found = if by_points {
                let teams: Vec<Team> = spawn_points.iter().filter(|p| rooms[i].holds(p.pos)).map(|p| p.team).collect();
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
    Ok(Spawns { rooms, points: spawn_points })
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

fn r1(v: f32) -> f64 {
    (f64::from(v) * 10.0).round() / 10.0
}

/// The map's spawns, packs, capture areas, playable bounds, floors (sorted from low to high) and
/// walls.
pub fn geometry(data: &[u8]) -> Result<Geometry> {
    if !data.starts_with(b"VBSP") {
        bail!("not a VBSP file");
    }
    let planes = lump(data, LUMP_PLANES)?;
    let verts = lump(data, LUMP_VERTEXES)?;
    let texinfo = lump(data, LUMP_TEXINFO)?;
    let faces = lump(data, LUMP_FACES)?;
    let edges = lump(data, LUMP_EDGES)?;
    let surfedges = lump(data, LUMP_SURFEDGES)?;
    let models = lump(data, LUMP_MODELS)?;
    let names = texture_names(data)?;
    let bounds = playable_bounds(data)?;

    // Only the world model: brush entities (doors, triggers, func_brush) are separate models.
    let (first, count) = (index_at(&models, MODEL_FIRST_FACE)?, index_at(&models, MODEL_NUM_FACES)?);
    let vertex = |i: usize| -> Result<Vec3> {
        let o = i * VERTEX_SIZE;
        Ok([f32_at(&verts, o)?, f32_at(&verts, o + 4)?, f32_at(&verts, o + 8)?])
    };

    let mut floors: Vec<(f32, Floor)> = Vec::new();
    let mut floor_pts = Vec::new();
    let mut walls = Vec::new();
    // The world's faces each have their own edges; a crafted map could reuse the same ones in
    // every face and have the loop below build gigabytes of points.
    let mut edges_left = surfedges.len() / SURFEDGE_SIZE;
    for f in first..first + count {
        let o = f * FACE_SIZE;
        let (plane, [side]) = (usize::from(u16_at(&faces, o)?), bytes(&faces, o + FACE_SIDE)?);
        let first_edge = index_at(&faces, o + FACE_FIRST_EDGE)?;
        let num_edges = usize::from(u16_at(&faces, o + FACE_NUM_EDGES)?);
        let tex = i16_at(&faces, o + FACE_TEXINFO)?;
        edges_left = edges_left.checked_sub(num_edges).ok_or_else(|| anyhow!("faces use more edges than the map has"))?;
        if tex < 0 {
            continue;
        }
        let t = tex as usize * TEXINFO_SIZE;
        let (flags, tdata) = (i32_at(&texinfo, t + TEXINFO_FLAGS)?, i32_at(&texinfo, t + TEXINFO_TEXDATA)?);
        if flags & SKIP_FLAGS != 0 || names.get(tdata as usize).is_some_and(|n| n.starts_with("tools/")) {
            continue;
        }
        let mut nz = f32_at(&planes, plane * PLANE_SIZE + PLANE_NORMAL_Z)?;
        if side != 0 {
            nz = -nz;
        }
        let pts: Vec<Vec3> = (first_edge..first_edge + num_edges)
            .map(|e| {
                let se = i32_at(&surfedges, e * SURFEDGE_SIZE)?;
                let eo = se.unsigned_abs() as usize * EDGE_SIZE;
                vertex(usize::from(if se >= 0 { u16_at(&edges, eo)? } else { u16_at(&edges, eo + 2)? }))
            })
            .collect::<Result<_>>()?;
        if pts.len() < 3 {
            continue;
        }
        let n = pts.len() as f32;
        let centre = [0, 1, 2].map(|i| pts.iter().map(|p| p[i]).sum::<f32>() / n);
        if bounds.as_ref().is_some_and(|b| !inside(&centre, b)) {
            continue;
        }
        if nz > FLOOR_MIN_NORMAL_Z {
            let p: Vec<f64> = pts.iter().flat_map(|p| [r1(p[0]), r1(p[1])]).collect();
            floors.push((centre[2], Floor { z: r1(centre[2]), p }));
            if bounds.is_none() {
                floor_pts.extend(pts);
            }
        } else if nz.abs() < WALL_MAX_NORMAL_Z {
            // total_cmp: a broken map can have NaN coordinates.
            let key = |a: &&Vec3, b: &&Vec3| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1]));
            let lo = pts.iter().min_by(key).unwrap();
            let hi = pts.iter().max_by(key).unwrap();
            if (lo[0] - hi[0]).powi(2) + (lo[1] - hi[1]).powi(2) > MIN_WALL_LENGTH.powi(2) {
                let top = pts.iter().map(|p| p[2]).fold(f32::MIN, f32::max);
                walls.push(Wall { z: r1(top), p: [r1(lo[0]), r1(lo[1]), r1(hi[0]), r1(hi[1])] });
            }
        }
    }
    floors.sort_by(|a, b| a.0.total_cmp(&b.0));
    // Without points or pickups, the floors frame the map; the top keeps the PAD above the
    // highest floor that the viewer's roof cut expects.
    let bounds = bounds.or_else(|| {
        let (lo, mut hi) = bounds_of(&floor_pts, FLOOR_MARGIN);
        hi[2] += PAD - FLOOR_MARGIN;
        (!floor_pts.is_empty()).then_some((lo, hi))
    });
    Ok(Geometry {
        spawns: spawns(data)?,
        packs: packs(&entities(data)?),
        cap_areas: cap_areas(data)?,
        bounds: bounds.map(|(lo, hi)| [lo.map(f64::from), hi.map(f64::from)]),
        floors: floors.into_iter().map(|f| f.1).collect(),
        walls,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process() -> Option<Vec<u8>> {
        std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/cache/cp_process_f12.bsp")).ok()
    }

    #[test]
    fn rejects_non_bsp() {
        assert!(geometry(b"not a map").is_err());
    }

    /// A VBSP header (64 lumps) with every lump empty.
    fn empty_bsp() -> Vec<u8> {
        let mut b = b"VBSP".to_vec();
        b.extend_from_slice(&20i32.to_le_bytes());
        b.resize(8 + 64 * 16 + 4, 0);
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
            set_i32(&mut b, 8 + 16 * LUMP_MODELS, ofs);
            set_i32(&mut b, 8 + 16 * LUMP_MODELS + 4, len);
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
        set_i32(&mut b, 8 + 16 * LUMP_PLANES, ofs as i32);
        set_i32(&mut b, 8 + 16 * LUMP_PLANES + 4, 17);
        assert!(geometry(&b).is_err());
    }

    #[test]
    fn lump_over_the_limit_is_an_error() {
        let mut b = empty_bsp();
        let ofs = b.len();
        b.resize(ofs + MAX_LUMP + 1, 0);
        set_i32(&mut b, 8 + 16 * LUMP_PLANES, ofs as i32);
        set_i32(&mut b, 8 + 16 * LUMP_PLANES + 4, (MAX_LUMP + 1) as i32);
        assert!(lump(&b, LUMP_PLANES).unwrap_err().to_string().contains("bytes"));
    }

    #[test]
    fn faces_sharing_edges_are_an_error() {
        let Some(mut data) = process() else { return };
        // Every face claims the same 65535 edges; the rewritten lump goes, unpacked, at the end.
        let mut faces = lump(&data, LUMP_FACES).unwrap();
        for face in faces.as_chunks_mut::<56>().0 {
            face[4..8].fill(0);
            face[8..10].copy_from_slice(&u16::MAX.to_le_bytes());
        }
        let h = 8 + 16 * LUMP_FACES;
        let (ofs, len) = (data.len() as i32, faces.len() as i32);
        data.extend_from_slice(&faces);
        set_i32(&mut data, h, ofs);
        set_i32(&mut data, h + 4, len);
        assert!(geometry(&data).unwrap_err().to_string().contains("more edges"));
    }

    #[test]
    fn process_truncated_or_corrupted_is_not_a_panic() {
        let Some(data) = process() else { return };
        for n in [9, 500, 1036, data.len() / 3, data.len() / 2, data.len() - 1] {
            let _ = geometry(&data[..n]);
        }
        for index in [LUMP_PLANES, LUMP_TEXDATA, LUMP_VERTEXES, LUMP_TEXINFO, LUMP_FACES, LUMP_EDGES, LUMP_SURFEDGES, LUMP_MODELS] {
            let h = LUMP_DIRECTORY + LUMP_ENTRY * index;
            let (ofs, len) = (index_at(&data, h).unwrap(), index_at(&data, h + 4).unwrap());
            // Too short, and the contents overwritten (out of range indices, NaN coordinates).
            let mut b = data.clone();
            set_i32(&mut b, h + 4, (len / 2) as i32);
            let _ = geometry(&b);
            let mut b = data.clone();
            let start = ofs + if b[ofs..].starts_with(b"LZMA") { 17 } else { 0 };
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
        let s = spawns(&data).unwrap();
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
        assert!(spawns(&data).unwrap().points.iter().all(|p| p.point.is_none_or(|i| (0..5).contains(&i))));
    }

    #[test]
    fn swiftwater_teamless_spawn_rooms_take_their_spawn_points_team() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/cache/pl_swiftwater_final1.bsp");
        let Ok(data) = std::fs::read(path) else { return eprintln!("{path} not downloaded; skipped") };
        let teams: Vec<Team> = spawns(&data).unwrap().rooms.iter().map(|r| r.team).collect();
        let (b, r) = (Team::Blue, Team::Red);
        assert_eq!(teams, [b, b, r, r, b, r]);
    }

    #[test]
    fn process_spawns_far_away_are_ignored() {
        let Some(data) = process() else { return };
        let (lo, hi) = playable_bounds(&data).unwrap().unwrap();
        assert!(lo[0] > -8000.0 && hi[1] < 8000.0, "{lo:?} {hi:?}");
    }
}
