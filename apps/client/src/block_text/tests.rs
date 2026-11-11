//! Proofs of the sign text paths: a deterministic script of 240 frames (120 signs, both faces,
//! texts changed, glowing outlines toggled by distance, faces appearing and disappearing, light
//! changing, camera circling the signs and crossing block origins) played by the old path and
//! by the glyph pool; the glyphs drawn by each pass, rebuilt from copies of the GPU buffers that
//! receive only the uploads, are identical, in order. Then captures on the real GPU: old and new
//! paths, back faces left out or not, from in front, behind, at an angle and grazing, are
//! identical to the bit; the GPU itself draws nothing of a face seen from behind. Every proof
//! is checked against a sabotage.
use super::*;
use crate::glyph_pool::PooledGlyph;
use crate::glyph_pool::tests::{
    PoolMirror, SIZE, Target, apply_buffer, glyph_sheet, glyph_sheet_view, gpu, lit_pixels,
};
use glam::Vec3;

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

/// Dummy face layout: up to 4 centred lines of 5 × 8 glyphs (line `i` at `10·i − 20`), italic
/// capitals, pages 0 and 1; glowing: 8 outline copies one pixel away. `seed` 0 = empty face.
fn fake_layout(seed: u32, glowing: bool) -> (Vec<Glyph>, Vec<Glyph>) {
    let (mut text, mut outline) = (Vec::new(), Vec::new());
    if seed == 0 {
        return (text, outline);
    }
    for line in 0..1 + seed % 4 {
        let n = 3 + (seed + line * 7) % 12;
        let x0 = -(n as f32 * 6.0) / 2.0;
        let y = line as f32 * 10.0 - 20.0;
        for i in 0..n {
            let b = (seed * 31 + line * 5 + i) % 26;
            let x = x0 + i as f32 * 6.0;
            let u = (b % 4) as f32 * 0.25;
            let g = Glyph {
                rect: [x, x + 5.0, y, y + 8.0],
                skew: if b < 6 { 1.0 } else { 0.0 },
                uv: [u, 0.0, u + 0.25, 1.0],
                color: 0xff00_0000 | (seed.wrapping_mul(0x9e37_79b9) & 0xff_ffff),
                mode: 1 + b % 2,
            };
            text.push(g);
            if glowing {
                for (ox, oy) in [(-1.0, -1.0), (0.0, -1.0), (1.0, -1.0), (-1.0, 0.0), (1.0, 0.0), (-1.0, 1.0)] {
                    let mut o = Glyph { color: 0xff30_2010, ..g };
                    o.rect[0] += ox;
                    o.rect[1] += ox;
                    o.rect[2] += oy;
                    o.rect[3] += oy;
                    outline.push(o);
                }
            }
        }
    }
    (text, outline)
}

/// Faces of a standing sign at block `pos` (relative to the camera origin) turned by `angle`
/// degrees, as the block-entity renderer makes them.
fn sign_faces(pos: Vec3, angle: f32) -> [Mat4; 2] {
    let face = |back: bool| {
        let flip = if back { Mat4::from_rotation_y(std::f32::consts::PI) } else { Mat4::IDENTITY };
        Mat4::from_translation(pos + Vec3::splat(0.5))
            * Mat4::from_rotation_y((-angle).to_radians())
            * flip
            * Mat4::from_translation(Vec3::new(0.0, 1.0 / 3.0, 0.046_666_667))
            * Mat4::from_scale(Vec3::new(1.0 / 96.0, -1.0 / 96.0, 1.0 / 96.0))
    };
    [face(false), face(true)]
}

/// Light of a block entity: block and sky levels (0 to 15).
fn packed_light(block: u32, sky: u32) -> u32 {
    (block * 16) | ((sky * 16) << 8)
}

/// One drawn glyph: matrix, light, glyph, as bytes (identical between the paths).
type Drawn = Vec<u8>;

