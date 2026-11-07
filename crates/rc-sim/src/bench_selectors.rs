//! Micro-bench of the selector switches (`commands::paths`: candidate view, type first, first
//! match of `execute if entity`) and of the indexes they lean on (`RUSTCRAFT_OPT_MOBINDEX`,
//! `RUSTCRAFT_OPT_KEPT_ENTITY_INDEX`: read once per process, so compared by running the bench
//! twice, the second time with both set to 0), on the flat world:
//! - one search over 100, 1,000 and 5,000 zombies (`type=`, `distance=`, `sort=nearest`, `limit`);
//! - the quadratic case: every zombie searching the zombies around it (`execute as … at @s run …`
//!   and `execute as … at @s if entity … run …`), 100 and 1,000 zombies;
//! - searches over 1,000 markers, ground items and orbs (the families read as records);
//! - 1,000 markers each adding to its own score (`@s`: the kept entity lookup).
//!
//! One thread: commands run on the server thread (a search reads the world in order; no
//! multi-core or GPU variant). Run alone:
//! `cargo test --release -p rc-sim [--features memstats] bench_selectors -- --ignored --nocapture --test-threads=1`
//! then again with `RUSTCRAFT_OPT_MOBINDEX=0` and `RUSTCRAFT_OPT_KEPT_ENTITY_INDEX=0`.

use crate::bench_kit::{self, cmd};
use crate::blocks::probe::Probe;
use crate::commands::paths::CommandPaths;

const RUNS: usize = 5;

