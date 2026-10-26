//! Per-block loop and face removal (C2-M10): each combination of the paths
//! (`CullPaths`: property grid, palette, candidate rows, border stacks) returns,
//! for each removal rule, the fingerprint STORED here over the whole reference capture
//! (compact formats; default settings, opaque leaves, no occlusion); the reference
//! version (without shortcuts) and the partial remesh return it too.
//!
//! Old rule: the "default" fingerprint is `EXPECTED_COMPACT` (before the series). Game rule
//! (face shapes, `RUSTCRAFT_OPT_SHAPECULL`): new values, justified by
//! `face_rule_matches_known_states` (face shapes of known states, inclusion against an
//! independent sampling) and by the reference version, which applies the rule face by
//! face without the grid or the rows.
//!
//! Without data (capture, cache, installed game) the test is ignored; under `RUSTCRAFT_OPT_REQUIRE=1`
//! it is a failure.

mod common;

use std::sync::Arc;

use common::{Cols, EXPECTED_COMPACT, MIN_Y, job_with};
use rayon::prelude::*;
use rc_mesh::blocks::{BiomePalette, BlockMesh, Packed, Tables, digest, mesh, mesh_reference};
use rc_mesh::cull::{CullPaths, FACE_BLOCK, FACE_EMPTY};
use rc_mesh::wire::{MeshJob, Sections};

/// Expected fingerprints per rule (old, game): default, opaque leaves, no occlusion.
const EXPECTED: [[u64; 3]; 2] = [
    [EXPECTED_COMPACT, 0x1b0f_f1b4_7b27_c532, 0x5dbd_7a6c_3e30_3263],
    // Game rule: 6,189,376 faces instead of 6,193,742 (−4,366, of which 5 fluid ones) default settings.
    [0x6015_660c_daa2_d469, 0xc10a_24a5_6d12_3545, 0x660e_1344_bdf3_84ab],
];

fn total<'a>(meshes: impl Iterator<Item = &'a BlockMesh>) -> u64 {
    meshes.fold(0u64, |s, m| {
        let at = (u64::from(m.x as u32) << 32 | u64::from(m.z as u32)).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        s.wrapping_add(digest(m) ^ at)
    })
}

fn faces(meshes: &[BlockMesh]) -> usize {
    meshes.iter().map(BlockMesh::len).sum()
}

type Set<'a> = &'a (dyn Fn(&mut MeshJob) + Sync);

fn mesh_all(keys: &[(i32, i32)], cols: &Cols, t: &Tables, p: &BiomePalette, cull: CullPaths, set: Set) -> Vec<BlockMesh> {
    keys.par_iter()
        .map(|&(x, z)| {
            let mut j = job_with(cols, x, z, cull);
            set(&mut j);
            mesh(&j, t, p, MIN_Y)
        })
        .collect()
}

/// The 16 combinations of the loop paths for a rule.
fn combos(shape: bool) -> Vec<CullPaths> {
    (0..16u8)
        .map(|b| CullPaths { props: b & 1 != 0, palprops: b & 2 != 0, rows: b & 4 != 0, pile: b & 8 != 0, shape })
        .collect()
}

