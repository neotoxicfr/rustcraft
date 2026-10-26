//! Terrain generation, part 1 (spec `docs/specs/generation-noise.md`): randomness, noises,
//! density functions, biomes. Bit-for-bit reproduction of the official game; the data
//! (noises, functions, settings, parameter lists) are read from the jar at run time.

pub mod aquifer;
pub mod biome;
pub mod carver;
pub mod chunkgen;
pub mod data;
pub mod decor;
pub mod decor_blocks;
pub mod decor_data;
pub mod decor_pred;
pub mod decor_region;
pub mod decor_rng;
pub mod decor_rules;
pub mod density;
pub mod feature;
pub mod feature_basic;
pub mod feature_cave;
pub mod feature_end;
pub mod feature_geode;
pub mod feature_ice;
pub mod feature_nether;
pub mod feature_sculk;
pub mod feature_templ;
pub mod feature_veg;
pub mod generator;
pub mod jigsaw;
pub mod jmath;
pub mod jset;
pub mod json;
pub mod material;
pub mod math;
pub mod noise;
pub mod opt;
pub mod palette;
pub mod placement;
pub mod proto;
pub mod rng;
pub mod sampler;
pub mod shape_update;
pub mod structure;
pub mod structure_beard;
pub mod structure_coded;
pub mod structure_endcity;
pub mod structure_entities;
pub mod structure_fortress;
pub mod structure_mansion;
pub mod structure_mineshaft;
pub mod structure_monument;
pub mod structure_nbt;
pub mod structure_piece;
pub mod structure_place;
pub mod structure_procs;
pub mod structure_scattered;
pub mod structure_stronghold;
pub mod structure_templ;
pub mod template;
pub mod terrain;
pub mod tree;
pub mod volume;

#[cfg(test)]
mod jar_share_tests;
#[cfg(test)]
mod name_lookup_tests;
#[cfg(test)]
mod vine_rules_tests;

pub use generator::{Generator, NoiseSettings};
pub use sampler::Ctx;
pub use terrain::{BiomeCache, TerrainGenerator};
pub use volume::Volume;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("missing data: {0}")]
    Missing(String),
    #[error("format : {0}")]
    Format(String),
    #[error("{0} : {1}")]
    Io(String, std::io::Error),
}
