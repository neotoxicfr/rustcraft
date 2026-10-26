//! wgpu rendering of milestone 1: one pass, one wireframe pipeline. One instance per face (the
//! shader rebuilds the corners), one buffer and one bind group per column, columns
//! outside the field of view discarded on the CPU, GPU time measured by timestamp.

use hashbrown::HashMap;
use std::sync::Arc;
use std::time::Instant;

use crate::gpu_time::Role;
use glam::{Mat4, Vec3, Vec4};
use wgpu::util::DeviceExt;
use winit::window::Window;

use rc_assets::BlockModels;
use rc_mesh::blocks::{BUCKETS, BlockMesh};

use crate::arena::Arena;
use crate::present_watch::Stage;

mod lists;
pub(crate) mod upload;
use upload::Uploader;

/// Camera slots in the frame's uniforms (`upload::Slots`).
const CAM_WORLD: usize = 0;
const CAM_HAND: usize = 1;
const CAM_TOTEM: usize = 2;
const CAM_GUI: usize = 3;
const _: () = assert!(size_of::<CameraUniform>() == upload::SLOT);

/// Reversed depth, like the game: near plane at 1, far at 0, buffer cleared to 0, "nearer
/// or equal" test = `GreaterEqual`. The precision of the 32-bit float then stays roughly
/// relative to the distance: no more flicker of almost coplanar faces in the distance.
pub const DEPTH_CLEAR: f32 = 0.0;
/// Default depth test (passes if at least as near as what is already written).
pub const DEPTH_NEARER: wgpu::CompareFunction = wgpu::CompareFunction::GreaterEqual;

/// Depth offset towards the camera (values of the game: constant, slope), in the sense of the
/// reversed depth (positive = nearer).
pub const fn depth_bias(constant: i32, slope: f32) -> wgpu::DepthBiasState {
    wgpu::DepthBiasState { constant, slope_scale: slope, clamp: 0.0 }
}

/// Perspective projection of the world (straight view, reversed [0, 1] depth): near and
/// far planes swapped, like the game.
pub fn perspective(fov_deg: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    glam::camera::rh::proj::directx::perspective(fov_deg.to_radians(), aspect, far, near)
}

/// Layer offset of the game in perspective: the view is scaled by (1 − bias/4096) around
/// the eye (bias 1 = towards the camera: armors, trims, lines, shadows; −1 = towards the back).
/// Same pixel on screen, depth slightly nearer.
pub fn view_layering(eye: Vec3, bias: f32) -> Mat4 {
    Mat4::from_translation(eye) * Mat4::from_scale(Vec3::splat(1.0 - bias / 4096.0)) * Mat4::from_translation(-eye)
}

/// Centre of a quad for the translucent order: middle of the diagonal vertex 0 – vertex 2,
/// like the game (differs from the mean of the 4 vertices for a non-planar quad or a trapezoid).
fn quad_center<const N: usize>(p: &[[f32; N]; 4]) -> [f32; 3] {
    std::array::from_fn(|a| (p[0][a] + p[2][a]) * 0.5)
}

/// Visibility walk produced by the background task: buffers, sections reached, duration (ms).
/// Node of the occlusion walk: (section, arrival directions, step).
type GraphNode = ((i32, i32, i32), u8, u16);
/// Node for F3: walk node, face connectivity, geometry present.
pub type OccNode = ((i32, i32, i32), u8, u16, rc_mesh::visibility::Vis, bool);
/// With `RUSTCRAFT_OPT_REACHGRID`, the reached sections as a bit grid, made by the task.
type GraphResult = (rc_mesh::visibility::Graph, Vec<(i32, i32, i32)>, f64, Option<lists::ReachGrid>);

/// Maximum radius (in columns) of the visibility walk; the columns present are already
/// limited by the server's view distance.
const VIEW_LIMIT: i32 = 64;

/// Layer bit that designates the second texture array (larger textures).
const BIG_LAYER: u32 = 0x8000;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CameraUniform {
    view_proj: [[f32; 4]; 4],
    origin: [i32; 4],
    /// Shading per direction (bottom, top, north, south, west, east).
    shade: [f32; 6],
    /// GPU appearance fade (`RUSTCRAFT_OPT_FADEGPU`): frame instant
    /// ([`crate::terrain_shader::fade_clock`]) and fade duration, in µs.
    fade_now: u32,
    fade_len: f32,
    /// Eye in the vertex frame, fog, sky colour, view without translation.
    eye: [f32; 4],
    fog_color: [f32; 4],
    fog: [f32; 4],
    fog_sky: [f32; 4],
    sky_color: [f32; 4],
    sky_view_proj: [[f32; 4]; 4],
}

impl CameraUniform {
    /// Camera of a layer without terrain or fog (totem, interface models): the fade clock
    /// is not read there, it stays at 0 so that the unchanged upload is skipped.
    fn overlay(self, view_proj: [[f32; 4]; 4], eye: [f32; 4]) -> Self {
        CameraUniform { view_proj, eye, fog_color: [0.0; 4], fog: [1e9, 2e9, 1e9, 2e9], fade_now: 0, ..self }
    }
}

/// Environment of the frame: fog, sky colour, dome and dark disc to draw.
#[derive(Debug, Clone, Copy)]
pub struct FrameEnv {
    pub fog: rc_env::Fog,
    pub sky: [f32; 4],
    pub dome: bool,
    pub disc: bool,
    /// Celestial bodies and clouds.
    pub bodies: crate::sky::SkyFrame,
    /// Far plane: max(64 × render distance, 16 × cloud range), like the game.
    pub far: f32,
    /// Water or lava in the camera (field of view, `hand-camera.md` § 4.3).
    pub fluid: bool,
}

/// Region of 16 × 16 columns: one face buffer and one fluid buffer shared by its columns
/// (one binding and one buffer per region instead of one per column), origin in uniform.
struct RegionGpu {
    faces: wgpu::Buffer,
    face_arena: Arena,
    fluids: wgpu::Buffer,
    fluid_arena: Arena,
    /// Draw order of the translucent quads: indices into `fluids`, re-sorted without re-sending the
    /// quads themselves (4 bytes instead of 108 or 128 per quad).
    order: wgpu::Buffer,
    order_arena: Arena,
    /// Origin of the region (uniform) and binding (origin + fluid quads read-only).
    pos: wgpu::Buffer,
    bind: wgpu::BindGroup,
    columns: u32,
}

/// Side of a region in columns (4 bits per axis in the faces).
const REGION: i32 = 16;
/// Default duration of the section appearance fade (setting `chunkSectionFadeInTime`).
const FADE_SECS: f64 = 0.75;
/// Batch key of the global translucent buffer (no real region has this key).
const TRANS_KEY: (i32, i32) = (i32::MIN, i32::MIN);
const FACE_BYTES: u64 = 24;
const ORDER_BYTES: u64 = 4;
/// Usages of the region buffers (the fluids are also read by index in the shader).
const REGION_USAGE: wgpu::BufferUsages = wgpu::BufferUsages::VERTEX
    .union(wgpu::BufferUsages::STORAGE)
    .union(wgpu::BufferUsages::COPY_DST)
    .union(wgpu::BufferUsages::COPY_SRC);

/// Range of `n` elements in a region's buffer, grown (GPU copy) if needed.
fn alloc_in(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    up: &mut Uploader,
    buf: &mut wgpu::Buffer,
    arena: &mut Arena,
    n: u32,
    stride: u64,
) -> u32 {
    if let Some(s) = arena.alloc(n) {
        return s;
    }
    // Growth of 25%: little space wasted (the GPU copy only costs at loading).
    let cap = (arena.capacity + arena.capacity / 4).max(arena.capacity + n);
    let bigger = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("region"),
        size: u64::from(cap) * stride,
        usage: REGION_USAGE,
        mapped_at_creation: false,
    });
    // Copy submitted right away, preceded by the pending staging (writes into the old
    // buffer): the following writes into the new buffer come after.
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_buffer_to_buffer(buf, 0, &bigger, 0, u64::from(arena.capacity) * stride);
    up.submit(queue, enc.finish());
    *buf = bigger;
    arena.grow(cap);
    arena.alloc(n).expect("room after growth")
}

impl RegionGpu {
    fn new(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        key: (i32, i32),
        fluid_bytes: u64,
        slots: &wgpu::Buffer,
    ) -> Self {
        let buffer = |label, n: u32, stride: u64| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: u64::from(n) * stride,
                usage: REGION_USAGE,
                mapped_at_creation: false,
            })
        };
        let (faces_cap, fluids_cap, order_cap) = (1 << 16, 1 << 10, 1 << 10);
        let pos = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&[key.0 * REGION, key.1 * REGION, 0, 0]),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let fluids = buffer("region fluids", fluids_cap, fluid_bytes);
        let faces = buffer("region faces", faces_cap, FACE_BYTES);
        let bind = region_bind(device, layout, &pos, &fluids, slots, &faces);
        RegionGpu {
            faces,
            face_arena: Arena::new(faces_cap),
            fluids,
            fluid_arena: Arena::new(fluids_cap),
            order: buffer("translucent order", order_cap, ORDER_BYTES),
            order_arena: Arena::new(order_cap),
            pos,
            bind,
            columns: 0,
        }
    }
}

fn region_bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    pos: &wgpu::Buffer,
    fluids: &wgpu::Buffer,
    slots: &wgpu::Buffer,
    faces: &wgpu::Buffer,
) -> wgpu::BindGroup {
    // The same quad buffer is read in the old format (1) or as compact translucents (2); the
    // faces are read by index in the indexed draws (4).
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: pos.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: fluids.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: fluids.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 3, resource: slots.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 4, resource: faces.as_entire_binding() },
        ],
    })
}

/// Draw order of the translucent quads of a column: within each section, from the farthest to the
/// nearest (distance of their centre to the camera, stable sort), like the game; absolute indices
/// into the region's fluid buffer (`base` = first translucent quad of the column).
fn translucent_order(centers: &[[f32; 3]], starts: &[u32], eye: Option<[f32; 3]>, base: u32) -> Vec<u32> {
    let mut order: Vec<u32> = (0..centers.len() as u32).collect();
    if let Some(eye) = eye {
        let mut scratch = SortScratch::default();
        for w in starts.windows(2) {
            sort_section(centers, &mut order[w[0] as usize..w[1] as usize], eye, trans_key(), &mut scratch);
        }
    }
    order.iter_mut().for_each(|i| *i += base);
    order
}

/// Way of sorting a translucent section (`RUSTCRAFT_OPT_TRANSKEY`, same order bit for bit): `Old`
/// recomputes the two distances at each comparison of a stable sort; `Pairs` (`=pairs`) computes them
/// once then sorts (decreasing distance, index); `Bits` (default) sorts integer keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransKey {
    Old,
    Pairs,
    Bits,
}

fn trans_key() -> TransKey {
    static K: std::sync::OnceLock<TransKey> = std::sync::OnceLock::new();
    *K.get_or_init(|| {
        let v = rc_data::env_flags::read_os("RUSTCRAFT_OPT_TRANSKEY");
        let k = match v.as_ref().and_then(|v| v.to_str()) {
            Some("0") => TransKey::Old,
            Some("pairs") => TransKey::Pairs,
            _ => TransKey::Bits,
        };
        eprintln!("[opt] translucent sort: {k:?}");
        k
    })
}

/// Buffers of the section sorts, kept from one sort to the next.
#[derive(Default)]
struct SortScratch {
    pairs: Vec<(f32, u32)>,
    keys: Vec<u64>,
}

/// Sorts `section` (indices into `centers`, increasing on input) from the farthest to the nearest of
/// `eye`, ties in input order; `scratch`: reused buffers.
fn sort_section(centers: &[[f32; 3]], section: &mut [u32], eye: [f32; 3], how: TransKey, scratch: &mut SortScratch) {
    if section.len() < 2 {
        return;
    }
    let dist = |i: u32| {
        let c = centers[i as usize];
        (0..3).map(|a| (c[a] - eye[a]) * (c[a] - eye[a])).sum::<f32>()
    };
    match how {
        TransKey::Old => section.sort_by(|&a, &b| dist(b).total_cmp(&dist(a))),
        TransKey::Pairs => {
            let pairs = &mut scratch.pairs;
            pairs.clear();
            pairs.extend(section.iter().map(|&i| (dist(i), i)));
            pairs.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
            section.iter_mut().zip(pairs.iter()).for_each(|(s, p)| *s = p.1);
        }
        TransKey::Bits => {
            // Float bits made increasing in the order of `total_cmp`, inverted for the
            // decreasing distance; the index in the low bits breaks ties.
            let keys = &mut scratch.keys;
            keys.clear();
            keys.extend(section.iter().map(|&i| {
                let b = dist(i).to_bits();
                let k = (b ^ (((b as i32) >> 31) as u32 >> 1)) ^ 0x8000_0000;
                u64::from(!k) << 32 | u64::from(i)
            }));
            keys.sort_unstable();
            section.iter_mut().zip(keys.iter()).for_each(|(s, &k)| *s = k as u32);
        }
    }
}

/// `RUSTCRAFT_OPT_TRANSPOV` (default): re-sort of the translucents section by section according to its
/// viewpoint, like the game (`docs/specs/visibility.md`, "Translucent sorting"); `=0`: whole column
/// re-sorted at each camera block, in rotation.
fn trans_pov() -> bool {
    static P: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *P.get_or_init(|| {
        let on = rc_data::env_flags::read_os("RUSTCRAFT_OPT_TRANSPOV").is_none_or(|v| v != "0");
        eprintln!("[opt] translucent re-sort by viewpoint: {on}");
        on
    })
}

/// Buffers of the per-section re-sort, kept from one frame to the next.
#[derive(Default)]
struct TransScratch {
    order: Vec<u32>,
    sort: SortScratch,
}

/// Viewpoint never noted (section never sorted with a camera): always different.
const POV_UNSEEN: u8 = u8::MAX;

/// Viewpoint of a section (section coordinates `sec`): on each axis, camera section
/// minus this one, bounded to −1..1; coded (x + 1)·9 + (y + 1)·3 + (z + 1).
fn section_pov(eye: [f64; 3], sec: [i32; 3]) -> u8 {
    let c = |e: f64, s: i32| (((e.floor() as i32) >> 4) - s).clamp(-1, 1) + 1;
    (c(eye[0], sec[0]) * 9 + c(eye[1], sec[1]) * 3 + c(eye[2], sec[2])) as u8
}

/// Camera within the slab of the section on at least one axis.
fn pov_aligned(p: u8) -> bool {
    p / 9 == 1 || p / 3 % 3 == 1 || p % 3 == 1
}

/// Viewpoints of the sections of a column (`starts`: first quad of each section, plus the end;
/// `sy0`: bottom section of the world) for camera `eye`, or never noted without a camera.
fn column_povs(eye: Option<[f64; 3]>, starts: &[u32], column: (i32, i32), sy0: i32) -> Vec<u8> {
    (0..starts.len().saturating_sub(1))
        .map(|s| eye.map_or(POV_UNSEEN, |e| section_pov(e, [column.0, sy0 + s as i32, column.1])))
        .collect()
}

/// Translucent sections of an examined column to re-sort, into `out` (increasing), viewpoints
/// noted: viewpoint changed since the last sort, or camera that has just changed block
/// (`moved`) and section on the axis. Sections without translucents ignored.
fn pov_resorts(
    povs: &mut [u8],
    starts: &[u32],
    eye: [f64; 3],
    column: (i32, i32),
    sy0: i32,
    moved: bool,
    out: &mut Vec<usize>,
) {
    out.clear();
    for (s, w) in starts.windows(2).enumerate() {
        if w[1] == w[0] {
            continue;
        }
        let p = section_pov(eye, [column.0, sy0 + s as i32, column.1]);
        if p != povs[s] || moved && pov_aligned(p) {
            povs[s] = p;
            out.push(s);
        }
    }
}

/// Translucent columns examined per frame (`keys` non-empty, `start` < length): the 25 within
/// ±2 of the camera's column, then max(n / 8, 15) in rotation from `start` (without the
/// nearby ones, already seen); and the start of the next rotation.
fn pov_picks(keys: &[(i32, i32)], start: usize, cam: (i32, i32)) -> (impl Iterator<Item = (i32, i32)> + '_, usize) {
    let n = keys.len();
    let window = (n / 8).max(15).min(n);
    let near = move |k: (i32, i32)| (k.0 - cam.0).abs() <= 2 && (k.1 - cam.1).abs() <= 2;
    let picks = (-2..=2)
        .flat_map(move |dx| (-2..=2).map(move |dz| (cam.0 + dx, cam.1 + dz)))
        .chain((0..window).map(move |i| keys[(start + i) % n]).filter(move |&k| !near(k)));
    (picks, (start + window) % n)
}

/// Sections reached by the walk: generation, bottom of the world, number of sections, set.
type ReachedSet = (u64, i32, i32, Arc<lists::Reach>);

/// Key of the walk radius: version of the columns, camera column.
type RadiusKey = (u64, (i32, i32));

/// True if the order of the translucent quads of each section cannot change the image: in
/// each section (`starts` = first quad of each section), all in a single axis plane
/// (surface of a lake, single-plane pane), each contained in a unit cell, never two of the
/// same orientation in the same cell. Back faces removed: from any viewpoint, the quads
/// drawn of such a section do not overlap.
fn order_free(quads: &[rc_mesh::fluid::FluidQuad], starts: &[u32]) -> bool {
    const EPS: f32 = 1e-4;
    let mut keys: Vec<(i32, i32, bool)> = Vec::new();
    for (i, &s) in starts.iter().enumerate() {
        let e = starts.get(i + 1).map_or(quads.len(), |&e| e as usize);
        let section = &quads[(s as usize).min(e)..e];
        if section.len() < 2 {
            continue;
        }
        let c = &section[0].corners;
        let Some(a) = (0..3).find(|&a| c.iter().all(|p| p[a] == c[0][a])) else { return false };
        let plane = c[0][a];
        let (u, v) = ((a + 1) % 3, (a + 2) % 3);
        keys.clear();
        for q in section {
            let p = &q.corners;
            if p.iter().any(|p| p[a] != plane) {
                return false;
            }
            let cell = |i: usize| {
                let lo = p.iter().map(|p| p[i]).fold(f32::MAX, f32::min);
                let hi = p.iter().map(|p| p[i]).fold(f32::MIN, f32::max);
                let k = (lo + EPS).floor();
                (hi <= k + 1.0 + EPS).then_some(k as i32)
            };
            let (Some(cu), Some(cv)) = (cell(u), cell(v)) else { return false };
            // Orientation: sign of the normal (p1 − p0) × (p2 − p0) on the plane's axis.
            let (e1, e2) = ([p[1][u] - p[0][u], p[1][v] - p[0][v]], [p[2][u] - p[0][u], p[2][v] - p[0][v]]);
            let front = e1[0] * e2[1] - e1[1] * e2[0] > 0.0;
            keys.push((cu, cv, front));
            // Double face: both orientations, like its two earlier quads.
            if q.texture & rc_mesh::fluid::DOUBLE_FACE != 0 {
                keys.push((cu, cv, !front));
            }
        }
        keys.sort_unstable();
        if keys.windows(2).any(|k| k[0] == k[1]) {
            return false;
        }
    }
    true
}

