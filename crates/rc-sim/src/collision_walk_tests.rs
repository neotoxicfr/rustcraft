//! Proofs of the server's collision walk on a real world: the walk trimmed to the reach of the
//! shapes (`RUSTCRAFT_OPT_COLLISION_SCAN_TRIM`) and the reach read from the section palettes
//! (`RUSTCRAFT_OPT_COLLISION_PALETTE_PREFILTER`) keep every box of the whole walk, around fences,
//! walls, slabs, piston heads, pitcher crops, a palette entry no block uses any more and blocks
//! moved by pistons in the six directions. Micro-bench ignored by default (`--ignored`).

use rc_physics::{Aabb, World, collide};

use crate::blocks::flag;
use crate::blocks::level::Mover;
use crate::blocks::probe::Probe;
use crate::play::SimWorld;

/// Fixed randomness (splitmix64).
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

/// Blocks whose every state is scattered over the test area: shapes rising above their cell,
/// spilling out of it sideways or below, partial, full, empty, and the player-specific ones.
const SCATTERED: [&str; 10] = [
    "minecraft:oak_fence",
    "minecraft:cobblestone_wall",
    "minecraft:oak_fence_gate",
    "minecraft:oak_slab",
    "minecraft:piston_head",
    "minecraft:pitcher_crop",
    "minecraft:scaffolding",
    "minecraft:powder_snow",
    "minecraft:stone",
    "minecraft:short_grass",
];

/// Area of the scattered blocks: x and z 0..32 (four columns), y −60..−55 above the flat ground.
const AREA_MIN: [i32; 3] = [0, -60, 0];
const AREA_MAX: [i32; 3] = [31, -55, 31];

/// Cells of the moving blocks, one per push direction, along x = 40.
fn mover_cells() -> [[i32; 3]; 6] {
    std::array::from_fn(|i| [40, -58, 3 * i as i32])
}

fn state_named(p: &Probe, name: &str) -> u32 {
    p.s.data.blocks.iter().find(|b| b.name == name).map(|b| b.default_state).expect(name)
}

/// Flat world with the scattered blocks, a stale palette entry and six moving blocks.
fn prepared(tag: &str) -> (Probe, std::path::PathBuf) {
    let (mut p, dir) = crate::tests_g3f13::open(tag);
    let mut states: Vec<u32> = Vec::new();
    for name in SCATTERED {
        let b = p.s.data.blocks.iter().find(|b| b.name == name).expect(name);
        states.extend(b.first_state..b.first_state + b.state_count);
    }
    let mut r = Rng(0xC5_13);
    for x in AREA_MIN[0]..=AREA_MAX[0] {
        for y in AREA_MIN[1]..=AREA_MAX[1] {
            for z in AREA_MIN[2]..=AREA_MAX[2] {
                if r.next().is_multiple_of(3) {
                    let s = states[(r.next() % states.len() as u64) as usize];
                    p.s.set_block([x, y, z], s, flag::KNOWN_SHAPE);
                }
            }
        }
    }
    // A piston head placed then removed high up: its section keeps it in its palette.
    let head = state_named(&p, "minecraft:piston_head");
    p.s.set_block([20, -30, 20], head, flag::KNOWN_SHAPE);
    p.s.set_block([20, -30, 20], 0, flag::KNOWN_SHAPE);
    let (stone, moving) = (state_named(&p, "minecraft:stone"), state_named(&p, "minecraft:moving_piston"));
    use crate::blocks::Dir::*;
    for (i, (cell, dir)) in mover_cells().into_iter().zip([Down, Up, North, South, West, East]).enumerate() {
        p.s.set_block(cell, moving, flag::KNOWN_SHAPE);
        let mover = Mover {
            id: i as u64,
            moved: stone,
            dir,
            extending: true,
            source: false,
            progress: 0.5,
            progress_o: 0.5,
            last_ticked: 0,
        };
        p.s.bs.movers.insert(cell, mover);
    }
    (p, dir)
}

