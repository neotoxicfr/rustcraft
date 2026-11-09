//! Equivalence proofs of the data, world and container-slot command switches
//! (`RUSTCRAFT_OPT_DATABORROW`, `MERGECHANGED`, `FILLPREDICATE`, `SLOTVIEW`, `BOSSBARDIRTY`) and of
//! the registry lookups through the name index. Each corpus runs on fresh copies of the flat world
//! (its own thread: the same test seeds), every switch off, each alone, then all on, by an operator
//! (replies in the chat) and by the silent server: the results, every event sent to the client and
//! the state after every command (storage and its save bytes, block entities, block states,
//! scheduled ticks, entities) are the same.

use std::path::Path;

use rc_proto::nbt::Nbt;

use super::cmds_data::{merge, merge_would_change};
use super::paths::CommandPaths;
use crate::blocks::probe::Probe;
use crate::items::nbt_eq;

// ---------- harness ----------

/// What a corpus records of the world after each command.
type Record = fn(&mut Probe, &mut Vec<String>);

struct Corpus {
    tag: &'static str,
    setup: &'static [&'static str],
    lines: &'static [&'static str],
    record: Record,
    /// Game ticks run after the setup (a furnace lit, a hopper settled).
    ticks: u32,
}

const MCMETA: &str = r#"{"pack":{"description":"dw","pack_format":121,"min_format":121,"max_format":121}}"#;

fn write_pack(world: &Path) {
    let pack = world.join("datapacks").join("dw");
    std::fs::create_dir_all(pack.join("data/dw/function")).expect("pack folder");
    std::fs::write(pack.join("pack.mcmeta"), MCMETA).expect("pack file");
    std::fs::write(pack.join("data/dw/function/mac.mcfunction"), "$say got $(c)\n").expect("pack file");
}

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3))
}

fn run(p: &mut Probe, c: &Corpus, log: &mut Vec<String>) {
    let server = |p: &mut Probe, line: &str| {
        let src = p.s.server_source();
        p.s.perform(src, line)
    };
    for line in c.setup {
        let r = server(p, line);
        log.push(format!("setup {line} -> {r:?}"));
    }
    for _ in 0..c.ticks {
        p.s.game_tick();
        p.s.tick += 1;
    }
    let _ = p.take_events();
    for line in c.lines {
        for who in ["operator", "server"] {
            let r = if who == "server" {
                server(p, line)
            } else {
                let mut src = p.s.player_source();
                src.level = 4;
                p.s.perform(src, line)
            };
            log.push(format!("{who} {line} -> {r:?}"));
            super::function_cache_tests::log_events(p, log);
            (c.record)(p, log);
        }
    }
}

fn trace(paths: CommandPaths, c: &'static Corpus) -> Vec<String> {
    let go = move || {
        CommandPaths::forced(paths, || {
            let (p, dir) = crate::tests_g3f13::open(c.tag);
            let data = p.s.data.clone();
            drop(p);
            write_pack(&dir);
            let mut p = crate::tests_g3f13::reopen(&dir, data);
            assert_eq!(p.s.cmd.paths, paths, "world opened on other paths");
            let mut log = Vec::new();
            run(&mut p, c, &mut log);
            crate::tests_g3f13::finish(p, dir);
            log
        })
    };
    std::thread::Builder::new()
        .stack_size(crate::server::SOLO_STACK)
        .spawn(go)
        .expect("thread")
        .join()
        .expect("world run")
}

fn only(set: impl Fn(&mut CommandPaths)) -> CommandPaths {
    let mut p = CommandPaths::OLD;
    set(&mut p);
    p
}

/// The corpus on every path: each switch of `leads` alone, then all on, against all off.
fn assert_paths_agree(c: &'static Corpus, leads: &[(&str, CommandPaths)], needles: &[&str]) -> Vec<String> {
    let reference = trace(CommandPaths::OLD, c);
    let text = reference.join("\n");
    for needle in needles {
        assert!(text.contains(needle), "{}: the corpus never produced {needle}", c.tag);
    }
    for (what, paths) in leads.iter().copied().chain([("every path", CommandPaths::NEW)]) {
        super::function_cache_tests::assert_same(&reference, &trace(paths, c), what);
    }
    reference
}

