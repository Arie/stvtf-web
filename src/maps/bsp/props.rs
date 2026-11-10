//! The tops of a map's solid static props (containers, crates, train cars): floors that are not
//! in its brushes, for strokes drawn on them in TF2.
//!
//! Where each prop stands comes from the BSP's static prop game lump (`sprp`); what its top is,
//! from `props.txt.gz`: the upward faces of each stock prop model's collision mesh, made from
//! TF2's VPKs by `script/model-bounds.mjs`. A map's own models (in its pakfile) are left out.

use super::format::{LUMP_GAME_LUMP, bytes, i32_at, index_at, lump, u16_at, unpack, vec3_at};
use super::material::Kind;
use super::{Bounds, FLOOR_MIN_NORMAL_Z, Floor, Vec3, cross, gradient, outline};
use anyhow::{Result, bail};
use std::collections::HashMap;
use std::io::Read;
use std::sync::OnceLock;
use stvtf::output::round1;

/// `sprp`, as the game lump directory names it.
const SPRP: i32 = 0x7370_7270;
/// `dgamelump_t`: its size, and where its flags, version, offset and length are.
const GAME_LUMP_SIZE: usize = 16;
const GAME_LUMP_FLAGS: usize = 4;
const GAME_LUMP_VERSION: usize = 6;
const GAME_LUMP_OFFSET: usize = 8;
const GAME_LUMP_LENGTH: usize = 12;
const COMPRESSED: u16 = 1;
/// A model name in the dictionary.
const NAME_SIZE: usize = 128;
/// `StaticPropLump_t`: where its origin, angles, model, and solidity are, and its size by version
/// (games differ: TF2's version 10 is 72 bytes, so the size the lump holds is taken first).
const PROP_ANGLES: usize = 12;
const PROP_TYPE: usize = 24;
const PROP_SOLID: usize = 30;
const SOLID_NONE: u8 = 0;
fn prop_size(version: u16) -> Option<usize> {
    Some(match version {
        4 => 56,
        5 => 60,
        6 => 64,
        7 | 8 => 68,
        9 => 72,
        10 => 76,
        11 => 80,
        _ => return None,
    })
}
/// The smallest a prop is (version 4).
const MIN_PROP_SIZE: usize = 56;
/// More than this many props, or prop floors, is a crafted map: the rest are left out.
const MAX_PROPS: usize = 1 << 16;
const MAX_FLOORS: usize = 1 << 18;

/// A solid static prop: its model's name, where it stands and how it is turned (pitch, yaw, roll).
struct Prop {
    model: String,
    origin: Vec3,
    angles: Vec3,
}

/// The upward faces of each stock prop model, by name: triangles in the model's own units.
fn models() -> &'static HashMap<String, Vec<[f32; 9]>> {
    static MODELS: OnceLock<HashMap<String, Vec<[f32; 9]>>> = OnceLock::new();
    MODELS.get_or_init(|| {
        let mut text = String::new();
        let _ = flate2::read::GzDecoder::new(&include_bytes!("../../../props.txt.gz")[..]).read_to_string(&mut text);
        text.lines()
            .filter_map(|l| {
                let mut words = l.split(' ');
                let name = words.next()?.to_string();
                let nums: Vec<f32> = words.filter_map(|w| w.parse().ok()).collect();
                let tris = nums.as_chunks::<9>().0.to_vec();
                Some((name, tris))
            })
            .collect()
    })
}

/// The map's solid static props, or none when it has no prop lump read here.
fn props(data: &[u8]) -> Result<Vec<Prop>> {
    let dir = lump(data, LUMP_GAME_LUMP)?;
    let count = index_at(&dir, 0)?;
    for i in 0..count.min(MAX_PROPS) {
        let e = 4 + i * GAME_LUMP_SIZE;
        if i32_at(&dir, e)? != SPRP {
            continue;
        }
        let (flags, version) = (u16_at(&dir, e + GAME_LUMP_FLAGS)?, u16_at(&dir, e + GAME_LUMP_VERSION)?);
        let (ofs, len) = (index_at(&dir, e + GAME_LUMP_OFFSET)?, index_at(&dir, e + GAME_LUMP_LENGTH)?);
        let Some(raw) = data.get(ofs..ofs.saturating_add(len)) else { bail!("static props out of range") };
        let lump = if flags & COMPRESSED != 0 { unpack(raw)? } else { raw.to_vec() };
        return read_props(&lump, version);
    }
    Ok(Vec::new())
}

