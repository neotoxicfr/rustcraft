//! Spawning and despawning (`server-mobs.md` § 2): natural cycle (census,
//! caps, shuffled chunks, groups), position validation and spawn rules,
//! finalization (equipment, babies, variants), spawn eggs, despawning.

use rc_data::{PropKey, flags};
use rc_proto::nbt::Nbt;

use super::env::Fluid;
use super::jrand::JRand;
use super::mob::{Ent, Mob};
use super::mth::dist2;
use super::species::{Category, Kind};
use super::structure_spawn::DecodedStarts;
use crate::player::Stack;
use crate::server::Server;

/// Simulation levels of a chunk (lower is stronger): 31 or less "ticking entities" (entities,
/// random ticks, thunder), 32 "ticking blocks" (scheduled ticks, block events, block entities).
pub(crate) const ENTITY_TICKING: i32 = 31;
pub(crate) const BLOCK_TICKING: i32 = 32;
/// Ticket level of a loaded ("full") chunk.
pub(crate) const FULL: i32 = 33;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Natural,
    SpawnEgg,
    /// Conversion (villager → zombie villager); F01.
    Conversion,
    /// F03: sulfur cube bucket.
    Bucket,
    /// F05: birth (villager), event (wandering trader).
    Breeding,
    Event,
    /// F07: patrol, summoning (vex).
    Patrol,
    /// Mob placed by the generation of a structure (mansion) and finalized on load.
    Structure,
    MobSummoned,
    /// Reinforcement called by a hurt zombie (Hard).
    Reinforcement,
    /// F12: mount or rider placed at spawn (baby strider, zombified piglin).
    Jockey,
    /// F14: spawner (creaking heart); triggered (sculk shrieker).
    Spawner,
    Triggered,
    /// Trial spawner (light ignored, support block not required).
    TrialSpawner,
}

/// Data shared by a spawn group.
#[derive(Debug, Clone, Default)]
pub struct Group {
    /// Animals: group size (the first is never a baby).
    pub size: i32,
    pub ageable: bool,
    /// Zombies: group baby (drawn for the first).
    pub zombie_baby: Option<bool>,
    /// Zombies: group data without chicken jockey (strider rider).
    pub zombie_no_jockey: bool,
    pub wolf_variant: Option<i32>,
    pub spider_done: bool,
    /// Group effect of the spiders (Hard), set on each one.
    pub spider_effect: Option<&'static str>,
    /// F02: shared variant (rabbit), baby probability (1 for rabbit, ocelot, bear),
    /// never a baby (parrot).
    pub surf_variant: Option<i32>,
    pub baby_chance: Option<f32>,
    pub no_baby: bool,
    /// F04: horse coat or llama variant shared by the group.
    pub mount_variant: Option<i32>,
    /// F09: school leader (id) and shared variant (tropical fish).
    pub school_leader: Option<i32>,
    pub tropical_variant: Option<i32>,
    /// F10: two axolotl variants drawn for the group.
    pub axolotl: Option<[i32; 2]>,
}

const MOON: [f32; 8] = [1.0, 0.75, 0.5, 0.25, 0.0, 0.25, 0.5, 0.75];

/// Natural spawn census of a tick (F3 "SC" line): dimension, spawn chunk count, mobs per category
/// (`Category::SPAWNING` order) once the tick's groups are placed.
#[derive(Debug, Clone, Default)]
pub struct Census {
    pub dim: String,
    pub chunks: i32,
    pub counts: [i32; 7],
}

/// Spawner carrier: block entity, or spawner minecart (G5-F06).
pub(crate) enum SpawnerAt<'a> {
    Block([i32; 3]),
    Cart(&'a mut crate::minecart::Cart),
}

/// Optimised paths of the spawner blocks and the natural spawns (`RUSTCRAFT_OPT_*`, on by
/// default, `=0` for the former path, read once); a test picks its own per thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SpawnPaths {
    /// Trial spawner: its block entity rewritten only when the write changes it (the chunk is no
    /// longer marked for saving on every tick).
    pub trial_write_on_change: bool,
    /// Trial spawner: block entity read in place instead of copied, configuration shared with
    /// the pack, state read from the block property without a string, block entity found once.
    pub trial_borrow: bool,
    /// Vault: a paused vault that shows nothing new leaves its tick before decoding anything.
    pub vault_early_exit: bool,
    /// Vault: its data decoded from the block entity in place instead of from a copy (tick and
    /// item use).
    pub vault_borrow: bool,
    /// Monster spawner: player range read on the block entity before copying it.
    pub spawner_range_first: bool,
    /// Monster spawner: the countdown between two spawns is written in place, without a copy.
    /// It needs the range test first, so it runs that test even with `spawner_range_first` off:
    /// measuring the former range order takes both switches off.
    pub spawner_borrow: bool,
    /// Natural spawns: spawn chunks counted without sets, spawn costs memoized per biome, charges
    /// gathered only when a category may spawn.
    pub spawn_census: bool,
    /// Natural spawns: structure starts decoded once per pass, pieces grouped by chunk column,
    /// spawn list not read twice at the drawing position.
    pub structure_starts: bool,
    /// Natural spawns: a position's placement tested before its spawn list (the cheaper test
    /// first once the placement test no longer copies the block name: most positions fail it,
    /// and the list is then never read).
    pub placement_first: bool,
}

#[cfg(test)]
thread_local! {
    /// Tests: paths used by this thread (equivalence proofs), without touching the environment.
    pub(crate) static FORCED_SPAWN_PATHS: std::cell::Cell<Option<SpawnPaths>> = const { std::cell::Cell::new(None) };
}

impl SpawnPaths {
    pub(crate) fn current() -> SpawnPaths {
        #[cfg(test)]
        if let Some(paths) = FORCED_SPAWN_PATHS.with(std::cell::Cell::get) {
            return paths;
        }
        SpawnPaths {
            trial_write_on_change: rc_data::opt_switch!("RUSTCRAFT_OPT_TRIAL_WRITE_ON_CHANGE"),
            trial_borrow: rc_data::opt_switch!("RUSTCRAFT_OPT_TRIAL_BORROW"),
            vault_early_exit: rc_data::opt_switch!("RUSTCRAFT_OPT_VAULT_EARLY_EXIT"),
            vault_borrow: rc_data::opt_switch!("RUSTCRAFT_OPT_VAULT_BORROW"),
            spawner_range_first: rc_data::opt_switch!("RUSTCRAFT_OPT_SPAWNER_RANGE_FIRST"),
            spawner_borrow: rc_data::opt_switch!("RUSTCRAFT_OPT_SPAWNER_BORROW"),
            spawn_census: rc_data::opt_switch!("RUSTCRAFT_OPT_SPAWN_CENSUS"),
            structure_starts: rc_data::opt_switch!("RUSTCRAFT_OPT_STRUCTURE_STARTS"),
            placement_first: rc_data::opt_switch!("RUSTCRAFT_OPT_SPAWN_PLACEMENT_FIRST"),
        }
    }

    /// The default paths: every new one.
    #[cfg(test)]
    pub(crate) const NEW: SpawnPaths = SpawnPaths {
        trial_write_on_change: true,
        trial_borrow: true,
        vault_early_exit: true,
        vault_borrow: true,
        spawner_range_first: true,
        spawner_borrow: true,
        spawn_census: true,
        structure_starts: true,
        placement_first: true,
    };

    /// Every former path.
    #[cfg(test)]
    pub(crate) const OLD: SpawnPaths = SpawnPaths {
        trial_write_on_change: false,
        trial_borrow: false,
        vault_early_exit: false,
        vault_borrow: false,
        spawner_range_first: false,
        spawner_borrow: false,
        spawn_census: false,
        structure_starts: false,
        placement_first: false,
    };

    /// Runs `f` with these paths on the current thread.
    #[cfg(test)]
    pub(crate) fn run<R>(self, f: impl FnOnce() -> R) -> R {
        let before = FORCED_SPAWN_PATHS.with(|c| c.replace(Some(self)));
        let out = f();
        FORCED_SPAWN_PATHS.with(|c| c.set(before));
        out
    }
}

/// State of the special "cats" and "zombie siege" spawners (not saved, like the
/// game: it restarts from zero on load).
#[derive(Debug, Clone, Default)]
pub struct CustomSpawners {
    /// Ticks before the next cat attempt (1,200 between two).
    pub cat_next: i32,
    /// Siege: planned tonight, set up, zombies remaining, delay, spawn center.
    pub siege_tonight: bool,
    pub siege_setup: bool,
    pub siege_left: i32,
    pub siege_next: i32,
    pub siege_at: [i32; 3],
    /// Spawner blocks (cages, trials, vaults) and ominous spawners.
    pub blocks: super::trial::SpawnerBlocks,
    /// Reason of the spawn rules being evaluated (spawners).
    pub rule_reason: Option<Reason>,
}

impl Server {
    /// Local difficulty (effective, "special" multiplier).
    pub(crate) fn local_difficulty(&self, p: [i32; 3]) -> (f32, f32) {
        let d = self.level.difficulty_id();
        if d == 0 {
            return (0.0, 0.0);
        }
        let hard = d == 3;
        // Total time: overworld clock (not game time); absent chunk: neither inhabited time
        // nor moon.
        let total = self.overworld_clock() as f32;
        let chunk = self.chunks.get(&(p[0] >> 4, p[2] >> 4)).and_then(|c| c.as_ref());
        let inhabited = chunk.map_or(0, |c| c.inhabited_time) as f32;
        // Moon phase: overworld "moon" timeline; elsewhere full moon (default value).
        let moon = match chunk {
            None => 0.0,
            Some(_) if self.dim.name == "minecraft:overworld" => MOON[self.day_index().rem_euclid(8) as usize],
            Some(_) => MOON[0],
        };
        let mut scale = 0.75;
        let global = ((total - 72000.0) / 1_440_000.0).clamp(0.0, 1.0) * 0.25;
        scale += global;
        let mut local = 0.0;
        local += (inhabited / 3_600_000.0).clamp(0.0, 1.0) * if hard { 1.0 } else { 0.75 };
        local += (moon * 0.25).clamp(0.0, global);
        if d == 1 {
            local *= 0.5;
        }
        scale += local;
        let eff = d as f32 * scale;
        let special = if eff < 2.0 {
            0.0
        } else if eff > 4.0 {
            1.0
        } else {
            (eff - 2.0) / 2.0
        };
        (eff, special)
    }

    fn day_index(&self) -> i64 {
        self.overworld_clock().div_euclid(24000)
    }

    /// Overworld clock (accumulated ticks, night skips and `/time` included).
    pub(crate) fn overworld_clock(&self) -> i64 {
        self.clocks.iter().find(|(n, _)| n == "minecraft:overworld").map_or(self.time, |c| c.1)
    }

    // ---------- Creation ----------

    /// New mob (next id, own random source), position and yaw.
    pub(crate) fn new_mob(&mut self, kind: Kind, pos: [f64; 3], yaw: f32) -> Mob {
        self.next_entity += 1;
        let id = self.next_entity;
        let mut rng = super::jrand::JRand::from_time(i64::from(id));
        let uuid = (rng.next_long() as u128) << 64 | rng.next_long() as u64 as u128;
        // UUID version 4, IETF variant.
        let uuid = (uuid & !(0xF000u128 << 64) | (0x4000u128 << 64)) & !(0xC000_0000_0000_0000u128)
            | 0x8000_0000_0000_0000u128;
        let mut m = Mob::new(kind, id, uuid, pos, super::mth::wrap(yaw), rng);
        self.end_reserve_parts(kind); // F15: ids of the dragon's parts
        self.init_mob(&mut m);
        m
    }

    /// Navigation and goals by species (construction).
    pub(crate) fn init_mob(&self, m: &mut Mob) {
        m.define_data();
        super::brains::register_goals(self, m);
        super::surface::init_nav(m);
        super::mounts::init_nav(m);
        super::golems::init_nav(m);
        super::sealife::init(self, m); // F09: swimming, zombie nautilus variant
        super::brainy::init_server(self, m); // F10: navigation, frog variant
        super::wild::init_nav(m); // F11: bee flight, fox paths
        super::piglins::init_nav(m); // F13: piglin doors
        super::recent::init(m); // F14
        super::wither::init_nav(m); // F16: wither flight
        if m.kind.base() == Kind::Spider {
            m.nav.climber = true;
        }
        if m.kind == Kind::Villager {
            m.nav.can_open_doors = true;
            m.nav.can_float = true;
            m.nav.required_length = 48.0;
        }
        self.variant_init(m);
        self.npc_init(m); // F05
        let max = (m.follow_range().max(f64::from(m.nav.required_length)) * 16.0).floor() as i32;
        m.nav.max_visited = (m.attrs.follow.base * 16.0).floor() as i32;
        let _ = max;
    }

    /// Addition to the world.
    pub(crate) fn add_mob(&mut self, m: Mob) {
        self.mobs.push(Box::new(m));
    }

