//! Top-down geometry from a Source BSP: walkable floors and wall outlines.
//!
//! Control points, pickups and nearby spawns frame the playable area, which keeps
//! the 3D skybox out of the picture.

use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};

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

type Vec3 = [f32; 3];

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
    let h = 8 + 16 * index;
    let (ofs, len) = (index_at(data, h)?, index_at(data, h + 4)?);
    let Some(raw) = data.get(ofs..ofs + len) else { bail!("lump {index} out of range") };
    if len > MAX_LUMP {
        bail!("lump {index}: {len} bytes");
    }
    if !raw.starts_with(b"LZMA") {
        return Ok(raw.to_vec());
    }
    // Valve's LZMA lump header: id, actual size, packed size, 5 property bytes.
    let actual = u32::from_le_bytes(bytes(raw, 4)?) as usize;
    if actual > MAX_LUMP {
        bail!("lump {index}: {actual} bytes unpacked");
    }
    let props: [u8; 5] = bytes(raw, 12)?;
    let mut alone = props.to_vec();
    alone.extend_from_slice(&(actual as u64).to_le_bytes());
    alone.extend_from_slice(&raw[17..]);
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
    let near = bounds_of(&core, 4096.0);
    core.extend(spawns.into_iter().filter(|s| inside(s, &near)));
    Ok(Some(bounds_of(&core, PAD)))
}

fn team(v: Option<&str>) -> &'static str {
    match v {
        Some("2") => "red",
        Some("3") => "blue",
        _ => "none",
    }
}

fn vec3(v: Option<&str>) -> Option<Vec3> {
    let v: Vec<f32> = v?.split_whitespace().filter_map(|s| s.parse().ok()).collect();
    <Vec3>::try_from(v.as_slice()).ok()
}

fn get(e: &[(String, String)], k: &str) -> Option<String> {
    e.iter().find(|(key, _)| key.eq_ignore_ascii_case(k)).map(|(_, v)| v.clone())
}

/// A brush entity's bounds: its model's box (models lump, 48 bytes each) moved by its origin.
fn brush_bounds(models: &[u8], e: &[(String, String)]) -> Option<(Vec3, Vec3)> {
    let m = usize::from(get(e, "model")?.strip_prefix('*')?.parse::<u16>().ok()?);
    if (m + 1) * 48 > models.len() {
        return None;
    }
    let origin = vec3(get(e, "origin").as_deref()).unwrap_or([0.0; 3]);
    let corner = |o: usize| -> Option<Vec3> {
        let mut c = origin;
        for (i, v) in c.iter_mut().enumerate() {
            *v += f32_at(models, m * 48 + o + i * 4).ok()?;
        }
        Some(c)
    };
    Some((corner(0)?, corner(12)?))
}

/// Health and ammo packs by size.
fn packs(ents: &[Vec<(String, String)>]) -> Vec<Value> {
    ents.iter()
        .filter_map(|e| {
            let class = e.iter().find(|(k, _)| k.eq_ignore_ascii_case("classname"))?.1.as_str();
            let (kind, size) = class
                .strip_prefix("item_healthkit_")
                .map(|s| ("health", s))
                .or_else(|| class.strip_prefix("item_ammopack_").map(|s| ("ammo", s)))?;
            if !["small", "medium", "full"].contains(&size) {
                return None;
            }
            let pos = vec3(e.iter().find(|(k, _)| k.eq_ignore_ascii_case("origin"))?.1.as_str().into())?;
            Some(json!({"kind": kind, "size": size, "pos": pos.map(r1)}))
        })
        .collect()
}