#[test]
fn cull_paths_match_stored() {
    let Some((_d, cols, t, palette)) = common::load() else { return };
    let mut keys: Vec<(i32, i32)> = cols.keys().copied().collect();
    keys.sort_unstable();
    let variants: [(&str, Set); 3] = [
        ("default", &|_| {}),
        ("opaque leaves", &|j| j.opts.cutout_leaves = false),
        ("no occlusion", &|j| j.opts.ao = false),
    ];
    let mut listing = String::new();
    let mut bad = Vec::new();
    for shape in [false, true] {
        let rule = usize::from(shape);
        let mut got = [0u64; 3];
        for (v, (name, set)) in variants.iter().enumerate() {
            let base = mesh_all(&keys, &cols, &t, &palette, CullPaths { shape, ..CullPaths::OLD }, *set);
            got[v] = total(base.iter());
            eprintln!("rule {rule}, {name}: {} faces", faces(&base));
            for p in combos(shape) {
                let d = total(mesh_all(&keys, &cols, &t, &palette, p, *set).iter());
                if d != got[v] {
                    bad.push(format!("{p:?}, {name}: 0x{d:016x} instead of 0x{:016x}", got[v]));
                }
            }
            // Reference without shortcuts: the rule applied face by face.
            let slow: Vec<BlockMesh> = keys
                .par_iter()
                .map(|&(x, z)| {
                    let mut j = job_with(&cols, x, z, CullPaths { shape, ..CullPaths::DEFAULT });
                    set(&mut j);
                    mesh_reference(&j, &t, &palette, MIN_Y)
                })
                .collect();
            let s = total(slow.iter());
            if s != got[v] {
                bad.push(format!("reference, rule {rule}, {name}: 0x{s:016x} instead of 0x{:016x}", got[v]));
            }
            if v == 0 {
                // Partial remesh (sections 4 to 10) on the base of the full mesh, default paths.
                let mut mask = Sections::default();
                (4..=10).for_each(|s| mask.insert(s));
                let reused: Vec<BlockMesh> = keys
                    .par_iter()
                    .zip(&base)
                    .map(|(&(x, z), m)| {
                        let mut j = job_with(&cols, x, z, CullPaths { shape, ..CullPaths::DEFAULT });
                        j.reuse = Some((mask, Arc::new(rc_mesh::blocks::pack_exact(m)) as Packed));
                        mesh(&j, &t, &palette, MIN_Y)
                    })
                    .collect();
                let r = total(reused.iter());
                if r != got[0] {
                    bad.push(format!("partial remesh, rule {rule}: 0x{r:016x}"));
                }
            }
        }
        listing += &format!("    [0x{:016x}, 0x{:016x}, 0x{:016x}],\n", got[0], got[1], got[2]);
        for (v, (name, _)) in variants.iter().enumerate() {
            if got[v] != EXPECTED[rule][v] {
                bad.push(format!("rule {rule}, {name}: fingerprint 0x{:016x} instead of 0x{:016x}", got[v], EXPECTED[rule][v]));
            }
        }
    }
    eprintln!("fingerprints:\n{listing}");
    assert!(bad.is_empty(), "{}\ncurrent values:\n{listing}", bad.join("\n"));
}

/// First state of block `name` whose properties contain all those of `props`.
fn state(d: &rc_data::VersionData, name: &str, props: &[(&str, &str)]) -> u32 {
    let b = d.blocks.iter().position(|b| b.name == name).unwrap_or_else(|| panic!("block {name}")) as u16;
    (0..d.states.len() as u32)
        .find(|&s| {
            d.states[s as usize].block == b && props.iter().all(|p| d.state_properties(s).any(|q| q == *p))
        })
        .unwrap_or_else(|| panic!("state {name} {props:?}"))
}

/// Inclusion by sampling at the centres of a 1/64 grid (exact for shapes in sixteenths).
fn sampled_within(a: &[[f64; 4]], b: &[[f64; 4]]) -> bool {
    let inside = |r: &[f64; 4], u: f64, v: f64| r[0] <= u && u < r[1] && r[2] <= v && v < r[3];
    (0..64).all(|i| {
        (0..64).all(|j| {
            let (u, v) = ((f64::from(i) + 0.5) / 64.0, (f64::from(j) + 0.5) / 64.0);
            !a.iter().any(|r| inside(r, u, v)) || b.iter().any(|r| inside(r, u, v))
        })
    })
}

