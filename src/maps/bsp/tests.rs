use std::sync::OnceLock;

use stvtf::output::{PickupItem, Team};

use super::entities::{Entities, playable_bounds, spawns};
use super::floors::{fill, outline_area, without_shallow_water};
use super::format::{
    FACE_FIRST_EDGE, FACE_NUM_EDGES, FACE_SIZE, FACE_TEXINFO, LUMP_BRUSHES, LUMP_BRUSHSIDES, LUMP_DISP_VERTS, LUMP_DISPINFO, LUMP_EDGES,
    LUMP_FACES, LUMP_LEAFBRUSHES, LUMP_LEAFS, LUMP_LENGTH, LUMP_MODELS, LUMP_NODES, LUMP_OFFSET, LUMP_PLANES, LUMP_SURFEDGES, LUMP_TEXDATA,
    LUMP_TEXINFO, LUMP_VERTEXES, LZMA_HEADER, LZMA_MAGIC, MAX_LUMP, index_at, lump, lump_entry,
};
use super::geometry::{Floor, Geometry};
use super::material::Kind;
use super::polygon::contains_point;
use super::read::{Bounds, geometry, geometry_counted, gradient};

/// The map `name` from `testdata/maps`, or None when it is not there: the test then skips
/// itself, except where the maps are required (CI).
pub(super) fn map(name: &str) -> Option<Vec<u8>> {
    let path = format!("{}/testdata/maps/{name}.bsp", env!("CARGO_MANIFEST_DIR"));
    let data = std::fs::read(&path).ok();
    if data.is_none() {
        missing(&path, std::env::var_os("CI").is_some());
    }
    data
}

fn missing(path: &str, required: bool) {
    assert!(!required, "{path} is missing");
    eprintln!("skipping: {path} is missing");
}

/// The geometry of `cp_process_f12`, built once for the tests that only read it.
pub(crate) fn process_built() -> Option<&'static Geometry> {
    static BUILT: OnceLock<Option<Geometry>> = OnceLock::new();
    BUILT.get_or_init(|| process().map(|data| geometry(&data).unwrap())).as_ref()
}

/// The geometry of `pl_swiftwater_final1`, built once for the tests that only read it.
pub(super) fn swiftwater_built() -> Option<&'static Geometry> {
    static BUILT: OnceLock<Option<Geometry>> = OnceLock::new();
    BUILT.get_or_init(|| swiftwater().map(|data| geometry(&data).unwrap())).as_ref()
}

#[test]
#[should_panic(expected = "is missing")]
fn a_missing_map_fails_where_maps_are_required() {
    missing("nowhere.bsp", true);
}

pub(crate) fn process() -> Option<Vec<u8>> {
    map("cp_process_f12")
}

pub(super) fn swiftwater() -> Option<Vec<u8>> {
    map("pl_swiftwater_final1")
}

#[test]
fn a_slope_rises_as_its_normal_leans_and_a_flat_floor_has_none() {
    assert_eq!(gradient([0.0, -0.6, 0.8]), Some([0.0, 0.75]));
    assert_eq!(gradient([0.0, 0.0, 1.0]), None);
}

#[test]
fn process_has_ramps_and_terrain_with_slopes() {
    let Some(g) = process_built() else { return };
    assert!(g.floors.iter().any(|f| !f.d && f.g.is_some()), "a sloped brush floor");
    assert!(g.floors.iter().any(|f| f.d && f.g.is_some()), "sloped terrain");
    assert!(g.floors.iter().filter_map(|f| f.g).all(|[x, y]| x.hypot(y) < 1.1), "within 45° or so");
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

/// Points lump `index` at `contents`, appended unpacked to `data`.
pub(super) fn replace_lump(data: &mut Vec<u8>, index: usize, contents: &[u8]) {
    let h = lump_entry(index);
    let (ofs, len) = (data.len() as i32, contents.len() as i32);
    data.extend_from_slice(contents);
    set_i32(data, h + LUMP_OFFSET, ofs);
    set_i32(data, h + LUMP_LENGTH, len);
}

// Broken maps must be an error, not a panic: a panic while serving `/map/NAME` would
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
    // Every face claims the same 65535 edges.
    let mut faces = lump(&data, LUMP_FACES).unwrap();
    for face in faces.as_chunks_mut::<FACE_SIZE>().0 {
        face[FACE_FIRST_EDGE..FACE_NUM_EDGES].fill(0);
        face[FACE_NUM_EDGES..FACE_TEXINFO].copy_from_slice(&u16::MAX.to_le_bytes());
    }
    replace_lump(&mut data, LUMP_FACES, &faces);
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
        LUMP_NODES,
        LUMP_LEAFS,
        LUMP_LEAFBRUSHES,
        LUMP_BRUSHES,
        LUMP_BRUSHSIDES,
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
    let Some(g) = process_built() else { return };
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
    let Some(g) = process_built() else { return };
    let of = |k| g.floors.iter().filter(|f| f.k == k).count();
    assert!(of(Kind::Concrete) + of(Kind::Metal) > 100, "{} concrete, {} metal", of(Kind::Concrete), of(Kind::Metal));
    assert!(of(Kind::Other) < g.floors.len(), "every floor is other");
    // A (convex) floor contains its own middle.
    let f = g.floors.iter().find(|f| f.k == Kind::Metal).unwrap();
    let n = (f.p.len() / 2) as f64;
    let mid = |i: usize| f.p.iter().skip(i).step_by(2).sum::<f64>() / n;
    assert!(contains_point(&f.p, mid(0), mid(1)), "{:?}", f.p);
}

