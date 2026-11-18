//! Cold stage of the base cache (`rc_mesh::packed_codec`): every column of the reference capture,
//! flattened, compressed then decoded, gives back the same words; a partial remesh on a base decoded
//! on first read gives the stored fingerprint of the compact formats (`common::EXPECTED_COMPACT`).
//! Without data the tests are ignored (`RUSTCRAFT_OPT_REQUIRE=1`: failure).
//!
//! Bench (`cargo test --release -p rc-mesh --test packed_codec -- --ignored --nocapture`): ratio,
//! fluid share, compression and decoding per column against flattening and meshing, 1 thread up to
//! all threads.

mod common;

use std::sync::Arc;
use std::time::Instant;

use common::{Cols, EXPECTED_COMPACT, MIN_Y, job};
use rayon::prelude::*;
use rc_mesh::blocks::{BiomePalette, BlockMesh, Packed, Tables, digest, mesh, pack_exact};
use rc_mesh::packed_codec::{compress, decoded_on_first_read, decompress};
use rc_mesh::wire::Sections;

/// Same fingerprint as `tests/digest.rs`: order-free sum tied to each column's position.
fn total<'a>(meshes: impl Iterator<Item = &'a BlockMesh>) -> u64 {
    meshes.fold(0u64, |s, m| {
        let at = (u64::from(m.x as u32) << 32 | u64::from(m.z as u32)).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        s.wrapping_add(digest(m) ^ at)
    })
}

fn whole_meshes(cols: &Cols, t: &Tables, palette: &BiomePalette) -> (Vec<(i32, i32)>, Vec<BlockMesh>) {
    let mut keys: Vec<(i32, i32)> = cols.keys().copied().collect();
    keys.sort_unstable();
    let meshes = keys.par_iter().map(|&(x, z)| mesh(&job(cols, x, z), t, palette, MIN_Y)).collect();
    (keys, meshes)
}

#[test]
fn compressed_bases_give_back_the_same_meshes() {
    let Some((_d, cols, t, palette)) = common::load() else { return };
    let (keys, base) = whole_meshes(&cols, &t, &palette);
    assert_eq!(total(base.iter()), EXPECTED_COMPACT, "whole meshes changed: the proof below would say nothing");
    // Word for word round trip of every column.
    let flat: Vec<Vec<u32>> = base.par_iter().map(pack_exact).collect();
    let decoded: Vec<Vec<u32>> = flat.par_iter().map(|w| decompress(&compress(w)).expect("decodes")).collect();
    let differ = flat.iter().zip(&decoded).filter(|(a, b)| a != b).count();
    assert_eq!(differ, 0, "{differ} columns differ after the round trip");
    // Partial remesh (sections 4 to 10) on bases decoded on the pool: same fingerprint.
    let mut mask = Sections::default();
    (4..=10).for_each(|s| mask.insert(s));
    let reused: Vec<BlockMesh> = keys
        .par_iter()
        .zip(&flat)
        .map(|(&(x, z), w)| {
            let mut j = job(&cols, x, z);
            j.reuse = Some((mask, decoded_on_first_read(Arc::new(compress(w)))));
            mesh(&j, &t, &palette, MIN_Y)
        })
        .collect();
    assert_eq!(total(reused.iter()), EXPECTED_COMPACT, "partial remesh on a compressed base differs");
    // Sabotage: one word of a face the partial remesh keeps from its base (section 11 or above,
    // layer 0) changed in one decoded base is seen.
    let kept_face = |m: &BlockMesh| {
        let (n, starts) = (m.vis.len(), &m.starts[0]);
        (0..rc_mesh::blocks::BUCKETS).flat_map(|g| (11..n).map(move |s| g * (n + 1) + s)).find_map(|at| {
            let (a, e) = (*starts.get(at)?, *starts.get(at + 1)?);
            (e > a).then_some(a as usize)
        })
    };
    let (i, face) = base.iter().enumerate().find_map(|(i, m)| Some((i, kept_face(m)?))).expect("a kept face");
    let mut shifted = decoded[i].clone();
    // Words 0 to 2: x, z, sections; word 3: face count of layer 0; then 6 words per face.
    assert_eq!(shifted[3] as usize, base[i].layers[0].len(), "layout of the flat format changed");
    shifted[4 + 6 * face] ^= 1;
    let mut j = job(&cols, keys[i].0, keys[i].1);
    j.reuse = Some((mask, Arc::new(shifted) as Packed));
    let mut sabotaged = reused;
    sabotaged[i] = mesh(&j, &t, &palette, MIN_Y);
    assert_ne!(total(sabotaged.iter()), EXPECTED_COMPACT, "changed face of a base not seen");
}

