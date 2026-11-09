//! Micro-bench of the data, world and container-slot command switches (`commands::paths`), each
//! lead against the former path, on the flat world:
//! - replies of a silent source: `tag`, `effect`, `scoreboard players add` over 100 pigs and
//!   `kill` of 500 items (`RUSTCRAFT_OPT_LAZYFEEDBACK`);
//! - a storage of 10,000 elements: `append`, `execute if data`, `data merge` on 10,000 keys,
//!   `function … with storage` (`RUSTCRAFT_OPT_DATABORROW`, `RUSTCRAFT_OPT_MERGECHANGED`);
//! - `fill` of a 32³ box with a filter that keeps few blocks, then every block
//!   (`RUSTCRAFT_OPT_FILLPREDICATE`);
//! - a full chest (27 slots) and a dispenser (9): `execute if items`, `item modify`, `loot insert`
//!   (`RUSTCRAFT_OPT_SLOTVIEW`);
//! - `/particle dust{…}` and the registry lookups behind it (`RUSTCRAFT_OPT_NAMEINDEX`).
//!
//! One thread: a command reads and writes the world in order on the server thread (no multi-core
//! or GPU variant). Run alone:
//! `cargo test --release -p rc-sim [--features memstats] bench_data_world_slot_commands:: -- --ignored --nocapture --test-threads=1`.

use crate::bench_kit::{self, cmd};
use crate::blocks::probe::Probe;
use crate::commands::paths::CommandPaths;

const RUNS: usize = 5;

/// Flat world opened on `paths` (every other switch on: the lead is measured in today's code).
fn world(tag: &str, paths: CommandPaths) -> (Probe, std::path::PathBuf) {
    CommandPaths::forced(paths, || {
        let (p, dir) = crate::tests_g3f13::open(tag);
        assert_eq!(p.s.cmd.paths, paths);
        (p, dir)
    })
}

/// µs and allocations per command of `reps` runs of `command` by the silent server, `prepare`
/// run (unmeasured) before each; the medians of `RUNS` runs are reported.
fn time_command(p: &mut Probe, bench: &str, case: &str, command: &str, reps: u32, prepare: &dyn Fn(&mut Probe)) {
    let (mut us, mut allocs) = (Vec::new(), Vec::new());
    for _ in 0..RUNS {
        let (mut spent, mut count) = (std::time::Duration::ZERO, 0u64);
        for _ in 0..reps {
            prepare(p);
            let src = p.s.server_source();
            let (start, a) = (std::time::Instant::now(), bench_kit::allocations());
            std::hint::black_box(p.s.perform(src, command));
            spent += start.elapsed();
            count += bench_kit::allocations() - a;
            let _ = p.take_events();
        }
        us.push(spent.as_secs_f64() * 1e6 / f64::from(reps));
        allocs.push(count as f64 / f64::from(reps));
    }
    bench_kit::report(bench, case, "us_per_command", bench_kit::median(&us));
    if cfg!(feature = "memstats") {
        bench_kit::report(bench, case, "allocs_per_command", bench_kit::median(&allocs));
    }
}

fn with(set: impl Fn(&mut CommandPaths)) -> CommandPaths {
    let mut p = CommandPaths::NEW;
    set(&mut p);
    p
}

// ---------- replies of a silent source ----------

fn feedback_case(variant: &str, paths: CommandPaths) {
    let (mut p, dir) = world(&format!("bf-{variant}"), paths);
    cmd(&mut p, "scoreboard objectives add bf dummy");
    for i in 0..100 {
        cmd(&mut p, &format!("summon minecraft:pig {} -60 {} {{NoAI:1b}}", 2 + i % 20, 2 + i / 20));
    }
    let none: &dyn Fn(&mut Probe) = &|_| {};
    for (case, command) in [
        ("tag_100", "tag @e[type=minecraft:pig] add seen"),
        ("tag_list_100", "tag @e[type=minecraft:pig] list"),
        ("effect_100", "effect give @e[type=minecraft:pig] minecraft:speed 5 0 true"),
        ("score_add_100", "scoreboard players add @e[type=minecraft:pig] bf 1"),
        ("tp_one", "tp @e[type=minecraft:pig,limit=1] 2.5 -60 2.5"),
    ] {
        time_command(&mut p, "command_feedback", &format!("{case}_{variant}"), command, 200, none);
    }
    // 500 items summoned again (unmeasured) before each kill.
    let summon_items = |p: &mut Probe| {
        for i in 0..500 {
            cmd(
                p,
                &format!(
                    "summon minecraft:item {} -60 {} {{Item:{{id:\"minecraft:stone\",count:1}},PickupDelay:32767}}",
                    30 + i % 25,
                    2 + i / 25
                ),
            );
        }
    };
    time_command(
        &mut p,
        "command_feedback",
        &format!("kill_500_items_{variant}"),
        "kill @e[type=minecraft:item]",
        4,
        &summon_items,
    );
    crate::tests_g3f13::finish(p, dir);
}

