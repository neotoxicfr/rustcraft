//! Equivalence proofs of the shared Java arithmetic, and micro-benches
//! (`cargo test --release -p rc-data --lib java_math -- --ignored --nocapture`).
//! Each checker is also run on a sabotaged version, which must fail.

use std::f64::consts::{PI, TAU};
use std::time::Instant;

use rayon::prelude::*;

use super::*;

// ---------------------------------------------------------------- sine table

/// FNV-1a 64 of the table's bits, little-endian bytes.
fn table_hash(table: &SinTable) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for v in table {
        for byte in v.to_bits().to_le_bytes() {
            h ^= u64::from(byte);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

/// Hash of the table of the game's virtual machine: the double sine of `i / 10430.378350470453`
/// rounded to a float, dumped by Java 25.0.1 (the launcher's runtime); the same with the virtual
/// machine's own sine code turned off (`-XX:-UseLibmIntrinsic`).
const GAME_TABLE_HASH: u64 = 0x115b_15b6_ad9d_2006;

/// First index where two tables differ, bit for bit.
fn first_difference(a: &SinTable, b: impl Fn(usize) -> f32) -> Option<usize> {
    (0..SIN_TABLE_LEN).find(|&i| a[i].to_bits() != b(i).to_bits())
}

/// The formulas of the copies that existed before the shared table (kept here as references).
/// Physics, carver, decorations, environment curves and sky: `i / scale`.
fn old_entry_divided(i: usize) -> f32 {
    (i as f64 / 10_430.378_350_470_453).sin() as f32
}

/// Fluid mesh: `i * 2π / 65536`.
fn old_entry_tau(i: usize) -> f32 {
    (i as f64 * TAU / 65_536.0).sin() as f32
}

/// Command coordinates: `i * π * 2 / 65536`.
fn old_entry_pi_times_two(i: usize) -> f32 {
    (i as f64 * PI * 2.0 / 65_536.0).sin() as f32
}

#[test]
fn frozen_table_is_the_game_table() {
    assert_eq!(table_hash(&FROZEN_SIN_TABLE), GAME_TABLE_HASH);
    // Sabotage: one entry one unit of the last bit off must change the hash.
    let mut sabotaged = FROZEN_SIN_TABLE;
    sabotaged[12_345] = f32::from_bits(sabotaged[12_345].to_bits() + 1);
    assert_ne!(table_hash(&sabotaged), GAME_TABLE_HASH, "sabotage not detected");
}

/// Entry `i` of a sine table.
type EntryFormula = fn(usize) -> f32;

/// The C library tables, as every old copy computed them. Bit for bit on Windows; the C library
/// of other systems is only reported (known gap between the libraries' sines).
#[test]
fn frozen_table_equals_every_old_formula() {
    let formulas: [(&str, EntryFormula); 4] = [
        ("library entry", library_entry),
        ("i / scale", old_entry_divided),
        ("i * 2pi / 65536", old_entry_tau),
        ("i * pi * 2 / 65536", old_entry_pi_times_two),
    ];
    for (name, formula) in formulas {
        let differences = (0..SIN_TABLE_LEN).filter(|&i| FROZEN_SIN_TABLE[i].to_bits() != formula(i).to_bits()).count();
        if cfg!(windows) {
            assert_eq!(first_difference(&FROZEN_SIN_TABLE, formula), None, "{name}");
        } else {
            eprintln!("{name}: {differences} entries differ from the C library of this system");
        }
    }
    assert_eq!(
        first_difference(&FROZEN_SIN_TABLE, |i| library_table()[i]),
        first_difference(&FROZEN_SIN_TABLE, library_entry)
    );
    // Sabotage: the checker sees a single entry shifted by one unit of the last bit.
    let shifted =
        |i: usize| if i == 40_000 { f32::from_bits(FROZEN_SIN_TABLE[i].to_bits() + 1) } else { FROZEN_SIN_TABLE[i] };
    assert_eq!(first_difference(&FROZEN_SIN_TABLE, shifted), Some(40_000), "sabotage not detected");
}

fn sin_arguments() -> Vec<f64> {
    let mut args = vec![0.0, -0.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1e300, -1e300, 1e18, -1e18, 9.3e14];
    // Steps between 2^31 and 2^63: the long conversion keeps their low bits, an int would saturate.
    let mut big = 2.1e5;
    while big < 8.8e14 {
        args.extend([big, -big, big + 0.37, -big - 0.37]);
        big *= 1.37;
    }
    let mut x = -1_000.0;
    while x < 1_000.0 {
        args.push(x);
        x += 0.013_7;
    }
    for i in 0..SIN_TABLE_LEN as u32 {
        args.push(f64::from(i) / SIN_TABLE_SCALE);
        args.push(-f64::from(i) / SIN_TABLE_SCALE);
    }
    args
}

/// Entry `i` of a source, read without its index code: the reference compares each source with
/// itself, so the index is proven on every system (the C library may differ from the frozen
/// table outside Windows; that equality is the subject of the test above).
fn source_entry(source: SinSource, i: usize) -> f32 {
    source.table().map_or_else(|| library_entry(i), |table| table[i])
}

/// Index and value as the game reads them: the long conversion saturates (NaN gives 0).
fn game_sin(x: f64, source: SinSource) -> f32 {
    source_entry(source, ((x * 10_430.378_350_470_453) as i64 & 65_535) as usize)
}

fn game_cos(x: f64, source: SinSource) -> f32 {
    source_entry(source, ((x * 10_430.378_350_470_453 + 16_384.0) as i64 & 65_535) as usize)
}

#[test]
fn every_source_reads_the_same_entries() {
    let selected = SinSource::selected();
    for x in sin_arguments() {
        for source in SinSource::ALL {
            assert_eq!(sin_with(x, source).to_bits(), game_sin(x, source).to_bits(), "sin {x} {source:?}");
            assert_eq!(cos_with(x, source).to_bits(), game_cos(x, source).to_bits(), "cos {x} {source:?}");
        }
        assert_eq!(sin(x).to_bits(), game_sin(x, selected).to_bits(), "sin {x}");
        assert_eq!(cos(x).to_bits(), game_cos(x, selected).to_bits(), "cos {x}");
    }
    assert_eq!(sin(f64::NAN), 0.0);
    assert_eq!(sin(f64::INFINITY), source_entry(selected, 65_535));
    assert_eq!(sin(f64::NEG_INFINITY), 0.0);
}

/// The fluid mesh multiplied in single precision before the shared table; the game widens its
/// float angle and multiplies in double. Some angles of the fluid flows land on another entry.
#[test]
fn float_angles_are_widened_before_the_product() {
    let single = |a: f32| ((a * 10_430.378) as i64 & 0xffff) as usize;
    let double = |a: f32| table_index(f64::from(a) * SIN_TABLE_SCALE);
    let moved = (0..=u32::MAX / 97)
        .into_par_iter()
        .map(|k| f32::from_bits(k * 97))
        .filter(|a| a.abs() <= 7.0 && single(*a) != double(*a))
        .count();
    eprintln!("float angles in [-7, 7] (1 in 97 sampled) on another entry with a single product: {moved}");
    assert!(moved > 0, "the two products never differ: this test proves nothing");
    let a = -1.570_796_4f32;
    assert_eq!(sin(f64::from(a)).to_bits(), FROZEN_SIN_TABLE[double(a)].to_bits());
}

// ---------------------------------------------------------------- reals to integers

/// The game's floor to int: round down, then NaN gives 0 and the conversion saturates.
fn reference_floor_i32(v: f64) -> i32 {
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

/// The game's rounding of a float before the shared module (worldgen copy, exact).
fn reference_round_f32(v: f32) -> i32 {
    if v.is_nan() {
        0
    } else if v.abs() >= 8_388_608.0 {
        v as i32
    } else {
        (f64::from(v) + 0.5).floor() as i32
    }
}

/// The game's rounding of a double before the shared module (projectile copy, exact).
fn reference_round_f64(v: f64) -> i64 {
    let f = v.floor();
    (if v - f >= 0.5 { f + 1.0 } else { f }) as i64
}

fn same_real(a: f64, b: f64) -> bool {
    // The C library's NaN payload depends on the platform (glibc can hand a signaling NaN back as
    // is, Windows makes it quiet); the game sees one NaN and every conversion of it gives 0.
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

/// One value through every conversion, against the C library; `None` if all agree.
fn check_f64(v: f64) -> Option<&'static str> {
    let in_range = v.abs() < 2_147_483_648.0;
    let checks = [
        ("floor_to_i32", floor_to_i32(v) == reference_floor_i32(v)),
        ("floor_to_i64", floor_to_i64(v) == v.floor() as i64),
        ("floor", same_real(floor(v), v.floor())),
        ("ceil", same_real(ceil(v), v.ceil())),
        ("round_to_i64", round_to_i64(v) == reference_round_f64(v)),
        (
            "floor_in_i32_range",
            !in_range || {
                let (i, f) = floor_in_i32_range(v);
                i == reference_floor_i32(v) && same_real(f, f64::from(i))
            },
        ),
    ];
    checks.into_iter().find(|c| !c.1).map(|c| c.0)
}

fn check_f32(v: f32) -> Option<&'static str> {
    if floor_f32_to_i32(v) != reference_floor_i32(f64::from(v)) {
        return Some("floor_f32_to_i32");
    }
    if round_f32_to_i32(v) != reference_round_f32(v) {
        return Some("round_f32_to_i32");
    }
    check_f64(f64::from(v))
}

/// Every float (2^32), and each widened to a double.
#[test]
fn conversions_exhaustive_over_floats() {
    let bad = (0..=u32::MAX).into_par_iter().find_map_any(|b| check_f32(f32::from_bits(b)).map(|c| (b, c)));
    assert_eq!(bad, None);
}

fn limits() -> Vec<f64> {
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
        0.499_999_999_999_999_94,
        -0.499_999_999_999_999_94,
        2_147_483_647.0,
        2_147_483_647.5,
        2_147_483_648.0,
        -2_147_483_648.0,
        -2_147_483_648.5,
        -2_147_483_649.0,
        -2_147_483_647.5,
    ];
    for n in -40..40 {
        let n = f64::from(n);
        for d in [0.5, 0.25, 0.999_999_999_999_999_9, 1e-12] {
            l.push(n + d);
            l.push(n - d);
        }
        l.extend([n, n.next_up(), n.next_down()]);
    }
    for e in [23, 24, 31, 32, 51, 52, 53, 62, 63, 64] {
        let p = 2f64.powi(e);
        for v in [p, p.next_up(), p.next_down(), p + 0.5, p - 0.5, p + 1.5] {
            l.extend([v, -v]);
        }
    }
    l
}

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
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * ((self.next() >> 11) as f64 / (1u64 << 53) as f64)
    }
}