/// The armor stands' data (they are kept as compounds), in identifier order.
fn log_stands(p: &Probe, log: &mut Vec<String>) {
    let mut ids: Vec<i32> =
        p.s.entities
            .values()
            .flatten()
            .filter(|(_, n)| n.get("id").and_then(Nbt::as_str) == Some("minecraft:armor_stand"))
            .map(|(id, _)| *id)
            .collect();
    ids.sort_unstable();
    for id in ids {
        log.push(format!("stand {:?}", p.s.entity_data(super::ents::E::Other(id))));
    }
}

// ---------- /data, execute if|store data, function … with, nbt text ----------

const CHEST: [i32; 3] = [2, -60, 2];
const FURNACE: [i32; 3] = [4, -60, 2];

fn record_data(p: &mut Probe, log: &mut Vec<String>) {
    log.push(format!("storage {:?}", p.s.cmd.storage));
    // Taken like a save takes it: each command shows the namespaces it marked.
    let dirty: std::collections::BTreeSet<String> = std::mem::take(&mut p.s.cmd.storage_dirty).into_iter().collect();
    log.push(format!("dirty {dirty:?}"));
    // The save of every namespace, byte for byte.
    let all: std::collections::BTreeSet<String> =
        p.s.cmd.storage.keys().map(|k| super::cmds_data::storage_namespace(k).to_owned()).collect();
    for job in super::cmds_data::storage_jobs(Path::new("w"), &p.s.cmd.storage, &all) {
        if let crate::save::Job::Dat { path, nbt, .. } = job {
            log.push(format!("save {} {:016x}", path.display(), fnv(&nbt)));
        }
    }
    for at in [CHEST, FURNACE] {
        log.push(format!("block {at:?} {:?}", p.s.data_of_block(at)));
    }
    log_stands(p, log);
}

