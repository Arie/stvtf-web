//! A room's strokes in TF2: laid on the map's floors (see `maps::drape`) and shown by its relay
//! as beams: the pen's once, for as long as pages show it; the marker's kept by the relay (which
//! shows them again before they fade), handed it whole whenever the marker changes. A beam cannot
//! be taken back, so a cleared marker goes within the relay's refresh.

use super::Room;
use super::drawing::Draw;
use crate::lock;
use crate::maps::drape::{self, Drape, Ground, Piece};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use stvtf::relay::Beam;

/// The pages' inks for those who picked none (`app/room/telestrator.js`).
const INKS: [[u8; 3]; 6] =
    [[0xff, 0xd2, 0x3f], [0x4f, 0xd1, 0xc5], [0xf4, 0x72, 0xb6], [0xa3, 0xe6, 0x35], [0xfb, 0x92, 0x3c], [0xc0, 0x84, 0xfc]];
/// How long the pen's lines show: as long as pages hold and fade a stroke.
const PEN_LIFE: f32 = 4.0;
/// How wide the lines are, in units.
const WIDTH: f32 = 5.0;
/// Each line is a fence: the line on the floor, another this much above it, and posts between them
/// at every corner and at least this often.
const FENCE: f64 = 40.0;
const POST_EVERY: f64 = 64.0;
/// How fast the lines' texture runs toward the stroke's end, showing which way it goes.
const SCROLL: u8 = 20;
/// The arrowhead at a stroke's end: its wings' length and angle off the line.
const HEAD: f64 = 40.0;
const HEAD_ANGLE: f64 = 30.0;
/// A pen stroke with no new points for this long is done: it gets its arrowhead.
const PEN_DONE: Duration = Duration::from_millis(400);
/// The most strokes followed at once: more, and the oldest are forgotten.
const MAX_DRAPES: usize = 64;

/// A pen stroke being laid on the floors: by whom, which, in what ink, where its lines end now,
/// when it last grew, and whether it has its arrowhead.
pub(super) struct PenStroke {
    key: (u32, u32),
    drape: Drape,
    ink: [u8; 3],
    ends: Vec<Piece>,
    grew: Instant,
    headed: bool,
}

impl Room {
    /// Shows a piece of a pen stroke by person `by` to the TF2 clients in its relay.
    pub(super) fn draw_in_relay(&self, d: &Draw, by: u32) {
        let Some(ground) = self.ground_for_relay() else { return };
        let ink = self.ink_now(by);
        let pieces = {
            let mut drapes = lock(&self.drapes);
            if !drapes.iter().any(|s| s.key == (by, d.stroke)) {
                if drapes.len() >= MAX_DRAPES {
                    drapes.remove(0);
                }
                let fresh =
                    PenStroke { key: (by, d.stroke), drape: Drape::new(), ink, ends: Vec::new(), grew: Instant::now(), headed: false };
                drapes.push(fresh);
            }
            let Some(s) = drapes.iter_mut().find(|s| s.key == (by, d.stroke)) else { return };
            let pieces = s.drape.push(&ground, &d.points);
            if let Some(&[x, y, _]) = d.points.last() {
                s.ends = ends(&pieces, [x, y]);
            }
            s.grew = Instant::now();
            s.headed = false;
            pieces
        };
        let beams = beams(&pieces, ink, PEN_LIFE);
        if !beams.is_empty()
            && let Some(r) = &*lock(&self.relay)
        {
            r.draw(&beams);
        }
    }

    /// Hands the relay the marker as it is now, each stroke in the ink it was drawn in, once
    /// TF2 clients are in to see it; until then it waits.
    pub(super) fn keep_marker(&self) {
        self.marker_dirty.store(true, Ordering::Relaxed);
        let Some(ground) = self.ground_for_relay() else { return };
        let strokes: Vec<(u32, Option<String>, Vec<[f64; 3]>)> =
            lock(&self.marker).iter().map(|k| (k.by, k.ink.clone(), k.points.clone())).collect();
        let mut all = Vec::new();
        for (by, ink, points) in strokes {
            let ink = ink.as_deref().and_then(rgb).unwrap_or_else(|| self.ink_now(by));
            let pieces = Drape::new().push(&ground, &points);
            all.extend(beams(&pieces, ink, 0.0));
            if let Some(&[x, y, _]) = points.last() {
                all.extend(heads(&ends(&pieces, [x, y]), ink, 0.0));
            }
        }
        if let Some(r) = &*lock(&self.relay) {
            r.keep(all);
            self.marker_dirty.store(false, Ordering::Relaxed);
        }
    }

