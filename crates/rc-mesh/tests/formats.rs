//! Equivalence proof of the compact mesh formats (`MeshOptions::offset_faces`,
//! `MeshOptions::water_double`) on all the columns of the reference capture: each
//! column is meshed in the old format (explicit quads, back face as a second quad) and
//! in the new one, then the new one is unfolded as the shader does and compared bit for bit.
//!
//! - Compact offset faces: corners rebuilt by the shader's formula
//!   ((block + offset) + baked corner, in f32) bit for bit equal to the corners of the earlier
//!   explicit quads, colour and light per corner equal, same section and same layer.
//! - Double face: the marked quad unfolded into (quad, back face) gives back the earlier sequence.
//! - The rest (faces, fluids, starts, visibility) is unchanged.
//!
//! Without data the test is ignored; under `RUSTCRAFT_OPT_REQUIRE=1` it is a failure.

mod common;

use common::{Cols, MIN_Y, job};
use rayon::prelude::*;
use rc_mesh::blocks::{BUCKETS, BlockMesh, Face, Tables, mesh, offset_face_quad};
use rc_mesh::fluid::{DOUBLE_FACE, FluidQuad, back_face};

fn bits(q: &FluidQuad) -> Vec<u32> {
    let mut v: Vec<u32> = q.corners.iter().flatten().map(|c| c.to_bits()).collect();
    v.push(q.texture);
    v.extend(q.colors);
    v.extend(q.light.map(u32::from));
    v
}

/// Faces of layer `l`, section `s`, group by group.
fn section_faces(m: &BlockMesh, l: usize, s: usize) -> Vec<Face> {
    let s1 = m.vis.len() + 1;
    (0..BUCKETS)
        .flat_map(|g| {
            let (a, e) = (m.starts[l][g * s1 + s] as usize, m.starts[l][g * s1 + s + 1] as usize);
            m.layers[l][a..e].iter().copied()
        })
        .collect()
}

fn section_quads(m: &BlockMesh, l: usize, s: usize) -> &[FluidQuad] {
    &m.fluids[l][m.fluid_starts[l][s] as usize..m.fluid_starts[l][s + 1] as usize]
}

/// Count (offset faces, double-face quads); panics at the first difference.
fn check(old: &BlockMesh, new: &BlockMesh, t: &Tables, table: &[f32], shade: &[f32; 6]) -> (usize, usize) {
    assert_eq!(old.vis, new.vis);
    let (mut offsets, mut doubles) = (0, 0);
    let at = (old.x, old.z);
    for l in 0..3 {
        assert_eq!(old.starts[l].len(), new.starts[l].len());
        for s in 0..old.vis.len() {
            // Faces: the compact ones removed, the order of the others kept.
            let (compact, plain): (Vec<Face>, Vec<Face>) =
                section_faces(new, l, s).into_iter().partition(|f| f[0] >> 28 != 0);
            assert_eq!(plain, section_faces(old, l, s), "{at:?} layer {l} section {s}: faces");
            assert!(l != 2 || compact.is_empty(), "compact offset face in the translucent layer");
            let mut rebuilt: Vec<Vec<u32>> =
                compact.iter().map(|f| bits(&offset_face_quad(t, table, f, shade).expect("offset"))).collect();
            offsets += rebuilt.len();
            // Quads: doubles unfolded, then the earlier sequence minus the rebuilt offset ones.
            let mut expanded = Vec::new();
            for q in section_quads(new, l, s) {
                if q.texture & DOUBLE_FACE != 0 {
                    let front = FluidQuad { texture: q.texture & !DOUBLE_FACE, ..*q };
                    expanded.extend([front, back_face(&front)]);
                    doubles += 1;
                } else {
                    expanded.push(*q);
                }
            }
            let mut rest = Vec::new();
            let mut it = expanded.iter().peekable();
            for q in section_quads(old, l, s) {
                if it.peek().is_some_and(|n| *n == q) {
                    it.next();
                } else {
                    rest.push(bits(q));
                }
            }
            assert!(it.next().is_none(), "{at:?} layer {l} section {s}: new quads absent from before");
            rest.sort_unstable();
            rebuilt.sort_unstable();
            assert_eq!(rest, rebuilt, "{at:?} layer {l} section {s}: rebuilt offset ones differ");
        }
    }
    (offsets, doubles)
}

