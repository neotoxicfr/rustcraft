//! Proofs of the mob body work without repeated reads (mission C6-M05, `docs/perf.md`):
//!
//! - the per-state rules the movement now reads from the physics tables (support kept under the
//!   feet, water and bubble column, powder snow, air, bubble column by block index) match the
//!   name and tag rules on every state of 26.3, before and after a tag reload that moves fences
//!   and walls (`RUSTCRAFT_OPT_MOBSTATES`);
//! - the server world keeping its last column (`RUSTCRAFT_OPT_SIMWORLDCURSOR`) reads, walks and
//!   resolves like the hash lookup for a mob astride four chunks, one of them missing;
//! - a crowd of 1,000 mobs on a course of fences, walls, gates, slabs, water, powder snow, magma,
//!   amethyst, farmland and pitcher crops, 400 ticks: positions, velocities, supports, freezing,
//!   health and every sound sent, against a stored fingerprint. Every movement of it (and of the
//!   whole suite) also checks the support taken from the kept boxes against the walk
//!   (`body.rs`). The fingerprint is the same with every switch of the mission at 0.
//! - a mob freezing in powder snow then thawing, against stored values.
//!
//! Micro-benches at the end (ignored).

use std::sync::Arc;

use crate::blocks::probe::Probe;
use crate::mobs::species::Kind;
use crate::play::SimWorld;
use crate::tag_bits::TagId;

pub(super) fn cmd(p: &mut Probe, c: &str) {
    let src = p.s.server_source();
    let (ok, _) = p.s.perform(src, c);
    assert!(ok, "command refused: {c}");
}

pub(super) fn game_ticks(p: &mut Probe, n: i32) {
    for _ in 0..n {
        p.s.game_tick();
        p.s.tick += 1;
    }
}

/// Supporting block kept under the feet at offset `off`, by tags and names (the rule before
/// `RUSTCRAFT_OPT_MOBSTATES`).
fn keeps_support_by_names(s: &crate::server::Server, state: u32, off: f32) -> bool {
    let block = s.data.state(state).map_or(u32::MAX, |i| u32::from(i.block));
    let mode = crate::tag_bits::mode();
    (f64::from(off) <= 0.5 && s.tag_id_u32(TagId::Fences, block, mode))
        || s.tag_id_u32(TagId::Walls, block, mode)
        || s.state_name(state).ends_with("_fence_gate")
}

/// Every state: the table rules equal the name rules. Returns (kept supports, water or bubble,
/// powder snow, air, bubble columns) for the coverage.
fn check_every_state(s: &crate::server::Server) -> [usize; 5] {
    let mut seen = [0usize; 5];
    let bubble = s.bs.t.id.bubble_column;
    for state in 0..s.data.states.len() as u32 {
        let name = s.state_name(state);
        for off in [0.2f32, 0.500_001, 0.6] {
            let by_names = keeps_support_by_names(s, state, off);
            assert_eq!(s.phys.keeps_support(state, off), by_names, "support kept, state {state} ({name}), offset {off}");
            seen[0] += usize::from(by_names);
        }
        let here_only = name == "minecraft:water" || name == "minecraft:bubble_column";
        assert_eq!(s.phys.speed_factor_here_only(state), here_only, "water or bubble column, state {state} ({name})");
        let snow = name == "minecraft:powder_snow";
        assert_eq!(s.phys.is_powder_snow(state), snow, "powder snow, state {state} ({name})");
        let air = matches!(name, "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air");
        assert_eq!(s.phys.is_air(state), air, "air, state {state} ({name})");
        let is_bubble = name == "minecraft:bubble_column";
        assert_eq!(s.view().block(state) == bubble, is_bubble, "bubble column by index, state {state} ({name})");
        seen[1] += usize::from(here_only);
        seen[2] += usize::from(snow);
        seen[3] += usize::from(air);
        seen[4] += usize::from(is_bubble);
    }
    seen
}

