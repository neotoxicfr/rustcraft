//! The game's platform arithmetic, shared by every crate so that the client and the integrated
//! server hold one copy: its tabulated sine, and its conversions of reals to integers (rounding
//! down, rounding halves up), exact and without calls to the C library.
//!
//! Why without the C library: the release targets any x86-64, which has no rounding
//! instruction (SSE4.1 is not assumed), so `f64::floor` compiles to a call into the system
//! library. A truncating conversion, a comparison and a subtraction give the same bits.

use std::sync::OnceLock;

// ---------------------------------------------------------------- tabulated sine

/// Table steps per radian (65,536 per turn), the constant as the game writes it.
pub const SIN_TABLE_SCALE: f64 = 10_430.378_350_470_453;

/// Entries of the table: one turn.
pub const SIN_TABLE_LEN: usize = 65_536;

/// A quarter turn in table steps: the cosine reads the table that much further.
const QUARTER_TURN_STEPS: f64 = 16_384.0;

pub type SinTable = [f32; SIN_TABLE_LEN];

/// The game's tabulated sine of `x` radians. A caller holding a float widens it first, as the
/// game does: its sine takes a double.
#[inline]
pub fn sin(x: f64) -> f32 {
    lookup(table_index(x * SIN_TABLE_SCALE))
}

/// The game's tabulated cosine: the sine table read a quarter turn further.
#[inline]
pub fn cos(x: f64) -> f32 {
    lookup(table_index(x * SIN_TABLE_SCALE + QUARTER_TURN_STEPS))
}

/// [`sin`] through an explicit source (equivalence tests and benches).
#[inline]
pub fn sin_with(x: f64, source: SinSource) -> f32 {
    entry_from(source.table(), table_index(x * SIN_TABLE_SCALE))
}

/// [`cos`] through an explicit source (equivalence tests and benches).
#[inline]
pub fn cos_with(x: f64, source: SinSource) -> f32 {
    entry_from(source.table(), table_index(x * SIN_TABLE_SCALE + QUARTER_TURN_STEPS))
}

/// Index the game reads: the steps truncated to a long (saturated, NaN gives 0), low 16 bits.
#[inline]
fn table_index(steps: f64) -> usize {
    (steps as i64 & 0xFFFF) as usize
}

#[inline]
fn lookup(i: usize) -> f32 {
    entry_from(selected_table(), i)
}

#[inline]
fn entry_from(table: Option<&'static SinTable>, i: usize) -> f32 {
    match table {
        Some(table) => table[i],
        None => library_entry(i),
    }
}

/// Table of the switches, chosen at the first call (not a read of the environment per call).
fn selected_table() -> Option<&'static SinTable> {
    static TABLE: OnceLock<Option<&'static SinTable>> = OnceLock::new();
    *TABLE.get_or_init(|| SinSource::selected().table())
}

/// Where the entries of the sine table come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SinSource {
    /// Table computed by the compiler (default).
    Frozen,
    /// Table filled by the C library at first use (`RUSTCRAFT_OPT_FROZEN_SIN_TABLE=0`).
    BuiltAtFirstUse,
    /// No table: each entry computed by the C library when read, as several crates did before
    /// the shared table (`RUSTCRAFT_OPT_SHARED_SIN_TABLE=0`).
    ComputedPerCall,
}

impl SinSource {
    pub const ALL: [SinSource; 3] = [SinSource::Frozen, SinSource::BuiltAtFirstUse, SinSource::ComputedPerCall];

    /// Source chosen by the switches (each read once).
    pub fn selected() -> Self {
        if !crate::opt_switch!("RUSTCRAFT_OPT_SHARED_SIN_TABLE") {
            SinSource::ComputedPerCall
        } else if !crate::opt_switch!("RUSTCRAFT_OPT_FROZEN_SIN_TABLE") {
            SinSource::BuiltAtFirstUse
        } else {
            SinSource::Frozen
        }
    }