/// For each group of faces (order `Dir`, then unaligned), range of sections of the
/// column where it can be seen from the front. A face oriented +x is only visible if the camera is
/// beyond its plane; the planes of a section stay within `MARGE` blocks of its box (model
/// elements that overflow the block), hence a cautious test that only removes back faces.
fn facing(eye: [f64; 3], column: (i32, i32), min_y: i32) -> [(usize, usize); BUCKETS] {
    const MARGE: f64 = 2.0;
    let all = (0, usize::MAX);
    let none = (0, 0);
    let (x0, z0) = (f64::from(column.0 * 16), f64::from(column.1 * 16));
    let y = eye[1] - f64::from(min_y);
    // Top (+y): sections whose bottom − margin is below the eye; bottom (−y): those whose
    // top + margin is above.
    let up_end = ((y + MARGE) / 16.0).ceil().max(0.0) as usize;
    let down_start = ((y - MARGE - 16.0) / 16.0).floor().max(-1.0) as i64 + 1;
    let side = |ok: bool| if ok { all } else { none };
    [
        (down_start as usize, usize::MAX),
        (0, up_end),
        side(eye[2] < z0 + 16.0 + MARGE),
        side(eye[2] > z0 - MARGE),
        side(eye[0] < x0 + 16.0 + MARGE),
        side(eye[0] > x0 - MARGE),
        all,
    ]
}

/// Camera relative to a column (vertex frame: x, z in the column, y from the bottom).
fn eye_in_column(eye: [f64; 3], column: (i32, i32), min_y: i32) -> [f32; 3] {
    [
        (eye[0] - f64::from(column.0 * 16)) as f32,
        (eye[1] - f64::from(min_y)) as f32,
        (eye[2] - f64::from(column.1 * 16)) as f32,
    ]
}

/// Command batch: layer, fluid?, region, first command, count.
type Batch = (usize, bool, (i32, i32), u32, u32);

/// Dependencies of a frame's commands: eye, yaw, pitch (and sort field in the high bits),
/// revision of the columns, walk, frame size, bottom of the world, number of sections.
type DrawKey = ([u64; 3], u32, u64, u64, u64, (u32, u32), i32, i32);

/// Indirect draw command (format expected by the GPU).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DrawArgs {
    vertex_count: u32,
    instance_count: u32,
    first_vertex: u32,
    first_instance: u32,
}

/// Indirect indexed draw command (format expected by the GPU).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
struct DrawIndexedArgs {
    index_count: u32,
    instance_count: u32,
    first_index: u32,
    base_vertex: i32,
    first_instance: u32,
}

/// Faces at most per indexed command: 16-bit indices (4 × 16,384 vertices), 192 KiB of indices.
const INDEXED_FACES: u32 = 1 << 14;

/// Starting size of the identity buffer (`FACEIDX`), in faces: 256 KiB.
const FACE_IDS_START: u32 = 1 << 16;

/// Indices of the first `faces` faces: triangles 0-1-2 then 0-2-3 of each, vertices 4f to 4f + 3.
pub(crate) fn face_indices(faces: u32) -> Vec<u16> {
    (0..faces)
        .flat_map(|f| {
            let b = (f * 4) as u16;
            [b, b + 1, b + 2, b, b + 2, b + 3]
        })
        .collect()
}

/// Identity buffer 0..n (u32), read as an instance attribute by the indexed faces.
fn identity_buffer(device: &wgpu::Device, n: u32) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("face identity"),
        contents: bytemuck::cast_slice(&(0..n).collect::<Vec<u32>>()),
        usage: wgpu::BufferUsages::VERTEX,
    })
}

/// Indexed commands equivalent to the instanced commands `args` (block faces, 6 vertices
/// per instance): same faces in the same order, an instance whose attribute (identity buffer,
/// `first_instance`) gives the first face, split at [`INDEXED_FACES`] faces.
fn indexed_args(args: &[DrawArgs], out: &mut Vec<DrawIndexedArgs>) {
    for a in args {
        let mut start = a.first_instance;
        let end = a.first_instance + a.instance_count;
        while start < end {
            let n = (end - start).min(INDEXED_FACES);
            out.push(DrawIndexedArgs {
                index_count: n * 6,
                instance_count: 1,
                first_index: 0,
                base_vertex: 0,
                first_instance: start,
            });
            start += n;
        }
    }
}

/// Connectivity of the sections of each column, input of the visibility walk.
type VisSnapshot = Arc<HashMap<(i32, i32), Arc<[rc_mesh::visibility::Vis]>>>;

struct ColumnGpu {
    /// Fingerprint of the uploaded mesh (`RUSTCRAFT_MESHCHECK` only, otherwise 0).
    digest: u64,
    region: (i32, i32),
    /// Ranges (start, length) of the column's faces and fluids in its region.
    face_alloc: (u32, u32),
    fluid_alloc: (u32, u32),
    /// Range of the translucent quads in the global buffer.
    trans_alloc: (u32, u32),
    /// Instance ranges per layer (opaque, cutout, translucent).
    ranges: [std::ops::Range<u32>; 3],
    /// Fluid faces and their ranges per layer.
    fluid_ranges: [std::ops::Range<u32>; 3],
    /// Start (absolute in the buffer) of the faces of each section, per layer, plus the end.
    starts: [Vec<u32>; 3],
    fluid_starts: [Vec<u32>; 3],
    /// Connectivity between the faces of each section (occlusion sorting).
    vis: Arc<[rc_mesh::visibility::Vis]>,
    /// Translucent quads (glass, water…) kept CPU-side to re-sort them, start of each section,
    /// and camera block at the last sort.
    trans_centers: Vec<[f32; 3]>,
    trans_starts: Vec<u32>,
    /// Range (start, length) of the translucent order in the global buffer, and absolute start of
    /// each section in this buffer (one more entry for the end).
    order_alloc: (u32, u32),
    order_starts: Vec<u32>,
    sorted_at: Option<[i32; 3]>,
    /// Viewpoint of each section at its last sort (`RUSTCRAFT_OPT_TRANSPOV`).
    povs: Vec<u8>,
    version: u64,
    /// Slot of the column in the compact translucents table.
    trans_slot: Option<u16>,
}

/// Baked face as read by the shader (96 bytes).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct QuadGpu {
    p: [[f32; 4]; 4],
    v: [f32; 4],
    info: [u32; 4],
}

/// Explicit face (fluid, translucent, offset model) as read by the shader (128 bytes):
/// corners, v, (texture | column in the region, 0, absolute column x, z), colour of each corner,
/// light of corners 0-1 and 2-3.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct FluidGpu {
    p: [[f32; 4]; 4],
    v: [f32; 4],
    meta: [u32; 4],
    colors: [u32; 4],
    light: [u32; 4],
}

/// GPU formats of the explicit quads, chosen once at startup (same image; `docs/perf.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuFormats {
    /// `RUSTCRAFT_OPT_FLUID96`: region fluids (lava, layers 0 and 1) in 96 bytes, a
    /// single colour for the 4 corners. Only if every offset model fits in a compact face and
    /// these faces are in force (otherwise 4-colour quads remain in those layers).
    pub fluid96: bool,
    /// `RUSTCRAFT_OPT_TRANSCOMPACT`: translucents sorted in 108 bytes, absolute column read from
    /// a slot (table of 65,536 columns), texture on 15 bits.
    pub trans_compact: bool,
}

/// Words of a quad depending on the format: old 32, compact fluid 24, compact translucent 27.
const OLD_WORDS: usize = 32;
const FLUID96_WORDS: usize = 24;
const TRANS_WORDS: usize = 27;
/// Column slots of the compact translucents (16 bits of the information word).
const TRANS_SLOTS: usize = 1 << 16;

impl GpuFormats {
    fn decide(all_compact: bool, textures: usize) -> Self {
        let on = |n: &str| rc_data::env_flags::read_os(n).is_none_or(|v| v != "0");
        GpuFormats {
            fluid96: on("RUSTCRAFT_OPT_FLUID96") && all_compact && crate::remesh::formats().0,
            trans_compact: on("RUSTCRAFT_OPT_TRANSCOMPACT") && textures <= 0x8000,
        }
    }

    /// Shader entry points: sorted translucents, region fluids.
    fn entries(self) -> (&'static str, &'static str) {
        (
            if self.trans_compact { "vs_sorted_c" } else { "vs_sorted" },
            if self.fluid96 { "vs_fluid_c" } else { "vs_fluid" },
        )
    }

    pub fn fluid_words(self) -> usize {
        if self.fluid96 { FLUID96_WORDS } else { OLD_WORDS }
    }

    pub fn trans_words(self) -> usize {
        if self.trans_compact { TRANS_WORDS } else { OLD_WORDS }
    }

    fn fluid_bytes(self) -> u64 {
        self.fluid_words() as u64 * 4
    }

    fn trans_bytes(self) -> u64 {
        self.trans_words() as u64 * 4
    }
}

static GPU_FORMATS: std::sync::OnceLock<GpuFormats> = std::sync::OnceLock::new();

/// Formats in force (set by the first renderer; without it, from the environment).
pub fn gpu_formats() -> GpuFormats {
    *GPU_FORMATS.get_or_init(|| GpuFormats::decide(true, 0))
}

/// Sets the formats from the mesh tables (once per launch).
pub fn init_gpu_formats(t: &rc_mesh::blocks::Tables) -> GpuFormats {
    *GPU_FORMATS.get_or_init(|| {
        let f = GpuFormats::decide(t.offsets_all_compact(), t.models.texture_names.len());
        eprintln!("[opt] GPU formats: {f:?}");
        f
    })
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

/// `RUSTCRAFT_SORTALL=1`: all the translucent columns re-sorted (comparison).
fn sort_all() -> bool {
    rc_data::env_flag!("RUSTCRAFT_SORTALL")
}

/// Bytes of a column ready for the GPU: faces (column in the region in bits 20-27),
/// fluids (bits 16-23), translucents (absolute column, or slot set at upload), centres
/// of the translucents and whether their order is free (`order_free`). Made on the pool
/// (`RUSTCRAFT_OPT_MESHPREP`) or at upload.
pub struct Prepared {
    faces: Vec<rc_mesh::blocks::Face>,
    counts: [u32; 3],
    /// Words of the fluids (layers 0 and 1) and translucents, in format `formats`.
    fluids: Vec<u32>,
    fluid_counts: [u32; 3],
    trans: Vec<u32>,
    formats: GpuFormats,
    /// Empty (without being computed) when the order is free and `RUSTCRAFT_OPT_TRANSFREE`.
    centers: Vec<[f32; 3]>,
    free: bool,
}

impl Prepared {
    pub fn bytes(&self) -> usize {
        (self.faces.len() * FACE_BYTES as usize / 4 + self.fluids.len() + self.trans.len()) * 4
    }

    fn fluid_len(&self) -> usize {
        self.fluids.len() / self.formats.fluid_words()
    }

    fn trans_len(&self) -> usize {
        self.trans.len() / self.formats.trans_words()
    }

    fn is_empty(&self) -> bool {
        self.faces.is_empty() && self.fluids.is_empty() && self.trans.is_empty()
    }

    /// Same bytes, same counts and same free order (centres compared bit by bit).
    fn same(&self, o: &Prepared) -> bool {
        fn b<T: bytemuck::NoUninit>(v: &[T]) -> &[u8] {
            bytemuck::cast_slice(v)
        }
        b(&self.faces) == b(&o.faces)
            && self.fluids == o.fluids
            && self.trans == o.trans
            && b(&self.centers) == b(&o.centers)
            && (self.counts, self.fluid_counts, self.free, self.formats) == (o.counts, o.fluid_counts, o.free, o.formats)
    }

    /// Check (`RUSTCRAFT_MESHCHECK`): bytes identical to those of the old upload.
    pub fn check(&self, m: &BlockMesh, key: (i32, i32)) {
        let old = prepare_legacy(m, crate::remesh::opts().transfree, self.formats);
        if !self.same(&old) {
            eprintln!("[meshcheck] ({}, {}): bytes prepared by the pool differ from the upload", key.0, key.1);
        }
    }

    /// Sets the column's slot in each compact translucent.
    fn set_slot(&mut self, slot: u16) {
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
        || o.faces.1 as usize != p.faces.len()
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
    .filter_map(|(t, at, stride, s)| {
        let s = s?;
        let bytes = s.start as usize * stride as usize..s.end as usize * stride as usize;
        Some((t, (u64::from(at) + u64::from(s.start)) * stride, bytes))
    })
    .collect();
    Some(Keep { slot: o, writes, order: !(free && o.free) && p.trans_len() != 0 })
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
    Prepared { faces: all, counts, fluids: fluid_all, fluid_counts, trans: trans_all, formats, centers, free }
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

/// Old format of a quad (128 bytes), reference of the format tests.
#[cfg(test)]
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

/// Instance buffer of each entry point of the terrain (`terrain.wgsl`).
fn instance_layout(vs: &str) -> wgpu::VertexBufferLayout<'static> {
    const ORDER: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![0 => Uint32];
    const FACE: [wgpu::VertexAttribute; 5] =
        wgpu::vertex_attr_array![0 => Uint32, 1 => Uint32, 2 => Uint32, 3 => Uint32x2, 4 => Uint32];
    const FLUID_C: [wgpu::VertexAttribute; 8] = wgpu::vertex_attr_array![
        0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Uint32,
        6 => Uint32, 7 => Uint32x2
    ];
    const FLUID: [wgpu::VertexAttribute; 8] = wgpu::vertex_attr_array![
        0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Uint32x4,
        6 => Uint32x4, 7 => Uint32x4
    ];
    let (array_stride, attributes): (u64, &'static [wgpu::VertexAttribute]) = match vs {
        // Indexed faces: first face of the command, read from the identity buffer.
        "vs_sorted" | "vs_sorted_c" | "vs_idx" => (ORDER_BYTES, &ORDER),
        "vs" => (FACE_BYTES, &FACE),
        "vs_fluid_c" => (FLUID96_WORDS as u64 * 4, &FLUID_C),
        _ => (size_of::<FluidGpu>() as u64, &FLUID),
    };
    wgpu::VertexBufferLayout { array_stride, step_mode: wgpu::VertexStepMode::Instance, attributes }
}

fn quad_gpu(q: &rc_assets::BakedQuad) -> QuadGpu {
    let mut p = [[0.0; 4]; 4];
    let mut v = [0.0; 4];
    for i in 0..4 {
        p[i] = [q.pos[i][0], q.pos[i][1], q.pos[i][2], q.uv[i][0]];
        v[i] = q.uv[i][1];
    }
    QuadGpu { p, v, info: [q.texture, q.tint as u32, q.shade_dir as u32, 0] }
}

pub struct Renderer {
    pub window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    depth: wgpu::TextureView,
    pipelines: [wgpu::RenderPipeline; 3],
    fluid_pipelines: [wgpu::RenderPipeline; 3],
    /// Translucents drawn in sorted order (indices to the region's fluid quads).
    sorted_pipeline: wgpu::RenderPipeline,
    sky_pipeline: wgpu::RenderPipeline,
    sky: crate::sky::Sky,
    /// 2D interface, filled by the app before `render`.
    pub gui: crate::gui::Gui,
    /// Entity rendering (absent without extracted models).
    pub entities: Option<crate::entity_render::EntityRenderer>,
    pub tags: crate::nametag::NameTags,
    /// Sign text (`block-entities.md` § 3).
    pub block_text: crate::block_text::BlockText,
    /// Text displays (`entities-objects.md` § 13.8).
    pub text_displays: crate::text_display::TextDisplays,
    /// Beacon beams and End portals (`block-entities.md` § 4.9 to 4.11).
    pub block_fx: crate::block_fx::BlockFx,
    pub outline: crate::outline::Outline,
    /// Glow outline of the glowing entities.
    pub entity_outline: crate::entity_outline::EntityOutline,
    /// Mining cracks (`interactions.md` § 2.2).
    pub cracks: crate::crack::Cracks,
    /// Debug screen traces (F3+B, F3+G, 3D crosshair); empty: nothing is drawn.
    pub debug_lines: crate::debug_render::DebugLines,
    /// Adapter, effective presentation mode, indirect draw: F3 lines `system_specs`, `fps`.
    pub adapter_info: wgpu::AdapterInfo,
    pub present_mode: wgpu::PresentMode,
    warned_surface: bool,
    /// Presentation modes offered by the surface (choice of vertical sync).
    present_modes_caps: Vec<wgpu::PresentMode>,
    /// Camera binding and block textures to redo when the filtering changes (`video.rs`).
    terrain_res: crate::video::TerrainRes,
    /// RGSS filtering of the block textures (`textureFiltering` = 1).
    pub rgss: bool,
    pub particles: crate::particles::ParticleRenderer,
    /// Rain, snow and world border (`weather_render.rs`).
    pub weather: crate::weather_render::WeatherRenderer,
    /// Title screen panorama, drawn when `show_panorama` (out-of-world screens).
    pub panorama: Option<crate::panorama::Panorama>,
    pub show_panorama: bool,
    /// Menu background blur (requested by the interface, `Gui::blur`).
    blur: crate::blur::Blur,
    /// Leash ropes (`leash.rs`) and screen effects of the next frame (`post_fx.rs`), with the
    /// menu blur radius for the `blur` effect; the list is consumed every frame.
    pub leash: crate::leash::LeashRenderer,
    post_fx: crate::post_fx::PostFx,
    pub post_effects: Vec<crate::post_fx::Effect>,
    pub post_blur: i32,
    /// Writes of the render buffers (shared staging, `upload.rs`).
    up: Uploader,
    /// End-of-frame `device.poll` removed (`RUSTCRAFT_OPT_NOPOLL`, default 1): the read
    /// callbacks arrive at the next submit.
    no_poll: bool,
    /// Cameras of the world, hand, totem and interface (slots `CAM_*`).
    cameras: upload::Slots<4>,
    bind: wgpu::BindGroup,
    /// Projection of the frame (field of view, shake and sway, hand), set by the app.
    pub view: crate::camera::RenderView,
    /// Camera of the hand pass (same binding as `bind`, own projection).
    hand_bind: wgpu::BindGroup,
    /// Camera of the interface 3D models (orthographic projection, `special.rs`).
    gui_bind: wgpu::BindGroup,
    /// Totem animation: projection of the hand alone, pose in the view frame.
    totem_bind: wgpu::BindGroup,
    /// Block textures (main, large), for the overlays.
    block_views: (wgpu::TextureView, wgpu::TextureView),
    /// Screen effects and camera overlays (`overlays.rs`).
    pub overlays: Option<crate::overlays::Overlays>,
    /// Portal and nausea distortion, after the shake and sway.
    pub distort: glam::Mat4,
    column_layout: wgpu::BindGroupLayout,
    columns: HashMap<(i32, i32), ColumnGpu>,
    /// Columns meshed without any face (End void, empty superflat): only their
    /// connectivity, so that the walk crosses them like the game's empty sections.
    empty_vis: HashMap<(i32, i32), Arc<[rc_mesh::visibility::Vis]>>,
    /// Connectivity of all the columns for the walk, redone only when `vis_rev`
    /// changes (a moving camera reuses the same copy).
    vis_snapshot: Option<(u64, VisSnapshot)>,
    /// Layer displayed per texture (animations), CPU-side copy and GPU buffer.
    anim: wgpu::Buffer,
    frames: Vec<[u32; 2]>,
    /// Model layer → real layer (bit `BIG_LAYER`: second array; `u32::MAX`: absent).
    remap: Vec<u32>,
    /// Occlusion sorting of the sections (setting, `RUSTCRAFT_OCCLUSION=0` to turn it off).
    pub occlusion: bool,
    /// CPU duration of the visibility sort of the last frame.
    pub vis_ms: f64,
    /// Buffers of the walk, absent while a background task uses them.
    graph: Option<rc_mesh::visibility::Graph>,
    /// Fixed GPU sizes (baked faces, textures with mips), for the memory report.
    quads_bytes: u64,
    atlas_bytes: u64,
    /// Directional shading of the current dimension.
    pub shade: [f32; 6],
    regions: HashMap<(i32, i32), RegionGpu>,
    /// Appearance fade: births, columns fading in, ring (`terrain_shader::FadeRing`).
    fade_ring: crate::terrain_shader::FadeRing,
    /// Duration of the appearance fade (`chunkSectionFadeInTime`, 0 = no fade).
    pub fade_secs: f64,
    fade: wgpu::Buffer,
    lightmap: wgpu::Texture,
    /// Commands of the last frame and what they depend on: as long as camera, walk and
    /// columns are unchanged, they are identical and reused as is.
    draw_key: Option<DrawKey>,
    /// Series, batches and commands of the frame, cache of the column order (`lists`).
    lists: lists::Lists,
    drawn_cache: (usize, u64),
    /// Instanced and indexed commands as written in their GPU buffer
    /// (`RUSTCRAFT_OPT_ARGSKIP`: only the changed range is rewritten).
    sent_args: Vec<DrawArgs>,
    sent_iargs: Vec<DrawIndexedArgs>,
    /// Radius of the walk and its key (version of the columns, camera column).
    radius_cache: Option<(RadiusKey, i32)>,
    /// Grid of the sections reached by the last walk to arrive (`RUSTCRAFT_OPT_REACHGRID`).
    reached_grid: Option<lists::ReachGrid>,
    /// Number of the current walk result (`reached`).
    reached_gen: u64,
    /// Copy of the walk nodes for F3 (generation of the copied walk), made only
    /// when an occlusion visualisation is active.
    occ_nodes: (u64, Vec<GraphNode>),
    /// Occlusion visualisation active at the previous frame (set by the F3 traces).
    pub occ_debug: bool,
    /// Sections reached (generation, bottom of the world, number of sections, set) for the
    /// visibility of the entities.
    reached_set: ReachedSet,
    /// Translucent quads of all the columns and their draw order, in a single pair of
    /// buffers: the translucent pass, ordered from the farthest to the nearest, has only one
    /// binding and one batch (each quad carries its absolute column, origin (0, 0)).
    trans: RegionGpu,
    /// Formats of the explicit quads (`GpuFormats`).
    formats: GpuFormats,
    /// Table of the compact translucents' columns and free slots (`free_slots`, then
    /// `next_slot` never served).
    trans_slots: wgpu::Buffer,
    free_slots: Vec<u16>,
    next_slot: usize,
    /// Translucent sort: last camera, bottom of the world, camera block, rotation cursor.
    sort_eye: Option<[f64; 3]>,
    sort_min_y: i32,
    /// Re-sorts all the translucent columns, even those whose order does not change the image.
    sort_all: bool,
    /// Contiguous indirect commands kept separate (`RUSTCRAFT_SPLITDRAWS=1`, comparison).
    split_draws: bool,
    /// Entities tested against the field of view only, not the walk (`RUSTCRAFT_ALLENTITIES=1`).
    all_entities: bool,
    sort_block: Option<[i32; 3]>,
    sort_cursor: usize,
    /// Translucent columns (sorted by key), version of the columns at their computation, and number of
    /// columns not yet sorted for the camera's current block.
    trans_keys: Vec<(i32, i32)>,
    trans_keys_rev: Option<u64>,
    unsorted: usize,
    /// Buffers of the per-section re-sort (`RUSTCRAFT_OPT_TRANSPOV`): order of a run of sections, sorts.
    sort_scratch: TransScratch,
    /// Buffer of the frame's indirect draw commands, and whether the GPU accepts them.
    indirect: wgpu::Buffer,
    indirect_ok: bool,
    /// Variants of the terrain shader (`terrain_shader`).
    shader_opts: crate::terrain_shader::ShaderOpts,
    /// Indexed faces (`RUSTCRAFT_OPT_FACEIDX`): shared indices, identity buffer and its size,
    /// the frame's indexed commands, their buffer, instanced commands of the fluids alone (those
    /// sent to the GPU), and (first, count) of each batch in the list of its kind.
    face_index: wgpu::Buffer,
    face_ids: (wgpu::Buffer, u32),
    iargs: Vec<DrawIndexedArgs>,
    fargs: Vec<DrawArgs>,
    indirect_idx: wgpu::Buffer,
    ibatches: Vec<(u32, u32)>,
    graph_tx: crossbeam_channel::Sender<GraphResult>,
    graph_rx: crossbeam_channel::Receiver<GraphResult>,
    /// Last walk result and its key (exact position, version of the columns, settings).
    reached: Vec<(i32, i32, i32)>,
    reached_key: Option<([i64; 3], u64, bool, i32, i32)>,
    columns_rev: u64,
    /// Version of the sections' connectivity (column appeared, disappeared, or one of whose sections
    /// connects differently): the only walk key on the column side (hotspot no. 7).
    vis_rev: u64,
    /// Inputs of the last walk launched (check `RUSTCRAFT_MESHCHECK`).
    graph_args: Option<([f64; 3], i32, i32, i32, bool)>,
    /// Duration of the last walk actually redone.
    pub graph_ms: f64,
    /// Sum and count of the CPU durations per frame since the last report.
    cpu_ms_sum: f64,
    cpu_frames: u32,
    timer: Option<crate::gpu_time::GpuTimer>,
    pub adapter_name: String,
    pub wireframe: bool,
    capture: Option<std::path::PathBuf>,
    can_capture: bool,
    /// Faces and columns actually drawn in the last frame.
    pub drawn: (usize, u64),
    /// Improved transparency requested (option); effective if the GPU blends RGBA32F.
    pub improved_transparency: bool,
    oit_ok: bool,
    oit: Option<crate::oit::Oit>,
    /// Translucent terrain in improved transparency: block faces, sorted fluids.
    terrain_oit: Option<[crate::oit::Set; 2]>,
}

fn depth_view(device: &wgpu::Device, w: u32, h: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            // Read by the depth copies of the improved transparency.
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

fn uniform_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let storage = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            storage(1),
            storage(2),
            storage(3),
            storage(4),
        ],
    })
}

