//! Proof on a real GPU of the compact terrain formats (C3-M03): a column of the reference
//! capture is meshed in the old format (explicit quads of offset models, back
//! face of water as a second quad, 128-byte quads) and in the new one (compact offset
//! faces, double face, 108-byte translucents with slot), then drawn with the
//! real entry points of `terrain.wgsl`. The fragment shader writes the interpolated vertex
//! outputs (tint × light, fog distance, uv, fade, layer) into two
//! float targets blended by maximum: image independent of the draw order, bit-equal
//! if and only if the same triangles carry the same values; each view is
//! also drawn without blending (last triangle wins), an image that depends on the order. Views: from
//! above, underwater (back faces), flush with an offset model, eye 0.03 under the surface with
//! walking bob (projection centre above), folded water quad seen so that one
//! triangle faces front and the other back. Sabotages verified: corrupted offset table, double-face
//! bit removed; side decided by `cam.eye` (610,785 wrong values in the bobbed view),
//! triangle order not carried over (12,838 wrong values on the folded quad).
//!
//! Without data or without a GPU the test skips itself; under `RUSTCRAFT_OPT_REQUIRE=1` it is a failure.

#[path = "../../../crates/rc-mesh/tests/common/mod.rs"]
mod common;

use super::*;
use crate::terrain_shader::{FADE_RING, ShaderOpts};
use rc_mesh::fluid::{DOUBLE_FACE, FluidQuad, back_face};
use wgpu::util::DeviceExt;

const SIZE: u32 = 384;
const OLD: GpuFormats = GpuFormats { fluid96: false, trans_compact: false };
const NEW: GpuFormats = GpuFormats { fluid96: true, trans_compact: true };
const SLOT: u16 = 5;

const PROBE: &str = "
struct Probe {
    @location(0) a: vec4<f32>,
    @location(1) b: vec4<f32>,
};
@fragment
fn fs_probe(o: Out) -> Probe {
    let i = frag(o);
    var p: Probe;
    p.a = vec4<f32>(i.tint, i.fogd.x);
    p.b = vec4<f32>(i.uv, i.vis, f32(i.layer));
    return p;
}
";

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    module: wgpu::ShaderModule,
}

fn gpu() -> Option<Gpu> {
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
    let blend = adapter.features().contains(wgpu::Features::FLOAT32_BLENDABLE);
    rc_testdata::need(blend.then_some(()), "blending of float targets (FLOAT32_BLENDABLE)")?;
    eprintln!("GPU : {} ({:?})", adapter.get_info().name, adapter.get_info().backend);
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: wgpu::Features::FLOAT32_BLENDABLE,
        ..Default::default()
    }))
    .expect("device");
    let module = probe_module(&device, &crate::terrain_shader::terrain(ShaderOpts::NEW));
    Some(Gpu { device, queue, module })
}

/// Terrain shader `source` completed with the check fragment shader.
fn probe_module(device: &wgpu::Device, source: &str) -> wgpu::ShaderModule {
    let source = format!("{source}\n{PROBE}");
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl(source.into()),
    })
}

/// Variant drawn: shader, its options, face indices (sabotage possible).
struct Variant<'a> {
    module: &'a wgpu::ShaderModule,
    opts: ShaderOpts,
    index: Option<&'a [u16]>,
}

impl Gpu {
    fn default_variant(&self) -> Variant<'_> {
        Variant { module: &self.module, opts: ShaderOpts::NEW, index: None }
    }
}

/// GPU fade of the bench: duration 2^20 µs and births at multiples of 2^14 µs before the frame
/// instant, so that elapsed / duration comes out exact in f32 as in f64 (same image to the bit).
const FADE_NOW: u32 = 1 << 30;
const FADE_LEN: u32 = 1 << 20;

/// Visibility of bench cell `i` (m / 64) and the birth that gives it on the GPU.
fn bench_fade(i: usize) -> (f32, u32) {
    let m = 16 + (i % 37) as u32;
    (m as f32 / 64.0, FADE_NOW - m * (FADE_LEN / 64))
}

/// Common resources: baked faces, animation, fade, lightmap, sampler, offsets.
struct Res {
    quads: wgpu::Buffer,
    anim: wgpu::Buffer,
    fade: wgpu::Buffer,
    /// Same fade as birth instants (`FADEGPU`).
    fade_gpu: wgpu::Buffer,
    lightmap: wgpu::TextureView,
    sampler: wgpu::Sampler,
    slots: wgpu::Buffer,
}

fn storage(g: &Gpu, data: &[u8]) -> wgpu::Buffer {
    g.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: data,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::UNIFORM,
    })
}

/// Data of a draw: faces, fluids, translucents in format `f`.
struct Draw<'a> {
    p: &'a Prepared,
    f: GpuFormats,
    offsets: &'a [f32],
}

/// Image (two targets, 16 bytes per pixel each) of the faces, fluids and translucents seen by
/// `cam`.
fn render(g: &Gpu, r: &Res, d: &Draw<'_>, cam: &CameraUniform, key: (i32, i32), last: bool) -> Vec<u32> {
    render_timed(g, r, d, cam, key, last, 1, &g.default_variant()).0
}

