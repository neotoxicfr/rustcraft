//! Proofs on the preparation of the bytes of a column and on the decision to rewrite a column in place.

use super::super::pipelines::FluidGpu;
use super::super::test_meshes::{flat, random_mesh};
use super::super::{GpuFormats, OLD_WORDS, TRANS_WORDS, gpu_formats};
use super::*;
use rc_mesh::fluid::FluidQuad;

/// Old format of a quad (128 bytes), reference of the format tests.
fn fluid_gpu(q: &rc_mesh::fluid::FluidQuad) -> FluidGpu {
    let c = &q.corners;
    let l = q.light.map(u32::from);
    FluidGpu {
        p: [0, 1, 2, 3].map(|i| [c[i][0], c[i][1], c[i][2], c[i][3]]),
        v: [c[0][4], c[1][4], c[2][4], c[3][4]],
        meta: [q.texture, 0, 0, 0],
        colors: q.colors,
        light: [l[0] | l[1] << 16, l[2] | l[3] << 16, 0, 0],
    }
}

const OLD: GpuFormats = GpuFormats { fluid96: false, trans_compact: false };
const FORMATS: [GpuFormats; 4] = [
    OLD,
    GpuFormats { fluid96: true, trans_compact: false },
    GpuFormats { fluid96: false, trans_compact: true },
    GpuFormats { fluid96: true, trans_compact: true },
];

/// Words of a quad in the compact format read back like the shaders (`vs_fluid_c`, `vs_sorted_c`):
/// same corners, v, texture (double face in bit 31), colours and lights as the old
/// format (`FluidGpu`), column included.
fn decode(w: &[u32], f: GpuFormats, trans: bool, column: [u32; 2], slots: &[[u32; 2]]) -> FluidGpu {
    let p = std::array::from_fn(|c| std::array::from_fn(|a| f32::from_bits(w[c * 4 + a])));
    let v = std::array::from_fn(|c| f32::from_bits(w[16 + c]));
    match (trans, f.trans_compact, f.fluid96) {
        (true, true, _) => {
            let col = slots[(w[20] >> 16) as usize];
            let info = (w[20] & 0x7fff) | (w[20] & 0x8000) << 16;
            FluidGpu {
                p,
                v,
                meta: [info, 0, col[0], col[1]],
                colors: [w[21], w[22], w[23], w[24]],
                light: [w[25], w[26], 0, 0],
            }
        }
        (false, _, true) => FluidGpu { p, v, meta: [w[20], 0, 0, 0], colors: [w[21]; 4], light: [w[22], w[23], 0, 0] },
        _ => {
            let g: FluidGpu = bytemuck::pod_read_unaligned(bytemuck::cast_slice(&w[..OLD_WORDS]));
            assert_eq!(&g.meta[2..], if trans { &column[..] } else { &[0, 0] });
            g
        }
    }
}

