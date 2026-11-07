//! Rust singleplayer (`docs/architecture.md` § 14): server integrated into the client, in the same process.
//! Reading and writing worlds in the official game's format (`docs/specs/anvil-world.md`),
//! tick loop and player (`docs/specs/integrated-server.md`). No graphics dependency.

// Test scripts read their settings directly (at launch, off the hot path). The game
// code stays checked: the clippy pass without `cfg(test)` forbids any read outside rc_data::env_*!.
#![cfg_attr(test, allow(clippy::disallowed_methods))]

mod blocks;
mod fluid_use;
pub mod chunk;
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
pub mod tickrate;
// Mobs (S4).
pub mod mobs;
mod walk; // L10-ORDER: one entity walk for every family, in list order
pub(crate) mod decoration;
// Dimensions and portals (S6).
pub(crate) mod dims;
mod portal;
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
#[cfg(test)]
mod tests_g5f04;
#[cfg(test)]
mod tests_damage_tag_names; // short damage tag and damage type names, read in `minecraft`
pub mod randsrc;
// Commands (S5).
pub mod commands;
// Advancements and statistics (S9).
pub mod progress;
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
    }

    /// Allocations made by the current thread so far.
    pub(crate) fn allocations() -> u64 {
        ALLOCATIONS.with(Cell::get)
    }

    pub(crate) struct Counting;

    // Delegates to mimalloc; the counter is a const-initialised thread-local (no allocation,
    // no destructor), safe to touch from inside the allocator.
    #[allow(unsafe_code)]
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let _ = ALLOCATIONS.try_with(|c| c.set(c.get() + 1));
            // SAFETY: same contract as the caller's, passed on unchanged.
            unsafe { mimalloc::MiMalloc.alloc(layout) }
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            let _ = ALLOCATIONS.try_with(|c| c.set(c.get() + 1));
            // SAFETY: same contract as the caller's, passed on unchanged.
            unsafe { mimalloc::MiMalloc.alloc_zeroed(layout) }
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            let _ = ALLOCATIONS.try_with(|c| c.set(c.get() + 1));
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
mod portal_tests;
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
mod bench_kit; // common kit of the micro-benches: flat world, perf pack, timed ticks, `[bench]` lines
#[cfg(test)]
mod bench_datapack_tick; // tick of the bench's `datapack` scene (crowd, #tick workload, command blocks)
#[cfg(test)]
mod bench_path_search; // mob path search on the village of the bench model world, each lead alone
#[cfg(test)]
mod bench_scores; // scoreboard switches: writes over every holder, save, entity removals, team lookups
#[cfg(test)]
mod bench_function_tick; // command switches: functions per tick, macros, command blocks, schedule, parse
mod bench_spawner_blocks; // trial spawners, vaults, monster spawners, natural spawn pass, structure lists
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
