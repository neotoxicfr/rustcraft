//! Meshed columns on the GPU: the bytes of a column prepared for the GPU (`prepare`, on the pool or at
//! upload), the place a column holds in its region (`ColumnGpu`), the decision to rewrite a remeshed
//! column in place, and the upload, removal and clearing of columns.

use std::sync::Arc;
use std::time::Instant;

use rc_mesh::blocks::BlockMesh;

use super::regions::{RegionGpu, alloc_in, identity_buffer, region_bind};
use super::translucent_sort::{column_povs, eye_in_column, order_free, quad_center, sort_all, translucent_order};
use super::{
    FACE_BYTES, FACE_IDS_START, FLUID96_WORDS, GpuFormats, ORDER_BYTES, REGION, Renderer, TRANS_SLOTS, TRANS_WORDS,
};
use crate::perf_log::StageTimer;

/// Stages of a column upload in the perf log (µs per frame): lookup of the old version and
/// preparation of the bytes, release of the old place, places taken in the buffers (growths and
/// new regions included), GPU writes per target, translucent order, bookkeeping (places, viewpoints,
/// maps, slot table).
const UPLOAD_PREPARE: &str = "us_upload_prepare_per_frame";
const UPLOAD_RELEASE: &str = "us_upload_release_per_frame";
const UPLOAD_ALLOC: &str = "us_upload_alloc_per_frame";
const UPLOAD_WRITE_FACES: &str = "us_upload_write_faces_per_frame";
const UPLOAD_WRITE_FLUIDS: &str = "us_upload_write_fluids_per_frame";
const UPLOAD_WRITE_TRANSLUCENT: &str = "us_upload_write_translucent_per_frame";
const UPLOAD_WRITE_ORDER: &str = "us_upload_write_order_per_frame";
const UPLOAD_SORT: &str = "us_upload_sort_per_frame";
const UPLOAD_BOOKKEEPING: &str = "us_upload_bookkeeping_per_frame";

/// Perf log: one growth of a terrain buffer (copied into a bigger one), per buffer kind.
fn count_growth(name: &'static str, grew: bool) {
    if grew {
        crate::perf_log::counter_per_second(name, 1);
    }
}

pub(super) struct ColumnGpu {
    /// Fingerprint of the uploaded mesh (`RUSTCRAFT_MESHCHECK` only, otherwise 0).
    pub(super) digest: u64,
    pub(super) region: (i32, i32),
    /// Ranges (start, length) of the column's faces and fluids in its region.
    pub(super) face_alloc: (u32, u32),
    pub(super) fluid_alloc: (u32, u32),
    /// Range of the translucent quads in the global buffer.
    pub(super) trans_alloc: (u32, u32),
    /// Instance ranges per layer (opaque, cutout, translucent).
    pub(super) ranges: [std::ops::Range<u32>; 3],
    /// Fluid faces and their ranges per layer.
    pub(super) fluid_ranges: [std::ops::Range<u32>; 3],
    /// Start (absolute in the buffer) of the faces of each section, per layer, plus the end.
    pub(super) starts: [Vec<u32>; 3],
    pub(super) fluid_starts: [Vec<u32>; 3],
    /// Connectivity between the faces of each section (occlusion sorting).
    pub(super) vis: Arc<[rc_mesh::visibility::Vis]>,
    /// Translucent quads (glass, water…) kept CPU-side to re-sort them, start of each section,
    /// and camera block at the last sort.
    pub(super) trans_centers: Vec<[f32; 3]>,
    pub(super) trans_starts: Vec<u32>,
    /// Range (start, length) of the translucent order in the global buffer, and absolute start of
    /// each section in this buffer (one more entry for the end).
    pub(super) order_alloc: (u32, u32),
    pub(super) order_starts: Vec<u32>,
    pub(super) sorted_at: Option<[i32; 3]>,
    /// Viewpoint of each section at its last sort (`RUSTCRAFT_OPT_TRANSPOV`).
    pub(super) povs: Vec<u8>,
    pub(super) version: u64,
    /// Slot of the column in the compact translucents table.
    pub(super) trans_slot: Option<u16>,
}

