//! Micro-bench of the command switches (`commands::paths`), each lead alone then all of them, on
//! the flat world with a pack written by the bench itself (no `RUSTCRAFT_PERF_PACK` needed):
//! - a 20-line function run every tick by 1, 100 and 1,000 markers (`execute as … run function`),
//!   plain lines then macro lines (same arguments every tick, then 16 values in turn);
//! - 256 repeating command blocks in one chunk, then spread over 16 chunks, then failing (unknown
//!   command, tracked output);
//! - a tick with 1,000 and 10,000 `/schedule` entries waiting, none due;
//! - the parse of 1,000 typical commands (server parse, no world).
//!
//! - the memory and load time of a 100,000-line function.
//!
//! One thread: commands run on the server thread (no multi-core or GPU variant: a command reads
//! and writes the world in order). One test per part (`bench_function_memory`,
//! `bench_command_parse`, `bench_function_tick`, `bench_command_blocks`, `bench_schedule_queue`);
//! run alone:
//! `cargo test --release -p rc-sim [--features memstats] bench_function_tick:: -- --ignored --nocapture --test-threads=1`.

use std::path::Path;

use crate::bench_kit::{self, cmd};
use crate::blocks::probe::Probe;
use crate::commands::paths::CommandPaths;

const MCMETA: &str = r#"{"pack":{"description":"bench","pack_format":121,"min_format":121,"max_format":121}}"#;

/// 20 plain lines working on the executing marker's score and a storage (no entity search).
const WORK: &str = "scoreboard players add @s bw 1
scoreboard players operation #sum bw += @s bw
execute if score @s bw matches 1000.. run scoreboard players set @s bw 0
data modify storage bench:s last set value 1
execute store result storage bench:s v int 1 run scoreboard players get @s bw
scoreboard players set #tmp bw 3
scoreboard players operation #tmp bw *= #tmp bw
execute if score #tmp bw matches 9 run scoreboard players add #nine bw 1
execute unless score #tmp bw matches 10 run scoreboard players add #ten bw 1
scoreboard players remove #tmp bw 1
scoreboard players add #lines bw 1
execute if score @s bw > #tmp bw run scoreboard players add #more bw 1
scoreboard players operation #max bw > @s bw
scoreboard players operation #min bw < @s bw
data modify storage bench:s flag set value 1b
execute store success score #ok bw run data get storage bench:s v
scoreboard players add #lines bw 1
execute if data storage bench:s {flag:1b} run scoreboard players add #flag bw 1
scoreboard players set #end bw 1
scoreboard players add #lines bw 1
";

/// The same 20 lines with their numbers given as macro arguments.
const MACRO_WORK: &str = "$scoreboard players add @s bw $(n)
scoreboard players operation #sum bw += @s bw
$execute if score @s bw matches $(big).. run scoreboard players set @s bw 0
$data modify storage bench:s last set value $(n)
execute store result storage bench:s v int 1 run scoreboard players get @s bw
$scoreboard players set #tmp bw $(n)
scoreboard players operation #tmp bw *= #tmp bw
$execute if score #tmp bw matches $(n) run scoreboard players add #nine bw 1
execute unless score #tmp bw matches 10 run scoreboard players add #ten bw 1
$scoreboard players remove #tmp bw $(n)
scoreboard players add #lines bw 1
execute if score @s bw > #tmp bw run scoreboard players add #more bw 1
scoreboard players operation #max bw > @s bw
scoreboard players operation #min bw < @s bw
data modify storage bench:s flag set value 1b
execute store success score #ok bw run data get storage bench:s v
$scoreboard players add #lines bw $(n)
execute if data storage bench:s {flag:1b} run scoreboard players add #flag bw 1
scoreboard players set #end bw 1
scoreboard players add #lines bw 1
";

const TICK: &str = "execute if score #mode bw matches 1 as @e[type=minecraft:marker,tag=src] run function bench:work
execute if score #mode bw matches 2 as @e[type=minecraft:marker,tag=src] run function bench:mwork with storage bench:args
execute if score #mode bw matches 3 run scoreboard players add #turn bw 1
execute if score #mode bw matches 3 if score #turn bw matches 16.. run scoreboard players set #turn bw 0
execute if score #mode bw matches 3 store result storage bench:args n int 1 run scoreboard players get #turn bw
execute if score #mode bw matches 3 as @e[type=minecraft:marker,tag=src] run function bench:mwork with storage bench:args
";

