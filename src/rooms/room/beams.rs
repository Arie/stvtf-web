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
/// The most strokes followed at once: more, and the oldest are forgotten.
const MAX_DRAPES: usize = 64;

impl Room {
    /// Shows a piece of a pen stroke by person `by` to the TF2 clients in its relay.
    pub(super) fn draw_in_relay(&self, d: &Draw, by: u32) {
        let Some(ground) = self.ground_for_relay() else { return };
        let pieces = {
            let mut drapes = lock(&self.drapes);
            if !drapes.iter().any(|(k, _)| *k == (by, d.stroke)) {
                if drapes.len() >= MAX_DRAPES {
                    drapes.remove(0);
                }
                drapes.push(((by, d.stroke), Drape::new()));
            }
            let Some((_, drape)) = drapes.iter_mut().find(|(k, _)| *k == (by, d.stroke)) else { return };
            drape.push(&ground, &d.points)
        };
        let ink = self.ink_now(by);
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
            all.extend(beams(&Drape::new().push(&ground, &points), ink, 0.0));
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

/// `pieces` as a fence of beams of `ink` (see [`FENCE`]), shown for `life` seconds.
fn beams(pieces: &[Piece], ink: [u8; 3], life: f32) -> Vec<Beam> {
    let beam = |from: [f64; 3], to: [f64; 3]| Beam { from: from.map(|v| v as f32), to: to.map(|v| v as f32), rgb: ink, life, width: WIDTH };
    let up = |p: [f64; 3]| [p[0], p[1], p[2] + FENCE];
    let mut out = Vec::new();
    let mut posts: Vec<[f64; 3]> = Vec::new();
    for p in pieces {
        out.push(beam(p.from, p.to));
        out.push(beam(up(p.from), up(p.to)));
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
    out.extend(posts.into_iter().map(|p| beam(p, up(p))));
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
    fn inks_are_their_bytes() {
        assert_eq!(rgb("#ff8000"), Some([255, 128, 0]));
        assert_eq!(rgb("#ff80"), None);
        assert_eq!(rgb("ff8000"), None);
    }
}
