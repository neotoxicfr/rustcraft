//! Proof on a real GPU of the hand and the interface drawn at the end of the world pass
//! (`RUSTCRAFT_OPT_DEPTH_RANGE_PASS_MERGE`): the world in the depth share [0, `WORLD_DEPTH_SHARE`],
//! then the hand and the interface in the share in front of it, against the old frame (world pass,
//! then a pass of their own on a cleared depth). The draws stand in for those of the frame with
//! their depth states: opaque world, a decal with the cracks' depth offset, an enchantment glint
//! drawn with the "equal" test over its surface, a wall at 0.06 block of the eye; the hand with a
//! glint and an offset text, the totem (projection without the view), a 3D crosshair, interface
//! models (orthographic, depth in [0.5, 1]) between 2D quads that neither test nor write the depth.
//! Two scenes: the hand at arm's length, and the hand at 0.06 block (eating, spyglass), where it
//! hides an interface model and another one stays in front of it. The images must be equal to the
//! byte. Sabotage: a hand share that overlaps the world's lets the close wall cover the hand.

use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

use super::super::{DEPTH_CLEAR, DEPTH_NEARER, depth_bias, perspective};
use super::{PassViewport, WORLD_DEPTH_SHARE};
use crate::render::upload::tests::{gpu, gpu_with_timestamps, read};

const SIZE: (u32, u32) = (192, 128);
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

const SHADER: &str = r"
struct Draw {
    mvp: mat4x4<f32>,
    color: vec4<f32>,
};
@group(0) @binding(0) var<uniform> d: Draw;

@vertex
fn vs(@location(0) p: vec3<f32>) -> @builtin(position) vec4<f32> {
    return d.mvp * vec4<f32>(p, 1.0);
}

@fragment
fn fs() -> @location(0) vec4<f32> {
    return d.color;
}
";

/// Depth use of a stand-in draw.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    /// Tested and written (terrain, entities, hand, interface models).
    Solid,
    /// Tested, not written, offset toward the camera like the cracks and the offset texts.
    Decal,
    /// Drawn again over its own surface with the "equal" test, added (enchantment glint).
    Glint,
    /// Neither tested nor written (2D interface, screen overlays).
    Flat,
}

const KINDS: [Kind; 4] = [Kind::Solid, Kind::Decal, Kind::Glint, Kind::Flat];

/// A draw on the GPU: its depth use, vertices and uniform binding.
type PreparedDraw = (Kind, wgpu::Buffer, wgpu::BindGroup);

/// A quad drawn through `mvp`.
struct Draw {
    kind: Kind,
    mvp: Mat4,
    corners: [Vec3; 4],
    color: [f32; 4],
}

/// Upright quad `distance` ahead (toward −z), `x` and `y` ranges.
fn upright(x: (f32, f32), y: (f32, f32), distance: f32) -> [Vec3; 4] {
    let z = -distance;
    [Vec3::new(x.0, y.0, z), Vec3::new(x.1, y.0, z), Vec3::new(x.1, y.1, z), Vec3::new(x.0, y.1, z)]
}

