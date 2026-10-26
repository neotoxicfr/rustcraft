//! Biomes (spec § 5): quantised climate, parameter list, search tree and tie
//! rule, End source, biome of a chunk per quart of block, block zoom.

use hashbrown::HashMap;

use crate::Error;
use crate::data::DataPack;
use crate::generator::Generator;
use crate::json::Json;
use crate::rng::biome_zoom_seed;
use crate::sampler::Ctx;
use crate::volume::Volume;

/// Quantised interval `[min ; max]`.
pub type Param = (i64, i64);
/// Temperature, humidity, continentalness, erosion, depth, weirdness, offset.
pub type Space = [Param; 7];

/// Quantisation of a climate parameter: f32 product then truncation.
#[inline]
pub fn quantize(v: f32) -> i64 {
    (v * 10_000.0) as i64
}

/// Exact inverse of `l → (f32)l / 10000` (values written by the official report).
fn unquantize_inverse(f: f32) -> Option<i64> {
    let guess = (f64::from(f) * 10_000.0).round() as i64;
    (guess - 2..=guess + 2).find(|l| (*l as f32) / 10_000.0 == f)
}

#[inline]
fn param_distance(p: Param, t: i64) -> i64 {
    let above = t - p.1;
    let below = p.0 - t;
    if above > 0 { above } else { below.max(0) }
}

fn space_distance(s: &Space, t: &[i64; 7]) -> i64 {
    let mut d = 0i64;
    for i in 0..7 {
        let x = param_distance(s[i], t[i]);
        d += x * x;
    }
    d
}

#[derive(Debug, Clone)]
enum Node {
    Leaf { space: Space, value: u16 },
    Sub { space: Space, children: Vec<u32> },
}

impl Node {
    fn space(&self) -> &Space {
        match self {
            Node::Leaf { space, .. } | Node::Sub { space, .. } => space,
        }
    }
}

/// Search tree (19 children per node).
#[derive(Debug, Clone)]
pub struct Tree {
    nodes: Vec<Node>,
    root: u32,
    /// All bounds within `±SMALL` (see [`SMALL`]).
    small: bool,
}

/// No-overflow regime: bounds and target within `±2²⁹`, hence differences `≤ 2³⁰`, squares `≤ 2⁶⁰` and
/// sum of the 7 `< 2⁶³`: exact, positive distances. Outside this regime, the distances
/// wrap like the game's `long` (release) and nothing can be deduced from their sign.
const SMALL: u64 = 1 << 29;

fn small_space(s: &Space) -> bool {
    s.iter().all(|p| p.0.unsigned_abs() <= SMALL && p.1.unsigned_abs() <= SMALL)
}

/// Search paths (switches `RUSTCRAFT_OPT_BTREE_*`), all with the same result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreePaths {
    /// Distance of the candidate carried with it; stop at a zero distance (no-overflow regime).
    pub early: bool,
    /// Flattened tree: contiguous children, i32 bounds as a structure of arrays.
    pub soa: bool,
    /// With `soa`: distances of a node's children in AVX2 (if the processor has it).
    pub simd: bool,
    /// Memo of the last climate: same target and same last leaf → same result.
    pub memo: bool,
}

impl TreePaths {
    /// Old path.
    pub const OLD: Self = TreePaths { early: false, soa: false, simd: false, memo: false };

    /// Paths chosen by the environment (read once).
    #[inline]
    pub fn current() -> Self {
        TreePaths {
            early: crate::opt::btree_early(),
            // The flattened tree in scalar loses on the micro-bench (+12%): only with AVX2.
            soa: crate::opt::btree_soa() && crate::opt::btree_simd(),
            simd: crate::opt::btree_simd(),
            memo: crate::opt::btree_memo(),
        }
    }
}

/// Flattened tree, numbered breadth-first: the children of a node are contiguous (`first .. first +
/// count`, same order as the original tree), bounds per dimension in i32 (≈ 56 B per node
/// instead of ≈ 144). Built only if all the bounds fit in 32 bits.
#[derive(Debug, Clone)]
struct Flat {
    /// Low and high bounds per dimension, plus 8 padding slots (reads by 8).
    lo: [Vec<i32>; 7],
    hi: [Vec<i32>; 7],
    first: Vec<u32>,
    /// Number of children; 0 = leaf.
    count: Vec<u8>,
    /// Original node of each flattened node, and the inverse.
    orig: Vec<u32>,
    of_old: Vec<u32>,
    /// AVX2 batches allowed: bounds within `±SMALL` and `lo ≤ hi` everywhere (then
    /// `max(t − hi, lo − t, 0)` is exactly the distance of a parameter).
    simd: bool,
}

impl Flat {
    fn new(tree: &Tree) -> Option<Self> {
        let n = tree.nodes.len();
        let mut order = Vec::with_capacity(n);
        let mut of_old = vec![u32::MAX; n];
        order.push(tree.root);
        of_old[tree.root as usize] = 0;
        let (mut first, mut count) = (vec![0u32; n], vec![0u8; n]);
        let mut i = 0;
        while i < order.len() {
            if let Node::Sub { children, .. } = &tree.nodes[order[i] as usize] {
                first[i] = order.len() as u32;
                count[i] = u8::try_from(children.len()).ok()?;
                for &c in children {
                    of_old[c as usize] = order.len() as u32;
                    order.push(c);
                }
            }
            i += 1;
        }
        // Nodes outside the tree (none in practice): kept at the end, without parent.
        for (o, slot) in of_old.iter_mut().enumerate() {
            if *slot == u32::MAX {
                *slot = order.len() as u32;
                order.push(o as u32);
            }
        }
        let mut lo: [Vec<i32>; 7] = Default::default();
        let mut hi: [Vec<i32>; 7] = Default::default();
        let mut simd = true;
        for &o in &order {
            let s = tree.nodes[o as usize].space();
            simd &= small_space(s) && s.iter().all(|p| p.0 <= p.1);
            for d in 0..7 {
                lo[d].push(i32::try_from(s[d].0).ok()?);
                hi[d].push(i32::try_from(s[d].1).ok()?);
            }
        }
        for d in 0..7 {
            lo[d].extend([0; 8]);
            hi[d].extend([0; 8]);
        }
        Some(Flat { lo, hi, first, count, orig: order, of_old, simd })
    }

    /// Same computation as [`space_distance`] (differences and squares in i64).
    #[inline]
    fn dist(&self, n: usize, t: &[i64; 7]) -> i64 {
        let mut d = 0i64;
        for (i, &ti) in t.iter().enumerate() {
            let x = param_distance((i64::from(self.lo[i][n]), i64::from(self.hi[i][n])), ti);
            d += x * x;
        }
        d
    }

