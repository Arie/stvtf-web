//! A jump arc's shape: a rocket or sticky jump from the first click to the second, straight up at
//! first, then over (the air-strafe) and down onto the landing; seen from above, straight. And
//! drawing one in a room: two clicks, launch then landing, kept with the marker.

use super::Room;
use super::beams::{beam, heads, hex};
use super::drawing::Kept;
use crate::lock;
use crate::maps::drape::Piece;
use anyhow::{Result, bail};
use serde::Deserialize;
use stvtf::relay::Beam;

/// Two clicks closer than this are no jump.
const MIN_JUMP: f64 = 16.0;
/// Arcs' ids, apart from the pages' strokes'.
const ARC_IDS: u32 = 0x8000_0000;

/// An arc as a page asks for it: the launch, then the landing.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArcDraw {
    pub(crate) from: [f64; 2],
    pub(crate) to: [f64; 2],
}

impl Room {
    /// Admin `guest` draws a jump arc from one spot to another, onto the floors under them: kept
    /// with the marker, shown to every page and in TF2.
    pub(crate) fn arc(&self, a: &ArcDraw, guest: &str) -> Result<()> {
        let Some(ground) = self.ground() else { bail!("The map is not ready yet.") };
        let spot = |[x, y]: [f64; 2]| ground.top(x, y).map(|z| [x, y, z]);
        let (Some(from), Some(to)) = (spot(a.from), spot(a.to)) else { bail!("Pick a spot on a floor.") };
        if (from[0] - to[0]).hypot(from[1] - to[1]) < MIN_JUMP {
            bail!("Too short for a jump.");
        }
        let points = jump(from, to);
        let by = lock(&self.people).id(guest);
        let ink = hex(self.ink_now(by));
        self.change_marker(|marker| {
            let stroke = ARC_IDS | marker.iter().filter(|k| k.arc.is_some()).count() as u32;
            let line = serde_json::json!({ "type": "arc", "by": by, "stroke": stroke, "ink": ink, "points": points });
            let clicks = vec![[a.from[0], a.from[1], 0.0], [a.to[0], a.to[1], 0.0]];
            marker.push(Kept { stroke, by, points: clicks, ink: Some(ink), open: false, arc: Some(points), circle: None });
            line.to_string().into()
        })
    }
}

/// Pieces the arc is made of: the first that go straight up, then those over.
const PIECES: usize = 16;
const UP_PIECES: usize = PIECES / 4;
const OVER_PIECES: usize = PIECES - UP_PIECES;
/// Halvings in the search for the handles' height: past an `f64`'s precision.
const HALVINGS: usize = 60;
/// How much of its climb goes straight up.
const STRAIGHT: f64 = 1.0 / 3.0;
/// Where along the way the curve's second handle is, at its height.
const HANDLE: f64 = 0.6;

/// How far a soldier's jump rises above the higher of its spots, by how far it goes (flat): the
/// RGL LAN final's medians, flatter the longer it goes.
const RISES: [(f64, f64); 4] = [(0.0, 64.0), (400.0, 100.0), (800.0, 170.0), (1100.0, 220.0)];

fn rise(distance: f64) -> f64 {
    RISES
        .windows(2)
        .find(|w| distance < w[1].0)
        .map_or(RISES[RISES.len() - 1].1, |w| w[0].1 + (w[1].1 - w[0].1) * (distance - w[0].0) / (w[1].0 - w[0].0))
}

/// The arc from `launch` to `land`, up or down: its points, the ends those spots.
pub(super) fn jump(launch: [f64; 3], land: [f64; 3]) -> Vec<[f64; 3]> {
    let peak = launch[2].max(land[2]) + rise((land[0] - launch[0]).hypot(land[1] - launch[1]));
    let up = launch[2] + (peak - launch[2]) * STRAIGHT;
    let mut out: Vec<[f64; 3]> =
        (0..=UP_PIECES).map(|i| [launch[0], launch[1], launch[2] + (up - launch[2]) * i as f64 / UP_PIECES as f64]).collect();
    let handle = handle_for(up, peak, land[2]);
    for i in 1..=OVER_PIECES {
        let (along, z) = curve(i as f64 / OVER_PIECES as f64, up, handle, land[2]);
        out.push([launch[0] + (land[0] - launch[0]) * along, launch[1] + (land[1] - launch[1]) * along, z]);
    }
    out
}

/// The curve's point at `t`: how far along and how high, from (0, `up`) via handles (0, `handle`)
/// and (`HANDLE`, `handle`) to (1, `land`).
fn curve(t: f64, up: f64, handle: f64, land: f64) -> (f64, f64) {
    let s = 1.0 - t;
    let (b1, b2, b3) = (3.0 * s * s * t, 3.0 * s * t * t, t * t * t);
    (b2 * HANDLE + b3, s * s * s * up + (b1 + b2) * handle + b3 * land)
}