    /// Hands the relay the marker if it has changed since it last could.
    pub(super) fn keep_marker_if_changed(&self) {
        if self.marker_dirty.load(Ordering::Relaxed) {
            self.keep_marker();
        }
    }

    /// Gives each pen stroke that stopped growing its arrowhead, for as long as the stroke shows.
    pub(super) fn head_pens(&self) {
        let mut heads_due = Vec::new();
        for s in lock(&self.drapes).iter_mut().filter(|s| !s.headed && s.grew.elapsed() >= PEN_DONE) {
            s.headed = true;
            let life = PEN_LIFE - s.grew.elapsed().as_secs_f32();
            if life > 0.0 {
                heads_due.extend(heads(&s.ends, s.ink, life));
            }
        }
        if !heads_due.is_empty()
            && let Some(r) = &*lock(&self.relay)
        {
            r.draw(&heads_due);
        }
    }

    /// The strokes followed so far are done with: the map was wiped.
    pub(super) fn forget_drapes(&self) {
        lock(&self.drapes).clear();
    }

    /// The map's floors, when TF2 clients are in the relay to show strokes to.
    fn ground_for_relay(&self) -> Option<Arc<Ground>> {
        if self.relay_clients() == 0 {
            return None;
        }
        let maps = self.offer.get()?.maps.as_ref()?;
        let map = lock(&self.hub).map.clone()?;
        drape::ground(maps, &map)
    }

    /// Person `by`'s ink now: theirs, or the pages' for their number.
    pub(super) fn ink_now(&self, by: u32) -> [u8; 3] {
        lock(&self.people).ink_of(by).and_then(rgb).unwrap_or(INKS[(by.max(1) as usize - 1) % INKS.len()])
    }
}

/// A beam of `ink` from `from` to `to`, shown for `life` seconds, its texture running at `speed`.
fn beam(from: [f64; 3], to: [f64; 3], ink: [u8; 3], life: f32, speed: u8) -> Beam {
    Beam { from: from.map(|v| v as f32), to: to.map(|v| v as f32), rgb: ink, life, width: WIDTH, speed }
}

/// The pieces of `pieces` ending where the stroke does, at (x, y): one per floor it ends on.
fn ends(pieces: &[Piece], [x, y]: [f64; 2]) -> Vec<Piece> {
    pieces.iter().filter(|p| (p.to[0] - x).hypot(p.to[1] - y) < 1.0).copied().collect()
}

/// An arrowhead on the end of each of `ends`, as a fence: wings on the floor and above, and posts
/// at their tips.
fn heads(ends: &[Piece], ink: [u8; 3], life: f32) -> Vec<Beam> {
    let mut out = Vec::new();
    for p in ends {
        let (dx, dy) = (p.to[0] - p.from[0], p.to[1] - p.from[1]);
        let len = dx.hypot(dy);
        if len < 1.0 {
            continue;
        }
        let (back_x, back_y) = (-dx / len, -dy / len);
        for side in [-1.0, 1.0] {
            let (s, c) = (side * HEAD_ANGLE).to_radians().sin_cos();
            let tip = [p.to[0] + (back_x * c - back_y * s) * HEAD, p.to[1] + (back_x * s + back_y * c) * HEAD, p.to[2]];
            let up = |q: [f64; 3]| [q[0], q[1], q[2] + FENCE];
            out.push(beam(tip, p.to, ink, life, 0));
            out.push(beam(up(tip), up(p.to), ink, life, 0));
            out.push(beam(tip, up(tip), ink, life, 0));
        }
    }
    out
}