    /// Finalization (§ 2.7); world random source unless stated.
    pub(crate) fn finalize_spawn(&mut self, m: &mut Mob, reason: Reason, group: &mut Group) {
        let (_, special) = self.local_difficulty(m.block_pos());
        let diff = self.level.difficulty_id();
        let biome_p = m.block_pos();
        // Before the common spawn finalization: animal variants, sheep color.
        match m.kind {
            Kind::Cow | Kind::Pig | Kind::Chicken => {
                let reg = match m.kind {
                    Kind::Cow => "cow",
                    Kind::Pig => "pig",
                    _ => "chicken",
                };
                if let Some(v) = self.select_variant(reg, biome_p) {
                    m.variant = v;
                }
                let key = ["minecraft:", reg, "_sound_variant"];
                let n = rc_data::ids::with_key(&key, |k| self.regs.entries(k).len()) as i32;
                m.sound_variant = self.mobs.rng.next_int_bound(n.max(1));
            }
            Kind::Sheep => m.wool = self.sheep_color(biome_p),
            Kind::Villager => {
                if reason == Reason::Breeding {
                    m.villager[1] = 0;
                }
                // Biome type only if the data are not already fixed, then fixed.
                if !m.x.vdata_final {
                    m.villager[0] = self.villager_type(biome_p);
                    m.x.vdata_final = true;
                }
                group.no_baby = true;
            }
            Kind::WanderingTrader => group.no_baby = true,
            // Zombie villager: type by biome (before the zombie's finalization), same rule.
            Kind::ZombieVillager => {
                if !m.x.vdata_final {
                    let t = self.villager_type(biome_p);
                    self.set_villager_type(m, t);
                }
            }
            // Cave spider: finalizes nothing (not even the tracking bonus or the hand).
            Kind::CaveSpider => return,
            // Phantom: anchor 5 blocks above, size 0, before the common spawn finalization.
            Kind::Phantom => {
                let b = m.block_pos();
                m.x.anchor = Some([b[0], b[1] + 5, b[2]]);
                m.x.size = 0;
            }
            Kind::Wolf => {
                match group.wolf_variant {
                    Some(v) => m.variant = v,
                    None => {
                        if let Some(v) = self.select_variant("wolf", biome_p) {
                            m.variant = v;
                            group.wolf_variant = Some(v);
                        }
                    }
                }
                let n = self.regs.entries("minecraft:wolf_sound_variant").len() as i32;
                m.sound_variant = self.mobs.rng.next_int_bound(n.max(1));
            }
            _ => {}
        }
        if super::surface::handles(m.kind) {
            super::surface::finalize_spawn(self, m, group);
        }
        // F09: salmon size, dolphin air, nautilus memories and variant.
        if super::sealife::handles(m.kind) {
            super::sealife::finalize_pre(self, m, group);
        }
        // F10: goat, frog, axolotl (bucket: finalization interrupted).
        if super::brainy::handles(m.kind) && super::brainy::finalize_pre(self, m, reason, group) {
            return;
        }
        // F11: fox variant and item, panda genes.
        super::wild::finalize_spawn(self, m, group);
        // F12: strider mounts, own group.
        super::nether::finalize_pre(self, m, group);
        // F13: babies, gold weapons and armor, memories, brute's home.
        super::piglins::finalize_pre(self, m, reason);
        // F14: warden (digging cooldown, emergence).
        super::recent::finalize_pre(self, m, reason);
        // F15: the shulker looks straight ahead (yaw 0).
        if m.kind == Kind::Shulker {
            m.yaw = 0.0;
            m.head_yaw = 0.0;
        }
        self.mount_finalize(m, reason, group); // F04
        self.illager_finalize_pre(m, reason); // F07: crossbow, sword, bow, patrol
        // F06: the copper golem plays its spawn sound before the common finalization.
        if m.kind == Kind::CopperGolem {
            self.mob_sound(m, "minecraft:entity.copper_golem.spawn", 1.0, 1.0);
        }
        // Babies (5%, never the first of the group).
        if m.kind.ageable() {
            if !group.ageable {
                group.ageable = true;
                group.size = 0;
            }
            let chance = group.baby_chance.unwrap_or(0.05);
            if !group.no_baby && group.size > 0 && self.mobs.rng.next_float() <= chance {
                m.age = super::brainy::baby_start_age(m.kind);
                super::piglins::age_boundary(m); // F13: baby hoglin damage
            }
            group.size += 1;
        }
        // Tracking bonus, left-handed.
        let bonus = self.mobs.rng.triangle(0.0, 0.114_850_000_000_000_01);
        m.attrs.follow.set_perm_mod("minecraft:random_spawn_bonus", bonus, 1);
        m.left_handed = self.mobs.rng.next_float() < 0.05;
        let conversion = reason == Reason::Conversion;
        if conversion {
            // Group imposed by the conversion: never a baby, mount possible.
            group.zombie_baby.get_or_insert(false);
        }
        match m.kind {
            Kind::Zombie | Kind::Husk | Kind::ZombieVillager | Kind::Drowned | Kind::ZombifiedPiglin => {
                if !conversion {
                    m.can_pickup = self.mobs.rng.next_float() < 0.55 * special;
                }
                let baby = *group.zombie_baby.get_or_insert_with(|| self.mobs.rng.next_float() < 0.05);
                if baby {
                    m.baby = true;
                    m.attrs.speed.set_mod("minecraft:baby", 0.5, 1);
                    if !group.zombie_no_jockey {
                        self.zombie_chicken_jockey(m);
                    }
                }
                let doors = self.mobs.rng.next_float() < special * 0.1;
                super::brains::set_can_break_doors(m, doors);
                if !conversion && m.kind == Kind::ZombifiedPiglin {
                    // F13: gold sword or spear (replaces the zombie's equipment).
                    super::piglins::zombified::default_equipment(self, m);
                    self.enchant_equipment(m, special);
                } else if !conversion && m.kind == Kind::Drowned {
                    // F08: drowned's own equipment (replaces the zombie's).
                    self.drowned_equipment(m);
                    self.enchant_equipment(m, special);
                } else if !conversion {
                    self.default_equipment(m, special, diff);
                    let zr = self.mobs.rng.next_float() < if diff == 3 { 0.05 } else { 0.01 };
                    if zr {
                        let r = self.mobs.rng.next_int_bound(6);
                        let w = match r {
                            0 => "minecraft:iron_sword",
                            1 => "minecraft:iron_spear",
                            _ => "minecraft:iron_shovel",
                        };
                        m.equipment[0] = Some(crate::player::Stack { id: w.into(), count: 1, components: None });
                    }
                    self.enchant_equipment(m, special);
                }
                self.halloween_head(m);
                // Setting up the attributes (entity's random source); F13: the zombified piglin has a base
                // reinforcement chance of zero, without a draw.
                super::variants::zombie_attributes(m, special, conversion);
                if m.kind == Kind::Husk {
                    self.finalize_husk(m, reason, special);
                }
                if m.kind == Kind::Drowned {
                    self.drowned_finalize(m, reason);
                }
            }
            Kind::Skeleton | Kind::Stray | Kind::Bogged | Kind::Parched => {
                self.default_equipment(m, special, diff);
                m.equipment[0] = Some(crate::player::Stack { id: "minecraft:bow".into(), count: 1, components: None });
                self.enchant_equipment(m, special);
                super::brains::reassess_weapon(self, m);
                m.can_pickup = self.mobs.rng.next_float() < 0.55 * special;
                self.halloween_head(m);
            }
            Kind::Spider => {
                if self.mobs.rng.next_int_bound(100) == 0 {
                    self.spider_skeleton_jockey(m, reason);
                }
                // First of the group: effect drawn on Hard, then set (infinite duration) on
                // each spider of the group.
                if !group.spider_done {
                    group.spider_done = true;
                    if diff == 3 && self.mobs.rng.next_float() < 0.1 * special {
                        group.spider_effect = Some(match self.mobs.rng.next_int_bound(5) {
                            0 | 1 => "minecraft:speed",
                            2 => "minecraft:strength",
                            3 => "minecraft:regeneration",
                            _ => "minecraft:invisibility",
                        });
                    }
                }
                if let Some(e) = group.spider_effect {
                    self.give_mob_effect(m, e, -1, 0);
                }
            }
            Kind::Cat => {
                if let Some(v) = self.select_variant("cat", biome_p) {
                    m.variant = v;
                }
                let n = self.regs.entries("minecraft:cat_sound_variant").len() as i32;
                m.sound_variant = self.mobs.rng.next_int_bound(n.max(1));
            }
            Kind::Slime | Kind::MagmaCube | Kind::SulfurCube => self.cube_spawn_size(m, special),
            // F09: schools (group leader), tropical fish patterns.
            k if super::sealife::handles(k) => super::sealife::finalize_post(self, m, group),
            // F12: wither skeleton sword and damage.
            k if super::nether::handles(k) => super::nether::finalize_post(self, m, special),
            _ => {}
        }
        self.illager_finalize_post(m); // F07: vindicator's axe
    }

    /// On October 31 (local date), empty head: carved pumpkin or lantern (10%) one time
    /// in four, never dropped (skeletons and zombies; world random source).
    pub(crate) fn halloween_head(&mut self, m: &mut Mob) {
        if m.equipment[5].is_some() || !self.halloween() || self.mobs.rng.next_float() >= 0.25 {
            return;
        }
        let id = if self.mobs.rng.next_float() < 0.1 { "minecraft:jack_o_lantern" } else { "minecraft:carved_pumpkin" };
        m.equipment[5] = Some(crate::player::Stack { id: id.into(), count: 1, components: None });
        m.drop_chance[5] = 0.0;
    }

    /// Halloween day imposed at construction: "no" in the package's tests (exact fingerprints
    /// and random draws do not depend on the date, like `rng::test_seed`), otherwise
    /// `RUSTCRAFT_HALLOWEEN=0|1` (bench); `None`: local date, like the game.
    pub(crate) fn halloween_override() -> Option<bool> {
        if cfg!(test) {
            return Some(false);
        }
        rc_data::env_os!("RUSTCRAFT_HALLOWEEN").map(|v| v == "1")
    }

    /// Halloween day (October 31 only in game 26.3), system local date.
    fn halloween(&self) -> bool {
        self.mobs.halloween.unwrap_or_else(|| {
            let (_, mo, d, ..) = crate::storage::local_datetime(crate::progress::now_ms());
            mo == 10 && d == 31
        })
    }

    /// Default equipment of monsters.
    fn default_equipment(&mut self, m: &mut Mob, special: f32, diff: i32) {
        let r = &mut self.mobs.rng;
        if r.next_float() >= 0.15 * special {
            return;
        }
        let mut armor = r.next_int_bound(3);
        for _ in 1..=3 {
            if r.next_float() < 0.1087 {
                armor += 1;
            }
        }
        let partial = if diff == 3 { 0.1 } else { 0.25 };
        let mats = ["leather", "copper", "golden", "chainmail", "iron", "diamond"];
        // Fill order: head, chest, legs, feet (slots 5, 4, 3, 2).
        let mut first = true;
        for (slot, piece) in [(5usize, "helmet"), (4, "chestplate"), (3, "leggings"), (2, "boots")] {
            if !first && r.next_float() < partial {
                break;
            }
            first = false;
            if m.equipment[slot].is_none()
                && let Some(mat) = mats.get(armor as usize)
            {
                m.equipment[slot] =
                    Some(crate::player::Stack { id: format!("minecraft:{mat}_{piece}"), count: 1, components: None });
            }
        }
    }

    /// Enchantments of the equipment at spawn:
    /// weapon 25%, each armor piece (feet → head) 50%, × special multiplier; then
    /// provider `mob_spawn_equipment`: cost 5 + U[0, ⌊special × 17⌋], draw from the enchantment
    /// table (S7) among the tag `on_mob_spawn_equipment`. World random source.
    pub(crate) fn enchant_equipment(&mut self, m: &mut Mob, special: f32) {
        for (slot, chance) in [(0usize, 0.25f32), (2, 0.5), (3, 0.5), (4, 0.5), (5, 0.5)] {
            let Some(s) = m.equipment[slot].clone() else { continue };
            if self.mobs.rng.next_float() >= chance * special {
                continue;
            }
            let s = mob_spawn_enchant(&self.life.pack, &self.life.items, s, special, &mut self.mobs.rng);
            m.equipment[slot] = Some(s);
        }
    }

    /// Variant of a registry according to the spawn conditions (priorities, biome, moon).
    pub(crate) fn select_variant(&mut self, reg: &str, p: [i32; 3]) -> Option<i32> {
        let entries: Vec<String> =
            rc_data::ids::with_key(&["minecraft:", reg, "_variant"], |k| self.regs.entries(k).to_vec());
        let mut unpacked: Vec<(i32, i32, bool)> = Vec::new();
        let moon = MOON[(self.day_index() % 8) as usize];
        // Read once: every biome condition of every variant asks for the same block.
        let biome = self.biome_str(p);
        for (i, name) in entries.iter().enumerate() {
            let Some(v) = self.life.pack.get(&format!("{reg}_variant"), name) else { continue };
            for sel in v["spawn_conditions"].as_array().cloned().unwrap_or_default() {
                let prio = sel["priority"].as_i64().unwrap_or(0) as i32;
                let c = &sel["condition"];
                let ok = if c.is_null() {
                    true
                } else {
                    match c["type"].as_str().unwrap_or("") {
                        "minecraft:biome" => match &c["biomes"] {
                            serde_json::Value::String(s) => match s.strip_prefix('#') {
                                Some(t) => self.biome_tag(p, t),
                                None => biome == s,
                            },
                            serde_json::Value::Array(a) => a.iter().any(|b| b.as_str() == Some(biome)),
                            _ => false,
                        },
                        "minecraft:moon_brightness" => {
                            let r = &c["range"];
                            r["min"].as_f64().is_none_or(|x| f64::from(moon) >= x)
                                && r["max"].as_f64().is_none_or(|x| f64::from(moon) <= x)
                        }
                        // Position in a room of a structure of the set (witch
                        // hut: black cat).
                        "minecraft:structure" => self.p_structure_at(p, &c["structures"]),
                        _ => false,
                    }
                };
                unpacked.push((i as i32, prio, ok));
            }
        }
        unpacked.sort_by_key(|u| std::cmp::Reverse(u.1));
        let mut best = i32::MIN;
        let mut kept = Vec::new();
        for (i, prio, ok) in unpacked {
            if prio < best {
                continue;
            }
            if ok {
                best = prio;
                kept.push(i);
            }
        }
        if kept.is_empty() {
            return None;
        }
        let k = self.mobs.rng.next_int_bound(kept.len() as i32) as usize;
        Some(kept[k])
    }

