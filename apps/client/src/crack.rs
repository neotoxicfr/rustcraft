//! Mining cracks (`docs/specs/interactions.md` § 2.2): localized pass that redraws the
//! faces of the cracked blocks' model with the texture `destroy_stage_<stage>` (`destroy_oit_stage_<stage>` for translucent blocks in
//! improved transparency), with projected UVs and
//! multiplicative blending.
//!
//! The vertices go through the uploader (its transit ring under `RUSTCRAFT_OPT_FRAME_WRITES_RING`).
//! Switches of [`CrackOptions`], read once (default 1, `=0` = the path before):
//! `RUSTCRAFT_OPT_CRACK_BUFFER_REUSE`, `RUSTCRAFT_OPT_CRACK_ROTATION_TABLE`,
//! `RUSTCRAFT_OPT_CRACK_GEOMETRY_CACHE`.

use std::sync::{Arc, OnceLock, Weak};

use glam::{Quat, Vec3};
use rc_assets::BlockModels;
use rc_data::VersionData;

use crate::render::upload::Uploader;
use crate::render::upload::per_frame::FrameBuffer;
use crate::small_layers::READ_BACK;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pos: [f32; 3],
    uv: [f32; 2],
    stage: u32,
}

/// Paths of the cracks; the tests set them per renderer to compare them in one process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrackOptions {
    /// The vertex lists and the model face list are kept from one frame to the next, instead of
    /// lists allocated on every frame (`RUSTCRAFT_OPT_CRACK_BUFFER_REUSE`).
    pub buffer_reuse: bool,
    /// The six rotations of the projected UVs are computed once, instead of once per vertex
    /// (`RUSTCRAFT_OPT_CRACK_ROTATION_TABLE`).
    pub rotation_table: bool,
    /// The vertices are rebuilt and sent only when the cracked blocks, their stages or the
    /// camera's integer origin change (`RUSTCRAFT_OPT_CRACK_GEOMETRY_CACHE`).
    pub geometry_cache: bool,
}

impl CrackOptions {
    #[cfg(test)]
    pub const NEW: Self = CrackOptions { buffer_reuse: true, rotation_table: true, geometry_cache: true };
    /// The paths before.
    #[cfg(test)]
    pub const OLD: Self = CrackOptions { buffer_reuse: false, rotation_table: false, geometry_cache: false };

    pub fn from_switches() -> Self {
        CrackOptions {
            buffer_reuse: rc_data::opt_switch!("RUSTCRAFT_OPT_CRACK_BUFFER_REUSE"),
            rotation_table: rc_data::opt_switch!("RUSTCRAFT_OPT_CRACK_ROTATION_TABLE"),
            geometry_cache: rc_data::opt_switch!("RUSTCRAFT_OPT_CRACK_GEOMETRY_CACHE"),
        }
    }
}

/// Rotation specific to each face (order down, up, north, south, west, east).
fn face_rotation(dir: usize) -> Quat {
    use std::f32::consts::{FRAC_PI_2, PI};
    let xyz = |x: f32, z: f32| Quat::from_rotation_x(x) * Quat::from_rotation_z(z);
    match dir {
        0 => Quat::from_rotation_x(PI),
        1 => Quat::IDENTITY,
        2 => xyz(FRAC_PI_2, PI),
        3 => Quat::from_rotation_x(FRAC_PI_2),
        4 => xyz(FRAC_PI_2, FRAC_PI_2),
        _ => xyz(FRAC_PI_2, -FRAC_PI_2),
    }
}

/// Rotation of the projected UVs of face `dir`: 180° around Y, −90° around X, then the face
/// rotation.
fn uv_rotation(dir: usize) -> Quat {
    face_rotation(dir)
        * Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)
        * Quat::from_rotation_y(std::f32::consts::PI)
}

/// [`uv_rotation`] of the six faces, computed once with the same products (so the same bits): it
/// costs up to four sine-cosine pairs and three quaternion products, paid for every vertex before.
fn uv_rotation_table() -> &'static [Quat; 6] {
    static TABLE: OnceLock<[Quat; 6]> = OnceLock::new();
    TABLE.get_or_init(|| std::array::from_fn(uv_rotation))
}

/// Projected UV of a vertex: block-local position turned by `rotation`; `uv = (−x, −y)`.
fn projected_uv(p: Vec3, rotation: Quat) -> [f32; 2] {
    let w = rotation * p;
    [-w.x, -w.y]
}

/// Random offset of the model (tall grass…), same rule as the mesher.
pub(crate) fn model_offset(data: &VersionData, state: u32, pos: [i32; 3]) -> Vec3 {
    let Some(b) = data.block_of(state) else { return Vec3::ZERO };
    let o = offset_of(b, pos);
    Vec3::new(o[0] as f32, o[1] as f32, o[2] as f32)
}

