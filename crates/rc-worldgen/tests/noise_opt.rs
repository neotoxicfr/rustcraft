//! Bit-for-bit equivalence of the optimised noise paths (`crate::opt`) against the old code, and
//! micro-benches (`cargo test --release -p rc-worldgen --test noise_opt -- --ignored --nocapture`).
//! Each checker is also run on a sabotaged version, which must fail.

mod common;

use std::time::Instant;

use rayon::prelude::*;
use rc_worldgen::jmath;
use rc_worldgen::math::{floor_f_java, floor_f_libm, floor_i_java, floor_i_libm, floor_l_java, floor_small};
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
// floor

/// Reference of the game's 26.3 `floor` to i32: round down then saturated conversion,
/// hence NaN → 0, ≥ 2³¹ (and +∞) → MAX, < −2³¹ (and −∞) → MIN.
fn java_floor_ref(v: f64) -> i32 {
    if v.is_nan() {
        0
    } else if v >= 2_147_483_648.0 {
        i32::MAX
    } else if v < -2_147_483_648.0 {
        i32::MIN
    } else {
        v.floor() as i32
    }
}

fn java_floor_ref_f(v: f32) -> i32 {
    java_floor_ref(f64::from(v))
}

/// All the f32 values (2³²), in f32 and converted to f64; returns the first difference.
fn check_floor_all_f32(fi: fn(f64) -> i32, ff: fn(f32) -> i32) -> Option<u32> {
    (0..=u32::MAX).into_par_iter().find_any(|&b| {
        let v = f32::from_bits(b);
        ff(v) != java_floor_ref_f(v) || fi(f64::from(v)) != java_floor_ref(f64::from(v))
    })
}

fn floor_limits() -> Vec<f64> {
    let mut l = vec![
        0.0,
        -0.0,
        f64::NAN,
        -f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::MIN_POSITIVE,
        -f64::MIN_POSITIVE,
        f64::from_bits(1),
        -f64::from_bits(1),
        f64::MAX,
        f64::MIN,
        2_147_483_647.0,
        2_147_483_647.5,
        2_147_483_648.0,
        -2_147_483_648.0,
        -2_147_483_648.5,
        -2_147_483_649.0,
        -2_147_483_647.5,
        16_777_216.0,
        -16_777_216.0,
        9.007_199_254_740_992e15,
        -9.007_199_254_740_992e15,
    ];
    for n in -40..40 {
        let n = f64::from(n);
        for d in [0.5, 0.25, 0.999_999_999_999_999_9, 1e-12] {
            l.push(n + d);
            l.push(n - d);
        }
        l.push(n);
        l.push(n.next_up());
        l.push(n.next_down());
    }
    for e in [31, 32, 52, 53, 62, 63, 64] {
        let p = 2f64.powi(e);
        for v in [p, p.next_up(), p.next_down(), -p, (-p).next_up(), (-p).next_down()] {
            l.push(v);
        }
    }
    l
}

fn check_floor_f64(fi: fn(f64) -> i32, n: u64) -> Option<f64> {
    if let Some(v) = floor_limits().into_iter().find(|&v| fi(v) != java_floor_ref(v)) {
        return Some(v);
    }
    (0..n / 1_000_000).into_par_iter().find_map_any(|chunk| {
        let mut r = Mix(chunk.wrapping_mul(0x1234_5678_9ABC_DEF1) ^ 0xF00D);
        for i in 0..1_000_000u32 {
            let v = match i % 4 {
                0 => f64::from_bits(r.next()),
                1 => r.range(-3e9, 3e9),
                2 => r.range(-100.0, 100.0),
                _ => (r.range(-2.2e9, 2.2e9)).round() + [0.0, 0.5, -0.5, 1e-9][r.below(4) as usize],
            };
            if fi(v) != java_floor_ref(v) {
                return Some(v);
            }
        }
        None
    })
}

/// "Without correction of the negatives" floor: truncation alone.
fn sabotaged_floor(v: f64) -> i32 {
    v as i32
}

fn sabotaged_floor_f(v: f32) -> i32 {
    v as i32
}

/// Old form of `floor` (truncation then −1 without saturation): MAX under −2³¹.
fn sabotaged_floor_wrap(v: f64) -> i32 {
    let t = v as i32;
    t.wrapping_sub(i32::from(v < f64::from(t)))
}

/// `floor_small`: returned f64 value = `f64::from(integer)` bit for bit (never −0).
fn floor_small_ok(v: f64) -> bool {
    let (i, f) = floor_small(v);
    i == java_floor_ref(v) && f.to_bits() == f64::from(i).to_bits()
}