fn drawn(matrix: &[[f32; 4]; 4], light: u32, glyph: &Glyph) -> Drawn {
    let mut v = bytemuck::bytes_of(matrix).to_vec();
    v.extend_from_slice(&light.to_le_bytes());
    v.extend_from_slice(bytemuck::bytes_of(glyph));
    v
}

/// Glyphs drawn by the old path: outline pass, text pass.
fn old_passes(l: &SignLists) -> [Vec<Drawn>; 2] {
    let pass = |inst: &[TextInstance]| -> Vec<Drawn> {
        inst.iter().map(|t| drawn(&l.faces[t.face as usize], t.light, &t.glyph)).collect()
    };
    [pass(&l.outline_inst), pass(&l.text_inst)]
}

/// Copies of the GPU buffers of the new path, written only by the uploads.
#[derive(Default)]
struct Mirror {
    pool: PoolMirror,
    faces: Vec<u8>,
    refs: Vec<u8>,
}

impl Mirror {
    fn apply(&mut self, l: &SignLists, send: Option<SignUpload>) {
        let Some(s) = send else { return };
        self.pool.apply(&l.pool, s.pool);
        apply_buffer(&mut self.faces, s.faces, bytemuck::cast_slice(&l.faces));
        apply_buffer(&mut self.refs, s.refs, bytemuck::cast_slice(&l.refs));
    }

    /// Glyphs drawn by each pass, read from the copies (an index out of them, after a sabotage,
    /// reads as an empty entry); checks the vertices of each draw against its references.
    fn passes(&self, l: &SignLists) -> [Vec<Drawn>; 2] {
        let refs: &[FaceRef] = bytemuck::cast_slice(&self.refs[..l.refs.len() * size_of::<FaceRef>()]);
        let faces: &[[[f32; 4]; 4]] = bytemuck::cast_slice(&self.faces);
        std::array::from_fn(|p| {
            let mut out = Vec::new();
            for d in l.pass_draws(p) {
                let range = d.first as usize..(d.first + d.count) as usize;
                let planned = &l.refs[range.clone()];
                assert!(
                    planned.iter().all(|r| r.count > 0 && r.count <= d.glyphs_each),
                    "vertices per instance too short"
                );
                let useful: u32 = planned.iter().map(|r| r.count).sum();
                assert!(d.count == 1 || d.glyphs_each * d.count <= 2 * useful, "draw too wide");
                for r in &refs[range] {
                    for g in r.first..r.first + r.count.min(d.glyphs_each) {
                        let glyph = self.pool.0.get(g as usize).map(PooledGlyph::glyph);
                        match (faces.get(r.face as usize), glyph) {
                            (Some(face), Some(glyph)) => out.push(drawn(face, r.light, &glyph)),
                            _ => out.push(Vec::new()),
                        }
                    }
                }
            }
            out
        })
    }
}

const SIGNS: u32 = 120;
const FRAMES: u32 = 240;

/// Plays the script with `lists`; returns the glyphs drawn by each pass of each frame and the
/// number of faces sent.
fn play(mut l: SignLists) -> (Vec<[Vec<Drawn>; 2]>, usize) {
    let mut mirror = Mirror::default();
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut out = Vec::new();
    let mut faces_sent = 0;
    for frame in 0..FRAMES {
        // Camera circling the signs (radius 9 blocks), origin shifted every 60 frames; still
        // between frames 100 and 140 (nothing changes: unchanged writes are skipped).
        let t = if (100..140).contains(&frame) { 100.0 } else { frame as f32 };
        let origin_shift = Vec3::new((frame / 60) as f32, 0.0, 0.0);
        let a = t * 0.05;
        let eye = Vec3::new(a.cos() * 9.0, 1.5, a.sin() * 9.0) - origin_shift;
        let view = BackFaceTest { eye, pixels_per_radian: 643.0 };
        l.begin();
        for id in 0..SIGNS {
            if rng.below(9) == 0 && !(100..140).contains(&frame) {
                continue;
            }
            let pos = Vec3::new((id % 12) as f32 - 6.0, (id % 3) as f32, (id / 12) as f32 - 5.0) - origin_shift;
            let matrices = sign_faces(pos, (id * 37 % 360) as f32);
            let rev = if id % 7 == 0 { t as u32 / 50 } else { 0 };
            for side in 0..2u8 {
                let seed = if side == 1 && id % 3 == 0 { 0 } else { id * 2 + u32::from(side) + rev * 1000 + 1 };
                let glowing = (id + u32::from(side)) % 4 == 0;
                let light = packed_light(id % 16, t as u32 % 16);
                let face = FaceDraw {
                    key: ([id as i32, 0, 0], side),
                    rev,
                    matrix: matrices[usize::from(side)],
                    light: if glowing { FULL_BRIGHT } else { light },
                    outline: glowing && (id % 8 == 0 || (t as u32 + id) % 30 < 15),
                };
                l.push(&face, &view, || fake_layout(seed, glowing));
            }
        }
        faces_sent += l.faces.len();
        let send = l.finish();
        if l.use_pool {
            mirror.apply(&l, send);
            out.push(mirror.passes(&l));
        } else {
            out.push(old_passes(&l));
        }
    }
    (out, faces_sent)
}

