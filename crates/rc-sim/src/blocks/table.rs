//! Block table for the simulation: behaviour family of each block (from its
//! identifier, as the game associates a class with each block), common identifiers, fast
//! access to the properties of a state, facts extracted from the game (`rc_data::sim_bits`).

use hashbrown::HashMap;

use rc_data::prop_index::PropSlot;
use rc_data::{PropIndex, PropKey, VersionData, flags, sim_bits};

use super::Dir;

/// Behaviour family of a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Other,
    Air,
    // Redstone.
    Wire,
    Repeater,
    Comparator,
    RedstoneTorch,
    RedstoneWallTorch,
    Lever,
    /// Press delay, can be triggered by arrows.
    Button {
        ticks: u8,
        arrows: bool,
    },
    /// Simple plate: 0 all entities, 1 living entities.
    Plate {
        mobs_only: bool,
    },
    /// Weighted plate: number of entities for 15.
    WeightedPlate {
        max: u16,
    },
    RedstoneBlock,
    Observer,
    Lamp,
    Piston {
        sticky: bool,
    },
    PistonHead,
    MovingPiston,
    Door {
        hand: bool,
    },
    Trapdoor {
        hand: bool,
    },
    FenceGate,
    Dispenser,
    Dropper,
    Crafter,
    Hopper,
    Tnt,
    Target,
    NoteBlock,
    CopperBulb,
    DaylightDetector,
    Rail,
    PoweredRail,
    DetectorRail,
    // Fluids.
    Water,
    Lava,
    BubbleColumn,
    // Gravity.
    Falling {
        delay: u8,
    },
    ConcretePowder,
    Anvil,
    DragonEgg,
    // Plants and nature.
    Crop {
        max_age: u8,
    },
    Stem,
    AttachedStem,
    NetherWart,
    Cocoa,
    SweetBerryBush,
    Sapling,
    /// Grass, mycelium.
    Spreading,
    Nylium,
    Leaves,
    Ice,
    FrostedIce,
    Farmland,
    Fire,
    SoulFire,
    SugarCane,
    Cactus,
    /// Simple plant on dirt (grasses, flowers, ferns…).
    Bush,
    DoublePlant,
    /// Pitcher crop: double plant from age 3, light required at the bottom.
    PitcherCrop,
    Mushroom,
    /// Huge mushroom blocks: one face per direction, off against the same block.
    HugeMushroom,
    /// Pale hanging moss: hangs from a full underside or from itself, tip at the bottom.
    HangingMoss,
    /// Pale moss carpet: base and sides (low, high) along the walls.
    PaleMossCarpet,
    DeadBush,
    Vine,
    // Shapes and attachments.
    Fence,
    Wall,
    Pane,
    Stairs,
    Slab,
    Bed,
    Torch,
    WallTorch,
    Carpet,
    Ladder,
    Snow,
    Lantern,
    StandingSign,
    WallSign,
    Chest,
    Grindstone,
    Bell,
    // Attachments (`attach.rs`).
    TripwireHook,
    Tripwire,
    Multiface,
    /// Head of a climbing plant (grows upwards or downwards).
    GrowingHead {
        up: bool,
    },
    GrowingBody {
        up: bool,
    },
    Bamboo {
        sapling: bool,
    },
    Scaffolding,
    /// Small family with its own survival rule.
    Support(super::attach::Sup),
    // G6-F01: server click blocks (own block entity).
    Jukebox,
    Lectern,
    Campfire,
    /// G5-F11: beacon (ticking block entity, menu).
    Beacon,
    /// Wooden shelves (3 slots, side chain).
    Shelf,
    ChiseledBookshelf,
    DecoratedPot,
    // G5-F09: blocks with their own rules (tilt, growth, neighbourhood).
    BigDripleaf,
    BigDripleafStem,
    ChorusFlower,
    ChorusPlant,
    BuddingAmethyst,
    /// Heads and skulls (standing or wall-mounted): powered by redstone.
    Skull,
    /// Beehive and bee nest.
    Beehive,
    /// G6-F09a: speleothems (pointed dripstone if `dripstone`, sulfur spike otherwise): placement,
    /// survival, fall, growth (`speleothem.rs`).
    Speleothem {
        dripstone: bool,
    },
    /// G6-F09a: active sulfur (state from the water and the block below, ticking block entity).
    PotentSulfur,
    // G6-F14: game master blocks with server data (`tech_blocks.rs`).
    Structure,
    Jigsaw,
    Test,
    TestInstance,
}

/// Paths of the property reads and writes, read once when the table is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PropSwitches {
    /// `RUSTCRAFT_OPT_PROP_IDX`: a property is found by its typed key in the index, integers
    /// are read and written without parsing or formatting (`=0`: found by name, as before).
    pub by_index: bool,
    /// `RUSTCRAFT_OPT_STATE_FACT_BITS`: the hottest properties (facing, powered, lit, water…)
    /// are read from bits packed per state, one load per read.
    pub packed_facts: bool,
}

impl PropSwitches {
    pub fn from_env() -> PropSwitches {
        PropSwitches {
            by_index: rc_data::opt_switch!("RUSTCRAFT_OPT_PROP_IDX"),
            packed_facts: rc_data::opt_switch!("RUSTCRAFT_OPT_STATE_FACT_BITS"),
        }
    }
}

/// Legacy fluid levels a liquid block can be asked for: 0 (source) to 15, and 16, which no
/// block has (an empty falling fluid: the default state stays).
const LEGACY_LEVELS: usize = 17;