fn read_props(b: &[u8], version: u16) -> Result<Vec<Prop>> {
    let names = index_at(b, 0)?.min(MAX_PROPS);
    let dict: Vec<String> = (0..names)
        .map(|i| {
            let raw: [u8; NAME_SIZE] = bytes(b, 4 + i * NAME_SIZE)?;
            let end = raw.iter().position(|&c| c == 0).unwrap_or(NAME_SIZE);
            Ok(String::from_utf8_lossy(&raw[..end]).to_ascii_lowercase().replace('\\', "/"))
        })
        .collect::<Result<_>>()?;
    let leaves_at = 4 + names * NAME_SIZE;
    let props_at = leaves_at + 4 + index_at(b, leaves_at)? * 2;
    let count = index_at(b, props_at)?.min(MAX_PROPS);
    let rest = b.len().saturating_sub(props_at + 4);
    let size = match rest.checked_div(count) {
        Some(s) if s >= MIN_PROP_SIZE && rest.is_multiple_of(count) => s,
        _ => prop_size(version).ok_or_else(|| anyhow::anyhow!("static props of version {version}"))?,
    };
    let mut out = Vec::new();
    for i in 0..count {
        let o = props_at + 4 + i * size;
        let [solid] = bytes(b, o + PROP_SOLID)?;
        let Some(model) = dict.get(usize::from(u16_at(b, o + PROP_TYPE)?)) else { continue };
        if solid != SOLID_NONE {
            out.push(Prop { model: model.clone(), origin: vec3_at(b, o)?, angles: vec3_at(b, o + PROP_ANGLES)? });
        }
    }
    Ok(out)
}

/// The tops of the map's solid stock props inside `bounds`: each upward face of their collision
/// meshes, placed as the map places the prop. A prop is kept when `stands(x, y)` says players get
/// to where it stands (at any height: a stack of containers is taller than what they reach).
pub(super) fn prop_floors(data: &[u8], bounds: Option<Bounds>, stands: impl Fn(f64, f64) -> bool) -> Vec<Floor> {
    let props = props(data).unwrap_or_else(|e| {
        eprintln!("stvtf-web: static props left out: {e:#}");
        Vec::new()
    });
    let models = models();
    let mut out = Vec::new();
    for p in props {
        if bounds.is_some_and(|b| !b.contains(&p.origin)) || !stands(f64::from(p.origin[0]), f64::from(p.origin[1])) {
            continue;
        }
        let Some(tris) = models.get(&p.model) else { continue };
        let turn = rotation(p.angles);
        let place = |v: &[f32]| -> Vec3 { [0, 1, 2].map(|k| p.origin[k] + turn[k][0] * v[0] + turn[k][1] * v[1] + turn[k][2] * v[2]) };
        for t in tris {
            let corners = [place(&t[0..3]), place(&t[3..6]), place(&t[6..9])];
            let Some(normal) = upward_normal(&corners) else { continue };
            let z = corners.iter().map(|q| q[2]).sum::<f32>() / 3.0;
            out.push(Floor { z: round1(z), p: outline(&corners), k: Kind::Other, d: false, g: gradient(normal) });
            if out.len() >= MAX_FLOORS {
                return out;
            }
        }
    }
    out
}

/// The normal of a triangle that faces up as a floor does, or None for one that does not (or has
/// no area).
fn upward_normal([first, second, third]: &[Vec3; 3]) -> Option<Vec3> {
    let along = [0, 1, 2].map(|k| second[k] - first[k]);
    let across = [0, 1, 2].map(|k| third[k] - first[k]);
    let normal = cross(along, across);
    let len = normal.iter().map(|x| x * x).sum::<f32>().sqrt();
    (len >= 1.0 && normal[2] / len > FLOOR_MIN_NORMAL_Z).then_some(normal)
}

/// Source's rotation for `angles` (pitch, yaw, roll in degrees): its columns are the model's
/// forward, left and up in the world.
fn rotation(angles: Vec3) -> [[f32; 3]; 3] {
    let [p, y, r] = angles.map(f32::to_radians);
    let (sp, cp, sy, cy, sr, cr) = (p.sin(), p.cos(), y.sin(), y.cos(), r.sin(), r.cos());
    [
        [cp * cy, sr * sp * cy - cr * sy, cr * sp * cy + sr * sy],
        [cp * sy, sr * sp * sy + cr * cy, cr * sp * sy - sr * cy],
        [-sp, sr * cp, cr * cp],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turned_a_quarter_forward_goes_left() {
        let m = rotation([0.0, 90.0, 0.0]);
        let x = [m[0][0], m[1][0], m[2][0]];
        assert!((x[0]).abs() < 1e-6 && (x[1] - 1.0).abs() < 1e-6, "{x:?}");
    }

    #[test]
    fn process_has_prop_tops_where_players_get_to() {
        let Some(g) = crate::maps::bsp::tests::process_built() else { return };
        assert!(!g.props.is_empty());
        assert!(g.props.iter().all(|f| f.p.len() == 6 && f.z.is_finite()), "triangles");
    }

    #[test]
    fn the_container_model_has_its_top() {
        let tris = &models()["models/props_vehicles/train_flatcar_container.mdl"];
        assert!(tris.iter().any(|t| t[2] > 140.0 && t[5] > 140.0 && t[8] > 140.0), "{tris:?}");
    }
}