    fn sheep_color(&mut self, p: [i32; 3]) -> u8 {
        // Colours (ids): white 0, light grey 8, grey 7, black 15, brown 12, pink 6.
        let (list, common): ([(u8, i32); 4], u8) = if self.biome_tag(p, "minecraft:spawns_warm_variant_farm_animals") {
            ([(7, 5), (8, 5), (0, 5), (15, 3)], 12)
        } else if self.biome_tag(p, "minecraft:spawns_cold_variant_farm_animals") {
            ([(8, 5), (7, 5), (0, 5), (12, 3)], 15)
        } else {
            ([(15, 5), (7, 5), (8, 5), (12, 3)], 0)
        };
        let r = &mut self.mobs.rng;
        let mut n = r.next_int_bound(100);
        for (c, w) in list {
            n -= w;
            if n < 0 {
                return c;
            }
        }
        if r.next_int_bound(500) < 499 { common } else { 6 }
    }

    pub(crate) fn villager_type(&self, p: [i32; 3]) -> i32 {
        let ty = villager_type_name(self.biome_str(p));
        self.data
            .registry("minecraft:villager_type")
            .and_then(|r| r.entries.iter().position(|e| e.trim_start_matches("minecraft:") == ty))
            .unwrap_or(0) as i32
    }

    // ---------- Eggs ----------
}

/// Villager type of a biome (the game's exact table); any other biome (stony peaks,
/// added biomes…): plains.
pub(crate) fn villager_type_name(biome: &str) -> &'static str {
    match biome {
        "minecraft:badlands" | "minecraft:desert" | "minecraft:eroded_badlands" | "minecraft:wooded_badlands" => {
            "desert"
        }
        "minecraft:bamboo_jungle" | "minecraft:jungle" | "minecraft:sparse_jungle" => "jungle",
        "minecraft:savanna_plateau" | "minecraft:savanna" | "minecraft:windswept_savanna" => "savanna",
        "minecraft:deep_frozen_ocean"
        | "minecraft:frozen_ocean"
        | "minecraft:frozen_river"
        | "minecraft:ice_spikes"
        | "minecraft:snowy_beach"
        | "minecraft:snowy_taiga"
        | "minecraft:snowy_plains"
        | "minecraft:grove"
        | "minecraft:snowy_slopes"
        | "minecraft:frozen_peaks"
        | "minecraft:jagged_peaks" => "snow",
        "minecraft:swamp" | "minecraft:mangrove_swamp" => "swamp",
        "minecraft:old_growth_spruce_taiga"
        | "minecraft:old_growth_pine_taiga"
        | "minecraft:windswept_gravelly_hills"
        | "minecraft:windswept_hills"
        | "minecraft:taiga"
        | "minecraft:windswept_forest" => "taiga",
        _ => "plains",
    }
}

impl Server {
    /// Spawn egg used on a block; true if a mob was born. The species is the one of the egg's
    /// entity data, whatever the item.
    pub(crate) fn use_spawn_egg(&mut self, egg: &Stack, clicked: [i32; 3], face: i32) -> bool {
        let Some(kind) = self.egg_kind(egg) else { return false };
        if kind.info().not_in_peaceful && self.level.difficulty_id() == 0 {
            return false;
        }
        if let Some(used) = self.spawn_egg_on_spawner(clicked, &self.egg_spawner_entity(egg, kind)) {
            return used;
        }
        let dirs = [[0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]];
        let spawn = if self.has_collision(clicked) {
            let d = dirs[face.clamp(0, 5) as usize];
            [clicked[0] + d[0], clicked[1] + d[1], clicked[2] + d[2]]
        } else {
            clicked
        };
        let more = spawn != clicked && face == 1;
        let y_off = self.spawn_y_offset(spawn, more, kind);
        let pos = [f64::from(spawn[0]) + 0.5, f64::from(spawn[1]) + y_off, f64::from(spawn[2]) + 0.5];
        self.spawn_mob_from_stack(kind, pos, egg);
        self.game_event_block("minecraft:entity_place", spawn, Some(Ent::Player), None);
        true
    }

    /// Entity a spawn egg sets on a spawner block: its species alone, or the egg's whole entity
    /// data when an operator uses it.
    fn egg_spawner_entity(&self, egg: &Stack, kind: Kind) -> Nbt {
        let mut entity = match crate::items::component(egg, "minecraft:entity_data") {
            Some(data) if self.cmd.op > 0 => data.clone(),
            _ => Nbt::Compound(Vec::new()),
        };
        crate::nbtio::set(&mut entity, "id", Nbt::String(kind.info().name.to_owned()));
        entity
    }

    /// Mob placed by an item stack (spawn egg on a block, in a liquid, from a dispenser), in the
    /// game's order: random yaw, spawn finalised, what the stack hands over, ambient sound.
    pub(crate) fn spawn_mob_from_stack(&mut self, kind: Kind, pos: [f64; 3], stack: &Stack) {
        let yaw = super::mth::wrap(self.mobs.rng.next_float() * 360.0);
        let mut m = self.new_mob(kind, pos, yaw);
        m.head_yaw = m.yaw;
        m.body_yaw = m.yaw;
        let mut g = Group::default();
        self.finalize_spawn(&mut m, Reason::SpawnEgg, &mut g);
        self.apply_stack_to_placed_mob(&mut m, stack);
        if let Some(a) = kind.info().ambient {
            let a = a.to_owned();
            self.make_sound(&mut m, &a);
        }
        self.add_mob(m);
    }

    /// What a stack hands to the mob it placed, once the spawn is finalised: its custom name and
    /// custom data, then the fields of its entity data (other than the type) read over the mob's
    /// own save, as `/data merge` would; the mob keeps its UUID.
    pub(crate) fn apply_stack_to_placed_mob(&mut self, m: &mut Mob, stack: &Stack) {
        Self::apply_stack_name_and_data(m, stack);
        let Some(Nbt::Compound(fields)) = crate::items::component(stack, "minecraft:entity_data") else { return };
        // The type alone (every plain egg): reading the mob's own save back would change nothing.
        if fields.iter().all(|(key, _)| key == "id") {
            return;
        }
        let extra = Nbt::Compound(fields.iter().filter(|(key, _)| key != "id").cloned().collect());
        let mut merged = self.mob_nbt(m);
        crate::commands::cmds_data::merge(&mut merged, &extra);
        let uuid = m.uuid;
        self.load_mob(m, &merged);
        self.load_portal_cooldown(m.id, &merged);
        m.uuid = uuid;
        m.sync_data();
    }

    /// Custom name and custom data of a stack, copied to the mob it placed or bred.
    pub(crate) fn apply_stack_name_and_data(m: &mut Mob, stack: &Stack) {
        if let Some(name) = crate::items::component(stack, "minecraft:custom_name") {
            m.custom_name = Some(name.clone());
        }
        if let Some(data) = crate::items::component(stack, "minecraft:custom_data") {
            let kept = m.nbt.get_or_insert_with(|| Nbt::Compound(Vec::new()));
            crate::nbtio::set(kept, "data", data.clone());
        }
    }

    /// Vertical offset.
    pub(crate) fn spawn_y_offset(&self, p: [i32; 3], more: bool, kind: Kind) -> f64 {
        let i = kind.info();
        self.box_y_offset(p, more, i.width, i.height)
    }