/// Identifiers (indices of `VersionData::blocks`) of the blocks named by the behaviours.
#[derive(Debug, Clone, Default)]
#[allow(dead_code)] // complete table of named blocks, not all of them are used yet
pub(crate) struct Ids {
    pub air: u16,
    pub cave_air: u16,
    pub void_air: u16,
    pub water: u16,
    pub lava: u16,
    pub stone: u16,
    pub cobblestone: u16,
    pub obsidian: u16,
    pub basalt: u16,
    pub soul_soil: u16,
    pub soul_sand: u16,
    pub blue_ice: u16,
    pub wire: u16,
    pub piston: u16,
    pub sticky_piston: u16,
    pub piston_head: u16,
    pub moving_piston: u16,
    pub slime: u16,
    pub honey: u16,
    pub comparator: u16,
    pub repeater: u16,
    pub hopper: u16,
    /// G6-F12: conduit (ticking block entity, no kind of its own).
    pub conduit: u16,
    /// G3-F13: furnaces and brewing stand (block entities ticked in the common list).
    pub furnace: u16,
    pub blast_furnace: u16,
    pub smoker: u16,
    pub brewing_stand: u16,
    pub fire: u16,
    pub soul_fire: u16,
    pub dirt: u16,
    pub farmland: u16,
    pub grass_block: u16,
    pub mycelium: u16,
    pub snow: u16,
    pub ice: u16,
    pub frosted_ice: u16,
    pub netherrack: u16,
    pub redstone_block: u16,
    pub observer: u16,
    pub tnt: u16,
    pub sugar_cane: u16,
    pub cactus: u16,
    /// Crossed-block pass (`inside_blocks`); powder snow also for the geyser column (G6-F09a).
    pub campfire: u16,
    pub soul_campfire: u16,
    pub powder_snow: u16,
    pub sand: u16,
    pub red_sand: u16,
    pub magma: u16,
    /// Redstone ores (lit when walked on).
    pub redstone_ore: u16,
    pub deepslate_redstone_ore: u16,
    /// Sculk sensors (normal, calibrated): "walked on" test of each entity on the ground.
    pub sculk_sensor: u16,
    pub calibrated_sculk_sensor: u16,
    pub kelp: u16,
    pub kelp_plant: u16,
    pub seagrass: u16,
    pub tall_seagrass: u16,
    pub bubble_column: u16,
    pub cactus_flower: u16,
    pub mangrove_propagule: u16,
    // C6-M02: blocks compared by their name in the behaviours (to be migrated by the missions
    // that own these files).
    pub nether_portal: u16,
    pub end_portal: u16,
    pub end_gateway: u16,
    pub bedrock: u16,
    pub frogspawn: u16,
    pub sniffer_egg: u16,
    pub creaking_heart: u16,
    pub sculk_shrieker: u16,
    pub dried_ghast: u16,
    pub command_block: u16,
    /// The two other command blocks, told apart by identifier at each of their ticks.
    pub chain_command_block: u16,
    pub repeating_command_block: u16,
    pub lightning_rod: u16,
    pub carved_pumpkin: u16,
    /// G6-F09a: blocks under which speleothems grow.
    pub dripstone_block: u16,
    pub sulfur: u16,
    /// G6-F08: nature ticks (vine spread, turtle eggs, eyeblossoms, cauldrons fed by a
    /// stalactite, mud dried into clay).
    pub vine: u16,
    pub turtle_egg: u16,
    pub open_eyeblossom: u16,
    pub closed_eyeblossom: u16,
    pub potted_open_eyeblossom: u16,
    pub potted_closed_eyeblossom: u16,
    pub cauldron: u16,
    pub water_cauldron: u16,
    pub lava_cauldron: u16,
    pub powder_snow_cauldron: u16,
    pub mud: u16,
    pub clay: u16,
}

pub(crate) struct Table {
    pub kind: Vec<Kind>,
    /// Properties of every block and state, by typed key.
    index: PropIndex,
    pub switches: PropSwitches,
    /// Liquid block state of water (row 0) and lava (row 1) per legacy level
    /// (`Server::legacy_block`), resolved once instead of writing the level at every call.
    liquid_states: [[u32; LEGACY_LEVELS]; 2],
    pub id: Ids,
    /// Block → solidified block (concrete powder → concrete), stem fruits, etc.
    pub pair: Vec<u16>,
    /// Flammability, combustibility (§ 2.3).
    pub fire: Vec<(u8, u8)>,
    /// Covered faces of each shape (fluid passage).
    pub face_masks: Vec<[[u64; 4]; 6]>,
    /// Per fluid (index of `VersionData::fluids`): 1 water, 2 lava, 0 other (exact names,
    /// sources and flowing), resolved once instead of comparing the name at each call.
    pub fluid_kind: Vec<u8>,
    /// Per fluid: 1 if the name contains `water`, 2 if it contains `lava` (mob view).
    pub fluid_mob: Vec<u8>,
    /// Per fluid: exactly `minecraft:water` (source).
    pub fluid_water_source: Vec<bool>,
    /// Per block: tints the beacon beam (beacon, stained glass and panes).
    pub beam: Vec<bool>,
    /// Per block: oxidation stage (0..3) of an aging (unwaxed) copper block, including
    /// the final stage that no longer ticks; `u8::MAX` otherwise. Resolved once (the sweep of the 128
    /// neighbours of the oxidation no longer compares names).
    pub weather: Vec<u8>,
    by_name: HashMap<String, u16>,
}

/// Oxidation stage from the name (see `Table::weather`): all unwaxed copper blocks
/// age, except the ore, the raw block and the copper torches (floor and wall).
pub(crate) fn weather_stage(name: &str) -> u8 {
    let n = name.strip_prefix("minecraft:").unwrap_or(name);
    if n.starts_with("waxed_") || n.contains("ore") || n.starts_with("raw_") || n.ends_with("torch") {
        return u8::MAX;
    }
    if !(n.contains("copper") || n.contains("lightning_rod")) {
        return u8::MAX;
    }
    if n.starts_with("exposed_") {
        1
    } else if n.starts_with("weathered_") {
        2
    } else if n.starts_with("oxidized_") {
        3
    } else {
        0
    }
}

