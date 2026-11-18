//! Biome search by columns of quarts (`RUSTCRAFT_OPT_BIOME_COLUMN_SEARCH`).
//!
//! Along a column of a chunk's biome fill, five climate parameters (temperature, humidity,
//! continentalness, erosion, weirdness) stay the same and only the depth changes. The distance of a
//! leaf to a target then splits in two: its distance over the six dimensions that stay constant
//! (those five and the offset), the same at every height, plus the square of its depth distance.
//! Leaves that share a depth interval keep their order at every height, so per depth interval only
//! the nearest leaf over the constant dimensions can win anywhere in the column: one walk of the
//! tree per column finds them all, and each quart only compares a handful of candidates.
//!
//! The chained search of the game is reproduced exactly in the no-overflow regime ([`SMALL`]): it
//! returns the last leaf when that leaf is at the minimal distance, otherwise the first leaf at the
//! minimal distance in the walk order of the tree (proved for the two-phase `/locate biome`, see
//! `docs/perf.md`). A column outside that regime, or whose five parameters change with height,
//! goes through the chained search.

use super::{ClimateList, Flat, Param, SMALL, Searcher, TreePaths, param_distance, simd_dists, space_distance};

/// Index of the one climate parameter that changes along a column: the depth.
pub(super) const DEPTH: usize = 4;
/// Dimensions that stay constant along a column: the five other climate parameters and the offset.
const CONSTANT_DIMENSIONS: [usize; 6] = [0, 1, 2, 3, 5, 6];
/// Most distinct depth intervals a list can have for the column search (one bit each in a mask).
const MAX_DEPTH_GROUPS: usize = 32;
/// Columns of quarts in a chunk (4 × 4).
const COLUMNS: usize = 16;
/// Quarts of one section of a chunk (4 × 4 × 4), the unit of the fill order.
pub(super) const SECTION_QUARTS: usize = 64;

/// What the column search needs on top of the flattened tree: the leaves grouped by depth
/// interval, the groups found under each node, and each leaf's rank in the walk order of the search.
#[derive(Debug, Clone)]
pub(super) struct DepthGroups {
    /// Distinct depth intervals of the leaves, in order of first appearance.
    intervals: Vec<Param>,
    /// Per flattened node: bit `g` set when a leaf of group `g` lies under it.
    masks: Vec<u32>,
    /// Per flattened leaf: its group.
    group: Vec<u8>,
    /// Per flattened leaf: its rank in the depth-first walk of the tree.
    walk_rank: Vec<u32>,
}

/// Paths of the column search, all with the same result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColumnPaths {
    /// Each column's walk starts bounded by the winners of a neighbouring column instead of
    /// unbounded (`RUSTCRAFT_OPT_BIOME_COLUMN_SEED`).
    pub seeded: bool,
    /// Distances of a node's children computed 8 at a time in AVX2, if the processor has it
    /// (`RUSTCRAFT_OPT_BTREE_SIMD`, like the quart search).
    pub simd: bool,
}

impl ColumnPaths {
    /// Paths chosen by the environment (read once).
    pub fn current() -> Self {
        ColumnPaths { seeded: rc_data::opt_switch!("RUSTCRAFT_OPT_BIOME_COLUMN_SEED"), simd: crate::opt::btree_simd() }
    }
}

/// Sabotages of the column search: the equivalence tests must reject each of them.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnSabotage {
    /// Inside a depth group, a later leaf at the same distance replaces the earlier one.
    LaterLeafWinsTies,
    /// Between depth groups, the later group wins a tie.
    LaterGroupWinsTies,
    /// The last leaf is ignored: always the first leaf at the minimal distance.
    LastLeafIgnored,
}

impl DepthGroups {
    /// `None` when the column search cannot be exact on this tree: a bound outside the no-overflow
    /// regime or an inverted interval (a node's box would no longer bound the distances of its
    /// leaves), a root that is a leaf, or more depth intervals than a mask holds.
    pub(super) fn new(f: &Flat) -> Option<Self> {
        if !f.simd || f.count[0] == 0 {
            return None;
        }
        let n = f.orig.len();
        let mut intervals: Vec<Param> = Vec::new();
        let mut group = vec![0u8; n];
        for (i, g) in group.iter_mut().enumerate().filter(|(i, _)| f.count[*i] == 0) {
            let depth = (i64::from(f.lo[DEPTH][i]), i64::from(f.hi[DEPTH][i]));
            let at = match intervals.iter().position(|p| *p == depth) {
                Some(at) => at,
                None if intervals.len() < MAX_DEPTH_GROUPS => {
                    intervals.push(depth);
                    intervals.len() - 1
                }
                None => return None,
            };
            *g = at as u8;
        }
        // Children are numbered after their parent: walking the nodes backwards meets them first.
        let mut masks = vec![0u32; n];
        for i in (0..n).rev() {
            masks[i] = match f.children(i) {
                None => 1 << group[i],
                Some(children) => masks[children].iter().fold(0, |m, c| m | c),
            };
        }
        Some(DepthGroups { intervals, masks, group, walk_rank: walk_ranks(f) })
    }

