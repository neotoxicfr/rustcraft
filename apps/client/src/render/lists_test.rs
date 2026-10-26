//! Proofs of equivalence of the draw lists (`RUSTCRAFT_OPT_VISORDER`, `RUSTCRAFT_OPT_ARGSKIP`,
//! `RUSTCRAFT_OPT_REACHGRID`) and micro-bench of the construction.

use super::*;
use std::sync::Arc;

/// Deterministic generator (64-bit LCG, high bits).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }

    fn below(&mut self, n: u32) -> u32 {
        self.next() % n
    }
}

/// Fixed seed of the trials (no dependence on the clock).
const SEED: u64 = 0x5eed_c306;
const SECTIONS: i32 = 24;
const MIN_Y: i32 = -64;

/// Test column: random faces, fluids and translucents per layer, group and section,
/// absolute instances contiguous within their region (`cursor`), like the real upload.
fn column(rng: &mut Lcg, k: (i32, i32), region_div: i32, cursor: &mut HashMap<(i32, i32), u32>, trans: &mut u32) -> ColumnGpu {
    let region = (k.0.div_euclid(region_div), k.1.div_euclid(region_div));
    let s1 = SECTIONS as usize + 1;
    let cum = |rng: &mut Lcg, n: usize, at: &mut u32, empty: bool| {
        let mut v = Vec::with_capacity(n);
        for i in 0..n {
            v.push(*at);
            // Last entry of a group = end (no faces after).
            if i % s1 != s1 - 1 && !empty && rng.below(3) != 0 {
                *at += rng.below(6);
            }
        }
        v
    };
    let at = cursor.entry(region).or_insert(0);
    let mut starts: [Vec<u32>; 3] = Default::default();
    let mut ranges: [Range<u32>; 3] = Default::default();
    for layer in 0..3 {
        let a0 = *at;
        let empty = rng.below(4) == 0;
        starts[layer] = cum(rng, BUCKETS * s1, at, empty);
        ranges[layer] = a0..*at;
    }
    let mut fluid_starts: [Vec<u32>; 3] = Default::default();
    let mut fluid_ranges: [Range<u32>; 3] = Default::default();
    for layer in 0..2 {
        let a0 = *at;
        let empty = rng.below(2) == 0;
        fluid_starts[layer] = cum(rng, s1, at, empty);
        fluid_ranges[layer] = a0..*at;
    }
    let t0 = *trans;
    let empty = rng.below(3) == 0;
    let order_starts = cum(rng, s1, trans, empty);
    fluid_ranges[2] = t0..*trans;
    fluid_starts[2] = order_starts.clone();
    ColumnGpu {
        digest: 0,
        region,
        face_alloc: (0, 0),
        fluid_alloc: (0, 0),
        trans_alloc: (0, 0),
        ranges,
        fluid_ranges,
        starts,
        fluid_starts,
        vis: Arc::from(Vec::new()),
        trans_centers: Vec::new(),
        trans_starts: Vec::new(),
        order_alloc: (t0, *trans - t0),
        order_starts,
        sorted_at: None,
        povs: Vec::new(),
        version: 0,
        trans_slot: None,
    }
}

/// Loaded columns and reached sections.
type World = (HashMap<(i32, i32), ColumnGpu>, Vec<(i32, i32, i32)>);

/// Test world of radius `r` around (0, 0): loaded columns (a few are missing: the
/// list must ignore them) and reached sections (`density` out of 8), stored like the walk.
fn world(rng: &mut Lcg, r: i32, density: u32) -> World {
    let (mut cols, mut reached) = (HashMap::new(), Vec::new());
    let (mut cursor, mut trans) = (HashMap::new(), 0);
    for x in -r..=r {
        for z in -r..=r {
            if rng.below(16) != 0 {
                cols.insert((x, z), column(rng, (x, z), 4, &mut cursor, &mut trans));
            }
        }
    }
    reach(rng, r, density, &mut reached);
    (cols, reached)
}

fn reach(rng: &mut Lcg, r: i32, density: u32, out: &mut Vec<(i32, i32, i32)>) {
    out.clear();
    for x in -r..=r {
        for z in -r..=r {
            for sy in 0..SECTIONS {
                if rng.below(8) < density {
                    out.push((x, sy, z));
                }
            }
        }
    }
    out.sort_unstable_by_key(|&(x, sy, z)| (x, z, sy));
}

