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

/// Unit tests on the game's allocator (mimalloc, that of `rc-app`) behind an allocation counter:
/// a bench of this crate then times and counts what the game does.
#[cfg(test)]
#[global_allocator]
static TEST_ALLOCATOR: test_alloc::Counting = test_alloc::Counting;

/// The tests' allocator counts the allocations of each thread (each test has its own).
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
