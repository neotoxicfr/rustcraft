//! References of a chunk (`generation-decoration.md` § 7.4): the structure starts of the chunks
//! within 8 whose box reaches its columns. A chunk needs them for its terrain (beard), for its
//! saved `structures` compound and for its decoration (piece placement).
//!
//! Nearly every neighbour holds no start at all, so the walk first finds the *candidate* chunks
//! (those where some set places a structure) and only reads the starts of these. Asking every set
//! about each of the 289 neighbours costs thousands of seeded draws per walk; a random-spread set
//! holds one chunk per region of `spacing` chunks, so the candidates of a whole window come from
//! the few regions it crosses ([`ReferencePaths::candidates_by_region`]), and the strongholds
//! that can land in the window are picked once for it ([`ReferencePaths::ring_window_filter`]).

use std::collections::BTreeMap;
use std::ops::ControlFlow;

use hashbrown::HashMap;

use super::{LongSet, Placement, RING_REACH, Start, Starts, StructureGen, chunk_key, rings_lazy, starts_prefilter, unkey};
use crate::decor_blocks::Blocks;
use crate::rng::LegacyRandom;
use crate::terrain::{ChunkRect, TerrainGenerator};

/// Chunks looked at on each side of a chunk for its references.
pub const REFERENCE_REACH: i32 = 8;

/// References of a chunk: per structure (registry order), the start lists of the chunks holding a
/// start of it that reaches the chunk, in the game's iteration order.
pub type References = BTreeMap<usize, Vec<Starts>>;

/// Paths of the reference walk. Default: `RUSTCRAFT_OPT_*` switches read at startup; the
/// equivalence tests set them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReferencePaths {
    /// `STARTS_PREFILTRE`: a neighbour where no set places a structure is skipped without touching
    /// the start cache. The two paths below only act with it.
    pub skip_chunks_without_candidate: bool,
    /// `CANDIDATES_BY_REGION`: the candidates of a random-spread set come from the regions the
    /// window crosses, not from a question per chunk.
    pub candidates_by_region: bool,
    /// `RING_WINDOW_FILTER`: the strongholds that can land in the window are picked once for the
    /// window, not once per chunk.
    pub ring_window_filter: bool,
    /// `EMPTY_STARTS_NOT_CLONED`: a neighbour without start is skipped without taking a handle on
    /// the shared empty list.
    pub empty_lists_not_cloned: bool,
    #[doc(hidden)]
    pub sabotage: ReferenceSabotage,
}

impl ReferencePaths {
    /// Every neighbour's starts read (before the prefilter).
    pub const UNFILTERED: ReferencePaths = ReferencePaths {
        skip_chunks_without_candidate: false,
        candidates_by_region: false,
        ring_window_filter: false,
        empty_lists_not_cloned: false,
        sabotage: ReferenceSabotage::None,
    };
    /// Prefilter asked chunk by chunk and set by set.
    pub const PER_CHUNK: ReferencePaths = ReferencePaths { skip_chunks_without_candidate: true, ..Self::UNFILTERED };
    pub const NEW: ReferencePaths = ReferencePaths {
        candidates_by_region: true,
        ring_window_filter: true,
        empty_lists_not_cloned: true,
        ..Self::PER_CHUNK
    };

    pub fn from_env() -> Self {
        ReferencePaths {
            skip_chunks_without_candidate: starts_prefilter(),
            candidates_by_region: crate::opt::candidates_by_region(),
            ring_window_filter: crate::opt::ring_window_filter(),
            empty_lists_not_cloned: crate::opt::empty_starts_not_cloned(),
            sabotage: ReferenceSabotage::None,
        }
    }

    /// The candidates of the window are found before the walk (otherwise chunk by chunk during it).
    fn finds_candidates_per_window(&self) -> bool {
        self.skip_chunks_without_candidate && (self.candidates_by_region || self.ring_window_filter)
    }
}

/// Deliberate errors of the per-window candidate search, for the tests that must see them.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ReferenceSabotage {
    #[default]
    None,
    /// The regions walked start one region too far in x.
    RegionShiftedByOne,
    /// A triangular spread drawn like a linear one.
    TriangularAsLinear,
    /// Exclusion zones ignored.
    ExclusionOmitted,
    /// Stronghold draws picked without the reach margin around the window.
    RingDrawsWithoutReach,
    /// Candidates marked with x and z swapped inside the window.
    WindowTransposed,
}

/// Square of chunks of side `2 · reach + 1` around (cx, cz).
fn chunks_around(cx: i32, cz: i32, reach: i32) -> ChunkRect {
    ChunkRect { min_x: cx - reach, min_z: cz - reach, width: 2 * reach + 1, depth: 2 * reach + 1 }
}

