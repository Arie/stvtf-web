//! A match's voice routes, the same on its page and on a room's on it.

pub(crate) mod link;
pub(crate) mod mumble;
pub(crate) mod who;
pub(crate) mod who_sync;

mod incoming;
mod routes;

pub(super) use self::routes::{comms, invite, known, link, linked, matched, mumble, mumble_jobs, status, told, voice, voices, who};