/// Adds the words of an explicit quad in format `words`: `meta0` = texture | column in the
/// region << 16 (fluids); `col` = absolute column (old translucents).
fn push_quad(out: &mut Vec<u32>, q: &rc_mesh::fluid::FluidQuad, words: usize, meta0: u32, col: [u32; 2]) {
    let c = &q.corners;
    for p in c {
        out.extend([p[0], p[1], p[2], p[3]].map(f32::to_bits));
    }
    out.extend(c.iter().map(|p| p[4].to_bits()));
    let l = q.light.map(u32::from);
    let light = [l[0] | l[1] << 16, l[2] | l[3] << 16];
    match words {
        FLUID96_WORDS => {
            if q.colors.iter().any(|&k| k != q.colors[0]) {
                fluid96_mixed();
            }
            out.extend([meta0, q.colors[0], light[0], light[1]]);
        }
        TRANS_WORDS => {
            // Texture on 15 bits, double face in bit 15, slot set at upload (bits 16-31).
            out.push((q.texture & 0x7fff) | (q.texture >> 31) << 15);
            out.extend(q.colors);
            out.extend(light);
        }
        _ => {
            out.extend([meta0, 0, col[0], col[1]]);
            out.extend(q.colors);
            out.extend([light[0], light[1], 0, 0]);
        }
    }
}

/// Quad with different colours in a compact fluid buffer (resource pack beyond the compact
/// offset faces): colour of the first corner, reported once.
#[cold]
fn fluid96_mixed() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| eprintln!("[opt] FLUID96: 4-colour quad in a fluid buffer (colour of corner 0)"));
}

/// Bytes of a column ready for the GPU: faces (column in the region in bits 20-27),
/// fluids (bits 16-23), translucents (absolute column, or slot set at upload), centres
/// of the translucents and whether their order is free (`order_free`). Made on the pool
/// (`RUSTCRAFT_OPT_MESHPREP`) or at upload.
pub struct Prepared {
    pub(super) faces: PreparedFaces,
    counts: [u32; 3],
    /// Words of the fluids (layers 0 and 1) and translucents, in format `formats`.
    pub(super) fluids: Vec<u32>,
    fluid_counts: [u32; 3],
    pub(super) trans: Vec<u32>,
    formats: GpuFormats,
    /// Empty (without being computed) when the order is free and `RUSTCRAFT_OPT_TRANSFREE`.
    centers: Vec<[f32; 3]>,
    free: bool,
}

/// Faces of a column ready for the GPU (column in the region in bits 20-27), written one part after
/// the other.
pub(super) enum PreparedFaces {
    /// Copied here (all in the first part), or the mesh's three layers moved here
    /// (`prepare_moving_faces`).
    Owned([Vec<rc_mesh::blocks::Face>; 3]),
    /// The faces of the column's flat base, tagged there (`prepare_from_base`): word range of each
    /// layer in it.
    InBase(rc_mesh::blocks::Packed, [std::ops::Range<usize>; 3]),
}

impl PreparedFaces {
    pub(super) fn parts(&self) -> [&[rc_mesh::blocks::Face]; 3] {
        match self {
            PreparedFaces::Owned(parts) => parts.each_ref().map(Vec::as_slice),
            PreparedFaces::InBase(base, words) => words.clone().map(|r| base[r].as_chunks::<6>().0),
        }
    }
}

impl Prepared {
    pub fn bytes(&self) -> usize {
        (self.face_len() * FACE_BYTES as usize / 4 + self.fluids.len() + self.trans.len()) * 4
    }

    /// Number of faces, all parts together.
    pub(super) fn face_len(&self) -> usize {
        self.faces.parts().iter().map(|p| p.len()).sum()
    }

    pub(super) fn fluid_len(&self) -> usize {
        self.fluids.len() / self.formats.fluid_words()
    }

    pub(super) fn trans_len(&self) -> usize {
        self.trans.len() / self.formats.trans_words()
    }

    fn is_empty(&self) -> bool {
        self.face_len() == 0 && self.fluids.is_empty() && self.trans.is_empty()
    }