#[test]
fn glyph_pool_draws_the_same_sign_text() {
    // Bytes per face and pass sent each frame, against 52 per glyph on the old path.
    assert_eq!((size_of::<FaceRef>(), size_of::<TextInstance>(), size_of::<PooledGlyph>()), (16, 52, 48));
    for skip in [false, true] {
        let (old, old_faces) = play(SignLists::new(false, skip, false));
        let (new, new_faces) = play(SignLists::new(true, skip, true));
        assert_eq!(old_faces, new_faces);
        let glyphs: usize = old.iter().map(|f| f[0].len() + f[1].len()).sum();
        assert!(glyphs > 200_000, "script too poor: {glyphs} glyphs");
        assert!(old.iter().any(|f| !f[0].is_empty()), "the script must draw outlines");
        for (i, (a, b)) in old.iter().zip(&new).enumerate() {
            assert!(a == b, "skip={skip} frame {i}: different passes");
        }
    }
    // Back faces left out: fewer faces sent (the camera goes round the signs).
    let (_, all) = play(SignLists::new(true, false, true));
    let (_, front) = play(SignLists::new(true, true, true));
    assert!(front * 10 < all * 8, "back faces left out: {front} of {all} faces sent");
    // Sabotage: additions to the pool never uploaded.
    let (old, _) = play(SignLists::new(false, false, false));
    let mut bad = SignLists::new(true, false, true);
    bad.pool.sabotage = true;
    assert!(play(bad).0 != old, "the pool sabotage must be seen");
    // Sabotage: unchanged-write test that only compares lengths.
    let mut stale = SignLists::new(true, false, true);
    stale.ref_buffer.sabotage = true;
    stale.face_buffer.sabotage = true;
    assert!(play(stale).0 != old, "the skipped-write sabotage must be seen");
}

