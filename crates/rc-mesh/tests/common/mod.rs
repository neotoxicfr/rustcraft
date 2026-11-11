//! Real data shared by the meshing tests and benches: columns of the reference capture
//! (outside the repository), models and tints of the installed game, tasks set up as in game (bottom of
//! the world −64, biome zoom seed of the capture, plains as the biome of absent columns).
//!
//! `load()` returns `None` (test ignored) if a piece of data is missing, or fails under
//! `RUSTCRAFT_OPT_REQUIRE=1`.
#![allow(dead_code)]

use hashbrown::HashMap;
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
/// (canonical vertex order of the turned faces, `bake.rs`), then by C7-M05 (fluid flow angle widened
/// before the sine table product, as the game does; with the old single-precision product every
/// previous value passed again).
pub const EXPECTED_COMPACT: u64 = 0xac72_bdc3_1c59_1906;

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
    let t = Tables::new(&d, models);
    // The tests and benches force all the light paths, face geometry included (built now, or
    // waited for when its switch has it built in the background).
    t.build_light_faces();
    Some((d, cols, t, palette))
}

/// Solid-cell masks of the sections of one column of the capture (`None`: empty section).
pub type ColumnMasks = ((i32, i32), Vec<Option<[u64; 64]>>);

/// Solid-cell masks of the capture's sections, column by column in packet order, from the capture
/// and the data cache only (no installed game needed).
pub fn capture_solid_masks() -> Option<Vec<ColumnMasks>> {
    let (cap, d) = (rc_testdata::reference_capture()?, rc_testdata::version_data()?);
    let solid: Vec<bool> = d.states.iter().map(|s| s.has(rc_data::flags::SOLID_RENDER)).collect();
    let ctx = ChunkContext::new(24, d.states.len() as u32, 67);
    let mut values = vec![0u32; 4096];
    let mut out = Vec::new();
    for p in rc_testdata::play_packets(&cap, &d, "minecraft:level_chunk_with_light") {
        let c = read_chunk(&mut Reader::new(&p), &ctx).expect("chunk of the capture");
        let masks = c
            .sections
            .iter()
            .map(|s| {
                (!s.is_empty()).then(|| {
                    s.blocks.unpack_into(&mut values);
                    rc_mesh::visibility::solid_bits(&values, &solid)
                })
            })
            .collect();
        out.push(((c.x, c.z), masks));
    }
    Some(out)
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

/// First state of block `name` whose properties contain all those of `props`.
pub fn state(d: &rc_data::VersionData, name: &str, props: &[(&str, &str)]) -> u32 {
    let b = d.blocks.iter().position(|b| b.name == name).unwrap_or_else(|| panic!("block {name}")) as u16;
    (0..d.states.len() as u32)
        .find(|&s| {
            d.states[s as usize].block == b && props.iter().all(|p| d.state_properties(s).any(|q| q == *p))
        })
        .unwrap_or_else(|| panic!("state {name} {props:?}"))
}

/// Column of the capture closest to its middle whose eight neighbours are in the capture too.
pub fn surrounded_column(cols: &Cols) -> (i32, i32) {
    let mut keys: Vec<(i32, i32)> = cols.keys().copied().collect();
    keys.sort_unstable();
    let surrounded = |&(x, z): &(i32, i32)| (-1..=1).all(|dz| (-1..=1).all(|dx| cols.contains_key(&(x + dx, z + dz))));
    let middle = keys.len() / 2;
    keys[middle..].iter().chain(&keys[..middle]).copied().find(surrounded).expect("a column with its eight neighbours")
}

/// Copy of `cols` where the `sections` of the column `key` and of its eight neighbours are
/// rewritten: each cell takes the next state of `next_state` (columns north-west to south-east,
/// sections upwards, cells in storage order y, z, x).
pub fn with_sections_rewritten(
    d: &rc_data::VersionData,
    cols: &Cols,
    key: (i32, i32),
    sections: std::ops::Range<usize>,
    next_state: &mut dyn FnMut() -> u32,
) -> Cols {
    let bits = (u32::BITS - (d.states.len() as u32 - 1).leading_zeros()) as u8;
    let is_air = |s: u32| d.states.get(s as usize).is_none_or(|state| state.has(rc_data::flags::AIR));
    let mut rewritten = cols.clone();
    for dz in -1..=1 {
        for dx in -1..=1 {
            let at = (key.0 + dx, key.1 + dz);
            let mut column = (*cols[&at]).clone();
            for section in sections.clone() {
                let changes: Vec<(u16, u32)> = (0..4096).map(|cell| (cell, next_state())).collect();
                column.sections[section].set_blocks(&changes, bits, is_air);
            }
            rewritten.insert(at, Arc::new(column));
        }
    }
    rewritten
}

/// Source of random states for [`with_sections_rewritten`]: about a third air, a third water of
/// every level (sources, flowing, falling), some lava, and blocks that hide fluid faces fully or in
/// part (stone, glass, ice, slabs, a waterlogged stair). Same sequence for the same `seed` (odd:
/// its lowest bit is set).
pub fn random_fluid_states(d: &rc_data::VersionData, seed: u64) -> impl FnMut() -> u32 {
    let level_state = |name: &str, level: u32| state(d, name, &[("level", level.to_string().as_str())]);
    let levels = |name: &str| -> Vec<u32> { (0..16).map(|level| level_state(name, level)).collect() };
    let (water, lava) = (levels("minecraft:water"), levels("minecraft:lava"));
    let stairs = [("facing", "north"), ("half", "bottom"), ("shape", "straight"), ("waterlogged", "true")];
    let blocks = [
        state(d, "minecraft:stone", &[]),
        state(d, "minecraft:glass", &[]),
        state(d, "minecraft:ice", &[]),
        state(d, "minecraft:smooth_stone_slab", &[("type", "bottom"), ("waterlogged", "false")]),
        state(d, "minecraft:smooth_stone_slab", &[("type", "top"), ("waterlogged", "true")]),
        state(d, "minecraft:oak_stairs", &stairs),
    ];
    let mut word = seed | 1;
    move || {
        word ^= word << 13;
        word ^= word >> 7;
        word ^= word << 17;
        let (kind, pick) = ((word >> 32) % 100, (word >> 48) as usize);
        match kind {
            0..35 => 0,
            // Half of the water is a source, so that still surfaces and slopes both appear.
            35..52 => water[0],
            52..70 => water[pick % 16],
            70..78 => lava[pick % 16],
            _ => blocks[pick % blocks.len()],
        }
    }
}

pub fn same(a: &BlockMesh, b: &BlockMesh) -> bool {
    a.layers == b.layers
        && a.fluids == b.fluids
        && a.starts == b.starts
        && a.fluid_starts == b.fluid_starts
        && a.vis == b.vis
}

/// Gaps (faces, fluids, translucents) tried by [`check_in_place`]: one range, runs touching, small
/// and large gaps.
pub const IN_PLACE_GAPS: [Option<[u32; 3]>; 4] = [None, Some([0; 3]), Some([4, 1, 1]), Some([170, 10, 10])];

/// Words of a fluid face as the flat mesh stores them.
pub fn fluid_words(q: &rc_mesh::fluid::FluidQuad) -> [u32; 27] {
    let mut w = Vec::new();
    w.extend(q.corners.iter().flatten().map(|c| c.to_bits()));
    w.push(q.texture);
    w.extend(q.colors);
    w.push(u32::from(q.light[0]) | u32::from(q.light[1]) << 16);
    w.push(u32::from(q.light[2]) | u32::from(q.light[3]) << 16);
    w.try_into().expect("27 words")
}

/// Simulated GPU buffer: other columns before and after (known pattern) and the old elements,
/// then the announced ranges alone written from the new ones; must equal a full upload.
pub fn apply_ranges<T: Copy + PartialEq + std::fmt::Debug>(old: &[T], new: &[T], ranges: &[std::ops::Range<u32>], pad: T) {
    let around = |v: &[T]| -> Vec<T> {
        std::iter::repeat_n(pad, 7).chain(v.iter().copied()).chain(std::iter::repeat_n(pad, 5)).collect()
    };
    let mut gpu = around(old);
    for s in ranges {
        let s = s.start as usize..s.end as usize;
        gpu[7 + s.start..7 + s.end].copy_from_slice(&new[s]);
    }
    assert_eq!(gpu, around(new), "ranges {ranges:?}: buffer differs from a full upload");
    // Each range starts and ends on a changed element (nothing sent for nothing at the ends).
    for s in ranges {
        assert!(old[s.start as usize] != new[s.start as usize] && old[s.end as usize - 1] != new[s.end as usize - 1]);
    }
}

/// In-place write from the base `old` to `new` checked on every path: for each gap of
/// [`IN_PLACE_GAPS`], the whole comparison and the comparison of the `remeshed` sections only give
/// the same ranges, which applied to a simulated buffer give a full upload; without gaps they are the
/// old diff's. `None` if the counts changed (new place, checked as such by every path), otherwise
/// the number of ranges of the smallest gap.
pub fn check_in_place(old: &BlockMesh, new: &BlockMesh, remeshed: Option<rc_mesh::wire::Sections>) -> Option<usize> {
    use rc_mesh::blocks::{InPlacePaths, in_place_diff, in_place_diff_with, pack_exact};
    let base = pack_exact(old);
    let faces = |m: &BlockMesh| m.layers.iter().flatten().copied().collect::<Vec<_>>();
    let fluids = |m: &BlockMesh, ls: std::ops::Range<usize>| {
        ls.flat_map(|l| m.fluids[l].iter().map(fluid_words)).collect::<Vec<_>>()
    };
    let old_diff = in_place_diff(&base, new);
    let mut writes = None;
    for gaps in IN_PLACE_GAPS {
        let whole = in_place_diff_with(&base, new, None, InPlacePaths { touched_only: false, gaps });
        let touched = in_place_diff_with(&base, new, remeshed, InPlacePaths { touched_only: true, gaps });
        assert_eq!(touched, whole, "gaps {gaps:?}: touched-only comparison differs from the whole one");
        let Some(d) = whole else {
            assert!(old_diff.is_none(), "gaps {gaps:?}: in-place write refused, the old diff accepts it");
            continue;
        };
        if gaps.is_none() {
            assert_eq!(Some(&d), old_diff.as_ref(), "single range differs from the old diff");
        }
        apply_ranges(&faces(old), &faces(new), &d.faces, [0xdead; 6]);
        apply_ranges(&fluids(old, 0..2), &fluids(new, 0..2), &d.fluids, [0xbeef; 27]);
        apply_ranges(&fluids(old, 2..3), &fluids(new, 2..3), &d.trans, [0xcafe; 27]);
        if gaps == Some([0; 3]) {
            writes = Some(d.faces.len() + d.fluids.len() + d.trans.len());
        }
    }
    writes
}
