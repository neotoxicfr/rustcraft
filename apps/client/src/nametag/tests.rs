//! Proofs of the kept glyphs (`RUSTCRAFT_OPT_TAGPERSIST`): a deterministic script of
//! 300 frames (200 named entities, appearances and disappearances, names changed, crouching posture,
//! obfuscated text, teams and background opacity changed, improved transparency toggled) played by
//! the old and the new path; the glyphs drawn by each pass, read back in a copy of the
//! GPU buffer that receives only the uploads, are identical (exact order for the normal
//! and see-through passes, as a set for the solid parts and improved-transparency stages, whose
//! order was already arbitrary). Then captures on the real GPU of the normal and see-through passes.
//! A skipped upload makes both proofs fail (checked right here).
use super::*;
use bytemuck::Zeroable;

/// Default opacity: text 127, background 63; "everywhere" text background at 0.8: background 204,
/// text (0.8 + 0.75) / 2 = 0.775 → 197 (the old code kept 127 / 63).
#[test]
fn background_opacity_drives_tag_colors() {
    assert_eq!(tag_colors(DEFAULT_OPACITY), (127 << 24 | 0xff_ffff, 63 << 24));
    assert_eq!(tag_colors(0.8), (197 << 24 | 0xff_ffff, 204 << 24));
    assert_eq!(tag_colors(0.0), (127 << 24 | 0xff_ffff, 0));
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

/// Dummy line: background fill if `bg` is visible, a 5 × 8 glyph per byte ("~"
/// translucent in the middle of the opaque text, italic capitals, pages 0 and 1).
fn line(name: &str, color: u32, bg: u32, salt: u32) -> Vec<Glyph> {
    let w = name.len() as f32 * 6.0;
    let x0 = -w / 2.0;
    let mut v = Vec::new();
    if bg >> 24 != 0 {
        v.push(Glyph { rect: [x0 - 1.0, x0 + w, -1.0, 9.0], skew: 0.0, uv: [0.0; 4], color: bg, mode: 0 });
    }
    for (i, b) in name.bytes().enumerate() {
        let x = x0 + i as f32 * 6.0;
        let u = ((u32::from(b) + salt) % 4) as f32 * 0.25;
        v.push(Glyph {
            rect: [x, x + 5.0, 0.0, 8.0],
            skew: if b.is_ascii_uppercase() { 1.0 } else { 0.0 },
            uv: [u, 0.0, u + 0.25, 1.0],
            color: if b == b'~' { color & 0xff_ffff | 0x80 << 24 } else { color },
            mode: 1 + u32::from(b % 2),
        });
    }
    v
}

/// Dummy layout (same passes as the interface); "#" = obfuscated, drawn on every frame.
fn fake(name: &str, (text, bg): (u32, u32), discrete: bool, frame: u32) -> (Vec<Glyph>, Vec<Glyph>, bool) {
    let obf = name.contains('#');
    let salt = if obf { frame } else { 0 };
    if discrete {
        (line(name, text, bg, salt), Vec::new(), obf)
    } else {
        (line(name, 0xffff_ffff, 0, salt), line(name, text, bg, salt), obf)
    }
}

/// Name of an entity; entity 113 has a very long name among the short ones (hologram).
fn name_of(id: i32, frame: u32) -> String {
    if id == 113 {
        return format!("Holo{}", "gram~~~".repeat(12));
    }
    let epoch = if id % 5 == 0 { frame / 23 } else { 0 };
    let mark = match id {
        _ if id % 53 == 0 => "#",
        _ if id % 7 == 0 => "~",
        _ => "",
    };
    format!("Va{mark}che{id}.{epoch}")
}

type Passes = Vec<Vec<Vec<u8>>>;

fn bytes(t: &TagInstance) -> Vec<u8> {
    bytemuck::bytes_of(t).to_vec()
}

fn sorted(mut v: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
    v.sort();
    v
}

/// Glyphs drawn by each pass of the old path.
fn old_passes(l: &Lists, oit: bool) -> Passes {
    let b = |s: &[TagInstance]| s.iter().map(bytes).collect::<Vec<_>>();
    if oit {
        let s = l.solid.min(l.counts.0) as usize;
        vec![sorted(b(&l.normal_inst[..s])), sorted(b(&l.normal_inst[s..])), b(&l.see_inst)]
    } else {
        vec![b(&l.normal_inst), b(&l.see_inst)]
    }
}

/// Glyphs drawn by each pass of the new path, read from the GPU buffer copy, draws
/// taken in order; and worst ratio of vertices drawn / vertices of a pass's glyphs.
fn new_passes(l: &Lists, mirror: &[Stored], oit: bool) -> (Passes, f64) {
    let mut worst = 0f64;
    let mut pass = |p: usize| {
        let (a, b) = l.passes[p];
        let (mut drawn, mut useful) = (0u64, 0u64);
        let mut next = None;
        let mut out = Vec::new();
        for &(start, n, most) in &l.segs[a as usize..b as usize] {
            assert!(n > 0, "empty draw");
            assert!(next.is_none_or(|s| s == start), "draws of a pass not contiguous");
            next = Some(start + n);
            let refs = &l.refs[start as usize..(start + n) as usize];
            assert!(refs.iter().all(|r| r.count > 0 && r.count <= most), "vertices per instance too short");
            drawn += 6 * u64::from(most) * u64::from(n);
            useful += refs.iter().map(|r| 6 * u64::from(r.count)).sum::<u64>();
            out.extend(refs.iter().flat_map(|r| {
                (0..r.count).map(move |g| {
                    let s = mirror[(r.first + g) as usize];
                    let glyph = Glyph { rect: s.rect, skew: s.skew, uv: s.uv, color: s.color, mode: s.mode };
                    bytes(&TagInstance { anchor: r.anchor, light: r.light, glyph })
                })
            }));
        }
        if useful > 0 {
            worst = worst.max(drawn as f64 / useful as f64);
        }
        out
    };
    let passes =
        if oit { vec![sorted(pass(SOLID)), sorted(pass(REST)), pass(SEE)] } else { vec![pass(NORMAL), pass(SEE)] };
    (passes, worst)
}

/// Applies the frame's upload to the GPU buffer copy (recreated empty if it grows).
fn apply(mirror: &mut Vec<Stored>, l: &Lists, send: Option<StoreSend>) {
    let Some(s) = send else { return };
    if let Some(cap) = s.grow {
        *mirror = vec![Stored::zeroed(); cap];
    }
    let g = l.store.glyphs.get(s.from..).unwrap_or_default();
    mirror[s.from..s.from + g.len()].copy_from_slice(g);
}

const FRAMES: u32 = 300;
const TAGS: i32 = 200;

/// Plays the script; returns the passes of each frame, the number of compactions and the worst ratio
/// vertices drawn / vertices of the glyphs (new path). `unsplit`: passes in a single draw.
fn play(persist: bool, sabotage: bool, unsplit: bool) -> (Vec<Passes>, u32, f64) {
    let mut l = Lists::new(persist);
    l.store.sabotage = sabotage;
    l.unsplit = unsplit;
    let mut worst = 0f64;
    let mut mirror = Vec::new();
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let mut out = Vec::new();
    for frame in 0..FRAMES {
        if frame % 100 == 50 {
            l.sync(u64::from(frame));
        }
        if frame == 150 {
            l.set_background(0.8);
        }
        let oit = (frame / 30) % 2 == 1;
        if frame % 37 == 36 {
            // Frame without a name tag (no frame start).
            l.clear();
        } else {
            l.begin(Vec3::X, Vec3::Y);
            for id in 0..TAGS {
                if rng.below(10) == 0 {
                    continue;
                }
                let name = name_of(id, frame);
                let discrete = id % 17 == 0 && frame % 40 < 10;
                let anchor = Vec3::new(id as f32 * 0.5, frame as f32 * 0.01, -3.0);
                let light = ((id % 16) as u8, (frame % 16) as u8);
                l.push_with(id, NameRef::Text(&name), anchor, discrete, light, |c| fake(&name, c, discrete, frame));
            }
        }
        let send = l.finish(oit);
        if persist {
            apply(&mut mirror, &l, send);
            let (passes, w) = new_passes(&l, &mirror, oit);
            worst = worst.max(w);
            out.push(passes);
        } else {
            out.push(old_passes(&l, oit));
        }
    }
    (out, l.store.compactions, worst)
}

#[test]
fn persistent_glyphs_draw_the_same_instances() {
    let (old, ..) = play(false, false, false);
    let (new, compactions, worst) = play(true, false, false);
    assert!(compactions > 0, "the script must compact the reserve");
    let drawn: usize = old.iter().map(|f| f.iter().map(Vec::len).sum::<usize>()).sum();
    assert!(drawn > 1_000_000, "script too poor: {drawn} glyphs");
    for (i, (a, b)) in old.iter().zip(&new).enumerate() {
        assert!(a == b, "frame {i}: different passes");
    }
    // A long name among the short ones: vertices drawn at most twice those of the glyphs.
    assert!(worst <= 2.0, "vertices drawn: {worst:.2} × those of the glyphs");
    // Sabotage: additions to the reserve never uploaded.
    let (bad, ..) = play(true, true, false);
    assert!(old != bad, "the sabotage must be seen");
    // Sabotage: a pass in a single draw (vertices of the longest name for each tag).
    let (same, _, unbounded) = play(true, false, true);
    assert!(same == old, "a single draw draws the same glyphs");
    assert!(unbounded > 4.0, "the long name must cost without splitting: {unbounded:.2}");
}

/// CPU micro-bench (1 core, loaded machine): 200 tags kept per frame, old and new
/// path, and bytes uploaded per frame. `cargo test --release -p rc-app nametag::tests::bench -- --ignored --nocapture`
#[test]
#[ignore]
fn bench() {
    for (persist, long) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut l = Lists::new(persist);
        // `long`: an armor stand with a 120-character name among the cows.
        let mut names: Vec<String> = (0..TAGS).map(|id| format!("Cows_{id}")).collect();
        if long {
            names[TAGS as usize / 2] = "ArmorStand".repeat(12);
        }
        let mut times = Vec::new();
        let mut sent = 0usize;
        let (mut verts, mut draws) = (0u64, 0usize);
        for frame in 0..2000u32 {
            let t = std::time::Instant::now();
            l.begin(Vec3::X, Vec3::Y);
            for (id, name) in names.iter().enumerate() {
                let anchor = Vec3::new(id as f32, frame as f32 * 0.01, -3.0);
                l.push_with(id as i32, NameRef::Text(name), anchor, false, (0, 15), |c| fake(name, c, false, frame));
            }
            let send = l.finish(false);
            times.push(t.elapsed().as_secs_f64() * 1e6);
            if frame >= 1000 {
                for p in [NORMAL, SEE] {
                    let (a, b) = l.passes[p];
                    let segs = &l.segs[a as usize..b as usize];
                    verts += segs.iter().map(|&(_, n, most)| 6 * u64::from(n) * u64::from(most)).sum::<u64>();
                    draws += segs.len();
                }
                sent += 32
                    + if persist {
                        let dirty = send.map_or(0, |s| l.store.glyphs.len() - s.from);
                        l.refs.len() * size_of::<TagRef>() + dirty * size_of::<Stored>()
                    } else {
                        (l.normal_inst.len() + l.see_inst.len()) * size_of::<TagInstance>()
                    };
            }
        }
        times.sort_by(f64::total_cmp);
        eprintln!(
            "persist={persist} long={long}: median {:.1} µs, p90 {:.1} µs per frame; {} bytes sent, {} vertices drawn, {} draws per frame",
            times[1000],
            times[1800],
            sent / 1000,
            verts / 1000,
            draws / 1000
        );
    }
}