/// `RUSTCRAFT_OPT_MOBSTATES`: the rules read from the physics tables are the name and tag rules of
/// every state, and still after a reload that takes blocks out of `fences` and `walls` and puts a
/// stone and a gate in them.
#[test]
fn table_rules_of_every_state_match_the_name_rules() {
    let (mut p, _dir) = crate::tests_g3f13::open("c6m05-states");
    let seen = check_every_state(&p.s);
    assert!(seen[0] > 1000 && seen[1] > 10 && seen[2] == 1 && seen[3] == 3 && seen[4] == 2, "coverage: {seen:?}");
    let mut tags = p.s.regs.tags.clone();
    let blocks = &mut tags.iter_mut().find(|(r, _)| r == "minecraft:block").expect("block tags").1;
    let id = |name: &str| p.s.data.blocks.iter().position(|b| b.name == name).expect(name) as i32;
    for (tag, add) in [("minecraft:fences", "minecraft:stone"), ("minecraft:walls", "minecraft:oak_fence_gate")] {
        let e = blocks.iter_mut().find(|(n, _)| n == tag).expect("tag");
        e.1.drain(..2);
        e.1.push(id(add));
    }
    p.s.set_registry_tags(tags);
    let reloaded = check_every_state(&p.s);
    assert_ne!(reloaded[0], seen[0], "the reload must move kept supports");
}

/// The server world of `p` with the column cursor on or off.
fn sim_world(p: &Probe, cursor: bool) -> SimWorld<'_> {
    SimWorld { column_cursor_on: cursor, ..p.s.world() }
}

/// `RUSTCRAFT_OPT_SIMWORLDCURSOR`: a mob-sized box astride the corner of four chunks, one of them
/// missing, reads every cell, walks, resolves and finds its support like the hash lookup, cells
/// read in a scattered order that jumps between the columns.
#[test]
fn column_cursor_reads_like_the_hash_lookup_across_four_chunks() {
    let (mut p, _dir) = crate::tests_g3f13::open("c6m05-cursor");
    cmd(&mut p, "fill 14 -60 14 17 -59 17 minecraft:oak_fence");
    cmd(&mut p, "setblock 15 -60 16 minecraft:cobblestone_wall");
    cmd(&mut p, "setblock 16 -60 15 minecraft:oak_slab");
    // Column (1, 0) missing.
    assert!(p.s.chunks.remove(&(1, 0)).is_some(), "column to remove is loaded");
    let (on, off) = (sim_world(&p, true), sim_world(&p, false));
    use rc_physics::World;
    let mut cells = Vec::new();
    for y in -66..-55 {
        for z in 13..19 {
            for x in 13..19 {
                cells.push([x, y, z]);
            }
        }
    }
    // Scattered order: every 7th cell, so consecutive reads change column often.
    let n = cells.len();
    let (mut read, mut missing) = (0, 0);
    for i in 0..n {
        let [x, y, z] = cells[(i * 7) % n];
        assert_eq!(on.state(x, y, z), off.state(x, y, z), "state of {x} {y} {z}");
        read += 1;
        missing += usize::from(off.state(x, y, z).is_none());
    }
    assert!(read > 300 && missing > 50, "coverage: {read} cells read, {missing} unloaded");
    let mut out = [0u32; 64];
    let mut out_off = [0u32; 64];
    assert_eq!(on.states_in([13, -62, 13], [16, -59, 16], &mut out), off.states_in([13, -62, 13], [16, -59, 16], &mut out_off));
    assert_eq!(out, out_off, "batched read");
    let mut supports = 0;
    for (i, &(x, z)) in [(15.9, 15.9), (16.1, 16.1), (15.5, 16.4), (16.4, 15.5), (15.0, 15.0)].iter().enumerate() {
        for bottom in [-60.0, -59.5, -58.5, -60.3] {
            let b = rc_physics::Aabb { min: [x - 0.45, bottom, z - 0.45], max: [x + 0.45, bottom + 1.4, z + 0.45] };
            let delta = [0.1 * (i as f64 - 2.0), -0.0784, 0.05];
            let feet = [x, bottom, z];
            for step in [0.0, 0.6] {
                let (a, b2) = (
                    rc_physics::collide::resolve(&on, &b, delta, true, step),
                    rc_physics::collide::resolve(&off, &b, delta, true, step),
                );
                assert_eq!(a.map(f64::to_bits), b2.map(f64::to_bits), "resolve of {b:?}");
            }
            let found = rc_physics::support_of(&on, &b, feet);
            assert_eq!(found, rc_physics::support_of(&off, &b, feet), "support of {b:?}");
            assert_eq!(rc_physics::is_free(&on, &b), rc_physics::is_free(&off, &b), "free {b:?}");
            supports += usize::from(found.is_some());
        }
    }
    assert!(supports > 5, "coverage: {supports} supports found");
}

