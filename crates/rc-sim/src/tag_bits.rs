//! Block tags as bits (`RUSTCRAFT_OPT_TAG_BITS`), built with `Server::block_tags`
//! and rebuilt in the same place at each tag reload.
//!
//! - Tag read by its name (data, predicates, commands): a bit set per
//!   tag instead of a sorted list walked by binary search.
//! - Tag named in code ([`TagId`]): one bit word per block, without hashing the name.
//!
//! The list of `TagId`s is that of the literal-name calls in rc-sim; it is regenerated (and the
//! calls migrated) by `target/impl/tag_ids.py` of mission C6-M02.

use hashbrown::HashMap;

macro_rules! tag_ids {
    ($($v:ident = $n:literal,)*) => {
        /// Block tag named in code.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[allow(dead_code)] // complete list of named tags, not all sites are migrated
        pub(crate) enum TagId {
            $($v,)*
        }

        impl TagId {
            pub(crate) const ALL: &[TagId] = &[$(TagId::$v,)*];

            pub(crate) fn name(self) -> &'static str {
                match self {
                    $(TagId::$v => $n,)*
                }
            }
        }
    };
}

tag_ids! {
    // <tag-ids>
    AnimalsSpawnableOn = "minecraft:animals_spawnable_on",
    ArmadilloSpawnableOn = "minecraft:armadillo_spawnable_on",
    AxolotlsSpawnableOn = "minecraft:axolotls_spawnable_on",
    BatsSpawnableOn = "minecraft:bats_spawnable_on",
    Beds = "minecraft:beds",
    BeeAttractive = "minecraft:bee_attractive",
    BeeGrowables = "minecraft:bee_growables",
    Beehives = "minecraft:beehives",
    BlocksDolphinJump = "minecraft:blocks_dolphin_jump",
    BlocksLavaFireSpread = "minecraft:blocks_lava_fire_spread",
    BlocksWindChargeExplosions = "minecraft:blocks_wind_charge_explosions",
    CamelSandStepSoundBlocks = "minecraft:camel_sand_step_sound_blocks",
    CamelsSpawnableOn = "minecraft:camels_spawnable_on",
    Campfires = "minecraft:campfires",
    CannotSupportKelp = "minecraft:cannot_support_kelp",
    CannotSupportSeagrass = "minecraft:cannot_support_seagrass",
    CatsCanLieOn = "minecraft:cats_can_lie_on",
    CatsCanSitOn = "minecraft:cats_can_sit_on",
    Cauldrons = "minecraft:cauldrons",
    CausesSuffocation = "minecraft:causes_suffocation",
    Climbable = "minecraft:climbable",
    ConvertibleToMud = "minecraft:convertible_to_mud",
    CushionUsesCollisionShape = "minecraft:cushion_uses_collision_shape",
    DoesNotBlockHoppers = "minecraft:does_not_block_hoppers",
    Doors = "minecraft:doors",
    DragonImmune = "minecraft:dragon_immune",
    DragonTransparent = "minecraft:dragon_transparent",
    EdibleForSheep = "minecraft:edible_for_sheep",
    EnchantmentPowerProvider = "minecraft:enchantment_power_provider",
    EnchantmentPowerTransmitter = "minecraft:enchantment_power_transmitter",
    EndermanDoesNotTeleportTo = "minecraft:enderman_does_not_teleport_to",
    EndermanHoldable = "minecraft:enderman_holdable",
    EntitiesCanTeleportTo = "minecraft:entities_can_teleport_to",
    Fences = "minecraft:fences",
    Fire = "minecraft:fire",
    Flowers = "minecraft:flowers",
    FoxesSpawnableOn = "minecraft:foxes_spawnable_on",
    FrogPreferJumpTo = "minecraft:frog_prefer_jump_to",
    FrogsSpawnableOn = "minecraft:frogs_spawnable_on",
    GoatsSpawnableOn = "minecraft:goats_spawnable_on",
    GrowsCrops = "minecraft:grows_crops",
    GuardedByPiglins = "minecraft:guarded_by_piglins",
    HappyGhastAvoids = "minecraft:happy_ghast_avoids",
    InvalidSpawnInside = "minecraft:invalid_spawn_inside",
    Leaves = "minecraft:leaves",
    Logs = "minecraft:logs",
    MaintainsFarmland = "minecraft:maintains_farmland",
    MobInteractableDoors = "minecraft:mob_interactable_doors",
    MooshroomsSpawnableOn = "minecraft:mooshrooms_spawnable_on",
    NetherPortalFrame = "minecraft:nether_portal_frame",
    PaleOakLogs = "minecraft:pale_oak_logs",
    ParrotsSpawnableOn = "minecraft:parrots_spawnable_on",
    PolarBearsSpawnableOnAlternate = "minecraft:polar_bears_spawnable_on_alternate",
    Portals = "minecraft:portals",
    PreventMobSpawningInside = "minecraft:prevent_mob_spawning_inside",
    PreventsNearbyLeafDecay = "minecraft:prevents_nearby_leaf_decay",
    RabbitsSpawnableOn = "minecraft:rabbits_spawnable_on",
    Rails = "minecraft:rails",
    Sand = "minecraft:sand",
    ShulkerBoxes = "minecraft:shulker_boxes",
    ShulkerDoesNotTeleportTo = "minecraft:shulker_does_not_teleport_to",
    SnapsGoatHorn = "minecraft:snaps_goat_horn",
    SnifferDiggableBlock = "minecraft:sniffer_diggable_block",
    SnifferEggHatchBoost = "minecraft:sniffer_egg_hatch_boost",
    Snow = "minecraft:snow",
    SoulFireBaseBlocks = "minecraft:soul_fire_base_blocks",
    SpeedsUpZombieVillagerCuring = "minecraft:speeds_up_zombie_villager_curing",
    Speleothems = "minecraft:speleothems",
    StriderWarmBlocks = "minecraft:strider_warm_blocks",
    SupportsBamboo = "minecraft:supports_bamboo",
    SupportsBigDripleaf = "minecraft:supports_big_dripleaf",
    SupportsChorusFlower = "minecraft:supports_chorus_flower",
    SupportsChorusPlant = "minecraft:supports_chorus_plant",
    SupportsCocoa = "minecraft:supports_cocoa",
    SupportsCrops = "minecraft:supports_crops",
    SupportsFrogspawn = "minecraft:supports_frogspawn",
    SupportsStemFruit = "minecraft:supports_stem_fruit",
    SupportsSugarCaneAdjacently = "minecraft:supports_sugar_cane_adjacently",
    SuppressesBounce = "minecraft:suppresses_bounce",
    VibrationResonators = "minecraft:vibration_resonators",
    VillagerBabiesCanJumpOnBed = "minecraft:villager_babies_can_jump_on_bed",
    VillagersCanSleepOnBed = "minecraft:villagers_can_sleep_on_bed",
    WallPostOverride = "minecraft:wall_post_override",
    Walls = "minecraft:walls",
    WashedAwayByFluids = "minecraft:washed_away_by_fluids",
    WitherImmune = "minecraft:wither_immune",
    WitherSummonBaseBlocks = "minecraft:wither_summon_base_blocks",
    WolvesSpawnableOn = "minecraft:wolves_spawnable_on",
    WoodenFences = "minecraft:wooden_fences",
    // </tag-ids>
}

