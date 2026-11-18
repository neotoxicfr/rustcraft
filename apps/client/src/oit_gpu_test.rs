//! Proof on a real GPU of the forms of improved transparency: a scene of stained glass panes
//! (holes discarded at the bounds stage, one pane nearly opaque, one tilted across the others), a
//! water plane seen at a grazing angle (a depth range far beyond the 10 precise blocks), an additive
//! beam, an opaque wall and clouds in their own depth goes through the whole chain (bounds, culling,
//! coefficients, accumulation, composition) with the shaders and writes of each form. Color and
//! depth of every frame must equal those of the form before (RGBA32F bounds, wavelet loops,
//! projection written by the queue on every frame) byte for byte; the frames change the field of
//! view, keep it, then change it back. Sabotages, each seen: a wavelet level left out, the far
//! bound read with the wrong sign, a projection left stale.
//!
//! Without a GPU the test skips itself; under `RUSTCRAFT_OPT_REQUIRE=1` it is a failure.

use super::{Oit, Options, Set, Spec, Stage};
use crate::render::upload::Uploader;
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const BACKGROUND: wgpu::Color = wgpu::Color { r: 0.45, g: 0.6, b: 0.85, a: 1.0 };

/// Test family: quads read from storage, an alpha with holes, the three stage entries and an
/// opaque entry for the wall.
const FAMILY: &str = "
struct TestQuad {
    corners: array<vec4<f32>, 4>,
    color: vec4<f32>,
    // x: additive surface, y: holes cut in a grid of pixels.
    kind: vec4<f32>,
};
@group(0) @binding(0) var<uniform> test_view_proj: mat4x4<f32>;
@group(0) @binding(1) var<storage, read> test_quads: array<TestQuad>;

struct TestOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) @interpolate(flat) kind: vec4<f32>,
};

@vertex
fn vs(@builtin(vertex_index) v: u32, @builtin(instance_index) i: u32) -> TestOut {
    // Two triangles: corners 0, 1, 2 then 0, 2, 3.
    let corner = (0x320210u >> (v * 4u)) & 3u;
    var out: TestOut;
    out.pos = test_view_proj * test_quads[i].corners[corner];
    // A shade that varies over the quad, so that neighbouring pixels differ.
    out.color = vec4<f32>(test_quads[i].color.rgb * (0.7 + 0.1 * f32(corner)), test_quads[i].color.a);
    out.kind = test_quads[i].kind;
    return out;
}

fn test_alpha(o: TestOut) -> f32 {
    let cell = vec2<u32>(o.pos.xy) / vec2<u32>(7u, 5u);
    let hole = o.kind.y > 0.5 && (cell.x + cell.y) % 6u == 0u;
    return select(o.color.a, 0.0, hole);
}

@fragment
fn fs_opaque(o: TestOut) -> @location(0) vec4<f32> {
    return vec4<f32>(o.color.rgb, 1.0);
}

@fragment
fn fs_bounds(o: TestOut) -> OitBounds {
    return oit_depth_bounds(o.pos, test_alpha(o));
}

@fragment
fn fs_trans(o: TestOut) -> OitCoeffs {
    if o.kind.x > 0.5 {
        return OitCoeffs(vec4<f32>(0.0), vec4<f32>(0.0));
    }
    return oit_add_transmittance(o.pos, 1.0 - test_alpha(o));
}

@fragment
fn fs_accum(o: TestOut) -> @location(0) vec4<f32> {
    return oit_accumulate(o.pos, vec4<f32>(o.color.rgb, test_alpha(o)), o.kind.x > 0.5);
}
";

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Quad {
    corners: [[f32; 4]; 4],
    color: [f32; 4],
    kind: [f32; 4],
}

impl Quad {
    fn new(corners: [[f32; 3]; 4], color: [f32; 4]) -> Self {
        let corners = corners.map(|[x, y, z]| [x, y, z, 1.0]);
        Quad { corners, color, kind: [0.0; 4] }
    }

    /// Upright rectangle facing the camera at depth `z`.
    fn pane(x: [f32; 2], y: [f32; 2], z: f32, color: [f32; 4]) -> Self {
        Quad::new([[x[0], y[0], z], [x[1], y[0], z], [x[1], y[1], z], [x[0], y[1], z]], color)
    }

