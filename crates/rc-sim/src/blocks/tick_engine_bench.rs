//! Micro-benches of the world tick engine (`ticks.rs`), former path against new path: ticking
//! chunks, scheduled-tick collection, random-tick section test, ticker walk, tick export of a
//! save. Measurements, not proofs (see `tick_engine_tests`): run alone, quiet machine,
//! `cargo test --release -p rc-sim tick_engine_bench -- --ignored --nocapture --test-threads 1`.
//! Everything measured here runs on the server thread, one core: no thread count to vary.

use std::hint::black_box;
use std::time::Instant;

use rc_world::{Container, Section};

use super::ticks::{BlockTickPaths, TickQueue, TickingChunks, section_has_random_ticks};
use crate::tests_g3f13::{finish, open};

/// Median time of one run of `f` over `samples` samples of `batch` runs each, in microseconds
/// (batches: the clock cannot time a call of a few nanoseconds).
fn median_us(samples: usize, batch: usize, mut f: impl FnMut()) -> f64 {
    let mut times: Vec<f64> = (0..samples)
        .map(|_| {
            let t = Instant::now();
            for _ in 0..batch {
                f();
            }
            t.elapsed().as_secs_f64() * 1e6 / batch as f64
        })
        .collect();
    times.sort_by(f64::total_cmp);
    times[times.len() / 2]
}

/// Ticking chunks and random-tick chunks of one tick, scanned against listed, for 1,000 to 20,000
/// loaded chunks (light copies of one flat chunk around the player) and simulation distances 6,
/// 12 and 32.
#[test]
#[ignore = "measurement bench"]
fn bench_ticking_chunks() {
    let (mut p, dir) = open("bench-ticking-chunks");
    let mut model = p.s.chunks.get(&(0, 0)).and_then(Clone::clone).expect("chunk (0, 0)");
    // Only its presence matters here: no light or block entity copied 20,000 times.
    model.sky.clear();
    model.block.clear();
    model.block_entities.clear();
    for loaded in [1_000i32, 5_000, 20_000] {
        let side = f64::from(loaded).sqrt() as i32;
        p.s.chunks.clear();
        for x in -side / 2..side - side / 2 {
            for z in -side / 2..side - side / 2 {
                let mut c = model.clone();
                (c.x, c.z) = (x, z);
                p.s.chunks.insert((x, z), Some(c));
            }
        }
        for sim in [6, 12, 32] {
            p.s.sim_dist = sim;
            let centers = p.s.sim_centers();
            // The three variants alternate sample by sample (a loaded machine drifts between runs).
            let variants = [
                ("scan", BlockTickPaths::OLD),
                ("listed", BlockTickPaths { loaded_grid: false, ..BlockTickPaths::NEW }),
                ("listed with the loaded grid", BlockTickPaths::NEW),
            ];
            let mut times = [Vec::new(), Vec::new(), Vec::new()];
            for _ in 0..41 {
                for (k, (_, paths)) in variants.iter().enumerate() {
                    let t = Instant::now();
                    paths.run(|| {
                        let ticking = p.s.ticking_chunks(&centers, paths.simulated_candidates);
                        black_box(p.s.random_tick_chunks(&ticking, &centers));
                    });
                    times[k].push(t.elapsed().as_secs_f64() * 1e6);
                }
            }
            let line: Vec<String> = variants
                .iter()
                .zip(&mut times)
                .map(|((label, _), t)| {
                    t.sort_by(f64::total_cmp);
                    format!("{label} {:.1} µs", t[t.len() / 2])
                })
                .collect();
            eprintln!("bench_ticking_chunks loaded {} simulation {sim}: {}", p.s.chunks.len(), line.join(", "));
        }
    }
    finish(p, dir);
}

/// Collection with nothing due in the 49 ticking chunks and 1 to 3,000 chunks left behind with
/// due ticks (fluids of a flight): former copy of the keys against the listed lookup.
#[test]
#[ignore = "measurement bench"]
fn bench_tick_queue_collect() {
    let mut list: Vec<(i32, i32)> = (-3..=3).flat_map(|x| (-3..=3).map(move |z| (x, z))).collect();
    list.sort_unstable();
    let ticking = TickingChunks::Sorted(list);
    for left in [1, 100, 1_000, 3_000] {
        let mut q = TickQueue::default();
        let mut sub = 0;
        for i in 0..left {
            let (cx, cz) = (100 + i % 60, 100 + i / 60);
            for k in 0..4 {
                sub += 1;
                q.schedule([cx * 16 + k, 64, cz * 16], 1, 5, 0, sub);
            }
        }
        let mut time = |by_candidates: bool| {
            median_us(201, 100, || {
                q.collect(10, 65536, &ticking, by_candidates);
                black_box(q.pop_next());
                q.cleanup();
            })
        };
        let (old, new) = (time(false), time(true));
        eprintln!("bench_tick_queue_collect {left} chunks left behind: key copy {old:.2} µs, listed {new:.2} µs");
    }
}

