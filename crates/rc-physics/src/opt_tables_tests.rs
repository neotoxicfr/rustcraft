//! Proofs of mission C6-M02 on the game's real data: fence gate recognised by its kind
//! (fidelity fix), borrowed shape and precomputed top (`RUSTCRAFT_OPT_SHAPEREF`),
//! `is_free` without collection (`RUSTCRAFT_OPT_ISFREE`), collection through the common walk. Micro-benches
//! ignored by default (`--ignored`, 1 thread then all threads).

use super::*;
use crate::collide::{EPS, collect, collect_ordered, is_free_with};

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
}

impl World for Mix<'_> {
    fn state(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        let h = Rng((x as u64) ^ ((y as u64) << 21) ^ ((z as u64) << 42)).next();
        Some(if !h.is_multiple_of(5) { 0 } else { ((h >> 8) % self.n) as u32 })
    }
    fn collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) -> bool {
        self.state(x, y, z).is_some_and(|s| self.t.collision(s, out))
    }
    fn border(&self) -> Option<&border::WorldBorder> {
        self.border.as_ref()
    }
    fn entity_colliders(&self, area: &Aabb, out: &mut Vec<Aabb>) {
        entity_boxes_in(&self.ents, area, out);
    }
}

fn mix<'a>(t: &'a Tables, d: &VersionData) -> Mix<'a> {
    // Boat (1.375 × 0.5625) and shulker box (cube) placed near the origin; border of
    // 40 blocks centred at (0, 0).
    let boat = Aabb { min: [2.3125, 64.0, 2.3125], max: [3.6875, 64.5625, 3.6875] };
    let shulker = Aabb { min: [-4.0, 64.0, -4.0], max: [-3.0, 65.0, -3.0] };
    let mut b = border::WorldBorder::default();
    b.set_size(40.0);
    Mix { t, n: d.states.len() as u64, ents: vec![boat, shulker], border: Some(b) }
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
        let old = is_free_with(&w, &b, 0);
        assert_eq!(is_free_with(&w, &b, 1), old, "level 1, box {b:?}");
        assert_eq!(is_free_with(&w, &b, 2), old, "level 2, box {b:?}");
        free += usize::from(old);
        let no_ent = Mix { ents: Vec::new(), ..mix(&t, &d) };
        if !old && is_free_with(&no_ent, &b, 0) {
            by_ent += 1;
        }
        let no_border = Mix { border: None, ..mix(&t, &d) };
        if !old && is_free_with(&no_border, &b, 0) {
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

/// Time per call of `f` (ns), on `threads` threads that each make `n` calls.
fn per_call(threads: usize, n: usize, f: impl Fn(usize) -> u64 + Sync) -> f64 {
    let f = &f;
    let t0 = std::time::Instant::now();
    let sink: u64 = std::thread::scope(|sc| {
        let hs: Vec<_> = (0..threads).map(|k| sc.spawn(move || (0..n).map(|i| f(i + k)).fold(0u64, u64::wrapping_add))).collect();
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
    let w = mix(&t, &d);
    let bs = boxes(100_000, &w);
    for threads in thread_counts() {
        let r: Vec<f64> =
            (0..3u8).map(|m| per_call(threads, bs.len(), |i| u64::from(is_free_with(&w, &bs[i % bs.len()], m)))).collect();
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