    /// Horizontal rectangle at height `y`.
    fn floor(x: [f32; 2], y: f32, z: [f32; 2], color: [f32; 4]) -> Self {
        Quad::new([[x[0], y, z[0]], [x[1], y, z[0]], [x[1], y, z[1]], [x[0], y, z[1]]], color)
    }

    fn additive(mut self) -> Self {
        self.kind[0] = 1.0;
        self
    }

    fn holes(mut self) -> Self {
        self.kind[1] = 1.0;
        self
    }
}

/// The scene's quads, in draw order: the opaque wall, the water (terrain pass), the panes and
/// beam (elements pass), the clouds (their own pass).
struct Scene {
    quads: Vec<Quad>,
    wall: std::ops::Range<u32>,
    water: std::ops::Range<u32>,
    elements: std::ops::Range<u32>,
    clouds: std::ops::Range<u32>,
}

impl Scene {
    /// A few surfaces of every kind (the equivalence proof).
    fn proof() -> Self {
        let elements = vec![
            Quad::pane([-1.2, 0.4], [-1.0, 0.8], -2.0, [0.9, 0.3, 0.3, 0.5]).holes(),
            Quad::pane([-0.5, 1.5], [-0.5, 1.2], -3.5, [0.3, 0.9, 0.4, 0.6]),
            Quad::pane([-3.0, 0.5], [-2.0, 0.2], -6.0, [0.3, 0.4, 1.0, 0.45]).holes(),
            // Nearly opaque: culls what lies behind it.
            Quad::pane([0.0, 5.0], [-3.0, 3.0], -9.0, [0.8, 0.8, 0.2, 1.0]),
            // Tilted across the panes: depths change along it.
            Quad::new(
                [[-4.0, -1.0, -1.5], [-2.0, -1.0, -12.0], [-2.0, 2.0, -12.0], [-4.0, 2.0, -1.5]],
                [0.6, 0.2, 0.8, 0.35],
            ),
            Quad::pane([-20.0, 20.0], [-5.0, 15.0], -30.0, [0.5, 0.5, 0.5, 0.3]),
            Quad::pane([1.0, 1.4], [-2.0, 4.0], -4.5, [1.0, 0.6, 0.2, 0.8]).additive(),
        ];
        let clouds = vec![
            Quad::floor([-200.0, 200.0], 12.0, [-20.0, -600.0], [1.0, 1.0, 1.0, 0.8]),
            Quad::pane([-60.0, 20.0], [8.0, 12.0], -150.0, [0.95, 0.95, 1.0, 0.8]),
        ];
        let wall = Quad::pane([-50.0, -5.0], [-10.0, 10.0], -20.0, [0.4, 0.3, 0.2, 1.0]);
        let water = Quad::floor([-60.0, 60.0], -1.5, [-0.3, -400.0], [0.15, 0.3, 0.75, 0.7]);
        Scene::assemble(wall, water, elements, clouds)
    }

    /// Many full-screen layers (the micro-bench): `layers` panes from 1 to 60 blocks.
    fn heavy(layers: u32) -> Self {
        let elements = (0..layers)
            .map(|i| {
                let z = -1.0 - 59.0 * i as f32 / layers as f32;
                let tint = i as f32 / layers as f32;
                let pane = Quad::pane([-100.0, 100.0], [-100.0, 100.0], z, [tint, 0.5, 1.0 - tint, 0.3]);
                if i % 3 == 0 { pane.holes() } else { pane }
            })
            .collect();
        let clouds = vec![Quad::floor([-200.0, 200.0], 12.0, [-20.0, -600.0], [1.0, 1.0, 1.0, 0.8])];
        let wall = Quad::pane([-50.0, -5.0], [-10.0, 10.0], -80.0, [0.4, 0.3, 0.2, 1.0]);
        let water = Quad::floor([-60.0, 60.0], -1.5, [-0.3, -400.0], [0.15, 0.3, 0.75, 0.7]);
        Scene::assemble(wall, water, elements, clouds)
    }

    fn assemble(wall: Quad, water: Quad, elements: Vec<Quad>, clouds: Vec<Quad>) -> Self {
        let mut quads = vec![wall, water];
        let elements_start = quads.len() as u32;
        quads.extend(elements);
        let clouds_start = quads.len() as u32;
        quads.extend(clouds);
        Scene {
            wall: 0..1,
            water: 1..2,
            elements: elements_start..clouds_start,
            clouds: clouds_start..quads.len() as u32,
            quads,
        }
    }
}

