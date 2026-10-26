//! Heightmaps of a chunk (`anvil-world.md` § 4.5), common to the client and the integrated
//! server: values packed into 64-bit integers (first free y − minimum y) and update rule
//! after a block is placed.

/// Rule bits of the heightmaps, per state.
pub const HM_NOT_AIR: u8 = 1;
/// Tag `blocks_motion_in_heightmap` (map `OCEAN_FLOOR`).
pub const HM_MATERIAL_MOTION: u8 = 2;
/// Tag `blocks_motion_in_heightmap` or non-empty fluid (`MOTION_BLOCKING`).
pub const HM_MOTION: u8 = 4;
/// Tag `blocks_motion_in_heightmap_no_leaves` or fluid (`MOTION_BLOCKING_NO_LEAVES`).
pub const HM_MOTION_NO_LEAVES: u8 = 8;

/// Maps kept by a complete chunk (identifier, rule), in the order in which a placement
/// updates them: blocking movement, without leaves, ocean floor, surface.
pub const LIVE_HEIGHTMAPS: [(i32, u8); 4] =
    [(4, HM_MOTION), (5, HM_MOTION_NO_LEAVES), (3, HM_MATERIAL_MOTION), (1, HM_NOT_AIR)];

/// Width of a value for a world `height` blocks tall (ceiling of log2(height + 1)).
pub fn bits(height: usize) -> usize {
    (usize::BITS - height.leading_zeros()) as usize
}

/// Number of integers of a map (256 values, no value straddling two integers).
pub fn longs(height: usize) -> usize {
    256usize.div_ceil(64 / bits(height))
}

pub fn get(longs: &[i64], i: usize, bits: usize) -> i32 {
    let per = 64 / bits;
    ((longs[i / per] as u64 >> ((i % per) * bits)) & ((1u64 << bits) - 1)) as i32
}

pub fn set(longs: &mut [i64], i: usize, bits: usize, v: i32) {
    let per = 64 / bits;
    let shift = (i % per) * bits;
    let mask = ((1u64 << bits) - 1) << shift;
    let w = &mut longs[i / per];
    *w = ((*w as u64 & !mask) | (((v as u64) << shift) & mask)) as i64;
}

/// Block placed at `y` (world y) in column `i` (z × 16 + x): `hit` says whether it counts for
/// the map, `hit_at(yy)` says it for the blocks below. More than 2 below the top: nothing; block
/// that counts at or above the top: top = y + 1; block that no longer counts just below the
/// top: descent to the first that counts (otherwise the world bottom).
pub fn update(longs: &mut [i64], bits: usize, i: usize, y: i32, min_y: i32, hit: bool, hit_at: impl Fn(i32) -> bool) {
    let first = get(longs, i, bits) + min_y;
    if y <= first - 2 {
        return;
    }
    let new = if hit {
        (y >= first).then_some(y + 1)
    } else if first - 1 == y {
        Some((min_y..y).rev().find(|&yy| hit_at(yy)).map_or(min_y, |yy| yy + 1))
    } else {
        None
    };
    if let Some(v) = new {
        set(longs, i, bits, v - min_y);
    }
}
