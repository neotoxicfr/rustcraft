//! Proofs of the collision tables and walk on the game's real data: fence gate recognised by its
//! kind (fidelity fix), borrowed shape and precomputed top (`RUSTCRAFT_OPT_SHAPEREF`), `is_free`
//! without collection (`RUSTCRAFT_OPT_ISFREE`), collection through the common walk, walk trimmed
//! to the reach of the shapes (`RUSTCRAFT_OPT_COLLISION_SCAN_TRIM`), `resolve` without copies
//! (`RUSTCRAFT_OPT_RESOLVE_NO_COPY`). Micro-benches ignored by default (`--ignored`, 1 thread then
//! all threads).

use super::*;
use crate::collide::{
    EPS, IsFreePath, ResolvePath, WalkOrder, WalkPath, collect, collect_ordered, is_free_with, resolve_with, walk_cells,
};

fn data() -> Option<VersionData> {
    rc_testdata::need(rc_testdata::version_data(), "26.3 data cache")
}

/// Fixed randomness of the proofs (splitmix64).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn f(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// A single state everywhere.
struct One(u32);
impl World for One {
    fn state(&self, _: i32, _: i32, _: i32) -> Option<u32> {
        Some(self.0)
    }
    fn collision(&self, _: i32, _: i32, _: i32, _: &mut Vec<Aabb>) -> bool {
        false
    }
}

#[test]
fn fence_gate_keeps_its_support_without_tag() {
    let Some(d) = data() else { return };
    let mut t = Tables::new(&d);
    let gates: Vec<u32> = d.blocks.iter().filter(|b| is_fence_gate(&b.name)).map(|b| b.default_state).collect();
    assert_eq!(gates.len(), 13, "fence gates of the 26.3 game (wood, bamboo, Nether)");
    let fence = d.blocks.iter().position(|b| b.name == "minecraft:oak_fence").expect("fence");
    let wall = d.blocks.iter().position(|b| b.name == "minecraft:cobblestone_wall").expect("wall");
    let below = |t: &Tables, s: u32, h: f32| below_of(Some([0, 5, 0]), [0.5, 5.5, 0.5], t, &One(s), h);
    // Tags reloaded without `fence_gates` (nor any other): the fence gate stays guarded.
    t.set_block_tags(&[]);
    for &s in &gates {
        assert_eq!(below(&t, s, 0.6), [0, 5, 0], "fence gate {s} without tag");
    }
    let fence_s = d.blocks[fence].default_state;
    let wall_s = d.blocks[wall].default_state;
    // Without a tag, fence and wall are not guarded (offset 0.6: the cell under the feet).
    assert_eq!(below(&t, fence_s, 0.6), [0, 4, 0]);
    assert_eq!(below(&t, wall_s, 0.6), [0, 4, 0]);
    // With their tags: fence guarded up to 0.5, wall always.
    t.set_block_tags(&[
        ("minecraft:fences".into(), vec![fence as i32]),
        ("minecraft:walls".into(), vec![wall as i32]),
    ]);
    assert_eq!(below(&t, fence_s, 0.5), [0, 5, 0]);
    assert_eq!(below(&t, fence_s, 0.6), [0, 4, 0]);
    assert_eq!(below(&t, wall_s, 0.6), [0, 5, 0]);
    // A block put in `fence_gates` without being a fence gate is not guarded.
    let stone = d.blocks.iter().position(|b| b.name == "minecraft:stone").expect("stone");
    t.set_block_tags(&[("minecraft:fence_gates".into(), vec![stone as i32])]);
    assert_eq!(below(&t, d.blocks[stone].default_state, 0.6), [0, 4, 0]);
    for &s in &gates {
        assert_eq!(below(&t, s, 0.6), [0, 5, 0]);
    }
}

/// Old top: shape copied into a new `Vec` then folded.
fn top_old(t: &Tables, s: u32) -> Option<f64> {
    let mut v = Vec::new();
    t.collision(s, &mut v);
    v.iter().map(|b| b.max[1]).fold(None, |m, y| Some(m.map_or(y, |m: f64| m.max(y))))
}

#[test]
fn borrowed_shape_and_top_per_state_identical() {
    let Some(d) = data() else { return };
    let t = Tables::new(&d);
    let n = d.states.len() as u32;
    let (mut empty, mut tops) = (0usize, 0usize);
    // All the states, plus unknown states (beyond the table).
    for s in (0..n).chain([n, n + 1, u32::MAX]) {
        let mut v = Vec::new();
        t.collision(s, &mut v);
        assert_eq!(t.shape(s), v.as_slice(), "shape of state {s}");
        let old = top_old(&t, s);
        assert_eq!(t.collision_top(s).map(f64::to_bits), old.map(f64::to_bits), "walked top, state {s}");
        assert_eq!(t.collision_top_table(s).map(f64::to_bits), old.map(f64::to_bits), "table top, state {s}");
        assert_eq!(t.shape(s).is_empty(), v.is_empty());
        if old.is_none() {
            empty += 1;
        } else if old != Some(1.0) {
            tops += 1;
        }
    }
    assert!(empty > 1000 && tops > 1000, "coverage: {empty} empty, {tops} non-full tops");
}

/// Mixed world: states drawn from all those of the game (four cells in five of air), solid entities (boat,
/// shulker box) and border nearby.
struct Mix<'a> {
    t: &'a Tables,
    n: u64,
    ents: Vec<Aabb>,
    border: Option<border::WorldBorder>,
    /// Reach announced to the collision walk: the tables' one, a wrong one in the sabotages.
    reach: Aabb,
    /// Moving blocks: cell and offset of their full-cube shape, one per direction (like a block
    /// pushed by a piston, the shape leaves its cell).
    movers: Vec<([i32; 3], [f64; 3])>,
}

impl Mix<'_> {
    fn mover_shape(&self, p: [i32; 3], out: &mut Vec<Aabb>) -> bool {
        let Some((_, o)) = self.movers.iter().find(|(c, _)| *c == p) else { return false };
        out.push(Aabb::UNIT.offset(o[0], o[1], o[2]));
        true
    }
}

