//! The spotlight: one click lights a spot on the floor in the drawer's ink ("look here"), kept
//! with the marker; in TF2 the game's dynamic light (see `stvtf::relay::Light`). There is one: the
//! next click moves it, as a TF2 client shows one such light at a time.

use super::Room;
use super::beams::{hex, rgb};
use super::drawing::Kept;
use crate::lock;
use anyhow::{Result, bail};
use serde::Deserialize;
use stvtf::relay::Light;

/// How far the light reaches in TF2 and how fast it falls off (as the user picked there), and how
/// far above the floor it is.
const RADIUS: f32 = 500.0;
const EXPONENT: u8 = 5;
const LIFT: f64 = 8.0;
/// The light's id, apart from the pages' strokes', arcs', circles' and notes'.
const LIGHT_ID: u32 = 0x1000_0000;

/// The spotlight as a page asks for it: where it shines.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LightDraw {
    pub(crate) at: [f64; 2],
}

impl Room {
    /// Admin `guest` puts the spotlight on the floor under a spot, in place of where it was: kept
    /// with the marker, shown to every page and in TF2.
    pub(crate) fn light(&self, l: &LightDraw, guest: &str) -> Result<()> {
        let Some(ground) = self.ground() else { bail!("The map is not ready yet.") };
        let [x, y] = l.at;
        let Some(z) = ground.top(x, y) else { bail!("Pick a spot on a floor.") };
        let by = lock(&self.people).id(guest);
        let ink = hex(self.ink_now(by));
        self.change_marker(|marker| {
            marker.retain(|k| k.light.is_none());
            let line = serde_json::json!({ "type": "light", "by": by, "stroke": LIGHT_ID, "ink": ink, "at": [x, y, z] });
            let points = vec![[x, y, 0.0]];
            let light = Some([x, y, z]);
            marker.push(Kept { stroke: LIGHT_ID, by, points, ink: Some(ink), open: false, arc: None, circle: None, note: None, light });
            line.to_string().into()
        })
    }
}

/// The marker's spotlight, as TF2 shows it.
pub(super) fn tf2_light(marker: &[Kept]) -> Option<Light> {
    let k = marker.iter().find(|k| k.light.is_some())?;
    let [x, y, z] = k.light?;
    let at = [x as f32, y as f32, (z + LIFT) as f32];
    Some(Light { at, rgb: k.ink.as_deref().and_then(rgb)?, radius: RADIUS, exponent: EXPONENT })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rooms::room::tests::arc_room;

    #[test]
    fn the_spotlight_is_kept_on_the_floor_under_the_click_moves_with_the_next_and_is_cleared_with_the_marker() {
        let (r, _dir) = arc_room();
        r.set_ink("me", "#ff8000").unwrap();
        r.light(&LightDraw { at: [1256.0, 256.0] }, "me").unwrap();
        let marker = r.current_marker();
        assert_eq!(marker[0].light, Some([1256.0, 256.0, 200.0]), "on the high floor");
        let light = tf2_light(&marker).unwrap();
        assert_eq!((light.at, light.rgb), ([1256.0, 256.0, 200.0 + LIFT as f32], [255, 128, 0]));
        r.light(&LightDraw { at: [256.0, 256.0] }, "me").unwrap();
        let marker = r.current_marker();
        assert_eq!(marker.len(), 1, "one light");
        assert_ne!(marker[0].light, Some([1256.0, 256.0, 200.0]), "where the next click was");
        r.clear("me");
        assert!(tf2_light(&r.current_marker()).is_none());
    }

    #[test]
    fn a_spotlight_off_the_floors_is_refused_and_the_one_there_stays() {
        let (r, _dir) = arc_room();
        r.light(&LightDraw { at: [1256.0, 256.0] }, "me").unwrap();
        let e = r.light(&LightDraw { at: [700.0, 256.0] }, "me").unwrap_err();
        assert_eq!(e.to_string(), "Pick a spot on a floor.");
        assert_eq!(r.current_marker()[0].light, Some([1256.0, 256.0, 200.0]));
    }
}