/// Planes of the view frustum (camera frame), normals pointing inwards.
fn frustum_planes(m: Mat4) -> [Vec4; 6] {
    let r = [m.row(0), m.row(1), m.row(2), m.row(3)];
    // Reversed [0, 1] depth: far plane = row 2 alone, near plane = row 3 − row 2.
    [r[3] + r[0], r[3] - r[0], r[3] + r[1], r[3] - r[1], r[2], r[3] - r[2]]
}

fn aabb_visible(planes: &[Vec4; 6], min: Vec3, max: Vec3) -> bool {
    planes.iter().all(|p| {
        // Most advanced corner in the direction of the normal.
        let v = Vec3::new(
            if p.x >= 0.0 { max.x } else { min.x },
            if p.y >= 0.0 { max.y } else { min.y },
            if p.z >= 0.0 { max.z } else { min.z },
        );
        p.truncate().dot(v) + p.w >= 0.0
    })
}

/// Box entirely in the field (its least advanced corner is on the right side of each plane):
/// any box it contains is too.
fn aabb_inside(planes: &[Vec4; 6], min: Vec3, max: Vec3) -> bool {
    planes.iter().all(|p| {
        let v = Vec3::new(
            if p.x >= 0.0 { min.x } else { max.x },
            if p.y >= 0.0 { min.y } else { max.y },
            if p.z >= 0.0 { min.z } else { max.z },
        );
        p.truncate().dot(v) + p.w >= 0.0
    })
}

impl Renderer {
    /// "Box in the field of view" test of the frame (frame relative to the camera's integer origin),
    /// same projection as the render.
    pub fn visibility(
        &self,
        eye: [f64; 3],
        yaw: f32,
        pitch: f32,
        far: f32,
    ) -> impl Fn([f32; 3], [f32; 3]) -> bool + use<> {
        let (origin, _, view_m) = crate::camera::view_matrix(eye, yaw, pitch);
        let aspect = self.config.width as f32 / self.config.height.max(1) as f32;
        let proj = perspective(self.view.cull_fov, aspect, 0.05, far);
        let planes = frustum_planes(proj * view_m);
        let (_, min_y, sections, reached) = self.reached_set.clone();
        let occlusion = self.occlusion && !self.all_entities;
        move |min, max| {
            if !aabb_visible(&planes, Vec3::from(min), Vec3::from(max)) {
                return false;
            }
            if !occlusion || reached.is_empty() {
                return true;
            }
            // Occlusion walk (`visibility.md`): visible if one of the sections touched by the
            // box (+ 1 block) was reached; outside the world in height: kept.
            let cell = |v: f32, o: i32| (v.floor() as i32 + o).div_euclid(16);
            let (x0, x1) = (cell(min[0] - 1.0, origin[0]), cell(max[0] + 1.0, origin[0]));
            let (z0, z1) = (cell(min[2] - 1.0, origin[1]), cell(max[2] + 1.0, origin[1]));
            let (y0, y1) = (cell(min[1] - 1.0, -min_y), cell(max[1] + 1.0, -min_y));
            if y0 < 0 || y1 >= sections {
                return true;
            }
            (x0..=x1).any(|x| (z0..=z1).any(|z| (y0..=y1).any(|y| reached.contains(x, y, z))))
        }
    }

    pub fn new(
        window: Arc<Window>,
        models: &BlockModels,
        textures: &[Vec<rc_assets::texture::Image>],
        sky_assets: &crate::sky::SkyAssets,
        gui_assets: crate::gui::GuiAssets,
        entity_assets: Option<(Arc<rc_data::VersionData>, crate::entity_render::EntityTextures)>,
        tables: &rc_mesh::blocks::Tables,
    ) -> Self {
        let formats = init_gpu_formats(tables);
        // Order of the APIs according to the "graphics API" setting read at startup (Vulkan first by
        // default, arch § 8); RUSTCRAFT_BACKEND=vulkan|dx12 to force.
        let crate::gpu_backend::Picked { surface, adapter, device, queue } = crate::gpu_backend::pick(&window);
        let info = adapter.get_info();
        // Features retained when the device was opened (`gpu_backend::open_device`).
        let features = device.features();
        let wireframe = features.contains(wgpu::Features::POLYGON_MODE_LINE);
        // Wireframe on demand (RUSTCRAFT_WIRE=1), otherwise solid rendering.
        let wire_mode = wireframe && rc_data::env_str!("RUSTCRAFT_WIRE") == Some("1");
        let timestamps = features.contains(wgpu::Features::TIMESTAMP_QUERY);
        let indirect_ok = features.contains(wgpu::Features::INDIRECT_FIRST_INSTANCE);
        let oit_ok = features.contains(wgpu::Features::FLOAT32_BLENDABLE);
        eprintln!(
            "GPU features: indirect draw {}, timestamp {}, wireframe {}",
            if indirect_ok { "yes" } else { "no (one draw per series)" },
            if timestamps { crate::gpu_time::GpuTimer::mode_for(features).describe() } else { "no (GPU time absent)" },
            if wireframe { "yes" } else { "no" },
        );

        let size = window.inner_size();
        let mut config = surface.get_default_config(&adapter, size.width.max(1), size.height.max(1)).expect("format");
        // Like the game: colours computed directly on the gamma values (neither texture nor
        // sRGB surface), otherwise tints and shading come out washed out.
        let caps = surface.get_capabilities(&adapter);
        if let Some(f) = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb() && f.remove_srgb_suffix() == config.format.remove_srgb_suffix())
        {
            config.format = f;
        }
        let can_capture = surface.get_capabilities(&adapter).usages.contains(wgpu::TextureUsages::COPY_SRC);
        if can_capture {
            config.usage |= wgpu::TextureUsages::COPY_SRC;
        }
        // Vertical sync: setting `enableVsync` (set by `set_vsync`); `RUSTCRAFT_VSYNC=0|1`
        // forces it for measurements. Without forcing, the window starts without sync until the setting.
        let (requested, present_mode) =
            crate::video::present_modes(&caps.present_modes, crate::video::vsync_forced().unwrap_or(false));
        config.present_mode = requested;
        crate::present_watch::during(Stage::Configure, || surface.configure(&device, &config));
        eprintln!("presentation: {present_mode:?} (surface: {:?})", caps.present_modes);
        let present_modes_caps = caps.present_modes.clone();

