//! Light that the post-processing of a target reads at its marked mushrooms (`Worker::process`,
//! step 4), settled from the blocks alone when they are enough, without the full light of the
//! target's 3 × 3 chunks: that resolution costs about a millisecond and runs on the generation
//! thread alone, between the parallel phases (`RUSTCRAFT_OPT_MUSHROOM_LIGHT_DECISION`).
//!
//! A marked mushroom becomes air when nothing under it lifts the light requirement and either its
//! support is not a full opaque block or its raw light reaches 13. The light is not read when the
//! support settles it, and the blocks settle the light itself in the two common cases: open sky
//! (15) and no sky source nor strong emitter within two steps (under 13, see
//! [`rc_light::raw_light_at_least`]). The others are settled exactly by a propagation over the cells
//! within two steps (`RUSTCRAFT_OPT_MUSHROOM_LIGHT_NEARBY_SEARCH`, default on), or have their chunk
//! lit as before (`=0`).

use rc_light::{Columns, LightSheets, LightTables};
use rc_worldgen::decor_blocks::Blocks;
use rc_worldgen::decor_region::DecoChunk;

/// Raw light from which a marked mushroom no longer holds (without a lifting block under it).
const MUSHROOM_LIGHT_LIMIT: u8 = 13;

/// How the light of one marked mushroom was settled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MushroomLight {
    /// The support decides alone: a block that lifts the requirement, or one that is not full.
    SupportDecides,
    /// At or above the sky source height of its stack: sky 15.
    OpenSky,
    /// No light of 13 can reach it.
    Dark,
    /// A light of 13 could reach it: the propagation over the cells within two steps tells.
    NearbySearch,
    /// Only the full light of its chunk tells (nearby search off).
    NeedsFullLight,
}

/// Marked mushrooms met, per way their light was settled (one field per [`MushroomLight`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MushroomLightCounts {
    pub(crate) support_decides: u64,
    pub(crate) open_sky: u64,
    pub(crate) dark: u64,
    pub(crate) nearby_search: u64,
    pub(crate) needs_full_light: u64,
}

impl MushroomLightCounts {
    pub(crate) fn record(&mut self, how: MushroomLight, mushrooms: u64) {
        *match how {
            MushroomLight::SupportDecides => &mut self.support_decides,
            MushroomLight::OpenSky => &mut self.open_sky,
            MushroomLight::Dark => &mut self.dark,
            MushroomLight::NearbySearch => &mut self.nearby_search,
            MushroomLight::NeedsFullLight => &mut self.needs_full_light,
        } += mushrooms;
    }

    pub(crate) fn add(&mut self, other: &MushroomLightCounts) {
        self.support_decides += other.support_decides;
        self.open_sky += other.open_sky;
        self.dark += other.dark;
        self.nearby_search += other.nearby_search;
        self.needs_full_light += other.needs_full_light;
    }

    #[cfg(test)]
    pub(crate) fn total(&self) -> u64 {
        self.support_decides + self.open_sky + self.dark + self.nearby_search + self.needs_full_light
    }
}

/// Positions marked for post-processing that hold a mushroom before it starts. Post-processing never
/// turns another block into a mushroom, so these are the only marks whose light it reads.
pub(crate) fn marked_mushrooms(c: &DecoChunk, b: &Blocks) -> Vec<[i32; 3]> {
    let k = &b.k;
    c.post_process
        .iter()
        .copied()
        .filter(|q| {
            let s = c.blocks[c.index(q[0], q[1], q[2])];
            b.is(s, k.brown_mushroom) || b.is(s, k.red_mushroom)
        })
        .collect()
}

/// What the zone needs to settle a mushroom's light: the 9 columns the full light would read (the
/// target's blocks before its post-processing in the centre), the world height, whether it has sky,
/// and whether the close cases are searched nearby (otherwise lit in full).
pub(crate) struct LightZone<'a> {
    pub(crate) tables: &'a LightTables,
    pub(crate) columns: Columns<'a>,
    pub(crate) height: i32,
    pub(crate) has_sky: bool,
    pub(crate) search_nearby: bool,
}

/// Level of the light each mushroom of `mushrooms` reads, settled without the full light (15 or 0:
/// only its side of the limit counts), each one counted in `counts`; `None` when one of them needs
/// the full light.
pub(crate) fn settle_mushroom_light(
    zone: &LightZone,
    b: &Blocks,
    c: &DecoChunk,
    mushrooms: &[[i32; 3]],
    counts: &mut MushroomLightCounts,
) -> Option<Vec<([i32; 3], u8)>> {
    let mut levels = Some(Vec::with_capacity(mushrooms.len()));
    for &p in mushrooms {
        let (how, level) = settle_one(zone, b, c, p);
        counts.record(how, 1);
        match level {
            Some(level) => {
                if let Some(v) = &mut levels {
                    v.push((p, level));
                }
            }
            // The others are still counted (statistics of the batch log).
            None => levels = None,
        }
    }
    levels
}

