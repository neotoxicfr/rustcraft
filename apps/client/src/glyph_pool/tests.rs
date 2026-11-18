//! Proofs of the shared pieces (back-face test, per-frame buffers, draw cutting) and the GPU
//! helpers of the 3D text captures (name tags, sign text, text displays).
use super::*;
use glam::{Mat4, Vec3};

/// Standing sign face facing +z at the origin (font pixel = 1/96 block, y downwards).
fn sign_face() -> Mat4 {
    Mat4::from_scale(Vec3::new(1.0 / 96.0, -1.0 / 96.0, 1.0 / 96.0))
}

fn test_at(eye: Vec3) -> BackFaceTest {
    BackFaceTest { eye, pixels_per_radian: 643.0 }
}

#[test]
fn back_faces_are_left_out_only_when_surely_behind() {
    let face = sign_face();
    // In front, beside, in the plane: kept.
    for eye in [Vec3::new(0.0, 0.0, 3.0), Vec3::new(5.0, 0.0, 0.01), Vec3::new(2.0, 1.0, 0.0)] {
        assert!(!test_at(eye).faces_away(&face, 48.0), "{eye}");
    }
    // Straight behind: left out, from close up to far away.
    for d in [1.0, 4.0, 16.0, 40.0] {
        assert!(test_at(Vec3::new(0.0, 0.0, -d)).faces_away(&face, 48.0), "behind at {d}");
    }
    // Behind but within the bobbing slack, or grazing from far away: kept for the GPU to decide.
    assert!(!test_at(Vec3::new(0.0, 0.0, -0.2)).faces_away(&face, 48.0));
    assert!(!test_at(Vec3::new(60.0, 0.0, -2.0)).faces_away(&face, 48.0));
    // The back face (half-turn) is the mirror case.
    let back = Mat4::from_rotation_y(std::f32::consts::PI) * face;
    assert!(test_at(Vec3::new(0.0, 0.0, 3.0)).faces_away(&back, 48.0));
    assert!(!test_at(Vec3::new(0.0, 0.0, -3.0)).faces_away(&back, 48.0));
    // Degenerate matrix or unknown view: never left out.
    assert!(!test_at(Vec3::new(0.0, 0.0, -3.0)).faces_away(&Mat4::ZERO, 48.0));
    let blind = BackFaceTest { eye: Vec3::new(0.0, 0.0, -3.0), pixels_per_radian: 0.0 };
    assert!(!blind.faces_away(&face, 48.0));
}

#[test]
fn frame_buffer_skips_only_what_the_gpu_already_holds() {
    let mut b = FrameBuffer::default();
    let first = b.plan(&[1, 2, 3, 4], true);
    assert_eq!(first, BufferPlan { recreate: Some(1024), write: true });
    assert_eq!(b.plan(&[1, 2, 3, 4], true), BufferPlan { recreate: None, write: false });
    // Shorter prefix of what is there: nothing to write either.
    assert_eq!(b.plan(&[1, 2], true), BufferPlan { recreate: None, write: false });
    assert_eq!(b.plan(&[1, 2, 3, 5], true), BufferPlan { recreate: None, write: true });
    // Growth: new buffer, content lost, written again even when identical.
    let big = vec![7u8; 2000];
    assert_eq!(b.plan(&big, true), BufferPlan { recreate: Some(2048), write: true });
    assert_eq!(b.plan(&big, true), BufferPlan { recreate: None, write: false });
    // Without skipping: written on every frame.
    assert_eq!(b.plan(&big, false), BufferPlan { recreate: None, write: true });
    assert_eq!(b.plan(&big, true), BufferPlan { recreate: None, write: true });
    assert!(!b.plan(&[], true).write);
}