#[test]
#[ignore = "measurement bench"]
fn bench_command_feedback() {
    crate::mobs::index::set_verify(false);
    for _ in 0..2 {
        feedback_case("old", with(|p| p.lazy_feedback = false));
        feedback_case("lazy", CommandPaths::NEW);
    }
}

// ---------- a large storage ----------

fn storage_case(variant: &str, paths: CommandPaths) {
    let (p, dir) = world(&format!("bs-{variant}"), paths);
    let pack = dir.join("datapacks").join("bs");
    std::fs::create_dir_all(pack.join("data/bs/function")).expect("pack folder");
    std::fs::write(
        pack.join("pack.mcmeta"),
        r#"{"pack":{"description":"bs","pack_format":121,"min_format":121,"max_format":121}}"#,
    )
    .expect("pack");
    std::fs::write(pack.join("data/bs/function/f.mcfunction"), "$scoreboard players set #v bs $(v)\n").expect("pack");
    let data = p.s.data.clone();
    drop(p);
    let mut p = CommandPaths::forced(paths, || crate::tests_g3f13::reopen(&dir, data));
    cmd(&mut p, "scoreboard objectives add bs dummy");
    // 10,000 list elements and 10,000 keys, beside a small argument compound.
    let list: Vec<String> = (0..10_000).map(|i| i.to_string()).collect();
    cmd(&mut p, &format!("data merge storage bs:s {{list:[{}],args:{{v:1}}}}", list.join(",")));
    let keys: Vec<String> = (0..10_000).map(|i| format!("k{i}:{i}")).collect();
    cmd(&mut p, &format!("data merge storage bs:k {{{}}}", keys.join(",")));
    let none: &dyn Fn(&mut Probe) = &|_| {};
    for (case, command) in [
        ("append", "data modify storage bs:s list append value 1"),
        ("if_data", "execute if data storage bs:s list[0]"),
        ("get_path", "data get storage bs:s args.v"),
        ("function_with_storage", "function bs:f with storage bs:s args"),
        ("store_result", "execute store result storage bs:s args.v int 1 run scoreboard players get #v bs"),
        ("merge_unchanged_10k_keys", "data merge storage bs:k {k5:5}"),
    ] {
        time_command(&mut p, "storage", &format!("{case}_{variant}"), command, 100, none);
    }
    // A merge that changes: the value flips between two numbers.
    let flip = std::cell::Cell::new(false);
    let toggle = |p: &mut Probe| {
        flip.set(!flip.get());
        // Never 7 (the merged value) and never the value it holds: the set always changes it.
        let v = if flip.get() { 100 } else { 101 };
        cmd(p, &format!("data modify storage bs:k k6 set value {v}"));
    };
    time_command(
        &mut p,
        "storage",
        &format!("merge_changed_10k_keys_{variant}"),
        "data merge storage bs:k {k6:7}",
        100,
        &toggle,
    );
    crate::tests_g3f13::finish(p, dir);
}

#[test]
#[ignore = "measurement bench"]
fn bench_storage() {
    for _ in 0..2 {
        storage_case(
            "old",
            with(|p| {
                p.data_borrow = false;
                p.merge_changed = false;
            }),
        );
        storage_case("borrow", with(|p| p.merge_changed = false));
        storage_case("merge_changed", with(|p| p.data_borrow = false));
        storage_case("all", CommandPaths::NEW);
    }
}

// ---------- fill with a filter, 32³ ----------

fn fill_case(variant: &str, paths: CommandPaths) {
    let (mut p, dir) = world(&format!("bfill-{variant}"), paths);
    cmd(&mut p, "fill 0 -60 0 31 -29 31 minecraft:stone");
    // A few dirt blocks: the filter keeps 64 of 32,768.
    for i in 0..64 {
        cmd(&mut p, &format!("setblock {} {} {} minecraft:dirt", i % 32, -60 + i / 4, (i * 7) % 32));
    }
    let box_ = "0 -60 0 31 -29 31";
    // Few kept: the 64 dirt blocks turned to glass (unmeasured), then back; the predicate is
    // tested 32,768 times, 64 blocks placed.
    let to_glass = |p: &mut Probe| cmd(p, &format!("fill {box_} minecraft:glass replace minecraft:dirt"));
    let few = format!("fill {box_} minecraft:dirt replace minecraft:glass");
    time_command(&mut p, "fill_filter", &format!("keeps_few_{variant}"), &few, 20, &to_glass);
    // A tag and properties that keep nothing: the predicate alone.
    let tag = format!("fill {box_} minecraft:dirt replace #minecraft:wool");
    time_command(&mut p, "fill_filter", &format!("keeps_none_tag_{variant}"), &tag, 20, &|_| {});
    let props = format!("fill {box_} minecraft:air replace minecraft:oak_stairs[facing=east]");
    time_command(&mut p, "fill_filter", &format!("keeps_none_props_{variant}"), &props, 20, &|_| {});
    // Every block kept: the stone turned to andesite (unmeasured), then back; the placement dominates.
    let to_andesite = |p: &mut Probe| cmd(p, &format!("fill {box_} minecraft:andesite replace minecraft:stone"));
    let all = format!("fill {box_} minecraft:stone replace minecraft:andesite");
    time_command(&mut p, "fill_filter", &format!("keeps_all_{variant}"), &all, 4, &to_andesite);
    crate::tests_g3f13::finish(p, dir);
}