impl World for Mix<'_> {
    fn state(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        let h = Rng((x as u64) ^ ((y as u64) << 21) ^ ((z as u64) << 42)).next();
        Some(if !h.is_multiple_of(5) { 0 } else { ((h >> 8) % self.n) as u32 })
    }
    fn collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) -> bool {
        self.mover_shape([x, y, z], out) || self.state(x, y, z).is_some_and(|s| self.t.collision(s, out))
    }
    fn border(&self) -> Option<&border::WorldBorder> {
        self.border.as_ref()
    }
    fn entity_colliders(&self, area: &Aabb, out: &mut Vec<Aabb>) {
        entity_boxes_in(&self.ents, area, out);
    }
    fn moving_collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) -> bool {
        self.mover_shape([x, y, z], out)
    }
    fn shape_reach(&self, lo: [i32; 3], hi: [i32; 3]) -> Aabb {
        let walked = |p: &[i32; 3]| (0..3).all(|a| lo[a] <= p[a] && p[a] <= hi[a]);
        if self.movers.iter().any(|(p, _)| walked(p)) { Aabb::EVERYWHERE } else { self.reach }
    }
    fn states_in(&self, first: [i32; 3], last: [i32; 3], out: &mut [u32]) -> bool {
        let mut i = 0;
        for y in first[1]..=last[1] {
            for z in first[2]..=last[2] {
                for x in first[0]..=last[0] {
                    out[i] = self.state(x, y, z).unwrap_or(UNLOADED);
                    i += 1;
                }
            }
        }
        true
    }
    fn collision_of_state(&self, state: Option<u32>, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) -> bool {
        self.mover_shape([x, y, z], out) || state.is_some_and(|s| self.t.collision(s, out))
    }
}

/// The six directions a piston pushes in, with the shape 0.6 of the way.
const PUSHES: [[f64; 3]; 6] =
    [[0.6, 0.0, 0.0], [-0.6, 0.0, 0.0], [0.0, 0.6, 0.0], [0.0, -0.6, 0.0], [0.0, 0.0, 0.6], [0.0, 0.0, -0.6]];

fn mix<'a>(t: &'a Tables, d: &VersionData) -> Mix<'a> {
    // Boat (1.375 × 0.5625) and shulker box (cube) placed near the origin; border of
    // 40 blocks centred at (0, 0); one moving block per direction along x = 10.
    let boat = Aabb { min: [2.3125, 64.0, 2.3125], max: [3.6875, 64.5625, 3.6875] };
    let shulker = Aabb { min: [-4.0, 64.0, -4.0], max: [-3.0, 65.0, -3.0] };
    let mut b = border::WorldBorder::default();
    b.set_size(40.0);
    let movers = PUSHES.iter().enumerate().map(|(i, &o)| ([10, 64, 4 * i as i32 - 10], o)).collect();
    Mix { t, n: d.states.len() as u64, ents: vec![boat, shulker], border: Some(b), reach: t.shape_reach(), movers }
}

/// Test boxes around the moving blocks of `w` (`n` per block), touching the shifted shape.
fn boxes_near_movers(n: usize, w: &Mix) -> Vec<Aabb> {
    let mut r = Rng(0xC5_0013);
    let mut out = Vec::new();
    for &(p, o) in &w.movers {
        for _ in 0..n {
            let size = [0.1 + r.f() * 1.4, 0.1 + r.f() * 1.9, 0.1 + r.f() * 1.4];
            let min: [f64; 3] = std::array::from_fn(|a| f64::from(p[a]) + o[a] + r.f() * 2.0 - 1.0 - size[a] * 0.5);
            out.push(Aabb { min, max: std::array::from_fn(|a| min[a] + size[a]) });
        }
    }
    out
}

/// Test boxes: drawn around the entities (often in exact contact with a face) and up to
/// the border.
fn boxes(n: usize, w: &Mix) -> Vec<Aabb> {
    let mut r = Rng(0xC6_0002);
    (0..n)
        .map(|i| {
            let size = [0.1 + r.f() * 1.4, 0.1 + r.f() * 1.9, 0.1 + r.f() * 1.4];
            let min = if i % 4 == 0 {
                // In contact with a face of an entity.
                let e = w.ents[(r.next() % 2) as usize];
                let mut m = [e.min[0] + r.f() - 0.5, e.min[1] + r.f() * 0.5, e.min[2] + r.f() - 0.5];
                match r.next() % 3 {
                    0 => m[0] = e.max[0],
                    1 => m[1] = e.max[1],
                    _ => m[2] = e.max[2] - size[2] - (r.next() % 2) as f64 * 1.0e-8,
                }
                m
            } else {
                [r.f() * 50.0 - 25.0, 60.0 + r.f() * 10.0, r.f() * 50.0 - 25.0]
            };
            Aabb { min, max: [min[0] + size[0], min[1] + size[1], min[2] + size[2]] }
        })
        .collect()
}

#[test]
fn is_free_identical_at_all_levels() {
    let Some(d) = data() else { return };
    let t = Tables::new(&d);
    let w = mix(&t, &d);
    let (mut free, mut by_ent, mut by_border) = (0, 0, 0);
    for b in boxes(10_000, &w) {
        let old = is_free_with(&w, &b, IsFreePath::CollectAll);
        assert_eq!(is_free_with(&w, &b, IsFreePath::FirstOverlap), old, "level 1, box {b:?}");
        assert_eq!(is_free_with(&w, &b, IsFreePath::FirstOverlapSharedBuffer), old, "level 2, box {b:?}");
        free += usize::from(old);
        let no_ent = Mix { ents: Vec::new(), ..mix(&t, &d) };
        if !old && is_free_with(&no_ent, &b, IsFreePath::CollectAll) {
            by_ent += 1;
        }
        let no_border = Mix { border: None, ..mix(&t, &d) };
        if !old && is_free_with(&no_border, &b, IsFreePath::CollectAll) {
            by_border += 1;
        }
    }
    assert!(free > 1000 && free < 9000, "coverage: {free} free boxes out of 10,000");
    assert!(by_ent > 50 && by_border > 50, "coverage: {by_ent} blocked by an entity, {by_border} by the border");
}