/// Per fluid: 1 if the name contains `water`, 2 if it contains `lava`, 0 otherwise (`Table::fluid_mob`).
pub(crate) fn fluid_mob(d: &VersionData) -> Vec<u8> {
    d.fluids
        .iter()
        .map(|n| {
            if n.contains("water") {
                1
            } else if n.contains("lava") {
                2
            } else {
                0
            }
        })
        .collect()
}

fn find(d: &VersionData, name: &str) -> u16 {
    d.blocks.iter().position(|b| b.name == name).map_or(u16::MAX, |i| i as u16)
}

fn kind_of(name: &str) -> Kind {
    let n = name.strip_prefix("minecraft:").unwrap_or(name);
    if let Some(k) = super::attach::kind_of(n) {
        return k;
    }
    use Kind::*;
    match n {
        "air" | "cave_air" | "void_air" => return Air,
        "redstone_wire" => return Wire,
        "repeater" => return Repeater,
        "comparator" => return Comparator,
        "redstone_torch" => return RedstoneTorch,
        "redstone_wall_torch" => return RedstoneWallTorch,
        "lever" => return Lever,
        "stone_button" | "polished_blackstone_button" => return Button { ticks: 20, arrows: false },
        "light_weighted_pressure_plate" => return WeightedPlate { max: 15 },
        "heavy_weighted_pressure_plate" => return WeightedPlate { max: 150 },
        "stone_pressure_plate" | "polished_blackstone_pressure_plate" => return Plate { mobs_only: true },
        "redstone_block" => return RedstoneBlock,
        "observer" => return Observer,
        "redstone_lamp" => return Lamp,
        "piston" => return Piston { sticky: false },
        "sticky_piston" => return Piston { sticky: true },
        "piston_head" => return PistonHead,
        "moving_piston" => return MovingPiston,
        "iron_door" => return Door { hand: false },
        "iron_trapdoor" => return Trapdoor { hand: false },
        "dispenser" => return Dispenser,
        "dropper" => return Dropper,
        "crafter" => return Crafter,
        "hopper" => return Hopper,
        "tnt" => return Tnt,
        "target" => return Target,
        "note_block" => return NoteBlock,
        "daylight_detector" => return DaylightDetector,
        "rail" => return Rail,
        "powered_rail" | "activator_rail" => return PoweredRail,
        "detector_rail" => return DetectorRail,
        "water" => return Water,
        "lava" => return Lava,
        "bubble_column" => return BubbleColumn,
        // Suspicious sand and gravel: same fall and same ticks (2); their tick also resets the
        // brushing to zero (`brush.rs`).
        "sand" | "red_sand" | "gravel" | "suspicious_sand" | "suspicious_gravel" => return Falling { delay: 2 },
        "anvil" | "chipped_anvil" | "damaged_anvil" => return Anvil,
        "dragon_egg" => return DragonEgg,
        "wheat" | "carrots" | "potatoes" => return Crop { max_age: 7 },
        "beetroots" => return Crop { max_age: 3 },
        "torchflower_crop" => return Crop { max_age: 2 },
        "pumpkin_stem" | "melon_stem" => return Stem,
        "attached_pumpkin_stem" | "attached_melon_stem" => return AttachedStem,
        "nether_wart" => return NetherWart,
        "cocoa" => return Cocoa,
        "sweet_berry_bush" => return SweetBerryBush,
        "mangrove_propagule" => return Sapling,
        "grass_block" | "mycelium" => return Spreading,
        "crimson_nylium" | "warped_nylium" => return Nylium,
        "ice" => return Ice,
        "frosted_ice" => return FrostedIce,
        "snow" => return Snow,
        "farmland" => return Farmland,
        "fire" => return Fire,
        "soul_fire" => return SoulFire,
        "sugar_cane" => return SugarCane,
        "cactus" => return Cactus,
        "short_grass" | "fern" | "dandelion" | "poppy" | "blue_orchid" | "allium" | "azure_bluet" | "red_tulip"
        | "orange_tulip" | "white_tulip" | "pink_tulip" | "oxeye_daisy" | "cornflower" | "lily_of_the_valley"
        | "torchflower" | "open_eyeblossom" | "closed_eyeblossom" | "bush" | "red_shrub" | "firefly_bush" | "pink_petals"
        | "wildflowers" | "golden_dandelion" => {
            return Bush;
        }
        "dead_bush" => return DeadBush,
        "tall_grass" | "large_fern" | "sunflower" | "lilac" | "rose_bush" | "peony" | "pitcher_plant"
        | "small_dripleaf" => {
            return DoublePlant;
        }
        "pitcher_crop" => return PitcherCrop,
        "brown_mushroom" | "red_mushroom" => return Mushroom,
        "brown_mushroom_block" | "red_mushroom_block" | "mushroom_stem" => return HugeMushroom,
        "pale_hanging_moss" => return HangingMoss,
        "pale_moss_carpet" => return PaleMossCarpet,
        "vine" => return Vine,
        "torch" | "soul_torch" | "copper_torch" => return Torch,
        "wall_torch" | "soul_wall_torch" | "copper_wall_torch" => return WallTorch,
        "ladder" => return Ladder,
        "lantern" | "soul_lantern" => return Lantern,
        "chest" | "trapped_chest" => return Chest,
        "grindstone" => return Grindstone,
        "bell" => return Bell,
        "iron_bars" => return Pane,
        "jukebox" => return Jukebox,
        "lectern" => return Lectern,
        "beacon" => return Beacon,
        "campfire" | "soul_campfire" => return Campfire,
        "chiseled_bookshelf" => return ChiseledBookshelf,
        "decorated_pot" => return DecoratedPot,
        "big_dripleaf" => return BigDripleaf,
        "big_dripleaf_stem" => return BigDripleafStem,
        "chorus_flower" => return ChorusFlower,
        "chorus_plant" => return ChorusPlant,
        "budding_amethyst" => return BuddingAmethyst,
        "beehive" | "bee_nest" => return Beehive,
        "pointed_dripstone" => return Speleothem { dripstone: true },
        "sulfur_spike" => return Speleothem { dripstone: false },
        "potent_sulfur" => return PotentSulfur,
        "structure_block" => return Structure,
        "jigsaw" => return Jigsaw,
        "test_block" => return Test,
        "test_instance_block" => return TestInstance,
        _ => {}
    }
    // `piston_head` is taken above.
    if n.ends_with("_head") || n.ends_with("_skull") {
        return Skull;
    }
    if n.ends_with("_shelf") {
        return Shelf;
    }
    if n.ends_with("_button") {
        return Button { ticks: 30, arrows: true };
    }
    if n.ends_with("_pressure_plate") {
        return Plate { mobs_only: false };
    }
    if n.ends_with("_concrete_powder") {
        return ConcretePowder;
    }
    if n.ends_with("_sapling") {
        return Sapling;
    }
    if n.ends_with("_leaves") {
        return Leaves;
    }
    if n.ends_with("_door") {
        return Door { hand: true };
    }
    if n.ends_with("_trapdoor") {
        return Trapdoor { hand: true };
    }
    if n.ends_with("_fence_gate") {
        return FenceGate;
    }
    if n.ends_with("_fence") {
        return Fence;
    }
    if n.ends_with("_wall") && !n.ends_with("_wall_torch") {
        return Wall;
    }
    if n.ends_with("_pane") || n.ends_with("copper_bars") || n == "iron_bars" {
        return Pane;
    }
    if n.ends_with("_stairs") {
        return Stairs;
    }
    if n.ends_with("_slab") {
        return Slab;
    }
    if n.ends_with("_bed") {
        return Bed;
    }
    if n.ends_with("_carpet") && n != "pale_moss_carpet" {
        return Carpet;
    }
    if n.ends_with("copper_bulb") {
        return CopperBulb;
    }
    // Copper chests (eight variants, waxed included): chest rules, joining between
    // variants (`chest_connects`).
    if n.ends_with("copper_chest") {
        return Chest;
    }
    if n.ends_with("_wall_sign") {
        return WallSign;
    }
    if n.ends_with("_sign") && !n.ends_with("_hanging_sign") {
        return StandingSign;
    }
    if n.ends_with("_tulip") || n.ends_with("_orchid") {
        return Bush;
    }
    if n.ends_with("_lantern") {
        return Lantern;
    }
    Kind::Other
}