/// The world, then the hand and the interface, of one scene; `hand_distance`: the hand's distance
/// to the eye.
fn scene(hand_distance: f32) -> (Vec<Draw>, Vec<Draw>) {
    let aspect = SIZE.0 as f32 / SIZE.1 as f32;
    // Camera at the origin toward −z: world, hand and totem through the same projection (the
    // hand's field equal to the world's here).
    let world = perspective(70.0, aspect, 0.05, 512.0);
    let (hand, totem) = (world, world);
    // Interface models: same matrix as the frame's (depth 0.001 z + 0.75, in [0.5, 1]).
    let (w, h) = (SIZE.0 as f32 / 2.0, SIZE.1 as f32 / 2.0);
    let gui = Mat4::from_cols(
        glam::Vec4::new(2.0 / w, 0.0, 0.0, 0.0),
        glam::Vec4::new(0.0, 2.0 / h, 0.0, 0.0),
        glam::Vec4::new(0.0, 0.0, 0.001, 0.0),
        glam::Vec4::new(-1.0, 1.0, 0.75, 1.0),
    );
    let d = |kind, mvp, corners, color| Draw { kind, mvp, corners, color };
    let wall = upright((-30.0, 30.0), (-10.0, 20.0), 25.0);
    let world_draws = vec![
        d(
            Kind::Solid,
            world,
            [
                Vec3::new(-60.0, -2.0, 5.0),
                Vec3::new(60.0, -2.0, 5.0),
                Vec3::new(60.0, -2.0, -80.0),
                Vec3::new(-60.0, -2.0, -80.0),
            ],
            [0.3, 0.5, 0.2, 1.0],
        ),
        d(Kind::Solid, world, wall, [0.5, 0.45, 0.4, 1.0]),
        // Crack on the wall, offset toward the camera.
        d(Kind::Decal, world, upright((8.0, 14.0), (2.0, 8.0), 25.0), [0.1, 0.1, 0.1, 0.6]),
        // Glint over the wall.
        d(Kind::Glint, world, wall, [0.3, 0.0, 0.4, 0.5]),
        // A wall at 0.06 block of the eye, left of the centre, under the totem and the item.
        d(Kind::Solid, world, upright((-0.03, 0.01), (-0.02, 0.025), 0.06), [0.8, 0.2, 0.2, 1.0]),
        d(Kind::Solid, world, upright((-9.0, -3.0), (-1.0, 3.0), 8.0), [0.2, 0.2, 0.7, 1.0]),
    ];
    let item = upright((0.02, 0.4), (-0.35, 0.05), hand_distance);
    let hand_draws = vec![
        // Arm, item and its glint, an offset text on the item.
        d(Kind::Solid, hand, upright((0.1, 0.5), (-0.6, -0.2), hand_distance * 1.4), [0.9, 0.75, 0.6, 1.0]),
        d(Kind::Solid, hand, item, [0.6, 0.6, 0.65, 1.0]),
        d(Kind::Glint, hand, item, [0.25, 0.0, 0.35, 0.5]),
        d(Kind::Decal, hand, upright((0.1, 0.3), (-0.2, -0.05), hand_distance), [0.0, 0.0, 0.0, 0.8]),
        // Screen overlay over the hand (vignette, fire).
        d(Kind::Flat, Mat4::IDENTITY, upright((-1.0, -0.6), (-1.0, 1.0), 0.0), [1.0, 0.5, 0.0, 0.3]),
        // Totem, in the view frame.
        d(Kind::Solid, totem, upright((-0.15, 0.15), (-0.1, 0.3), 1.2), [0.9, 0.8, 0.1, 1.0]),
        // 3D crosshair: three thin bars at the centre.
        d(Kind::Solid, hand, upright((-0.002, 0.02), (-0.001, 0.001), 0.1), [1.0, 0.0, 0.0, 1.0]),
        d(Kind::Solid, hand, upright((-0.001, 0.001), (-0.002, 0.02), 0.1), [0.0, 1.0, 0.0, 1.0]),
        // Interface: a 2D panel, a model behind it, a model under the hand, a model in front of
        // everything, then 2D text over them.
        d(
            Kind::Flat,
            gui,
            [
                Vec3::new(10.0, -10.0, 0.0),
                Vec3::new(80.0, -10.0, 0.0),
                Vec3::new(80.0, -60.0, 0.0),
                Vec3::new(10.0, -60.0, 0.0),
            ],
            [0.2, 0.2, 0.2, 0.8],
        ),
        d(
            Kind::Solid,
            gui,
            [
                Vec3::new(20.0, -20.0, -100.0),
                Vec3::new(50.0, -20.0, -100.0),
                Vec3::new(50.0, -50.0, -100.0),
                Vec3::new(20.0, -50.0, -100.0),
            ],
            [0.1, 0.7, 0.7, 1.0],
        ),
        d(
            Kind::Solid,
            gui,
            [
                Vec3::new(50.0, -30.0, 0.0),
                Vec3::new(90.0, -30.0, 0.0),
                Vec3::new(90.0, -60.0, 0.0),
                Vec3::new(50.0, -60.0, 0.0),
            ],
            [0.7, 0.1, 0.7, 1.0],
        ),
        d(
            Kind::Solid,
            gui,
            [
                Vec3::new(70.0, -40.0, 150.0),
                Vec3::new(95.0, -40.0, 150.0),
                Vec3::new(95.0, -62.0, 150.0),
                Vec3::new(70.0, -62.0, 150.0),
            ],
            [0.9, 0.9, 0.1, 1.0],
        ),
        d(
            Kind::Flat,
            gui,
            [
                Vec3::new(15.0, -12.0, 0.0),
                Vec3::new(60.0, -12.0, 0.0),
                Vec3::new(60.0, -18.0, 0.0),
                Vec3::new(15.0, -18.0, 0.0),
            ],
            [1.0, 1.0, 1.0, 1.0],
        ),
    ];
    (world_draws, hand_draws)
}

