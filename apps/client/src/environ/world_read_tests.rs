//! Equivalence proofs of `RUSTCRAFT_OPT_BIOMEZOOMMEMO` and `RUSTCRAFT_OPT_LIGHTPAIRONCE`: over a
//! test terrain (biome checkerboard, light layers present, missing or holed, columns absent), the
//! biome after the blur zoom and the light reads give the same values on both paths, and their
//! fingerprint is the stored one.

use std::sync::Arc;

use super::cache_tests::{HEIGHT, MIN_Y, column, states};
use super::*;

/// FNV-1a 64-bit fingerprint of one value.
fn fnv(h: &mut u64, v: u32) {
    for b in v.to_le_bytes() {
        *h ^= u64::from(b);
        *h = h.wrapping_mul(0x100_0000_01b3);
    }
}

const FNV_START: u64 = 0xcbf2_9ce4_8422_2325;

/// Columns −2..=1 on both axes, biomes shifted per column; block light by a pattern with one
/// section in five missing; sky light missing on one section in three (read from the section
/// above) and on the whole column (1, 1).
fn terrain(d: &rc_data::VersionData) -> ChunkGrid {
    let s = states(d);
    let mut grid = ChunkGrid::new(4);
    for cx in -2..=1 {
        for cz in -2..=1 {
            let mut c = column(&s, cx, cz, (cx + cz).rem_euclid(4) as u32);
            let mut sky = c.sky_light.to_vec();
            let mut block = c.block_light.to_vec();
            for i in 0..sky.len() {
                let offset = (i as i32 * 31 + cx * 7 + cz * 13) as u8;
                let layer = |k: u8| {
                    Some(Arc::new(std::array::from_fn(|j| (j as u8).wrapping_mul(k).wrapping_add(offset))))
                };
                block[i] = if i % 5 == 4 { None } else { layer(37) };
                sky[i] = if i % 3 == 0 || (cx, cz) == (1, 1) { None } else { layer(91) };
            }
            c.sky_light = sky.into_boxed_slice();
            c.block_light = block.into_boxed_slice();
            grid.insert(c);
        }
    }
    grid
}

/// Blocks read: loaded columns and beyond (absent), below and above the world.
fn blocks() -> impl Iterator<Item = [i32; 3]> {
    (MIN_Y - 9..MIN_Y + HEIGHT + 9).step_by(5).flat_map(|y| {
        (-40..40).flat_map(move |z| (-40..40).map(move |x| [x, y, z]))
    })
}

/// Expected fingerprints of the direct paths (any change to the zoom or the light reads makes
/// them fail).
const BIOME_FINGERPRINT: u64 = 0xbabc_7326_1af0_5006;
const LIGHT_FINGERPRINT: u64 = 0xd39a_589d_26a8_f97d;

#[test]
fn zoomed_biome_same_with_and_without_memo() {
    let Some(d) = rc_testdata::version_data() else { return };
    let grid = terrain(&d);
    let w = WorldView { grid: &grid, min_y: MIN_Y, height: HEIGHT };
    let mut h = FNV_START;
    for seed in [0x5eed_1234_abcd, -77] {
        let direct: Vec<u32> = blocks().map(|b| w.zoomed_biome(false, seed, b, 99)).collect();
        // Forward then backward through the memo: entries filled, hit, then evicted by collisions.
        for (b, &want) in blocks().zip(&direct) {
            assert_eq!(w.zoomed_biome(true, seed, b, 99), want, "seed {seed}, block {b:?}");
        }
        let all: Vec<[i32; 3]> = blocks().collect();
        for (b, &want) in all.iter().zip(&direct).rev() {
            assert_eq!(w.zoomed_biome(true, seed, *b, 99), want, "seed {seed}, block {b:?} (backward)");
        }
        direct.iter().for_each(|&v| fnv(&mut h, v));
    }
    assert!((0..4).all(|b| blocks().any(|p| w.zoomed_biome(false, -77, p, 99) == b)), "biomes missing");
    assert_eq!(h, BIOME_FINGERPRINT, "biome fingerprint changed: {h:#x}");
}

#[test]
fn light_reads_same_with_one_column_lookup() {
    let Some(d) = rc_testdata::version_data() else { return };
    let grid = terrain(&d);
    let w = WorldView { grid: &grid, min_y: MIN_Y, height: HEIGHT };
    let mut h = FNV_START;
    for [x, y, z] in blocks() {
        let pair = w.light_pair_with(false, x, y, z);
        assert_eq!(w.light_pair_with(true, x, y, z), pair, "light pair at {x} {y} {z}");
        fnv(&mut h, u32::from(pair.0) << 8 | u32::from(pair.1));
        for (has_sky, darken) in [(true, 0), (true, 4), (true, 11), (false, 0), (false, 3)] {
            let level = w.raw_brightness_with(false, x, y, z, has_sky, darken);
            assert_eq!(w.raw_brightness_with(true, x, y, z, has_sky, darken), level, "brightness at {x} {y} {z}");
            fnv(&mut h, level as u32);
        }
    }
    assert_eq!(h, LIGHT_FINGERPRINT, "light fingerprint changed: {h:#x}");
}

/// Micro-bench (loaded machine): `cargo test --release -p rc-app --bin rustcraft
/// bench_main_thread_world_reads -- --ignored --nocapture`. The same 64 blocks read every frame
/// (moving blocks, rain columns): zoom recomputed against the memo; two column lookups against one.
#[test]
#[ignore]
fn bench_main_thread_world_reads() {
    use std::hint::black_box;
    use std::time::Instant;
    let Some(d) = rc_testdata::version_data() else { return };
    let grid = terrain(&d);
    let w = WorldView { grid: &grid, min_y: MIN_Y, height: HEIGHT };
    let moving: Vec<[i32; 3]> = (0..64).map(|i| [i * 3 % 50 - 25, 60 + i % 7, i * 5 % 50 - 25]).collect();
    const FRAMES: u32 = 20_000;
    for memo in [false, true, false, true] {
        let t = Instant::now();
        for _ in 0..FRAMES {
            for &b in &moving {
                black_box(w.zoomed_biome(memo, 0x5eed_1234_abcd, black_box(b), 0));
            }
        }
        let ns = t.elapsed().as_nanos() as f64 / f64::from(FRAMES * 64);
        println!("biome zoom memo={memo}: {ns:.1} ns per read");
    }
    for once in [false, true, false, true] {
        let t = Instant::now();
        for _ in 0..FRAMES {
            for &[x, y, z] in &moving {
                black_box(w.light_pair_with(once, black_box(x), y, z));
                black_box(w.raw_brightness_with(once, black_box(x), y, z, true, 2));
            }
        }
        let ns = t.elapsed().as_nanos() as f64 / f64::from(FRAMES * 64);
        println!("light pair + brightness once={once}: {ns:.1} ns per block");
    }
}