    /// Search of the original tree on the internal node `node`, same tests in the same order.
    /// `EARLY`: `cand_d` is the distance of `cand` (otherwise recomputed like the old code) and, in
    /// no-overflow regime (`small`), a zero distance ends the search. `batch`: distances of the
    /// children in AVX2 (target `t32`). `SAB`: sabotage for the tests (`≥` instead of `>` at the
    /// two comparisons: a tie changes the winner).
    #[allow(clippy::too_many_arguments)]
    fn search<const EARLY: bool, const SAB: bool>(
        &self,
        node: usize,
        t: &[i64; 7],
        t32: &[i32; 7],
        cand: Option<u32>,
        cand_d: i64,
        small: bool,
        batch: bool,
    ) -> (Option<u32>, i64) {
        let a = self.first[node] as usize;
        let k = usize::from(self.count[node]);
        let mut dists = [0i64; 24];
        if batch {
            simd_dists(self, a, k, t32, &mut dists);
        }
        let mut min = if EARLY { cand_d } else { cand.map_or(i64::MAX, |c| self.dist(c as usize, t)) };
        let mut closest = cand;
        for (j, &dj) in dists.iter().enumerate().take(k) {
            if EARLY && small && min == 0 {
                break;
            }
            let c = a + j;
            let cd = if batch { dj } else { self.dist(c, t) };
            if if SAB { min >= cd } else { min > cd } {
                let (leaf, ld) = if self.count[c] == 0 {
                    (Some(c as u32), cd)
                } else {
                    let (leaf, ld) = self.search::<EARLY, SAB>(c, t, t32, closest, min, small, batch);
                    if EARLY { (leaf, ld) } else { (leaf, leaf.map_or(i64::MAX, |l| self.dist(l as usize, t))) }
                };
                if if SAB { min >= ld } else { min > ld } {
                    min = ld;
                    closest = leaf;
                }
            }
        }
        (closest, min)
    }
}

/// Distances of the children `a .. a + k` (k ≤ 19) to the target, in batches of 8 in AVX2 (no-overflow
/// regime and `lo ≤ hi` checked by the caller: exact differences in i32, exact squares by
/// 32 × 32 → 64-bit product). Scalar fallback if the processor has no AVX2.
#[cfg(target_arch = "x86_64")]
#[allow(unsafe_code)]
fn simd_dists(f: &Flat, a: usize, k: usize, t: &[i32; 7], out: &mut [i64; 24]) {
    #[target_feature(enable = "avx2")]
    fn body(f: &Flat, a: usize, k: usize, t: &[i32; 7], out: &mut [i64; 24]) {
        use std::arch::x86_64::*;
        let zero = _mm256_setzero_si256();
        let mut j = 0;
        while j < k {
            let (mut even, mut odd) = (zero, zero);
            for (d, &td) in t.iter().enumerate() {
                let tv = _mm256_set1_epi32(td);
                let (lo, hi) = (&f.lo[d][a + j..a + j + 8], &f.hi[d][a + j..a + j + 8]);
                // SAFETY: slices of 8 i32 (32 bytes) checked above, unaligned read.
                let (lo, hi) = unsafe { (_mm256_loadu_si256(lo.as_ptr().cast()), _mm256_loadu_si256(hi.as_ptr().cast())) };
                let x = _mm256_max_epi32(_mm256_max_epi32(_mm256_sub_epi32(tv, hi), _mm256_sub_epi32(lo, tv)), zero);
                even = _mm256_add_epi64(even, _mm256_mul_epi32(x, x));
                let xo = _mm256_srli_epi64(x, 32);
                odd = _mm256_add_epi64(odd, _mm256_mul_epi32(xo, xo));
            }
            let (mut e, mut o) = ([0i64; 4], [0i64; 4]);
            // SAFETY: write of 32 bytes into arrays of 4 i64.
            unsafe {
                _mm256_storeu_si256(e.as_mut_ptr().cast(), even);
                _mm256_storeu_si256(o.as_mut_ptr().cast(), odd);
            }
            for i in 0..4 {
                out[j + 2 * i] = e[i];
                out[j + 2 * i + 1] = o[i];
            }
            j += 8;
        }
    }
    if !crate::opt::avx2_available() {
        let t64 = t.map(i64::from);
        for (j, o) in out.iter_mut().enumerate().take(k) {
            *o = f.dist(a + j, &t64);
        }
        return;
    }
    // SAFETY: AVX2 has just been detected on this processor.
    unsafe { body(f, a, k, t, out) }
}

#[cfg(not(target_arch = "x86_64"))]
fn simd_dists(f: &Flat, a: usize, k: usize, t: &[i32; 7], out: &mut [i64; 24]) {
    let t64 = t.map(i64::from);
    for (j, o) in out.iter_mut().enumerate().take(k) {
        *o = f.dist(a + j, &t64);
    }
}

fn center(p: Param) -> i64 {
    (p.0 + p.1) / 2
}

fn span(children: &[u32], nodes: &[Node]) -> Space {
    let mut s = *nodes[children[0] as usize].space();
    for c in &children[1..] {
        let o = nodes[*c as usize].space();
        for d in 0..7 {
            s[d] = (s[d].0.min(o[d].0), s[d].1.max(o[d].1));
        }
    }
    s
}

fn sort_key(space: &Space, dim: usize, abs: bool) -> [i64; 7] {
    let mut k = [0i64; 7];
    for (i, kk) in k.iter_mut().enumerate() {
        let c = center(space[(dim + i) % 7]);
        *kk = if abs { c.abs() } else { c };
    }
    k
}

impl Tree {
    fn build_from(leaves: Vec<(Space, u16)>) -> Self {
        let mut nodes: Vec<Node> = leaves.into_iter().map(|(space, value)| Node::Leaf { space, value }).collect();
        let mut ids: Vec<u32> = (0..nodes.len() as u32).collect();
        let root = Self::build(&mut nodes, &mut ids);
        let small = nodes.iter().all(|n| small_space(n.space()));
        Tree { nodes, root, small }
    }

    fn push_sub(nodes: &mut Vec<Node>, children: Vec<u32>) -> u32 {
        let space = span(&children, nodes);
        nodes.push(Node::Sub { space, children });
        (nodes.len() - 1) as u32
    }

    fn build(nodes: &mut Vec<Node>, children: &mut [u32]) -> u32 {
        assert!(!children.is_empty(), "node without child");
        if children.len() == 1 {
            return children[0];
        }
        if children.len() <= 19 {
            children.sort_by_key(|c| nodes[*c as usize].space().iter().map(|p| center(*p).abs()).sum::<i64>());
            return Self::push_sub(nodes, children.to_vec());
        }
        type Buckets = Vec<(Space, Vec<u32>)>;
        let mut best: Option<(i64, usize, Buckets)> = None;
        let n = children.len();
        let k = 19i64.pow(((n as f64 - 0.01).ln() / 19f64.ln()).floor() as u32) as usize;
        for d in 0..7 {
            children.sort_by_key(|c| sort_key(nodes[*c as usize].space(), d, false));
            let buckets: Vec<(Space, Vec<u32>)> = children.chunks(k).map(|b| (span(b, nodes), b.to_vec())).collect();
            let cost: i64 = buckets.iter().map(|(s, _)| s.iter().map(|p| (p.1 - p.0).abs()).sum::<i64>()).sum();
            if best.as_ref().is_none_or(|b| b.0 > cost) {
                best = Some((cost, d, buckets));
            }
        }
        let (_, dim, mut buckets) = best.unwrap_or_default();
        buckets.sort_by_key(|(s, _)| sort_key(s, dim, true));
        let subs: Vec<u32> = buckets.into_iter().map(|(_, mut b)| Self::build(nodes, &mut b)).collect();
        Self::push_sub(nodes, subs)
    }

