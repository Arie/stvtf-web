//! The entity lump and what is read from it: bounds, packs, capture areas, spawns.

use anyhow::Result;
use stvtf::output::{PickupItem, PickupSize, Team, round1};

use super::format::{LUMP_ENTITIES, LUMP_MODELS, MODEL_MAXS, MODEL_MINS, MODEL_SIZE, lump, vec3_at};
use super::{Bounds, CapArea, PAD, Pack, SpawnPoint, SpawnRoom, Spawns, Vec3};

/// Spawns frame the map when they are this near the points and packs.
const SPAWN_REACH: f32 = 4096.0;
/// `TeamNum` of an entity on each team.
const TEAM_RED: &str = "2";
const TEAM_BLUE: &str = "3";

/// One entity of the entity lump: its keys and values.
pub(super) struct Entity(pub(super) Vec<(String, String)>);

impl Entity {
    /// The value of `key`; Hammer keys are not case-sensitive.
    pub(super) fn get(&self, key: &str) -> Option<&str> {
        self.0.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())
    }

    pub(super) fn class(&self) -> &str {
        self.get("classname").unwrap_or_default()
    }

    pub(super) fn origin(&self) -> Option<Vec3> {
        let v: Vec<f32> = self.get("origin")?.split_whitespace().filter_map(|s| s.parse().ok()).collect();
        <Vec3>::try_from(v.as_slice()).ok()
    }

    pub(super) fn team(&self) -> Team {
        match self.get("TeamNum") {
            Some(TEAM_RED) => Team::Red,
            Some(TEAM_BLUE) => Team::Blue,
            _ => Team::Unassigned,
        }
    }

    pub(super) fn start_disabled(&self) -> bool {
        self.get("StartDisabled") == Some("1")
    }
}

pub(super) fn entities(data: &[u8]) -> Result<Vec<Entity>> {
    let text = String::from_utf8_lossy(&lump(data, LUMP_ENTITIES)?).into_owned();
    let mut out = Vec::new();
    for block in text.split('{').skip(1) {
        let body = block.split('}').next().unwrap_or("");
        let quoted: Vec<&str> = body.split('"').skip(1).step_by(2).collect();
        out.push(Entity(quoted.chunks(2).filter(|kv| kv.len() == 2).map(|kv| (kv[0].to_string(), kv[1].to_string())).collect()));
    }
    Ok(out)
}

/// What the spawns, packs and capture areas come from: the entities, the brush models some of
/// them are, and the game's index of each control point.
pub(super) struct Entities {
    pub(super) list: Vec<Entity>,
    pub(super) models: Vec<u8>,
    /// `team_control_point` targetname -> the point's index in the game, which is its rank by
    /// `point_index` (`cp_gullywash` numbers its points from 1).
    pub(super) points: Vec<(String, i64)>,
}

impl Entities {
    pub(super) fn read(data: &[u8]) -> Result<Self> {
        let list = entities(data)?;
        let mut points: Vec<(String, i64)> = list
            .iter()
            .filter(|e| e.class() == "team_control_point")
            .filter_map(|e| Some((e.get("targetname")?.to_string(), e.get("point_index")?.parse().ok()?)))
            .collect();
        points.sort_by_key(|p| p.1);
        let mut ranks: Vec<i64> = points.iter().map(|p| p.1).collect();
        ranks.dedup();
        let points = points.into_iter().map(|(name, i)| (name, ranks.iter().position(|&r| r == i).unwrap_or_default() as i64)).collect();
        Ok(Entities { list, models: lump(data, LUMP_MODELS)?, points })
    }

    pub(super) fn of_class<'a>(&'a self, class: &'a str) -> impl Iterator<Item = &'a Entity> {
        self.list.iter().filter(move |e| e.class() == class)
    }

    /// The control point a key of `e` names, by the game's index.
    pub(super) fn point(&self, e: &Entity, key: &str) -> Option<i64> {
        let target = e.get(key)?;
        self.points.iter().find(|(name, _)| name.eq_ignore_ascii_case(target)).map(|p| p.1)
    }

    /// A brush entity's bounds: its model's box moved by its origin.
    pub(super) fn brush_bounds(&self, e: &Entity) -> Option<Bounds> {
        let m = usize::from(e.get("model")?.strip_prefix('*')?.parse::<u16>().ok()?);
        let model = self.models.get(m * MODEL_SIZE..(m + 1) * MODEL_SIZE)?;
        let origin = e.origin().unwrap_or([0.0; 3]);
        let corner = |o: usize| -> Option<Vec3> {
            let v = vec3_at(model, o).ok()?;
            Some([0, 1, 2].map(|i| origin[i] + v[i]))
        };
        Some(Bounds { lo: corner(MODEL_MINS)?, hi: corner(MODEL_MAXS)? })
    }
}

