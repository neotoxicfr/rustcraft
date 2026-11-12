//! `execute` across worlds, like the game whose entities carry their own world: a selector that no
//! option limits to one world searches every open world (overworld first), `@s` is found where its
//! entity lives, and each entity is read there (score holders, data, relations, the `this` of a
//! predicate); `store entity` writes and `tp` moves an entity in its own world. Fresh flat world:
//! the 26.3 data cache and the client jar are required (absent: FAILURE).

use crate::blocks::probe::Probe;
use crate::dims::NETHER;
use crate::other_dimension_send_tests::{new_world, run, value_of};

/// The value a server command returns, 0 when it fails (`execute if` finding nothing).
fn count(p: &mut Probe, text: &str) -> i32 {
    let src = p.s.server_source();
    let (done, value) = p.s.perform(src, text);
    if done { value } else { 0 }
}

/// A frozen pig in each world: the overworld's at (20, 70, 0) tagged `overworld`, the Nether's at
/// (2, 70, 0) tagged `nether` (its chunk forced and loaded).
fn pig_in_each_world(tag: &str) -> Probe {
    let (mut p, _dir) = new_world(tag);
    run(&mut p, "execute in minecraft:the_nether run forceload add 0 0");
    p.s.in_world(NETHER, |s| s.load_now(&[(0, 0)]));
    run(&mut p, "summon minecraft:pig 20 70 0 {NoAI:1b,NoGravity:1b,Tags:[\"overworld\"]}");
    run(
        &mut p,
        "execute in minecraft:the_nether run summon minecraft:pig 2 70 0 {NoAI:1b,NoGravity:1b,Tags:[\"nether\"]}",
    );
    p
}

/// A selector that no option limits to one world finds the entities of every world, in a middle
/// step as in a final condition, from either world. Before: a middle step searched the world the
/// server works in, a final condition the source's.
#[test]
fn an_unlimited_selector_searches_every_world() {
    let mut p = pig_in_each_world("every-world");
    assert_eq!(count(&mut p, "execute if entity @e[tag=nether]"), 1, "final condition from the overworld");
    assert_eq!(count(&mut p, "execute in minecraft:the_nether if entity @e[tag=overworld]"), 1, "from the Nether");
    assert_eq!(count(&mut p, "execute as @e[type=minecraft:pig] if entity @s"), 2, "middle step");
    run(&mut p, "execute as @e[type=minecraft:pig] at @s run tag @s add seen");
    let seen_in_nether = "execute in minecraft:the_nether positioned 2 70 0 if entity @e[tag=seen,distance=..1]";
    assert_eq!(count(&mut p, seen_in_nether), 1, "the Nether's pig tagged where it stands");
    assert_eq!(count(&mut p, "execute positioned 20 70 0 if entity @e[tag=seen,distance=..1]"), 1, "overworld pig");
}

/// The search of every world keeps the game's order: `@n` takes the nearest coordinates whatever
/// the world, and an arbitrary order the overworld's entities first, even while the player (and
/// the server) stand in the Nether (the overworld pig's chunk forced, so that it stays loaded).
#[test]
fn the_search_of_every_world_keeps_the_game_order() {
    let mut p = pig_in_each_world("world-order");
    // The Nether's pig stands 2 blocks from the point, the overworld's 20.
    run(&mut p, "execute positioned 0 70 0 as @n[type=minecraft:pig] at @s run tag @s add nearest");
    assert_eq!(count(&mut p, "execute if entity @e[tag=nearest]"), 1, "one pig taken");
    assert_eq!(count(&mut p, "execute if entity @e[tag=nearest,tag=nether]"), 1, "the nearest coordinates");
    run(&mut p, "forceload add 20 0");
    run(&mut p, "execute in minecraft:the_nether run tp @a 0.5 80 0.5");
    assert_eq!(p.s.dim.name, NETHER, "setup: the player in the Nether");
    run(&mut p, "execute as @e[type=minecraft:pig,limit=1] at @s run tag @s add first");
    assert_eq!(count(&mut p, "execute if entity @e[tag=first]"), 1, "one pig taken");
    assert_eq!(count(&mut p, "execute if entity @e[tag=first,tag=overworld]"), 1, "the overworld's first");
}

