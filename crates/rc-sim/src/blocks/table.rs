//! Block table for the simulation: behaviour family of each block (from its
//! identifier, as the game associates a class with each block), common identifiers, fast
//! access to the properties of a state, facts extracted from the game (`rc_data::sim_bits`).

use hashbrown::HashMap;

use rc_data::{VersionData, flags, sim_bits};

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
}

/// Property of a block: index into `VersionData::properties`, not into the identifiers.
#[derive(Debug, Clone, Copy)]
struct Prop {
    property: u16,
    count: u8,
    stride: u32,
}

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
    pub sand: u16,
    pub red_sand: u16,
    pub magma: u16,
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
    pub lightning_rod: u16,
    pub carved_pumpkin: u16,
}

pub(crate) struct Table {
    pub kind: Vec<Kind>,
    props: Vec<Vec<Prop>>,
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
        | "torchflower" | "open_eyeblossom" | "closed_eyeblossom" | "bush" | "firefly_bush" | "pink_petals"
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
        let props = d
            .blocks
            .iter()
            .map(|b| {
                let counts: Vec<u32> =
                    b.properties.iter().map(|&p| d.properties[p as usize].values.len() as u32).collect();
                b.properties
                    .iter()
                    .enumerate()
                    .map(|(i, &p)| Prop {
                        property: p,
                        count: counts[i] as u8,
                        stride: counts[i + 1..].iter().product(),
                    })
                    .collect()
            })
            .collect();
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
            sand: f("sand"),
            red_sand: f("red_sand"),
            magma: f("magma_block"),
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
            lightning_rod: f("lightning_rod"),
            carved_pumpkin: f("carved_pumpkin"),
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
        Table {
            weather,
            beam,
            kind,
            props,
            id,
            pair,
            fire,
            face_masks: super::fluid::face_masks(d),
            fluid_kind,
            fluid_mob,
            fluid_water_source,
            by_name,
        }
    }

    pub fn block_id(&self, name: &str) -> u16 {
        let n = name.strip_prefix("minecraft:").unwrap_or(name);
        self.by_name.get(n).copied().unwrap_or(u16::MAX)
    }
}

/// View of the table linked to the data: reading and writing the properties of a state.
pub(crate) struct View {
    pub t: std::sync::Arc<Table>,
    pub d: std::sync::Arc<VersionData>,
}

impl View {
    #[inline]
    pub fn block(&self, s: u32) -> u16 {
        self.d.states.get(s as usize).map_or(u16::MAX, |i| i.block)
    }

    #[inline]
    pub fn kind(&self, s: u32) -> Kind {
        self.t.kind.get(self.block(s) as usize).copied().unwrap_or(Kind::Other)
    }

    #[inline]
    pub fn info(&self, s: u32) -> rc_data::StateInfo {
        self.d.states.get(s as usize).copied().unwrap_or_default()
    }

    #[inline]
    pub fn sim(&self, s: u32) -> u16 {
        self.d.sim.get(s as usize).copied().unwrap_or(0)
    }

    pub fn default_state(&self, block: u16) -> u32 {
        self.d.blocks.get(block as usize).map_or(0, |b| b.default_state)
    }

    fn slot(&self, s: u32, name: &str) -> Option<(usize, Prop, u8)> {
        let info = self.d.states.get(s as usize)?;
        let props = self.t.props.get(info.block as usize)?;
        props.iter().enumerate().find_map(|(i, p)| {
            (self.d.properties[p.property as usize].name == name)
                .then(|| (i, *p, self.d.state_values[info.values as usize + i]))
        })
    }

    pub fn has(&self, s: u32, name: &str) -> bool {
        self.slot(s, name).is_some()
    }

    /// Properties of state `s`: name, possible values (game order) and rank of the current
    /// value, sorted by name like the game's state definition (G5-F10, debug stick).
    pub fn props_sorted(&self, s: u32) -> Vec<(&str, &[String], usize)> {
        let Some(info) = self.d.states.get(s as usize) else { return Vec::new() };
        let Some(props) = self.t.props.get(info.block as usize) else { return Vec::new() };
        let mut out: Vec<(&str, &[String], usize)> = props
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let d = &self.d.properties[p.property as usize];
                (d.name.as_str(), d.values.as_slice(), usize::from(self.d.state_values[info.values as usize + i]))
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(b.0));
        out
    }

    pub fn get(&self, s: u32, name: &str) -> Option<&str> {
        let (_, p, v) = self.slot(s, name)?;
        Some(self.d.properties[p.property as usize].values[v as usize].as_str())
    }

    pub fn get_bool(&self, s: u32, name: &str) -> bool {
        self.get(s, name) == Some("true")
    }

    pub fn get_int(&self, s: u32, name: &str) -> i32 {
        self.get(s, name).and_then(|v| v.parse().ok()).unwrap_or(0)
    }

    pub fn get_dir(&self, s: u32, name: &str) -> Option<Dir> {
        self.get(s, name).and_then(Dir::parse)
    }

    pub fn facing(&self, s: u32) -> Dir {
        self.get_dir(s, "facing").unwrap_or(Dir::North)
    }

    /// State with `name = value` (unchanged if the property or the value does not exist).
    pub fn set(&self, s: u32, name: &str, value: &str) -> u32 {
        let Some((_, p, v)) = self.slot(s, name) else { return s };
        let Some(nv) = self.d.properties[p.property as usize].values.iter().position(|x| x == value) else {
            return s;
        };
        let delta = (nv as i64 - i64::from(v)) * i64::from(p.stride);
        (i64::from(s) + delta) as u32
    }

    pub fn set_bool(&self, s: u32, name: &str, v: bool) -> u32 {
        self.set(s, name, if v { "true" } else { "false" })
    }

    pub fn set_int(&self, s: u32, name: &str, v: i32) -> u32 {
        self.set(s, name, &v.to_string())
    }

    pub fn set_dir(&self, s: u32, name: &str, d: Dir) -> u32 {
        self.set(s, name, d.name())
    }

    /// Next value of the property (cycle).
    pub fn cycle(&self, s: u32, name: &str) -> u32 {
        let Some((_, p, v)) = self.slot(s, name) else { return s };
        let nv = (u32::from(v) + 1) % u32::from(p.count);
        (i64::from(s) + (i64::from(nv) - i64::from(v)) * i64::from(p.stride)) as u32
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
        self.d.hardness.get(s as usize).copied().unwrap_or(0.0)
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