const DATA: Corpus = Corpus {
    tag: "dw-data",
    setup: &[
        "setblock 2 -60 2 minecraft:chest{Items:[{Slot:0b,id:\"minecraft:stone\",count:3}]}",
        "setblock 4 -60 2 minecraft:furnace{Items:[{Slot:0b,id:\"minecraft:raw_iron\",count:4},{Slot:1b,id:\"minecraft:coal\",count:2}]}",
        "summon minecraft:armor_stand 6.5 -60 2.5 {Tags:[\"d\"],NoGravity:1b}",
        "data merge storage dw:big {l:[1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16],m:{a:{b:{c:1}}}}",
    ],
    ticks: 30,
    lines: &[
        "data merge storage dw:s {a:1,b:{c:2,d:[1,2,3]},f:1.5f,z:0.0d,s:\"Zoé\"}",
        "data merge storage dw:s {a:1}",
        "data merge storage dw:s {b:{c:2}}",
        "data merge storage dw:s {b:{c:3}}",
        "data merge storage dw:s {z:-0.0d}",
        "data merge storage dw:s {z:-0.0d}",
        "data merge storage dw:s {b:{d:[1,2,3]}}",
        "data merge storage dw:s {b:{d:[3,2,1]}}",
        "data merge storage dw:new {}",
        "data merge storage dw:new {x:1}",
        "data get storage dw:s",
        "data get storage dw:s b.d",
        "data get storage dw:s b.d[1]",
        "data get storage dw:s b.d[]",
        "data get storage dw:s missing",
        "data get storage dw:s f 10",
        "data get storage dw:s s",
        "data get storage dw:absent",
        "data get storage dw:big l[-1]",
        "data remove storage dw:s b.d[0]",
        "data remove storage dw:s nothere",
        "data remove storage dw:new x",
        "data modify storage dw:s b.d append value 9",
        "data modify storage dw:s b.d prepend value 0",
        "data modify storage dw:s b.d insert 1 value 5",
        "data modify storage dw:s b.d insert 99 value 5",
        "data modify storage dw:s q.r.s set value 1",
        "data modify storage dw:s q.r set value 1",
        "data modify storage dw:s q.r set value 1",
        "data modify storage dw:s b merge value {e:1}",
        "data modify storage dw:s b merge value {e:1}",
        "data modify storage dw:s b merge from storage dw:big m",
        "data modify storage dw:s b merge from storage dw:big m",
        "data modify storage dw:s b merge from storage dw:big l[]",
        "data modify storage dw:s b.d set from storage dw:s b.d",
        "data modify storage dw:fail p.q[0] set value 1",
        "data modify storage dw:s a.x set value 1",
        "data modify storage dw:s t set string storage dw:s s 1",
        "data modify storage dw:s u set from storage dw:big",
        "data modify block 2 -60 2 Items[0].count set value 5b",
        "data merge block 2 -60 2 {CustomName:'\"Box\"'}",
        "data merge block 2 -60 2 {CustomName:'\"Box\"'}",
        "data get block 2 -60 2",
        "data get block 2 -60 2 Items[0].count",
        "data get block 4 -60 2 cooking_time_spent",
        "data merge entity @e[type=minecraft:armor_stand,limit=1] {Invisible:1b}",
        "data merge entity @e[type=minecraft:armor_stand,limit=1] {Invisible:1b}",
        "data get entity @e[type=minecraft:armor_stand,limit=1] Invisible",
        "data modify entity @e[type=minecraft:armor_stand,limit=1] Tags append value \"x\"",
        "data remove entity @e[type=minecraft:armor_stand,limit=1] Tags[0]",
        "execute if data storage dw:s b.d[]",
        "execute if data storage dw:s nope",
        "execute if data block 2 -60 2 Items[{id:\"minecraft:stone\"}]",
        "execute if data entity @e[type=minecraft:armor_stand,limit=1] Tags",
        "execute store result storage dw:s v int 1 run data get storage dw:s b.d[0]",
        "execute store result storage dw:new2 deep.path double 0.5 run data get storage dw:big l[3]",
        "execute store success storage dw:s a.bad byte 1 run data get storage dw:s a",
        "execute store result block 2 -60 2 Items[0].count byte 1 run data get storage dw:s a",
        "execute store result entity @e[type=minecraft:armor_stand,limit=1] Rotation[0] float 1 run data get storage dw:s a",
        "function dw:mac with storage dw:s b",
        "function dw:mac with storage dw:s",
        "function dw:mac with storage dw:s a",
        "function dw:mac with block 2 -60 2 Items[0]",
        "function dw:mac with entity @e[type=minecraft:armor_stand,limit=1]",
        "tellraw @a {\"nbt\":\"b\",\"storage\":\"dw:s\"}",
        "tellraw @a {\"nbt\":\"Items[0]\",\"block\":\"2 -60 2\"}",
        "tellraw @a {\"nbt\":\"Tags\",\"entity\":\"@e[type=minecraft:armor_stand]\"}",
        "tellraw @a {\"nbt\":\"b.d[]\",\"storage\":\"dw:s\",\"separator\":\"|\"}",
        "tellraw @a {\"nbt\":\"cooking_time_spent\",\"block\":\"4 -60 2\"}",
    ],
    record: record_data,
};

/// Sabotages seen failing (by hand): the in-place edit marking nothing to save; a failed edit
/// of a missing identifier keeping its fresh compound; a merge said to change whatever it holds.
#[test]
fn data_commands_are_the_same_on_every_path() {
    let leads = [
        ("data borrow", only(|p| p.data_borrow = true)),
        ("merge changed", only(|p| p.merge_changed = true)),
        (
            "borrow and merge",
            only(|p| {
                p.data_borrow = true;
                p.merge_changed = true;
            }),
        ),
    ];
    let needles = [
        "commands.data.merge.failed",
        "commands.data.storage.modified",
        "commands.data.storage.query",
        "commands.data.get.multiple",
        "commands.function.error.argument_not_compound",
        "arguments.nbtpath.nothing_found",
        "dw:fail",
        "got 3",
    ];
    assert_paths_agree(&DATA, &leads, &needles);
}

/// A random compound of small depth: keys drawn from a few, values of every kind, reals with
/// both zeros, NaN and close values.
fn draw(rng: &mut u64, n: u32) -> u32 {
    *rng = rng.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
    ((*rng >> 33) as u32) % n
}

