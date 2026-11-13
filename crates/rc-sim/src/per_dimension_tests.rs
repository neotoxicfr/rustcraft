//! What each dimension keeps and searches on its own, like the game's per-world state: forced
//! chunks, the entities a selector limited to one world finds, the world `execute at` takes, the
//! position a global world event is heard at from another world. Fresh flat world: the 26.3 data
//! cache and the client jar are required (absent: FAILURE).

use rc_client::Event;

use crate::blocks::probe::Probe;
use crate::dims::{NETHER, OVERWORLD};
use crate::other_dimension_send_tests::{new_world, open_world, run, value_of};

/// Forced chunks saved in the folder of `dimension`, in their saved order.
fn saved_forced(p: &Probe, dimension: &str) -> Vec<(i32, i32)> {
    let dir = crate::world::dimension_dir(&p.s.world, dimension);
    crate::commands::cmds_world::read_forced(&dir).into_iter().collect()
}

/// The value a server command returns, 0 when it fails (`execute if` finding nothing).
fn count(p: &mut Probe, text: &str) -> i32 {
    let src = p.s.server_source();
    let (done, value) = p.s.perform(src, text);
    if done { value } else { 0 }
}

/// Each dimension keeps its own forced chunks, like the game's per-world chunk tickets: a chunk
/// forced from the Nether is forced, listed and queried there only, saved in the Nether's folder
/// and read back from it; once the player stands in the Nether, the overworld's forced chunk
/// forces nothing there. Before: one set for every dimension, saved in the active one's folder.
#[test]
fn each_dimension_keeps_its_own_forced_chunks() {
    let (mut p, dir) = new_world("forced");
    run(&mut p, "forceload add 32 32");
    run(&mut p, "execute in minecraft:the_nether run forceload add 0 0");
    assert_eq!(p.s.cmd.forced.iter().copied().collect::<Vec<_>>(), [(2, 2)], "overworld's forced chunks");
    assert_eq!(value_of(&mut p, "execute in minecraft:the_nether run forceload query"), 1, "Nether's count");
    assert_eq!(count(&mut p, "execute in minecraft:the_nether run forceload query 0 0"), 1);
    assert_eq!(count(&mut p, "forceload query 0 0"), 0, "the Nether's chunk forced in the overworld");
    p.s.save(true);
    assert_eq!(saved_forced(&p, OVERWORLD), [(2, 2)]);
    assert_eq!(saved_forced(&p, NETHER), [(0, 0)]);
    run(&mut p, "execute in minecraft:the_nether run tp @a 0.5 80 0.5");
    assert_eq!(p.s.dim.name, NETHER, "player not in the Nether");
    assert!(!p.s.chunk_entity_ticking((2, 2), &[]), "the overworld's forced chunk ticks in the Nether");
    assert!(p.s.chunk_entity_ticking((0, 0), &[]), "the Nether's own forced chunk does not tick");
    drop(p);
    let mut p = open_world(&dir);
    assert_eq!(value_of(&mut p, "execute in minecraft:overworld run forceload query"), 1, "overworld read back");
    assert_eq!(count(&mut p, "execute in minecraft:the_nether run forceload query 0 0"), 1, "Nether read back");
}

/// A game test batch finished in the overworld (its definition's world) while the player stands
/// in the Nether releases the overworld's forced chunks (those of `forceload` too, like the game)
/// and leaves the Nether's alone. Before: the forced chunks of the world the server worked in were
/// released, the overworld's kept.
#[test]
fn a_finished_test_batch_releases_its_own_world_forced_chunks() {
    let (mut p, _dir) = new_world("test-batch");
    run(&mut p, "forceload add 32 32");
    run(&mut p, "execute in minecraft:the_nether run forceload add 0 0");
    run(&mut p, "execute in minecraft:the_nether run tp @a 0.5 80 0.5");
    assert_eq!(p.s.dim.name, NETHER, "player not in the Nether");
    run(&mut p, "execute in minecraft:overworld positioned 8 -60 8 run test run minecraft:always_pass");
    let running = |p: &Probe| p.s.cmd.tests.runner.as_ref().is_some_and(|r| r.ticked_len() > 0);
    assert!(running(&p), "setup: the test does not run");
    let mut ticks = 0;
    while running(&p) {
        assert!(ticks < 400, "the test still runs after 400 game ticks");
        p.s.game_tick();
        p.s.tick += 1;
        ticks += 1;
    }
    assert_eq!(p.s.cmd.forced.iter().copied().collect::<Vec<_>>(), [(0, 0)], "the Nether's forced chunks");
    assert_eq!(count(&mut p, "execute in minecraft:overworld run forceload query"), 0, "the overworld's kept");
}