    /// Number of depth groups.
    fn len(&self) -> usize {
        self.intervals.len()
    }

    /// Nearest leaf of each depth group to `t` over the constant dimensions (the depth of `t` is
    /// not read), the first leaf in walk order on ties. `seeds`: the winners of a neighbouring
    /// column, whose distances to `t` bound the walk.
    fn nearest_per_group(
        &self,
        f: &Flat,
        t: &[i64; 7],
        best: &mut [GroupBest],
        seeds: Option<&[GroupBest]>,
        paths: ColumnPaths,
        sabotage: Option<ColumnSabotage>,
    ) {
        match seeds {
            // Bound d + 1, the seed itself not kept: the walk still meets it or a nearer leaf, and
            // keeps the first of them in walk order (a seed kept as the best would win ties against
            // the leaves walked before it).
            Some(seeds) => {
                for (b, seed) in best.iter_mut().zip(seeds) {
                    *b =
                        GroupBest { distance: distance_without_depth(f, seed.leaf as usize, t) + 1, ..GroupBest::NONE };
                }
            }
            None => best.fill(GroupBest::NONE),
        }
        // In AVX2, the target narrowed to 32 bits (no-overflow regime checked by the caller).
        let t32 = (paths.simd && rc_data::cpu::has_avx2()).then(|| t.map(|v| v as i32));
        if sabotage == Some(ColumnSabotage::LaterLeafWinsTies) {
            self.walk::<true>(f, 0, t, t32.as_ref(), best);
        } else {
            self.walk::<false>(f, 0, t, t32.as_ref(), best);
        }
    }

    /// Depth-first walk under `node`, children in tree order like the search of the game. A child
    /// is skipped when it cannot strictly beat the best of every group under it: the leaves met
    /// later in the walk lose ties. `t32`: the children's distances computed 8 at a time in AVX2.
    /// `LATER_WINS`: sabotage (ties go to the later leaf).
    fn walk<const LATER_WINS: bool>(
        &self,
        f: &Flat,
        node: usize,
        t: &[i64; 7],
        t32: Option<&[i32; 7]>,
        best: &mut [GroupBest],
    ) {
        let Some(children) = f.children(node) else { return };
        let mut batch = [0i64; 24];
        if let Some(t32) = t32 {
            simd_dists::<true>(f, children.start, children.len(), t32, &mut batch);
        }
        for (j, c) in children.enumerate() {
            let d = if t32.is_some() { batch[j] } else { distance_without_depth(f, c, t) };
            let bound = self.best_under(c, best);
            if if LATER_WINS { d > bound } else { d >= bound } {
                continue;
            }
            if f.count[c] == 0 {
                best[usize::from(self.group[c])] = GroupBest { distance: d, leaf: c as u32, rank: self.walk_rank[c] };
            } else {
                self.walk::<LATER_WINS>(f, c, t, t32, best);
            }
        }
    }

    /// Worst of the current bests of the groups under node `c`: a leaf of `c` closer than that can
    /// still improve one of them.
    fn best_under(&self, c: usize, best: &[GroupBest]) -> i64 {
        let mut mask = self.masks[c];
        let mut worst = i64::MIN;
        while mask != 0 {
            worst = worst.max(best[mask.trailing_zeros() as usize].distance);
            mask &= mask - 1;
        }
        worst
    }

    /// Nearest leaf at `depth` among the group bests of a column: minimal distance, flattened leaf
    /// that reaches it first in walk order, and the distance of the flattened leaf `last` when it is
    /// one of the group bests (the usual case: then its distance is already known).
    fn nearest_at_depth(
        &self,
        best: &[GroupBest],
        depth: i64,
        last: Option<u32>,
        sabotage: Option<ColumnSabotage>,
    ) -> (i64, u32, Option<i64>) {
        let later_group_wins = sabotage == Some(ColumnSabotage::LaterGroupWinsTies);
        let (mut nearest, mut first) = (i64::MAX, GroupBest::NONE);
        let mut last_distance = None;
        for (b, &interval) in best.iter().zip(&self.intervals) {
            let x = param_distance(interval, depth);
            let d = b.distance + x * x;
            if Some(b.leaf) == last {
                last_distance = Some(d);
            }
            let wins_tie = later_group_wins || b.rank < first.rank;
            if d < nearest || (d == nearest && wins_tie) {
                (nearest, first) = (d, *b);
            }
        }
        (nearest, first.leaf, last_distance)
    }
}