/// Doubles of every kind: raw bits, around ±2^31, small, near halves, beyond 2^52 and 2^63.
fn random_double(r: &mut Mix, i: u32) -> f64 {
    match i % 6 {
        0 => f64::from_bits(r.next()),
        1 => r.range(-3e9, 3e9),
        2 => r.range(-100.0, 100.0),
        3 => r.range(-2.2e9, 2.2e9).round() + [0.0, 0.5, -0.5, 1e-9][(r.next() % 4) as usize],
        4 => r.range(-1e19, 1e19),
        _ => r.range(-1e16, 1e16),
    }
}

#[test]
fn conversions_on_limits_and_random_doubles() {
    for v in limits() {
        assert_eq!(check_f64(v), None, "{v:e}");
    }
    let bad = (0..100u64).into_par_iter().find_map_any(|chunk| {
        let mut r = Mix(chunk.wrapping_mul(0x1234_5678_9ABC_DEF1) ^ 0xF00D);
        (0..1_000_000u32).map(|i| random_double(&mut r, i)).find_map(|v| check_f64(v).map(|c| (v.to_bits(), c)))
    });
    assert_eq!(bad, None);
    // Signaling NaN (never written by the game: its saves and packets write NaN in one form):
    // made quiet like the C library does, the integer conversions give 0.
    for bits in [0x7FF0_0000_0000_0001u64, 0xFFF0_0000_0000_0001, 0x7FF4_0000_0000_0000] {
        let signaling = f64::from_bits(bits);
        assert_eq!(check_f64(signaling), None, "{bits:x}");
        assert_eq!(floor(signaling).to_bits(), bits | 0x0008_0000_0000_0000);
        assert_eq!(floor_to_i32(signaling), 0);
    }
}