/// Chunks of the reference window of a chunk where some set places a structure: one bit per
/// chunk of the 17 × 17 window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CandidateWindow {
    window: ChunkRect,
    bits: [u64; 5],
}

impl CandidateWindow {
    fn around(cx: i32, cz: i32) -> Self {
        CandidateWindow { window: chunks_around(cx, cz, REFERENCE_REACH), bits: [0; 5] }
    }

    fn mark(&mut self, x: i32, z: i32) {
        if let Some(i) = self.window.index(x, z) {
            self.bits[i / 64] |= 1 << (i % 64);
        }
    }

    pub(super) fn contains(&self, x: i32, z: i32) -> bool {
        self.window.index(x, z).is_some_and(|i| self.bits[i / 64] >> (i % 64) & 1 != 0)
    }
}

/// One random-spread placement, as the per-window search reads it.
struct Spread {
    spacing: i32,
    /// Bound (excluded) of the offset drawn inside a region: spacing − separation.
    offset_bound: i32,
    triangular: bool,
    salt: i32,
}

impl Spread {
    /// The game requires 0 ≤ separation < spacing; anything else is asked chunk by chunk, like
    /// the old path (which fails the same way the game's data check would).
    fn is_well_formed(&self) -> bool {
        self.spacing >= 1 && self.offset_bound >= 1
    }

    /// The one chunk of region (gx, gz) where the set may place a structure.
    fn region_chunk(&self, world_seed: i64, gx: i32, gz: i32) -> (i32, i32) {
        // A bound of 1 only ever draws 0 (spacing 1: buried treasures and mineshafts; nether
        // fossils): the chunk is the first of its region and the region's seed is not needed.
        if self.offset_bound == 1 {
            return (gx * self.spacing, gz * self.spacing);
        }
        let mut rng = LegacyRandom::new(
            i64::from(gx)
                .wrapping_mul(341873128712)
                .wrapping_add(i64::from(gz).wrapping_mul(132897987541))
                .wrapping_add(world_seed)
                .wrapping_add(i64::from(self.salt)),
        );
        let bound = self.offset_bound;
        let mut offset = || {
            if self.triangular { (rng.next_int_bounded(bound) + rng.next_int_bounded(bound)) / 2 } else { rng.next_int_bounded(bound) }
        };
        let offset_x = offset();
        let offset_z = offset();
        (gx * self.spacing + offset_x, gz * self.spacing + offset_z)
    }
}

/// What a visitor of structure chunks answers: go on, or stop the search.
type Visit<'a> = &'a mut dyn FnMut(i32, i32) -> ControlFlow<()>;

impl StructureGen {
    /// References of the chunk (§ 7.4): starts of the chunks within 8 whose box intersects its
    /// columns, per structure (registry order), in the game's iteration order.
    pub fn references(&self, cx: i32, cz: i32, t: &TerrainGenerator, b: &Blocks) -> References {
        self.references_with(cx, cz, t, b, self.reference_paths)
    }

    /// Is the neighbour a candidate chunk of at least one set? Otherwise its starts are
    /// surely empty: each attempt of [`Self::compute_starts`] goes through this same test.
    pub(super) fn any_structure_chunk(&self, cx: i32, cz: i32, t: &TerrainGenerator) -> bool {
        (0..self.sets.len()).any(|si| self.is_structure_chunk(si, cx, cz, t))
    }

    /// [`Self::references`] through the given paths.
    #[doc(hidden)]
    pub fn references_with(
        &self,
        cx: i32,
        cz: i32,
        t: &TerrainGenerator,
        b: &Blocks,
        paths: ReferencePaths,
    ) -> References {
        let candidates = paths.finds_candidates_per_window().then(|| self.candidate_window(cx, cz, t, paths));
        let mut keys: BTreeMap<usize, LongSet> = BTreeMap::new();
        let mut found: HashMap<(i32, i32), Starts> = HashMap::new();
        let (x0, z0) = (cx * 16, cz * 16);
        // The order of this double loop is the game's: the start cache is filled and the keys are
        // inserted in it, whichever way the candidates were found.
        for sx in cx - REFERENCE_REACH..=cx + REFERENCE_REACH {
            for sz in cz - REFERENCE_REACH..=cz + REFERENCE_REACH {
                let candidate = match &candidates {
                    Some(window) => window.contains(sx, sz),
                    None => !paths.skip_chunks_without_candidate || self.any_structure_chunk(sx, sz, t),
                };
                if !candidate {
                    continue;
                }
                let Some(st) = self.starts_of_candidate(sx, sz, t, b, paths) else { continue };
                for s in st.iter() {
                    if s.ref_box(&self.defs).intersects_xz(x0, z0, x0 + 15, z0 + 15) {
                        keys.entry(s.structure).or_default().add(chunk_key(sx, sz));
                        found.insert((sx, sz), st.clone());
                    }
                }
            }
        }
        keys.into_iter()
            .map(|(s, set)| (s, set.iter().filter_map(|k| found.get(&unkey(k)).cloned()).collect()))
            .collect()
    }