/// Nearest leaf of one depth group over the constant dimensions of a column.
#[derive(Debug, Clone, Copy)]
struct GroupBest {
    distance: i64,
    /// Flattened leaf.
    leaf: u32,
    /// Its rank in walk order (ties between groups go to the lower one).
    rank: u32,
}

impl GroupBest {
    /// Nothing found yet.
    const NONE: Self = GroupBest { distance: i64::MAX, leaf: u32::MAX, rank: u32::MAX };
}

impl Flat {
    /// Flattened children of node `n`, `None` for a leaf.
    fn children(&self, n: usize) -> Option<std::ops::Range<usize>> {
        let first = self.first[n] as usize;
        (self.count[n] != 0).then(|| first..first + usize::from(self.count[n]))
    }
}

/// Rank of every flattened leaf in the depth-first walk of the tree, children in tree order.
fn walk_ranks(f: &Flat) -> Vec<u32> {
    let mut ranks = vec![u32::MAX; f.orig.len()];
    let (mut stack, mut next) = (vec![0usize], 0u32);
    while let Some(n) = stack.pop() {
        match f.children(n) {
            // Pushed backwards so that the first child comes out first.
            Some(children) => stack.extend(children.rev()),
            None => {
                ranks[n] = next;
                next += 1;
            }
        }
    }
    ranks
}

/// Distance of node `n` to `t` over the constant dimensions (sum of squares, exact in the
/// no-overflow regime).
pub(super) fn distance_without_depth(f: &Flat, n: usize, t: &[i64; 7]) -> i64 {
    let mut d = 0i64;
    for dim in CONSTANT_DIMENSIONS {
        let x = param_distance((i64::from(f.lo[dim][n]), i64::from(f.hi[dim][n])), t[dim]);
        d += x * x;
    }
    d
}

/// Index in fill order (sections from the bottom, then x, y, z) of the quart (x, qy, z).
fn fill_index(x: usize, qy: usize, z: usize) -> usize {
    (((qy >> 2) * 4 + x) * 4 + (qy & 3)) * 4 + z
}

/// Column (x · 4 + z) of the quart at fill-order index `i`.
fn column_of(i: usize) -> usize {
    ((i >> 4) & 3) * 4 + (i & 3)
}

/// Quart (x, qy, z) at fill-order index `i`.
pub(super) fn fill_position(i: usize) -> (i32, i32, i32) {
    let qy = (i >> 6) * 4 + ((i >> 2) & 3);
    (((i >> 4) & 3) as i32, qy as i32, (i & 3) as i32)
}

/// Climates of a chunk's whole sections (`height_q` quarts high) in fill order, `climate(x, qy, z)`
/// giving each quart's.
pub(super) fn climates_in_fill_order(height_q: i32, climate: impl Fn(i32, i32, i32) -> [i64; 6]) -> Vec<[i64; 6]> {
    let quarts = (height_q / 4).max(0) as usize * SECTION_QUARTS;
    (0..quarts)
        .map(|i| {
            let (x, qy, z) = fill_position(i);
            climate(x, qy, z)
        })
        .collect()
}

/// Climate shared by the `heights` quarts of column `c` (depth and offset at 0), when the column
/// search is exact there: the five parameters other than depth equal at every height, and every
/// value of the column inside the no-overflow regime.
fn constant_climate(ts: &[[i64; 6]], c: usize, heights: usize) -> Option<[i64; 7]> {
    let (x, z) = (c / 4, c % 4);
    let first = ts[fill_index(x, 0, z)];
    for qy in 0..heights {
        let t = ts[fill_index(x, qy, z)];
        let constant = (0..6).all(|k| k == DEPTH || t[k] == first[k]);
        if !constant || t.iter().any(|v| v.unsigned_abs() > SMALL) {
            return None;
        }
    }
    Some([first[0], first[1], first[2], first[3], 0, first[5], 0])
}

impl ClimateList {
    /// Does this list's tree allow the column search (otherwise every quart takes the chained
    /// search)?
    pub fn column_search_ready(&self) -> bool {
        self.depth_groups.is_some()
    }