/// View of a frame: field of view (degrees), far plane (the depth coefficients follow it, not the
/// field of view), eye and direction.
struct View {
    fov: f32,
    far: f32,
    eye: Vec3,
    dir: Vec3,
}

/// Projection changed, kept (the eye moves: same projection), then changed back.
const VIEWS: [View; 4] = [
    View { fov: 70.0, far: 2000.0, eye: Vec3::ZERO, dir: Vec3::new(0.0, 0.05, -1.0) },
    View { fov: 55.0, far: 640.0, eye: Vec3::ZERO, dir: Vec3::new(0.15, -0.05, -1.0) },
    View { fov: 55.0, far: 640.0, eye: Vec3::new(0.2, 0.1, 0.3), dir: Vec3::new(0.1, 0.0, -1.0) },
    View { fov: 70.0, far: 2000.0, eye: Vec3::new(-0.3, 0.0, 0.0), dir: Vec3::new(-0.1, 0.08, -1.0) },
];

/// How a run draws: the form of improved transparency, the uploader's transit ring, an edit of
/// the family shader (sabotage), the projection written for the first frame only (sabotage).
#[derive(Clone, Copy)]
struct Form {
    options: Options,
    ring: bool,
    family_edit: Option<(&'static str, &'static str)>,
    stale_projection: bool,
}

impl Form {
    const OLD: Form = Form { options: Options::OLD, ring: false, family_edit: None, stale_projection: false };

    fn new(options: Options) -> Self {
        Form { options, ring: options.projection_on_change, ..Form::OLD }
    }
}

/// Everything a run draws with.
struct Run<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    size: (u32, u32),
    scene: &'a Scene,
    form: Form,
    oit: Oit,
    up: Uploader,
    surfaces: Set,
    clouds: Set,
    wall: wgpu::RenderPipeline,
    camera: wgpu::Buffer,
    scene_bind: wgpu::BindGroup,
    color: wgpu::Texture,
    depth: wgpu::Texture,
    color_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
}

