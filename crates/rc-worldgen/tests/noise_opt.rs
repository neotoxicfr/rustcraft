//! Bit-for-bit equivalence of the optimised noise paths (`crate::opt`) against the old code, and
//! micro-benches (`cargo test --release -p rc-worldgen --test noise_opt -- --ignored --nocapture`).
//! Each checker is also run on a sabotaged version, which must fail.

mod common;

use std::time::Instant;

use rayon::prelude::*;
use rc_data::java_math::floor_to_i32;
use rc_worldgen::jmath;
use rc_worldgen::math::floor_i_libm;
use rc_worldgen::noise::{Layer, NoiseStack, Octave, Paths, Perlin, Smeared};
use rc_worldgen::rng::Rng;
use rc_worldgen::volume::Volume;

/// Small generator for the test draws (splitmix64).
struct Mix(u64);

impl Mix {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }
}

// ---------------------------------------------------------------------------------------------
// floor: its equivalence proofs moved with the shared code (`rc-data`, `java_math` tests).

// ---------------------------------------------------------------------------------------------
// Noise: volumes

const VARIANTS: [Paths; 15] = {
    const fn p(floor_java: bool, perm512: bool, ypre: bool, colconst: bool, simd: bool, avx2: bool) -> Paths {
        Paths { floor_java, perm512, ypre, colconst, simd, avx2, ..Paths::OLD }
    }
    /// The work in x once per octave and the table of the permutations in x and y on top of `p`.
    const fn x_and_xy(p: Paths, x_precomputed: bool, xy_perm_table: bool) -> Paths {
        Paths { x_precomputed, xy_perm_table, ..p }
    }
    let precomputed = p(false, false, true, false, false, false);
    let all_but_avx2 = p(true, true, true, true, true, false);
    [
        p(true, false, false, false, false, false),
        p(false, true, false, false, false, false),
        precomputed,
        p(false, false, true, true, false, false),
        p(false, false, true, false, true, false),
        p(false, false, true, false, true, true),
        p(true, true, true, true, false, false),
        all_but_avx2,
        p(true, true, true, true, true, true),
        x_and_xy(precomputed, true, false),
        x_and_xy(precomputed, false, true),
        x_and_xy(p(false, true, true, true, false, false), true, true),
        x_and_xy(all_but_avx2, true, false),
        x_and_xy(all_but_avx2, false, true),
        x_and_xy(p(true, true, true, true, true, true), true, true),
    ]
};

fn variants() -> Vec<Paths> {
    let avx2 = rc_data::cpu::has_avx2();
    VARIANTS.iter().copied().filter(|p| avx2 || !(p.avx2 || p.batch_avx2)).collect()
}

/// Every combination of the switches the volume path reads (2⁸, those with AVX2 only if the
/// processor has it).
fn all_volume_paths() -> Vec<Paths> {
    let avx2 = rc_data::cpu::has_avx2();
    let bit = |b: u32, k: u32| b & (1 << k) != 0;
    (0..1u32 << 8)
        .map(|b| Paths {
            floor_java: bit(b, 0),
            perm512: bit(b, 1),
            ypre: bit(b, 2),
            colconst: bit(b, 3),
            simd: bit(b, 4),
            avx2: bit(b, 5),
            x_precomputed: bit(b, 6),
            xy_perm_table: bit(b, 7),
            batch_avx2: false,
            batch_shared_cell: false,
        })
        .filter(|p| avx2 || !p.avx2)
        .collect()
}

fn random_stack(r: &mut Mix) -> NoiseStack {
    let mut rng = Rng::xoroshiro(r.next() as i64);
    let n = 1 + r.below(3) as usize;
    let layers = (0..n)
        .map(|_| {
            let noise = if r.below(3) == 0 {
                let fudge = [0.0, 1.0, 684.412 * 0.125, -3.0, 1e-3][r.below(5) as usize];
                Octave::Smeared(Smeared::new(&mut rng, fudge))
            } else {
                Octave::Perlin(Perlin::new(&mut rng))
            };
            let freq = [1.0, 0.5, 2.0, 1.0 / 128.0, 0.018_126_888_217_522_7][r.below(5) as usize];
            Layer { noise, freq, amp: r.range(-2.0, 2.0) as f32 }
        })
        .collect();
    NoiseStack { layers }
}

struct Case {
    v: Volume,
    xz: f64,
    ys: f64,
}

fn random_case(r: &mut Mix, k: u64) -> Case {
    let near = |r: &mut Mix| -> i32 {
        match r.below(6) {
            0 => r.below(2000) as i32 - 1000,
            1 => 16_777_216 - 50 + r.below(100) as i32,
            2 => -16_777_216 - 50 + r.below(100) as i32,
            3 => 30_000_000 - r.below(1000) as i32,
            4 => -30_000_000 + r.below(1000) as i32,
            _ => r.below(200_000) as i32 - 100_000,
        }
    };
    let sy = match k % 5 {
        0 => 1,
        1 => 1 + r.below(8) as i32,
        2 => 384,
        3 => 600,
        _ => 1 + r.below(64) as i32,
    };
    let (sx, sz) = (1 + r.below(5) as i32, 1 + r.below(5) as i32);
    let st = |r: &mut Mix| [1, 1, 2, 4, 8][r.below(5) as usize];
    let (stx, sty, stz) = (st(r), st(r), st(r));
    let v = Volume::stepped(sx, sy, sz, near(r), r.below(800) as i32 - 400, near(r), stx, sty, stz);
    let xz = [1.0, 0.25, 684.412 / 80.0, 1.0 / 3.0, 7.5, 1e-3][r.below(6) as usize];
    let ys = match r.below(7) {
        0 => 0.0,
        1 => -0.25,
        2 => 1.0 / 160.0,
        3 => 684.412 / 160.0,
        4 => 1e-9,
        5 => 3.0,
        _ => 0.25,
    };
    Case { v, xz, ys }
}