#[test]
fn signed_zeros_and_halves() {
    assert!(floor(-0.0).is_sign_negative());
    assert!(floor(0.25).is_sign_positive());
    assert_eq!(floor(-0.25), -1.0);
    assert!(ceil(-0.25).is_sign_negative() && ceil(-0.25) == 0.0);
    assert_eq!(ceil(0.25), 1.0);
    assert_eq!(round_f32_to_i32(0.499_999_97), 0);
    assert_eq!(round_f32_to_i32(-2.5), -2);
    assert_eq!(round_f32_to_i32(2.5), 3);
    assert_eq!(round_f32_to_i32(8_388_609.0), 8_388_609);
    assert_eq!(round_to_i64(0.499_999_999_999_999_94), 0);
    assert_eq!(round_to_i64(-2.5), -2);
    assert_eq!(round_to_i64(f64::INFINITY), i64::MAX);
    assert_eq!(floor_to_i32(-1e10), i32::MIN);
    assert_eq!(floor_to_i64(f64::NEG_INFINITY), i64::MIN);
    assert_eq!(floor_f32_to_i32(-1e10), i32::MIN);
}

/// The checkers fail on the classic mistakes.
#[test]
fn checkers_detect_sabotage() {
    // Truncation without the correction of negatives.
    assert!(limits().into_iter().any(|v| v as i32 != reference_floor_i32(v)));
    // Correction without saturation: -2^31 - 0.5 wraps to the maximum.
    let wrapping = |v: f64| (v as i32).wrapping_sub(i32::from(v < f64::from(v as i32)));
    assert!(limits().into_iter().any(|v| wrapping(v) != reference_floor_i32(v)));
    // Floor without the sign copied back: -0 becomes +0.
    let unsigned =
        |v: f64| if v.abs() < 4.5e15 { (v as i64) as f64 - if ((v as i64) as f64) > v { 1.0 } else { 0.0 } } else { v };
    assert!(limits().into_iter().any(|v| !same_real(unsigned(v), v.floor())));
    // Rounding through a float sum (the old enchanting and font copies): 0.49999997 gives 1.
    let float_sum = |v: f32| (v + 0.5).floor() as i32;
    assert!((0..=u32::MAX).into_par_iter().map(f32::from_bits).any(|v| float_sum(v) != reference_round_f32(v)));
}