/// Random offset of a block at `pos`, in double precision like the game (zero without offset).
fn offset_of(b: &rc_data::BlockInfo, pos: [i32; 3]) -> [f64; 3] {
    if b.offset_kind == 0 {
        return [0.0; 3];
    }
    let seed = rc_assets::bake::position_seed(pos[0], 0, pos[2]);
    let h = |bits: i64| (((bits & 15) as f32 / 15.0) as f64 - 0.5) * 0.5;
    let (mh, mv) = (f64::from(b.offset_h), f64::from(b.offset_v));
    let x = h(seed).clamp(-mh, mh);
    let z = h(seed >> 8).clamp(-mh, mh);
    let y = if b.offset_kind == 2 { ((((seed >> 4) & 15) as f32 / 15.0) as f64 - 1.0) * mv } else { 0.0 };
    [x, y, z]
}

/// Offset to add to the shapes (outline, collision) of a state at `pos`. Blocks whose shape
/// follows the model offset (flowers, bamboo, bamboo shoot, propagule, dripstones) were
/// extracted at block (0, 0, 0): their shapes already carry that offset, hence the difference
/// offset(pos) − offset(0). The other offset blocks (grass, roots…) keep a fixed
/// shape: they are recognized by their shape centered on the block (the two families separate
/// exactly on the 26.3 data). Zero otherwise.
pub(crate) fn shape_shift(data: &VersionData, state: u32, pos: [i32; 3]) -> [f64; 3] {
    let (Some(s), Some(b)) = (data.state(state), data.block_of(state)) else { return [0.0; 3] };
    if b.offset_kind == 0 {
        return [0.0; 3];
    }
    let boxes = data.shape(s.outline);
    let at0 = offset_of(b, [0; 3]);
    if boxes.is_empty() || (at0[0] == 0.0 && at0[2] == 0.0) {
        return [0.0; 3];
    }
    let span = |a: usize| {
        let lo = boxes.iter().map(|x| x.min[a]).fold(f64::MAX, f64::min);
        let hi = boxes.iter().map(|x| x.max[a]).fold(f64::MIN, f64::max);
        (lo + hi) / 2.0
    };
    let baked = |a: usize| (span(a) - (0.5 + at0[a])).abs() < 1e-6;
    if !(baked(0) && baked(2)) {
        return [0.0; 3];
    }
    let o = offset_of(b, pos);
    [o[0] - at0[0], o[1] - at0[1], o[2] - at0[2]]
}

#[cfg(test)]
mod shape_shift_tests {
    use super::*;
    use rc_data::Aabb;

    /// Two offset blocks: flower (extracted shape already offset by −0.25) and grass (centered shape).
    fn data() -> VersionData {
        let d = VersionData {
            aabbs: vec![
                Aabb { min: [0.0625, 0.0, 0.0625], max: [0.4375, 0.625, 0.4375] },
                Aabb { min: [0.125, 0.0, 0.125], max: [0.875, 0.8125, 0.875] },
            ],
            shapes: vec![(0, 0), (0, 1), (1, 2)],
            ..Default::default()
        };
        let block = |name: &str, kind: u8, v: f32, first: u32| rc_data::BlockInfo {
            name: name.into(),
            first_state: first,
            state_count: 1,
            default_state: first,
            friction: 0.6,
            speed_factor: 1.0,
            jump_factor: 1.0,
            bounce: 0.0,
            inside: 0,
            offset_kind: kind,
            offset_h: 0.25,
            offset_v: v,
            dynamic_shape: false,
            properties: Vec::new(),
        };
        VersionData {
            blocks: vec![block("minecraft:dandelion", 1, 0.0, 0), block("minecraft:short_grass", 2, 0.2, 1)],
            states: vec![
                rc_data::StateInfo { block: 0, outline: 1, ..Default::default() },
                rc_data::StateInfo { block: 1, outline: 2, ..Default::default() },
            ],
            ..d
        }
    }

    /// The flower's shape follows its model (same offset as the render); the grass keeps
    /// its own. Without the correction (zero offset), the outline stayed stuck to the offset of block 0.
    #[test]
    fn flower_shape_follows_model_grass_does_not() {
        let d = data();
        let pos = [5, 64, -3];
        let shift = shape_shift(&d, 0, pos);
        let model = model_offset(&d, 0, pos);
        assert!(shift[0] != 0.0 || shift[2] != 0.0, "{shift:?}");
        // Game shape (0.3125 wide) + model offset at `pos`.
        assert!((0.0625 + shift[0] - (0.3125 + f64::from(model.x))).abs() < 1e-6, "{shift:?} {model:?}");
        assert!((0.0625 + shift[2] - (0.3125 + f64::from(model.z))).abs() < 1e-6, "{shift:?} {model:?}");
        assert_eq!(shift[1], 0.0);
        assert_eq!(shape_shift(&d, 1, pos), [0.0; 3]);
        // At block (0, 0, 0), the extracted shape is already the right one.
        assert_eq!(shape_shift(&d, 0, [0, 70, 0]), [0.0; 3]);
    }

