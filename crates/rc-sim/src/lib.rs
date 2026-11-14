//! Rust singleplayer (`docs/architecture.md` § 14): server integrated into the client, in the same process.
//! Reading and writing worlds in the official game's format (`docs/specs/anvil-world.md`),
//! tick loop and player (`docs/specs/integrated-server.md`). No graphics dependency.

// Test scripts read their settings directly (at launch, off the hot path). The game
// code stays checked: the clippy pass without `cfg(test)` forbids any read outside rc_data::env_*!.
#![cfg_attr(test, allow(clippy::disallowed_methods))]

mod blocks;
mod fluid_use;
pub mod chunk;
mod chunk_disk;
mod stored_light; // the game's reading of a chunk's light sheets, shared by every server reader
pub mod debug_report;
pub mod demo;
pub mod gen_debug;
pub mod generation;
pub mod generation_nbt;
pub mod generation_timing;
pub mod input;
pub mod level;
mod loader;
pub mod lock;
pub mod mem_stats;
pub mod nbtio;
pub mod newworld;
mod play;
mod building_blockers; // entities in the way of a block placement or a mob spawn
mod spectator_camera; // a spectator's camera on an entity: followed, given back on sneak, death, mode or teleport
#[cfg(test)]
mod spectator_camera_tests;
mod solid_entities; // the solid entities (shulkers, still happy ghasts) in the server's free-room tests
#[cfg(test)]
mod solid_entities_tests;
pub mod placement_prediction; // the client's block placement at the click, with the server's rules
pub mod entity_use_prediction; // the client's reaction to a right click on an entity, with the server's tables
#[cfg(test)]
mod placement_prediction_tests;
#[cfg(test)]
mod movement_collisions_tests; // movements against the solid entities, one test per family of sites
pub mod player;
pub mod prep;
pub mod region;
pub mod registries;
mod save;
pub mod server;
pub mod states;
pub mod storage;
pub mod version_info;
pub mod world;
// W8a: equipment attributes, client packets specific to singleplayer.
pub mod attrs;
pub mod contact; // G5-F02e: crossed blocks, enchanted boots
mod solo_packets;
mod tech_blocks; // G6-F14: structure, jigsaw and test blocks, live templates
#[cfg(test)]
mod tests_g6f14;
mod gametest; // G3-F06: test instances, environments, `/test` runner
#[cfg(test)]
mod tests_g3f06;
// Survival loop (milestone 2, S2).
pub mod book;
pub mod containers;
pub mod datapack;
pub mod packs;
pub mod drops;
mod block_loot;
mod display; // G5-F05: display and interaction entities
pub mod life;
pub mod items;
pub mod item_use;
pub mod loot;
pub mod menus;
pub mod netstack;
pub mod recipes;
pub mod rng;
pub mod vitals;
pub(crate) mod inside_blocks; // G5-F02a: shared crossed-block pass
pub(crate) mod usercache; // L9-COMBAT: profile name cache (`usercache.json`)
pub mod weather;
pub(crate) mod waypoints; // locator bar waypoints: transmitters, receivers, connections
#[cfg(test)]
mod waypoints_tests;
pub(crate) mod world_clocks; // clocks of the world_clock registry: pace, time packets, save file
pub mod tickrate;
mod tick_stages; // composition of the slowest ticks (`RUSTCRAFT_PERFLOG_STAGES`)
mod helper_pool; // short parallel jobs of the server, on threads kept apart from chunk generation
// Mobs (S4).
pub mod mobs;
mod walk; // L10-ORDER: one entity walk for every family, in list order
pub(crate) mod decoration;
// Dimensions and portals (S6).
pub(crate) mod dims;
mod portal;
mod portal_site;
pub(crate) mod portal_shape;
mod respawn;
// Effects, potions, enchanting, stations, loot chests (S7).
pub mod brewing;
pub mod effects;
pub mod enchanting;
pub mod lootchest;
mod lootfn;
pub mod potions;
pub mod special;
pub mod stations;
mod stations_srv;
mod beacon; // G5-F11: beacon
pub(crate) mod tag_bits; // C6-M02: block tags as bits
mod crafter; // G5-F11: crafter menu
#[cfg(test)]
mod tests_g5f11;
#[cfg(test)]
mod tests_g1f16; // G1-F16: F3 values answered by the server
#[cfg(test)]
mod tests_g4f09; // G4-F09: turtle helmet, 1.21 effects, hurt sound, statistics
#[cfg(test)]
mod tests_g4f12; // G4-F12: iron and copper golems
#[cfg(test)]
mod tests_g5f02c; // G5-F02c: fall (wind charge, per-block rules, multiplier, sound, dust)
#[cfg(test)]
mod tests_g5f02e; // G5-F02e: contacts with crossed blocks, enchanted boots
mod thorns; // G5-F02d: player thorns
mod tests_g5f02d;
mod tests_g5f05; // G5-F05: orbs, ground items, displays, interaction, marker
#[cfg(test)]
mod tests_kept_entity_chunks; // kept compounds: dirty chunk on change, refiled on chunk change
mod ench_fx; // G5-F04: enchantment effects (protection, armor effectiveness, after-hit effects)
mod mace; // G5-F04: smash attack
mod player_spear; // the player's spear: stab, charge of a held use
#[cfg(test)]
mod tests_g5f04;
#[cfg(test)]
mod tests_damage_tag_names; // short damage tag and damage type names, read in `minecraft`
pub mod randsrc;
// Commands (S5).
pub mod commands;
// Advancements and statistics (S9).
pub mod progress;
mod resource_id; // identifiers compared and completed with their default namespace, without allocating
pub mod stats;
pub mod score_criteria;
// Local network (S9).
pub mod lan;
// Points of interest (F05).
pub mod poi;
// Player projectiles, fishing, sealed loot read by redstone (S8).
mod lootseal;
pub mod maps;
pub mod projectiles;
mod eye_of_ender;
pub(crate) mod brush;
// Minecarts (G5-F06).
pub mod minecart;
pub mod boat; // boats and rafts, a kind of the minecarts' vehicle structure