/// The 16 colors of the colored families (wool, carpet…).
const COLORS: [&str; 16] = [
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "light_gray",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
];

/// Flammability and combustibility (§ 2.3): table taken from the game, then families by
/// color (wool, its stairs and slabs 30/60; colored carpets 60/20). Computed once per
/// block when the table is created.
pub(crate) fn fire_odds(name: &str) -> (u8, u8) {
    let n = name.strip_prefix("minecraft:").unwrap_or(name);
    let t = &super::fire_table::FIRE_TABLE;
    if let Ok(i) = t.binary_search_by(|e| e.0.cmp(n)) {
        return (t[i].1, t[i].2);
    }
    let colored = |suffix: &str| n.strip_suffix(suffix).is_some_and(|c| COLORS.contains(&c));
    if colored("_wool") || colored("_wool_stairs") || colored("_wool_slab") {
        return (30, 60);
    }
    if colored("_carpet") {
        return (60, 20);
    }
    (0, 0)
}

impl Table {
    pub fn new(d: &VersionData) -> Table {
        Table::with_switches(d, PropSwitches::from_env())
    }

    pub fn with_switches(d: &VersionData, switches: PropSwitches) -> Table {
        let kind: Vec<Kind> = d.blocks.iter().map(|b| kind_of(&b.name)).collect();
        let by_name: HashMap<String, u16> = d
            .blocks
            .iter()
            .enumerate()
            .map(|(i, b)| (b.name.strip_prefix("minecraft:").unwrap_or(&b.name).to_owned(), i as u16))
            .collect();
        let f = |n: &str| find(d, &format!("minecraft:{n}"));
        let id = Ids {
            air: f("air"),
            cave_air: f("cave_air"),
            void_air: f("void_air"),
            water: f("water"),
            lava: f("lava"),
            stone: f("stone"),
            cobblestone: f("cobblestone"),
            obsidian: f("obsidian"),
            basalt: f("basalt"),
            soul_soil: f("soul_soil"),
            soul_sand: f("soul_sand"),
            blue_ice: f("blue_ice"),
            wire: f("redstone_wire"),
            piston: f("piston"),
            sticky_piston: f("sticky_piston"),
            piston_head: f("piston_head"),
            moving_piston: f("moving_piston"),
            slime: f("slime_block"),
            honey: f("honey_block"),
            comparator: f("comparator"),
            repeater: f("repeater"),
            hopper: f("hopper"),
            conduit: f("conduit"),
            furnace: f("furnace"),
            blast_furnace: f("blast_furnace"),
            smoker: f("smoker"),
            brewing_stand: f("brewing_stand"),
            fire: f("fire"),
            soul_fire: f("soul_fire"),
            dirt: f("dirt"),
            farmland: f("farmland"),
            grass_block: f("grass_block"),
            mycelium: f("mycelium"),
            snow: f("snow"),
            ice: f("ice"),
            frosted_ice: f("frosted_ice"),
            netherrack: f("netherrack"),
            redstone_block: f("redstone_block"),
            observer: f("observer"),
            tnt: f("tnt"),
            sugar_cane: f("sugar_cane"),
            cactus: f("cactus"),
            campfire: f("campfire"),
            soul_campfire: f("soul_campfire"),
            powder_snow: f("powder_snow"),
            sand: f("sand"),
            red_sand: f("red_sand"),
            magma: f("magma_block"),
            redstone_ore: f("redstone_ore"),
            deepslate_redstone_ore: f("deepslate_redstone_ore"),
            sculk_sensor: f("sculk_sensor"),
            calibrated_sculk_sensor: f("calibrated_sculk_sensor"),
            kelp: f("kelp"),
            kelp_plant: f("kelp_plant"),
            seagrass: f("seagrass"),
            tall_seagrass: f("tall_seagrass"),
            bubble_column: f("bubble_column"),
            cactus_flower: f("cactus_flower"),
            mangrove_propagule: f("mangrove_propagule"),
            nether_portal: f("nether_portal"),
            end_portal: f("end_portal"),
            end_gateway: f("end_gateway"),
            bedrock: f("bedrock"),
            frogspawn: f("frogspawn"),
            sniffer_egg: f("sniffer_egg"),
            creaking_heart: f("creaking_heart"),
            sculk_shrieker: f("sculk_shrieker"),
            dried_ghast: f("dried_ghast"),
            command_block: f("command_block"),
            chain_command_block: f("chain_command_block"),
            repeating_command_block: f("repeating_command_block"),
            lightning_rod: f("lightning_rod"),
            carved_pumpkin: f("carved_pumpkin"),
            dripstone_block: f("dripstone_block"),
            sulfur: f("sulfur"),
            vine: f("vine"),
            turtle_egg: f("turtle_egg"),
            open_eyeblossom: f("open_eyeblossom"),
            closed_eyeblossom: f("closed_eyeblossom"),
            potted_open_eyeblossom: f("potted_open_eyeblossom"),
            potted_closed_eyeblossom: f("potted_closed_eyeblossom"),
            cauldron: f("cauldron"),
            water_cauldron: f("water_cauldron"),
            lava_cauldron: f("lava_cauldron"),
            powder_snow_cauldron: f("powder_snow_cauldron"),
            mud: f("mud"),
            clay: f("clay"),
        };
        let pair = d
            .blocks
            .iter()
            .map(|b| {
                let n = b.name.strip_prefix("minecraft:").unwrap_or(&b.name);
                let other = if let Some(c) = n.strip_suffix("_powder") {
                    c.to_owned()
                } else if n == "pumpkin_stem" {
                    "attached_pumpkin_stem".into()
                } else if n == "melon_stem" {
                    "attached_melon_stem".into()
                } else if n == "attached_pumpkin_stem" {
                    "pumpkin_stem".into()
                } else if n == "attached_melon_stem" {
                    "melon_stem".into()
                } else {
                    return u16::MAX;
                };
                by_name.get(&other).copied().unwrap_or(u16::MAX)
            })
            .collect();
        let fire = d.blocks.iter().map(|b| fire_odds(&b.name)).collect();
        let fluid_kind = d
            .fluids
            .iter()
            .map(|n| match n.as_str() {
                "minecraft:water" | "minecraft:flowing_water" => 1,
                "minecraft:lava" | "minecraft:flowing_lava" => 2,
                _ => 0,
            })
            .collect();
        let fluid_mob = fluid_mob(d);
        let fluid_water_source = d.fluids.iter().map(|n| n == "minecraft:water").collect();
        // Blocks that the beacon beam passes through while tinting it: the beacon, stained glass and
        // stained glass panes.
        let beam = d
            .blocks
            .iter()
            .map(|b| {
                let n = b.name.strip_prefix("minecraft:").unwrap_or(&b.name);
                n == "beacon" || n.ends_with("_stained_glass") || n.ends_with("_stained_glass_pane")
            })
            .collect();
        let weather = d.blocks.iter().map(|b| weather_stage(&b.name)).collect();
        let mut table = Table {
            weather,
            beam,
            kind,
            index: PropIndex::new(d),
            switches,
            liquid_states: [[0; LEGACY_LEVELS]; 2],
            id,
            pair,
            fire,
            face_masks: super::fluid::face_masks(d),
            fluid_kind,
            fluid_mob,
            fluid_water_source,
            by_name,
        };
        table.liquid_states = table.legacy_liquid_states(d);
        table
    }