        let shader_opts = crate::terrain_shader::opts();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terrain"),
            source: wgpu::ShaderSource::Wgsl(crate::terrain_shader::terrain(shader_opts).into()),
        });
        let cameras = upload::Slots::new(
            &device,
            ["camera", "hand camera", "totem animation camera", "interface model camera"],
            upload::merged_on(),
            upload::skip_on(),
        );
        let up = Uploader::new(&device, upload::ring_on());
        // Baked faces of all the models (read by the shader).
        let quads: Vec<QuadGpu> = models.quads.iter().map(quad_gpu).collect();
        let quads = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("baked faces"),
            contents: bytemuck::cast_slice(&quads),
            usage: wgpu::BufferUsages::STORAGE,
        });
        // Block textures: two 2D arrays with mipmaps, one at the main size, the other
        // for the rare larger textures (bit 15 of the layer). Beyond the GPU limit,
        // the surplus images are dropped (the texture stays fixed).
        // Offsets of the compact face models (`offset_table`).
        let offsets = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("model offsets"),
            contents: bytemuck::cast_slice(&tables.offset_table()),
            usage: wgpu::BufferUsages::STORAGE,
        });
        // Absolute columns of the compact translucents' slots.
        let trans_slots = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("translucent columns"),
            size: if formats.trans_compact { TRANS_SLOTS as u64 * 8 } else { 8 },
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let quads_bytes = quads.size() + offsets.size() + trans_slots.size();
        let max_layers = device.limits().max_texture_array_layers;
        let (main_size, big_size) = (models.texture_size, models.big_texture_size);
        let mut remap: Vec<u32> = Vec::with_capacity(textures.len());
        let (mut main_layers, mut big_layers): (Vec<&Vec<rc_assets::texture::Image>>, Vec<_>) =
            (Vec::new(), Vec::new());
        for t in textures {
            let big = big_size > 0 && t[0].width == big_size;
            let list = if big { &mut big_layers } else { &mut main_layers };
            if (list.len() as u32) < max_layers {
                remap.push(list.len() as u32 | if big { BIG_LAYER } else { 0 });
                list.push(t);
            } else {
                remap.push(u32::MAX);
            }
        }
        let make_array = |label: &str, size: u32, list: &[&Vec<rc_assets::texture::Image>]| {
            let size = size.max(1);
            let mips = size.ilog2() + 1;
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: list.len().max(1) as u32 },
                mip_level_count: mips,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            // Mip levels computed at baking, like the game (rc_assets::mip).
            let mut bytes = 0u64;
            for (layer, chain) in list.iter().enumerate() {
                for (level, data) in chain.iter().map(|i| &i.rgba).enumerate().take(mips as usize) {
                    let s = (size >> level).max(1);
                    bytes += data.len() as u64;
                    queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: &texture,
                            mip_level: level as u32,
                            origin: wgpu::Origin3d { x: 0, y: 0, z: layer as u32 },
                            aspect: wgpu::TextureAspect::All,
                        },
                        data,
                        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(s * 4), rows_per_image: Some(s) },
                        wgpu::Extent3d { width: s, height: s, depth_or_array_layers: 1 },
                    );
                }
            }
            let view = texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            });
            (view, bytes)
        };
        let (atlas_view, main_bytes) = make_array("block textures", main_size, &main_layers);
        let (big_view, big_bytes) = make_array("large block textures", big_size.max(1), &big_layers);
        crate::boot::mark("block textures written (write_texture)");
        let atlas_bytes = main_bytes + big_bytes;
        let sampler = crate::video::atlas_sampler(&device, 4, 1);
        let camera_layout = camera_layout(&device);
        // Animation: per texture, (displayed layer | next layer << 16, fade in thousandths).
        let layer_of = |i: u32| remap.get(i as usize).copied().filter(|&l| l != u32::MAX).unwrap_or(0);
        let frames: Vec<[u32; 2]> =
            (0..models.texture_names.len().max(1) as u32).map(|i| [layer_of(i) | layer_of(i) << 16, 0]).collect();
        let anim = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("texture animation"),
            contents: bytemuck::cast_slice(&frames),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        let column_layout = uniform_layout(&device);
        // Fade ring: visibilities (1 = visible) or birth instants (finished = sentinel).
        let fade_ring = crate::terrain_shader::FadeRing::new(shader_opts.fadegpu, Instant::now());
        let fade = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("appearance fade"),
            contents: bytemuck::cast_slice(fade_ring.ring()),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
        // Indexed faces: shared indices [0, 1, 2, 0, 2, 3] + 4 × face, and identity buffer (the
        // first face of a command arrives through its instance attribute).
        let face_index = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("face indices"),
            contents: bytemuck::cast_slice(&face_indices(INDEXED_FACES)),
            usage: wgpu::BufferUsages::INDEX,
        });
        let face_ids = identity_buffer(&device, FACE_IDS_START);
        // Lightmap 16 × 16 (column = block, row = sky), white until the first computation.
        let lightmap = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("lightmap"),
            size: wgpu::Extent3d { width: 16, height: 16, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let lightmap_view = lightmap.create_view(&Default::default());
        let lightmap_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("lightmap"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        // Camera binding resources, kept to redo it when the sampler changes.
        let terrain_res = crate::video::TerrainRes {
            layout: camera_layout.clone(),
            quads: quads.clone(),
            atlas: atlas_view.clone(),
            big: big_view.clone(),
            anim: anim.clone(),
            fade: fade.clone(),
            lightmap: lightmap_view.clone(),
            lightmap_sampler: lightmap_sampler.clone(),
            offsets,
            key: (4, 1),
        };
        let make_bind = |i: usize| terrain_res.bind(&device, cameras.binding(i), &sampler);
        let bind = make_bind(CAM_WORLD);
        let hand_bind = make_bind(CAM_HAND);
        let gui_bind = make_bind(CAM_GUI);
        let totem_bind = make_bind(CAM_TOTEM);
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&camera_layout), Some(&column_layout)],
            ..Default::default()
        });
        let make = |label: &str, vs: &str, fs: &str, blend: Option<wgpu::BlendState>, depth_write: bool| {
            let instance_layout = instance_layout(vs);
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &[Some(instance_layout)],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: config.format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                // Back faces removed, like the game's terrain pipelines (water seen from below
                // has its own back face emitted by the mesher).
                primitive: wgpu::PrimitiveState {
                    // Block faces as a 4-vertex strip (`RUSTCRAFT_OPT_STRIP`).
                    topology: if vs == "vs" { shader_opts.topology() } else { wgpu::PrimitiveTopology::TriangleList },
                    polygon_mode: if wire_mode { wgpu::PolygonMode::Line } else { wgpu::PolygonMode::Fill },
                    cull_mode: Some(wgpu::Face::Back),
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(depth_write),
                    // Ties accepted, write also for translucent: like the game ("≥" in
                    // reversed depth).
                    depth_compare: Some(DEPTH_NEARER),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let face_vs = shader_opts.face_entry();
        let pipelines = [
            make("opaque terrain", face_vs, "fs_solid", None, true),
            make("cutout terrain", face_vs, "fs_cutout", None, true),
            make("translucent terrain", face_vs, "fs_translucent", Some(wgpu::BlendState::ALPHA_BLENDING), true),
        ];
        let (vs_sorted, vs_fluid) = formats.entries();
        let sorted_pipeline =
            make("sorted translucent", vs_sorted, "fs_translucent", Some(wgpu::BlendState::ALPHA_BLENDING), true);
        let fluid_pipelines = [
            make("opaque fluid", vs_fluid, "fs_solid", None, true),
            make("cutout fluid", vs_fluid, "fs_cutout", None, true),
            make("translucent fluid", vs_fluid, "fs_translucent", Some(wgpu::BlendState::ALPHA_BLENDING), true),
        ];
        // Flat sky: fans generated in the shader, neither buffer nor depth.
        let sky_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&camera_layout)],
            ..Default::default()
        });
        let sky_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sky"),
            layout: Some(&sky_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_sky"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_sky"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let timer = timestamps.then(|| {
            let mode = crate::gpu_time::GpuTimer::mode_for(features);
            crate::gpu_time::GpuTimer::new(&device, &queue, mode, crate::gpu_time::settings().passes)
        });
        let (graph_tx, graph_rx) = crossbeam_channel::unbounded();
        let trans = RegionGpu::new(&device, &column_layout, (0, 0), formats.trans_bytes(), &trans_slots);
        let sky = crate::sky::Sky::new(&device, &queue, config.format, sky_assets);
        let gui = crate::gui::Gui::new(&device, &queue, config.format, gui_assets);
        let blur = crate::blur::Blur::new(&device, config.format);
        let leash = crate::leash::LeashRenderer::new(&device, &camera_layout, config.format);
        let post_fx = crate::post_fx::PostFx::new(&device, config.format);
        let tags = crate::nametag::NameTags::new(&device, &camera_layout, config.format);
        let block_text = crate::block_text::BlockText::new(&device, &camera_layout, config.format);
        let text_displays = crate::text_display::TextDisplays::new(&device, &camera_layout, config.format);
        let block_fx = crate::block_fx::BlockFx::new(&device, &camera_layout, config.format);
        let outline = crate::outline::Outline::new(&device, &camera_layout, config.format);
        let entity_outline = crate::entity_outline::EntityOutline::new(&device, config.format);
        let cracks = crate::crack::Cracks::new(&device, &camera_layout, config.format);
        let debug_lines = crate::debug_render::DebugLines::new(&device, &camera_layout, config.format);
        // Block fragments: layers of the main array only.
        let particle_layers =
            remap.iter().map(|&l| if l == u32::MAX || l & BIG_LAYER != 0 { u32::MAX } else { l }).collect();
        let particles =
            crate::particles::ParticleRenderer::new(&device, &camera_layout, config.format, particle_layers);
        let entities = entity_assets.map(|(data, tex)| {
            crate::entity_render::EntityRenderer::new(&device, &camera_layout, config.format, data, tex)
        });
        let weather = crate::weather_render::WeatherRenderer::new(&device, &camera_layout, config.format);
        Renderer {
            weather,
            anim,
            frames,
            remap,
            occlusion: rc_data::env_str!("RUSTCRAFT_OCCLUSION") != Some("0"),
            vis_ms: 0.0,
            graph: Some(Default::default()),
            quads_bytes,
            atlas_bytes,
            shade: rc_mesh::wire::SHADE_DEFAULT,
            regions: HashMap::new(),
            fade_ring,
            fade_secs: FADE_SECS,
            fade,
            lightmap,
            draw_key: None,
            lists: lists::Lists::default(),
            drawn_cache: (0, 0),
            sent_args: Vec::new(),
            sent_iargs: Vec::new(),
            radius_cache: None,
            reached_grid: None,
            reached_gen: 0,
            occ_nodes: (u64::MAX, Vec::new()),
            occ_debug: false,
            reached_set: (0, 0, 0, Arc::default()),
            trans,
            formats,
            trans_slots,
            free_slots: Vec::new(),
            next_slot: 0,
            sort_eye: None,
            sort_min_y: 0,
            sort_all: sort_all(),
            split_draws: rc_data::env_flag!("RUSTCRAFT_SPLITDRAWS"),
            all_entities: rc_data::env_flag!("RUSTCRAFT_ALLENTITIES"),
            sort_block: None,
            sort_cursor: 0,
            trans_keys: Vec::new(),
            trans_keys_rev: None,
            unsorted: 0,
            sort_scratch: TransScratch::default(),
            indirect: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("indirect commands"),
                size: 1 << 16,
                usage: wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            indirect_ok,
            shader_opts,
            face_index,
            face_ids: (face_ids, FACE_IDS_START),
            iargs: Vec::new(),
            fargs: Vec::new(),
            indirect_idx: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("indirect indexed commands"),
                size: 1 << 16,
                usage: wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            ibatches: Vec::new(),
            graph_tx,
            graph_rx,
            reached: Vec::new(),
            reached_key: None,
            columns_rev: 0,
            vis_rev: 0,
            graph_args: None,
            graph_ms: 0.0,
            cpu_ms_sum: 0.0,
            cpu_frames: 0,
            depth: depth_view(&device, config.width, config.height),
            window,
            surface,
            device,
            queue,
            config,
            pipelines,
            fluid_pipelines,
            sorted_pipeline,
            sky_pipeline,
            sky,
            gui,
            entities,
            tags,
            block_text,
            text_displays,
            block_fx,
            outline,
            entity_outline,
            cracks,
            debug_lines,
            adapter_info: info.clone(),
            present_mode,
            warned_surface: false,
            present_modes_caps,
            terrain_res,
            rgss: false,
            panorama: None,
            show_panorama: false,
            blur,
            leash,
            post_fx,
            post_effects: Vec::new(),
            post_blur: 5,
            particles,
            up,
            no_poll: rc_data::env_str!("RUSTCRAFT_OPT_NOPOLL") != Some("0"),
            cameras,
            bind,
            view: crate::camera::RenderView::default(),
            hand_bind,
            gui_bind,
            totem_bind,
            block_views: (atlas_view.clone(), big_view.clone()),
            overlays: None,
            distort: glam::Mat4::IDENTITY,
            column_layout,
            columns: HashMap::new(),
            empty_vis: HashMap::new(),
            vis_snapshot: None,
            timer,
            adapter_name: format!("{} ({:?})", info.name, info.backend),
            wireframe,
            capture: None,
            can_capture,
            drawn: (0, 0),
            improved_transparency: false,
            oit_ok,
            oit: None,
            terrain_oit: None,
        }
    }

    pub fn resize(&mut self, w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        self.config.width = w;
        self.config.height = h;
        crate::present_watch::during(Stage::Configure, || self.surface.configure(&self.device, &self.config));
        self.depth = depth_view(&self.device, w, h);
        // Improved transparency targets bound to the old depth: redone as needed.
        if let Some(o) = &mut self.oit {
            o.release();
        }
    }

    /// Sampler of the block textures: mip levels (`mipmapLevels`) and anisotropy
    /// (1 = none); the three camera bindings are redone if the key changes.
    pub fn set_atlas_sampler(&mut self, mips: u32, aniso: u16) {
        let aniso = aniso.clamp(1, 16);
        if self.terrain_res.key == (mips, aniso) {
            return;
        }
        self.terrain_res.key = (mips, aniso);
        let s = crate::video::atlas_sampler(&self.device, mips, aniso);
        let bind = |i| self.terrain_res.bind(&self.device, self.cameras.binding(i), &s);
        (self.bind, self.hand_bind, self.gui_bind, self.totem_bind) =
            (bind(CAM_WORLD), bind(CAM_HAND), bind(CAM_GUI), bind(CAM_TOTEM));
        // Commands reused from the previous frame: invalidated.
        self.columns_rev += 1;
    }

    /// Vertical sync (`enableVsync`): surface reconfigured on the fly if the mode changes,
    /// like the game; `RUSTCRAFT_VSYNC=0|1` overrides the setting.
    pub fn set_vsync(&mut self, on: bool) {
        let (requested, effective) =
            crate::video::present_modes(&self.present_modes_caps, crate::video::vsync_forced().unwrap_or(on));
        if requested != self.config.present_mode {
            self.config.present_mode = requested;
            self.present_mode = effective;
            crate::present_watch::during(Stage::Configure, || self.surface.configure(&self.device, &self.config));
            eprintln!("presentation: {effective:?}");
        }
    }

    /// Tick animation frames: (texture, displayed layer, next layer, fade ‰).
    pub fn animate(&mut self, steps: impl Iterator<Item = (u32, u32, u32, u32)>) {
        let mut changed = false;
        for (tex, cur, next, milli) in steps {
            let map = |l: u32| self.remap.get(l as usize).copied().filter(|&v| v != u32::MAX);
            let ok = |l: u32| map(l).or_else(|| map(tex)).unwrap_or(0);
            let v = [ok(cur) | ok(next) << 16, milli];
            if let Some(e) = self.frames.get_mut(tex as usize)
                && *e != v
            {
                *e = v;
                changed = true;
            }
        }
        if changed {
            self.up.write(&self.queue, &self.anim, 0, bytemuck::cast_slice(&self.frames));
        }
    }

    /// Appearance fade (0 to 1) of the already meshed column `(x, z)`, `None` before.
    pub fn column_visibility(&self, x: i32, z: i32) -> Option<f32> {
        self.fade_ring.visibility((x, z), self.fade_secs)
    }

    /// Uploads the mesh of a column. `prep`: bytes already prepared by the pool (otherwise made
    /// here); `in_place`: partial remesh of the same size as its base (version, changed
    /// ranges), written into the current place if the GPU still holds that base.
    pub fn upload(&mut self, m: &BlockMesh, prep: Option<Prepared>, in_place: Option<(u64, rc_mesh::blocks::InPlace)>) {
        if self.columns.get(&(m.x, m.z)).is_some_and(|c| c.version > m.version) {
            return;
        }
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
        let p = if empty {
            None
        } else {
            Some(prep.unwrap_or_else(|| prepare_legacy(m, crate::remesh::opts().transfree, formats)))
        };
        let sort_all = self.sort_all;
        let keep = p
            .as_ref()
            .and_then(|p| in_place_writes(old.as_ref().map(ColumnGpu::slot), p, !sort_all && p.free, in_place));
        let old_slot = old.as_ref().and_then(|o| o.trans_slot);
        if let Some(old) = old
            && keep.is_none()
        {
            self.release(&old);
        }
        self.fade_ring.birth((m.x, m.z), Instant::now());
        self.columns_rev += 1;
        let Some(mut p) = p else {
            self.empty_vis.insert((m.x, m.z), m.vis.as_slice().into());
            return;
        };
        // Compact translucents: the column's slot (kept in place, otherwise taken), its
        // absolute column written into the table, and set in each quad.
        let trans_slot = if formats.trans_compact && p.trans_len() != 0 {
            let s = match keep.as_ref().and(old_slot) {
                Some(s) => s,
                None => self.take_slot(),
            };
            self.up.write(&self.queue, &self.trans_slots, u64::from(s) * 8, bytemuck::cast_slice(&[m.x, m.z]));
            p.set_slot(s);
            Some(s)
        } else {
            None
        };
        let sent = keep.as_ref().map_or_else(|| p.bytes(), |k| k.writes.iter().map(|(.., r)| r.len()).sum());
        crate::perf_log::upload_bytes(sent, keep.is_some());
        let (fluid_n, trans_n) = (p.fluid_len() as u32, p.trans_len() as u32);
        let Prepared { faces: all, counts, fluids: fluid_all, fluid_counts, trans: trans_all, centers, free, .. } = p;
        let (fluid_bytes, trans_bytes) = (formats.fluid_bytes(), formats.trans_bytes());
        let trans_starts = m.fluid_starts[2].clone();
        let key = (m.x.div_euclid(REGION), m.z.div_euclid(REGION));
        let (device, queue, layout, slots) = (&self.device, &self.queue, &self.column_layout, &self.trans_slots);
        let up = &mut self.up;
        let region = self.regions.entry(key).or_insert_with(|| RegionGpu::new(device, layout, key, fluid_bytes, slots));
        let trans = &mut self.trans;
        // Column whose order does not change the image: mesh order, never re-sorted, no centre
        // kept (`RUSTCRAFT_SORTALL=1` re-sorts everything, for comparison).
        let free = !self.sort_all && free;
        let eye = self.sort_eye.filter(|_| !free).map(|e| eye_in_column(e, (m.x, m.z), self.sort_min_y));
        let order_of = |ts: u32| {
            if centers.len() == trans_n as usize {
                translucent_order(&centers, &trans_starts, eye, ts)
            } else {
                (ts..ts + trans_n).collect()
            }
        };
        let (fs, ls, ts, os, order_len);
        if let Some(k) = keep {
            // In place: only the changed ranges go out; the order is redone (it depends on
            // the exact eye of the last sort), except an unchanged identity order.
            let s = k.slot;
            (fs, ls, ts, os) = (s.faces.0, s.fluids.0, s.trans.0, s.order.0);
            for (t, at, r) in k.writes {
                let (buf, b): (&wgpu::Buffer, &[u8]) = match t {
                    Target::Faces => (&region.faces, bytemuck::cast_slice(&all)),
                    Target::Fluids => (&region.fluids, bytemuck::cast_slice(&fluid_all)),
                    Target::Trans => (&trans.fluids, bytemuck::cast_slice(&trans_all)),
                };
                up.write(queue, buf, at, &b[r]);
            }
            order_len = trans_n;
            if k.order {
                let order = order_of(ts);
                up.write(queue, &trans.order, u64::from(os) * ORDER_BYTES, bytemuck::cast_slice(&order));
            }
        } else {
            let faces_size = region.faces.size();
            fs = alloc_in(device, queue, up, &mut region.faces, &mut region.face_arena, all.len() as u32, FACE_BYTES);
            up.write(queue, &region.faces, u64::from(fs) * FACE_BYTES, bytemuck::cast_slice(&all));
            let fluids_size = region.fluids.size();
            ls = alloc_in(device, queue, up, &mut region.fluids, &mut region.fluid_arena, fluid_n, fluid_bytes);
            if region.fluids.size() != fluids_size || region.faces.size() != faces_size {
                region.bind = region_bind(device, layout, &region.pos, &region.fluids, slots, &region.faces);
            }
            if !fluid_all.is_empty() {
                up.write(queue, &region.fluids, u64::from(ls) * fluid_bytes, bytemuck::cast_slice(&fluid_all));
            }
            let trans_size = trans.fluids.size();
            ts = alloc_in(device, queue, up, &mut trans.fluids, &mut trans.fluid_arena, trans_n, trans_bytes);
            if trans.fluids.size() != trans_size {
                trans.bind = region_bind(device, layout, &trans.pos, &trans.fluids, slots, &trans.faces);
            }
            if !trans_all.is_empty() {
                up.write(queue, &trans.fluids, u64::from(ts) * trans_bytes, bytemuck::cast_slice(&trans_all));
            }
            let order = order_of(ts);
            order_len = order.len() as u32;
            os = alloc_in(device, queue, up, &mut trans.order, &mut trans.order_arena, order_len, ORDER_BYTES);
            if !order.is_empty() {
                up.write(queue, &trans.order, u64::from(os) * ORDER_BYTES, bytemuck::cast_slice(&order));
            }
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
        let order_starts: Vec<u32> = trans_starts.iter().map(|&r| os + r).collect();
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
        let (ranges, starts) = place(fs, counts, &m.starts);
        let (mut fluid_ranges, mut fluid_starts) = place(ls, fluid_counts, &m.fluid_starts);
        fluid_ranges[2] = ts..ts + trans_n;
        fluid_starts[2] = m.fluid_starts[2].iter().map(|&r| ts + r).collect();
        let column = ColumnGpu {
            digest: if crate::remesh::checking() { crate::remesh::digest(m) } else { 0 },
            region: key,
            face_alloc: (fs, all.len() as u32),
            fluid_alloc: (ls, fluid_n),
            trans_alloc: (ts, trans_n),
            ranges,
            fluid_ranges,
            starts,
            fluid_starts,
            vis: m.vis.as_slice().into(),
            trans_centers,
            trans_starts,
            order_alloc: (os, order_len),
            order_starts,
            sorted_at,
            povs,
            version: m.version,
            trans_slot,
        };
        self.columns.insert((m.x, m.z), column);
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

    /// The format of the region fluids depends on the compact offset faces (`GpuFormats`).
    pub fn fluid96(&self) -> bool {
        self.formats.fluid96
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

    /// Records the next frame in `path` (32-bit BMP).
    /// Enables the title screen panorama (test: drawn over the world).
    pub fn enable_panorama(&mut self, assets: &crate::panorama::PanoramaAssets) {
        self.panorama = Some(crate::panorama::Panorama::new(&self.device, &self.queue, self.config.format, assets));
    }

    /// Screen effects and camera overlays (`overlays.rs`).
    pub fn enable_overlays(
        &mut self,
        assets: Arc<rc_assets::Assets>,
        models: Arc<BlockModels>,
        data: Arc<rc_data::VersionData>,
    ) {
        let views = (&self.block_views.0, &self.block_views.1);
        let o =
            crate::overlays::Overlays::new(&self.device, &self.queue, self.config.format, views, assets, models, data);
        self.overlays = Some(o);
    }

    /// Vertices of the frame's overlays (current layers of the animated block textures).
    pub fn prepare_overlays(&mut self, f: &crate::overlays::Frame<'_>) {
        let (frames, size, scale) = (&self.frames, (self.config.width, self.config.height), self.gui.scale);
        if let Some(o) = &mut self.overlays {
            o.prepare(&self.device, &self.queue, f, size, scale, |t| frames.get(t as usize).map(|v| v[0] & 0xffff));
        }
    }

    /// Textures of the beams and End portals.
    pub fn load_block_fx(&mut self, assets: &rc_assets::Assets) {
        let load = |p: &str| {
            assets
                .read(&format!("assets/minecraft/textures/{p}.png"))
                .and_then(|b| rc_assets::texture::Image::decode_png(&b))
                .map(|i| i.first_frame())
                .unwrap_or_else(|| rc_assets::texture::Image { width: 1, height: 1, rgba: vec![255; 4] })
        };
        let imgs = [
            load("entity/beacon/beacon_beam"),
            load("entity/end_portal/end_gateway_beam"),
            load("entity/end_portal/end_sky"),
            load("entity/end_portal/end_portal"),
        ];
        self.block_fx.set_textures(&self.device, &self.queue, [&imgs[0], &imgs[1], &imgs[2], &imgs[3]]);
    }

    pub fn request_capture(&mut self, path: std::path::PathBuf) {
        if self.can_capture {
            self.capture = Some(path);
        } else {
            eprintln!("capture impossible: the surface does not allow the copy");
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
        self.trans =RegionGpu::new(&self.device, &self.column_layout, (0, 0), self.formats.trans_bytes(), &self.trans_slots);
        self.free_slots.clear();
        self.next_slot = 0;
        self.columns_rev += 1;
        self.vis_rev += 1;
    }

    /// Check (`RUSTCRAFT_MESHCHECK`): fingerprint of each column uploaded, and the last
    /// walk result compared with a walk redone on the current columns (`None` if no
    /// walk).
    pub fn mesh_check(&self) -> (HashMap<(i32, i32), u64>, Option<bool>) {
        let digests = self.columns.iter().map(|(&k, c)| (k, c.digest)).collect();
        let reached = self.graph_args.map(|(eye, min_y, sections, radius, occlusion)| {
            let vis = |x, z| {
                let empty = || self.empty_vis.get(&(x, z)).map(|v| &v[..]);
                self.columns.get(&(x, z)).map(|c| &c.vis[..]).or_else(empty)
            };
            let mut r =
                rc_mesh::visibility::Graph::default().run(vis, eye, min_y, sections, radius, occlusion).to_vec();
            r.sort_unstable_by_key(|&(x, sy, z)| (x, z, sy));
            r == self.reached
        });
        (digests, reached)
    }

    /// F3 (paths and section visibility): per section reached by the last walk,
    /// (section, arrival directions, step, face connectivity, geometry present). The
    /// nodes are copied when a new walk has come back (the old copy serves while
    /// a walk runs in the background).
    pub fn occlusion_debug(&mut self) -> Vec<OccNode> {
        self.occ_debug = true;
        self.keep_occ_nodes();
        self.occ_nodes
            .1
            .iter()
            .map(|&(p, sources, step)| {
                let col = self.columns.get(&(p.0, p.2));
                let vis = col.and_then(|c| c.vis.get(p.1 as usize).copied()).unwrap_or(rc_mesh::visibility::VIS_NONE);
                let s = p.1 as usize;
                let any = |starts: &[Vec<u32>; 3]| starts.iter().any(|v| v.len() > s + 1 && v[s + 1] > v[s]);
                let geometry = col.is_some_and(|c| any(&c.starts) || any(&c.fluid_starts));
                (p, sources, step, vis, geometry)
            })
            .collect()
    }

    /// Copy of the nodes of the walk that came back, if an occlusion visualisation asks for it.
    fn keep_occ_nodes(&mut self) {
        if let Some(g) = &self.graph
            && self.occ_debug
            && self.occ_nodes.0 != self.reached_gen
        {
            self.occ_nodes.1.clear();
            self.occ_nodes.1.extend(g.nodes());
            self.occ_nodes.0 = self.reached_gen;
        }
    }

    /// Number of columns in GPU memory and the corresponding faces.
    pub fn stats(&self) -> (usize, u64) {
        (self.columns.len(), self.columns.values().map(|c| u64::from(c.face_alloc.1)).sum())
    }

    /// Columns having translucent quads, and among them those never re-sorted.
    pub fn trans_stats(&self) -> (usize, usize) {
        let with = self.columns.values().filter(|c| c.trans_alloc.1 > 0);
        with.fold((0, 0), |(n, f), c| (n + 1, f + usize::from(c.trans_centers.is_empty())))
    }

    /// CPU copies of the translucent quads (to re-sort them), in bytes.
    #[cfg_attr(not(feature = "memstats"), allow(dead_code))]
    pub fn trans_bytes(&self) -> usize {
        self.columns.values().map(|c| c.trans_centers.capacity() * 12 + c.order_starts.capacity() * 4).sum()
    }

    /// GPU memory allocated by us, in bytes: (region buffers allocated, of which used,
    /// textures, other buffers and depth).
    /// Report of the GPU allocator (backends that provide it).
    pub fn allocator_report(&self) -> Option<wgpu::AllocatorReport> {
        self.device.generate_allocator_report()
    }

    pub fn vram(&self) -> (u64, u64, u64, u64) {
        let alloc: u64 = self.regions.values().map(|r| r.faces.size() + r.fluids.size()).sum::<u64>()
            + self.trans.fluids.size()
            + self.trans.order.size();
        let used: u64 = self
            .columns
            .values()
            .map(|c| {
                u64::from(c.face_alloc.1) * FACE_BYTES
                    + u64::from(c.fluid_alloc.1) * self.formats.fluid_bytes()
                    + u64::from(c.trans_alloc.1) * self.formats.trans_bytes()
                    + u64::from(c.order_alloc.1) * ORDER_BYTES
            })
            .sum();
        let depth = u64::from(self.config.width) * u64::from(self.config.height) * 4;
        // Indexed faces (`FACEIDX`): shared indices, indexed commands, identity buffer.
        let faceidx = self.face_index.size() + self.indirect_idx.size() + self.face_ids.0.size();
        (alloc, used, self.atlas_bytes, self.quads_bytes + self.indirect.size() + self.anim.size() + faceidx + depth)
    }

    /// Average GPU time of a frame since the last call (ms), if measured.
    /// Average CPU duration per frame since the last call (ms).
    /// Replaces the lightmap (16 × 16 RGBA8, row = sky level).
    pub fn set_lightmap(&self, rgba: &[u8]) {
        self.queue.write_texture(
            self.lightmap.as_image_copy(),
            rgba,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(64), rows_per_image: Some(16) },
            wgpu::Extent3d { width: 16, height: 16, depth_or_array_layers: 1 },
        );
    }

    /// Command queue (for sending the glyphs from the app).
    pub fn queue_handle(&self) -> wgpu::Queue {
        self.queue.clone()
    }

    /// Window size in pixels.
    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// Width / height ratio of the frame.
    pub fn aspect(&self) -> f32 {
        self.config.width as f32 / self.config.height.max(1) as f32
    }

    pub fn cpu_ms(&mut self) -> f64 {
        let v = if self.cpu_frames == 0 { 0.0 } else { self.cpu_ms_sum / f64::from(self.cpu_frames) };
        (self.cpu_ms_sum, self.cpu_frames) = (0.0, 0);
        v
    }

    /// Terrain in grouped indirect draws (F3 line `Terrain Rendering`).
    pub fn uses_indirect(&self) -> bool {
        self.indirect_ok
    }

    pub fn gpu_ms(&mut self) -> Option<f64> {
        let pixels = u64::from(self.config.width) * u64::from(self.config.height);
        self.timer.as_mut().and_then(|t| t.average_ms(pixels))
    }

    /// Timestamps of a named pass (GPU time), `None` without measurement in this frame.
    fn stamp(&self, name: &'static str, role: Role) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        self.timer.as_ref().and_then(|t| t.pass(name, role))
    }

    /// Draws and presents a frame; `false` if the surface provided nothing (hidden window,
    /// timeout, reconfiguration): the caller then counts no displayed frame.
    pub fn render(&mut self, eye: [f64; 3], yaw: f32, pitch: f32, min_y: i32, height: i32, env: &FrameEnv) -> bool {
        if let Some(t) = &mut self.timer {
            t.collect();
        }
        let frame = match crate::present_watch::during(Stage::Acquire, || self.surface.get_current_texture()) {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => {
                if std::mem::take(&mut self.warned_surface) {
                    eprintln!("presentation resumed: the surface provides frames again");
                }
                f
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                crate::present_watch::during(Stage::Configure, || self.surface.configure(&self.device, &self.config));
                return false;
            }
            other => {
                if !self.warned_surface {
                    self.warned_surface = true;
                    eprintln!("presentation blocked: the surface provides no frame ({})", surface_state(&other));
                }
                return false;
            }
        };
        // CPU work of the frame: from the acquired frame to the upload to the GPU (excluding the wait for display).
        let t_cpu = Instant::now();
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Camera frame: integer origin (x, z) in the shader, fractional part here.
        // Game convention: yaw 0 = towards +z, angles increasing towards −x; positive pitch = downwards.
        let (origin, local, view_m) = crate::camera::view_matrix(eye, yaw, pitch);
        let sky_view = crate::camera::view_rotation(yaw, pitch);
        let aspect = self.config.width as f32 / self.config.height.max(1) as f32;
        let perspective = |fov: f32| perspective(fov, aspect, 0.05, env.far);
        // World projection = P × shake × sway (view space, `hand-camera.md` § 1.1).
        let proj = perspective(self.view.fov) * self.view.tilt * self.distort;
        let view_proj = proj * view_m;
        // Section sorting: same view without shake, field max(fov, option) (§ 1.1).
        let cull_view_proj = perspective(self.view.cull_fov) * view_m;
        // Instant of the frame for the appearance fade (same instant CPU- and GPU-side).
        let frame_now = Instant::now();
        let uniform = CameraUniform {
            view_proj: view_proj.to_cols_array_2d(),
            origin: [origin[0], min_y, origin[1], i32::from(self.rgss)],
            shade: self.shade,
            fade_now: self.fade_ring.clock(frame_now),
            fade_len: (self.fade_secs * 1e6) as f32,
            eye: [local.x, local.y, local.z, 0.0],
            fog_color: env.fog.color,
            fog: [env.fog.env_start, env.fog.env_end, env.fog.render_start, env.fog.render_end],
            fog_sky: [env.fog.sky_end, env.fog.cloud_end, 0.0, 0.0],
            sky_color: env.sky,
            sky_view_proj: (proj * sky_view).to_cols_array_2d(),
        };
        self.cameras.set(CAM_WORLD, bytemuck::bytes_of(&uniform));
        // Hand: own projection (hand field, without shake: it is in the hand's stack),
        // same frame, fog and lights as the world.
        if self.view.hand {
            let hand =
                CameraUniform { view_proj: (perspective(self.view.hud_fov) * view_m).to_cols_array_2d(), ..uniform };
            self.cameras.set(CAM_HAND, bytemuck::bytes_of(&hand));
            // Totem: same projection, without the view rotation (pose in the view frame), without fog.
            let totem = uniform.overlay(perspective(self.view.hud_fov).to_cols_array_2d(), [0.0; 4]);
            self.cameras.set(CAM_TOTEM, bytemuck::bytes_of(&totem));
        }
        // Interface models: x to the right and y upwards in interface units (y = −row),
        // z towards the screen, (reversed) depth in [0.5, 1] (always in front of the hand), without fog.
        if self.gui.has_models() {
            // Same projected size as the 2D interface (unrounded, like the game).
            let (w, h) = (self.gui.proj[0].max(1.0), self.gui.proj[1].max(1.0));
            let vp = glam::Mat4::from_cols(
                glam::Vec4::new(2.0 / w, 0.0, 0.0, 0.0),
                glam::Vec4::new(0.0, 2.0 / h, 0.0, 0.0),
                glam::Vec4::new(0.0, 0.0, 0.001, 0.0),
                glam::Vec4::new(-1.0, 1.0, 0.75, 1.0),
            );
            let g = uniform.overlay(vp.to_cols_array_2d(), [0.0, 0.0, 1000.0, 0.0]);
            self.cameras.set(CAM_GUI, bytemuck::bytes_of(&g));
        }
        self.cameras.flush(&mut self.up, &self.queue);
        let sky_vp = proj * sky_view;
        self.sky.prepare(&self.device, &self.queue, &mut self.up, sky_vp, &env.bodies, env.dome, eye);
        if let Some(p) = self.panorama.as_mut().filter(|_| self.show_panorama) {
            p.prepare(&self.queue, (self.config.width, self.config.height));
        }
        // Improved transparency: texts stored (solids or stages) as soon as the frame is uploaded.
        let oit_on = self.improved_transparency && self.oit_ok;
        self.tags.oit = oit_on;
        self.text_displays.oit = oit_on;
        self.gui.upload(&self.device, &self.queue, &mut self.up);
        if let Some(e) = &mut self.entities {
            e.upload(&self.device, &self.queue);
        }
        let gui = &self.gui;
        self.tags.upload(&self.device, &self.queue, &mut self.up, gui.glyph_gen, || gui.glyph_view());
        self.block_text.upload(&self.device, &self.queue, &self.gui);
        self.text_displays.upload(&self.device, &self.queue, &self.gui);
        if let Some(e) = &self.entities {
            self.block_fx.upload(&self.device, &self.queue, &e.blocks.fx);
        }
        self.outline.upload(&self.device, &self.queue, (self.config.width, self.config.height));
        self.cracks.upload(&self.device, &self.queue);
        self.leash.upload(&self.device, &self.queue);
        self.debug_lines.upload(&self.device, &self.queue, (self.config.width, self.config.height), &self.gui);
        self.particles.upload(&self.device, &self.queue);
        self.weather.upload(&self.device, &self.queue);
        // Appearance fade: visibilities of the recent columns (0 → 1 over `fade_secs`), or
        // only births and ends of fade on the GPU (`RUSTCRAFT_OPT_FADEGPU`).
        match self.fade_ring.frame(frame_now, self.fade_secs) {
            crate::terrain_shader::FadeWrite::None => {}
            crate::terrain_shader::FadeWrite::All(ring) => {
                self.up.write(&self.queue, &self.fade, 0, bytemuck::cast_slice(ring));
            }
            crate::terrain_shader::FadeWrite::Slots { slots, ring } => {
                for &s in slots {
                    let s = s as usize;
                    self.up.write(&self.queue, &self.fade, s as u64 * 4, bytemuck::cast_slice(&ring[s..s + 1]));
                }
            }
        }
        let planes = frustum_planes(cull_view_proj);
        // Improved transparency: targets and pipelines prepared as needed, targets freed otherwise.
        if oit_on {
            self.prepare_oit(proj);
        } else if let Some(o) = &mut self.oit {
            o.release();
        }
        // Storage of the next frame's objects (solids or stages).
        if let Some(e) = &mut self.entities {
            e.oit = oit_on;
        }

        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        if let Some(t) = &mut self.timer {
            t.begin(&mut enc);
        }
        // Last pass of the frame: that of the interface after the blur if there is one.
        let blur_radius = self.gui.blur_radius();
        self.gui.encode_items(&self.queue, &mut enc, self.timer.as_ref());
        let drawn: (usize, u64);
        {
            let timestamp_writes = self.stamp("world", Role::World);
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("world"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // Background: fog colour, like the game (§ 7.4).
                        load: wgpu::LoadOp::Clear({
                            let c = env.fog.color.map(f64::from);
                            wgpu::Color { r: c[0], g: c[1], b: c[2], a: 0.0 }
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(DEPTH_CLEAR),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes,
                ..Default::default()
            });
            // Diagnostic: fragments and triangles of the world pass (real overdraw).
            if let Some(t) = &self.timer {
                t.stats_begin(&mut pass);
            }
            pass.set_bind_group(0, &self.bind, &[]);
            // Sky before the terrain: dome, glow, sun, moon, stars, then dark disc
            // below the horizon (docs/specs/sky.md § 1).
            if env.dome {
                pass.set_pipeline(&self.sky_pipeline);
                pass.draw(0..24, 0..1);
                self.sky.draw_bodies(&mut pass);
                pass.set_bind_group(0, &self.bind, &[]);
            }
            if env.disc {
                pass.set_pipeline(&self.sky_pipeline);
                pass.draw(0..24, 1..2);
            }
            if env.bodies.end.is_some() {
                self.sky.draw_end(&mut pass);
                pass.set_bind_group(0, &self.bind, &[]);
            }
            // Sections reached from the camera (occlusion sorting, docs/specs/visibility.md),
            // kept if their box (one block of margin: a model may overflow) is in the field.
            let t_vis = Instant::now();
            let sections = height / 16;
            let cam_col = (eye[0].floor() as i32 >> 4, eye[2].floor() as i32 >> 4);
            // Radius: farthest loaded column (Chebyshev); only changes with the columns
            // or the camera's column (`RUSTCRAFT_OPT_VISORDER`: kept in cache).
            let rkey = (self.columns_rev, cam_col);
            let radius = match self.radius_cache {
                Some((k, r)) if k == rkey && lists::vis_order() => r,
                _ => {
                    let r = self
                        .columns
                        .keys()
                        .chain(self.empty_vis.keys())
                        .map(|&(x, z)| (x - cam_col.0).abs().max((z - cam_col.1).abs()))
                        .max()
                        .unwrap_or(0)
                        .min(VIEW_LIMIT);
                    self.radius_cache = Some((rkey, r));
                    r
                }
            };
            crate::perf_log::list_stage(0, t_vis.elapsed());
            // Walk redone only if the camera or the columns changed, in a background task
            // like the game: we draw with the last result to arrive (one frame of lag at
            // most); the very first is computed in place. Camera counted in cells of 8 blocks
            // like the game (not at each step: the walk and its copies cost every frame
            // while walking).
            let key = (eye.map(|v| (v / 8.0).floor() as i64), self.vis_rev, self.occlusion, min_y, sections);
            while let Ok((graph, reached, ms, grid)) = self.graph_rx.try_recv() {
                self.graph = Some(graph);
                self.reached = reached;
                self.reached_grid = grid;
                self.reached_gen += 1;
                self.graph_ms = ms;
                self.keep_occ_nodes();
            }
            if self.reached_key != Some(key)
                && let Some(mut graph) = self.graph.take()
            {
                self.reached_key = Some(key);
                let snapshot = match &self.vis_snapshot {
                    Some((rev, s)) if *rev == self.vis_rev => s.clone(),
                    _ => {
                        let cols = self.columns.iter().map(|(&k, c)| (k, c.vis.clone()));
                        let s: VisSnapshot =
                            Arc::new(cols.chain(self.empty_vis.iter().map(|(&k, v)| (k, v.clone()))).collect());
                        self.vis_snapshot = Some((self.vis_rev, s.clone()));
                        s
                    }
                };
                let occlusion = self.occlusion;
                self.graph_args = Some((eye, min_y, sections, radius, occlusion));
                let job = move || {
                    let t = Instant::now();
                    let mut r = graph
                        .run(|x, z| snapshot.get(&(x, z)).map(|v| &v[..]), eye, min_y, sections, radius, occlusion)
                        .to_vec();
                    // Sorted by column then height: the frame draws its series from them in one pass.
                    r.sort_unstable_by_key(|&(x, sy, z)| (x, z, sy));
                    // Grid of the sections reached for the entity visibility, made here
                    // rather than on the main thread.
                    let grid = lists::reach_grid().then(|| lists::ReachGrid::new(&r));
                    crate::perf_log::graph_work(t.elapsed());
                    (graph, r, t.elapsed().as_secs_f64() * 1e3, grid)
                };
                if self.reached.is_empty() {
                    let (g, r, ms, grid) = job();
                    (self.graph, self.reached, self.graph_ms, self.reached_grid) = (Some(g), r, ms, grid);
                    self.reached_gen += 1;
                    self.keep_occ_nodes();
                } else {
                    let tx = self.graph_tx.clone();
                    rayon::spawn(move || {
                        let _ = tx.send(job());
                    });
                }
            }
            // Sections reached as a set, for the entity visibility test.
            let t_set = Instant::now();
            if self.reached_set.0 != self.reached_gen {
                let reach = match self.reached_grid.take() {
                    Some(g) => lists::Reach::Grid(g),
                    None => lists::Reach::Set(self.reached.iter().copied().collect()),
                };
                self.reached_set = (self.reached_gen, min_y, sections, Arc::new(reach));
            }
            crate::perf_log::list_stage(1, t_set.elapsed());
            let t_sort = Instant::now();
            // Translucent sorting (like the game): when the camera changes block, neighbouring
            // columns re-sorted right away; the others in rotation, at least 15 per frame.
            let cam_block = eye.map(|v| v.floor() as i32);
            self.sort_eye = Some(eye);
            self.sort_min_y = min_y;
            let moved = self.sort_block != Some(cam_block);
            self.sort_block = Some(cam_block);
            // List of translucent columns redone only when the columns change; nothing to
            // do as long as the camera stays in the same block and everything is sorted.
            if self.trans_keys_rev != Some(self.columns_rev) {
                self.trans_keys =
                    self.columns.iter().filter(|(_, c)| !c.trans_centers.is_empty()).map(|(&k, _)| k).collect();
                self.trans_keys.sort_unstable();
                self.trans_keys_rev = Some(self.columns_rev);
                self.unsorted = self.trans_keys.len();
            }
            if trans_pov() {
                // Like the game: nothing in improved transparency; otherwise columns within ±2 of the camera
                // examined at every frame, plus max(n / 8, 15) others in rotation; in an
                // examined column, only the sections to re-sort (`pov_resorts`) are, one write per
                // run of contiguous sections.
                let n = self.trans_keys.len();
                if !oit_on && n > 0 {
                    let start = self.sort_cursor % n;
                    let (picks, next) = pov_picks(&self.trans_keys, start, cam_col);
                    let (sy0, how) = (min_y.div_euclid(16), trans_key());
                    let scratch = &mut self.sort_scratch;
                    let mut secs = Vec::new();
                    for k in picks {
                        let Some(c) = self.columns.get_mut(&k) else { continue };
                        if c.trans_centers.is_empty() {
                            continue;
                        }
                        pov_resorts(&mut c.povs, &c.trans_starts, eye, k, sy0, moved, &mut secs);
                        let (e, st, base) = (eye_in_column(eye, k, min_y), &c.trans_starts, c.fluid_ranges[2].start);
                        let mut i = 0;
                        while i < secs.len() {
                            // Run of sections to re-sort separated only by empty sections.
                            let mut j = i;
                            while j + 1 < secs.len() && st[secs[j + 1]] == st[secs[j] + 1] {
                                j += 1;
                            }
                            let (lo, hi) = (st[secs[i]], st[secs[j] + 1]);
                            let order = &mut scratch.order;
                            order.clear();
                            order.extend(lo..hi);
                            for &s in &secs[i..=j] {
                                let r = (st[s] - lo) as usize..(st[s + 1] - lo) as usize;
                                sort_section(&c.trans_centers, &mut order[r], e, how, &mut scratch.sort);
                            }
                            order.iter_mut().for_each(|v| *v += base);
                            self.up.write(
                                &self.queue,
                                &self.trans.order,
                                u64::from(c.order_alloc.0 + lo) * ORDER_BYTES,
                                bytemuck::cast_slice(order),
                            );
                            crate::perf_log::trans_resorts(j + 1 - i);
                            i = j + 1;
                        }
                    }
                    self.sort_cursor = next;
                }
            } else {
                if moved {
                    self.unsorted = self.trans_keys.len();
                }
                let keys = if self.unsorted > 0 { std::mem::take(&mut self.trans_keys) } else { Vec::new() };
                let mut budget = (keys.len() / 8).max(15);
                let n = keys.len();
                for step in 0..n {
                    let k = keys[(self.sort_cursor + step) % n];
                    let near = (k.0 - cam_col.0).abs() <= 1 && (k.1 - cam_col.1).abs() <= 1;
                    let Some(c) = self.columns.get_mut(&k) else { continue };
                    if c.sorted_at == Some(cam_block) || !(near && moved || budget > 0) {
                        continue;
                    }
                    if !near {
                        budget -= 1;
                    }
                    let order = translucent_order(
                        &c.trans_centers,
                        &c.trans_starts,
                        Some(eye_in_column(eye, k, min_y)),
                        c.fluid_ranges[2].start,
                    );
                    c.sorted_at = Some(cam_block);
                    crate::perf_log::trans_resorts(c.trans_starts.windows(2).filter(|w| w[1] > w[0]).count());
                    self.up.write(
                        &self.queue,
                        &self.trans.order,
                        u64::from(c.order_alloc.0) * ORDER_BYTES,
                        bytemuck::cast_slice(&order),
                    );
                }
                if n > 0 {
                    self.sort_cursor = (self.sort_cursor + (n / 8).max(15)) % n;
                    let columns = &self.columns;
                    self.unsorted =
                        keys.iter().filter(|k| columns.get(*k).is_some_and(|c| c.sorted_at != Some(cam_block))).count();
                    self.trans_keys = keys;
                }
            }
            // Runs of consecutive visible sections per column (`reached` is sorted by column
            // then height), in a reused buffer; each column points a range into it.
            let size = (self.config.width, self.config.height);
            let dkey: DrawKey = (
                eye.map(f64::to_bits),
                yaw.to_bits(),
                u64::from(pitch.to_bits()) | u64::from(self.view.cull_fov.to_bits()) << 32,
                self.columns_rev,
                self.reached_gen,
                size,
                min_y,
                sections,
            );
            if self.draw_key != Some(dkey) {
                crate::perf_log::list_stage(2, t_sort.elapsed());
                let t_lists = Instant::now();
                // Visible columns from the farthest to the nearest, runs of sections in the
                // field, batches and commands (`lists`).
                let inp = lists::ListIn {
                    columns: &self.columns,
                    reached: &self.reached,
                    planes: &planes,
                    origin,
                    eye,
                    min_y,
                    sections,
                    face_vertices: self.shader_opts.face_vertices(),
                    split_draws: self.split_draws,
                    reached_gen: self.reached_gen,
                    columns_rev: self.columns_rev,
                };
                let vis_ms = &mut self.vis_ms;
                drawn = lists::build(&inp, &mut self.lists, lists::vis_order(), || {
                    *vis_ms = t_vis.elapsed().as_secs_f64() * 1e3;
                });
                let (batches, args) = (&self.lists.batches, &self.lists.args);
                crate::perf_log::list_stage(3, t_lists.elapsed());
                let t_send = Instant::now();
                // Indexed faces: indexed commands of each batch of block faces, split;
                // only the instanced commands of the fluids remain, grouped.
                let (mut iargs, mut fargs) = (std::mem::take(&mut self.iargs), std::mem::take(&mut self.fargs));
                iargs.clear();
                fargs.clear();
                self.ibatches.clear();
                if self.shader_opts.faceidx {
                    for &(_, fluid, _, first, count) in batches {
                        let cmds = &args[first as usize..(first + count) as usize];
                        if fluid {
                            self.ibatches.push((fargs.len() as u32, count));
                            fargs.extend_from_slice(cmds);
                        } else {
                            let at = iargs.len() as u32;
                            indexed_args(cmds, &mut iargs);
                            self.ibatches.push((at, iargs.len() as u32 - at));
                        }
                    }
                }
                // Commands sent: with `RUSTCRAFT_OPT_ARGSKIP`, only the range that differs from what the
                // buffer already holds (unless a new buffer).
                let skip = lists::args_skip();
                let instanced: &[DrawArgs] = if self.shader_opts.faceidx { &fargs } else { args };
                if self.indirect_ok && !instanced.is_empty() {
                    let bytes = std::mem::size_of_val(instanced) as u64;
                    let mut fresh = !skip;
                    if self.indirect.size() < bytes {
                        self.indirect = self.device.create_buffer(&wgpu::BufferDescriptor {
                            label: Some("indirect commands"),
                            size: bytes.next_power_of_two(),
                            usage: wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
                            mapped_at_creation: false,
                        });
                        fresh = true;
                    }
                    let range =
                        if fresh { Some(0..instanced.len()) } else { lists::changed_range(&self.sent_args, instanced) };
                    if let Some(r) = range {
                        let at = (r.start * std::mem::size_of::<DrawArgs>()) as u64;
                        self.up.write(&self.queue, &self.indirect, at, bytemuck::cast_slice(&instanced[r]));
                    }
                    if skip {
                        self.sent_args.clear();
                        self.sent_args.extend_from_slice(instanced);
                    }
                }
                if self.shader_opts.faceidx {
                    let need = iargs.iter().map(|a| a.first_instance + 1).max().unwrap_or(0);
                    if need > self.face_ids.1 {
                        let n = need.next_power_of_two();
                        self.face_ids = (identity_buffer(&self.device, n), n);
                    }
                    if self.indirect_ok && !iargs.is_empty() {
                        let bytes = (iargs.len() * std::mem::size_of::<DrawIndexedArgs>()) as u64;
                        let mut fresh = !skip;
                        if self.indirect_idx.size() < bytes {
                            self.indirect_idx = self.device.create_buffer(&wgpu::BufferDescriptor {
                                label: Some("indirect indexed commands"),
                                size: bytes.next_power_of_two(),
                                usage: wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
                                mapped_at_creation: false,
                            });
                            fresh = true;
                        }
                        let range = if fresh { Some(0..iargs.len()) } else { lists::changed_range(&self.sent_iargs, &iargs) };
                        if let Some(r) = range {
                            let at = (r.start * std::mem::size_of::<DrawIndexedArgs>()) as u64;
                            self.up.write(&self.queue, &self.indirect_idx, at, bytemuck::cast_slice(&iargs[r]));
                        }
                        if skip {
                            self.sent_iargs.clear();
                            self.sent_iargs.extend_from_slice(&iargs);
                        }
                    }
                }
                (self.iargs, self.fargs) = (iargs, fargs);
                crate::perf_log::list_stage(4, t_send.elapsed());
                (self.drawn_cache, self.draw_key) = (drawn, Some(dkey));
            } else {
                drawn = self.drawn_cache;
            }
            let mut current: Option<(usize, bool)> = None;
            // Entities: after the opaque and cutout terrain, before the translucent (part 2 § 3.11).
            let mut entities_done = false;
            for (b, &(layer, fluid, key, first, count)) in self.lists.batches.iter().enumerate() {
                // Improved transparency: the pass only keeps the solids; the translucent (the
                // last commands) follows in its stages (`draw_oit`).
                if layer == 2 && oit_on {
                    break;
                }
                if layer == 2 && !entities_done {
                    entities_done = true;
                    // Cracks of the opaque blocks: after the opaque and cutout terrain.
                    self.cracks.draw(&mut pass, false);
                    if let Some(e) = &self.entities {
                        e.draw(&mut pass);
                        pass.set_bind_group(0, &self.bind, &[]);
                        self.leash.draw(&mut pass);
                        self.tags.draw_normal(&mut pass);
                        self.block_text.draw(&mut pass);
                        self.text_displays.draw_offset(&mut pass);
                        self.block_fx.draw(&mut pass);
                        self.outline.draw(&mut pass, false);
                    }
                    // Opaque particles with the entities (`particles.md` § 2.5).
                    pass.set_bind_group(0, &self.bind, &[]);
                    self.particles.draw_opaque(&mut pass);
                    // Debug (F3+B, F3+G, F3+F6 tracers): with the outlines, before the translucent
                    // terrain (the water in front tints the lines).
                    self.debug_lines.draw(&mut pass, crate::debug_render::Layer::Depth);
                    current = None;
                }
                let region = if key == TRANS_KEY {
                    &self.trans
                } else if let Some(r) = self.regions.get(&key) {
                    r
                } else {
                    continue;
                };
                if current != Some((layer, fluid)) {
                    pass.set_pipeline(match (fluid, layer) {
                        (true, 2) => &self.sorted_pipeline,
                        (true, _) => &self.fluid_pipelines[layer],
                        _ => &self.pipelines[layer],
                    });
                    current = Some((layer, fluid));
                }
                pass.set_bind_group(1, &region.bind, &[]);
                if !fluid && self.shader_opts.faceidx {
                    self.draw_indexed_faces(&mut pass, b);
                    continue;
                }
                pass.set_vertex_buffer(
                    0,
                    match (fluid, layer) {
                        (true, 2) => region.order.slice(..),
                        (true, _) => region.fluids.slice(..),
                        _ => region.faces.slice(..),
                    },
                );
                self.draw_instanced(&mut pass, b, first, count);
            }
            if oit_on {
                self.draw_solid_features(&mut pass);
            } else {
                if !entities_done {
                    self.cracks.draw(&mut pass, false);
                }
                if !entities_done && let Some(e) = &self.entities {
                    e.draw(&mut pass);
                    pass.set_bind_group(0, &self.bind, &[]);
                    self.leash.draw(&mut pass);
                    self.tags.draw_normal(&mut pass);
                    self.block_text.draw(&mut pass);
                    self.text_displays.draw_offset(&mut pass);
                    self.block_fx.draw(&mut pass);
                }
                if !entities_done {
                    self.outline.draw(&mut pass, false);
                    pass.set_bind_group(0, &self.bind, &[]);
                    self.particles.draw_opaque(&mut pass);
                    self.debug_lines.draw(&mut pass, crate::debug_render::Layer::Depth);
                }
                // Translucent particles after the translucent terrain.
                pass.set_bind_group(0, &self.bind, &[]);
                self.particles.draw_translucent(&mut pass);
                self.cracks.draw(&mut pass, true);
                self.outline.draw(&mut pass, true);
                // Debug: "always in front" traces (cube of the current section).
                self.debug_lines.draw(&mut pass, crate::debug_render::Layer::Front);
                // "See-through" labels after the translucent (and the outlines).
                self.tags.draw_see_through(&mut pass);
                self.text_displays.draw_see_through(&mut pass);
                // Clouds after the translucent terrain, then the interface.
                self.sky.draw_clouds(&mut pass);
                // Weather after the clouds (`sky.md` § 1).
                pass.set_bind_group(0, &self.bind, &[]);
                self.weather.draw(&mut pass);
                self.weather.wall.draw(&mut pass);
                // Panorama of the out-of-world screens: covers everything, under the interface.
                if let Some(p) = self.panorama.as_ref().filter(|_| self.show_panorama) {
                    p.draw(&mut pass);
                }
            }
            if let Some(t) = &self.timer {
                t.stats_end(&mut pass);
            }
        }
        if oit_on {
            self.draw_oit(&mut enc, &view);
            // After the composition: see-through labels, then "always in front" traces.
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("after transparency"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: self.stamp("after transparency", Role::Inner),
                ..Default::default()
            });
            pass.set_bind_group(0, &self.bind, &[]);
            self.draw_after_oit(&mut pass);
            if let Some(p) = self.panorama.as_ref().filter(|_| self.show_panorama) {
                p.draw(&mut pass);
            }
        }
        // Outline of the glowing entities: coloured silhouettes, edge and blur, then blend over
        // the world image (visible through blocks), before the hand and the interface.
        if let Some(e) = self.entities.as_ref().filter(|e| e.has_glow()) {
            self.entity_outline.ensure(&self.device, (self.config.width, self.config.height));
            if let Some(target) = self.entity_outline.silhouettes() {
                {
                    let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("glowing entity silhouettes"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: target,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        timestamp_writes: self.stamp("glowing entity silhouettes", Role::Inner),
                        ..Default::default()
                    });
                    pass.set_bind_group(0, &self.bind, &[]);
                    e.draw_glow(&mut pass);
                }
                self.entity_outline.process(&mut enc, self.timer.as_ref());
                self.entity_outline.composite(&mut enc, &view, self.timer.as_ref());
            }
        }
        // Screen effects (`miscellaneous.md` § 6.2): after the world and the hand, before the
        // screen overlays and the interface; the hand then has its own pass.
        let effects = std::mem::take(&mut self.post_effects);
        let post = !effects.is_empty() && self.can_capture;
        if post {
            if self.view.hand
                && let Some(e) = &self.entities
            {
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("main"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &self.depth,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(DEPTH_CLEAR),
                            store: wgpu::StoreOp::Discard,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: self.stamp("main", Role::Inner),
                    ..Default::default()
                });
                pass.set_bind_group(0, &self.hand_bind, &[]);
                e.draw_hand(&mut pass);
                // Names of the held map's decorations, same camera.
                self.text_displays.draw_hand(&mut pass);
            }
            let blur = &mut self.blur;
            self.post_fx.run(
                &self.device,
                &self.queue,
                &mut enc,
                &frame.texture,
                &view,
                &effects,
                blur,
                self.post_blur,
                self.timer.as_ref(),
            );
        }
        {
            // First-person hand (`hand-camera.md` § 1.2): depth cleared (the hand is
            // always in front of the world), own projection; then the interface.
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hand and interface"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(DEPTH_CLEAR),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: self.stamp("hand and interface", if blur_radius.is_none() { Role::Last } else { Role::Inner }),
                ..Default::default()
            });
            if self.view.hand
                && !post
                && let Some(e) = &self.entities
            {
                pass.set_bind_group(0, &self.hand_bind, &[]);
                e.draw_hand(&mut pass);
                // Names of the held map's decorations, same camera.
                self.text_displays.draw_hand(&mut pass);
            }
            // Screen effects over the hand, camera overlays between vignette and crosshair
            // (under the menu blur, like the rest of the HUD).
            let overlays = self.overlays.as_ref();
            if let Some(o) = overlays {
                o.draw_screen(&mut pass);
            }
            // Totem animation: over the screen effects (fire, water, block), fixed lights.
            if self.view.hand
                && let Some(e) = &self.entities
            {
                pass.set_bind_group(0, &self.totem_bind, &[]);
                e.draw_totem(&mut pass);
            }
            // 3D crosshair (F3): hand camera (hand field, without sway or
            // shake: it stays centred and only turns with the camera), above the hand,
            // under the interface.
            if self.debug_lines.has(crate::debug_render::Layer::Crosshair) {
                pass.set_bind_group(0, if self.view.hand { &self.hand_bind } else { &self.bind }, &[]);
                self.debug_lines.draw(&mut pass, crate::debug_render::Layer::Crosshair);
            }
            let (entities, gui_bind) = (self.entities.as_ref(), &self.gui_bind);
            self.gui.draw_blur_part_models(
                &mut pass,
                false,
                |p| {
                    if let Some(o) = overlays {
                        o.draw_gui(p);
                    }
                },
                |p| {
                    if let Some(e) = entities {
                        p.set_bind_group(0, gui_bind, &[]);
                        e.draw_gui(p);
                    }
                },
            );
        }
        // Menu background: blur of everything that precedes, then the rest of the interface (depth cleared).
        if let Some(radius) = blur_radius {
            // No image copy possible: interface without blur.
            if self.can_capture {
                let timer = self.timer.as_ref();
                self.blur.run(&self.device, &self.queue, &mut enc, &frame.texture, &view, radius, timer);
            }
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("interface after the blur"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(DEPTH_CLEAR),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: self.stamp("interface after the blur", Role::Last),
                ..Default::default()
            });
            self.gui.draw_blur_part(&mut pass, true, |_| {});
        }
        self.drawn = drawn;
        // End of the measurement; the capture copy that follows is not part of it (intended).
        if let Some(t) = &mut self.timer {
            t.end(&mut enc);
        }
        let readback = self.capture.take().map(|path| {
            let (w, h) = (self.config.width, self.config.height);
            let row = (w * 4).div_ceil(256) * 256;
            let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("capture"),
                size: u64::from(row * h),
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            enc.copy_texture_to_buffer(
                frame.texture.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(row),
                        rows_per_image: Some(h),
                    },
                },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
            (path, buffer, row, w, h)
        });
        crate::present_watch::during(Stage::Submit, || self.up.submit(&self.queue, enc.finish()));
        let s = self.up.take_stats();
        crate::perf_log::gpu_writes(s.writes, s.transits, s.bytes);
        self.cpu_ms_sum += t_cpu.elapsed().as_secs_f64() * 1e3;
        self.cpu_frames += 1;
        if let Some(t) = &mut self.timer {
            t.after_submit();
        }
        // Read callbacks handled at the next submit (wgpu picks up finished work there).
        if !self.no_poll {
            let _ = self.device.poll(wgpu::PollType::Poll);
        }
        if let Some((path, buffer, row, w, h)) = readback {
            let slice = buffer.slice(..);
            slice.map_async(wgpu::MapMode::Read, |_| {});
            // Bounded wait: a GPU that does not finish must not freeze the loop.
            let waited = crate::present_watch::during(Stage::Readback, || {
                self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: Some(READBACK_TIMEOUT) })
            });
            let bgra =
                matches!(self.config.format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb);
            if let Err(e) = waited {
                eprintln!(
                    "presentation blocked: capture {} abandoned, the GPU did not finish in {} s ({e})",
                    path.display(),
                    READBACK_TIMEOUT.as_secs()
                );
            } else {
                let data = slice.get_mapped_range().expect("capture read");
                // Player capture (F2): PNG written on another thread (`screenshot.rs`).
                if path.extension().is_some_and(|e| e == "png") {
                    crate::screenshot::save(path, data.to_vec(), row as usize, w, h, bgra);
                } else {
                    match write_bmp(&path, &data, row as usize, w, h, bgra) {
                        Ok(()) => eprintln!("capture: {}", path.display()),
                        Err(e) => eprintln!("capture {}: {e}", path.display()),
                    }
                }
            }
        }
        self.window.pre_present_notify();
        crate::present_watch::during(Stage::Present, || self.queue.present(frame));
        true
    }

    /// Improved transparency: full-screen passes, targets at the frame size, pipelines of the
    /// families (made once), frame projection.
    fn prepare_oit(&mut self, proj: Mat4) {
        let oit = self.oit.get_or_insert_with(|| {
            eprintln!("improved transparency: passes prepared");
            crate::oit::Oit::new(&self.device, self.config.format)
        });
        oit.ensure(&self.device, (self.config.width, self.config.height), &self.depth);
        oit.set_projection(&self.queue, proj);
        if self.terrain_oit.is_none() {
            self.terrain_oit = Some(terrain_oit_sets(
                &self.device,
                &oit.layouts,
                &self.terrain_res.layout,
                &self.column_layout,
                self.formats.entries().0,
                self.shader_opts,
            ));
        }
        self.sky.ensure_oit(&self.device, &oit.layouts);
        self.particles.ensure_oit(&self.device, &oit.layouts);
        self.weather.ensure_oit(&self.device, &oit.layouts);
        if let Some(e) = &mut self.entities {
            e.ensure_oit(&self.device, &oit.layouts);
        }
        self.tags.ensure_oit(&self.device, &oit.layouts);
        self.text_displays.ensure_oit(&self.device, &oit.layouts);
        self.outline.ensure_oit(&self.device, &oit.layouts);
        self.block_fx.ensure_oit(&self.device, &oit.layouts);
        self.cracks.ensure_oit(&self.device, &oit.layouts);
        self.debug_lines.ensure_oit(&self.device, &oit.layouts);
    }

    /// Improved transparency, solids pass: what follows the opaque and cutout terrain
    /// (cracks, entities, texts and effects of opaque blocks, opaque particles, debug).
    fn draw_solid_features(&self, pass: &mut wgpu::RenderPass<'_>) {
        // Cracks of the opaque blocks: stay in the solids pass (classic blend).
        pass.set_bind_group(0, &self.bind, &[]);
        self.cracks.draw(pass, false);
        if let Some(e) = &self.entities {
            e.draw_solid(pass);
            pass.set_bind_group(0, &self.bind, &[]);
            self.leash.draw(pass);
            self.tags.draw_solid(pass);
            // Sign text: always opaque and without background, therefore solid in the game.
            self.block_text.draw(pass);
            self.text_displays.draw_solid(pass);
            self.block_fx.draw_solid(pass);
        }
        self.outline.draw_solid(pass);
        pass.set_bind_group(0, &self.bind, &[]);
        self.particles.draw_opaque(pass);
        self.debug_lines.draw_solid(pass);
    }

    /// Improved transparency, after the composition.
    fn draw_after_oit(&self, pass: &mut wgpu::RenderPass<'_>) {
        // Like the game: see-through texts, then "always in front" traces.
        self.tags.draw_see_through(pass);
        self.text_displays.draw_see_through(pass);
        self.debug_lines.draw(pass, crate::debug_render::Layer::Front);
    }

    /// Stages of the improved transparency then composition (after the solids pass).
    /// Terrain in the terrain depth (boats' water mask included), elements in the
    /// main depth, clouds in theirs; bounds culling after the first stage.
    fn draw_oit(&self, enc: &mut wgpu::CommandEncoder, view: &wgpu::TextureView) {
        use crate::oit::Stage;
        let Some(oit) = &self.oit else { return };
        let mask = self.entities.as_ref().is_some_and(|e| e.has_water_mask());
        let ts = |name| self.stamp(name, Role::Inner);
        let water_mask = |enc: &mut wgpu::CommandEncoder| {
            let mut pass = oit.begin_water_mask(enc, ts("transparency: water mask"));
            pass.set_bind_group(0, &self.bind, &[]);
            if let Some(e) = &self.entities {
                e.draw_water_mask(&mut pass);
            }
        };
        if mask {
            water_mask(enc);
        }
        let terrain_depth = if mask { oit.terrain_depth() } else { &self.depth };
        let clouds = self.sky.has_clouds();
        for stage in Stage::ALL {
            let f = oit.frame(stage);
            let [terrain, features, cloud_pass] = stage.pass_names();
            {
                let mut pass = oit.begin(enc, stage, terrain_depth, true, terrain, ts(terrain));
                pass.set_bind_group(0, &self.bind, &[]);
                self.draw_translucent_terrain(&mut pass, f);
            }
            {
                let mut pass = oit.begin(enc, stage, &self.depth, false, features, ts(features));
                pass.set_bind_group(0, &self.bind, &[]);
                self.draw_translucent_features(&mut pass, f);
            }
            if stage == Stage::Bounds {
                oit.cull(enc, &self.depth, ts("transparency: bounds culling"));
            }
            if clouds {
                let mut pass = oit.begin_clouds(enc, stage, ts(cloud_pass));
                self.sky.draw_clouds_oit(&mut pass, f);
            }
            // Main depth changed by the culling: water mask redone on it.
            if stage == Stage::Bounds && mask {
                water_mask(enc);
            }
        }
        oit.composite(enc, view, &self.depth, ts("transparency: composition"));
    }

    /// Translucent elements (stage `f`): shadows, models, labels, texts, outlines, items,
    /// cracks, particles, debug, then border and weather; group 0 (camera) bound.
    fn draw_translucent_features(&self, pass: &mut wgpu::RenderPass<'_>, f: crate::oit::Frame<'_>) {
        if let Some(e) = &self.entities {
            e.draw_oit(pass, f);
            pass.set_bind_group(0, &self.bind, &[]);
        }
        self.tags.draw_oit(pass, f);
        self.text_displays.draw_oit(pass, f);
        self.outline.draw_oit(pass, f);
        self.block_fx.draw_oit(pass, f);
        self.cracks.draw_oit(pass, f);
        self.particles.draw_translucent_oit(pass, f);
        self.debug_lines.draw_oit(pass, f);
        pass.set_bind_group(0, &self.bind, &[]);
        self.weather.draw_oit(pass, f);
    }

    /// Translucent commands of the terrain (layer 2) with the pipelines of stage `f`.
    fn draw_translucent_terrain(&self, pass: &mut wgpu::RenderPass<'_>, f: crate::oit::Frame<'_>) {
        let Some([blocks, sorted]) = &self.terrain_oit else { return };
        let mut current = None;
        for (b, &(layer, fluid, key, first, count)) in self.lists.batches.iter().enumerate() {
            if layer != 2 {
                continue;
            }
            let region = if key == TRANS_KEY {
                &self.trans
            } else if let Some(r) = self.regions.get(&key) {
                r
            } else {
                continue;
            };
            if current != Some(fluid) {
                f.set(pass, if fluid { sorted } else { blocks });
                current = Some(fluid);
            }
            pass.set_bind_group(1, &region.bind, &[]);
            if !fluid && self.shader_opts.faceidx {
                self.draw_indexed_faces(pass, b);
                continue;
            }
            pass.set_vertex_buffer(0, if fluid { region.order.slice(..) } else { region.faces.slice(..) });
            self.draw_instanced(pass, b, first, count);
        }
    }

    /// Instanced commands of batch `b` (`first..first + count` of its commands; with
    /// `RUSTCRAFT_OPT_FACEIDX`, place in the fluid list); fallback: one per series without
    /// indirect draw.
    fn draw_instanced(&self, pass: &mut wgpu::RenderPass<'_>, b: usize, first: u32, count: u32) {
        let (list, first, count) = if self.shader_opts.faceidx {
            (&self.fargs, self.ibatches[b].0, self.ibatches[b].1)
        } else {
            (&self.lists.args, first, count)
        };
        if self.indirect_ok {
            pass.multi_draw_indirect(&self.indirect, u64::from(first) * 16, count);
        } else {
            for a in &list[first as usize..(first + count) as usize] {
                pass.draw(0..a.vertex_count, a.first_instance..a.first_instance + a.instance_count);
            }
        }
    }

    /// Indexed commands of batch `b` (block faces, `RUSTCRAFT_OPT_FACEIDX`).
    fn draw_indexed_faces(&self, pass: &mut wgpu::RenderPass<'_>, b: usize) {
        let (first, count) = self.ibatches[b];
        pass.set_vertex_buffer(0, self.face_ids.0.slice(..));
        pass.set_index_buffer(self.face_index.slice(..), wgpu::IndexFormat::Uint16);
        if self.indirect_ok {
            pass.multi_draw_indexed_indirect(&self.indirect_idx, u64::from(first) * 20, count);
        } else {
            for a in &self.iargs[first as usize..(first + count) as usize] {
                pass.draw_indexed(0..a.index_count, 0, a.first_instance..a.first_instance + 1);
            }
        }
    }
}