/// Course of the crowd fingerprint: rows of fences, walls and gates, slabs, stairs, a stone wall,
/// water, powder snow, magma under carpets, soul sand, honey, slime, blue ice, amethyst,
/// farmland with pitcher crops.
pub(super) const COURSE: &[&str] = &[
    "fill 0 -60 0 22 -60 0 minecraft:oak_fence",
    "fill 0 -60 4 22 -60 4 minecraft:cobblestone_wall",
    "fill 0 -60 8 22 -60 8 minecraft:oak_fence_gate",
    "fill 0 -60 9 10 -60 9 minecraft:spruce_fence_gate[open=true]",
    "fill 24 -60 0 30 -60 6 minecraft:oak_slab",
    "fill 24 -60 8 30 -60 9 minecraft:stone_stairs",
    "fill 32 -60 0 32 -59 20 minecraft:stone",
    "fill 34 -61 0 42 -61 8 minecraft:water",
    "fill 0 -61 12 8 -61 20 minecraft:powder_snow",
    "fill 10 -61 12 14 -61 20 minecraft:magma_block",
    "fill 10 -60 12 12 -60 20 minecraft:white_carpet",
    "fill 16 -61 12 18 -61 20 minecraft:soul_sand",
    "fill 20 -61 12 22 -61 20 minecraft:honey_block",
    "fill 24 -61 12 26 -61 20 minecraft:slime_block",
    "fill 28 -61 12 30 -61 20 minecraft:blue_ice",
    "fill 0 -61 22 10 -61 30 minecraft:amethyst_block",
    "fill 12 -61 22 22 -61 30 minecraft:farmland",
    "fill 12 -60 22 16 -60 26 minecraft:pitcher_crop",
];

/// Species of the crowd: grazing animals, climbers, powder snow walkers, monsters, fliers.
pub(super) const CROWD: [Kind; 12] = [
    Kind::Cow,
    Kind::Sheep,
    Kind::Pig,
    Kind::Chicken,
    Kind::Rabbit,
    Kind::Spider,
    Kind::Zombie,
    Kind::Wolf,
    Kind::Goat,
    Kind::Fox,
    Kind::Frog,
    Kind::Skeleton,
];

/// Fingerprint of a crowd of `n` mobs on the course after `ticks` ticks, with the number of sounds
/// heard (SHA-256 of every mob's body state then of every sound packet).
fn crowd_on_course(n: usize, ticks: i32) -> (String, usize) {
    let (mut p, _dir) = crate::tests_g3f13::open("c6m05-crowd");
    cmd(&mut p, crate::tests_opt::PLAYER_TAKES_NO_DAMAGE);
    for c in COURSE {
        cmd(&mut p, c);
    }
    let mut r: u64 = 0xC6_05;
    for i in 0..n {
        r ^= r << 13;
        r ^= r >> 7;
        r ^= r << 17;
        let (fx, fz) = ((r & 0xFFFF) as f64 / 65536.0, ((r >> 16) & 0xFFFF) as f64 / 65536.0);
        let at = [fx * 44.0, -58.0 + ((r >> 32) % 3) as f64, -2.0 + fz * 34.0];
        let mut m = p.s.new_mob(CROWD[i % CROWD.len()], at, ((r >> 40) % 360) as f32);
        m.persistent = true;
        if i % 5 == 0 {
            m.age = -24000;
        }
        p.s.add_mob(m);
    }
    let _ = p.take_events();
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    let mut sounds = 0;
    for _ in 0..ticks {
        game_ticks(&mut p, 1);
        for e in p.take_events() {
            if let rc_client::Event::Sound(s) = e {
                sounds += 1;
                ctx.update(format!("{:?}", s).as_bytes());
            }
        }
    }
    let mut bodies: Vec<String> = p
        .s
        .mobs
        .list
        .iter()
        .map(|m| {
            format!(
                "{} {:?} {:?} {} {:?} {:x} {} {:x} {:x} {:x}",
                m.id,
                m.pos().map(f64::to_bits),
                m.vel.map(f64::to_bits),
                m.on_ground,
                m.support,
                m.fall_distance.to_bits(),
                m.ticks_frozen,
                m.health.to_bits(),
                m.yaw.to_bits(),
                m.attrs.speed.value().to_bits()
            )
        })
        .collect();
    bodies.sort();
    for b in &bodies {
        ctx.update(b.as_bytes());
    }
    let hex: String = ctx.finish().as_ref().iter().map(|b| format!("{b:02x}")).collect();
    (hex, sounds)
}