    fn search(&self, node: u32, t: &[i64; 7], candidate: Option<u32>) -> Option<u32> {
        match &self.nodes[node as usize] {
            Node::Leaf { .. } => Some(node),
            Node::Sub { children, .. } => {
                let mut min = candidate.map_or(i64::MAX, |c| space_distance(self.nodes[c as usize].space(), t));
                let mut closest = candidate;
                for &c in children {
                    let cd = space_distance(self.nodes[c as usize].space(), t);
                    if min > cd {
                        let leaf = self.search(c, t, closest);
                        let ld = match leaf {
                            Some(l) if l == c => cd,
                            Some(l) => space_distance(self.nodes[l as usize].space(), t),
                            None => i64::MAX,
                        };
                        if min > ld {
                            min = ld;
                            closest = leaf;
                        }
                    }
                }
                closest
            }
        }
    }

    /// [`Self::search`] on an internal node, candidate distance carried (`cand_d` = distance of
    /// `cand`, `i64::MAX` without candidate: invariant held by the old code, which recomputes it);
    /// in no-overflow regime (`small`), a zero distance is unbeatable and ends the search.
    fn search_carry(&self, node: u32, t: &[i64; 7], cand: Option<u32>, cand_d: i64, small: bool) -> (Option<u32>, i64) {
        let Node::Sub { children, .. } = &self.nodes[node as usize] else { unreachable!("leaf") };
        let (mut min, mut closest) = (cand_d, cand);
        for &c in children {
            if small && min == 0 {
                break;
            }
            let cn = &self.nodes[c as usize];
            let cd = space_distance(cn.space(), t);
            if min > cd {
                let (leaf, ld) = match cn {
                    Node::Leaf { .. } => (Some(c), cd),
                    Node::Sub { .. } => self.search_carry(c, t, closest, min, small),
                };
                if min > ld {
                    min = ld;
                    closest = leaf;
                }
            }
        }
        (closest, min)
    }
}

/// Last leaf found: the game keeps one per thread of execution, and it breaks
/// distance ties (§ 5.3). One `Searcher` per thread (or per task) to reproduce an order.
#[derive(Debug, Clone, Copy, Default)]
pub struct Searcher {
    last: Option<u32>,
    /// Last search (`RUSTCRAFT_OPT_BTREE_MEMO`): list, target, last leaf before
    /// the search, result. The search depends only on these three inputs.
    memo: Option<(usize, [i64; 6], Option<u32>, u32)>,
}

/// Stateless prepared target (`RUSTCRAFT_OPT_BTREE_SPLIT`): climate, and (F(t), dmin(t)) in the no-overflow
/// regime.
#[derive(Debug, Clone, Copy, Default)]
struct Pre {
    t: [i64; 6],
    /// Leaf, dmin(t), leaf surely equal to F(t).
    free: Option<(u32, i64, bool)>,
}

/// Targets per task of the stateless phase: serial within a task (neighbour's help).
pub const SPLIT_RUN: usize = 64;

/// Paths of a biome search over a sequence of points ([`BiomeSource::first_match`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SplitPaths {
    /// Two phases: stateless parallel batches, then last leaf in series.
    pub split: bool,
    /// Leaf of the neighbouring point for pruning.
    pub hint: bool,
    /// With `hint`: the neighbouring leaf serves as candidate (otherwise as bound + 1).
    pub cand: bool,
}

impl SplitPaths {
    /// Old path: one point after the other.
    pub const OLD: Self = SplitPaths { split: false, hint: false, cand: false };

    /// Paths chosen by the environment (read once).
    pub fn current() -> Self {
        SplitPaths {
            split: crate::opt::btree_split(),
            hint: crate::opt::btree_split_hint(),
            cand: crate::opt::btree_split_cand(),
        }
    }
}

/// List of climate parameters and its tree.
#[derive(Debug, Clone)]
pub struct ClimateList {
    pub names: Vec<String>,
    pub entries: Vec<(Space, u16)>,
    tree: Tree,
    /// Flattened tree (`None` if a bound exceeds 32 bits: old tree).
    flat: Option<Box<Flat>>,
}

impl ClimateList {
    pub fn new(entries: Vec<(Space, String)>) -> Self {
        let mut names: Vec<String> = Vec::new();
        let mut index: HashMap<String, u16> = HashMap::new();
        let entries: Vec<(Space, u16)> = entries
            .into_iter()
            .map(|(s, n)| {
                let i = *index.entry(n.clone()).or_insert_with(|| {
                    names.push(n);
                    (names.len() - 1) as u16
                });
                (s, i)
            })
            .collect();
        let tree = Tree::build_from(entries.clone());
        let flat = Flat::new(&tree).map(Box::new);
        ClimateList { names, entries, tree, flat }
    }

    /// Leaf (node of the original tree) kept for the target, through paths `p`; updates
    /// the `Searcher`'s last leaf like the game.
    fn find_leaf<const SAB: bool>(&self, target: [i64; 6], s: &mut Searcher, p: TreePaths) -> u32 {
        let id = self.tree.nodes.as_ptr() as usize;
        if p.memo
            && let Some((l, t, last, leaf)) = s.memo
            && l == id
            && t == target
            && last == s.last
        {
            s.last = Some(leaf);
            return leaf;
        }
        let t = [target[0], target[1], target[2], target[3], target[4], target[5], 0];
        let leaf = self.search::<SAB>(&t, s.last, p).unwrap_or(self.tree.root);
        if p.memo {
            s.memo = Some((id, target, s.last, leaf));
        }
        s.last = Some(leaf);
        leaf
    }