    /// The vertical offset of an entity type for an entity of `width × height` (mob or decoration).
    pub(crate) fn box_y_offset(&self, p: [i32; 3], more: bool, width: f32, height: f32) -> f64 {
        let mut b = rc_physics::Aabb {
            min: [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])],
            max: [f64::from(p[0] + 1), f64::from(p[1] + 1), f64::from(p[2] + 1)],
        };
        if more {
            b.min[1] -= 1.0;
        }
        let w = self.world();
        // Mob's box placed one block higher.
        let start = Mob::bbox_at([f64::from(p[0]) + 0.5, f64::from(p[1] + 1), f64::from(p[2]) + 0.5], width, height);
        let mut boxes = Vec::new();
        rc_physics::collide::collect(&w, &b, &mut boxes);
        let d = rc_physics::collide::clip_axis(&boxes, &start, 1, if more { -2.0 } else { -1.0 });
        1.0 + d
    }

    // ---------- Despawning ----------

    /// The despawn test (§ 2.9); true if the mob is removed.
    pub(crate) fn check_despawn(&self, m: &mut Mob) -> bool {
        // F15: the dragon is never removed.
        if super::end::never_despawn(m) {
            return false;
        }
        if self.level.difficulty_id() == 0 && m.kind.info().not_in_peaceful {
            return true;
        }
        // F16: the wither never despawns (inaction reset to zero).
        if let Some(r) = super::wither::check_despawn(m) {
            return r;
        }
        // The species' own persistence cuts the evaluation BEFORE the random draw.
        let persistent = m.persistent || m.requires_custom_persistence();
        let d = dist2(self.pos, m.pos());
        let cat = m.kind.info().category;
        let far = f64::from(cat.despawn_distance().pow(2));
        let near = f64::from(cat.no_despawn_distance().pow(2));
        let remove_far = self.remove_when_far_away(m);
        if !persistent && d > far && remove_far {
            return true;
        }
        if !persistent && m.no_action_time > 600 && m.rng.next_int_bound(800) == 0 && d > near && remove_far {
            return true;
        } else if d < near {
            m.no_action_time = 0;
        }
        false
    }

    fn remove_when_far_away(&self, m: &Mob) -> bool {
        // F07: patroller (128 blocks), raid raider (never).
        if let Some(r) = super::illagers::remove_when_far_away(m, dist2(self.pos, m.pos())) {
            return r;
        }
        if let Some(r) = super::surface::remove_when_far_away(m)
            .or_else(|| super::mounts::remove_when_far_away(m))
            .or_else(|| super::sealife::remove_when_far_away(m))
            .or_else(|| super::brainy::remove_when_far_away(m))
            .or_else(|| super::wild::remove_when_far_away(m))
            .or_else(|| super::nether::remove_when_far_away(m))
            .or_else(|| super::piglins::remove_when_far_away(m))
            .or_else(|| super::recent::remove_when_far_away(m))
            .or_else(|| super::end::remove_when_far_away(m))
        {
            return r;
        }
        match m.kind {
            Kind::Chicken => m.chicken_jockey,
            Kind::Pig | Kind::Cow | Kind::Sheep | Kind::Wolf => false,
            // F06 : golems.
            Kind::IronGolem | Kind::SnowGolem | Kind::CopperGolem => false,
            Kind::Cat => !m.tame && m.tick_count > 2400,
            Kind::Villager | Kind::WanderingTrader => false,
            Kind::Enderman => m.carried.is_none(),
            Kind::ZombieVillager => m.x.cure_time < 0 && m.x.villager_xp == 0 && m.custom_name.is_none(),
            Kind::SulfurCube => !m.has_body_item() && !m.cube.from_bucket && m.custom_name.is_none(),
            _ => m.custom_name.is_none(),
        }
    }

    // ---------- Natural cycle ----------

    /// Census, allowed categories, shuffled spawn chunks; for each, in
    /// the game's order: inhabited time, thunderstorm (within simulation range), groups (§ 2.2). The
    /// inhabited time and the thunderstorm run even without the `spawn_mobs` rule; a spectator
    /// retains no chunk.
    pub(crate) fn natural_spawn_tick(&mut self) {
        #[cfg(test)]
        crate::server::phase("spawns");
        let paths = SpawnPaths::current();
        let do_spawn = self.rule("minecraft:spawn_mobs", true);
        // Players (host and guests): position and spectator state. Spawn chunks
        // (the spawn chunk collection): within 8 of a player at most, loaded, and within 128 blocks
        // of a non-spectator player.
        let players = self.spawn_players();
        let host_active = self.game_mode != 3;
        let mut chunks = if paths.spawn_census {
            self.spawn_chunks_near(&players)
        } else {
            self.spawn_chunks_near_with_set(&players)
        };
        let near = chunk_within_spawn_distance;
        chunks.retain(|c| players.iter().any(|(pos, spectator)| !spectator && near(pos, *c)));
        // Game counter: chunks within 8 of a player at most, loaded or not (289 only).
        let centers = self.sim_centers();
        let spawnable =
            if paths.spawn_census { spawn_chunk_count(&centers) } else { spawn_chunk_count_with_set(&centers) };
        let slot = Category::slot;
        let mut player_caps = PlayerMobCaps::new(&players);
        let (counts, mut pot) = if paths.spawn_census {
            (self.mob_counts_for_caps(&centers, &mut player_caps), Vec::new())
        } else {
            // Former path: its own walk of the counted mobs.
            for m in self.mobs_under_caps(&centers) {
                player_caps.add_mob(chunk_of_block(m.block_pos()), m.kind.info().category);
            }
            self.spawn_state_counts()
        };
        let mut pass = SpawnPass {
            paths,
            costs: SpawnCostMemo::default(),
            starts: DecodedStarts::default(),
            entry_drawn_at: None,
            counts,
            player_caps,
            world_spawn: self.world_spawn_in_this_dimension(),
        };
        let monsters_ok = self.rule("minecraft:spawn_monsters", true) && self.level.difficulty_id() != 0;
        let persistent_tick = self.time % 400 == 0;
        let mut cats: Vec<Category> = Vec::new();
        for c in Category::SPAWNING {
            let i = slot(c);
            if !host_active || !do_spawn || !(monsters_ok || c.friendly()) {
                continue;
            }
            if !(persistent_tick || !c.persistent()) {
                continue;
            }
            if pass.counts[i] >= c.max() * spawnable / 289 {
                continue;
            }
            cats.push(c);
        }
        // The charges are only read by spawn attempts, so not gathered when no category may
        // spawn; still gathered before the chunk loop, whose thunderstorms may add or convert
        // mobs before the first attempt.
        if paths.spawn_census && !cats.is_empty() {
            pot = self.spawn_charges(&centers, &mut pass.costs);
        }
        // Candidate chunks, shuffled; groups are only drawn around the host.
        let mut cand = chunks;
        self.mobs.rng.shuffle(&mut cand);
        for c in cand {
            if let Some(Some(ch)) = self.chunks.get_mut(&c) {
                ch.inhabited_time += 1;
            }
            if self.chunk_entity_ticking(c, &centers) {
                self.thunder_chunk_tick(c.0, c.1);
            }
            // A chunk that is not wholly inside the world border spawns nothing.
            if !host_active || !near(&self.pos, c) || !self.chunk_in_border(c) {
                continue;
            }
            for &cat in &cats {
                // The local cap: one of the players near this chunk still has room for the category.
                if pass.player_caps.can_spawn(cat, c) {
                    self.spawn_category_in_chunk(cat, c, &mut pot, &mut pass);
                }
            }
        }
        // Census kept for the debug screen (dimension name copied into the kept buffer).
        let census = self.mobs.census.get_or_insert_with(Census::default);
        census.dim.clone_from(&self.dim.name);
        (census.chunks, census.counts) = (spawnable, pass.counts);
    }

    /// Centre of the world spawn block when this dimension is the one players spawn in: natural
    /// spawns keep 24 blocks away from it.
    fn world_spawn_in_this_dimension(&self) -> Option<[f64; 3]> {
        let centre = self.level.spawn_pos.map(|v| f64::from(v) + 0.5);
        (self.level.spawn_dimension == self.dim.name).then_some(centre)
    }

    /// Both corner blocks of chunk `c` lie inside the world border.
    fn chunk_in_border(&self, c: (i32, i32)) -> bool {
        let border = self.border_bounds();
        let (x, z) = (c.0 * 16, c.1 * 16);
        Self::block_in_border(border, [x, 0, z]) && Self::block_in_border(border, [x + 15, 0, z + 15])
    }

    /// Spawn cost of the type in the biome (charge, energy budget), if it has one. The species'
    /// names carry their namespace: the cost key is the name itself.
    pub(super) fn spawn_cost(&self, kind: Kind, p: [i32; 3]) -> Option<(f64, f64)> {
        let def = self.life.pack.get("worldgen/biome", self.biome_str(p))?;
        let c =
            &def["attributes"]["minecraft:gameplay/natural_mob_spawns"]["argument"]["spawn_costs"][kind.info().name];
        Some((c["charge"].as_f64()?, c["energy_budget"].as_f64()?))
    }

    /// Spawn cost for a pass: memoized on the census path, read from the pack otherwise.
    fn spawn_cost_in_pass(&self, pass: &mut SpawnPass, kind: Kind, p: [i32; 3]) -> Option<(f64, f64)> {
        if pass.paths.spawn_census { self.spawn_cost_memo(&mut pass.costs, kind, p) } else { self.spawn_cost(kind, p) }
    }

    /// `spawn_cost`, read once per biome and species: the pack cannot change during a pass, and
    /// a pass meets a few biomes and species many times.
    pub(super) fn spawn_cost_memo(&self, memo: &mut SpawnCostMemo, kind: Kind, p: [i32; 3]) -> Option<(f64, f64)> {
        // Same biome index as `biome_str` reads.
        let biome = self.biome_at(p).unwrap_or(0);
        if let Some(known) = memo.known.iter().find(|e| e.biome == biome && e.kind == kind) {
            return known.cost;
        }
        let cost = self.spawn_cost(kind, p);
        memo.known.push(KnownCost { biome, kind, cost });
        cost
    }

    /// Does the potential of the charges already placed allow this type here?
    fn spawn_potential_ok(&self, pass: &mut SpawnPass, kind: Kind, p: [i32; 3], pot: &[([i32; 3], f64)]) -> bool {
        let Some((charge, budget)) = self.spawn_cost_in_pass(pass, kind, p) else { return true };
        potential_ok(charge, budget, p, pot)
    }

    fn spawn_category_in_chunk(
        &mut self,
        cat: Category,
        c: (i32, i32),
        pot: &mut Vec<([i32; 3], f64)>,
        pass: &mut SpawnPass,
    ) {
        let x = c.0 * 16 + self.mobs.rng.next_int_bound(16);
        let z = c.1 * 16 + self.mobs.rng.next_int_bound(16);
        let top = self.surface_y(x, z, 1) + 1;
        let min = self.dim.height.min_y;
        let y = self.mobs.rng.next_int_bound(top - min + 1) + min;
        if y < min + 1 {
            return;
        }
        let start = [x, y, z];
        if self.blocks_spawn_group_start(start) {
            return;
        }
        let mut cluster = 0;
        for _ in 0..3 {
            let (mut x, mut z) = (start[0], start[2]);
            let mut current: Option<(Kind, i32)> = None;
            let mut entry = serde_json::Value::Null;
            let mut known = true;
            let mut group = Group::default();
            let mut max = (self.mobs.rng.next_float() * 4.0).ceil() as i32;
            let mut group_size = 0;
            let mut ll = 0;
            while ll < max {
                ll += 1;
                x += self.mobs.rng.next_int_bound(6) - self.mobs.rng.next_int_bound(6);
                z += self.mobs.rng.next_int_bound(6) - self.mobs.rng.next_int_bound(6);
                let p = [x, y, z];
                let (xx, zz) = (f64::from(x) + 0.5, f64::from(z) + 0.5);
                let at = [xx, f64::from(y), zz];
                let d = dist2(self.pos, at);
                if d <= 576.0 || pass.world_spawn.is_some_and(|centre| dist2(centre, at) < 576.0) {
                    continue;
                }
                // Outside the group's own chunk: loaded and wholly inside the world border.
                let chunk = (x >> 4, z >> 4);
                if chunk != c && !(self.chunks.get(&chunk).is_some_and(Option::is_some) && self.chunk_in_border(chunk))
                {
                    continue;
                }
                if current.is_none() && known {
                    match self.pick_spawn_entry(cat, p, pass) {
                        None => break,
                        Some((name, count, e)) => {
                            max = count;
                            entry = e;
                            pass.entry_drawn_at = Some(p);
                            match Kind::from_name(&name) {
                                Some(k) => current = Some((k, count)),
                                None => {
                                    known = false;
                                }
                            }
                        }
                    }
                }
                let Some((kind, _)) = current else { continue };
                if !self.valid_spawn_position(kind, p, d, cat, &entry, pass)
                    || !self.spawn_potential_ok(pass, kind, p, pot)
                {
                    continue;
                }
                let yaw = self.mobs.rng.next_float() * 360.0;
                let mut m = self.new_mob(kind, at, yaw);
                if !self.valid_position_for_mob(&mut m, d) {
                    continue;
                }
                self.finalize_spawn(&mut m, Reason::Natural, &mut group);
                // Charge placed where the mob is (biome cost at its position).
                let bp = m.block_pos();
                if let Some((charge, _)) = self.spawn_cost_in_pass(pass, kind, bp)
                    && charge != 0.0
                {
                    pot.push((bp, charge));
                }
                // The new mob counts at once, under its own category, for the world and for the
                // players near its chunk.
                let category = kind.info().category;
                pass.counts[Category::slot(category)] += 1;
                pass.player_caps.add_mob(chunk_of_block(bp), category);
                cluster += 1;
                group_size += 1;
                // F09: complete group (tropical fish outside a school).
                let full = super::sealife::max_group_reached(&m, group_size);
                self.add_mob(m);
                let most = super::mounts::max_cluster(kind)
                    .or_else(|| super::sealife::max_cluster(kind))
                    .or_else(|| super::nether::max_cluster(kind))
                    .or_else(|| super::recent::max_cluster(kind))
                    .unwrap_or(match kind {
                        Kind::Wolf => 8,
                        Kind::Pillager => 1,
                        _ => 4,
                    });
                if cluster >= most {
                    return;
                }
                if full {
                    break;
                }
            }
        }
    }

    /// A spawn group is abandoned when its start block is a redstone conductor (the game's
    /// per-block flag, not the opacity: a redstone block or glowstone lets the group start).
    pub(super) fn blocks_spawn_group_start(&self, start: [i32; 3]) -> bool {
        self.view().conductor(self.st(start))
    }

    /// Entry drawn from the biome's list (name, group size).
    fn pick_spawn_entry(
        &mut self,
        cat: Category,
        p: [i32; 3],
        pass: &mut SpawnPass,
    ) -> Option<(String, i32, serde_json::Value)> {
        let key = spawn_key(cat)?;
        // F07: structures that replace the list (outpost, hut, fortress, monument), otherwise biome.
        // F09: rarefied aquatic ambience (98% refusal) in the biomes of the tag.
        if cat == Category::WaterAmbient
            && self.biome_tag(p, "minecraft:reduced_water_ambient_spawns")
            && self.mobs.rng.next_float() < 0.98
        {
            return None;
        }
        let list = self.spawn_list_in_pass(pass, cat, key, p);
        let total: i32 = list.iter().map(|e| e["weight"].as_i64().unwrap_or(0) as i32).sum();
        if total <= 0 {
            return None;
        }
        let mut r = self.mobs.rng.next_int_bound(total);
        let e = list.iter().find(|e| {
            r -= e["weight"].as_i64().unwrap_or(0) as i32;
            r < 0
        })?;
        let e = e.clone();
        let name = e["type"].as_str()?.to_owned();
        let count = match &e["count"] {
            serde_json::Value::Number(n) => n.as_i64().unwrap_or(1) as i32,
            c => {
                let lo = c["min_inclusive"].as_i64().unwrap_or(1) as i32;
                let hi = c["max_inclusive"].as_i64().unwrap_or(1) as i32;
                self.mobs.rng.next_int_bound(hi - lo + 1) + lo
            }
        };
        Some((name, count, e))
    }

    /// The drawn entry still appears in the position's list (type and
    /// count; the weight is not part of it).
    fn can_spawn_entry_at(&self, pass: &mut SpawnPass, cat: Category, entry: &serde_json::Value, p: [i32; 3]) -> bool {
        let Some(key) = spawn_key(cat) else { return false };
        self.spawn_list_in_pass(pass, cat, key, p)
            .iter()
            .any(|e| e["type"] == entry["type"] && e["count"] == entry["count"])
    }

    /// Spawn list at `p`, from the starts decoded for the pass (structure path).
    fn spawn_list_in_pass(
        &self,
        pass: &mut SpawnPass,
        cat: Category,
        key: &str,
        p: [i32; 3],
    ) -> std::sync::Arc<[serde_json::Value]> {
        if pass.paths.structure_starts {
            self.spawn_list_decoded(&mut pass.starts, cat, key, p)
        } else {
            self.spawn_list_at(cat, key, p)
        }
    }

    /// Distance, ground placement, spawn rule, free box.
    fn valid_spawn_position(
        &mut self,
        kind: Kind,
        p: [i32; 3],
        d: f64,
        list_cat: Category,
        entry: &serde_json::Value,
        pass: &mut SpawnPass,
    ) -> bool {
        let cat = kind.info().category;
        if cat == Category::Misc {
            return false;
        }
        // F07: only the pillager can spawn far from players (outposts).
        if kind != Kind::Pillager && d > f64::from(cat.despawn_distance().pow(2)) {
            return false;
        }
        // F07: the entry must appear in the list of the new position. Structure path: the list
        // of the position the entry was drawn from holds it (the lists cannot change during a
        // pass), not read again there.
        let drawn_here = pass.paths.structure_starts && pass.entry_drawn_at == Some(p);
        let placement_first = pass.paths.placement_first;
        let mut listed = |s: &Self| drawn_here || s.can_spawn_entry_at(pass, list_cat, entry, p);
        // Both tests only read the world (no random draw): their order does not change the answer.
        let placed_and_listed = if placement_first {
            let placed = self.spawn_placement_ok(kind, p);
            placed && listed(self)
        } else {
            let in_list = listed(self);
            in_list && self.spawn_placement_ok(kind, p)
        };
        if !placed_and_listed {
            return false;
        }
        if !self.spawn_rules(kind, p) {
            return false;
        }
        let i = kind.info();
        let pos = [f64::from(p[0]) + 0.5, f64::from(p[1]), f64::from(p[2]) + 0.5];
        let s = i.spawn_scale;
        rc_physics::is_free(&self.world(), &Mob::bbox_at(pos, i.width * s, i.height * s))
    }

    /// Spawn placement of the type; `None` outside "ground" placement.
    pub(crate) fn spawn_placement_ok(&self, kind: Kind, p: [i32; 3]) -> bool {
        // F11: fox and panda without placement restriction.
        if super::wild::no_placement_restriction(kind) {
            return true;
        }
        // Every restricted placement (water, lava, ground) refuses a block outside the world border.
        if !Self::block_in_border(self.border_bounds(), p) {
            return false;
        }
        // F08: placement in water (drowned, guardians).
        if super::aquatic::spawns_in_water(kind) {
            return self.in_water_placement_ok(p);
        }
        // F09: "in water" placement of the aquatic species; F10: axolotl; F12: placement
        // "in lava" of the strider.
        if let Some(ok) = super::nether::placement_ok(self, kind, p)
            .or_else(|| super::sealife::placement_ok(self, kind, p))
            .or_else(|| super::brainy::placement_ok(self, kind, p))
        {
            return ok;
        }
        self.valid_spawn_on([p[0], p[1] - 1, p[2]], kind)
            && self.valid_empty_spawn(p, kind)
            && self.valid_empty_spawn([p[0], p[1] + 1, p[2]], kind)
    }

    /// "Ground" placement (neither water, nor lava, nor unrestricted).
    fn placed_on_ground(&self, kind: Kind, p: [i32; 3]) -> bool {
        !super::aquatic::spawns_in_water(kind)
            && super::nether::placement_ok(self, kind, p)
                .or_else(|| super::sealife::placement_ok(self, kind, p))
                .or_else(|| super::brainy::placement_ok(self, kind, p))
                .is_none()
            && !super::wild::no_placement_restriction(kind)
    }

    /// Animals placed at the generation of a new chunk (§ 2.8):
    /// "creature" list and biome probability at the chunk's corner (at the world ceiling); chunk
    /// decoration random source for the draws, world random source for the spawn rules.
    pub(crate) fn spawn_original_mobs(&mut self, c: (i32, i32)) {
        let Some(seed) = self.generator.as_ref().filter(|g| g.mob_gen).map(|g| g.seed) else { return };
        if !self.rule("minecraft:spawn_mobs", true) {
            return;
        }
        let (xo, zo) = (c.0 * 16, c.1 * 16);
        let max_y = self.dim.height.min_y + self.dim.height.sections as i32 * 16 - 1;
        // Two environment attributes read at the same corner: the spawn list on the biome of the
        // block (through the zoom), the probability on the stored biome cell.
        let corner = [xo, max_y, zo];
        let spawn_biome = self.biome_for_generation_spawns(corner);
        let Some(def) = self.life.pack.get("worldgen/biome", self.biome_id_name(spawn_biome)) else { return };
        let spawns = &def["attributes"]["minecraft:gameplay/natural_mob_spawns"];
        let list = spawns["argument"]["spawns_by_category"]["creature"].as_array().cloned().unwrap_or_default();
        if list.is_empty() {
            return;
        }
        let prob = self.creature_generation_spawn_probability(corner);
        let total: i32 = list.iter().map(|e| e["weight"].as_i64().unwrap_or(0) as i32).sum();
        let mut g = JRand::new(seed);
        let (a, b) = (g.next_long() | 1, g.next_long() | 1);
        let mut g = JRand::new((i64::from(xo).wrapping_mul(a)).wrapping_add(i64::from(zo).wrapping_mul(b)) ^ seed);
        while g.next_float() < prob {
            if total <= 0 {
                continue;
            }
            let mut r = g.next_int_bound(total);
            let Some(e) = list.iter().find(|e| {
                r -= e["weight"].as_i64().unwrap_or(0) as i32;
                r < 0
            }) else {
                continue;
            };
            let count = match &e["count"] {
                serde_json::Value::Number(n) => n.as_i64().unwrap_or(1) as i32,
                cnt => {
                    let lo = cnt["min_inclusive"].as_i64().unwrap_or(1) as i32;
                    let hi = cnt["max_inclusive"].as_i64().unwrap_or(1) as i32;
                    g.next_int_bound(hi - lo + 1) + lo
                }
            };
            let kind = e["type"].as_str().and_then(Kind::from_name);
            let mut group = Group::default();
            let (mut x, mut z) = (xo + g.next_int_bound(16), zo + g.next_int_bound(16));
            let (sx, sz) = (x, z);
            for _ in 0..count {
                let mut success = false;
                let mut attempts = 0;
                while !success && attempts < 4 {
                    attempts += 1;
                    if let Some(kind) = kind {
                        let p = self.top_non_colliding(kind, x, z);
                        if self.spawn_placement_ok(kind, p) {
                            let i = kind.info();
                            let w = f64::from(i.width);
                            let fx = f64::from(x).clamp(f64::from(xo) + w, f64::from(xo) + 16.0 - w);
                            let fz = f64::from(z).clamp(f64::from(zo) + w, f64::from(zo) + 16.0 - w);
                            let at = [fx, f64::from(p[1]), fz];
                            let free = rc_physics::is_free(&self.world(), &Mob::bbox_at(at, i.width, i.height));
                            if !free || !self.spawn_rules(kind, [fx.floor() as i32, p[1], fz.floor() as i32]) {
                                // Next attempt without moving (like the game).
                                continue;
                            }
                            let yaw = g.next_float() * 360.0;
                            let mut m = self.new_mob(kind, at, yaw);
                            if self.valid_position_for_mob(&mut m, 0.0) {
                                self.finalize_spawn(&mut m, Reason::Natural, &mut group);
                                self.add_mob(m);
                                success = true;
                            }
                        }
                    }
                    x += g.next_int_bound(5) - g.next_int_bound(5);
                    z += g.next_int_bound(5) - g.next_int_bound(5);
                    while x < xo || x >= xo + 16 || z < zo || z >= zo + 16 {
                        x = sx + g.next_int_bound(5) - g.next_int_bound(5);
                        z = sz + g.next_int_bound(5) - g.next_int_bound(5);
                    }
                }
            }
        }
    }

    /// Chance of one more group of animals placed with a new chunk: an environment attribute,
    /// read on the stored biome cell of the chunk's corner block, without the block zoom.
    pub(super) fn creature_generation_spawn_probability(&self, corner: [i32; 3]) -> f32 {
        self.life
            .pack
            .get("worldgen/biome", self.stored_biome_str(corner))
            .and_then(|def| def["attributes"]["minecraft:gameplay/creature_world_gen_spawn_probability"].as_f64())
            .unwrap_or(0.1) as f32
    }

    /// Biome (registry index) of the corner block of a chunk just generated, read through the
    /// zoom for the animals placed with it. The zoom often picks a cell of a neighbour chunk that
    /// has not arrived yet: the game reads that neighbour's biomes (generated before the animals
    /// are placed), so the cell is asked to the world generator instead of falling back on the
    /// block's own cell, and the answer no longer depends on the order the chunks arrive in.
    pub(super) fn biome_for_generation_spawns(&mut self, corner: [i32; 3]) -> Option<usize> {
        let (qx, qy, qz) = rc_worldgen::biome::zoom(self.wgs.zoom_seed, corner[0], corner[1], corner[2]);
        let chosen_cell = [qx << 2, qy << 2, qz << 2];
        if let Some(stored) = self.stored_biome_at(chosen_cell) {
            return Some(stored);
        }
        // Height clamped to the world, like the read of a stored cell.
        let min_y = self.dim.height.min_y;
        let max_y = min_y + self.dim.height.sections as i32 * 16 - 1;
        let in_world = [chosen_cell[0], chosen_cell[1].clamp(min_y, max_y), chosen_cell[2]];
        let generated = self.generated_biomes(&[in_world]).pop().flatten();
        generated.or_else(|| self.stored_biome_at(corner))
    }

    /// Connected players: position and spectator state.
    pub(crate) fn spawn_players(&self) -> Vec<([f64; 3], bool)> {
        let mut out = vec![(self.pos, self.game_mode == 3)];
        if let Some(l) = &self.lan {
            for g in &l.guests {
                if let Some(c) = g.ctx.as_ref().filter(|_| g.joined && !g.gone) {
                    out.push((c.pos, c.game_mode == 3));
                }
            }
        }
        out
    }

    /// A player in this game mode holds the player tickets (loading and simulation): not a
    /// spectator under the rule `spectators_generate_chunks` false, around whom nothing loads
    /// and nothing is simulated (G3-F13).
    pub(crate) fn holds_tickets(&self, game_mode: i32) -> bool {
        game_mode != 3 || self.rule("minecraft:spectators_generate_chunks", true)
    }

    /// Center chunks of the simulation: those of the players (host and guests) holding tickets.
    pub(crate) fn sim_centers(&self) -> Vec<(i32, i32)> {
        let mut out = Vec::with_capacity(1);
        if self.holds_tickets(self.game_mode) {
            out.push(self.chunk_pos());
        }
        if let Some(l) = &self.lan {
            for g in &l.guests {
                if let Some(c) = g.ctx.as_ref().filter(|c| g.joined && !g.gone && self.holds_tickets(c.game_mode)) {
                    out.push(((c.pos[0].floor() as i32) >> 4, (c.pos[2].floor() as i32) >> 4));
                }
            }
        }
        out
    }

    /// "Ticking blocks" chunk (scheduled ticks, events and block entities): players'
    /// simulation ticket (level 31 − simulation distance) propagated up to level
    /// 32, i.e. simulation distance + 1 chunks (Chebyshev); a force-loaded chunk
    /// (level 31) also covers its neighbors.
    pub(crate) fn chunk_simulated(&self, c: (i32, i32), centers: &[(i32, i32)]) -> bool {
        let r = self.sim_dist + 1;
        self.forced_near(c, 1)
            || centers.iter().any(|p| (c.0 - p.0).abs() <= r && (c.1 - p.1).abs() <= r)
            || self.ticket_level(c) <= BLOCK_TICKING
    }

    /// Squares (centre chunk, Chebyshev radius) whose union holds every chunk `chunk_simulated`
    /// accepts, for listing candidates instead of testing every loaded chunk: the players'
    /// squares, one chunk around each forced chunk and each applied pearl ticket, the dragon
    /// fight's square around the origin. Keep it in step with `chunk_simulated`, which still
    /// judges every candidate.
    pub(crate) fn simulated_squares(&self, centers: &[(i32, i32)]) -> Vec<((i32, i32), i32)> {
        let mut squares: Vec<((i32, i32), i32)> = centers.iter().map(|&c| (c, self.sim_dist + 1)).collect();
        squares.extend(self.cmd.forced.iter().map(|&c| (c, 1)));
        // A ticket of level L at its own chunk weakens by one per chunk: it keeps the "ticking
        // blocks" level up to BLOCK_TICKING − L chunks away.
        let pearl = BLOCK_TICKING - crate::projectiles::PEARL_TICKET_LEVEL;
        squares.extend(self.proj.tickets.iter().filter(|t| t.live).map(|t| (t.chunk, pearl)));
        let fight = self.fight_ticket_level((0, 0));
        if fight <= BLOCK_TICKING {
            squares.push(((0, 0), BLOCK_TICKING - fight));
        }
        squares
    }

    /// "Ticking entities" chunk (level 31): random ticks, thunderstorm; simulation
    /// distance, or the force-loaded chunk itself.
    pub(crate) fn chunk_entity_ticking(&self, c: (i32, i32), centers: &[(i32, i32)]) -> bool {
        let r = self.sim_dist;
        self.cmd.forced.contains(&c)
            || centers.iter().any(|p| (c.0 - p.0).abs() <= r && (c.1 - p.1).abs() <= r)
            || self.ticket_level(c) <= ENTITY_TICKING
    }

    /// L9-TICK: lowest simulation level at chunk `c` of the tickets other than the players' and
    /// the forced ones (`i32::MAX` without any): the players' ender pearls and the dragon
    /// fight. 31 or less: "ticking entities"; 32: "ticking blocks".
    pub(crate) fn ticket_level(&self, c: (i32, i32)) -> i32 {
        let fight = self.fight_ticket_level(c);
        if self.proj.tickets.is_empty() { fight } else { fight.min(self.proj.pearl_ticket_level(c)) }
    }

    /// Ticked mob: "ticking entities" chunk on the simulation side AND the loading side
    /// (players' ticket over the view distance), hence min(view, simulation) around a player.
    pub(crate) fn mob_chunk_ticking(&self, c: (i32, i32)) -> bool {
        let r = self.view.min(self.sim_dist);
        let near = |p: (i32, i32)| (c.0 - p.0).abs() <= r && (c.1 - p.1).abs() <= r;
        if self.cmd.forced.contains(&c) || (near(self.chunk_pos()) && self.holds_tickets(self.game_mode)) {
            return true;
        }
        self.lan.as_ref().is_some_and(|l| {
            l.guests.iter().any(|g| {
                g.ctx
                    .as_ref()
                    .filter(|ctx| g.joined && !g.gone && self.holds_tickets(ctx.game_mode))
                    .is_some_and(|ctx| near(((ctx.pos[0].floor() as i32) >> 4, (ctx.pos[2].floor() as i32) >> 4)))
            })
        })
    }

    /// An entity other than a player at `pos` ticks this tick (§ 1.2 step 11b): its chunk is
    /// loaded and "ticking entities" (`mob_chunk_ticking`, or a pearl's or the dragon fight's
    /// ticket); in a parked dimension (portal ticket, no player there), a loaded chunk is enough.
    pub(crate) fn entity_ticks_at(&self, pos: [f64; 3]) -> bool {
        let c = ((pos[0].floor() as i32) >> 4, (pos[2].floor() as i32) >> 4);
        // Distance tests first (a few comparisons), the chunk table lookup last.
        (self.dims.home.is_some() || self.entities_tick_in(c)) && matches!(self.chunks.get(&c), Some(Some(_)))
    }

    /// Chunk `c` is "ticking entities" for an entity other than a player: around a player
    /// (`mob_chunk_ticking`) or under a pearl's or the dragon fight's ticket (L9-TICK).
    pub(crate) fn entities_tick_in(&self, c: (i32, i32)) -> bool {
        self.mob_chunk_ticking(c) || self.ticket_level(c) <= ENTITY_TICKING
    }

    /// A forced chunk lies within `r` chunks (Chebyshev) of `c`.
    fn forced_near(&self, c: (i32, i32), r: i32) -> bool {
        let forced = &self.cmd.forced;
        // Up to a few forced chunks, comparing them all costs less than searching the set.
        if forced.len() <= 8 || !crate::blocks::ticks::BlockTickPaths::current().simulated_candidates {
            return forced.iter().any(|f| (c.0 - f.0).abs() <= r && (c.1 - f.1).abs() <= r);
        }
        // The set is sorted by (x, z): one range search per column of the square.
        (c.0 - r..=c.0 + r).any(|x| forced.range((x, c.1 - r)..=(x, c.1 + r)).next().is_some())
    }

    /// Mob count for the caps: seven categories
    /// (game order) and spawn cost potential, over the non-persistent mobs of only the chunks
    /// with loaded entities (view + 2 around a player): the game unloads the
    /// others; here they stay in memory without counting.
    pub(crate) fn spawn_state_counts(&self) -> ([i32; 7], Vec<([i32; 3], f64)>) {
        let mut counts = [0i32; 7];
        let mut pot: Vec<([i32; 3], f64)> = Vec::new();
        let centers = self.sim_centers();
        for m in self.mobs_under_caps(&centers) {
            counts[Category::slot(m.kind.info().category)] += 1;
            let bp = m.block_pos();
            if self.chunks.get(&(bp[0] >> 4, bp[2] >> 4)).is_some_and(Option::is_some)
                && let Some((charge, _)) = self.spawn_cost(m.kind, bp)
                && charge != 0.0
            {
                pot.push((bp, charge));
            }
        }
        (counts, pot)
    }

    /// The mobs the caps count: not persistent, not miscellaneous, in a chunk with loaded entities.
    fn mobs_under_caps<'a>(&'a self, centers: &'a [(i32, i32)]) -> impl Iterator<Item = &'a Mob> + 'a {
        self.mobs.list.iter().filter(move |m| {
            if m.persistent || m.requires_custom_persistence() || m.kind.info().category == Category::Misc {
                return false;
            }
            let bp = m.block_pos();
            self.entities_loaded_at((bp[0] >> 4, bp[2] >> 4), centers)
        })
    }

    /// The counts of `spawn_state_counts` alone (census path: the charges are read apart, only
    /// when a category may spawn), and in the same walk the counts of each player near the mob.
    fn mob_counts_for_caps(&self, centers: &[(i32, i32)], per_player: &mut PlayerMobCaps) -> [i32; 7] {
        let mut counts = [0i32; 7];
        for m in self.mobs_under_caps(centers) {
            let category = m.kind.info().category;
            counts[Category::slot(category)] += 1;
            per_player.add_mob(chunk_of_block(m.block_pos()), category);
        }
        counts
    }

    /// The charges of `spawn_state_counts` (mobs counted by the caps standing in a loaded chunk),
    /// with the pass's memoized costs.
    fn spawn_charges(&self, centers: &[(i32, i32)], costs: &mut SpawnCostMemo) -> Vec<([i32; 3], f64)> {
        let mut pot = Vec::new();
        for m in self.mobs_under_caps(centers) {
            let bp = m.block_pos();
            if self.chunks.get(&(bp[0] >> 4, bp[2] >> 4)).is_some_and(Option::is_some)
                && let Some((charge, _)) = self.spawn_cost_memo(costs, m.kind, bp)
                && charge != 0.0
            {
                pot.push((bp, charge));
            }
        }
        pot
    }

    /// Loaded chunks within 8 (Chebyshev) of a player, in the players' order, each once: a chunk
    /// already covered by an earlier player's square was listed with it (no set needed; one
    /// player, the common case, has no earlier square at all).
    fn spawn_chunks_near(&self, players: &[([f64; 3], bool)]) -> Vec<(i32, i32)> {
        let chunk_of = |pos: &[f64; 3]| ((pos[0].floor() as i32) >> 4, (pos[2].floor() as i32) >> 4);
        let mut chunks = Vec::new();
        for (i, (pos, _)) in players.iter().enumerate() {
            let pc = chunk_of(pos);
            for dx in -8..=8 {
                for dz in -8..=8 {
                    let c = (pc.0 + dx, pc.1 + dz);
                    let listed = players[..i].iter().any(|(q, _)| within_8(chunk_of(q), c));
                    if !listed && self.chunks.get(&c).is_some_and(Option::is_some) {
                        chunks.push(c);
                    }
                }
            }
        }
        chunks
    }

    /// `spawn_chunks_near`, former path (a set of the chunks seen).
    fn spawn_chunks_near_with_set(&self, players: &[([f64; 3], bool)]) -> Vec<(i32, i32)> {
        let mut chunks: Vec<(i32, i32)> = Vec::new();
        let mut seen = hashbrown::HashSet::new();
        for (pos, _) in players {
            let pc = ((pos[0].floor() as i32) >> 4, (pos[2].floor() as i32) >> 4);
            for dx in -8..=8 {
                for dz in -8..=8 {
                    let c = (pc.0 + dx, pc.1 + dz);
                    if self.chunks.get(&c).is_some_and(Option::is_some) && seen.insert(c) {
                        chunks.push(c);
                    }
                }
            }
        }
        chunks
    }

    /// Chunk with loaded entities: players' loading ticket (view, Chebyshev) or
    /// force-loaded chunk, propagated by two chunks up to the "full" status.
    pub(crate) fn entities_loaded_at(&self, c: (i32, i32), centers: &[(i32, i32)]) -> bool {
        let r = self.view.clamp(2, 32) + 2;
        centers.iter().any(|p| (c.0 - p.0).abs() <= r && (c.1 - p.1).abs() <= r) || self.forced_near(c, 2)
    }

    /// `chunk_simulated` without a prepared list (hot paths: no allocation).
    pub(crate) fn chunk_simulated_now(&self, c: (i32, i32)) -> bool {
        let r = self.sim_dist + 1;
        let near = |p: (i32, i32)| (c.0 - p.0).abs() <= r && (c.1 - p.1).abs() <= r;
        if (near(self.chunk_pos()) && self.holds_tickets(self.game_mode))
            || self.forced_near(c, 1)
            || self.ticket_level(c) <= BLOCK_TICKING
        {
            return true;
        }
        self.lan.as_ref().is_some_and(|l| {
            l.guests.iter().any(|g| {
                g.ctx
                    .as_ref()
                    .filter(|ctx| g.joined && !g.gone && self.holds_tickets(ctx.game_mode))
                    .is_some_and(|ctx| near(((ctx.pos[0].floor() as i32) >> 4, (ctx.pos[2].floor() as i32) >> 4)))
            })
        })
    }

    /// Positions of living non-spectator players (host and guests).
    pub(crate) fn alive_players(&self) -> Vec<[f64; 3]> {
        let mut out = Vec::new();
        if self.game_mode != 3 && !self.life.vit.dead {
            out.push(self.pos);
        }
        if let Some(l) = &self.lan {
            for g in &l.guests {
                if let Some(c) = g.ctx.as_ref().filter(|_| g.joined && !g.gone)
                    && c.game_mode != 3
                    && !c.vit.dead
                {
                    out.push(c.pos);
                }
            }
        }
        out
    }

    /// Monster spawner: living player in range, delay elapsed.
    #[inline]
    pub(crate) fn spawner_tick(&mut self, p: [i32; 3], players: &[[f64; 3]]) {
        self.spawner_tick_at(SpawnerAt::Block(p), players);
    }

    /// Spawner of a block or carried by a minecart (G5-F06: data in its compound,
    /// entity event instead of the block event, nothing sent to the client).
    pub(crate) fn spawner_tick_at(&mut self, mut at: SpawnerAt<'_>, players: &[[f64; 3]]) {
        let paths = SpawnPaths::current();
        // Most ticks end here (no player in range, or counting down): settled on the block entity
        // itself, before the copy the rest of the tick works on.
        if paths.spawner_range_first || paths.spawner_borrow {
            if !self.spawner_player_in_range(&at, players) {
                return;
            }
            if paths.spawner_borrow && self.spawner_count_down(&mut at) {
                return;
            }
        }
        let (p, be) = match &at {
            SpawnerAt::Block(p) => match self.be_get(*p).cloned() {
                Some(be) => (*p, be),
                None => return,
            },
            SpawnerAt::Cart(c) => (c.block_pos(), c.nbt.clone()),
        };
        let geti = |k: &str, d: i32| spawner_int(&be, k, d);
        if !player_in_spawner_range(p, geti("RequiredPlayerRange", 16), players) {
            return;
        }
        let mut st = SpawnerState {
            delay: geti("Delay", 20),
            min: geti("MinSpawnDelay", 200),
            max: geti("MaxSpawnDelay", 800),
            next: be.get("SpawnData").cloned(),
            potentials: crate::nbtio::list(be.get("SpawnPotentials")).to_vec(),
            changed: false,
        };
        if be.get("SpawnPotentials").is_none() {
            // List absent: the next data alone (or an empty data).
            let d =
                st.next.clone().unwrap_or_else(|| Nbt::Compound(vec![("entity".into(), Nbt::Compound(Vec::new()))]));
            st.potentials = vec![Nbt::Compound(vec![("data".into(), d), ("weight".into(), Nbt::Int(1))])];
        }
        if st.delay == -1 {
            self.spawner_delay(&mut at, &mut st);
        }
        if st.delay > 0 {
            st.delay -= 1;
            self.spawner_store(&mut at, &st);
            return;
        }
        let count = geti("SpawnCount", 4);
        let max_near = geti("MaxNearbyEntities", 6);
        let spawn_range = geti("SpawnRange", 4);
        if st.next.is_none() {
            st.next = Some(
                pick_weighted(&st.potentials, &mut self.mobs.rng)
                    .unwrap_or_else(|| Nbt::Compound(vec![("entity".into(), Nbt::Compound(Vec::new()))])),
            );
            st.changed = true;
        }
        let next = st.next.clone().unwrap_or(Nbt::Compound(Vec::new()));
        let compound = next.get("entity").cloned().unwrap_or(Nbt::Compound(Vec::new()));
        // A mob, or any other entity type of the game (primed TNT, item, minecart…): width and
        // height of its box, `None` for an unknown type.
        let type_name = compound.get("id").and_then(Nbt::as_str).unwrap_or_default();
        let mob_kind = Kind::from_name(type_name);
        let mob_size = mob_kind.map(|k| [k.info().width, k.info().height]);
        let spawned_size = mob_size.or_else(|| self.type_dims(type_name).map(|d| [d[0], d[1]]));
        let spawn = SpawnerEntity {
            spawner: p,
            type_name,
            compound: &compound,
            custom_rules: next.get("custom_spawn_rules"),
            // Entities of the spawned type are counted in the spawner's block grown by its range.
            area: rc_physics::Aabb {
                min: p.map(|v| f64::from(v - spawn_range)),
                max: p.map(|v| f64::from(v + 1 + spawn_range)),
            },
            max_near,
        };
        let mut draw_new_delay = false;
        for _ in 0..count {
            // An unknown type spawns nothing: a new delay.
            let Some([width, height]) = spawned_size else {
                draw_new_delay = true;
                break;
            };
            let pos = match crate::nbtio::list(compound.get("Pos")) {
                [Nbt::Double(x), Nbt::Double(y), Nbt::Double(z)] => [*x, *y, *z],
                _ => {
                    let r = &mut self.mobs.rng;
                    let x = f64::from(p[0]) + (r.next_double() - r.next_double()) * f64::from(spawn_range) + 0.5;
                    let y = f64::from(p[1] + r.next_int_bound(3) - 1);
                    let z = f64::from(p[2]) + (r.next_double() - r.next_double()) * f64::from(spawn_range) + 0.5;
                    [x, y, z]
                }
            };
            if !rc_physics::is_free(&self.world(), &Mob::bbox_at(pos, width, height)) {
                continue;
            }
            let attempt = match mob_kind {
                Some(kind) => self.spawner_spawn_mob(kind, &spawn, pos),
                None => self.spawner_spawn_non_mob(&spawn, pos),
            };
            match attempt {
                SpawnerAttempt::Skipped => {}
                SpawnerAttempt::Spawned => draw_new_delay = true,
                SpawnerAttempt::GaveUp => {
                    draw_new_delay = true;
                    break;
                }
            }
        }
        if draw_new_delay {
            self.spawner_delay(&mut at, &mut st);
        }
        self.spawner_store(&mut at, &st);
    }

    /// One attempt of a monster spawner whose entity is a mob: its spawn rules (the spawn data's
    /// own ones when it has some), the mob read from its compound, the cap on mobs of its species
    /// around the spawner, then a spot it fits in, facing a random way.
    fn spawner_spawn_mob(&mut self, kind: Kind, spawn: &SpawnerEntity<'_>, pos: [f64; 3]) -> SpawnerAttempt {
        let bp = super::mth::block_of(pos);
        if let Some(rules) = spawn.custom_rules {
            if !kind.info().category.friendly() && self.level.difficulty_id() == 0 {
                return SpawnerAttempt::Skipped;
            }
            if !self.spawn_custom_rules_ok(rules, bp) {
                return SpawnerAttempt::Skipped;
            }
        } else if !self.spawn_rules_for(kind, bp, Reason::Spawner) {
            return SpawnerAttempt::Skipped;
        }
        // The species alone (its `id`, which named `kind`): the spawn is finalised like a natural
        // one; anything more: the mob is read as saved.
        let species_only = matches!(spawn.compound, Nbt::Compound(fields) if fields.len() == 1);
        let mut m = self.new_mob(kind, pos, 0.0);
        if !species_only {
            let mut full = spawn.compound.clone();
            crate::nbtio::set(&mut full, "Pos", Nbt::List(pos.iter().map(|&v| Nbt::Double(v)).collect()));
            self.load_mob(&mut m, &full);
            m.set_pos(pos);
            m.old = pos;
            m.sync_data();
        }
        let near = self.mobs.of_kind(kind).filter(|o| o.alive() && o.touches(&spawn.area)).count();
        if near as i32 >= spawn.max_near {
            return SpawnerAttempt::GaveUp;
        }
        let yaw = self.mobs.rng.next_float() * 360.0;
        m.yaw = yaw;
        m.head_yaw = yaw;
        m.body_yaw = yaw;
        m.pitch = 0.0;
        let walk_ok = spawn.custom_rules.is_some() || self.walk_target_value(&m, bp) >= 0.0;
        if !walk_ok || !self.spawn_obstruction(&mut m) {
            return SpawnerAttempt::Skipped;
        }
        if species_only {
            let mut g = Group::default();
            self.finalize_spawn(&mut m, Reason::Spawner, &mut g);
        }
        let id = m.id;
        self.add_mob(m);
        self.level_event(2004, spawn.spawner, 0);
        let src = crate::mobs::recent::vibration::Src::Living(Ent::Mob(id));
        self.game_event_src("minecraft:entity_place", bp.map(|v| f64::from(v) + 0.5), Some(src), None);
        if let Some(m) = self.mob_ref(id) {
            self.mob_event(m, 20);
        }
        SpawnerAttempt::Spawned
    }

    /// One attempt of a monster spawner whose entity is not a mob (primed TNT, item, minecart,
    /// armor stand…): its own spawn rules when the spawn data has some, the cap on entities of its
    /// class around the spawner, then the entity created from its compound as a summon does,
    /// facing a random way.
    fn spawner_spawn_non_mob(&mut self, spawn: &SpawnerEntity<'_>, pos: [f64; 3]) -> SpawnerAttempt {
        let bp = super::mth::block_of(pos);
        if spawn.custom_rules.is_some_and(|rules| !self.spawn_custom_rules_ok(rules, bp)) {
            return SpawnerAttempt::Skipped;
        }
        if self.non_mob_entities_in(spawn.type_name, &spawn.area) as i32 >= spawn.max_near {
            return SpawnerAttempt::GaveUp;
        }
        let yaw = self.mobs.rng.next_float() * 360.0;
        let mut compound = spawn.compound.clone();
        crate::nbtio::set(&mut compound, "Rotation", Nbt::List(vec![Nbt::Float(yaw), Nbt::Float(0.0)]));
        let source = self.server_source();
        if self.summon_entity(&source, spawn.type_name, pos, Some(compound)).is_err() {
            return SpawnerAttempt::GaveUp;
        }
        self.level_event(2004, spawn.spawner, 0);
        let src = crate::mobs::recent::vibration::Src::Other;
        self.game_event_src("minecraft:entity_place", bp.map(|v| f64::from(v) + 0.5), Some(src), None);
        SpawnerAttempt::Spawned
    }

    /// Entities of the class of `type_name`, other than mobs and players, whose box touches
    /// `area` (projectiles in flight are not listed here: a spawner of arrows never reaches its
    /// cap).
    fn non_mob_entities_in(&self, type_name: &str, area: &rc_physics::Aabb) -> usize {
        use crate::blocks::entities::EntKind;
        use crate::commands::ents::E;
        let listed = self.all_ents().into_iter().filter(|&e| {
            !matches!(e, E::Player | E::Mob(_))
                && same_entity_class(&self.ent_type(e), type_name)
                && self.e_bbox(e).intersects(area)
        });
        // Primed TNT and falling blocks live in the block simulation's own list.
        let simulated = self.bs.entities.list.iter().filter(|e| {
            let name = match e.kind {
                EntKind::Tnt(_) => "minecraft:tnt",
                EntKind::Falling(_) => "minecraft:falling_block",
            };
            !e.removed && name == type_name && Self::entity_box(e).intersects(area)
        });
        listed.count() + simulated.count()
    }

    /// A living player within `RequiredPlayerRange` of the spawner, read on its data in place.
    /// No block entity: false (the tick does nothing, like the former path).
    fn spawner_player_in_range(&self, at: &SpawnerAt<'_>, players: &[[f64; 3]]) -> bool {
        let (p, nbt) = match at {
            SpawnerAt::Block(p) => match self.be_get(*p) {
                Some(be) => (*p, be),
                None => return false,
            },
            SpawnerAt::Cart(c) => (c.block_pos(), &c.nbt),
        };
        player_in_spawner_range(p, spawner_int(nbt, "RequiredPlayerRange", 16), players)
    }

    /// Spawner counting down (delay above zero): the tick only lowers the delay, written in place
    /// without marking the chunk, like the full tick does (whose rewrite of the unchanged
    /// `SpawnData` changes nothing). The block entity is no longer copied on each of the 200 to
    /// 800 ticks between two spawns. False: the full tick must run (first delay, spawn).
    fn spawner_count_down(&mut self, at: &mut SpawnerAt<'_>) -> bool {
        let nbt = match at {
            SpawnerAt::Block(p) => {
                let p = *p;
                let Some(Some(c)) = self.chunks.get_mut(&(p[0] >> 4, p[2] >> 4)) else { return false };
                let Some(be) = c.block_entities.iter_mut().find(|v| be_is_at(v, p)) else { return false };
                be
            }
            SpawnerAt::Cart(c) => &mut c.nbt,
        };
        let delay = spawner_int(nbt, "Delay", 20);
        if delay <= 0 {
            return false;
        }
        crate::nbtio::set(nbt, "Delay", Nbt::Short((delay - 1) as i16));
        true
    }

    /// `delay`: delay drawn between the bounds, next data drawn from the list, block
    /// event 1 (the client resets its rotation to the minimal delay).
    fn spawner_delay(&mut self, at: &mut SpawnerAt<'_>, st: &mut SpawnerState) {
        st.delay = if st.max <= st.min { st.min } else { st.min + self.mobs.rng.next_int_bound(st.max - st.min) };
        if let Some(d) = pick_weighted(&st.potentials, &mut self.mobs.rng) {
            st.next = Some(d);
            st.changed = true;
        }
        match at {
            SpawnerAt::Block(p) => {
                let b = self.view().block(self.get(*p));
                self.block_event(*p, b, 1, 0);
            }
            // G5-F06: broadcast of an entity event.
            SpawnerAt::Cart(c) => self.cart_event(c, 1),
        }
    }

    /// Delay and next data rewritten (without marking the chunk); data changed: the client
    /// receives the block entity (without the potentials list). Minecart: compound only.
    fn spawner_store(&mut self, at: &mut SpawnerAt<'_>, st: &SpawnerState) {
        let p = match at {
            SpawnerAt::Block(p) => *p,
            SpawnerAt::Cart(c) => {
                crate::nbtio::set(&mut c.nbt, "Delay", Nbt::Short(st.delay as i16));
                if let Some(n) = &st.next {
                    crate::nbtio::set(&mut c.nbt, "SpawnData", n.clone());
                }
                return;
            }
        };
        let Some(Some(c)) = self.chunks.get_mut(&(p[0] >> 4, p[2] >> 4)) else { return };
        let Some(be) = c.block_entities.iter_mut().find(|v| be_is_at(v, p)) else {
            return;
        };
        crate::nbtio::set(be, "Delay", Nbt::Short(st.delay as i16));
        if let Some(n) = &st.next {
            crate::nbtio::set(be, "SpawnData", n.clone());
        }
        if st.changed {
            c.dirty = true;
            let mut nbt = be.clone();
            for k in ["id", "x", "y", "z", "keepPacked", "SpawnPotentials"] {
                crate::nbtio::remove(&mut nbt, k);
            }
            if let Some(kind) = self
                .data
                .registry("minecraft:block_entity_type")
                .and_then(|r| r.entries.iter().position(|e| e == "minecraft:mob_spawner"))
            {
                let _ = self.send(rc_client::Event::BlockEntityData { pos: p, kind: kind as i32, nbt: Some(nbt) });
            }
        }
    }

    /// Height map of the type (blocking movement for the ocelot
    /// and the parrot, without foliage otherwise); under a ceiling, descent to the air then
    /// to the ground; on the ground, one notch lower if the block below is walkable.
    fn top_non_colliding(&self, kind: Kind, x: i32, z: i32) -> [i32; 3] {
        let map = if matches!(kind, Kind::Ocelot | Kind::Parrot) { 4 } else { 5 };
        let mut p = [x, self.surface_y(x, z, map), z];
        let ceiling = self.dim_kind().ceiling;
        if ceiling {
            loop {
                p[1] -= 1;
                if self.is_air(p) || p[1] <= self.dim.height.min_y {
                    break;
                }
            }
            loop {
                p[1] -= 1;
                if !(self.is_air(p) && p[1] > self.dim.height.min_y) {
                    break;
                }
            }
        }
        if self.placed_on_ground(kind, p) && self.pathfindable_land([p[0], p[1] - 1, p[2]]) {
            p[1] -= 1;
        }
        p
    }

    fn valid_position_for_mob(&self, m: &mut Mob, d: f64) -> bool {
        let far = f64::from(m.kind.info().category.despawn_distance().pow(2));
        if d > far && self.remove_when_far_away(m) {
            return false;
        }
        let p = m.block_pos();
        self.walk_target_value(m, p) >= 0.0 && self.spawn_obstruction(m)
    }

    /// No obstructing entity, no liquid (except species that
    /// tolerate it).
    pub(crate) fn spawn_obstruction(&self, m: &mut Mob) -> bool {
        if !super::surface::spawn_obstruction_ok(self, m) {
            return false;
        }
        // F08: liquid allowed (for aquatic species).
        if super::aquatic::spawn_obstruction_skips_fluid(m.kind) {
            return true;
        }
        // F09: aquatic mobs: only free space from entities counts (fluids allowed).
        if let Some(ok) = super::nether::obstruction_ok(self, m)
            .or_else(|| super::piglins::obstruction_ok(self, m))
            .or_else(|| super::sealife::obstruction_ok(self, m))
            .or_else(|| super::brainy::obstruction_ok(self, m))
        {
            return ok;
        }
        !self.box_has_liquid(&m.bbox())
    }

    /// No "blocking" entity not removed overlaps `bb` (mobs,
    /// even dying ones, player even dead, primed TNT, falling block; spectator excluded);
    /// `except`: the entity being tested itself.
    pub(crate) fn unobstructed(&self, bb: &rc_physics::Aabb, except: i32) -> bool {
        if self.game_mode != 3 && self.player_aabb().intersects(bb) {
            return false;
        }
        if self.mobs.list.iter().any(|o| o.id != except && !o.removed && o.touches(bb)) {
            return false;
        }
        !self.bs.entities.list.iter().any(|e| {
            let h = 0.98 / 2.0;
            !e.removed
                && bb.intersects(&rc_physics::Aabb {
                    min: [e.pos[0] - h, e.pos[1], e.pos[2] - h],
                    max: [e.pos[0] + h, e.pos[1] + 0.98, e.pos[2] + h],
                })
        })
    }

    /// Block below that allows spawning.
    pub(crate) fn valid_spawn_on(&self, p: [i32; 3], kind: Kind) -> bool {
        // The name is lent by the block table: no copy (this test runs for every spawn attempt).
        let n = self.bname(p).trim_start_matches("minecraft:");
        // Species-specific predicates (F13: portal zombified piglin on magma).
        if n == "magma_block" {
            return kind.info().fire_immune;
        }
        if n == "ice" || n == "frosted_ice" {
            return kind == Kind::PolarBear;
        }
        if n.ends_with("_leaves") {
            return matches!(kind, Kind::Ocelot | Kind::Parrot);
        }
        const NEVER: [&str; 8] = [
            "bedrock",
            "glass",
            "tinted_glass",
            "moving_piston",
            "repeater",
            "barrier",
            "chorus_flower",
            "scaffolding",
        ];
        // The eight copper grates (every weathering stage, waxed or not) end the same way.
        let grate = n.ends_with("copper_grate");
        if NEVER.contains(&n) || n.ends_with("_stained_glass") || n.ends_with("_trapdoor") || grate {
            return false;
        }
        const ALWAYS: [&str; 5] = ["soul_sand", "carved_pumpkin", "jack_o_lantern", "redstone_lamp", "mud"];
        if ALWAYS.contains(&n) {
            return true;
        }
        let s = self.st(p);
        let Some(i) = self.data.state(s) else { return false };
        i.sturdy & 2 != 0 && i.light_emission < 14
    }

    /// Valid empty cell.
    pub(crate) fn valid_empty_spawn(&self, p: [i32; 3], kind: Kind) -> bool {
        self.mob_fluid(p) == Fluid::None && self.valid_empty_spawn_dry(p, kind)
    }

    /// Valid empty cell without looking at its fluid (caller that passes an empty fluid).
    pub(crate) fn valid_empty_spawn_dry(&self, p: [i32; 3], kind: Kind) -> bool {
        if self.sflag(p, flags::FULL_COLLISION) {
            return false;
        }
        if self.btag_id(p, crate::tag_bits::TagId::PreventMobSpawningInside) {
            return false;
        }
        let n = self.bname(p).trim_start_matches("minecraft:");
        let signal = n.contains("redstone")
            || n.ends_with("_button")
            || n.ends_with("_pressure_plate")
            || n.ends_with("lightning_rod")
            || matches!(
                n,
                "lever"
                    | "repeater"
                    | "comparator"
                    | "observer"
                    | "daylight_detector"
                    | "target"
                    | "tripwire_hook"
                    | "trapped_chest"
                    | "lightning_rod"
            );
        !signal && !self.block_dangerous(p, kind)
    }

    /// Block dangerous for a species (the game's per-type test, shared by spawning and the
    /// breeze's jumps): never for the species' own "immune to" tag (`default_immune_to` for the
    /// species without one, empty in the vanilla data, filled by a data pack); the burning blocks
    /// (fire tag, lava, magma, lava cauldron, lit campfire) unless it is fire immune; wither
    /// rose, sweet berry bush, cactus and powder snow.
    pub(crate) fn block_dangerous(&self, p: [i32; 3], kind: Kind) -> bool {
        use crate::tag_bits::TagId;
        let immune = match kind {
            Kind::Fox => TagId::FoxImmuneTo,
            Kind::PolarBear => TagId::PolarBearImmuneTo,
            Kind::SnowGolem => TagId::SnowGolemImmuneTo,
            Kind::Stray => TagId::StrayImmuneTo,
            Kind::Wither => TagId::WitherImmuneTo,
            Kind::WitherSkeleton => TagId::WitherSkeletonImmuneTo,
            _ => TagId::DefaultImmuneTo,
        };
        if self.btag_id(p, immune) {
            return false;
        }
        let n = self.bname(p).trim_start_matches("minecraft:");
        let burning = || {
            matches!(n, "lava" | "magma_block" | "lava_cauldron")
                || self.btag_id(p, crate::tag_bits::TagId::Fire)
                || ((n == "campfire" || n == "soul_campfire")
                    && self.view().get_bool(self.st(p), PropKey::Lit))
        };
        matches!(n, "wither_rose" | "sweet_berry_bush" | "cactus" | "powder_snow")
            || (!kind.info().fire_immune && burning())
    }

    /// Own rules of a spawn data (`custom_spawn_rules`): block light and
    /// effective sky light within their bounds (single integer: exact value).
    pub(crate) fn spawn_custom_rules_ok(&mut self, rules: &Nbt, bp: [i32; 3]) -> bool {
        let (sky, block) = self.light(bp);
        let eff_sky = (i32::from(sky) - self.mob_sky_darken()).max(0);
        let within = |key: &str, v: i32| match rules.get(key) {
            Some(Nbt::Compound(r)) => {
                let g = |k: &str| r.iter().find(|(n, _)| n == k).and_then(|(_, v)| v.as_i64()).map(|v| v as i32);
                g("min_inclusive").unwrap_or(0) <= v && v <= g("max_inclusive").unwrap_or(15)
            }
            Some(n) => n.as_i64().is_none_or(|x| x as i32 == v),
            None => true,
        };
        within("block_light_limit", i32::from(block)) && within("sky_light_limit", eff_sky)
    }

    /// Spawn rules for a given reason: monster spawner and trial spawner do not require
    /// a valid support block or a visible sky; the trial spawner ignores light.
    pub(crate) fn spawn_rules_for(&mut self, kind: Kind, p: [i32; 3], reason: Reason) -> bool {
        let old = self.mobs.spawners.rule_reason.replace(reason);
        let r = self.spawn_rules(kind, p);
        self.mobs.spawners.rule_reason = old;
        r
    }

    /// The "spawn by a spawner" test for the rule being evaluated.
    pub(crate) fn rule_is_spawner(&self) -> bool {
        matches!(self.mobs.spawners.rule_reason, Some(Reason::Spawner | Reason::TrialSpawner))
    }

    /// Spawn rule of the type (world random source).
    pub(crate) fn spawn_rules(&mut self, kind: Kind, p: [i32; 3]) -> bool {
        if let Some(r) = self.aquatic_spawn_rules(kind, p) {
            return r;
        }
        if let Some(r) = super::surface::spawn_rules(self, kind, p)
            .or_else(|| super::mounts::spawn_rules(self, kind, p))
            .or_else(|| super::sealife::spawn_rules(self, kind, p))
            .or_else(|| super::brainy::spawn_rules(self, kind, p))
            .or_else(|| super::wild::spawn_rules(self, kind, p))
            .or_else(|| super::nether::spawn_rules(self, kind, p))
            .or_else(|| super::piglins::spawn_rules(self, kind, p))
        {
            return r;
        }
        let below = [p[0], p[1] - 1, p[2]];
        let reason = self.mobs.spawners.rule_reason;
        let spawner = matches!(reason, Some(Reason::Spawner | Reason::TrialSpawner));
        // Slime from a spawner: difficulty then common rule (no support block required).
        if kind == Kind::Slime && spawner {
            return self.level.difficulty_id() != 0;
        }
        // F03: own rules (slime: chunk or swamp then support block; magma, sulfur).
        if kind.is_cube() {
            return self.cube_spawn_rule(kind, p) && (kind != Kind::Slime || self.valid_spawn_on(below, kind));
        }
        if kind.is_monster() {
            if self.level.difficulty_id() == 0 {
                return false;
            }
            // Breeze and zoglin: the monster spawn rules without light condition (any light; support block
            // except spawner).
            if matches!(kind, Kind::Breeze | Kind::Zoglin) {
                return spawner || self.valid_spawn_on(below, kind);
            }
            if spawner {
                // Spawners: support block and sky not required; light ignored by the
                // trial spawner only; nearby player allowed for the silverfish.
                let light_ok = reason == Some(Reason::TrialSpawner)
                    || matches!(kind, Kind::Silverfish | Kind::Endermite)
                    || self.monster_dark_enough(kind, p);
                return light_ok;
            }
            // Pillager (patroller rule): block light <= 8, no sky condition.
            if kind == Kind::Pillager {
                let (_, block) = self.light(p);
                return block <= 8 && self.valid_spawn_on(below, kind);
            }
            // Silverfish, endermite: any light, no player within 5 blocks.
            if matches!(kind, Kind::Silverfish | Kind::Endermite) {
                return self.valid_spawn_on(below, kind) && self.no_player_within(p, 5.0);
            }
            if !self.monster_dark_enough(kind, p) {
                return false;
            }
            if !self.valid_spawn_on(below, kind) {
                return false;
            }
            return self.variant_sky_rule(kind, p);
        }
        if kind == Kind::Wolf {
            return self.btag_id(below, crate::tag_bits::TagId::WolvesSpawnableOn)
                && self.light(p).0.max(self.light(p).1) > 8;
        }
        self.btag_id(below, crate::tag_bits::TagId::AnimalsSpawnableOn) && {
            let (s, b) = self.light(p);
            s.max(b) > 8
        }
    }

    /// The spawn brightness test (pillager: block light at most 8).
    fn monster_dark_enough(&mut self, kind: Kind, p: [i32; 3]) -> bool {
        if kind == Kind::Pillager {
            return self.light(p).1 <= 8;
        }
        let (sky, block) = self.light(p);
        if i32::from(sky) > self.mobs.rng.next_int_bound(32) {
            return false;
        }
        // Light limit block (0 at the surface), tested before the brightness draw.
        if i32::from(block) > 0 {
            return false;
        }
        // Thunderstorm (effective thunderstorm above 0.9): sky light reduced by 10.
        let raw = if self.world_thundering() {
            let (s, b) = self.light(p);
            (i32::from(s) - 10).max(0).max(i32::from(b))
        } else {
            self.mob_brightness(p)
        };
        let limit = self.mobs.rng.next_int_bound(8);
        raw <= limit
    }

    /// Last player who hurt (reset after 100 ticks), player's clock.
    pub(crate) fn player_memory_tick(&mut self) {
        self.mobs.player_ticks += 1;
        if let Some((_, ts)) = self.mobs.player_hurt_by
            && self.mobs.player_ticks - ts > 100
        {
            self.mobs.player_hurt_by = None;
        }
        self.mobs.attack_ticker += 1;
        // Server-known speed for the player (damped as outside the simulation).
        let v = &mut self.mobs.player_vel;
        *v = v.map(|x| x * 0.98);
        if v[0].abs() < 0.003 {
            v[0] = 0.0;
        }
        if v[1].abs() < 0.003 {
            v[1] = 0.0;
        }
        if v[2].abs() < 0.003 {
            v[2] = 0.0;
        }
        let _ = Ent::Player;
    }
}