/// Mean µs per column of `f` over the columns, on a pool of `threads` threads.
fn per_column<T: Sync, R: Send>(threads: usize, items: &[T], f: impl Fn(&T) -> R + Sync + Send) -> f64 {
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().expect("pool");
    pool.install(|| {
        // One warm-up pass, then the best of three.
        let _: Vec<R> = items.par_iter().map(&f).collect();
        (0..3)
            .map(|_| {
                let t = Instant::now();
                let r: Vec<R> = items.par_iter().map(&f).collect();
                std::hint::black_box(r);
                t.elapsed().as_secs_f64() * 1e6 / items.len() as f64
            })
            .fold(f64::INFINITY, f64::min)
    })
}

/// Deflate (fastest level) of `words` after a XOR with the word one face earlier (6 words): the
/// prefilter of the "custom codec" variant, measured to know whether plain deflate is enough.
fn xor_previous_face_ratio(words: &[u32]) -> usize {
    let filtered: Vec<u32> =
        (0..words.len()).map(|i| if i >= 6 { words[i] ^ words[i - 6] } else { words[i] }).collect();
    compress(&filtered).bytes_held()
}

/// Deflate size of `words` at `level`, as is or split into byte planes (all the low bytes, then
/// the second bytes…), to know what a stronger level or a prefilter would gain.
fn deflate_size(words: &[u32], level: i32, byte_planes: bool) -> usize {
    let bytes: Vec<u8> = if byte_planes {
        (0..4).flat_map(|b| words.iter().map(move |w| (w >> (8 * b)) as u8)).collect()
    } else {
        words.iter().flat_map(|w| w.to_le_bytes()).collect()
    };
    let mut c = libdeflater::Compressor::new(libdeflater::CompressionLvl::new(level).expect("level"));
    let mut out = vec![0; c.deflate_compress_bound(bytes.len())];
    c.deflate_compress(&bytes, &mut out).expect("bound")
}

#[test]
#[ignore]
fn cold_stage_bench() {
    let Some((_d, cols, t, palette)) = common::load() else { return };
    let (keys, base) = whole_meshes(&cols, &t, &palette);
    let flat: Vec<Vec<u32>> = base.iter().map(pack_exact).collect();
    let compressed: Vec<_> = flat.iter().map(|w| compress(w)).collect();
    let plain: usize = flat.iter().map(|w| w.len() * 4).sum();
    let packed: usize = compressed.iter().map(|c| c.bytes_held()).sum();
    let xor: usize = flat.iter().map(|w| xor_previous_face_ratio(w)).sum();
    let fluid_faces: usize = base.iter().map(|m| m.fluids.iter().map(Vec::len).sum::<usize>()).sum();
    let largest = flat.iter().map(Vec::len).max().unwrap_or(0) * 4;
    eprintln!(
        "{} columns: flat {:.1} MB (largest {} KB), compressed {:.1} MB (ratio {:.2}), xor of the previous face \
         {:.1} MB (ratio {:.2}), fluid faces {:.1} % of the flat bytes",
        keys.len(),
        plain as f64 / 1_048_576.0,
        largest / 1024,
        packed as f64 / 1_048_576.0,
        plain as f64 / packed as f64,
        xor as f64 / 1_048_576.0,
        plain as f64 / xor as f64,
        (fluid_faces * 27 * 4) as f64 * 100.0 / plain as f64,
    );
    for (level, planes) in [(1, false), (6, false), (12, false), (1, true), (6, true)] {
        let size: usize = flat.par_iter().map(|w| deflate_size(w, level, planes)).sum();
        eprintln!("deflate level {level}, byte planes {planes}: ratio {:.2}", plain as f64 / size as f64);
    }
    let jobs: Vec<_> = keys.iter().map(|&(x, z)| job(&cols, x, z)).collect();
    let all = std::thread::available_parallelism().map_or(1, usize::from);
    let mut threads = 1;
    loop {
        let pack = per_column(threads, &base, pack_exact);
        let enc = per_column(threads, &flat, |w| compress(w));
        let dec = per_column(threads, &compressed, decompress);
        let whole = per_column(threads, &jobs, |j| mesh(j, &t, &palette, MIN_Y));
        eprintln!(
            "{threads:>2} threads, µs per column: flatten {pack:.1}, compress {enc:.1} ({:.1}× flatten), decode \
             {dec:.1} ({:.1}× flatten), whole mesh {whole:.0} ({:.0}× decode)",
            enc / pack,
            dec / pack,
            whole / dec
        );
        if threads >= all {
            break;
        }
        threads = (threads * 2).min(all);
    }
}
