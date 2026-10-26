//! Removal of the hidden faces of the mesh: game rule by face shapes, per-state properties
//! useful to the per-block loop (stored in an 18³ grid parallel to that of the states), and
//! loop paths (`RUSTCRAFT_OPT_MESH*` and `RUSTCRAFT_OPT_SHAPECULL` switches).
//!
//! Game rule for a face turned towards d, neighbour n (specification in our own words):
//! 1. the neighbour's face shape on the opposite side is the game's full cube → hidden;
//! 2. "same block" rule (glasses, ices…, opaque leaves) → hidden;
//! 3. neighbour's face shape empty → visible;
//! 4. own face shape empty → visible;
//! 5. otherwise hidden if the own shape is included in the neighbour's.
//!
//! The face shape is the slice of the occlusion shape at the edge (empty for a state that
//! does not occlude). It is "the full cube" only for a solid render state, or if the slice
//! is full AND all the boxes of the shape cover 0..1 on both axes of the side (the shape's
//! grid then has only one cell on those axes): the bottom face of a stair,
//! full but cut in two by the grid, is not.

use rc_data::{Aabb, VersionData, flags};

/// Paths of the mesh's per-block loop. Each returns the same mesh as the old one for the
/// same removal rule; `shape` chooses the rule (game, or old: one box covers the side).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CullPaths {
    /// 18³ grid of per-state properties, filled along with that of the states; the loop reads air,
    /// fluid, faces to remove and neighbour coverage from it (`RUSTCRAFT_OPT_MESHPROPS`).
    pub props: bool,
    /// Properties of the section interior read through its palette (table of a few entries)
    /// instead of a per-state lookup (`RUSTCRAFT_OPT_MESHPALPROPS`, with `props`).
    pub palprops: bool,
    /// Rows of 16 blocks: only the blocks that can produce a face are visited, in the
    /// same order (`RUSTCRAFT_OPT_MESHROWS`, with `props`).
    pub rows: bool,
    /// Border stacks read in one go from the neighbouring columns (`RUSTCRAFT_OPT_MESHPILE`).
    pub pile: bool,
    /// Game rule by face shapes (`RUSTCRAFT_OPT_SHAPECULL`); off: old rule.
    pub shape: bool,
}

impl CullPaths {
    pub const ALL: CullPaths = CullPaths { props: true, palprops: true, rows: true, pile: true, shape: true };
    pub const OLD: CullPaths = CullPaths { props: false, palprops: false, rows: false, pile: false, shape: false };
    /// Default: paths that win in the micro-bench (`docs/perf.md`, C2-M10), game rule.
    pub const DEFAULT: CullPaths = CullPaths { props: true, palprops: false, rows: true, pile: true, shape: true };

    /// Paths chosen by the environment (`=0` turns off, `=1` forces), read once.
    /// `RUSTCRAFT_OPT_MESHPROPS=0` really turns the grid off: the paths that read it (palette,
    /// rows) are turned off with it, whatever their variable.
    pub fn from_env() -> Self {
        static P: std::sync::OnceLock<CullPaths> = std::sync::OnceLock::new();
        *P.get_or_init(|| {
            let pick = |v: Option<&std::ffi::OsStr>, def: bool| match v {
                Some(v) if v == "0" => false,
                Some(v) if v == "1" => true,
                _ => def,
            };
            let d = CullPaths::DEFAULT;
            CullPaths {
                props: pick(rc_data::env_os!("RUSTCRAFT_OPT_MESHPROPS"), d.props),
                palprops: pick(rc_data::env_os!("RUSTCRAFT_OPT_MESHPALPROPS"), d.palprops),
                rows: pick(rc_data::env_os!("RUSTCRAFT_OPT_MESHROWS"), d.rows),
                pile: pick(rc_data::env_os!("RUSTCRAFT_OPT_MESHPILE"), d.pile),
                shape: pick(rc_data::env_os!("RUSTCRAFT_OPT_SHAPECULL"), d.shape),
            }
            .grid_cut()
        })
    }