/// Capture areas (`trigger_capture_area`: their brush model's bounds) and the point each caps.
fn cap_areas(data: &[u8]) -> Result<Vec<Value>> {
    let ents = entities(data)?;
    let models = lump(data, LUMP_MODELS)?;
    let points: Vec<(String, i64)> = ents
        .iter()
        .filter(|e| get(e, "classname").as_deref() == Some("team_control_point"))
        .filter_map(|e| Some((get(e, "targetname")?, get(e, "point_index")?.parse().ok()?)))
        .collect();
    Ok(ents
        .iter()
        .filter(|e| get(e, "classname").as_deref() == Some("trigger_capture_area"))
        .filter_map(|e| {
            let (min, max) = brush_bounds(&models, e)?;
            let point = get(e, "area_cap_point").and_then(|cp| points.iter().find(|(name, _)| name.eq_ignore_ascii_case(&cp)).map(|p| p.1));
            Some(json!({"point": point, "min": min.map(r1), "max": max.map(r1)}))
        })
        .collect())
}

/// Spawn rooms (`func_respawnroom`: their brush model's bounds) and spawn points
/// (`info_player_teamspawn`), with the control point each spawn point belongs to: TF2 enables
/// such a spawn point for the team that owns that point.
pub fn spawns(data: &[u8]) -> Result<Value> {
    let ents = entities(data)?;
    let models = lump(data, LUMP_MODELS)?;
    // team_control_point targetname -> point index
    let points: Vec<(String, i64)> = ents
        .iter()
        .filter(|e| get(e, "classname").as_deref() == Some("team_control_point"))
        .filter_map(|e| Some((get(e, "targetname")?, get(e, "point_index")?.parse().ok()?)))
        .collect();
    let mut rooms = Vec::new();
    let mut spawn_points = Vec::new();
    for e in &ents {
        match get(e, "classname").as_deref() {
            Some("func_respawnroom") => {
                let Some((min, max)) = brush_bounds(&models, e) else { continue };
                rooms.push(json!({
                    "name": get(e, "targetname").unwrap_or_default(),
                    "team": team(get(e, "TeamNum").as_deref()),
                    "min": min.map(r1),
                    "max": max.map(r1),
                    "disabled": get(e, "StartDisabled").as_deref() == Some("1"),
                }));
            }
            Some("info_player_teamspawn") => {
                let Some(pos) = vec3(get(e, "origin").as_deref()) else { continue };
                // Team-less spawn points are unused leftovers (on cp_process far outside the map).
                if team(get(e, "TeamNum").as_deref()) == "none" {
                    continue;
                }
                let point =
                    get(e, "controlpoint").and_then(|cp| points.iter().find(|(name, _)| name.eq_ignore_ascii_case(&cp)).map(|p| p.1));
                spawn_points.push(json!({
                    "team": team(get(e, "TeamNum").as_deref()),
                    "pos": pos.map(r1),
                    "point": point,
                    "disabled": get(e, "StartDisabled").as_deref() == Some("1"),
                }));
            }
            _ => {}
        }
    }
    Ok(json!({"rooms": rooms, "points": spawn_points}))
}

fn texture_names(data: &[u8]) -> Result<Vec<String>> {
    let table = lump(data, LUMP_TEXDATA_STRING_TABLE)?;
    let strings = lump(data, LUMP_TEXDATA_STRING_DATA)?;
    let texdata = lump(data, LUMP_TEXDATA)?;
    (0..texdata.len() / 32)
        .map(|i| {
            let id = index_at(&texdata, i * 32 + 12)?;
            // A name missing from the tables is only used to skip tool textures, so it is left empty.
            let start = index_at(&table, id * 4).unwrap_or(0);
            let s = strings.get(start..).unwrap_or_default();
            let s = &s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())];
            Ok(String::from_utf8_lossy(s).to_lowercase())
        })
        .collect()
}

fn r1(v: f32) -> f64 {
    (f64::from(v) * 10.0).round() / 10.0
}