/// Expected crowd fingerprint (1,000 mobs, 400 ticks) and number of sounds, recorded on the new
/// paths and identical with `RUSTCRAFT_OPT_SUPPORTSHARE`, `SIMWORLDCURSOR`, `MOBSTATES`,
/// `FREEZESHARE`, `FREEZE_SKIP_UNFROZEN`, `DIMFLAGS` and `STEPSOUNDREF` all at 0. Replaced on purpose
/// by L15-ENTITES: the crowd killed the player, who now leaves the world 20 ticks after its death
/// (nothing simulated around it, the rest of the run frozen: "d2fe1e38…6724", 11,504 sounds), so
/// the player takes no damage now; the value is the same with the removal neutralised (checked),
/// and the value before ("9a67e432…6856", 22,846) comes back without the resistance and the removal.
/// Replaced on purpose by the boats work: the push between two mobs now multiplies in the game's
/// order (× pow, then × 0.05), bit for bit ("8fc7bd3e…7e71", 23,116); the former order alone gives
/// back "7e828b5a…5042", 23,143 (checked). Replaced on purpose by L16-RAMASSAGE: a fox that takes
/// one piece of a stack no longer leaves the rest twice (the rest leaves as a new item with a
/// random push, the original is removed); alone on "7e828b5a…5042" it gave "bd798a5f…ae20", 23,113,
/// and with the old fox put back the boats value comes back (checked).
const CROWD_ON_COURSE: (&str, usize) = ("8fc7bd3ecb0e01f51cd88d97accbf3f737060ca83a01e65450418e8151397e71", 23116);

/// 1,000 mobs, 400 ticks on the course: same bodies and sounds as the stored fingerprint, in a
/// fresh thread (the per-thread test seed restarts there).
#[test]
fn crowd_on_course_moves_like_the_stored_fingerprint() {
    let got = std::thread::spawn(|| crowd_on_course(1000, 400)).join().expect("simulation");
    eprintln!("crowd on course: fingerprint {} ({} sounds)", got.0, got.1);
    assert!(got.1 > 1000, "coverage: only {} sounds", got.1);
    assert_eq!(
        (got.0.as_str(), got.1),
        CROWD_ON_COURSE,
        "crowd fingerprint changed: an optimisation must never change it; an intended behaviour fix \
         replaces CROWD_ON_COURSE with ({:?}, {})",
        got.0,
        got.1
    );
}

/// Freezing of a cow walking in a pit of powder snow, then in the emptied pit, tick by tick:
/// (frozen ticks, speed, health).
fn freeze_and_thaw() -> Vec<(i32, u64, u32)> {
    let (mut p, _dir) = crate::tests_g3f13::open("c6m05-freeze");
    cmd(&mut p, "fill 2 -63 2 6 -58 6 minecraft:stone");
    cmd(&mut p, "fill 3 -62 3 5 -59 5 minecraft:powder_snow");
    let mut m = p.s.new_mob(Kind::Cow, [4.5, -61.5, 4.5], 0.0);
    m.persistent = true;
    p.s.add_mob(m);
    let mut trace = Vec::new();
    let body = |p: &Probe| {
        let m = p.s.mobs.list.iter().next().expect("cow");
        (m.ticks_frozen, m.attrs.speed.value().to_bits(), m.health.to_bits())
    };
    for _ in 0..240 {
        game_ticks(&mut p, 1);
        trace.push(body(&p));
    }
    cmd(&mut p, "fill 3 -62 3 5 -59 5 minecraft:air");
    for _ in 0..100 {
        game_ticks(&mut p, 1);
        trace.push(body(&p));
    }
    trace
}