/// Old collection (new buffer, own loop), reference of the common walk.
fn collect_old(w: &dyn World, b: &Aabb, out: &mut Vec<Aabb>, zyx: bool) {
    out.clear();
    let lo = [0, 1, 2].map(|a| (b.min[a] - EPS).floor() as i32 - 1);
    let hi = [0, 1, 2].map(|a| (b.max[a] + EPS).floor() as i32 + 1);
    let mut cell = Vec::new();
    let (o, m) = if zyx { (2, 1) } else { (1, 2) };
    for a in lo[o]..=hi[o] {
        for c in lo[m]..=hi[m] {
            let (y, z) = if zyx { (c, a) } else { (a, c) };
            for x in lo[0]..=hi[0] {
                let edges = u8::from(x == lo[0] || x == hi[0])
                    + u8::from(y == lo[1] || y == hi[1])
                    + u8::from(z == lo[2] || z == hi[2]);
                cell.clear();
                if edges >= 3 || (edges == 2 && !w.moving_collision(x, y, z, &mut cell)) {
                    continue;
                }
                if edges < 2 {
                    let oversized = w.collision(x, y, z, &mut cell);
                    if edges == 1 && !oversized {
                        continue;
                    }
                }
                for local in &cell {
                    let bx = local.offset(f64::from(x), f64::from(y), f64::from(z));
                    if bx.intersects(b) {
                        out.push(bx);
                    }
                }
            }
        }
    }
}

#[test]
fn collection_identical_to_the_old_one() {
    let Some(d) = data() else { return };
    let t = Tables::new(&d);
    let w = mix(&t, &d);
    let (mut a, mut b2) = (Vec::new(), Vec::new());
    let mut total = 0;
    for b in boxes(3_000, &w) {
        collect(&w, &b, &mut a);
        collect_old(&w, &b, &mut b2, false);
        assert_eq!(a, b2);
        collect_ordered(&w, &b, &mut a);
        collect_old(&w, &b, &mut b2, true);
        assert_eq!(a, b2);
        total += a.len();
    }
    assert!(total > 3_000, "coverage: {total} boxes collected");
}

/// Stored figures the trimmed walk relies on: in 26.3 only piston heads and pitcher crops leave
/// their cell sideways or below (by 0.25), fences and walls rise to 1.5; every shape lies inside
/// the reach of its kind.
#[test]
fn shape_reach_of_the_game() {
    let Some(d) = data() else { return };
    let t = Tables::new(&d);
    let mut spilling: Vec<&str> = (0..d.states.len() as u32)
        .filter(|&s| t.shape_extent(s) == ShapeExtent::Spills)
        .filter_map(|s| d.block_of(s).map(|b| b.name.as_str()))
        .collect();
    spilling.dedup();
    assert_eq!(spilling, ["minecraft:piston_head", "minecraft:pitcher_crop"]);
    let (all, common) = (t.shape_reach(), t.common_shape_reach());
    assert_eq!((all.min, all.max), ([-0.25; 3], [1.25, 1.5, 1.25]));
    assert_eq!((common.min, common.max), ([0.0; 3], [1.0, 1.5, 1.0]));
    let inside = |b: &Aabb, r: &Aabb| (0..3).all(|a| b.min[a] >= r.min[a] && b.max[a] <= r.max[a]);
    let (mut taller, mut empty) = (0, 0);
    for s in 0..d.states.len() as u32 {
        let extent = t.shape_extent(s);
        assert_eq!(extent == ShapeExtent::Empty, t.shape(s).is_empty(), "state {s}");
        empty += usize::from(extent == ShapeExtent::Empty);
        for b in t.shape(s) {
            assert!(inside(b, &all), "state {s}: {b:?}");
            assert!(extent == ShapeExtent::Spills || inside(b, &common), "state {s}: {b:?}");
            taller += usize::from(b.max[1] > 1.0);
        }
    }
    assert!(taller > 1000 && empty > 1000, "coverage: {taller} boxes above their cell, {empty} empty shapes");
}

/// `RUSTCRAFT_OPT_STAND_SHAPE_FLAGS`: the per-state flags of the mob envelopes say what the block
/// names said, on every state (powder snow; lava source = the lava block at level 0).
#[test]
fn stand_flags_match_the_block_names() {
    let Some(d) = data() else { return };
    let t = Tables::new(&d);
    let (mut snow, mut sources) = (0, 0);
    for s in 0..d.states.len() as u32 + 2 {
        let name = d.block_of(s).map_or("minecraft:air", |b| b.name.as_str());
        let level_0 = d.state_properties(s).any(|(k, v)| k == "level" && v == "0");
        assert_eq!(t.is_powder_snow(s), name == "minecraft:powder_snow", "state {s}");
        assert_eq!(t.is_lava_source(s), name == "minecraft:lava" && level_0, "state {s}");
        snow += usize::from(t.is_powder_snow(s));
        sources += usize::from(t.is_lava_source(s));
    }
    assert_eq!((snow, sources), (1, 1));
}

/// The player's envelope announces the unit cube for its own shapes: scaffolding platforms
/// (from the tables) never leave their cell.
#[test]
fn scaffolding_shapes_stay_in_their_cell() {
    let Some(d) = data() else { return };
    let t = Tables::new(&d);
    let scaffolding: Vec<u32> = (0..d.states.len() as u32).filter(|&s| t.is_scaffolding(s)).collect();
    assert!(!scaffolding.is_empty());
    for s in scaffolding {
        assert!(t.shape(s).iter().all(|b| (0..3).all(|a| b.min[a] >= 0.0 && b.max[a] <= 1.0)), "state {s}");
    }
}

/// Palette summary: nothing to reach without a shape, the common reach without a spilling state,
/// the whole reach otherwise.
#[test]
fn reach_from_palettes_by_kind() {
    let Some(d) = data() else { return };
    let t = Tables::new(&d);
    let state = |name: &str| d.blocks.iter().find(|b| b.name == name).map(|b| b.default_state).expect(name);
    let holding = |states: Vec<u32>| t.reach_from_palettes(|pred| states.iter().any(|&s| pred(s)));
    assert_eq!(holding(vec![0, state("minecraft:short_grass")]), Aabb::NOTHING);
    assert_eq!(holding(vec![0, state("minecraft:oak_fence")]), t.common_shape_reach());
    assert_eq!(holding(vec![state("minecraft:stone"), state("minecraft:piston_head")]), t.shape_reach());
}

