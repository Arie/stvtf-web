//! A room's strokes in TF2: laid on the map's floors (see `maps::drape`) and shown by its relay
//! as beams: the pen's once, for as long as pages show it; the marker's kept by the relay (which
//! shows them again before they fade), handed it whole whenever the marker changes. A beam cannot
//! be taken back, so a cleared marker goes within the relay's refresh.

use super::Room;
use super::drawing::Draw;
use super::tube::{heads, tube};
use crate::lock;
use crate::maps::drape::{self, Drape, Ground, Piece};
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// The pages' inks for those who picked none (`app/room/telestrator.js`).
const INKS: [[u8; 3]; 6] =
    [[0xff, 0xd2, 0x3f], [0x4f, 0xd1, 0xc5], [0xf4, 0x72, 0xb6], [0xa3, 0xe6, 0x35], [0xfb, 0x92, 0x3c], [0xc0, 0x84, 0xfc]];
/// How long the pen's lines show: as long as pages hold and fade a stroke.
const PEN_LIFE: f32 = 4.0;
/// The most strokes followed at once: more, and the oldest are forgotten.
const MAX_DRAPES: usize = 64;

/// A pen stroke being laid on the floors: by whom, which, and where its lines end now.
pub(super) struct PenStroke {
    key: (u32, u32),
    drape: Drape,
    ends: Vec<Piece>,
}

impl Room {
    /// Shows a piece of a pen stroke by person `by` to the TF2 clients in its relay.
    pub(super) fn draw_in_relay(&self, d: &Draw, by: u32) {
        let Some(ground) = self.ground_for_relay() else { return };
        let ink = self.ink_now(by);
        let beams = {
            let mut drapes = lock(&self.drapes);
            if !drapes.iter().any(|s| s.key == (by, d.stroke)) {
                if drapes.len() >= MAX_DRAPES {
                    drapes.remove(0);
                }
                drapes.push(PenStroke { key: (by, d.stroke), drape: Drape::new(), ends: Vec::new() });
            }
            let Some(s) = drapes.iter_mut().find(|s| s.key == (by, d.stroke)) else { return };
            let pieces = s.drape.push(&ground, &d.points);
            if let Some(&[x, y, _]) = d.points.last() {
                s.ends = ends(&pieces, [x, y]);
            }
            let mut beams = tube(&pieces, ink, PEN_LIFE);
            if d.end {
                beams.extend(heads(&s.ends, ink, PEN_LIFE));
            }
            beams
        };
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
        let strokes = lock(&self.marker).clone();
        let mut all = Vec::new();
        for k in strokes {
            let (by, ink, points, open) = (k.by, k.ink, k.points, k.open);
            let ink = ink.as_deref().and_then(rgb).unwrap_or_else(|| self.ink_now(by));
            let pieces = Drape::new().push(&ground, &points);
            all.extend(tube(&pieces, ink, 0.0));
            if let Some(&[x, y, _]) = points.last().filter(|_| !open) {
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

/// The pieces of `pieces` ending where the stroke does, at (x, y): one per floor it ends on.
fn ends(pieces: &[Piece], [x, y]: [f64; 2]) -> Vec<Piece> {
    pieces.iter().filter(|p| (p.to[0] - x).hypot(p.to[1] - y) < 1.0).copied().collect()
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
    fn a_stroke_ends_on_each_floor_it_ends_on() {
        let piece = |from, to| Piece { from, to, ms: 0.0 };
        let pieces = [piece([0.0, 0.0, 8.0], [100.0, 0.0, 8.0]), piece([0.0, 0.0, 200.0], [100.0, 0.0, 200.0])];
        assert_eq!(ends(&pieces, [100.0, 0.0]).len(), 2);
        assert_eq!(ends(&pieces, [50.0, 0.0]), []);
    }

    #[test]
    fn inks_are_their_bytes() {
        assert_eq!(rgb("#ff8000"), Some([255, 128, 0]));
        assert_eq!(rgb("#ff80"), None);
        assert_eq!(rgb("ff8000"), None);
    }
}