    /// Same bytes, same counts and same free order (centres compared bit by bit); the faces
    /// compared end to end, whatever their split into parts.
    fn same(&self, o: &Prepared) -> bool {
        fn b<T: bytemuck::NoUninit>(v: &[T]) -> &[u8] {
            bytemuck::cast_slice(v)
        }
        self.faces.parts().into_iter().flatten().eq(o.faces.parts().into_iter().flatten())
            && self.fluids == o.fluids
            && self.trans == o.trans
            && b(&self.centers) == b(&o.centers)
            && (self.counts, self.fluid_counts, self.free, self.formats)
                == (o.counts, o.fluid_counts, o.free, o.formats)
    }

    /// Check (`RUSTCRAFT_MESHCHECK`): bytes identical to those of the old upload.
    pub fn check(&self, m: &BlockMesh, key: (i32, i32)) {
        let old = prepare_legacy(m, crate::remesh::opts().transfree, self.formats);
        if !self.same(&old) {
            eprintln!("[meshcheck] ({}, {}): bytes prepared by the pool differ from the upload", key.0, key.1);
        }
    }

    /// Sets the column's slot in each compact translucent.
    pub(super) fn set_slot(&mut self, slot: u16) {
        if self.formats.trans_compact {
            for q in self.trans.as_chunks_mut::<TRANS_WORDS>().0 {
                q[20] = (q[20] & 0xffff) | u32::from(slot) << 16;
            }
        }
    }
}

/// Place of a column on the GPU (version and ranges start, length), what the in-place
/// write decision looks at; `free`: translucent order sent as is (identity).
#[derive(Clone, Copy, Debug)]
struct Slot {
    version: u64,
    faces: (u32, u32),
    fluids: (u32, u32),
    trans: (u32, u32),
    order: (u32, u32),
    free: bool,
}

impl ColumnGpu {
    fn slot(&self) -> Slot {
        Slot {
            version: self.version,
            faces: self.face_alloc,
            fluids: self.fluid_alloc,
            trans: self.trans_alloc,
            order: self.order_alloc,
            free: self.trans_centers.is_empty(),
        }
    }
}

/// Buffer targeted by an in-place write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    Faces,
    Fluids,
    Trans,
}

/// Partial remesh written into the place of its base: (buffer, byte offset, bytes of the
/// `Prepared` to copy there), and whether the translucent order must be rewritten.
struct Keep {
    slot: Slot,
    writes: Vec<(Target, u64, std::ops::Range<usize>)>,
    order: bool,
}

/// Decision of the in-place write, without GPU: the GPU must hold the exact base (`version`) and
/// the same numbers of faces, fluids, translucents and order; otherwise `None` (new place).
/// `free`: free order of the new mesh; the order is only kept if it already was.
fn in_place_writes(
    old: Option<Slot>,
    p: &Prepared,
    free: bool,
    in_place: Option<(u64, rc_mesh::blocks::InPlace)>,
) -> Option<Keep> {
    let (o, (base, d)) = (old?, in_place?);
    if o.version != base
        || o.faces.1 as usize != p.face_len()
        || o.fluids.1 as usize != p.fluid_len()
        || o.trans.1 as usize != p.trans_len()
        || o.order.1 as usize != p.trans_len()
    {
        return None;
    }
    let writes = [
        (Target::Faces, o.faces.0, FACE_BYTES, d.faces),
        (Target::Fluids, o.fluids.0, p.formats.fluid_bytes(), d.fluids),
        (Target::Trans, o.trans.0, p.formats.trans_bytes(), d.trans),
    ]
    .into_iter()
    .flat_map(|(t, at, stride, ranges)| {
        ranges.into_iter().map(move |s| {
            let bytes = s.start as usize * stride as usize..s.end as usize * stride as usize;
            (t, (u64::from(at) + u64::from(s.start)) * stride, bytes)
        })
    })
    .collect();
    Some(Keep { slot: o, writes, order: !(free && o.free) && p.trans_len() != 0 })
}

/// In-place gaps of `gap_bytes` bytes as numbers of elements (faces, fluid faces, translucent
/// faces) in the GPU formats `formats`.
pub fn in_place_gaps(gap_bytes: u64, formats: GpuFormats) -> [u32; 3] {
    [FACE_BYTES, formats.fluid_bytes(), formats.trans_bytes()].map(|stride| (gap_bytes / stride) as u32)
}