    fn search<const SAB: bool>(&self, t: &[i64; 7], last: Option<u32>, p: TreePaths) -> Option<u32> {
        let tree = &self.tree;
        if let Node::Leaf { .. } = tree.nodes[tree.root as usize] {
            return Some(tree.root);
        }
        let small = tree.small && t.iter().all(|v| v.unsigned_abs() <= SMALL);
        match &self.flat {
            Some(f) if p.soa || SAB => {
                let cand = last.map(|l| f.of_old[l as usize]);
                let cand_d = cand.map_or(i64::MAX, |c| f.dist(c as usize, t));
                // Outside the no-overflow regime (extreme target), scalar i64 distances.
                let batch = p.simd && small && f.simd && crate::opt::avx2_available();
                let t32 = if batch { t.map(|v| v as i32) } else { [0; 7] };
                let leaf = if p.early {
                    f.search::<true, SAB>(0, t, &t32, cand, cand_d, small, batch).0
                } else {
                    f.search::<false, SAB>(0, t, &t32, cand, cand_d, small, batch).0
                };
                leaf.map(|l| f.orig[l as usize])
            }
            _ if p.early => {
                let cand_d = last.map_or(i64::MAX, |c| space_distance(tree.nodes[c as usize].space(), t));
                tree.search_carry(tree.root, t, last, cand_d, small).0
            }
            _ => tree.search(tree.root, t, last),
        }
    }

    /// Like [`Self::find_entry`], through paths `p` (equivalence tests and benches).
    pub fn find_entry_with(&self, target: [i64; 6], s: &mut Searcher, p: TreePaths) -> usize {
        self.find_leaf::<false>(target, s, p) as usize
    }

    /// Sabotaged flattened tree (`≥` instead of `>` between distances): the equivalence tests
    /// must reject it.
    #[doc(hidden)]
    pub fn find_entry_sabotaged(&self, target: [i64; 6], s: &mut Searcher, p: TreePaths) -> usize {
        self.find_leaf::<true>(target, s, p) as usize
    }

    /// Stateless phase: (leaf, dmin(t), sure). In no-overflow regime the pruning is
    /// exact: the chained search returns the last leaf if it is at dmin, otherwise F(t)
    /// (first leaf at dmin in walk order, whatever the starting candidate further
    /// than dmin). `sure`: the returned leaf is F(t). Outside the regime: `None` (the serial
    /// phase redoes the chained search). `hint`: leaf of a neighbouring point S. `cand`: S serves as
    /// candidate (like the last leaf); the result is F(t) only if it strictly beats S.
    /// Otherwise S serves as bound d(S) + 1 without candidate (pruning kept, result always F(t)).
    fn free_leaf<const SAB: bool>(&self, t6: [i64; 6], hint: Option<u32>, cand: bool, p: TreePaths) -> Option<(u32, i64, bool)> {
        let t = [t6[0], t6[1], t6[2], t6[3], t6[4], t6[5], 0];
        let tree = &self.tree;
        if !(tree.small && t.iter().all(|v| v.unsigned_abs() <= SMALL)) {
            return None;
        }
        if let Node::Leaf { space, .. } = &tree.nodes[tree.root as usize] {
            return Some((tree.root, space_distance(space, &t), true));
        }
        let dh = hint.map(|h| space_distance(tree.nodes[h as usize].space(), &t));
        // No-overflow regime: distance < 2⁶³ − 1, the bound does not overflow.
        let (c, bound) = match (hint, dh) {
            (Some(h), Some(d)) if cand => (Some(h), d),
            (_, Some(d)) => (None, d + 1),
            _ => (None, i64::MAX),
        };
        let (leaf, d) = match &self.flat {
            Some(f) if p.soa || SAB => {
                let batch = p.simd && f.simd && crate::opt::avx2_available();
                let t32 = if batch { t.map(|v| v as i32) } else { [0; 7] };
                let fc = c.map(|h| f.of_old[h as usize]);
                let (leaf, d) = f.search::<true, SAB>(0, &t, &t32, fc, bound, true, batch);
                (leaf.map(|l| f.orig[l as usize]), d)
            }
            _ => tree.search_carry(tree.root, &t, c, bound, true),
        };
        leaf.map(|l| (l, d, c.is_none() || d < bound))
    }

    /// Serial phase: last leaf if it is at dmin, otherwise F(t) (or chained search if
    /// F(t) is not known); updates `s` like the chained search. `IGNORE_LAST`:
    /// test sabotage (leaf of the stateless phase always).
    fn resolve<const IGNORE_LAST: bool>(&self, pre: &Pre, s: &mut Searcher, p: TreePaths) -> u32 {
        let Some((free, dmin, sure)) = pre.free else {
            return self.find_leaf::<false>(pre.t, s, p);
        };
        let t = [pre.t[0], pre.t[1], pre.t[2], pre.t[3], pre.t[4], pre.t[5], 0];
        let leaf = match s.last {
            _ if IGNORE_LAST => free,
            Some(l) if space_distance(self.tree.nodes[l as usize].space(), &t) == dmin => l,
            _ if sure => free,
            _ => return self.find_leaf::<false>(pre.t, s, p),
        };
        s.last = Some(leaf);
        leaf
    }

    fn leaf_value(&self, leaf: u32) -> u16 {
        match &self.tree.nodes[leaf as usize] {
            Node::Leaf { value, .. } => *value,
            Node::Sub { .. } => unreachable!("search ended on an internal node"),
        }
    }

    /// Stateless phase of a task, in series, each target helped by the leaf of the previous one
    /// (`paths.hint`).
    fn prepare<const SAB: bool>(&self, ts: impl Iterator<Item = [i64; 6]>, paths: SplitPaths, p: TreePaths, out: &mut [Pre]) {
        let mut prev = None;
        for (t, o) in ts.zip(out) {
            let free = self.free_leaf::<SAB>(t, if paths.hint { prev } else { None }, paths.cand, p);
            if let Some((l, ..)) = free {
                prev = Some(l);
            }
            *o = Pre { t, free };
        }
    }

    /// Entries kept for a sequence of targets, by the two phases (parallel tasks of
    /// [`SPLIT_RUN`] targets, then series): same sequence as [`Self::find_entry`] called in
    /// order with `s` (equivalence tests and benches).
    pub fn find_entries_split(&self, ts: &[[i64; 6]], s: &mut Searcher, paths: SplitPaths, p: TreePaths) -> Vec<usize> {
        self.split_impl::<false, false>(ts, s, paths, p)
    }

    /// Sabotages of the two phases (`phase` 1: `≥` in the stateless phase; otherwise last
    /// leaf ignored in the serial phase): the equivalence tests must reject them.
    #[doc(hidden)]
    pub fn find_entries_split_sabotaged(&self, ts: &[[i64; 6]], s: &mut Searcher, paths: SplitPaths, p: TreePaths, phase: u8) -> Vec<usize> {
        if phase == 1 { self.split_impl::<true, false>(ts, s, paths, p) } else { self.split_impl::<false, true>(ts, s, paths, p) }
    }

    fn split_impl<const SAB: bool, const IGNORE_LAST: bool>(
        &self,
        ts: &[[i64; 6]],
        s: &mut Searcher,
        paths: SplitPaths,
        p: TreePaths,
    ) -> Vec<usize> {
        use rayon::prelude::*;
        let mut pres = vec![Pre::default(); ts.len()];
        pres.par_chunks_mut(SPLIT_RUN)
            .zip(ts.par_chunks(SPLIT_RUN))
            .for_each(|(o, c)| self.prepare::<SAB>(c.iter().copied(), paths, p, o));
        pres.iter().map(|pre| self.resolve::<IGNORE_LAST>(pre, s, p) as usize).collect()
    }