/// First case (number) where a path differs from the old one, bit for bit over the whole buffer.
fn check_volumes(n: u64, paths: &[Paths], f: impl Fn(&NoiseStack, &mut [f32], &Case, Paths) + Sync) -> Option<String> {
    (0..n).into_par_iter().find_map_any(|k| {
        let mut r = Mix(k ^ 0xA5A5_0000);
        let stack = random_stack(&mut r);
        let c = random_case(&mut r, k);
        let init: Vec<f32> = (0..c.v.size()).map(|_| r.range(-1.0, 1.0) as f32).collect();
        let mut want = init.clone();
        stack.add_to_volume_with(&mut want, &c.v, c.xz, c.ys, 1.0, Paths::OLD);
        for &p in paths {
            let mut got = init.clone();
            f(&stack, &mut got, &c, p);
            if let Some(i) = (0..got.len()).find(|&i| got[i].to_bits() != want[i].to_bits()) {
                return Some(format!("case {k} {p:?} {:?} xz {} ys {} index {i}", c.v, c.xz, c.ys));
            }
        }
        None
    })
}

#[test]
fn noise_volume_paths_match_old_bitwise() {
    let ok = |s: &NoiseStack, b: &mut [f32], c: &Case, p: Paths| s.add_to_volume_with(b, &c.v, c.xz, c.ys, 1.0, p);
    assert_eq!(check_volumes(1000, &variants(), ok), None);
    // The constant columns must really be used (zero y scale, a single row).
    let with_const = (0..1000u64).filter(|&k| {
        let mut r = Mix(k ^ 0xA5A5_0000);
        let _ = random_stack(&mut r);
        let c = random_case(&mut r, k);
        c.ys == 0.0 || c.v.sy == 1
    });
    assert!(with_const.count() > 100);
}

#[test]
fn noise_volume_checker_detects_sabotage() {
    // Sabotage: a value shifted by one unit of the last bit at one place of the buffer.
    let bad = |s: &NoiseStack, b: &mut [f32], c: &Case, p: Paths| {
        s.add_to_volume_with(b, &c.v, c.xz, c.ys, 1.0, p);
        let i = b.len() / 2;
        b[i] = f32::from_bits(b[i].to_bits() ^ 1);
    };
    assert!(check_volumes(20, &variants(), bad).is_some());
}

/// Every combination of the volume switches against the old path, bit for bit (fewer cases than
/// the chosen variants above: 2⁸ paths).
#[test]
fn noise_volume_every_switch_combination_matches_old() {
    let ok = |s: &NoiseStack, b: &mut [f32], c: &Case, p: Paths| s.add_to_volume_with(b, &c.v, c.xz, c.ys, 1.0, p);
    assert_eq!(check_volumes(150, &all_volume_paths(), ok), None);
}

/// Sabotage of the work in x: each column computed with the position in x of its neighbour (as a
/// precomputed column read at the wrong index would), on the paths with the new switches.
#[test]
fn noise_volume_checker_detects_neighbour_column_x() {
    let paths: Vec<Paths> = variants().into_iter().filter(|p| p.x_precomputed || p.xy_perm_table).collect();
    assert!(!paths.is_empty());
    let neighbour_x = |s: &NoiseStack, b: &mut [f32], c: &Case, p: Paths| {
        let v = c.v;
        let shifted = Volume::stepped(v.sx, v.sy, v.sz, v.x0 + v.stx, v.y0, v.z0, v.stx, v.sty, v.stz);
        s.add_to_volume_with(b, &shifted, c.xz, c.ys, 1.0, p);
    };
    assert!(check_volumes(20, &paths, neighbour_x).is_some(), "sabotage not detected");
}

// ---------------------------------------------------------------------------------------------
// Noise: point path

fn point(r: &mut Mix) -> f64 {
    match r.below(8) {
        0 => r.range(-1e3, 1e3),
        1 => r.range(-3e7, 3e7),
        2 => 16_777_216.0 + r.range(-2.0, 2.0),
        3 => -16_777_216.0 + r.range(-2.0, 2.0),
        4 => r.range(-1e12, 1e12),
        5 => (r.below(4000) as f64 - 2000.0) * 0.25,
        6 => [0.0, -0.0, 16_777_215.999_999_998, -16_777_215.999_999_998, 1e17, -1e17][r.below(6) as usize],
        _ => r.range(-1.0, 1.0),
    }
}