/// Column in its region (4 bits per axis).
fn local_of(x: i32, z: i32) -> u32 {
    ((x & (REGION - 1)) | ((z & (REGION - 1)) << 4)) as u32
}

/// Old preparation, done by the upload on the main thread (reference and `=0`).
fn prepare_legacy(m: &BlockMesh, transfree: bool, formats: GpuFormats) -> Prepared {
    let local = local_of(m.x, m.z);
    let mut all = Vec::with_capacity(m.len());
    let mut counts = [0u32; 3];
    for (i, l) in m.layers.iter().enumerate() {
        all.extend(l.iter().map(|f| [f[0] | local << 20, f[1], f[2], f[3], f[4], f[5]]));
        counts[i] = l.len() as u32;
    }
    let mut fluid_all: Vec<u32> = Vec::new();
    let mut fluid_counts = [0u32; 3];
    for (i, l) in m.fluids.iter().enumerate().take(2) {
        for q in l {
            push_quad(&mut fluid_all, q, formats.fluid_words(), q.texture | local << 16, [0, 0]);
        }
        fluid_counts[i] = l.len() as u32;
    }
    let mut trans_all: Vec<u32> = Vec::new();
    for q in &m.fluids[2] {
        push_quad(&mut trans_all, q, formats.trans_words(), q.texture, [m.x as u32, m.z as u32]);
    }
    let free = !sort_all() && order_free(&m.fluids[2], &m.fluid_starts[2]);
    let centers = if free && transfree {
        Vec::new()
    } else {
        trans_all
            .chunks_exact(formats.trans_words())
            .map(|w| {
                let p: [[f32; 3]; 4] = std::array::from_fn(|c| std::array::from_fn(|a| f32::from_bits(w[c * 4 + a])));
                quad_center(&p)
            })
            .collect()
    };
    let faces = PreparedFaces::Owned([all, Vec::new(), Vec::new()]);
    Prepared { faces, counts, fluids: fluid_all, fluid_counts, trans: trans_all, formats, centers, free }
}

/// Preparation on the pool: buffers of the exact size, centres taken from the quads without going through
/// the GPU format, no centre for a free column (`transfree`).
pub fn prepare(m: &BlockMesh, transfree: bool, formats: GpuFormats) -> Prepared {
    let local = local_of(m.x, m.z);
    let counts = m.layers.each_ref().map(|l| l.len() as u32);
    let mut faces = Vec::with_capacity(counts.iter().sum::<u32>() as usize);
    for l in &m.layers {
        faces.extend(l.iter().map(|f| [f[0] | local << 20, f[1], f[2], f[3], f[4], f[5]]));
    }
    prepare_with_faces(m, PreparedFaces::Owned([faces, Vec::new(), Vec::new()]), counts, transfree, formats)
}

/// [`prepare`] with the faces MOVED out of the mesh (`RUSTCRAFT_OPT_MESH_PREPARE_BY_MOVE`): each layer
/// becomes a part of the GPU bytes, tagged where it lies, without a copy of the faces; the mesh is
/// left without them.
pub fn prepare_moving_faces(m: &mut BlockMesh, transfree: bool, formats: GpuFormats) -> Prepared {
    let local = local_of(m.x, m.z);
    let mut faces = std::mem::take(&mut m.layers);
    faces.iter_mut().flatten().for_each(|f| f[0] |= local << 20);
    let counts = faces.each_ref().map(|l| l.len() as u32);
    prepare_with_faces(m, PreparedFaces::Owned(faces), counts, transfree, formats)
}

/// GPU tag of the column (`x`, `z`): its place in the region, in `rc_mesh::blocks::GPU_TAG_BITS`.
pub fn region_tag(x: i32, z: i32) -> u32 {
    local_of(x, z) << 20
}