/// Stand-in pipeline of `kind`.
fn pipeline(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, kind: Kind) -> wgpu::RenderPipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(layout)],
        ..Default::default()
    });
    let add = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::SrcAlpha,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    };
    let (compare, write, bias, blend) = match kind {
        Kind::Solid => (DEPTH_NEARER, true, Default::default(), None),
        Kind::Decal => (DEPTH_NEARER, false, depth_bias(10, 1.0), Some(wgpu::BlendState::ALPHA_BLENDING)),
        Kind::Glint => {
            (wgpu::CompareFunction::Equal, false, Default::default(), Some(wgpu::BlendState { color: add, alpha: add }))
        }
        Kind::Flat => {
            (wgpu::CompareFunction::Always, false, Default::default(), Some(wgpu::BlendState::ALPHA_BLENDING))
        }
    };
    const ATTRIBUTES: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![0 => Float32x3];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None,
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: 12,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &ATTRIBUTES,
            })],
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState { format: FORMAT, blend, write_mask: wgpu::ColorWrites::ALL })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(write),
            depth_compare: Some(compare),
            stencil: Default::default(),
            bias,
        }),
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}

/// How the frame is drawn: two passes (depth cleared between them), or one pass split into depth
/// shares, the world's ending at `world_share_end` and the hand's starting at `hand_share_start`.
#[derive(Clone, Copy)]
enum Frame {
    TwoPasses,
    OnePass { world_share_end: f32, hand_share_start: f32 },
}

struct Rig {
    device: wgpu::Device,
    queue: wgpu::Queue,
    layout: wgpu::BindGroupLayout,
    pipelines: Vec<(Kind, wgpu::RenderPipeline)>,
    color: wgpu::Texture,
    color_view: wgpu::TextureView,
    depth: wgpu::TextureView,
}