#[test]
fn swiftwater_trench_has_no_water() {
    let Some(g) = swiftwater_built() else { return };
    assert!(!g.floors.iter().any(|f| f.k == Kind::Water), "the slab under the trench bed is buried");
}

/// A 256-unit square floor at height `z`.
fn sq(z: f64, k: Kind) -> Floor {
    Floor { z, p: vec![0.0, 0.0, 256.0, 0.0, 256.0, 256.0, 0.0, 256.0], k, d: false, g: None }
}

#[test]
fn deep_water_is_kept() {
    let deep = without_shallow_water(vec![sq(-100.0, Kind::Dirt), sq(0.0, Kind::Water)]);
    assert_eq!(deep.len(), 2);
    let shallow = without_shallow_water(vec![sq(-10.0, Kind::Dirt), sq(0.0, Kind::Water)]);
    assert_eq!(shallow.len(), 1);
}

#[test]
fn buried_water_and_water_over_nothing_are_dropped() {
    // A floor within 8 units above the water buries it, however deep the bed under it.
    let buried = without_shallow_water(vec![sq(-100.0, Kind::Dirt), sq(5.0, Kind::Dirt), sq(0.0, Kind::Water)]);
    assert_eq!(buried.len(), 2);
    // A floor above that is out of the way.
    let open = without_shallow_water(vec![sq(-100.0, Kind::Dirt), sq(50.0, Kind::Dirt), sq(0.0, Kind::Water)]);
    assert_eq!(open.len(), 3);
    // No floor under the water at all: not deep.
    assert_eq!(without_shallow_water(vec![sq(0.0, Kind::Water)]).len(), 0);
}

#[test]
fn floors_under_spawns_and_packs_are_kept() {
    let mut floors = vec![sq(0.0, Kind::Dirt), sq(100.0, Kind::Dirt), sq(200.0, Kind::Dirt), sq(300.0, Kind::Dirt)];
    // The fill keeps the top one; a pack 40 units above the second, one 70 above the third.
    let stands = [[128.0, 128.0, 140.0], [128.0, 128.0, 270.0]];
    assert!(fill(&mut floors, |f| f.z > 250.0, &stands, 4.0 * 256.0 * 256.0));
    assert_eq!(floors.iter().map(|f| f.z).collect::<Vec<_>>(), [100.0, 300.0]);
}

#[test]
fn a_fill_keeping_almost_nothing_keeps_every_floor() {
    // Seeds in a sealed pocket: the fill keeps one floor in eleven.
    let mut floors: Vec<Floor> = (0..11).map(|i| sq(f64::from(i) * 100.0, Kind::Dirt)).collect();
    let area = 11.0 * 256.0 * 256.0;
    assert!(!fill(&mut floors, |f| f.z < 50.0, &[], area));
    assert_eq!(floors.len(), 11);
    // One in ten is enough.
    floors.pop();
    assert!(fill(&mut floors, |f| f.z < 50.0, &[], area - 256.0 * 256.0));
    assert_eq!(floors.len(), 1);
}