/// [`render`] with the shader variant `v`.
fn render_v(
    g: &Gpu,
    r: &Res,
    d: &Draw<'_>,
    cam: &CameraUniform,
    key: (i32, i32),
    last: bool,
    v: &Variant<'_>,
) -> Vec<u32> {
    render_timed(g, r, d, cam, key, last, 1, v).0
}

/// [`render`] with each draw repeated `reps` times, and the duration (ms) from the upload to the end of the GPU.
#[allow(clippy::too_many_arguments)]
fn render_timed(
    g: &Gpu,
    r: &Res,
    d: &Draw<'_>,
    cam: &CameraUniform,
    key: (i32, i32),
    last: bool,
    reps: u32,
    v: &Variant<'_>,
) -> (Vec<u32>, f64) {
    let dev = &g.device;
    let cam = &CameraUniform { fade_now: FADE_NOW, fade_len: FADE_LEN as f32, ..*cam };
    let target = |label| {
        dev.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    };
    let targets = [target("a"), target("b")];
    let max = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Max,
    };
    let color = Some(wgpu::ColorTargetState {
        format: wgpu::TextureFormat::Rgba32Float,
        // `last`: without blending, the last triangle drawn wins (order-sensitive image).
        blend: (!last).then_some(wgpu::BlendState { color: max, alpha: max }),
        write_mask: wgpu::ColorWrites::ALL,
    });
    let pipeline = |vs: &str| {
        dev.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(vs),
            layout: None,
            vertex: wgpu::VertexState {
                module: v.module,
                entry_point: Some(vs),
                compilation_options: Default::default(),
                buffers: &[Some(instance_layout(vs))],
            },
            fragment: Some(wgpu::FragmentState {
                module: v.module,
                entry_point: Some("fs_probe"),
                compilation_options: Default::default(),
                targets: &[color.clone(), color.clone()],
            }),
            primitive: wgpu::PrimitiveState {
                topology: if vs == "vs" { v.opts.topology() } else { wgpu::PrimitiveTopology::TriangleList },
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        })
    };
    let cam_buf = storage(g, bytemuck::bytes_of(cam));
    let col = storage(g, bytemuck::cast_slice(&[key.0 * REGION, key.1 * REGION, 0, 0]));
    let offsets = storage(g, bytemuck::cast_slice(d.offsets));
    let pad = |w: &[u32]| -> Vec<u32> { if w.is_empty() { vec![0; 64] } else { w.to_vec() } };
    let faces = storage(g, bytemuck::cast_slice(&pad(bytemuck::cast_slice(&d.p.faces))));
    let fluids = storage(g, bytemuck::cast_slice(&pad(&d.p.fluids)));
    let trans = storage(g, bytemuck::cast_slice(&pad(&d.p.trans)));
    let order: Vec<u32> = (0..d.p.trans_len().max(1) as u32).collect();
    let order = storage(g, bytemuck::cast_slice(&order));
    use wgpu::BindingResource as B;
    let bind = |p: &wgpu::RenderPipeline, group: u32, entries: &[(u32, B<'_>)]| {
        dev.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &p.get_bind_group_layout(group),
            entries: &entries
                .iter()
                .map(|(binding, resource)| wgpu::BindGroupEntry { binding: *binding, resource: resource.clone() })
                .collect::<Vec<_>>(),
        })
    };
    let (vs_sorted, vs_fluid) = d.f.entries();
    let face_vs = v.opts.face_entry();
    let ids: Vec<u32> = (0..d.p.faces.len().max(1) as u32).collect();
    let ids = storage(g, bytemuck::cast_slice(&ids));
    let index_data = v.index.map_or_else(|| face_indices(INDEXED_FACES), <[u16]>::to_vec);
    let index = dev.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(&index_data),
        usage: wgpu::BufferUsages::INDEX,
    });
    let draws: [(&str, &wgpu::Buffer, u32, &wgpu::Buffer); 3] = [
        (face_vs, if face_vs == "vs_idx" { &ids } else { &faces }, d.p.faces.len() as u32, &faces),
        (vs_fluid, &fluids, d.p.fluid_len() as u32, &fluids),
        (vs_sorted, &order, d.p.trans_len() as u32, &trans),
    ];
    let mut enc = dev.create_command_encoder(&Default::default());
    let views = targets.each_ref().map(|t| t.create_view(&Default::default()));
    {
        let attach = |v| {
            Some(wgpu::RenderPassColorAttachment {
                view: v,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color { r: -1e9, g: -1e9, b: -1e9, a: -1e9 }),
                    store: wgpu::StoreOp::Store,
                },
            })
        };
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[attach(&views[0]), attach(&views[1])],
            ..Default::default()
        });
        for (vs, instances, n, data) in draws {
            if n == 0 {
                continue;
            }
            let p = pipeline(vs);
            let fade = if v.opts.fadegpu { &r.fade_gpu } else { &r.fade };
            let mut g0 = vec![
                (0, cam_buf.as_entire_binding()),
                (4, r.anim.as_entire_binding()),
                (6, fade.as_entire_binding()),
                (7, B::TextureView(&r.lightmap)),
                (8, B::Sampler(&r.sampler)),
            ];
            if vs == "vs" || vs == "vs_idx" {
                g0.extend([(1, r.quads.as_entire_binding()), (9, offsets.as_entire_binding())]);
            }
            let g1 = match vs {
                "vs_sorted" => vec![(1, data.as_entire_binding())],
                "vs_sorted_c" => vec![(2, data.as_entire_binding()), (3, r.slots.as_entire_binding())],
                "vs_idx" => vec![(0, col.as_entire_binding()), (4, data.as_entire_binding())],
                _ => vec![(0, col.as_entire_binding())],
            };
            let (b0, b1) = (bind(&p, 0, &g0), bind(&p, 1, &g1));
            pass.set_pipeline(&p);
            pass.set_bind_group(0, &b0, &[]);
            pass.set_bind_group(1, &b1, &[]);
            pass.set_vertex_buffer(0, instances.slice(..));
            if vs == "vs_idx" {
                // Same commands as the renderer: one instance per slice, first face as attribute.
                let mut iargs = Vec::new();
                indexed_args(
                    &[DrawArgs { vertex_count: 6, instance_count: n, first_vertex: 0, first_instance: 0 }],
                    &mut iargs,
                );
                pass.set_index_buffer(index.slice(..), wgpu::IndexFormat::Uint16);
                for _ in 0..reps {
                    for a in &iargs {
                        pass.draw_indexed(0..a.index_count, 0, a.first_instance..a.first_instance + 1);
                    }
                }
                continue;
            }
            let verts = if vs == "vs" { v.opts.face_vertices() } else { 6 };
            for _ in 0..reps {
                pass.draw(0..verts, 0..n);
            }
        }
    }
    let row = SIZE * 16;
    let out = dev.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(row * SIZE) * 2,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    for (i, t) in targets.iter().enumerate() {
        enc.copy_texture_to_buffer(
            t.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &out,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: u64::from(row * SIZE) * i as u64,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(SIZE),
                },
            },
            wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
        );
    }
    let t = std::time::Instant::now();
    g.queue.submit([enc.finish()]);
    out.slice(..).map_async(wgpu::MapMode::Read, |r| r.expect("read"));
    dev.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("GPU wait");
    let ms = t.elapsed().as_secs_f64() * 1e3;
    (bytemuck::pod_collect_to_vec(&out.slice(..).get_mapped_range().expect("mapping")[..]), ms)
}

