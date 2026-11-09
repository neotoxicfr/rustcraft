//! Proofs of the text display paths: a deterministic script of 240 frames (50 displays and 6 map
//! names: texts changed, shadow, see-through, default background, held map names, opacity and
//! background interpolated, obfuscated text redone on every frame, improved transparency
//! toggled) played by the old path and by the glyph pool; the glyphs drawn by each pass, rebuilt
//! from copies of the GPU buffers that receive only the uploads (opacity and background applied
//! like the shader), are identical (exact order, as a set for the opaque and translucent parts
//! of improved transparency, whose order was already arbitrary). Then captures on the real GPU,
//! with and without improved transparency. Every proof is checked against a sabotage.
use super::*;
use crate::glyph_pool::tests::{
    PoolMirror, SIZE, Target, apply_buffer, glyph_sheet, glyph_sheet_view, gpu, lit_pixels, test_projection,
};
use rc_proto::nbt::Nbt;

fn g(x: f32, color: u32) -> Glyph {
    Glyph { rect: [x, x + 5.0, 0.0, 8.0], skew: 0.0, uv: [0.0; 4], color, mode: 1 }
}

#[test]
fn shadow_is_what_the_plain_pass_lacks() {
    let main = [g(0.0, 0xffff_ffff), g(6.0, 0xffff_ffff)];
    let all = vec![g(1.0, 0xff3f_3f3f), main[0], g(7.0, 0xff3f_3f3f), main[1]];
    let shadow = split_shadow(all, &main);
    assert_eq!(shadow.len(), 2);
    assert_eq!(shadow[0].rect[0], 1.0);
    assert_eq!(shadow[1].rect[0], 7.0);
}

#[test]
fn shader_validates() {
    let m = wgpu::naga::front::wgsl::parse_str(include_str!("../text_display.wgsl")).expect("parsing");
    wgpu::naga::valid::Validator::new(
        wgpu::naga::valid::ValidationFlags::all(),
        wgpu::naga::valid::Capabilities::all(),
    )
    .validate(&m)
    .expect("valid shader");
}

#[test]
fn alignment_flags() {
    assert_eq!(align(0, 40, 10), 15.0);
    assert_eq!(align(8, 40, 10), 0.0);
    assert_eq!(align(16, 40, 10), 30.0);
    // Half-pixel centering (odd widths).
    assert_eq!(align(0, 41, 10), 15.5);
}

