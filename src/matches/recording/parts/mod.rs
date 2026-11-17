//! A match's parts: each map load of it that showed something, one after another in a recording.
//! A part begins at its load's first snapshot (see [`Loads`](crate::matches::loads::Loads)); the first of a recording at its
//! first line. What comes between a part's last snapshot and the next part's first (the server
//! changing map, the next map's server and world lines, its signon again and again) is the part's
//! tail.

mod part;
mod recorded;

pub(crate) use self::part::Part;
pub(super) use self::part::{Header, Parts, Seed};

#[cfg(test)]
pub(crate) mod real_tests;
#[cfg(test)]
pub(crate) mod tests;

#[cfg(test)]
mod file_tests;