/// Compact formats read back like the shaders: equal to the old format bit for bit (single-colour
/// quads for the fluids, arbitrary and double-face for the translucents), and the
/// decoding sees a sabotage (texture bit, slot).
#[test]
fn compact_formats_decode_to_old_format() {
    let mut seed = 0x0dd_ba11_cafe_f00du64;
    let mut seen = 0;
    for _ in 0..300 {
        let mut m = random_mesh(&mut seed, 1);
        for (l, q) in m.fluids.iter_mut().enumerate().flat_map(|(l, v)| v.iter_mut().map(move |q| (l, q))) {
            if l < 2 {
                q.colors = [q.colors[0]; 4];
            }
            if q.light[0] & 1 == 1 {
                q.texture |= rc_mesh::fluid::DOUBLE_FACE;
            }
        }
        let old = prepare(&m, false, OLD);
        let col = [m.x as u32, m.z as u32];
        // Old format: bytes of the old construction (`fluid_gpu`) unchanged.
        let mut want: Vec<FluidGpu> = Vec::new();
        for q in m.fluids[0].iter().chain(&m.fluids[1]) {
            let mut g = fluid_gpu(q);
            g.meta[0] |= local_of(m.x, m.z) << 16;
            want.push(g);
        }
        assert_eq!(bytemuck::cast_slice::<FluidGpu, u32>(&want), &old.fluids[..]);
        let want: Vec<FluidGpu> =
            m.fluids[2].iter().map(|q| FluidGpu { meta: [q.texture, 0, col[0], col[1]], ..fluid_gpu(q) }).collect();
        assert_eq!(bytemuck::cast_slice::<FluidGpu, u32>(&want), &old.trans[..]);
        for f in FORMATS {
            let mut p = prepare(&m, false, f);
            let slot = (seed % 65_536) as u16;
            p.set_slot(slot);
            let mut slots = vec![[0u32; 2]; 65_536];
            slots[usize::from(slot)] = col;
            let local = local_of(m.x, m.z);
            for (trans, words, n, mine, theirs) in [
                (false, f.fluid_words(), p.fluid_len(), &p.fluids, &old.fluids),
                (true, f.trans_words(), p.trans_len(), &p.trans, &old.trans),
            ] {
                for i in 0..n {
                    let got = decode(&mine[i * words..(i + 1) * words], f, trans, col, &slots);
                    let want = decode(&theirs[i * OLD_WORDS..(i + 1) * OLD_WORDS], OLD, trans, col, &slots);
                    assert_eq!(
                        bytemuck::bytes_of(&got),
                        bytemuck::bytes_of(&want),
                        "{f:?} translucent {trans} quad {i}"
                    );
                    if !trans {
                        assert_eq!(got.meta[0] >> 16 & 0xff, local);
                    }
                    seen += 1;
                }
            }
        }
    }
    assert!(seen > 10_000, "draw too poor: {seen}");
    // Sabotage: lost double-face bit, or wrong slot, are seen.
    let q = FluidQuad { texture: 5 | rc_mesh::fluid::DOUBLE_FACE, ..flat(0.0, 0.0, 1.0, true) };
    let f = GpuFormats { fluid96: false, trans_compact: true };
    let mut w = Vec::new();
    push_quad(&mut w, &q, TRANS_WORDS, q.texture, [0, 0]);
    let slots = vec![[7u32, 9u32]; 2];
    assert_eq!(decode(&w, f, true, [7, 9], &slots).meta[0], q.texture);
    w[20] &= !0x8000;
    assert_ne!(decode(&w, f, true, [7, 9], &slots).meta[0], q.texture);
}

/// Bytes prepared on the pool identical to those of the old upload (faces with the column
/// in the region, fluids, translucents, centres, free order), with or without the shortcut
/// of the free columns; and the shortcut only changes the centres.
#[test]
fn pool_prepared_bytes_match_legacy_upload() {
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let (mut free, mut faces) = (0, 0);
    for _ in 0..500 {
        let m = random_mesh(&mut seed, 1);
        for transfree in [false, true] {
            for f in FORMATS {
                let (a, b) = (prepare(&m, transfree, f), prepare_legacy(&m, transfree, f));
                assert!(a.same(&b), "({}, {}) transfree {transfree} {f:?}: different bytes", m.x, m.z);
            }
        }
        let (full, short) = (prepare_legacy(&m, false, OLD), prepare_legacy(&m, true, OLD));
        assert_eq!(full.free, short.free);
        assert_eq!(full.centers.len(), m.fluids[2].len());
        assert_eq!(short.centers.len(), if short.free { 0 } else { m.fluids[2].len() });
        free += usize::from(full.free && !m.fluids[2].is_empty());
        faces += full.face_len();
        // Column in the region placed on each face (the drawn faces fit in 20 bits).
        let local = local_of(m.x, m.z);
        assert!(full.faces.parts().into_iter().flatten().all(|f| f[0] >> 20 == local));
    }
    eprintln!("500 meshes, {faces} faces, {free} free translucent columns");
    assert!(free > 50 && faces > 100_000, "draw too poor");
}

/// Copy of a mesh (the test meshes are not `Clone`).
fn copy_of(m: &BlockMesh) -> BlockMesh {
    BlockMesh {
        x: m.x,
        z: m.z,
        version: m.version,
        layers: m.layers.clone(),
        fluids: m.fluids.clone(),
        starts: m.starts.clone(),
        fluid_starts: m.fluid_starts.clone(),
        vis: m.vis.clone(),
    }
}