    /// Liquid states by legacy level, written by name (the path before the table) so that the
    /// table holds exactly what that path gave.
    fn legacy_liquid_states(&self, d: &VersionData) -> [[u32; LEGACY_LEVELS]; 2] {
        let v = View { table: self, data: d };
        [self.id.water, self.id.lava].map(|block| {
            let default = v.default_state(block);
            std::array::from_fn(|level| v.set_named(default, "level", &level.to_string()))
        })
    }

    /// Property index (tests: sabotages of its tables).
    #[cfg(test)]
    pub(crate) fn index_mut(&mut self) -> &mut PropIndex {
        &mut self.index
    }

    #[cfg(test)]
    pub(crate) fn index(&self) -> &PropIndex {
        &self.index
    }

    /// Liquid block state of water (`kind` 1) or lava (`kind` 2) at a legacy level, `None` for
    /// another kind or a level no liquid block can be asked for.
    #[inline]
    pub fn liquid_state(&self, kind: u8, level: i32) -> Option<u32> {
        let row = self.liquid_states.get(usize::from(kind).checked_sub(1)?)?;
        row.get(usize::try_from(level).ok()?).copied()
    }

    pub fn block_id(&self, name: &str) -> u16 {
        let n = name.strip_prefix("minecraft:").unwrap_or(name);
        self.by_name.get(n).copied().unwrap_or(u16::MAX)
    }
}

/// View of the table linked to the data: reading and writing the properties of a state.
///
/// Two borrowed references, so making one is free: the earlier owned version cloned two `Arc`s
/// (four atomic operations) on every call, and the server makes one per block read.
#[derive(Clone, Copy)]
pub(crate) struct View<'a> {
    pub table: &'a Table,
    pub data: &'a VersionData,
}