const _: () = assert!(TagId::ALL.len() <= 128, "one 128-bit word per block");

/// `RUSTCRAFT_OPT_TAG_BITS`, read once: 0 old path (sorted list searched by name,
/// `TagId` read by name); 1 bit sets by name, `TagId` read by name; 2 (default): 1,
/// plus the bit word per block for the `TagId`s.
pub(crate) fn mode() -> u8 {
    static M: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *M.get_or_init(|| match rc_data::env_str!("RUSTCRAFT_OPT_TAG_BITS").map(str::trim) {
        Some("0") => 0,
        Some("1") => 1,
        _ => 2,
    })
}

#[derive(Default)]
pub(crate) struct TagBits {
    /// Per tag: one bit per block of index below `blocks`.
    sets: HashMap<String, Box<[u64]>>,
    /// Per block: bit `TagId as usize` set if the block carries the tag.
    named: Box<[u128]>,
    blocks: usize,
    /// Full block name → index (the first block of that name, like a walk of the list).
    by_name: HashMap<String, u16>,
}

impl TagBits {
    pub(crate) fn new(tags: &HashMap<String, Vec<u32>>, d: &rc_data::VersionData) -> TagBits {
        let blocks = d.blocks.len();
        let mut by_name = HashMap::with_capacity(blocks);
        for (i, b) in d.blocks.iter().enumerate() {
            by_name.entry(b.name.clone()).or_insert(i as u16);
        }
        let words = blocks.div_ceil(64);
        let sets = tags
            .iter()
            .map(|(n, ids)| {
                let mut b = vec![0u64; words].into_boxed_slice();
                for &i in ids.iter().filter(|&&i| (i as usize) < blocks) {
                    b[i as usize / 64] |= 1 << (i % 64);
                }
                (n.clone(), b)
            })
            .collect();
        let mut named = vec![0u128; blocks].into_boxed_slice();
        for (k, t) in TagId::ALL.iter().enumerate() {
            for &i in tags.get(t.name()).into_iter().flatten() {
                if let Some(w) = named.get_mut(i as usize) {
                    *w |= 1 << k;
                }
            }
        }
        TagBits { sets, named, blocks, by_name }
    }

    /// Index of the block with full name `name` (`minecraft:` included).
    pub(crate) fn block_of_name(&self, name: &str) -> Option<u16> {
        self.by_name.get(name).copied()
    }

    /// Does the block with full name `name` carry the tag `tag` (loot table conditions):
    /// same answer as looking up the name in the block list followed by the tag's sorted list
    /// (`RUSTCRAFT_OPT_BLOCKIDS`). Captures only this table: usable in the
    /// closures that borrow the rest of the server.
    #[allow(dead_code)] // called by the closures of loot tables migrated by their missions
    pub(crate) fn name_in_tag(&self, tag: &str, name: &str) -> bool {
        self.by_name.get(name).is_some_and(|&b| self.sets.get(tag).is_some_and(|s| s[b as usize / 64] >> (b % 64) & 1 != 0))
    }

    /// The block carries the tag `tag`; `None` outside the table (unknown block: the caller keeps
    /// the sorted list, which alone can contain such an identifier).
    pub(crate) fn has(&self, tag: &str, block: u32) -> Option<bool> {
        if block as usize >= self.blocks {
            return None;
        }
        Some(self.sets.get(tag).is_some_and(|b| b[block as usize / 64] >> (block % 64) & 1 != 0))
    }

    /// The block carries the named tag `t`; `None` outside the table.
    pub(crate) fn has_id(&self, t: TagId, block: u32) -> Option<bool> {
        self.named.get(block as usize).map(|w| w >> t as u32 & 1 != 0)
    }
}