fn check_points(n: u64, paths: &[Paths], f: impl Fn(&NoiseStack, f64, f64, f64, Paths) -> f32 + Sync) -> Option<String> {
    (0..n / 10_000).into_par_iter().find_map_any(|k| {
        let mut r = Mix(k ^ 0x5EED_0000);
        let stack = random_stack(&mut r);
        for _ in 0..10_000 {
            let (x, y, z) = (point(&mut r), point(&mut r), point(&mut r));
            let want = stack.get_with(x, y, z, Paths::OLD).to_bits();
            for &p in paths {
                if f(&stack, x, y, z, p).to_bits() != want {
                    return Some(format!("batch {k} {p:?} ({x}, {y}, {z})"));
                }
            }
        }
        None
    })
}

#[test]
fn noise_point_paths_match_old_bitwise() {
    let paths: Vec<Paths> = variants().into_iter().filter(|p| p.floor_java || p.perm512).collect();
    assert_eq!(check_points(10_000_000, &paths, |s, x, y, z, p| s.get_with(x, y, z, p)), None);
    let bad = |s: &NoiseStack, x: f64, y: f64, z: f64, p: Paths| s.get_with(x, y, z.next_up(), p);
    assert!(check_points(100_000, &paths, bad).is_some(), "sabotage not detected");
}

// ---------------------------------------------------------------------------------------------
// Noise: batches of points

/// Coordinate of a batch: mostly where the AVX2 groups take the registers, now and then far
/// (outside ±2³¹ after the scale), at the edge of the wrap, infinite or NaN, so that both the
/// registers and the fallback of a group run.
fn batch_coordinate(r: &mut Mix) -> f64 {
    match r.below(1000) {
        0 => [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 2_147_483_648.0, -2_147_483_649.5, 1e12, -3e9][r.below(7) as usize],
        1..=2 => point(r),
        3..=10 => [16_777_216.0, -16_777_216.0][r.below(2) as usize] + r.range(-4.0, 4.0),
        _ => r.range(-1e5, 1e5),
    }
}

/// Paths of the batch proofs: the old one, the current one, and each switch the point path reads.
fn batch_paths() -> Vec<Paths> {
    let o = Paths::OLD;
    let mut paths = vec![
        o,
        Paths::current(),
        Paths { floor_java: true, ..o },
        Paths { perm512: true, ..o },
        Paths { floor_java: true, perm512: true, ..o },
    ];
    if rc_data::cpu::has_avx2() {
        let avx2 = Paths { perm512: true, batch_avx2: true, ..o };
        paths.push(avx2);
        paths.push(Paths { floor_java: true, ..avx2 });
        paths.push(Paths { batch_shared_cell: true, ..avx2 });
        paths.push(Paths { floor_java: true, batch_shared_cell: true, ..avx2 });
    }
    paths
}

/// A batch computed by `f` against the point path, point by point: batches of 900 to 1 099 points
/// (groups of 8 and a tail), `n` points in all. Bit for bit against the point path of the same
/// switches, NaN included; and against the old point path, where a NaN only has to stay a NaN (its
/// payload is not observable, and the 256 and 512 tables of the point path already give NaNs of
/// different bits for an infinite or NaN coordinate). First difference, if any.
fn check_batches(
    n: u64,
    paths: &[Paths],
    f: impl Fn(&NoiseStack, [&[f64]; 3], &mut [f32], Paths) + Sync,
) -> Option<String> {
    (0..n / 1000).into_par_iter().find_map_any(|k| {
        let mut r = Mix(k ^ 0xBA7C_0000);
        let stack = random_stack(&mut r);
        let len = 900 + r.below(200) as usize;
        // One batch in two gathers its points within a few blocks, as a chunk's batch does: the
        // low octaves then put whole groups in one cell.
        let clustered = k % 2 == 0;
        let mut coords = || {
            let center = r.range(-1e5, 1e5);
            let one = |r: &mut Mix| if clustered && r.below(1000) != 0 { center + r.range(-8.0, 8.0) } else { batch_coordinate(r) };
            (0..len).map(|_| one(&mut r)).collect::<Vec<f64>>()
        };
        let (xs, ys, zs) = (coords(), coords(), coords());
        let point_path = |p: Paths| -> Vec<f32> { (0..len).map(|i| stack.get_with(xs[i], ys[i], zs[i], p)).collect() };
        let old = point_path(Paths::OLD);
        for &p in paths {
            let same = point_path(p);
            let mut got = vec![f32::NAN; len];
            f(&stack, [&xs, &ys, &zs], &mut got, p);
            let differs = |i: usize| {
                let both_nan = got[i].is_nan() && old[i].is_nan();
                got[i].to_bits() != same[i].to_bits() || (got[i].to_bits() != old[i].to_bits() && !both_nan)
            };
            if let Some(i) = (0..len).find(|&i| differs(i)) {
                let (x, y, z) = (xs[i], ys[i], zs[i]);
                return Some(format!("batch {k} {p:?} point {i} ({x}, {y}, {z}): {} ≠ {} (old {})", got[i], same[i], old[i]));
            }
        }
        None
    })
}

#[test]
fn noise_batches_match_point_path_bitwise() {
    let batch = |s: &NoiseStack, [xs, ys, zs]: [&[f64]; 3], out: &mut [f32], p: Paths| s.get_many_with(xs, ys, zs, out, p);
    assert_eq!(check_batches(1_000_000, &batch_paths(), batch), None);
}