// ---------------------------------------------------------------- angles

/// First float (bit pattern) where `f` differs from the library remainder `x % turn`, bit for
/// bit (NaN payloads included).
fn first_remainder_mismatch(turn: f32, f: impl Fn(f32) -> f32 + Sync) -> Option<u32> {
    (0..=u32::MAX).into_par_iter().find_map_any(|b| {
        let x = f32::from_bits(b);
        (f(x).to_bits() != (x % turn).to_bits()).then_some(b)
    })
}

/// Floats where a remainder path can go wrong: both sides of zero, one and two turns (degrees
/// and radians), the folding bounds, the integer limit of floats, NaN, infinities.
fn angle_limits() -> Vec<f32> {
    let mut l = vec![0.0, -0.0, f32::NAN, -f32::NAN, f32::INFINITY, f32::NEG_INFINITY, f32::from_bits(1), f32::MAX];
    let tau = std::f32::consts::TAU;
    let pi = std::f32::consts::PI;
    for v in [1.0, 2.0, 3.0, 180.0, 360.0, 540.0, 720.0, 1080.0, pi, tau, 2.0 * tau, 3.0 * tau, 8_388_608.0, 65_536.0] {
        for w in [v, v.next_up(), v.next_down()] {
            l.extend([w, -w]);
        }
    }
    l
}

/// Every float, through each path: the remainder by 360 and by 2π are the library's, bit for
/// bit (the wrap test alone would not see a `<=` at the fast path: only ±360 tell it).
/// About a minute on 32 threads: run once per change of these paths (passed on 06/11).
#[test]
#[ignore = "exhaustive, about a minute"]
fn turn_remainders_exhaustive_over_floats() {
    for path in TurnRemainder::ALL {
        assert_eq!(first_remainder_mismatch(360.0, |x| degrees_remainder_with(x, path)), None, "degrees {path:?}");
        assert_eq!(
            first_remainder_mismatch(std::f32::consts::TAU, |x| radians_remainder_with(x, path)),
            None,
            "radians {path:?}"
        );
    }
    for path in TurnRemainder::ALL {
        assert_eq!(first_remainder_mismatch(1.0, |x| remainder_by_one_with(x, path)), None, "remainder by one {path:?}");
    }
}