fn random_compound(rng: &mut u64, depth: u32) -> Nbt {
    let n = draw(rng, 5);
    let mut fields: Vec<(String, Nbt)> = Vec::new();
    for _ in 0..n {
        let key = ["a", "b", "c", "d"][draw(rng, 4) as usize].to_owned();
        let value = match draw(rng, 9) {
            0 if depth > 0 => random_compound(rng, depth - 1),
            1 => Nbt::Double([0.0, -0.0, f64::NAN, 1.0][draw(rng, 4) as usize]),
            2 => Nbt::Float([0.0, -0.0, f32::NAN, 1.0][draw(rng, 4) as usize]),
            3 => Nbt::List(vec![Nbt::Int(draw(rng, 2) as i32)]),
            4 => Nbt::Long(i64::from(draw(rng, 2))),
            5 => Nbt::String(["x", "y"][draw(rng, 2) as usize].to_owned()),
            6 => Nbt::IntArray(vec![draw(rng, 2) as i32]),
            _ => Nbt::Int(draw(rng, 3) as i32),
        };
        // One entry per key, like an SNBT compound (the last value of a key wins).
        fields.retain(|(k, _)| *k != key);
        fields.push((key, value));
    }
    Nbt::Compound(fields)
}

/// The leaf test answers what comparing the compound before and after the merge answers.
#[test]
fn merge_change_read_from_the_leaves_matches_the_whole_comparison() {
    let mut rng = 0x2545_f491_4f6c_dd1du64;
    let mut changed = 0;
    for _ in 0..200_000 {
        let base = random_compound(&mut rng, 2);
        let add = random_compound(&mut rng, 2);
        let mut merged = base.clone();
        merge(&mut merged, &add);
        let whole = !nbt_eq(&base, &merged);
        assert_eq!(merge_would_change(&base, &add), whole, "{base:?} + {add:?}");
        changed += usize::from(whole);
        let mut leafwise = base.clone();
        assert_eq!(super::cmds_data::merge_if_changes(&mut leafwise, &add), whole);
        assert_eq!(format!("{leafwise:?}"), format!("{:?}", if whole { &merged } else { &base }), "merged bits");
    }
    assert!((20_000..180_000).contains(&changed), "{changed} changes: the draws do not test both answers");
}

// ---------- fill and clone with a block filter, execute if blocks ----------

fn record_world(p: &mut Probe, log: &mut Vec<String>) {
    let mut states = Vec::new();
    for z in 8..=36 {
        for y in -61..=-56 {
            for x in 0..=60 {
                states.extend_from_slice(&p.s.get([x, y, z]).to_le_bytes());
                if let Some(d) = p.s.data_of_block([x, y, z]) {
                    log.push(format!("data {x} {y} {z} {d:?}"));
                }
            }
        }
    }
    log.push(format!("states {:016x}", fnv(&states)));
    let mut ticks = p.s.bs.block_ticks.in_box([0, -64, 8], [60, -50, 36]);
    ticks.sort();
    log.push(format!("ticks {ticks:?}"));
}

