//! Micro-bench of the spawner blocks and natural spawn paths (`SpawnPaths`, the
//! `RUSTCRAFT_OPT_TRIAL_*`, `VAULT_EARLY_EXIT`, `SPAWNER_*`, `SPAWN_CENSUS`, `STRUCTURE_STARTS`
//! switches), each case on the former paths, the new ones, and each new path alone:
//! - block entity phase of 64 waiting trial spawners, 64 active ones with nobody near, 32 idle
//!   vaults, 32 monster spawners out of range, 32 counting down in range (µs per tick);
//! - natural spawn pass at night, about 70 monsters, two players, plain and soul sand valley;
//! - structure spawn list of 10,000 positions around a 300-piece fortress (ns per position).
//!
//! One thread: these run on the server thread, in the game's order (no multi-core or GPU
//! variant). Run alone:
//! `cargo test --release -p rc-sim [--features memstats] bench_spawner_blocks -- --ignored --nocapture --test-threads=1`.

use std::time::Instant;

use rc_proto::nbt::Nbt;

use crate::bench_kit::{self, cmd};
use crate::blocks::probe::Probe;
use crate::mobs::spawn::SpawnPaths;
use crate::nbtio;

const BENCH: &str = "spawner_blocks";
const RUNS: usize = 5;

/// The paths compared: former, new, and each new path alone among the former ones.
fn path_sets() -> Vec<(&'static str, SpawnPaths)> {
    let old = SpawnPaths::OLD;
    vec![
        ("former", old),
        ("new", SpawnPaths::NEW),
        ("trial_write_on_change", SpawnPaths { trial_write_on_change: true, ..old }),
        ("trial_borrow", SpawnPaths { trial_borrow: true, ..old }),
        ("vault_early_exit", SpawnPaths { vault_early_exit: true, ..old }),
        ("spawner_range_first", SpawnPaths { spawner_range_first: true, ..old }),
        ("spawner_borrow", SpawnPaths { spawner_borrow: true, ..old }),
        ("spawn_census", SpawnPaths { spawn_census: true, ..old }),
        ("structure_starts", SpawnPaths { structure_starts: true, ..old }),
        ("placement_first", SpawnPaths { placement_first: true, ..old }),
        ("new_with_placement_first", SpawnPaths { placement_first: true, ..SpawnPaths::NEW }),
    ]
}

/// Flat world (player at the origin) with `place` run on it, block entities ticked 40 times.
fn world(tag: &str, place: impl Fn(&mut Probe)) -> (Probe, std::path::PathBuf) {
    let (mut p, dir) = crate::tests_g3f13::open(tag);
    place(&mut p);
    block_entity_ticks(&mut p, 40);
    (p, dir)
}

fn block_entity_ticks(p: &mut Probe, n: u32) {
    for _ in 0..n {
        p.s.time += 1;
        p.s.tick_block_entities();
        let _ = p.take_events();
    }
}

/// Grid of `n` positions at y = -60, spaced by 4, from (`x0`, `z0`), 8 per row.
fn grid(n: usize, x0: i32, z0: i32) -> Vec<[i32; 3]> {
    (0..n as i32).map(|i| [x0 + 4 * (i % 8), -60, z0 + 4 * (i / 8)]).collect()
}

/// Trial spawners configured like a generated chamber's (registry ids of the vanilla pack). An
/// active one has a registered player far away: it spawns its zombies, then waits for them to
/// die (they never do here), staying active.
fn trial_spawners(p: &mut Probe, at: &[[i32; 3]], state: &str) {
    cmd(p, "gamerule minecraft:spawn_mobs true");
    for &q in at {
        cmd(p, &format!("setblock {} {} {} minecraft:trial_spawner[trial_spawner_state={state}]", q[0], q[1], q[2]));
        let be = p.s.be_get_mut(q).expect("trial spawner");
        let config = |kind: &str| Nbt::String(format!("minecraft:trial_chamber/melee/zombie/{kind}"));
        nbtio::set(be, "normal_config", config("normal"));
        nbtio::set(be, "ominous_config", config("ominous"));
        if state == "active" {
            nbtio::set(be, "registered_players", Nbt::List(vec![nbtio::uuid_nbt(42)]));
        }
    }
}