    /// Start list of a candidate neighbour, `None` when it holds no start (most candidates: the
    /// biome or the terrain refuses the structure).
    fn starts_of_candidate(&self, sx: i32, sz: i32, t: &TerrainGenerator, b: &Blocks, paths: ReferencePaths) -> Option<Starts> {
        if paths.empty_lists_not_cloned {
            return self.starts_if_any(sx, sz, t, b);
        }
        let starts = self.starts(sx, sz, t, b);
        (!starts.is_empty()).then_some(starts)
    }

    /// Candidate chunks of the reference window of (cx, cz): the chunks
    /// [`Self::any_structure_chunk`] says yes to, found set by set over the whole window.
    pub(super) fn candidate_window(&self, cx: i32, cz: i32, t: &TerrainGenerator, paths: ReferencePaths) -> CandidateWindow {
        let mut candidates = CandidateWindow::around(cx, cz);
        let window = candidates.window;
        let lazy = rings_lazy();
        let transposed = paths.sabotage == ReferenceSabotage::WindowTransposed;
        for set in 0..self.sets.len() {
            let _ = self.structure_chunks_in(set, window, t, lazy, paths, &mut |x, z| {
                if transposed {
                    candidates.mark(window.min_x + (z - window.min_z), window.min_z + (x - window.min_x));
                } else {
                    candidates.mark(x, z);
                }
                ControlFlow::Continue(())
            });
        }
        candidates
    }

    /// Visits the chunks of `area` where the set places a structure (those
    /// [`Self::is_structure_chunk`] says yes to), until `visit` stops the search.
    fn structure_chunks_in(
        &self,
        set: usize,
        area: ChunkRect,
        t: &TerrainGenerator,
        lazy: bool,
        paths: ReferencePaths,
        visit: Visit,
    ) -> ControlFlow<()> {
        match &self.sets[set].placement {
            Placement::Random { spacing, separation, triangular, salt, .. } if paths.candidates_by_region => {
                let spread =
                    Spread { spacing: *spacing, offset_bound: spacing - separation, triangular: *triangular, salt: *salt };
                if spread.is_well_formed() {
                    return self.spread_chunks_in(set, &spread, area, t, lazy, paths, visit);
                }
            }
            Placement::Rings { .. } if paths.ring_window_filter => {
                return self.ring_chunks_in(set, area, t, lazy, paths, visit);
            }
            Placement::Origin => {
                let (x, z) = self.origin;
                return if area.index(x, z).is_some() { visit(x, z) } else { ControlFlow::Continue(()) };
            }
            _ => {}
        }
        // Chunk by chunk (path switched off, or a spacing the game refuses).
        for x in area.min_x..area.min_x + area.width {
            for z in area.min_z..area.min_z + area.depth {
                if self.is_structure_chunk_with(set, x, z, t, lazy) {
                    visit(x, z)?;
                }
            }
        }
        ControlFlow::Continue(())
    }

    /// Random-spread set: the chunk of each region `area` crosses, when it lies in `area` and
    /// passes the frequency reducer and the exclusion zone (the checks of
    /// [`Self::is_structure_chunk`], in the same order). A 17 × 17 window crosses at most 4
    /// regions of a set whose spacing is 17 or more, against 289 questions chunk by chunk.
    #[allow(clippy::too_many_arguments)]
    fn spread_chunks_in(
        &self,
        set: usize,
        spread: &Spread,
        area: ChunkRect,
        t: &TerrainGenerator,
        lazy: bool,
        paths: ReferencePaths,
        visit: Visit,
    ) -> ControlFlow<()> {
        let Placement::Random { frequency, reducer, exclusion, .. } = &self.sets[set].placement else {
            return ControlFlow::Continue(());
        };
        let (max_x, max_z) = (area.min_x + area.width - 1, area.min_z + area.depth - 1);
        let shift = i32::from(paths.sabotage == ReferenceSabotage::RegionShiftedByOne);
        let linear = paths.sabotage == ReferenceSabotage::TriangularAsLinear;
        let spread = Spread { triangular: spread.triangular && !linear, ..*spread };
        for gx in area.min_x.div_euclid(spread.spacing) + shift..=max_x.div_euclid(spread.spacing) + shift {
            for gz in area.min_z.div_euclid(spread.spacing)..=max_z.div_euclid(spread.spacing) {
                #[cfg(test)]
                self.count(|c| &c.region_draws, u64::from(spread.offset_bound != 1));
                let (x, z) = spread.region_chunk(self.seed, gx, gz);
                if area.index(x, z).is_none() {
                    continue;
                }
                if *frequency < 1.0 && !self.reduce(*reducer, spread.salt, x, z, *frequency) {
                    continue;
                }
                if self.excluded_in_area(exclusion, x, z, t, lazy, paths) {
                    continue;
                }
                visit(x, z)?;
            }
        }
        ControlFlow::Continue(())
    }

