//! Small game rules shared by the client and the integrated server, for lack of being extracted:
//! block placed by an item with a different name, entity types forbidden in peaceful difficulty.

/// Block placed by an item whose name differs (otherwise the item's own name).
pub fn placed_block(item: &str) -> &str {
    match item {
        "minecraft:redstone" => "minecraft:redstone_wire",
        "minecraft:string" => "minecraft:tripwire",
        "minecraft:wheat_seeds" => "minecraft:wheat",
        "minecraft:carrot" => "minecraft:carrots",
        "minecraft:potato" => "minecraft:potatoes",
        "minecraft:beetroot_seeds" => "minecraft:beetroots",
        "minecraft:melon_seeds" => "minecraft:melon_stem",
        "minecraft:pumpkin_seeds" => "minecraft:pumpkin_stem",
        "minecraft:torchflower_seeds" => "minecraft:torchflower_crop",
        "minecraft:pitcher_pod" => "minecraft:pitcher_crop",
        "minecraft:sweet_berries" => "minecraft:sweet_berry_bush",
        "minecraft:cocoa_beans" => "minecraft:cocoa",
        "minecraft:glow_berries" => "minecraft:cave_vines",
        "minecraft:powder_snow_bucket" => "minecraft:powder_snow",
        _ => item,
    }
}

/// Entity types forbidden in peaceful difficulty (recorded from game 26.3 code, sorted).
pub const NOT_IN_PEACEFUL: [&str; 38] = [
    "minecraft:blaze",
    "minecraft:bogged",
    "minecraft:breeze",
    "minecraft:cave_spider",
    "minecraft:creaking",
    "minecraft:creeper",
    "minecraft:drowned",
    "minecraft:elder_guardian",
    "minecraft:enderman",
    "minecraft:endermite",
    "minecraft:evoker",
    "minecraft:ghast",
    "minecraft:giant",
    "minecraft:guardian",
    "minecraft:hoglin",
    "minecraft:husk",
    "minecraft:illusioner",
    "minecraft:magma_cube",
    "minecraft:parched",
    "minecraft:phantom",
    "minecraft:piglin_brute",
    "minecraft:pillager",
    "minecraft:ravager",
    "minecraft:silverfish",
    "minecraft:skeleton",
    "minecraft:slime",
    "minecraft:spider",
    "minecraft:stray",
    "minecraft:vex",
    "minecraft:vindicator",
    "minecraft:warden",
    "minecraft:witch",
    "minecraft:wither",
    "minecraft:wither_skeleton",
    "minecraft:zoglin",
    "minecraft:zombie",
    "minecraft:zombie_villager",
    "minecraft:zombified_piglin",
];

/// The entity type can exist in peaceful difficulty.
pub fn allowed_in_peaceful(entity: &str) -> bool {
    NOT_IN_PEACEFUL.binary_search(&entity).is_err()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peaceful_list_is_sorted_for_the_search() {
        assert!(NOT_IN_PEACEFUL.windows(2).all(|w| w[0] < w[1]));
        assert!(!allowed_in_peaceful("minecraft:zombie"));
        assert!(!allowed_in_peaceful("minecraft:giant"));
        assert!(allowed_in_peaceful("minecraft:piglin"));
        assert!(allowed_in_peaceful("minecraft:cow"));
    }

    #[test]
    fn crops_and_wires_place_their_block() {
        assert_eq!(placed_block("minecraft:carrot"), "minecraft:carrots");
        assert_eq!(placed_block("minecraft:glow_berries"), "minecraft:cave_vines");
        assert_eq!(placed_block("minecraft:stone"), "minecraft:stone");
    }
}
