//! A room: a recording played into its own hub, paused, sped up and sought by its owner, with
//! everyone in it seeing the same.

mod arc;
mod beams;
mod circle;
mod drawing;
mod gap;
mod light;
mod markers;
mod note;
mod part;
mod play;
mod relay;
mod state;
mod view;

pub(crate) use self::arc::ArcDraw;
pub(crate) use self::circle::CircleDraw;
pub(crate) use self::drawing::Draw;
pub(crate) use self::light::LightDraw;
pub(crate) use self::markers::read_marker;
pub(crate) use self::note::NoteDraw;
pub(crate) use self::part::MapChoice;
pub(crate) use self::play::Control;
pub(crate) use self::state::{Room, SPEEDS, Saved, StartAt};
pub(crate) use self::view::ViewPost;

#[cfg(test)]
mod follow_tests;
#[cfg(test)]
mod gap_tests;
#[cfg(test)]
mod live_tests;
#[cfg(test)]
mod move_tests;
#[cfg(test)]
mod part_tests;
#[cfg(test)]
mod stream_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod voice_tests;
