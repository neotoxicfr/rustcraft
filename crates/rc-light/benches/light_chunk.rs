//! Bench of the full light of a new chunk on the real columns of the reference capture
//! (outside the repository, the same as the meshing bench). First: output identical, byte
//! for byte, to the frozen copy (`rc_light::reference`) on all columns, then difference
//! counted against the official game's light recorded in the capture. Then: time per
//! chunk on one thread, then 1, 2, 4… threads up to the number of logical cores.
//!
//! `cargo bench -p rc-light --bench light_chunk`; `--features memstats` for the allocations;
//! `RUSTCRAFT_OPT_REQUIRE=1`: capture absent or unknown = failure.

mod common;

use common::*;
use criterion::{Criterion, criterion_group, criterion_main};
use hashbrown::HashMap;
use rayon::prelude::*;
use rc_light::{ColumnScan, Columns, FullOpts, LightTables};
use rc_world::LightLayer;
use std::hint::black_box;

/// Recognised captures: (capture fingerprint, fingerprint of the computed light).
const KNOWN: &[(u64, u64)] = &[
    // Session capture 1791720243814.rccap (53 MB, 1,248 columns), light from 22/10/2026.
    (0x0246_a9bd_d6c3_5007, 0x9db0_e314_d55a_9fb9),
];

type Out = (Vec<LightLayer>, Vec<LightLayer>);

/// Switches of the old layout (series 2, the default before the property grid).
const FAST: FullOpts = FullOpts { skip: true, noemit: true, layout: true, intern_uniform: true, ..FullOpts::OLD };

/// The property grid with the old layout's switches, none of its own.
const GRID: FullOpts = FullOpts { property_grid: true, ..FAST };

/// Variants of the matrix (switches, with the cached scans): the old layout's (series 2), then the
/// property grid alone, with each of its own switches alone, and all of them; the variants without
/// scans are those of the lightings that have none (mushroom generation, new world).
fn variants() -> Vec<(FullOpts, bool)> {
    let all_grid = FullOpts { union_tiles: false, ..FullOpts::ALL };
    vec![
        (FullOpts { intern_uniform: true, ..FullOpts::OLD }, false),
        (FAST, true),
        (FullOpts { noemit: false, ..FAST }, false),
        (GRID, true),
        (FullOpts { occlusion_table: true, ..GRID }, true),
        (FullOpts { queue_directions: true, ..GRID }, true),
        (FullOpts { occlusion_table: true, queue_directions: true, ..GRID }, true),
        (FullOpts { no_emitter_exit: true, ..GRID }, true),
        (FullOpts { sky_seed_filter: true, ..GRID }, true),
        (FullOpts { no_emitter_exit: true, sky_seed_filter: true, ..GRID }, true),
        (FullOpts { emitters_in_fill: true, ..GRID }, true),
        (FullOpts { skip_depths: true, ..GRID }, true),
        (FullOpts { skip_depths: false, ..all_grid }, true),
        (all_grid, true),
        (FullOpts { intern_uniform: false, ..all_grid }, true),
        (FullOpts { noemit: false, emitters_in_fill: false, ..all_grid }, false),
        (FullOpts { noemit: false, ..all_grid }, false),
    ]
}

/// Short name of a variant: the switches on.
fn name((o, sc): &(FullOpts, bool)) -> String {
    let on = [
        (o.skip, "SKIP"),
        (o.noemit, "NOEMIT"),
        (o.layout, "LAYOUT"),
        (o.property_grid, "GRID"),
        (o.occlusion_table, "OCCTABLE"),
        (o.queue_directions, "QUEUEDIRS"),
        (o.no_emitter_exit, "NOEMITEXIT"),
        (o.sky_seed_filter, "SEEDFILTER"),
        (o.emitters_in_fill, "EMITINFILL"),
        (o.skip_depths, "SKIPDEPTHS"),
        (o.union_tiles, "UNION"),
    ];
    let v: Vec<&str> = on.iter().filter(|(b, _)| *b).map(|(_, n)| *n).collect();
    let base = if v.is_empty() { "old".to_string() } else { v.join("+") };
    let sharing = if o.intern_uniform { "" } else { " -INTERN" };
    format!("{base}{sharing}{}", if *sc { " +scans" } else { "" })
}

