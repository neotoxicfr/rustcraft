//! Equivalence proof of the meshing: all the columns of the reference capture meshed
//! with each variant of the settings that change the meshing (biome blend 0, 1, 2 and 7,
//! ambient occlusion off, opaque leaves, dimension without sky with "nether" shading,
//! partial remesh), each reduced to a fingerprint of ALL the fields of the mesh
//! (`rc_mesh::blocks::digest`: layers, starts per group, fluids bit for bit, visibility),
//! COMPARED to a value written here. An optimisation must never change one; an intended
//! rendering fix replaces them (the failure message gives the new values).
//!
//! Game settings: bottom of the world −64, biome zoom seed of the capture, plains for
//! absent columns (variant: desert). The capture and the cache are checked first (`INPUT`).
//! Values of the mesher of commit 08df4a1 (before optimisation series 2): computed on the
//! branch whose `src/` differs from 08df4a1 only by the addition of `digest` and the removal of the
//! wireframe (no line of the textured mesher or of `MeshJob` touched). Replaced by G2-F12
//! (intended fix: canonical vertex order of the turned faces, `bake.rs`); with that
//! fix alone turned off, the old values all passed again.
//!
//! These values are those of the old format (`old_job`); the compact formats (C3-M03) have
//! their own fingerprint (`EXPECTED_COMPACT`).
//!
//! Without data (capture, cache, installed game) the test is ignored; under `RUSTCRAFT_OPT_REQUIRE=1`
//! it is a failure.

mod common;

use std::sync::Arc;

use common::{Cols, EXPECTED_COMPACT, MIN_Y, job};
use rayon::prelude::*;
use rc_mesh::blocks::{BiomePalette, BlockMesh, Packed, Tables, digest, mesh, mesh_reference};
use rc_mesh::wire::{MeshJob, SHADE_NETHER, Sections};

/// Expected fingerprints, per variant.
const EXPECTED: &[(&str, u64)] = &[
    ("default", 0xa1ec_3f83_bb51_b4b8),
    ("blend 0", 0x4d9c_04ac_e557_a9c9),
    ("blend 1", 0x5f7e_0e78_ed08_6d0a),
    ("blend 7", 0x3a99_5f2c_8626_6b59),
    ("no occlusion", 0x34cf_e294_3531_8947),
    ("opaque leaves", 0x3577_3532_ca5a_0ed0),
    ("no sky, nether shading", 0xd11d_801f_f21f_f42c),
    ("absent columns as desert", 0x0517_1882_b549_0701),
    ("partial remesh", 0xa1ec_3f83_bb51_b4b8),
];

/// Inputs of the computation: ((size, fingerprint) of the capture, number of states, number of biomes).
const INPUT: ((usize, u64), usize, usize) = ((53_361_964, 0x1cec_5571_96ea_cb22), 35_723, 101);

/// Fingerprint of all the columns: independent of the order (sum), but tied to the position
/// of each column.
fn total<'a>(meshes: impl Iterator<Item = &'a BlockMesh>) -> u64 {
    meshes.fold(0u64, |s, m| {
        let at = (u64::from(m.x as u32) << 32 | u64::from(m.z as u32)).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        s.wrapping_add(digest(m) ^ at)
    })
}

/// Task in the old format (explicit quads of the offset models, water back face as a
/// second quad): that of the `EXPECTED` values.
fn old_job(cols: &Cols, x: i32, z: i32) -> MeshJob {
    let mut j = job(cols, x, z);
    j.opts.offset_faces = false;
    j.opts.water_double = false;
    j
}

// Fingerprints of the compact formats (default since C3-M03, `common::EXPECTED_COMPACT`): default,
// reference version and partial remesh (sections 4 to 10) equal; equivalence with
// the old format is proven by `tests/formats.rs`.

#[test]
fn compact_format_digests_match_stored() {
    let Some((_d, cols, t, palette)) = common::load() else { return };
    assert_eq!((common::capture_fingerprint(), _d.states.len(), _d.biomes.len()), INPUT, "capture or data differ");
    let mut keys: Vec<(i32, i32)> = cols.keys().copied().collect();
    keys.sort_unstable();
    let base: Vec<BlockMesh> = keys.par_iter().map(|&(x, z)| mesh(&job(&cols, x, z), &t, &palette, MIN_Y)).collect();
    let slow: Vec<BlockMesh> =
        keys.par_iter().map(|&(x, z)| mesh_reference(&job(&cols, x, z), &t, &palette, MIN_Y)).collect();
    let mut mask = Sections::default();
    (4..=10).for_each(|s| mask.insert(s));
    let reused: Vec<BlockMesh> = keys
        .par_iter()
        .zip(&base)
        .map(|(&(x, z), m)| {
            let mut j = job(&cols, x, z);
            j.reuse = Some((mask, Arc::new(pack(m)) as Packed));
            mesh(&j, &t, &palette, MIN_Y)
        })
        .collect();
    let (d, s, r) = (total(base.iter()), total(slow.iter()), total(reused.iter()));
    eprintln!("compact formats: 0x{d:016x}");
    assert_eq!(s, d, "reference version differs (compact formats)");
    assert_eq!(r, d, "partial remesh differs from the full one (compact formats)");
    assert_eq!(d, EXPECTED_COMPACT, "compact formats fingerprint changed: 0x{d:016x}");
    // Faithful flat copy, and one bit of an offset face is seen.
    let copy: Vec<BlockMesh> = base.iter().map(|m| rc_mesh::blocks::unpack(&pack(m), 0).expect("flat")).collect();
    assert_eq!(total(copy.iter()), d);
    let mut sab = copy;
    let f = sab.iter_mut().flat_map(|m| m.layers.iter_mut().flatten()).find(|f| f[0] >> 28 != 0).expect("offset");
    f[1] ^= 1 << 20;
    assert_ne!(total(sab.iter()), d, "offset nibble not seen");
}

