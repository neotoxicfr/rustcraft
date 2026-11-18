//! Spans of cells read column by column. The physics worlds of the client and of the server
//! store their columns differently but share these walks, so both answer the same sections
//! and the same cells for a collision walk.

use crate::Section;

/// Could one of the sections holding cells of `lo..=hi` hold a state satisfying `pred`? Answered
/// by the palettes, without reading a block. `sections(cx, cz)` gives the sections of a loaded
/// column, bottom first (`None`: not loaded); the world spans `min_y..min_y + height`. Unloaded
/// columns and cells outside the height hold nothing.
pub fn sections_may_hold<'a>(
    lo: [i32; 3],
    hi: [i32; 3],
    min_y: i32,
    height: i32,
    sections: impl Fn(i32, i32) -> Option<&'a [Section]>,
    pred: &dyn Fn(u32) -> bool,
) -> bool {
    let bottom = (lo[1] - min_y).max(0);
    let top = (hi[1] - min_y).min(height - 1);
    if bottom > top {
        return false;
    }
    let rows = (bottom >> 4) as usize..=(top >> 4) as usize;
    (lo[0] >> 4..=hi[0] >> 4).any(|cx| {
        (lo[2] >> 4..=hi[2] >> 4).any(|cz| {
            sections(cx, cz).is_some_and(|column| rows.clone().any(|i| column.get(i).is_some_and(|s| s.blocks.may_contain(pred))))
        })
    })
}

/// Writes the state of every cell of `first..=last` into `out` (x fastest, then z, then y),
/// finding each column once: `column(cx, cz)` looks it up, `state_in(column, cell)` reads one of
/// its cells.
pub fn states_by_column<C: Copy>(
    first: [i32; 3],
    last: [i32; 3],
    out: &mut [u32],
    column: impl Fn(i32, i32) -> C,
    state_in: impl Fn(C, [i32; 3]) -> u32,
) {
    let (nx, nz) = ((last[0] - first[0] + 1) as usize, (last[2] - first[2] + 1) as usize);
    for cx in first[0] >> 4..=last[0] >> 4 {
        for cz in first[2] >> 4..=last[2] >> 4 {
            let c = column(cx, cz);
            let xs = first[0].max(cx << 4)..=last[0].min((cx << 4) + 15);
            let zs = first[2].max(cz << 4)..=last[2].min((cz << 4) + 15);
            for y in first[1]..=last[1] {
                for z in zs.clone() {
                    let row = ((y - first[1]) as usize * nz + (z - first[2]) as usize) * nx;
                    for x in xs.clone() {
                        out[row + (x - first[0]) as usize] = state_in(c, [x, y, z]);
                    }
                }
            }
        }
    }
}
