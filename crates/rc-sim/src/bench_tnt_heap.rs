//! Bench of a mass explosion: the bench's `tnt-heap` scene rebuilt in the flat test world, a
//! heap of 12 × 12 × 12 TNT blocks (1,728) set off from the top by a redstone block, run until no
//! primed TNT is left. Reported per configuration of the visible-part and object-pass switches:
//! every tick of the chain (sum, worst, median of the ticks with an explosion), the explosions,
//! and a digest of what the chain left (blocks, items, primed TNT), the same for every
//! configuration.
//!
//! `cargo test --release -p rc-sim --lib bench_tnt_heap -- --ignored --nocapture --test-threads=1`.

use crate::bench_kit::{cmd, median, report};
use crate::blocks::entities::EntKind;
use crate::blocks::probe::Probe;
use crate::mobs::visible_part::{FORCED_VISIBLE_PART_PATHS, VisiblePartPaths};

/// Corner of the heap on the grass of the flat world (ground surface at y = -60).
pub(crate) const HEAP_CORNER: [i32; 3] = [40, -60, 4];
/// Side of the heap, in blocks.
pub(crate) const HEAP_SIDE: i32 = 12;

/// The heap filled and the redstone block set on its top centre (which primes the TNT below it).
pub(crate) fn build_and_set_off_heap(p: &mut Probe) {
    let [x, y, z] = HEAP_CORNER;
    let n = HEAP_SIDE - 1;
    cmd(p, &format!("fill {x} {y} {z} {} {} {} minecraft:tnt", x + n, y + n, z + n));
    let top = [x + HEAP_SIDE / 2, y + HEAP_SIDE, z + HEAP_SIDE / 2];
    cmd(p, &format!("setblock {} {} {} minecraft:redstone_block", top[0], top[1], top[2]));
}

pub(crate) fn primed_tnt_left(p: &Probe) -> bool {
    p.s.bs.entities.list.iter().any(|e| !e.removed && matches!(e.kind, EntKind::Tnt(_)))
}

/// One tick, the events to the client thrown away; microseconds.
fn timed_tick(p: &mut Probe) -> f64 {
    let start = std::time::Instant::now();
    p.s.game_tick();
    p.s.tick += 1;
    let us = start.elapsed().as_secs_f64() * 1e6;
    let _ = p.take_events();
    us
}

/// What a run of the chain measured.
pub(crate) struct HeapRun {
    /// Microseconds of each tick with at least one explosion.
    pub blast_ticks: Vec<f64>,
    pub all_ticks_us: f64,
    pub explosions: usize,
}

/// The chain run from the setting off until no primed TNT is left (at most `max_ticks`).
pub(crate) fn run_heap_chain(p: &mut Probe, max_ticks: u32) -> HeapRun {
    let mut run = HeapRun { blast_ticks: Vec::new(), all_ticks_us: 0.0, explosions: 0 };
    for tick in 0..max_ticks {
        let us = timed_tick(p);
        let blasts = crate::mobs::explode::TNT_BLASTS.with_borrow_mut(std::mem::take).len();
        crate::mobs::explode::BLAST_CELLS.with_borrow_mut(Vec::clear);
        run.all_ticks_us += us;
        if blasts > 0 {
            run.explosions += blasts;
            run.blast_ticks.push(us);
        }
        if tick > 10 && !primed_tnt_left(p) {
            break;
        }
    }
    run
}

/// The heap chain in a fresh world on a thread of its own (the test random seeds start afresh,
/// so every run plays the same chain), `before` run first on that thread.
pub(crate) fn heap_chain_on_own_thread<T: Send + 'static>(
    tag: String,
    before: impl FnOnce() + Send + 'static,
    after: impl FnOnce(&Probe, HeapRun) -> T + Send + 'static,
) -> T {
    std::thread::Builder::new()
        .stack_size(32 << 20)
        .spawn(move || {
            before();
            let (mut p, _dir) = crate::tests_g3f13::open(&tag);
            p.s.game_mode = 1;
            build_and_set_off_heap(&mut p);
            let run = run_heap_chain(&mut p, 1200);
            after(&p, run)
        })
        .expect("heap thread")
        .join()
        .expect("heap chain")
}

fn report_run(case: &str, p: &Probe, run: &HeapRun) {
    let worst = run.blast_ticks.iter().copied().fold(0.0, f64::max);
    report("tnt_heap", case, "explosions", run.explosions as f64);
    report("tnt_heap", case, "chain_ms", run.all_ticks_us / 1000.0);
    report("tnt_heap", case, "worst_tick_ms", worst / 1000.0);
    report("tnt_heap", case, "median_blast_tick_ms", median(&run.blast_ticks) / 1000.0);
    report("tnt_heap", case, "items_left", p.s.life.drops.iter().filter(|d| !d.is_removed()).count() as f64);
}

