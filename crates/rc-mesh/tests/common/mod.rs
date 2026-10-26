//! Real data shared by the meshing tests and benches: columns of the reference capture
//! (outside the repository), models and tints of the installed game, tasks set up as in game (bottom of
//! the world −64, biome zoom seed of the capture, plains as the biome of absent columns).
//!
//! `load()` returns `None` (test ignored) if a piece of data is missing, or fails under
//! `RUSTCRAFT_OPT_REQUIRE=1`.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use rc_mesh::blocks::{BiomePalette, BlockMesh, Tables};
use rc_mesh::wire::MeshJob;
use rc_proto::Reader;
use rc_proto_v777::chunk::{ChunkContext, read_chunk};
use rc_world::Column;

pub type Cols = HashMap<(i32, i32), Arc<Column>>;

/// Bottom of the normal world.
pub const MIN_Y: i32 = -64;
/// Hashed seed of the capture's dimension (login packet).
pub const ZOOM_SEED: i64 = 293_737_985_876_514_017;
/// Fingerprint of the compact formats over the whole capture, default settings (default since
/// C3-M03): single source of `tests/digest.rs` and `tests/light_paths.rs`. Replaced by G2-F12
/// (canonical vertex order of the turned faces, `bake.rs`).
pub const EXPECTED_COMPACT: u64 = 0xed99_c527_6a39_023e;

/// Index of `minecraft:plains` in the data's biomes, set by `load()` (the game puts it
/// in each task: biome of absent columns).
static PLAINS: OnceLock<u32> = OnceLock::new();
/// Fingerprint (size, FNV-1a by 8-byte words) of the capture loaded by `load()`.
static CAPTURE: OnceLock<(usize, u64)> = OnceLock::new();

pub fn plains() -> u32 {
    *PLAINS.get().expect("common::load() first")
}

pub fn capture_fingerprint() -> (usize, u64) {
    *CAPTURE.get().expect("common::load() first")
}

pub fn load() -> Option<(rc_data::VersionData, Cols, Tables, BiomePalette)> {
    let (cap, d) = (rc_testdata::reference_capture()?, rc_testdata::version_data()?);
    let fnv = cap.chunks(8).fold(0xcbf2_9ce4_8422_2325u64, |h, w| {
        let mut b = [0u8; 8];
        b[..w.len()].copy_from_slice(w);
        (h ^ u64::from_le_bytes(b)).wrapping_mul(0x100_0000_01b3)
    });
    CAPTURE.get_or_init(|| (cap.len(), fnv));
    let plains = d.biomes.iter().position(|b| b.name == "minecraft:plains").expect("plains in the biomes");
    PLAINS.get_or_init(|| plains as u32);
    let ctx = ChunkContext::new(24, d.states.len() as u32, 67);
    let mut cols: Cols = HashMap::new();
    for p in rc_testdata::play_packets(&cap, &d, "minecraft:level_chunk_with_light") {
        let c = read_chunk(&mut Reader::new(&p), &ctx).expect("chunk of the capture");
        cols.insert(
            (c.x, c.z),
            Arc::new(Column {
                x: c.x,
                z: c.z,
                sections: c.sections.into_boxed_slice(),
                sky_light: c.light.sky.into_boxed_slice(),
                block_light: c.light.block.into_boxed_slice(),
                heightmaps: c.heightmaps,
            }),
        );
    }
    assert!(!cols.is_empty(), "capture without chunks");
    let install = rc_testdata::need(rc_data::locate("26.3").ok(), "installed 26.3 game")?;
    let assets =
        rc_testdata::need(rc_assets::Assets::for_install(&install.client_jar, &[], None).ok(), "client jar")?;
    let models = Arc::new(rc_assets::build(&assets, &d));
    let maps = rc_assets::texture::Colormaps::load(&assets);
    let names: Vec<String> = d.biomes.iter().map(|b| b.name.clone()).collect();
    let palette = rc_mesh::blocks::biome_palette(&d, &maps, &names);
    let mut t = Tables::new(&d, models);
    // The tests and benches force all the light paths, face geometry included.
    t.build_light_faces();
    Some((d, cols, t, palette))
}

/// Task for column (`x`, `z`) with the game's settings.
pub fn job(cols: &Cols, x: i32, z: i32) -> MeshJob {
    let mut j = MeshJob::gather(cols[&(x, z)].clone(), 0, |x, z| cols.get(&(x, z)).cloned());
    j.biome_zoom_seed = ZOOM_SEED;
    j.min_y = MIN_Y;
    j.plains_biome = plains();
    // Stored fingerprints (`EXPECTED_COMPACT`, `tests/digest.rs`): old face removal
    // rule; those of the game rule (C2-M10) are in `tests/cull_paths.rs`.
    j.cull.shape = false;
    j
}

/// Task for column (`x`, `z`) with the game's settings and the `cull` paths.
pub fn job_with(cols: &Cols, x: i32, z: i32, cull: rc_mesh::cull::CullPaths) -> MeshJob {
    let mut j = job(cols, x, z);
    j.cull = cull;
    j
}

pub fn same(a: &BlockMesh, b: &BlockMesh) -> bool {
    a.layers == b.layers
        && a.fluids == b.fluids
        && a.starts == b.starts
        && a.fluid_starts == b.fluid_starts
        && a.vis == b.vis
}