fn mesh_old(cols: &Cols, t: &Tables, p: &rc_mesh::blocks::BiomePalette, x: i32, z: i32) -> BlockMesh {
    let mut j = job(cols, x, z);
    j.opts.offset_faces = false;
    j.opts.water_double = false;
    mesh(&j, t, p, MIN_Y)
}

#[test]
fn compact_formats_match_explicit() {
    let Some((_d, cols, t, palette)) = common::load() else { return };
    assert!(t.offsets_all_compact(), "offset model without a possible compact face");
    let table = t.offset_table();
    let shade = job(&cols, 0, 0).shade;
    let mut keys: Vec<(i32, i32)> = cols.keys().copied().collect();
    keys.sort_unstable();
    let (offsets, doubles, bytes_old, bytes_new) = keys
        .par_iter()
        .map(|&(x, z)| {
            let old = mesh_old(&cols, &t, &palette, x, z);
            let new = mesh(&job(&cols, x, z), &t, &palette, MIN_Y);
            let (o, d) = check(&old, &new, &t, &table, &shade);
            let size = |m: &BlockMesh| m.layers.iter().map(Vec::len).sum::<usize>() * 24 + m.fluids.iter().map(Vec::len).sum::<usize>() * 128;
            (o, d, size(&old), size(&new))
        })
        .reduce(|| (0, 0, 0, 0), |a, b| (a.0 + b.0, a.1 + b.1, a.2 + b.2, a.3 + b.3));
    eprintln!(
        "{} columns: {offsets} compact offset faces, {doubles} double-face quads; GPU bytes {bytes_old} -> {bytes_new}",
        keys.len()
    );
    assert!(offsets > 1000 && doubles > 100, "bench without offsets or water: nothing proven");

    // Sabotages: the order of the additions reversed, or one ulp in the table, are seen.
    let col = keys.iter().copied().find(|&(x, z)| {
        let m = mesh(&job(&cols, x, z), &t, &palette, MIN_Y);
        m.layers.iter().flatten().any(|f| f[0] >> 28 != 0)
    });
    let (x, z) = col.expect("column with offsets");
    let (old, new) = (mesh_old(&cols, &t, &palette, x, z), mesh(&job(&cols, x, z), &t, &palette, MIN_Y));
    let bad: Vec<f32> = table.iter().map(|v| v + 1.0 / 256.0).collect();
    let sabotaged = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(&old, &new, &t, &bad, &shade)));
    assert!(sabotaged.is_err(), "falsified table not seen");
    // Reversed order: block + (offset + corner) instead of (block + offset) + corner.
    let reordered = new.layers.iter().flatten().filter(|f| f[0] >> 28 != 0).any(|f| {
        let q = offset_face_quad(&t, &table, f, &shade).expect("offset");
        let baked = &t.models.quads[(f[1] & rc_mesh::blocks::QUAD_MASK) as usize];
        let n = [(f[1] >> 20) & 15, (f[1] >> 24) & 15, (f[1] >> 28) & 15];
        let c = (f[0] >> 28) as usize - 1;
        let o: [f32; 3] = std::array::from_fn(|a| table[(c * 3 + a) * 16 + n[a] as usize]);
        let b = [(f[0] & 15) as f32, ((f[0] >> 8) & 4095) as f32, ((f[0] >> 4) & 15) as f32];
        (0..4).any(|i| (0..3).any(|a| (b[a] + (o[a] + baked.pos[i][a])).to_bits() != q.corners[i][a].to_bits()))
    });
    assert!(reordered, "the order of the additions changes no corner: test insensitive to the order");
}
