//! Reading the light stored in a chunk's sheets, the way the game reads it. Every server reader goes
//! through [`stored_light`] (block survival and growth, mob spawning, the generator's post-processing),
//! so a missing sheet means the same thing everywhere.
//!
//! - Missing sky sheet: the game reads the bottom plane of the first stored sheet above it, or 15 when
//!   none is stored above (the light resolution drops the all-15 sheets at the top of a column).
//! - Dimension without sky light: the sky reads 0 everywhere, whatever the sheets hold.
//! - Missing block sheet: 0.

use rc_world::{LIGHT_BYTES, LightLayer};

/// Light (sky, block) stored at `p` in the sheets of a chunk whose lowest block is at `min_y`; sheet
/// `i` covers section `i − 1` (one sheet below the world, one above). Above the sheets: sky 15; under
/// them: 0 for both.
pub(crate) fn stored_light(sky: &[LightLayer], block: &[LightLayer], min_y: i32, has_sky: bool, p: [i32; 3]) -> (u8, u8) {
    let ly = p[1] - min_y;
    let Ok(sheet) = usize::try_from((ly >> 4) + 1) else { return (0, 0) };
    let cell = rc_world::block_index((p[0] & 15) as usize, (ly & 15) as usize, (p[2] & 15) as usize);
    let block_level = block.get(sheet).and_then(Option::as_ref).map_or(0, |l| nibble(l, cell));
    let sky_level = if has_sky { sky_level(sky, sheet, cell) } else { 0 };
    (sky_level, block_level)
}

fn sky_level(sky: &[LightLayer], sheet: usize, cell: usize) -> u8 {
    match sky.get(sheet) {
        None => 15,
        Some(Some(l)) => nibble(l, cell),
        // Bottom plane of the first sheet stored above: same column, y = 0 of that section.
        Some(None) => sky[sheet + 1..].iter().flatten().next().map_or(15, |above| nibble(above, cell & 0xff)),
    }
}

fn nibble(l: &[u8; LIGHT_BYTES], cell: usize) -> u8 {
    (l[cell >> 1] >> ((cell & 1) * 4)) & 15
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rc_world::{LIGHT_BYTES, LightLayer};

    use super::stored_light;

    const MIN_Y: i32 = -64;

    /// Sheet whose cells all read `level`, except the bottom plane, which reads `bottom`.
    fn sheet(level: u8, bottom: u8) -> LightLayer {
        let mut a = [level * 0x11; LIGHT_BYTES];
        a[..128].fill(bottom * 0x11);
        Some(Arc::new(a))
    }

    /// Sheets of a world of 3 sections (5 sheets: one below, three in the world, one above).
    fn sheets(sky: [LightLayer; 5]) -> (Vec<LightLayer>, Vec<LightLayer>) {
        (sky.to_vec(), vec![None, None, sheet(7, 7), None, None])
    }

    /// A missing sky sheet under a stored one reads that sheet's bottom plane; with nothing stored
    /// above, 15 (the resolution drops the all-15 sheets at the top; the old server reading took them
    /// for 0, so a crop on the bottom row of such a section saw no sky).
    #[test]
    fn missing_sky_sheet_reads_like_the_game() {
        let (sky, block) = sheets([None, sheet(3, 3), None, sheet(9, 12), None]);
        let at = |y: i32| stored_light(&sky, &block, MIN_Y, true, [5, y, -3]);
        assert_eq!(at(-64), (3, 0), "stored sheet");
        assert_eq!(at(-48), (12, 7), "missing sheet under a stored one: its bottom plane");
        assert_eq!(at(-33), (12, 7), "every row of the missing sheet reads the same plane");
        assert_eq!(at(-32), (12, 0), "bottom plane of the stored sheet");
        assert_eq!(at(-31), (9, 0), "stored sheet above its bottom plane");
        let (top_dropped, block) = sheets([None, sheet(3, 3), None, None, None]);
        assert_eq!(stored_light(&top_dropped, &block, MIN_Y, true, [0, -48, 0]), (15, 7), "nothing stored above");
        assert_eq!(stored_light(&top_dropped, &block, MIN_Y, true, [0, -16, 0]), (15, 0), "above the world");
        assert_eq!(stored_light(&top_dropped, &block, MIN_Y, true, [0, 100, 0]), (15, 0), "beyond the sheets");
        assert_eq!(stored_light(&top_dropped, &block, MIN_Y, true, [0, -65, 0]), (3, 0), "sheet below the world");
        assert_eq!(stored_light(&top_dropped, &block, MIN_Y, true, [0, -81, 0]), (0, 0), "under the sheets");
    }

    /// Without sky light (Nether, End), the sky reads 0 whatever the sheets say (the old mob reading
    /// found no sheet there and read 15: a monster was refused about half the time for brightness).
    #[test]
    fn no_sky_dimension_reads_no_sky_light() {
        let (sky, block) = sheets([None, None, sheet(15, 15), None, None]);
        for y in [-64, -48, -40, 0, 100] {
            assert_eq!(stored_light(&sky, &block, MIN_Y, false, [1, y, 1]).0, 0, "y {y}");
        }
        assert_eq!(stored_light(&sky, &block, MIN_Y, false, [1, -40, 1]), (0, 7));
    }
}
