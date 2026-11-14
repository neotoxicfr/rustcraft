//! The "redstone experiments" feature of a world: read from its enabled features, every
//! neighbour update then draws an orientation on the level's random, and dust power goes
//! through the second evaluator, which works the new powers out first and writes each dust once
//! (the usual one rewrites a line of dust many times while it fades). Before, the feature was
//! shown on the world but the server kept the usual behaviour. Fresh flat worlds.

use std::path::PathBuf;
use std::sync::Arc;

use crate::blocks::probe::Probe;
use crate::newworld::{NewWorld, Preset, create_world};
use crate::tests_g3f13::{finish, reopen};

/// Flat world (dust at y = -60 on the grass), with the redstone experiments when `experiments`.
fn open(tag: &str, experiments: bool) -> (Probe, PathBuf) {
    let data = Arc::new(rc_testdata::version_data().expect("26.3 version data missing"));
    assert!(crate::registries::client_jar(&data).is_some(), "26.3 client jar missing");
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-redstone-exp-{tag}-{}", std::process::id())));
    let mut w = NewWorld::new(tag);
    w.seed = 1;
    w.preset = Preset::Flat;
    create_world(&dir, &data, &w).expect("world creation");
    if experiments {
        let packs = ["vanilla".to_owned(), "redstone_experiments".to_owned()];
        let features = ["minecraft:vanilla".to_owned(), "minecraft:redstone_experiments".to_owned()];
        crate::storage::set_data_config(&dir, &packs, &[], &features).expect("data packs");
    }
    (reopen(&dir, data), dir)
}

fn cmd(p: &mut Probe, c: &str) {
    let src = p.s.server_source();
    let (ok, _) = p.s.perform(src, c);
    assert!(ok, "command refused: {c}");
}

#[test]
fn experiment_flag_follows_the_world_features() {
    let (p, dir) = open("flag-on", true);
    assert!(p.s.redstone_experiments, "features: {:?}", p.s.features);
    finish(p, dir);
    let (p, dir) = open("flag-off", false);
    assert!(!p.s.redstone_experiments);
    finish(p, dir);
}

/// Under the experiments a series of neighbour updates draws one orientation (one bounded draw
/// of the level's random); without them, nothing is drawn.
#[test]
fn neighbour_updates_draw_an_orientation_only_under_the_experiments() {
    for experiments in [true, false] {
        let (mut p, dir) = open(if experiments { "draw-on" } else { "draw-off" }, experiments);
        let mut expected = p.s.bs.rng.clone();
        if experiments {
            let _ = expected.next_int_bound(48);
        }
        let stone = p.s.view().block(p.s.default_state("minecraft:stone"));
        p.s.update_neighbors_at([0, 10, 0], stone);
        assert_eq!(p.s.bs.rng.next_int(), expected.next_int(), "experiments: {experiments}");
        finish(p, dir);
    }
}

/// The orientation a neighbour job carries fits in its spare bytes: a job stays 40 bytes (the
/// shape update, the largest), as before it carried one.
#[test]
fn neighbour_jobs_keep_their_size() {
    assert_eq!(std::mem::size_of::<crate::blocks::level::Job>(), 40);
    assert_eq!(std::mem::size_of::<Option<crate::blocks::orientation::Orientation>>(), 2);
}

/// A stone block at `b` with a lever on its north face (lever facing north), the `dust` on top
/// of it, and the extra `blocks`; then the lever is pulled. Returns the pistons, in the order
/// their extend events were queued.
fn pistons_fired_by_lever(p: &mut Probe, b: [i32; 3], dust: &str, blocks: &[(&str, [i32; 3])]) -> Vec<[i32; 3]> {
    let [x, y, z] = b;
    cmd(p, &format!("setblock {x} {y} {z} minecraft:stone"));
    cmd(p, &format!("setblock {x} {} {z} {dust}", y + 1));
    for (block, [bx, by, bz]) in blocks {
        cmd(p, &format!("setblock {bx} {by} {bz} {block}"));
    }
    let lever = [x, y, z - 1];
    cmd(p, &format!("setblock {x} {y} {} minecraft:lever[face=wall,facing=north]", z - 1));
    p.s.bs.events.clear();
    p.s.lever_pull(lever, p.s.get(lever));
    let piston = p.s.default_state("minecraft:piston");
    let is_piston = |q: [i32; 3]| p.s.view().block(p.s.get(q)) == p.s.view().block(piston);
    p.s.bs.events.iter().map(|e| e.pos).filter(|&q| is_piston(q)).collect()
}

