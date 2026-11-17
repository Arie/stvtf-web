//! The HTTP side: pages, files, event streams and starting matches.

pub(crate) mod server;
pub(crate) mod token;
pub(crate) mod voice;

mod answer;
mod asked;
mod assets;
mod client;
mod content;
mod events;
mod limits;
mod matches;
mod mine;
mod pages;
mod rooms;
mod routes;
mod start;
mod unpack;
mod upload;
mod visitor;

pub(crate) use self::answer::{Answered, Early, Shared};
pub(crate) use self::assets::app_version;
pub(crate) use self::limits::MAX_STREAMS_PER_CLIENT;

#[cfg(test)]
mod tests;