/// One step reads each entity in its own world: `@s` (the overworld pig) and a world-limited
/// selector of the source's world (the Nether's pig) compared in one `if score`. Before: the step
/// ran in the Nether, where `@s` is not, and failed.
#[test]
fn a_step_reads_each_entity_in_its_own_world() {
    let mut p = pig_in_each_world("mixed-step");
    run(&mut p, "scoreboard objectives add o dummy");
    run(&mut p, "scoreboard players set @e[tag=overworld] o 5");
    run(&mut p, "execute in minecraft:the_nether positioned 2 70 0 run scoreboard players set @e[distance=..1] o 5");
    let mixed = "execute as @e[tag=overworld] in minecraft:the_nether positioned 2 70 0 if score @s o = @e[distance=..1,limit=1] o";
    assert_eq!(count(&mut p, mixed), 1, "equal scores");
    let from_nether_pig = "execute as @e[tag=nether] if score @s o = @e[tag=overworld,limit=1] o";
    assert_eq!(count(&mut p, from_nether_pig), 1, "`@s` in the Nether, the other side in the overworld");
    run(&mut p, "execute in minecraft:the_nether positioned 2 70 0 run scoreboard players set @e[distance=..1] o 6");
    assert_eq!(count(&mut p, mixed), 0, "the scores are compared");
    let stored =
        "execute as @e[tag=overworld] in minecraft:the_nether store result score @s o if entity @e[tag=nether]";
    assert_eq!(count(&mut p, stored), 1, "store score of an entity of another world");
    assert_eq!(value_of(&mut p, "scoreboard players get @e[tag=overworld,limit=1] o"), 1, "stored score");
}

/// `store result entity` writes the entity where it lives, and `if data entity` reads it there.
/// Before: the store wrote into the world the command ran in (nothing), and the condition searched
/// the source's world only.
#[test]
fn store_and_data_reach_an_entity_of_another_world() {
    let mut p = pig_in_each_world("store-data");
    run(&mut p, "execute in minecraft:the_nether run summon minecraft:marker 2 71 0 {Tags:[\"nether_marker\"]}");
    run(&mut p, "scoreboard objectives add o dummy");
    assert_eq!(count(&mut p, "execute if data entity @e[tag=nether_marker,limit=1] data.v"), 0, "setup: no value yet");
    let store = "execute in minecraft:the_nether positioned 2 71 0 store result entity @e[type=minecraft:marker,distance=..1,limit=1] data.v int 1 in minecraft:overworld run scoreboard players set #seven o 7";
    run(&mut p, store);
    assert_eq!(count(&mut p, "execute if data entity @e[tag=nether_marker,limit=1] data.v"), 1, "if data entity");
    let read = "execute in minecraft:the_nether positioned 2 71 0 run data get entity @e[type=minecraft:marker,distance=..1,limit=1] data.v";
    assert_eq!(value_of(&mut p, read), 7, "stored value");
}

/// `execute in the_nether run tp @e[distance=…]` moves the Nether's pig in the Nether. Before: the
/// teleport ran in the overworld, where the pig is not.
#[test]
fn tp_moves_an_entity_of_the_source_world_there() {
    let mut p = pig_in_each_world("tp");
    run(&mut p, "execute in minecraft:the_nether positioned 2 70 0 run tp @e[type=minecraft:pig,distance=..1] ~ ~5 ~");
    let moved = "execute in minecraft:the_nether positioned 2 75 0 if entity @e[type=minecraft:pig,distance=..0.5]";
    assert_eq!(count(&mut p, moved), 1, "the Nether's pig moved");
    assert_eq!(
        count(&mut p, "execute positioned 20 70 0 if entity @e[type=minecraft:pig,distance=..0.5]"),
        1,
        "overworld"
    );
}

