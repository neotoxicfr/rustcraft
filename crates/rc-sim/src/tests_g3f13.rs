//! G3-F13: order of the world tick (phases, entities before block entities, one block-entity
//! list in insertion order), entities frozen outside the "ticking entities" chunks, entities
//! born during the entity walk, spectators without the chunk rule, block-entity state kept
//! with its dimension. Fresh flat world (no test world required; 26.3 version data mandatory).

use std::path::PathBuf;
use std::sync::Arc;

use rc_proto::nbt::Nbt;

use crate::blocks::probe::Probe;
use crate::newworld::{NewWorld, Preset, create_world};
use crate::player::Stack;

/// Flat world, player standing at (0.5, -60, 0.5), chunks -2..=9 × -2..=2 loaded.
pub(crate) fn open(tag: &str) -> (Probe, PathBuf) {
    let data = Arc::new(rc_testdata::version_data().expect("26.3 version data missing"));
    assert!(crate::registries::client_jar(&data).is_some(), "26.3 client jar missing");
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-g3f13-{tag}-{}", std::process::id())));
    let mut w = NewWorld::new(tag);
    w.seed = 1;
    w.preset = Preset::Flat;
    create_world(&dir, &data, &w).expect("world creation");
    let mut p = Probe::open(&dir, data);
    p.s.pos = [0.5, -60.0, 0.5];
    p.s.loaded = true;
    p.s.game_mode = 0;
    p.s.teleport = None;
    p.s.view = 8;
    let around: Vec<(i32, i32)> = (-2..=9).flat_map(|x| (-2..=2).map(move |z| (x, z))).collect();
    p.s.load_now(&around);
    cmd(&mut p, "gamerule minecraft:spawn_mobs false");
    (p, dir)
}

pub(crate) fn finish(p: Probe, dir: PathBuf) {
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

fn cmd(p: &mut Probe, c: &str) {
    let src = p.s.server_source();
    let (ok, _) = p.s.perform(src, c);
    assert!(ok, "command refused: {c}");
}

fn game_ticks(p: &mut Probe, n: i32) {
    for _ in 0..n {
        p.s.game_tick();
        p.s.tick += 1;
    }
}

/// Phases of one server tick, recorded.
fn phases_of_one_tick(p: &mut Probe) -> Vec<&'static str> {
    crate::server::PHASES.with(|v| *v.borrow_mut() = Some(Vec::new()));
    game_ticks(p, 1);
    crate::server::PHASES.with(|v| v.borrow_mut().take()).expect("phases")
}

/// § 1.2: scheduled functions inside the "handling the tick" span, scheduled ticks, raids,
/// spawning chunks, random ticks, special spawners, block events, dragon fight, then the entity
/// walk (falling blocks first) and the block entities last. Frozen (`tick freeze`): the night
/// skip still runs, the world phases and the block entities wait.
#[test]
fn world_tick_runs_the_game_phases_in_order() {
    let (mut p, dir) = open("phases");
    let got = phases_of_one_tick(&mut p);
    let want = [
        "sleep",
        "functions:handling",
        "scheduled",
        "raids",
        "spawns",
        "random",
        "spawners",
        "events",
        "fight",
        "falling",
        "items",
        "mobs",
        "projectiles",
        "block_entities",
    ];
    assert_eq!(got, want, "world tick phases");
    p.s.cmd.ticking.set_frozen(true);
    let got = phases_of_one_tick(&mut p);
    assert_eq!(got, ["sleep", "mobs", "projectiles"], "frozen world tick phases");
    finish(p, dir);
}

fn hopper_count(p: &Probe, at: [i32; 3], id: &str) -> i64 {
    let Some(Nbt::List(items)) = p.s.be_get(at).and_then(|b| b.get("Items")) else { return 0 };
    items
        .iter()
        .filter(|i| i.get("id").and_then(Nbt::as_str) == Some(id))
        .filter_map(|i| crate::nbtio::int(i, "count"))
        .sum()
}