/// Under the experiments the dust walks its neighbours in its orientation's order. A wall lever
/// facing north gives the update an orientation fully set by the lever (up, then front south),
/// which the stone passes up to the dust turned to face up; the dust's evaluator turns it back
/// up: up, front south, bias left. Its neighbour order is then north, south, east, west, below:
/// the piston east of the dust gets its update before the one west of it (in the plain
/// direction order, west comes first). Its spread order (north, south, east, west) meets the
/// dust south of it before the one east of it, so the south branch is written, and updates its
/// piston, first (in the plain plane order, east comes before south).
#[test]
fn dust_updates_follow_the_orientation_order() {
    let (mut p, dir) = open("order", true);
    let up = "minecraft:piston[facing=up]";
    // One dust, a cross pointing every way, between two pistons, east and west.
    let cross = "minecraft:redstone_wire[north=side,south=side,east=side,west=side]";
    let fired = pistons_fired_by_lever(&mut p, [10, -59, 0], cross, &[(up, [11, -58, 0]), (up, [9, -58, 0])]);
    assert_eq!(fired, [[11, -58, 0], [9, -58, 0]], "neighbour order");
    // Two branches, south and east, each ending at a piston.
    let branches = [
        ("minecraft:stone", [20, -59, 1]),
        ("minecraft:redstone_wire", [20, -58, 1]),
        (up, [20, -58, 2]),
        ("minecraft:stone", [21, -59, 0]),
        ("minecraft:redstone_wire", [21, -58, 0]),
        (up, [22, -58, 0]),
    ];
    let fired = pistons_fired_by_lever(&mut p, [20, -59, 0], "minecraft:redstone_wire", &branches);
    assert_eq!(fired, [[20, -58, 2], [22, -58, 0]], "spread order");
    finish(p, dir);
}

/// Positions of the 15 dusts of the line, east of a redstone block at x = 9.
fn line() -> Vec<[i32; 3]> {
    (10..25).map(|x| [x, -60, 0]).collect()
}

/// A line of 15 dusts loses its power: same powers in the end with both evaluators (all 15 to 1,
/// then all 0), but the second one writes each dust exactly once while it fades.
#[test]
fn second_evaluator_writes_each_dust_once() {
    for experiments in [true, false] {
        let (mut p, dir) = open(if experiments { "line-on" } else { "line-off" }, experiments);
        cmd(&mut p, "fill 10 -60 0 24 -60 0 minecraft:redstone_wire");
        cmd(&mut p, "setblock 9 -60 0 minecraft:redstone_block");
        let power = |p: &Probe, q: [i32; 3]| p.s.view().get_int(p.s.get(q), rc_data::PropKey::Power);
        let lit: Vec<i32> = line().iter().map(|&q| power(&p, q)).collect();
        assert_eq!(lit, (1..=15).rev().collect::<Vec<i32>>(), "experiments: {experiments}");
        p.s.changed.clear();
        cmd(&mut p, "setblock 9 -60 0 minecraft:air");
        assert!(line().iter().all(|&q| power(&p, q) == 0), "experiments: {experiments}");
        let writes: Vec<usize> = line().iter().map(|q| p.s.changed.iter().filter(|c| *c == q).count()).collect();
        let most_writes = writes.iter().copied().max().unwrap_or(0);
        // A count, shown (not checked beyond the rule below): writes of the fading line.
        eprintln!("experiments {experiments}: {} dust writes, at most {most_writes} per dust", writes.iter().sum::<usize>());
        if experiments {
            assert_eq!(most_writes, 1, "a dust written more than once");
        } else {
            assert!(most_writes > 1, "the usual evaluator rewrites the fading line");
        }
        finish(p, dir);
    }
}