#[test]
fn floor_java_exhaustive_f32_and_sabotage() {
    assert_eq!(check_floor_all_f32(floor_i_java, floor_f_java), None);
    let bad = (0..=u32::MAX).into_par_iter().find_any(|&b| {
        let v = f64::from(f32::from_bits(b));
        v.abs() < 2_147_483_648.0 && !floor_small_ok(v)
    });
    assert_eq!(bad, None);
    assert!(floor_limits().into_iter().filter(|v| v.abs() < 2_147_483_648.0).all(floor_small_ok));
    assert!(check_floor_all_f32(sabotaged_floor, sabotaged_floor_f).is_some(), "sabotage not detected");
}

#[test]
fn floor_java_random_f64_and_limits() {
    assert_eq!(check_floor_f64(floor_i_java, 100_000_000), None);
    assert!(check_floor_f64(sabotaged_floor, 1_000_000).is_some(), "sabotage not detected");
    assert!(check_floor_f64(sabotaged_floor_wrap, 1_000_000).is_some(), "overflow not detected");
    // The game saturates everywhere: the old library path and the new one are equal everywhere.
    for v in floor_limits().into_iter().chain([-1e10, 1e10, -1e19, 1e19, -1e300]) {
        assert_eq!(floor_i_libm(v), floor_i_java(v), "{v}");
        assert_eq!(floor_l_java(v), v.floor() as i64, "{v}");
    }
    assert_eq!(floor_i_java(-1e10), i32::MIN);
    assert_eq!(floor_i_java(f64::NEG_INFINITY), i32::MIN);
    assert_eq!(floor_l_java(-1e19), i64::MIN);
    assert_eq!(floor_l_java(f64::NEG_INFINITY), i64::MIN);
    assert_eq!(floor_f_java(-1e10), i32::MIN);
    for b in [0u32, 0x8000_0000, 0x7F80_0000, 0xFF80_0000, 0x4F00_0000, 0xCF00_0000, 0xCF00_0001, 0x7FC0_0000] {
        let v = f32::from_bits(b);
        assert_eq!(floor_f_libm(v), floor_f_java(v), "{v}");
    }
}

// ---------------------------------------------------------------------------------------------
// Noise: volumes

const VARIANTS: [Paths; 9] = {
    const fn p(floor_java: bool, perm512: bool, ypre: bool, colconst: bool, simd: bool, avx2: bool) -> Paths {
        Paths { floor_java, perm512, ypre, colconst, simd, avx2 }
    }
    [
        p(true, false, false, false, false, false),
        p(false, true, false, false, false, false),
        p(false, false, true, false, false, false),
        p(false, false, true, true, false, false),
        p(false, false, true, false, true, false),
        p(false, false, true, false, true, true),
        p(true, true, true, true, false, false),
        p(true, true, true, true, true, false),
        p(true, true, true, true, true, true),
    ]
};

fn variants() -> Vec<Paths> {
    let avx2 = rc_worldgen::opt::avx2_available();
    VARIANTS.iter().copied().filter(|p| avx2 || !p.avx2).collect()
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
// sincos

fn sincos_inputs(k: u32) -> [f64; 3] {
    let f = k as f32 * (1.0 / 16_777_216.0);
    [
        f64::from(f) * std::f64::consts::PI,
        f64::from(f * std::f32::consts::PI),
        f64::from(f) * 2.0 * std::f64::consts::PI - 1e3,
    ]
}

fn check_sincos(f: fn(f64) -> (f64, f64)) -> Option<f64> {
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

const NEW: Paths = Paths { floor_java: true, perm512: true, ypre: true, colconst: true, simd: true, avx2: false };

fn named() -> Vec<(&'static str, Paths)> {
    let o = Paths::OLD;
    let mut v = vec![
        ("old", o),
        ("floor_java", Paths { floor_java: true, ..o }),
        ("perm512", Paths { perm512: true, ..o }),
        ("ypre", Paths { ypre: true, ..o }),
        ("ypre+colconst", Paths { ypre: true, colconst: true, ..o }),
        ("ypre+simd", Paths { ypre: true, simd: true, ..o }),
        ("all but avx2", NEW),
    ];
    if rc_worldgen::opt::avx2_available() {
        v.push(("ypre+simd+avx2", Paths { ypre: true, simd: true, avx2: true, ..o }));
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
        time(&format!("floor java 10^7 {label}"), 5, || xs.iter().map(|&v| i64::from(floor_i_java(v))).sum::<i64>());
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
    let v = Volume::new(16, 384, 16, 1600, -64, -800);
    let mut buf = vec![0.0f32; v.size()];
    for (ys, label) in [(1.0, "y 1"), (0.0, "y 0")] {
        for (name, paths) in named() {
            time(&format!("volume 16x384x16 8 octaves {label}, {name}"), 7, || {
                buf.fill(0.0);
                st.add_to_volume_with(&mut buf, &v, 1.0, ys, 1.0, paths);
                buf[100]
            });
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
