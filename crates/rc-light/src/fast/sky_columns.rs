//! Sky columns of a resolution (`LIGHTSKYCOLUMNCACHE`, mission C2-M11 of series 3, `docs/perf.md`):
//! the same source heights, stack for stack, as the descent of `clear_above` from the top of the world.
//!
//! The source height of every interior stack starts with a descent from the top of the world down to
//! the zone; each stack looked its column up in the grid (a hash map on the server) and walked the
//! sections above the zone one by one, though the stacks of a chunk column share those sections. Here
//! the zone's chunk columns are fetched once per resolution, and each one's highest section that does
//! not let the sky through untouched is found once: every stack of the column starts its descent at
//! the top cell of that section.
//!
//! Why the start is exact: a clear section (`section_clear`: no attenuation, no occluding face in any
//! of its states) is skipped whole by the descent, testing only the edge between the shape above it
//! and a shapeless cell. Once one clear section is skipped the shape above is none, and two shapeless
//! cells never close an edge; the first skip tests the air above the world, checked once here. So the
//! descent over the clear sections at the top never stops, and its result only depends on the cells
//! below them.

use super::*;

/// Deliberate error of the tests: the highest section that is not clear is overlooked, the descent
/// starting at the next one down.
pub(super) const SABOTAGE_HIGHEST_SECTION_OVERLOOKED: u8 = 53;

/// A chunk column of the zone and where the descent of its stacks starts.
#[derive(Clone, Copy)]
pub(super) struct SkyColumn<'a> {
    pub(super) sections: &'a [Section],
    /// Top cell of the column's highest section that is not clear (-1 when every section is), when
    /// at least one clear section lies above it; `None`: the descent starts at the top of the world.
    pub(super) descent_start: Option<i32>,
}

/// The chunk columns of a zone, fetched once per resolution (absent ones kept as `None`).
pub(super) struct SkyColumns<'a> {
    first_cx: i32,
    first_cz: i32,
    columns_x: i32,
    columns: Vec<Option<SkyColumn<'a>>>,
}

impl<'a> SkyColumns<'a> {
    /// The columns of the zone `g` when `LIGHTSKYCOLUMNCACHE` is on (`None`: the stacks look them up).
    pub(super) fn when_cached(grid: &'a impl LightGrid, c: &Ctx, aux: &mut Aux, g: Geo) -> Option<Self> {
        c.o.sky_column_cache.then(|| SkyColumns::new(grid, c, aux, g))
    }

    /// Fetches the chunk columns of the zone `g` and finds each one's start of descent; the sections'
    /// clear flags go through the caches of `aux` (those of the zone being resolved).
    pub(super) fn new(grid: &'a impl LightGrid, c: &Ctx, aux: &mut Aux, g: Geo) -> Self {
        let (first_cx, first_cz) = (g.lo[0] >> 4, g.lo[2] >> 4);
        let (last_cx, last_cz) = ((g.lo[0] + g.size[0] - 1) >> 4, (g.lo[2] + g.size[2] - 1) >> 4);
        // The first clear section skipped from the top tests the air above the world against a
        // shapeless cell: only when that edge is open can the clear sections be passed at once.
        let can_start_lower = !c.t.occluded_faces(c.t.face(0), 0, 0, c.o.occ_tab);
        let mut columns = Vec::with_capacity(((last_cx - first_cx + 1) * (last_cz - first_cz + 1)) as usize);
        for cz in first_cz..=last_cz {
            for cx in first_cx..=last_cx {
                columns.push(grid.column(cx, cz).map(|(sections, _, _)| SkyColumn {
                    sections,
                    descent_start: if can_start_lower { descent_start(c, aux, cx, cz, sections) } else { None },
                }));
            }
        }
        SkyColumns { first_cx, first_cz, columns_x: last_cx - first_cx + 1, columns }
    }

    /// The chunk column `(cx, cz)` of the zone, `None` when it is not loaded.
    #[inline]
    pub(super) fn get(&self, cx: i32, cz: i32) -> Option<SkyColumn<'a>> {
        self.columns[((cz - self.first_cz) * self.columns_x + cx - self.first_cx) as usize]
    }
}

/// Top cell of the highest section of the column that is not clear (-1 when every section is), when
/// at least one clear section lies above it.
fn descent_start(c: &Ctx, aux: &mut Aux, cx: i32, cz: i32, sections: &[Section]) -> Option<i32> {
    let top = (c.height + 15) >> 4;
    let mut blocking = (0..top).rev().filter(|&s| !sec_clear(aux, c.t, cx, cz, Some(sections), s));
    let mut highest = blocking.next();
    if sabotage(SABOTAGE_HIGHEST_SECTION_OVERLOOKED) {
        highest = blocking.next();
    }
    let highest = highest.unwrap_or(-1);
    (highest < top - 1).then_some((highest + 1) * 16 - 1)
}
