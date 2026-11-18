//! Advancement micro-bench (`docs/perf.md`, advancements): each optimised path against the former
//! one, on a fresh flat world with the 26.3 tree, a fresh player and a half-advanced one:
//! - inventory: one slot changed, 36 slots changed (shift-click of a full inventory), 41 slots
//!   emptied (death, `/clear`), `RUSTCRAFT_OPT_ADVINDEX` off and on, then `RUSTCRAFT_OPT_IDEQ` read;
//! - recipes: every recipe learned at once (`/recipe give @s *`), `RUSTCRAFT_OPT_RECIPEINDEX`;
//! - location: the `location` trigger (every 20 ticks), `RUSTCRAFT_OPT_LOCATIONFAST`;
//! - loading: a complete advancements file read, the visibility walk, the tree parsed on 1 thread
//!   then powers of 2 up to every core, `RUSTCRAFT_OPT_ADVLOADFAST`.
//!
//! µs (or ns) per event, allocations per event (per-thread counter of the test allocator). Two
//! alternating passes of every case. Run alone: `cargo test --release -p rc-sim
//! bench_advancement_triggers -- --ignored --nocapture --test-threads=1`.

use std::time::Instant;

use crate::bench_kit::{median, report};
use crate::blocks::probe::Probe;
use crate::player::Stack;
use crate::progress::ProgressPaths;

/// Fresh flat world on `paths`, every other advancement done when `half_done`.
fn open(tag: &str, paths: ProgressPaths, half_done: bool) -> (Probe, std::path::PathBuf) {
    let (mut p, dir) = ProgressPaths::forced(paths, || crate::tests_g3f13::open(tag));
    if half_done {
        for a in (0..p.s.prog.tree.list.len()).step_by(2) {
            for c in 0..p.s.prog.tree.list[a].criteria.len() {
                p.s.prog.grant(a, c);
            }
        }
    }
    p.s.prog.grant_log.clear();
    p.s.prog.inv_seen = p.s.slots.clone();
    (p, dir)
}

fn player(half_done: bool) -> &'static str {
    if half_done { "half" } else { "fresh" }
}

