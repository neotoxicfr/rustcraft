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
    (reopen(&dir, data), dir)
}

/// The world of `dir` opened again (same player setup and loaded chunks as `open`).
pub(crate) fn reopen(dir: &std::path::Path, data: Arc<rc_data::VersionData>) -> Probe {
    let mut p = Probe::open(dir, data);
    p.s.pos = [0.5, -60.0, 0.5];
    p.s.loaded = true;
    p.s.game_mode = 0;
    p.s.teleport = None;
    p.s.view = 8;
    let around: Vec<(i32, i32)> = (-2..=9).flat_map(|x| (-2..=2).map(move |z| (x, z))).collect();
    p.s.load_now(&around);
    // Refused when already false (reopened world).
    let src = p.s.server_source();
    let _ = p.s.perform(src, "gamerule minecraft:spawn_mobs false");
    p
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
    // Order of the tickers of a loaded chunk: the table walk alone, then the whole registration
    // (chunk 0: 256 brewing stands, already registered: the idempotent path).
    let at: Vec<[i32; 3]> = (0..256).map(|i| [i & 15, -59, i >> 4]).collect();
    for pass in 0..3 {
        let t = std::time::Instant::now();
        for _ in 0..rounds {
            std::hint::black_box(crate::blocks::entities::chunk_be_order(std::hint::black_box(&at)));
        }
        let order = t.elapsed().as_nanos() as f64 / f64::from(rounds);
        let t = std::time::Instant::now();
        for _ in 0..rounds {
            p.s.register_chunk_tickers((0, 0));
        }
        let all = t.elapsed().as_nanos() as f64 / f64::from(rounds);
        eprintln!("bench_g3f13 pass {pass}: 256 block entities, table order {order:.0} ns, registration {all:.0} ns");
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

/// Walk order of the game's per-chunk block-entity table, frozen from an independent model of
/// that table (32 slots, doubled past 3/4 full, mixed position hash, walked from the last slot):
/// the four cells of the reload test, then 30 positions (two growths of the table).
#[test]
fn chunk_block_entity_order_is_the_game_table_walk() {
    use crate::blocks::entities::chunk_be_order;
    let four = [[10, -60, 3], [10, -61, 3], [3, -60, 3], [3, -61, 3]];
    assert_eq!(chunk_be_order(&four), [0, 1, 3, 2], "slots 27, 24, 21, 7");
    let many: Vec<[i32; 3]> = (0..30).map(|x| [x * 3 % 16 + 32, -60 + x % 5, (x * 7) % 16 - 16]).collect();
    let want =
        [11, 20, 6, 25, 4, 0, 2, 7, 8, 9, 5, 1, 12, 23, 13, 15, 24, 27, 14, 22, 26, 19, 28, 29, 10, 18, 3, 16, 21, 17];
    assert_eq!(chunk_be_order(&many), want);
    // A repeated position keeps its slot and takes the later entry.
    assert_eq!(chunk_be_order(&[[10, -60, 3], [3, -60, 3], [10, -60, 3]]), [2, 1]);
}

/// A chunk read from disk registers its tickers in the game's order, every family together:
/// after a reload the furnace at x = 10 (slot 27) ticks before the hopper under it (slot 24), so
/// the ingot is pulled in the same tick; at x = 3 the hopper (21) comes before the furnace (7)
/// and the ingot waits one tick. Before: the furnaces of a read chunk registered after all its
/// other block entities (x = 10 one tick late) and never for a generated chunk.
#[test]
fn reloaded_chunk_ticks_its_block_entities_in_table_order() {
    let (mut p, dir) = open("reload");
    let furnace = "setblock {X} -60 3 minecraft:furnace[lit=true]{Items:[{Slot:0b,id:\"minecraft:raw_iron\",count:1}],\
        cooking_time_spent:199,cooking_total_time:200,lit_time_remaining:100,lit_total_time:1600}";
    let hopper = "setblock {X} -61 3 minecraft:hopper[facing=down]";
    for x in ["3", "10"] {
        cmd(&mut p, &furnace.replace("{X}", x));
        cmd(&mut p, &hopper.replace("{X}", x));
    }
    p.s.save(true);
    let data = p.s.data.clone();
    drop(p);
    let mut p = reopen(&dir, data);
    assert!(p.s.life.furnaces.contains_key(&[10, -60, 3]), "furnace not simulated after the reload");
    game_ticks(&mut p, 1);
    assert_eq!(hopper_count(&p, [10, -61, 3], "minecraft:iron_ingot"), 1, "x = 10: furnace first, same tick");
    assert_eq!(hopper_count(&p, [3, -61, 3], "minecraft:iron_ingot"), 0, "x = 3: hopper first, ingot waits");
    game_ticks(&mut p, 1);
    assert_eq!(hopper_count(&p, [3, -61, 3], "minecraft:iron_ingot"), 1, "x = 3: pulled on the next tick");
    finish(p, dir);
}

/// Going through a portal is part of an entity's tick: simulation distance 2, an item, a zombie
/// and a minecart in an End portal in chunk 4 (loaded, outside the "ticking entities" chunks)
/// stay; the same in chunk 1 go to the End. Before: all of them passed.
#[test]
fn frozen_entities_do_not_go_through_portals() {
    let (mut p, dir) = open("portal");
    p.s.sim_dist = 2;
    for x in [24, 72] {
        for z in [0, 4, 8] {
            cmd(&mut p, &format!("setblock {x} -60 {z} minecraft:end_portal"));
        }
        let c = f64::from(x) + 0.5;
        cmd(&mut p, &format!("summon minecraft:item {c} -59.6 0.5 {{Item:{{id:\"minecraft:stone\",count:1}}}}"));
        cmd(&mut p, &format!("summon minecraft:zombie {c} -60 4.5 {{NoAI:1b}}"));
        cmd(&mut p, &format!("summon minecraft:minecart {c} -60 8.5"));
    }
    let count = |p: &Probe, far: bool| {
        let side = |x: f64| (x > 64.0) == far;
        (
            p.s.life.drops.iter().filter(|d| side(d.pos[0])).count(),
            p.s.mobs.list.iter().flatten().filter(|m| side(m.pos[0])).count(),
            p.s.carts.list.iter().flatten().filter(|c| side(c.pos[0])).count(),
        )
    };
    assert_eq!((count(&p, false), count(&p, true)), ((1, 1, 1), (1, 1, 1)), "summoned");
    game_ticks(&mut p, 3);
    assert_eq!(count(&p, true), (1, 1, 1), "chunk 4: frozen entities passed");
    assert_eq!(count(&p, false), (0, 0, 0), "chunk 1: ticking entities stayed");
    finish(p, dir);
}

/// A player's ender pearl asks for the chunks within its ticket's radius (2), not only its own:
/// flying from chunk 9 (last loaded) towards +x, chunks 11 are read or generating after one
/// tick, before it reaches them. Before: only its current chunk, so it stopped at each new one
/// until that one was generated.
#[test]
fn player_pearl_loads_the_chunks_of_its_ticket() {
    let (mut p, dir) = open("pearl");
    assert!(!p.s.chunks.contains_key(&(11, 0)), "chunk 11 already loaded");
    cmd(&mut p, "summon minecraft:ender_pearl 150.5 -50 0.5 {NoGravity:1b,Motion:[1.5d,0.0d,0.0d]}");
    let uuid = p.s.uuid;
    p.s.proj.list.last_mut().expect("pearl").owner = Some(uuid);
    game_ticks(&mut p, 1);
    for c in [(11, 0), (11, 2), (10, -2)] {
        assert!(p.s.chunks.contains_key(&c), "chunk {c:?} of the pearl's ticket not requested");
    }
    finish(p, dir);
}

/// Container contents are thrown by every removal with side effects: `/setblock ... destroy` on
/// a furnace with 5 raw iron and on a chest with 3 stone throws the blocks and their contents
/// (before: the contents were erased). The player's path (contents first, then the removal)
/// throws them once.
#[test]
fn destroyed_containers_throw_their_contents() {
    let (mut p, dir) = open("contents");
    cmd(&mut p, "setblock 6 -60 4 minecraft:furnace{Items:[{Slot:0b,id:\"minecraft:raw_iron\",count:5}]}");
    cmd(&mut p, "setblock 8 -60 4 minecraft:chest{Items:[{Slot:0b,id:\"minecraft:stone\",count:3}]}");
    cmd(&mut p, "setblock 6 -60 4 minecraft:air destroy");
    cmd(&mut p, "setblock 8 -60 4 minecraft:air destroy");
    let thrown = |p: &Probe, id: &str| -> i32 {
        p.s.life.drops.iter().filter(|d| d.stack.id == id).map(|d| d.stack.count).sum()
    };
    assert_eq!(thrown(&p, "minecraft:raw_iron"), 5, "furnace contents");
    assert_eq!(thrown(&p, "minecraft:furnace"), 1);
    assert_eq!(thrown(&p, "minecraft:stone"), 3, "chest contents");
    assert_eq!(thrown(&p, "minecraft:chest"), 1);
    cmd(&mut p, "setblock 10 -60 4 minecraft:barrel{Items:[{Slot:0b,id:\"minecraft:dirt\",count:2}]}");
    p.s.container_broken([10, -60, 4]);
    let air = p.s.air();
    p.s.set_block([10, -60, 4], air, crate::blocks::flag::ALL);
    assert_eq!(thrown(&p, "minecraft:dirt"), 2, "contents thrown twice");
    finish(p, dir);
}