/// The configurations the bench compares: every former path, the objects in one pass alone,
/// then with each visible-part path, every default but the threads, the threads on 2 threads
/// and powers of 2 up to `RUSTCRAFT_THREADS` or every logical core (from 32 distinct boxes), on
/// every core from 8, 64 and 128, and the cube per call on top of the defaults.
fn bench_configurations() -> Vec<(String, VisiblePartPaths)> {
    let old = VisiblePartPaths::OLD;
    let one_pass = VisiblePartPaths { objects_one_pass: true, ..old };
    let no_threads = VisiblePartPaths { parallel: false, ..VisiblePartPaths::NEW };
    let mut configs = vec![
        ("former".to_owned(), old),
        ("objects one pass".to_owned(), one_pass),
        ("one pass, cell cube".to_owned(), VisiblePartPaths { cell_cube: true, ..one_pass }),
        ("one pass, air shortcut".to_owned(), VisiblePartPaths { air_shortcut: true, ..one_pass }),
        ("one pass, same box once".to_owned(), VisiblePartPaths { same_box_once: true, ..one_pass }),
        ("defaults but threads".to_owned(), no_threads),
        ("defaults but threads, cube per call".to_owned(), VisiblePartPaths { cube_per_call: true, ..no_threads }),
    ];
    let cores = rc_data::env_str!("RUSTCRAFT_THREADS")
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, std::num::NonZero::get));
    let mut threads = 2;
    while threads <= cores {
        configs.push((format!("threads {threads} from 32"), VisiblePartPaths { threads, ..VisiblePartPaths::NEW }));
        threads *= 2;
    }
    for from in [8, 64, 128] {
        let paths = VisiblePartPaths { threads: cores, parallel_min_targets: from, ..VisiblePartPaths::NEW };
        configs.push((format!("threads {cores} from {from}"), paths));
    }
    configs
}

#[test]
#[ignore = "measurement bench"]
fn bench_tnt_heap() {
    for pass in 0..2 {
        for (label, paths) in bench_configurations() {
            let case = format!("pass{pass} {label}");
            let before = move || FORCED_VISIBLE_PART_PATHS.set(Some(paths));
            heap_chain_on_own_thread(format!("tnt-heap-bench-{pass}"), before, move |p, run| report_run(&case, p, &run));
        }
    }
}

/// Bench (ignored): one blast push among 2,000 items lying on the ground, about 300 of them in
/// reach (the push of a wind burst: velocities only, so every call meets the same items), for the
/// object pass one by one (each item in reach searched by identifier), in one pass, and with the
/// visible-part paths. The configurations alternate over 7 rounds, the best kept.
#[test]
#[ignore = "measurement bench"]
fn bench_object_pass_among_items() {
    let (mut p, _dir) = crate::tests_g3f13::open("object-pass-items");
    for i in 0..2000 {
        let (x, z) = (20.0 + f64::from(i % 50) * 0.8, f64::from(i / 50) * 0.8 - 8.0);
        let item = "{id:\"minecraft:stone\",count:1}";
        cmd(&mut p, &format!("summon minecraft:item {x:.2} -60 {z:.2} {{Item:{item},PickupDelay:32767s,NoGravity:1b}}"));
    }
    let old = VisiblePartPaths::OLD;
    let configs = [
        ("one by one", old),
        ("one pass", VisiblePartPaths { objects_one_pass: true, ..old }),
        ("defaults but threads", VisiblePartPaths { parallel: false, ..VisiblePartPaths::NEW }),
        ("defaults", VisiblePartPaths::NEW),
    ];
    let mut best = [f64::INFINITY; 4];
    for _ in 0..7 {
        for ((_, paths), best) in configs.iter().zip(&mut best) {
            FORCED_VISIBLE_PART_PATHS.set(Some(*paths));
            let start = std::time::Instant::now();
            for _ in 0..20 {
                p.s.blast_push_objects([40.0, -59.5, 8.0], 4.0, 1.0, true);
            }
            *best = best.min(start.elapsed().as_secs_f64() * 1e6 / 20.0);
        }
    }
    FORCED_VISIBLE_PART_PATHS.set(None);
    for ((label, _), us) in configs.iter().zip(best) {
        report("object_pass_items", label, "us_per_push", us);
    }
}