/// A selector limited to one world (`distance`, `x`…`dz`) searches the source's world, like the
/// game: under `execute in the_nether`, `as @e[distance=…]` finds the Nether's pig (before: the
/// overworld's entities around the same point, here none), and a search of the Nether limited to
/// that world never finds the player standing in the overworld, while an unlimited one searches
/// the whole player list.
#[test]
fn a_world_limited_selector_searches_the_source_world() {
    let (mut p, _dir) = new_world("limited-selector");
    run(&mut p, "execute in minecraft:the_nether run forceload add 0 0");
    run(&mut p, "summon minecraft:pig 2 70 0 {NoAI:1b,NoGravity:1b,Tags:[\"overworld\"]}");
    run(
        &mut p,
        "execute in minecraft:the_nether run summon minecraft:pig 2 70 0 {NoAI:1b,NoGravity:1b,Tags:[\"nether\"]}",
    );
    let tag = "execute in minecraft:the_nether positioned 0 70 0 as @e[type=minecraft:pig,distance=..10] run tag @s add found";
    assert_eq!(count(&mut p, tag), 1, "Nether pig not tagged");
    assert_eq!(count(&mut p, "execute in minecraft:the_nether if entity @e[tag=nether,tag=found]"), 1);
    assert_eq!(count(&mut p, "execute if entity @e[tag=overworld,tag=found]"), 0, "the overworld pig tagged");
    assert_eq!(
        count(&mut p, "execute in minecraft:the_nether if entity @a[distance=..100000]"),
        0,
        "player found in the Nether"
    );
    assert_eq!(count(&mut p, "execute in minecraft:the_nether if entity @a"), 1, "player list");
    assert_eq!(count(&mut p, "execute if entity @a[distance=..100000]"), 1, "player in their own world");
}

/// `@s` never looks at a world, even with an option limited to one (`distance`): the overworld
/// pig selected by `as`, tested under `execute in the_nether`, is still found, like the game, in
/// a middle step and in a final condition. Before: the step ran in the Nether, where the pig is
/// not.
#[test]
fn current_entity_with_options_is_found_whatever_the_source_world() {
    let (mut p, _dir) = new_world("current-entity");
    run(&mut p, "summon minecraft:pig 2 70 0 {NoAI:1b,NoGravity:1b}");
    let pig_in_nether = "execute as @e[type=minecraft:pig] positioned as @s in minecraft:the_nether";
    let middle = format!("{pig_in_nether} if entity @s[distance=..1] run forceload query");
    assert!(p.s.perform(p.s.server_source(), &middle).0, "middle step: the pig's own @s not found");
    let last = format!("{pig_in_nether} if entity @s[distance=..1]");
    assert_eq!(count(&mut p, &last), 1, "final condition: the pig's own @s not found");
    assert_eq!(count(&mut p, &format!("{pig_in_nether} if entity @s")), 1, "final condition: plain @s");
}

/// Under `execute in the_nether`, a step reading or changing the world at a position reads the
/// Nether, like the game: `if block`, `summon` and `store result block` (before: the overworld's
/// block was tested, the pig summoned in the overworld, and the store refused, no chest there).
#[test]
fn execute_steps_at_a_position_use_the_source_world() {
    let (mut p, _dir) = new_world("step-world");
    run(&mut p, "execute in minecraft:the_nether run forceload add 0 0 16 0");
    p.s.in_world(NETHER, |s| s.load_now(&[(0, 0)]));
    run(&mut p, "execute in minecraft:the_nether run setblock 0 70 0 minecraft:gold_block");
    run(&mut p, "execute in minecraft:the_nether run setblock 0 71 0 minecraft:command_block");
    let nether = "execute in minecraft:the_nether";
    assert_eq!(count(&mut p, &format!("{nether} if block 0 70 0 minecraft:gold_block run forceload query")), 2);
    assert_eq!(count(&mut p, "execute if block 0 70 0 minecraft:gold_block run forceload query"), 0, "overworld");
    let summon = format!("{nether} positioned 1 72 0 summon minecraft:pig run data merge entity @s {{NoAI:1b,NoGravity:1b}}");
    assert!(p.s.perform(p.s.server_source(), &summon).0, "summon refused");
    assert_eq!(count(&mut p, &format!("{nether} positioned 1 72 0 if entity @e[type=minecraft:pig,distance=..1]")), 1);
    assert_eq!(count(&mut p, "execute positioned 1 72 0 if entity @e[type=minecraft:pig,distance=..1]"), 0, "overworld pig");
    let store = format!("{nether} store result block 0 71 0 SuccessCount int 1 run forceload query");
    assert_eq!(count(&mut p, &store), 2, "store refused");
    assert_eq!(count(&mut p, &format!("{nether} run data get block 0 71 0 SuccessCount")), 2, "stored count");
}

/// `execute in the_nether at @s`, typed by the player, takes the player's world back, like the
/// game (`at` copies the entity's world with its position): the block lands above the player in
/// the overworld. Before: the Nether kept, where the block is out of the world.
#[test]
fn execute_at_takes_the_entity_world() {
    let (mut p, _dir) = new_world("at-world");
    let mut src = p.s.player_source();
    src.level = 4;
    let (done, _) = p.s.perform(src, "execute in minecraft:the_nether at @s run setblock ~ ~3 ~ minecraft:gold_block");
    assert!(done, "setblock refused");
    let gold = p.s.default_state("minecraft:gold_block");
    assert_eq!(p.s.get([0, -57, 0]), gold, "block not set above the player in the overworld");
}

/// A global world event (wither spawn) set off in a world nobody stands in reaches the player at
/// their own position, like the game; before, 32 blocks from them towards its coordinates.
#[test]
fn a_global_event_from_another_world_is_heard_where_the_player_stands() {
    let (mut p, _dir) = new_world("global-event");
    p.s.in_world(NETHER, |s| s.global_level_event(1023, [500, 70, 0], 0));
    let heard_at = p.take_events().into_iter().find_map(|e| match e {
        Event::LevelEvent { kind: 1023, pos, global: true, .. } => Some(pos),
        _ => None,
    });
    assert_eq!(heard_at, Some([0, -60, 0]));
}