/// [`prepare`] whose faces are those of the column's flat base `base`, made by
/// `rc_mesh::blocks::pack_tagged` with [`region_tag`] (`RUSTCRAFT_OPT_MESH_BASE_IS_PREPARED`): no
/// copy nor pass over the faces, the base and the upload share them. `None` if `base` is not a
/// flat mesh of `m`'s faces.
pub fn prepare_from_base(
    m: &BlockMesh,
    base: rc_mesh::blocks::Packed,
    transfree: bool,
    formats: GpuFormats,
) -> Option<Prepared> {
    let words = rc_mesh::blocks::packed_face_words(&base)?;
    let counts = words.clone().map(|r| (r.len() / 6) as u32);
    (counts == m.layers.each_ref().map(|l| l.len() as u32))
        .then(|| prepare_with_faces(m, PreparedFaces::InBase(base, words), counts, transfree, formats))
}

/// The rest of [`prepare`] around faces already prepared (`counts` per layer): fluids, translucents,
/// centres and free order.
fn prepare_with_faces(m: &BlockMesh, faces: PreparedFaces, counts: [u32; 3], transfree: bool, formats: GpuFormats) -> Prepared {
    let local = local_of(m.x, m.z);
    let fluid_counts = [m.fluids[0].len() as u32, m.fluids[1].len() as u32, 0];
    let mut fluids = Vec::with_capacity((fluid_counts[0] + fluid_counts[1]) as usize * formats.fluid_words());
    for l in &m.fluids[..2] {
        for q in l {
            push_quad(&mut fluids, q, formats.fluid_words(), q.texture | local << 16, [0, 0]);
        }
    }
    let tq = &m.fluids[2];
    let mut trans = Vec::with_capacity(tq.len() * formats.trans_words());
    for q in tq {
        push_quad(&mut trans, q, formats.trans_words(), q.texture, [m.x as u32, m.z as u32]);
    }
    let free = !sort_all() && order_free(tq, &m.fluid_starts[2]);
    let centers = if free && transfree {
        Vec::new()
    } else {
        tq.iter().map(|q| std::array::from_fn(|a| (q.corners[0][a] + q.corners[2][a]) * 0.5)).collect()
    };
    Prepared { faces, counts, fluids, fluid_counts, trans, formats, centers, free }
}

/// The bytes `bytes` of faces stored in parts put end to end: each piece with its offset from the
/// start of `bytes`, one per part it touches.
fn face_byte_pieces(
    parts: [&[rc_mesh::blocks::Face]; 3],
    bytes: std::ops::Range<usize>,
) -> impl Iterator<Item = (u64, &[u8])> {
    let mut part_start = 0;
    parts.into_iter().filter_map(move |part| {
        let b: &[u8] = bytemuck::cast_slice(part);
        let (start, end) = (bytes.start.max(part_start), bytes.end.min(part_start + b.len()));
        let piece = (start < end).then(|| ((start - bytes.start) as u64, &b[start - part_start..end - part_start]));
        part_start += b.len();
        piece
    })
}

/// Writes the bytes `bytes` of the faces at `at` in `target`. The pieces are contiguous in the
/// target: the uploader records them as one GPU copy.
fn write_faces(
    up: &mut super::Uploader,
    queue: &wgpu::Queue,
    target: &wgpu::Buffer,
    at: u64,
    faces: &PreparedFaces,
    bytes: std::ops::Range<usize>,
) {
    for (offset, piece) in face_byte_pieces(faces.parts(), bytes) {
        up.write(queue, target, at + offset, piece);
    }
}