    /// Bench: cost of one call (target: a dozen blocks crossed per frame). Run with
    /// `--ignored --nocapture`.
    #[test]
    #[ignore]
    fn bench_shape_shift() {
        let d = data();
        for (state, label) in [(0u32, "flower"), (1, "grass")] {
            let t = std::time::Instant::now();
            let mut acc = 0.0;
            for i in 0..1_000_000 {
                acc += shape_shift(&d, state, [i & 1023, 64, i >> 10])[0];
            }
            eprintln!("shape_shift {label}: {:.1} ns/call ({acc})", t.elapsed().as_secs_f64() * 1e3);
        }
    }
}

/// A cracked block of the frame: position, block state and crack stage (0 to 9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrackedBlock {
    pub pos: [i32; 3],
    pub state: u32,
    pub stage: u8,
}

/// Crack vertices of the frame: those of the opaque blocks (solid pass) and of the translucent
/// ones (late pass).
#[derive(Default)]
struct CrackVertices {
    opaque: Vec<Vertex>,
    translucent: Vec<Vertex>,
    /// Faces of the model of the block being added.
    model_faces: Vec<u32>,
}

impl CrackVertices {
    /// Adds the crack of `block` (two triangles per face of its model), relative to the integer
    /// origin of the camera `origin` (x, z), absolute y. A block with a translucent face goes
    /// whole to the late pass.
    fn add_block(
        &mut self,
        models: &BlockModels,
        data: &VersionData,
        block: CrackedBlock,
        origin: [i32; 2],
        rotation_table: bool,
    ) {
        let CrackedBlock { pos, state, stage } = block;
        let seed_offset = data.state(state).map_or([0; 3], |s| s.seed_offset());
        models.select(state, seed_offset, pos, &mut self.model_faces);
        let face = |q: &u32| &models.quads[*q as usize];
        let translucent = self.model_faces.iter().any(|q| face(q).layer == rc_assets::bake::Layer::Translucent);
        let out = if translucent { &mut self.translucent } else { &mut self.opaque };
        let offset = model_offset(data, state, pos);
        let corner = Vec3::new((pos[0] - origin[0]) as f32, pos[1] as f32, (pos[2] - origin[1]) as f32);
        let rotation = |dir: usize| if rotation_table { uv_rotation_table()[dir.min(5)] } else { uv_rotation(dir) };
        for q in &self.model_faces {
            let quad = face(q);
            let dir = quad.dir as usize;
            let v: [Vertex; 4] = std::array::from_fn(|i| {
                let local = Vec3::from(quad.pos[i]) + offset;
                let p = corner + local;
                Vertex { pos: p.to_array(), uv: projected_uv(local, rotation(dir)), stage: u32::from(stage) }
            });
            out.extend_from_slice(&[v[0], v[1], v[2], v[0], v[2], v[3]]);
        }
    }
}

/// What the kept vertices were built from. The block models are told apart by their allocation
/// (held weakly: its address cannot be reused while this lives), so a resource reload rebuilds;
/// the version data never changes during a session.
struct BuiltFrom {
    blocks: Vec<CrackedBlock>,
    origin: [i32; 2],
    models: Weak<BlockModels>,
}

impl BuiltFrom {
    fn is(&self, models: &Arc<BlockModels>, blocks: &[CrackedBlock], origin: [i32; 2]) -> bool {
        self.blocks == blocks && self.origin == origin && std::ptr::eq(self.models.as_ptr(), Arc::as_ptr(models))
    }
}

/// CPU side of the cracks: the frame's vertices and what they were built from.
pub(crate) struct CrackGeometry {
    pub(crate) options: CrackOptions,
    vertices: CrackVertices,
    /// Source of `vertices`, while the geometry cache keeps them.
    built_from: Option<BuiltFrom>,
    /// `vertices` changed since the renderer last sent them.
    changed: bool,
    /// Test sabotage: the kept vertices are judged without the camera origin.
    #[cfg(test)]
    pub(crate) sabotage_origin_ignored: bool,
}

impl CrackGeometry {
    pub(crate) fn new(options: CrackOptions) -> Self {
        CrackGeometry {
            options,
            vertices: CrackVertices::default(),
            built_from: None,
            changed: false,
            #[cfg(test)]
            sabotage_origin_ignored: false,
        }
    }

    /// The frame's cracked blocks, seen from the integer origin of the camera `origin` (x, z).
    /// With the geometry cache, the same blocks from the same origin keep the vertices built for
    /// them (and nothing is sent): a crack only changes when its stage does, a few times a second.
    fn set_blocks(&mut self, models: &Arc<BlockModels>, data: &VersionData, blocks: &[CrackedBlock], origin: [i32; 2]) {
        let key_origin = self.key_origin(origin);
        if self.options.geometry_cache && self.built_from.as_ref().is_some_and(|b| b.is(models, blocks, key_origin)) {
            return;
        }
        if !self.options.buffer_reuse {
            // The path before: every list allocated anew on each frame.
            self.vertices = CrackVertices::default();
        }
        self.vertices.opaque.clear();
        self.vertices.translucent.clear();
        for &block in blocks {
            self.vertices.add_block(models, data, block, origin, self.options.rotation_table);
        }
        self.changed = true;
        if self.options.geometry_cache {
            self.remember_source(models, blocks, key_origin);
        }
    }