/// The selector paths compared: former search, candidate view, then each lead added.
fn variants() -> [(&'static str, CommandPaths); 4] {
    let with = |view: bool, type_first: bool, any: bool| CommandPaths {
        selector_candidate_view: view,
        selector_type_first: type_first,
        execute_if_entity_first_match: any,
        ..CommandPaths::NEW
    };
    [
        ("old", with(false, false, false)),
        ("candidate_view", with(true, false, false)),
        ("view_type_first", with(true, true, false)),
        ("all", CommandPaths::NEW),
    ]
}

/// Flat world with `n` entities of type `what` (own fields `fields`) on a 64-block wide band,
/// two per block, tagged `t0` to `t3` in turn.
fn world(tag: &str, n: usize, what: &str, fields: &str) -> (Probe, std::path::PathBuf) {
    let (mut p, dir) = crate::tests_g3f13::open(tag);
    for i in 0..n {
        let (x, z) = ((i % 128) as f64 / 2.0, (i / 128) as f64 / 2.0 - 16.0);
        let common = format!("NoAI:1b,Silent:1b,PersistenceRequired:1b,Tags:[\"t{}\"]", i % 4);
        cmd(&mut p, &format!("summon {what} {x} -60 {z} {{{fields}{common}}}"));
    }
    cmd(&mut p, "scoreboard objectives add b dummy");
    let _ = p.take_events();
    (p, dir)
}

/// µs (median and best of `RUNS`) and allocations per run of `repeat` runs of `command`.
fn time_command(p: &mut Probe, command: &str, repeat: u32) -> (f64, f64, f64) {
    let src = p.s.server_source();
    let _ = p.s.perform(src.clone(), command);
    let (mut us, mut allocs) = (Vec::new(), Vec::new());
    for _ in 0..RUNS {
        let (start, a) = (std::time::Instant::now(), bench_kit::allocations());
        for _ in 0..repeat {
            std::hint::black_box(p.s.perform(src.clone(), command));
        }
        us.push(start.elapsed().as_secs_f64() * 1e6 / f64::from(repeat));
        allocs.push((bench_kit::allocations() - a) as f64 / f64::from(repeat));
    }
    (bench_kit::median(&us), us.iter().copied().fold(f64::INFINITY, f64::min), bench_kit::median(&allocs))
}

fn report(bench: &str, case: &str, (median, best, allocs): (f64, f64, f64)) {
    bench_kit::report(bench, case, "us", median);
    bench_kit::report(bench, case, "us_best", best);
    if cfg!(feature = "memstats") {
        bench_kit::report(bench, case, "allocs", allocs);
    }
}

/// Every command of `cases` on every variant, on one world.
fn run_cases(bench: &str, p: &mut Probe, n: usize, cases: &[(&str, &str, u32)]) {
    for (name, command, repeat) in cases {
        for (variant, paths) in variants() {
            p.s.cmd.paths = paths;
            report(bench, &format!("{name}_{n}_{variant}"), time_command(p, command, *repeat));
        }
    }
}

#[test]
#[ignore = "measurement bench"]
fn bench_selectors_one_search() {
    crate::mobs::index::set_verify(false);
    for n in [100, 1_000, 5_000] {
        let (mut p, dir) = world(&format!("bench-select-{n}"), n, "minecraft:zombie", "");
        let repeat = (200_000 / n as u32).clamp(20, 2_000);
        let cases = [
            ("type", "execute if entity @e[type=minecraft:zombie]", repeat),
            ("type_tag", "execute if entity @e[type=minecraft:zombie,tag=t1]", repeat),
            ("distance", "execute positioned 16 -60 0 if entity @e[distance=..2]", repeat),
            ("nearest", "execute positioned 16 -60 0 if entity @e[type=minecraft:zombie,sort=nearest,limit=1]", repeat),
            ("limit", "execute if entity @e[type=minecraft:zombie,limit=5]", repeat),
            ("first_match", "execute if entity @e[type=minecraft:zombie,tag=t3] run scoreboard players add #n b 1", repeat),
        ];
        run_cases("select_one", &mut p, n, &cases);
        crate::tests_g3f13::finish(p, dir);
    }
}

#[test]
#[ignore = "measurement bench"]
fn bench_selectors_quadratic() {
    crate::mobs::index::set_verify(false);
    for n in [100, 1_000] {
        let (mut p, dir) = world(&format!("bench-select-quad-{n}"), n, "minecraft:zombie", "");
        let repeat = if n > 100 { 2 } else { 20 };
        let cases = [
            ("as_at_run", "execute as @e[type=minecraft:zombie] at @s run scoreboard players add @e[type=minecraft:zombie,distance=..2] b 1", repeat),
            ("as_at_if_entity", "execute as @e[type=minecraft:zombie] at @s if entity @e[type=minecraft:zombie,distance=..2,tag=t2] run scoreboard players add @s b 1", repeat),
        ];
        run_cases("select_quadratic", &mut p, n, &cases);
        crate::tests_g3f13::finish(p, dir);
    }
}

#[test]
#[ignore = "measurement bench"]
fn bench_selectors_other_families() {
    crate::mobs::index::set_verify(false);
    for (family, what, fields) in [
        ("marker", "minecraft:marker", ""),
        ("item", "minecraft:item", "Item:{id:\"minecraft:stone\",count:1},PickupDelay:32767s,NoGravity:1b,"),
        ("orb", "minecraft:experience_orb", "Value:1,"),
    ] {
        let (mut p, dir) = world(&format!("bench-select-{family}"), 1_000, what, fields);
        let cases = [
            ("type", format!("execute if entity @e[type={what}]")),
            ("distance", "execute positioned 16 -60 0 if entity @e[distance=..2]".to_owned()),
            ("tag", "execute if entity @e[tag=t1]".to_owned()),
        ];
        let cases: Vec<(&str, &str, u32)> = cases.iter().map(|(n, c)| (*n, c.as_str(), 200)).collect();
        run_cases(&format!("select_{family}"), &mut p, 1_000, &cases);
        crate::tests_g3f13::finish(p, dir);
    }
}

/// What re-reading a selector's text at every run costs (`select::parse` × 100,000): the figure
/// behind pre-reading arguments with the kept parse of a line.
#[test]
#[ignore = "measurement bench"]
fn bench_selectors_parse() {
    for text in ["@s", "@e[type=minecraft:marker,tag=src]", "@e[type=minecraft:zombie,distance=..2,limit=1,sort=nearest]"] {
        let (mut ns, mut allocs) = (Vec::new(), Vec::new());
        for _ in 0..RUNS {
            let (start, a) = (std::time::Instant::now(), bench_kit::allocations());
            for _ in 0..100_000 {
                std::hint::black_box(crate::commands::select::parse(std::hint::black_box(text)));
            }
            ns.push(start.elapsed().as_secs_f64() * 1e9 / 1e5);
            allocs.push((bench_kit::allocations() - a) as f64 / 1e5);
        }
        let case = text.split('[').next().unwrap_or(text).trim_start_matches('@').to_owned() + &format!("_{}_chars", text.len());
        bench_kit::report("select_parse", &case, "ns", bench_kit::median(&ns));
        if cfg!(feature = "memstats") {
            bench_kit::report("select_parse", &case, "allocs", bench_kit::median(&allocs));
        }
    }
}

#[test]
#[ignore = "measurement bench"]
fn bench_selectors_markers_self() {
    crate::mobs::index::set_verify(false);
    for n in [100, 1_000] {
        let (mut p, dir) = world(&format!("bench-select-self-{n}"), n, "minecraft:marker", "");
        let cases = [
            ("as_score_self", "execute as @e[type=minecraft:marker] run scoreboard players add @s b 1", 10),
            ("as_if_self", "execute as @e[type=minecraft:marker] if entity @s[tag=t1] run scoreboard players add #n b 1", 10),
        ];
        run_cases("select_markers_self", &mut p, n, &cases);
        crate::tests_g3f13::finish(p, dir);
    }
}