/// Pipelines of the translucent terrain in improved transparency: block faces (`vs`) and sorted
/// fluids (`vs_sorted`), back faces culled like the classic terrain.
fn terrain_oit_sets(
    device: &wgpu::Device,
    layouts: &crate::oit::Layouts,
    camera: &wgpu::BindGroupLayout,
    column: &wgpu::BindGroupLayout,
    vs_sorted: &str,
    opts: crate::terrain_shader::ShaderOpts,
) -> [crate::oit::Set; 2] {
    let module = crate::oit::module(
        device,
        "terrain (transparency)",
        &crate::terrain_shader::terrain(opts),
        include_str!("terrain_oit.wgsl"),
    );
    let groups = [Some(camera), Some(column)];
    let face_vs = opts.face_entry();
    let faces = [Some(instance_layout(face_vs))];
    let sorted = [Some(instance_layout(vs_sorted))];
    let mut blocks = crate::oit::Spec::new(
        "translucent terrain (transparency)",
        &groups,
        &module,
        face_vs,
        &faces,
        "fs_translucent",
    )
    .cull(true);
    if face_vs == "vs" {
        blocks.topology = opts.topology();
    }
    [
        layouts.set(device, &blocks),
        layouts.set(
            device,
            &crate::oit::Spec::new(
                "sorted fluids (transparency)",
                &groups,
                &module,
                vs_sorted,
                &sorted,
                "fs_translucent",
            )
            .cull(true),
        ),
    ]
}