/// The batch proof can see the order of the additions and a fused multiply-add: the layers added
/// in reverse order, or each layer added through `mul_add`, must be caught.
#[test]
fn noise_batch_checker_detects_sabotage() {
    // One layer alone with amplitude 1: its noise, as the batch computes it.
    let layer_noise = |l: &Layer, [xs, ys, zs]: [&[f64]; 3], p: Paths| {
        let alone = NoiseStack { layers: vec![Layer { amp: 1.0, ..l.clone() }] };
        let mut n = vec![0.0f32; xs.len()];
        alone.get_many_with(xs, ys, zs, &mut n, p);
        n
    };
    let reversed = |s: &NoiseStack, pts: [&[f64]; 3], out: &mut [f32], p: Paths| {
        out.fill(0.0);
        for l in s.layers.iter().rev() {
            for (o, n) in out.iter_mut().zip(layer_noise(l, pts, p)) {
                *o += l.amp * n;
            }
        }
    };
    let fused = |s: &NoiseStack, pts: [&[f64]; 3], out: &mut [f32], p: Paths| {
        out.fill(0.0);
        for l in &s.layers {
            for (o, n) in out.iter_mut().zip(layer_noise(l, pts, p)) {
                *o = l.amp.mul_add(n, *o);
            }
        }
    };
    let paths = batch_paths();
    assert!(check_batches(100_000, &paths, reversed).is_some(), "reversed layers not detected");
    assert!(check_batches(100_000, &paths, fused).is_some(), "fused multiply-add not detected");
}

// ---------------------------------------------------------------------------------------------
// sincos

fn sincos_inputs(k: u32) -> [f64; 3] {
    let f = k as f32 * (1.0 / 16_777_216.0);
    [
        f64::from(f) * std::f64::consts::PI,
        f64::from(f * std::f32::consts::PI),
        f64::from(f) * 2.0 * std::f64::consts::PI - 1e3,
    ]
}

fn check_sincos(f: impl Fn(f64) -> (f64, f64) + Sync) -> Option<f64> {
    let extra = [0.0, -0.0, 1e-9, -1e-9, 1e-8, 1e5, 1e5 + 1.0, -1e5, f64::NAN, f64::INFINITY, 1e300];
    if let Some(&x) = extra.iter().find(|&&x| {
        let (s, c) = f(x);
        s.to_bits() != jmath::sin(x).to_bits() || c.to_bits() != jmath::cos(x).to_bits()
    }) {
        return Some(x);
    }
    (0..1u32 << 24).into_par_iter().find_map_any(|k| {
        sincos_inputs(k).into_iter().find(|&x| {
            let (s, c) = f(x);
            s.to_bits() != jmath::sin(x).to_bits() || c.to_bits() != jmath::cos(x).to_bits()
        })
    })
}

#[test]
fn sincos_matches_sin_cos_exhaustive() {
    assert_eq!(check_sincos(jmath::sincos_shared), None);
    assert_eq!(check_sincos(jmath::sincos), None);
    // Sabotage: reduction shifted by one ulp.
    assert!(check_sincos(|x| jmath::sincos_shared(x.next_up())).is_some(), "sabotage not detected");
}

/// Every way of computing, through sin and cos taken separately and through `sincos_shared`,
/// gives the bits of the selected one over the same exhaustive inputs.
#[test]
fn sincos_paths_match_exhaustive() {
    use jmath::{ProductPath, SinCosPaths};
    eprintln!("selected: {:?}, FMA on this processor: {}", SinCosPaths::selected(), rc_data::cpu::has_fma());
    for product in [ProductPath::LibraryFma, ProductPath::Split, ProductPath::HardwareFma] {
        // Each series switch alone is checked by the unit tests of `jmath` (10⁷ angles).
        for (only_needed_series, interleaved_series) in [(false, false), (true, true)] {
            let paths = SinCosPaths { product, only_needed_series, interleaved_series };
            assert_eq!(check_sincos(|x| (jmath::sin_with(x, paths), jmath::cos_with(x, paths))), None, "{paths:?}");
            assert_eq!(check_sincos(|x| jmath::sincos_shared_with(x, paths)), None, "{paths:?}");
        }
    }
    // Sabotage: cosine of the old path shifted by one ulp.
    let old = SinCosPaths::OLD;
    let bad = |x: f64| (jmath::sin_with(x, old), jmath::cos_with(x, old).next_up());
    assert!(check_sincos(bad).is_some(), "sabotage not detected");
}

// ---------------------------------------------------------------------------------------------
// Inventory and micro-benches (measurement)

#[test]
#[ignore = "benchmark"]
fn router_noise_inventory() {
    use rc_worldgen::sampler::S;
    let Some(pack) = common::data_pack() else { return };
    for dim in ["overworld", "nether", "end"] {
        let g = rc_worldgen::Generator::new(&pack, dim, 0).unwrap();
        let (mut vol, mut y0, mut zero_amp, mut layers) = (0, 0, 0, 0);
        for n in &g.program.nodes {
            if let S::Noise { noise, xz, y } = n {
                vol += 1;
                if *y == 0.0 {
                    y0 += 1;
                }
                eprintln!("  {dim} noise {noise} xz {xz} y {y}");
            }
        }
        for s in &g.program.noises {
            for l in &s.layers {
                layers += 1;
                if l.amp == 0.0 {
                    zero_amp += 1;
                }
            }
        }
        eprintln!("{dim}: {vol} noise nodes (of which {y0} with zero y scale), {layers} layers, {zero_amp} with zero amplitude");
    }
}