const FILL: Corpus = Corpus {
    tag: "dw-fill",
    setup: &[
        "fill 0 -60 10 15 -58 25 minecraft:stone",
        "fill 2 -60 12 4 -59 14 minecraft:oak_stairs[facing=east]",
        "fill 2 -58 12 4 -58 14 minecraft:oak_stairs[facing=west,half=top]",
        "fill 5 -60 12 6 -59 13 minecraft:oak_planks",
        "fill 5 -58 12 6 -58 13 minecraft:birch_planks",
        "setblock 7 -60 12 minecraft:chest{Items:[{Slot:0b,id:\"minecraft:apple\",count:1}]}",
        "setblock 8 -60 12 minecraft:chest",
        "setblock 9 -60 12 minecraft:barrel{CustomName:'\"x\"'}",
        "setblock 10 -60 12 minecraft:oak_log[axis=x]",
        "setblock 10 -59 12 minecraft:birch_log[axis=z]",
        "setblock 11 -60 12 minecraft:glass",
        "setblock 12 -59 12 minecraft:redstone_wire",
        "setblock 13 -58 12 minecraft:air",
        "setblock 13 -58 13 minecraft:sand",
    ],
    ticks: 0,
    lines: &[
        "fill 0 -60 10 15 -58 25 minecraft:dirt replace minecraft:stone",
        "fill 0 -60 10 15 -58 25 minecraft:cobblestone replace #minecraft:planks",
        "fill 0 -60 10 15 -58 25 minecraft:acacia_stairs replace minecraft:oak_stairs[facing=east]",
        "fill 0 -60 10 15 -58 25 minecraft:air replace minecraft:oak_stairs[facing=east,half=top]",
        "fill 0 -60 10 15 -58 25 minecraft:spruce_stairs replace minecraft:oak_stairs[half=top]",
        "fill 0 -60 10 15 -58 25 minecraft:stone replace minecraft:chest{Items:[{id:\"minecraft:apple\"}]}",
        "fill 0 -60 10 15 -58 25 minecraft:oak_log[axis=y] replace #minecraft:logs",
        "fill 0 -60 10 15 -58 25 minecraft:birch_log replace #logs",
        "fill 0 -60 10 15 -58 25 minecraft:glass replace #minecraft:not_a_tag",
        "fill 0 -60 10 15 -58 25 minecraft:glass replace minecraft:not_a_block",
        "fill 1 -60 26 5 -58 30 minecraft:glass outline",
        "fill 1 -60 26 5 -58 30 minecraft:stone hollow",
        "fill 1 -60 26 5 -58 30 minecraft:dirt keep",
        "fill 1 -60 26 5 -58 30 minecraft:air destroy",
        "fill 1 -60 26 5 -58 30 minecraft:oak_stairs strict",
        "fill 1 -60 26 5 -58 30 minecraft:oak_stairs[facing=north]",
        "fill 1 -60 26 3 -60 28 minecraft:barrier",
        "fill 0 -60 10 15 -58 25 minecraft:chest{Items:[{Slot:1b,id:\"minecraft:stone\",count:2}]} replace minecraft:dirt",
        "clone 0 -60 10 15 -58 25 20 -60 10 filtered #minecraft:planks",
        "clone 0 -60 10 15 -58 25 20 -60 10 filtered minecraft:chest",
        "clone 0 -60 10 15 -58 25 20 -60 10 filtered minecraft:chest{Items:[{Slot:1b}]}",
        "clone 0 -60 10 15 -58 25 40 -60 10 filtered minecraft:acacia_stairs[facing=east] move",
        "clone 0 -60 10 15 -58 25 20 -60 30 masked",
        "clone 0 -60 10 15 -58 25 20 -60 30 filtered minecraft:glass strict",
        "execute if blocks 0 -60 10 15 -58 25 20 -60 10 all",
        "execute if blocks 0 -60 10 15 -58 25 20 -60 10 masked",
        "execute if blocks 20 -60 10 35 -58 25 20 -60 10 all",
        // One block, then a box of one kind: the filter answers without its table of states.
        "fill 5 -58 12 5 -58 12 minecraft:air replace #minecraft:planks",
        "fill 5 -58 12 5 -58 12 minecraft:dirt replace #minecraft:planks",
        "fill 0 -60 33 15 -60 34 minecraft:sand replace minecraft:air",
    ],
    record: record_world,
};

/// Sabotages seen failing (by hand): the filter's properties ignored (the east stairs line);
/// a tag read under one of its two names only (`#logs` or `#minecraft:logs`).
#[test]
fn fill_and_clone_filters_are_the_same_on_every_path() {
    // `execute if blocks` reads its destination's data under the data borrow.
    let leads =
        [("fill predicate", only(|p| p.fill_predicate = true)), ("data borrow", only(|p| p.data_borrow = true))];
    let needles = ["commands.fill.success", "commands.fill.failed", "commands.clone.success", "commands.clone.failed"];
    assert_paths_agree(&FILL, &leads, &needles);
}

