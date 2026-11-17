//! A match as a room plays it: stvtf lines in gzipped chunks of play time, each starting from a
//! checkpoint of the hub, so a seek replays at most one chunk. A live match's recording grows
//! as it is played: its last chunk stays open, as text, until its time is up, and each chunk
//! closed can go to a file (one gzip member after another), from which it resumes. A new map
//! starts a part, and nothing before it is dropped for that; what is more than [`KEEP_MS`](self::lines::KEEP_MS) old is,
//! though chunks and marks keep their numbers.

mod lines;
mod parts;

pub(crate) use self::lines::{Mark, Recording, Save, Sink};
pub(crate) use self::parts::Part;

#[cfg(test)]
pub(crate) mod tests;

#[cfg(test)]
pub(crate) use self::parts::{real_tests as real_part_tests, tests as part_tests};
