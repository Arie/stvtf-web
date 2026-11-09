use super::*;

/// A flat or sloped box of floor from (x0, y0) to (x1, y1), `z` high at its middle.
fn floor(x0: f64, y0: f64, x1: f64, y1: f64, z: f64, g: Option<[f64; 2]>) -> Floor {
    Floor { z, p: vec![x0, y0, x1, y0, x1, y1, x0, y1], g }
}

/// A stroke along y = 0 from x = 0 to `to`, a point every 32 units, 10 ms apart.
fn along(to: f64) -> Vec<[f64; 3]> {
    (0..=(to / 32.0) as usize).map(|i| [i as f64 * 32.0, 0.0, i as f64 * 10.0]).collect()
}

/// The pieces joined into lines: each a run of pieces, end to start.
fn lines(pieces: &[Piece]) -> Vec<Vec<[f64; 3]>> {
    let mut out: Vec<Vec<[f64; 3]>> = Vec::new();
    for p in pieces {
        match out.iter_mut().find(|l| l.last() == Some(&p.from)) {
            Some(l) => l.push(p.to),
            None => out.push(vec![p.from, p.to]),
        }
    }
    out
}

fn drape(floors: Vec<Floor>, points: &[[f64; 3]]) -> Vec<Vec<[f64; 3]>> {
    lines(&Drape::new().push(&Ground::new(floors), points))
}

#[test]
fn a_flat_floor_is_one_straight_piece_above_it() {
    let got = Drape::new().push(&Ground::new(vec![floor(-64.0, -64.0, 1024.0, 64.0, 100.0, None)]), &along(512.0));
    assert_eq!(got, vec![Piece { from: [0.0, 0.0, 108.0], to: [512.0, 0.0, 108.0], ms: 160.0 }]);
}

#[test]
fn under_a_balcony_the_stroke_is_on_both() {
    let got = drape(vec![floor(-64.0, -64.0, 1024.0, 64.0, 0.0, None), floor(256.0, -64.0, 512.0, 64.0, 128.0, None)], &along(768.0));
    assert_eq!(got.len(), 2, "{got:?}");
    let low = got.iter().find(|l| (l[0][2] - 8.0).abs() < 0.01).unwrap();
    assert_eq!((low[0][0], low.last().unwrap()[0]), (0.0, 768.0), "the ground all the way");
    let high = got.iter().find(|l| (l[0][2] - 136.0).abs() < 0.01).unwrap();
    assert!(high[0][0] >= 256.0 && high.last().unwrap()[0] <= 512.0, "the balcony over its length");
}

#[test]
fn a_ramp_is_followed_up() {
    // Rising half a unit per unit along x, 64 high at x = 128.
    let got = drape(vec![floor(-64.0, -64.0, 320.0, 64.0, 64.0, Some([0.5, 0.0]))], &along(256.0));
    assert_eq!(got, vec![vec![[0.0, 0.0, 8.0], [256.0, 0.0, 136.0]]]);
}

#[test]
fn stairs_are_one_line_a_cliff_two() {
    let stairs: Vec<Floor> =
        (0..8).map(|i| floor(f64::from(i) * 64.0, -64.0, f64::from(i + 1) * 64.0, 64.0, f64::from(i) * 16.0, None)).collect();
    assert_eq!(drape(stairs, &along(512.0)).len(), 1);
    let cliff = vec![floor(-64.0, -64.0, 256.0, 64.0, 0.0, None), floor(256.0, -64.0, 600.0, 64.0, 256.0, None)];
    assert_eq!(drape(cliff, &along(512.0)).len(), 2);
}

#[test]
fn a_wall_is_crossed_a_gap_ends_the_line() {
    let across = |gap: f64| {
        drape(vec![floor(-64.0, -64.0, 256.0, 64.0, 0.0, None), floor(256.0 + gap, -64.0, 768.0, 64.0, 0.0, None)], &along(768.0))
    };
    assert_eq!(across(32.0).len(), 1, "over a wall");
    assert_eq!(across(160.0).len(), 2, "across a pit");
}