/// Trimmed and batched walk (the default path).
const NEW_WALK: WalkPath = WalkPath { trim: true, batch: true };

/// Cells and boxes of a walk, in order, without the check mode: on the new path (trimmed,
/// batched) or the whole one.
fn walked(w: &dyn World, b: &Aabb, order: WalkOrder, new_path: bool) -> Vec<([i32; 3], Aabb)> {
    let mut found = Vec::new();
    let path = if new_path { NEW_WALK } else { WalkPath::WHOLE };
    walk_cells(w, b, order, path, &mut Vec::new(), |c, bx| {
        found.push((c, bx));
        false
    });
    found
}

/// Boxes of the Mix world, then around its six moving blocks.
fn walk_boxes(w: &Mix) -> Vec<Aabb> {
    boxes(3_000, w).into_iter().chain(boxes_near_movers(300, w)).collect()
}

/// `RUSTCRAFT_OPT_COLLISION_SCAN_TRIM`: the trimmed walk finds the same cells and boxes, in the
/// same order, as the whole one, in both orders, moving blocks pushed in the six directions included.
#[test]
fn trimmed_walk_identical_to_the_whole_one() {
    let Some(d) = data() else { return };
    let t = Tables::new(&d);
    let w = mix(&t, &d);
    let (mut found, mut moving, mut above) = (0, 0, 0);
    for b in walk_boxes(&w) {
        for order in [WalkOrder::YOuter, WalkOrder::ZOuter] {
            let whole = walked(&w, &b, order, false);
            assert_eq!(walked(&w, &b, order, true), whole, "box {b:?}, {order:?}");
            found += whole.len();
            moving += whole.iter().filter(|(c, _)| w.movers.iter().any(|(p, _)| p == c)).count();
            above += whole.iter().filter(|(c, bx)| bx.max[1] > f64::from(c[1]) + 1.0).count();
        }
    }
    assert!(found > 10_000 && moving > 500 && above > 200, "coverage: {found} boxes, {moving} moving, {above} above their cell");
}

/// First test box whose trimmed walk differs from the whole one.
fn first_difference(w: &Mix) -> Option<Aabb> {
    walk_boxes(w).into_iter().find(|b| walked(w, b, WalkOrder::YOuter, true) != walked(w, b, WalkOrder::YOuter, false))
}

/// Sabotages of the reach, each caught by the proof above: a top at 1.0 loses the fences and walls
/// of the bottom margin row, a bottom at 0.5 the bottom slabs, the common reach the piston heads
/// and pitcher crops of the margins, and forgetting the moving blocks loses their shifted shapes.
#[test]
fn a_wrong_reach_changes_the_walk() {
    let Some(d) = data() else { return };
    let t = Tables::new(&d);
    let all = t.shape_reach();
    let low_top = Mix { reach: Aabb { max: [all.max[0], 1.0, all.max[2]], ..all }, ..mix(&t, &d) };
    assert!(first_difference(&low_top).is_some(), "a reach topped at 1.0 must lose a box");
    let high_bottom = Mix { reach: Aabb { min: [all.min[0], 0.5, all.min[2]], ..all }, ..mix(&t, &d) };
    assert!(first_difference(&high_bottom).is_some(), "a reach starting at 0.5 must lose a box");
    // The moving blocks need the unbounded reach: a world that forgets them loses their boxes.
    let w = mix(&t, &d);
    let forgets = ForgetsMovers(&w);
    let lost = boxes_near_movers(300, &w)
        .into_iter()
        .any(|b| walked(&forgets, &b, WalkOrder::YOuter, true) != walked(&w, &b, WalkOrder::YOuter, false));
    assert!(lost, "a moving block's shape leaves the tables' reach");
}

/// One block in the cell at the origin, air elsewhere, and the reach it announces.
struct Lone<'a> {
    t: &'a Tables,
    state: u32,
    reach: Aabb,
}

impl World for Lone<'_> {
    fn state(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        Some(if [x, y, z] == [0, 0, 0] { self.state } else { 0 })
    }
    fn collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) -> bool {
        self.state(x, y, z).is_some_and(|s| self.t.collision(s, out))
    }
    fn shape_reach(&self, _: [i32; 3], _: [i32; 3]) -> Aabb {
        self.reach
    }
}

/// Every state that spills out of its cell (piston heads, pitcher crops): the tables' reach keeps
/// all its boxes around the cell, the common reach (the sabotage, as if the palette had been
/// misread) loses some for each of these blocks.
#[test]
fn spilling_states_need_the_whole_reach() {
    let Some(d) = data() else { return };
    let t = Tables::new(&d);
    let mut r = Rng(0x5_7111);
    let around: Vec<Aabb> = (0..400)
        .map(|_| {
            let size = [0.05 + r.f() * 0.6, 0.05 + r.f() * 0.6, 0.05 + r.f() * 0.6];
            let min: [f64; 3] = std::array::from_fn(|a| r.f() * 2.2 - 0.6 - size[a] * 0.5);
            Aabb { min, max: std::array::from_fn(|a| min[a] + size[a]) }
        })
        .collect();
    let mut lost_by_block: Vec<&str> = Vec::new();
    for s in (0..d.states.len() as u32).filter(|&s| t.shape_extent(s) == ShapeExtent::Spills) {
        let right = Lone { t: &t, state: s, reach: t.shape_reach() };
        let wrong = Lone { reach: t.common_shape_reach(), ..right };
        let mut lost = false;
        for b in &around {
            let whole = walked(&right, b, WalkOrder::YOuter, false);
            assert_eq!(walked(&right, b, WalkOrder::YOuter, true), whole, "state {s}, box {b:?}");
            lost |= walked(&wrong, b, WalkOrder::YOuter, true) != whole;
        }
        if lost {
            lost_by_block.push(d.block_of(s).map_or("?", |b| b.name.as_str()));
        }
    }
    lost_by_block.dedup();
    assert_eq!(lost_by_block, ["minecraft:piston_head", "minecraft:pitcher_crop"]);
}