#[test]
fn opacity_scales_glyph_alpha_and_background_is_argb() {
    assert_eq!(with_opacity(g(0.0, 0xffff_ffff), 0x40).color, 0x40ff_ffff);
    assert_eq!(rgba(0x4000_0000), 0x4000_0000);
    assert_eq!(rgba(0x80ff_0000), 0x8000_00ff);
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

/// Dummy layout of text `seed`: up to 3 lines of 5 × 8 glyphs (some translucent, italic ones,
/// pages 0 and 1), shadow one pixel lower right when `flags & 1`; seeds multiple of 13 are
/// obfuscated (glyphs change with `frame`).
fn fake(seed: u32, flags: i8, frame: u32) -> (DisplayLayout, bool) {
    let obfuscated = seed.is_multiple_of(13);
    let salt = if obfuscated { frame } else { 0 };
    let (mut shadow, mut text) = (Vec::new(), Vec::new());
    let lines = 1 + seed % 3;
    let mut width = 0;
    for line in 0..lines {
        let n = 2 + (seed + line * 5) % 9;
        width = width.max(n * 6);
        for i in 0..n {
            let b = (seed * 7 + line * 3 + i + salt) % 23;
            let (x, y) = (i as f32 * 6.0, line as f32 * 10.0);
            let u = (b % 4) as f32 * 0.25;
            let alpha = if b.is_multiple_of(5) { 0x80 } else { 0xff };
            let glyph = Glyph {
                rect: [x, x + 5.0, y, y + 8.0],
                skew: if b < 4 { 1.0 } else { 0.0 },
                uv: [u, 0.0, u + 0.25, 1.0],
                color: alpha << 24 | (seed.wrapping_mul(0x2545_f491) & 0xff_ffff),
                mode: 1 + b % 2,
            };
            if flags & 1 != 0 {
                let mut s = Glyph { color: alpha << 24 | 0x3f3f3f, ..glyph };
                s.rect = [x + 1.0, x + 6.0, y + 1.0, y + 9.0];
                shadow.push(s);
            }
            text.push(glyph);
        }
    }
    let key = (Some(Nbt::String(format!("text {seed}"))), 200, flags);
    (DisplayLayout::new(key, shadow, text, width as f32, lines as f32 * 10.0 - 1.0), obfuscated)
}

/// Displays of a frame: (draw, text seed), and map names (draw, label index).
fn frame_draws(frame: u32, rng: &mut Rng, still: bool) -> Vec<(TextDraw, u32)> {
    let mut out = Vec::new();
    for id in 0..56i32 {
        if !still && rng.below(10) == 0 {
            continue;
        }
        let f = if still { 0 } else { frame };
        let epoch = if id % 4 == 0 { f / 40 } else { 0 };
        let flags = ((id % 2) | if id % 5 == 1 { 2 } else { 0 } | if id % 7 == 2 { 4 } else { 0 }) as i8;
        // A sixth turned away from the camera (at the origin), slowly turning.
        let away = if id % 6 == 5 { std::f32::consts::PI } else { 0.0 };
        let m = Mat4::from_translation(Vec3::new(
            (id % 8) as f32 - 4.0,
            (id / 8) as f32 * 0.7 - 2.0,
            -6.0 - id as f32 * 0.05,
        )) * Mat4::from_rotation_y(away + (id as f32 * 0.3 + f as f32 * 0.01 * (id % 3) as f32).sin() * 0.4)
            * Mat4::from_scale(Vec3::new(0.025, -0.025, 0.025));
        let opacity = if id % 3 == 0 { ((f * 7 + id as u32 * 13) % 256) as u8 } else { 255 };
        let background = match id % 4 {
            0 => 0,
            1 => 0x4000_0000,
            2 => 0xff11_2233,
            _ => ((f * 3 + id as u32) % 256) << 24 | 0x0055_6677,
        };
        let label = if id >= 50 { (id - 50) as u32 } else { u32::MAX };
        let hand = id % 11 == 3;
        let light = packed_light(id as u32 % 16, f % 16);
        let d = TextDraw {
            id,
            m,
            light,
            opacity,
            background,
            flags: if label != u32::MAX { 0 } else { flags },
            label,
            hand,
        };
        out.push((d, id as u32 * 3 + epoch * 1000 + 1));
    }
    out
}

/// Light of an entity: block and sky levels (0 to 15).
fn packed_light(block: u32, sky: u32) -> u32 {
    (block * 16) | ((sky * 16) << 8)
}

/// One drawn glyph: layer matrix, light, glyph, as bytes.
type Drawn = Vec<u8>;

fn drawn(matrix: &[[f32; 4]; 4], light: u32, glyph: &Glyph) -> Drawn {
    let mut v = bytemuck::bytes_of(matrix).to_vec();
    v.extend_from_slice(&light.to_le_bytes());
    v.extend_from_slice(bytemuck::bytes_of(glyph));
    v
}

fn sorted(mut v: Vec<Drawn>) -> Vec<Drawn> {
    v.sort();
    v
}

/// Glyphs drawn by each pass of the old path.
fn old_passes(l: &DisplayLists, oit: bool) -> Vec<Vec<Drawn>> {
    let o = &l.out;
    let pass = |inst: &[TextInstance]| -> Vec<Drawn> {
        inst.iter().map(|t| drawn(&o.layers[t.face as usize], t.light, &t.glyph)).collect()
    };
    let offset = pass(&o.offset_inst);
    let rest = vec![pass(&o.see_inst), pass(&o.hand_inst)];
    let head = if oit {
        let solid = l.solid.min(l.counts.offset) as usize;
        vec![sorted(offset[..solid].to_vec()), sorted(offset[solid..].to_vec())]
    } else {
        vec![offset]
    };
    head.into_iter().chain(rest).collect()
}

/// Copies of the GPU buffers of the new path.
#[derive(Default)]
struct Mirror {
    pool: PoolMirror,
    layers: Vec<u8>,
    refs: Vec<u8>,
}

impl Mirror {
    fn apply(&mut self, l: &DisplayLists, send: Option<DisplayUpload>) {
        let Some(s) = send else { return };
        self.pool.apply(&l.pool, s.pool);
        apply_buffer(&mut self.layers, s.layers, bytemuck::cast_slice(&l.out.layers));
        apply_buffer(&mut self.refs, s.refs, bytemuck::cast_slice(&l.refs));
    }

    /// Glyphs drawn by each pass, colour and alpha computed like the shader.
    fn passes(&self, l: &DisplayLists, oit: bool) -> Vec<Vec<Drawn>> {
        let refs: &[LayerRef] = bytemuck::cast_slice(&self.refs[..l.refs.len() * size_of::<LayerRef>()]);
        let layers: &[[[f32; 4]; 4]] = bytemuck::cast_slice(&self.layers);
        let pass = |p: usize| {
            let mut out = Vec::new();
            for d in l.pass_draws(p) {
                let range = d.first as usize..(d.first + d.count) as usize;
                let planned = &l.refs[range.clone()];
                assert!(
                    planned.iter().all(|r| r.count > 0 && r.count <= d.glyphs_each),
                    "vertices per instance too short"
                );
                // An index out of the copies (after a sabotage) reads as an empty entry.
                for r in &refs[range] {
                    for i in r.first..r.first + r.count.min(d.glyphs_each) {
                        let (Some(layer), Some(stored)) = (layers.get(r.layer as usize), self.pool.0.get(i as usize))
                        else {
                            out.push(Vec::new());
                            continue;
                        };
                        let mut glyph = stored.glyph();
                        let c = if r.fill != 0 { r.fill } else { glyph.color };
                        glyph.color = (c & 0xff_ffff) | ((c >> 24) * r.opacity / 255) << 24;
                        out.push(drawn(layer, r.light, &glyph));
                    }
                }
            }
            out
        };
        if oit {
            vec![sorted(pass(SOLID)), sorted(pass(REST)), pass(SEE), pass(HAND)]
        } else {
            vec![pass(OFFSET), pass(SEE), pass(HAND)]
        }
    }
}

const FRAMES: u32 = 240;

/// Map names of the script.
fn label_texts() -> Vec<Nbt> {
    (0..6).map(|i| Nbt::String(format!("map name {i}"))).collect()
}

/// Plays the script with `l`; glyphs drawn by each pass of each frame.
fn play(mut l: DisplayLists) -> Vec<Vec<Vec<Drawn>>> {
    let labels = label_texts();
    let mut mirror = Mirror::default();
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let mut out = Vec::new();
    for frame in 0..FRAMES {
        let oit = (frame / 30) % 2 == 1;
        // Still between frames 100 and 140 (nothing changes: unchanged writes are skipped).
        let still = (100..140).contains(&frame);
        l.begin();
        l.out.back_faces = l.skip_back_faces.then_some(BackFaceTest { eye: Vec3::ZERO, pixels_per_radian: 643.0 });
        for (d, seed) in frame_draws(frame, &mut rng, still) {
            let bg = 0x3f00_0000 | ((frame % 3) * 0x10);
            if d.label != u32::MAX {
                let text = &labels[d.label as usize];
                l.push_label(&d, text, bg, || fake(seed, 0, frame));
            } else {
                let (layout, obfuscated) = fake(seed, d.flags, frame);
                let key = layout.key.clone();
                l.push_entity(&d, (&key.0, key.1, key.2), bg, || (layout, obfuscated));
            }
        }
        let send = l.finish(oit);
        if l.use_pool {
            mirror.apply(&l, send);
            out.push(mirror.passes(&l, oit));
        } else {
            out.push(old_passes(&l, oit));
        }
    }
    out
}

#[test]
fn glyph_pool_draws_the_same_display_text() {
    // Bytes per layer and pass sent each frame, against 52 per glyph on the old path.
    assert_eq!((size_of::<LayerRef>(), size_of::<TextInstance>()), (24, 52));
    let mut drawn = Vec::new();
    for skip in [false, true] {
        let old = play(DisplayLists::new(false, skip, false));
        let new = play(DisplayLists::new(true, skip, true));
        let glyphs: usize = old.iter().map(|f| f.iter().map(Vec::len).sum::<usize>()).sum();
        assert!(glyphs > 100_000, "script too poor: {glyphs} glyphs");
        for (i, (a, b)) in old.iter().zip(&new).enumerate() {
            assert!(a == b, "skip={skip} frame {i}: different passes");
        }
        drawn.push(glyphs);
    }
    // The displays turned away are left out.
    assert!(drawn[1] * 10 < drawn[0] * 9, "back faces left out: {} of {} glyphs drawn", drawn[1], drawn[0]);
    let old = play(DisplayLists::new(false, false, false));
    // Sabotage: additions to the pool never uploaded.
    let mut bad = DisplayLists::new(true, false, true);
    bad.pool.sabotage = true;
    assert!(play(bad) != old, "the pool sabotage must be seen");
    // Sabotage: unchanged-write test that only compares lengths.
    let mut stale = DisplayLists::new(true, false, true);
    stale.layer_buffer.sabotage = true;
    stale.ref_buffer.sabotage = true;
    assert!(play(stale) != old, "the skipped-write sabotage must be seen");
}

/// CPU micro-bench (1 core, loaded machine): 50 kept displays (shadow, background, a third
/// fading), old and new path; time of prepare + end of frame, bytes sent per frame.
/// `cargo test --release -p rc-app text_display::tests::bench -- --ignored --nocapture`
#[test]
#[ignore]
fn bench() {
    for (pool, unchanged) in [(false, false), (true, false), (true, true)] {
        let mut l = DisplayLists::new(pool, false, unchanged);
        let mut rng = Rng(1);
        let mut times = Vec::new();
        let mut sent = 0usize;
        // Seeds kept off the multiples of 13: no obfuscated text in the bench.
        let draws: Vec<(TextDraw, u32, LayoutKey)> = frame_draws(0, &mut rng, true)
            .into_iter()
            .filter(|(d, _)| d.label == u32::MAX)
            .map(|(d, seed)| (d, seed * 13 + 1, (Some(Nbt::String(format!("text {}", seed * 13 + 1))), 200, d.flags)))
            .collect();
        for frame in 0..2000u32 {
            let t = std::time::Instant::now();
            l.begin();
            for (d, seed, key) in &draws {
                l.push_entity(d, (&key.0, key.1, key.2), 0x3f00_0000, || fake(*seed, d.flags, frame));
            }
            let send = l.finish(false);
            times.push(t.elapsed().as_secs_f64() * 1e6);
            if frame >= 1000 {
                let o = &l.out;
                sent += match &send {
                    Some(s) => {
                        let bytes = |p: &BufferPlan, n: usize| if p.write { n } else { 0 };
                        (l.pool.glyphs.len() - s.pool.from) * size_of::<crate::glyph_pool::PooledGlyph>()
                            + bytes(&s.layers, o.layers.len() * 64)
                            + bytes(&s.refs, l.refs.len() * size_of::<LayerRef>())
                    }
                    None => {
                        (o.offset_inst.len() + o.see_inst.len() + o.hand_inst.len()) * size_of::<TextInstance>()
                            + o.layers.len() * 64
                    }
                };
            }
        }
        times.sort_by(f64::total_cmp);
        eprintln!(
            "pool={pool} skip_unchanged={unchanged}: median {:.1} µs, p90 {:.1} µs per frame; {} bytes sent per frame",
            times[1000],
            times[1800],
            sent / 1000
        );
    }
}

// --- Captures on the real GPU -------------------------------------------------------------------

/// Three frames of 40 displays (opacity and background changing, text changed at the third),
/// polygon offset, see-through and held map passes; `oit`: full improved-transparency chain
/// then see-through and held names, each display at its own depth.
fn capture(device: &wgpu::Device, queue: &wgpu::Queue, lists: DisplayLists, oit: bool) -> Vec<Vec<u8>> {
    let target = Target::new(device, queue, test_projection(), Vec3::ZERO);
    let sheet = glyph_sheet(device, queue);
    let mut displays = TextDisplays::with_mode(device, &target.cam_layout, wgpu::TextureFormat::Rgba8Unorm, lists);
    displays.oit = oit;
    let mut up = Uploader::new(device, true);
    let mut chain = oit.then(|| crate::oit::Oit::new(device, wgpu::TextureFormat::Rgba8Unorm));
    if let Some(chain) = &mut chain {
        displays.ensure_oit(device, &chain.layouts);
        chain.ensure(device, (SIZE, SIZE), &target.depth_view);
        chain.set_projection(&mut up, queue, test_projection());
    }
    let mut images = Vec::new();
    for frame in 0..3u32 {
        displays.lists.begin();
        let view = BackFaceTest { eye: Vec3::ZERO, pixels_per_radian: SIZE as f32 / (2.0 * 0.6f32.tan()) };
        displays.lists.out.back_faces = displays.lists.skip_back_faces.then_some(view);
        for id in 0..40i32 {
            let flags = ((id % 2) | if id % 9 == 4 { 2 } else { 0 } | if id % 7 == 2 { 4 } else { 0 }) as i8;
            let z = -4.0 - id as f32 * 0.07;
            // Some turned away, some seen edge-on (left to the GPU), the others facing.
            let turn = match id % 5 {
                0 => std::f32::consts::PI,
                1 => 1.45 + frame as f32 * 0.05,
                _ => 0.0,
            };
            let m = Mat4::from_translation(Vec3::new((id % 8) as f32 * 0.9 - 3.4, (id / 8) as f32 * 0.55 - 1.3, z))
                * Mat4::from_rotation_y(turn)
                * Mat4::from_scale(Vec3::new(0.025, -0.025, 0.025));
            let d = TextDraw {
                id,
                m,
                light: packed_light(id as u32 % 16, 15 - id as u32 % 16),
                opacity: if id % 3 == 0 { (60 + frame * 70 + id as u32) as u8 } else { 255 },
                background: match id % 4 {
                    0 => 0,
                    1 => 0x6000_0000 | (frame * 0x20),
                    2 => 0xff20_4060,
                    _ => (0x30 + frame * 0x30) << 24 | 0x0050_3010,
                },
                flags,
                label: u32::MAX,
                hand: id % 11 == 3,
            };
            let seed = id as u32 * 3 + if frame == 2 && id % 4 == 0 { 1000 } else { 0 } + 2;
            let (layout, obfuscated) = fake(seed, flags, frame);
            let key = layout.key.clone();
            displays.lists.push_entity(&d, (&key.0, key.1, key.2), 0x4000_0000, || (layout, obfuscated));
        }
        displays.upload(device, queue, &mut up, 0, || glyph_sheet_view(&sheet));
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = target.pass(&mut enc, true);
            if oit {
                displays.draw_solid(&mut pass);
            } else {
                displays.draw_offset(&mut pass);
                displays.draw_see_through(&mut pass);
                displays.draw_hand(&mut pass);
            }
        }
        if let Some(chain) = &chain {
            use crate::oit::Stage;
            for stage in Stage::ALL {
                {
                    let mut pass = chain.begin(&mut enc, stage, &target.depth_view, true, "stages", None);
                    pass.set_bind_group(0, &target.cam_bind, &[]);
                    displays.draw_oit(&mut pass, chain.frame(stage));
                }
                if stage == Stage::Bounds {
                    chain.cull(&mut enc, &target.depth_view, None);
                }
            }
            chain.composite(&mut enc, &target.color_view, &target.depth_view, None);
            let mut pass = target.pass(&mut enc, false);
            displays.draw_see_through(&mut pass);
            displays.draw_hand(&mut pass);
        }
        images.push(target.read(device, queue, &mut up, enc));
    }
    images
}

