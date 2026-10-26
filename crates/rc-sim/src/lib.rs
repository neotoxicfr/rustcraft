//! Rust singleplayer (`docs/architecture.md` § 14): server integrated into the client, in the same process.
//! Reading and writing worlds in the official game's format (`docs/specs/anvil-world.md`),
//! tick loop and player (`docs/specs/integrated-server.md`). No graphics dependency.

// Test scripts read their settings directly (at launch, off the hot path). The game
// code stays checked: the clippy pass without `cfg(test)` forbids any read outside rc_data::env_*!.
#![cfg_attr(test, allow(clippy::disallowed_methods))]

mod blocks;
mod fluid_use;
pub mod chunk;
pub mod demo;
pub mod generation;
pub mod generation_nbt;
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
mod solo_packets;
// Survival loop (milestone 2, S2).
pub mod book;
pub mod containers;
pub mod datapack;
pub mod packs;
pub mod drops;
pub mod life;
pub mod items;
pub mod item_use;
pub mod loot;
pub mod menus;
pub mod netstack;
pub mod recipes;
pub mod rng;
pub mod vitals;
pub mod weather;
pub mod tickrate;
// Mobs (S4).
pub mod mobs;
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
mod tests_g4f09; // G4-F09: turtle helmet, 1.21 effects, hurt sound, statistics
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

pub use blocks::piston_plan::{MovingBlock, PistonReplay, PistonWorld, ReplayOp, ReplayWorld};
pub use blocks::redstone::{button_sound, openable_sound_name};
pub use server::{Handle, Options, start};

/// Tests on the game's allocator (mimalloc, that of `rc-app`). With Windows' system heap,
/// the simultaneous server openings of the suite (unpacked jar, generation) stalled the
/// whole process for tens of seconds, processor almost idle.
#[cfg(test)]
#[global_allocator]
static TEST_ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

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