    /// List of the official report `biome_parameters/<preset>` (values written de-quantised).
    pub fn from_report(pack: &DataPack, preset: &str) -> Result<Self, Error> {
        Self::from_json(&pack.json("biome_parameters", preset)?, true)
    }

    /// `report`: numbers written by the report (exact inverse of the de-quantisation);
    /// otherwise numbers of ordinary data (quantised as the game reads them).
    pub fn from_json(j: &Json, report: bool) -> Result<Self, Error> {
        let bad = |w: &str| Error::Format(format!("biome list: {w}"));
        let q = |v: &Json| -> Result<i64, Error> {
            let f = v.as_f32().ok_or_else(|| bad("number expected"))?;
            if report {
                unquantize_inverse(f).ok_or_else(|| bad("value outside quantisation"))
            } else {
                Ok(quantize(f))
            }
        };
        let param = |v: Option<&Json>| -> Result<Param, Error> {
            let v = v.ok_or_else(|| bad("missing parameter"))?;
            match v.as_array() {
                Some([a, b]) => Ok((q(a)?, q(b)?)),
                Some(_) => Err(bad("interval")),
                None => {
                    let x = q(v)?;
                    Ok((x, x))
                }
            }
        };
        let list = j.get("biomes").and_then(Json::as_array).ok_or_else(|| bad("biomes"))?;
        let mut entries = Vec::with_capacity(list.len());
        for e in list {
            let p = e.get("parameters").ok_or_else(|| bad("parameters"))?;
            let name = e.get("biome").and_then(Json::as_str).ok_or_else(|| bad("biome"))?;
            let off = q(p.get("offset").ok_or_else(|| bad("offset"))?)?;
            let space = [
                param(p.get("temperature"))?,
                param(p.get("humidity"))?,
                param(p.get("continentalness"))?,
                param(p.get("erosion"))?,
                param(p.get("depth"))?,
                param(p.get("weirdness"))?,
                (off, off),
            ];
            entries.push((space, crate::data::full_id(name)));
        }
        if entries.is_empty() {
            return Err(bad("empty list"));
        }
        Ok(Self::new(entries))
    }

    /// Biome (index into `names`) nearest to the quantised target (6 values, offset 0).
    pub fn find(&self, target: [i64; 6], s: &mut Searcher) -> u16 {
        let leaf = self.find_leaf::<false>(target, s, TreePaths::current());
        match &self.tree.nodes[leaf as usize] {
            Node::Leaf { value, .. } => *value,
            Node::Sub { .. } => unreachable!("search ended on an internal node"),
        }
    }

    /// Like `find`, but returns the index of the kept entry.
    pub fn find_entry(&self, target: [i64; 6], s: &mut Searcher) -> usize {
        // The leaves are the first nodes, in list order.
        self.find_leaf::<false>(target, s, TreePaths::current()) as usize
    }

    /// Distance (sum of squares) of an entry to a target.
    pub fn distance(&self, entry: usize, target: [i64; 6]) -> i64 {
        let t = [target[0], target[1], target[2], target[3], target[4], target[5], 0];
        space_distance(&self.entries[entry].0, &t)
    }

    /// Exhaustive search (first strict minimum, in list order).
    pub fn find_brute_force(&self, target: [i64; 6]) -> u16 {
        let t = [target[0], target[1], target[2], target[3], target[4], target[5], 0];
        let mut best = (i64::MAX, 0u16);
        for (s, v) in &self.entries {
            let d = space_distance(s, &t);
            if d < best.0 {
                best = (d, *v);
            }
        }
        best.1
    }

    /// Distinct biomes at minimal distance from the target (more than one: the history breaks the tie).
    pub fn tied_biomes(&self, target: [i64; 6]) -> usize {
        let t = [target[0], target[1], target[2], target[3], target[4], target[5], 0];
        let mut best = i64::MAX;
        let mut set: Vec<u16> = Vec::new();
        for (s, v) in &self.entries {
            let d = space_distance(s, &t);
            if d < best {
                best = d;
                set.clear();
            }
            if d == best && !set.contains(v) {
                set.push(*v);
            }
        }
        set.len()
    }
}

const END_BIOMES: [&str; 5] = [
    "minecraft:the_end",
    "minecraft:end_highlands",
    "minecraft:end_midlands",
    "minecraft:small_end_islands",
    "minecraft:end_barrens",
];

/// Biome source of a dimension.
#[derive(Debug, Clone)]
pub enum BiomeSource {
    MultiNoise(ClimateList),
    TheEnd,
    Fixed(String),
    /// Checkerboard of columns: biome `list[(qx >> shift) + (qz >> shift) mod n]`; `names` without
    /// duplicates (order of first appearance), `list` in indices of `names`.
    Checkerboard {
        names: Vec<String>,
        list: Vec<u16>,
        shift: i32,
    },
}

impl BiomeSource {
    /// Checkerboard source: biomes in list order (duplicates kept), cell side
    /// `2^scale` quarts (`scale` 0 to 62, default 2).
    pub fn checkerboard(biomes: &[String], scale: i32) -> Result<Self, Error> {
        if biomes.is_empty() {
            return Err(Error::Format("checkerboard without biome".into()));
        }
        let mut names: Vec<String> = Vec::new();
        let list = biomes
            .iter()
            .map(|b| {
                let b = crate::data::full_id(b);
                let i = names.iter().position(|n| *n == b).unwrap_or_else(|| {
                    names.push(b);
                    names.len() - 1
                });
                i as u16
            })
            .collect();
        if !(0..=62).contains(&scale) {
            return Err(Error::Format(format!("checkerboard: scale {scale} outside 0 to 62")));
        }
        Ok(BiomeSource::Checkerboard { names, list, shift: scale + 2 })
    }

    fn checker(list: &[u16], shift: i32, qx: i32, qz: i32) -> u16 {
        // Integer shift of the game: only its 5 low bits count (scale 30 and more: wrapped).
        let s = shift & 31;
        list[((qx >> s).wrapping_add(qz >> s)).rem_euclid(list.len() as i32) as usize]
    }
}