    /// Origin as the geometry cache compares it.
    fn key_origin(&self, origin: [i32; 2]) -> [i32; 2] {
        #[cfg(test)]
        if self.sabotage_origin_ignored {
            return [0; 2];
        }
        origin
    }

    fn remember_source(&mut self, models: &Arc<BlockModels>, blocks: &[CrackedBlock], origin: [i32; 2]) {
        let built =
            self.built_from.get_or_insert_with(|| BuiltFrom { blocks: Vec::new(), origin, models: Weak::new() });
        built.blocks.clear();
        built.blocks.extend_from_slice(blocks);
        built.origin = origin;
        built.models = Arc::downgrade(models);
    }
}

pub struct Cracks {
    pipeline: wgpu::RenderPipeline,
    bind: wgpu::BindGroup,
    texture: wgpu::Texture,
    loaded: bool,
    /// Vertices of the opaque blocks then of the translucent ones.
    vertex_buffer: FrameBuffer,
    pub(crate) geometry: CrackGeometry,
    /// Vertices on the GPU: opaque blocks, and all.
    opaque_count: u32,
    total_count: u32,
    /// Layouts (camera, cracks) for the pipelines made on demand.
    layouts: (wgpu::BindGroupLayout, wgpu::BindGroupLayout),
    /// Improved transparency.
    oit: Option<crate::oit::Set>,
}

impl Cracks {
    /// Improved-transparency pipelines (once): back faces culled.
    pub fn ensure_oit(&mut self, device: &wgpu::Device, layouts: &crate::oit::Layouts) {
        if self.oit.is_some() {
            return;
        }
        let module =
            layouts.module(device, "cracks (transparency)", include_str!("crack.wgsl"), include_str!("crack_oit.wgsl"));
        let groups = [Some(&self.layouts.0), Some(&self.layouts.1)];
        let buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Uint32],
        })];
        let spec = crate::oit::Spec::new("cracks (transparency)", &groups, &module, "vs", &buffers, "fs").cull(true);
        self.oit = Some(layouts.set(device, &spec));
    }

    /// Cracks of translucent blocks alone (those of opaque ones stay in the solids pass),
    /// stage `f` of improved transparency, with the `destroy_oit_stage_<n>` texture set; group 0 (camera) bound.
    pub fn draw_oit(&self, pass: &mut wgpu::RenderPass<'_>, f: crate::oit::Frame<'_>) {
        let (Some(buf), Some(set)) = (self.vertex_buffer.buffer(), &self.oit) else { return };
        if self.opaque_count >= self.total_count || !self.loaded {
            return;
        }
        f.set(pass, set);
        pass.set_bind_group(1, &self.bind, &[]);
        pass.set_vertex_buffer(0, buf.slice(..));
        pass.draw(self.opaque_count..self.total_count, 0..1);
    }

    pub fn new(device: &wgpu::Device, camera_layout: &wgpu::BindGroupLayout, format: wgpu::TextureFormat) -> Self {
        Self::with_options(device, camera_layout, format, CrackOptions::from_switches())
    }

    pub(crate) fn with_options(
        device: &wgpu::Device,
        camera_layout: &wgpu::BindGroupLayout,
        format: wgpu::TextureFormat,
        options: CrackOptions,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cracks"),
            source: wgpu::ShaderSource::Wgsl(include_str!("crack.wgsl").into()),
        });
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("destroy stages"),
            size: wgpu::Extent3d { width: 16, height: 16, depth_or_array_layers: 20 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("cracks"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            ..Default::default()
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cracks"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cracks"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cracks"),
            bind_group_layouts: &[Some(camera_layout), Some(&layout)],
            immediate_size: 0,
        });
        // Multiplicative blending: color = src · dst + dst · src; alpha unchanged.
        let blend = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Dst,
                dst_factor: wgpu::BlendFactor::Src,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::Zero,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cracks"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Uint32],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(blend),
                    write_mask: wgpu::ColorWrites::COLOR,
                })],
            }),
            primitive: wgpu::PrimitiveState { cull_mode: Some(wgpu::Face::Back), ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(false),
                depth_compare: Some(crate::render::DEPTH_NEARER),
                stencil: Default::default(),
                // Offset toward the camera (slope 1, constant 10) to pass in front of the face.
                bias: crate::render::depth_bias(10, 1.0),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        Cracks {
            pipeline,
            bind,
            texture,
            loaded: false,
            vertex_buffer: FrameBuffer::new("cracks", wgpu::BufferUsages::VERTEX | READ_BACK, 4096),
            geometry: CrackGeometry::new(options),
            opaque_count: 0,
            total_count: 0,
            layouts: (camera_layout.clone(), layout),
            oit: None,
        }
    }

    pub fn loaded(&self) -> bool {
        self.loaded
    }

    /// The 20 images: `destroy_stage_<n>` (layers 0 to 9) then `destroy_oit_stage_<n>` (10 to 19), 16 × 16
    /// expected, otherwise resized.
    pub fn load(&mut self, queue: &wgpu::Queue, images: &[rc_assets::texture::Image]) {
        for (layer, img) in images.iter().take(20).enumerate() {
            let img = if img.width == 16 && img.height == 16 { img.clone() } else { img.resized(16) };
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: 0, y: 0, z: layer as u32 },
                    aspect: wgpu::TextureAspect::All,
                },
                &img.rgba,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(16 * 4), rows_per_image: Some(16) },
                wgpu::Extent3d { width: 16, height: 16, depth_or_array_layers: 1 },
            );
        }
        self.loaded = true;
    }

    /// The frame's cracked blocks, seen from the integer origin of the camera `origin` (x, z).
    pub fn set_blocks(
        &mut self,
        models: &Arc<BlockModels>,
        data: &VersionData,
        blocks: &[CrackedBlock],
        origin: [i32; 2],
    ) {
        self.geometry.set_blocks(models, data, blocks, origin);
    }

    /// Sends the vertices that changed since the last call through the uploader, the opaque and
    /// translucent ones copied side by side straight into the transit (no joined list).
    pub fn upload(&mut self, queue: &wgpu::Queue, up: &mut Uploader) {
        if !std::mem::take(&mut self.geometry.changed) {
            return;
        }
        let vertices = &self.geometry.vertices;
        let opaque: &[u8] = bytemuck::cast_slice(&vertices.opaque);
        let translucent: &[u8] = bytemuck::cast_slice(&vertices.translucent);
        self.opaque_count = vertices.opaque.len() as u32;
        self.total_count = self.opaque_count + vertices.translucent.len() as u32;
        self.vertex_buffer.write_with(up, queue, (opaque.len() + translucent.len()) as u64, |mut transit| {
            transit.slice(..opaque.len()).copy_from_slice(opaque);
            transit.slice(opaque.len()..).copy_from_slice(translucent);
        });
    }

    /// Drawing of the opaque cracks (`late` false, with the solid pass) or translucent ones (after the
    /// translucent terrain); group 0 (camera) is already bound.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, late: bool) {
        let Some(buf) = self.vertex_buffer.buffer() else { return };
        let range = if late { self.opaque_count..self.total_count } else { 0..self.opaque_count };
        if range.is_empty() || !self.loaded {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(1, &self.bind, &[]);
        pass.set_vertex_buffer(0, buf.slice(..));
        pass.draw(range, 0..1);
    }

    /// Distance from `eye`, in blocks, of the farthest crack of the frame (0 without cracks):
    /// the cracks multiply the pixel under them, so plain fog does not turn them into its colour
    /// (`render::fog_cull`).
    pub fn farthest_vertex(&self, eye: Vec3) -> f32 {
        // The kept vertices are rebuilt whenever the camera origin changes: both lists are always
        // in the frame of this frame's camera.
        let vertices = &self.geometry.vertices;
        farthest_vertex(&vertices.opaque, eye).max(farthest_vertex(&vertices.translucent, eye))
    }
}