fn write_pack(world: &Path) {
    let pack = world.join("datapacks").join("bench");
    let write = |rel: &str, body: &str| {
        let file = pack.join(rel);
        std::fs::create_dir_all(file.parent().expect("parent")).expect("pack folder");
        std::fs::write(file, body).expect("pack file");
    };
    write("pack.mcmeta", MCMETA);
    write("data/bench/function/work.mcfunction", WORK);
    write("data/bench/function/mwork.mcfunction", MACRO_WORK);
    write("data/bench/function/tick.mcfunction", TICK);
    write("data/bench/function/noop.mcfunction", "scoreboard players add #noop bw 1\n");
    write("data/minecraft/tags/function/tick.json", r#"{"values":["bench:tick"]}"#);
}

/// Flat world with the bench pack, opened on `paths`.
fn world(tag: &str, paths: CommandPaths) -> (Probe, std::path::PathBuf) {
    CommandPaths::forced(paths, || {
        let (p, dir) = crate::tests_g3f13::open(tag);
        let data = p.s.data.clone();
        drop(p);
        write_pack(&dir);
        let mut p = crate::tests_g3f13::reopen(&dir, data);
        assert_eq!(p.s.cmd.paths, paths);
        assert!(p.s.cmd.functions.contains_key("bench:work"), "bench pack not loaded");
        cmd(&mut p, "scoreboard objectives add bw dummy");
        cmd(&mut p, "data merge storage bench:args {n:3,big:1000}");
        (p, dir)
    })
}

const WARMUP_TICKS: u32 = 10;
const RUNS: usize = 3;

/// Ticks per run: about 20,000 function calls per run whatever the number of sources.
fn measured_ticks(sources: usize) -> u32 {
    (20_000 / sources.max(1)).clamp(20, 200) as u32
}

/// One lead measured: its name and the switches it turns on.
type Lead = (&'static str, fn(&mut CommandPaths));

/// The leads compared: every switch off, each lead alone, all on.
fn variants(leads: &[Lead]) -> Vec<(&'static str, CommandPaths)> {
    let mut out = vec![("old", CommandPaths::OLD)];
    for (name, set) in leads {
        let mut p = CommandPaths::OLD;
        set(&mut p);
        out.push((name, p));
    }
    out.push(("all", CommandPaths::NEW));
    out
}

const FUNCTION_LEADS: &[Lead] = &[
    ("function_cache", |p| p.function_cache = true),
    ("macro_cache", |p| p.macro_cache = true),
    ("shared_source", |p| p.shared_source = true),
    ("parser_no_alloc", |p| p.parser_no_alloc = true),
    ("function_and_macro_cache", |p| {
        p.function_cache = true;
        p.macro_cache = true;
    }),
];

/// Mode 1: plain lines; 2: macro lines, same arguments; 3: macro lines, 16 values in turn.
fn function_case(sources: usize, mode: u8, variant: &str, paths: CommandPaths) {
    let (mut p, dir) = world(&format!("fn-{sources}-{mode}-{variant}"), paths);
    for i in 0..sources {
        let (x, z) = (i % 32, i / 32);
        cmd(&mut p, &format!("summon minecraft:marker {x} -60 {z} {{Tags:[\"src\"]}}"));
    }
    cmd(&mut p, &format!("scoreboard players set #mode bw {mode}"));
    let t = bench_kit::time_ticks(&mut p, WARMUP_TICKS, measured_ticks(sources), RUNS);
    let ran = p.s.cmd.scores.get("#end", "bw");
    assert_eq!(ran, Some(1), "the function did not run");
    let kind = ["", "plain", "macro_same", "macro_16"][usize::from(mode)];
    bench_kit::report_timing("function_tick", &format!("{kind}_{sources}_{variant}"), &t);
    crate::tests_g3f13::finish(p, dir);
}

#[derive(Clone, Copy)]
enum Blocks {
    OneChunk,
    SixteenChunks,
    Failing,
}

const BLOCK_LEADS: &[Lead] = &[
    ("parse_cache", |p| p.parse_cache = true),
    ("command_block_borrow", |p| p.command_block_borrow = true),
    ("local_time_cache", |p| p.local_time_cache = true),
    ("parser_no_alloc", |p| p.parser_no_alloc = true),
];

const BLOCK_TICKS: u32 = 100;

fn command_block_case(layout: Blocks, variant: &str, paths: CommandPaths) {
    let (mut p, dir) = world(&format!("cb-{variant}"), paths);
    // One chunk: 16 × 16 blocks in chunk (1, 0); sixteen: 8 blocks apart along x, 2 along z,
    // over chunks 1..=8 × -1..=0 (inside the loaded area: its edge chunks do not tick).
    let ((dx, dz), z0, command, track) = match layout {
        Blocks::OneChunk => ((1, 1), 0, "scoreboard players add #cb bw 1", 0),
        Blocks::SixteenChunks => ((8, 2), -16, "scoreboard players add #cb bw 1", 0),
        Blocks::Failing => ((1, 1), 0, "unknowncommand here", 1),
    };
    for i in 0..256i32 {
        let (x, z) = (16 + (i % 16) * dx, z0 + (i / 16) * dz);
        cmd(
            &mut p,
            &format!("setblock {x} -62 {z} minecraft:repeating_command_block{{auto:1b,TrackOutput:{track}b,Command:'{command}'}}"),
        );
    }
    let t = bench_kit::time_ticks(&mut p, WARMUP_TICKS, BLOCK_TICKS, RUNS);
    if !matches!(layout, Blocks::Failing) {
        let ticks = (WARMUP_TICKS + BLOCK_TICKS * RUNS as u32) as i32;
        let n = p.s.cmd.scores.get("#cb", "bw").unwrap_or(0);
        assert!(n >= 256 * (ticks - 2), "{n}: the command blocks did not all run");
    }
    let name = match layout {
        Blocks::OneChunk => "one_chunk",
        Blocks::SixteenChunks => "sixteen_chunks",
        Blocks::Failing => "failing",
    };
    bench_kit::report_timing("command_blocks", &format!("{name}_{variant}"), &t);
    crate::tests_g3f13::finish(p, dir);
}

fn schedule_case(waiting: usize, variant: &str, paths: CommandPaths) {
    let (mut p, dir) = world(&format!("sched-{waiting}-{variant}"), paths);
    for i in 0..waiting {
        // Far in the future, each its own name (append keeps them all).
        let src = p.s.server_source();
        let _ = p.s.perform(src, &format!("schedule function bench:noop {}t append", 100_000 + i));
    }
    assert_eq!(p.s.cmd.scheduled.len(), waiting);
    let t = bench_kit::time_ticks(&mut p, WARMUP_TICKS, BLOCK_TICKS, RUNS);
    bench_kit::report_timing("schedule", &format!("waiting_{waiting}_{variant}"), &t);
    crate::tests_g3f13::finish(p, dir);
}

/// 1,000 commands of the usual shapes (the `WORK` lines and a few typed ones).
fn parse_corpus() -> Vec<String> {
    let typed = [
        "execute as @e[type=minecraft:cow,distance=..64] at @s run tp @s ~ ~ ~ ~2 ~",
        "give @a minecraft:diamond_sword[custom_name='x',!damage] 1",
        "setblock ~ ~ ~ minecraft:oak_stairs[facing=east]",
        "effect give @e[type=minecraft:chicken,distance=..64] minecraft:speed 2 0 true",
        "particle minecraft:happy_villager ~ ~ ~ 4 1 4 0 8 normal",
    ];
    WORK.lines().chain(typed).cycle().take(1_000).map(str::to_owned).collect()
}

fn parse_case(no_alloc: bool) {
    let data = rc_testdata::version_data().expect("26.3 version data missing");
    let levels = crate::commands::load_levels(&rc_data::commands_path(&data.version));
    let d = rc_commands::Data::from_version(&data);
    let corpus = parse_corpus();
    let mut us = Vec::new();
    let mut allocs = Vec::new();
    for _ in 0..7 {
        let (start, a) = (std::time::Instant::now(), bench_kit::allocations());
        for c in &corpus {
            std::hint::black_box(crate::commands::analyse_with(&levels[2], &d, c, 2, no_alloc).is_ok());
        }
        us.push(start.elapsed().as_secs_f64() * 1e6);
        allocs.push((bench_kit::allocations() - a) as f64 / corpus.len() as f64);
    }
    let name = if no_alloc { "parser_no_alloc" } else { "old" };
    bench_kit::report("parse_1000", name, "us", bench_kit::median(&us));
    bench_kit::report("parse_1000", name, "us_best", us.iter().copied().fold(f64::INFINITY, f64::min));
    if cfg!(feature = "memstats") {
        bench_kit::report("parse_1000", name, "allocs_per_command", bench_kit::median(&allocs));
    }
}

/// Memory of a 100,000-line function, bytes per line (text, kept parse, counted once when
/// shared), and its load time; compiled on `paths` straight from the lines. `distinct`: the
/// `WORK` lines with a different objective in each copy (no two lines alike), otherwise the same
/// 20 lines 5,000 times (a generated pack).
fn memory_case(variant: &str, paths: CommandPaths, distinct: bool) {
    use crate::commands::functions::{ReadyLine, SharedLines, compile_with};
    let data = rc_testdata::version_data().expect("26.3 version data missing");
    let levels = crate::commands::load_levels(&rc_data::commands_path(&data.version));
    let d = rc_commands::Data::from_version(&data);
    let owned: Vec<String> = (0..5_000)
        .flat_map(|k| WORK.lines().map(move |l| if distinct { l.replace(" bw", &format!(" bw{k}")) } else { l.to_owned() }))
        .collect();
    let raw: Vec<&str> = owned.iter().map(String::as_str).collect();
    let start = std::time::Instant::now();
    let f = compile_with(&raw, levels.get(2), &d, &paths, &mut SharedLines::default()).expect("big function");
    let load_ms = start.elapsed().as_secs_f64() * 1e3;
    let start = std::time::Instant::now();
    f.parse_every_line(&levels[2], &d);
    let first_run_ms = start.elapsed().as_secs_f64() * 1e3;
    let mut seen = std::collections::HashSet::new();
    let mut bytes = 0usize;
    for line in f.ready_lines() {
        bytes += size_of::<ReadyLine>();
        if seen.insert(line.text.as_ptr()) {
            // Text and the two counters of its shared allocation.
            bytes += line.text.len() + 16;
        }
        if let Some(cell) = &line.parsed
            && seen.insert(std::sync::Arc::as_ptr(cell).cast::<u8>())
        {
            bytes += size_of::<std::sync::OnceLock<std::sync::Arc<crate::commands::Parsed>>>() + 16;
            bytes += cell.get().map_or(0, |p| p.held_bytes() + 16);
        }
    }
    // Memory once every line ran (a line that never runs keeps only its text).
    let case = format!("{variant}_{}", if distinct { "distinct" } else { "repeated" });
    bench_kit::report("function_memory", &case, "bytes_per_line_after_runs", bytes as f64 / raw.len() as f64);
    bench_kit::report("function_memory", &case, "load_ms_100k_lines", load_ms);
    bench_kit::report("function_memory", &case, "first_runs_parse_ms", first_run_ms);
}

#[test]
#[ignore = "measurement bench"]
fn bench_function_memory() {
    for distinct in [true, false] {
        memory_case("old", CommandPaths::OLD, distinct);
        memory_case("function_cache", CommandPaths::NEW, distinct);
    }
}

#[test]
#[ignore = "measurement bench"]
fn bench_command_parse() {
    for no_alloc in [false, true, false, true] {
        parse_case(no_alloc);
    }
}

#[test]
#[ignore = "measurement bench"]
fn bench_function_tick() {
    crate::mobs::index::set_verify(false);
    for sources in [1, 100, 1_000] {
        for mode in [1, 2, 3] {
            for (variant, paths) in variants(FUNCTION_LEADS) {
                function_case(sources, mode, variant, paths);
            }
        }
    }
}

#[test]
#[ignore = "measurement bench"]
fn bench_command_blocks() {
    crate::mobs::index::set_verify(false);
    for layout in [Blocks::OneChunk, Blocks::SixteenChunks, Blocks::Failing] {
        for (variant, paths) in variants(BLOCK_LEADS) {
            command_block_case(layout, variant, paths);
        }
    }
}

#[test]
#[ignore = "measurement bench"]
fn bench_schedule_queue() {
    let mut queue = CommandPaths::OLD;
    queue.schedule_queue = true;
    for waiting in [1_000, 10_000] {
        for (variant, paths) in [("old", CommandPaths::OLD), ("schedule_queue", queue)] {
            schedule_case(waiting, variant, paths);
        }
    }
}
