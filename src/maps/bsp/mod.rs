//! Top-down geometry from a Source BSP: walkable floors and wall outlines.
//!
//! Control points, pickups and nearby spawns frame the playable area, which keeps
//! the 3D skybox out of the picture.

mod displacement;
mod entities;
mod floors;
mod format;
mod geometry;
mod material;
mod polygon;
mod props;
mod reach;
mod read;
mod track;

pub(crate) use self::geometry::PAD;
pub use self::geometry::{CapArea, Floor, Geometry, Pack, SpawnPoint, SpawnRoom, Spawns, Wall};
pub(super) use self::polygon::{contains_point, crosses, outline_box};
pub(super) use self::read::cross;
pub use self::read::geometry;

#[cfg(test)]
pub(crate) mod tests;