/// A filter that only ever meets one kind of block (a one-block `fill` run every tick) answers
/// without building its 30 KB table of states; a second kind builds it, and every answer is the
/// one of the text read at each block. Sabotage seen failing: the table built when the filter is
/// read.
#[test]
fn one_kind_of_block_needs_no_state_table() {
    let (mut p, dir) = crate::tests_g3f13::open("dw-pred-table");
    for c in ["setblock 0 -60 0 minecraft:oak_planks", "setblock 1 -60 0 minecraft:stone"] {
        let src = p.s.server_source();
        p.s.perform(src, c);
    }
    let (planks, stone) = ([0, -60, 0], [1, -60, 0]);
    for text in ["#minecraft:planks", "#planks", "minecraft:oak_planks", "minecraft:stone", "#minecraft:not_a_tag"] {
        let mut pred = super::args::BlockPred::new(text);
        for _ in 0..3 {
            assert_eq!(p.s.block_pred_matches(&mut pred, planks), p.s.block_matches(text, planks), "{text}");
        }
        assert!(!pred.has_state_table(), "{text}: one kind of block");
        for at in [stone, planks, stone] {
            assert_eq!(p.s.block_pred_matches(&mut pred, at), p.s.block_matches(text, at), "{text} {at:?}");
        }
        assert!(pred.has_state_table(), "{text}: two kinds of block");
    }
    crate::tests_g3f13::finish(p, dir);
}

// ---------- container slots: item, loot, execute if items ----------

fn record_slots(p: &mut Probe, log: &mut Vec<String>) {
    for x in (2..=20).step_by(2) {
        let at = [x, -60, 20];
        log.push(format!("{x} {:?}", p.s.data_of_block(at)));
        // The comparator behind each container.
        log.push(format!("{x} comparator {}", p.s.get([x, -60, 21])));
    }
    log_stands(p, log);
}

const SLOTS: Corpus = Corpus {
    tag: "dw-slots",
    setup: &[
        "setblock 2 -60 20 minecraft:chest{Items:[{Slot:0b,id:\"minecraft:stone\",count:3},{Slot:5b,id:\"minecraft:dirt\",count:64},{Slot:5b,id:\"minecraft:apple\",count:1},{Slot:7b,id:\"minecraft:not_an_item\",count:1}]}",
        "setblock 4 -60 20 minecraft:barrel{Items:[{Slot:2b,id:\"minecraft:bread\",count:5}]}",
        "setblock 6 -60 20 minecraft:shulker_box{Items:[{Slot:0b,id:\"minecraft:stone\",count:60}]}",
        "setblock 8 -60 20 minecraft:furnace{Items:[{Slot:0b,id:\"minecraft:raw_iron\",count:8},{Slot:1b,id:\"minecraft:coal\",count:8}]}",
        "setblock 10 -60 20 minecraft:chest{LootTable:\"minecraft:chests/simple_dungeon\",LootTableSeed:7L}",
        "setblock 12 -60 20 minecraft:jukebox",
        "setblock 14 -60 20 minecraft:oak_shelf{Items:[{Slot:1b,id:\"minecraft:dirt\",count:64},{Slot:1b,id:\"minecraft:apple\",count:1}]}",
        "setblock 16 -60 20 minecraft:chiseled_bookshelf",
        "setblock 18 -60 20 minecraft:hopper",
        "setblock 20 -60 20 minecraft:chest{LootTable:\"minecraft:chests/simple_dungeon\",LootTableSeed:9L}",
        "fill 2 -60 21 20 -60 21 minecraft:comparator[facing=north]",
        "summon minecraft:armor_stand 22.5 -60 20.5 {NoGravity:1b,Tags:[\"s\"]}",
    ],
    ticks: 40,
    lines: &[
        "execute if items block 2 -60 20 container.* minecraft:stone",
        "execute if items block 2 -60 20 container.* *",
        "execute if items block 2 -60 20 container.5 minecraft:apple",
        "execute if items block 10 -60 20 container.* *",
        "execute if items block 8 -60 20 container.* *",
        "execute if items block 12 -60 20 container.0 *",
        "item modify block 2 -60 20 container.* {type:\"minecraft:set_count\",count:2}",
        "item modify block 4 -60 20 container.* {type:\"minecraft:set_count\",count:{type:\"minecraft:uniform\",min:1,max:9}}",
        "item modify block 20 -60 20 container.* {type:\"minecraft:set_count\",count:1,add:true}",
        "item replace block 2 -60 20 container.1 from block 4 -60 20 container.2",
        "item replace block 6 -60 20 container.0 from block 2 -60 20 container.0",
        "item replace block 8 -60 20 container.0 with minecraft:raw_gold 3",
        "item replace block 12 -60 20 container.0 with minecraft:music_disc_cat",
        "item replace block 16 -60 20 container.2 with minecraft:book",
        "item replace block 14 -60 20 container.1 with minecraft:apple 3",
        "item replace block 14 -60 20 container.2 with minecraft:apple 3",
        "item replace entity @e[type=minecraft:armor_stand,limit=1] armor.head from block 2 -60 20 container.0",
        "item modify entity @e[type=minecraft:armor_stand,limit=1] armor.head {type:\"minecraft:set_count\",count:1}",
        "loot insert 2 -60 20 loot minecraft:chests/simple_dungeon",
        "loot insert 4 -60 20 loot minecraft:chests/simple_dungeon",
        "loot insert 6 -60 20 loot minecraft:chests/simple_dungeon",
        "loot insert 18 -60 20 loot minecraft:chests/simple_dungeon",
        "loot insert 14 -60 20 loot minecraft:chests/simple_dungeon",
        "loot replace block 6 -60 20 container.3 loot minecraft:chests/simple_dungeon",
        "loot replace block 16 -60 20 container.0 2 loot minecraft:chests/simple_dungeon",
        "loot insert 2 -60 20 loot minecraft:chests/simple_dungeon",
        "execute if items block 2 -60 20 container.* *",
    ],
    record: record_slots,
};