/// Enchantment provider `mob_spawn_equipment` (5 to
/// 5 + 17): cost 5 + U[0, ⌊special × 17⌋], then draw from the table (S7) in the tag
/// `on_mob_spawn_equipment`; the levels add to those of the stack (maximum).
pub(crate) fn mob_spawn_enchant(
    pack: &crate::datapack::DataPack,
    items: &crate::items::Items,
    s: crate::player::Stack,
    special: f32,
    rng: &mut dyn crate::randsrc::Rand,
) -> crate::player::Stack {
    let defs = crate::enchanting::of(pack);
    let cost = 5 + rng.next_int_bound((special * 17.0) as i32 + 1);
    let list = defs.select(items, rng, &s, cost, defs.tag("minecraft:on_mob_spawn_equipment"));
    let mut s = s;
    for (i, l) in list {
        crate::enchanting::upgrade(items, &mut s, &defs.list[i].id, l);
    }
    s
}

/// State of a monster spawner during its tick.
struct SpawnerState {
    delay: i32,
    min: i32,
    max: i32,
    next: Option<Nbt>,
    potentials: Vec<Nbt>,
    changed: bool,
}

/// What a monster spawner spawns during one tick, shared by its attempts.
struct SpawnerEntity<'a> {
    /// Block of the spawner, or of the minecart that carries it.
    spawner: [i32; 3],
    type_name: &'a str,
    /// The `entity` compound of the spawn data.
    compound: &'a Nbt,
    custom_rules: Option<&'a Nbt>,
    /// Entities of the spawned kind whose box touches this one count towards `max_near`.
    area: rc_physics::Aabb,
    max_near: i32,
}