/// Sabotage of the batched read: the states written z fastest instead of x fastest.
struct Transposed<'a>(&'a Mix<'a>);

impl World for Transposed<'_> {
    fn state(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        self.0.state(x, y, z)
    }
    fn collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) -> bool {
        self.0.collision(x, y, z, out)
    }
    fn shape_reach(&self, lo: [i32; 3], hi: [i32; 3]) -> Aabb {
        self.0.shape_reach(lo, hi)
    }
    fn states_in(&self, first: [i32; 3], last: [i32; 3], out: &mut [u32]) -> bool {
        let mut i = 0;
        for y in first[1]..=last[1] {
            for x in first[0]..=last[0] {
                for z in first[2]..=last[2] {
                    out[i] = self.0.state(x, y, z).unwrap_or(UNLOADED);
                    i += 1;
                }
            }
        }
        true
    }
    fn collision_of_state(&self, state: Option<u32>, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) -> bool {
        self.0.collision_of_state(state, x, y, z, out)
    }
}

/// `RUSTCRAFT_OPT_COLLISION_BATCH_READ`: the proof above sees a batched read in the wrong order.
#[test]
fn a_wrong_batch_order_changes_the_walk() {
    let Some(d) = data() else { return };
    let t = Tables::new(&d);
    let w = mix(&t, &d);
    let transposed = Transposed(&w);
    let lost = boxes(3_000, &w)
        .into_iter()
        .any(|b| walked(&transposed, &b, WalkOrder::YOuter, true) != walked(&w, &b, WalkOrder::YOuter, false));
    assert!(lost, "states read in the wrong order must change the walk");
}

/// Sabotage: the Mix world announcing the tables' reach even where a block moves.
struct ForgetsMovers<'a>(&'a Mix<'a>);

impl World for ForgetsMovers<'_> {
    fn state(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        self.0.state(x, y, z)
    }
    fn collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) -> bool {
        self.0.collision(x, y, z, out)
    }
    fn moving_collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) -> bool {
        self.0.moving_collision(x, y, z, out)
    }
    fn shape_reach(&self, _: [i32; 3], _: [i32; 3]) -> Aabb {
        self.0.reach
    }
}

/// The check mode (always on in this crate's tests) catches a wrong reach.
#[test]
#[should_panic(expected = "trimmed collision walk differs")]
fn check_mode_catches_a_wrong_reach() {
    let Some(d) = data() else { panic!("trimmed collision walk differs (no data)") };
    let t = Tables::new(&d);
    let low_top = Mix { reach: Aabb::UNIT, ..mix(&t, &d) };
    let mut out = Vec::new();
    for b in walk_boxes(&low_top) {
        collect(&low_top, &b, &mut out);
    }
}

/// A micro-bench skipping the walk check on its own thread leaves it on for a test running beside
/// it: while a second thread holds the skip, a wrong reach still panics here, and not there.
#[test]
fn skipped_walk_check_stays_on_its_thread() {
    let Some(d) = data() else { return };
    let t = Tables::new(&d);
    let low_top = Mix { reach: Aabb::UNIT, ..mix(&t, &d) };
    let boxes = walk_boxes(&low_top);
    let a_walk_panics = || {
        boxes.iter().any(|b| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| collect(&low_top, b, &mut Vec::new()))).is_err()
        })
    };
    let (skip_held, checked_here) = (std::sync::Barrier::new(2), std::sync::Barrier::new(2));
    std::thread::scope(|sc| {
        let bench = sc.spawn(|| {
            let _check_skipped = crate::collide::skip_walk_check_on_this_thread();
            skip_held.wait();
            checked_here.wait();
            a_walk_panics()
        });
        skip_held.wait();
        // Asserted once both threads are past the barriers, so a failure never leaves one waiting.
        let checked_beside = a_walk_panics();
        checked_here.wait();
        let bench_checked = bench.join().expect("bench thread");
        assert!(checked_beside, "the check must stay on beside a bench");
        // `RUSTCRAFT_COLLISION_WALK_CHECK=1` checks every walk, the bench's included.
        if rc_data::env_str!("RUSTCRAFT_COLLISION_WALK_CHECK") != Some("1") {
            assert!(!bench_checked, "the bench's own walks are not checked");
        }
    });
}

/// Both switches of `resolve` on (the default).
const NEW_RESOLVE: ResolvePath = ResolvePath { walk: NEW_WALK, no_copy: true };

/// `RUSTCRAFT_OPT_RESOLVE_NO_COPY` with the trimmed walk: same movement, bit for bit, as the old
/// path (whole walk, copies), with and without step-up, against blocks, border, entities and
/// moving blocks.
#[test]
fn resolve_without_copies_identical() {
    let Some(d) = data() else { return };
    let t = Tables::new(&d);
    let w = mix(&t, &d);
    let mut r = Rng(0x05E5_017E);
    let (mut stepped, mut by_entity) = (0, 0);
    let no_ents = Mix { ents: Vec::new(), ..mix(&t, &d) };
    for b in walk_boxes(&w) {
        let delta = [r.f() * 1.2 - 0.6, r.f() * 1.2 - 0.8, r.f() * 1.2 - 0.6];
        for (on_ground, step) in [(true, 0.6), (false, 0.6), (true, 0.0), (true, 1.0)] {
            let old = resolve_with(&w, &b, delta, on_ground, step, ResolvePath::OLD);
            let new = resolve_with(&w, &b, delta, on_ground, step, NEW_RESOLVE);
            assert_eq!(new.map(f64::to_bits), old.map(f64::to_bits), "box {b:?}, delta {delta:?}, step {step}");
            stepped += usize::from(step > 0.0 && old != resolve_with(&w, &b, delta, on_ground, 0.0, ResolvePath::OLD));
            by_entity += usize::from(old != resolve_with(&no_ents, &b, delta, on_ground, step, ResolvePath::OLD));
        }
    }
    assert!(stepped > 300 && by_entity > 50, "coverage: {stepped} step-ups, {by_entity} movements changed by an entity");
}