/// Maximum wait for the readback of a capture.
const READBACK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Reason why the surface provided no frame (log, once per episode).
fn surface_state(s: &wgpu::CurrentSurfaceTexture) -> &'static str {
    match s {
        wgpu::CurrentSurfaceTexture::Timeout => "timeout",
        wgpu::CurrentSurfaceTexture::Occluded => "window hidden",
        wgpu::CurrentSurfaceTexture::Validation => "validation error",
        _ => "other",
    }
}

/// 32-bit BMP, rows from top to bottom (negative height), channels B, G, R, A.
fn write_bmp(path: &std::path::Path, data: &[u8], row: usize, w: u32, h: u32, bgra: bool) -> std::io::Result<()> {
    let size = 54 + w as usize * h as usize * 4;
    let mut out = Vec::with_capacity(size);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(size as u32).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(-(h as i32)).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&[0; 24]);
    for y in 0..h as usize {
        for px in data[y * row..y * row + w as usize * 4].as_chunks::<4>().0 {
            if bgra {
                out.extend_from_slice(&[px[0], px[1], px[2], 255]);
            } else {
                out.extend_from_slice(&[px[2], px[1], px[0], 255]);
            }
        }
    }
    std::fs::write(path, out)
}

/// Layout of the camera bindings (uniforms, atlases, instance buffers), common
/// to the terrain and to all the entity renderers.
pub(crate) fn camera_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let entry = |binding, visibility, ty| wgpu::BindGroupLayoutEntry { binding, visibility, ty, count: None };
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[
            entry(
                0,
                wgpu::ShaderStages::VERTEX_FRAGMENT,
                wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
            ),
            entry(
                1,
                wgpu::ShaderStages::VERTEX,
                wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
            ),
            entry(
                2,
                wgpu::ShaderStages::FRAGMENT,
                wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                    multisampled: false,
                },
            ),
            entry(3, wgpu::ShaderStages::FRAGMENT, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
            entry(
                5,
                wgpu::ShaderStages::FRAGMENT,
                wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                    multisampled: false,
                },
            ),
            entry(
                7,
                wgpu::ShaderStages::VERTEX,
                wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
            ),
            entry(8, wgpu::ShaderStages::VERTEX, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
            entry(
                9,
                wgpu::ShaderStages::VERTEX,
                wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
            ),
            entry(
                6,
                wgpu::ShaderStages::VERTEX,
                wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
            ),
            entry(
                4,
                wgpu::ShaderStages::VERTEX,
                wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
            ),
        ],
    })
}