    /// The table of this source, `None` when entries are computed on demand.
    pub fn table(self) -> Option<&'static SinTable> {
        match self {
            SinSource::Frozen => Some(&FROZEN_SIN_TABLE),
            SinSource::BuiltAtFirstUse => Some(library_table()),
            SinSource::ComputedPerCall => None,
        }
    }
}

/// Entry `i` computed by the C library with the game's formula.
#[inline]
pub fn library_entry(i: usize) -> f32 {
    (i as f64 / SIN_TABLE_SCALE).sin() as f32
}

/// Table filled by the C library at first use, shared by every caller.
pub fn library_table() -> &'static SinTable {
    static TABLE: OnceLock<Box<SinTable>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let entries: Vec<f32> = (0..SIN_TABLE_LEN).map(library_entry).collect();
        entries.into_boxed_slice().try_into().expect("one entry per table step")
    })
}

/// Table computed by the compiler: nothing to fill at startup, and the same bits on every system
/// (the C libraries do not all round the sine alike). The tests check it against the table of
/// the game's virtual machine and against the C library's.
pub static FROZEN_SIN_TABLE: SinTable = frozen_sin_table();

const fn frozen_sin_table() -> SinTable {
    let mut table = [0.0f32; SIN_TABLE_LEN];
    let mut i = 0;
    while i < SIN_TABLE_LEN {
        table[i] = sin_of_table_angle(i as f64 / SIN_TABLE_SCALE) as f32;
        i += 1;
    }
    table
}

/// π/2 in three pieces: the first two have 33 significant bits, so their products by a quarter
/// count up to 4 are exact; the third is the rest rounded to nearest.
const HALF_PI_HIGH: f64 = 1.570_796_326_734_125_6;
const HALF_PI_MIDDLE: f64 = 6.077_100_506_303_966e-11;
const HALF_PI_LOW: f64 = 2.022_266_248_795_950_6e-21;

/// Sine of an angle of the table (`0 <= x < 2π`), within about one unit of the last bit of the
/// exact value, with only what a constant may compute: the nearest quarter turn is taken off,
/// then a Taylor series runs on the remainder (`|r| <= π/4`).
const fn sin_of_table_angle(x: f64) -> f64 {
    let quarters = (x * std::f64::consts::FRAC_2_PI + 0.5) as i64;
    let q = quarters as f64;
    // The first subtraction is exact (both terms are within a factor 2 of each other).
    let r = ((x - q * HALF_PI_HIGH) - q * HALF_PI_MIDDLE) - q * HALF_PI_LOW;
    match quarters & 3 {
        0 => sin_near_zero(r),
        1 => cos_near_zero(r),
        2 => -sin_near_zero(r),
        _ => -cos_near_zero(r),
    }
}

/// `(-1)^k / (first + 2k)!` for k = 1, 2, …: the Taylor coefficients after the first term of the
/// sine (`first` = 1) or of the cosine (`first` = 0). The factorials up to 18! are exact doubles.
const fn taylor_coefficients<const N: usize>(first: u32) -> [f64; N] {
    let mut coefficients = [0.0; N];
    let mut factorial = 1.0;
    let mut n = first;
    let mut k = 0;
    while k < N {
        factorial *= ((n + 1) * (n + 2)) as f64;
        n += 2;
        coefficients[k] = if k % 2 == 0 { -1.0 / factorial } else { 1.0 / factorial };
        k += 1;
    }
    coefficients
}

/// Terms of r^3 to r^17: the next one is below 10^-19 of the result at π/4.
const SIN_TERMS: [f64; 8] = taylor_coefficients(1);
/// Terms of r^2 to r^18.
const COS_TERMS: [f64; 9] = taylor_coefficients(0);

/// Sine for `|r| <= π/4`. The exact first term is added last, to the small rest.
const fn sin_near_zero(r: f64) -> f64 {
    let [c3, c5, c7, c9, c11, c13, c15, c17] = SIN_TERMS;
    let z = r * r;
    // Written out rather than looped: the compiler evaluates this 65,536 times.
    let rest = c3 + z * (c5 + z * (c7 + z * (c9 + z * (c11 + z * (c13 + z * (c15 + z * c17))))));
    r + r * z * rest
}