impl Rig {
    fn new(device: wgpu::Device, queue: wgpu::Queue) -> Rig {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let pipelines = KINDS.iter().map(|&k| (k, pipeline(&device, &layout, k))).collect();
        let texture = |format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width: SIZE.0, height: SIZE.1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | usage,
                view_formats: &[],
            })
        };
        let color = texture(FORMAT, wgpu::TextureUsages::COPY_SRC);
        let color_view = color.create_view(&Default::default());
        let depth =
            texture(wgpu::TextureFormat::Depth32Float, wgpu::TextureUsages::empty()).create_view(&Default::default());
        Rig { device, queue, layout, pipelines, color, color_view, depth }
    }

    /// Vertex buffer and binding of each draw.
    fn prepared(&self, draws: &[Draw]) -> Vec<PreparedDraw> {
        draws
            .iter()
            .map(|d| {
                let c = d.corners;
                let vertices: Vec<[f32; 3]> =
                    [c[0], c[1], c[2], c[2], c[3], c[0]].iter().map(|v| v.to_array()).collect();
                let vertices = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: bytemuck::cast_slice(&vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                });
                let mut uniform = [0f32; 20];
                uniform[..16].copy_from_slice(&d.mvp.to_cols_array());
                uniform[16..].copy_from_slice(&d.color);
                let buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: bytemuck::cast_slice(&uniform),
                    usage: wgpu::BufferUsages::UNIFORM,
                });
                let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &self.layout,
                    entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }],
                });
                (d.kind, vertices, bind)
            })
            .collect()
    }

    fn encode(&self, pass: &mut wgpu::RenderPass<'_>, draws: &[PreparedDraw]) {
        for (kind, vertices, bind) in draws {
            let pipeline = &self.pipelines.iter().find(|(k, _)| k == kind).expect("pipeline").1;
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, bind, &[]);
            pass.set_vertex_buffer(0, vertices.slice(..));
            pass.draw(0..6, 0..1);
        }
    }

    fn pass<'e>(
        &self,
        enc: &'e mut wgpu::CommandEncoder,
        first: bool,
        timestamp_writes: Option<wgpu::RenderPassTimestampWrites<'_>>,
    ) -> wgpu::RenderPass<'e> {
        let color = if first {
            wgpu::LoadOp::Clear(wgpu::Color { r: 0.5, g: 0.7, b: 1.0, a: 0.0 })
        } else {
            wgpu::LoadOp::Load
        };
        enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.color_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: color, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(DEPTH_CLEAR),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes,
            ..Default::default()
        })
    }

    /// The frame's passes of `world` and `hand` as `frame` says; `stamps`: timestamps of the
    /// frame's start and end at these two query indices.
    fn encode_frame(
        &self,
        enc: &mut wgpu::CommandEncoder,
        (world, hand): (&[PreparedDraw], &[PreparedDraw]),
        frame: Frame,
        stamps: Option<(&wgpu::QuerySet, u32)>,
    ) {
        let stamp = |begin: bool, end: bool| {
            stamps.map(|(query_set, at)| wgpu::RenderPassTimestampWrites {
                query_set,
                beginning_of_pass_write_index: begin.then_some(at),
                end_of_pass_write_index: end.then_some(at + 1),
            })
        };
        let full = PassViewport::full(SIZE);
        match frame {
            Frame::TwoPasses => {
                self.encode(&mut self.pass(enc, true, stamp(true, false)), world);
                self.encode(&mut self.pass(enc, false, stamp(false, true)), hand);
            }
            Frame::OnePass { world_share_end, hand_share_start } => {
                let mut pass = self.pass(enc, true, stamp(true, true));
                full.with_depth(0.0, world_share_end).set(&mut pass);
                self.encode(&mut pass, world);
                full.with_depth(hand_share_start, 1.0).set(&mut pass);
                self.encode(&mut pass, hand);
            }
        }
    }

    /// Image (RGBA) of the scene with the hand at `hand_distance`, drawn as `frame` says.
    fn render(&self, hand_distance: f32, frame: Frame) -> Vec<u8> {
        let (world, hand) = scene(hand_distance);
        let (world, hand) = (self.prepared(&world), self.prepared(&hand));
        let mut enc = self.device.create_command_encoder(&Default::default());
        self.encode_frame(&mut enc, (&world, &hand), frame, None);
        let out = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(SIZE.0 * SIZE.1 * 4),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        enc.copy_texture_to_buffer(
            self.color.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &out,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(SIZE.0 * 4),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d { width: SIZE.0, height: SIZE.1, depth_or_array_layers: 1 },
        );
        self.queue.submit([enc.finish()]);
        read(&self.device, &self.queue, &out)
    }
}

fn differing_pixels(a: &[u8], b: &[u8]) -> usize {
    a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0).filter(|(x, y)| x != y).count()
}

#[test]
fn hand_in_the_world_pass_renders_identically_on_gpu() {
    let Some((device, queue)) = gpu() else { return };
    let rig = Rig::new(device, queue);
    let merged = Frame::OnePass { world_share_end: WORLD_DEPTH_SHARE, hand_share_start: WORLD_DEPTH_SHARE };
    for hand_distance in [0.5, 0.06] {
        let old = rig.render(hand_distance, Frame::TwoPasses);
        let new = rig.render(hand_distance, merged);
        let differing = differing_pixels(&old, &new);
        eprintln!("hand at {hand_distance} block: {differing} pixels differ");
        assert_eq!(differing, 0, "hand at {hand_distance} block: the merged pass changes the image");
    }
    // The close hand hides part of the magenta interface model, which stays over the hand at arm's
    // length: the scene shows the relation it is about.
    let magenta =
        |image: &[u8]| image.as_chunks::<4>().0.iter().filter(|p| p[0] > 150 && p[1] < 60 && p[2] > 150).count();
    let (far, close) = (magenta(&rig.render(0.5, Frame::TwoPasses)), magenta(&rig.render(0.06, Frame::TwoPasses)));
    assert!(far > 200 && close + 200 < far, "the close hand does not hide the model: {far} then {close} pixels");
    // Sabotage: a hand share overlapping the world's lets the close wall cover the hand.
    let overlapping = Frame::OnePass { world_share_end: WORLD_DEPTH_SHARE, hand_share_start: 0.1 };
    let covered = differing_pixels(&rig.render(0.5, Frame::TwoPasses), &rig.render(0.5, overlapping));
    assert!(covered > 50, "overlapping depth shares not seen: {covered} pixels");
}