#[test]
fn draws_stay_under_twice_their_glyphs() {
    let counts = [3u32, 4, 3, 120, 2, 5, 5, 1, 90, 90];
    let mut out = Vec::new();
    let mut s = DrawSplitter::new(10, true);
    for c in counts {
        s.add(c, &mut out);
    }
    s.finish(&mut out);
    let mut next = 10;
    let mut i = 0;
    for d in &out {
        assert_eq!(d.first, next, "contiguous");
        next += d.count;
        let glyphs: u32 = counts[i..i + d.count as usize].iter().sum();
        assert_eq!(d.glyphs_each, *counts[i..i + d.count as usize].iter().max().expect("non-empty"));
        assert!(d.glyphs_each * d.count <= 2 * glyphs || d.count == 1, "{d:?}");
        i += d.count as usize;
    }
    assert_eq!(i, counts.len());
}

// --- Copies of the GPU buffers that receive only the uploads ------------------------------------

/// Copy of a pool's GPU buffer.
#[derive(Default)]
pub(crate) struct PoolMirror(pub Vec<PooledGlyph>);

impl PoolMirror {
    pub fn apply(&mut self, pool: &GlyphPool, plan: PoolUpload) {
        if let Some(capacity) = plan.grow {
            self.0 = vec![<PooledGlyph as bytemuck::Zeroable>::zeroed(); capacity];
        }
        let new = pool.glyphs.get(plan.from..).unwrap_or_default();
        self.0[plan.from..plan.from + new.len()].copy_from_slice(new);
    }
}

/// Applies a per-frame buffer upload (`bytes` planned by `plan`) to its copy.
pub(crate) fn apply_buffer(copy: &mut Vec<u8>, plan: BufferPlan, bytes: &[u8]) {
    if let Some(size) = plan.recreate {
        *copy = vec![0; size as usize];
    }
    if plan.write {
        copy[..bytes.len()].copy_from_slice(bytes);
    }
}

// --- GPU helpers of the captures ----------------------------------------------------------------

/// Hardware adapter with float blending (improved transparency), or `None` (reported) on a
/// machine without one.
pub(crate) fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    crate::render::upload::tests::gpu_serial();
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .ok();
    // A software adapter (WARP, llvmpipe) is not the real GPU these proofs are about.
    let hardware = adapter.filter(|a| a.get_info().device_type != wgpu::DeviceType::Cpu);
    let adapter = rc_testdata::need(hardware, "hardware GPU adapter")?;
    // Blended float targets: improved transparency stages.
    let blend = adapter.features().contains(wgpu::Features::FLOAT32_BLENDABLE);
    rc_testdata::need(blend.then_some(()), "float target blending (FLOAT32_BLENDABLE)")?;
    let desc = wgpu::DeviceDescriptor { required_features: wgpu::Features::FLOAT32_BLENDABLE, ..Default::default() };
    Some(pollster::block_on(adapter.request_device(&desc)).expect("device"))
}

pub(crate) fn texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: u32,
    layers: u32,
    f: impl Fn(u32, u32, u32) -> [u8; 4],
) -> wgpu::Texture {
    use wgpu::util::DeviceExt;
    let f = &f;
    let data: Vec<u8> =
        (0..layers).flat_map(|l| (0..size * size).flat_map(move |i| f(l, i % size, i / size))).collect();
    device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: layers },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &data,
    )
}

/// Glyph sheet of the captures: two 16 × 16 pages, some texels empty.
pub(crate) fn glyph_sheet(device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::Texture {
    texture(device, queue, 16, 2, |l, x, y| if (x + y + l) % 3 == 0 { [0; 4] } else { [255, (x * 13) as u8, 200, 255] })
}

pub(crate) fn glyph_sheet_view(sheet: &wgpu::Texture) -> wgpu::TextureView {
    sheet.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    })
}

/// Side of the square capture target.
pub(crate) const SIZE: u32 = 256;