#[test]
#[ignore = "measurement bench"]
fn bench_fill_filter() {
    for _ in 0..2 {
        fill_case("old", with(|p| p.fill_predicate = false));
        fill_case("predicate", CommandPaths::NEW);
    }
}

// ---------- container slots ----------

fn slots_case(variant: &str, paths: CommandPaths) {
    let (mut p, dir) = world(&format!("bslots-{variant}"), paths);
    let items = |n: usize| -> String {
        (0..n)
            .map(|i| format!("{{Slot:{i}b,id:\"minecraft:diamond_sword\",count:1,components:{{\"minecraft:custom_name\":\"s{i}\",\"minecraft:damage\":{i}}}}}"))
            .collect::<Vec<_>>()
            .join(",")
    };
    // The container placed again with `filled` stacks (the air first: a refused setblock is fine).
    let place = |p: &mut Probe, block: &str, filled: usize| {
        let src = p.s.server_source();
        let _ = p.s.perform(src, "setblock 4 -60 4 minecraft:air");
        cmd(p, &format!("setblock 4 -60 4 minecraft:{block}{{Items:[{}]}}", items(filled)));
    };
    for (name, block, size) in [("chest_27", "chest", 27), ("dispenser_9", "dispenser", 9)] {
        let at = "4 -60 4";
        place(&mut p, block, size);
        let none: &dyn Fn(&mut Probe) = &|_| {};
        time_command(
            &mut p,
            "container_slots",
            &format!("if_items_{name}_{variant}"),
            &format!("execute if items block {at} container.* *"),
            200,
            none,
        );
        // A random damage: (almost) every slot changes at every run.
        let modify = format!(
            "item modify block {at} container.* {{type:\"minecraft:set_damage\",damage:{{type:\"minecraft:uniform\",min:0.0,max:1.0}}}}"
        );
        time_command(&mut p, "container_slots", &format!("item_modify_{name}_{variant}"), &modify, 100, none);
        // Room again before each insert (the last three slots empty).
        let room = |p: &mut Probe| place(p, block, size - 3);
        time_command(
            &mut p,
            "container_slots",
            &format!("loot_insert_{name}_{variant}"),
            &format!("loot insert {at} loot minecraft:chests/simple_dungeon"),
            100,
            &room,
        );
    }
    crate::tests_g3f13::finish(p, dir);
}

#[test]
#[ignore = "measurement bench"]
fn bench_container_slots() {
    for _ in 0..2 {
        slots_case("old", with(|p| p.slot_view = false));
        slots_case("view", CommandPaths::NEW);
    }
}

// ---------- /particle and registry lookups ----------

#[test]
#[ignore = "measurement bench"]
fn bench_particle_options() {
    for (variant, paths) in [("old_feedback", with(|p| p.lazy_feedback = false)), ("lazy", CommandPaths::NEW)] {
        let (mut p, dir) = world(&format!("bpart-{variant}"), paths);
        let command = "particle minecraft:dust{color:[1.0,0.0,0.0],scale:1.0} 0 -59 0 0 0 0 0 1";
        time_command(&mut p, "particle", &format!("dust_{variant}"), command, 10_000, &|_| {});
        crate::tests_g3f13::finish(p, dir);
    }
    // The lookups alone: a walk of the registry entries (former) against the name index.
    let data = rc_testdata::version_data().expect("26.3 version data missing");
    let names = ["minecraft:dust", "minecraft:flame", "minecraft:witch"];
    let items = ["minecraft:stone", "minecraft:diamond_sword", "minecraft:mace"];
    for (variant, indexed) in [("walk", false), ("index", true)] {
        let (particles, item_reg) = (data.particle_types().expect("particles"), data.items().expect("items"));
        let start = std::time::Instant::now();
        for i in 0..100_000 {
            std::hint::black_box(particles.id_with(names[i % 3], indexed));
            std::hint::black_box(item_reg.id_with(items[i % 3], indexed));
        }
        bench_kit::report("registry_lookup", variant, "ns_per_pair", start.elapsed().as_secs_f64() * 1e9 / 100_000.0);
    }
}