/// Furnace one tick from the end of its cooking, hopper below it: the block entities tick in one
/// list in insertion order, after all the entities. Furnace placed first: its ingot is in the
/// hopper at the end of that tick. Hopper placed first: the hopper has already looked (empty
/// output), the ingot waits in the furnace until the next tick. Before: furnaces always ticked
/// before every hopper (and before the items and mobs).
#[test]
fn furnace_and_hopper_tick_in_insertion_order() {
    let (mut p, dir) = open("insertion");
    let furnace = "setblock {X} -60 3 minecraft:furnace[lit=true]{Items:[{Slot:0b,id:\"minecraft:raw_iron\",count:1}],\
        cooking_time_spent:199,cooking_total_time:200,lit_time_remaining:100,lit_total_time:1600}";
    let hopper = "setblock {X} -61 3 minecraft:hopper[facing=down]";
    // Furnace then hopper (x = 3), hopper then furnace (x = 6).
    cmd(&mut p, &furnace.replace("{X}", "3"));
    cmd(&mut p, &hopper.replace("{X}", "3"));
    cmd(&mut p, &hopper.replace("{X}", "6"));
    cmd(&mut p, &furnace.replace("{X}", "6"));
    let output = |p: &Probe, x: i32| {
        p.s.life.furnaces.get(&[x, -60, 3]).and_then(|f| f.items[2].as_ref()).map_or(0, |s| s.count)
    };
    game_ticks(&mut p, 1);
    assert_eq!(hopper_count(&p, [3, -61, 3], "minecraft:iron_ingot"), 1, "furnace first: ingot pulled in the same tick");
    assert_eq!(output(&p, 3), 0);
    assert_eq!(hopper_count(&p, [6, -61, 3], "minecraft:iron_ingot"), 0, "hopper first: it ticked before the ingot");
    assert_eq!(output(&p, 6), 1, "hopper first: ingot still in the furnace");
    game_ticks(&mut p, 1);
    assert_eq!(hopper_count(&p, [6, -61, 3], "minecraft:iron_ingot"), 1, "hopper first: ingot pulled on the next tick");
    finish(p, dir);
}

