//! G1-F16: F3 values answered by the integrated server (`debug_report.rs`), on a fresh flat world
//! (26.3 data required: absent, explicit failure). Expected values from the game's rules.

use std::path::PathBuf;
use std::sync::Arc;

use crate::blocks::probe::Probe;
use crate::newworld::{NewWorld, Preset, create_world};

/// Test world removed when the test ends, failed assertion included; fields drop in order, so the
/// server lets go of its files before the folder goes.
struct World {
    p: Probe,
    _dir: Cleanup,
}

struct Cleanup(PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn open(tag: &str) -> World {
    let data = Arc::new(rc_testdata::version_data().expect("26.3 version data missing"));
    assert!(crate::registries::client_jar(&data).is_some(), "26.3 client jar missing");
    let dir = std::env::temp_dir().join(format!("rc-g1f16-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut w = NewWorld::new(tag);
    w.seed = 1;
    w.preset = Preset::Flat;
    let dir = Cleanup(dir);
    create_world(&dir.0, &data, &w).expect("world creation");
    let mut p = Probe::open(&dir.0, data);
    p.s.pos = [0.5, -60.0, 0.5];
    let around: Vec<(i32, i32)> = (-1..=1).flat_map(|x| (-1..=1).map(move |z| (x, z))).collect();
    p.s.load_now(&around);
    p.s.loaded = true;
    World { p, _dir: dir }
}

fn cmd(p: &mut Probe, c: &str) {
    let src = p.s.server_source();
    let (ok, _) = p.s.perform(src, c);
    assert!(ok, "command refused: {c}");
}

/// Flat world (grass on top at y = −61): the four server maps give the highest block taken, a
/// leaves block raises all but the no-leaves map; unloaded chunk: no maps nor local difficulty;
/// feet above the build height: no local difficulty.
#[test]
fn server_heightmaps_and_local_difficulty() {
    let mut w = open("hm");
    let p = &mut w.p;
    let feet = [0, -60, 0];
    let r = p.s.debug_report_values(feet);
    assert_eq!(r.heightmaps, Some([-61; 4]));
    let leaves = p.s.default_state("minecraft:oak_leaves");
    p.s.set_block([0, -60, 0], leaves, 2);
    let r = p.s.debug_report_values(feet);
    assert_eq!(r.heightmaps, Some([-60, -60, -60, -61]), "S O M ML");
    // Normal, clock and inhabited time at 0: 2 × 0.75.
    assert_eq!(r.local_difficulty, Some((1.5, 0.0)));
    assert_eq!(r.local_difficulty, Some(p.s.local_difficulty(feet)));
    let far = p.s.debug_report_values([10_000, -60, 10_000]);
    assert_eq!((far.heightmaps, far.local_difficulty), (None, None));
    assert_eq!(p.s.debug_report_values([0, 400, 0]).local_difficulty, None);
}

/// Forced chunks counted, sprint seen, census of the last natural spawn pass (one player:
/// 17 × 17 spawn chunks) kept for its dimension only.
#[test]
fn forced_sprint_and_census() {
    let mut w = open("fc");
    let p = &mut w.p;
    let r = p.s.debug_report_values([0, -60, 0]);
    assert_eq!((r.forced, r.sprinting, r.spawn), (0, false, None));
    cmd(p, "forceload add 0 0 31 15");
    p.s.cmd.ticking.request_sprint(1000);
    p.s.natural_spawn_tick();
    let r = p.s.debug_report_values([0, -60, 0]);
    assert_eq!((r.forced, r.sprinting), (2, true));
    let (chunks, counts) = r.spawn.expect("census of the tick");
    assert_eq!(chunks, 289);
    assert!(counts.iter().all(|&n| n >= 0));
    // Census of another dimension: not shown.
    p.s.mobs.census.as_mut().expect("census").dim = "minecraft:the_nether".into();
    assert_eq!(p.s.debug_report_values([0, -60, 0]).spawn, None);
}

/// Micro-bench: server cost of an answer (values + exchange) and of a loop turn without a
/// request; client cost of a request.
#[test]
#[ignore = "micro-bench"]
fn bench_debug_report() {
    let mut w = open("bench");
    let p = &mut w.p;
    p.s.natural_spawn_tick();
    let (dr, dim) = (p.s.debug_report.clone(), p.s.dim.name.clone());
    let n = 200_000u32;
    let per = |t: std::time::Instant| t.elapsed().as_nanos() as f64 / f64::from(n);
    for pass in 0..2 {
        let t = std::time::Instant::now();
        let mut acc = 0usize;
        for _ in 0..n {
            acc += std::hint::black_box(p.s.debug_report_values(std::hint::black_box([0, -60, 0]))).forced;
        }
        let values = per(t);
        let t = std::time::Instant::now();
        for _ in 0..n {
            p.s.debug_report_tick();
        }
        let idle = per(t);
        let t = std::time::Instant::now();
        for _ in 0..n {
            acc += usize::from(dr.request(&dim, [0, -60, 0]).is_some());
            p.s.debug_report_tick();
        }
        let exchange = per(t);
        eprintln!(
            "pass {pass}: values {values:.0} ns, turn without request {idle:.0} ns, request + answer {exchange:.0} ns ({acc})"
        );
    }
}

/// Exchange through the shared slot: the server answers a request at its loop turn, once.
#[test]
fn request_answered_at_loop_turn() {
    let mut w = open("req");
    let p = &mut w.p;
    let dr = p.s.debug_report.clone();
    let dim = p.s.dim.name.clone();
    assert_eq!(dr.request(&dim, [0, -60, 0]), None);
    p.s.debug_report_tick();
    let r = dr.request(&dim, [0, -60, 0]).expect("answer");
    assert_eq!(r.heightmaps, Some([-61; 4]));
}