    /// Grid off: its dependent paths are too (otherwise `uses_props` turns it back on).
    #[must_use]
    pub fn grid_cut(self) -> Self {
        if self.props { self } else { CullPaths { palprops: false, rows: false, ..self } }
    }

    /// The property grid is used (requested, or required by a path that reads it).
    #[inline]
    pub fn uses_props(&self) -> bool {
        self.props || self.palprops || self.rows
    }
}

/// Empty face shape (state that does not occlude, or shape that does not touch the side).
pub const FACE_EMPTY: u16 = 0;
/// "Full cube" face shape of the game.
pub const FACE_BLOCK: u16 = 1;

// Per-state properties (`CullTables::props`), a 32-bit word:
/// Bits 0-5 (direction of the neighbour seen from it, `Dir` order): its face hides a non-empty own face
/// (old rule: one box covers the side; game rule: the shape covers the whole side).
pub const HS: u32 = 0;
/// Bits 6-11: its face hides any face (old rule: like `HS`; game: full cube).
pub const HA: u32 = 6;
/// Bits 12-17: non-empty face shape (old rule: always).
pub const NE: u32 = 12;
/// Bits 18-23: model faces that are removed towards each direction.
pub const NEED: u32 = 18;
pub const ALWAYS_BIT: u32 = 1 << 24;
pub const AIR_BIT: u32 = 1 << 25;
pub const FLUID_BIT: u32 = 1 << 26;
pub const LEAF_BIT: u32 = 1 << 27;
pub const SEALED_BIT: u32 = 1 << 28;
/// Non-empty own face, non-empty neighbour without coverage: inclusion to read in the table.
const SIX: u32 = 0x3f;

/// Rectangle of a face shape on the two axes of the side: (min u, max u, min v, max v).
type Rect = [f64; 4];

/// Face shapes per state and inclusion table between shapes.
#[derive(Default)]
pub struct CullTables {
    /// Properties per state: `[old rule, game rule]`.
    pub props: [Vec<u32>; 2],
    /// Set of face shapes (per direction) of each state, index in `sets`.
    state_set: Vec<u32>,
    sets: Vec<[u16; 6]>,
    /// Number of distinct shapes (empty, full, then the others) and inclusion bits
    /// a ⊆ b at `a × n + b`.
    n: usize,
    contain: Vec<u64>,
    /// Rectangles of each shape (checks).
    rects: Vec<Vec<Rect>>,
    /// Per shape: largest H such that [0, H] × [0, 1] (index 0), then [0, 1] × [0, H]
    /// (index 1), is included in the shape; 0 if none. Used for fluid face removal.
    fluid_cover: Vec<[f64; 2]>,
}

/// Axis and side (`true`: max side) of a direction.
#[inline]
const fn axis_of(d: usize) -> (usize, bool) {
    match d {
        0 => (1, false),
        1 => (1, true),
        2 => (2, false),
        3 => (2, true),
        4 => (0, false),
        _ => (0, true),
    }
}

/// Transverse axes (u, v) of an axis.
#[inline]
const fn cross(axis: usize) -> (usize, usize) {
    match axis {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    }
}

const EPS: f64 = 1.0e-7;

/// Face shape of each side of an occlusion shape (`solid`: solid render state):
/// `None` empty, `Some(None)` full cube, `Some(Some(rects))` other.
#[allow(clippy::type_complexity)]
pub fn faces_of(boxes: &[Aabb], solid: bool) -> [Option<Option<Vec<Rect>>>; 6] {
    let mut out: [Option<Option<Vec<Rect>>>; 6] = Default::default();
    if boxes.is_empty() {
        return out;
    }
    if solid {
        return std::array::from_fn(|_| Some(None));
    }
    // Shape grid reduced to one cell on the axis: all boxes go from 0 to 1.
    let unit = |a: usize| boxes.iter().all(|b| (b.min[a]).abs() <= EPS && (b.max[a] - 1.0).abs() <= EPS);
    let units = [unit(0), unit(1), unit(2)];
    for (d, slot) in out.iter_mut().enumerate() {
        let (axis, at_max) = axis_of(d);
        // Grid cell that contains the point just inside the side.
        let at = if at_max { 1.0 - EPS } else { EPS };
        let (u, v) = cross(axis);
        let rects: Vec<Rect> = boxes
            .iter()
            .filter(|b| b.min[axis] <= at && at < b.max[axis])
            .map(|b| [b.min[u], b.max[u], b.min[v], b.max[v]])
            .collect();
        if rects.is_empty() {
            continue;
        }
        *slot = Some(if units[u] && units[v] { None } else { Some(rects) });
    }
    out
}