/// Pixels covered (first channel above the background).
fn covered(img: &[u32]) -> usize {
    let f: Vec<f32> = img[..img.len() / 2].iter().map(|&w| f32::from_bits(w)).collect();
    f.as_chunks::<4>().0.iter().filter(|p| p[0] > -1e8).count()
}

fn camera(origin: [i32; 3], eye: Vec3, target: Vec3, up: Vec3, shade: [f32; 6]) -> CameraUniform {
    camera_tilted(origin, eye, target, up, shade, Mat4::IDENTITY)
}

/// Camera whose projection carries `tilt` in view space (walking bob): the projection centre
/// is no longer `eye`, which stays in the uniform as in the game (`render.rs`).
fn camera_tilted(origin: [i32; 3], eye: Vec3, target: Vec3, up: Vec3, shade: [f32; 6], tilt: Mat4) -> CameraUniform {
    let f = (target - eye).normalize();
    let s = f.cross(up).normalize();
    let u = s.cross(f);
    let view = Mat4::from_cols(
        Vec4::new(s.x, u.x, -f.x, 0.0),
        Vec4::new(s.y, u.y, -f.y, 0.0),
        Vec4::new(s.z, u.z, -f.z, 0.0),
        Vec4::new(-s.dot(eye), -u.dot(eye), f.dot(eye), 1.0),
    );
    let vp = perspective(70.0, 1.0, 0.05, 500.0) * tilt * view;
    CameraUniform {
        view_proj: vp.to_cols_array_2d(),
        origin: [origin[0], origin[1], origin[2], 0],
        shade,
        fade_now: 0,
        fade_len: 0.0,
        eye: [eye.x, eye.y, eye.z, 0.0],
        fog_color: [0.5, 0.6, 0.7, 1.0],
        fog: [8.0, 40.0, 12.0, 60.0],
        fog_sky: [0.0; 4],
        sky_color: [0.0; 4],
        sky_view_proj: Mat4::IDENTITY.to_cols_array_2d(),
    }
}