/// Reversed infinite perspective (depth = near / distance), camera at the origin toward −z.
pub(crate) fn test_projection() -> Mat4 {
    let (f, near) = (1.0 / 0.6f32.tan(), 0.05);
    Mat4::from_cols_array(&[f, 0.0, 0.0, 0.0, 0.0, f, 0.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, near, 0.0])
}

/// Camera bind group (uniform, lightmap) and colour and depth targets of a capture.
pub(crate) struct Target {
    pub cam_layout: wgpu::BindGroupLayout,
    pub cam_bind: wgpu::BindGroup,
    pub color: wgpu::Texture,
    pub color_view: wgpu::TextureView,
    pub depth_view: wgpu::TextureView,
    /// Keeps the lightmap alive.
    _lightmap: wgpu::Texture,
}

impl Target {
    /// Camera `view_proj` with its eye at `eye` (no fog).
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, view_proj: Mat4, eye: Vec3) -> Self {
        use wgpu::util::DeviceExt;
        let entry = |binding, visibility, ty| wgpu::BindGroupLayoutEntry { binding, visibility, ty, count: None };
        let cam_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
                    7,
                    wgpu::ShaderStages::VERTEX,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                entry(8, wgpu::ShaderStages::VERTEX, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
            ],
        });
        // Camera uniform: view-projection, eye at 28..31, fog pushed out of reach at 36..40.
        let mut cam = [0f32; 64];
        cam[..16].copy_from_slice(&view_proj.to_cols_array());
        cam[28..31].copy_from_slice(&eye.to_array());
        cam[36..40].copy_from_slice(&[1e9, 2e9, 1e9, 2e9]);
        let cam_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&cam),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let lightmap = texture(device, queue, 16, 1, |_, x, y| [(x * 16) as u8, (y * 16) as u8, 160, 255]);
        let sampler = device.create_sampler(&Default::default());
        let lm_view = lightmap.create_view(&Default::default());
        let cam_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &cam_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: cam_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::TextureView(&lm_view) },
                wgpu::BindGroupEntry { binding: 8, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });
        let target = |format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let color = target(
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let depth = target(
            wgpu::TextureFormat::Depth32Float,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let (color_view, depth_view) = (color.create_view(&Default::default()), depth.create_view(&Default::default()));
        Target { cam_layout, cam_bind, color, color_view, depth_view, _lightmap: lightmap }
    }

    /// Render pass on the targets, cleared (`clear`) or loaded, camera bound at group 0.
    pub fn pass<'e>(&self, enc: &'e mut wgpu::CommandEncoder, clear: bool) -> wgpu::RenderPass<'e> {
        let (color, depth) = if clear {
            (wgpu::LoadOp::Clear(wgpu::Color { r: 0.1, g: 0.2, b: 0.3, a: 1.0 }), wgpu::LoadOp::Clear(0.0))
        } else {
            (wgpu::LoadOp::Load, wgpu::LoadOp::Load)
        };
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.color_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: color, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth_view,
                depth_ops: Some(wgpu::Operations { load: depth, store: wgpu::StoreOp::Store }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        pass.set_bind_group(0, &self.cam_bind, &[]);
        pass
    }

    /// Submits `enc` with a copy of the colour target and returns its pixels (RGBA).
    pub fn read(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        up: &mut Uploader,
        mut enc: wgpu::CommandEncoder,
    ) -> Vec<u8> {
        let out = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(SIZE * SIZE * 4),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        enc.copy_texture_to_buffer(
            self.color.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &out,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(SIZE * 4), rows_per_image: None },
            },
            wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
        );
        up.submit(queue, enc.finish());
        out.slice(..).map_async(wgpu::MapMode::Read, |r| r.expect("read"));
        device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("GPU wait");
        out.slice(..).get_mapped_range().expect("mapping").to_vec()
    }
}

/// Pixels that differ from the first one (a corner the captures leave empty).
pub(crate) fn lit_pixels(image: &[u8]) -> usize {
    image.as_chunks::<4>().0.iter().filter(|p| p[..] != image[..4]).count()
}