/// A predicate reads `this` (the overworld pig) where it lives and the location in the source's
/// world (the Nether's gold block). Before: the whole predicate ran in the Nether, where the pig is
/// not.
#[test]
fn a_predicate_reads_this_where_it_lives_and_the_place_in_the_source_world() {
    let mut p = pig_in_each_world("predicate");
    run(&mut p, "execute in minecraft:the_nether run setblock 0 70 0 minecraft:gold_block");
    let is_pig = "{condition:\"minecraft:entity_properties\",entity:\"this\",predicate:{type:\"minecraft:pig\"}}";
    let on_gold = "{condition:\"minecraft:location_check\",predicate:{block:{blocks:\"minecraft:gold_block\"}}}";
    let pig_in_nether = "execute as @e[tag=overworld] in minecraft:the_nether positioned 0 70 0";
    assert_eq!(count(&mut p, &format!("{pig_in_nether} if predicate {is_pig}")), 1, "this read where it lives");
    assert_eq!(count(&mut p, &format!("{pig_in_nether} if predicate [{is_pig},{on_gold}]")), 1, "both worlds");
    let pig_in_overworld = "execute as @e[tag=overworld] positioned 0 70 0";
    assert_eq!(count(&mut p, &format!("{pig_in_overworld} if predicate [{is_pig},{on_gold}]")), 0, "no gold there");
}

/// The location parts of an entity predicate on `this` read the SOURCE's world at the coordinates
/// the entity has in its own, like the game, which lends the source's world to the whole
/// predicate: the overworld pig under `in the_nether` is not "in the overworld" there, and stands
/// on the Nether's gold block placed at its coordinates. Before: read in the pig's world.
#[test]
fn a_location_inside_entity_properties_reads_the_source_world() {
    let mut p = pig_in_each_world("location-in-entity");
    p.s.in_world(NETHER, |s| s.load_now(&[(1, 0)]));
    run(&mut p, "execute in minecraft:the_nether run setblock 20 70 0 minecraft:gold_block");
    let located = |inside: &str| {
        format!("{{condition:\"minecraft:entity_properties\",entity:\"this\",predicate:{{location:{{{inside}}}}}}}")
    };
    let in_overworld = located("dimension:\"minecraft:overworld\"");
    let in_nether = located("dimension:\"minecraft:the_nether\"");
    let on_gold = located("block:{blocks:\"minecraft:gold_block\"}");
    let pig_from_nether = "execute as @e[tag=overworld] in minecraft:the_nether if predicate";
    assert_eq!(count(&mut p, &format!("{pig_from_nether} {in_overworld}")), 0, "dimension of the source");
    assert_eq!(count(&mut p, &format!("{pig_from_nether} {in_nether}")), 1, "the source's world");
    assert_eq!(count(&mut p, &format!("{pig_from_nether} {on_gold}")), 1, "the Nether's block at the pig");
    let pig_from_overworld = "execute as @e[tag=overworld] if predicate";
    assert_eq!(count(&mut p, &format!("{pig_from_overworld} {on_gold}")), 0, "no gold in the overworld");
    assert_eq!(count(&mut p, &format!("{pig_from_overworld} {in_overworld}")), 1, "one world");
}

/// A UUID is looked up among the entities the game would have loaded: in a world nobody stands
/// in, only in the chunks a ticket holds, and a world without any ticket is not visited (no world
/// exchange). Before: every parked world was visited and its stale entities found.
#[test]
fn a_uuid_finds_only_the_loaded_entities_of_another_world() {
    let mut p = pig_in_each_world("uuid-held-chunks");
    p.s.in_world(NETHER, |s| s.load_now(&[(10, 0)]));
    let near = "00000001-0000-0002-0000-000300000004";
    let far = "00000005-0000-0006-0000-000700000008";
    run(&mut p, "execute in minecraft:the_nether run summon minecraft:pig 3 70 0 {NoAI:1b,NoGravity:1b,UUID:[I;1,2,3,4]}");
    run(&mut p, "execute in minecraft:the_nether run summon minecraft:pig 162 70 0 {NoAI:1b,NoGravity:1b,UUID:[I;5,6,7,8]}");
    let far_kept = p.s.in_world(NETHER, |s| s.ent_by_uuid(0x0000_0005_0000_0006_0000_0007_0000_0008).is_some());
    assert!(far_kept, "setup: the far pig kept in the Nether's lists");
    assert_eq!(count(&mut p, &format!("execute if entity {near}")), 1, "a forced chunk's pig");
    assert_eq!(count(&mut p, &format!("execute if entity {far}")), 0, "a chunk nothing holds");
    run(&mut p, "execute in minecraft:the_nether run forceload remove all");
    let before = p.s.chunk_map_swaps;
    assert_eq!(count(&mut p, &format!("execute as {near} run tag @s add seen")), 0, "nothing held there");
    assert_eq!(p.s.chunk_map_swaps - before, 0, "a world without a ticket visited");
}