/// Cost of a frame of the scene in two passes and in one (`cargo test --release -p rc-app
/// hand_pass_merge_cost -- --ignored --nocapture`): CPU time to encode, finish and submit it (the
/// machine's load enters this one), GPU time from the start of its first pass to the end of its
/// last (timestamps). The two forms alternate over rounds; median and best.
#[test]
#[ignore]
fn hand_pass_merge_cost() {
    const FRAMES: u32 = 500;
    const ROUNDS: usize = 4;
    let Some((device, queue)) = gpu_with_timestamps() else {
        eprintln!("no hardware GPU with timestamp queries");
        return;
    };
    let rig = Rig::new(device, queue);
    let (world, hand) = scene(0.5);
    let (world, hand) = (rig.prepared(&world), rig.prepared(&hand));
    let merged = Frame::OnePass { world_share_end: WORLD_DEPTH_SHARE, hand_share_start: WORLD_DEPTH_SHARE };
    let forms = [("two passes", Frame::TwoPasses), ("one pass, depth shares", merged)];
    let mut cpu = vec![Vec::new(); forms.len()];
    let mut gpu = vec![Vec::new(); forms.len()];
    for _ in 0..ROUNDS {
        for (k, &(_, frame)) in forms.iter().enumerate() {
            let queries = rig.device.create_query_set(&wgpu::QuerySetDescriptor {
                label: None,
                ty: wgpu::QueryType::Timestamp,
                count: 2 * FRAMES,
            });
            for f in 0..FRAMES {
                let started = std::time::Instant::now();
                let mut enc = rig.device.create_command_encoder(&Default::default());
                rig.encode_frame(&mut enc, (&world, &hand), frame, Some((&queries, 2 * f)));
                rig.queue.submit([enc.finish()]);
                cpu[k].push(started.elapsed().as_secs_f64() * 1e6);
            }
            gpu[k].extend(frame_gpu_times(&rig, &queries, FRAMES));
        }
    }
    println!("hand pass merge, {} x {}, {} frames per form:", SIZE.0, SIZE.1, FRAMES as usize * ROUNDS);
    for ((label, _), (c, g)) in forms.iter().zip(cpu.iter_mut().zip(&mut gpu)) {
        c.sort_by(f64::total_cmp);
        g.sort_by(f64::total_cmp);
        println!(
            "    {label}: CPU median {:.2} µs, best {:.2} µs; GPU median {:.2} µs, best {:.2} µs",
            c[c.len() / 2],
            c[0],
            g[g.len() / 2],
            g[0]
        );
    }
}

/// GPU time (µs) of each of `frames` frames, from the timestamp pairs of `queries`.
fn frame_gpu_times(rig: &Rig, queries: &wgpu::QuerySet, frames: u32) -> Vec<f64> {
    let resolved = rig.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 16 * u64::from(frames),
        usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let mut enc = rig.device.create_command_encoder(&Default::default());
    enc.resolve_query_set(queries, 0..2 * frames, &resolved, 0);
    rig.queue.submit([enc.finish()]);
    let ticks: Vec<u64> = bytemuck::cast_slice(&read(&rig.device, &rig.queue, &resolved)).to_vec();
    let period_ns = f64::from(rig.queue.get_timestamp_period());
    ticks.chunks(2).map(|t| t[1].saturating_sub(t[0]) as f64 * period_ns / 1e3).collect()
}
