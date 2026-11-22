//! A match's voice routes, the same under its page and under the page of a room on it.

pub(crate) mod link;
pub(crate) mod mumble;
pub(crate) mod who;
pub(crate) mod who_sync;

mod incoming;
pub(super) use self::incoming::is_key;
mod routes;

pub(super) use self::routes::{comms, invite, known, link, linked, matched, mumble, mumble_jobs, status, told, voice, voices, who};