pub use blocks::fluid::shape_face_full;
pub use blocks::piston_plan::{MovingBlock, PistonReplay, PistonWorld, ReplayOp, ReplayWorld};
pub use blocks::redstone::{button_sound, openable_sound_name};
pub use server::{Handle, Options, start};

/// Tests on the game's allocator (mimalloc, that of `rc-app`). With Windows' system heap,
/// the simultaneous server openings of the suite (unpacked jar, generation) stalled the
/// whole process for tens of seconds, processor almost idle. Feature `memstats`: the same
/// allocator behind an allocation counter (`bench_kit`).
#[cfg(all(test, not(feature = "memstats")))]
#[global_allocator]
static TEST_ALLOC: test_alloc::Counting = test_alloc::Counting;

/// The tests' allocator counts the allocations of each thread (each test has its own), so that
/// a test can prove a path does not allocate.
#[cfg(test)]
pub(crate) mod test_alloc {
    use std::alloc::{GlobalAlloc, Layout};
    use std::cell::Cell;

    thread_local! {
        static ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
        static REQUESTED_BYTES: Cell<u64> = const { Cell::new(0) };
    }

    /// Allocations made by the current thread so far.
    pub(crate) fn allocations() -> u64 {
        ALLOCATIONS.with(Cell::get)
    }

    /// Bytes the current thread asked the allocator for so far (frees not subtracted): the heap
    /// of a value is what its copy asks for.
    pub(crate) fn requested_bytes() -> u64 {
        REQUESTED_BYTES.with(Cell::get)
    }

    fn count(bytes: usize) {
        let _ = ALLOCATIONS.try_with(|c| c.set(c.get() + 1));
        let _ = REQUESTED_BYTES.try_with(|c| c.set(c.get() + bytes as u64));
    }

    pub(crate) struct Counting;