/// Baked faces, animation, fade, lightmap and slot table (column `col` in `SLOT`).
fn resources(g: &Gpu, t: &rc_mesh::blocks::Tables, col: (i32, i32)) -> Res {
    let quads: Vec<QuadGpu> = t.models.quads.iter().map(quad_gpu).collect();
    let frames: Vec<[u32; 2]> = (0..t.models.texture_names.len().max(1) as u32).map(|i| [i | i << 16, 0]).collect();
    let fade: Vec<f32> = (0..FADE_RING * FADE_RING).map(|i| bench_fade(i).0).collect();
    let fade_gpu: Vec<u32> = (0..FADE_RING * FADE_RING).map(|i| bench_fade(i).1).collect();
    let mut slots = vec![[0i32; 2]; 8];
    slots[usize::from(SLOT)] = [col.0, col.1];
    let lm = g.device.create_texture_with_data(
        &g.queue,
        &wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: 16, height: 16, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &(0..256u32)
            .flat_map(|i| [(i % 16 * 16) as u8, (i / 16 * 16) as u8, (i * 7 % 256) as u8, 255])
            .collect::<Vec<u8>>(),
    );
    Res {
        quads: storage(g, bytemuck::cast_slice(&quads)),
        anim: storage(g, bytemuck::cast_slice(&frames)),
        fade: storage(g, bytemuck::cast_slice(&fade)),
        fade_gpu: storage(g, bytemuck::cast_slice(&fade_gpu)),
        lightmap: lm.create_view(&Default::default()),
        sampler: g.device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        }),
        slots: storage(g, bytemuck::cast_slice(&slots)),
    }
}

/// Translucents alone (water, glass) in format `f`, slot set. For the images without blending
/// (last wins): the offset models change draw between the formats (explicit fluid quads,
/// then faces), which changes the order outside the water (known difference, `docs/perf.md`).
fn only_water(m: &BlockMesh, f: GpuFormats) -> Prepared {
    let mut p = prepare(m, false, f);
    p.faces.clear();
    p.fluids.clear();
    p.set_slot(SLOT);
    p
}

/// Column of the capture with offset models and double-sided water (more than 20 of each).
fn rich_column(cols: &common::Cols, mesh_with: &dyn Fn(i32, i32, bool) -> BlockMesh) -> (i32, i32) {
    let mut keys: Vec<(i32, i32)> = cols.keys().copied().collect();
    keys.sort_unstable();
    keys.into_iter()
        .find(|&(x, z)| {
            let m = mesh_with(x, z, true);
            m.layers.iter().flatten().filter(|f| f[0] >> 28 != 0).count() > 20
                && m.fluids[2].iter().filter(|q| q.texture & DOUBLE_FACE != 0).count() > 20
        })
        .expect("column with offset models and water")
}