/// The same on one float in 61 and the limits (every run).
#[test]
fn turn_remainders_on_sampled_floats() {
    let floats: Vec<f32> = (0..=u32::MAX).step_by(61).map(f32::from_bits).chain(angle_limits()).collect();
    let same = |turn: f32, f: &(dyn Fn(f32) -> f32 + Sync)| {
        floats.par_iter().find_any(|&&x| f(x).to_bits() != (x % turn).to_bits()).copied()
    };
    for path in TurnRemainder::ALL {
        assert_eq!(same(360.0, &|x| degrees_remainder_with(x, path)), None, "degrees {path:?}");
        assert_eq!(same(std::f32::consts::TAU, &|x| radians_remainder_with(x, path)), None, "radians {path:?}");
        assert_eq!(same(1.0, &|x| remainder_by_one_with(x, path)), None, "remainder by one {path:?}");
    }
}

/// The double remainder on the widened floats (one in 7), the limits and random doubles.
#[test]
fn double_degrees_remainder_matches_library() {
    let mut r = Mix(11);
    let widened = (0..=u32::MAX).step_by(7).map(|b| f64::from(f32::from_bits(b)));
    let draws: Vec<f64> = (0..2_000_000).map(|i| if i % 2 == 0 { r.range(-2000.0, 2000.0) } else { random_double(&mut r, i) }).collect();
    let values: Vec<f64> = widened.chain(limits()).chain(draws).chain([360.0, -360.0, 720.0, -720.0, 719.999_999_999_999_9]).collect();
    for path in TurnRemainder::ALL {
        let bad = values.par_iter().find_any(|&&v| degrees_remainder_f64_with(v, path).to_bits() != (v % 360.0).to_bits());
        assert_eq!(bad, None, "{path:?}");
    }
}

/// The folds and Euclidean remainders built on the shared remainders give the bits of the
/// formulas they replace: the game's wrap, Rust's `rem_euclid`.
#[test]
fn folded_angles_match_their_formulas() {
    let game_wrap = |a: f32| {
        let mut r = a % 360.0;
        if r >= 180.0 {
            r -= 360.0;
        }
        if r < -180.0 {
            r += 360.0;
        }
        r
    };
    let bad = (0..=u32::MAX).into_par_iter().map(f32::from_bits).find_any(|&a| {
        wrap_degrees(a).to_bits() != game_wrap(a).to_bits()
            || euclid_degrees_remainder(a).to_bits() != a.rem_euclid(360.0).to_bits()
            || euclid_remainder_by_one(a).to_bits() != a.rem_euclid(1.0).to_bits()
    });
    assert_eq!(bad, None);
    for a in limits() {
        let game = {
            let mut r = a % 360.0;
            if r >= 180.0 {
                r -= 360.0;
            }
            if r < -180.0 {
                r += 360.0;
            }
            r
        };
        assert_eq!(wrap_degrees_f64(a).to_bits(), game.to_bits(), "{a}");
    }
    assert_eq!(lerp_angle_degrees(0.5, 350.0, 10.0), 360.0);
    assert_eq!(lerp_angle_degrees(0.5, 10.0, 350.0), 0.0);
}

/// The game's folding loops in radians, without our guard (it hangs beyond about 2^26).
fn game_fold_radians(mut d: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    while d < -PI {
        d += TAU;
    }
    while d >= PI {
        d -= TAU;
    }
    d
}

/// The old fold of the humanoid swim and drowned arms: library remainder by 2π first.
fn old_fold_radians_by_remainder(d: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    let mut r = d % TAU;
    if r >= PI {
        r -= TAU;
    }
    if r < -PI {
        r += TAU;
    }
    r
}

/// Below the guard the shared fold is the game's loops, bit for bit; below one turn it is also
/// the old remainder fold (so the humanoid swim moves no bit), which differs beyond (counted).
#[test]
fn radian_fold_is_the_game_loops() {
    let tau = std::f32::consts::TAU;
    let bad = (0..=u32::MAX).into_par_iter().map(f32::from_bits).find_any(|&d| {
        let shared = fold_radians(d).to_bits();
        let game = (d.abs() < 1024.0 || d.is_nan()).then(|| game_fold_radians(d).to_bits());
        game.is_some_and(|g| g != shared) || d.abs() < tau && shared != old_fold_radians_by_remainder(d).to_bits()
    });
    assert_eq!(bad, None);
    let beyond_a_turn = (0..=u32::MAX)
        .into_par_iter()
        .map(f32::from_bits)
        .filter(|d| (tau..1024.0).contains(&d.abs()))
        .filter(|&d| fold_radians(d).to_bits() != old_fold_radians_by_remainder(d).to_bits())
        .count();
    eprintln!("floats in [2π, 1024[ where the old remainder fold differs from the game's loops: {beyond_a_turn}");
    assert!(fold_radians(f32::NEG_INFINITY).is_nan() && fold_radians(f32::NAN).is_nan());
    assert_eq!(fold_radians(1.0e30).to_bits(), old_fold_radians_by_remainder(1.0e30).to_bits());
}