/// Biomes of a list (`a`, `[a, b]`, `#tag`) in order, tags expanded in place.
pub fn biome_list(d: &crate::decor_data::DecorData, items: &[String]) -> Vec<String> {
    fn walk(d: &crate::decor_data::DecorData, s: &str, out: &mut Vec<String>, depth: u32) {
        match s.strip_prefix('#') {
            Some(tag) if depth < 16 => {
                let Ok(t) = d.json("tags_worldgen_biome", tag) else { return };
                for v in t.get("values").and_then(Json::as_array).unwrap_or(&[]) {
                    let id = v.as_str().or_else(|| v.get("id").and_then(Json::as_str));
                    if let Some(id) = id {
                        walk(d, id, out, depth + 1);
                    }
                }
            }
            Some(_) => {}
            None => out.push(crate::data::full_id(s)),
        }
    }
    let mut out = Vec::new();
    for s in items {
        walk(d, s, &mut out, 0);
    }
    // A tag is an ordered set (first occurrence kept); a direct list
    // keeps its duplicates.
    if items.iter().any(|s| s.starts_with('#')) {
        let mut seen = hashbrown::HashSet::new();
        out.retain(|b| seen.insert(b.clone()));
    }
    out
}

/// Biomes of a chunk, per quart: index `(qy · 4 + qz) · 4 + qx` (qy from the bottom).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkBiomes {
    pub min_qy: i32,
    pub height_q: i32,
    pub data: Vec<u16>,
}

impl ChunkBiomes {
    pub fn get(&self, qx: i32, qy: i32, qz: i32) -> u16 {
        self.data[((qy * 4 + qz) * 4 + qx) as usize]
    }
}

impl BiomeSource {
    /// Source described by a `world_preset` for a dimension (`minecraft:overworld`…), with
    /// the identifier of its noise settings.
    pub fn from_preset(pack: &DataPack, preset: &str, dimension: &str) -> Result<(Self, String), Error> {
        let j = pack.json("world_preset", preset)?;
        let bad = |w: &str| Error::Format(format!("world_preset {preset} : {w}"));
        let generator = j
            .get("dimensions")
            .and_then(|d| crate::data::with_full_id(dimension, |f| d.get(f)))
            .and_then(|d| d.get("generator"))
            .ok_or_else(|| bad("dimension"))?;
        let settings = generator.get("settings").and_then(Json::as_str).ok_or_else(|| bad("settings"))?;
        let src = generator.get("biome_source").ok_or_else(|| bad("biome_source"))?;
        Ok((Self::from_json(pack, src)?, crate::data::full_id(settings)))
    }

    pub fn from_json(pack: &DataPack, src: &Json) -> Result<Self, Error> {
        let ty = src.get("type").and_then(Json::as_str).unwrap_or_default();
        match ty.strip_prefix("minecraft:").unwrap_or(ty) {
            "multi_noise" => match src.get("preset").and_then(Json::as_str) {
                Some(p) => Ok(BiomeSource::MultiNoise(ClimateList::from_report(pack, p)?)),
                None => Ok(BiomeSource::MultiNoise(ClimateList::from_json(src, false)?)),
            },
            "the_end" => Ok(BiomeSource::TheEnd),
            "fixed" => {
                let b = src.get("biome").and_then(Json::as_str).ok_or_else(|| Error::Format("fixed biome".into()))?;
                Ok(BiomeSource::Fixed(crate::data::full_id(b)))
            }
            "checkerboard" => {
                let biomes: Vec<String> = match src.get("biomes") {
                    Some(Json::Str(s)) => vec![s.clone()],
                    Some(Json::Arr(a)) => a.iter().filter_map(Json::as_str).map(str::to_owned).collect(),
                    _ => return Err(Error::Format("checkerboard: biomes".into())),
                };
                if biomes.iter().any(|b| b.starts_with('#')) {
                    return Err(Error::Format("checkerboard: biome tag outside decoration data".into()));
                }
                Self::checkerboard(&biomes, src.get("scale").and_then(Json::as_i32).unwrap_or(2))
            }
            other => Err(Error::Format(format!("unhandled biome source: {other}"))),
        }
    }

    /// Names of the biomes, indexed by the returned values.
    pub fn names(&self) -> Vec<String> {
        match self {
            BiomeSource::MultiNoise(l) => l.names.clone(),
            BiomeSource::TheEnd => END_BIOMES.iter().map(|s| (*s).to_owned()).collect(),
            BiomeSource::Fixed(b) => vec![b.clone()],
            BiomeSource::Checkerboard { names, .. } => names.clone(),
        }
    }

    fn end_biome(g: &Generator, ctx: &mut Ctx, qx: i32, qy: i32, qz: i32) -> u16 {
        let (bx, by, bz) = (qx << 2, qy << 2, qz << 2);
        let (cx, cz) = (bx >> 4, bz >> 4);
        if i64::from(cx) * i64::from(cx) + i64::from(cz) * i64::from(cz) <= 4096 {
            return 0;
        }
        let h = g.value(g.router.erosion, ctx, (cx * 2 + 1) * 8, by, (cz * 2 + 1) * 8);
        if h > 0.25 {
            1
        } else if h >= -0.0625 {
            2
        } else if h < -0.21875 {
            3
        } else {
            4
        }
    }

    /// Quantised climate of a quart (point path).
    pub fn climate_at(g: &Generator, ctx: &mut Ctx, qx: i32, qy: i32, qz: i32) -> [i64; 6] {
        let (x, y, z) = (qx << 2, qy << 2, qz << 2);
        let r = g.router;
        [r.temperature, r.vegetation, r.continents, r.erosion, r.depth, r.ridges]
            .map(|s| quantize(g.value(s, ctx, x, y, z)))
    }

    /// Biome of a quart outside chunk filling (commands, structures): point path.
    pub fn biome_at(&self, g: &Generator, ctx: &mut Ctx, s: &mut Searcher, qx: i32, qy: i32, qz: i32) -> u16 {
        match self {
            BiomeSource::MultiNoise(l) => l.find(Self::climate_at(g, ctx, qx, qy, qz), s),
            BiomeSource::TheEnd => Self::end_biome(g, ctx, qx, qy, qz),
            BiomeSource::Fixed(_) => 0,
            BiomeSource::Checkerboard { list, shift, .. } => Self::checker(list, *shift, qx, qz),
        }
    }