/// Cosine for `|r| <= π/4`.
const fn cos_near_zero(r: f64) -> f64 {
    let [c2, c4, c6, c8, c10, c12, c14, c16, c18] = COS_TERMS;
    let z = r * r;
    let rest = c2 + z * (c4 + z * (c6 + z * (c8 + z * (c10 + z * (c12 + z * (c14 + z * (c16 + z * c18)))))));
    1.0 + z * rest
}

// ---------------------------------------------------------------- angles

/// How the remainder of an angle by a full turn is computed. All give the bits of the
/// library remainder (`%`, sign of the operand); they differ in when the library is called.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnRemainder {
    /// Always the library call (`RUSTCRAFT_OPT_ANGLE_WRAP_FAST_PATH=0`, the old path).
    Library,
    /// Below one turn the remainder is the operand itself: no call (default).
    BelowOneTurnSkipped,
    /// Also from one to two turns: one exact subtraction (`RUSTCRAFT_OPT_ANGLE_WRAP_STERBENZ=1`).
    BelowTwoTurnsSubtracted,
}

impl TurnRemainder {
    pub const ALL: [TurnRemainder; 3] =
        [TurnRemainder::Library, TurnRemainder::BelowOneTurnSkipped, TurnRemainder::BelowTwoTurnsSubtracted];

    /// Path chosen by the switches, read once for the process.
    #[inline]
    pub fn selected() -> Self {
        static PATH: OnceLock<TurnRemainder> = OnceLock::new();
        *PATH.get_or_init(|| {
            if !crate::opt_switch!("RUSTCRAFT_OPT_ANGLE_WRAP_FAST_PATH") {
                TurnRemainder::Library
            } else if crate::opt_switch!("RUSTCRAFT_OPT_ANGLE_WRAP_STERBENZ", default_off) {
                TurnRemainder::BelowTwoTurnsSubtracted
            } else {
                TurnRemainder::BelowOneTurnSkipped
            }
        })
    }
}

/// `angle % turn` as the game computes it, through `path`. Without a rounding instruction the
/// library remainder is a call costing as much as a sine, and nearly every angle a caller hands
/// over is already under one turn, where the remainder is exact and equal to the operand (zeros
/// keep their sign). NaN and the infinities fail both tests and reach the library.
macro_rules! turn_remainder {
    ($angle:expr, $turn:expr, $path:expr) => {{
        let (angle, turn, path) = ($angle, $turn, $path);
        let magnitude = angle.abs();
        if path != TurnRemainder::Library && magnitude < turn {
            angle
        } else if path == TurnRemainder::BelowTwoTurnsSubtracted && magnitude < 2.0 * turn {
            // From one turn to two the operand is within a factor 2 of the turn, so the
            // difference is exact (Sterbenz lemma) and is the remainder; copying the sign back
            // gives -360 the -0 of the library.
            (angle - turn.copysign(angle)).copysign(angle)
        } else {
            angle % turn
        }
    }};
}

/// `angle % 360` (degrees, float), the remainder the game takes before folding an angle.
#[inline]
pub fn degrees_remainder(angle: f32) -> f32 {
    degrees_remainder_with(angle, TurnRemainder::selected())
}

/// [`degrees_remainder`] through an explicit path (equivalence tests and benches).
#[inline]
pub fn degrees_remainder_with(angle: f32, path: TurnRemainder) -> f32 {
    turn_remainder!(angle, 360.0f32, path)
}

/// `angle % 360` of a double.
#[inline]
pub fn degrees_remainder_f64(angle: f64) -> f64 {
    degrees_remainder_f64_with(angle, TurnRemainder::selected())
}

#[inline]
pub fn degrees_remainder_f64_with(angle: f64, path: TurnRemainder) -> f64 {
    turn_remainder!(angle, 360.0f64, path)
}

/// `angle % 2π` (radians, float; the turn is the float nearest 2π, twice the float π).
#[inline]
pub fn radians_remainder_with(angle: f32, path: TurnRemainder) -> f32 {
    turn_remainder!(angle, std::f32::consts::TAU, path)
}