/// Random-tick test of one section: stone, stone with ores, surface, Nether with lava; cell scan
/// against palette first.
#[test]
#[ignore = "measurement bench"]
fn bench_section_random_ticks() {
    let (p, dir) = open("bench-random-sections");
    let state = |name: &str| p.s.view().default_state(p.s.bs.t.block_id(name));
    let v = p.s.view();
    let random = |s: u32| v.random_ticks(s) || p.s.fluid_of(s).kind == 2;
    let packed = |states: &[u32], cells: &dyn Fn(usize) -> usize| {
        let values: Vec<u32> = (0..4096).map(|i| states[cells(i)]).collect();
        Section { non_empty: 4096, blocks: Container::pack(&values, 4, 8, 15), biomes: Container::Single(0) }
    };
    let stone = state("minecraft:stone");
    let ores = [stone, state("minecraft:iron_ore"), state("minecraft:coal_ore"), state("minecraft:copper_ore")];
    let sections = [
        ("stone", Section { non_empty: 4096, blocks: Container::Single(stone), biomes: Container::Single(0) }),
        ("stone with ores", packed(&ores, &|i| usize::from(i % 13 == 0) + usize::from(i % 29 == 0) * 2)),
        ("surface", packed(&[state("minecraft:dirt"), state("minecraft:grass_block")], &|i| usize::from(i >= 3840))),
        ("Nether", packed(&[state("minecraft:netherrack"), state("minecraft:lava")], &|i| usize::from(i == 4000))),
    ];
    for (label, sec) in &sections {
        let time = |palette: bool| {
            median_us(201, 100, || {
                black_box(section_has_random_ticks(black_box(sec), palette, random));
            })
        };
        let (scan, palette) = (time(false), time(true));
        eprintln!("bench_section_random_ticks {label}: cell scan {scan:.3} µs, palette first {palette:.3} µs");
    }
    finish(p, dir);
}

/// Ticker walk with 1,000 daylight detectors (hopper family, a tick that does almost nothing, so
/// the walk itself shows), 100 of them removed and 100 placed again before each walk, among
/// beehives, open shulker boxes and furnaces (the other families' maps are not empty): former
/// removal one by one against the compacting walk, the family dispatch, both, alternated walk by
/// walk in one world (the registration set is not asked here, so switching paths between walks
/// is harmless).
#[test]
#[ignore = "measurement bench"]
fn bench_ticker_walk() {
    let (mut p, dir) = open("bench-ticker-walk");
    p.s.sim_dist = 8;
    let at = |i: i32| [i % 40 - 20, -59, i / 40 - 12];
    let set = |p: &mut crate::blocks::probe::Probe, i: i32, block: &str| {
        let [x, y, z] = at(i);
        let src = p.s.server_source();
        let _ = p.s.perform(src, &format!("setblock {x} {y} {z} minecraft:{block}"));
    };
    for i in 0..1_000 {
        set(&mut p, i, "daylight_detector");
    }
    for i in 0..20 {
        for (z, block) in [(16, "beehive"), (18, "shulker_box"), (20, "furnace")] {
            let src = p.s.server_source();
            let _ = p.s.perform(src, &format!("setblock {} -59 {z} minecraft:{block}", 2 * i - 20));
        }
        p.s.shulker_lid_event([2 * i - 20, -59, 18], 1);
    }
    let variants = [
        ("former", BlockTickPaths::OLD),
        ("compaction", BlockTickPaths { ticker_compaction: true, ..BlockTickPaths::OLD }),
        ("family", BlockTickPaths { ticker_family: true, ..BlockTickPaths::OLD }),
        ("compaction and family", BlockTickPaths::NEW),
    ];
    let mut times = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    for round in 0..240 {
        // 100 die (removed), 100 come back (the ones removed the round before).
        for k in 0..100 {
            set(&mut p, (round * 100 + k) % 1_000, "air");
            set(&mut p, ((round + 9) * 100 + k) % 1_000, "daylight_detector");
        }
        let k = round as usize % variants.len();
        let t = Instant::now();
        variants[k].1.run(|| p.s.tick_block_entities());
        times[k].push(t.elapsed().as_secs_f64() * 1e6);
    }
    finish(p, dir);
    for ((label, _), t) in variants.iter().zip(&mut times) {
        t.sort_by(f64::total_cmp);
        eprintln!("bench_ticker_walk {label}: {:.1} µs per walk (1,000 detectors, 100 dying)", t[t.len() / 2]);
    }
}

/// Tick export of 100 chunks holding 50 block ticks and 50 fluid ticks each, against the copy of
/// every block name the former export made per chunk.
#[test]
#[ignore = "measurement bench"]
fn bench_export_chunk_ticks() {
    let (mut p, dir) = open("bench-export-ticks");
    let chunks: Vec<(i32, i32)> = (-2..=7).flat_map(|x| (-2..=7).map(move |z| (x, z))).collect();
    p.s.load_now(&chunks);
    let mut sub = 0;
    for &(cx, cz) in &chunks {
        for k in 0..50 {
            sub += 1;
            let pos = [cx * 16 + k % 16, -60 + k / 16, cz * 16];
            p.s.bs.block_ticks.schedule(pos, 1 + k as u32, 100, 0, sub);
            p.s.bs.fluid_ticks.schedule(pos, 1, 100, 0, sub);
        }
    }
    let export = median_us(51, 1, || {
        for &ch in &chunks {
            p.s.export_chunk_ticks(ch);
        }
    });
    let names = median_us(51, 1, || {
        for _ in &chunks {
            black_box(p.s.data.blocks.iter().map(|b| b.name.clone()).collect::<Vec<String>>());
        }
    });
    eprintln!("bench_export_chunk_ticks 100 chunks: export {export:.1} µs; the former name copies alone {names:.1} µs");
    finish(p, dir);
}