#[test]
fn compact_formats_render_identically_on_gpu() {
    let Some((_d, cols, t, palette)) = common::load() else { return };
    let Some(g) = gpu() else { return };
    let mesh_with = |x, z, compact: bool| {
        let mut j = common::job(&cols, x, z);
        j.opts.offset_faces = compact;
        j.opts.water_double = compact;
        rc_mesh::blocks::mesh(&j, &t, &palette, common::MIN_Y)
    };
    let (x, z) = rich_column(&cols, &mesh_with);
    let (old_m, new_m) = (mesh_with(x, z, false), mesh_with(x, z, true));
    let old = prepare(&old_m, false, OLD);
    let mut new = prepare(&new_m, false, NEW);
    new.set_slot(SLOT);
    let (old_water, new_water) = (only_water(&old_m, OLD), only_water(&new_m, NEW));
    eprintln!("column ({x}, {z}): old {} B, new {} B", old.bytes(), new.bytes());

    let res = resources(&g, &t, (x, z));
    let table = t.offset_table();
    let key = (x.div_euclid(REGION), z.div_euclid(REGION));
    let origin = [x * 16, common::MIN_Y, z * 16];
    let shade = common::job(&cols, x, z).shade;
    // Vertex frame: x, z relative to the origin, y absolute.
    let at = |p: [f32; 3]| Vec3::new(p[0], p[1] + common::MIN_Y as f32, p[2]);
    let water = new_m.fluids[2].iter().find(|q| q.texture & DOUBLE_FACE != 0).expect("water");
    let c = at(std::array::from_fn(|a| (water.corners[0][a] + water.corners[2][a]) * 0.5));
    let f = new_m.layers.iter().flatten().find(|f| f[0] >> 28 != 0).expect("offset model");
    let b = at([(f[0] & 15) as f32 + 0.5, ((f[0] >> 8) & 4095) as f32 + 0.5, ((f[0] >> 4) & 15) as f32 + 0.5]);
    let views = [
        ("from above", camera(origin, c + Vec3::new(-6.0, 10.0, -9.0), c, Vec3::Y, shade)),
        ("underwater", camera(origin, c + Vec3::new(0.3, -3.0, 0.4), c + Vec3::new(0.0, 1.0, 0.0), Vec3::Z, shade)),
        ("flush with an offset model", camera(origin, b + Vec3::new(2.5, 1.5, 3.0), b, Vec3::Y, shade)),
    ];
    let bad_table: Vec<f32> = table.iter().map(|v| v + 1.0 / 64.0).collect();
    let mut single = prepare(&new_m, false, NEW);
    for q in single.trans.as_chunks_mut::<TRANS_WORDS>().0 {
        q[20] &= !0x8000;
    }
    single.set_slot(SLOT);
    for (i, (name, cam)) in views.iter().enumerate() {
        let a = render(&g, &res, &Draw { p: &old, f: OLD, offsets: &table }, cam, key, false);
        let n = render(&g, &res, &Draw { p: &new, f: NEW, offsets: &table }, cam, key, false);
        let px = covered(&a);
        eprintln!("view '{name}': {px} pixels covered");
        assert!(px > 2000, "view '{name}' almost empty");
        let diff = a.iter().zip(&n).filter(|(p, q)| p != q).count();
        assert_eq!(diff, 0, "view '{name}': {diff} values differ between old and new format");
        let a_last = render(&g, &res, &Draw { p: &old_water, f: OLD, offsets: &table }, cam, key, true);
        let n_last = render(&g, &res, &Draw { p: &new_water, f: NEW, offsets: &table }, cam, key, true);
        assert!(a_last == n_last, "view '{name}': triangle order differs (last wins)");
        // Sabotages: each changes the image of the view that shows it.
        if i == 2 {
            let s = render(&g, &res, &Draw { p: &new, f: NEW, offsets: &bad_table }, cam, key, false);
            assert_ne!(s, a, "corrupted offset table not seen");
        }
        if i == 1 {
            let s = render(&g, &res, &Draw { p: &single, f: NEW, offsets: &table }, cam, key, false);
            assert_ne!(s, a, "missing water back faces not seen");
        }
    }

    // Flat water top (double face, four corners at the same height).
    let top = *new_m.fluids[2]
        .iter()
        .find(|q| q.texture & DOUBLE_FACE != 0 && q.corners.iter().all(|p| p[1] == q.corners[0][1]))
        .expect("flat water top");
    let corner = |i: usize| at([top.corners[i][0], top.corners[i][1], top.corners[i][2]]);
    let ct = (corner(0) + corner(2)) * 0.5;
    // Walking at the bottom of the water: eye 0.03 under the surface, maximum bob (translation of
    // 0.1 downwards in view space): the rasterisation sees the top, `cam.eye` the bottom.
    let bob = Mat4::from_translation(Vec3::new(0.0, -0.1, 0.0));
    let cam =
        camera_tilted(origin, ct + Vec3::new(-0.3, -0.03, -0.4), ct + Vec3::new(0.3, -0.5, 0.4), Vec3::Y, shade, bob);
    let a = render(&g, &res, &Draw { p: &old, f: OLD, offsets: &table }, &cam, key, false);
    let n = render(&g, &res, &Draw { p: &new, f: NEW, offsets: &table }, &cam, key, false);
    assert!(covered(&a) > 2000, "bobbed view almost empty");
    let diff = a.iter().zip(&n).filter(|(p, q)| p != q).count();
    assert_eq!(diff, 0, "bobbed view near the surface: {diff} values differ");

    // Quad folded along diagonal 0-2 (corner 1 raised by 0.8), eye on the side of corner 1, under
    // the raised triangle and above the other: one triangle faces front, the other back, with
    // overlapping projections. Without blending, the image depends on the order of the two triangles.
    let fold = |compact: bool| {
        let mut m = mesh_with(x, z, compact);
        let quads = &mut m.fluids[2];
        let i = quads.iter().position(|q| q.corners == top.corners).expect("quad to fold");
        quads[i].corners[1][1] += 0.8;
        if !compact {
            assert_eq!(quads[i + 1].corners, back_face(&FluidQuad { corners: top.corners, ..quads[i] }).corners);
            quads[i + 1] = back_face(&quads[i]);
        }
        only_water(&m, if compact { NEW } else { OLD })
    };
    let (old_f, new_f) = (fold(false), fold(true));
    let mid = ct;
    let out = Vec3::new(corner(1).x - mid.x, 0.0, corner(1).z - mid.z);
    let eye = mid + out.normalize() * (out.length() + 1.0) + Vec3::Y;
    let cam = camera(origin, eye, mid, Vec3::Y, shade);
    let a = render(&g, &res, &Draw { p: &old_f, f: OLD, offsets: &table }, &cam, key, true);
    let n = render(&g, &res, &Draw { p: &new_f, f: NEW, offsets: &table }, &cam, key, true);
    assert!(covered(&a) > 2000, "folded quad view almost empty");
    let diff = a.iter().zip(&n).filter(|(p, q)| p != q).count();
    assert_eq!(diff, 0, "folded quad, last triangle wins: {diff} values differ");
}