/// The cow freezes up to 140, slows down while on a block, takes freezing damage every 40 ticks,
/// then thaws by 2 per tick out of the snow: stored summary, same on every freezing path.
#[test]
fn mob_freezes_and_thaws_like_before() {
    let trace = std::thread::spawn(freeze_and_thaw).join().expect("simulation");
    let max = trace.iter().map(|t| t.0).max().unwrap_or(0);
    let hurt = trace.windows(2).filter(|w| w[1].2 != w[0].2).count();
    let speeds: std::collections::BTreeSet<u64> = trace.iter().map(|t| t.1).collect();
    assert_eq!(max, crate::inside_blocks::FREEZE_REQUIRED, "frozen to the maximum");
    assert_eq!(trace.last().map(|t| t.0), Some(0), "thawed out of the snow");
    assert!(hurt >= 2 && speeds.len() > 10, "coverage: {hurt} hurts, {} speeds", speeds.len());
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    ctx.update(format!("{trace:?}").as_bytes());
    let hex: String = ctx.finish().as_ref().iter().map(|b| format!("{b:02x}")).collect();
    eprintln!("freeze and thaw: {hex}");
    assert_eq!(hex, FREEZE_AND_THAW, "freezing trace changed");
}

/// Expected freezing trace (SHA-256), recorded on the new paths, identical with the freezing
/// switches at 0.
const FREEZE_AND_THAW: &str = "812890f0293ad6e861463c9b1d9ebb3e3a6f4086d8e7402c4b3d15547bee73d0";

/// Time per call (ns) of `f` over `n` calls on one thread.
fn per_call(n: usize, mut f: impl FnMut(usize) -> u64) -> f64 {
    let t0 = std::time::Instant::now();
    let mut sink = 0u64;
    for i in 0..n {
        sink = sink.wrapping_add(f(i));
    }
    std::hint::black_box(sink);
    t0.elapsed().as_nanos() as f64 / n as f64
}