/// The handles' height that brings the curve's highest point to `peak`.
fn handle_for(up: f64, peak: f64, land: f64) -> f64 {
    let top = |handle: f64| (1..=OVER_PIECES).map(|i| curve(i as f64 / OVER_PIECES as f64, up, handle, land).1).fold(up, f64::max);
    let (mut lo, mut hi) = (peak, peak + 4.0 * (peak - up) + (peak - land));
    for _ in 0..HALVINGS {
        let mid = f64::midpoint(lo, hi);
        if top(mid) < peak { lo = mid } else { hi = mid }
    }
    hi
}

/// A jump arc's beams are this far above its points, clear of the floors it starts and ends on.
const ARC_LIFT: f64 = 8.0;

/// A jump arc's beams, in `ink`: a piece each, and an arrowhead where it lands.
pub(super) fn arc_beams(points: &[[f64; 3]], ink: [u8; 3], life: f32) -> Vec<Beam> {
    let lifted: Vec<[f64; 3]> = points.iter().map(|p| [p[0], p[1], p[2] + ARC_LIFT]).collect();
    let mut out: Vec<Beam> = lifted.windows(2).map(|w| beam(w[0], w[1], ink, life, 0)).collect();
    if let [.., from, to] = lifted.as_slice() {
        out.extend(heads(&[Piece { from: *from, to: *to, ms: 0.0 }], ink, life));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f64; 3], b: [f64; 3]) -> bool {
        (0..3).all(|k| (a[k] - b[k]).abs() < 0.01)
    }

    #[test]
    fn it_goes_from_the_first_click_to_the_second_up_or_down() {
        let (low, high) = ([0.0, 0.0, 0.0], [300.0, 0.0, 200.0]);
        let (up, down) = (jump(low, high), jump(high, low));
        assert_eq!(up.len(), 17);
        assert!(close(up[0], low) && close(up[16], high));
        assert!(close(down[0], high) && close(down[16], low), "a jump down off the high ground");
        assert!(down[1][2] > high[2], "straight up first, down too");
    }

    #[test]
    fn it_climbs_straight_up_first() {
        let arc = jump([10.0, 20.0, 0.0], [300.0, 0.0, 200.0]);
        assert_eq!([arc[1][0], arc[1][1]], [10.0, 20.0]);
        assert!(arc[1][2] > 0.0);
    }

    #[test]
    fn it_rises_as_soldiers_jumps_do_flatter_the_longer_they_go() {
        // The RGL LAN final's medians: about 70 up on a short hop, 220 or so from 1100 on.
        assert!((rise(0.0) - 64.0).abs() < 1.0 && (rise(1100.0) - 220.0).abs() < 1.0);
        assert!((rise(3000.0) - 220.0).abs() < 1.0, "no higher for longer");
        let flatness = |d: f64| rise(d) / d;
        assert!(flatness(300.0) > flatness(800.0) && flatness(800.0) > flatness(2000.0));
    }

    #[test]
    fn it_rises_above_the_landing_and_comes_down_onto_it() {
        let arc = jump([0.0, 0.0, 0.0], [300.0, 0.0, 200.0]);
        let peak = arc.iter().map(|p| p[2]).fold(f64::MIN, f64::max);
        assert!((peak - (200.0 + rise(300.0))).abs() < 1.0, "{peak}");
        assert!(arc[15][2] > arc[16][2] && arc[14][2] > arc[15][2], "falling onto it");
    }

    #[test]
    fn seen_from_above_it_is_straight() {
        let (a, b) = ([0.0, 0.0, 0.0], [300.0, 400.0, 200.0]);
        let mut last = -1.0;
        for p in jump(a, b) {
            let along = (p[0] * 300.0 + p[1] * 400.0) / 250_000.0;
            let off = [p[0] - along * 300.0, p[1] - along * 400.0];
            assert!(off[0].abs() < 0.01 && off[1].abs() < 0.01, "{p:?} off the line");
            assert!(along >= last - 1e-9, "never back");
            last = along;
        }
    }
    #[test]
    fn an_arc_is_a_beam_per_piece_and_an_arrowhead() {
        let arc = jump([0.0, 0.0, 0.0], [300.0, 0.0, 200.0]);
        let beams = arc_beams(&arc, [255, 0, 0], 2.0);
        assert_eq!(beams.len(), 16 + 6, "a beam per piece, then the arrowhead's");
        assert!(beams.iter().all(|b| b.rgb == [255, 0, 0]), "in its ink, whatever its height");
    }
}
