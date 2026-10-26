//! What a floor is made of, guessed from its texture name.

use serde::Serialize;

/// The material a floor is drawn as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Grass,
    Dirt,
    Concrete,
    Metal,
    Wood,
    Tile,
    Water,
    Lava,
    Snow,
    Other,
}

/// Words that give a texture its kind. The first row with a word in the name wins, so the order
/// matters: water and lava beat everything, snow comes after metal (a snowy roof is a roof), and
/// the plain `floor` and `wall` of concrete come last.
const KINDS: [(Kind, &[&str]); 9] = [
    (Kind::Water, &["water", "slime"]),
    (Kind::Lava, &["lava"]),
    (Kind::Grass, &["grass", "leaf", "moss", "foliage"]),
    (Kind::Dirt, &["dirt", "ground", "mud", "gravel", "sand", "beach", "rock", "cliff", "stone", "nature/"]),
    (Kind::Wood, &["wood", "plank", "barn", "crate"]),
    (Kind::Metal, &["metal", "steel", "grate", "ibeam", "tin", "iron", "rail", "roof"]),
    (Kind::Snow, &["snow", "/ice"]),
    (Kind::Tile, &["tile", "brick", "street", "cobble", "asphalt", "road"]),
    (Kind::Concrete, &["concrete", "cement", "plaster", "wall", "floor"]),
];

/// The suffix a patched copy of a texture gets in the map's own pak: its position.
const PATCH_SUFFIX: &str = "_wvt_patch";

/// `name` without a leading `maps/<map>/` and a trailing `_x_y_z` (any of them negative) or
/// `_wvt_patch`: the patched copy of a texture in a map's pak is named after the map and where it is.
fn original(name: &str) -> &str {
    let name = name.strip_prefix("maps/").map_or(name, |rest| rest.split_once('/').map_or(rest, |(_, name)| name));
    if let Some(name) = name.strip_suffix(PATCH_SUFFIX) {
        return name;
    }
    let mut parts = name.rsplitn(4, '_');
    let numbers = [parts.next(), parts.next(), parts.next()];
    match parts.next() {
        Some(name) if numbers.iter().all(|n| n.is_some_and(|n| n.parse::<i32>().is_ok())) => name,
        _ => name,
    }
}

fn first_match(name: &str) -> Option<Kind> {
    KINDS.iter().find(|(_, words)| words.iter().any(|w| name.contains(w))).map(|(kind, _)| *kind)
}

/// The kind of the texture `texture` (lower case, as in the map's texture table).
pub(super) fn kind(texture: &str) -> Kind {
    let name = original(texture);
    // The file name first: a directory only decides when the file says nothing, so a roof in a
    // `pl_barnblitz` directory is not wood, while `metal/officedoor002` is still metal.
    let file = name.rsplit('/').next().unwrap_or(name);
    first_match(file).or_else(|| first_match(name)).unwrap_or(Kind::Other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_from_texture_names() {
        for (name, k) in [
            ("concrete/concretefloor003", Kind::Concrete),
            ("nature/blendgroundtograss007", Kind::Grass),
            ("nature/rockwall008", Kind::Dirt),
            ("wood/wood_bridge001", Kind::Wood),
            ("metal/metaldoor001", Kind::Metal),
            ("water/water_well_beneath.vmt", Kind::Water),
            ("nature/underworld_lava002", Kind::Lava),
            ("boreal/snow", Kind::Snow),
            ("soho/streetfloor003", Kind::Tile),
            ("dev/dev_blendmeasure2", Kind::Other),
            // Snow after metal: a snowy roof is a roof, and a bare "ice" is in "officedoor".
            ("maps/cp_sunshine/pl_barnblitz/blendmetalroofsnow_02_wvt_patch", Kind::Metal),
            ("metal/officedoor002", Kind::Metal),
            // The map's own name in a patched copy's path does not count.
            ("maps/pl_swiftwater_final1/tile/floor_tile_002a_2226_-4230_-944", Kind::Tile),
            ("maps/pl_swiftwater_final1/props/metalduct001a_2372_1872_-744", Kind::Metal),
            ("maps/pl_swiftwater_final1/water/water_hydro_cheap_2848_-2728_-944", Kind::Water),
        ] {
            assert_eq!(kind(name), k, "{name}");
        }
    }
}