/// Where a batch lighting takes its column scans from.
#[derive(Clone, Copy, PartialEq)]
enum Scanning {
    /// Done before, kept for the batch (generation).
    Cached,
    /// None: each zone scans its stacks on the fly (mushroom chunks, bonus chest before C2-M15).
    Without,
    /// Done by the batch itself, timed with it, over the columns its lighting reads.
    MadeInBatch,
}

/// One way of lighting a batch: name, switches, scans.
type BatchOption = (&'static str, FullOpts, Scanning);

fn batch_options() -> Vec<BatchOption> {
    let per_chunk = FullOpts { union_tiles: false, ..FullOpts::ALL };
    let tiles = |side: i32| FullOpts { union_tile_side: side, ..FullOpts::ALL };
    let series_2_without_scans = FullOpts { noemit: false, ..FAST };
    vec![
        ("chunk by chunk", per_chunk, Scanning::Cached),
        ("tiles auto", tiles(0), Scanning::Cached),
        ("tiles 2", tiles(2), Scanning::Cached),
        ("tiles 3", tiles(3), Scanning::Cached),
        ("tiles 4", tiles(4), Scanning::Cached),
        ("tiles auto kept 16 MiB", FullOpts { max_kept_buffer_bytes: 16 << 20, ..tiles(0) }, Scanning::Cached),
        ("no scans: series 2 chunk by chunk", series_2_without_scans, Scanning::Without),
        ("no scans: grid chunk by chunk", per_chunk, Scanning::Without),
        ("no scans: tiles auto", tiles(0), Scanning::Without),
        ("scanned in the batch: tiles auto", tiles(0), Scanning::MadeInBatch),
    ]
}

/// Lights `batch` the way `(_, opts, scanning)` says.
fn light_batch(
    t: &LightTables,
    flats: &HashMap<(i32, i32), Vec<u32>>,
    cached: &HashMap<(i32, i32), ColumnScan>,
    batch: &[(i32, i32)],
    (_, opts, scanning): BatchOption,
    h: i32,
) -> Vec<Out> {
    let column = |x: i32, z: i32| flats.get(&(x, z)).map(Vec::as_slice);
    let made: HashMap<(i32, i32), ColumnScan> = if scanning == Scanning::MadeInBatch {
        let mut read: Vec<(i32, i32)> =
            batch.iter().flat_map(|&(x, z)| (0..9).map(move |i| (x + i % 3 - 1, z + i / 3 - 1))).collect();
        read.sort_unstable();
        read.dedup();
        read.par_iter().filter_map(|k| flats.get(k).map(|v| (*k, rc_light::scan_column(t, v, h)))).collect()
    } else {
        HashMap::new()
    };
    let scans = match scanning {
        Scanning::Cached => cached,
        Scanning::Without | Scanning::MadeInBatch => &made,
    };
    rc_light::light_chunks_with(t, batch, column, |x, z| scans.get(&(x, z)), h, true, opts)
}

/// Batch level: the chunks of a generation batch lit one by one or by tiles
/// (`FULL_UNION_TILES`, see [`batch_options`]), checked first against the frozen copy. Sets of
/// batches made of the capture's columns: every one in one batch (a dense area); a flight, 12
/// batches of one row of 24 chunks (the server's batch), each followed by the relight of the row
/// before; two squares of 8 × 8; eight blocks of 3 × 3 (the relight around a bonus chest). 1
/// thread then all threads, minimum of 5 alternated passes, time of the whole set; cells resolved
/// and peak bytes of zone buffers in use at once.
fn bench_batches(
    t: &LightTables,
    flats: &HashMap<(i32, i32), Vec<u32>>,
    scans: &HashMap<(i32, i32), ColumnScan>,
    keys: &[(i32, i32)],
    want: &HashMap<(i32, i32), &Out>,
    h: i32,
) {
    let (x0, z0) = (keys.iter().map(|k| k.0).min().unwrap_or(0), keys.iter().map(|k| k.1).min().unwrap_or(0));
    let present = |v: Vec<(i32, i32)>| -> Vec<(i32, i32)> { v.into_iter().filter(|k| want.contains_key(k)).collect() };
    let rows: Vec<Vec<(i32, i32)>> = (0..12).map(|r| present((0..24).map(|x| (x0 + 4 + x, z0 + 4 + r)).collect())).collect();
    let flight: Vec<Vec<(i32, i32)>> = (0..rows.len())
        .flat_map(|r| std::iter::once(rows[r].clone()).chain(r.checked_sub(1).map(|p| rows[p].clone())))
        .collect();
    let squares: Vec<Vec<(i32, i32)>> =
        (0..2).map(|s| present((0..64).map(|i| (x0 + 4 + 10 * s + i % 8, z0 + 4 + i / 8)).collect())).collect();
    let chest_blocks: Vec<Vec<(i32, i32)>> =
        (0..8).map(|s| present((0..9).map(|i| (x0 + 4 + 4 * s + i % 3, z0 + 18 + i / 3)).collect())).collect();
    type Batches = Vec<Vec<(i32, i32)>>;
    let sets: [(&str, Batches); 4] = [
        ("all columns", vec![keys.to_vec()]),
        ("flight rows 24 x 1", flight),
        ("squares 8 x 8", squares),
        ("bonus chest 3 x 3", chest_blocks),
    ];
    let opts = batch_options();
    for (label, set) in &sets {
        for &o in &opts {
            for batch in set {
                let got = light_batch(t, flats, scans, batch, o, h);
                for (k, sheets) in batch.iter().zip(&got) {
                    assert_eq!(sheets, want[k], "batches {label}, chunk {k:?}, {}", o.0);
                }
            }
        }
    }
    eprintln!("batches identical to the frozen reference chunk by chunk and by tiles");
    let one = rayon::ThreadPoolBuilder::new().num_threads(1).build().expect("thread pool");
    let pools = [("1 thread", Some(&one)), ("all threads", None)];
    let on = |pool: Option<&rayon::ThreadPool>, f: &(dyn Fn() + Sync)| match pool {
        Some(p) => p.install(f),
        None => f(),
    };
    // Cells resolved per chunk lit (tiles chosen for the full pool) and peak bytes of zone buffers
    // in use at once, independent of the clock.
    for (label, set) in &sets {
        let chunks: usize = set.iter().map(Vec::len).sum();
        for (threads, pool) in pools {
            let counts: Vec<String> = opts
                .iter()
                .map(|&o| {
                    let before = rc_light::stats::full_light_cells();
                    let (queues_before, drops_before) = rc_light::stats::kept_buffer_releases();
                    rc_light::stats::reset_zone_buffer_peak();
                    on(pool, &|| set.iter().for_each(|b| drop(light_batch(t, flats, scans, b, o, h))));
                    let cells = (rc_light::stats::full_light_cells() - before) as f64 / chunks as f64;
                    let peak = rc_light::stats::zone_buffer_peak_bytes() as f64 / 1e6;
                    let (queues, drops) = rc_light::stats::kept_buffer_releases();
                    let (queues, drops) = (queues - queues_before, drops - drops_before);
                    format!("{} {cells:.0} cells, {peak:.1} MB, kept queues released {queues}, kept buffers dropped {drops}", o.0)
                })
                .collect();
            eprintln!("batches {label} ({threads}): per chunk / peak zone buffers: {}", counts.join(", "));
        }
    }
    for (label, set) in &sets {
        let chunks: usize = set.iter().map(Vec::len).sum();
        for (threads, pool) in pools {
            let mut best = vec![f64::MAX; opts.len()];
            for pass in 0..5 {
                for j in 0..opts.len() {
                    let i = (j + pass) % opts.len();
                    let t0 = std::time::Instant::now();
                    on(pool, &|| set.iter().for_each(|b| drop(black_box(light_batch(t, flats, scans, b, opts[i], h)))));
                    best[i] = best[i].min(t0.elapsed().as_secs_f64() * 1e3);
                }
            }
            let times: Vec<String> = opts.iter().zip(best).map(|((n, ..), b)| format!("{n} {b:.1} ms")).collect();
            eprintln!("batches {label} ({} batches, {chunks} chunks, {threads}): {}", set.len(), times.join(", "));
        }
    }
}

fn bench(c: &mut Criterion) {
    let Some(d) = need(rc_testdata::version_data(), "26.3 data cache") else { return };
    let Some(cap) = need(capture(&d), "reference capture") else { return };
    let t = LightTables::new(&d);
    let flats: HashMap<(i32, i32), Vec<u32>> = cap.cols.iter().map(|(&k, (s, _, _))| (k, flat_states(s))).collect();
    let mut keys: Vec<(i32, i32)> = flats.keys().copied().collect();
    keys.sort_unstable();
    let cols = |k: (i32, i32)| -> Columns {
        std::array::from_fn(|i| flats.get(&(k.0 + i as i32 % 3 - 1, k.1 + i as i32 / 3 - 1)).map(Vec::as_slice))
    };
    let h = cap.height;
    // Equivalence with the frozen copy, all columns.
    let outs: Vec<Out> = keys
        .par_iter()
        .map(|&k| {
            let (a, b) =
                (rc_light::light_chunk(&t, cols(k), h, true), rc_light::reference::light_chunk(&t, cols(k), h, true));
            assert_eq!(a, b, "column {k:?} differs from the reference");
            a
        })
        .collect();
    let mut dg = Fnv::default();
    for (k, (sky, block)) in keys.iter().zip(&outs) {
        dg.u64(k.0 as u64);
        dg.u64(k.1 as u64);
        for l in sky.iter().chain(block) {
            match l {
                Some(a) => dg.bytes(&a[..]),
                None => dg.u64(0xdead),
            }
        }
    }
    // Column scans done once (per-batch cache of the generation): same light.
    let scans: HashMap<(i32, i32), rc_light::ColumnScan> =
        flats.par_iter().map(|(&k, v)| (k, rc_light::scan_column(&t, v, cap.height))).collect();
    let scans_of = |k: (i32, i32)| -> rc_light::Scans {
        std::array::from_fn(|i| scans.get(&(k.0 + i as i32 % 3 - 1, k.1 + i as i32 / 3 - 1)))
    };
    keys.par_iter().zip(&outs).for_each(|(&k, want)| {
        let got = rc_light::light_chunk_scanned(&t, cols(k), scans_of(k), h, true);
        assert_eq!(&got, want, "column {k:?}: cached scans differ from the reference");
    });
    eprintln!("{} columns: light identical to the frozen reference (on-the-fly and cached scans)", keys.len());
    check_known("capture", cap.digest, dg.0, KNOWN);
    // Difference from the official game: columns surrounded by their 8 neighbours, sheets present on both
    // sides, counted cell by cell.
    // `edge`: differences within 2 cells of the column edge.
    let (mut seen, mut diff, mut presence, mut edge) = ([0u64; 2], [0u64; 2], [0u64; 2], [0u64; 2]);
    for (k, (sky, block)) in keys.iter().zip(&outs) {
        if !(-1..=1).all(|dx| (-1..=1).all(|dz| flats.contains_key(&(k.0 + dx, k.1 + dz)))) {
            continue;
        }
        let (_, osky, oblock) = &cap.cols[k];
        for (w, (ours, theirs)) in [(sky, osky), (block, oblock)].into_iter().enumerate() {
            for (a, b) in ours.iter().zip(theirs) {
                match (a, b) {
                    (Some(a), Some(b)) => {
                        seen[w] += 4096;
                        for i in 0..4096 {
                            let sh = (i & 1) * 4;
                            if (a[i >> 1] >> sh) & 15 != (b[i >> 1] >> sh) & 15 {
                                diff[w] += 1;
                                let (x, z) = (i & 15, (i >> 4) & 15);
                                edge[w] += u64::from(!(2..14).contains(&x) || !(2..14).contains(&z));
                            }
                        }
                    }
                    (None, None) => {}
                    _ => presence[w] += 1,
                }
            }
        }
    }
    eprintln!(
        "difference from the official game (light of the column packets): sky {} cells out of {} of which {} at the edge, \
         {} sheets present on one side only; block {} out of {} of which {} at the edge, {} sheets on one side only",
        diff[0], seen[0], edge[0], presence[0], diff[1], seen[1], edge[1], presence[1]
    );
    let count =
        |n: &str| rc_testdata::reference_capture().map_or(0, |cap| rc_testdata::play_packets(&cap, &d, n).len());
    eprintln!(
        "capture: {} light updates, {} block changes, {} section changes (not replayed here)",
        count("minecraft:light_update"),
        count("minecraft:block_update"),
        count("minecraft:section_blocks_update")
    );
    // Measured subset (the whole takes several seconds per pass).
    let one: Vec<(i32, i32)> = keys.iter().copied().step_by((keys.len() / 64).max(1)).take(64).collect();
    let many: Vec<(i32, i32)> = keys.iter().copied().step_by((keys.len() / 256).max(1)).take(256).collect();
    if cfg!(feature = "memstats") {
        let (_, a, b) = allocs(|| one.iter().for_each(|&k| drop(rc_light::light_chunk(&t, cols(k), h, true))));
        let (_, ra, rb) =
            allocs(|| one.iter().for_each(|&k| drop(rc_light::reference::light_chunk(&t, cols(k), h, true))));
        let n = one.len() as f64;
        eprintln!(
            "allocations per chunk: work {:.1} / {:.0} KiB, reference {:.1} / {:.0} KiB",
            a as f64 / n,
            b as f64 / n / 1024.0,
            ra as f64 / n,
            rb as f64 / n / 1024.0
        );
    }
    // Light sheets the chunks keep in their own allocation, with and without the shared uniform
    // sheets (all the columns, new path with scans).
    for intern in [false, true] {
        let (mut sheets, mut owned) = (0usize, 0usize);
        let (zero, full) = (rc_world::zero_layer(), rc_world::full_layer());
        for &k in &keys {
            let opts = FullOpts { intern_uniform: intern, ..FullOpts::ALL };
            let (sky, block) = rc_light::light_chunk_with(&t, cols(k), scans_of(k), h, true, opts);
            for l in sky.iter().chain(&block).flatten() {
                sheets += 1;
                owned += usize::from(!std::sync::Arc::ptr_eq(l, &zero) && !std::sync::Arc::ptr_eq(l, &full));
            }
        }
        eprintln!(
            "sheets, uniform ones shared {intern}: {sheets} sheets, {owned} own allocations ({:.1} MiB)",
            (owned * rc_world::LIGHT_BYTES) as f64 / (1 << 20) as f64
        );
    }
    // Matrix of the switches in this process (loaded bench: from one process to the next the own
    // noise exceeds the gain): 7 passes, variants in an order rotated at each pass, minimal
    // time per variant, 64 columns on 1 thread then 256 columns on all threads. Every variant is
    // first checked against the frozen copy on all the columns.
    let combos = variants();
    keys.par_iter().zip(&outs).for_each(|(&k, want)| {
        for (o, sc) in &combos {
            let s = if *sc { scans_of(k) } else { [None; 9] };
            assert_eq!(&rc_light::light_chunk_with(&t, cols(k), s, h, true, *o), want, "column {k:?}, {}", name(&(*o, *sc)));
        }
    });
    eprintln!("{} variants identical to the frozen reference on the {} columns", combos.len(), keys.len());
    // Work counted independently of the clock: cells resolved per chunk (margins and ring included).
    for (o, sc) in &combos {
        let before = rc_light::stats::full_light_cells();
        keys.par_iter().for_each(|&k| {
            let s = if *sc { scans_of(k) } else { [None; 9] };
            drop(rc_light::light_chunk_with(&t, cols(k), s, h, true, *o));
        });
        let cells = (rc_light::stats::full_light_cells() - before) as f64 / keys.len() as f64;
        eprintln!("  cells resolved per chunk, {:<56} {:9.0}", name(&(*o, *sc)), cells);
    }
    let by_key: HashMap<(i32, i32), &Out> = keys.iter().copied().zip(&outs).collect();
    bench_batches(&t, &flats, &scans, &keys, &by_key, h);
    drop(by_key);
    drop(outs);
    let run = |list: &[(i32, i32)], (o, sc): &(FullOpts, bool), par: bool| {
        let f = |&k: &(i32, i32)| {
            let s = if *sc { scans_of(k) } else { [None; 9] };
            black_box(rc_light::light_chunk_with(&t, cols(k), s, h, true, *o)).0.len()
        };
        let t0 = std::time::Instant::now();
        if par {
            black_box(list.par_iter().map(f).sum::<usize>());
        } else {
            black_box(list.iter().map(f).sum::<usize>());
        }
        t0.elapsed().as_secs_f64() * 1e3
    };
    for (label, list, par) in [("64 columns, 1 thread", &one, false), ("256 columns, all threads", &many, true)] {
        let mut best = vec![f64::MAX; combos.len()];
        for pass in 0..7 {
            for j in 0..combos.len() {
                let i = (j + pass) % combos.len();
                best[i] = best[i].min(run(list, &combos[i], par));
            }
        }
        eprintln!("matrix ({label}, minimum of 7 alternated passes):");
        for (cmb, b) in combos.iter().zip(&best) {
            eprintln!("  {:<56} {b:8.1} ms", name(cmb));
        }
    }
    let mut g = c.benchmark_group("light_chunk");
    g.sample_size(10);
    g.bench_function(format!("work_{}_columns_1t", one.len()), |b| {
        b.iter(|| one.iter().map(|&k| black_box(rc_light::light_chunk(&t, cols(k), h, true)).0.len()).sum::<usize>())
    });
    g.bench_function(format!("scanned_{}_columns_1t", one.len()), |b| {
        b.iter(|| {
            one.iter()
                .map(|&k| black_box(rc_light::light_chunk_scanned(&t, cols(k), scans_of(k), h, true)).0.len())
                .sum::<usize>()
        })
    });
    // Cost of the scan alone, per column (done once per chunk and per batch).
    g.bench_function(format!("scan_{}_columns_1t", one.len()), |b| {
        b.iter(|| one.iter().for_each(|k| drop(black_box(rc_light::scan_column(&t, &flats[k], h)))))
    });
    g.bench_function(format!("reference_{}_columns_1t", one.len()), |b| {
        b.iter(|| {
            one.iter()
                .map(|&k| black_box(rc_light::reference::light_chunk(&t, cols(k), h, true)).0.len())
                .sum::<usize>()
        })
    });
    // Scale: 1 thread then powers of 2 up to the number of logical cores (and that number).
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut counts: Vec<usize> =
        std::iter::successors(Some(1usize), |n| Some(n * 2)).take_while(|&n| n <= cores).collect();
    if counts.last() != Some(&cores) {
        counts.push(cores);
    }
    for n in counts {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(n).build().expect("thread pool");
        g.bench_function(format!("work_{}_columns_{n}t", many.len()), |b| {
            b.iter(|| {
                pool.install(|| {
                    many.par_iter()
                        .map(|&k| black_box(rc_light::light_chunk(&t, cols(k), h, true)).0.len())
                        .sum::<usize>()
                })
            })
        });
        g.bench_function(format!("scanned_{}_columns_{n}t", many.len()), |b| {
            b.iter(|| {
                pool.install(|| {
                    many.par_iter()
                        .map(|&k| black_box(rc_light::light_chunk_scanned(&t, cols(k), scans_of(k), h, true)).0.len())
                        .sum::<usize>()
                })
            })
        });
    }
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