/// Faces moved out of the mesh into the GPU bytes (`RUSTCRAFT_OPT_MESH_PREPARE_BY_MOVE`): the same
/// bytes as the copy and as the old upload, one part per layer, the mesh left without faces.
/// Sabotage: a face left untagged is seen.
#[test]
fn moved_faces_match_the_copy() {
    let mut seed = 0x0bad_5eed_1234_5678u64;
    let mut faces = 0;
    for _ in 0..300 {
        let m = random_mesh(&mut seed, 1);
        for transfree in [false, true] {
            for f in FORMATS {
                let mut moved_from = copy_of(&m);
                let moved = prepare_moving_faces(&mut moved_from, transfree, f);
                assert!(moved.same(&prepare(&m, transfree, f)), "({}, {}) {f:?}: moved bytes differ", m.x, m.z);
                assert!(moved.same(&prepare_legacy(&m, transfree, f)));
                assert!(moved_from.layers.iter().all(Vec::is_empty), "faces left in the mesh");
                assert_eq!(moved.faces.parts().map(<[_]>::len), m.layers.each_ref().map(Vec::len));
                faces += moved.face_len();
                // Faces read from the base flattened with the column's tag (`MESH_BASE_IS_PREPARED`).
                let base: rc_mesh::blocks::Packed = Arc::new(rc_mesh::blocks::pack_exact_tagged(&m, region_tag(m.x, m.z)));
                let shared = prepare_from_base(&m, base, transfree, f).expect("flat base of the mesh");
                assert!(shared.same(&moved), "({}, {}) {f:?}: faces read from the base differ", m.x, m.z);
                assert!(matches!(shared.faces, PreparedFaces::InBase(..)));
                // Sabotage: a base flattened without the tag gives other bytes.
                let plain: rc_mesh::blocks::Packed = Arc::new(rc_mesh::blocks::pack_exact(&m));
                let untagged = prepare_from_base(&m, plain, transfree, f).expect("flat base of the mesh");
                if local_of(m.x, m.z) != 0 && moved.face_len() > 0 {
                    assert!(!untagged.same(&moved), "untagged base not seen");
                }
            }
        }
    }
    assert!(faces > 100_000, "draw too poor: {faces}");
    let m = random_mesh(&mut seed, 1);
    // A base of other counts is refused.
    let mut other = copy_of(&m);
    other.layers[0].push([0; 6]);
    assert!(prepare_from_base(&m, Arc::new(rc_mesh::blocks::pack_exact(&other)), false, OLD).is_none());
    let mut moved = prepare_moving_faces(&mut copy_of(&m), false, OLD);
    let PreparedFaces::Owned(parts) = &mut moved.faces else { panic!("faces not owned") };
    let Some(f) = parts.iter_mut().flatten().next() else { panic!("no face drawn") };
    f[0] &= !(0xff << 20);
    if local_of(m.x, m.z) != 0 {
        assert!(!moved.same(&prepare(&m, false, OLD)), "untagged face not seen");
    }
}

/// Pieces of a byte range over faces in parts: end to end they are the bytes of the range, each at
/// its offset, one per part touched.
#[test]
fn face_byte_pieces_cover_the_range() {
    let mut rng = 0x2545_f491_4f6c_dd1du64;
    let mut next = move |n: u64| {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng % n.max(1)
    };
    let mut k = 0u32;
    for _ in 0..2000 {
        let parts: [Vec<rc_mesh::blocks::Face>; 3] = std::array::from_fn(|_| {
            (0..next(5)).map(|_| {
                k += 1;
                [k; 6]
            })
            .collect()
        });
        let all: Vec<u8> = parts.iter().flat_map(|p| bytemuck::cast_slice::<_, u8>(p).to_vec()).collect();
        let (a, b) = (next(all.len() as u64 + 1) as usize, next(all.len() as u64 + 1) as usize);
        let range = a.min(b)..a.max(b);
        let pieces: Vec<_> = face_byte_pieces(parts.each_ref().map(Vec::as_slice), range.clone()).collect();
        let mut at = 0;
        for (offset, piece) in &pieces {
            assert_eq!(*offset, at as u64, "pieces not contiguous");
            at += piece.len();
        }
        let joined: Vec<u8> = pieces.iter().flat_map(|(_, p)| p.to_vec()).collect();
        assert_eq!(joined, all[range], "pieces differ from the range");
        assert!(pieces.len() <= 3 && pieces.iter().all(|(_, p)| !p.is_empty()));
    }
}

