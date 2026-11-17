//! A stroke drawn on the map from above, laid on every floor under it, as lines in the world:
//! what a TF2 client in a room's relay is shown.
//!
//! The overview cannot tell a balcony from the route under it, so a stroke goes on both. Along the
//! stroke each floor under it continues the line on the floor before when it is within a step or a
//! slope of it; a line goes on over a short gap (a wall, a railing) and ends after a longer one.

mod ground;
mod strokes;

pub(crate) use self::ground::{Ground, ground};
pub(crate) use self::strokes::{Drape, Piece, dist};

#[cfg(test)]
mod tests;
