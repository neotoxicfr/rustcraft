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
        let most_writes = line().iter().map(|q| p.s.changed.iter().filter(|c| *c == q).count()).max().unwrap_or(0);
        if experiments {
            assert_eq!(most_writes, 1, "a dust written more than once");
        } else {
            assert!(most_writes > 1, "the usual evaluator rewrites the fading line");
        }
        finish(p, dir);
    }
}