/// The checkers catch the classic slips: fast path at `<=` (only ±360 differ), the subtraction
/// without its sign, bounded at `<=` two turns, the fraction without its sign.
#[test]
fn angle_checkers_detect_sabotage() {
    let limits = angle_limits();
    let differs = |turn: f32, f: &dyn Fn(f32) -> f32| limits.iter().any(|&x| f(x).to_bits() != (x % turn).to_bits());
    assert!(differs(360.0, &|a| if a.abs() <= 360.0 { a } else { a % 360.0 }));
    assert!(differs(360.0, &|a| if (360.0..720.0).contains(&a.abs()) { a - 360f32.copysign(a) } else { a % 360.0 }));
    assert!(differs(360.0, &|a| if (360.0..=720.0).contains(&a.abs()) {
        (a - 360f32.copysign(a)).copysign(a)
    } else {
        a % 360.0
    }));
    assert!(differs(1.0, &|x| if x.abs() < 8_388_608.0 { x - (x as i32) as f32 } else { x % 1.0 }));
    // And the sampled limits pass on the real paths.
    for path in TurnRemainder::ALL {
        assert!(!differs(360.0, &|a| degrees_remainder_with(a, path)), "{path:?}");
    }
}

// ---------------------------------------------------------------- micro-benches

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

/// Calls per source, on one thread then on every thread (each thread its own 10^6 calls).
#[test]
#[ignore = "benchmark"]
fn sin_table_micro_bench() {
    let mut r = Mix(3);
    let xs: Vec<f64> = (0..1_000_000).map(|_| r.range(-10.0, 10.0)).collect();
    time("fill the table with the C library (startup of the old tables)", 5, || {
        (0..SIN_TABLE_LEN).map(library_entry).fold(0.0f32, |s, v| s + v)
    });
    let threads = rayon::current_num_threads();
    for source in SinSource::ALL {
        let _ = source.table();
        time(&format!("sin+cos 10^6, 1 thread, {source:?}"), 7, || {
            xs.iter().map(|&x| sin_with(x, source) + cos_with(x, source)).sum::<f32>()
        });
        time(&format!("sin+cos 10^6 per thread, {threads} threads, {source:?}"), 5, || {
            (0..threads)
                .into_par_iter()
                .map(|_| xs.iter().map(|&x| sin_with(x, source) + cos_with(x, source)).sum::<f32>())
                .sum::<f32>()
        });
    }
    time("sin+cos 10^6, 1 thread, selected by the switches", 7, || xs.iter().map(|&x| sin(x) + cos(x)).sum::<f32>());
}