/// How the light of the mushroom at `p` is settled (see the module header), and the level it reads
/// (`None`: only the full light tells).
fn settle_one(zone: &LightZone, b: &Blocks, c: &DecoChunk, p: [i32; 3]) -> (MushroomLight, Option<u8>) {
    if support_decides(b, c, p) {
        return (MushroomLight::SupportDecides, Some(0));
    }
    let cell = [p[0] & 15, p[1] - c.min_y, p[2] & 15];
    let reaches = |search_nearby: bool| {
        let (t, cols) = (zone.tables, zone.columns);
        rc_light::raw_light_at_least(t, cols, zone.height, zone.has_sky, cell, MUSHROOM_LIGHT_LIMIT, search_nearby)
    };
    let level = |reached: bool| Some(if reached { 15 } else { 0 });
    match reaches(false) {
        Some(true) => (MushroomLight::OpenSky, level(true)),
        Some(false) => (MushroomLight::Dark, level(false)),
        None if zone.search_nearby => (MushroomLight::NearbySearch, reaches(true).and_then(level)),
        None => (MushroomLight::NeedsFullLight, None),
    }
}

/// The block under `p` settles its survival without the light: it lifts the requirement (stays) or
/// is not full and opaque (air). Not when that block is marked too: its own post-processing could
/// change it before the mushroom's.
fn support_decides(b: &Blocks, c: &DecoChunk, p: [i32; 3]) -> bool {
    let under = [p[0], p[1] - 1, p[2]];
    if c.post_process.contains(&under) {
        return false;
    }
    let below =
        if under[1] < c.min_y { b.default_state(b.k.void_air) } else { c.blocks[c.index(under[0], under[1], under[2])] };
    b.in_tag(below, b.rules.overrides_mushroom_light) || !b.solid_render(below)
}

/// Raw light (larger of sky and block light) at `p` in the sheets of a full light of the chunk whose
/// lowest block is at `min_y`, read like the game (`stored_light.rs`: the sky sheets the resolution
/// drops at the top read full).
pub(crate) fn raw_light_in(sheets: &LightSheets, min_y: i32, has_sky: bool, p: [i32; 3]) -> u8 {
    let (sky, block) = crate::stored_light::stored_light(&sheets.0, &sheets.1, min_y, has_sky, p);
    sky.max(block)
}

/// Tests: each settled level is on the same side of the limit as the full light of the chunk (every
/// test that generates checks it), so a wrong rule fails wherever a mushroom meets it.
#[cfg(test)]
pub(crate) fn check_against_full_light(zone: &LightZone, c: &DecoChunk, b: &Blocks, levels: &[([i32; 3], u8)]) {
    let sheets = rc_light::light_chunk(zone.tables, zone.columns, zone.height, zone.has_sky);
    for &(p, level) in levels {
        if support_decides(b, c, p) {
            continue;
        }
        let light = raw_light_in(&sheets, c.min_y, zone.has_sky, p);
        assert_eq!(
            level >= MUSHROOM_LIGHT_LIMIT,
            light >= MUSHROOM_LIGHT_LIMIT,
            "marked mushroom at {p:?}: settled level {level}, full light {light}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::raw_light_in;

    /// A sky sheet the resolution dropped from the top reads full (the old reading took it for dark,
    /// so a marked mushroom on the lowest row of an all-15 section stayed under open sky); a missing
    /// block sheet reads zero, and without sky nothing reads as sky.
    #[test]
    fn absent_sheets_read_like_the_game() {
        let mut block_sheet = [0u8; rc_world::LIGHT_BYTES];
        let i = rc_world::block_index(3, 0, 5);
        block_sheet[i / 2] = 7 << ((i & 1) * 4);
        let sky = vec![Some(std::sync::Arc::new([0u8; rc_world::LIGHT_BYTES])), None, None];
        let block = vec![None, Some(std::sync::Arc::new(block_sheet)), None];
        let sheets = (sky, block);
        let (min_y, cell) = (-64, [16 * 4 + 3, -64, 16 * 2 + 5]);
        assert_eq!(raw_light_in(&sheets, min_y, true, cell), 15);
        assert_eq!(raw_light_in(&sheets, min_y, false, cell), 7);
        assert_eq!(raw_light_in(&sheets, min_y, false, [cell[0] + 1, cell[1], cell[2]]), 0);
    }
}