#[cfg(test)]
#[path = "render_gpu_test.rs"]
mod gpu_test;

#[cfg(test)]
mod tests {
    use super::*;
    use rc_mesh::fluid::FluidQuad;

    /// Totem and interface do not read the fade clock: their cameras do not change
    /// when only the world clock advances (unchanged upload skipped).
    #[test]
    fn overlay_camera_ignores_the_fade_clock() {
        let base = |fade_now| CameraUniform { fade_now, fade_len: 5e5, shade: [0.5; 6], ..bytemuck::Zeroable::zeroed() };
        let vp = glam::Mat4::IDENTITY.to_cols_array_2d();
        let (a, b) = (base(1_000).overlay(vp, [0.0; 4]), base(17_667).overlay(vp, [0.0; 4]));
        assert_eq!(bytemuck::bytes_of(&a), bytemuck::bytes_of(&b));
        assert_eq!((a.fade_len, a.shade, a.fog), (5e5, [0.5; 6], [1e9, 2e9, 1e9, 2e9]));
    }

    /// Indexed commands (`RUSTCRAFT_OPT_FACEIDX`): same faces, in the same order, as the
    /// instanced commands, including beyond an index slice; one instance, no
    /// vertex offset (false on DX12 without indirect validation).
    #[test]
    fn indexed_args_draw_the_same_faces_in_order() {
        let a = |n, first| DrawArgs { vertex_count: 6, instance_count: n, first_vertex: 0, first_instance: first };
        let args = [a(40_000, 7), a(1, 100_000), a(INDEXED_FACES, 3), a(INDEXED_FACES + 1, 9)];
        let mut out = Vec::new();
        indexed_args(&args, &mut out);
        let index = face_indices(INDEXED_FACES);
        let drawn: Vec<u32> = out
            .iter()
            .flat_map(|c| {
                assert!(c.index_count as usize <= index.len() && c.index_count % 6 == 0, "{c:?}");
                assert_eq!((c.instance_count, c.first_index, c.base_vertex), (1, 0, 0), "{c:?}");
                index[..c.index_count as usize].chunks(6).map(move |t| c.first_instance + u32::from(t[0] >> 2))
            })
            .collect();
        let expected: Vec<u32> =
            args.iter().flat_map(|a| a.first_instance..a.first_instance + a.instance_count).collect();
        assert_eq!(drawn, expected);
        assert_eq!(out.len(), 3 + 1 + 1 + 2);
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
            (false, _, true) => {
                FluidGpu { p, v, meta: [w[20], 0, 0, 0], colors: [w[21]; 4], light: [w[22], w[23], 0, 0] }
            }
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

    /// Free order: a double-face quad counts as its two earlier quads.
    #[test]
    fn order_free_counts_double_faces_as_both() {
        let lake: Vec<FluidQuad> = (0..6).map(|i| flat(i as f32, 0.0, 14.875, true)).collect();
        let old: Vec<FluidQuad> = lake.iter().flat_map(|q| [*q, rc_mesh::fluid::back_face(q)]).collect();
        let new: Vec<FluidQuad> =
            lake.iter().map(|q| FluidQuad { texture: q.texture | rc_mesh::fluid::DOUBLE_FACE, ..*q }).collect();
        assert_eq!(order_free(&old, &[0]), order_free(&new, &[0]));
        assert!(order_free(&new, &[0]));
        // Two double faces in the same cell: not free, as before.
        let twice = [new[0], new[0]];
        let old_twice: Vec<FluidQuad> =
            [lake[0], lake[0]].iter().flat_map(|q| [*q, rc_mesh::fluid::back_face(q)]).collect();
        assert_eq!(order_free(&old_twice, &[0]), order_free(&twice, &[0]));
        // A double face and a lone back face in the same cell: not free.
        assert!(!order_free(&[new[0], rc_mesh::fluid::back_face(&lake[0])], &[0]));
    }

    /// Horizontal quad at y = h on cell (x, z), top (counter-clockwise seen from above) or bottom.
    fn flat(x: f32, z: f32, h: f32, up: bool) -> FluidQuad {
        let mut c = [
            [x, h, z, 0.0, 0.0],
            [x, h, z + 1.0, 0.0, 0.0],
            [x + 1.0, h, z + 1.0, 0.0, 0.0],
            [x + 1.0, h, z, 0.0, 0.0],
        ];
        if !up {
            c.reverse();
        }
        FluidQuad { corners: c, texture: 0, colors: [0; 4], light: [0; 4] }
    }

    /// Randomly drawn mesh: faces of the three layers, fluids, free translucents (lake
    /// surface) one time in two, otherwise arbitrary.
    fn random_mesh(seed: &mut u64, scale: u64) -> BlockMesh {
        let mut next = |n: u64| {
            *seed ^= *seed << 13;
            *seed ^= *seed >> 7;
            *seed ^= *seed << 17;
            *seed % n.max(1)
        };
        let faces = |next: &mut dyn FnMut(u64) -> u64| -> Vec<rc_mesh::blocks::Face> {
            (0..next(300 * scale)).map(|_| std::array::from_fn(|_| next(1 << 20) as u32)).collect()
        };
        let quad = |next: &mut dyn FnMut(u64) -> u64| FluidQuad {
            corners: std::array::from_fn(|_| std::array::from_fn(|_| next(64) as f32 * 0.25)),
            texture: next(900) as u32,
            colors: std::array::from_fn(|_| next(u64::from(u32::MAX)) as u32),
            light: std::array::from_fn(|_| next(0xffff) as u16),
        };
        let layers = [faces(&mut next), faces(&mut next), faces(&mut next)];
        let fl0: Vec<FluidQuad> = (0..next(10)).map(|_| quad(&mut next)).collect();
        let fl1: Vec<FluidQuad> = (0..next(10)).map(|_| quad(&mut next)).collect();
        let trans: Vec<FluidQuad> = if next(2) == 0 {
            (0..next(40)).map(|i| flat((i % 16) as f32, (i / 16) as f32, 14.875, true)).collect()
        } else {
            (0..next(40)).map(|_| quad(&mut next)).collect()
        };
        let n = trans.len() as u32;
        BlockMesh {
            x: next(200) as i32 - 100,
            z: next(200) as i32 - 100,
            version: 1,
            layers,
            fluids: [fl0, fl1, trans],
            starts: [vec![0], vec![0], vec![0]],
            fluid_starts: [vec![0, 0], vec![0, 0], vec![0, n]],
            vis: vec![0; 1],
        }
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
            faces += full.faces.len();
            // Column in the region placed on each face (the drawn faces fit in 20 bits).
            let local = local_of(m.x, m.z);
            assert!(full.faces.iter().all(|f| f[0] >> 20 == local));
        }
        eprintln!("500 meshes, {faces} faces, {free} free translucent columns");
        assert!(free > 50 && faces > 100_000, "draw too poor");
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
        for _ in 0..500 {
            let m = random_mesh(&mut seed, 1);
            let mut n = BlockMesh {
                x: m.x,
                z: m.z,
                version: 2,
                layers: m.layers.clone(),
                fluids: m.fluids.clone(),
                starts: m.starts.clone(),
                fluid_starts: m.fluid_starts.clone(),
                vis: m.vis.clone(),
            };
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
            let (pb, pn) = (prepare(&m, true, f), prepare(&n, true, f));
            let mut packed = Vec::new();
            rc_mesh::blocks::pack(&m, &mut packed);
            let d = rc_mesh::blocks::in_place_diff(&packed, &n).expect("same counts");
            let at = [1 + next(50) as u32, 1 + next(50) as u32, 1 + next(50) as u32, 1 + next(50) as u32];
            let len = [pb.faces.len() as u32, pb.fluid_len() as u32, pb.trans_len() as u32];
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
                    bytemuck::cast_slice(&p.faces).to_vec(),
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
                gpu[i][off..off + r.len()].copy_from_slice(&src[i][r]);
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
        thread_local!(static SCRATCH: std::cell::RefCell<Vec<u32>> = const { std::cell::RefCell::new(Vec::new()) });
        time("flattening + copy (reused buffer)", &|m| {
            SCRATCH.with_borrow_mut(|w| {
                rc_mesh::blocks::pack(m, w);
                std::hint::black_box(w.clone()).len()
            })
        });
        time("exact flattening", &|m| std::hint::black_box(rc_mesh::blocks::pack_exact(m)).len());
    }

    /// Test generator with a fixed seed (independent of the clock).
    fn lcg(s: &mut u64) -> u64 {
        *s = s.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        *s >> 33
    }

    /// Centres on a half-block grid (many equal distances) and camera, with a fixed seed.
    fn trans_case(s: &mut u64, n: usize) -> (Vec<[f32; 3]>, [f32; 3]) {
        let mut v = || (lcg(s) % 32) as f32 * 0.5;
        let centers: Vec<[f32; 3]> = (0..n).map(|_| [v(), v(), v()]).collect();
        let eye = [v(), v(), v()];
        (centers, eye)
    }

    /// Order of a section by each method of `RUSTCRAFT_OPT_TRANSKEY`.
    fn section_by(centers: &[[f32; 3]], eye: [f32; 3], how: TransKey) -> Vec<u32> {
        let mut order: Vec<u32> = (0..centers.len() as u32).collect();
        sort_section(centers, &mut order, eye, how, &mut SortScratch::default());
        order
    }

    /// `RUSTCRAFT_OPT_TRANSKEY`: distances computed once (pairs or integer keys) give, bit for bit,
    /// the order of the original stable sort, ties included (sections long enough for the
    /// unstable sort to move them), non-finite cameras included.
    #[test]
    fn trans_key_orders_match_stable_sort() {
        let mut s = 0x5eed_1606u64;
        let mut ties = 0;
        for case in 0..300 {
            let n = [0, 1, 2, 7, 33, 150, 600][case % 7];
            let (centers, mut eye) = trans_case(&mut s, n);
            match case % 50 {
                1 => eye = [f32::NAN, 0.0, 0.0],
                2 => eye = [f32::INFINITY, 1.0, -f32::INFINITY],
                3 => eye = [1e30, -1e30, 0.0],
                _ => {}
            }
            let old = section_by(&centers, eye, TransKey::Old);
            let d = |i: u32| {
                (0..3).map(|a| (centers[i as usize][a] - eye[a]) * (centers[i as usize][a] - eye[a])).sum::<f32>()
            };
            ties += old.windows(2).filter(|w| d(w[0]) == d(w[1])).count();
            for how in [TransKey::Pairs, TransKey::Bits] {
                assert_eq!(section_by(&centers, eye, how), old, "case {case} ({n} quads, {how:?})");
            }
        }
        assert!(ties > 1000, "too few ties to test the stability ({ties})");
        // Whole column: sections split by `starts`, indices shifted by `base`.
        let (centers, eye) = trans_case(&mut s, 400);
        let starts = [0, 0, 120, 121, 400];
        let mut expect: Vec<u32> = Vec::new();
        for w in starts.windows(2) {
            let mut sec: Vec<u32> = (w[0]..w[1]).collect();
            sec.sort_by(|&a, &b| {
                let d = |i: u32| {
                    (0..3).map(|k| (centers[i as usize][k] - eye[k]) * (centers[i as usize][k] - eye[k])).sum::<f32>()
                };
                d(b).total_cmp(&d(a))
            });
            expect.extend(sec.iter().map(|i| i + 77));
        }
        assert_eq!(translucent_order(&centers, &starts, Some(eye), 77), expect);
        assert_eq!(translucent_order(&centers, &starts, None, 3), (3..403).collect::<Vec<u32>>());
    }

    /// Cost of the sort of a section (old, pairs, integer keys), 1 core then all:
    /// `cargo test --release -p rc-app trans_key_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn trans_key_cost() {
        use rayon::prelude::*;
        let mut s = 7u64;
        for n in [8, 64, 512, 4096] {
            let cases: Vec<_> = (0..(200_000 / n).max(50)).map(|_| trans_case(&mut s, n)).collect();
            for how in [TransKey::Old, TransKey::Pairs, TransKey::Bits] {
                for threads in [1, 4, rayon::current_num_threads()] {
                    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().expect("pool");
                    let t = std::time::Instant::now();
                    let k: u64 =
                        pool.install(|| cases.par_iter().map(|(c, e)| u64::from(section_by(c, *e, how)[0])).sum());
                    let ns = t.elapsed().as_secs_f64() * 1e9 / (cases.len() * n) as f64;
                    eprintln!("sort {how:?}, sections of {n}: {ns:.1} ns per quad ({threads} threads, {k})");
                }
            }
        }
    }