/// Time per call of `f` (ns), on `threads` threads that each make `n` calls, the walk check skipped
/// on them (micro-benches only).
fn per_call(threads: usize, n: usize, f: impl Fn(usize) -> u64 + Sync) -> f64 {
    let f = &f;
    let t0 = std::time::Instant::now();
    let sink: u64 = std::thread::scope(|sc| {
        let measured = move |k: usize| {
            let _check_skipped = crate::collide::skip_walk_check_on_this_thread();
            (0..n).map(|i| f(i + k)).fold(0u64, u64::wrapping_add)
        };
        let hs: Vec<_> = (0..threads).map(|k| sc.spawn(move || measured(k))).collect();
        hs.into_iter().map(|h| h.join().expect("thread")).fold(0, u64::wrapping_add)
    });
    std::hint::black_box(sink);
    t0.elapsed().as_nanos() as f64 / n as f64
}

fn thread_counts() -> Vec<usize> {
    let max = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut v: Vec<usize> = std::iter::successors(Some(1usize), |&k| (k * 2 <= max).then_some(k * 2)).collect();
    if *v.last().expect("one") != max {
        v.push(max);
    }
    v
}

#[test]
#[ignore = "micro-bench"]
fn bench_collision_top() {
    let Some(d) = data() else { return };
    let t = Tables::new(&d);
    let n = d.states.len();
    let ids: Vec<u32> = (0..1_000_000u64).map(|i| (Rng(i).next() % n as u64) as u32).collect();
    let bits = |o: Option<f64>| o.map_or(7, f64::to_bits);
    for threads in thread_counts() {
        let old = per_call(threads, ids.len(), |i| bits(top_old(&t, ids[i % ids.len()])));
        let slice = per_call(threads, ids.len(), |i| bits(t.collision_top(ids[i % ids.len()])));
        let table = per_call(threads, ids.len(), |i| bits(t.collision_top_table(ids[i % ids.len()])));
        println!(
            "collision_top, {threads} thread(s), ns per call and per thread: old {old:.2}, borrowed shape {slice:.2}, table {table:.2}"
        );
    }
}

#[test]
#[ignore = "micro-bench"]
fn bench_is_free() {
    let Some(d) = data() else { return };
    let t = Tables::new(&d);
    let _check_skipped = crate::collide::skip_walk_check_on_this_thread();
    let w = mix(&t, &d);
    let bs = boxes(100_000, &w);
    let paths = [IsFreePath::CollectAll, IsFreePath::FirstOverlap, IsFreePath::FirstOverlapSharedBuffer];
    for threads in thread_counts() {
        let r: Vec<f64> =
            paths.iter().map(|&p| per_call(threads, bs.len(), |i| u64::from(is_free_with(&w, &bs[i % bs.len()], p)))).collect();
        println!(
            "is_free, {threads} thread(s), ns per call and per thread: level 0 {:.1}, level 1 {:.1}, level 2 {:.1}",
            r[0], r[1], r[2]
        );
    }
    // Collection (thread buffer at level 2; same walk code) against the old loop.
    let mut out = Vec::new();
    let t0 = std::time::Instant::now();
    for b in &bs {
        collect(&w, b, &mut out);
    }
    let new = t0.elapsed().as_nanos() as f64 / bs.len() as f64;
    let t0 = std::time::Instant::now();
    for b in &bs {
        collect_old(&w, b, &mut out, false);
    }
    let old = t0.elapsed().as_nanos() as f64 / bs.len() as f64;
    println!("collection, 1 thread, ns per call: old {old:.1}, common walk {new:.1}");
}

/// A world that counts the cells the walk reads (`collision` and `moving_collision` calls).
struct Counted<'a> {
    w: &'a dyn World,
    reads: std::cell::Cell<u64>,
}

impl World for Counted<'_> {
    fn state(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        self.w.state(x, y, z)
    }
    fn collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) -> bool {
        self.reads.set(self.reads.get() + 1);
        self.w.collision(x, y, z, out)
    }
    fn moving_collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) -> bool {
        self.reads.set(self.reads.get() + 1);
        self.w.moving_collision(x, y, z, out)
    }
    fn border(&self) -> Option<&border::WorldBorder> {
        self.w.border()
    }
    fn entity_colliders(&self, area: &Aabb, out: &mut Vec<Aabb>) {
        self.w.entity_colliders(area, out);
    }
    fn shape_reach(&self, lo: [i32; 3], hi: [i32; 3]) -> Aabb {
        self.w.shape_reach(lo, hi)
    }
}

/// Plain ground: stone under y = 64, then one cell in ten a fence and one in ten a bottom slab on
/// the surface row, air above.
struct Ground<'a> {
    t: &'a Tables,
    stone: u32,
    fence: u32,
    slab: u32,
}

impl World for Ground<'_> {
    fn state(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        let h = Rng((x as u64) ^ ((z as u64) << 32)).next() % 10;
        Some(match y {
            ..64 => self.stone,
            64 if h == 0 => self.fence,
            64 if h == 1 => self.slab,
            _ => 0,
        })
    }
    fn collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) -> bool {
        self.state(x, y, z).is_some_and(|s| self.t.collision(s, out))
    }
    fn shape_reach(&self, _: [i32; 3], _: [i32; 3]) -> Aabb {
        self.t.shape_reach()
    }
}

fn ground<'a>(t: &'a Tables, d: &VersionData) -> Ground<'a> {
    let state = |name: &str| d.blocks.iter().find(|b| b.name == name).map(|b| b.default_state).expect(name);
    Ground { t, stone: state("minecraft:stone"), fence: state("minecraft:oak_fence"), slab: state("minecraft:oak_slab") }
}

/// Mob-sized boxes standing on the cell grid or a slab (bottom at .0 or .5) and in the air.
fn mob_boxes(n: usize) -> Vec<Aabb> {
    let mut r = Rng(0xB0D1E5);
    (0..n)
        .map(|i| {
            let bottom = 64.0 + (r.next() % 2) as f64 + [0.0, 0.5, r.f()][i % 3];
            let (x, z) = (r.f() * 50.0 - 25.0, r.f() * 50.0 - 25.0);
            Aabb { min: [x - 0.3, bottom, z - 0.3], max: [x + 0.3, bottom + 1.8, z + 0.3] }
        })
        .collect()
}