/// CPU micro-bench (1 core, loaded machine): 100 kept signs (both faces written, a quarter
/// glowing) seen from the front, old and new path, back faces left out or not; time of
/// prepare + end of frame, and bytes sent per frame.
/// `cargo test --release -p rc-app block_text::tests::bench -- --ignored --nocapture`
#[test]
#[ignore]
fn bench() {
    for (pool, skip, unchanged) in [
        (false, false, false),
        (true, false, false),
        (true, false, true),
        (false, true, false),
        (true, true, false),
        (true, true, true),
    ] {
        let mut l = SignLists::new(pool, skip, unchanged);
        let view = BackFaceTest { eye: Vec3::new(0.0, 1.5, 12.0), pixels_per_radian: 643.0 };
        let mut times = Vec::new();
        let (mut sent, mut vertices) = (0usize, 0usize);
        for frame in 0..2000u32 {
            let t = std::time::Instant::now();
            l.begin();
            for id in 0..100u32 {
                let pos = Vec3::new((id % 10) as f32 - 5.0, (id / 10) as f32, 0.0);
                let matrices = sign_faces(pos, 0.0);
                for side in 0..2u8 {
                    let glowing = id % 4 == 0;
                    let face = FaceDraw {
                        key: ([id as i32, 0, 0], side),
                        rev: 0,
                        matrix: matrices[usize::from(side)],
                        light: 0x00f0,
                        outline: glowing,
                    };
                    l.push(&face, &view, || fake_layout(id * 2 + u32::from(side) + 1, glowing));
                }
            }
            let send = l.finish();
            times.push(t.elapsed().as_secs_f64() * 1e6);
            if frame >= 1000 {
                sent += match &send {
                    Some(s) => {
                        let bytes = |p: &BufferPlan, n: usize| if p.write { n } else { 0 };
                        (l.pool.glyphs.len() - s.pool.from) * size_of::<PooledGlyph>()
                            + bytes(&s.faces, l.faces.len() * 64)
                            + bytes(&s.refs, l.refs.len() * size_of::<FaceRef>())
                    }
                    None => (l.outline_inst.len() + l.text_inst.len()) * size_of::<TextInstance>() + l.faces.len() * 64,
                };
                vertices += if pool {
                    l.draws.iter().map(|d| 6 * (d.glyphs_each * d.count) as usize).sum::<usize>()
                } else {
                    6 * (l.outline_inst.len() + l.text_inst.len())
                };
            }
        }
        times.sort_by(f64::total_cmp);
        eprintln!(
            "pool={pool} backface_skip={skip} skip_unchanged={unchanged}: median {:.1} µs, p90 {:.1} µs per frame; {} bytes sent, {} vertices, {} faces per frame",
            times[1000],
            times[1800],
            sent / 1000,
            vertices / 1000,
            l.faces.len()
        );
    }
}

// --- Captures on the real GPU -------------------------------------------------------------------

/// Field of view of the captures (degrees).
const FOV: f32 = 70.0;

/// Two frames of 24 signs around the origin (all front faces written, every other back face,
/// a third glowing) seen from `eye` toward the origin; `front_only`: back faces empty.
fn capture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    lists: SignLists,
    eye: Vec3,
    front_only: bool,
) -> (Vec<Vec<u8>>, usize) {
    capture_with_shader(device, queue, lists, eye, front_only, SmallLayerOptions::NEW)
}

/// [`capture`] with the form of the vertex shader chosen.
fn capture_with_shader(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    lists: SignLists,
    eye: Vec3,
    front_only: bool,
    shader: SmallLayerOptions,
) -> (Vec<Vec<u8>>, usize) {
    let proj = crate::render::perspective(FOV, 1.0, 0.05, 1000.0);
    let view_m = glam::camera::rh::view::look_at_mat4(eye, Vec3::new(0.0, 0.5, 0.0), Vec3::Y);
    let target = Target::new(device, queue, proj * view_m, eye);
    let sheet = glyph_sheet(device, queue);
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut text = BlockText::with_mode(device, &target.cam_layout, format, lists, shader);
    let view = BackFaceTest { eye, pixels_per_radian: SIZE as f32 / (2.0 * (FOV.to_radians() / 2.0).tan()) };
    let mut up = Uploader::new(device, true);
    let (mut images, mut faces) = (Vec::new(), 0);
    for _frame in 0..2 {
        text.lists.begin();
        for id in 0..24u32 {
            let pos = Vec3::new((id % 6) as f32 * 1.2 - 3.5, (id / 6) as f32 * 0.6 - 1.0, (id % 3) as f32 * 0.4 - 0.4);
            let matrices = sign_faces(pos, (id * 23 % 90) as f32 - 45.0);
            for side in 0..2u8 {
                let written = side == 0 || (!front_only && id % 2 == 0);
                let seed = if written { id * 2 + u32::from(side) + 1 } else { 0 };
                let glowing = id % 3 == 0;
                let face = FaceDraw {
                    key: ([id as i32, 0, 0], side),
                    rev: 0,
                    matrix: matrices[usize::from(side)],
                    light: if glowing { FULL_BRIGHT } else { packed_light(id % 16, 15 - id % 16) },
                    outline: glowing,
                };
                text.lists.push(&face, &view, || fake_layout(seed, glowing));
            }
        }
        faces += text.lists.faces.len();
        text.upload(device, queue, &mut up, 0, || glyph_sheet_view(&sheet));
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = target.pass(&mut enc, true);
            text.draw(&mut pass);
        }
        images.push(target.read(device, queue, &mut up, enc));
    }
    (images, faces)
}

