//! What a floor is made of, guessed from its texture name.

use serde::Serialize;

/// The material a floor is drawn as.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Serialize)]
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

/// A surface's colour, served as `#rrggbb`.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Tint(pub [u8; 3]);

impl Serialize for Tint {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let [r, g, b] = self.0;
        s.serialize_str(&format!("#{r:02x}{g:02x}{b:02x}"))
    }
}

impl Tint {
    /// The colour seen for a texture's reflectivity (its mean colour, linear, as the map's
    /// compiler stored it); None when it has none (black).
    pub(super) fn of(reflectivity: [f32; 3]) -> Option<Tint> {
        let seen = |v: f32| (v.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0).round() as u8;
        (reflectivity.iter().all(|v| v.is_finite()) && reflectivity.iter().any(|v| *v > 0.0)).then(|| Tint(reflectivity.map(seen)))
    }

    /// `share` (0 to 1) of the way to `other`.
    pub(super) fn towards(self, other: Tint, share: f32) -> Tint {
        Tint(std::array::from_fn(|i| (f32::from(self.0[i]) + (f32::from(other.0[i]) - f32::from(self.0[i])) * share).round() as u8))
    }

    fn far_from(self, other: Tint) -> i32 {
        (0..3).map(|i| (i32::from(self.0[i]) - i32::from(other.0[i])).pow(2)).sum()
    }
}

/// The colours in TF2 of the materials a blend texture's name can mention. A blend's reflectivity
/// is that of its first texture alone, and that of its second texture is not in the map.
const BLENDS: [(&[&str], Tint); 8] = [
    (&["grass"], Tint([0x62, 0x59, 0x2a])),
    (&["snow"], Tint([0xba, 0xcd, 0xda])),
    (&["gravel"], Tint([0x79, 0x66, 0x4e])),
    (&["ground", "dirt"], Tint([0x7c, 0x68, 0x4f])),
    (&["rock", "cliff"], Tint([0x6b, 0x67, 0x63])),
    (&["concrete"], Tint([0x83, 0x74, 0x65])),
    (&["cobble"], Tint([0x55, 0x53, 0x54])),
    (&["leaf"], Tint([0x91, 0x56, 0x3f])),
];

/// The colour of the second texture of blend `texture`, whose first texture has colour `first`:
/// of the materials in its name, the one least like `first`. None when `texture` is not a blend.
pub(super) fn blended_with(texture: &str, first: Tint) -> Option<Tint> {
    let name = original(texture);
    let file = name.rsplit('/').next().unwrap_or(name);
    let named = BLENDS.iter().filter(|(words, _)| words.iter().any(|w| file.contains(w))).map(|(_, tint)| *tint);
    named.filter(|_| file.contains("blend")).max_by_key(|t| t.far_from(first))
}

/// The suffix of a blend texture's patched copy (`WorldVertexTransition`) in the map's own pak.
const PATCH_SUFFIX: &str = "_wvt_patch";

/// `name` without a leading `maps/<map>/` and a trailing `_x_y_z` (any of them negative) or
/// `_wvt_patch`: the patched copy of a texture in a map's pak is named after the map and where it
/// is.
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
    fn a_textures_colour_is_its_reflectivity_and_a_blends_second_the_material_its_name_has() {
        // concrete/concretewall011 and nature/blendgroundtograss001, as cp_gullywash_f9 has them.
        assert_eq!(serde_json::to_string(&Tint::of([0.486, 0.431, 0.376])).unwrap(), "\"#b8aea3\"");
        assert_eq!(Tint::of([0.0, 0.0, 0.0]), None);
        let ground = Tint::of([0.421, 0.262, 0.119]).unwrap();
        let grass = blended_with("nature/blendgroundtograss001", ground).unwrap();
        assert_eq!(grass, Tint([0x62, 0x59, 0x2a]));
        assert_eq!(blended_with("maps/cp_x/snowtower/blendmetalroofsnow_026_wvt_patch", ground), Some(Tint([0xba, 0xcd, 0xda])));
        assert_eq!(blended_with("nature/grass_03", ground), None);
        assert_eq!(ground.towards(grass, 0.5), Tint([0x87, 0x72, 0x46]));
    }

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