/// `{"bounds": [[x,y,z],[x,y,z]], "floors": [{"z", "p": [x,y,...]}], "walls": [{"z", "p": [x1,y1,x2,y2]}]}`,
/// floors sorted from low to high.
pub fn geometry(data: &[u8]) -> Result<Value> {
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
    let (first, count) = (index_at(&models, 40)?, index_at(&models, 44)?);
    let vertex = |i: usize| -> Result<Vec3> { Ok([f32_at(&verts, i * 12)?, f32_at(&verts, i * 12 + 4)?, f32_at(&verts, i * 12 + 8)?]) };

    let mut floors: Vec<(f32, Value)> = Vec::new();
    let mut floor_pts = Vec::new();
    let mut walls = Vec::new();
    // The world's faces each have their own edges; a crafted map could reuse the same ones in
    // every face and have the loop below build gigabytes of points.
    let mut edges_left = surfedges.len() / 4;
    for f in first..first + count {
        let o = f * 56;
        let (plane, [side]) = (usize::from(u16_at(&faces, o)?), bytes(&faces, o + 2)?);
        let (first_edge, num_edges, tex) = (index_at(&faces, o + 4)?, usize::from(u16_at(&faces, o + 8)?), i16_at(&faces, o + 10)?);
        edges_left = edges_left.checked_sub(num_edges).ok_or_else(|| anyhow!("faces use more edges than the map has"))?;
        if tex < 0 {
            continue;
        }
        let (flags, tdata) = (i32_at(&texinfo, tex as usize * 72 + 64)?, i32_at(&texinfo, tex as usize * 72 + 68)?);
        if flags & SKIP_FLAGS != 0 || names.get(tdata as usize).is_some_and(|n| n.starts_with("tools/")) {
            continue;
        }
        let mut nz = f32_at(&planes, plane * 20 + 8)?;
        if side != 0 {
            nz = -nz;
        }
        let pts: Vec<Vec3> = (first_edge..first_edge + num_edges)
            .map(|e| {
                let se = i32_at(&surfedges, e * 4)?;
                let eo = se.unsigned_abs() as usize * 4;
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
        if nz > 0.7 {
            let p: Vec<f64> = pts.iter().flat_map(|p| [r1(p[0]), r1(p[1])]).collect();
            floors.push((centre[2], json!({"z": r1(centre[2]), "p": p})));
            if bounds.is_none() {
                floor_pts.extend(pts);
            }
        } else if nz.abs() < 0.3 {
            // total_cmp: a broken map can have NaN coordinates.
            let key = |a: &&Vec3, b: &&Vec3| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1]));
            let lo = pts.iter().min_by(key).unwrap();
            let hi = pts.iter().max_by(key).unwrap();
            if (lo[0] - hi[0]).powi(2) + (lo[1] - hi[1]).powi(2) > 4.0 {
                let top = pts.iter().map(|p| p[2]).fold(f32::MIN, f32::max);
                walls.push(json!({"z": r1(top), "p": [r1(lo[0]), r1(lo[1]), r1(hi[0]), r1(hi[1])]}));
            }
        }
    }
    floors.sort_by(|a, b| a.0.total_cmp(&b.0));
    // Without points or pickups, the floors frame the map; the top keeps the PAD above the
    // highest floor that the viewer's roof cut expects.
    let bounds = bounds.or_else(|| {
        let (lo, mut hi) = bounds_of(&floor_pts, 256.0);
        hi[2] += PAD - 256.0;
        (!floor_pts.is_empty()).then_some((lo, hi))
    });
    Ok(json!({
        "spawns": spawns(data)?,
        "packs": packs(&entities(data)?),
        "cap_areas": cap_areas(data)?,
        "bounds": bounds.map(|(lo, hi)| json!([lo, hi])),
        "floors": floors.into_iter().map(|f| f.1).collect::<Vec<_>>(),
        "walls": walls,
    }))
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
        for face in faces.chunks_exact_mut(56) {
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
            let h = 8 + 16 * index;
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
        let b = &g["bounds"];
        // cp_process is symmetric around the middle point at the origin.
        assert!(b[0][0].as_f64().unwrap() < -5000.0 && b[1][0].as_f64().unwrap() > 5000.0, "{b}");
        assert!(b[0][1].as_f64().unwrap() > -6000.0 && b[1][1].as_f64().unwrap() < 6000.0, "{b}");
        let floors = g["floors"].as_array().unwrap();
        assert!(floors.len() > 1000, "{} floors", floors.len());
        let zs: Vec<f64> = floors.iter().map(|f| f["z"].as_f64().unwrap()).collect();
        assert!(zs.windows(2).all(|w| w[0] <= w[1]), "floors sorted by height");
        assert!(g["walls"].as_array().unwrap().len() > 1000);
    }

    #[test]
    fn maps_without_points_or_pickups_are_framed_by_their_floors() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/cache/mge_training_v8_beta4b.bsp");
        let Ok(data) = std::fs::read(path) else { return eprintln!("{path} not downloaded; skipped") };
        let g = geometry(&data).unwrap();
        let b = &g["bounds"];
        // Players fighting in an arena, far from the one spawn point.
        for p in [[2609.0, -10118.0, -473.0], [2061.0, -9786.0, -862.0]] {
            assert!((0..3).all(|i| b[0][i].as_f64().unwrap() <= p[i] && p[i] <= b[1][i].as_f64().unwrap()), "{b}");
        }
        assert!(g["floors"].as_array().unwrap().len() > 1000);
        assert_eq!(g["packs"], json!([]));
        assert_eq!(g["cap_areas"], json!([]));
    }

    #[test]
    fn process_packs_and_capture_areas() {
        let Some(data) = process() else { return eprintln!("cache/cp_process_f12.bsp not downloaded; skipped") };
        let g = geometry(&data).unwrap();
        let areas = g["cap_areas"].as_array().unwrap();
        for i in 0..5 {
            let n = areas.iter().filter(|a| a["point"] == i).count();
            eprintln!("cap areas point {i}: {n}");
            assert!(n >= 1, "point {i}");
        }
        assert!(areas.iter().all(|a| (0..3).all(|i| a["min"][i].as_f64() < a["max"][i].as_f64())), "{areas:?}");
        let packs = g["packs"].as_array().unwrap();
        let count = |k: &str| packs.iter().filter(|p| p["kind"] == k).count();
        eprintln!("packs health {} ammo {}", count("health"), count("ammo"));
        assert!(count("health") > 10 && count("ammo") > 10, "{packs:?}");
        assert!(
            packs
                .iter()
                .all(|p| ["small", "medium", "full"].contains(&p["size"].as_str().unwrap()) && p["pos"].as_array().unwrap().len() == 3)
        );
    }

    #[test]
    fn process_spawn_rooms_and_points() {
        let Some(data) = process() else { return };
        let s = spawns(&data).unwrap();
        let rooms = s["rooms"].as_array().unwrap();
        assert_eq!(rooms.len(), 6, "three spawn rooms per team");
        assert_eq!(rooms.iter().filter(|r| r["team"] == "red").count(), 3);
        // cp_process is mirrored through the middle point at the origin.
        let blue2 = rooms.iter().find(|r| r["name"] == "bluespawnroom2").unwrap();
        assert_eq!(blue2["min"], json!([-2400.0, -3136.0, 680.0]));
        let points = s["points"].as_array().unwrap();
        assert_eq!(points.len(), 44, "56 minus 12 unused team-less ones");
        let count = |team: &str, point: i64| points.iter().filter(|p| p["team"] == team && p["point"] == point).count();
        // Last spawns belong to the team's own last point, forward spawns to mid and the enemy second.
        assert_eq!(count("red", 4), 16);
        assert_eq!(count("blue", 0), 16);
        assert_eq!((count("red", 2), count("red", 1)), (3, 3));
        assert_eq!((count("blue", 2), count("blue", 3)), (3, 3));
    }

    #[test]
    fn process_spawns_far_away_are_ignored() {
        let Some(data) = process() else { return };
        let (lo, hi) = playable_bounds(&data).unwrap().unwrap();
        assert!(lo[0] > -8000.0 && hi[1] < 8000.0, "{lo:?} {hi:?}");
    }
}