/// Eyes of the captures: in front, behind, at an angle from behind, grazing from behind and
/// from in front, close behind (within the slack), far behind.
fn eyes() -> [Vec3; 7] {
    [
        Vec3::new(0.3, 0.8, 6.0),
        Vec3::new(-0.2, 0.6, -6.0),
        Vec3::new(4.0, 1.0, -3.5),
        Vec3::new(7.0, 0.5, -0.6),
        Vec3::new(-7.0, 0.4, 0.5),
        Vec3::new(0.1, 0.5, -0.9),
        Vec3::new(1.0, 2.0, -25.0),
    ]
}

#[test]
fn sign_text_captures_identically_on_gpu() {
    let Some((device, queue)) = gpu() else { return };
    let mut skipped_somewhere = false;
    for (i, eye) in eyes().into_iter().enumerate() {
        let (old, all) = capture(&device, &queue, SignLists::new(false, false, false), eye, false);
        for lists in
            [SignLists::new(true, false, true), SignLists::new(true, true, true), SignLists::new(false, true, false)]
        {
            let (pool, skip) = (lists.use_pool, lists.skip_back_faces);
            let (new, sent) = capture(&device, &queue, lists, eye, false);
            skipped_somewhere |= sent < all;
            for (f, (a, b)) in old.iter().zip(&new).enumerate() {
                assert!(lit_pixels(a) > 20, "eye {i} frame {f}: text missing ({} pixels)", lit_pixels(a));
                let diff = a.iter().zip(b).filter(|(x, y)| x != y).count();
                assert_eq!(diff, 0, "eye {i} (pool={pool} skip={skip}) frame {f}: {diff} bytes differ");
            }
        }
        // Sabotage: back-face test reversed, the faces turned toward the camera vanish.
        let mut reversed = SignLists::new(true, true, true);
        reversed.reversed_backface_test = true;
        let (bad, _) = capture(&device, &queue, reversed, eye, false);
        assert!(bad != old, "eye {i}: the reversed test must be seen");
        // Corner table of the vertex shader as an array literal (the form before) or packed in
        // a constant: the same vertices, so the same image to the byte, on both instance paths.
        for pool in [false, true] {
            let lists = || SignLists::new(pool, pool, pool);
            let (array, _) = capture_with_shader(&device, &queue, lists(), eye, false, SmallLayerOptions::OLD);
            let (packed, _) = capture_with_shader(&device, &queue, lists(), eye, false, SmallLayerOptions::NEW);
            assert!(array == packed, "eye {i} pool={pool}: the packed corner table changes the image");
            assert!(array == old, "eye {i} pool={pool}: the array form differs from the first capture");
        }
    }
    assert!(skipped_somewhere, "some eye must leave back faces out");
    // The GPU culls the faces seen from behind: from straight behind, the front texts alone
    // draw nothing (what the back-face test leaves out is never drawn).
    let (behind, _) = capture(&device, &queue, SignLists::new(false, false, false), eyes()[1], true);
    assert_eq!(lit_pixels(&behind[0]), 0, "text drawn from behind");
    let (front, _) = capture(&device, &queue, SignLists::new(false, false, false), eyes()[0], true);
    assert!(lit_pixels(&front[0]) > 200, "front texts must show");
}

#[test]
fn shader_validates() {
    for options in [SmallLayerOptions::OLD, SmallLayerOptions::NEW] {
        let source = options.shader_source(include_str!("../block_text.wgsl"));
        let m = wgpu::naga::front::wgsl::parse_str(&source).expect("parsing");
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        )
        .validate(&m)
        .expect("valid shader");
    }
}