fn mesh_all(keys: &[(i32, i32)], cols: &Cols, t: &Tables, p: &BiomePalette, set: impl Fn(&mut MeshJob) + Sync) -> Vec<BlockMesh> {
    keys.par_iter()
        .map(|&(x, z)| {
            let mut j = old_job(cols, x, z);
            set(&mut j);
            mesh(&j, t, p, MIN_Y)
        })
        .collect()
}

#[test]
fn mesh_digests_match_stored() {
    let Some((d, cols, t, palette)) = common::load() else { return };
    // Same inputs as the computation of the stored values: otherwise a failure would say nothing about the mesher.
    let input = (common::capture_fingerprint(), d.states.len(), d.biomes.len());
    assert_eq!(
        input, INPUT,
        "capture or data differ from those of the computation (08df4a1): fingerprints not comparable, \
         do NOT copy new values without recomputing on 08df4a1"
    );
    let mut keys: Vec<(i32, i32)> = cols.keys().copied().collect();
    keys.sort_unstable();
    let base = mesh_all(&keys, &cols, &t, &palette, |_| {});
    // Version without shortcuts: same mesh.
    let slow: Vec<BlockMesh> =
        keys.par_iter().map(|&(x, z)| mesh_reference(&old_job(&cols, x, z), &t, &palette, MIN_Y)).collect();
    assert_eq!(total(slow.iter()), total(base.iter()), "mesh differs from the reference version");
    drop(slow);
    type Set<'a> = &'a (dyn Fn(&mut MeshJob) + Sync);
    let desert = d.biomes.iter().position(|b| b.name == "minecraft:desert").expect("desert") as u32;
    let variants: [(&str, Set); 7] = [
        ("blend 0", &|j| j.opts.blend = 0),
        ("blend 1", &|j| j.opts.blend = 1),
        ("blend 7", &|j| j.opts.blend = 7),
        ("no occlusion", &|j| j.opts.ao = false),
        ("opaque leaves", &|j| j.opts.cutout_leaves = false),
        ("no sky, nether shading", &|j| {
            j.has_sky = false;
            j.shade = SHADE_NETHER;
        }),
        ("absent columns as desert", &|j| j.plains_biome = desert),
    ];
    let mut got = vec![("default", total(base.iter()))];
    for (name, set) in variants {
        got.push((name, total(mesh_all(&keys, &cols, &t, &palette, set).iter())));
    }
    // Partial remesh: sections 4 to 10 redone on the base of the full mesh (same world).
    let mut mask = Sections::default();
    (4..=10).for_each(|s| mask.insert(s));
    let packed: Vec<Packed> = base.iter().map(|m| Arc::new(pack(m)) as Packed).collect();
    let reused: Vec<BlockMesh> = keys
        .par_iter()
        .zip(&packed)
        .map(|(&(x, z), w)| {
            let mut j = old_job(&cols, x, z);
            j.reuse = Some((mask, w.clone()));
            mesh(&j, &t, &palette, MIN_Y)
        })
        .collect();
    got.push(("partial remesh", total(reused.iter())));

    let faces: usize = base.iter().map(BlockMesh::len).sum();
    eprintln!("{} columns, {faces} faces (default)", keys.len());
    let listing: String = got.iter().map(|(n, d)| format!("    (\"{n}\", 0x{d:016x}),\n")).collect();
    eprintln!("fingerprints:\n{listing}");
    assert_eq!(got.len(), EXPECTED.len());
    for ((n, d), (en, e)) in got.iter().zip(EXPECTED) {
        assert_eq!(n, en);
        assert_eq!(*d, *e, "fingerprint '{n}' changed; current values:\n{listing}");
    }
    assert_eq!(got[8].1, got[0].1, "partial remesh differs from the full one");

    // Each setting does change the mesh (otherwise its variant would prove nothing).
    for (i, a) in got.iter().enumerate().take(8) {
        for b in &got[i + 1..8] {
            assert_ne!(a.1, b.1, "'{}' and '{}' give the same mesh", a.0, b.0);
        }
    }

    // Sabotages: each one changes the stored "default" fingerprint.
    let reference = EXPECTED[0].1;
    let copy = || -> Vec<BlockMesh> {
        base.iter().map(|m| rc_mesh::blocks::unpack(&pack(m), 0).expect("flat")).collect()
    };
    assert_eq!(total(copy().iter()), reference, "unfaithful flat copy");
    let sabotaged = |f: &dyn Fn(&mut BlockMesh) -> bool| {
        let mut ms = copy();
        assert!(ms.iter_mut().any(f), "no column to sabotage");
        total(ms.iter())
    };
    // Two different faces of a same layer swapped.
    let swap = sabotaged(&|m| {
        let l = &mut m.layers[0];
        match (1..l.len()).find(|&k| l[k] != l[0]) {
            Some(k) => {
                l.swap(0, k);
                true
            }
            None => false,
        }
    });
    assert_ne!(swap, reference, "swap of two faces not seen");
    // A visibility bit.
    let vis = sabotaged(&|m| m.vis.first_mut().map(|v| *v ^= 1 << 7).is_some());
    assert_ne!(vis, reference, "visibility bit not seen");
    // A fluid start index.
    let starts = sabotaged(&|m| {
        m.fluid_starts.iter_mut().find(|s| !s.is_empty()).and_then(|s| s.last_mut()).map(|v| *v += 1).is_some()
    });
    assert_ne!(starts, reference, "fluid_starts index not seen");
    // A fluid corner by one ulp.
    let corner = sabotaged(&|m| {
        m.fluids.iter_mut().flatten().next().map(|q| q.corners[0][1] = f32::from_bits(q.corners[0][1].to_bits() ^ 1)).is_some()
    });
    assert_ne!(corner, reference, "fluid corner not seen");
}