    // Delegates to mimalloc; the counters are const-initialised thread-locals (no allocation,
    // no destructor), safe to touch from inside the allocator.
    #[allow(unsafe_code)]
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            count(layout.size());
            // SAFETY: same contract as the caller's, passed on unchanged.
            unsafe { mimalloc::MiMalloc.alloc(layout) }
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            count(layout.size());
            // SAFETY: same contract as the caller's, passed on unchanged.
            unsafe { mimalloc::MiMalloc.alloc_zeroed(layout) }
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            count(new_size.saturating_sub(layout.size()));
            // SAFETY: same contract as the caller's, passed on unchanged.
            unsafe { mimalloc::MiMalloc.realloc(ptr, layout, new_size) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            // SAFETY: same contract as the caller's, passed on unchanged.
            unsafe { mimalloc::MiMalloc.dealloc(ptr, layout) }
        }
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_s2;
#[cfg(test)]
mod newworld_tests;
#[cfg(test)]
mod chunk_disk_tests;
#[cfg(test)]
mod portal_tests;
#[cfg(test)]
mod portal_site_tests;
#[cfg(test)]
mod tests_s7;
#[cfg(test)]
mod tests_s8;
#[cfg(test)]
mod parity_tests;
#[cfg(test)]
mod tests_tick;
#[cfg(test)]
mod tests_w8a;
#[cfg(test)]
mod tests_air;
#[cfg(test)]
mod tests_opt;
#[cfg(test)]
mod tests_c6m02;
#[cfg(test)]
mod collision_walk_tests;
#[cfg(test)]
mod tests_light_m04;
#[cfg(test)]
mod tests_light_m05;
#[cfg(test)]
mod jar_index_tests;
#[cfg(test)]
mod tests_w8c;
#[cfg(test)]
mod tests_g3f02;
#[cfg(test)]
mod tests_g5f10;
#[cfg(test)]
mod tests_g3f03;
#[cfg(test)]
mod tests_g6f11;
#[cfg(test)]
mod tests_g3f11;
#[cfg(test)]
mod tests_g3f13;
#[cfg(test)]
mod tests_l9tick; // L9-TICK: pearl and fight tickets, state kept with its dimension
#[cfg(test)]
mod tests_l10order; // L10-ORDER: one entity walk, every kind in the order of arrival
#[cfg(test)]
mod bench_l10order; // L10-ORDER: server tick with 1,000 entities of five families
#[cfg(test)]
mod tests_ground_items; // ground items and orbs: pickup and recycled bodies, both ways of each switch
#[cfg(test)]
mod bench_ground_items; // ground items: merges of a dropped pile, births and removals per tick
#[cfg(test)]
mod bench_kit; // common kit of the micro-benches: flat world, perf pack, timed ticks, `[bench]` lines
#[cfg(test)]
mod bench_free_room; // free-room tests with their solid entities: index, whole-list walk, blocks alone
#[cfg(test)]
mod bench_movement_collisions; // a mob's movement on the plain world against the world made for it
#[cfg(test)]
mod bench_spawn_obstruction; // the default spawn obstruction test with and without its entities
#[cfg(test)]
mod bench_spear_reach_search; // the player's spear search among 1,000 mobs, with and without its search box
#[cfg(test)]
mod bench_datapack_tick; // tick of the bench's `datapack` scene (crowd, #tick workload, command blocks)
#[cfg(test)]
mod bench_path_search; // mob path search on the village of the bench model world, each lead alone
#[cfg(test)]
mod bench_scores; // scoreboard switches: writes over every holder, save, entity removals, team lookups
#[cfg(test)]
mod bench_function_tick; // command switches: functions per tick, macros, command blocks, schedule, parse
#[cfg(test)]
mod bench_command_suggestions; // suggestion requests: tree shared, lists deduplicated in one pass, criteria kept
#[cfg(test)]
mod bench_selectors; // selector switches: one search, the quadratic `execute as … at @s`, families, `@s`
#[cfg(test)]
mod bench_player_selectors; // `@a`, `@p`, names and player options, alone and with seven LAN guests
#[cfg(test)]
mod bench_execute_across_worlds; // `execute` steps in one world, with an empty or a holding other world
#[cfg(test)]
mod bench_data_world_slot_commands; // silent replies, a large storage, filtered fill, container slots, particles
#[cfg(test)]
mod bench_spawner_blocks; // trial spawners, vaults, monster spawners, natural spawn pass, structure lists
#[cfg(test)]
mod tests_menu_switches; // menu switches: open chest fed and drained by hoppers, every combination
#[cfg(test)]
mod bench_menus; // menus: broadcast with a full inventory, open chest re-read, clicks
#[cfg(test)]
mod bench_advancement_triggers; // advancement switches: inventory and recipe indexes, location, file and tree loading
#[cfg(test)]
mod bench_tnt_heap; // mass explosion: 1,728 TNT blocks set off at once, visible parts and object pass
#[cfg(test)]
mod bench_visible_part; // visible part of 1 to 1,000 explosion targets, per call and together
#[cfg(test)]
mod bench_player_effect_data; // player effect data: sync of the particle list and ambient flag, spectator tick
#[cfg(test)]
mod tests_perf_pack; // the scene bench's `perf` data pack compiles with our command parser
#[cfg(test)]
mod tests_g4f08; // G4-F08: shulker push, TNT, sulfur cube, creaking, warden, smaller mob gaps
#[cfg(test)]
mod tests_g3f08; // G3-F08: compute, posteffect, fetchprofile, attribute reset, loot targets
#[cfg(test)]
mod tests_l10ent; // L10-ENT: pistons and shulkers on stands, mannequins, minecarts; item fields; raids per dimension
#[cfg(test)]
mod tests_l9combat; // L9-COMBAT: smash and wind reach, hurt trigger, stand attributes, summon, profile cache
#[cfg(test)]
mod tests_l10combat; // L10-COMBAT: blast reach and direct entity, decoration sizes, sweep, vector normalization
#[cfg(test)]
mod world_clock_tests; // world clocks: registry clocks, pace steps, save file, time packets, wake-up
#[cfg(test)]
mod data_reload_tests; // /reload: recipes, loot and tags read again, resent to the host and LAN guests
#[cfg(test)]
mod block_change_order_tests; // block packets of a tick: changed blocks before block events, light after its relight
#[cfg(test)]
mod other_dimension_send_tests; // `execute in`: player and server-wide packets leave, the other world's stay
#[cfg(test)]
mod per_dimension_tests; // forced chunks, world-limited selectors, `execute at`, global events per world
#[cfg(test)]
mod living_entity_rules_tests; // equipment reaction, conversion team, enemy test, ghast invulnerability
#[cfg(test)]
mod execute_across_worlds_tests; // `execute` steps find and read each entity in its own world

use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path} : {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("invalid format: {0}")]
    Format(String),
    #[error("{0}")]
    Refused(String),
}

impl Error {
    pub(crate) fn io(path: &Path, source: std::io::Error) -> Self {
        Error::Io { path: path.to_owned(), source }
    }
}

/// Height of a dimension from its type (version data), surface by default.
pub fn dimension_height(data: &rc_data::VersionData, dimension_type: &str) -> chunk::Height {
    let t = data.dimension_types.iter().find(|t| t.name == dimension_type);
    let (min_y, height) = t.map_or((-64, 384), |t| (t.min_y, t.height));
    chunk::Height { min_y, sections: (height / 16) as usize }
}

/// Ignition chances of a block (fire table, from its name): the fire animation on the
/// client reads the flammable sides there.
pub fn fire_ignite_odds(name: &str) -> u8 {
    blocks::table::fire_odds(name).0
}

/// The Nether portal frame rule a fire lit by flint and steel follows, shared with the client's
/// prediction (what a cell is for the frame search, the axis tried first, the rule itself).
pub use portal_shape::{
    Axis as PortalAxis, Cell as PortalCell, axis_across_facing as portal_axis_across_facing,
    frame_allows_fire as portal_frame_allows_fire,
};