#[test]
fn a_crate_top_too_small_to_matter_is_left_out() {
    let got = drape(vec![floor(-64.0, -64.0, 1024.0, 64.0, 0.0, None), floor(300.0, -64.0, 320.0, 64.0, 48.0, None)], &along(512.0));
    assert_eq!(got.len(), 1, "{got:?}");
}

#[test]
fn a_stroke_in_pieces_carries_on() {
    let ground = Ground::new(vec![floor(-64.0, -64.0, 1024.0, 64.0, 0.0, None)]);
    let mut d = Drape::new();
    let points = along(512.0);
    let mut got = d.push(&ground, &points[..8]);
    got.extend(d.push(&ground, &points[8..]));
    assert_eq!(lines(&got), vec![vec![[0.0, 0.0, 8.0], [224.0, 0.0, 8.0], [512.0, 0.0, 8.0]]]);
}

#[test]
fn off_the_map_nothing_is_drawn() {
    assert_eq!(Drape::new().push(&Ground::new(Vec::new()), &along(512.0)), []);
    assert_eq!(Drape::new().push(&Ground::new(vec![floor(0.0, 0.0, 64.0, 64.0, 0.0, None)]), &[[1e5, 1e5, 0.0], [1e5, -1e5, 9.0]]), []);
}

#[test]
fn on_process_every_piece_is_just_above_a_floor() {
    let Some(g) = crate::maps::bsp::tests::process_built() else { return };
    let ground = Ground::from_json(&serde_json::to_vec(g).unwrap()).unwrap();
    // From one end's capture area to the other's, through the middle.
    let mid = |a: &crate::maps::bsp::CapArea| [f64::midpoint(a.min[0], a.max[0]), f64::midpoint(a.min[1], a.max[1])];
    let ends: Vec<[f64; 2]> = g.cap_areas.iter().map(mid).collect();
    let (a, b) = (ends.iter().min_by(|p, q| p[0].total_cmp(&q[0])).unwrap(), ends.iter().max_by(|p, q| p[0].total_cmp(&q[0])).unwrap());
    let points: Vec<[f64; 3]> =
        (0..=100).map(|i| f64::from(i) / 100.0).map(|t| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, t * 5000.0]).collect();
    let pieces = Drape::new().push(&ground, &points);
    let length: f64 = pieces.iter().map(|p| dist(p.from, p.to)).sum();
    let across = (b[0] - a[0]).hypot(b[1] - a[1]);
    assert!(length > across * 0.8, "{length:.0} of lines for {across:.0} across");
    assert!(pieces.len() < 400, "{} pieces", pieces.len());
    for p in &pieces {
        for q in [p.from, p.to] {
            let on = ground.heights(q[0], q[1]).iter().any(|z| (q[2] - LIFT - z).abs() < 1.0);
            assert!(on, "{q:?} over {:?}", ground.heights(q[0], q[1]));
        }
    }
}

/// Geometry JSON with these floors, the bounds' top `hi` high.
fn json(floors: &[Floor], hi: f64) -> Vec<u8> {
    let fl: Vec<serde_json::Value> = floors.iter().map(|f| serde_json::json!({ "z": f.z, "p": f.p })).collect();
    serde_json::to_vec(&serde_json::json!({ "floors": fl, "bounds": [[0.0, 0.0, -100.0], [1000.0, 1000.0, hi]] })).unwrap()
}

#[test]
fn the_top_floor_is_the_highest_under_the_roof() {
    let floors = [0.0, 128.0, 4000.0].map(|z| floor(0.0, 0.0, 256.0, 256.0, z, None));
    // The roof cut is 2000 - 1536 + 400 = 864: the floor at 4000 is a roof the overview leaves out.
    let ground = Ground::from_json(&json(&floors, 2000.0)).unwrap();
    assert_eq!(ground.top(100.0, 100.0), Some(128.0));
}

#[test]
fn no_floor_no_top() {
    let ground = Ground::from_json(&json(&[floor(0.0, 0.0, 256.0, 256.0, 0.0, None)], 2000.0)).unwrap();
    assert_eq!(ground.top(5000.0, 5000.0), None);
}