    /// Concentric rings: only the strongholds drawn within [`RING_REACH`] of `area` can land in
    /// it, so they are picked once for the whole area (128 comparisons) instead of once per chunk
    /// (289 × 128 for a reference window). Same checks as [`Self::is_structure_chunk`] then.
    fn ring_chunks_in(
        &self,
        set: usize,
        area: ChunkRect,
        t: &TerrainGenerator,
        lazy: bool,
        paths: ReferencePaths,
        visit: Visit,
    ) -> ControlFlow<()> {
        let Placement::Rings { salt, frequency, reducer, preferred, jobs, exclusion, .. } = &self.sets[set].placement
        else {
            return ControlFlow::Continue(());
        };
        let mut visit_landing = |x: i32, z: i32| {
            let lands_here = area.index(x, z).is_some()
                && (*frequency >= 1.0 || self.reduce(*reducer, *salt, x, z, *frequency))
                && !self.excluded_in_area(exclusion, x, z, t, lazy, paths);
            if lands_here { visit(x, z) } else { ControlFlow::Continue(()) }
        };
        if !lazy {
            for &(x, z) in self.ring_positions_old(set, t).iter() {
                visit_landing(x, z)?;
            }
            return ControlFlow::Continue(());
        }
        let reach = if paths.sabotage == ReferenceSabotage::RingDrawsWithoutReach { 0 } else { RING_REACH as i32 };
        let draws_in_reach = ChunkRect {
            min_x: area.min_x - reach,
            min_z: area.min_z - reach,
            width: area.width + 2 * reach,
            depth: area.depth + 2 * reach,
        };
        #[cfg(test)]
        self.count(|c| &c.ring_draws_compared, jobs.jobs.len() as u64);
        for (i, &(x0, z0, _)) in jobs.jobs.iter().enumerate() {
            if draws_in_reach.index(x0, z0).is_none() {
                continue;
            }
            let (x, z) = self.ring_cell(jobs, preferred, i, t);
            // Same bound as the chunk-by-chunk test: a stronghold further than the reach from
            // its draw is ignored there too.
            if x0.abs_diff(x) <= RING_REACH && z0.abs_diff(z) <= RING_REACH {
                visit_landing(x, z)?;
            }
        }
        ControlFlow::Continue(())
    }

    /// [`Self::excluded`] asked over the whole square at once: is there a chunk of the excluded
    /// set within `reach` chunks?
    fn excluded_in_area(
        &self,
        exclusion: &Option<(String, i32)>,
        cx: i32,
        cz: i32,
        t: &TerrainGenerator,
        lazy: bool,
        paths: ReferencePaths,
    ) -> bool {
        if paths.sabotage == ReferenceSabotage::ExclusionOmitted {
            return false;
        }
        let Some((other, reach)) = exclusion else { return false };
        let Some(other) = self.set_index(other) else { return false };
        self.structure_chunks_in(other, chunks_around(cx, cz, *reach), t, lazy, paths, &mut |_, _| ControlFlow::Break(()))
            .is_break()
    }

    /// Starts of each structure referenced by the chunk, in reference order, every start copied
    /// (old path of `RUSTCRAFT_OPT_REFERENCED_WITHOUT_CLONE`; see [`referenced_starts`]).
    pub fn referenced(&self, cx: i32, cz: i32, t: &TerrainGenerator, b: &Blocks) -> Vec<(usize, Vec<Start>)> {
        referenced_starts_cloned(&self.references(cx, cz, t, b))
    }
}

/// Starts of each referenced structure, in reference order, borrowed from the reference lists:
/// a start (its pieces, their junctions) is never copied, the caller keeps `references` alive.
pub fn referenced_starts(references: &References) -> Vec<(usize, Vec<&Start>)> {
    references
        .iter()
        .map(|(&structure, lists)| {
            // A chunk's list also holds its starts of other structures: only this one's count.
            (structure, lists.iter().flat_map(|list| list.iter().filter(move |start| start.structure == structure)).collect())
        })
        .collect()
}

/// [`referenced_starts`] with every start copied (pieces and junctions included).
pub fn referenced_starts_cloned(references: &References) -> Vec<(usize, Vec<Start>)> {
    references
        .iter()
        .map(|(&structure, lists)| {
            (structure, lists.iter().flat_map(|list| list.iter().filter(|start| start.structure == structure).cloned()).collect())
        })
        .collect()
}

#[cfg(test)]
#[path = "structure_references_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "structure_reference_use_tests.rs"]
mod use_tests;