/// Outcome of one spawn attempt of a monster spawner.
enum SpawnerAttempt {
    Spawned,
    /// This attempt fails, the next one of the tick is still tried.
    Skipped,
    /// A new delay is drawn and the tick ends.
    GaveUp,
}

/// What a natural spawn pass shares between its attempts: what no spawn can change (the pack,
/// the structures of the loaded chunks, the world spawn), and the counts each spawn adds to.
struct SpawnPass {
    paths: SpawnPaths,
    costs: SpawnCostMemo,
    starts: DecodedStarts,
    /// Position whose spawn list the group's current entry was drawn from.
    entry_drawn_at: Option<[i32; 3]>,
    /// Counted mobs per spawning category (`Category::SPAWNING` order), spawns of the pass included.
    counts: [i32; 7],
    player_caps: PlayerMobCaps,
    /// Centre of the world spawn block, in its dimension only (24 blocks without natural spawns).
    world_spawn: Option<[f64; 3]>,
}

/// Mob counts of each player for the local cap of a natural spawn pass: a mob counts for every
/// non-spectator player within 128 blocks of its chunk, and a chunk may spawn a category as long
/// as one of the players near it is under that category's cap.
pub(super) struct PlayerMobCaps {
    /// Position and counts per spawning category of each non-spectator player.
    players: Vec<([f64; 3], [i32; 7])>,
}

