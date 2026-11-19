//! A map's top-down geometry: floors, walls, packs, capture areas and spawns.

use serde::Serialize;
use stvtf::output::{PickupItem, PickupSize, Team};

use super::material::{Kind, Tint};
use super::read::SPAWN_SLACK;

/// Room around the points, packs and spawns that frame the playable area.
pub(crate) const PAD: f32 = 1536.0;

pub(super) type Vec3 = [f32; 3];

/// A position as served, to a tenth of a unit.
pub(super) type Position = [f64; 3];

/// A map's top-down geometry, as `/map/NAME` serves it.
#[derive(Debug, Serialize)]
pub struct Geometry {
    pub spawns: Spawns,
    pub packs: Vec<Pack>,
    pub cap_areas: Vec<CapArea>,
    /// The playable area: lowest and highest corner, unrounded (widened from f32, as served
    /// before).
    pub bounds: Option<[[f64; 3]; 2]>,
    /// Upward faces, from low to high.
    pub floors: Vec<Floor>,
    /// The tops of solid static props (see `props`), for strokes drawn on them in TF2.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub props: Vec<Floor>,
    pub walls: Vec<Wall>,
    /// The payload track's segments (`x1, y1, z1, x2, y2, z2`); empty without a payload cart.
    pub track: Vec<[f64; 6]>,
    /// Where players can get to, seen from above; None when it could not be worked out.
    pub playable: Option<super::reach::Mask>,
}

/// An upward face: its height, outline (`x, y, x, y, …`) and what it is made of.
#[derive(Debug, Serialize)]
pub struct Floor {
    /// The height at the mean of the outline's points.
    pub z: f64,
    pub p: Vec<f64>,
    pub k: Kind,
    /// A cell of a displacement (terrain), drawn at its real height.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub d: bool,
    /// How much the height rises per unit along x and y; absent on a flat floor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub g: Option<[f64; 2]>,
    /// Its texture's colour; absent where the map has none for it, and on water and lava.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub c: Option<Tint>,
}

/// A wall seen from above: its top and its two ends (`x1, y1, x2, y2`).
#[derive(Debug, Serialize)]
pub struct Wall {
    pub z: f64,
    pub p: [f64; 4],
    /// Its texture's colour, as a floor's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub c: Option<Tint>,
}

/// A health or ammo pack.
#[derive(Debug, Serialize)]
pub struct Pack {
    pub kind: PickupItem,
    pub size: PickupSize,
    pub pos: Position,
}

/// A capture area's bounds, and the point it captures (by the game's index).
#[derive(Debug, Serialize)]
pub struct CapArea {
    pub point: Option<i64>,
    pub min: Position,
    pub max: Position,
}

#[derive(Debug, Serialize)]
pub struct Spawns {
    pub rooms: Vec<SpawnRoom>,
    pub points: Vec<SpawnPoint>,
}

/// A spawn room (`func_respawnroom`).
#[derive(Debug, Serialize)]
pub struct SpawnRoom {
    pub name: String,
    pub team: Team,
    pub min: Position,
    pub max: Position,
    pub disabled: bool,
}

impl SpawnRoom {
    /// Whether `p` is in the room, with some room above and below for where spawn points stand.
    pub(super) fn holds(&self, p: Position) -> bool {
        (0..3).all(|k| {
            let slack = if k == 2 { SPAWN_SLACK } else { 0.0 };
            p[k] >= self.min[k] - slack && p[k] <= self.max[k] + slack
        })
    }

    pub(super) fn centre(&self) -> Position {
        [0, 1, 2].map(|k| f64::midpoint(self.min[k], self.max[k]))
    }
}

/// A spawn point (`info_player_teamspawn`), with the control point it belongs to.
#[derive(Debug, Serialize)]
pub struct SpawnPoint {
    pub team: Team,
    pub pos: Position,
    pub point: Option<i64>,
    pub disabled: bool,
}
