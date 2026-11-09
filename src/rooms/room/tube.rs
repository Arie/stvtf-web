//! A stroke's look in TF2: a tube of beams along its lines, ringed now and then, ending in an
//! arrowhead.

use crate::maps::drape::Piece;
use stvtf::relay::Beam;

/// How wide the beams are, in units.
const WIDTH: f32 = 5.0;
/// The tube: its lines around the stroke's, how far out, and how high its middle is above the
/// stroke's line (which is just above the floor).
const LINES: usize = 6;
const RADIUS: f64 = 12.0;
const RAISE: f64 = 16.0;
/// A ring round the tube at each piece's ends and at least this often.
const RING_EVERY: f64 = 128.0;
/// How fast the lines' texture runs toward the stroke's end, showing which way it goes.
const SCROLL: u8 = 20;
/// The arrowhead at a finished stroke's end: its wings' length and angle off the line.
const HEAD: f64 = 96.0;
const HEAD_ANGLE: f64 = 30.0;

type Vec3 = [f64; 3];

/// A beam of `ink` from `from` to `to`, shown for `life` seconds, its texture running at `speed`.
fn beam(from: Vec3, to: Vec3, ink: [u8; 3], life: f32, speed: u8) -> Beam {
    Beam { from: from.map(|v| v as f32), to: to.map(|v| v as f32), rgb: ink, life, width: WIDTH, speed }
}

fn add(a: Vec3, b: Vec3, k: f64) -> Vec3 {
    [0, 1, 2].map(|i| a[i] + b[i] * k)
}

fn unit(v: Vec3) -> Option<Vec3> {
    let len = v.iter().map(|x| x * x).sum::<f64>().sqrt();
    (len > 1e-9).then(|| v.map(|x| x / len))
}

fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// Two directions square to `along` and each other: the first level, the second as far up as it
/// can be. None for a piece with no length or straight up.
fn across(along: Vec3) -> Option<(Vec3, Vec3)> {
    let side = unit(cross(along, [0.0, 0.0, 1.0]))?;
    Some((side, cross(side, along)))
}

/// The middle of the tube over `p`.
fn middle(p: Vec3) -> Vec3 {
    [p[0], p[1], p[2] + RAISE]
}

/// Where the tube's line `k` runs, off the middle, for a piece going `along`.
fn around(k: usize, (side, up): (Vec3, Vec3)) -> Vec3 {
    let angle = std::f64::consts::TAU * k as f64 / LINES as f64;
    add(side.map(|v| v * angle.cos()), up, angle.sin())
}

/// `pieces` as a tube of beams of `ink`, shown for `life` seconds: its lines' texture running
/// toward the stroke's end, its rings still.
pub(super) fn tube(pieces: &[Piece], ink: [u8; 3], life: f32) -> Vec<Beam> {
    let mut out = Vec::new();
    for p in pieces {
        let (from, to) = (middle(p.from), middle(p.to));
        let Some(along) = unit([0, 1, 2].map(|i| to[i] - from[i])) else { continue };
        let Some(frame) = across(along) else { continue };
        for k in 0..LINES {
            let off = around(k, frame);
            out.push(beam(add(from, off, RADIUS), add(to, off, RADIUS), ink, life, SCROLL));
        }
        let length = (0..3).map(|i| (to[i] - from[i]).powi(2)).sum::<f64>().sqrt();
        let rings = (length / RING_EVERY).ceil().max(1.0) as usize;
        for r in 0..=rings {
            let at = add(from, along, length * r as f64 / rings as f64);
            for k in 0..LINES {
                let (a, b) = (around(k, frame), around((k + 1) % LINES, frame));
                out.push(beam(add(at, a, RADIUS), add(at, b, RADIUS), ink, life, 0));
            }
        }
    }
    out
}

/// An arrowhead on the end of each of `ends`: four wings back from the tip, to the sides and up
/// and down, so it reads from any side.
pub(super) fn heads(ends: &[Piece], ink: [u8; 3], life: f32) -> Vec<Beam> {
    let mut out = Vec::new();
    for p in ends {
        let (from, tip) = (middle(p.from), middle(p.to));
        let Some(along) = unit([0, 1, 2].map(|i| tip[i] - from[i])) else { continue };
        let Some((side, up)) = across(along) else { continue };
        let (s, c) = HEAD_ANGLE.to_radians().sin_cos();
        for wing in [side, side.map(|v| -v), up, up.map(|v| -v)] {
            let back = add(along.map(|v| -v * c), wing, s);
            out.push(beam(add(tip, back, HEAD), tip, ink, life, 0));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn piece(from: Vec3, to: Vec3) -> Piece {
        Piece { from, to, ms: 0.0 }
    }

    #[test]
    fn a_piece_is_a_tube_of_lines_round_it_with_rings() {
        let got = tube(&[piece([0.0, 0.0, 8.0], [200.0, 0.0, 8.0])], [1, 2, 3], 4.0);
        let (lines, rings): (Vec<&Beam>, Vec<&Beam>) = got.iter().partition(|b| b.speed > 0);
        assert_eq!(lines.len(), LINES);
        for b in &lines {
            let off = (f64::from(b.from[1]).powi(2) + (f64::from(b.from[2]) - 24.0).powi(2)).sqrt();
            assert!((off - RADIUS).abs() < 0.01, "{RADIUS} off the middle, 24 up: {b:?}");
            assert!(b.to[0] > b.from[0], "running its way");
        }
        assert_eq!(rings.len(), 3 * LINES, "at both ends and one between, 200 long");
    }

    #[test]
    fn an_arrowhead_has_four_wings_back_from_the_tip() {
        let got = heads(&[piece([0.0, 0.0, 8.0], [100.0, 0.0, 8.0])], [1, 2, 3], 4.0);
        assert_eq!(got.len(), 4);
        for b in &got {
            assert_eq!(b.to.map(f32::round), [100.0, 0.0, 24.0], "at the tip, in the tube's middle");
            let back = 100.0 - f64::from(b.from[0]);
            assert!((back - HEAD * HEAD_ANGLE.to_radians().cos()).abs() < 0.01, "{b:?}");
        }
    }
}