// Fails to compile if the view stops being `Copy`, which an owned field (an `Arc`) would cause.
const _: () = {
    const fn must_be_copy<T: Copy>() {}
    must_be_copy::<View<'static>>();
};

impl<'a> View<'a> {
    #[inline]
    pub fn block(&self, s: u32) -> u16 {
        self.data.states.get(s as usize).map_or(u16::MAX, |i| i.block)
    }

    #[inline]
    pub fn kind(&self, s: u32) -> Kind {
        self.table.kind.get(self.block(s) as usize).copied().unwrap_or(Kind::Other)
    }

    #[inline]
    pub fn info(&self, s: u32) -> rc_data::StateInfo {
        self.data.states.get(s as usize).copied().unwrap_or_default()
    }

    #[inline]
    pub fn sim(&self, s: u32) -> u16 {
        self.data.sim.get(s as usize).copied().unwrap_or(0)
    }

    pub fn default_state(&self, block: u16) -> u32 {
        self.data.blocks.get(block as usize).map_or(0, |b| b.default_state)
    }

    // Properties by name: names that come from commands, data or the player (any property of
    // any version), and the path before the typed keys (`RUSTCRAFT_OPT_PROP_IDX=0`).

    fn slot_named(&self, s: u32, name: &str) -> Option<(&'a PropSlot, u8)> {
        let info = self.data.states.get(s as usize)?;
        self.table.index.block_slots(info.block).iter().find_map(|p| {
            (self.data.properties[p.property as usize].name == name)
                .then(|| (p, self.data.state_values[info.values as usize + p.position as usize]))
        })
    }

    pub fn has_named(&self, s: u32, name: &str) -> bool {
        self.slot_named(s, name).is_some()
    }

    pub fn get_named(&self, s: u32, name: &str) -> Option<&'a str> {
        let (p, v) = self.slot_named(s, name)?;
        Some(self.data.properties[p.property as usize].values[v as usize].as_str())
    }

    /// State with `name = value` (unchanged if the property or the value does not exist).
    pub fn set_named(&self, s: u32, name: &str, value: &str) -> u32 {
        let Some((p, v)) = self.slot_named(s, name) else { return s };
        let Some(nv) = self.data.properties[p.property as usize].values.iter().position(|x| x == value) else {
            return s;
        };
        let delta = (nv as i64 - i64::from(v)) * i64::from(p.stride);
        (i64::from(s) + delta) as u32
    }

    fn cycle_named(&self, s: u32, name: &str) -> u32 {
        let Some((p, v)) = self.slot_named(s, name) else { return s };
        let nv = (u32::from(v) + 1) % u32::from(p.count);
        (i64::from(s) + (i64::from(nv) - i64::from(v)) * i64::from(p.stride)) as u32
    }

    /// Properties of state `s`: name, possible values (game order) and rank of the current
    /// value, sorted by name like the game's state definition (G5-F10, debug stick).
    pub fn props_sorted(&self, s: u32) -> Vec<(&'a str, &'a [String], usize)> {
        let Some(info) = self.data.states.get(s as usize) else { return Vec::new() };
        let mut out: Vec<(&'a str, &'a [String], usize)> = self
            .table
            .index
            .block_slots(info.block)
            .iter()
            .map(|p| {
                let d = &self.data.properties[p.property as usize];
                let current = self.data.state_values[info.values as usize + p.position as usize];
                (d.name.as_str(), d.values.as_slice(), usize::from(current))
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(b.0));
        out
    }

    // Properties by typed key: the index, the packed bits for the hottest reads, or the name.

    #[inline]
    pub fn has(&self, s: u32, key: PropKey) -> bool {
        if self.table.switches.by_index {
            return self.table.index.has(self.data, s, key);
        }
        self.has_named(s, key.name())
    }

    #[inline]
    pub fn get(&self, s: u32, key: PropKey) -> Option<&'a str> {
        if self.table.switches.by_index {
            return self.table.index.get(self.data, s, key);
        }
        self.get_named(s, key.name())
    }

    /// The property is `true` (absent: `false`).
    #[inline]
    pub fn get_bool(&self, s: u32, key: PropKey) -> bool {
        let t = self.table;
        if t.switches.packed_facts
            && let Some(b) = t.index.packed_bool(s, key)
        {
            return b;
        }
        if t.switches.by_index {
            return t.index.get_bool(self.data, s, key);
        }
        self.get_named(s, key.name()) == Some("true")
    }

    /// Integer value (absent or not a number: 0).
    #[inline]
    pub fn get_int(&self, s: u32, key: PropKey) -> i32 {
        let t = self.table;
        if t.switches.packed_facts
            && let Some(n) = t.index.packed_int(s, key)
        {
            return n;
        }
        if t.switches.by_index {
            return t.index.get_int(self.data, s, key);
        }
        self.get_named(s, key.name()).and_then(|v| v.parse().ok()).unwrap_or(0)
    }

    #[inline]
    pub fn get_dir(&self, s: u32, key: PropKey) -> Option<Dir> {
        let t = self.table;
        if t.switches.packed_facts
            && let Some(d) = t.index.packed_dir(s, key)
        {
            return d.map(|d| Dir::ALL[usize::from(d)]);
        }
        if t.switches.by_index {
            return t.index.get_dir(self.data, s, key).map(|d| Dir::ALL[usize::from(d)]);
        }
        self.get_named(s, key.name()).and_then(Dir::parse)
    }

    #[inline]
    pub fn facing(&self, s: u32) -> Dir {
        self.get_dir(s, PropKey::Facing).unwrap_or(Dir::North)
    }

    /// State with `key = value` (unchanged if the property or the value does not exist).
    #[inline]
    pub fn set(&self, s: u32, key: PropKey, value: &str) -> u32 {
        if self.table.switches.by_index {
            return self.table.index.set(self.data, s, key, value);
        }
        self.set_named(s, key.name(), value)
    }

    #[inline]
    pub fn set_bool(&self, s: u32, key: PropKey, v: bool) -> u32 {
        if self.table.switches.by_index {
            return self.table.index.set_bool(self.data, s, key, v);
        }
        self.set_named(s, key.name(), if v { "true" } else { "false" })
    }

    #[inline]
    pub fn set_int(&self, s: u32, key: PropKey, v: i32) -> u32 {
        if self.table.switches.by_index {
            return self.table.index.set_int(self.data, s, key, v);
        }
        self.set_named(s, key.name(), &v.to_string())
    }

    #[inline]
    pub fn set_dir(&self, s: u32, key: PropKey, d: Dir) -> u32 {
        if self.table.switches.by_index {
            return self.table.index.set_dir(self.data, s, key, d as u8);
        }
        self.set_named(s, key.name(), d.name())
    }

    /// State with `key = value` as a state rebuilt from text gives it (`Server::with_prop`): a
    /// value the property lacks becomes its default value. Index only: the path by name is the
    /// rebuild itself.
    #[inline]
    pub fn set_or_default(&self, s: u32, key: PropKey, value: &str) -> Option<u32> {
        self.table.index.set_or_default(self.data, s, key, value)
    }

    #[inline]
    pub fn set_int_or_default(&self, s: u32, key: PropKey, value: i32) -> Option<u32> {
        self.table.index.set_int_or_default(self.data, s, key, value)
    }

    /// Next value of the property (cycle).
    #[inline]
    pub fn cycle(&self, s: u32, key: PropKey) -> u32 {
        if self.table.switches.by_index {
            return self.table.index.cycle(self.data, s, key);
        }
        self.cycle_named(s, key.name())
    }

    // Facts.
    #[inline]
    pub fn is_air(&self, s: u32) -> bool {
        self.info(s).has(flags::AIR)
    }

    pub fn conductor(&self, s: u32) -> bool {
        self.sim(s) & sim_bits::CONDUCTOR != 0
    }

    pub fn push_reaction(&self, s: u32) -> u16 {
        (self.sim(s) & sim_bits::PUSH_MASK) >> sim_bits::PUSH_SHIFT
    }

    pub fn has_block_entity(&self, s: u32) -> bool {
        self.sim(s) & sim_bits::BLOCK_ENTITY != 0
    }

    pub fn has_analog(&self, s: u32) -> bool {
        self.sim(s) & sim_bits::ANALOG != 0
    }

    /// Sturdy face (full support) in direction `d`.
    pub fn sturdy(&self, s: u32, d: Dir) -> bool {
        self.info(s).sturdy & (1 << d as u8) != 0
    }

    pub fn rigid_up(&self, s: u32) -> bool {
        self.sim(s) & sim_bits::RIGID_UP != 0
    }

    pub fn center_up(&self, s: u32) -> bool {
        self.sim(s) & sim_bits::CENTER_UP != 0
    }

    pub fn center_down(&self, s: u32) -> bool {
        self.sim(s) & sim_bits::CENTER_DOWN != 0
    }

    pub fn full_collision(&self, s: u32) -> bool {
        self.info(s).has(flags::FULL_COLLISION)
    }

    pub fn replaceable(&self, s: u32) -> bool {
        self.info(s).has(flags::REPLACEABLE)
    }

    pub fn legacy_solid(&self, s: u32) -> bool {
        self.info(s).has(flags::LEGACY_SOLID)
    }

    pub fn random_ticks(&self, s: u32) -> bool {
        self.info(s).has(flags::RANDOM_TICK)
    }

    pub fn hardness(&self, s: u32) -> f32 {
        self.data.hardness.get(s as usize).copied().unwrap_or(0.0)
    }
}