/// Sabotages seen failing (by hand): a slot read up to its first entry (the chest's slot 5 holds
/// a dirt stack then an apple); the view not read again after a write.
#[test]
fn container_slot_commands_are_the_same_on_every_path() {
    let leads = [("slot view", only(|p| p.slot_view = true))];
    let needles =
        ["commands.item.block.modify.success", "commands.drop.success", "commands.execute.conditional.pass_count"];
    let reference = assert_paths_agree(&SLOTS, &leads, &needles);
    // The duplicate slot reads as its last valid entry: the apple.
    let text = reference.join("\n");
    assert!(
        text.contains("operator execute if items block 2 -60 20 container.5 minecraft:apple -> (true, 1)"),
        "{text}"
    );
}

/// One slot read alone gives what the whole list's read gives: the last valid entry of the slot
/// (a later entry overwrites; an unreadable one is skipped), nothing past the size.
#[test]
fn one_slot_read_alone_matches_the_whole_list() {
    let be = super::snbt::parse(
        "{Items:[{Slot:0b,id:\"minecraft:stone\",count:3},{Slot:5b,id:\"minecraft:dirt\",count:64},\
         {Slot:5b,id:\"minecraft:apple\",count:1},{Slot:5b},{Slot:7b,id:\"minecraft:bread\",count:2},\
         {id:\"minecraft:apple\",count:1},{Slot:30b,id:\"minecraft:apple\",count:1},{Slot:-1b,id:\"minecraft:apple\"}]}",
    )
    .expect("block entity");
    for size in [0, 5, 6, 27, 54] {
        let whole = crate::containers::read_items(&be, size);
        for i in 0..64 {
            let alone = crate::containers::read_item_at(&be, i, size);
            assert_eq!(alone, whole.get(i).cloned().flatten(), "slot {i} of {size}");
        }
    }
    assert_eq!(crate::containers::read_item_at(&be, 5, 27).map(|s| s.id), Some("minecraft:apple".to_owned()));
}

// ---------- boss bars marked to save ----------

