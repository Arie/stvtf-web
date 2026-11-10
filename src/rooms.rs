//! Rooms: a recording played for everyone in it, owned and drawn on by its admins.

pub(crate) mod people;
pub(crate) mod registry;
pub(crate) mod room;

/// How much of a room's id the log shows.
const LOGGED_ID: usize = 8;

/// Room `id` as the log names it.
pub(crate) fn logged(id: &str) -> &str {
    id.get(..LOGGED_ID).unwrap_or(id)
}