#[cfg(test)]
mod fire_tests {
    use super::fire_odds;

    #[test]
    fn poplar_wood_burns_like_other_planks() {
        for n in ["poplar_planks", "poplar_slab", "poplar_stairs", "poplar_fence", "poplar_fence_gate"] {
            assert_eq!(fire_odds(n), fire_odds("oak_planks"), "{n}");
        }
        assert_ne!(fire_odds("poplar_planks"), (0, 0));
    }

    /// G6-F06: complete game table (the old name-based rule forgot wool stairs and slabs,
    /// and gave 60/100 to every simple plant, chorus flowers included).
    #[test]
    fn fire_table_matches_the_game() {
        let t = &super::super::fire_table::FIRE_TABLE;
        assert!(t.windows(2).all(|w| w[0].0 < w[1].0), "table sorted");
        for (n, a, b) in [
            ("white_wool_stairs", 30, 60),
            ("black_wool_slab", 30, 60),
            ("red_wool", 30, 60),
            ("lime_carpet", 60, 20),
            ("moss_carpet", 0, 0),
            ("pale_moss_carpet", 5, 100),
            ("leaf_litter", 60, 100),
            ("orange_poplar_leaves", 30, 60),
            ("stone", 0, 0),
        ] {
            assert_eq!(fire_odds(&format!("minecraft:{n}")), (a, b), "{n}");
        }
    }
}

/// Measurement of the block view (not a proof: the proofs are the simulation fingerprints).
#[cfg(test)]
mod view_bench {
    use std::sync::Arc;
    use std::time::Instant;

    use rc_data::PropKey;

    use super::{Table, View};

    /// Views made per thread and per state.
    const ROUNDS: u32 = 50;

    /// Mean nanoseconds per view made and read once, with `threads` threads at the same time,
    /// each reading the `states` states `ROUNDS` times.
    fn ns_per_view(threads: usize, states: u32, read: impl Fn(u32) -> bool + Sync) -> f64 {
        let start = Instant::now();
        std::thread::scope(|scope| {
            for _ in 0..threads {
                scope.spawn(|| {
                    let mut hits = 0u32;
                    for _ in 0..ROUNDS {
                        for s in 0..states {
                            hits += u32::from(read(std::hint::black_box(s)));
                        }
                    }
                    std::hint::black_box(hits);
                });
            }
        });
        start.elapsed().as_nanos() as f64 / f64::from(ROUNDS * states)
    }