impl<'a> Run<'a> {
    fn new(device: &'a wgpu::Device, queue: &'a wgpu::Queue, size: (u32, u32), scene: &'a Scene, form: Form) -> Self {
        let mut oit = Oit::with_options(device, FORMAT, form.options);
        let target = |format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC | usage,
                view_formats: &[],
            })
        };
        let color = target(FORMAT, wgpu::TextureUsages::empty());
        let depth = target(wgpu::TextureFormat::Depth32Float, wgpu::TextureUsages::TEXTURE_BINDING);
        let color_view = color.create_view(&Default::default());
        let depth_view = depth.create_view(&Default::default());
        oit.ensure(device, size, &depth_view);
        let scene_layout = scene_layout(device);
        let module = family_module(device, form);
        let groups = [Some(&scene_layout)];
        let spec = |label| Spec::new(label, &groups, &module, "vs", &[], "fs");
        let surfaces = oit.layouts.set(device, &spec("test surfaces"));
        let clouds = oit.layouts.set(device, &spec("test clouds").bounds_write());
        let wall = wall_pipeline(device, &scene_layout, &module);
        let camera = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let quads = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&scene.quads),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let scene_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &scene_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: camera.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: quads.as_entire_binding() },
            ],
        });
        let up = Uploader::new(device, form.ring);
        Run {
            device,
            queue,
            size,
            scene,
            form,
            oit,
            up,
            surfaces,
            clouds,
            wall,
            camera,
            scene_bind,
            color,
            depth,
            color_view,
            depth_view,
        }
    }

    /// Records frame `n` of `view`: uploads, wall, the three stages with the culling, composition.
    fn frame(&mut self, n: usize, view: &View) -> wgpu::CommandEncoder {
        let proj = crate::render::perspective(view.fov, self.size.0 as f32 / self.size.1 as f32, 0.05, view.far);
        let look = glam::camera::rh::view::look_to_mat4(view.eye, view.dir.normalize(), Vec3::Y);
        let view_proj: Mat4 = proj * look;
        self.queue.write_buffer(&self.camera, 0, bytemuck::cast_slice(&view_proj.to_cols_array()));
        if n == 0 || !self.form.stale_projection {
            self.oit.set_projection(&mut self.up, self.queue, proj);
        }
        let mut enc = self.device.create_command_encoder(&Default::default());
        self.draw_wall(&mut enc);
        let s = self.scene;
        for stage in Stage::ALL {
            let f = self.oit.frame(stage);
            for (first, range) in [(true, &s.water), (false, &s.elements)] {
                let mut pass = self.oit.begin(&mut enc, stage, &self.depth_view, first, "test surfaces", None);
                f.set(&mut pass, &self.surfaces);
                pass.set_bind_group(0, &self.scene_bind, &[]);
                pass.draw(0..6, range.clone());
            }
            if stage == Stage::Bounds {
                self.oit.cull(&mut enc, &self.depth_view, None);
            }
            let mut pass = self.oit.begin_clouds(&mut enc, stage, None);
            f.set(&mut pass, &self.clouds);
            pass.set_bind_group(0, &self.scene_bind, &[]);
            pass.draw(0..6, s.clouds.clone());
        }
        self.oit.composite(&mut enc, &self.color_view, &self.depth_view, None);
        enc
    }

    /// Solids pass: background, cleared depth, the opaque wall.
    fn draw_wall(&self, enc: &mut wgpu::CommandEncoder) {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("test solids"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.color_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(BACKGROUND), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(crate::render::DEPTH_CLEAR),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        pass.set_pipeline(&self.wall);
        pass.set_bind_group(0, &self.scene_bind, &[]);
        pass.draw(0..6, self.scene.wall.clone());
    }

    /// Color then depth bytes of each view.
    fn images(mut self) -> Vec<Vec<u8>> {
        let (w, h) = self.size;
        let plane = u64::from(w * h * 4);
        let out = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: plane * 2,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut images = Vec::new();
        for (n, view) in VIEWS.iter().enumerate() {
            let mut enc = self.frame(n, view);
            for (i, (texture, aspect)) in
                [(&self.color, wgpu::TextureAspect::All), (&self.depth, wgpu::TextureAspect::DepthOnly)]
                    .into_iter()
                    .enumerate()
            {
                enc.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect },
                    wgpu::TexelCopyBufferInfo {
                        buffer: &out,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: plane * i as u64,
                            bytes_per_row: Some(w * 4),
                            rows_per_image: Some(h),
                        },
                    },
                    wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                );
            }
            self.up.submit(self.queue, enc.finish());
            out.slice(..).map_async(wgpu::MapMode::Read, |r| r.expect("read"));
            self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("GPU wait");
            images.push(out.slice(..).get_mapped_range().expect("mapping").to_vec());
            out.unmap();
        }
        images
    }

    /// Duration (ms) of `frames` frames drawn back to back, from the submission to the end of the GPU.
    fn time_frames(&mut self, frames: usize) -> f64 {
        let start = std::time::Instant::now();
        for n in 0..frames {
            let enc = self.frame(n, &VIEWS[n % 2]);
            self.up.submit(self.queue, enc.finish());
        }
        self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("GPU wait");
        start.elapsed().as_secs_f64() * 1e3
    }
}

/// Group 0 of the test family: camera and quads, read by the vertices.
fn scene_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX,
        ty: wgpu::BindingType::Buffer { ty, has_dynamic_offset: false, min_binding_size: None },
        count: None,
    };
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("test scene"),
        entries: &[
            entry(0, wgpu::BufferBindingType::Uniform),
            entry(1, wgpu::BufferBindingType::Storage { read_only: true }),
        ],
    })
}

/// Test family shader with the common functions of the form, edited for a sabotage.
fn family_module(device: &wgpu::Device, form: Form) -> wgpu::ShaderModule {
    let mut common = form.options.common_source();
    if let Some((from, to)) = form.family_edit {
        assert!(common.contains(from), "sabotage: `{from}` not found in the shader");
        common = common.replace(from, to);
    }
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("test family (transparency)"),
        source: wgpu::ShaderSource::Wgsl(format!("{common}\n{FAMILY}").into()),
    })
}

fn wall_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    module: &wgpu::ShaderModule,
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("test wall"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some("fs_opaque"),
            compilation_options: Default::default(),
            targets: &[Some(FORMAT.into())],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(true),
            depth_compare: Some(crate::render::DEPTH_NEARER),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}

fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let (device, queue) = crate::render::upload::tests::gpu()?;
    let blend = device.features().contains(wgpu::Features::FLOAT32_BLENDABLE);
    rc_testdata::need(blend.then_some(()), "blending of float targets (FLOAT32_BLENDABLE)")?;
    Some((device, queue))
}