/// 16 × 16 mask of the rectangles if they are all in exact sixteenths.
fn mask16(rects: &[Rect]) -> Option<[u64; 4]> {
    let mut m = [0u64; 4];
    for r in rects {
        let c = r.map(|x| x * 16.0);
        if c.iter().any(|&x| x != x.round() || !(0.0..=16.0).contains(&x)) {
            return None;
        }
        for j in c[2] as usize..c[3] as usize {
            for i in c[0] as usize..c[1] as usize {
                let k = j * 16 + i;
                m[k >> 6] |= 1 << (k & 63);
            }
        }
    }
    Some(m)
}

/// Union of the rectangles `a` included in that of `b` (grid of the coordinates of both: each
/// cell is entirely inside a rectangle or outside it, exact test).
pub fn rects_within(a: &[Rect], b: &[Rect]) -> bool {
    let mut us: Vec<f64> = a.iter().chain(b).flat_map(|r| [r[0], r[1]]).collect();
    let mut vs: Vec<f64> = a.iter().chain(b).flat_map(|r| [r[2], r[3]]).collect();
    for c in [&mut us, &mut vs] {
        c.sort_by(f64::total_cmp);
        c.dedup();
    }
    let inside = |r: &Rect, u: f64, v: f64| r[0] <= u && u < r[1] && r[2] <= v && v < r[3];
    for w in us.windows(2) {
        for z in vs.windows(2) {
            let (u, v) = ((w[0] + w[1]) / 2.0, (z[0] + z[1]) / 2.0);
            if a.iter().any(|r| inside(r, u, v)) && !b.iter().any(|r| inside(r, u, v)) {
                return false;
            }
        }
    }
    true
}