/// The game's angle folded into [-180, 180[ degrees: remainder by a turn, then one turn off
/// or on.
#[inline]
pub fn wrap_degrees(angle: f32) -> f32 {
    let mut a = degrees_remainder(angle);
    if a >= 180.0 {
        a -= 360.0;
    }
    if a < -180.0 {
        a += 360.0;
    }
    a
}

/// [`wrap_degrees`] of a double.
#[inline]
pub fn wrap_degrees_f64(angle: f64) -> f64 {
    let mut a = degrees_remainder_f64(angle);
    if a >= 180.0 {
        a -= 360.0;
    }
    if a < -180.0 {
        a += 360.0;
    }
    a
}

/// The game's interpolation of an angle in degrees along the shorter way, from `from` to `to`
/// at `t`.
#[inline]
pub fn lerp_angle_degrees(t: f32, from: f32, to: f32) -> f32 {
    from + t * wrap_degrees(to - from)
}

/// Beyond this many radians the game's folding loops would run for thousands of turns (and
/// forever once a turn no longer changes the float): the remainder is taken first.
const RADIANS_LOOP_LIMIT: f32 = 65_536.0;

/// The game's interpolation of an angle in radians along the shorter way.
#[inline]
pub fn lerp_angle_radians(t: f32, from: f32, to: f32) -> f32 {
    from + t * fold_radians(to - from)
}

/// A difference of radians brought into [-π, π[ by whole float turns, one at a time, as the
/// game loops (no remainder call: an animation hands over less than a turn).
#[inline]
pub fn fold_radians(mut d: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    if d.abs() >= RADIANS_LOOP_LIMIT {
        // Never reached by an animation (NaN fails the test and passes through the loops
        // untouched); the game would hang here, we take the exact remainder.
        d %= TAU;
    }
    while d < -PI {
        d += TAU;
    }
    while d >= PI {
        d -= TAU;
    }
    d
}

/// `x % 1` of a float (fraction with the sign of `x`), the scroll of animated textures. Below
/// 2^23 the float minus its truncation is exact (both are within a factor 2 from 1 up), and the
/// conversion to an integer is one instruction where the library remainder and `trunc` are
/// calls; the copied sign gives a negative integer the library's -0. Beyond, every float is an
/// integer and the library answers. `RUSTCRAFT_OPT_ANGLE_WRAP_FAST_PATH=0`: always the library.
#[inline]
pub fn remainder_by_one(x: f32) -> f32 {
    remainder_by_one_with(x, TurnRemainder::selected())
}

/// [`remainder_by_one`] through an explicit path (any path but the library's takes the
/// truncation).
#[inline]
pub fn remainder_by_one_with(x: f32, path: TurnRemainder) -> f32 {
    if path != TurnRemainder::Library && x.abs() < 8_388_608.0 { (x - (x as i32) as f32).copysign(x) } else { x % 1.0 }
}

/// `x.rem_euclid(1)` (fraction in [0, 1[ for a finite `x`) without the library remainder.
#[inline]
pub fn euclid_remainder_by_one(x: f32) -> f32 {
    let r = remainder_by_one(x);
    if r < 0.0 { r + 1.0 } else { r }
}

/// `angle.rem_euclid(360)` (degrees in [0, 360[ for a finite angle) through
/// [`degrees_remainder`].
#[inline]
pub fn euclid_degrees_remainder(angle: f32) -> f32 {
    let r = degrees_remainder(angle);
    if r < 0.0 { r + 360.0 } else { r }
}

// ---------------------------------------------------------------- reals to integers

/// Rounding down then saturated conversion, as the game turns a double into an int: NaN gives 0,
/// below -2^31 and -inf the minimum, from 2^31 and +inf the maximum.
#[inline]
pub fn floor_to_i32(v: f64) -> i32 {
    // Outside ]-2^31, 2^31[ (and for NaN) the saturated truncation is already the floor's result.
    // A branch, not a select: computing both costs twice as much in range, where every caller is.
    if v.abs() < 2_147_483_648.0 { floor_in_i32_range(v).0 } else { v as i32 }
}

