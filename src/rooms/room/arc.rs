//! A jump arc's shape: a rocket or sticky jump from low ground onto high ground, straight up at
//! first, then over (the air-strafe) and down onto the high spot; seen from above, straight. And
//! drawing one in a room: two clicks, kept with the marker.

use super::Room;
use super::beams::hex;
use super::drawing::{Kept, MAX_KEPT_POINTS, kept_line};
use crate::lock;
use crate::matches::hub::send_keeping;
use anyhow::{Result, bail};
use serde::Deserialize;

/// Two clicks closer than this are no jump.
const MIN_JUMP: f64 = 16.0;
/// Arcs' ids, apart from the pages' strokes'.
const ARC_IDS: u32 = 0x8000_0000;

/// An arc as a page asks for it: the two clicks, either order.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArcDraw {
    pub(crate) from: [f64; 2],
    pub(crate) to: [f64; 2],
}

impl Room {
    /// Admin `guest` draws a jump arc between two spots, onto the floors under them: kept with the
    /// marker, shown to every page and in TF2.
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
        let mut marker = lock(&self.marker);
        if marker.iter().map(|k| k.points.len()).sum::<usize>() + 2 > MAX_KEPT_POINTS {
            bail!("The map is full of marker ({MAX_KEPT_POINTS} points): clear it first.");
        }
        let before = marker.clone();
        let stroke = ARC_IDS | marker.iter().filter(|k| k.arc.is_some()).count() as u32;
        let clicks = vec![[a.from[0], a.from[1], 0.0], [a.to[0], a.to[1], 0.0]];
        let kept = Kept { stroke, by, points: clicks, ink: Some(ink.clone()), open: false, arc: Some(points.clone()) };
        marker.push(kept);
        let beams = self.marker_in_tf2(&marker).inspect_err(|_| *marker = before)?;
        let line = serde_json::json!({ "type": "arc", "by": by, "stroke": stroke, "ink": ink, "points": points });
        send_keeping(&self.hub, &line.to_string().into(), "marker", kept_line(&marker));
        drop(marker);
        self.hand_marker(beams);
        Ok(())
    }
}

/// Pieces the arc is made of, and of those the first that go straight up.
const PIECES: usize = 16;
const UP_PIECES: usize = PIECES / 4;
/// How far the arc rises above the higher spot, and how much of its climb goes straight up.
const RISE: f64 = 128.0;
const STRAIGHT: f64 = 1.0 / 3.0;
/// Where along the way the curve's second handle is, at its height.
const HANDLE: f64 = 0.6;

/// The arc from the lower of `a` and `b` (`a` when as high) to the other: its points, the ends
/// those spots.
pub(super) fn jump(a: [f64; 3], b: [f64; 3]) -> Vec<[f64; 3]> {
    let (launch, land) = if b[2] < a[2] { (b, a) } else { (a, b) };
    let peak = launch[2].max(land[2]) + RISE;
    let up = launch[2] + (peak - launch[2]) * STRAIGHT;
    let mut out: Vec<[f64; 3]> =
        (0..=UP_PIECES).map(|i| [launch[0], launch[1], launch[2] + (up - launch[2]) * i as f64 / UP_PIECES as f64]).collect();
    let handle = handle_for(up, peak, land[2]);
    for i in 1..=PIECES - UP_PIECES {
        let (along, z) = curve(i as f64 / (PIECES - UP_PIECES) as f64, up, handle, land[2]);
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
    let top = |handle: f64| {
        (1..=PIECES - UP_PIECES).map(|i| curve(i as f64 / (PIECES - UP_PIECES) as f64, up, handle, land).1).fold(up, f64::max)
    };
    let (mut lo, mut hi) = (peak, peak + 4.0 * RISE + (peak - land));
    for _ in 0..60 {
        let mid = f64::midpoint(lo, hi);
        if top(mid) < peak { lo = mid } else { hi = mid }
    }
    hi
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f64; 3], b: [f64; 3]) -> bool {
        (0..3).all(|k| (a[k] - b[k]).abs() < 0.01)
    }

    #[test]
    fn it_goes_from_the_low_spot_to_the_high_one_either_way() {
        let (low, high) = ([0.0, 0.0, 0.0], [300.0, 0.0, 200.0]);
        let (one, other) = (jump(low, high), jump(high, low));
        assert_eq!(one.len(), 17);
        assert!(one.iter().zip(&other).all(|(a, b)| close(*a, *b)));
        assert!(close(one[0], low) && close(one[16], high));
    }

    #[test]
    fn it_climbs_straight_up_first() {
        let arc = jump([10.0, 20.0, 0.0], [300.0, 0.0, 200.0]);
        assert_eq!([arc[1][0], arc[1][1]], [10.0, 20.0]);
        assert!(arc[1][2] > 0.0);
    }

    #[test]
    fn it_rises_above_the_landing_and_comes_down_onto_it() {
        let arc = jump([0.0, 0.0, 0.0], [300.0, 0.0, 200.0]);
        let peak = arc.iter().map(|p| p[2]).fold(f64::MIN, f64::max);
        assert!((peak - (200.0 + RISE)).abs() < 1.0, "{peak}");
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
}