/// Bytes that differ between two images, and the largest difference of a byte.
fn difference(a: &[u8], b: &[u8]) -> (usize, u8) {
    let diff = a.iter().zip(b).filter(|(x, y)| x != y).count();
    (diff, a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0))
}

/// Normal mode: identical to the bit. Improved transparency: the translucent stages add up the
/// fragments of a pixel in drawing order, and floating-point addition of three fragments or more
/// (background, shadow, text) depends on that order; the old order of the translucent glyphs was
/// the arbitrary output of an unstable sort, the new one is the layout's. Same glyphs (proved on
/// the CPU), at most one level on a few bytes.
#[test]
fn display_text_captures_identically_on_gpu() {
    let Some((device, queue)) = gpu() else { return };
    let beyond_rounding = |old: &[Vec<u8>], other: &[Vec<u8>]| {
        old.iter().zip(other).any(|(a, b)| {
            let (diff, worst) = difference(a, b);
            worst > 1 || diff * 1000 >= a.len()
        })
    };
    for oit in [false, true] {
        let old = capture(&device, &queue, DisplayLists::new(false, false, false), oit);
        for (pool, skip) in [(true, false), (true, true), (false, true)] {
            let new = capture(&device, &queue, DisplayLists::new(pool, skip, true), oit);
            for (i, (a, b)) in old.iter().zip(&new).enumerate() {
                assert!(lit_pixels(a) > 3000, "oit={oit} frame {i}: displays missing ({} pixels)", lit_pixels(a));
                let (diff, worst) = difference(a, b);
                let case = format!("oit={oit} pool={pool} skip={skip} frame {i}: {diff} bytes differ, by {worst}");
                if oit {
                    assert!(worst <= 1 && diff * 1000 < a.len(), "{case}");
                } else {
                    assert_eq!(diff, 0, "{case}");
                }
            }
        }
        let mut bad = DisplayLists::new(true, false, true);
        bad.pool.sabotage = true;
        let sabotaged = capture(&device, &queue, bad, oit);
        assert!(beyond_rounding(&old, &sabotaged), "oit={oit}: the pool sabotage must be seen beyond rounding");
        let mut reversed = DisplayLists::new(true, true, true);
        reversed.out.reversed_backface_test = true;
        let sabotaged = capture(&device, &queue, reversed, oit);
        assert!(beyond_rounding(&old, &sabotaged), "oit={oit}: the reversed back-face test must be seen");
    }
}