/// Bytes that differ between two runs, over all the views.
fn differing_bytes(a: &[Vec<u8>], b: &[Vec<u8>]) -> usize {
    a.iter().zip(b).map(|(x, y)| x.iter().zip(y).filter(|(p, q)| p != q).count()).sum()
}

/// Where two images (color then depth planes of `plane` bytes) differ: pixels, largest color step,
/// largest depth step in units in the last place.
fn describe_difference(got: &[u8], want: &[u8], plane: usize) -> String {
    let (got_color, got_depth) = got.split_at(plane);
    let (want_color, want_depth) = want.split_at(plane);
    let color_pixels = got_color.as_chunks::<4>().0.iter().zip(want_color.as_chunks::<4>().0);
    let color_steps: Vec<u8> =
        color_pixels.map(|(g, w)| (0..4).map(|c| g[c].abs_diff(w[c])).max().unwrap_or(0)).collect();
    let depth_pixels = got_depth.as_chunks::<4>().0.iter().zip(want_depth.as_chunks::<4>().0);
    let depth_steps: Vec<u32> =
        depth_pixels.map(|(g, w)| u32::from_le_bytes(*g).abs_diff(u32::from_le_bytes(*w))).collect();
    format!(
        "{} color pixels differ (largest step {}), {} depths differ (largest {} ulp)",
        color_steps.iter().filter(|s| **s > 0).count(),
        color_steps.iter().max().unwrap_or(&0),
        depth_steps.iter().filter(|s| **s > 0).count(),
        depth_steps.iter().max().unwrap_or(&0),
    )
}

const PROOF_SIZE: (u32, u32) = (320, 240);

/// Rounding allowed to the compact bounds: their distances are computed by the shader that reads
/// them rather than by the family shader of each fragment, and the GPU's division does not round
/// alike in every shader. Depths stay exact (nearest depth stored as is); a color channel may move
/// by one step on at most 1% of the pixels (measured: 0.4 to 0.7% of the proof scene's pixels,
/// one step at most).
fn within_compact_rounding(got: &[u8], want: &[u8], plane: usize) -> bool {
    let (got_color, got_depth) = got.split_at(plane);
    let (want_color, want_depth) = want.split_at(plane);
    let pixels = got_color.as_chunks::<4>().0.iter().zip(want_color.as_chunks::<4>().0);
    let steps: Vec<u8> = pixels.map(|(g, w)| (0..4).map(|c| g[c].abs_diff(w[c])).max().unwrap_or(0)).collect();
    let moved = steps.iter().filter(|s| **s > 0).count();
    got_depth == want_depth && steps.iter().all(|s| *s <= 1) && moved * 100 <= steps.len()
}

/// Every form of improved transparency, each lead alone then all of them, draws the images of
/// the form before: to the byte without the compact bounds, within their rounding with them; each
/// sabotage changes them.
#[test]
fn transparency_forms_render_identically_on_gpu() {
    let Some((device, queue)) = gpu() else { return };
    let scene = Scene::proof();
    let render = |form| Run::new(&device, &queue, PROOF_SIZE, &scene, form).images();
    let want = render(Form::OLD);
    let (w, h) = PROOF_SIZE;
    let plane = (w * h * 4) as usize;
    for (n, image) in want.iter().enumerate() {
        let (color, depth) = image.split_at(plane);
        let painted = color.as_chunks::<4>().0.iter().filter(|p| p[..3] != color[..3]).count();
        let translucent_depths = depth.as_chunks::<4>().0.iter().filter(|d| **d != [0; 4]).count();
        assert!(painted > (w * h / 3) as usize, "view {n}: {painted} pixels drawn over the background");
        assert!(translucent_depths > (w * h / 3) as usize, "view {n}: {translucent_depths} depths written");
    }
    let exact = Options { compact_bounds: false, ..Options::NEW };
    let forms = [
        Options { unrolled_wavelet: true, ..Options::OLD },
        Options { projection_on_change: true, ..Options::OLD },
        exact,
        Options { compact_bounds: true, ..Options::OLD },
        Options::NEW,
    ];
    for options in forms {
        for (n, (got, want)) in render(Form::new(options)).iter().zip(&want).enumerate() {
            let difference = describe_difference(got, want, plane);
            eprintln!("{options:?}, view {n}: {difference}");
            if options.compact_bounds {
                assert!(within_compact_rounding(got, want, plane), "{options:?}, view {n}: {difference}");
            } else {
                assert!(got == want, "{options:?}, view {n}: {difference}");
            }
        }
    }
    // Against the exact forms: any byte changed shows the sabotage.
    let sabotages = [
        (
            "middle wavelet level left out",
            Form {
                family_edit: Some((
                    "vec4<f32>(mean, value.z, middle_left, middle_right)",
                    "vec4<f32>(mean, value.z, 0.0, 0.0)",
                )),
                ..Form::new(exact)
            },
        ),
        ("projection of the first frame kept", Form { stale_projection: true, ..Form::new(exact) }),
    ];
    for (name, form) in sabotages {
        let diff = differing_bytes(&render(form), &want);
        eprintln!("sabotage '{name}': {diff} bytes differ");
        assert!(diff > 0, "sabotage '{name}' not seen");
    }
    // Against the compact rounding: the sabotage goes beyond it.
    let wrong_far = Form { family_edit: Some(("oit_linear(-b.g)", "oit_linear(b.g)")), ..Form::new(Options::NEW) };
    let got = render(wrong_far);
    eprintln!("sabotage 'far bound read with the wrong sign': {} bytes differ", differing_bytes(&got, &want));
    let caught = got.iter().zip(&want).any(|(g, w)| !within_compact_rounding(g, w, plane));
    assert!(caught, "sabotage 'far bound read with the wrong sign' within the rounding");
}