    /// First of the `n` points (quarts `at(i)`, in order) whose biome is accepted by `ok`:
    /// index and biome. `s` carries the last leaf like a point-by-point walk with
    /// [`Self::biome_at`]. `paths.split`: climate and stateless leaf computed in increasing
    /// parallel batches, then last leaf applied in series up to the first accepted point (same
    /// sequence of biomes, bit for bit).
    pub fn first_match(
        &self,
        g: &Generator,
        n: usize,
        at: impl Fn(usize) -> [i32; 3] + Sync,
        ok: &[bool],
        s: &mut Searcher,
        paths: SplitPaths,
    ) -> Option<(usize, u16)> {
        use rayon::prelude::*;
        let admitted = |b: u16| ok.get(usize::from(b)).copied().unwrap_or(false);
        if !paths.split {
            let mut ctx = Ctx::cached();
            for i in 0..n {
                let [x, y, z] = at(i);
                let b = self.biome_at(g, &mut ctx, s, x, y, z);
                if admitted(b) {
                    return Some((i, b));
                }
            }
            return None;
        }
        let p = TreePaths::current();
        // Increasing batches: a common biome found right away only pays for a small batch.
        let unit = SPLIT_RUN * rayon::current_num_threads().max(1);
        let (mut start, mut block) = (0usize, unit);
        let mut pres: Vec<Pre> = Vec::new();
        while start < n {
            let end = (start + block).min(n);
            let runs = (end - start).div_ceil(SPLIT_RUN);
            let run = |r: usize| start + r * SPLIT_RUN..(start + (r + 1) * SPLIT_RUN).min(end);
            match self {
                BiomeSource::MultiNoise(l) => {
                    pres.resize(end - start, Pre::default());
                    pres.par_chunks_mut(SPLIT_RUN).enumerate().for_each_init(Ctx::cached, |ctx, (r, o)| {
                        let ts = run(r).map(|i| {
                            let [x, y, z] = at(i);
                            Self::climate_at(g, ctx, x, y, z)
                        });
                        l.prepare::<false>(ts, paths, p, o);
                    });
                    for (k, pre) in pres.iter().enumerate() {
                        let b = l.leaf_value(l.resolve::<false>(pre, s, p));
                        if admitted(b) {
                            return Some((start + k, b));
                        }
                    }
                }
                _ => {
                    // Sources without last leaf: direct biome, stateless.
                    let bs: Vec<Vec<u16>> = (0..runs)
                        .into_par_iter()
                        .map_init(
                            || (Ctx::cached(), Searcher::default()),
                            |(ctx, sr), r| {
                                run(r)
                                    .map(|i| {
                                        let [x, y, z] = at(i);
                                        self.biome_at(g, ctx, sr, x, y, z)
                                    })
                                    .collect()
                            },
                        )
                        .collect();
                    if let Some((k, &b)) = bs.iter().flatten().enumerate().find(|(_, b)| admitted(**b)) {
                        return Some((start + k, b));
                    }
                }
            }
            start = end;
            block = (block * 2).min(unit * 32);
        }
        None
    }

    /// Biomes of a chunk like the game's filling: climate in a 4 × (h/4) × 4 volume of step 4,
    /// then sections from bottom to top, x, y, z.
    pub fn chunk(&self, g: &Generator, cx: i32, cz: i32, s: &mut Searcher) -> ChunkBiomes {
        self.chunk_range(g, cx, cz, g.settings.min_y, g.settings.height, s)
    }

    /// Like [`Self::chunk`], over the world height `min_y`, `height` (that of the proto-chunk,
    /// which can exceed the noise's: the Nether and End are 256 blocks for 128 generated).
    pub fn chunk_range(
        &self,
        g: &Generator,
        cx: i32,
        cz: i32,
        min_y: i32,
        height: i32,
        s: &mut Searcher,
    ) -> ChunkBiomes {
        let min_qy = min_y >> 2;
        let height_q = height >> 2;
        let (qx0, qz0) = (cx * 4, cz * 4);
        let mut data = vec![0u16; (height_q * 16) as usize];
        let mut ctx = Ctx::cached();
        let mut store = |qx: i32, qy: i32, qz: i32, v: u16| data[((qy * 4 + qz) * 4 + qx) as usize] = v;
        match self {
            BiomeSource::MultiNoise(list) => {
                let v = Volume::stepped(4, height_q, 4, qx0 << 2, min_qy << 2, qz0 << 2, 4, 4, 4);
                let r = g.router;
                let bufs: Vec<Vec<f32>> = [r.temperature, r.vegetation, r.continents, r.erosion, r.depth, r.ridges]
                    .iter()
                    .map(|sid| g.volume(*sid, &mut ctx, &v))
                    .collect();
                for sec in 0..height_q / 4 {
                    for x in 0..4 {
                        for y in 0..4 {
                            for z in 0..4 {
                                let qy = sec * 4 + y;
                                let i = v.index(x, qy, z);
                                let t = [0, 1, 2, 3, 4, 5].map(|k| quantize(bufs[k][i]));
                                store(x, qy, z, list.find(t, s));
                            }
                        }
                    }
                }
            }
            BiomeSource::TheEnd => {
                for sec in 0..height_q / 4 {
                    for x in 0..4 {
                        for y in 0..4 {
                            for z in 0..4 {
                                let qy = sec * 4 + y;
                                store(x, qy, z, Self::end_biome(g, &mut ctx, qx0 + x, min_qy + qy, qz0 + z));
                            }
                        }
                    }
                }
            }
            BiomeSource::Fixed(_) => {}
            BiomeSource::Checkerboard { list, shift, .. } => {
                for qy in 0..height_q {
                    for z in 0..4 {
                        for x in 0..4 {
                            store(x, qy, z, Self::checker(list, *shift, qx0 + x, qz0 + z));
                        }
                    }
                }
            }
        }
        ChunkBiomes { min_qy, height_q, data }
    }
}

/// Block zoom (§ 5.5): quart whose biome applies to the block `(x, y, z)`, for the hashed
/// seed `zoom_seed` (the one the client receives, or `biome_zoom_seed(seed)`).
pub fn zoom(zoom_seed: i64, x: i32, y: i32, z: i32) -> (i32, i32, i32) {
    // Subtraction wrapped like the game's integer (identical to release; debug used to panic).
    let (ax, ay, az) = (x.wrapping_sub(2), y.wrapping_sub(2), z.wrapping_sub(2));
    let (px, py, pz) = (ax >> 2, ay >> 2, az >> 2);
    let fx = f64::from(ax & 3) / 4.0;
    let fy = f64::from(ay & 3) / 4.0;
    let fz = f64::from(az & 3) / 4.0;
    let mut best = (0usize, f64::INFINITY);
    for i in 0..8 {
        let (xe, ye, ze) = (i & 4 == 0, i & 2 == 0, i & 1 == 0);
        let cx = if xe { px } else { px + 1 };
        let cy = if ye { py } else { py + 1 };
        let cz = if ze { pz } else { pz + 1 };
        let dx = if xe { fx } else { fx - 1.0 };
        let dy = if ye { fy } else { fy - 1.0 };
        let dz = if ze { fz } else { fz - 1.0 };
        let d = fiddled_distance(zoom_seed, cx, cy, cz, dx, dy, dz);
        if best.1 > d {
            best = (i, d);
        }
    }
    let i = best.0;
    (if i & 4 == 0 { px } else { px + 1 }, if i & 2 == 0 { py } else { py + 1 }, if i & 1 == 0 { pz } else { pz + 1 })
}

/// Shortcut: zoom with the world seed.
pub fn zoom_world(seed: i64, x: i32, y: i32, z: i32) -> (i32, i32, i32) {
    zoom(biome_zoom_seed(seed), x, y, z)
}

