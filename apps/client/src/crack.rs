//! Mining cracks (`docs/specs/interactions.md` § 2.2): localized pass that redraws the
//! faces of the cracked blocks' model with the texture `destroy_stage_<stage>` (`destroy_oit_stage_<stage>` for translucent blocks in
//! improved transparency), with projected UVs and
//! multiplicative blending.

use glam::{Quat, Vec3};
use rc_assets::BlockModels;
use rc_data::VersionData;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pos: [f32; 3],
    uv: [f32; 2],
    stage: u32,
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

/// Projected UV of a vertex: block-local position rotated by 180° around Y, by −90°
/// around X, then by the face rotation; `uv = (−x, −y)`.
fn projected_uv(p: Vec3, dir: usize) -> [f32; 2] {
    let q = face_rotation(dir)
        * Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)
        * Quat::from_rotation_y(std::f32::consts::PI);
    let w = q * p;
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

/// Vertices of a block's crack (two triangles per face of the model), relative to the integer
/// origin of the camera `origin` (x, z), absolute y. Also returns "translucent" (late pass).
pub fn geometry(
    models: &BlockModels,
    data: &VersionData,
    pos: [i32; 3],
    state: u32,
    stage: u8,
    origin: [i32; 2],
    out: &mut Vec<Vertex>,
) -> bool {
    let seed_offset = data.state(state).map_or([0; 3], |s| s.seed_offset());
    let mut selected = Vec::new();
    models.select(state, seed_offset, pos, &mut selected);
    let offset = model_offset(data, state, pos);
    let corner = Vec3::new((pos[0] - origin[0]) as f32, pos[1] as f32, (pos[2] - origin[1]) as f32);
    let mut translucent = false;
    for &q in &selected {
        let quad = &models.quads[q as usize];
        translucent |= quad.layer == rc_assets::bake::Layer::Translucent;
        let dir = quad.dir as usize;
        let v: [Vertex; 4] = std::array::from_fn(|i| {
            let local = Vec3::from(quad.pos[i]) + offset;
            let p = corner + local;
            Vertex { pos: p.to_array(), uv: projected_uv(local, dir), stage: u32::from(stage) }
        });
        out.extend_from_slice(&[v[0], v[1], v[2], v[0], v[2], v[3]]);
    }
    translucent
}

pub struct Cracks {
    pipeline: wgpu::RenderPipeline,
    bind: wgpu::BindGroup,
    texture: wgpu::Texture,
    loaded: bool,
    buf: Option<(wgpu::Buffer, u64)>,
    /// Vertices of the opaque blocks then of the translucent ones.
    verts: Vec<Vertex>,
    early: u32,
    total: u32,
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
        let module = crate::oit::module(
            device,
            "cracks (transparency)",
            include_str!("crack.wgsl"),
            include_str!("crack_oit.wgsl"),
        );
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
        let (Some((buf, _)), Some(set)) = (&self.buf, &self.oit) else { return };
        if self.early >= self.total || !self.loaded {
            return;
        }
        f.set(pass, set);
        pass.set_bind_group(1, &self.bind, &[]);
        pass.set_vertex_buffer(0, buf.slice(..));
        pass.draw(self.early..self.total, 0..1);
    }

    pub fn new(device: &wgpu::Device, camera_layout: &wgpu::BindGroupLayout, format: wgpu::TextureFormat) -> Self {
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
            buf: None,
            verts: Vec::new(),
            early: 0,
            total: 0,
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

    /// Frame vertices: opaque blocks (`early`) then translucent ones (`late`).
    pub fn set(&mut self, early: Vec<Vertex>, late: Vec<Vertex>) {
        self.early = early.len() as u32;
        self.verts = early;
        self.verts.extend(late);
    }

    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.total = self.verts.len() as u32;
        if self.verts.is_empty() {
            return;
        }
        let bytes: &[u8] = bytemuck::cast_slice(&self.verts);
        if self.buf.as_ref().is_none_or(|(_, cap)| *cap < bytes.len() as u64) {
            let cap = (bytes.len() as u64).next_power_of_two().max(4096);
            self.buf = Some((
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("cracks"),
                    size: cap,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                cap,
            ));
        }
        queue.write_buffer(&self.buf.as_ref().expect("created above").0, 0, bytes);
    }

    /// Drawing of the opaque cracks (`late` false, with the solid pass) or translucent ones (after the
    /// translucent terrain); group 0 (camera) is already bound.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, late: bool) {
        let Some((buf, _)) = &self.buf else { return };
        let range = if late { self.early..self.total } else { 0..self.early };
        if range.is_empty() || !self.loaded {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(1, &self.bind, &[]);
        pass.set_vertex_buffer(0, buf.slice(..));
        pass.draw(range, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_face_uv_follows_block_axes() {
        // Top face: the projection keeps x and z (up to sign) and ignores y.
        let a = projected_uv(Vec3::new(0.0, 1.0, 0.0), 1);
        let b = projected_uv(Vec3::new(1.0, 1.0, 0.0), 1);
        let c = projected_uv(Vec3::new(0.0, 1.0, 1.0), 1);
        let du = ((b[0] - a[0]).abs(), (b[1] - a[1]).abs());
        let dv = ((c[0] - a[0]).abs(), (c[1] - a[1]).abs());
        assert!((du.0 + du.1 - 1.0).abs() < 1e-5 && (dv.0 + dv.1 - 1.0).abs() < 1e-5);
        assert!(du.0 * dv.0 + du.1 * dv.1 < 1e-5, "orthogonal axes: {du:?} {dv:?}");
    }
}
