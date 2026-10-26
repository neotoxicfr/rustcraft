//! Equivalence proof of the smoothed light paths (C2-M07, `RUSTCRAFT_OPT_AO*`
//! switches): each path alone, all except one and all together give the mesh
//! of the old path bit for bit (fingerprint of all the fields), on the whole reference capture
//! (occlusion on, off, dimension without sky) and on a built scene (grass, stairs,
//! slabs, leaves, water, emissive and luminous blocks, in contact with each other). The cache
//! kept by the thread is proven on two successive tasks of the same thread and at the wrap of the
//! generation through its maximum (16 B and 6 B cells).
//!
//! Without data (capture, cache, installed game) the test is ignored; under `RUSTCRAFT_OPT_REQUIRE=1`
//! it is a failure.

mod common;

use std::sync::Arc;

use common::{Cols, EXPECTED_COMPACT, MIN_Y, job};
use rayon::prelude::*;
use rc_mesh::blocks::{BiomePalette, BlockMesh, Tables, digest, mesh};
use rc_mesh::light::LightPaths;
use rc_mesh::wire::{MeshJob, SHADE_NETHER};


fn total<'a>(meshes: impl Iterator<Item = &'a BlockMesh>) -> u64 {
    meshes.fold(0u64, |s, m| {
        let at = (u64::from(m.x as u32) << 32 | u64::from(m.z as u32)).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        s.wrapping_add(digest(m) ^ at)
    })
}

/// Old path, each path alone, all except one, all, and the default.
fn variants() -> Vec<(String, LightPaths)> {
    let (old, all) = (LightPaths::OLD, LightPaths::ALL);
    type Field = fn(&mut LightPaths) -> &mut bool;
    let fields: [(&str, Field); 7] = [
        ("cell", |p| &mut p.cell),
        ("corners", |p| &mut p.corners),
        ("facetab", |p| &mut p.facetab),
        ("scratch", |p| &mut p.scratch),
        ("cell8", |p| &mut p.cell8),
        ("fluid", |p| &mut p.fluid),
        ("neigh", |p| &mut p.neigh),
    ];
    let mut v = vec![("old".to_string(), old)];
    for (name, f) in fields {
        let mut one = old;
        *f(&mut one) = true;
        v.push((name.to_string(), one));
        let mut but = all;
        *f(&mut but) = false;
        v.push((format!("all_but_{name}"), but));
    }
    v.push(("all".to_string(), all));
    v.push(("default".to_string(), LightPaths::DEFAULT));
    v
}