#[test]
#[ignore = "benchmark"]
fn rounding_micro_bench() {
    let mut r = Mix(7);
    let xs: Vec<f64> = (0..10_000_000).map(|_| r.range(-3e7, 3e7)).collect();
    let fs: Vec<f32> = xs.iter().map(|&v| v as f32).collect();
    let sum_i32 = |f: fn(f64) -> i32| xs.iter().map(|&v| i64::from(f(v))).sum::<i64>();
    time("floor to i32 10^7, library", 5, || sum_i32(|v| v.floor() as i32));
    // Variant measured and not kept: the saturating conversions cost extra tests on x86-64.
    time("floor to i32 10^7, truncation then saturated correction (not kept)", 5, || {
        sum_i32(|v| (v as i32).saturating_sub(i32::from(v < f64::from(v as i32))))
    });
    time("floor to i32 10^7, shared (1.5*2^52 shift under 2^31)", 5, || sum_i32(floor_to_i32));
    time("floor to i64 10^7, library", 5, || xs.iter().map(|&v| v.floor() as i64).sum::<i64>());
    time("floor to i64 10^7, truncation then saturated correction (not kept)", 5, || {
        xs.iter().map(|&v| (v as i64).saturating_sub(i64::from(v < (v as i64) as f64))).sum::<i64>()
    });
    time("floor to i64 10^7, shared", 5, || xs.iter().map(|&v| floor_to_i64(v)).sum::<i64>());
    time("floor f32 to i32 10^7, library", 5, || fs.iter().map(|&v| i64::from(v.floor() as i32)).sum::<i64>());
    time("floor f32 to i32 10^7, truncation in single precision (old worldgen path)", 5, || {
        fs.iter().map(|&v| i64::from((v as i32).saturating_sub(i32::from(v < (v as i32) as f32)))).sum::<i64>()
    });
    time("floor f32 to i32 10^7, shared", 5, || fs.iter().map(|&v| i64::from(floor_f32_to_i32(v))).sum::<i64>());
    time("floor f64 10^7, library", 5, || xs.iter().map(|&v| v.floor()).sum::<f64>());
    time("floor f64 10^7, truncation through a long (not kept)", 5, || {
        xs.iter()
            .map(|&v| {
                let t = (v as i64) as f64;
                (t - if t > v { 1.0 } else { 0.0 }).copysign(v)
            })
            .sum::<f64>()
    });
    time("floor f64 10^7, shared", 5, || xs.iter().map(|&v| floor(v)).sum::<f64>());
    time("ceil f64 10^7, library", 5, || xs.iter().map(|&v| v.ceil()).sum::<f64>());
    time("ceil f64 10^7, shared", 5, || xs.iter().map(|&v| ceil(v)).sum::<f64>());
    time("round f32 10^7, old float sum + library floor", 5, || {
        fs.iter().map(|&v| i64::from((v + 0.5).floor() as i32)).sum::<i64>()
    });
    time("round f32 10^7, shared", 5, || fs.iter().map(|&v| i64::from(round_f32_to_i32(v))).sum::<i64>());
    time("round f64 10^7, old library floor", 5, || xs.iter().map(|&v| reference_round_f64(v)).sum::<i64>());
    time("round f64 10^7, shared", 5, || xs.iter().map(|&v| round_to_i64(v)).sum::<i64>());
}

/// Angle folds of the entity render: per-tick yaw differences (a few degrees), head minus body
/// (tens of degrees, one in 50 beyond a turn), texture scroll ages; one thread, then every
/// thread (each its own copy of the work).
#[test]
#[ignore = "benchmark"]
fn angle_wrap_micro_bench() {
    let mut r = Mix(5);
    let diffs: Vec<f32> =
        (0..1_000_000).map(|i| (if i % 50 == 0 { r.range(-700.0, 700.0) } else { r.range(-90.0, 90.0) }) as f32).collect();
    let ages: Vec<f32> = (0..1_000_000).map(|_| r.range(0.0, 2e5) as f32).collect();
    let threads = rayon::current_num_threads();
    let both = |label: &str, f: &(dyn Fn() -> f32 + Sync)| {
        time(&format!("{label}, 1 thread"), 7, f);
        time(&format!("{label}, {threads} threads"), 5, || (0..threads).into_par_iter().map(|_| f()).sum::<f32>());
    };
    for path in TurnRemainder::ALL {
        both(&format!("wrap degrees 10^6, {path:?}"), &|| {
            diffs.iter().map(|&d| {
                let mut a = degrees_remainder_with(d, path);
                if a >= 180.0 {
                    a -= 360.0;
                }
                if a < -180.0 {
                    a += 360.0;
                }
                a
            }).sum::<f32>()
        });
    }
    let radians: Vec<f32> = diffs.iter().map(|d| d.to_radians() * 0.1).collect();
    both("fold radians 10^6, old library remainder", &|| radians.iter().map(|&d| old_fold_radians_by_remainder(d)).sum());
    both("fold radians 10^6, game loops", &|| radians.iter().map(|&d| fold_radians(d)).sum());
    let scrolls: Vec<f32> = ages.iter().map(|a| a * 0.01).collect();
    both("scroll fraction 10^6, library % 1 then rem_euclid", &|| scrolls.iter().map(|&x| (x % 1.0).rem_euclid(1.0)).sum());
    both("scroll fraction 10^6, shared", &|| {
        scrolls
            .iter()
            .map(|&x| {
                let r = remainder_by_one_with(x, TurnRemainder::BelowOneTurnSkipped);
                if r < 0.0 { r + 1.0 } else { r }
            })
            .sum()
    });
}