#[test]
fn face_rule_matches_known_states() {
    let Some(d) = rc_testdata::version_data() else { return };
    let empty: Vec<bool> = vec![false; d.states.len()];
    let zeros = vec![0u8; d.states.len()];
    let ct = rc_mesh::cull::CullTables::new(&d, &zeros, &zeros, &empty, &empty, &empty, &empty);
    eprintln!("{} distinct face shapes, {} outside sixteenths", ct.shapes(), ct.inexact());
    let (down, up, north, south) = (0, 1, 2, 3);
    let stone = state(&d, "minecraft:stone", &[]);
    let glass = state(&d, "minecraft:glass", &[]);
    let slab = state(&d, "minecraft:smooth_stone_slab", &[("type", "bottom")]);
    let top = state(&d, "minecraft:smooth_stone_slab", &[("type", "top")]);
    let stairs = state(&d, "minecraft:oak_stairs", &[("facing", "north"), ("half", "bottom"), ("shape", "straight")]);
    // Stone: full cube; glass: does not occlude.
    assert!((0..6).all(|k| ct.face(stone, k) == FACE_BLOCK));
    assert!((0..6).all(|k| ct.face(glass, k) == FACE_EMPTY));
    // Bottom slab: bottom full cube, top empty, sides half-face.
    assert_eq!(ct.face(slab, down), FACE_BLOCK);
    assert_eq!(ct.face(slab, up), FACE_EMPTY);
    assert_eq!(ct.face(top, up), FACE_BLOCK);
    let side = ct.face(slab, north);
    assert!(side > FACE_BLOCK);
    // Two slabs side by side: faces hidden (included); bottom slab against top slab: visible.
    assert!(ct.hidden_by_shape(slab, slab, north));
    assert!(!ct.hidden_by_shape(slab, top, north));
    // Slab against stone: hidden; stone against the slab: visible (half-face).
    assert!(ct.hidden_by_shape(slab, stone, north));
    assert!(!ct.hidden_by_shape(stone, slab, south));
    // Top of glass under a stair: the stair's underside covers but is not the game's full
    // cube (grid cut in z) → glass face visible; stone below: hidden.
    let under = ct.face(stairs, down);
    assert!(under > FACE_BLOCK, "stair underside: {under}");
    assert!(ct.within(FACE_BLOCK, under));
    assert!(!ct.hidden_by_shape(glass, stairs, up));
    assert!(ct.hidden_by_shape(stone, stairs, up));
    // Inclusion table against an independent sampling, all pairs in sixteenths.
    let n = ct.shapes() as u16;
    let exact = |id: u16| ct.rects(id).iter().all(|r| r.iter().all(|&x| x * 16.0 == (x * 16.0).round()));
    let block = [[0.0, 1.0, 0.0, 1.0]];
    let rects = |id: u16| if id == FACE_BLOCK { &block[..] } else { ct.rects(id) };
    let mut checked = 0;
    for a in 1..n {
        for b in 1..n {
            if exact(a) && exact(b) {
                assert_eq!(ct.within(a, b), sampled_within(rects(a), rects(b)), "shapes {a} ⊆ {b}");
                checked += 1;
            }
        }
    }
    eprintln!("{checked} inclusion pairs checked");

    // Fluids (game rule): full cube neighbour, included shape, waterlogged block itself.
    let (west, east) = (4, 5);
    let src = 8.0 / 9.0;
    assert!(ct.fluid_hidden(stone, north, 0.1) && ct.fluid_hidden(stone, up, 1.0));
    assert!(!ct.fluid_hidden(stone, up, src), "top lower than 1 under a full cube: visible");
    assert!(!ct.fluid_hidden(glass, north, 1.0));
    // Side against a bottom slab (half-face 0..0.5): hidden up to 0.5 inclusive.
    for d in [north, south, west, east] {
        assert!(ct.fluid_hidden(slab, d, 0.5), "slab, side {d}, h 0.5");
        assert!(ct.fluid_hidden(slab, d, 0.3), "slab, side {d}, h 0.3");
        assert!(!ct.fluid_hidden(slab, d, 0.6), "slab, side {d}, h 0.6");
    }
    // Top under a bottom slab (full underside cube) and under the stair (full, not a cube):
    // hidden only at height 1; bottom on a top slab: hidden; on a bottom slab: not.
    assert!(ct.fluid_hidden(slab, up, 1.0) && !ct.fluid_hidden(slab, up, src));
    assert!(ct.fluid_hidden(stairs, up, 1.0) && !ct.fluid_hidden(stairs, up, src));
    assert!(ct.fluid_hidden(top, down, src) && !ct.fluid_hidden(slab, down, src));
    // Stair facing north: north face full through the union of its two boxes.
    assert!(ct.fluid_hidden(stairs, south, src), "water north of the stair's back");
    assert!(!ct.fluid_hidden(stairs, north, src) && ct.fluid_hidden(stairs, north, 0.5));
    let wet = state(&d, "minecraft:oak_stairs", &[("facing", "north"), ("half", "bottom"), ("shape", "straight"), ("waterlogged", "true")]);
    let own: Vec<bool> = (0..6).map(|k| ct.fluid_self_hidden(wet, k)).collect();
    assert_eq!(own, [true, false, true, false, false, false], "water faces hidden by the stair");
    assert!((0..6).all(|k| !ct.fluid_self_hidden(glass, k)));
}