#[inline]
fn lcg_next(r: i64, c: i64) -> i64 {
    r.wrapping_mul(r.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407)).wrapping_add(c)
}

fn fiddle(r: i64) -> f64 {
    let u = (r >> 24).rem_euclid(1024) as f64 / 1024.0;
    (u - 0.5) * 0.9
}

/// Pseudo-random offsets `(x, y, z)` of the corner `(x, y, z)` of the quart grid.
#[inline]
fn corner_offsets(seed: i64, x: i32, y: i32, z: i32) -> [f64; 3] {
    let mut r = seed;
    for c in [x, y, z, x, y, z] {
        r = lcg_next(r, i64::from(c));
    }
    let fx = fiddle(r);
    r = lcg_next(r, seed);
    let fy = fiddle(r);
    r = lcg_next(r, seed);
    let fz = fiddle(r);
    [fx, fy, fz]
}

fn fiddled_distance(seed: i64, x: i32, y: i32, z: i32, dx: f64, dy: f64, dz: f64) -> f64 {
    let [fx, fy, fz] = corner_offsets(seed, x, y, z);
    let (a, b, c) = (dz + fz, dy + fy, dx + fx);
    a * a + b * b + c * c
}

const MEMO_SLOTS: usize = 256;

/// Zoom with memoised corner offsets (`RUSTCRAFT_OPT_ZOOM_MEMO`): direct cache of 256 corners,
/// slot chosen by `(x & 3, z & 3, y & 15)` (a column walked in y reuses its 4
/// columns of corners; scattered neighbouring positions share their corners). Same computations
/// and same comparison order as [`zoom`], hence the same result bit for bit.
#[derive(Debug, Clone)]
pub struct ZoomMemo {
    seed: i64,
    keys: [[i32; 3]; MEMO_SLOTS],
    vals: [[f64; 3]; MEMO_SLOTS],
}

impl ZoomMemo {
    pub fn new(zoom_seed: i64) -> Box<Self> {
        // All the slots hold the corner (0, 0, 0): correct content, no validity flag.
        let v = corner_offsets(zoom_seed, 0, 0, 0);
        Box::new(ZoomMemo { seed: zoom_seed, keys: [[0; 3]; MEMO_SLOTS], vals: [v; MEMO_SLOTS] })
    }

    pub fn seed(&self) -> i64 {
        self.seed
    }

    /// Same result as `zoom(self.seed(), x, y, z)`.
    pub fn zoom(&mut self, x: i32, y: i32, z: i32) -> (i32, i32, i32) {
        self.zoom_impl::<false>(x, y, z)
    }

    /// Sabotaged memo (key without z): the equivalence tests must reject it.
    #[doc(hidden)]
    pub fn zoom_sabotaged(&mut self, x: i32, y: i32, z: i32) -> (i32, i32, i32) {
        self.zoom_impl::<true>(x, y, z)
    }

    #[inline]
    fn offsets<const SAB: bool>(&mut self, x: i32, y: i32, z: i32) -> [f64; 3] {
        let i = (((x & 3) << 6) | ((z & 3) << 4) | (y & 15)) as usize;
        let k = self.keys[i];
        if k == [x, y, z] || (SAB && k[0] == x && k[1] == y) {
            return self.vals[i];
        }
        let v = corner_offsets(self.seed, x, y, z);
        self.keys[i] = [x, y, z];
        self.vals[i] = v;
        v
    }

    #[inline]
    fn zoom_impl<const SAB: bool>(&mut self, x: i32, y: i32, z: i32) -> (i32, i32, i32) {
        let (ax, ay, az) = (x.wrapping_sub(2), y.wrapping_sub(2), z.wrapping_sub(2));
        let (px, py, pz) = (ax >> 2, ay >> 2, az >> 2);
        let fx = f64::from(ax & 3) / 4.0;
        let fy = f64::from(ay & 3) / 4.0;
        let fz = f64::from(az & 3) / 4.0;
        let mut best = (0usize, f64::INFINITY);
        for i in 0..8 {
            let (xe, ye, ze) = (i & 4 == 0, i & 2 == 0, i & 1 == 0);
            let cx = if xe { px } else { px + 1 };
            let cy = if ye { py } else { py + 1 };
            let cz = if ze { pz } else { pz + 1 };
            let dx = if xe { fx } else { fx - 1.0 };
            let dy = if ye { fy } else { fy - 1.0 };
            let dz = if ze { fz } else { fz - 1.0 };
            let [ox, oy, oz] = self.offsets::<SAB>(cx, cy, cz);
            let (a, b, c) = (dz + oz, dy + oy, dx + ox);
            let d = a * a + b * b + c * c;
            if best.1 > d {
                best = (i, d);
            }
        }
        let i = best.0;
        (if i & 4 == 0 { px } else { px + 1 }, if i & 2 == 0 { py } else { py + 1 }, if i & 1 == 0 { pz } else { pz + 1 })
    }
}

/// [`zoom`] through the chosen path: with `RUSTCRAFT_OPT_ZOOM_MEMO` (default), memo of the current thread
/// (recreated when the seed changes). For column walks (materials): on scattered
/// positions (decoration) the memo loses on the micro-bench, they keep [`zoom`].
pub fn zoom_cached(zoom_seed: i64, x: i32, y: i32, z: i32) -> (i32, i32, i32) {
    use std::cell::RefCell;
    thread_local! {
        static MEMO: RefCell<Option<Box<ZoomMemo>>> = const { RefCell::new(None) };
    }
    if !crate::opt::zoom_memo() {
        return zoom(zoom_seed, x, y, z);
    }
    MEMO.with_borrow_mut(|m| match m {
        Some(m) if m.seed == zoom_seed => m.zoom(x, y, z),
        _ => m.insert(ZoomMemo::new(zoom_seed)).zoom(x, y, z),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Checkerboard: cells of 2^(scale + 2) quarts, Euclidean remainder, duplicates kept in the list,
    /// offset folded to 5 bits like the game's integer.
    #[test]
    fn checkerboard_cells() {
        let names = ["plains", "desert", "plains", "snowy_taiga"].map(String::from);
        let BiomeSource::Checkerboard { names: n, list, shift } = BiomeSource::checkerboard(&names, 1).unwrap() else {
            panic!()
        };
        assert_eq!(n, ["minecraft:plains", "minecraft:desert", "minecraft:snowy_taiga"]);
        assert_eq!(list, [0, 1, 0, 2]);
        assert_eq!(shift, 3);
        let at = |qx, qz| BiomeSource::checker(&list, shift, qx, qz);
        assert_eq!((at(0, 0), at(7, 7), at(8, 0), at(8, 8), at(16, 8)), (0, 0, 1, 0, 2));
        assert_eq!(at(-1, 0), 2);
        assert_eq!(BiomeSource::checker(&list, 32, 5, 6), list[11 % 4]);
        assert!(BiomeSource::checkerboard(&names, 63).is_err());
    }
}