    /// Borrowed view against the former owned one (two `Arc` clones and two drops per view), on
    /// 1 thread then powers of 2 up to `RUSTCRAFT_THREADS` or every logical core, all threads
    /// sharing the same counters as the server and client threads share the version data.
    /// `cargo test --release -p rc-sim bench_block_view -- --ignored --nocapture`.
    #[test]
    #[ignore = "measurement bench"]
    fn bench_block_view() {
        let Some(data) = rc_testdata::need(rc_testdata::version_data(), "26.3 data cache") else { return };
        let table = Arc::new(Table::new(&data));
        let data = Arc::new(data);
        let states = data.states.len() as u32;
        let cores = rc_data::env_str!("RUSTCRAFT_THREADS")
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, std::num::NonZero::get));
        let mut threads = 1;
        while threads <= cores {
            let borrowed = ns_per_view(threads, states, |s| View { table: &table, data: &data }.random_ticks(s));
            let counted = ns_per_view(threads, states, |s| {
                let (table, data) = (Arc::clone(&table), Arc::clone(&data));
                View { table: &table, data: &data }.random_ticks(s)
            });
            eprintln!(
                "bench_block_view {threads} threads: borrowed {borrowed:.2} ns, counted {counted:.2} ns per view"
            );
            threads *= 2;
        }
    }

    /// Prints the time per operation of `f` over the states in `order` (about 10⁶ operations
    /// per measure; 1 thread then powers of 2 up to `cores`), and the allocations per operation
    /// (`memstats` feature). Generic so that each operation is inlined in its own loop.
    fn measure_property_op(
        label: &str,
        cores: usize,
        order: &[u32],
        view: &View,
        f: impl Fn(&View, u32) -> u32 + Sync,
    ) {
        let rounds = 1_000_000_usize.div_ceil(order.len());
        let allocs = crate::bench_kit::allocations();
        let mut ops = 0;
        let mut threads = 1;
        while threads <= cores {
            ops += rounds * order.len() * threads;
            let start = Instant::now();
            std::thread::scope(|scope| {
                for _ in 0..threads {
                    scope.spawn(|| {
                        let mut acc = 0u32;
                        for _ in 0..rounds {
                            for &s in order {
                                acc = acc.wrapping_add(f(view, std::hint::black_box(s)));
                            }
                        }
                        std::hint::black_box(acc);
                    });
                }
            });
            let ns = start.elapsed().as_nanos() as f64 / (rounds * order.len()) as f64;
            eprintln!("[bench] property_paths {label} ns_per_op_{threads}_threads {ns:.2}");
            threads *= 2;
        }
        let per_op = (crate::bench_kit::allocations() - allocs) as f64 / ops as f64;
        eprintln!("[bench] property_paths {label} allocations_per_op {per_op:.3}");
    }

    /// Typed property reads and writes on each path (by name, by index, packed, both), states
    /// walked in order and in a fixed shuffled order (the world reads states in no order: the
    /// per-state tables then miss the cache); 1 thread then powers of 2 up to
    /// `RUSTCRAFT_THREADS` or every logical core. A measurement, not a proof (`prop_index_tests`).
    /// `cargo test --release -p rc-sim [--features memstats] bench_property_paths --
    /// --ignored --nocapture --test-threads=1`.
    #[test]
    #[ignore = "measurement bench"]
    fn bench_property_paths() {
        use super::PropSwitches;
        let Some(data) = rc_testdata::need(rc_testdata::version_data(), "26.3 data cache") else { return };
        let cores = rc_data::env_str!("RUSTCRAFT_THREADS")
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, std::num::NonZero::get));
        let in_order: Vec<u32> = (0..data.states.len() as u32).collect();
        let mut shuffled = in_order.clone();
        let mut seed = 0x9E37_79B9_7F4A_7C15_u64;
        for i in (1..shuffled.len()).rev() {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            shuffled.swap(i, (seed % (i as u64 + 1)) as usize);
        }
        let paths = [
            ("by_name", PropSwitches { by_index: false, packed_facts: false }),
            ("by_index", PropSwitches { by_index: true, packed_facts: false }),
            ("packed", PropSwitches { by_index: false, packed_facts: true }),
            ("both", PropSwitches { by_index: true, packed_facts: true }),
        ];
        for (path, switches) in paths {
            let table = Table::with_switches(&data, switches);
            let view = View { table: &table, data: &data };
            for (walk, order) in [("in_order", &in_order), ("shuffled", &shuffled)] {
                // One call per operation (no closure table): each loop is compiled for its own
                // operation, as at the server's call sites.
                let label = |op: &str| format!("{path}/{walk}/{op}");
                let v = &view;
                measure_property_op(&label("get_bool_powered"), cores, order, v, |v, s| {
                    u32::from(v.get_bool(s, PropKey::Powered))
                });
                measure_property_op(&label("facing"), cores, order, v, |v, s| v.facing(s) as u32);
                measure_property_op(&label("get_int_power"), cores, order, v, |v, s| {
                    v.get_int(s, PropKey::Power) as u32
                });
                measure_property_op(&label("set_int_level"), cores, order, v, |v, s| {
                    v.set_int(s, PropKey::Level, (s & 7) as i32)
                });
                measure_property_op(&label("set_bool_waterlogged"), cores, order, v, |v, s| {
                    v.set_bool(s, PropKey::Waterlogged, true)
                });
                measure_property_op(&label("get_half"), cores, order, v, |v, s| {
                    v.get(s, PropKey::Half).map_or(0, str::len) as u32
                });
            }
        }
    }
}
