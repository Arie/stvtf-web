//! A room's strokes in TF2: laid on the map's floors (see `maps::drape`) and shown by its relay
//! as beams, the pen's for as long as pages show it, the marker's sent again before it fades.

use super::Room;
use super::drawing::Draw;
use crate::lock;
use crate::maps::drape::{self, Drape, Ground, Piece};
use std::sync::Arc;
use std::time::{Duration, Instant};
use stvtf::relay::Beam;

/// The pages' inks for those who picked none (`app/room/telestrator.js`).
const INKS: [[u8; 3]; 6] =
    [[0xff, 0xd2, 0x3f], [0x4f, 0xd1, 0xc5], [0xf4, 0x72, 0xb6], [0xa3, 0xe6, 0x35], [0xfb, 0x92, 0x3c], [0xc0, 0x84, 0xfc]];
/// How long the pen's lines show: as long as pages hold and fade a stroke.
const PEN_LIFE: f32 = 4.0;
/// How long the marker's show, and how often they are sent again: before they fade.
const MARKER_LIFE: f32 = 25.0;
const MARKER_EVERY: Duration = Duration::from_secs(20);
/// How wide the lines are, in units.
const WIDTH: f32 = 3.0;
/// The most strokes followed at once: more, and the oldest are forgotten.
const MAX_DRAPES: usize = 64;

impl Room {
    /// Shows a piece of a stroke by person `by` to the TF2 clients in its relay.
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
        self.send_beams(&pieces, by, if d.keep { MARKER_LIFE } else { PEN_LIFE });
    }

    /// Sends the marker again before its lines fade, while TF2 clients are in.
    pub(super) fn redraw_marker(&self) {
        if lock(&self.marker_drawn).is_some_and(|at| at.elapsed() < MARKER_EVERY) {
            return;
        }
        let Some(ground) = self.ground_for_relay() else { return };
        let strokes: Vec<(u32, Vec<[f64; 3]>)> = lock(&self.marker).iter().map(|k| (k.by, k.points.clone())).collect();
        *lock(&self.marker_drawn) = Some(Instant::now());
        for (by, points) in strokes {
            self.send_beams(&Drape::new().push(&ground, &points), by, MARKER_LIFE);
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

    fn send_beams(&self, pieces: &[Piece], by: u32, life: f32) {
        if pieces.is_empty() {
            return;
        }
        let rgb = lock(&self.people).ink_of(by).and_then(rgb).unwrap_or(INKS[(by.max(1) as usize - 1) % INKS.len()]);
        let f = |p: [f64; 3]| p.map(|v| v as f32);
        let beams: Vec<Beam> = pieces.iter().map(|p| Beam { from: f(p.from), to: f(p.to), rgb, life, width: WIDTH }).collect();
        if let Some(r) = &*lock(&self.relay) {
            r.draw(&beams);
        }
    }
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
    fn inks_are_their_bytes() {
        assert_eq!(rgb("#ff8000"), Some([255, 128, 0]));
        assert_eq!(rgb("#ff80"), None);
        assert_eq!(rgb("ff8000"), None);
    }
}
