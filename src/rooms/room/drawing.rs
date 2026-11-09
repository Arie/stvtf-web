//! Drawing on a room's map: the pen's strokes, and the permanent marker kept until cleared.

use super::Room;
use crate::lock;
use crate::matches::hub::{send, send_keeping};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Most points in one drawing request.
pub(super) const MAX_POINTS: usize = 256;

/// A piece of a telestrator stroke: points in map coordinates, each with when it was drawn, in
/// milliseconds into the stroke, so pages draw it as it was drawn.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Draw {
    pub(crate) stroke: u32,
    pub(crate) points: Vec<[f64; 3]>,
    /// Permanent marker: stays until cleared.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) keep: bool,
    /// The stroke's last piece: the pen was lifted (its points may be none).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) end: bool,
}

/// A permanent marker stroke, as drawn so far.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Kept {
    pub(super) stroke: u32,
    pub(super) by: u32,
    pub(super) points: Vec<[f64; 3]>,
    /// The ink it was drawn in (`#rrggbb`); none in rooms saved before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) ink: Option<String>,
    /// Still being drawn: its end not come yet.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) open: bool,
    /// A jump arc's points, launch to landing (its `points` are then the two clicks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) arc: Option<Vec<[f64; 3]>>,
}

/// Most permanent marker points on a room's map.
pub(crate) const MAX_KEPT_POINTS: usize = 8000;

/// The longest a stroke's points can say it took, in milliseconds.
pub(super) const MAX_STROKE_MS: f64 = 600_000.0;

/// The marker for joiners, empty too: a page back after a clear it missed drops its own.
pub(super) fn kept_line(marker: &[Kept]) -> Arc<str> {
    serde_json::json!({ "type": "marker", "strokes": marker, "max": MAX_KEPT_POINTS }).to_string().into()
}

impl Room {
    /// Passes a piece of a stroke by `guest` on to every page.
    pub(crate) fn draw(&self, d: &Draw, guest: &str) -> Result<()> {
        if (d.points.is_empty() && !d.end) || d.points.len() > MAX_POINTS {
            bail!("1 to {MAX_POINTS} points, or none at a stroke's end");
        }
        if !d.points.iter().all(|&[x, y, t]| [x, y].iter().all(|v| v.is_finite() && v.abs() < 1e6) && (0.0..=MAX_STROKE_MS).contains(&t)) {
            bail!("points must be on a map, in a stroke's time");
        }
        let by = lock(&self.people).id(guest);
        let mut line = serde_json::to_value(d)?;
        line["type"] = "draw".into();
        line["by"] = by.into();
        let line: Arc<str> = line.to_string().into();
        if !d.keep {
            self.draw_in_relay(d, by)?;
            send(&self.hub, &line);
            return Ok(());
        }
        let mut marker = lock(&self.marker);
        if marker.iter().map(|k| k.points.len()).sum::<usize>() + d.points.len() > MAX_KEPT_POINTS {
            bail!("The map is full of marker ({MAX_KEPT_POINTS} points): clear it first.");
        }
        let before = marker.clone();
        if let Some(k) = marker.iter_mut().find(|k| k.stroke == d.stroke && k.by == by) {
            k.points.extend(&d.points);
            k.open = !d.end;
        } else if !d.points.is_empty() {
            let ink = Some(super::beams::hex(self.ink_now(by)));
            marker.push(Kept { stroke: d.stroke, by, points: d.points.clone(), ink, open: !d.end, arc: None });
        }
        let beams = self.marker_in_tf2(&marker).inspect_err(|_| *marker = before)?;
        send_keeping(&self.hub, &line, "marker", kept_line(&marker));
        drop(marker);
        self.hand_marker(beams);
        Ok(())
    }

    /// Admin `guest` wipes every stroke, marker too.
    pub(crate) fn clear(&self, guest: &str) {
        let by = lock(&self.people).id(guest);
        lock(&self.marker).clear();
        self.forget_drapes();
        self.keep_marker();
        let line = serde_json::json!({ "type": "clear", "by": by }).to_string().into();
        send_keeping(&self.hub, &line, "marker", kept_line(&lock(&self.marker)));
    }
}