/// Walking mob: 0.1 forward in a direction that changes with `i`, gravity.
fn walk_delta(i: usize) -> [f64; 3] {
    [0.1 * ((i % 7) as f64 - 3.0) / 3.0, -0.0784, 0.1 * ((i % 5) as f64 - 2.0) / 2.0]
}

/// Feet of a box.
fn feet(b: &Aabb) -> [f64; 3] {
    [(b.min[0] + b.max[0]) / 2.0, b.min[1], (b.min[2] + b.max[2]) / 2.0]
}

/// Paths of `resolve` timed: the old one, each lead alone, both.
const BENCH_PATHS: [(&str, ResolvePath); 6] = [
    ("old", ResolvePath::OLD),
    ("trim", ResolvePath { walk: WalkPath { trim: true, batch: false }, no_copy: false }),
    ("batch", ResolvePath { walk: WalkPath { trim: false, batch: true }, no_copy: false }),
    ("no copy", ResolvePath { walk: WalkPath::WHOLE, no_copy: true }),
    ("trim + no copy", ResolvePath { walk: WalkPath { trim: true, batch: false }, no_copy: true }),
    ("all", NEW_RESOLVE),
];

/// `RUSTCRAFT_OPT_COLLISION_SCAN_TRIM` and `RUSTCRAFT_OPT_RESOLVE_NO_COPY`: cells read per call and
/// time of `support_of` and `resolve` (walking mob, step 0.6) on the Mix world and on plain ground,
/// old against each lead alone and combined; 1 thread then powers of 2, two alternating passes.
#[test]
#[ignore = "micro-bench"]
fn bench_resolve_and_support() {
    let Some(d) = data() else { return };
    let _check_skipped = crate::collide::skip_walk_check_on_this_thread();
    let t = Tables::new(&d);
    let mixed = Mix { movers: Vec::new(), ..mix(&t, &d) };
    let plain = ground(&t, &d);
    let bs = mob_boxes(100_000);
    print_cells_read("mixed", &mixed, &bs);
    print_cells_read("ground", &plain, &bs);
    for pass in 0..2 {
        time_paths("mixed", pass, &mixed, &bs);
        time_paths("ground", pass, &plain, &bs);
    }
}

fn print_cells_read(world: &str, w: &dyn World, bs: &[Aabb]) {
    for trim in [false, true] {
        let counted = Counted { w, reads: std::cell::Cell::new(0) };
        for b in bs {
            support_of_with(&counted, b, feet(b), WalkPath { trim, batch: false });
        }
        let support = counted.reads.replace(0) as f64 / bs.len() as f64;
        for (i, b) in bs.iter().enumerate() {
            resolve_with(&counted, b, walk_delta(i), true, 0.6, ResolvePath { walk: WalkPath { trim, batch: false }, no_copy: true });
        }
        let resolve = counted.reads.get() as f64 / bs.len() as f64;
        println!("{world}: cells read per call, trim {trim}: support_of {support:.1}, resolve {resolve:.1}");
    }
}

fn time_paths<W: World + Sync>(world: &str, pass: usize, w: &W, bs: &[Aabb]) {
    let at = |i: usize| &bs[i % bs.len()];
    let cell_bits = |o: Option<[i32; 3]>| o.map_or(7, |p| (p[0] as u64) ^ ((p[1] as u64) << 21) ^ ((p[2] as u64) << 42));
    for threads in thread_counts() {
        let support: Vec<f64> = [false, true]
            .iter()
            .map(|&new_path| {
                let path = if new_path { NEW_WALK } else { WalkPath::WHOLE };
                per_call(threads, bs.len(), |i| cell_bits(support_of_with(w, at(i), feet(at(i)), path)))
            })
            .collect();
        let resolve: Vec<String> = BENCH_PATHS
            .iter()
            .map(|&(name, p)| {
                let ns = per_call(threads, bs.len(), |i| {
                    resolve_with(w, at(i), walk_delta(i), true, 0.6, p).iter().fold(0, |a, v| a ^ v.to_bits())
                });
                format!("{name} {ns:.1}")
            })
            .collect();
        println!(
            "{world}, pass {pass}, {threads} thread(s), ns per call and per thread: support_of old {:.1} new {:.1}; resolve {}",
            support[0],
            support[1],
            resolve.join(", ")
        );
    }
}

// ---------- Support from the boxes a movement kept (`RUSTCRAFT_OPT_SUPPORTSHARE`) ----------

/// Mix world whose sideways-spilling states (piston heads, pitcher crops) are kept east of x = 0 and
/// whose moving blocks are at x = -10: west of x = 0 the walks get the common reach (the kept boxes
/// can answer), east of it the whole reach, and around the moving blocks an unbounded one (the walk
/// answers).
struct SpillsEastOnly<'a>(Mix<'a>);

impl World for SpillsEastOnly<'_> {
    fn state(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        let s = self.0.state(x, y, z)?;
        Some(if x < 0 && self.0.t.shape_extent(s) == ShapeExtent::Spills { 0 } else { s })
    }
    fn collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) -> bool {
        self.0.mover_shape([x, y, z], out) || self.state(x, y, z).is_some_and(|s| self.0.t.collision(s, out))
    }
    fn border(&self) -> Option<&border::WorldBorder> {
        self.0.border()
    }
    fn entity_colliders(&self, area: &Aabb, out: &mut Vec<Aabb>) {
        self.0.entity_colliders(area, out);
    }
    fn moving_collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) -> bool {
        self.0.moving_collision(x, y, z, out)
    }
    fn shape_reach(&self, lo: [i32; 3], hi: [i32; 3]) -> Aabb {
        let walked = |p: &[i32; 3]| (0..3).all(|a| lo[a] <= p[a] && p[a] <= hi[a]);
        if self.0.movers.iter().any(|(p, _)| walked(p)) {
            Aabb::EVERYWHERE
        } else if hi[0] >= 0 {
            self.0.t.shape_reach()
        } else {
            self.0.t.common_shape_reach()
        }
    }
}

fn spills_east_only<'a>(t: &'a Tables, d: &VersionData) -> SpillsEastOnly<'a> {
    let movers = PUSHES.iter().enumerate().map(|(i, &o)| ([-10, 64, 4 * i as i32 - 10], o)).collect();
    SpillsEastOnly(Mix { movers, ..mix(t, d) })
}