/// Frame of trajectory `f`: eye (including heights at the exact middle of a section and on a
/// boundary), yaw, pitch.
fn camera(f: u32) -> ([f64; 3], f32, f32) {
    let t = f64::from(f);
    let y = match f % 5 {
        // Exact middle of a section: two sections at equal distance on either side.
        0 => f64::from(MIN_Y + 16 * (4 + (f / 5 % 8) as i32) + 8),
        // Boundary between two sections.
        1 => f64::from(MIN_Y + 16 * (5 + (f / 5 % 6) as i32)),
        _ => 70.0 + (t * 0.37).sin() * 30.0,
    };
    // Horizontal position held for 4 frames (rotation in place: the cached order is used); the
    // height does not change the origin.
    let p = f64::from(f / 4);
    let eye = [(p * 0.9).sin() * 40.0 + p * 0.25, y, (p * 0.6).cos() * 40.0 - p * 0.1];
    (eye, ((t * 7.3) % 360.0 - 180.0) as f32, ((t * 1.7).sin() * 80.0) as f32)
}

fn planes(eye: [f64; 3], yaw: f32, pitch: f32, far: f32) -> ([i32; 2], [Vec4; 6]) {
    let (origin, _, view_m) = crate::camera::view_matrix(eye, yaw, pitch);
    (origin, super::super::frustum_planes(super::super::perspective(70.0, 16.0 / 9.0, 0.05, far) * view_m))
}