/// Points, pickups and the spawns near them; maps also hide unused spawns far away. None for a
/// map without points or pickups (MGE, jump maps): its spawns frame no more than a spawn room.
pub(super) fn playable_bounds(ents: &[Entity]) -> Option<Bounds> {
    let (mut core, mut spawns) = (Vec::new(), Vec::new());
    for ent in ents {
        let Some(p) = ent.origin() else { continue };
        let class = ent.class();
        if class == "team_control_point" || pack_of(class).is_some() {
            core.push(p);
        } else if class == "info_player_teamspawn" {
            spawns.push(p);
        }
    }
    if core.is_empty() {
        return None;
    }
    let near = Bounds::around(&core, SPAWN_REACH);
    core.extend(spawns.into_iter().filter(|s| near.contains(s)));
    Some(Bounds::around(&core, PAD))
}

/// A health or ammo pack's kind and its size's name, from its class.
pub(super) fn pack_of(class: &str) -> Option<(PickupItem, &str)> {
    class
        .strip_prefix("item_healthkit_")
        .map(|s| (PickupItem::Health, s))
        .or_else(|| class.strip_prefix("item_ammopack_").map(|s| (PickupItem::Ammo, s)))
}

/// Health and ammo packs by size.
pub(super) fn packs(ents: &[Entity]) -> Vec<Pack> {
    ents.iter()
        .filter_map(|e| {
            let (kind, size) = pack_of(e.class())?;
            let size = match size {
                "small" => PickupSize::Small,
                "medium" => PickupSize::Medium,
                "full" => PickupSize::Full,
                _ => return None,
            };
            Some(Pack { kind, size, pos: e.origin()?.map(round1) })
        })
        .collect()
}

/// Capture areas (`trigger_capture_area`: their brush model's bounds) and the point each caps.
/// Payload ones are parented to the cart and move with it, so their bounds are just its start.
pub(super) fn cap_areas(map: &Entities) -> Vec<CapArea> {
    map.of_class("trigger_capture_area")
        .filter(|e| e.get("parentname").is_none_or(str::is_empty))
        .filter_map(|e| {
            let (min, max) = map.brush_bounds(e)?.rounded();
            Some(CapArea { point: map.point(e, "area_cap_point"), min, max })
        })
        .collect()
}

/// Spawn rooms (`func_respawnroom`: their brush model's bounds) and spawn points
/// (`info_player_teamspawn`), with the control point each spawn point belongs to: TF2 enables
/// such a spawn point for the team that owns that point.
pub(super) fn spawns(map: &Entities) -> Spawns {
    let mut rooms: Vec<SpawnRoom> = map
        .of_class("func_respawnroom")
        .filter_map(|e| {
            let (min, max) = map.brush_bounds(e)?.rounded();
            let name = e.get("targetname").unwrap_or_default().to_string();
            Some(SpawnRoom { name, team: e.team(), min, max, disabled: e.start_disabled() })
        })
        .collect();
    let points: Vec<SpawnPoint> = map
        .of_class("info_player_teamspawn")
        // Team-less spawn points are unused leftovers (on cp_process far outside the map).
        .filter(|e| e.team() != Team::Unassigned)
        .filter_map(|e| {
            Some(SpawnPoint {
                team: e.team(),
                pos: e.origin()?.map(round1),
                point: map.point(e, "controlpoint"),
                disabled: e.start_disabled(),
            })
        })
        .collect();
    give_rooms_teams(&mut rooms, &points);
    Spawns { rooms, points }
}

/// Some maps (`pl_swiftwater`) give a room its team by input: take the team of the spawn points
/// inside, else of the room around its centre.
pub(super) fn give_rooms_teams(rooms: &mut [SpawnRoom], points: &[SpawnPoint]) {
    for by_points in [true, false] {
        for i in 0..rooms.len() {
            if rooms[i].team != Team::Unassigned {
                continue;
            }
            let found = if by_points {
                let teams: Vec<Team> = points.iter().filter(|p| rooms[i].holds(p.pos)).map(|p| p.team).collect();
                teams.first().copied().filter(|t| teams.iter().all(|u| u == t))
            } else {
                let centre = rooms[i].centre();
                rooms.iter().find(|o| o.team != Team::Unassigned && o.holds(centre)).map(|o| o.team)
            };
            if let Some(team) = found {
                rooms[i].team = team;
            }
        }
    }
}