/// Every shifted noise without y (in x and z, zero y scale) of the 26.3 programs is reached under
/// a slice in y only, so each of its volumes holds a single row: a batch is one layer of points of
/// a chunk. Shifted noises in x, y and z: none (cold path). Programs of the terrain (router, aquifer,
/// veins, caves), every preset and dimension.
#[test]
fn flat_shifted_noises_are_reached_under_a_slice_in_y() {
    use rc_worldgen::sampler::S;
    use rc_worldgen::terrain::TerrainGenerator;
    let Some(pack) = common::data_pack() else { return };
    let sources = [
        ("minecraft:normal", "minecraft:overworld"),
        ("minecraft:large_biomes", "minecraft:overworld"),
        ("minecraft:amplified", "minecraft:overworld"),
        ("minecraft:normal", "minecraft:the_nether"),
        ("minecraft:normal", "minecraft:the_end"),
    ];
    let mut flat_in_overworld = 0;
    for (preset, dim) in sources {
        let t = TerrainGenerator::from_preset(&pack, preset, dim, 0).expect("generator");
        let (nodes, router) = (&t.g.program.nodes, &t.g.router);
        let mut read_by_another = vec![false; nodes.len()];
        for n in nodes {
            n.children(|c| read_by_another[c as usize] = true);
        }
        // Entry points: the router fields and every node no other node reads.
        let mut stack: Vec<u32> = (0..nodes.len() as u32).filter(|&s| !read_by_another[s as usize]).collect();
        stack.extend([router.temperature, router.vegetation, router.continents, router.erosion, router.depth]);
        stack.extend([router.ridges, router.chunk_surface_level, router.final_density]);
        // Nodes reached without crossing a slice in y.
        let mut unsliced = vec![false; nodes.len()];
        while let Some(s) = stack.pop() {
            if std::mem::replace(&mut unsliced[s as usize], true) || matches!(nodes[s as usize], S::SliceY(..)) {
                continue;
            }
            nodes[s as usize].children(|c| stack.push(c));
        }
        let flat: Vec<usize> = (0..nodes.len()).filter(|&i| matches!(nodes[i], S::ShiftedXz { y, .. } if y == 0.0)).collect();
        let exposed: Vec<&usize> = flat.iter().filter(|&&i| unsliced[i]).collect();
        let shifted_in_y = nodes.iter().filter(|n| matches!(n, S::ShiftedXyz { .. })).count();
        let with_y = nodes.iter().filter(|n| matches!(n, S::ShiftedXz { y, .. } if *y != 0.0)).count();
        eprintln!("{preset} {dim}: {} flat shifted noises, {with_y} shifted with y, {shifted_in_y} shifted in y", flat.len());
        assert!(exposed.is_empty(), "{preset} {dim}: flat shifted noises reached outside a slice in y: {exposed:?}");
        assert_eq!(shifted_in_y, 0, "{preset} {dim}: shifted noise in y");
        if dim == "minecraft:overworld" {
            flat_in_overworld += flat.len();
        }
    }
    // Temperature, vegetation, continents, erosion and ridges at least, in each overworld.
    assert!(flat_in_overworld >= 15, "too few flat shifted noises ({flat_in_overworld}): the test proves nothing");
}

/// The same while generating: the volumes the shifted noises evaluate over 16 chunks per
/// dimension (biomes, then the terrain up to the caves) hold a single row each; their mean size
/// is printed (the batches of the micro-bench).
#[test]
fn shifted_noise_volumes_are_single_rows() {
    use rc_worldgen::terrain::{BiomeCache, Stage, TerrainGenerator};
    let Some(pack) = common::data_pack() else { return };
    rc_worldgen::sampler::count_evaluations(true);
    let targets: Vec<(i32, i32)> = (0..4).flat_map(|x| (0..4).map(move |z| (x * 7 - 10, z * 5 - 8))).collect();
    let mut volumes = 0;
    for dim in ["minecraft:overworld", "minecraft:the_nether", "minecraft:the_end"] {
        let t = TerrainGenerator::from_preset(&pack, "minecraft:normal", dim, 42).expect("generator");
        let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
        let c = pool.install(|| {
            rc_worldgen::sampler::take_eval_counts();
            let mut cache = BiomeCache::default();
            t.prepare_biomes(&mut cache, &targets);
            for &(x, z) in &targets {
                std::hint::black_box(t.generate_until(x, z, &cache, None, Stage::Carvers));
            }
            rc_worldgen::sampler::take_eval_counts()
        });
        let mean = c.shifted_noise_values as f64 / c.shifted_noise_volumes.max(1) as f64;
        eprintln!("{dim}: {} shifted noise volumes, {:.1} points each", c.shifted_noise_volumes, mean);
        assert_eq!(c.shifted_noise_volumes_taller_than_one_row, 0, "{dim}: shifted noise volume taller than one row");
        volumes += c.shifted_noise_volumes;
    }
    rc_worldgen::sampler::count_evaluations(false);
    assert!(volumes > 0, "no shifted noise volume counted: the test proves nothing");
}

fn time<T>(label: &str, reps: u32, mut f: impl FnMut() -> T) -> f64 {
    let _ = std::hint::black_box(f());
    let mut best = f64::MAX;
    for _ in 0..reps {
        let t = Instant::now();
        std::hint::black_box(f());
        best = best.min(t.elapsed().as_secs_f64());
    }
    eprintln!("{label}: {:.3} ms (best of {reps})", best * 1e3);
    best
}