/// Chunks marked for saving by `ticks` block entity ticks (each counted once): what the next
/// autosave re-encodes.
fn chunks_marked(p: &mut Probe, ticks: u32) -> usize {
    for c in p.s.chunks.values_mut().flatten() {
        c.dirty = false;
    }
    block_entity_ticks(p, ticks);
    p.s.chunks.values().flatten().filter(|c| c.dirty).count()
}

fn spawners(p: &mut Probe, at: &[[i32; 3]]) {
    for &q in at {
        cmd(p, &format!("setblock {} {} {} minecraft:spawner", q[0], q[1], q[2]));
        cmd(
            p,
            &format!(
                "data merge block {} {} {} {{Delay:30000s,MinSpawnDelay:30000s,MaxSpawnDelay:30000s,\
                 SpawnData:{{entity:{{id:\"minecraft:zombie\"}}}}}}",
                q[0], q[1], q[2]
            ),
        );
    }
}

/// µs per tick of the block entity phase, then allocations per tick (with `memstats`).
fn time_block_entities(p: &mut Probe, ticks: u32) -> (f64, f64) {
    let mut us = Vec::new();
    let mut allocs = Vec::new();
    for _ in 0..RUNS {
        let (start, a) = (Instant::now(), bench_kit::allocations());
        block_entity_ticks(p, ticks);
        us.push(start.elapsed().as_secs_f64() * 1e6 / f64::from(ticks));
        allocs.push((bench_kit::allocations() - a) as f64 / f64::from(ticks));
    }
    (bench_kit::median(&us), bench_kit::median(&allocs))
}

fn report_case(case: &str, paths: &str, (us, allocs): (f64, f64), spawners: usize) {
    bench_kit::report(BENCH, &format!("{case}/{paths}"), "us_per_tick", us);
    bench_kit::report(BENCH, &format!("{case}/{paths}"), "ns_per_block", us * 1000.0 / spawners as f64);
    if cfg!(feature = "memstats") {
        bench_kit::report(BENCH, &format!("{case}/{paths}"), "allocs_per_tick", allocs);
    }
}

#[test]
#[ignore = "micro-bench"]
fn bench_spawner_blocks() {
    type Place = fn(&mut Probe);
    let cases: [(&str, usize, Place); 5] = [
        ("trial_waiting_64", 64, |p| trial_spawners(p, &grid(64, 40, -14), "waiting_for_players")),
        ("trial_active_64_nobody_near", 64, |p| trial_spawners(p, &grid(64, 40, -14), "active")),
        ("vault_idle_32", 32, |p| {
            for q in grid(32, 40, -14) {
                cmd(p, &format!("setblock {} {} {} minecraft:vault", q[0], q[1], q[2]));
            }
        }),
        ("spawner_out_of_range_32", 32, |p| spawners(p, &grid(32, 40, -14))),
        ("spawner_counting_down_32", 32, |p| spawners(p, &grid(32, -14, -14))),
    ];
    // Two alternating passes over every case and path.
    for pass in 0..2 {
        for (case, n, place) in cases {
            let natural_only = ["spawn_census", "structure_starts", "placement_first", "new_with_placement_first"];
            for (name, paths) in path_sets().into_iter().filter(|(n, _)| !natural_only.contains(n)) {
                let (timing, marked) = paths.run(|| {
                    let (mut p, _dir) = world(&format!("bench-{case}-{name}-{pass}"), place);
                    (time_block_entities(&mut p, 10_000), chunks_marked(&mut p, 1000))
                });
                report_case(case, name, timing, n);
                bench_kit::report(BENCH, &format!("{case}/{name}"), "chunks_marked_per_1000_ticks", marked as f64);
            }
        }
    }
}

/// Night, spawns on, about 70 non-persistent monsters around, a LAN guest 3 chunks away, half
/// of the area in the soul sand valley (spawn costs).
fn natural_world(tag: &str) -> (Probe, std::path::PathBuf) {
    let (mut p, dir) = crate::tests_g3f13::open(tag);
    cmd(&mut p, "gamerule minecraft:spawn_mobs true");
    cmd(&mut p, "time set midnight");
    cmd(&mut p, "fillbiome 64 -64 -32 127 -57 31 minecraft:soul_sand_valley");
    for i in 0..70 {
        let at = [f64::from(16 + (i * 7) % 120) + 0.5, -60.0, f64::from((i * 11) % 60 - 30) + 0.5];
        let m = p.s.new_mob(crate::mobs::species::Kind::Zombie, at, 0.0);
        p.s.add_mob(m);
    }
    // The guest's channel is dropped at once: nothing reads it, the bench only needs its place.
    let _ = crate::mobs::tests_spawn_paths::add_guest(&mut p.s, "Guest", [48.5, -60.0, 0.5]);
    (p, dir)
}