/// `execute on <relation>` reads the relation where the source's entity lives: the Nether pig's
/// passenger found from the overworld. Before: the relation was read in the overworld.
#[test]
fn execute_on_reads_the_relation_where_the_entity_lives() {
    let mut p = pig_in_each_world("relation");
    run(
        &mut p,
        "execute in minecraft:the_nether run summon minecraft:chicken 2 70 0 {NoAI:1b,NoGravity:1b,Tags:[\"rider\"]}",
    );
    let mount = "execute in minecraft:the_nether positioned 2 70 0 run ride @e[type=minecraft:chicken,distance=..1,limit=1] mount @e[type=minecraft:pig,distance=..1,limit=1]";
    run(&mut p, mount);
    let tag = "execute in minecraft:the_nether positioned 0 70 0 as @e[type=minecraft:pig,distance=..10] on passengers run tag @s add carried";
    assert_eq!(count(&mut p, tag), 1, "passenger not found");
    assert_eq!(count(&mut p, "execute if entity @e[tag=carried,tag=rider]"), 1, "the chicken tagged");
    assert_eq!(count(&mut p, "execute as @e[tag=rider] on vehicle if entity @s[tag=nether]"), 1, "vehicle");
}

/// A world nobody stands in shows only the entities of the chunks a ticket holds, like the game,
/// which unloads the others: the Nether's pig of a forced chunk is found from the overworld, the
/// one of a chunk nothing holds is not, until it is forced. With no ticket left, the Nether is not
/// even visited.
#[test]
fn a_world_nobody_stands_in_shows_only_its_held_chunks() {
    let mut p = pig_in_each_world("held-chunks");
    p.s.in_world(NETHER, |s| s.load_now(&[(10, 0)]));
    run(
        &mut p,
        "execute in minecraft:the_nether run summon minecraft:pig 162 70 0 {NoAI:1b,NoGravity:1b,Tags:[\"far\"]}",
    );
    assert_eq!(count(&mut p, "execute if entity @e[tag=nether]"), 1, "the forced chunk's pig");
    assert_eq!(count(&mut p, "execute if entity @e[tag=far]"), 0, "a chunk nothing holds");
    run(&mut p, "execute in minecraft:the_nether run forceload add 160 0");
    assert_eq!(count(&mut p, "execute if entity @e[tag=far]"), 1, "once forced");
    run(&mut p, "execute in minecraft:the_nether run forceload remove all");
    let before = p.s.chunk_map_swaps;
    assert_eq!(count(&mut p, "execute if entity @e[type=minecraft:pig,tag=!overworld]"), 0, "nothing held there");
    assert_eq!(p.s.chunk_map_swaps - before, 0, "a world without a ticket visited");
}

/// A search of every world visits only the worlds that may hold an entity: an open Nether with
/// nothing in it costs no world exchange (deterministic count); once it holds a pig, one visit.
#[test]
fn a_search_of_every_world_skips_the_empty_worlds() {
    let (mut p, _dir) = new_world("skip-empty");
    run(&mut p, "execute in minecraft:the_nether run forceload query");
    assert!(p.s.dims.parked.iter().any(|w| w.dim.name == NETHER), "setup: the Nether open");
    let before = p.s.chunk_map_swaps;
    assert_eq!(count(&mut p, "execute if entity @e[tag=absent]"), 0);
    let exchanges = p.s.chunk_map_swaps - before;
    eprintln!("world exchanges, empty Nether: {exchanges}");
    assert_eq!(exchanges, 0, "an empty world visited");
    run(&mut p, "execute in minecraft:the_nether run forceload add 0 0");
    p.s.in_world(NETHER, |s| s.load_now(&[(0, 0)]));
    run(&mut p, "execute in minecraft:the_nether run summon minecraft:pig 2 70 0 {NoAI:1b,NoGravity:1b}");
    let before = p.s.chunk_map_swaps;
    assert_eq!(count(&mut p, "execute if entity @e[tag=absent]"), 0);
    let exchanges = p.s.chunk_map_swaps - before;
    eprintln!("world exchanges, Nether holding a pig: {exchanges}");
    assert_eq!(exchanges, 2, "one visit of the Nether (two exchanges)");
}