/// Biome tint paths (C2-M06): each combination gives the stored fingerprints
/// "default", "blend 0/1/7" and "absent columns as desert" on all the columns;
/// then `Tint` equals the direct computation without cache on a sample of columns, radii 0 to 7.
#[test]
fn tint_paths_digests_match_stored() {
    use rc_mesh::blocks::TintPaths;
    let Some((d, cols, t, palette)) = common::load() else { return };
    assert_eq!((common::capture_fingerprint(), d.states.len(), d.biomes.len()), INPUT, "capture or data differ");
    let mut keys: Vec<(i32, i32)> = cols.keys().copied().collect();
    keys.sort_unstable();
    let desert = d.biomes.iter().position(|b| b.name == "minecraft:desert").expect("desert") as u32;
    let (all, old) = (TintPaths::ALL, TintPaths::OLD);
    let combos = [
        all,
        old,
        TintPaths { uni: false, ..all },
        TintPaths { lattice: false, ..all },
        TintPaths { grid: false, ..all },
        TintPaths { idx: false, ..all },
        TintPaths { boxsum: 0, ..all },
        TintPaths { boxsum: 1, ..all },
        TintPaths { uni: true, ..old },
        TintPaths { lattice: true, ..old },
        TintPaths { grid: true, ..old },
        TintPaths { idx: true, ..old },
        TintPaths { boxsum: 1, ..old },
    ];
    let expected = |n: &str| EXPECTED.iter().find(|e| e.0 == n).expect("variant").1;
    for p in combos {
        type Set<'a> = &'a (dyn Fn(&mut MeshJob) + Sync);
        let variants: [(&str, Set); 5] = [
            ("default", &|_| {}),
            ("blend 0", &|j| j.opts.blend = 0),
            ("blend 1", &|j| j.opts.blend = 1),
            ("blend 7", &|j| j.opts.blend = 7),
            ("absent columns as desert", &|j| j.plains_biome = desert),
        ];
        for (name, set) in variants {
            let got = total(
                mesh_all(&keys, &cols, &t, &palette, |j| {
                    set(j);
                    j.tint = p;
                })
                .iter(),
            );
            assert_eq!(got, expected(name), "fingerprint '{name}' changed with {p:?}");
        }
    }
    // Sample (one column in 64, neighbours present or not) against the direct computation.
    let sample: Vec<(i32, i32)> = keys.iter().copied().step_by(64).collect();
    let (cols, palette) = (&cols, &palette);
    let fails: Vec<String> = sample
        .par_iter()
        .flat_map_iter(|&(x, z)| {
            (0..=7).filter_map(move |rad| {
                let mut j = job(cols, x, z);
                j.opts.blend = rad;
                rc_mesh::blocks::tint_matches_reference(&j, palette, &combos, 7).map(|e| format!("({x}, {z}) radius {rad}: {e}"))
            })
        })
        .collect();
    assert!(fails.is_empty(), "tints differ from the direct computation: {fails:?}");
}

fn pack(m: &BlockMesh) -> Vec<u32> {
    let mut w = Vec::new();
    rc_mesh::blocks::pack(m, &mut w);
    w
}