    /// Re-sort rule of the game written separately (`docs/specs/visibility.md`): section offset bounded per
    /// axis; re-sort if different from the last, or block changed and a zero component.
    fn reference_resorts(
        last: &mut [Option<[i32; 3]>],
        starts: &[u32],
        eye: [f64; 3],
        col: (i32, i32),
        sy0: i32,
        moved: bool,
    ) -> Vec<usize> {
        let mut out = Vec::new();
        for s in 0..starts.len() - 1 {
            if starts[s] == starts[s + 1] {
                continue;
            }
            let rel = |e: f64, sec: i32| (e.floor() as i64).div_euclid(16) as i32 - sec;
            let p = [rel(eye[0], col.0), rel(eye[1], sy0 + s as i32), rel(eye[2], col.1)].map(|v| v.clamp(-1, 1));
            if last[s] != Some(p) || moved && p.contains(&0) {
                last[s] = Some(p);
                out.push(s);
            }
        }
        out
    }

    /// `RUSTCRAFT_OPT_TRANSPOV`: hand-written cases, then scripted camera path (fixed seed)
    /// over several columns, sections re-sorted at each step == written rule.
    #[test]
    fn pov_resorts_follow_the_written_rule() {
        // Column (5, 5), sections 4 (y 0..16) and 10 (y 96..112) translucent; bottom of the world −64.
        let starts = [[0u32; 5].as_slice(), &[3; 6], &[9; 14]].concat();
        let (col, sy0) = ((5, 5), -4);
        let mut povs = column_povs(None, &starts, col, sy0);
        let mut out = Vec::new();
        let mut step = |eye: [f64; 3], moved: bool| {
            pov_resorts(&mut povs, &starts, eye, col, sy0, moved, &mut out);
            out.clone()
        };
        assert_eq!(step([8.5, 100.5, 8.5], true), [4, 10], "never sorted");
        assert_eq!(step([9.5, 100.5, 8.5], true), [10], "block changed: only the section on the axis (y)");
        assert_eq!(step([9.7, 100.5, 8.5], false), [] as [usize; 0], "same block, same viewpoint");
        assert_eq!(step([10.5, 140.5, 8.5], true), [10], "above: viewpoint of 10 changed");
        assert_eq!(step([80.5, 140.5, 8.5], true), [4, 10], "within the x slab of the column");
        assert_eq!(step([81.5, 140.5, 8.5], true), [4, 10], "on the x axis: re-sorted at each block");
        assert_eq!(step([81.5, 140.5, 8.5], false), [] as [usize; 0]);
        assert_eq!(step([200.0, 300.0, 300.0], true), [4, 10]);
        assert_eq!(step([201.0, 300.0, 300.0], true), [] as [usize; 0], "far away, off axis: never");
        // Scripted path: flights, falls, section jumps, over 9 columns around the origin.
        let mut s = 0x1606_0001u64;
        let cols: Vec<(i32, i32)> = (-1..=1).flat_map(|x| (-1..=1).map(move |z| (x * 3, z * 2))).collect();
        let layouts: Vec<Vec<u32>> = cols
            .iter()
            .map(|_| {
                let mut at = 0;
                (0..=24)
                    .map(|_| {
                        let next = at + u32::from(lcg(&mut s).is_multiple_of(3)) * 4;
                        std::mem::replace(&mut at, next)
                    })
                    .collect()
            })
            .collect();
        let mut mine: Vec<Vec<u8>> = layouts.iter().zip(&cols).map(|(st, &c)| column_povs(None, st, c, sy0)).collect();
        let mut theirs: Vec<Vec<Option<[i32; 3]>>> = layouts.iter().map(|st| vec![None; st.len() - 1]).collect();
        let (mut eye, mut last_block) = ([0.5f64, 70.0, 0.5], None);
        let mut resorted = 0;
        for t in 0..2000 {
            let r = |s: &mut u64, k: f64| (lcg(s) % 2001) as f64 / 1000.0 * k - k;
            let k = if t % 97 == 0 { 40.0 } else { 0.7 };
            eye = [eye[0] + r(&mut s, k), (eye[1] + r(&mut s, k)).clamp(-80.0, 330.0), eye[2] + r(&mut s, k)];
            let block = eye.map(|v| v.floor() as i64);
            let moved = last_block != Some(block);
            last_block = Some(block);
            for (i, &c) in cols.iter().enumerate() {
                pov_resorts(&mut mine[i], &layouts[i], eye, c, sy0, moved, &mut out);
                let want = reference_resorts(&mut theirs[i], &layouts[i], eye, c, sy0, moved);
                assert_eq!(out, want, "step {t}, column {c:?}, camera {eye:?}");
                resorted += out.len();
            }
        }
        assert!(resorted > 1000, "path too poor ({resorted} re-sorts)");
    }

    /// Columns examined: the 25 nearby first, then the rotation without them, which resumes where it
    /// stopped and covers max(n / 8, 15) keys.
    #[test]
    fn pov_picks_scan_near_then_window() {
        let keys: Vec<(i32, i32)> = (0..40).flat_map(|x| (0..5).map(move |z| (x, z))).collect();
        let (picks, next) = pov_picks(&keys, 190, (0, 0));
        let picks: Vec<_> = picks.collect();
        assert_eq!(&picks[..25], (-2..=2).flat_map(|x| (-2..=2).map(move |z| (x, z))).collect::<Vec<_>>());
        let window: Vec<_> = (0..25).map(|i| keys[(190 + i) % 200]).filter(|k| k.0 > 2 || k.1 > 2).collect();
        assert_eq!(picks[25..], window);
        assert_eq!(next, 15);
        let few = [(9, 9), (0, 0)];
        let (p, next) = pov_picks(&few, 1, (0, 0));
        assert_eq!(p.skip(25).collect::<Vec<_>>(), [(9, 9)]);
        assert_eq!(next, 1);
    }

    /// Depth (after division) of a point `d` blocks in front of the camera.
    fn depth_at(proj: Mat4, d: f32) -> f32 {
        let c = proj * Vec4::new(0.0, 0.0, -d, 1.0);
        c.z / c.w
    }

    #[test]
    fn depth_is_reversed_like_the_game() {
        let p = perspective(70.0, 16.0 / 9.0, 0.05, 512.0);
        assert!((depth_at(p, 0.05) - 1.0).abs() < 1e-6);
        assert!(depth_at(p, 512.0).abs() < 1e-6);
        assert!(depth_at(p, 10.0) > depth_at(p, 11.0), "nearer = greater");
        assert_eq!(DEPTH_CLEAR, 0.0);
        assert_eq!(DEPTH_NEARER, wgpu::CompareFunction::GreaterEqual);
    }

    /// Carpet (1/16 of a block) above the ground at 300 blocks: in classic depth the two faces
    /// fall on the same float or nearly (flicker), in reversed depth they
    /// stay thousands of steps apart.
    #[test]
    fn far_coplanar_faces_stay_apart() {
        let ulps = |a: f32, b: f32| (a.to_bits() as i64 - b.to_bits() as i64).abs();
        let (d0, d1) = (300.0, 300.0 + 1.0 / 16.0);
        let classic = glam::camera::rh::proj::directx::perspective(70f32.to_radians(), 16.0 / 9.0, 0.05, 512.0);
        assert!(ulps(depth_at(classic, d0), depth_at(classic, d1)) <= 2);
        let reversed = perspective(70.0, 16.0 / 9.0, 0.05, 512.0);
        assert!(ulps(depth_at(reversed, d0), depth_at(reversed, d1)) > 1000);
    }

    /// Sort centre of a non-planar quad: middle of the 0–2 diagonal (the game), not the mean
    /// of the four vertices (sloping water surface).
    #[test]
    fn quad_center_is_the_v0_v2_midpoint() {
        let p = [[0.0, 1.0, 0.0], [0.0, 0.5, 1.0], [1.0, 0.25, 1.0], [1.0, 0.875, 0.0]];
        assert_eq!(quad_center(&p), [0.5, 0.625, 0.5]);
        let mean: f32 = p.iter().map(|v| v[1]).sum::<f32>() / 4.0;
        assert_ne!(quad_center(&p)[1], mean);
    }

    #[test]
    fn coplanar_surface_needs_no_sort() {
        // Surface of a lake with its underside faces: same plane, a single one per cell and direction.
        let q: Vec<_> =
            (0..4).flat_map(|i| [flat(i as f32, 0.0, 15.875, true), flat(i as f32, 0.0, 15.875, false)]).collect();
        assert!(order_free(&q, &[0]));
        // Two sections, each planar at its own height.
        let q = vec![
            flat(0.0, 0.0, 3.0, true),
            flat(1.0, 0.0, 3.0, true),
            flat(0.0, 0.0, 20.0, true),
            flat(1.0, 0.0, 20.0, true),
        ];
        assert!(order_free(&q, &[0, 2]));
    }

    #[test]
    fn overlap_or_depth_needs_sort() {
        // Two different planes in a section.
        assert!(!order_free(&[flat(0.0, 0.0, 3.0, true), flat(1.0, 0.0, 4.0, true)], &[0]));
        // Same cell, same direction: overlap.
        assert!(!order_free(&[flat(0.0, 0.0, 3.0, true), flat(0.0, 0.0, 3.0, true)], &[0]));
        // Quad straddling two cells.
        let mut wide = flat(0.0, 0.0, 3.0, true);
        wide.corners[2][0] = 2.0;
        wide.corners[3][0] = 2.0;
        assert!(!order_free(&[wide, flat(5.0, 0.0, 3.0, true)], &[0]));
        // Sloping surface (flowing water).
        let mut slope = flat(0.0, 0.0, 3.0, true);
        slope.corners[0][1] = 2.5;
        assert!(!order_free(&[slope, flat(1.0, 0.0, 3.0, true)], &[0]));
    }
}