/// Fluids against non-full shapes, scene built in an empty section of the capture:
/// waterlogged stair (its back hides the water), water against the back of a stair, low water against
/// a bottom slab. The game rule removes these three faces; fingerprint of the column stored.
#[test]
fn fluid_faces_follow_shapes() {
    let Some((d, cols, t, palette)) = common::load() else { return };
    // Solid render table = "one box covers the six sides" (test of the back faces of the top).
    let odd = (0..d.states.len()).filter(|&s| (t.covers[s] == 0x3f) != t.solid[s]).count();
    assert_eq!(odd, 0, "states whose full coverage differs from the solid render");
    let mut keys: Vec<(i32, i32)> = cols.keys().copied().collect();
    keys.sort_unstable();
    let key = keys[keys.len() / 2];
    let mut c = (*cols[&key]).clone();
    let si = c.sections.iter().rposition(|s| !s.is_empty()).expect("non-empty section") + 1;
    assert!(si < c.sections.len() && c.sections[si].is_empty());
    let stairs = state(&d, "minecraft:oak_stairs", &[("facing", "north"), ("half", "bottom"), ("shape", "straight"), ("waterlogged", "false")]);
    let wet = state(&d, "minecraft:oak_stairs", &[("facing", "north"), ("half", "bottom"), ("shape", "straight"), ("waterlogged", "true")]);
    let water = state(&d, "minecraft:water", &[("level", "0")]);
    let low = state(&d, "minecraft:water", &[("level", "4")]);
    let slab = state(&d, "minecraft:smooth_stone_slab", &[("type", "bottom"), ("waterlogged", "false")]);
    let at = |x: u16, y: u16, z: u16| y << 8 | z << 4 | x;
    let changes = [
        (at(4, 4, 4), wet),
        (at(10, 4, 9), water),
        (at(10, 4, 10), stairs),
        (at(4, 4, 10), low),
        (at(4, 4, 11), slab),
    ];
    let bits = (u32::BITS - (d.states.len() as u32 - 1).leading_zeros()) as u8;
    c.sections[si].set_blocks(&changes, bits, |s| t.air[s as usize]);
    let mut built = cols.clone();
    built.insert(key, Arc::new(c));
    let run = |cs: &common::Cols, shape: bool| {
        let j = job_with(cs, key.0, key.1, CullPaths { shape, ..CullPaths::DEFAULT });
        let m = mesh(&j, &t, &palette, MIN_Y);
        let r = mesh_reference(&j, &t, &palette, MIN_Y);
        assert_eq!(digest(&m), digest(&r), "reference, rule {shape}");
        (m.fluids.iter().map(Vec::len).sum::<usize>(), digest(&m), j.opts.water_double)
    };
    let (n0, _, double) = run(&built, false);
    let (n1, d1, _) = run(&built, true);
    let (o0, ..) = run(&cols, false);
    let (o1, ..) = run(&cols, true);
    // Three side faces removed, each double-sided (a marked quad, or two).
    let per = if double { 1 } else { 2 };
    assert_eq!((n0 - o0) - (n1 - o1), 3 * per, "fluid faces: old {n0}-{o0}, game {n1}-{o1}");
    assert_eq!(d1, 0xe5ad_f141_5550_1c97, "scene fingerprint (game rule): 0x{d1:016x}");
}

/// Share of the capture's non-empty blocks that exit through "buried" (ceiling of the rows' gain)
/// and faces emitted per rule.
#[test]
fn buried_share() {
    use rc_mesh::cull::{AIR_BIT, candidate, sure_hidden};
    let Some((_d, cols, t, _palette)) = common::load() else { return };
    // State at the absolute block (x, y relative to the bottom of the world, z); outside the columns: no properties.
    let at = |x: i32, y: i32, z: i32| -> u32 {
        let Some(c) = cols.get(&(x.div_euclid(16), z.div_euclid(16))) else { return u32::MAX };
        if y < 0 {
            return u32::MAX;
        }
        c.block(x.rem_euclid(16) as usize, y as usize, z.rem_euclid(16) as usize).unwrap_or(u32::MAX)
    };
    let mut n = [0usize; 3];
    for c in cols.values() {
        for (si, s) in c.sections.iter().enumerate() {
            if s.is_empty() {
                continue;
            }
            for i in 0..4096usize {
                let (x, z, y) = (c.x * 16 + (i & 15) as i32, c.z * 16 + ((i >> 4) & 15) as i32, (si * 16 + (i >> 8)) as i32);
                let p = t.cull.prop(1, at(x, y, z));
                if p & AIR_BIT != 0 {
                    continue;
                }
                n[0] += 1;
                let nb = [(0, -1, 0), (0, 1, 0), (0, 0, -1), (0, 0, 1), (-1, 0, 0), (1, 0, 0)]
                    .map(|(dx, dy, dz)| t.cull.prop(1, at(x + dx, y + dy, z + dz)));
                if candidate(p, sure_hidden(p, nb)) {
                    n[1] += 1;
                } else {
                    n[2] += 1;
                }
            }
        }
    }
    eprintln!(
        "non-empty blocks {}: candidates {} ({:.1} %), buried or without face {} ({:.1} %)",
        n[0],
        n[1],
        100.0 * n[1] as f64 / n[0] as f64,
        n[2],
        100.0 * n[2] as f64 / n[0] as f64
    );
    assert_eq!(n[1] + n[2], n[0]);
}
