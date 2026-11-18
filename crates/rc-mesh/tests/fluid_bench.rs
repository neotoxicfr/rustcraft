//! Micro-bench of the fluid shortcuts (`FluidPaths`): the two layers around a fluid block read
//! once (`RUSTCRAFT_OPT_FLUIDNEIGHBORMEMO`), the fluid mesher not called for a block among its own
//! state (`RUSTCRAFT_OPT_FLUID_SKIP_BURIED`), the light of the block's level read once
//! (`RUSTCRAFT_OPT_FLUID_FACE_LIGHT_ONCE`); each alone then all, against none. Variants measured in
//! alternation (same machine load for each), best time of `RC_BENCH_ROUNDS` passes (7 by default),
//! 1 thread then all threads. Scenes: the whole capture, then eight sections of one column and
//! its neighbours rewritten as deep water (every block among water, an ocean below its surface),
//! as sheets of water one block apart (every block draws its top and bottom), and as random
//! fluids of every level (slopes, falls, lava, faces hidden in part).
//! Launch: `cargo test --release -p rc-mesh --test fluid_bench -- --ignored --nocapture`.
// Direct read allowed (bench: setting read at launch, off the measured path).
#![allow(clippy::disallowed_methods)]

mod common;

use std::hint::black_box;
use std::time::Instant;

use common::Cols;
use rayon::prelude::*;
use rc_mesh::blocks::{BiomePalette, Tables, mesh};
use rc_mesh::cull::CullPaths;
use rc_mesh::fluid::FluidPaths;
use rc_mesh::wire::MeshJob;

/// Sections rewritten by the built scenes.
const SCENE_SECTIONS: std::ops::Range<usize> = 8..16;

/// Measured variants. The first is the base of the percentages.
const VARIANTS: [(&str, FluidPaths); 5] = [
    ("none", FluidPaths::OLD),
    ("memo", FluidPaths { neighbor_memo: true, ..FluidPaths::OLD }),
    ("skip", FluidPaths { skip_buried: true, ..FluidPaths::OLD }),
    ("light", FluidPaths { face_light_once: true, ..FluidPaths::OLD }),
    ("all", FluidPaths::ALL),
];

struct Scene {
    name: &'static str,
    /// The same jobs for each of the [`VARIANTS`].
    jobs: [Vec<MeshJob>; 5],
}

impl Scene {
    /// Jobs of the `columns` of `world` (game's removal rule), once per variant.
    fn new(name: &'static str, world: &Cols, columns: &[(i32, i32)]) -> Self {
        let jobs = VARIANTS.map(|(_, fluid)| {
            let job = |&(x, z): &(i32, i32)| {
                let mut j = common::job_with(world, x, z, CullPaths::DEFAULT);
                j.fluid = fluid;
                j
            };
            columns.iter().map(job).collect()
        });
        Scene { name, jobs }
    }

    /// Best time (s) of each variant over `rounds` alternated passes.
    fn best_times(&self, t: &Tables, palette: &BiomePalette, rounds: usize, all_threads: bool) -> [f64; 5] {
        let fluid_faces =
            |j: &MeshJob| black_box(mesh(j, t, palette, common::MIN_Y)).fluids.iter().map(Vec::len).sum::<usize>();
        let mut best = [f64::INFINITY; 5];
        for _ in 0..rounds {
            for (jobs, best) in self.jobs.iter().zip(&mut best) {
                let start = Instant::now();
                let total: usize = match all_threads {
                    true => jobs.par_iter().map(fluid_faces).sum(),
                    false => jobs.iter().map(fluid_faces).sum(),
                };
                black_box(total);
                *best = best.min(start.elapsed().as_secs_f64());
            }
        }
        best
    }
}

#[test]
#[ignore]
fn fluid_shortcuts_bench() {
    let Some((d, cols, t, palette)) = common::load() else { return };
    let mut keys: Vec<(i32, i32)> = cols.keys().copied().collect();
    keys.sort_unstable();
    let key = common::surrounded_column(&cols);
    let water = common::state(&d, "minecraft:water", &[("level", "0")]);
    // The same column meshed 64 times: enough work for a timing, and for every thread.
    let built = |name: &'static str, next_state: &mut dyn FnMut() -> u32| {
        let world = common::with_sections_rewritten(&d, &cols, key, SCENE_SECTIONS, next_state);
        Scene::new(name, &world, &[key; 64])
    };
    let mut cell = 0usize;
    let scenes = [
        Scene::new("capture", &cols, &keys),
        built("deep water", &mut || water),
        // Storage order y, z, x: the layer of a cell is its index / 256.
        built("water sheets", &mut || {
            cell += 1;
            if ((cell - 1) / 256).is_multiple_of(2) { water } else { 0 }
        }),
        built("random fluids", &mut common::random_fluid_states(&d, 1)),
    ];
    let rounds: usize = rc_data::env_str!("RC_BENCH_ROUNDS").and_then(|v| v.parse().ok()).unwrap_or(7);
    for all_threads in [false, true] {
        for scene in &scenes {
            let best = scene.best_times(&t, &palette, rounds, all_threads);
            let line: Vec<String> = VARIANTS
                .iter()
                .zip(best)
                .map(|((name, ..), s)| format!("{name} {:8.2} ms ({:+5.1} %)", s * 1e3, (s / best[0] - 1.0) * 100.0))
                .collect();
            let threads = if all_threads { "all threads" } else { "1 thread   " };
            eprintln!("{threads} {:>14}: {}", scene.name, line.join(", "));
        }
    }
}