/// Test boxes: over the scattered blocks, around the moving blocks, by the stale palette entry
/// and in the empty sky.
fn test_boxes() -> Vec<Aabb> {
    let mut r = Rng(0xB0_C5);
    let mut around = |c: [f64; 3], spread: [f64; 3], n: usize| -> Vec<Aabb> {
        (0..n)
            .map(|_| {
                let size = [0.1 + r.f() * 1.4, 0.1 + r.f() * 1.9, 0.1 + r.f() * 1.4];
                let min: [f64; 3] = std::array::from_fn(|a| c[a] + (r.f() - 0.5) * spread[a]);
                Aabb { min, max: std::array::from_fn(|a| min[a] + size[a]) }
            })
            .collect()
    };
    let mut boxes = around([16.0, -57.0, 16.0], [34.0, 8.0, 34.0], 6_000);
    for c in mover_cells() {
        boxes.extend(around(c.map(f64::from), [3.0; 3], 200));
    }
    boxes.extend(around([20.5, -30.0, 20.5], [4.0; 3], 200));
    boxes.extend(around([16.0, 20.0, 16.0], [30.0; 3], 300));
    // Plain flat ground, away from the scattered blocks and the moving ones.
    boxes.extend(around([56.0, -60.0, 8.0], [12.0, 3.0, 12.0], 300));
    boxes
}

fn world_with_prefilter(p: &Probe, on: bool) -> SimWorld<'_> {
    SimWorld { palette_prefilter: on, ..p.s.world() }
}

/// A full cube half a block off the grid: the stone a piston moves.
fn moved_stone(b: &Aabb) -> bool {
    (0..3).all(|a| b.max[a] - b.min[a] == 1.0) && b.min.iter().any(|v| v.fract().abs() == 0.5)
}

/// Cells the walk starts from for `b` (one-cell margin), as `collide` computes them.
fn walked_cells(b: &Aabb) -> ([i32; 3], [i32; 3]) {
    (b.min.map(|v| (v - collide::EPS).floor() as i32 - 1), b.max.map(|v| (v + collide::EPS).floor() as i32 + 1))
}

#[test]
fn sim_world_walk_keeps_every_box() {
    let (p, dir) = prepared("walk-keeps");
    let stale = p.s.chunks[&(1, 1)].as_ref().expect("column").sections[2].blocks.may_contain(|s| s != 0);
    assert!(stale, "the removed piston head stays in its section's palette");
    let (mut found, mut moving, mut nothing, mut common) = (0, 0, 0, 0);
    let (mut a, mut whole) = (Vec::new(), Vec::new());
    for on in [false, true] {
        let w = world_with_prefilter(&p, on);
        for b in test_boxes() {
            collide::collect(&w, &b, &mut a);
            collide::collect_untrimmed(&w, &b, &mut whole);
            assert_eq!(a, whole, "box {b:?}, prefilter {on}");
            found += whole.len();
            moving += whole.iter().filter(|bx| moved_stone(bx)).count();
            let (lo, hi) = walked_cells(&b);
            let reach = w.shape_reach(lo, hi);
            nothing += usize::from(on && reach == Aabb::NOTHING);
            common += usize::from(on && reach == p.s.phys.common_shape_reach());
        }
    }
    assert!(
        found > 20_000 && moving > 100 && nothing > 200 && common > 100,
        "coverage: {found} boxes, {moving} moved stones, {nothing} walks with nothing to reach, {common} with the common reach"
    );
    crate::tests_g3f13::finish(p, dir);
}

/// Mob-sized boxes standing on the flat ground (bottom at −60) away from the scattered blocks,
/// walking 0.1 with gravity.
fn grazing_mobs(n: usize) -> Vec<(Aabb, [f64; 3])> {
    let mut r = Rng(0x6_4A2E);
    (0..n)
        .map(|i| {
            let (x, z) = (40.0 + r.f() * 100.0, -30.0 + r.f() * 70.0);
            let b = Aabb { min: [x - 0.45, -60.0, z - 0.45], max: [x + 0.45, -58.6, z + 0.45] };
            let a = i as f64 * 0.7;
            (b, [0.1 * a.cos(), -0.0784, 0.1 * a.sin()])
        })
        .collect()
}