/// [`floor_to_i32`] of a float (the game widens it to a double first).
#[inline]
pub fn floor_f32_to_i32(v: f32) -> i32 {
    floor_to_i32(f64::from(v))
}

/// Rounding down to a long, saturated like the game (below -2^63 and -inf: the minimum, NaN 0).
#[inline]
pub fn floor_to_i64(v: f64) -> i64 {
    if v.abs() < 2_251_799_813_685_248.0 {
        // Below 2^51 the shifted floor holds the whole integer: its bits minus those of the shift.
        let floor = floor_by_shift(v);
        ((floor + ROUNDING_SHIFT).to_bits() as i64).wrapping_sub(ROUNDING_SHIFT.to_bits() as i64)
    } else {
        floor(v) as i64
    }
}

/// Floor of `|v| < 2^31` as an integer and as a double equal to `f64::from(integer)` (never
/// -0), which spares the conversion back. Outside that range the integer is meaningless.
#[inline]
pub fn floor_in_i32_range(v: f64) -> (i32, f64) {
    let floor = floor_by_shift(v);
    // The integer is in the low 32 bits of the shifted value (two's complement).
    (((floor + ROUNDING_SHIFT).to_bits() as u32) as i32, floor)
}

/// 1.5·2^52: added to `|v| < 2^51`, the sum lies in [2^52, 2^53[, where the step of a double is
/// 1, so it rounds `v` to the nearest integer, and its low bits are that integer.
const ROUNDING_SHIFT: f64 = 6_755_399_441_055_744.0;

/// Floor of `|v| < 2^51` as a double (never -0). No conversion instruction: those of x86-64
/// need extra tests to saturate, which cost more than these four operations.
#[inline]
fn floor_by_shift(v: f64) -> f64 {
    let nearest = (v + ROUNDING_SHIFT) - ROUNDING_SHIFT;
    // Written to stay branchless (mask comparison): one less if it rounded up.
    nearest - if nearest > v { 1.0 } else { 0.0 }
}

/// 2^52: from there up every double is an integer.
const TWO_POW_52: f64 = 4_503_599_627_370_496.0;

/// The game's floor of a double, bits included: -0 stays -0, NaN stays NaN (made quiet).
#[inline]
pub fn floor(v: f64) -> f64 {
    // NaN fails the test too, the infinities are beyond.
    if v.abs() < TWO_POW_52 {
        // Adding then taking off 2^52 with the sign of v rounds v to the nearest integer (the step
        // of a double is 1 at that magnitude); one less if it rounded up. The sign copied back
        // keeps -0, lost by the sum, and changes nothing else.
        let shift = TWO_POW_52.copysign(v);
        let nearest = (v + shift) - shift;
        (nearest - if nearest > v { 1.0 } else { 0.0 }).copysign(v)
    } else if v.is_nan() {
        // A signaling NaN made quiet, as the C library and the processor's rounding instruction do.
        f64::from_bits(v.to_bits() | 0x0008_0000_0000_0000)
    } else {
        v
    }
}

/// The game's ceiling of a double, bits included (a value in ]-1, 0[ gives -0).
#[inline]
pub fn ceil(v: f64) -> f64 {
    -floor(-v)
}

/// The game's rounding of a float to the nearest int, halves up: `floor(v + 1/2)` computed
/// exactly, NaN gives 0, saturated.
#[inline]
pub fn round_f32_to_i32(v: f32) -> i32 {
    // In a double the sum is exact wherever it can reach an integer (|v| < 2^52); beyond, v is
    // already an integer and the half rounds back to it.
    floor_to_i32(f64::from(v) + 0.5)
}

/// The game's rounding of a double to the nearest long, halves up, saturated, NaN gives 0.
#[inline]
pub fn round_to_i64(v: f64) -> i64 {
    // The fraction `v - floor` is exact; the sum `v + 0.5` would not be (0.49999999999999994).
    // The choice is made on doubles, then converted once with saturation.
    let down = floor(v);
    (if v - down >= 0.5 { down + 1.0 } else { down }) as i64
}

#[cfg(test)]
#[path = "java_math_tests.rs"]
mod tests;