/// In-place write as the upload applies it: base sent whole into simulated buffers (other
/// columns around), then only the writes decided by `in_place_writes`; the buffers must be
/// those of a full upload of the new mesh. Then the refusals (GPU version different from the
/// base, counts, no old place) and the rewrite of the order (kept only if the old and the new
/// are free).
#[test]
fn in_place_writes_match_full_upload() {
    let mut seed = 0x51f1_5eed_c0ff_ee11u64;
    let mut rng = 0x2545_f491_4f6c_dd1du64;
    let mut next = move |n: u64| {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng % n.max(1)
    };
    let (mut written, mut shifted, mut kept_order) = (0, 0, 0);
    for round in 0..500 {
        let m = random_mesh(&mut seed, 1);
        let mut n = BlockMesh { version: 2, ..copy_of(&m) };
        for _ in 0..next(4) {
            let l = next(3) as usize;
            if let Some(f) = n.layers[l].get_mut(next(400) as usize) {
                f[1] ^= 1 + next(1000) as u32;
            }
            if let Some(q) = n.fluids[l].get_mut(next(40) as usize) {
                q.colors[0] ^= 1 + next(1000) as u32;
            }
        }
        let f = FORMATS[next(4) as usize];
        // Faces copied or moved into the GPU bytes, one range per buffer or several (gaps of 0, 1 KiB, 64 KiB).
        let pb = prepare(&m, true, f);
        let pn = if round % 2 == 0 { prepare(&n, true, f) } else { prepare_moving_faces(&mut copy_of(&n), true, f) };
        let mut packed = Vec::new();
        rc_mesh::blocks::pack(&m, &mut packed);
        let gaps = [None, Some(0), Some(1024), Some(65_536)][round % 4].map(|b| in_place_gaps(b, f));
        let paths = rc_mesh::blocks::InPlacePaths { touched_only: false, gaps };
        let d = rc_mesh::blocks::in_place_diff_with(&packed, &n, None, paths).expect("same counts");
        let at = [1 + next(50) as u32, 1 + next(50) as u32, 1 + next(50) as u32, 1 + next(50) as u32];
        let len = [pb.face_len() as u32, pb.fluid_len() as u32, pb.trans_len() as u32];
        let slot = Slot {
            version: 1,
            faces: (at[0], len[0]),
            fluids: (at[1], len[1]),
            trans: (at[2], len[2]),
            order: (at[3], len[2]),
            free: pb.free || pb.trans.is_empty(),
        };
        let stride = [FACE_BYTES, f.fluid_bytes(), f.trans_bytes()];
        let bytes = |p: &Prepared| -> [Vec<u8>; 3] {
            [
                p.faces.parts().iter().flat_map(|l| bytemuck::cast_slice::<_, u8>(l).to_vec()).collect(),
                bytemuck::cast_slice(&p.fluids).to_vec(),
                bytemuck::cast_slice(&p.trans).to_vec(),
            ]
        };
        // Simulated buffers: pattern of the neighbours, then the column placed in its place.
        let full = |p: &Prepared| -> [Vec<u8>; 3] {
            let b = bytes(p);
            std::array::from_fn(|i| {
                let size = (at[i] + len[i] + 3) as usize * stride[i] as usize;
                let mut buf: Vec<u8> = (0..size).map(|j| (j * 7 + i) as u8).collect();
                let a = at[i] as usize * stride[i] as usize;
                buf[a..a + b[i].len()].copy_from_slice(&b[i]);
                buf
            })
        };
        let mut gpu = full(&pb);
        let k = in_place_writes(Some(slot), &pn, pn.free, Some((1, d.clone()))).expect("place kept");
        let src = bytes(&pn);
        for (t, off, r) in k.writes {
            let i = t as usize;
            let off = off as usize;
            written += 1;
            shifted += usize::from(off > at[i] as usize * stride[i] as usize);
            if t == Target::Faces {
                // Through the pieces the upload writes, part by part.
                for (o, piece) in face_byte_pieces(pn.faces.parts(), r) {
                    gpu[i][off + o as usize..off + o as usize + piece.len()].copy_from_slice(piece);
                }
            } else {
                gpu[i][off..off + r.len()].copy_from_slice(&src[i][r]);
            }
        }
        assert!(gpu == full(&pn), "({}, {}): buffers different from a full upload", m.x, m.z);
        assert_eq!(k.order, !pn.trans.is_empty() && !(pn.free && slot.free));
        kept_order += usize::from(!k.order && !pn.trans.is_empty());
        // Refusals: the GPU holds another version than the base, different counts, nothing sent.
        let other = Slot { version: 3, ..slot };
        assert!(in_place_writes(Some(other), &pn, pn.free, Some((1, d.clone()))).is_none());
        let grown = Slot { faces: (at[0], len[0] + 1), ..slot };
        assert!(in_place_writes(Some(grown), &pn, pn.free, Some((1, d.clone()))).is_none());
        let order = Slot { order: (at[3], len[2] + 1), ..slot };
        assert!(in_place_writes(Some(order), &pn, pn.free, Some((1, d.clone()))).is_none());
        assert!(in_place_writes(None, &pn, pn.free, Some((1, d.clone()))).is_none());
        assert!(in_place_writes(Some(slot), &pn, pn.free, None).is_none());
        // Order: rewritten as soon as the old or the new one is not free.
        if !pn.trans.is_empty() {
            for (was, now) in [(false, false), (false, true), (true, false), (true, true)] {
                let s = Slot { free: was, ..slot };
                let k = in_place_writes(Some(s), &pn, now, Some((1, d.clone()))).expect("place kept");
                assert_eq!(k.order, !(was && now), "free order {was} -> {now}");
            }
        }
    }
    eprintln!("500 remeshes: {written} writes, {shifted} shifted, {kept_order} orders kept");
    assert!(written > 300 && shifted > 200 && kept_order > 50, "draw too poor");
}