impl CullTables {
    /// `covers`, `cull_dirs`, `air`, `leaves`, `sealed`, `fluid`: per-state tables of `Tables`.
    pub fn new(d: &VersionData, covers: &[u8], cull_dirs: &[u8], air: &[bool], leaves: &[bool], sealed: &[bool], fluid: &[bool]) -> Self {
        let full_rect: Vec<Rect> = vec![[0.0, 1.0, 0.0, 1.0]];
        let mut rects: Vec<Vec<Rect>> = vec![Vec::new(), full_rect.clone()];
        let mut masks: Vec<Option<[u64; 4]>> = vec![Some([0; 4]), Some([u64::MAX; 4])];
        // Distinct shapes: by mask if in sixteenths, otherwise by rectangles (bit for bit).
        let mut by_mask: hashbrown::HashMap<[u64; 4], u16> = hashbrown::HashMap::new();
        by_mask.insert([u64::MAX; 4], FACE_BLOCK);
        let mut by_rects: hashbrown::HashMap<Vec<[u64; 4]>, u16> = hashbrown::HashMap::new();
        let mut sets: Vec<[u16; 6]> = vec![[FACE_EMPTY; 6]];
        let mut set_of: hashbrown::HashMap<[u16; 6], u32> = hashbrown::HashMap::new();
        set_of.insert([FACE_EMPTY; 6], 0);
        let mut by_shape: hashbrown::HashMap<(u16, bool), u32> = hashbrown::HashMap::new();
        let mut state_set = Vec::with_capacity(d.states.len());
        for s in &d.states {
            if !s.has(flags::CAN_OCCLUDE) {
                state_set.push(0);
                continue;
            }
            let solid = s.has(flags::SOLID_RENDER);
            let set = *by_shape.entry((s.occlusion, solid)).or_insert_with(|| {
                let faces = faces_of(d.shape(s.occlusion), solid);
                let ids: [u16; 6] = std::array::from_fn(|k| match &faces[k] {
                    None => FACE_EMPTY,
                    Some(None) => FACE_BLOCK,
                    Some(Some(r)) => {
                        let m = mask16(r);
                        let found = match m {
                            // A full mask is not the game's full cube: separate shape.
                            Some(m) if m != [u64::MAX; 4] => by_mask.get(&m).copied(),
                            _ => {
                                let mut key: Vec<[u64; 4]> = r.iter().map(|x| x.map(f64::to_bits)).collect();
                                key.sort_unstable();
                                by_rects.get(&key).copied()
                            }
                        };
                        found.unwrap_or_else(|| {
                            let id = rects.len() as u16;
                            rects.push(r.clone());
                            masks.push(m);
                            match m {
                                Some(m) if m != [u64::MAX; 4] => {
                                    by_mask.insert(m, id);
                                }
                                _ => {
                                    let mut key: Vec<[u64; 4]> = r.iter().map(|x| x.map(f64::to_bits)).collect();
                                    key.sort_unstable();
                                    by_rects.insert(key, id);
                                }
                            }
                            id
                        })
                    }
                });
                *set_of.entry(ids).or_insert_with(|| {
                    sets.push(ids);
                    (sets.len() - 1) as u32
                })
            });
            state_set.push(set);
        }
        let n = rects.len();
        let mut contain = vec![0u64; (n * n).div_ceil(64)];
        for a in 0..n {
            for b in 0..n {
                let inside = match (masks[a], masks[b]) {
                    (Some(ma), Some(mb)) => (0..4).all(|i| ma[i] & !mb[i] == 0),
                    _ => rects_within(&rects[a], &rects[b]),
                };
                if inside {
                    let k = a * n + b;
                    contain[k >> 6] |= 1 << (k & 63);
                }
            }
        }
        // Coverage of a band starting at the edge: inclusion is monotonic in H, the largest H
        // is a coordinate of the shape (tries from largest to smallest).
        let fluid_cover = rects
            .iter()
            .map(|r| {
                std::array::from_fn(|k| {
                    let mut cs: Vec<f64> = r.iter().map(|x| x[2 * k + 1]).filter(|&c| c > 0.0).collect();
                    cs.sort_by(|a, b| b.total_cmp(a));
                    cs.into_iter()
                        .find(|&c| rects_within(&[if k == 0 { [0.0, c, 0.0, 1.0] } else { [0.0, 1.0, 0.0, c] }], r))
                        .unwrap_or(0.0)
                })
            })
            .collect();
        let mut t = CullTables { props: [Vec::new(), Vec::new()], state_set, sets, n, contain, rects, fluid_cover };
        let full: Vec<bool> = (0..n).map(|id| t.within(FACE_BLOCK, id as u16)).collect();
        for (rule, out) in t.props.iter_mut().enumerate() {
            *out = (0..d.states.len())
                .map(|s| {
                    let mut p = 0u32;
                    if rule == 0 {
                        let c = u32::from(covers[s] & 0x3f);
                        p |= c << HS | c << HA | SIX << NE;
                    } else {
                        let ids = sets_ref(&t.sets, &t.state_set, s);
                        for (k, &id) in ids.iter().enumerate() {
                            let k = k as u32;
                            if id != FACE_EMPTY {
                                p |= 1 << (NE + k);
                            }
                            if id == FACE_BLOCK {
                                p |= 1 << (HA + k);
                            }
                            if full[id as usize] && id != FACE_EMPTY {
                                p |= 1 << (HS + k);
                            }
                        }
                    }
                    let need = cull_dirs[s];
                    p |= u32::from(need & 0x3f) << NEED;
                    if need & 0x80 != 0 {
                        p |= ALWAYS_BIT;
                    }
                    for (flag, on) in [(AIR_BIT, air[s]), (FLUID_BIT, fluid[s]), (LEAF_BIT, leaves[s]), (SEALED_BIT, sealed[s])] {
                        if on {
                            p |= flag;
                        }
                    }
                    p
                })
                .collect();
        }
        t
    }

    /// Number of distinct face shapes (empty and full included).
    pub fn shapes(&self) -> usize {
        self.n
    }

    /// Shapes with a side that is not in exact sixteenths.
    pub fn inexact(&self) -> usize {
        self.rects.iter().filter(|r| mask16(r).is_none()).count()
    }