/// `pieces` as a fence of beams of `ink` (see [`FENCE`]), shown for `life` seconds: its lines'
/// texture running toward the stroke's end, its posts still.
fn beams(pieces: &[Piece], ink: [u8; 3], life: f32) -> Vec<Beam> {
    let beam = |from: [f64; 3], to: [f64; 3], speed: u8| beam(from, to, ink, life, speed);
    let up = |p: [f64; 3]| [p[0], p[1], p[2] + FENCE];
    let mut out = Vec::new();
    let mut posts: Vec<[f64; 3]> = Vec::new();
    for p in pieces {
        out.push(beam(p.from, p.to, SCROLL));
        out.push(beam(up(p.from), up(p.to), SCROLL));
        let length = (0..3).map(|k| (p.to[k] - p.from[k]).powi(2)).sum::<f64>().sqrt();
        let n = (length / POST_EVERY).ceil().max(1.0) as usize;
        for i in 0..=n {
            let t = i as f64 / n as f64;
            let at = [0, 1, 2].map(|k| p.from[k] + (p.to[k] - p.from[k]) * t);
            if !posts.contains(&at) {
                posts.push(at);
            }
        }
    }
    out.extend(posts.into_iter().map(|p| beam(p, up(p), 0)));
    out
}

/// `#rrggbb` as its bytes.
fn rgb(ink: &str) -> Option<[u8; 3]> {
    let hex = ink.strip_prefix('#').filter(|h| h.len() == 6)?;
    let at = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    Some([at(0)?, at(2)?, at(4)?])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::float_cmp)] // whole numbers, exact
    fn a_line_is_a_fence_with_posts_at_corners_and_along() {
        let piece = |from, to| Piece { from, to, ms: 0.0 };
        let got = beams(&[piece([0.0, 0.0, 8.0], [100.0, 0.0, 8.0]), piece([100.0, 0.0, 8.0], [100.0, 50.0, 8.0])], [1, 2, 3], 4.0);
        let (rails, posts): (Vec<&Beam>, Vec<&Beam>) = got.iter().partition(|b| b.from[2] == b.to[2]);
        assert_eq!(rails.len(), 4, "the line and the one above it, per piece");
        assert!(rails.iter().any(|b| b.from == [0.0, 0.0, 48.0] && b.to == [100.0, 0.0, 48.0]));
        let feet: Vec<[f32; 3]> = posts.iter().map(|b| b.from).collect();
        assert_eq!(feet, [[0.0, 0.0, 8.0], [50.0, 0.0, 8.0], [100.0, 0.0, 8.0], [100.0, 50.0, 8.0]], "ends shared once, one between");
        assert!(posts.iter().all(|b| b.to[2] - b.from[2] == 40.0));
    }

    #[test]
    #[allow(clippy::float_cmp)] // whole numbers and tenths rounded, exact
    fn a_stroke_ends_in_an_arrowhead_pointing_its_way() {
        let piece = |from, to| Piece { from, to, ms: 0.0 };
        let pieces = [piece([0.0, 0.0, 8.0], [100.0, 0.0, 8.0]), piece([0.0, 0.0, 200.0], [100.0, 0.0, 200.0])];
        let ends = ends(&pieces, [100.0, 0.0]);
        assert_eq!(ends.len(), 2, "on both floors it ends on");
        let head = heads(&ends[..1], [1, 2, 3], 4.0);
        assert_eq!(head.len(), 6, "two wings, each on the floor, above and a post");
        let tips: Vec<[f32; 3]> =
            head.iter().filter(|b| b.to == [100.0, 0.0, 8.0]).map(|b| b.from.map(|v| (v * 10.0).round() / 10.0)).collect();
        assert_eq!(tips, [[65.4, 20.0, 8.0], [65.4, -20.0, 8.0]], "back along the line, 30° off it");
        assert!(beams(&pieces[..1], [1, 2, 3], 4.0).iter().filter(|b| b.speed > 0).all(|b| b.to[0] > b.from[0]), "running its way");
    }

    #[test]
    fn inks_are_their_bytes() {
        assert_eq!(rgb("#ff8000"), Some([255, 128, 0]));
        assert_eq!(rgb("#ff80"), None);
        assert_eq!(rgb("ff8000"), None);
    }
}
