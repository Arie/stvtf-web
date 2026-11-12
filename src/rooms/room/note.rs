//! Notes: a short text at a spot on the map or on a player, kept with the marker; in TF2 the
//! game's text bubble (see `stvtf::relay::Note`).

use super::Room;
use super::beams::hex;
use super::drawing::Kept;
use crate::lock;
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use stvtf::relay::Note;

/// The most characters a note says.
const MAX_TEXT: usize = 40;
/// The most notes kept: each is sent to every TF2 client again and again.
pub(super) const MAX_NOTES: usize = 10;
/// Notes' ids, apart from the pages' strokes', arcs' and circles'.
const NOTE_IDS: u32 = 0x2000_0000;
/// A bubble at a spot is this high above its floor.
const ABOVE: f64 = 64.0;

/// A note as a page asks for it: its text, at a spot or on a player (by user id).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NoteDraw {
    pub(crate) text: String,
    #[serde(default)]
    pub(crate) at: Option<[f64; 2]>,
    #[serde(default)]
    pub(crate) player: Option<u16>,
}

/// What a kept note says, and whom it follows (its `points` are its spot otherwise).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Label {
    pub(crate) text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) player: Option<u16>,
}

impl Room {
    /// Admin `guest` puts a note at a spot, on the floor under it, or on a player: kept with the
    /// marker, shown to every page and in TF2.
    pub(crate) fn note(&self, n: &NoteDraw, guest: &str) -> Result<()> {
        let text = n.text.trim();
        if text.is_empty() {
            bail!("Write something first.");
        }
        if text.chars().count() > MAX_TEXT {
            bail!("At most {MAX_TEXT} characters.");
        }
        let spot = match (n.at, n.player) {
            (Some([x, y]), None) => {
                let Some(ground) = self.ground() else { bail!("The map is not ready yet.") };
                let Some(z) = ground.top(x, y) else { bail!("Pick a spot on a floor.") };
                Some([x, y, z])
            }
            (None, Some(_)) => None,
            _ => bail!("A spot or a player, one of them."),
        };
        let by = lock(&self.people).id(guest);
        let ink = hex(self.ink_now(by));
        let label = Label { text: text.to_string(), player: n.player };
        self.change_marker(|marker| {
            let stroke = NOTE_IDS | marker.iter().filter(|k| k.note.is_some()).count() as u32;
            let line = serde_json::json!({
                "type": "note", "by": by, "stroke": stroke, "ink": ink, "text": label.text, "at": spot, "player": label.player,
            });
            let points = vec![spot.unwrap_or_default()];
            marker.push(Kept { stroke, by, points, ink: Some(ink), open: false, arc: None, circle: None, note: Some(label), light: None });
            line.to_string().into()
        })
    }
}

/// The marker's notes, as TF2 shows them.
pub(super) fn tf2_notes(marker: &[Kept]) -> Vec<Note> {
    marker
        .iter()
        .filter_map(|k| {
            let label = k.note.as_ref()?;
            let [x, y, z] = k.points.first().copied().unwrap_or_default();
            let at = [x as f32, y as f32, (z + ABOVE) as f32];
            Some(Note { id: k.stroke, text: label.text.clone(), at, player: label.player })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rooms::room::tests::arc_room;

    fn spot(text: &str, at: [f64; 2]) -> NoteDraw {
        NoteDraw { text: text.into(), at: Some(at), player: None }
    }

    #[test]
    fn a_note_at_a_spot_is_kept_on_the_floor_there_and_cleared_with_the_marker() {
        let (r, _dir) = arc_room();
        r.note(&spot("  high ground ", [1256.0, 256.0]), "me").unwrap();
        let marker = r.current_marker();
        assert_eq!(marker[0].note, Some(Label { text: "high ground".into(), player: None }));
        assert_eq!(marker[0].points, [[1256.0, 256.0, 200.0]]);
        let notes = tf2_notes(&marker);
        assert_eq!(notes[0].at, [1256.0, 256.0, 200.0 + ABOVE as f32]);
        assert_eq!((notes[0].text.as_str(), notes[0].player), ("high ground", None));
        r.clear("me");
        assert!(r.current_marker().is_empty());
    }

    #[test]
    fn a_note_on_a_player_follows_them() {
        let (r, _dir) = arc_room();
        r.note(&NoteDraw { text: "watch".into(), at: None, player: Some(12) }, "me").unwrap();
        let notes = tf2_notes(&r.current_marker());
        assert_eq!(notes[0].player, Some(12));
        assert_ne!(notes[0].id, 0);
    }

    #[test]
    fn a_note_past_the_most_kept_is_refused_until_they_are_cleared() {
        let (r, _dir) = arc_room();
        let put = || r.note(&NoteDraw { text: "x".into(), at: None, player: Some(1) }, "me");
        for _ in 0..MAX_NOTES {
            put().unwrap();
        }
        assert_eq!(put().unwrap_err().to_string(), "At most 10 notes: clear them first.");
        assert_eq!(r.current_marker().len(), MAX_NOTES);
        r.clear("me");
        put().unwrap();
    }

    #[test]
    fn a_note_needs_some_text_not_too_much_and_a_floor_or_a_player() {
        let (r, _dir) = arc_room();
        let refused = |n: NoteDraw| r.note(&n, "me").unwrap_err().to_string();
        assert_eq!(refused(spot(" ", [256.0, 256.0])), "Write something first.");
        assert_eq!(refused(spot(&"x".repeat(41), [256.0, 256.0])), "At most 40 characters.");
        assert_eq!(refused(spot("off", [700.0, 256.0])), "Pick a spot on a floor.");
        assert_eq!(refused(NoteDraw { text: "x".into(), at: None, player: None }), "A spot or a player, one of them.");
        assert!(r.current_marker().is_empty());
    }
}