/// What the projection sends per frame over the four views (changed, changed, kept, changed): the
/// form before writes it on every frame through the queue (one staging buffer made and dropped each
/// time); the new one only on the frames where it changed, through the transit ring.
#[test]
fn projection_crosses_only_when_it_changes() {
    let Some((device, queue)) = gpu() else { return };
    let scene = Scene::proof();
    let per_frame = |form: Form| -> Vec<(u64, u64)> {
        let mut run = Run::new(&device, &queue, PROOF_SIZE, &scene, form);
        run.up.take_stats();
        let mut counts = Vec::new();
        for (n, view) in VIEWS.iter().enumerate() {
            let enc = run.frame(n, view);
            run.up.submit(&queue, enc.finish());
            let stats = run.up.take_stats();
            counts.push((stats.writes, stats.copies));
        }
        counts
    };
    let before = per_frame(Form::OLD);
    let after = per_frame(Form::new(Options::NEW));
    eprintln!("(writes, ring copies) per frame: before {before:?}, after {after:?}");
    assert_eq!(before, [(1, 0); 4], "form before: one queue write per frame");
    assert_eq!(after, [(1, 1), (1, 1), (0, 0), (1, 1)], "new form: one ring write per change");
}

/// GPU micro-bench of the forms: 24 full-screen layers, water and clouds at 1600 × 900, 30 frames
/// per round, best of 7 alternating rounds (time from the submission to the end of the GPU), and the
/// bytes of the bounds targets:
/// `cargo test --release -p rc-app --bin rustcraft transparency_forms_cost -- --ignored --nocapture`.
#[test]
#[ignore]
fn transparency_forms_cost() {
    let Some((device, queue)) = gpu() else { return };
    let scene = Scene::heavy(24);
    let size = (1600, 900);
    let forms = [
        ("before", Options::OLD),
        ("compact bounds", Options { compact_bounds: true, ..Options::OLD }),
        ("unrolled wavelet", Options { unrolled_wavelet: true, ..Options::OLD }),
        ("all", Options::NEW),
    ];
    let mut runs: Vec<Run<'_>> =
        forms.iter().map(|(_, o)| Run::new(&device, &queue, size, &scene, Form::new(*o))).collect();
    for run in &mut runs {
        run.time_frames(5);
    }
    let mut best = [f64::MAX; 4];
    for _ in 0..7 {
        for (i, run) in runs.iter_mut().enumerate() {
            best[i] = best[i].min(run.time_frames(30) / 30.0);
        }
    }
    let pixels = f64::from(size.0 * size.1);
    for ((name, options), ms) in forms.iter().zip(best) {
        let bounds_bytes = if options.compact_bounds { 8 + 4 + 8 } else { 16 + 16 };
        eprintln!(
            "{name}: {ms:.3} ms per frame ({:+.1}%), bounds targets {:.1} MB (loaded machine)",
            (ms / best[0] - 1.0) * 100.0,
            pixels * f64::from(bounds_bytes) / 1e6
        );
    }
}
