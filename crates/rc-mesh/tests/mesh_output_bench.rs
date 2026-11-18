//! Micro-bench of the end of the meshing (C2-M13), INTERLEAVED variants (order rotated at each
//! pass, best and median of 7) to hold up on a loaded machine. Ignored by default:
//! `cargo test --release -p rc-mesh --test mesh_output_bench -- --ignored --nocapture`.
//! - faces pushed straight into their group against the sort at the end
//!   (`MeshJob::faces_by_group`), whole columns and one section remeshed, 1 thread and all threads;
//! - in-place write of a light change in one section: old diff (whole column, one range),
//!   remeshed sections only, several ranges (gaps of 1, 4, 16, 64 KiB of faces), with the bytes each
//!   path sends.
//!
//! Identical output proven by `tests/digest.rs`, `tests/in_place.rs` and `tests/partial.rs`.

mod common;

use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use rayon::prelude::*;
use rc_mesh::blocks::{BUCKETS, BlockMesh, InPlace, InPlacePaths, Packed, in_place_diff_with, mesh, pack_exact};
use rc_mesh::wire::{MeshJob, Sections};

/// Best and median of a list of times.
fn stat(v: &mut [f64]) -> (f64, f64) {
    v.sort_by(f64::total_cmp);
    (v[0], v[v.len() / 2])
}

/// Times each variant (`run(i)`) 7 times, the order rotated at each pass.
fn interleaved(n: usize, mut run: impl FnMut(usize)) -> Vec<Vec<f64>> {
    let mut times = vec![Vec::new(); n];
    for pass in 0..7 {
        for k in 0..n {
            let i = (k + pass) % n;
            let s = Instant::now();
            run(i);
            times[i].push(s.elapsed().as_secs_f64() * 1e3);
        }
    }
    times
}

#[test]
#[ignore]
fn mesh_output_bench() {
    let Some((_d, cols, t, palette)) = common::load() else { return };
    let mut keys: Vec<(i32, i32)> = cols.keys().copied().collect();
    keys.sort_unstable();
    let mut ninth = Sections::default();
    ninth.insert(8);
    let bases: Vec<Packed> =
        keys.par_iter().map(|&(x, z)| Arc::new(pack_exact(&mesh(&common::job(&cols, x, z), &t, &palette, common::MIN_Y))) as Packed).collect();
    // Sizes of the (layer, group) lists the meshing threads keep (`KEPT_FACES_PER_GROUP`, 8,192).
    let mut sizes: Vec<usize> = bases
        .iter()
        .flat_map(|w| {
            let m = rc_mesh::blocks::unpack(w, 0).expect("flat");
            let n = m.vis.len() + 1;
            (0..3).flat_map(move |l| {
                let s = m.starts[l].clone();
                (0..BUCKETS).map(move |g| (s[g * n + n - 1] - s[g * n]) as usize)
            })
        })
        .collect();
    sizes.sort_unstable();
    let at = |q: f64| sizes[((sizes.len() - 1) as f64 * q) as usize];
    eprintln!("faces per (layer, group) of a column: median {}, 99% {}, max {}", at(0.5), at(0.99), at(1.0));
    // Same bases carrying the column's GPU tag (`pack_tagged`): the reused sections are copied
    // with the tag taken off.
    let tagged: Vec<Packed> = bases
        .iter()
        .map(|w| {
            let m = rc_mesh::blocks::unpack(w, 0).expect("flat");
            let tag = (((m.x & 15) | (m.z & 15) << 4) as u32 | 1) << 20;
            Arc::new(rc_mesh::blocks::pack_exact_tagged(&m, tag)) as Packed
        })
        .collect();
    // (name, by group, base of the 9th section remeshed: none, plain, tagged)
    let variants = [
        ("sorted at the end", false, 0),
        ("by group", true, 0),
        ("sorted, 1 section", false, 1),
        ("by group, 1 section", true, 1),
        ("by group, 1 sec. tagged", true, 2),
    ];
    let jobs = |by_group: bool, base: usize, step: usize| -> Vec<MeshJob> {
        keys.iter()
            .enumerate()
            .step_by(step)
            .map(|(i, &(x, z))| {
                let mut j = common::job(&cols, x, z);
                j.faces_by_group = by_group;
                j.reuse = [None, Some(&bases[i]), Some(&tagged[i])][base].map(|b| (ninth, b.clone()));
                j
            })
            .collect()
    };
    let one: Vec<Vec<MeshJob>> = variants.iter().map(|v| jobs(v.1, v.2, 4)).collect();
    let all: Vec<Vec<MeshJob>> = variants.iter().map(|v| jobs(v.1, v.2, 1)).collect();
    let mut t1 = interleaved(variants.len(), |i| {
        black_box(one[i].iter().map(|j| mesh(j, &t, &palette, common::MIN_Y).len()).sum::<usize>());
    });
    let mut tp = interleaved(variants.len(), |i| {
        black_box(all[i].par_iter().map(|j| mesh(j, &t, &palette, common::MIN_Y).len()).sum::<usize>());
    });
    eprintln!("meshing, ms (best / median), 1 thread on {} columns, all threads on {}:", one[0].len(), all[0].len());
    for (i, v) in variants.iter().enumerate() {
        let ((a, am), (b, bm)) = (stat(&mut t1[i]), stat(&mut tp[i]));
        let runs: Vec<String> = t1[i].iter().map(|t| format!("{t:.0}")).collect();
        eprintln!("  {:<22} 1 thread {a:8.1} / {am:8.1}   all {b:7.1} / {bm:7.1}   (1 thread: {})", v.0, runs.join(" "));
    }
    for every in [true, false] {
        in_place_bench(&keys, &cols, &t, &palette, &bases, ninth, every);
    }
}