#[test]
fn many_big_water_floors_are_bounded() {
    // Thousands of map-sized water faces over as many shallow beds: every sample would scan
    // every bed, for hours. The budget runs out instead, and the water left is kept.
    let big = |z: f64, k: Kind| Floor { z, p: vec![0.0, 0.0, 30000.0, 0.0, 30000.0, 30000.0, 0.0, 30000.0], k, d: false, g: None };
    let mut floors: Vec<Floor> = (0..4000).map(|i| if i % 2 == 0 { big(-10.0, Kind::Dirt) } else { big(0.0, Kind::Water) }).collect();
    // And one water face with a million outline points, which every sample is tested against.
    let ring = (0..1_000_000).flat_map(|i| {
        let a = f64::from(i) / 1_000_000.0 * std::f64::consts::TAU;
        [15000.0 + 15000.0 * a.cos(), 15000.0 + 15000.0 * a.sin()]
    });
    floors.insert(0, Floor { z: 0.0, p: ring.collect(), k: Kind::Water, d: false, g: None });
    let t = std::time::Instant::now();
    let kept = without_shallow_water(floors);
    eprintln!("{:.2?}", t.elapsed());
    assert!(t.elapsed() < std::time::Duration::from_secs(10), "{:.2?}", t.elapsed());
    assert!(kept.iter().filter(|f| f.k == Kind::Water).count() > 1000);
    assert!(kept[0].p.len() == 2_000_000, "the huge water face is kept");
}

#[test]
fn maps_without_points_or_pickups_are_framed_by_their_floors() {
    let Some(data) = map("mge_training_v8_beta4b") else { return };
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
    let Some(g) = process_built() else { return };
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
    let Some(data) = map("pl_upward_f12") else { return };
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
    // Last spawns belong to the team's own last point, forward spawns to mid and the enemy
    // second.
    assert_eq!(count(Team::Red, 4), 16);
    assert_eq!(count(Team::Blue, 0), 16);
    assert_eq!((count(Team::Red, 2), count(Team::Red, 1)), (3, 3));
    assert_eq!((count(Team::Blue, 2), count(Team::Blue, 3)), (3, 3));
}

#[test]
fn gullywash_points_numbered_from_one_map_to_the_games_indices() {
    let Some(data) = map("cp_gullywash_f9") else { return };
    let g = geometry(&data).unwrap();
    // Blu's last point is index 0, at x 3768; red's last is 4, at x -3766.
    let x = |i: i64| g.cap_areas.iter().find(|a| a.point == Some(i)).map(|a| a.min[0]);
    assert!(x(0).unwrap() > 3000.0 && x(4).unwrap() < -3000.0);
    assert!(spawns(&Entities::read(&data).unwrap()).points.iter().all(|p| p.point.is_none_or(|i| (0..5).contains(&i))));
}

#[test]
fn swiftwater_teamless_spawn_rooms_take_their_spawn_points_team() {
    let Some(data) = map("pl_swiftwater_final1") else { return };
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

/// Builds every map in `cache/`, for a look at how the playable-area filter and the material
/// kinds do on real maps. Run: `cargo test --release league_maps_build -- --ignored
/// --nocapture`.
#[test]
#[ignore = "needs the maps in cache/; run by hand"]
fn league_maps_build() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/cache");
    let mut names: Vec<String> =
        std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()?.file_name().to_str()?.strip_suffix(".bsp").map(str::to_owned)).collect();
    names.sort();
    let mut failed = Vec::new();
    for name in &names {
        let data = std::fs::read(format!("{dir}/{name}.bsp")).unwrap();
        let start = std::time::Instant::now();
        let (g, before) = match geometry_counted(&data) {
            Ok(r) => r,
            Err(e) => {
                println!("{name}: ERROR {e:#}");
                failed.push(format!("{name}: {e:#}"));
                continue;
            }
        };
        let took = start.elapsed();
        let mut shares: std::collections::BTreeMap<String, f64> = std::collections::BTreeMap::new();
        let mut total = 0.0;
        for f in &g.floors {
            let a = outline_area(&f.p);
            let kind = serde_json::to_value(f.k).unwrap().as_str().unwrap().to_owned();
            *shares.entry(kind).or_default() += a;
            total += a;
        }
        let kinds: Vec<String> = shares.iter().map(|(k, a)| format!("{k} {:.0}%", 100.0 * a / total.max(1.0))).collect();
        println!(
            "{name}: floors {} -> {} ({:.0}% by count, {:.0}% by area), playable {}, track {}, {} ms | {}",
            before.0,
            g.floors.len(),
            100.0 * g.floors.len() as f64 / before.0.max(1) as f64,
            100.0 * total / before.1.max(1.0),
            if g.playable.is_some() { "yes" } else { "NO" },
            g.track.len(),
            took.as_millis(),
            kinds.join(", ")
        );
        if total < before.1 * 0.15 {
            failed.push(format!("{name}: only {:.0}% of the floor area left", 100.0 * total / before.1));
        }
    }
    assert!(failed.is_empty(), "{failed:#?}");
}