/// Cost of the preparation (old, on the pool, with and without the shortcut of the free
/// columns) and of the flattening of the base (copy, exact size), 1 core then all:
/// `cargo test --release -p rc-app prepare_cost -- --ignored --nocapture`.
#[test]
#[ignore]
fn prepare_cost() {
    use rayon::prelude::*;
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let meshes: Vec<BlockMesh> = (0..2000).map(|_| random_mesh(&mut seed, 20)).collect();
    let time = |what: &str, f: &(dyn Fn(&BlockMesh) -> usize + Sync)| {
        for threads in [1, 1, 4, rayon::current_num_threads()] {
            let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().expect("pool");
            let t = std::time::Instant::now();
            let n: usize = pool.install(|| meshes.par_iter().map(f).sum());
            let us = t.elapsed().as_secs_f64() * 1e6 / meshes.len() as f64;
            eprintln!("{what}: {us:.2} µs per column ({threads} threads, {n})");
        }
    };
    time("old preparation", &|m| prepare_legacy(m, false, gpu_formats()).bytes());
    time("old preparation + shortcut", &|m| prepare_legacy(m, true, gpu_formats()).bytes());
    time("pool preparation", &|m| prepare(m, false, gpu_formats()).bytes());
    time("pool preparation + shortcut", &|m| prepare(m, true, gpu_formats()).bytes());
    // Faces copied against moved (`RUSTCRAFT_OPT_MESH_PREPARE_BY_MOVE`): both start from a copy of
    // the mesh (the pool's own), whose cost is the line "mesh copy alone".
    time("mesh copy alone", &|m| std::hint::black_box(copy_of(m)).len());
    time("mesh copy + faces copied", &|m| prepare(&copy_of(m), true, gpu_formats()).bytes());
    time("mesh copy + faces moved", &|m| prepare_moving_faces(&mut copy_of(m), true, gpu_formats()).bytes());
    thread_local!(static SCRATCH: std::cell::RefCell<Vec<u32>> = const { std::cell::RefCell::new(Vec::new()) });
    time("flattening + copy (reused buffer)", &|m| {
        SCRATCH.with_borrow_mut(|w| {
            rc_mesh::blocks::pack(m, w);
            std::hint::black_box(w.clone()).len()
        })
    });
    time("exact flattening", &|m| std::hint::black_box(rc_mesh::blocks::pack_exact(m)).len());
}