    /// Rectangles of a face shape (checks).
    pub fn rects(&self, id: u16) -> &[Rect] {
        &self.rects[id as usize]
    }

    /// Face shape of state `s` towards `d` (unknown state: empty).
    #[inline]
    pub fn face(&self, s: u32, d: usize) -> u16 {
        sets_ref(&self.sets, &self.state_set, s as usize)[d]
    }

    /// Shape `a` is included in shape `b`.
    #[inline]
    pub fn within(&self, a: u16, b: u16) -> bool {
        let k = a as usize * self.n + b as usize;
        self.contain[k >> 6] >> (k & 63) & 1 == 1
    }

    /// Game rule (steps 1, 3, 4, 5; the "same block" rule is separate): the face of state
    /// `s` turned towards `d` is hidden by the neighbour `n`.
    pub fn hidden_by_shape(&self, s: u32, n: u32, d: usize) -> bool {
        let nf = self.face(n, d ^ 1);
        if nf == FACE_BLOCK {
            return true;
        }
        if nf == FACE_EMPTY {
            return false;
        }
        let sf = self.face(s, d);
        sf != FACE_EMPTY && self.within(sf, nf)
    }

    /// Game rule for a fluid: its face turned towards `d`, of height `h`, is hidden by
    /// the neighbouring state `n`. "Full cube" neighbour: hidden, except the top of a fluid lower than
    /// 1; other non-empty shape: hidden if the rectangle of the fluid's face (side: full
    /// width over 0..h; bottom: full; top: full and only if h is 1) is included
    /// in the neighbour's shape, coordinates merged to within 1e-7 as in the game.
    pub fn fluid_hidden(&self, n: u32, d: usize, h: f32) -> bool {
        let nf = self.face(n, d ^ 1);
        if nf == FACE_EMPTY {
            return false;
        }
        if nf == FACE_BLOCK {
            return d != 1 || h == 1.0;
        }
        let [hu, hv] = self.fluid_cover[nf as usize];
        let h = f64::from(h);
        match d {
            0 => hu >= 1.0,
            1 => (h - 1.0).abs() <= EPS && hu >= 1.0,
            // North and south sides: the height is the second axis (y); west and east: the first.
            2 | 3 => h > EPS && h <= hv + EPS,
            _ => h > EPS && h <= hu + EPS,
        }
    }

    /// Game rule for a fluid in a block (waterlogged block): its face towards `d` is hidden
    /// by the block's own shape if it covers that whole side (union of the boxes).
    pub fn fluid_self_hidden(&self, s: u32, d: usize) -> bool {
        let f = self.face(s, d);
        f != FACE_EMPTY && self.fluid_cover[f as usize][0] >= 1.0
    }

    /// Properties of state `s` under rule `rule` (outside the columns: none; unknown
    /// state: air, like the per-state tables).
    #[inline]
    pub fn prop(&self, rule: usize, s: u32) -> u32 {
        if s == u32::MAX {
            return 0;
        }
        self.props[rule].get(s as usize).copied().unwrap_or(AIR_BIT)
    }
}

#[inline]
fn sets_ref<'a>(sets: &'a [[u16; 6]], state_set: &[u32], s: usize) -> &'a [u16; 6] {
    &sets[state_set.get(s).copied().unwrap_or(0) as usize]
}

/// Faces surely hidden of the property block `p` from its six neighbours `nb` (`Dir` order):
/// full coverage, without the "same block" rule, leaves or shape inclusion.
#[inline(always)]
pub fn sure_hidden(p: u32, nb: [u32; 6]) -> u32 {
    let mut h = 0u32;
    for (d, &n) in nb.iter().enumerate() {
        let o = (d ^ 1) as u32;
        let own = p >> (NE + d as u32) & 1;
        h |= ((n >> (HA + o)) & 1 | (own & (n >> (HS + o)) & 1)) << d;
    }
    h
}