impl PlayerMobCaps {
    /// `players`: position and spectator state of every connected player.
    pub(super) fn new(players: &[([f64; 3], bool)]) -> PlayerMobCaps {
        let counted = players.iter().filter(|(_, spectator)| !spectator);
        PlayerMobCaps { players: counted.map(|(pos, _)| (*pos, [0; 7])).collect() }
    }

    pub(super) fn add_mob(&mut self, chunk: (i32, i32), category: Category) {
        let slot = Category::slot(category);
        for (pos, counts) in &mut self.players {
            if chunk_within_spawn_distance(pos, chunk) {
                counts[slot] += 1;
            }
        }
    }

    pub(super) fn can_spawn(&self, category: Category, chunk: (i32, i32)) -> bool {
        let slot = Category::slot(category);
        self.players
            .iter()
            .any(|(pos, counts)| chunk_within_spawn_distance(pos, chunk) && counts[slot] < category.max())
    }
}

/// Centre of `chunk` less than 128 blocks (horizontally) from a player at `pos`: the chunk
/// spawns for that player, and its mobs count for that player.
fn chunk_within_spawn_distance(pos: &[f64; 3], chunk: (i32, i32)) -> bool {
    let dx = f64::from(chunk.0 * 16 + 8) - pos[0];
    let dz = f64::from(chunk.1 * 16 + 8) - pos[2];
    dx * dx + dz * dz < 128.0 * 128.0
}