/// Every switch of the noise kernel but the AVX2 bodies.
const NEW: Paths = Paths {
    floor_java: true,
    perm512: true,
    ypre: true,
    colconst: true,
    simd: true,
    avx2: false,
    x_precomputed: true,
    xy_perm_table: true,
    batch_avx2: false,
    batch_shared_cell: false,
};

/// Every switch but the AVX2 bodies, the work in x once per octave and the xy permutation table.
const BEFORE_X_WORK: Paths = Paths { x_precomputed: false, xy_perm_table: false, ..NEW };

fn named() -> Vec<(&'static str, Paths)> {
    let o = Paths::OLD;
    let mut v = vec![
        ("old", o),
        ("floor_java", Paths { floor_java: true, ..o }),
        ("perm512", Paths { perm512: true, ..o }),
        ("ypre", Paths { ypre: true, ..o }),
        ("ypre+colconst", Paths { ypre: true, colconst: true, ..o }),
        ("ypre+simd", Paths { ypre: true, simd: true, ..o }),
        ("all but avx2, x work and xy table", BEFORE_X_WORK),
        ("  + x work", Paths { x_precomputed: true, ..BEFORE_X_WORK }),
        ("  + xy table", Paths { xy_perm_table: true, ..BEFORE_X_WORK }),
        ("all but avx2", NEW),
    ];
    if rc_data::cpu::has_avx2() {
        v.push(("ypre+simd+avx2", Paths { ypre: true, simd: true, avx2: true, ..o }));
        v.push(("all + avx2, without x work and xy table", Paths { avx2: true, ..BEFORE_X_WORK }));
        v.push(("all + avx2", Paths { avx2: true, ..NEW }));
    }
    v
}

#[test]
#[ignore = "benchmark"]
fn noise_micro_bench() {
    let mut r = Mix(7);
    for (lo, label) in [(3e7, "±3e7"), (3e9, "±3e9 (28% out of range)")] {
        let xs: Vec<f64> = (0..10_000_000).map(|_| r.range(-lo, lo)).collect();
        time(&format!("floor libm 10^7 {label}"), 5, || xs.iter().map(|&v| i64::from(floor_i_libm(v))).sum::<i64>());
        time(&format!("floor java 10^7 {label}"), 5, || xs.iter().map(|&v| i64::from(floor_to_i32(v))).sum::<i64>());
        // The generator's old scattered floors read their switch at every call (an atomic load and
        // a branch); they now call the shared floor directly.
        let switch_per_call = |v: f64| if rc_worldgen::opt::floor_java() { floor_to_i32(v) } else { floor_i_libm(v) };
        time(&format!("floor java, switch read per call, 10^7 {label}"), 5, || {
            xs.iter().map(|&v| i64::from(switch_per_call(v))).sum::<i64>()
        });
    }

    let mut rng = Rng::xoroshiro(1);
    let p = Perlin::new(&mut rng);
    let stack = NoiseStack { layers: vec![Layer { noise: Octave::Perlin(p), freq: 1.0, amp: 1.0 }] };
    let pts: Vec<(f64, f64, f64)> =
        (0..1_000_000).map(|_| (r.range(-3e4, 3e4), r.range(-64.0, 320.0), r.range(-3e4, 3e4))).collect();
    for (name, paths) in named() {
        time(&format!("get 10^6, {name}"), 7, || pts.iter().map(|&(x, y, z)| stack.get_with(x, y, z, paths)).sum::<f32>());
    }

    // Typical volumes: full chunk 16 × 384 × 16 (step 1), terrain y scale, and zero y.
    let mut rng = Rng::xoroshiro(2);
    let st = NoiseStack {
        layers: (0..8)
            .map(|i| Layer {
                noise: Octave::Perlin(Perlin::new(&mut rng)),
                freq: 2f64.powi(-7 + i),
                amp: 1.0 / (1 << i) as f32,
            })
            .collect(),
    };
    // Also the cell corners of a chunk (5 × 49 × 5, steps 4, 8, 4), what the interpolated
    // densities evaluate: 64 of them per timing.
    let volumes = [
        ("16x384x16", Volume::new(16, 384, 16, 1600, -64, -800), 1),
        ("64 x 5x49x5 corners", Volume::stepped(5, 49, 5, 1600, -64, -800, 4, 8, 4), 64),
    ];
    for (shape, v, repeats) in volumes {
        let mut buf = vec![0.0f32; v.size()];
        for (ys, label) in [(1.0, "y 1"), (0.0, "y 0")] {
            for (name, paths) in named() {
                time(&format!("volume {shape} 8 octaves {label}, {name}"), 7, || {
                    for _ in 0..repeats {
                        buf.fill(0.0);
                        st.add_to_volume_with(&mut buf, &v, 1.0, ys, 1.0, paths);
                    }
                    buf[100]
                });
            }
        }
    }

    let xs: Vec<f64> = (0..1_000_000).map(|_| r.range(-10.0, 10.0)).collect();
    time("sin+cos 10^6", 3, || xs.iter().map(|&x| jmath::sin(x) + jmath::cos(x)).sum::<f64>());
    time("sincos 10^6", 3, || {
        xs.iter()
            .map(|&x| {
                let (s, c) = jmath::sincos_shared(x);
                s + c
            })
            .sum::<f64>()
    });
}