/// GPU micro-bench of double-sided water: the water-rich column drawn 2,000 times per frame
/// (time from the upload to the end of the GPU, best of 9 alternating rounds) in the old format (back
/// face as a second quad), the new one (one quad, side decided per vertex) and the new one without the
/// double-face bit (same quad without the test: cost of the test alone, wrong image). Views: from above, underwater
/// (back faces seen), out of view (everything clipped: vertex work alone):
/// `cargo test --release -p rc-app --bin rustcraft water_side_cost -- --ignored --nocapture`.
#[test]
#[ignore]
fn water_side_cost() {
    let Some((_d, cols, t, palette)) = common::load() else { return };
    let Some(g) = gpu() else { return };
    let mesh_with = |x, z, compact: bool| {
        let mut j = common::job(&cols, x, z);
        j.opts.offset_faces = compact;
        j.opts.water_double = compact;
        rc_mesh::blocks::mesh(&j, &t, &palette, common::MIN_Y)
    };
    let (x, z) = rich_column(&cols, &mesh_with);
    let (old_m, new_m) = (mesh_with(x, z, false), mesh_with(x, z, true));
    // Translucents only (water): the other draws are common to the three formats.
    let old = only_water(&old_m, OLD);
    let new = only_water(&new_m, NEW);
    let mut single = only_water(&new_m, NEW);
    for q in single.trans.as_chunks_mut::<TRANS_WORDS>().0 {
        q[20] &= !0x8000;
    }
    let res = resources(&g, &t, (x, z));
    let table = t.offset_table();
    let key = (x.div_euclid(REGION), z.div_euclid(REGION));
    let at = |p: [f32; 3]| Vec3::new(p[0], p[1] + common::MIN_Y as f32, p[2]);
    let water = new_m.fluids[2].iter().find(|q| q.texture & DOUBLE_FACE != 0).expect("water");
    let c = at(std::array::from_fn(|a| (water.corners[0][a] + water.corners[2][a]) * 0.5));
    let origin = [x * 16, common::MIN_Y, z * 16];
    let shade = common::job(&cols, x, z).shade;
    let views = [
        ("from above", camera(origin, c + Vec3::new(-6.0, 10.0, -9.0), c, Vec3::Y, shade)),
        ("underwater", camera(origin, c + Vec3::new(0.3, -3.0, 0.4), c + Vec3::new(0.0, 1.0, 0.0), Vec3::Z, shade)),
        // Back to the column: everything is clipped, only the vertex work remains.
        ("out of view", camera(origin, c + Vec3::new(0.0, 0.0, -40.0), c + Vec3::new(0.0, 0.0, -80.0), Vec3::Y, shade)),
    ];
    const REPS: u32 = 2000;
    eprintln!("column ({x}, {z}): {} water quads before, {} after", old.trans_len(), new.trans_len());
    for (name, cam) in &views {
        let mut best = [f64::MAX; 3];
        for _ in 0..9 {
            for (i, (p, f)) in [(&old, OLD), (&new, NEW), (&single, NEW)].into_iter().enumerate() {
                let (_, ms) = render_timed(
                    &g,
                    &res,
                    &Draw { p, f, offsets: &table },
                    cam,
                    key,
                    false,
                    REPS,
                    &g.default_variant(),
                );
                best[i] = best[i].min(ms);
            }
        }
        eprintln!(
            "view '{name}': old {:.2} ms, double-sided {:.2} ms ({:+.1}%), without test {:.2} ms (loaded machine)",
            best[0],
            best[1],
            (best[1] / best[0] - 1.0) * 100.0,
            best[2]
        );
    }
}

/// Micro-bench of the formats on the 1,248 columns of the capture: bytes uploaded to the GPU per
/// combination (offset faces and double face at meshing; FLUID96 and TRANSCOMPACT at upload),
/// meshing and preparation times, 1 thread then all:
/// `cargo test --release -p rc-app --bin rustcraft format_bench -- --ignored --nocapture`.
#[test]
#[ignore]
fn format_bench() {
    use rayon::prelude::*;
    let Some((_d, cols, t, palette)) = common::load() else { return };
    let mut keys: Vec<(i32, i32)> = cols.keys().copied().collect();
    keys.sort_unstable();
    let mesh_all = |offset: bool, double: bool| -> Vec<BlockMesh> {
        keys.par_iter()
            .map(|&(x, z)| {
                let mut j = common::job(&cols, x, z);
                (j.opts.offset_faces, j.opts.water_double) = (offset, double);
                rc_mesh::blocks::mesh(&j, &t, &palette, common::MIN_Y)
            })
            .collect()
    };
    for (offset, double) in [(false, false), (true, false), (false, true), (true, true)] {
        let ms = mesh_all(offset, double);
        for f in [
            OLD,
            GpuFormats { fluid96: offset, trans_compact: false },
            GpuFormats { fluid96: offset, trans_compact: true },
        ] {
            let (mut faces, mut fluids, mut trans) = (0usize, 0usize, 0usize);
            for m in &ms {
                let p = prepare(m, true, f);
                faces += p.faces.len() * 24;
                fluids += p.fluids.len() * 4;
                trans += p.trans.len() * 4;
            }
            let mb = |b: usize| b as f64 / 1e6;
            eprintln!(
                "offset {offset:5} double {double:5} {f:?}: faces {:.1} MB, fluids {:.1} MB, translucents {:.1} MB, total {:.1} MB",
                mb(faces),
                mb(fluids),
                mb(trans),
                mb(faces + fluids + trans)
            );
        }
    }
    for threads in [1, rayon::current_num_threads()] {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().expect("pool");
        for (offset, double) in [(false, false), (true, true), (false, false), (true, true)] {
            let t0 = std::time::Instant::now();
            let ms = pool.install(|| mesh_all(offset, double));
            let mesh_ms = t0.elapsed().as_secs_f64() * 1e3;
            let f = if offset { NEW } else { OLD };
            let t1 = std::time::Instant::now();
            let n: usize = pool.install(|| ms.par_iter().map(|m| prepare(m, true, f).bytes()).sum());
            eprintln!(
                "{threads} threads, formats {}: meshing {mesh_ms:.0} ms, preparation {:.1} ms ({n} B)",
                if offset { "compact" } else { "old" },
                t1.elapsed().as_secs_f64() * 1e3
            );
        }
    }
}