impl Renderer {
    /// Uploads the mesh of a column. `prep`: bytes already prepared by the pool (otherwise made
    /// here); `in_place`: partial remesh of the same size as its base (version, changed
    /// ranges), written into the current place if the GPU still holds that base.
    pub fn upload(&mut self, m: &BlockMesh, prep: Option<Prepared>, in_place: Option<(u64, rc_mesh::blocks::InPlace)>) {
        if self.columns.get(&(m.x, m.z)).is_some_and(|c| c.version > m.version) {
            return;
        }
        let mut clock = StageTimer::start();
        let empty = prep.as_ref().map_or_else(|| m.is_empty(), Prepared::is_empty);
        let old = self.columns.remove(&(m.x, m.z));
        let old_empty = self.empty_vis.remove(&(m.x, m.z));
        // Visibility walk to redo only if the column appears or if the connectivity
        // of one of its sections changes (empty column included: the walk crosses it).
        let old_vis = old.as_ref().map(|o| &o.vis).or(old_empty.as_ref());
        if old_vis.is_none_or(|v| **v != *m.vis) {
            self.vis_rev += 1;
        }
        let formats = self.formats;
        let prepared = if empty {
            None
        } else {
            Some(prep.unwrap_or_else(|| prepare_legacy(m, crate::remesh::opts().transfree, formats)))
        };
        let sort_all = self.sort_all;
        let base_version = in_place.as_ref().map(|(v, _)| *v);
        let in_place_plan = prepared.as_ref().and_then(|prepared| {
            in_place_writes(old.as_ref().map(ColumnGpu::slot), prepared, !sort_all && prepared.free, in_place)
        });
        if let Some(base) = base_version
            && in_place_plan.is_none()
        {
            crate::perf_log::in_place_refused(old.as_ref().is_some_and(|o| o.version != base));
        }
        let old_slot = old.as_ref().and_then(|o| o.trans_slot);
        clock.lap(UPLOAD_PREPARE);
        if let Some(old) = old
            && in_place_plan.is_none()
        {
            self.release(&old);
        }
        clock.lap(UPLOAD_RELEASE);
        self.fade_ring.birth((m.x, m.z), Instant::now());
        self.columns_rev += 1;
        let Some(mut prepared) = prepared else {
            self.empty_vis.insert((m.x, m.z), m.vis.as_slice().into());
            clock.lap(UPLOAD_BOOKKEEPING);
            return;
        };
        // Compact translucents: the column's slot (kept in place, otherwise taken), its
        // absolute column written into the table, and set in each quad.
        let trans_slot = if formats.trans_compact && prepared.trans_len() != 0 {
            let s = match in_place_plan.as_ref().and(old_slot) {
                Some(s) => s,
                None => self.take_slot(),
            };
            self.up.write(&self.queue, &self.trans_slots, u64::from(s) * 8, bytemuck::cast_slice(&[m.x, m.z]));
            prepared.set_slot(s);
            Some(s)
        } else {
            None
        };
        let sent =
            in_place_plan.as_ref().map_or_else(|| prepared.bytes(), |k| k.writes.iter().map(|(.., r)| r.len()).sum());
        crate::perf_log::upload_bytes(sent, prepared.bytes(), in_place_plan.is_some());
        let (face_n, fluid_n, trans_n) =
            (prepared.face_len() as u32, prepared.fluid_len() as u32, prepared.trans_len() as u32);
        let Prepared { faces, counts, fluids: fluid_words, fluid_counts, trans: trans_words, centers, free, .. } =
            prepared;
        let (fluid_bytes, trans_bytes) = (formats.fluid_bytes(), formats.trans_bytes());
        let trans_starts = m.fluid_starts[2].clone();
        let key = (m.x.div_euclid(REGION), m.z.div_euclid(REGION));
        let (device, queue, layout, slots) = (&self.device, &self.queue, &self.column_layout, &self.trans_slots);
        let up = &mut self.up;
        clock.lap(UPLOAD_BOOKKEEPING);
        let region = self.regions.entry(key).or_insert_with(|| RegionGpu::new(device, layout, key, fluid_bytes, slots));
        clock.lap(UPLOAD_ALLOC);
        let trans = &mut self.trans;
        // Column whose order does not change the image: mesh order, never re-sorted, no centre
        // kept (`RUSTCRAFT_SORTALL=1` re-sorts everything, for comparison).
        let free = !self.sort_all && free;
        let eye = self.sort_eye.filter(|_| !free).map(|e| eye_in_column(e, (m.x, m.z), self.sort_min_y));
        let order_of = |trans_start: u32| {
            if centers.len() == trans_n as usize {
                translucent_order(&centers, &trans_starts, eye, trans_start)
            } else {
                (trans_start..trans_start + trans_n).collect()
            }
        };
        let (face_start, fluid_start, trans_start, order_start, order_len);
        if let Some(k) = in_place_plan {
            // In place: only the changed ranges go out; the order is redone (it depends on
            // the exact eye of the last sort), except an unchanged identity order.
            let s = k.slot;
            (face_start, fluid_start, trans_start, order_start) = (s.faces.0, s.fluids.0, s.trans.0, s.order.0);
            crate::perf_log::counter_per_second("upload_in_place_writes_per_s", k.writes.len() as u64);
            for (t, at, r) in k.writes {
                let stage = match t {
                    Target::Faces => {
                        write_faces(up, queue, &region.faces, at, &faces, r);
                        UPLOAD_WRITE_FACES
                    }
                    Target::Fluids => {
                        up.write(queue, &region.fluids, at, &bytemuck::cast_slice(&fluid_words)[r]);
                        UPLOAD_WRITE_FLUIDS
                    }
                    Target::Trans => {
                        up.write(queue, &trans.fluids, at, &bytemuck::cast_slice(&trans_words)[r]);
                        UPLOAD_WRITE_TRANSLUCENT
                    }
                };
                clock.lap(stage);
            }
            order_len = trans_n;
            if k.order {
                let order = order_of(trans_start);
                clock.lap(UPLOAD_SORT);
                up.write(queue, &trans.order, u64::from(order_start) * ORDER_BYTES, bytemuck::cast_slice(&order));
                clock.lap(UPLOAD_WRITE_ORDER);
            }
        } else {
            let faces_size = region.faces.size();
            face_start = alloc_in(device, queue, up, &mut region.faces, &mut region.face_arena, face_n, FACE_BYTES);
            clock.lap(UPLOAD_ALLOC);
            let all_faces = 0..face_n as usize * FACE_BYTES as usize;
            write_faces(up, queue, &region.faces, u64::from(face_start) * FACE_BYTES, &faces, all_faces);
            clock.lap(UPLOAD_WRITE_FACES);
            let fluids_size = region.fluids.size();
            fluid_start =
                alloc_in(device, queue, up, &mut region.fluids, &mut region.fluid_arena, fluid_n, fluid_bytes);
            count_growth("upload_growths_faces_per_s", region.faces.size() != faces_size);
            count_growth("upload_growths_fluids_per_s", region.fluids.size() != fluids_size);
            if region.fluids.size() != fluids_size || region.faces.size() != faces_size {
                region.bind = region_bind(device, layout, &region.pos, &region.fluids, slots, &region.faces);
            }
            clock.lap(UPLOAD_ALLOC);
            if !fluid_words.is_empty() {
                up.write(
                    queue,
                    &region.fluids,
                    u64::from(fluid_start) * fluid_bytes,
                    bytemuck::cast_slice(&fluid_words),
                );
            }
            clock.lap(UPLOAD_WRITE_FLUIDS);
            let trans_size = trans.fluids.size();
            trans_start = alloc_in(device, queue, up, &mut trans.fluids, &mut trans.fluid_arena, trans_n, trans_bytes);
            count_growth("upload_growths_translucent_per_s", trans.fluids.size() != trans_size);
            if trans.fluids.size() != trans_size {
                trans.bind = region_bind(device, layout, &trans.pos, &trans.fluids, slots, &trans.faces);
            }
            clock.lap(UPLOAD_ALLOC);
            if !trans_words.is_empty() {
                up.write(
                    queue,
                    &trans.fluids,
                    u64::from(trans_start) * trans_bytes,
                    bytemuck::cast_slice(&trans_words),
                );
            }
            clock.lap(UPLOAD_WRITE_TRANSLUCENT);
            let order = order_of(trans_start);
            clock.lap(UPLOAD_SORT);
            order_len = order.len() as u32;
            let order_size = trans.order.size();
            order_start = alloc_in(device, queue, up, &mut trans.order, &mut trans.order_arena, order_len, ORDER_BYTES);
            count_growth("upload_growths_order_per_s", trans.order.size() != order_size);
            clock.lap(UPLOAD_ALLOC);
            if !order.is_empty() {
                up.write(queue, &trans.order, u64::from(order_start) * ORDER_BYTES, bytemuck::cast_slice(&order));
            }
            clock.lap(UPLOAD_WRITE_ORDER);
            region.columns += 1;
        }
        let trans_centers = if free { Vec::new() } else { centers };
        let sorted_at = self.sort_eye.map(|e| e.map(|v| v.floor() as i32));
        // New mesh: viewpoint of the camera of its sort (none for a column without a sort).
        let povs = if trans_centers.is_empty() {
            Vec::new()
        } else {
            column_povs(self.sort_eye, &trans_starts, (m.x, m.z), self.sort_min_y.div_euclid(16))
        };
        let order_starts: Vec<u32> = trans_starts.iter().map(|&r| order_start + r).collect();
        // Absolute ranges in the region's buffers.
        let place = |base: u32, counts: [u32; 3], rel: &[Vec<u32>; 3]| {
            let mut ranges: [std::ops::Range<u32>; 3] = Default::default();
            let mut starts: [Vec<u32>; 3] = Default::default();
            let mut at = base;
            for i in 0..3 {
                ranges[i] = at..at + counts[i];
                starts[i] = rel[i].iter().map(|&r| at + r).collect();
                at += counts[i];
            }
            (ranges, starts)
        };
        let (ranges, starts) = place(face_start, counts, &m.starts);
        let (mut fluid_ranges, mut fluid_starts) = place(fluid_start, fluid_counts, &m.fluid_starts);
        fluid_ranges[2] = trans_start..trans_start + trans_n;
        fluid_starts[2] = m.fluid_starts[2].iter().map(|&r| trans_start + r).collect();
        let column = ColumnGpu {
            digest: if crate::remesh::checking() { crate::remesh::digest(m) } else { 0 },
            region: key,
            face_alloc: (face_start, face_n),
            fluid_alloc: (fluid_start, fluid_n),
            trans_alloc: (trans_start, trans_n),
            ranges,
            fluid_ranges,
            starts,
            fluid_starts,
            vis: m.vis.as_slice().into(),
            trans_centers,
            trans_starts,
            order_alloc: (order_start, order_len),
            order_starts,
            sorted_at,
            povs,
            version: m.version,
            trans_slot,
        };
        self.columns.insert((m.x, m.z), column);
        clock.lap(UPLOAD_BOOKKEEPING);
    }