/// § 1.2 step 11b: an entity other than a player ticks only in a "ticking entities" chunk
/// (min(view, simulation) around a player). Simulation distance 2: falling block, item, orb and
/// arrow in chunk 4 stay put (loaded chunk); the same in chunk 1 move. Before: only the mobs,
/// minecarts and decorations tested their chunk.
#[test]
fn entities_outside_ticking_chunks_wait() {
    let (mut p, dir) = open("frozen");
    p.s.sim_dist = 2;
    for x in ["24.5", "72.5"] {
        cmd(&mut p, &format!("summon minecraft:item {x} -50 0.5 {{Item:{{id:\"minecraft:stone\",count:1}}}}"));
        cmd(&mut p, &format!("summon minecraft:falling_block {x} -50 2.5 {{BlockState:{{Name:\"minecraft:sand\"}}}}"));
        cmd(&mut p, &format!("summon minecraft:experience_orb {x} -50 -2.5 {{Value:3}}"));
        cmd(&mut p, &format!("summon minecraft:arrow {x} -50 4.5 {{Motion:[0.0d,0.0d,0.5d]}}"));
    }
    let snap = |p: &Probe| {
        let mut v: Vec<(&'static str, [f64; 3])> = Vec::new();
        v.extend(p.s.life.drops.iter().map(|d| ("item", d.pos)));
        v.extend(p.s.life.orbs.iter().map(|o| ("orb", o.pos)));
        v.extend(p.s.bs.entities.list.iter().map(|e| ("falling", e.pos)));
        v.extend(p.s.proj.list.iter().map(|q| ("arrow", q.pos)));
        v
    };
    let before = snap(&p);
    assert_eq!(before.len(), 8, "summoned: {before:?}");
    game_ticks(&mut p, 5);
    let after = snap(&p);
    for kind in ["item", "orb", "falling", "arrow"] {
        let far: Vec<_> = before.iter().filter(|(k, at)| *k == kind && at[0] > 64.0).collect();
        let far_after: Vec<_> = after.iter().filter(|(k, at)| *k == kind && at[0] > 64.0).collect();
        assert_eq!(far, far_after, "{kind} in chunk 4 moved");
        let near_moved = after.iter().filter(|(k, at)| *k == kind && at[0] < 64.0).all(|(_, at)| {
            before.iter().filter(|(k, _)| *k == kind).all(|(_, b)| b[0] > 64.0 || b != at)
        });
        assert!(near_moved, "{kind} in chunk 1 did not move: {after:?}");
    }
    finish(p, dir);
}

/// An entity born during the entity walk (an item dropped by a falling block, by mining) waits
/// for the next server tick before its first tick; born outside the walk, it ticks at once.
#[test]
fn items_and_orbs_born_during_the_walk_wait() {
    let (mut p, dir) = open("born");
    let stone = Stack { id: "minecraft:stone".into(), count: 1, components: None };
    p.s.walk.start();
    p.s.spawn_item_at([2.5, -55.0, 2.5], stone.clone());
    p.s.add_orb([2.5, -55.0, -2.5], 3);
    p.s.drops_tick();
    assert_eq!(p.s.life.drops.last().map(|d| d.ticks), Some(0), "item born in the walk ticked");
    assert_eq!(p.s.life.orbs.last().map(|o| o.ticks), Some(0), "orb born in the walk ticked");
    p.s.walk.end();
    p.s.walk.start();
    p.s.drops_tick();
    assert_eq!(p.s.life.drops.last().map(|d| d.ticks), Some(1), "item: first tick on the next walk");
    assert_eq!(p.s.life.orbs.last().map(|o| o.ticks), Some(1), "orb: first tick on the next walk");
    p.s.walk.end();
    p.s.spawn_item_at([2.5, -55.0, 4.5], stone);
    p.s.drops_tick();
    assert_eq!(p.s.life.drops.last().map(|d| d.ticks), Some(1), "item born outside the walk ticks at once");
    finish(p, dir);
}

/// Spectator under `spectators_generate_chunks` false: no player ticket, nothing simulated
/// around them (the item in the air does not fall); rule true: it falls.
#[test]
fn spectator_without_the_chunk_rule_simulates_nothing() {
    let (mut p, dir) = open("spectator");
    p.s.game_mode = 3;
    cmd(&mut p, "gamerule minecraft:spectators_generate_chunks false");
    assert!(p.s.sim_centers().is_empty(), "spectator still a simulation centre");
    assert!(!p.s.chunk_simulated_now((0, 0)), "block ticks around the spectator");
    cmd(&mut p, "summon minecraft:item 2.5 -50 2.5 {Item:{id:\"minecraft:stone\",count:1}}");
    let y = |p: &Probe| p.s.life.drops.last().map(|d| d.pos[1]).expect("item");
    game_ticks(&mut p, 3);
    assert_eq!(y(&p), -50.0, "item ticked around an ignored spectator");
    cmd(&mut p, "gamerule minecraft:spectators_generate_chunks true");
    assert_eq!(p.s.sim_centers(), vec![(0, 0)]);
    game_ticks(&mut p, 3);
    assert!(y(&p) < -50.0, "rule true: the item falls");
    finish(p, dir);
}

/// Micro-bench (measurement, not a proof): cost of the "ticking entities" test per entity, and
/// of the furnaces in the common block-entity list against their former separate loop (idle
/// furnaces: no fuel, the tick itself does almost nothing). Run alone:
/// `cargo test --release -p rc-sim bench_g3f13 -- --ignored --nocapture`.
#[test]
#[ignore = "measurement bench"]
fn bench_g3f13_entity_test_and_furnace_list() {
    let (mut p, dir) = open("bench");
    p.s.sim_dist = 2;
    let n = 1_000_000u32;
    for (what, x) in [("in range", 8.5), ("out of range", 72.5)] {
        let t = std::time::Instant::now();
        let mut k = 0u32;
        for i in 0..n {
            k += u32::from(p.s.entity_ticks_at(std::hint::black_box([x, -50.0, f64::from(i & 31)])));
        }
        let ns = t.elapsed().as_nanos() as f64 / f64::from(n);
        eprintln!("bench_g3f13 entity test {what}: {ns:.2} ns per call ({k} ticking)");
    }
    // 512 furnaces in chunks 0 and 1 (2 × 16 × 16 columns × 1 layer).
    for x in 0..32 {
        cmd(&mut p, &format!("fill {x} -59 0 {x} -59 15 minecraft:furnace"));
    }
    let furnaces: Vec<[i32; 3]> = p.s.life.furnaces.keys().copied().collect();
    assert_eq!(furnaces.len(), 512);
    let rounds = 2000;
    for pass in 0..3 {
        let t = std::time::Instant::now();
        for _ in 0..rounds {
            p.s.tick_block_entities();
        }
        let list = t.elapsed().as_nanos() as f64 / f64::from(rounds) / 512.0;
        // Former loop: keys copied each tick, border and chunk test, tick.
        let t = std::time::Instant::now();
        for _ in 0..rounds {
            let positions: Vec<[i32; 3]> = p.s.life.furnaces.keys().copied().collect();
            for q in positions {
                if p.s.be_ticking(q) {
                    p.s.furnace_tick(q);
                }
            }
        }
        let old = t.elapsed().as_nanos() as f64 / f64::from(rounds) / 512.0;
        eprintln!("bench_g3f13 pass {pass}: idle furnace in the common list {list:.1} ns, former loop {old:.1} ns");
    }
    // Same with 512 brewing stands instead (the former loop read the block name, a new string,
    // of each stand at every tick).
    for x in 0..32 {
        cmd(&mut p, &format!("fill {x} -59 0 {x} -59 15 minecraft:brewing_stand"));
    }
    assert_eq!(p.s.life.brewers.len(), 512, "brewing stands");
    for pass in 0..3 {
        let t = std::time::Instant::now();
        for _ in 0..rounds {
            p.s.tick_block_entities();
        }
        let list = t.elapsed().as_nanos() as f64 / f64::from(rounds) / 512.0;
        let t = std::time::Instant::now();
        for _ in 0..rounds {
            let positions: Vec<[i32; 3]> = p.s.life.brewers.keys().copied().collect();
            for q in positions {
                if p.s.block_name_s7(q).as_deref() == Some("minecraft:brewing_stand") && p.s.be_ticking(q) {
                    p.s.brewer_tick(q);
                }
            }
        }
        let old = t.elapsed().as_nanos() as f64 / f64::from(rounds) / 512.0;
        eprintln!("bench_g3f13 pass {pass}: idle brewing stand in the common list {list:.1} ns, former loop {old:.1} ns");
    }
    finish(p, dir);
}

/// Block-entity state follows its dimension: a monster spawner keeps counting down after a
/// trip to the Nether (before: its list was pruned there and it never ticked again), and the
/// brewing stands of the surface are not those of the Nether.
#[test]
fn block_entity_state_follows_its_dimension() {
    let (mut p, dir) = open("dims");
    let at = [3, -60, 3];
    cmd(&mut p, "setblock 3 -60 3 minecraft:spawner{Delay:100s,SpawnData:{entity:{id:\"minecraft:zombie\"}}}");
    cmd(&mut p, "setblock 5 -60 3 minecraft:brewing_stand");
    let delay = |p: &Probe| crate::nbtio::int(p.s.be_get(at).expect("spawner"), "Delay").expect("Delay");
    game_ticks(&mut p, 5);
    assert_eq!(delay(&p), 95, "player in range: the delay counts down");
    assert!(p.s.life.brewers.contains_key(&[5, -60, 3]));
    p.s.enter_world(crate::dims::NETHER);
    assert!(p.s.life.brewers.is_empty(), "surface brewing stand seen from the Nether");
    game_ticks(&mut p, 3);
    p.s.enter_world(crate::dims::OVERWORLD);
    p.s.pos = [0.5, -60.0, 0.5];
    assert!(p.s.life.brewers.contains_key(&[5, -60, 3]), "brewing stand lost on the trip");
    game_ticks(&mut p, 5);
    assert_eq!(delay(&p), 90, "spawner stopped after the trip");
    finish(p, dir);
}

/// A block entity replaced at the same position is a new one: a bell removed while it shakes and
/// placed back starts still, a furnace removed (`/setblock`) and placed back starts empty (their
/// ticker and their kept state went with the old block entity; before, the furnace revived the
/// old one's contents).
#[test]
fn replaced_block_entities_start_fresh() {
    let (mut p, dir) = open("replaced");
    let at = [4, -60, 4];
    cmd(&mut p, "setblock 4 -60 4 minecraft:bell[attachment=floor]");
    p.s.bell_rung(at);
    assert_eq!(p.s.mobs.bells.len(), 1);
    cmd(&mut p, "setblock 4 -60 4 minecraft:air");
    cmd(&mut p, "setblock 4 -60 4 minecraft:bell[attachment=floor]");
    assert!(p.s.mobs.bells.is_empty(), "the new bell inherits the shake of the removed one");
    let f = [6, -60, 4];
    cmd(&mut p, "setblock 6 -60 4 minecraft:furnace{Items:[{Slot:0b,id:\"minecraft:raw_iron\",count:5}]}");
    assert!(p.s.life.furnaces.get(&f).is_some_and(|f| f.items[0].is_some()));
    cmd(&mut p, "setblock 6 -60 4 minecraft:air");
    cmd(&mut p, "setblock 6 -60 4 minecraft:furnace");
    assert!(p.s.life.furnaces.get(&f).is_some_and(|f| f.items.iter().all(Option::is_none)), "old contents revived");
    finish(p, dir);
}
