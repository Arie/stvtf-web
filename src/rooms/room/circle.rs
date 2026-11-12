//! A circle: one click puts a small ring on the floor there ("stand here"), kept with the marker.

use super::Room;
use super::beams::{beam, hex};
use super::drawing::Kept;
use crate::lock;
use anyhow::{Result, bail};
use serde::Deserialize;
use std::f64::consts::TAU;
use stvtf::relay::Beam;

/// The ring's radius, and its fence's height above the floor ring (as the user picked in TF2).
const RADIUS: f64 = 38.0;
const FENCE: f64 = 24.0;
/// Beams around; a post on every other one.
const PIECES: usize = 24;
/// The floor ring is this far above the floor.
const LIFT: f64 = 4.0;
/// Circles' ids, apart from the pages' strokes' and arcs'.
const CIRCLE_IDS: u32 = 0x4000_0000;

/// A circle as a page asks for it: where its centre is.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CircleDraw {
    pub(crate) at: [f64; 2],
}

impl Room {
    /// Admin `guest` puts a circle on the floor under a spot: kept with the marker, shown to every
    /// page and in TF2.
    pub(crate) fn circle(&self, c: &CircleDraw, guest: &str) -> Result<()> {
        let Some(ground) = self.ground() else { bail!("The map is not ready yet.") };
        let [x, y] = c.at;
        let Some(z) = ground.top(x, y) else { bail!("Pick a spot on a floor.") };
        let by = lock(&self.people).id(guest);
        let ink = hex(self.ink_now(by));
        self.change_marker(|marker| {
            let stroke = CIRCLE_IDS | marker.iter().filter(|k| k.circle.is_some()).count() as u32;
            let line = serde_json::json!({ "type": "circle", "by": by, "stroke": stroke, "ink": ink, "at": [x, y, z] });
            let points = vec![[x, y, 0.0]];
            marker.push(Kept {
                stroke,
                by,
                points,
                ink: Some(ink),
                open: false,
                arc: None,
                circle: Some([x, y, z]),
                note: None,
                light: None,
            });
            line.to_string().into()
        })
    }
}

/// A circle's beams round `center` (on the floor), in `ink`: the floor ring, a ring above it, and
/// posts between them.
pub(super) fn circle_beams(center: [f64; 3], ink: [u8; 3], life: f32) -> Vec<Beam> {
    let at = |i: usize, up: f64| {
        let a = i as f64 / PIECES as f64 * TAU;
        [center[0] + RADIUS * a.cos(), center[1] + RADIUS * a.sin(), center[2] + LIFT + up]
    };
    let ring = |up: f64| (0..PIECES).map(move |i| beam(at(i, up), at(i + 1, up), ink, life, 0));
    let posts = (0..PIECES).step_by(2).map(|i| beam(at(i, 0.0), at(i, FENCE), ink, life, 0));
    ring(0.0).chain(ring(FENCE)).chain(posts).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rooms::room::tests::arc_room;

    #[test]
    fn a_circle_is_kept_on_the_floor_under_the_click_and_cleared_with_the_marker() {
        let (r, _dir) = arc_room();
        r.circle(&CircleDraw { at: [1256.0, 256.0] }, "me").unwrap();
        let marker = r.current_marker();
        assert_eq!(marker[0].circle, Some([1256.0, 256.0, 200.0]), "on the high floor");
        r.clear("me");
        assert!(r.current_marker().is_empty());
    }

    #[test]
    fn a_circle_off_the_floors_is_refused() {
        let (r, _dir) = arc_room();
        let e = r.circle(&CircleDraw { at: [700.0, 256.0] }, "me").unwrap_err();
        assert_eq!(e.to_string(), "Pick a spot on a floor.");
        assert!(r.current_marker().is_empty());
    }

    #[test]
    fn a_circle_is_a_fence_ring_round_its_centre() {
        let beams = circle_beams([100.0, 50.0, 10.0], [255, 0, 0], 2.0);
        assert_eq!(beams.len(), 24 + 24 + 12, "a floor ring, a ring above and posts");
        for b in &beams {
            for p in [b.from, b.to] {
                let off = f64::from(p[0] - 100.0).hypot(f64::from(p[1] - 50.0));
                assert!((off - RADIUS).abs() < 0.01, "{p:?} off the ring");
                assert!(p[2] >= 10.0 && p[2] <= 10.0 + LIFT as f32 + FENCE as f32 + 0.01, "{p:?}");
            }
        }
        assert!(beams.iter().all(|b| b.rgb == [255, 0, 0]));
    }
}