/// The property block `p` can produce a face (or a fluid): non-air, and fluid, face
/// never removed, or a face to remove that is not surely hidden.
#[inline(always)]
pub fn candidate(p: u32, sure: u32) -> bool {
    p & AIR_BIT == 0 && (p & (FLUID_BIT | ALWAYS_BIT) != 0 || (p >> NEED) & SIX & !sure != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rc_assets::geom::Dir;

    fn bx(min: [f64; 3], max: [f64; 3]) -> Aabb {
        Aabb { min, max }
    }

    #[test]
    fn face_shapes_follow_the_grid_rule() {
        // Bottom slab: full bottom (the game's full cube), empty top, half sides.
        let slab = [bx([0.0; 3], [1.0, 0.5, 1.0])];
        let f = faces_of(&slab, false);
        assert_eq!(f[Dir::Down as usize], Some(None));
        assert_eq!(f[Dir::Up as usize], None);
        assert_eq!(f[Dir::North as usize], Some(Some(vec![[0.0, 1.0, 0.0, 0.5]])));
        // Stair (full bottom + quarter top on the south side): full bottom but grid cut in z.
        let stairs = [bx([0.0; 3], [1.0, 0.5, 1.0]), bx([0.0, 0.5, 0.5], [1.0, 1.0, 1.0])];
        let f = faces_of(&stairs, false);
        assert!(matches!(&f[Dir::Down as usize], Some(Some(r)) if r.len() == 1));
        assert!(matches!(&f[Dir::South as usize], Some(Some(r)) if r.len() == 2));
        assert!(matches!(&f[Dir::Up as usize], Some(Some(r)) if r == &vec![[0.0, 1.0, 0.5, 1.0]]));
        // Empty shape, solid state.
        assert!(faces_of(&[], false).iter().all(Option::is_none));
        assert!(faces_of(&slab, true).iter().all(|f| *f == Some(None)));
    }

    #[test]
    fn rect_inclusion_is_exact() {
        let half = [[0.0, 1.0, 0.0, 0.5]];
        let full = [[0.0, 1.0, 0.0, 1.0]];
        let split = [[0.0, 0.5, 0.0, 1.0], [0.5, 1.0, 0.0, 1.0]];
        assert!(rects_within(&half, &full));
        assert!(!rects_within(&full, &half));
        assert!(rects_within(&full, &split));
        assert!(rects_within(&split, &full));
        let third = [[0.0, 1.0, 0.0, 1.0 / 3.0]];
        assert!(rects_within(&third, &half));
        assert!(!rects_within(&half, &third));
        assert_eq!(mask16(&third), None);
        assert_eq!(mask16(&half).map(|m| m.iter().map(|w| w.count_ones()).sum::<u32>()), Some(128));
    }

    #[test]
    fn props_off_cuts_the_grid() {
        let off = CullPaths { props: false, ..CullPaths::DEFAULT }.grid_cut();
        assert!(!off.uses_props(), "{off:?}");
        assert_eq!(off.pile, CullPaths::DEFAULT.pile);
        assert_eq!(off.shape, CullPaths::DEFAULT.shape);
        assert_eq!(CullPaths::ALL.grid_cut(), CullPaths::ALL);
    }

    #[test]
    fn sure_hidden_is_a_lower_bound() {
        // Old rule: neighbour coverage only.
        let me = SIX << NE | 0x3f << NEED;
        let cover_up = 1 << (HS + Dir::Down as u32) | 1 << (HA + Dir::Down as u32);
        let nb = [0, cover_up, 0, 0, 0, 0];
        assert_eq!(sure_hidden(me, nb), 1 << Dir::Up as u32);
        assert!(candidate(me, sure_hidden(me, nb)));
        assert!(!candidate(me, 0x3f));
        assert!(!candidate(me | AIR_BIT, 0));
        assert!(candidate(me | FLUID_BIT, 0x3f));
        // Game rule: covering neighbour that is not the full cube, own face empty → visible.
        let hs_only = 1 << (HS + Dir::Down as u32);
        assert_eq!(sure_hidden(0x3f << NEED, [0, hs_only, 0, 0, 0, 0]), 0);
        assert_eq!(sure_hidden(me, [0, hs_only, 0, 0, 0, 0]), 1 << Dir::Up as u32);
    }
}