/// Two entity types the game creates from one same class: a monster spawner counts them
/// together for its cap on nearby entities. Only the wooden boats share one, and the wooden
/// chest boats another (the bamboo rafts have their own).
fn same_entity_class(type_a: &str, type_b: &str) -> bool {
    fn class_of(type_name: &str) -> &str {
        if type_name.ends_with("_chest_boat") {
            "chest boat"
        } else if type_name.ends_with("_boat") {
            "boat"
        } else {
            type_name
        }
    }
    class_of(type_a) == class_of(type_b)
}

fn chunk_of_block(p: [i32; 3]) -> (i32, i32) {
    (p[0] >> 4, p[2] >> 4)
}

/// Spawn costs already read during a pass (a few biomes and species: a list beats a map).
#[derive(Default)]
pub(crate) struct SpawnCostMemo {
    known: Vec<KnownCost>,
}

/// Spawn cost of a species in a biome (registry index), `None` when it has none.
struct KnownCost {
    biome: usize,
    kind: Kind,
    cost: Option<(f64, f64)>,
}

/// Chunk `c` lies in the 17 × 17 square around `center`.
fn within_8(center: (i32, i32), c: (i32, i32)) -> bool {
    (c.0 - center.0).abs() <= 8 && (c.1 - center.1).abs() <= 8
}

/// Chunks within 8 of a center, loaded or not: the union of the 17 × 17 squares, counted
/// without a set (a chunk counts for the first center whose square covers it): 289 for one
/// center, 0 without any.
pub(super) fn spawn_chunk_count(centers: &[(i32, i32)]) -> i32 {
    let mut n = 0;
    for (i, &center) in centers.iter().enumerate() {
        for dx in -8..=8 {
            for dz in -8..=8 {
                let c = (center.0 + dx, center.1 + dz);
                if !centers[..i].iter().any(|&o| within_8(o, c)) {
                    n += 1;
                }
            }
        }
    }
    n
}

/// `spawn_chunk_count`, former path (a set of every chunk).
pub(super) fn spawn_chunk_count_with_set(centers: &[(i32, i32)]) -> i32 {
    let mut all = hashbrown::HashSet::new();
    for c in centers {
        for dx in -8..=8 {
            for dz in -8..=8 {
                all.insert((c.0 + dx, c.1 + dz));
            }
        }
    }
    all.len() as i32
}

/// Integer field of a monster spawner's data (any width, truncated like the game's read).
fn spawner_int(nbt: &Nbt, key: &str, default: i32) -> i32 {
    crate::nbtio::int(nbt, key).map_or(default, |v| v as i32)
}

/// A living player strictly within `range` of the spawner's center (negative range: always).
fn player_in_spawner_range(p: [i32; 3], range: i32, players: &[[f64; 3]]) -> bool {
    let c = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
    let r2 = f64::from(range) * f64::from(range);
    players.iter().any(|q| range < 0 || dist2(*q, c) < r2)
}

/// Block entity of position `p` (its `x`, `y`, `z` fields).
fn be_is_at(be: &Nbt, p: [i32; 3]) -> bool {
    crate::nbtio::int(be, "x") == Some(i64::from(p[0]))
        && crate::nbtio::int(be, "y") == Some(i64::from(p[1]))
        && crate::nbtio::int(be, "z") == Some(i64::from(p[2]))
}

/// Weighted draw in `[{data, weight}]`: no draw if the list
/// is empty or of zero weight.
pub(crate) fn pick_weighted(list: &[Nbt], rng: &mut JRand) -> Option<Nbt> {
    let w = |e: &Nbt| crate::nbtio::int(e, "weight").unwrap_or(1) as i32;
    let total: i32 = list.iter().map(w).sum();
    if total <= 0 {
        return None;
    }
    let mut r = rng.next_int_bound(total);
    let e = list.iter().find(|e| {
        r -= w(e);
        r < 0
    })?;
    e.get("data").cloned()
}

/// Energy variation of the potential (sum of the charges / distance, infinite on the same cell)
/// multiplied by the type's charge, compared to its budget.
pub(crate) fn potential_ok(charge: f64, budget: f64, p: [i32; 3], pot: &[([i32; 3], f64)]) -> bool {
    if charge == 0.0 {
        return 0.0 <= budget;
    }
    let mut e = 0.0;
    for (q, c) in pot {
        let d2 = crate::poi::dist2i(*q, p) as f64;
        e += if d2 == 0.0 { f64::INFINITY } else { c / d2.sqrt() };
    }
    e * charge <= budget
}

/// Category key of the spawn lists (miscellaneous have none).
fn spawn_key(cat: Category) -> Option<&'static str> {
    cat.key()
}
