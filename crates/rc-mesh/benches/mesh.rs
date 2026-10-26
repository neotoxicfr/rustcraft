// Direct read allowed (bench: setting read at launch, off the measured path); the game goes through rc_data::env_*!.
#![allow(clippy::disallowed_methods)]
//! Meshing of the real columns of the reference capture (outside the repository), with neighbours, set up
//! as in game (bottom of the world −64, biome zoom seed of the capture: `tests/common`).
//! Data absent: bench skipped, or failure under `RUSTCRAFT_OPT_REQUIRE=1`.

#[path = "../tests/common/mod.rs"]
mod common;

use hashbrown::HashMap;
use std::hint::black_box;
use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use rayon::prelude::*;
use rc_mesh::wire::MeshJob;

#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn bench(c: &mut Criterion) {
    let Some((d, cols, t, palette)) = common::load() else { return };
    let mut keys: Vec<(i32, i32)> = cols.keys().copied().collect();
    keys.sort_unstable();
    let jobs: Vec<MeshJob> = keys.iter().map(|&(x, z)| common::job(&cols, x, z)).collect();
    let mut g = c.benchmark_group("mesh");
    g.sample_size(10);
    {
        let m = |j| rc_mesh::blocks::mesh(j, &t, &palette, common::MIN_Y);
        for j in &jobs {
            let (a, b) = (m(j), rc_mesh::blocks::mesh_reference(j, &t, &palette, common::MIN_Y));
            assert!(common::same(&a, &b), "textured mesh ({}, {})", j.center.x, j.center.z);
        }
        eprintln!("textured mesh identical to the reference on all columns");
        // Stored fingerprint of all the option variants: `tests/digest.rs`.
        assert!(jobs.iter().all(|j| rc_mesh::blocks::blend_matches_reference(j, &palette)), "biome blend");
        eprintln!("biome blend identical to the reference on all columns");
        g.bench_function(format!("blocks_reference_{}_columns_1t", jobs.len()), |b| {
            b.iter(|| {
                jobs.iter()
                    .map(|j| black_box(rc_mesh::blocks::mesh_reference(j, &t, &palette, common::MIN_Y)).fluids.len())
                    .sum::<usize>()
            })
        });
        // Occlusion culling: communications of all columns, walk from the centre.
        let vis: HashMap<(i32, i32), Vec<u64>> = jobs.iter().map(|j| ((j.center.x, j.center.z), m(j).vis)).collect();
        let (cx, cz) = {
            let n = vis.len() as i32;
            let (sx, sz) = vis.keys().fold((0, 0), |a, k| (a.0 + k.0, a.1 + k.1));
            (sx / n, sz / n)
        };
        let eye = [f64::from(cx * 16) + 8.3, 80.6, f64::from(cz * 16) + 8.7];
        let mut graph = rc_mesh::visibility::Graph::default();
        let n = graph.run(|x, z| vis.get(&(x, z)).map(Vec::as_slice), eye, -64, 24, 32, true).len();
        let reference =
            rc_mesh::visibility::visible_sections(|x, z| vis.get(&(x, z)).map(Vec::as_slice), eye, -64, 24, 32, true);
        assert_eq!(n, reference.len());
        eprintln!("occlusion culling: {n} sections reached out of {}", vis.len() * 24);
        g.bench_function("visibility_graph", |b| {
            b.iter(|| graph.run(|x, z| vis.get(&(x, z)).map(Vec::as_slice), eye, -64, 24, 32, true).len())
        });
        g.bench_function(format!("blocks_{}_columns_1t", jobs.len()), |b| {
            b.iter(|| jobs.iter().map(|j| black_box(m(j)).fluids.len()).sum::<usize>())
        });
        g.bench_function(format!("blocks_{}_columns_parallel", jobs.len()), |b| {
            b.iter(|| jobs.par_iter().map(|j| black_box(m(j)).fluids.len()).sum::<usize>())
        });
        // Biome tint paths (C2-M06, same output: `tests/digest.rs`): old, each
        // alone, all except one, all, thresholds of the sliding sums; 1 thread and all threads.
        {
            use rc_mesh::blocks::TintPaths;
            let (all, old) = (TintPaths::ALL, TintPaths::OLD);
            let variants = [
                ("old", old),
                ("uni", TintPaths { uni: true, ..old }),
                ("lattice", TintPaths { lattice: true, ..old }),
                ("grid", TintPaths { grid: true, ..old }),
                ("idx", TintPaths { idx: true, ..old }),
                ("box", TintPaths { boxsum: all.boxsum, ..old }),
                ("grid_lattice", TintPaths { grid: true, lattice: true, ..old }),
                ("all_but_uni", TintPaths { uni: false, ..all }),
                ("all_but_lattice", TintPaths { lattice: false, ..all }),
                ("all_but_grid", TintPaths { grid: false, ..all }),
                ("all_but_idx", TintPaths { idx: false, ..all }),
                ("all_but_box", TintPaths { boxsum: 0, ..all }),
                ("all_box1", TintPaths { boxsum: 1, ..all }),
                ("all_box8", TintPaths { boxsum: 8, ..all }),
                ("all_box128", TintPaths { boxsum: 128, ..all }),
                ("all", all),
            ];
            for (name, p) in variants {
                // Kept until the end of the bench (the group lives longer than this block).
                let js: &'static [MeshJob] = keys
                    .iter()
                    .map(|&(x, z)| {
                        let mut j = common::job(&cols, x, z);
                        j.tint = p;
                        j
                    })
                    .collect::<Vec<_>>()
                    .leak();
                g.bench_function(format!("tint_{name}_1t"), |b| {
                    b.iter(|| js.iter().map(|j| black_box(m(j)).fluids.len()).sum::<usize>())
                });
                g.bench_function(format!("tint_{name}_parallel"), |b| {
                    b.iter(|| js.par_iter().map(|j| black_box(m(j)).fluids.len()).sum::<usize>())
                });
            }
        }
        // Smoothed light paths (C2-M07, same output: `tests/light_paths.rs`): old,
        // cumulative in the order D, A, B, C, then E, F, G alone and added, all, default; 1 thread and
        // all threads (`RC_BENCH_LIGHT_AO=0`: occlusion off).
        {
            use rc_mesh::light::LightPaths;
            let old = LightPaths::OLD;
            let d = LightPaths { scratch: true, ..old };
            let da = LightPaths { cell: true, ..d };
            let dab = LightPaths { corners: true, ..da };
            let dabc = LightPaths { facetab: true, ..dab };
            let ao = rc_data::env_os!("RC_BENCH_LIGHT_AO").is_none_or(|v| v != "0");
            let variants = [
                ("old", old),
                ("d", d),
                ("da", da),
                ("dab", dab),
                ("dabc", dabc),
                ("a", LightPaths { cell: true, ..old }),
                ("b", LightPaths { corners: true, ..old }),
                ("c", LightPaths { facetab: true, ..old }),
                ("e", LightPaths { cell8: true, ..old }),
                ("f", LightPaths { fluid: true, ..old }),
                ("g", LightPaths { neigh: true, ..old }),
                ("dabce", LightPaths { cell8: true, ..dabc }),
                ("dabcf", LightPaths { fluid: true, ..dabc }),
                ("dabcg", LightPaths { neigh: true, ..dabc }),
                ("dabcef", LightPaths { cell8: true, fluid: true, ..dabc }),
                ("all", LightPaths::ALL),
                ("default", LightPaths::DEFAULT),
            ];
            for (name, p) in variants {
                let js: &'static [MeshJob] = keys
                    .iter()
                    .map(|&(x, z)| {
                        let mut j = common::job(&cols, x, z);
                        j.light = p;
                        j.opts.ao = ao;
                        j
                    })
                    .collect::<Vec<_>>()
                    .leak();
                g.bench_function(format!("light_{name}_1t"), |b| {
                    b.iter(|| js.iter().map(|j| black_box(m(j)).fluids.len()).sum::<usize>())
                });
                g.bench_function(format!("light_{name}_parallel"), |b| {
                    b.iter(|| js.par_iter().map(|j| black_box(m(j)).fluids.len()).sum::<usize>())
                });
            }
            eprintln!("face geometry for lighting: {} B", t.light_faces.heap_bytes());
        }
        // Without the shortcut for sealed sections (same output, cost comparison).
        let mut t0 = rc_mesh::blocks::Tables::new(&d, t.models.clone());
        t0.sealed.iter_mut().for_each(|s| *s = false);
        // Property grid (C2-M10): same shortcut read through its bit.
        t0.cull.props.iter_mut().flatten().for_each(|p| *p &= !rc_mesh::cull::SEALED_BIT);
        let m0 = |j| rc_mesh::blocks::mesh(j, &t0, &palette, common::MIN_Y);
        for j in &jobs {
            let (a, b) = (m(j), m0(j));
            assert!(
                a.layers == b.layers && a.fluids == b.fluids && a.vis == b.vis,
                "sealed ({}, {})",
                j.center.x,
                j.center.z
            );
        }
        g.bench_function(format!("blocks_unsealed_{}_columns_1t", jobs.len()), |b| {
            b.iter(|| jobs.iter().map(|j| black_box(m0(j)).fluids.len()).sum::<usize>())
        });
        // Multi-core scale: 1 thread then powers of 2 up to the number of logical cores
        // (`RC_BENCH_SCALE=1`, long).
        if std::env::var_os("RC_BENCH_SCALE").is_some() {
            let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
            let mut n = 1;
            while n <= cores {
                let pool = rayon::ThreadPoolBuilder::new().num_threads(n).build().expect("pool");
                g.bench_function(format!("blocks_scale_{}_columns_{n}t", jobs.len()), |b| {
                    b.iter(|| pool.install(|| jobs.par_iter().map(|j| black_box(m(j)).fluids.len()).sum::<usize>()))
                });
                n *= 2;
            }
        }
        // Partial remeshing: one section (the 9th, plains ground) redone on the basis of the
        // full mesh of the column, against the whole column.
        let partial: Vec<MeshJob> = jobs
            .iter()
            .map(|j| {
                let mut words = Vec::new();
                rc_mesh::blocks::pack(&m(j), &mut words);
                let mut p = MeshJob::gather(j.center.clone(), 0, |x, z| {
                    rc_mesh::wire::NEIGHBORS
                        .iter()
                        .position(|&d| (j.center.x + d.0, j.center.z + d.1) == (x, z))
                        .and_then(|i| j.neighbors[i].clone())
                });
                // Same settings as the full task (zoom seed, bottom of the world, plains).
                (p.biome_zoom_seed, p.min_y, p.plains_biome, p.opts, p.tint) =
                    (j.biome_zoom_seed, j.min_y, j.plains_biome, j.opts, j.tint);
                let mut mask = rc_mesh::wire::Sections::default();
                mask.insert(8);
                p.reuse = Some((mask, Arc::new(words)));
                p
            })
            .collect();
        for (p, j) in partial.iter().zip(&jobs) {
            let (a, b) = (m(p), m(j));
            assert!(
                a.layers == b.layers && a.fluids == b.fluids && a.vis == b.vis,
                "partial ({}, {})",
                j.center.x,
                j.center.z
            );
        }
        g.bench_function(format!("blocks_{}_columns_1_section_1t", jobs.len()), |b| {
            b.iter(|| partial.iter().map(|j| black_box(m(j)).fluids.len()).sum::<usize>())
        });
    }
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