/// The work in x once per octave and the xy permutation table, alone and together, alternated over
/// 5 passes (a loaded machine moves between passes): volume 16 × 384 × 16 and 64 cell-corner
/// volumes 5 × 49 × 5, 8 octaves, y scale 1, generic and AVX2 bodies; best time of each.
#[test]
#[ignore = "benchmark"]
fn noise_volume_x_work_bench() {
    let mut rng = Rng::xoroshiro(2);
    let st = NoiseStack {
        layers: (0..8)
            .map(|i| Layer { noise: Octave::Perlin(Perlin::new(&mut rng)), freq: 2f64.powi(-7 + i), amp: 1.0 / (1 << i) as f32 })
            .collect(),
    };
    let volumes = [
        ("16x384x16", Volume::new(16, 384, 16, 1600, -64, -800), 4),
        ("64 x 5x49x5 corners", Volume::stepped(5, 49, 5, 1600, -64, -800, 4, 8, 4), 256),
    ];
    let mut bodies = vec![("generic", BEFORE_X_WORK)];
    if rc_data::cpu::has_avx2() {
        bodies.push(("avx2", Paths { avx2: true, ..BEFORE_X_WORK }));
    }
    for (shape, v, repeats) in volumes {
        let mut buf = vec![0.0f32; v.size()];
        for (body, base) in &bodies {
            let variants = [
                ("before", *base),
                ("x work", Paths { x_precomputed: true, ..*base }),
                ("xy table", Paths { xy_perm_table: true, ..*base }),
                ("both", Paths { x_precomputed: true, xy_perm_table: true, ..*base }),
            ];
            let mut best = [f64::MAX; 4];
            for _ in 0..5 {
                for (k, (_, paths)) in variants.iter().enumerate() {
                    let t = Instant::now();
                    for _ in 0..repeats {
                        buf.fill(0.0);
                        st.add_to_volume_with(&mut buf, &v, 1.0, 1.0, 1.0, *paths);
                    }
                    std::hint::black_box(buf[100]);
                    best[k] = best[k].min(t.elapsed().as_secs_f64() * 1e3 / repeats as f64);
                }
            }
            let line: Vec<String> = variants.iter().zip(best).map(|((name, _), b)| format!("{name} {b:.4} ms")).collect();
            eprintln!("{shape}, {body} body, per volume: {}", line.join(" | "));
        }
    }
}

/// Ways of computing a batch of shifted noise points (`NoiseStack::get_many_with`).
#[derive(Clone, Copy)]
enum BatchWay {
    /// The old volume path: the whole noise, point by point.
    PointByPoint,
    /// Layers outside, points inside, one point at a time per layer.
    Batch,
    /// The same with groups of 8 points in AVX2 registers.
    BatchAvx2,
    /// The same, a group of 8 points in one cell reading its permutations once.
    BatchAvx2SharedCell,
}

/// The batches of `grids` (coordinates x, y, z of each) computed the way `way` says, on the pool.
fn shifted_noise_grids(stack: &NoiseStack, grids: &[[Vec<f64>; 3]], way: BatchWay, pool: &rayon::ThreadPool) -> f32 {
    let base = Paths::current();
    let avx2 = Paths { batch_avx2: true, perm512: true, batch_shared_cell: false, ..base };
    let paths = match way {
        BatchWay::PointByPoint | BatchWay::Batch => Paths { batch_avx2: false, ..base },
        BatchWay::BatchAvx2 => avx2,
        BatchWay::BatchAvx2SharedCell => Paths { batch_shared_cell: true, ..avx2 },
    };
    let one = |out: &mut Vec<f32>, [xs, ys, zs]: &[Vec<f64>; 3]| {
        out.resize(xs.len(), 0.0);
        if let BatchWay::PointByPoint = way {
            for (i, o) in out.iter_mut().enumerate() {
                *o = stack.get_with(xs[i], ys[i], zs[i], paths);
            }
        } else {
            stack.get_many_with(xs, ys, zs, out, paths);
        }
        out[0]
    };
    pool.install(|| grids.par_iter().map_init(Vec::new, one).sum())
}

