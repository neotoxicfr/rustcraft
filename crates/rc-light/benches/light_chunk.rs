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
use rc_light::{Columns, LightTables};
use rc_world::LightLayer;
use std::hint::black_box;

/// Recognised captures: (capture fingerprint, fingerprint of the computed light).
const KNOWN: &[(u64, u64)] = &[
    // Session capture 1791720243814.rccap (53 MB, 1,248 columns), light from 22/10/2026.
    (0x0246_a9bd_d6c3_5007, 0x9db0_e314_d55a_9fb9),
];

type Out = (Vec<LightLayer>, Vec<LightLayer>);

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
    drop(outs);
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
    // Matrix of the switches in this process (loaded bench: from one process to the next the own
    // noise exceeds the gain): 7 passes, variants in an order rotated at each pass, minimal
    // time per variant, 64 columns on 1 thread then 256 columns on all threads.
    let combos: Vec<([bool; 3], bool)> = (0..8usize)
        .flat_map(|b| [([b & 1 != 0, b & 2 != 0, b & 4 != 0], false), ([b & 1 != 0, b & 2 != 0, b & 4 != 0], true)])
        .filter(|(o, sc)| !(o[1] && !sc))
        .collect();
    let name = |(o, sc): &([bool; 3], bool)| {
        let mut v: Vec<&str> = Vec::new();
        for (on, n) in o.iter().zip(["SKIP", "NOEMIT", "LAYOUT"]) {
            if *on {
                v.push(n);
            }
        }
        format!("{}{}", if v.is_empty() { "old".into() } else { v.join("+") }, if *sc { " +scans" } else { "" })
    };
    let run = |list: &[(i32, i32)], (o, sc): &([bool; 3], bool), par: bool| {
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
            eprintln!("  {:<28} {b:8.1} ms", name(cmb));
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