fn case_paths() -> [(&'static str, ProgressPaths); 2] {
    [("old", ProgressPaths::OLD), ("new", ProgressPaths::NEW)]
}

/// Median of `runs` timings of `f` (µs per event) and its allocations per event.
fn time(events: usize, runs: usize, mut f: impl FnMut()) -> (f64, f64) {
    let mut us = Vec::new();
    let mut allocs = Vec::new();
    for _ in 0..runs {
        let (start, a0) = (Instant::now(), crate::test_alloc::allocations());
        f();
        us.push(start.elapsed().as_secs_f64() * 1e6 / events as f64);
        allocs.push((crate::test_alloc::allocations() - a0) as f64 / events as f64);
    }
    (median(&us), median(&allocs))
}

/// Stacks that no criterion of 26.3 asks for (no advancement granted while timing).
fn neutral(k: usize) -> Stack {
    let ids = ["minecraft:dirt_path", "minecraft:barrier", "minecraft:structure_void", "minecraft:light"];
    Stack { id: ids[k % ids.len()].into(), count: 1, components: None }
}

fn inventory(pass: usize) {
    for half_done in [false, true] {
        for (case, paths) in case_paths() {
            let (mut p, dir) = open(&format!("bench-adv-inv-{case}-{pass}"), paths, half_done);
            let tag = format!("{}-{case}", player(half_done));
            // One changed slot: the event of a pickup.
            let s = neutral(0);
            let (us, allocs) = time(2_000, 5, || {
                for _ in 0..2_000 {
                    p.s.trig_inventory_changed(Some(&s));
                }
            });
            report("advancements", &format!("one-slot-{tag}"), "us_per_event", us);
            report("advancements", &format!("one-slot-{tag}"), "allocs_per_event", allocs);
            // 36 slots filled then emptied through the slot scan (shift-click, death).
            let (us, allocs) = time(100, 5, || {
                for round in 0..50 {
                    for (k, m) in (9..45).enumerate() {
                        p.s.slots[m] = Some(neutral(k + round));
                    }
                    p.s.trig_inventory_scan();
                    for m in 5..46 {
                        p.s.slots[m] = None;
                    }
                    p.s.trig_inventory_scan();
                }
            });
            report("advancements", &format!("fill-36-then-empty-41-{tag}"), "us_per_scan", us);
            report("advancements", &format!("fill-36-then-empty-41-{tag}"), "allocs_per_scan", allocs);
            assert!(p.s.prog.grant_log.len() < 10, "{tag}: the neutral stacks granted criteria");
            crate::tests_g3f13::finish(p, dir);
        }
    }
}

fn recipes(pass: usize) {
    for (case, paths) in case_paths() {
        let (mut p, dir) = open(&format!("bench-adv-recipes-{case}-{pass}"), paths, false);
        let ids: Vec<String> = p.s.life.recipes.list.iter().map(|r| r.id.clone()).collect();
        let start = Instant::now();
        p.s.trig_recipes_unlocked(&ids);
        let ms = start.elapsed().as_secs_f64() * 1e3;
        report("advancements", &format!("recipes-unlocked-all-{case}"), "ms", ms);
        report("advancements", &format!("recipes-unlocked-all-{case}"), "granted", p.s.prog.grant_log.len() as f64);
        crate::tests_g3f13::finish(p, dir);
    }
}

fn location(pass: usize) {
    for half_done in [false, true] {
        for (case, paths) in case_paths() {
            let (mut p, dir) = open(&format!("bench-adv-loc-{case}-{pass}"), paths, half_done);
            let (us, allocs) = time(500, 5, || {
                for _ in 0..500 {
                    p.s.trigger("minecraft:location", &|_, _| true);
                }
            });
            let tag = format!("location-{}-{case}", player(half_done));
            report("advancements", &tag, "us_per_event", us);
            report("advancements", &tag, "allocs_per_event", allocs);
            crate::tests_g3f13::finish(p, dir);
        }
    }
}

fn loading(pass: usize) {
    let (p, dir) = open(&format!("bench-adv-load-{pass}"), ProgressPaths::NEW, false);
    let tree = p.s.prog.tree.clone();
    let world = dir.join("bench-progress");
    // A complete file, granted in a shuffled order.
    let mut all: Vec<(usize, usize)> =
        tree.list.iter().enumerate().flat_map(|(a, adv)| (0..adv.criteria.len()).map(move |c| (a, c))).collect();
    let len = all.len();
    all = (0..len).map(|i| all[(i * 7919) % len]).collect();
    let mut full =
        ProgressPaths::forced(ProgressPaths::NEW, || crate::progress::Progress::new(tree.clone(), &world, 9));
    for &(a, c) in &all {
        full.grant(a, c);
    }
    let file = world.join("players").join("advancements").join(format!("{}.json", crate::nbtio::uuid_text(9)));
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, full.to_json(4000)).unwrap();
    for (case, paths) in case_paths() {
        let (us, allocs) = time(1, 5, || {
            let mut read = ProgressPaths::forced(paths, || crate::progress::Progress::new(tree.clone(), &world, 9));
            assert_eq!(read.order.len(), tree.list.len());
            let _ = read.flush();
        });
        report("advancements", &format!("read-complete-file-and-show-{case}"), "us", us);
        report("advancements", &format!("read-complete-file-and-show-{case}"), "allocs", allocs);
    }
    // The tree: parsed in order, then on 2, 4… helper threads up to every core; then the same with
    // every thread of the global pool busy, as at world opening when the generator started just
    // before fills it.
    let jar = crate::registries::client_jar(&p.s.data).expect("26.3 client jar");
    let jar = crate::registries::Jar::read(&jar).expect("jar");
    let cores = crate::helper_pool::logical_cores();
    for busy in [false, true] {
        let label = if busy { "-global-pool-busy" } else { "" };
        let _busy = busy.then(GlobalPoolBusy::start);
        let in_order = || drop(ProgressPaths::forced(ProgressPaths::OLD, || crate::progress::Tree::load(&jar)));
        let (us, _) = time(1, 3, in_order);
        report("advancements", &format!("tree-load-in-order{label}"), "ms", us / 1e3);
        let mut threads = 2;
        while threads <= cores {
            let (us, _) = time(1, 3, || drop(crate::progress::Tree::load_on(&jar, threads)));
            report("advancements", &format!("tree-load-parallel-{threads}{label}"), "ms", us / 1e3);
            threads = if threads * 2 > cores && threads < cores { cores } else { threads * 2 };
        }
    }
    crate::tests_g3f13::finish(p, dir);
}

/// Every thread of the global pool spinning until dropped.
struct GlobalPoolBusy(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl GlobalPoolBusy {
    fn start() -> GlobalPoolBusy {
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        for _ in 0..rayon::current_num_threads() {
            let stop = stop.clone();
            rayon::spawn(move || {
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    std::hint::spin_loop();
                }
            });
        }
        GlobalPoolBusy(stop)
    }
}

impl Drop for GlobalPoolBusy {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

#[test]
#[ignore = "micro-bench: run alone on a quiet machine"]
fn bench_advancement_triggers() {
    for pass in 0..2 {
        inventory(pass);
        recipes(pass);
        location(pass);
        loading(pass);
    }
}