/// The box of a mob whose feet are `pos`, `half` wide on each side and `height` tall, rebuilt from
/// the position like a mob's (its edges may round differently from the moved box).
fn box_at(pos: [f64; 3], half: f64, height: f64) -> Aabb {
    Aabb { min: [pos[0] - half, pos[1], pos[2] - half], max: [pos[0] + half, pos[1] + height, pos[2] + half] }
}

/// The support after a movement made by `resolve_keeping`, taken from the kept boxes, is the one
/// the walk finds, for walking and falling mobs, on and off the ground, with and without step-up,
/// next to sideways-spilling shapes and moving blocks; the movement itself is `resolve`'s, bit for
/// bit.
#[test]
fn support_from_kept_boxes_identical_to_the_walk() {
    let Some(d) = data() else { return };
    let t = Tables::new(&d);
    let w = spills_east_only(&t, &d);
    let mut r = Rng(0xC6_0005);
    let mut kept = collide::CollectedBlocks::default();
    let (mut shared, mut shared_found, mut walked, mut near_movers) = (0, 0, 0, 0);
    for i in 0..40_000 {
        let half = [0.3, 0.45, 0.7][(i / 3) % 3];
        let height = [0.9, 1.4, 1.95][(i / 3) % 3];
        let near = i % 10 == 0;
        let (x, z) = if near { (-10.0 + r.f() * 3.0 - 1.5, 4.0 * (i % 6) as f64 - 10.0 + r.f() * 2.0 - 1.0) } else { (r.f() * 50.0 - 25.0, r.f() * 50.0 - 25.0) };
        let bottom = 64.0 + (r.next() % 3) as f64 + [0.0, 0.5, 0.5625, r.f()][i % 4];
        let feet = [x, bottom, z];
        let start = box_at(feet, half, height);
        // Idle (gravity only: the moved box is the kept zone's own), walking, or thrown.
        let delta = match i % 3 {
            0 => [0.0, -0.0784, 0.0],
            1 => walk_delta(i),
            _ => [r.f() * 0.6 - 0.3, -r.f() * 1.2, r.f() * 0.6 - 0.3],
        };
        let (on_ground, step) = [(true, 0.6), (false, 0.6), (true, 0.0), (true, 1.0)][i % 4];
        let allowed = collide::resolve_keeping(&w, &start, delta, on_ground, step, &mut kept);
        let plain = collide::resolve(&w, &start, delta, on_ground, step);
        assert_eq!(allowed.map(f64::to_bits), plain.map(f64::to_bits), "movement of {start:?} by {delta:?}");
        // A mob searches its support only once it has landed (blocked downwards).
        if !(delta[1] < 0.0 && allowed[1] != delta[1]) {
            continue;
        }
        let pos = [feet[0] + allowed[0], feet[1] + allowed[1], feet[2] + allowed[2]];
        let moved = box_at(pos, half, height);
        let walk = support_of(&w, &moved, pos);
        assert_eq!(support_of_kept(&w, &kept, &moved, pos), walk, "support of {moved:?} after {start:?} by {delta:?}");
        if kept.answers_support_of(&moved) {
            shared += 1;
            shared_found += usize::from(walk.is_some());
        } else {
            walked += 1;
            near_movers += usize::from(near);
        }
    }
    eprintln!("kept support: {shared} landings from the kept boxes ({shared_found} found), {walked} walked ({near_movers} near moving blocks)");
    assert!(
        shared > 3_000 && shared_found > 2_500 && walked > 5_000 && near_movers > 1_000,
        "coverage: {shared} from the kept boxes ({shared_found} found), {walked} walked ({near_movers} near moving blocks)"
    );
}

/// One cell `at` with the shape `shape`, the rest air: a shape that breaks the assumptions of the
/// kept support search (a 26.3 shape never spills on two axes nor rises two cells).
struct LoneShape {
    at: [i32; 3],
    shape: Aabb,
}

impl World for LoneShape {
    fn state(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        Some(u32::from([x, y, z] == self.at))
    }
    fn collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) -> bool {
        if [x, y, z] == self.at {
            out.push(self.shape);
            return true;
        }
        false
    }
    fn shape_reach(&self, _: [i32; 3], _: [i32; 3]) -> Aabb {
        Aabb::UNIT.union(&self.shape)
    }
}

/// The support of `feet` (mob 0.6 wide, 1.8 tall) from the boxes kept by a movement of the same box
/// by `delta`, against the walk.
fn kept_and_walked(w: &dyn World, feet: [f64; 3], delta: [f64; 3]) -> (Option<[i32; 3]>, Option<[i32; 3]>) {
    let b = box_at(feet, 0.3, 1.8);
    let mut kept = collide::CollectedBlocks::default();
    collide::resolve_keeping(w, &b, delta, false, 0.0, &mut kept);
    (support_of_kept(w, &kept, &b, feet), support_of(w, &b, feet))
}

/// Sabotage targets of the kept support search, each a box the walk skips but the kept collection
/// holds: a shape spilling sideways and upwards from a cell on two edges of the support walk (the
/// sideways-spill test lets the walk answer), and a column 2.6 high rising from a cell under the
/// support walk's cells (the cell filter drops it).
#[test]
fn kept_support_search_skips_what_the_walk_skips() {
    let diagonal = LoneShape { at: [0, 63, 0], shape: Aabb { min: [0.0; 3], max: [1.6, 1.6, 1.0] } };
    let (kept, walk) = kept_and_walked(&diagonal, [1.6, 64.5, 0.5], [-0.5, -1.0, 0.0]);
    assert_eq!(walk, None, "the walk skips a cell on two of its edges");
    assert_eq!(kept, walk, "a shape spilling on two axes");
    let tall = LoneShape { at: [0, 62, 0], shape: Aabb { min: [0.0; 3], max: [1.0, 2.6, 1.0] } };
    let (kept, walk) = kept_and_walked(&tall, [0.5, 64.5, 0.5], [0.0, -1.5, 0.0]);
    assert_eq!(walk, None, "the walk does not read two cells under the feet");
    assert_eq!(kept, walk, "a column rising from a cell the walk does not read");
}