/// Settings proven: default, occlusion off, dimension without sky with "nether" shading.
type Setting = (&'static str, fn(&mut MeshJob));

fn settings() -> [Setting; 3] {
    [
        ("default", |_| {}),
        ("no occlusion", |j| j.opts.ao = false),
        ("no sky", |j| {
            j.has_sky = false;
            j.shade = SHADE_NETHER;
        }),
    ]
}

fn mesh_all(
    keys: &[(i32, i32)],
    cols: &Cols,
    t: &Tables,
    p: &BiomePalette,
    paths: LightPaths,
    set: fn(&mut MeshJob),
) -> u64 {
    let m: Vec<BlockMesh> = keys
        .par_iter()
        .map(|&(x, z)| {
            let mut j = job(cols, x, z);
            j.light = paths;
            set(&mut j);
            mesh(&j, t, p, MIN_Y)
        })
        .collect();
    total(m.iter())
}

#[test]
fn light_paths_match_old_on_capture() {
    let Some((_d, cols, t, palette)) = common::load() else { return };
    let mut keys: Vec<(i32, i32)> = cols.keys().copied().collect();
    keys.sort_unstable();
    let mut bad = Vec::new();
    for (sname, set) in settings() {
        let old = mesh_all(&keys, &cols, &t, &palette, LightPaths::OLD, set);
        if sname == "default" {
            assert_eq!(old, EXPECTED_COMPACT, "old path: stored fingerprint changed (0x{old:016x})");
        }
        for (name, p) in variants() {
            let d = mesh_all(&keys, &cols, &t, &palette, p, set);
            if d != old {
                bad.push(format!("{sname} / {name}: 0x{d:016x} instead of 0x{old:016x}"));
            }
        }
    }
    assert!(bad.is_empty(), "light paths differ from the old one:\n{}", bad.join("\n"));
}

/// Middle column of the capture (its 8 neighbours loaded).
fn middle(cols: &Cols) -> (i32, i32) {
    let mut keys: Vec<(i32, i32)> = cols.keys().copied().collect();
    keys.sort_unstable();
    *keys
        .iter()
        .filter(|&&(x, z)| (-1..=1).all(|dx| (-1..=1).all(|dz| cols.contains_key(&(x + dx, z + dz)))))
        .nth(keys.len() / 4)
        .expect("surrounded column")
}

/// Scene built in the highest non-empty section of a column of the capture (real
/// light): a mix drawn from a fixed seed of air, grass, stairs in several states, slabs,
/// leaves, water, emissive blocks (magma) and luminous ones (glowstone, sea
/// lantern), torches and tall grass.
fn scene(d: &rc_data::VersionData, cols: &Cols) -> Cols {
    let state = |name: &str, k: u32| {
        let b = d.blocks.iter().find(|b| b.name == name).unwrap_or_else(|| panic!("block {name}"));
        if k == 0 { b.default_state } else { b.first_state + k % b.state_count }
    };
    let palette = [
        state("minecraft:air", 0),
        state("minecraft:air", 0),
        state("minecraft:air", 0),
        state("minecraft:grass_block", 0),
        state("minecraft:grass_block", 0),
        state("minecraft:oak_stairs", 0),
        state("minecraft:oak_stairs", 7),
        state("minecraft:oak_stairs", 29),
        state("minecraft:stone_slab", 0),
        state("minecraft:stone_slab", 2),
        state("minecraft:oak_leaves", 0),
        state("minecraft:water", 0),
        state("minecraft:water", 3),
        state("minecraft:magma_block", 0),
        state("minecraft:glowstone", 0),
        state("minecraft:sea_lantern", 0),
        state("minecraft:torch", 0),
        state("minecraft:short_grass", 0),
        state("minecraft:stone", 0),
        state("minecraft:dirt_path", 0),
    ];
    let (cx, cz) = middle(cols);
    let mut c = (*cols[&(cx, cz)]).clone();
    let si = c.sections.iter().rposition(|s| !s.is_empty()).expect("non-empty section");
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut changes = Vec::new();
    for i in 0..4096u16 {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        changes.push((i, palette[(seed >> 33) as usize % palette.len()]));
    }
    let bits = (u32::BITS - (d.states.len() as u32 - 1).leading_zeros()) as u8;
    c.sections[si].set_blocks(&changes, bits, |s| d.states[s as usize].has(rc_data::flags::AIR));
    let mut out = cols.clone();
    out.insert((cx, cz), Arc::new(c));
    out
}

#[test]
fn light_paths_match_old_on_built_scene() {
    let Some((d, cols, t, palette)) = common::load() else { return };
    let cols = scene(&d, &cols);
    let key = middle(&cols);
    let mut bad = Vec::new();
    for (sname, set) in settings() {
        let run = |p: LightPaths| {
            let mut j = job(&cols, key.0, key.1);
            j.light = p;
            set(&mut j);
            mesh(&j, &t, &palette, MIN_Y)
        };
        let old = run(LightPaths::OLD);
        let faces: usize = old.layers.iter().map(Vec::len).sum::<usize>() + old.fluids.iter().map(Vec::len).sum::<usize>();
        assert!(faces > 5_000, "scene too poor ({faces} faces)");
        for (name, p) in variants() {
            if !common::same(&run(p), &old) {
                bad.push(format!("{sname} / {name}"));
            }
        }
    }
    assert!(bad.is_empty(), "scene: mesh differs from the old path:\n{}", bad.join("\n"));
}

/// Two columns meshed one after the other on a new thread, with `epoch` set between the two
/// (`None`: nothing); returns the second.
fn two_on_fresh_thread(
    cols: &Cols,
    t: &Tables,
    palette: &BiomePalette,
    a: (i32, i32),
    b: (i32, i32),
    paths: LightPaths,
    epoch: Option<u32>,
) -> BlockMesh {
    std::thread::scope(|s| {
        s.spawn(|| {
            let run = |k: (i32, i32)| {
                let mut j = job(cols, k.0, k.1);
                j.light = paths;
                mesh(&j, t, palette, MIN_Y)
            };
            run(a);
            if let Some(e) = epoch {
                rc_mesh::blocks::set_light_epoch(e);
            }
            run(b)
        })
        .join()
        .expect("meshing thread")
    })
}

#[test]
fn kept_cache_across_tasks_and_epoch_wrap() {
    let Some((_d, cols, t, palette)) = common::load() else { return };
    let a = middle(&cols);
    let b = (a.0 + 1, a.1);
    let fresh = two_on_fresh_thread(&cols, &t, &palette, b, b, LightPaths::OLD, None);
    let kept = LightPaths { scratch: true, ..LightPaths::OLD };
    for (name, p) in [("16 B", kept), ("6 B", LightPaths { cell8: true, ..kept }), ("default", LightPaths::DEFAULT)] {
        // Second task of the thread: the generation continues (reset to 1, the cells of the first
        // would be taken for valid).
        let m = two_on_fresh_thread(&cols, &t, &palette, a, b, p, None);
        assert!(common::same(&m, &fresh), "{name}: second task of the thread differs");
        // Generation passing through the maximum during the second task: cells emptied.
        let max = if p.cell8 { u32::from(u16::MAX) } else { u32::MAX };
        let m = two_on_fresh_thread(&cols, &t, &palette, a, b, p, Some(max - 1));
        assert!(common::same(&m, &fresh), "{name}: generation wrap through its maximum");
    }
}

/// Default = best proven combination (D + A + B); C, E, F, G are only turned on after a calm
/// machine bench that gives them as winners (then update this test and `docs/perf.md`).
#[test]
fn default_is_best_proven_combination() {
    let dab = LightPaths { cell: true, corners: true, scratch: true, ..LightPaths::OLD };
    assert_eq!(LightPaths::DEFAULT, dab);
}

/// Face geometry built only when its path is active (`RUSTCRAFT_OPT_AOFACETAB`):
/// off, neither its memory nor its walk at resource loading. Prints its build cost
/// and its size.
#[test]
fn face_tables_follow_their_switch() {
    let Some((d, _cols, t, _palette)) = common::load() else { return };
    let mut t0 = Tables::new(&d, t.models.clone());
    let on = LightPaths::from_env().facetab;
    assert_eq!(t0.light_faces.faces.is_empty(), !on, "face geometry built against its switch");
    t0.build_light_faces();
    assert_eq!(t0.light_faces.faces.len(), t.models.quads.len());
    let best = (0..5)
        .map(|_| {
            let s = std::time::Instant::now();
            std::hint::black_box(rc_mesh::light::FaceTables::new(t.models.quads.iter().map(|q| (&q.pos, q.dir))));
            s.elapsed().as_secs_f64() * 1e3
        })
        .fold(f64::MAX, f64::min);
    eprintln!(
        "face geometry: {} faces, {} B, built in {best:.2} ms (best of 5)",
        t0.light_faces.faces.len(),
        t0.light_faces.heap_bytes()
    );
}
