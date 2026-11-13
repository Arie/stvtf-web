//! A room's marker is kept per map: what was drawn on a map shows on it, and again when the room
//! comes back to it.

use super::Room;
use super::drawing::{Kept, kept_line};
use crate::lock;
use crate::matches::hub::announce;
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;

/// The marker's strokes, by the number of the map (part) they were drawn on.
pub(crate) type Markers = BTreeMap<usize, Vec<Kept>>;

/// A marker file's `bytes`: a map of parts' strokes, or (saved before parts) a plain list, the
/// `latest` part's. One unreadable is none.
pub(crate) fn read_marker(bytes: &[u8], latest: usize) -> Markers {
    if bytes.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'[') {
        let list: Vec<Kept> = serde_json::from_slice(bytes).unwrap_or_default();
        return Markers::from([(latest, list)]);
    }
    serde_json::from_slice(bytes).unwrap_or_default()
}

impl Room {
    /// The strokes of the map pages and the relay were last told.
    pub(super) fn current_marker(&self) -> Vec<Kept> {
        let all = lock(&self.marker);
        all.get(&self.marker_load.load(Ordering::Relaxed)).cloned().unwrap_or_default()
    }

    /// The room is on map `n`: its pages are told that map's marker, its relay is handed it.
    pub(super) fn show_marker(&self, n: usize) {
        let all = lock(&self.marker);
        if self.marker_load.swap(n, Ordering::Relaxed) == n {
            return;
        }
        announce(&self.hub, "marker", &kept_line(all.get(&n).map_or(&[], Vec::as_slice)));
        drop(all);
        self.keep_marker();
    }

    /// Drops the markers of the maps the trim dropped (the oldest first), whenever they came.
    pub(super) fn trim_markers(&self) {
        let Some(first) = self.recording.parts().first().map(|p| p.load) else { return };
        let mut all = lock(&self.marker);
        if all.keys().next().is_some_and(|n| *n < first) {
            all.retain(|n, _| *n >= first);
        }
    }
}