fn fnv(h: &mut u64, bytes: &[u8]) {
    for &b in bytes {
        *h = (*h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
    }
}

/// Batches and commands of a frame in bytes (exact comparison).
fn image_bytes(l: &Lists, drawn: (usize, u64)) -> Vec<u8> {
    let mut v = Vec::new();
    for &(layer, fluid, (rx, rz), first, count) in &l.batches {
        v.extend_from_slice(&(layer as u32).to_le_bytes());
        v.push(u8::from(fluid));
        for n in [rx, rz] {
            v.extend_from_slice(&n.to_le_bytes());
        }
        v.extend_from_slice(&first.to_le_bytes());
        v.extend_from_slice(&count.to_le_bytes());
    }
    v.extend_from_slice(bytemuck::cast_slice(&l.args));
    v.extend_from_slice(&(drawn.0 as u64).to_le_bytes());
    v.extend_from_slice(&drawn.1.to_le_bytes());
    v
}

/// Trajectory of 600 frames: walk redone every 7 frames, columns changed every
/// 13 (one removed, one put back), advancing origin; batches, commands and count identical frame by
/// frame between the old and the new path, and fingerprint of the old path stored.
#[test]
fn draw_lists_match_the_old_build_along_a_trajectory() {
    let mut rng = Lcg(SEED);
    let (mut cols, mut reached) = world(&mut rng, 12, 5);
    let (mut old, mut new) = (Lists::default(), Lists::default());
    let (mut rgen, mut rev) = (1u64, 1u64);
    let mut digest = 0xcbf2_9ce4_8422_2325u64;
    let mut ties = 0;
    let mut merged = 0;
    let mut hits = 0;
    for f in 0..600u32 {
        if f % 7 == 6 {
            reach(&mut rng, 12, 5, &mut reached);
            rgen += 1;
        }
        if f % 13 == 12 {
            let k = ((rng.below(25) as i32) - 12, (rng.below(25) as i32) - 12);
            if let Some(c) = cols.remove(&k) {
                let k2 = (k.1, k.0);
                cols.entry(k2).or_insert(c);
            }
            rev += 1;
        }
        let (eye, yaw, pitch) = camera(f);
        let (origin, pl) = planes(eye, yaw, pitch, 12.0 * 16.0);
        let inp = ListIn {
            columns: &cols,
            reached: &reached,
            planes: &pl,
            origin,
            eye,
            min_y: MIN_Y,
            sections: SECTIONS,
            face_vertices: 4,
            split_draws: false,
            reached_gen: rgen,
            columns_rev: rev,
        };
        let a = build(&inp, &mut old, false, || {});
        let b = build(&inp, &mut new, true, || {});
        hits += usize::from(new.order_key == Some((rgen, origin, rev)));
        let (ia, ib) = (image_bytes(&old, a), image_bytes(&new, b));
        assert!(ia == ib, "frame {f}: lists differ (eye {eye:?}, yaw {yaw}, pitch {pitch})");
        fnv(&mut digest, &ia);
        ties += usize::from(f % 5 == 0 && old.batches.iter().any(|b| b.0 == 2 && b.1));
        merged += old.args.iter().filter(|a| a.instance_count > 5).count();
    }
    assert!(ties > 0 && merged > 0, "trajectory without ties or merges: it proves nothing");
    assert!(hits > 200 && hits < 550, "frames served by the cached order: {hits} (both paths must serve)");
    // Fingerprint of the old path on this trajectory (fixed trial seed): changes if the
    // construction itself changes.
    assert_eq!(digest, EXPECTED_DIGEST, "trajectory fingerprint: {digest:#018x}");
}

const EXPECTED_DIGEST: u64 = 0x88c4_1e8e_1e14_3620;

/// Sections of a column from farthest to nearest: two-pointer merge = stable sort
/// of the old path, eye at the exact middle of a section and on a boundary included.
#[test]
fn far_to_near_matches_the_stable_sort() {
    let mut rng = Lcg(SEED ^ 1);
    let (mut src, mut got, mut want) = (Vec::new(), Vec::new(), Vec::new());
    let mut ties = 0;
    for case in 0..20_000u32 {
        let mut runs: Vec<Range<usize>> = Vec::new();
        let mut sy = rng.below(3) as usize;
        while sy < SECTIONS as usize && runs.len() < 6 {
            let len = 1 + rng.below(4) as usize;
            let end = (sy + len).min(SECTIONS as usize);
            runs.push(sy..end);
            sy = end + 1 + rng.below(3) as usize;
        }
        let ey = match case % 3 {
            0 => f64::from(16 * rng.below(SECTIONS as u32) + 8),
            1 => f64::from(16 * rng.below(SECTIONS as u32 + 1)),
            _ => f64::from(rng.below(SECTIONS as u32 * 16 * 64)) / 64.0 - 20.0,
        };
        got.clear();
        far_to_near(&runs, ey, &mut src, &mut got);
        want.clear();
        want.extend(runs.iter().flat_map(|r| r.clone()).map(|sy| sy..sy + 1));
        want.sort_by(|a: &Range<usize>, b: &Range<usize>| section_dist(b.start, ey).total_cmp(&section_dist(a.start, ey)));
        assert_eq!(got, want, "runs {runs:?}, eye {ey}");
        ties += usize::from(want.windows(2).any(|w| section_dist(w[0].start, ey) == section_dist(w[1].start, ey)));
    }
    assert!(ties > 100, "too few ties ({ties}) to separate");
}

/// Range to rewrite: applied on the old contents, it gives back exactly the new
/// commands; nothing when they are identical.
#[test]
fn changed_range_rebuilds_the_new_commands() {
    let mut rng = Lcg(SEED ^ 2);
    let arg = |rng: &mut Lcg| DrawArgs {
        vertex_count: 4 + rng.below(2) * 2,
        instance_count: rng.below(4),
        first_vertex: 0,
        first_instance: rng.below(8),
    };
    let mut skipped = 0;
    for _ in 0..20_000 {
        let old: Vec<DrawArgs> = (0..rng.below(12)).map(|_| arg(&mut rng)).collect();
        let mut new = old.clone();
        match rng.below(4) {
            0 => {}
            1 => new.truncate(rng.below(new.len() as u32 + 1) as usize),
            2 => new.extend((0..1 + rng.below(3)).map(|_| arg(&mut rng))),
            _ => {
                if !new.is_empty() {
                    let i = rng.below(new.len() as u32) as usize;
                    new[i] = arg(&mut rng);
                }
            }
        }
        let mut gpu = old.clone();
        match changed_range(&old, &new) {
            None => {
                skipped += 1;
                assert!(bytemuck::cast_slice::<_, u8>(&old[..new.len()]) == bytemuck::cast_slice::<_, u8>(&new[..]));
            }
            Some(r) => {
                gpu.resize(gpu.len().max(r.end), DrawArgs { vertex_count: 0, instance_count: 0, first_vertex: 0, first_instance: 0 });
                gpu[r.clone()].copy_from_slice(&new[r]);
            }
        }
        // The GPU reads only the first `new.len()`.
        assert!(
            bytemuck::cast_slice::<_, u8>(&gpu[..new.len()]) == bytemuck::cast_slice::<_, u8>(&new[..]),
            "old {} commands, new {}",
            old.len(),
            new.len()
        );
    }
    assert!(skipped > 1000, "no-change cases too rare ({skipped})");
}

/// Grid of reached sections = set of the same sections, over the whole box and beyond.
#[test]
fn reach_grid_matches_the_set() {
    let mut rng = Lcg(SEED ^ 3);
    let mut reached = Vec::new();
    for r in [0, 1, 5, 12] {
        for density in [0, 1, 4, 8] {
            reach(&mut rng, r, density, &mut reached);
            // Offset: grid not centred on 0.
            let shift = (rng.below(100) as i32 - 50, rng.below(4) as i32 - 2);
            reached.iter_mut().for_each(|s| (s.0, s.2) = (s.0 + shift.0, s.2 + shift.1));
            let set: hashbrown::HashSet<_> = reached.iter().copied().collect();
            let grid = ReachGrid::new(&reached);
            assert_eq!(grid.is_empty(), set.is_empty());
            let mut hits = 0;
            for x in shift.0 - r - 2..=shift.0 + r + 2 {
                for z in shift.1 - r - 2..=shift.1 + r + 2 {
                    for y in -2..SECTIONS + 2 {
                        assert_eq!(grid.contains(x, y, z), set.contains(&(x, y, z)), "({x}, {y}, {z})");
                        hits += usize::from(set.contains(&(x, y, z)));
                    }
                }
            }
            assert_eq!(hits, set.len());
            for far in [i32::MIN, i32::MAX] {
                assert!(!grid.contains(far, 0, 0) && !grid.contains(0, far, 0) && !grid.contains(0, 0, far));
            }
        }
    }
}

/// Micro-bench (`cargo test --release -- --ignored lists_bench --nocapture`): construction only,
/// rotation in place (fixed walk and origin) then walking (origin changing on every frame),
/// old and new path, at distance 12, 32 and 64; one core.
#[test]
#[ignore]
fn lists_bench() {
    for r in [12, 32, 64] {
        let mut rng = Lcg(SEED);
        let (cols, reached) = world(&mut rng, r, 5);
        for (walk, speed) in [("rotation", 0.0), ("walk", 0.07), ("fast flight", 1.1)] {
            let mut res = [0.0f64; 2];
            for (p, new) in [false, true, false, true].into_iter().enumerate() {
                let mut l = Lists::default();
                let frames = 400u32;
                let t = std::time::Instant::now();
                for f in 0..frames {
                    let s = f64::from(f) * speed;
                    let eye = [0.5 + s, 70.0, 0.5 + s * 0.3];
                    let (origin, pl) = planes(eye, f as f32 * 0.9, 10.0, r as f32 * 16.0);
                    let inp = ListIn {
                        columns: &cols,
                        reached: &reached,
                        planes: &pl,
                        origin,
                        eye,
                        min_y: MIN_Y,
                        sections: SECTIONS,
                        face_vertices: 4,
                        split_draws: false,
                        reached_gen: 1,
                        columns_rev: 1,
                    };
                    std::hint::black_box(build(&inp, &mut l, new, || {}));
                }
                // Two alternating passes; the second counts.
                if p >= 2 {
                    res[usize::from(new)] = t.elapsed().as_secs_f64() * 1e6 / f64::from(frames);
                }
            }
            println!(
                "lists distance {r} {}: old {:.1} µs/frame, new {:.1} µs/frame ({:+.0}%)",
                walk,
                res[0],
                res[1],
                (res[1] / res[0] - 1.0) * 100.0
            );
        }
    }
    // Reached sections: set rebuilt on the main thread against grid.
    for r in [12, 32, 64] {
        let mut rng = Lcg(SEED);
        let mut reached = Vec::new();
        reach(&mut rng, r, 5, &mut reached);
        let n = 20;
        let t = std::time::Instant::now();
        for _ in 0..n {
            std::hint::black_box(reached.iter().copied().collect::<hashbrown::HashSet<_>>());
        }
        let set_us = t.elapsed().as_secs_f64() * 1e6 / f64::from(n);
        let t = std::time::Instant::now();
        for _ in 0..n {
            std::hint::black_box(ReachGrid::new(&reached));
        }
        let grid_us = t.elapsed().as_secs_f64() * 1e6 / f64::from(n);
        let set: hashbrown::HashSet<_> = reached.iter().copied().collect();
        let grid = ReachGrid::new(&reached);
        let q: Vec<(i32, i32, i32)> =
            (0..200_000).map(|_| (rng.below(2 * r as u32 + 1) as i32 - r, rng.below(24) as i32, rng.below(2 * r as u32 + 1) as i32 - r)).collect();
        let t = std::time::Instant::now();
        let a = q.iter().filter(|p| set.contains(*p)).count();
        let set_q = t.elapsed().as_secs_f64() * 1e9 / q.len() as f64;
        let t = std::time::Instant::now();
        let b = q.iter().filter(|p| grid.contains(p.0, p.1, p.2)).count();
        let grid_q = t.elapsed().as_secs_f64() * 1e9 / q.len() as f64;
        assert_eq!(a, b);
        println!(
            "reached distance {r} ({} sections): set construction {set_us:.0} µs, grid {grid_us:.0} µs; query {set_q:.1} ns against {grid_q:.1} ns",
            reached.len()
        );
    }
}