    /// Leaves of a chunk's quarts given their climates `ts` in fill order (whole sections):
    /// `found(index, leaf)` in that order, with the same leaves and the same last leaf left in `s`
    /// as the chained search called quart after quart through paths `p`.
    pub(super) fn leaves_by_columns(
        &self,
        ts: &[[i64; 6]],
        s: &mut Searcher,
        p: TreePaths,
        paths: ColumnPaths,
        sabotage: Option<ColumnSabotage>,
        mut found: impl FnMut(usize, u32),
    ) {
        assert!(ts.len().is_multiple_of(SECTION_QUARTS), "climates of whole sections expected");
        let (Some(groups), Some(f)) = (self.depth_groups.as_deref(), self.flat.as_deref()) else {
            for (i, t) in ts.iter().enumerate() {
                found(i, self.find_leaf::<false>(*t, s, p));
            }
            return;
        };
        if ts.is_empty() {
            return;
        }
        let n = groups.len();
        let (best, by_column) = groups.column_bests(f, ts, paths, sabotage);
        // Serial part, in fill order: the last leaf when it is at the minimal distance.
        for (i, t) in ts.iter().enumerate() {
            let c = column_of(i);
            if !by_column[c] {
                found(i, self.find_leaf::<false>(*t, s, p));
                continue;
            }
            let last = s.last.map(|l| f.of_old[l as usize]);
            let (dmin, first, known) = groups.nearest_at_depth(&best[c * n..(c + 1) * n], t[DEPTH], last, sabotage);
            let last_distance = |l: u32| {
                known.unwrap_or_else(|| {
                    let t7 = [t[0], t[1], t[2], t[3], t[4], t[5], 0];
                    space_distance(self.tree.nodes[l as usize].space(), &t7)
                })
            };
            let leaf = match s.last {
                Some(l) if sabotage != Some(ColumnSabotage::LastLeafIgnored) && last_distance(l) == dmin => l,
                _ => f.orig[first as usize],
            };
            s.last = Some(leaf);
            found(i, leaf);
        }
    }

    /// Entries of a chunk's quarts through the column search, climates `ts` in fill order (whole
    /// sections): same entries as [`Self::find_entry`] called quart after quart with `s`
    /// (equivalence tests and benches).
    pub fn find_entries_by_columns(
        &self,
        ts: &[[i64; 6]],
        s: &mut Searcher,
        p: TreePaths,
        paths: ColumnPaths,
    ) -> Vec<usize> {
        let mut out = vec![0; ts.len()];
        self.leaves_by_columns(ts, s, p, paths, None, |i, leaf| out[i] = leaf as usize);
        out
    }

    /// Sabotaged column search: the equivalence tests must reject it.
    #[doc(hidden)]
    pub fn find_entries_by_columns_sabotaged(
        &self,
        ts: &[[i64; 6]],
        s: &mut Searcher,
        p: TreePaths,
        paths: ColumnPaths,
        sabotage: ColumnSabotage,
    ) -> Vec<usize> {
        let mut out = vec![0; ts.len()];
        self.leaves_by_columns(ts, s, p, paths, Some(sabotage), |i, leaf| out[i] = leaf as usize);
        out
    }
}

impl DepthGroups {
    /// Stateless part of the column search: per column, the nearest leaf of each depth group
    /// (`n` entries from `c · n`), and whether the column search serves that column at all. With
    /// `paths.seeded`, a column's walk is bounded by the winners of the column before it in z (or
    /// in x at z = 0), a few blocks away and usually in the same boxes.
    fn column_bests(
        &self,
        f: &Flat,
        ts: &[[i64; 6]],
        paths: ColumnPaths,
        sabotage: Option<ColumnSabotage>,
    ) -> (Vec<GroupBest>, [bool; COLUMNS]) {
        let (heights, n) = (ts.len() / COLUMNS, self.len());
        let mut best = vec![GroupBest::NONE; COLUMNS * n];
        let mut by_column = [false; COLUMNS];
        for c in 0..COLUMNS {
            let Some(t) = constant_climate(ts, c, heights) else { continue };
            let neighbour = if c % 4 > 0 { Some(c - 1) } else { c.checked_sub(4) };
            let (done, rest) = best.split_at_mut(c * n);
            let seeds = neighbour.filter(|&b| paths.seeded && by_column[b]).map(|b| &done[b * n..(b + 1) * n]);
            self.nearest_per_group(f, &t, &mut rest[..n], seeds, paths, sabotage);
            by_column[c] = true;
        }
        (best, by_column)
    }
}