/// Micro-bench of the mission's leads on the course world, one thread (loaded machine figures in
/// `docs/perf.md`): block under the feet (three reads by tags and names against one read and the
/// table), support after a movement (walk against the kept boxes), cell reads of the server world
/// (hash lookup against the column cursor).
#[test]
#[ignore = "micro-bench"]
fn bench_mob_body_reads() {
    let (mut p, _dir) = crate::tests_g3f13::open("c6m05-bench");
    for c in COURSE {
        cmd(&mut p, c);
    }
    let s = &p.s;
    let n = 1_000_000;
    let feet = |i: usize| {
        let h = (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        [(h % 44) as i32, -61 + ((h >> 8) % 2) as i32, ((h >> 16) % 34) as i32 - 2]
    };
    // The rule before `RUSTCRAFT_OPT_MOBSTATES`: the cell read for each of its three tests.
    let three_reads = |p: [i32; 3]| {
        let fence = s.btag_id(p, TagId::Fences);
        fence || s.btag_id(p, TagId::Walls) || s.bname(p).ends_with("_fence_gate")
    };
    for pass in 0..2 {
        let names = per_call(n, |i| u64::from(three_reads(feet(i))));
        let table = per_call(n, |i| u64::from(s.phys.keeps_support(s.st(feet(i)), 0.2)));
        println!("pass {pass}: support kept under the feet, ns per call: three reads by tags and name {names:.1}, one read and the table {table:.1}");
    }
    let boxes: Vec<(rc_physics::Aabb, [f64; 3])> = (0..10_000)
        .map(|i| {
            let h = (i as u64).wrapping_mul(0xD1B5_4A32_D192_ED03);
            let (x, z) = ((h % 4400) as f64 / 100.0, ((h >> 16) % 3400) as f64 / 100.0 - 2.0);
            let bottom = -60.0 + [0.0, 0.5, 1.5][(h >> 40) as usize % 3];
            (rc_physics::Aabb { min: [x - 0.45, bottom, z - 0.45], max: [x + 0.45, bottom + 1.4, z + 0.45] }, [x, bottom, z])
        })
        .collect();
    let bits = |o: Option<[i32; 3]>| o.map_or(7, |c| (c[0] as u64) ^ ((c[1] as u64) << 21) ^ ((c[2] as u64) << 42));
    for pass in 0..2 {
        for cursor in [false, true] {
            let w = sim_world(&p, cursor);
            let walk = per_call(boxes.len() * 20, |i| {
                let (b, feet) = &boxes[i % boxes.len()];
                let d = [0.0, -0.0784, 0.0];
                let a = rc_physics::collide::resolve(&w, b, d, true, 0.6);
                bits(rc_physics::support_of(&w, &b.offset(a[0], a[1], a[2]), [feet[0], feet[1] + a[1], feet[2]]))
            });
            let mut kept = rc_physics::collide::CollectedBlocks::default();
            let shared = per_call(boxes.len() * 20, |i| {
                let (b, feet) = &boxes[i % boxes.len()];
                let d = [0.0, -0.0784, 0.0];
                let a = rc_physics::collide::resolve_keeping(&w, b, d, true, 0.6, &mut kept);
                bits(rc_physics::support_of_kept(&w, &kept, &b.offset(a[0], a[1], a[2]), [feet[0], feet[1] + a[1], feet[2]]))
            });
            let fluids = per_call(boxes.len() * 20, |i| {
                let (b, feet) = &boxes[i % boxes.len()];
                let mut v = [0.0; 3];
                let (water, ..) = rc_physics::scan_fluids_entity(&w, &s.phys, b, *feet, feet[1] + 1.3, true, false, &mut v);
                water.to_bits()
            });
            println!(
                "pass {pass}, cursor {cursor}: resolve + support ns per mob: walked {walk:.0}, kept {shared:.0}; fluid scan {fluids:.0}"
            );
        }
    }
}

/// Crowd of N mobs on the bench model world (`RUSTCRAFT_OPT_WORLD`), like `tests_opt::bench_crowd`:
/// 50 warm-up ticks then 200 measured; `RUSTCRAFT_PERFLOG=1` adds the rest rate of the movements.
/// The mission's switches are read once: one process per variant.
#[test]
#[ignore = "measurement bench"]
fn bench_crowd_1000() {
    let Some(model) = rc_testdata::need(crate::tests_opt::model_dir(), "bench model world") else { return };
    let Some(data) = rc_testdata::need(rc_testdata::version_data(), "26.3 data cache") else { return };
    crate::mobs::index::set_verify(false);
    let data = Arc::new(data);
    let dir = crate::tests::fresh_copy(&model, "c6m05crowd");
    let mut s = crate::portal_tests::open_world(&dir, data);
    crate::tests_opt::setup(&mut s);
    crate::nbtio::set(&mut s.rules, "minecraft:spawn_mobs", rc_proto::nbt::Nbt::Byte(0));
    crate::tests_opt::go(&mut s, crate::dims::OVERWORLD, 1600.5, Some(76.0), -447.5, false);
    let n = 1000usize;
    let side = 24.0 * (n as f64 / 100.0).sqrt();
    let kinds = [Kind::Cow, Kind::Sheep, Kind::Pig, Kind::Chicken];
    let mut r: u64 = 0x5EED;
    for i in 0..n {
        r ^= r << 13;
        r ^= r >> 7;
        r ^= r << 17;
        let (fx, fz) = ((r & 0xFFFF) as f64 / 65536.0, ((r >> 16) & 0xFFFF) as f64 / 65536.0);
        let (x, z) = (1600.5 - side / 2.0 + fx * side, -447.5 - side / 2.0 + fz * side);
        let y = f64::from(s.surface_y(x.floor() as i32, z.floor() as i32, 4));
        let mut m = s.new_mob(kinds[i % 4], [x, y, z], 0.0);
        m.persistent = true;
        if i % 3 == 0 {
            m.age = -24000;
        }
        s.add_mob(m);
    }
    crate::tests_opt::ticks(&mut s, 50);
    let _ = crate::mobs::move_stats::take_report();
    let lookups = || crate::play::COLUMN_LOOKUPS.with(std::cell::Cell::get);
    let lookups_before = lookups();
    let mut d: Vec<f64> = (0..200)
        .map(|_| {
            let t = std::time::Instant::now();
            s.game_tick();
            s.tick += 1;
            t.elapsed().as_secs_f64() * 1e3
        })
        .collect();
    let mean = d.iter().sum::<f64>() / d.len() as f64;
    d.sort_by(f64::total_cmp);
    println!(
        "bench_crowd_1000 ({} mobs): median {:.3} ms, p99 {:.3} ms, mean {mean:.3} ms, {:.0} column lookups per tick",
        s.mobs.list.len(),
        d[d.len() / 2],
        d[d.len() * 99 / 100],
        (lookups() - lookups_before) as f64 / d.len() as f64
    );
    if let Some(line) = crate::mobs::move_stats::take_report() {
        println!("{line}");
    }
    s.stop_gen();
    s.stop_parked();
}