// --- Captures on the real GPU -------------------------------------------------------------------

fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    crate::render::upload::tests::gpu_serial();
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .ok();
    let adapter = rc_testdata::need(adapter, "GPU adapter")?;
    // Blended float targets: improved transparency stages.
    let blend = adapter.features().contains(wgpu::Features::FLOAT32_BLENDABLE);
    rc_testdata::need(blend.then_some(()), "float target blending (FLOAT32_BLENDABLE)")?;
    let desc = wgpu::DeviceDescriptor { required_features: wgpu::Features::FLOAT32_BLENDABLE, ..Default::default() };
    Some(pollster::block_on(adapter.request_device(&desc)).expect("device"))
}

const SIZE: u32 = 256;

fn texture(
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

/// Four frames of 40 tags (names changed at the third, obfuscated ones redrawn on every frame),
/// normal then see-through passes in a single render pass; returns the pixels. `oit`:
/// full improved-transparency chain (opaque glyphs with the solids, three stages
/// with culling, composition) then see-through; each tag at its own depth, because
/// the order of the opaque ones was already arbitrary in the old path.
fn capture(device: &wgpu::Device, queue: &wgpu::Queue, persist: bool, sabotage: bool, oit: bool) -> Vec<Vec<u8>> {
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
    // Camera at the origin toward -z, reversed depth (nearer = larger), no fog.
    let mut cam = [0f32; 64];
    // Reversed infinite perspective projection: depth = near / distance.
    let (f, near) = (1.0 / 0.6f32.tan(), 0.05);
    cam[..16].copy_from_slice(&[f, 0.0, 0.0, 0.0, 0.0, f, 0.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, near, 0.0]);
    cam[36..40].copy_from_slice(&[1e9, 2e9, 1e9, 2e9]);
    let cam_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(&cam),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let lightmap = texture(device, queue, 16, 1, |_, x, y| [(x * 16) as u8, (y * 16) as u8, 160, 255]);
    let glyph_tex =
        texture(
            device,
            queue,
            16,
            2,
            |l, x, y| if (x + y + l) % 3 == 0 { [0; 4] } else { [255, (x * 13) as u8, 200, 255] },
        );
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
    let color =
        target(wgpu::TextureFormat::Rgba8Unorm, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
    let depth = target(
        wgpu::TextureFormat::Depth32Float,
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
    );
    let (color_view, depth_view) = (color.create_view(&Default::default()), depth.create_view(&Default::default()));
    let mut tags = NameTags::with_mode(device, &cam_layout, wgpu::TextureFormat::Rgba8Unorm, persist);
    tags.lists.store.sabotage = sabotage;
    tags.oit = oit;
    let mut chain = oit.then(|| crate::oit::Oit::new(device, wgpu::TextureFormat::Rgba8Unorm));
    if let Some(chain) = &mut chain {
        tags.ensure_oit(device, &chain.layouts);
        chain.ensure(device, (SIZE, SIZE), &depth_view);
        let proj: [f32; 16] = cam[..16].try_into().expect("16 coefficients");
        chain.set_projection(queue, glam::Mat4::from_cols_array(&proj));
    }
    let mut up = Uploader::new(device, true);
    let mut images = Vec::new();
    for frame in 0..4u32 {
        tags.begin(Vec3::X, Vec3::Y);
        for id in 0..40 {
            let name = name_of(id * 5 + 3, if frame >= 2 { 23 } else { 0 });
            let discrete = id % 9 == 0;
            let z = if oit { -4.0 - id as f32 * 0.02 } else { -4.0 - (id % 3) as f32 * 0.3 };
            let anchor = Vec3::new((id % 8) as f32 * 0.9 - 3.2, (id / 8) as f32 * 0.5 - 1.2, z);
            let light = ((id % 16) as u8, (15 - id % 16) as u8);
            tags.lists
                .push_with(id, NameRef::Text(&name), anchor, discrete, light, |c| fake(&name, c, discrete, frame));
        }
        tags.upload(device, queue, &mut up, 0, || {
            glyph_tex.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            })
        });
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.1, g: 0.2, b: 0.3, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_bind_group(0, &cam_bind, &[]);
            if oit {
                tags.draw_solid(&mut pass);
            } else {
                tags.draw_normal(&mut pass);
                tags.draw_see_through(&mut pass);
            }
        }
        if let Some(chain) = &chain {
            use crate::oit::Stage;
            for stage in Stage::ALL {
                {
                    let mut pass = chain.begin(&mut enc, stage, &depth_view, true, "stages", None);
                    pass.set_bind_group(0, &cam_bind, &[]);
                    tags.draw_oit(&mut pass, chain.frame(stage));
                }
                if stage == Stage::Bounds {
                    chain.cull(&mut enc, &depth_view, None);
                }
            }
            chain.composite(&mut enc, &color_view, &depth_view, None);
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_bind_group(0, &cam_bind, &[]);
            tags.draw_see_through(&mut pass);
        }
        let out = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(SIZE * SIZE * 4),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        enc.copy_texture_to_buffer(
            color.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &out,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(SIZE * 4), rows_per_image: None },
            },
            wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
        );
        up.submit(queue, enc.finish());
        out.slice(..).map_async(wgpu::MapMode::Read, |r| r.expect("read"));
        device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("GPU wait");
        images.push(out.slice(..).get_mapped_range().expect("mapping").to_vec());
    }
    images
}

#[test]
fn persistent_glyphs_capture_identically_on_gpu() {
    let Some((device, queue)) = gpu() else { return };
    for oit in [false, true] {
        let old = capture(&device, &queue, false, false, oit);
        let new = capture(&device, &queue, true, false, oit);
        for (i, (a, b)) in old.iter().zip(&new).enumerate() {
            let lit = a.as_chunks::<4>().0.iter().filter(|p| p[..] != a[..4]).count();
            assert!(lit > 3000, "oit={oit} frame {i}: tags missing ({lit} pixels)");
            let diff = a.iter().zip(b).filter(|(x, y)| x != y).count();
            assert_eq!(diff, 0, "oit={oit} frame {i}: {diff} bytes differ");
        }
        let bad = capture(&device, &queue, true, true, oit);
        assert!(old != bad, "oit={oit}: the sabotage must be seen");
    }
}