/// Spawn lists of 10,000 positions around a 300-piece fortress, an outpost and a witch hut (test
/// world): every position as one more attempt of the same pass (decoded starts kept).
#[test]
#[ignore = "micro-bench"]
fn bench_structure_spawn_lists() {
    use crate::mobs::species::Category;
    let Some((mut p, _dir)) = crate::mobs::tests_spawn_paths::open("bench-structures") else { return };
    crate::mobs::tests_spawn_paths::write_structures(&mut p);
    let positions = crate::mobs::tests_spawn_paths::probe_positions(10_000);
    let per_position = |t: Instant| t.elapsed().as_secs_f64() * 1e9 / positions.len() as f64;
    for _ in 0..2 {
        let mut former = Vec::new();
        let mut decoded = Vec::new();
        let mut decoded_fresh = Vec::new();
        for _ in 0..RUNS {
            let t = Instant::now();
            for &q in &positions {
                std::hint::black_box(p.s.spawn_list_at(Category::Monster, "monster", q));
            }
            former.push(per_position(t));
            let t = Instant::now();
            let mut starts = crate::mobs::structure_spawn::DecodedStarts::default();
            for &q in &positions {
                std::hint::black_box(p.s.spawn_list_decoded(&mut starts, Category::Monster, "monster", q));
            }
            decoded.push(per_position(t));
            // Worst case: a pass of a single attempt (the starts decoded for it alone).
            let t = Instant::now();
            for &q in &positions[..1000] {
                let mut starts = crate::mobs::structure_spawn::DecodedStarts::default();
                std::hint::black_box(p.s.spawn_list_decoded(&mut starts, Category::Monster, "monster", q));
            }
            decoded_fresh.push(t.elapsed().as_secs_f64() * 1e9 / 1000.0);
        }
        bench_kit::report(BENCH, "structure_lists/former", "ns_per_position", bench_kit::median(&former));
        bench_kit::report(BENCH, "structure_lists/decoded", "ns_per_position", bench_kit::median(&decoded));
        bench_kit::report(
            BENCH,
            "structure_lists/decoded_one_per_pass",
            "ns_per_position",
            bench_kit::median(&decoded_fresh),
        );
    }
}

#[test]
#[ignore = "micro-bench"]
fn bench_natural_spawn_pass() {
    let relevant = ["former", "new", "spawn_census", "structure_starts", "placement_first", "new_with_placement_first"];
    // Two alternating passes; each run in its own thread: same test seeds, so the same world
    // evolution on every path (they are equivalent), the timings comparable.
    for _ in 0..2 {
        for (name, paths) in path_sets().into_iter().filter(|(n, _)| relevant.contains(n)) {
            let run = move || paths.run(|| natural_passes(&format!("bench-natural-{name}")));
            let (us, allocs) = std::thread::spawn(run).join().expect("bench thread");
            bench_kit::report(BENCH, &format!("natural_pass/{name}"), "us_per_pass", us);
            if cfg!(feature = "memstats") {
                bench_kit::report(BENCH, &format!("natural_pass/{name}"), "allocs_per_pass", allocs);
            }
        }
    }
}

/// µs and allocations per natural spawn pass (median of the runs of 200 passes).
fn natural_passes(tag: &str) -> (f64, f64) {
    let (mut p, _dir) = natural_world(tag);
    let mut us = Vec::new();
    let mut allocs = Vec::new();
    for _ in 0..RUNS {
        let (start, a) = (Instant::now(), bench_kit::allocations());
        for _ in 0..200 {
            p.s.time += 1;
            p.s.natural_spawn_tick();
        }
        us.push(start.elapsed().as_secs_f64() * 1e6 / 200.0);
        allocs.push((bench_kit::allocations() - a) as f64 / 200.0);
    }
    (bench_kit::median(&us), bench_kit::median(&allocs))
}