/// Distance from `eye` of the vertex of `vertices` farthest from it, both in the frame of the
/// world camera; 0 without vertices.
fn farthest_vertex(vertices: &[Vertex], eye: Vec3) -> f32 {
    vertices.iter().map(|vertex| Vec3::from(vertex.pos).distance(eye)).fold(0.0, f32::max)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use rc_assets::bake::{Layer, Part, StateModel};
    use rc_assets::geom::Dir;
    use rc_assets::{BakedQuad, FluidTextures};

    /// The six faces of the box `min`..`max`, counter-clockwise seen from outside, in the
    /// direction order (down, up, north, south, west, east).
    fn box_faces(a: [f32; 3], b: [f32; 3], layer: Layer) -> Vec<BakedQuad> {
        let corners = [
            [[a[0], a[1], a[2]], [b[0], a[1], a[2]], [b[0], a[1], b[2]], [a[0], a[1], b[2]]],
            [[a[0], b[1], a[2]], [a[0], b[1], b[2]], [b[0], b[1], b[2]], [b[0], b[1], a[2]]],
            [[a[0], a[1], a[2]], [a[0], b[1], a[2]], [b[0], b[1], a[2]], [b[0], a[1], a[2]]],
            [[a[0], a[1], b[2]], [b[0], a[1], b[2]], [b[0], b[1], b[2]], [a[0], b[1], b[2]]],
            [[a[0], a[1], a[2]], [a[0], a[1], b[2]], [a[0], b[1], b[2]], [a[0], b[1], a[2]]],
            [[b[0], a[1], a[2]], [b[0], b[1], a[2]], [b[0], b[1], b[2]], [b[0], a[1], b[2]]],
        ];
        let dirs = [Dir::Down, Dir::Up, Dir::North, Dir::South, Dir::West, Dir::East];
        let face = |(pos, dir)| BakedQuad {
            pos,
            uv: [[0.0; 2]; 4],
            texture: 0,
            tint: -1,
            cull: None,
            dir,
            shade_dir: dir,
            layer,
            ao: false,
            emission: 0,
        };
        corners.into_iter().zip(dirs).map(face).collect()
    }

    /// Block models of the proofs: state 0 a full cube, state 1 a translucent slab (late pass),
    /// state 2 a thin cutout post, state 3 a model drawn among two variants by its position.
    pub(crate) fn test_models() -> Arc<BlockModels> {
        let mut quads = box_faces([0.0; 3], [1.0; 3], Layer::Solid);
        quads.extend(box_faces([0.0; 3], [1.0, 0.5, 1.0], Layer::Translucent));
        quads.extend(box_faces([0.375, 0.0, 0.375], [0.625, 1.0, 0.625], Layer::Cutout));
        quads.extend(box_faces([0.0, 0.0, 0.0], [1.0, 0.25, 1.0], Layer::Solid));
        let model = |options: &[u32]| StateModel {
            parts: vec![Part {
                total_weight: options.len() as u32,
                options: options.iter().map(|&start| (1, start, 6)).collect(),
            }],
            multipart: false,
        };
        let names = |list: &[&str]| list.iter().map(|n| (*n).to_owned()).collect();
        Arc::new(BlockModels {
            fluids: FluidTextures {
                water_still: 0,
                water_flow: 0,
                water_overlay: 0,
                lava_still: 0,
                lava_flow: 0,
                water_layer: Layer::Translucent,
                lava_layer: Layer::Solid,
            },
            quads,
            states: vec![model(&[0]), model(&[6]), model(&[12]), model(&[18, 0])],
            textures: Vec::new(),
            texture_names: names(&["minecraft:block/fire_1", "minecraft:block/nether_portal", "minecraft:block/stone"]),
            animations: Vec::new(),
            texture_size: 16,
            big_texture_size: 0,
            missing: Default::default(),
            state_particles: vec![0; 4],
            particle_names: names(&["minecraft:block/stone"]),
        })
    }

    struct Rng(u64);
    impl Rng {
        fn below(&mut self, n: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0 % n
        }
    }

    /// A frame of the script: cracked blocks in position order, and the camera's integer origin.
    type Frame = (Vec<CrackedBlock>, [i32; 2]);

    /// 60 frames over 50 cracked blocks (states with a model or without, every stage): the same
    /// frame repeated, stages moving on, blocks coming and going, the camera origin moving alone.
    fn script() -> Vec<Frame> {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let mut block = |_| CrackedBlock {
            pos: [rng.below(64) as i32 - 32, rng.below(40) as i32 + 60, rng.below(64) as i32 - 32],
            // State 9 has no model: nothing is drawn for it.
            state: [0, 1, 2, 3, 9][rng.below(5) as usize],
            stage: rng.below(10) as u8,
        };
        let all: Vec<CrackedBlock> = (0..50).map(&mut block).collect();
        let mut frames = Vec::new();
        for n in 0..60usize {
            let mut blocks: Vec<CrackedBlock> = all.iter().copied().skip(n % 7).take(1 + n % 9).collect();
            for b in &mut blocks {
                b.stage = ((usize::from(b.stage) + n / 4) % 10) as u8;
            }
            blocks.sort_unstable_by_key(|b| b.pos);
            blocks.dedup_by_key(|b| b.pos);
            // Every third frame repeats the blocks of the frame before; the origin moves alone at
            // times (frames 20 and 50).
            let origin = [(n / 10) as i32 - 2, 3 - (n / 15) as i32];
            let previous = frames.last().filter(|_| n % 3 == 2).map(|(blocks, _): &Frame| blocks.clone());
            frames.push((previous.unwrap_or(blocks), origin));
        }
        frames
    }

    /// The crack vertices as the code before this module's switches built them (kept here as the
    /// reference): one list per block, its faces walked once with the three rotations of the
    /// projected UVs recomputed for every vertex, a block with a translucent face moved whole to
    /// the late list.
    fn vertices_before(
        models: &BlockModels,
        data: &VersionData,
        blocks: &[CrackedBlock],
        origin: [i32; 2],
    ) -> (Vec<Vertex>, Vec<Vertex>) {
        let (mut early, mut late) = (Vec::new(), Vec::new());
        for &CrackedBlock { pos, state, stage } in blocks {
            let seed_offset = data.state(state).map_or([0; 3], |s| s.seed_offset());
            let mut selected = Vec::new();
            models.select(state, seed_offset, pos, &mut selected);
            let offset = model_offset(data, state, pos);
            let corner = Vec3::new((pos[0] - origin[0]) as f32, pos[1] as f32, (pos[2] - origin[1]) as f32);
            let (mut vertices, mut translucent) = (Vec::new(), false);
            for &q in &selected {
                let quad = &models.quads[q as usize];
                translucent |= quad.layer == Layer::Translucent;
                let v: [Vertex; 4] = std::array::from_fn(|i| {
                    let local = Vec3::from(quad.pos[i]) + offset;
                    let turned = face_rotation(quad.dir as usize)
                        * Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)
                        * Quat::from_rotation_y(std::f32::consts::PI)
                        * local;
                    Vertex { pos: (corner + local).to_array(), uv: [-turned.x, -turned.y], stage: u32::from(stage) }
                });
                vertices.extend_from_slice(&[v[0], v[1], v[2], v[0], v[2], v[3]]);
            }
            if translucent { late.extend(vertices) } else { early.extend(vertices) }
        }
        (early, late)
    }

    /// The crack vertices reach the GPU as those of the reference, opaque then translucent, byte
    /// for byte, through the uploader with and without its transit ring, and the draw ranges
    /// follow: over frames with both kinds, only opaque blocks, only translucent ones, none, and a
    /// buffer that grows.
    #[test]
    fn crack_vertices_reach_the_gpu_unchanged() {
        use crate::render::upload::tests::{gpu, read};
        let Some((device, queue)) = gpu() else { return };
        let camera = crate::render::camera_layout(&device, false);
        let (models, data) = (test_models(), VersionData::default());
        // States 0 and 2 are opaque, state 1 is translucent.
        let block = |i: i32, state: u32| CrackedBlock { pos: [i, 64, -i], state, stage: (i % 10) as u8 };
        let frames: [Vec<CrackedBlock>; 5] = [
            vec![block(0, 0), block(1, 1), block(2, 2)],
            vec![block(3, 0)],
            vec![block(4, 1)],
            Vec::new(),
            (0..40).map(|i| block(i, (i % 3) as u32)).collect(),
        ];
        for (ring, options) in [
            (false, CrackOptions::OLD),
            (true, CrackOptions::OLD),
            (false, CrackOptions::NEW),
            (true, CrackOptions::NEW),
        ] {
            let mut cracks = Cracks::with_options(&device, &camera, wgpu::TextureFormat::Rgba8Unorm, options);
            let mut up = Uploader::new(&device, ring);
            for (n, blocks) in frames.iter().enumerate() {
                cracks.set_blocks(&models, &data, blocks, [0, 0]);
                cracks.upload(&queue, &mut up);
                up.submit(&queue, device.create_command_encoder(&Default::default()).finish());
                let (opaque, translucent) = vertices_before(&models, &data, blocks, [0, 0]);
                let mut want = bytemuck::cast_slice::<Vertex, u8>(&opaque).to_vec();
                want.extend_from_slice(bytemuck::cast_slice(&translucent));
                let buffer = cracks.vertex_buffer.buffer();
                let held = buffer.map_or(Vec::new(), |b| read(&device, &queue, b)[..want.len()].to_vec());
                assert!(held == want, "ring={ring} {options:?} frame {n}: vertices differ");
                let ranges = (opaque.len() as u32, (opaque.len() + translucent.len()) as u32);
                assert_eq!((cracks.opaque_count, cracks.total_count), ranges, "ring={ring} {options:?} frame {n}");
            }
        }
    }

    /// Vertices of each frame of `script` (opaque, translucent, as bytes) built by `geometry`.
    fn built_frames(models: &Arc<BlockModels>, geometry: &mut CrackGeometry) -> Vec<(Vec<u8>, Vec<u8>)> {
        let data = VersionData::default();
        let bytes = |v: &[Vertex]| bytemuck::cast_slice(v).to_vec();
        script()
            .iter()
            .map(|(blocks, origin)| {
                geometry.set_blocks(models, &data, blocks, *origin);
                (bytes(&geometry.vertices.opaque), bytes(&geometry.vertices.translucent))
            })
            .collect()
    }

    /// Every path of the cracks, alone and together (and with every switch off), builds the
    /// vertices of the reference to the bit over the script; the kept vertices compared without
    /// the camera origin (sabotage) do not.
    #[test]
    fn crack_paths_build_identical_vertices() {
        let models = test_models();
        let data = VersionData::default();
        let bytes = |v: &[Vertex]| bytemuck::cast_slice(v).to_vec();
        let reference = |(blocks, origin): &Frame| {
            let (opaque, translucent) = vertices_before(&models, &data, blocks, *origin);
            (bytes(&opaque), bytes(&translucent))
        };
        let want: Vec<(Vec<u8>, Vec<u8>)> = script().iter().map(reference).collect();
        let drawn: usize = want.iter().map(|(opaque, translucent)| opaque.len() + translucent.len()).sum();
        assert!(drawn > 100_000, "script nearly empty: {drawn} bytes of vertices");
        assert!(want.iter().any(|(_, translucent)| !translucent.is_empty()), "no translucent block in the script");
        let old = CrackOptions::OLD;
        for options in [
            old,
            CrackOptions { buffer_reuse: true, ..old },
            CrackOptions { rotation_table: true, ..old },
            CrackOptions { geometry_cache: true, ..old },
            CrackOptions::NEW,
        ] {
            let got = built_frames(&models, &mut CrackGeometry::new(options));
            let differing = got.iter().zip(&want).position(|(a, b)| a != b);
            assert_eq!(differing, None, "{options:?}: first differing frame");
        }
        let mut origin_ignored = CrackGeometry::new(CrackOptions::NEW);
        origin_ignored.sabotage_origin_ignored = true;
        assert!(built_frames(&models, &mut origin_ignored) != want, "kept vertices judged without the origin");
    }

    /// The table holds the rotation computed per vertex before, to the bit, for the six faces
    /// (and the last one for an out-of-range direction, like the match it replaces).
    #[test]
    fn rotation_table_holds_the_per_vertex_rotations() {
        for dir in 0..8 {
            let (table, direct) = (uv_rotation_table()[dir.min(5)], uv_rotation(dir));
            assert_eq!(table.to_array().map(f32::to_bits), direct.to_array().map(f32::to_bits), "face {dir}");
        }
        let distinct: hashbrown::HashSet<[u32; 4]> =
            uv_rotation_table().iter().map(|q| q.to_array().map(f32::to_bits)).collect();
        assert_eq!(distinct.len(), 6, "two faces share a rotation");
    }

    /// The kept vertices follow their source: rebuilt when the models are replaced (resource
    /// reload) even at the same blocks, kept otherwise.
    #[test]
    fn kept_vertices_follow_the_block_models() {
        let data = VersionData::default();
        let blocks = [CrackedBlock { pos: [1, 64, 2], state: 0, stage: 4 }];
        let mut geometry = CrackGeometry::new(CrackOptions::NEW);
        let models = test_models();
        geometry.set_blocks(&models, &data, &blocks, [0, 0]);
        assert!(std::mem::take(&mut geometry.changed), "first build");
        geometry.set_blocks(&models, &data, &blocks, [0, 0]);
        assert!(!geometry.changed, "same source: nothing rebuilt");
        geometry.set_blocks(&test_models(), &data, &blocks, [0, 0]);
        assert!(geometry.changed, "other models: rebuilt");
    }

    /// Micro-bench (machine loaded by other agents' builds: an order of magnitude only): building
    /// the crack vertices of a frame, per path. One block being mined (the usual case) and eight.
    #[test]
    #[ignore = "micro-bench: cargo test --release -p rc-app crack_geometry_bench -- --ignored --nocapture"]
    fn crack_geometry_bench() {
        let (models, data) = (test_models(), VersionData::default());
        let block = |i: i32| CrackedBlock { pos: [i, 64, -i], state: (i % 3) as u32, stage: 5 };
        let old = CrackOptions::OLD;
        let paths = [
            ("before", old),
            ("kept buffers", CrackOptions { buffer_reuse: true, ..old }),
            ("rotation table", CrackOptions { rotation_table: true, ..old }),
            ("buffers + table", CrackOptions { geometry_cache: false, ..CrackOptions::NEW }),
            ("all (vertices kept)", CrackOptions::NEW),
        ];
        for count in [1, 8] {
            let blocks: Vec<CrackedBlock> = (0..count).map(block).collect();
            for (name, options) in paths {
                let mut geometry = CrackGeometry::new(options);
                let mut best = f64::MAX;
                for _round in 0..7 {
                    let t = std::time::Instant::now();
                    for _frame in 0..10_000 {
                        geometry.set_blocks(&models, &data, std::hint::black_box(&blocks), [0, 0]);
                    }
                    best = best.min(t.elapsed().as_secs_f64() * 1e9 / 10_000.0);
                }
                eprintln!("{count} cracked block(s), {name}: {best:.0} ns per frame");
            }
        }
    }

    #[test]
    fn farthest_vertex_is_measured_from_the_eye() {
        let eye = Vec3::new(0.5, 70.0, 0.5);
        assert_eq!(farthest_vertex(&[], eye), 0.0);
        let at = |pos: [f32; 3]| Vertex { pos, uv: [0.0; 2], stage: 0 };
        let cracks = [at([0.5, 70.0, 3.5]), at([0.5, 46.0, 0.5]), at([-6.5, 70.0, 0.5])];
        assert_eq!(farthest_vertex(&cracks, eye), 24.0);
        assert_eq!(farthest_vertex(&cracks[..1], eye), 3.0);
    }

    #[test]
    fn top_face_uv_follows_block_axes() {
        // Top face: the projection keeps x and z (up to sign) and ignores y.
        let a = projected_uv(Vec3::new(0.0, 1.0, 0.0), uv_rotation(1));
        let b = projected_uv(Vec3::new(1.0, 1.0, 0.0), uv_rotation(1));
        let c = projected_uv(Vec3::new(0.0, 1.0, 1.0), uv_rotation(1));
        let du = ((b[0] - a[0]).abs(), (b[1] - a[1]).abs());
        let dv = ((c[0] - a[0]).abs(), (c[1] - a[1]).abs());
        assert!((du.0 + du.1 - 1.0).abs() < 1e-5 && (dv.0 + dv.1 - 1.0).abs() < 1e-5);
        assert!(du.0 * dv.0 + du.1 * dv.1 < 1e-5, "orthogonal axes: {du:?} {dv:?}");
    }
}