/// `RUSTCRAFT_OPT_COLLISION_PALETTE_PREFILTER`: time of `support_of` and `resolve` on the server
/// world, prefilter off then on, for mobs grazing on flat ground and for boxes among the
/// scattered blocks (piston heads there: whole reach); 1 thread, two alternating passes.
#[test]
#[ignore = "micro-bench"]
fn bench_palette_prefilter() {
    let (p, dir) = prepared("walk-bench");
    let grazing = grazing_mobs(50_000);
    let scattered: Vec<(Aabb, [f64; 3])> = test_boxes().into_iter().map(|b| (b, [0.1, -0.0784, 0.05])).collect();
    let feet = |b: &Aabb| [(b.min[0] + b.max[0]) / 2.0, b.min[1], (b.min[2] + b.max[2]) / 2.0];
    for pass in 0..2 {
        for (name, cases) in [("grazing", &grazing), ("scattered", &scattered)] {
            for on in [false, true] {
                let w = world_with_prefilter(&p, on);
                let t0 = std::time::Instant::now();
                let mut sink = 0usize;
                for (b, _) in cases.iter() {
                    sink += usize::from(rc_physics::support_of(&w, b, feet(b)).is_some());
                }
                let support = t0.elapsed().as_nanos() as f64 / cases.len() as f64;
                let t0 = std::time::Instant::now();
                for (b, d) in cases.iter() {
                    sink += collide::resolve(&w, b, *d, true, 0.6)[1].to_bits() as usize & 1;
                }
                let resolve = t0.elapsed().as_nanos() as f64 / cases.len() as f64;
                std::hint::black_box(sink);
                println!("[bench] palette_prefilter {name} pass {pass} prefilter {on}: support_of {support:.1} ns, resolve {resolve:.1} ns");
            }
        }
    }
    crate::tests_g3f13::finish(p, dir);
}

/// `RUSTCRAFT_OPT_COLLISION_BATCH_READ`: the states of a span, read once per column, are the ones
/// `state` gives cell by cell, across columns, unloaded ones (x ≥ 160) and the world's height.
#[test]
fn sim_world_states_in_matches_state() {
    let (p, dir) = prepared("walk-states");
    let w = p.s.world();
    let spans = [
        ([-3, -61, -3], [18, -55, 2]),
        ([150, -62, 10], [170, -58, 20]),
        ([30, 315, 30], [33, 325, 33]),
        ([0, -70, 0], [2, -60, 2]),
        ([40, -58, 0], [40, -58, 0]),
    ];
    let mut cells = 0;
    for (first, last) in spans {
        let n: usize = (0..3).map(|a| (last[a] - first[a] + 1) as usize).product();
        let mut out = vec![0; n];
        assert!(w.states_in(first, last, &mut out));
        let mut i = 0;
        for y in first[1]..=last[1] {
            for z in first[2]..=last[2] {
                for x in first[0]..=last[0] {
                    assert_eq!(out[i], w.state(x, y, z).unwrap_or(rc_physics::UNLOADED), "({x}, {y}, {z})");
                    i += 1;
                }
            }
        }
        cells += n;
    }
    assert!(cells > 2000);
    crate::tests_g3f13::finish(p, dir);
}

/// A world announcing a reach narrower than its shapes.
struct Narrowed<'a> {
    w: SimWorld<'a>,
    reach: Aabb,
}

impl World for Narrowed<'_> {
    fn state(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        self.w.state(x, y, z)
    }
    fn collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) -> bool {
        self.w.collision(x, y, z, out)
    }
    fn moving_collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) -> bool {
        self.w.moving_collision(x, y, z, out)
    }
    fn shape_reach(&self, _: [i32; 3], _: [i32; 3]) -> Aabb {
        self.reach
    }
}

/// Sabotages, each caught by the proof above: the common reach everywhere (a palette check that
/// would miss the piston heads and pitcher crops), the tables' reach where a block moves.
#[test]
fn a_narrowed_reach_loses_boxes() {
    let (p, dir) = prepared("walk-narrowed");
    // Under `RUSTCRAFT_COLLISION_WALK_CHECK=1` the walk itself panics on the first difference.
    let differs = |reach: Aabb| {
        let narrowed = Narrowed { w: p.s.world(), reach };
        let (mut a, mut whole) = (Vec::new(), Vec::new());
        test_boxes().iter().any(|b| {
            let checked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| collide::collect(&narrowed, b, &mut a)));
            collide::collect_untrimmed(&narrowed, b, &mut whole);
            checked.is_err() || a != whole
        })
    };
    assert!(differs(p.s.phys.common_shape_reach()), "spilling shapes must need the whole reach");
    assert!(differs(p.s.phys.shape_reach()), "moving blocks must need the unbounded reach");
    assert!(!differs(Aabb::EVERYWHERE));
    crate::tests_g3f13::finish(p, dir);
}
