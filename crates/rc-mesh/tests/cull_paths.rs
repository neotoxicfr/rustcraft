//! Per-block loop and face removal (C2-M10, C2-M08): each combination of the paths
//! (`CullPaths`: property grid, palette, candidate rows, border stacks; rows fill, solid mask
//! from the grid, vector candidates, border rows of the partial remesh) returns,
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

use common::{Cols, EXPECTED_COMPACT, MIN_Y, job_with, state};
use rayon::prelude::*;
use rc_mesh::blocks::{BiomePalette, BlockMesh, Packed, Tables, digest, mesh, mesh_reference};
use rc_mesh::cull::{CullPaths, FACE_BLOCK, FACE_EMPTY};
use rc_mesh::fluid::FluidPaths;
use rc_mesh::wire::{MeshJob, Sections};

/// Expected fingerprints per rule (old, game): default, opaque leaves, no occlusion.
const EXPECTED: [[u64; 3]; 2] = [
    [EXPECTED_COMPACT, 0x8ff1_31b9_4bea_0b7a, 0x8f56_d4da_6fd0_83e3],
    // Game rule: 6,189,376 faces instead of 6,193,742 (−4,366, of which 5 fluid ones) default settings.
    [0xe684_40dc_dfe3_b631, 0x7cc0_d3af_93fe_c3e5, 0x43e6_1412_b904_93fb],
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

/// Combinations of the loop paths for a rule: the 16 of the grid paths (property grid, palette,
/// rows, border stacks), each with the fill and candidate paths (rows fill, solid mask from the
/// grid, vector candidates, border rows only) all off then all on; then the default with each of
/// those turned off alone.
fn combos(shape: bool) -> Vec<CullPaths> {
    let grid = |b: u8, newer: bool| CullPaths {
        props: b & 1 != 0,
        palprops: b & 2 != 0,
        rows: b & 4 != 0,
        pile: b & 8 != 0,
        shape,
        fill_rows: newer,
        solid_from_props: newer,
        simd_candidates: newer,
        border_rows_only: newer,
    };
    let mut all: Vec<CullPaths> = (0..32u8).map(|b| grid(b & 15, b & 16 != 0)).collect();
    let d = CullPaths { shape, ..CullPaths::DEFAULT };
    all.extend([
        CullPaths { fill_rows: false, ..d },
        CullPaths { solid_from_props: false, ..d },
        CullPaths { simd_candidates: false, ..d },
        CullPaths { border_rows_only: false, ..d },
    ]);
    all
}

/// Paths of the partial remesh checks: default, and the default without the rows fill or
/// without the border rows (the two ways the neighbours' border rows are read).
fn partial_combos(shape: bool) -> [CullPaths; 3] {
    let d = CullPaths { shape, ..CullPaths::DEFAULT };
    [d, CullPaths { fill_rows: false, ..d }, CullPaths { border_rows_only: false, ..d }]
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
                // Partial remesh (sections 4 to 10) on the base of the full mesh.
                let mut mask = Sections::default();
                (4..=10).for_each(|s| mask.insert(s));
                for p in partial_combos(shape) {
                    let reused: Vec<BlockMesh> = keys
                        .par_iter()
                        .zip(&base)
                        .map(|(&(x, z), m)| {
                            let mut j = job_with(&cols, x, z, p);
                            j.reuse = Some((mask, Arc::new(rc_mesh::blocks::pack_exact(m)) as Packed));
                            mesh(&j, &t, &palette, MIN_Y)
                        })
                        .collect();
                    let r = total(reused.iter());
                    if r != got[0] {
                        bad.push(format!("partial remesh, rule {rule}, {p:?}: 0x{r:016x}"));
                    }
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

/// Light bits of the property words (both rules): those of the light table, the solid bit equal
/// to the visibility's solid render table, and the culling bits unchanged by them.
#[test]
fn property_light_bits_follow_the_light_table() {
    use rc_mesh::cull::{DARK_SHADE_BIT, LIGHT_CLOSED_BIT, SOLID_RENDER_BIT, light_bits};
    let Some((d, _cols, t, _palette)) = common::load() else { return };
    let light = SOLID_RENDER_BIT | LIGHT_CLOSED_BIT | DARK_SHADE_BIT;
    for rule in 0..2 {
        for s in 0..d.states.len() {
            let p = t.cull.props[rule][s];
            assert_eq!(p & light, light_bits(t.light_info[s]), "rule {rule}, state {s}");
            assert_eq!(p & SOLID_RENDER_BIT != 0, t.solid[s], "rule {rule}, state {s}: solid render");
        }
        // Air (state 0) carries no light bit: an `OUTSIDE` cell (word 0) reads like it.
        assert_eq!(t.cull.props[rule][0] & light, 0);
    }
    let solid = (0..d.states.len()).filter(|&s| t.solid[s]).count();
    eprintln!("{solid} solid render states out of {}", d.states.len());
}

/// Edge cases of the grid fill and candidate paths, every combination against the reference: a
/// column without neighbours (`OUTSIDE` ring), states outside the table in the interior and on the
/// borders of sections, blocks at the very bottom and top of the world; then a partial remesh of
/// those sections.
#[test]
fn grid_fill_edge_cases_match_reference() {
    let Some((d, cols, t, palette)) = common::load() else { return };
    let mut keys: Vec<(i32, i32)> = cols.keys().copied().collect();
    keys.sort_unstable();
    let key = keys[keys.len() / 2];
    let mut c = (*cols[&key]).clone();
    let top = c.sections.len() - 1;
    let unknown = d.states.len() as u32 + 7;
    let stone = state(&d, "minecraft:stone", &[]);
    let glass = state(&d, "minecraft:glass", &[]);
    let at = |x: u16, y: u16, z: u16| y << 8 | z << 4 | x;
    let bits = (u32::BITS - (unknown).leading_zeros()) as u8;
    for (si, changes) in [
        // Bottom of the world: unknown state on the floor and on a corner, stone and glass at y = 0.
        (0, vec![(at(0, 0, 0), unknown), (at(5, 0, 5), stone), (at(6, 0, 5), glass), (at(8, 3, 8), unknown)]),
        // Middle: unknown states on the section's borders (rows 0 and 15, sides).
        (8, vec![(at(3, 0, 3), unknown), (at(3, 15, 3), unknown), (at(0, 7, 9), unknown), (at(15, 7, 9), stone)]),
        // Top of the world: blocks in the top row, an unknown state next to them.
        (top, vec![(at(4, 15, 4), stone), (at(5, 15, 4), glass), (at(6, 15, 4), unknown), (at(4, 14, 4), stone)]),
    ] {
        c.sections[si].set_blocks(&changes, bits, |s| t.is_air(s));
    }
    let alone: common::Cols = [(key, Arc::new(c.clone()))].into_iter().collect();
    let mut surrounded = cols.clone();
    surrounded.insert(key, Arc::new(c));
    let mut mask = Sections::default();
    [0, 7, 8, 9, top as i32].into_iter().for_each(|s| mask.insert(s));
    let mut bad = Vec::new();
    for (name, world) in [("without neighbours", &alone), ("with neighbours", &surrounded)] {
        for shape in [false, true] {
            let reference = job_with(world, key.0, key.1, CullPaths { shape, ..CullPaths::OLD });
            let want = digest(&mesh_reference(&reference, &t, &palette, MIN_Y));
            for p in combos(shape) {
                let full = mesh(&job_with(world, key.0, key.1, p), &t, &palette, MIN_Y);
                if digest(&full) != want {
                    bad.push(format!("{name}, {p:?}: full mesh differs from the reference"));
                }
                let mut j = job_with(world, key.0, key.1, p);
                j.reuse = Some((mask, Arc::new(rc_mesh::blocks::pack_exact(&full)) as Packed));
                if digest(&mesh(&j, &t, &palette, MIN_Y)) != want {
                    bad.push(format!("{name}, {p:?}: partial remesh differs from the reference"));
                }
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
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
    let ct = rc_mesh::cull::CullTables::new(&d, &zeros, &zeros, &empty, &empty, &empty, &empty, &[]);
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
    // Stone north of a stair facing north: no single box of the stair covers its back, the union
    // of the two (lower half from the base, upper half from the step) does → stone face hidden.
    assert!(ct.hidden_by_shape(stone, stairs, south), "stone against the back of a stair");
    assert!(!ct.hidden_by_shape(stone, stairs, north), "stone against the front of a stair");
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
    c.sections[si].set_blocks(&changes, bits, |s| t.is_air(s));
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

/// Fingerprint of the centre column of the random fluid scene (seed 1) per rule (old, game),
/// taken from the code before the fluid shortcuts existed.
const RANDOM_FLUIDS: [u64; 2] = [0x48ed_3f0b_65be_fb65, 0x1f19_ecf9_38c8_2c3b];

/// The fluid shortcuts each alone, in pairs and all together (every combination but none).
fn fluid_shortcut_combinations() -> Vec<FluidPaths> {
    let on = |bits: u8, shortcut: u8| bits >> shortcut & 1 == 1;
    (1..8)
        .map(|bits| FluidPaths { neighbor_memo: on(bits, 0), skip_buried: on(bits, 1), face_light_once: on(bits, 2) })
        .collect()
}

/// Blocks of the `sections` of column `key` that hold `state` like their six neighbours.
fn blocks_buried_in_own_state(world: &Cols, key: (i32, i32), sections: std::ops::Range<usize>, state: u32) -> usize {
    let at = |x: i32, y: i32, z: i32| {
        let column = world.get(&(key.0 + x.div_euclid(16), key.1 + z.div_euclid(16)))?;
        column.block(x.rem_euclid(16) as usize, usize::try_from(y).ok()?, z.rem_euclid(16) as usize)
    };
    let neighbors = [(0, -1, 0), (0, 1, 0), (0, 0, -1), (0, 0, 1), (-1, 0, 0), (1, 0, 0)];
    let buried = |x: i32, y: i32, z: i32| {
        at(x, y, z) == Some(state) && neighbors.iter().all(|(dx, dy, dz)| at(x + dx, y + dy, z + dz) == Some(state))
    };
    let ys = sections.start as i32 * 16..sections.end as i32 * 16;
    ys.map(|y| (0..16).map(|z| (0..16).filter(|&x| buried(x, y, z)).count()).sum::<usize>()).sum()
}

/// Fluid shortcuts (`FluidPaths`: the two layers around a block read once, a block among its own
/// state passed over, the light of the block's level read once), in every combination: same mesh
/// as the fluid mesher called for every block with one world read and one light read per use, on
/// every column of the capture, then on two sections rewritten with random fluids (every level,
/// falling columns, lava, blocks hiding faces in part) and with a body of water pierced by such
/// states; both cross the borders of the column and of the sections, and are meshed with the
/// light read through the section cache and without it. The reference version agrees and the
/// fingerprint of the first scene is stored.
#[test]
fn fluid_shortcuts_match_world_reads() {
    let Some((d, cols, t, palette)) = common::load() else { return };
    let job_of = |world: &Cols, (x, z): (i32, i32), shape: bool, cached_light: bool, fluid: FluidPaths| {
        let mut j = job_with(world, x, z, CullPaths { shape, ..CullPaths::DEFAULT });
        (j.fluid, j.light.fluid) = (fluid, cached_light);
        j
    };
    let quads = |m: &BlockMesh| m.fluids.iter().map(Vec::len).sum::<usize>();
    let keys: Vec<(i32, i32)> = cols.keys().copied().collect();
    for shape in [false, true] {
        let (differing, fluid_quads) = keys
            .par_iter()
            .map(|&key| {
                let reads = mesh(&job_of(&cols, key, shape, true, FluidPaths::OLD), &t, &palette, MIN_Y);
                let same = |paths| {
                    let m = mesh(&job_of(&cols, key, shape, true, paths), &t, &palette, MIN_Y);
                    common::same(&m, &reads) && digest(&m) == digest(&reads)
                };
                (usize::from(!fluid_shortcut_combinations().into_iter().all(same)), quads(&reads))
            })
            .reduce(|| (0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
        assert_eq!(differing, 0, "rule {shape}: columns of the capture meshed differently with a fluid shortcut");
        assert!(fluid_quads > 1000, "rule {shape}: {fluid_quads} fluid faces in the capture, nothing proven");
    }

    let key = common::surrounded_column(&cols);
    let top = cols[&key].sections.len();
    let rewritten = top - 4..top - 2;
    let water = state(&d, "minecraft:water", &[("level", "0")]);
    // Odd seeds: the generator sets the lowest bit.
    let mut scenes: Vec<(String, Cols)> = [1u64, 3, 5, 7]
        .into_iter()
        .map(|seed| {
            let random = &mut common::random_fluid_states(&d, seed);
            let world = common::with_sections_rewritten(&d, &cols, key, rewritten.clone(), random);
            (format!("random fluids {seed}"), world)
        })
        .collect();
    for seed in [9u64, 11] {
        // Water sources, every 13th cell a random state: buried blocks and pierced ones side by side.
        let (mut random, mut cell) = (common::random_fluid_states(&d, seed), 0u32);
        let pierced = &mut || {
            cell += 1;
            let other = random();
            if cell.is_multiple_of(13) { other } else { water }
        };
        let world = common::with_sections_rewritten(&d, &cols, key, rewritten.clone(), pierced);
        let buried = blocks_buried_in_own_state(&world, key, rewritten.clone(), water);
        assert!(buried > 1000, "water body {seed}: {buried} buried water blocks, skip not exercised");
        scenes.push((format!("water body {seed}"), world));
    }
    let mut listing = String::new();
    for (name, scene) in &scenes {
        for shape in [false, true] {
            let j = job_of(scene, key, shape, true, FluidPaths::OLD);
            let reads = mesh(&j, &t, &palette, MIN_Y);
            let reference = digest(&mesh_reference(&j, &t, &palette, MIN_Y));
            assert_eq!(digest(&reads), reference, "{name}, rule {shape}: world reads against the reference");
            for cached_light in [true, false] {
                for paths in fluid_shortcut_combinations() {
                    let m = mesh(&job_of(scene, key, shape, cached_light, paths), &t, &palette, MIN_Y);
                    let case = format!("{name}, rule {shape}, cached light {cached_light}, {paths:?}");
                    assert!(common::same(&m, &reads), "{case}: the shortcuts change the mesh");
                    assert_eq!(digest(&m), reference, "{case}: against the reference");
                }
            }
            assert!(quads(&reads) > 2000, "{name}: {} fluid faces, scene too poor", quads(&reads));
            if name == "random fluids 1" {
                listing += &format!("0x{reference:016x}, ");
            }
            eprintln!("{name}, rule {shape}: {} fluid faces, fingerprint 0x{reference:016x}", quads(&reads));
        }
    }
    let stored = RANDOM_FLUIDS.map(|v| format!("0x{v:016x}, ")).concat();
    assert_eq!(listing, stored, "fingerprints of the random fluid scene (seed 1)");
}

/// Air, leaf and sealed states read in the property words (`RUSTCRAFT_OPT_MESHTABLEBITS`) or in
/// the byte tables kept by the old layout: same answer for every state and beyond the table, equal
/// to the game data; the byte tables are gone from the new layout; same meshes on the paths that
/// read the flags per state (old loop, opaque leaves) and on the default one.
#[test]
fn state_flags_read_from_property_words() {
    use rc_mesh::cull::AIR_BIT;
    let Some((d, cols, t, palette)) = common::load() else { return };
    let words = Tables::with_flag_tables(&d, t.models.clone(), false);
    let bytes = Tables::with_flag_tables(&d, t.models.clone(), true);
    let states = d.states.len() as u32;
    let (mut leaves, mut sealed) = (0, 0);
    for s in 0..states + 16 {
        let known = d.states.get(s as usize);
        let air = known.is_none_or(|state| state.has(rc_data::flags::AIR));
        let leaf = known.is_some_and(|state| d.blocks[state.block as usize].name.ends_with("leaves"));
        for (layout, tables) in [("words", &words), ("bytes", &bytes)] {
            assert_eq!(tables.is_air(s), air, "{layout}: air, state {s}");
            assert_eq!(tables.is_leaves(s), leaf, "{layout}: leaves, state {s}");
        }
        assert_eq!(words.is_sealed(s), bytes.is_sealed(s), "sealed, state {s}");
        // A sealed state is a full covering cube that is neither air nor leaves.
        assert!(!words.is_sealed(s) || (t.covers[s as usize] == 0x3f && !air && !leaf), "sealed state {s}");
        leaves += usize::from(leaf);
        sealed += usize::from(words.is_sealed(s));
    }
    assert!(leaves > 50 && sealed > 500, "{leaves} leaf states, {sealed} sealed states: nothing proven");
    // Three bytes per state less on the heap.
    assert_eq!(words.flag_tables_heap_bytes(), 0);
    assert!(bytes.flag_tables_heap_bytes() >= 3 * states as usize);
    eprintln!("{states} states: byte tables {} B, none in the new layout", bytes.flag_tables_heap_bytes());

    // The new layout really reads the words: a falsified word changes its answer, not the old one's.
    let stone = state(&d, "minecraft:stone", &[]);
    let (mut falsified_words, mut falsified_bytes) =
        (Tables::with_flag_tables(&d, t.models.clone(), false), Tables::with_flag_tables(&d, t.models.clone(), true));
    for tables in [&mut falsified_words, &mut falsified_bytes] {
        tables.cull.props[0][stone as usize] |= AIR_BIT;
    }
    assert!(falsified_words.is_air(stone) && !falsified_bytes.is_air(stone));

    // Same meshes: old loop (flags read per state) and default loop, leaves cutout and opaque.
    let mut keys: Vec<(i32, i32)> = cols.keys().copied().collect();
    keys.sort_unstable();
    for cull in [CullPaths::OLD, CullPaths::DEFAULT] {
        for cutout_leaves in [true, false] {
            let fingerprint = |tables: &Tables| {
                let set: Set = &|j| j.opts.cutout_leaves = cutout_leaves;
                total(mesh_all(&keys, &cols, tables, &palette, cull, set).iter())
            };
            let (with_words, with_bytes) = (fingerprint(&words), fingerprint(&bytes));
            assert_eq!(with_words, with_bytes, "{cull:?}, cutout leaves {cutout_leaves}");
            // The falsified word is seen by the old loop (stone skipped as air).
            if cull == CullPaths::OLD && cutout_leaves {
                assert_ne!(fingerprint(&falsified_words), with_words, "falsified air word not seen by the meshing");
            }
        }
    }
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