#[test]
fn an_unchanged_boss_bar_marks_nothing_to_save() {
    for (dirty_only_on_change, unchanged_marks) in [(true, false), (false, true)] {
        let paths = only(|p| p.bossbar_dirty = dirty_only_on_change);
        let go = move || {
            CommandPaths::forced(paths, || {
                let (mut p, dir) = crate::tests_g3f13::open("dw-bars");
                // Whether the command marked the bars to save (the mark is taken: a save).
                let marks = |p: &mut Probe, c: &str| {
                    let src = p.s.server_source();
                    p.s.perform(src, c);
                    std::mem::take(&mut p.s.cmd.bars_dirty)
                };
                assert!(marks(&mut p, "bossbar add dw:b \"B\""), "a new bar is saved");
                assert!(!marks(&mut p, "data merge storage dw:x {v:7}"));
                assert!(marks(&mut p, "bossbar set dw:b value 5"), "a change is saved");
                assert_eq!(marks(&mut p, "bossbar set dw:b value 5"), unchanged_marks, "unchanged set");
                assert_eq!(marks(&mut p, "bossbar set dw:missing value 5"), unchanged_marks, "unknown bar");
                assert!(!marks(&mut p, "bossbar get dw:b value"), "a query");
                assert!(marks(&mut p, "execute store result bossbar dw:b value run data get storage dw:x v"));
                let again = marks(&mut p, "execute store result bossbar dw:b value run data get storage dw:x v");
                assert_eq!(again, unchanged_marks, "unchanged store");
                assert!(marks(&mut p, "bossbar remove dw:b"));
                crate::tests_g3f13::finish(p, dir);
            })
        };
        std::thread::Builder::new()
            .stack_size(crate::server::SOLO_STACK)
            .spawn(go)
            .expect("thread")
            .join()
            .expect("run");
    }
}

// ---------- registry lookups through the name index ----------

/// Every entry (and absent names) of the five registries the commands read: the index gives the
/// first position of the walk, on both lookup paths of `RUSTCRAFT_OPT_NAMEINDEX`.
#[test]
fn registry_lookups_give_the_first_position_of_the_walk() {
    let data = rc_testdata::version_data().expect("26.3 version data missing");
    let known = [
        ("minecraft:item", data.items()),
        ("minecraft:entity_type", data.entity_types()),
        ("minecraft:particle_type", data.particle_types()),
        ("minecraft:block_entity_type", data.block_entity_types()),
        ("minecraft:mob_effect", data.mob_effects()),
    ];
    for (name, reg) in known {
        let walked = data.registry(name).expect("registry");
        let reg = reg.expect("known registry");
        let absent = ["", "minecraft:", "minecraft:not_there", "stone", "MINECRAFT:STONE"].map(str::to_owned);
        for n in walked.entries.iter().chain(absent.iter()) {
            let first = walked.entries.iter().position(|e| e == n).map(|i| i as u32);
            assert_eq!(reg.id(n), first, "{name} {n}");
            assert_eq!(reg.id_with(n, true), first, "{name} {n}, indexed");
            assert_eq!(reg.id_with(n, false), first, "{name} {n}, walked");
        }
    }
    // The item table side by side with the registry: an index past it finds nothing.
    let items = data.registry("minecraft:item").expect("items");
    for id in items.entries.iter().map(String::as_str).chain(["minecraft:not_there"]) {
        let walked = data.items.iter().zip(&items.entries).find(|(_, n)| *n == id).map(|(i, _)| i.name_key.clone());
        let indexed =
            data.items().and_then(|r| r.id(id)).and_then(|i| data.items.get(i as usize)).map(|i| i.name_key.clone());
        assert_eq!(walked, indexed, "{id}");
    }
    for (i, b) in data.blocks.iter().enumerate() {
        assert_eq!(data.block_id(&b.name), data.blocks.iter().position(|x| x.name == b.name).map(|j| j as u32), "{i}");
    }
}

/// `spreadplayers`' landing test: the tag read by the block's index answers what the tag read by
/// the block's name answers, for every block.
#[test]
fn landing_blocks_are_read_by_index_like_by_name() {
    let (p, dir) = crate::tests_g3f13::open("dw-tags");
    for (i, b) in p.s.data.blocks.iter().enumerate() {
        let by_name = p.s.p_tag("minecraft:block", "minecraft:entities_can_teleport_to", &b.name);
        let by_index = p.s.tag_id(crate::tag_bits::TagId::EntitiesCanTeleportTo, i as u16);
        assert_eq!(by_name, by_index, "{}", b.name);
    }
    crate::tests_g3f13::finish(p, dir);
}