/// Shader variants drawn by the proofs and the bench: old path, each
/// switch alone, then the combinations.
fn shader_variants() -> Vec<(&'static str, ShaderOpts)> {
    let old = ShaderOpts::OLD;
    vec![
        ("VSREAD", ShaderOpts { vsread: true, ..old }),
        ("QUADBITS", ShaderOpts { quadbits: true, ..old }),
        ("VARYINGS", ShaderOpts { varyings: true, ..old }),
        ("FADEGPU", ShaderOpts { fadegpu: true, ..old }),
        ("FACEIDX", ShaderOpts { faceidx: true, ..old }),
        ("STRIP", ShaderOpts { strip: true, ..old }),
        ("targeted read (VSREAD + QUADBITS)", ShaderOpts { vsread: true, quadbits: true, ..old }),
        ("all except FACEIDX (strip)", ShaderOpts { faceidx: false, ..ShaderOpts::NEW }),
        ("all (default)", ShaderOpts::NEW),
    ]
}

/// Rich column meshed in the old format and in the new one, and its views.
type Scene = (Vec<(Prepared, GpuFormats)>, Res, Vec<f32>, (i32, i32), Vec<(&'static str, CameraUniform)>);

fn scene(g: &Gpu) -> Option<Scene> {
    let (_d, cols, t, palette) = common::load()?;
    let mesh_with = |x, z, compact: bool| {
        let mut j = common::job(&cols, x, z);
        j.opts.offset_faces = compact;
        j.opts.water_double = compact;
        rc_mesh::blocks::mesh(&j, &t, &palette, common::MIN_Y)
    };
    let (x, z) = rich_column(&cols, &mesh_with);
    let (old_m, new_m) = (mesh_with(x, z, false), mesh_with(x, z, true));
    let old = prepare(&old_m, false, OLD);
    let mut new = prepare(&new_m, false, NEW);
    new.set_slot(SLOT);
    let res = resources(g, &t, (x, z));
    let key = (x.div_euclid(REGION), z.div_euclid(REGION));
    let origin = [x * 16, common::MIN_Y, z * 16];
    let shade = common::job(&cols, x, z).shade;
    let at = |p: [f32; 3]| Vec3::new(p[0], p[1] + common::MIN_Y as f32, p[2]);
    let water = new_m.fluids[2].iter().find(|q| q.texture & DOUBLE_FACE != 0).expect("water");
    let c = at(std::array::from_fn(|a| (water.corners[0][a] + water.corners[2][a]) * 0.5));
    let f = new_m.layers.iter().flatten().find(|f| f[0] >> 28 != 0).expect("offset model");
    let b = at([(f[0] & 15) as f32 + 0.5, ((f[0] >> 8) & 4095) as f32 + 0.5, ((f[0] >> 4) & 15) as f32 + 0.5]);
    let views = vec![
        ("from above", camera(origin, c + Vec3::new(-6.0, 10.0, -9.0), c, Vec3::Y, shade)),
        ("underwater", camera(origin, c + Vec3::new(0.3, -3.0, 0.4), c + Vec3::new(0.0, 1.0, 0.0), Vec3::Z, shade)),
        ("flush with an offset model", camera(origin, b + Vec3::new(2.5, 1.5, 3.0), b, Vec3::Y, shade)),
        // Back to the column: everything is clipped, only the vertex work remains (bench).
        ("out of view", camera(origin, c + Vec3::new(0.0, 0.0, -40.0), c + Vec3::new(0.0, 0.0, -80.0), Vec3::Y, shade)),
    ];
    Some((vec![(old, OLD), (new, NEW)], res, t.offset_table(), key, views))
}

/// Proof C3-M05: each shader variant (targeted read, corners in nibbles, grouped outputs,
/// GPU fade, indexed faces, strip) alone then combined gives, for both quad
/// formats and three views, the same image as the old path to the bit: blended by
/// maximum (same triangles, same values) and without blending (same triangle order).
/// Sabotages seen: wrong nibbles, wrong index, blended outputs, reversed fade, v of the wrong corner.
#[test]
fn shader_variants_render_identically_on_gpu() {
    let Some(g) = gpu() else { return };
    let Some((preps, res, table, key, views)) = scene(&g) else { return };
    let old_mod = probe_module(&g.device, &crate::terrain_shader::terrain(ShaderOpts::OLD));
    let old = Variant { module: &old_mod, opts: ShaderOpts::OLD, index: None };
    let variants: Vec<(&str, ShaderOpts, wgpu::ShaderModule)> = shader_variants()
        .into_iter()
        .map(|(n, o)| (n, o, probe_module(&g.device, &crate::terrain_shader::terrain(o))))
        .collect();
    for (p, f) in &preps {
        let d = Draw { p, f: *f, offsets: &table };
        for (name, cam) in views.iter().take(3) {
            let base = render_v(&g, &res, &d, cam, key, false, &old);
            let base_last = render_v(&g, &res, &d, cam, key, true, &old);
            assert!(covered(&base) > 2000, "view '{name}' almost empty");
            for (vname, o, m) in &variants {
                let v = Variant { module: m, opts: *o, index: None };
                let a = render_v(&g, &res, &d, cam, key, false, &v);
                let diff = a.iter().zip(&base).filter(|(p, q)| p != q).count();
                assert_eq!(diff, 0, "{f:?}, view '{name}', {vname}: {diff} values differ");
                let l = render_v(&g, &res, &d, cam, key, true, &v);
                let diff = l.iter().zip(&base_last).filter(|(p, q)| p != q).count();
                assert_eq!(diff, 0, "{f:?}, view '{name}', {vname}, last wins: {diff} values differ");
            }
            eprintln!("{f:?}, view '{name}': {} identical variants", variants.len());
        }
    }

    // Sabotages, new format, view from above (water and offset faces visible).
    let (p, f) = &preps[1];
    let d = Draw { p, f: *f, offsets: &table };
    let cam = &views[0].1;
    let base = render_v(&g, &res, &d, cam, key, false, &old);
    let sabotage = |name: &str, o: ShaderOpts, from: &str, to: &str| {
        let src = crate::terrain_shader::terrain(o);
        assert!(src.contains(from), "{name}: pattern '{from}' absent");
        let m = probe_module(&g.device, &src.replace(from, to));
        let s = render_v(&g, &res, &d, cam, key, false, &Variant { module: &m, opts: o, index: None });
        assert_ne!(s, base, "sabotage '{name}' not seen");
    };
    let only = |set: fn(&mut ShaderOpts)| {
        let mut o = ShaderOpts::OLD;
        set(&mut o);
        o
    };
    sabotage("nibbles 0x230210", only(|o| o.quadbits = true), "0x320210u", "0x230210u");
    sabotage(
        "v of the next corner",
        only(|o| o.vsread = true),
        "quads[qi].v[corner]",
        "quads[qi].v[(corner + 1u) & 3u]",
    );
    sabotage(
        "fog reversed in the outputs",
        only(|o| o.varyings = true),
        "vec4<f32>(f.uv, f.fogd)",
        "vec4<f32>(f.uv, f.fogd.yx)",
    );
    sabotage("fade reversed", only(|o| o.fadegpu = true), "e >= cam.fade_len", "e <= cam.fade_len");
    sabotage("strip as 0, 1, 2, 3", only(|o| o.strip = true), "0x3021u", "0x3210u");
    let bad: Vec<u16> =
        face_indices(INDEXED_FACES).chunks(6).flat_map(|t| [t[0], t[1], t[2], t[0], t[5], t[2]]).collect();
    let idx = only(|o| o.faceidx = true);
    let m = probe_module(&g.device, &crate::terrain_shader::terrain(idx));
    let s = render_v(&g, &res, &d, cam, key, false, &Variant { module: &m, opts: idx, index: Some(&bad) });
    assert_ne!(s, base, "sabotage 'index 0, 3, 2' not seen");
}

/// GPU micro-bench of the shader variants: the rich column (old and new formats)
/// drawn `REPS` times per frame, best of 7 alternating rounds, time from the upload to the end of the
/// GPU; views "from above" (all the work) and "out of view" (vertices alone):
/// `cargo test --release -p rc-app --bin rustcraft shader_variants_cost -- --ignored --nocapture`.
#[test]
#[ignore]
fn shader_variants_cost() {
    let Some(g) = gpu() else { return };
    let Some((preps, res, table, key, views)) = scene(&g) else { return };
    const REPS: u32 = 400;
    let mut all = vec![("old", ShaderOpts::OLD)];
    all.extend(shader_variants());
    let modules: Vec<wgpu::ShaderModule> =
        all.iter().map(|(_, o)| probe_module(&g.device, &crate::terrain_shader::terrain(*o))).collect();
    for (p, f) in &preps {
        let d = Draw { p, f: *f, offsets: &table };
        eprintln!("{f:?}: {} faces, {} fluids, {} translucent", p.faces.len(), p.fluid_len(), p.trans_len());
        for (name, cam) in views.iter().filter(|(n, _)| *n == "from above" || *n == "out of view") {
            let mut best = vec![f64::MAX; all.len()];
            for _ in 0..7 {
                for (i, ((_, o), m)) in all.iter().zip(&modules).enumerate() {
                    let v = Variant { module: m, opts: *o, index: None };
                    let (_, ms) = render_timed(&g, &res, &d, cam, key, false, REPS, &v);
                    best[i] = best[i].min(ms);
                }
            }
            for (i, (vname, _)) in all.iter().enumerate() {
                eprintln!(
                    "  view '{name}', {vname:38}: {:7.2} ms ({:+.1}%) (loaded machine)",
                    best[i],
                    (best[i] / best[0] - 1.0) * 100.0
                );
            }
        }
    }
}