/// Shifted 2D noise as the climate evaluates it: the continentalness stack (9 octaves, 18 layers)
/// over grids of 4 × 4, 5 × 5 (the generation's batches: 22.6 points on average, see
/// `shifted_noise_volumes_are_single_rows`), 7 × 7 (49) and 11 × 11 (121) quarter positions with
/// their shifts; point by point against batches, scalar and AVX2; 1 thread then powers of 2 up to
/// the number of logical cores.
#[test]
#[ignore = "benchmark"]
fn shifted_noise_batch_bench() {
    use rc_worldgen::noise::{NormalNoise, Normalize};
    let amplitudes = vec![1.0, 1.0, 2.0, 2.0, 2.0, 1.0, 1.0, 1.0, 1.0];
    let stack = NormalNoise::new(1.0, -9, 9, Normalize::Enabled, amplitudes).create(&mut Rng::xoroshiro(3));
    eprintln!("paths {:?}, AVX2 {}", Paths::current(), rc_data::cpu::has_avx2());
    let mut r = Mix(21);
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    for side in [4, 5, 7, 11] {
        let grids: Vec<[Vec<f64>; 3]> = (0..2048)
            .map(|_| {
                let (x0, z0) = (r.below(200_000) as i32 - 100_000, r.below(200_000) as i32 - 100_000);
                let mut grid = [Vec::new(), Vec::new(), Vec::new()];
                for k in 0..side * side {
                    // Quarter positions × 0.25 (the climate scale), plus a shift of a few units.
                    grid[0].push(f64::from(x0 + 4 * (k / side)) * 0.25 + r.range(-4.0, 4.0));
                    grid[1].push(0.0);
                    grid[2].push(f64::from(z0 + 4 * (k % side)) * 0.25 + r.range(-4.0, 4.0));
                }
                grid
            })
            .collect();
        let points = (grids.len() * side as usize * side as usize) as f64;
        let mut threads = 1;
        while threads <= cores {
            let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
            let mut ways = vec![("point by point", BatchWay::PointByPoint), ("batch", BatchWay::Batch)];
            if rc_data::cpu::has_avx2() {
                ways.push(("batch avx2", BatchWay::BatchAvx2));
                ways.push(("batch avx2 shared cell", BatchWay::BatchAvx2SharedCell));
            }
            for (name, way) in ways {
                let best = time(&format!("{} points, {threads} threads, {name}", side * side), 7, || {
                    shifted_noise_grids(&stack, &grids, way, &pool)
                });
                eprintln!("    {:.2} ns per point", best * 1e9 / points);
            }
            threads = if threads == cores { cores + 1 } else { (threads * 2).min(cores) };
        }
    }
}

/// Ways of computing `jmath` sin and cos, each alone then combined: 10⁶ angles in ±10, sin + cos
/// separately and `sincos_shared`; then the old and the selected ways on 1 thread and powers of 2
/// up to the number of logical cores (4 × 10⁶ angles in ±10⁵).
#[test]
#[ignore = "benchmark"]
fn sincos_paths_bench() {
    use jmath::{ProductPath, SinCosPaths};
    let old = SinCosPaths::OLD;
    let fma = ProductPath::HardwareFma;
    let named = [
        ("old", old),
        ("split", SinCosPaths { product: ProductPath::Split, ..old }),
        ("fma", SinCosPaths { product: fma, ..old }),
        ("one series", SinCosPaths { only_needed_series: true, ..old }),
        ("interleaved", SinCosPaths { interleaved_series: true, ..old }),
        ("one series + interleaved", SinCosPaths { only_needed_series: true, interleaved_series: true, ..old }),
        ("split + both series switches", SinCosPaths { product: ProductPath::Split, only_needed_series: true, interleaved_series: true }),
        ("fma + both series switches", SinCosPaths { product: fma, only_needed_series: true, interleaved_series: true }),
    ];
    let sincos_sum = |xs: &[f64], paths: SinCosPaths| -> f64 {
        xs.iter()
            .map(|&x| {
                let (s, c) = jmath::sincos_shared_with(x, paths);
                s + c
            })
            .sum()
    };
    let mut r = Mix(11);
    let xs: Vec<f64> = (0..1_000_000).map(|_| r.range(-10.0, 10.0)).collect();
    eprintln!("FMA on this processor: {}, selected: {:?}", rc_data::cpu::has_fma(), SinCosPaths::selected());
    for (name, paths) in named {
        time(&format!("sin+cos 10^6, {name}"), 5, || {
            xs.iter().map(|&x| jmath::sin_with(x, paths) + jmath::cos_with(x, paths)).sum::<f64>()
        });
        time(&format!("sincos 10^6, {name}"), 5, || sincos_sum(&xs, paths));
    }
    let many: Vec<f64> = (0..4_000_000).map(|_| r.range(-1e5, 1e5)).collect();
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut threads = 1;
    while threads <= cores {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
        for (name, paths) in [("old", old), ("selected", SinCosPaths::selected())] {
            time(&format!("sincos 4x10^6, {threads} threads, {name}"), 3, || {
                pool.install(|| many.par_chunks(4096).map(|c| sincos_sum(c, paths)).sum::<f64>())
            });
        }
        threads = if threads == cores { cores + 1 } else { (threads * 2).min(cores) };
    }
}

/// `final_density_many` over 256 chunks, 1 thread then powers of 2 up to the number of cores;
/// paths chosen by the environment (run once with and once without `RUSTCRAFT_OPT_*=0`).
#[test]
#[ignore = "benchmark"]
fn final_density_threads() {
    let Some(pack) = common::data_pack() else { return };
    let g = rc_worldgen::Generator::new(&pack, "overworld", 42).unwrap();
    let chunks: Vec<(i32, i32)> = (0..16).flat_map(|x| (0..16).map(move |z| (x - 8, z - 8))).collect();
    eprintln!("paths: {:?}", Paths::current());
    let max = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut t = 1;
    while t <= max {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(t).build().unwrap();
        let _ = pool.install(|| g.final_density_many(&chunks[..16]));
        let s = Instant::now();
        let out = pool.install(|| g.final_density_many(&chunks));
        let dt = s.elapsed().as_secs_f64();
        eprintln!("{t} threads: {:.1} ms ({:.3} ms / chunk)", dt * 1e3, dt * 1e3 / out.len() as f64);
        t *= 2;
    }
}