/// The 9th section remeshed with its light changed (same counts, same places), the rest copied from
/// the base: on every face of the section (`every`), or on its first face in each group only (a
/// local change, such as a lever or a wire, that touches every visible direction).
fn light_changed(m: &BlockMesh, section: usize, every: bool) -> BlockMesh {
    let n = m.vis.len() + 1;
    let mut layers = m.layers.clone();
    for (l, faces) in layers.iter_mut().enumerate() {
        for g in 0..BUCKETS {
            let k = g * n + section;
            let (a, e) = (m.starts[l][k] as usize, m.starts[l][k + 1] as usize);
            let e = if every { e } else { e.min(a + 1) };
            faces[a..e].iter_mut().for_each(|f| f[3] ^= 0x0101);
        }
    }
    BlockMesh {
        x: m.x,
        z: m.z,
        version: m.version + 1,
        layers,
        fluids: m.fluids.clone(),
        starts: m.starts.clone(),
        fluid_starts: m.fluid_starts.clone(),
        vis: m.vis.clone(),
    }
}

fn in_place_bench(
    keys: &[(i32, i32)],
    cols: &common::Cols,
    t: &rc_mesh::blocks::Tables,
    palette: &rc_mesh::blocks::BiomePalette,
    bases: &[Packed],
    remeshed: Sections,
    every: bool,
) {
    let meshes: Vec<BlockMesh> = keys
        .par_iter()
        .map(|&(x, z)| light_changed(&mesh(&common::job(cols, x, z), t, palette, common::MIN_Y), 8, every))
        .collect();
    let face_gap = |kib: u32| Some([kib * 1024 / 24, kib * 1024 / 96, kib * 1024 / 108]);
    let variants: [(&str, Option<Sections>, InPlacePaths); 7] = [
        ("old (whole, one range)", None, InPlacePaths::OLD),
        ("touched, one range", Some(remeshed), InPlacePaths { touched_only: true, gaps: None }),
        ("whole, gaps 4 KiB", None, InPlacePaths { touched_only: false, gaps: face_gap(4) }),
        ("touched, gaps 1 KiB", Some(remeshed), InPlacePaths { touched_only: true, gaps: face_gap(1) }),
        ("touched, gaps 4 KiB", Some(remeshed), InPlacePaths { touched_only: true, gaps: face_gap(4) }),
        ("touched, gaps 16 KiB", Some(remeshed), InPlacePaths { touched_only: true, gaps: face_gap(16) }),
        ("touched, gaps 64 KiB", Some(remeshed), InPlacePaths { touched_only: true, gaps: face_gap(64) }),
    ];
    let diff = |i: usize, k: usize| in_place_diff_with(&bases[k], &meshes[k], variants[i].1, variants[i].2);
    let mut times = interleaved(variants.len(), |i| {
        black_box((0..meshes.len()).filter_map(|k| diff(i, k)).count());
    });
    let whole_faces: usize = meshes.iter().map(|m| m.layers.iter().map(Vec::len).sum::<usize>()).sum();
    eprintln!(
        "in-place write of a light change on {} of the 9th section, {} columns ({:.1} MB of faces), ms (best / median):",
        if every { "every face" } else { "one face per group" },
        meshes.len(),
        whole_faces as f64 * 24.0 / 1e6
    );
    for (i, v) in variants.iter().enumerate() {
        let (a, am) = stat(&mut times[i]);
        let d: Vec<InPlace> = (0..meshes.len()).filter_map(|k| diff(i, k)).collect();
        let faces: u32 = d.iter().flat_map(|d| &d.faces).map(|r| r.end - r.start).sum();
        let writes: usize = d.iter().map(|d| d.faces.len()).sum();
        eprintln!(
            "  {:<24} {a:7.2} / {am:7.2}   faces sent {:6.2} MB, {:.1} writes per column",
            v.0,
            f64::from(faces) * 24.0 / 1e6,
            writes as f64 / d.len().max(1) as f64
        );
    }
}