    /// Free slot of the table of the compact translucents' columns.
    fn take_slot(&mut self) -> u16 {
        if let Some(s) = self.free_slots.pop() {
            return s;
        }
        assert!(self.next_slot < TRANS_SLOTS, "no more than {TRANS_SLOTS} translucent columns loaded");
        self.next_slot += 1;
        (self.next_slot - 1) as u16
    }

    /// Gives a removed column's place back to its region; empty region deleted.
    fn release(&mut self, c: &ColumnGpu) {
        self.trans.fluid_arena.release(c.trans_alloc.0, c.trans_alloc.1);
        self.trans.order_arena.release(c.order_alloc.0, c.order_alloc.1);
        self.free_slots.extend(c.trans_slot);
        let Some(r) = self.regions.get_mut(&c.region) else { return };
        r.face_arena.release(c.face_alloc.0, c.face_alloc.1);
        r.fluid_arena.release(c.fluid_alloc.0, c.fluid_alloc.1);
        r.columns -= 1;
        if r.columns == 0 {
            self.regions.remove(&c.region);
        }
    }

    pub fn remove(&mut self, x: i32, z: i32) {
        self.fade_ring.remove((x, z));
        if let Some(c) = self.columns.remove(&(x, z)) {
            self.release(&c);
            self.columns_rev += 1;
            self.vis_rev += 1;
        }
        if self.empty_vis.remove(&(x, z)).is_some() {
            self.columns_rev += 1;
            self.vis_rev += 1;
        }
    }

    pub fn clear(&mut self) {
        self.columns.clear();
        self.empty_vis.clear();
        self.regions.clear();
        self.fade_ring.clear();
        // Identity buffer returned to its starting size: it only grows with the largest region.
        if self.face_ids.1 > FACE_IDS_START {
            self.face_ids = (identity_buffer(&self.device, FACE_IDS_START), FACE_IDS_START);
        }
        self.trans =
            RegionGpu::new(&self.device, &self.column_layout, (0, 0), self.formats.trans_bytes(), &self.trans_slots);
        self.free_slots.clear();
        self.next_slot = 0;
        // The buffers restart from their first place: no cached command may outlive them.
        self.lists.forget_columns();
        self.columns_rev += 1;
        self.vis_rev += 1;
    }
}

#[cfg(test)]
#[path = "column_upload_test.rs"]
mod tests;
