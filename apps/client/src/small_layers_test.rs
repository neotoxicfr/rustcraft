//! Proofs of the small per-frame layers. On a real GPU, a scene with the targeted block outline,
//! cracks, a leash, beams with their halos and a portal, debug segments, points and faces, the
//! panorama and screen overlays is played for twelve frames (targets, stages, sizes and lists
//! changing or staying put, a frame with nothing) through the paths of before (queue writes, array
//! literals in the shaders, everything rebuilt and sent on every frame) and through each new path
//! alone then all together: the images are equal to the byte, and two sabotages are seen. The
//! writes of the new paths make no staging buffer once the ring has its chunk. Without a GPU:
//! both shader forms are valid, the packed one indexes no array, and its constants decode to the
//! tables of the array form.
//! Without a GPU the GPU tests skip themselves; under `RUSTCRAFT_OPT_REQUIRE=1` it is a failure.

use std::sync::Arc;

use glam::Vec3;
use rc_assets::BlockModels;
use rc_assets::texture::Image;
use rc_data::{Aabb, VersionData};

use super::*;
use crate::block_fx::{BeamVertex, BlockFx, FxFrame, PortalVertex};
use crate::crack::{CrackOptions, CrackedBlock, Cracks};
use crate::debug_render::{DebugLines, Layer};
use crate::glyph_pool::tests::{SIZE, Target, glyph_sheet_view, gpu, lit_pixels, test_projection, texture};
use crate::leash::{LeashRenderer, Rope};
use crate::outline::Outline;
use crate::overlays::{GuiOverlay, Overlays};
use crate::panorama::{Panorama, PanoramaAssets};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Shaders with a corner table, by name.
const SHADERS: [(&str, &str); 7] = [
    ("outline", include_str!("outline.wgsl")),
    ("debug lines", include_str!("debug_lines.wgsl")),
    ("debug text", include_str!("debug_text.wgsl")),
    ("name tags", include_str!("nametag.wgsl")),
    ("text displays", include_str!("text_display.wgsl")),
    ("sign text", include_str!("block_text.wgsl")),
    ("panorama", include_str!("panorama.wgsl")),
];

fn shader(name: &str, options: SmallLayerOptions) -> String {
    let (_, source) = SHADERS.iter().find(|(n, _)| *n == name).expect("known shader");
    options.shader_source(source)
}

/// Both forms of every shader are valid; the packed form indexes no array held by value in its
/// vertex stages, the array form does (the check sees what it looks for).
#[test]
fn both_shader_forms_are_valid_and_the_packed_one_indexes_no_array() {
    use crate::terrain_shader::tests::{validate, vertex_value_array_indexing};
    for (name, _) in SHADERS {
        let indexed_arrays = |options: SmallLayerOptions| {
            let (module, info) = validate(&shader(name, options));
            vertex_value_array_indexing(&module, &info, |_| true)
        };
        assert_eq!(indexed_arrays(SmallLayerOptions::NEW), 0, "{name}: array indexed in the packed form");
        assert!(indexed_arrays(SmallLayerOptions::OLD) >= 1, "{name}: the array form's table is not seen");
    }
}

/// Hexadecimal constants written in `source` between `marker` and the end of its item (the next
/// line that starts with a closing brace, or the end of the line for a `const`).
fn hex_constants(source: &str, marker: &str) -> Vec<u32> {
    let from = source.find(marker).unwrap_or_else(|| panic!("`{marker}` missing"));
    let item = &source[from..];
    let end = if marker.starts_with("const") { item.find('\n') } else { item.find("\n}") };
    let mut rest = &item[..end.unwrap_or(item.len())];
    let mut found = Vec::new();
    while let Some(at) = rest.find("0x") {
        let digits: String = rest[at + 2..].chars().take_while(char::is_ascii_hexdigit).collect();
        found.push(u32::from_str_radix(&digits, 16).expect("hexadecimal digits"));
        rest = &rest[at + 2 + digits.len()..];
    }
    found
}

/// Corner of each of the six vertices packed one nibble per vertex in `packed`.
fn corners_of(packed: u32) -> [u32; 6] {
    std::array::from_fn(|vertex| (packed >> (vertex * 4)) & 0xf)
}

/// The constants of the packed form, read in the shader sources, decode to the tables of the
/// array form (stored here): segment and quad corners, the 36 cube vertices of the panorama and
/// the six texture coordinates of its veil.
#[test]
fn packed_constants_decode_to_the_array_tables() {
    const SEGMENT: [u32; 6] = [0, 1, 2, 2, 1, 3];
    const QUAD: [u32; 6] = [0, 1, 2, 0, 2, 3];
    let packed = |name: &str, function: &str| {
        let constants = hex_constants(&shader(name, SmallLayerOptions::NEW), &format!("fn {function}("));
        corners_of(*constants.first().unwrap_or_else(|| panic!("{name}: no constant in {function}")))
    };
    assert_eq!(packed("outline", "segment_corner"), SEGMENT);
    assert_eq!(packed("debug lines", "segment_corner"), SEGMENT);
    assert_eq!(packed("debug lines", "face_corner"), QUAD);
    for name in ["debug text", "name tags", "text displays", "sign text"] {
        assert_eq!(packed(name, "glyph_corner"), QUAD, "{name}");
    }

    let panorama = shader("panorama", SmallLayerOptions::NEW);
    const CORNERS: [[f32; 3]; 8] = [
        [-1.0, -1.0, -1.0],
        [1.0, -1.0, -1.0],
        [1.0, 1.0, -1.0],
        [-1.0, 1.0, -1.0],
        [-1.0, -1.0, 1.0],
        [1.0, -1.0, 1.0],
        [1.0, 1.0, 1.0],
        [-1.0, 1.0, 1.0],
    ];
    #[rustfmt::skip]
    const INDICES: [usize; 36] = [
        4, 5, 6, 4, 6, 7,
        1, 0, 3, 1, 3, 2,
        5, 1, 2, 5, 2, 6,
        0, 4, 7, 0, 7, 3,
        7, 6, 2, 7, 2, 3,
        0, 1, 5, 0, 5, 4,
    ];
    let axis = |name: &str| hex_constants(&panorama, &format!("const CUBE_CORNER_{name}"))[0];
    let masks = [axis("X"), axis("Y"), axis("Z")];
    let vertex_corner = packed("panorama", "cube_vertex");
    for (i, index) in INDICES.into_iter().enumerate() {
        let bit = (i as u32 / 6) * 4 + vertex_corner[i % 6];
        let decoded = masks.map(|mask| ((mask >> bit) & 1) as f32 * 2.0 - 1.0);
        assert_eq!(decoded, CORNERS[index], "cube vertex {i}");
    }

    const VEIL: [[f32; 2]; 6] = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    // In the veil: the corner nibbles, their mask, then one bit mask per axis.
    let veil = hex_constants(&panorama, "fn veil_uv(");
    let [nibbles, 0xf, u_mask, v_mask] = veil[..] else { panic!("veil constants: {veil:x?}") };
    for (i, uv) in VEIL.into_iter().enumerate() {
        let corner = corners_of(nibbles)[i];
        assert_eq!([((u_mask >> corner) & 1) as f32, ((v_mask >> corner) & 1) as f32], uv, "veil vertex {i}");
    }
}

/// A buffer written on change holds the bytes last set, through the queue and through the ring,
/// skipping or not; skipping sends less; "unchanged" judged on the length alone (sabotage) leaves
/// stale bytes.
#[test]
fn buffer_written_on_change_holds_the_bytes_last_set() {
    use crate::render::upload::tests::{gpu, read};
    let Some((device, queue)) = gpu() else { return };
    let bytes =
        |seed: u8, len: usize| -> Vec<u8> { (0..len).map(|i| seed.wrapping_mul(31).wrapping_add(i as u8)).collect() };
    // Same bytes twice, other bytes of the same length, a growth, a prefix of what is held, back.
    let script = [bytes(1, 32), bytes(1, 32), bytes(2, 32), bytes(2, 32), bytes(3, 4096), bytes(3, 64), bytes(1, 32)];
    let play = |ring: bool, skip: bool, sabotage: bool| -> (bool, u32) {
        let mut buffer = BufferWrittenOnChange::vertices("test", 16, skip);
        buffer.sabotage_length_only = sabotage;
        let mut up = Uploader::new(&device, ring);
        let (mut right, mut sent) = (true, 0);
        for set in &script {
            sent += u32::from(buffer.set(&mut up, &queue, set));
            up.submit(&queue, device.create_command_encoder(&Default::default()).finish());
            let held = read(&device, &queue, buffer.buffer().expect("written"));
            right &= held[..set.len()] == set[..];
        }
        (right, sent)
    };
    for ring in [false, true] {
        assert_eq!(play(ring, false, false), (true, 7), "ring={ring}: every set sent");
        assert_eq!(play(ring, true, false), (true, 4), "ring={ring}: unchanged sets skipped");
        assert!(!play(ring, true, true).0, "ring={ring}: the sabotage must leave stale bytes");
    }
}

// --- Scene on the real GPU ----------------------------------------------------------------------

/// Paths of a capture.
#[derive(Debug, Clone, Copy)]
struct Paths {
    /// Transit ring of the uploader; off: the queue writes of before.
    ring: bool,
    layers: SmallLayerOptions,
    cracks: CrackOptions,
    skip_unchanged_outline_segments: bool,
}

const OLD: Paths = Paths {
    ring: false,
    layers: SmallLayerOptions::OLD,
    cracks: CrackOptions::OLD,
    skip_unchanged_outline_segments: false,
};
const NEW: Paths = Paths {
    ring: true,
    layers: SmallLayerOptions::NEW,
    cracks: CrackOptions::NEW,
    skip_unchanged_outline_segments: true,
};

/// Layers of the scene, in draw order.
const LAYERS: [&str; 7] =
    ["panorama", "beams and portal", "cracks", "outline", "leash", "debug overlays", "screen overlays"];
const EVERY_LAYER: [bool; 7] = [true; 7];
/// Frames of the script.
const FRAMES: u32 = 12;

fn image(side: u32, texel: impl Fn(u32, u32) -> [u8; 4]) -> Image {
    Image { width: side, height: side, rgba: (0..side * side).flat_map(|i| texel(i % side, i / side)).collect() }
}

/// The six vertices (two counter-clockwise triangles seen from +z) of a rectangle at depth `z`.
fn rectangle(x: f32, y: f32, w: f32, h: f32, z: f32) -> [[f32; 3]; 6] {
    let corner = [[x, y, z], [x + w, y, z], [x + w, y + h, z], [x, y + h, z]];
    [0, 1, 2, 0, 2, 3].map(|i| corner[i])
}

struct Scene {
    up: Uploader,
    outline: Outline,
    cracks: Cracks,
    leash: LeashRenderer,
    block_fx: BlockFx,
    debug: DebugLines,
    panorama: Panorama,
    overlays: Overlays,
    models: Arc<BlockModels>,
    data: VersionData,
    beams: FxFrame,
}

impl Scene {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue, target: &Target, paths: Paths) -> Self {
        let camera = &target.cam_layout;
        let models = crate::crack::tests::test_models();
        let mut cracks = Cracks::with_options(device, camera, FORMAT, paths.cracks);
        // One pattern per stage, dark enough to show under the multiplicative blend.
        let stages: Vec<Image> = (0..20u32)
            .map(|stage| {
                let period = 3 + stage % 4;
                image(16, |x, y| if (x + y * 3 + stage).is_multiple_of(period) { [40, 90, 40, 255] } else { [0; 4] })
            })
            .collect();
        cracks.load(queue, &stages);
        let mut block_fx = BlockFx::new(device, camera, FORMAT);
        let effect = |seed: u32| image(8, |x, y| [(x * 30 + seed) as u8, (y * 30) as u8, (seed * 40) as u8, 255]);
        let effects = [effect(1), effect(2), effect(3), effect(4)];
        block_fx.set_textures(device, queue, [&effects[0], &effects[1], &effects[2], &effects[3]]);
        let faces: Vec<Image> =
            (0..6u32).map(|f| image(4, |x, y| [(f * 40) as u8, (x * 60) as u8, (y * 60) as u8, 255])).collect();
        let veil = image(4, |x, y| [255, 255, 255, if (x + y).is_multiple_of(2) { 60 } else { 0 }]);
        let panorama_assets = PanoramaAssets { faces, overlay: Some(veil) };
        let block_textures =
            texture(device, queue, 16, 4, |layer, x, y| [(layer * 60) as u8, (x * 16) as u8, (y * 16) as u8, 200]);
        let block_view = glyph_sheet_view(&block_textures);
        let assets = Arc::new(rc_assets::Assets::new(Vec::new()));
        let data = Arc::new(VersionData::default());
        let views = (&block_view, &block_view);
        Scene {
            up: Uploader::new(device, paths.ring),
            outline: Outline::with_options(device, camera, FORMAT, paths.layers, paths.skip_unchanged_outline_segments),
            cracks,
            leash: LeashRenderer::new(device, camera, FORMAT),
            block_fx,
            debug: DebugLines::with_options(device, camera, FORMAT, paths.layers),
            panorama: Panorama::with_options(device, queue, FORMAT, &panorama_assets, paths.layers),
            overlays: Overlays::new(device, queue, FORMAT, views, assets, models.clone(), data),
            models,
            data: VersionData::default(),
            beams: FxFrame::default(),
        }
    }

    /// Lists of frame `n` (every frame number is valid: the script repeats after `FRAMES`).
    fn fill(&mut self, n: u32) {
        let n = n % FRAMES;
        self.fill_outline(n);
        self.fill_cracks(n);
        self.fill_leash(n);
        self.fill_beams(n);
        self.fill_debug(n);
        self.panorama.spin = n as f32 * 11.0;
    }

    /// The target changes every third frame (two frames out of three repeat the segments, a cube
    /// and a slab have as many); no target at frame 4; translucent block from frame 6, high
    /// contrast from frame 8.
    fn fill_outline(&mut self, n: u32) {
        let cube = [Aabb { min: [0.0; 3], max: [1.0; 3] }];
        let slab = [Aabb { min: [0.0; 3], max: [1.0, 0.5, 1.0] }];
        let target = n / 3;
        let (shape, boxes) = if target.is_multiple_of(2) { (0u16, &cube) } else { (1, &slab) };
        let corner = [-2.0 + target as f32 * 0.5, -1.0, -6.0];
        self.outline.high_contrast = n >= 8;
        self.outline.set((n != 4).then_some((shape, &boxes[..], corner, n >= 6)));
    }

    /// One to three blocks, the stage moving every other frame, none at frame 7; the camera
    /// origin moves alone at the last frame.
    fn fill_cracks(&mut self, n: u32) {
        let stage = (n / 2 % 10) as u8;
        let blocks = [
            CrackedBlock { pos: [-3, 0, -7], state: 0, stage },
            CrackedBlock { pos: [-1, 1, -6], state: 1, stage: 9 - stage },
            CrackedBlock { pos: [1, -1, -5], state: 2, stage: 3 },
        ];
        let shown = if n == 7 { 0 } else { 1 + (n as usize / 4) % 3 };
        let origin = [i32::from(n == FRAMES - 1), 0];
        self.cracks.set_blocks(&self.models, &self.data, &blocks[..shown], origin);
    }

    fn fill_leash(&mut self, n: u32) {
        self.leash.verts.clear();
        if n != 2 {
            let rope = Rope { start: [-2.5, 1.5, -5.0], end: [0.5 + f64::from(n / 2) * 0.3, 0.5, -6.0], sag: true };
            crate::leash::geometry(&rope, [(15, 15), (3, 12)], [0, 0], &mut self.leash.verts);
        }
    }

    /// Cores, halos and a portal that come and go, so that the list of cores or of halos is
    /// empty in some frames.
    fn fill_beams(&mut self, n: u32) {
        let beam = |color: u32, tex: u32| move |pos: [f32; 3]| BeamVertex { pos, uv: [pos[0], pos[1]], color, tex };
        self.beams.clear();
        if n % 4 != 3 {
            self.beams.beams.extend(rectangle(1.5, -1.5, 0.4, 3.0, -7.0).map(beam(0xff40_c0ff, 0)));
        }
        if n >= 2 {
            self.beams.halos.extend(rectangle(1.3, -1.5, 0.8, 3.0, -6.9).map(beam(0x50ff_ffff, 1)));
        }
        if n.is_multiple_of(2) {
            let portal = rectangle(-3.0, 1.2, 1.0, 1.0, -6.5);
            self.beams.portals.extend(portal.map(|pos| PortalVertex { pos, layers: 15 }));
        }
        self.beams.time = n as f32 * 0.37;
    }

    /// Opaque and translucent boxes, front lines, a crosshair point and filled faces; nothing at
    /// frame 9.
    fn fill_debug(&mut self, n: u32) {
        self.debug.clear();
        self.debug.clip = Some(([0.0; 3], [0.0, 0.0, -1.0]));
        if n == 9 {
            return;
        }
        let shift = (n / 2) as f32 * 0.2;
        self.debug.cuboid(Layer::Depth, [-0.5 + shift, -1.8, -4.5], [0.3 + shift, -1.0, -3.8], 0xffff_3030, 2.0);
        self.debug.cuboid(Layer::Depth, [0.6, -1.8, -4.5], [1.2, -1.2, -3.9], 0x80ff_ff30, 3.0);
        self.debug.line(Layer::Front, [-1.0, 1.8, -4.0], [1.0, 1.6 + shift, -4.0], 0xff30_ff30, 2.0);
        self.debug.line(Layer::Front, [-1.0, 1.4, -4.0], [1.0, 1.4, -4.0], 0x6030_30ff, 4.0);
        self.debug.point(Layer::Crosshair, [0.0, 0.0, -2.0], 0xffff_ffff, 6.0);
        self.debug.filled_cuboid([-2.2, -1.9, -4.6], [-1.6, -1.3, -4.0], 0x4030_c0c0);
    }

    /// Sends the lists of frame `n`, every layer.
    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, n: u32) {
        self.upload_layers(device, queue, n, EVERY_LAYER);
    }

    /// Sends the lists of frame `n` for the layers `sent` (in the order of `LAYERS`). The window
    /// is twice as wide in frames 4 to 6 (the line styles change, then come back).
    fn upload_layers(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, n: u32, sent: [bool; 7]) {
        let n = n % FRAMES;
        let window = if (4..7).contains(&n) { (2 * SIZE, SIZE) } else { (SIZE, SIZE) };
        let up = &mut self.up;
        if sent[0] {
            self.panorama.send_matrix(queue, up, window);
        }
        if sent[1] {
            self.block_fx.upload(queue, up, &self.beams);
        }
        if sent[2] {
            self.cracks.upload(queue, up);
        }
        if sent[3] {
            self.outline.upload(queue, up, window);
        }
        if sent[4] {
            self.leash.upload(queue, up);
        }
        if sent[5] {
            self.debug.upload_geometry(device, queue, up, window);
        }
        if sent[6] {
            // Fire every other frame, the portal veil from frame 5, the view inside a block (which
            // covers the screen) at frame 3.
            let overlays = crate::overlays::Frame {
                hud_fov: 70.0,
                block: (n == 3).then_some(0),
                water: None,
                fire: n % 2 == 1,
                gui: if n >= 5 { vec![GuiOverlay::Portal(0.25)] } else { Vec::new() },
            };
            self.overlays.prepare(queue, up, &overlays, window, 2, Some);
        }
    }

    /// Draws the layers `drawn` of the uploaded frame and returns the pixels.
    fn draw(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, target: &Target, drawn: [bool; 7]) -> Vec<u8> {
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = target.pass(&mut enc, true);
            if drawn[0] {
                // The panorama binds its own group 0: the camera is bound again after it.
                self.panorama.draw(&mut pass);
                pass.set_bind_group(0, &target.cam_bind, &[]);
            }
            if drawn[1] {
                self.block_fx.draw(&mut pass);
            }
            for late in [false, true] {
                if drawn[2] {
                    self.cracks.draw(&mut pass, late);
                }
                if drawn[3] {
                    self.outline.draw(&mut pass, late);
                }
                if drawn[4] && !late {
                    self.leash.draw(&mut pass);
                }
                if drawn[5] && !late {
                    self.debug.draw(&mut pass, Layer::Depth);
                }
            }
            if drawn[5] {
                self.debug.draw(&mut pass, Layer::Front);
                self.debug.draw(&mut pass, Layer::Crosshair);
            }
            if drawn[6] {
                self.overlays.draw_screen(&mut pass);
                self.overlays.draw_gui(&mut pass);
            }
        }
        target.read(device, queue, &mut self.up, enc)
    }

    /// Images of the whole script, every layer drawn.
    fn play(mut self, device: &wgpu::Device, queue: &wgpu::Queue, target: &Target) -> Vec<Vec<u8>> {
        (0..FRAMES)
            .map(|n| {
                self.fill(n);
                self.upload(device, queue, n);
                self.draw(device, queue, target, EVERY_LAYER)
            })
            .collect()
    }
}

/// Hardware device on one of `backends` (Vulkan, the default, and DirectX 12, the fallback, whose
/// shaders go through another compiler), or `None` (reported) on a machine without one.
fn gpu_on(backends: wgpu::Backends) -> Option<(wgpu::Device, wgpu::Queue)> {
    crate::render::upload::tests::gpu_serial();
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let options =
        wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, ..Default::default() };
    let adapter = pollster::block_on(instance.request_adapter(&options)).ok();
    // A software adapter (WARP, llvmpipe) is not the real GPU these proofs are about.
    let hardware = adapter.filter(|a| a.get_info().device_type != wgpu::DeviceType::Cpu);
    let adapter = rc_testdata::need(hardware, "hardware GPU adapter")?;
    eprintln!("GPU: {} ({:?})", adapter.get_info().name, adapter.get_info().backend);
    Some(pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("device"))
}

#[test]
fn small_layers_draw_the_same_image_on_every_path_on_vulkan() {
    same_image_on_every_path(wgpu::Backends::VULKAN);
}

#[cfg(windows)]
#[test]
fn small_layers_draw_the_same_image_on_every_path_on_dx12() {
    same_image_on_every_path(wgpu::Backends::DX12);
}

/// The scene drawn through every new path, alone and together, equals the paths of before to the
/// byte on every frame; each layer shows on its own; two sabotages are seen.
fn same_image_on_every_path(backends: wgpu::Backends) {
    let Some((device, queue)) = gpu_on(backends) else { return };
    let target = Target::new(&device, &queue, test_projection(), Vec3::ZERO);
    let play = |paths: Paths| Scene::new(&device, &queue, &target, paths).play(&device, &queue, &target);
    let want = play(OLD);

    // Each layer alone lights pixels (frame 0, and frame 1 for the fire of the screen overlays).
    for (layer, name) in LAYERS.iter().enumerate() {
        let mut scene = Scene::new(&device, &queue, &target, OLD);
        let frame = if layer == 6 { 1 } else { 0 };
        scene.fill(frame);
        scene.upload(&device, &queue, frame);
        let lit = lit_pixels(&scene.draw(&device, &queue, &target, std::array::from_fn(|l| l == layer)));
        eprintln!("{name}: {lit} pixels");
        assert!(lit > 150, "{name} does not show ({lit} pixels)");
    }
    // The script moves: no frame equals the one before.
    for (n, pair) in want.windows(2).enumerate() {
        assert!(pair[0] != pair[1], "frames {n} and {} are the same image", n + 1);
    }

    let paths = [
        ("transit ring", Paths { ring: true, ..OLD }),
        (
            "packed shader constants",
            Paths { layers: SmallLayerOptions { packed_shader_constants: true, ..OLD.layers }, ..OLD },
        ),
        (
            "unchanged uniforms skipped",
            Paths { layers: SmallLayerOptions { skip_unchanged_uniforms: true, ..OLD.layers }, ..OLD },
        ),
        ("unchanged outline segments skipped", Paths { skip_unchanged_outline_segments: true, ..OLD }),
        ("crack buffers kept", Paths { cracks: CrackOptions { buffer_reuse: true, ..OLD.cracks }, ..OLD }),
        ("crack rotation table", Paths { cracks: CrackOptions { rotation_table: true, ..OLD.cracks }, ..OLD }),
        ("crack vertices kept", Paths { cracks: CrackOptions { geometry_cache: true, ..OLD.cracks }, ..OLD }),
        ("all", NEW),
        ("all without the ring", Paths { ring: false, ..NEW }),
    ];
    for (name, paths) in paths {
        for (n, (old, new)) in want.iter().zip(play(paths)).enumerate() {
            let differing = old.iter().zip(&new).filter(|(a, b)| a != b).count();
            assert_eq!(differing, 0, "{name}: frame {n}: {differing} bytes differ");
        }
    }

    let sabotaged = |sabotage: fn(&mut Scene)| {
        let mut scene = Scene::new(&device, &queue, &target, NEW);
        sabotage(&mut scene);
        scene.play(&device, &queue, &target) != want
    };
    assert!(sabotaged(|s| s.cracks.geometry.sabotage_origin_ignored = true), "cracks kept without the camera origin");
    assert!(sabotaged(|s| s.outline.sabotage_segments_judged_on_length()), "outline segments judged on their number");
}

/// Once the ring has its chunk, the writes of the new paths make no staging buffer; the queue
/// made one per write. Counted over the script played five times after sixty frames.
#[test]
fn small_layers_make_no_staging_buffer_through_the_ring() {
    let Some((device, queue)) = gpu() else { return };
    let target = Target::new(&device, &queue, test_projection(), Vec3::ZERO);
    let count = |paths: Paths| {
        let mut scene = Scene::new(&device, &queue, &target, paths);
        let frame = |scene: &mut Scene, n: u32| {
            scene.fill(n);
            scene.upload(&device, &queue, n);
            scene.up.submit(&queue, device.create_command_encoder(&Default::default()).finish());
            // The frame is over before the next one, as when the client waits for its image.
            device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("GPU wait");
        };
        (0..60).for_each(|n| frame(&mut scene, n));
        scene.up.take_stats();
        (60..120).for_each(|n| frame(&mut scene, n));
        scene.up.take_stats()
    };
    let (old, new) = (count(OLD), count(NEW));
    eprintln!(
        "per frame, before: {:.1} writes, {:.1} staging buffers, {:.0} bytes",
        old.writes as f64 / 60.0,
        old.transits as f64 / 60.0,
        old.bytes as f64 / 60.0
    );
    eprintln!(
        "per frame, now: {:.1} writes, {:.1} staging buffers, {:.0} bytes",
        new.writes as f64 / 60.0,
        new.transits as f64 / 60.0,
        new.bytes as f64 / 60.0
    );
    assert!(old.writes > 0 && old.transits == old.writes, "the queue makes a staging buffer per write: {old:?}");
    assert_eq!(new.transits, 0, "staging buffers made through the ring: {new:?}");
    assert!(new.writes > 0 && new.writes < old.writes, "fewer writes expected: {} against {}", new.writes, old.writes);
}

/// Micro-bench on the GPU (machine loaded by other agents' builds: an order of magnitude only):
/// 400,000 debug segments of one pixel (2.4 million vertices, the vertex stage dominates) drawn
/// with the corner table as an array literal and packed in a constant, on Vulkan and DirectX 12;
/// submission to the end of the GPU work, best of 9 alternating rounds.
#[test]
#[ignore = "micro-bench: cargo test --release -p rc-app packed_shader_constants_gpu_bench -- --ignored --nocapture"]
fn packed_shader_constants_gpu_bench() {
    const SEGMENTS: u32 = 400_000;
    for backends in [wgpu::Backends::VULKAN, wgpu::Backends::DX12] {
        let Some((device, queue)) = gpu_on(backends) else { continue };
        let target = Target::new(&device, &queue, test_projection(), Vec3::ZERO);
        let forms = [("array literal", SmallLayerOptions::OLD), ("packed", SmallLayerOptions::NEW)];
        let lines = forms.map(|(_, options)| {
            let mut lines = DebugLines::with_options(&device, &target.cam_layout, FORMAT, options);
            for i in 0..SEGMENTS {
                let (x, y) = ((i % 800) as f32 * 0.005 - 2.0, (i / 800) as f32 * 0.008 - 2.0);
                lines.line(Layer::Depth, [x, y, -5.0], [x + 0.004, y, -5.0], 0xffff_ffff, 1.0);
            }
            // Through the queue: the frames below are submitted directly.
            lines.upload_geometry(&device, &queue, &mut Uploader::new(&device, false), (SIZE, SIZE));
            lines
        });
        let mut best = [f64::MAX; 2];
        for _round in 0..9 {
            for (form, lines) in lines.iter().enumerate() {
                let mut enc = device.create_command_encoder(&Default::default());
                lines.draw(&mut target.pass(&mut enc, true), Layer::Depth);
                let t = std::time::Instant::now();
                let submitted = queue.submit([enc.finish()]);
                let wait = wgpu::PollType::Wait { submission_index: Some(submitted), timeout: None };
                device.poll(wait).expect("GPU wait");
                best[form] = best[form].min(t.elapsed().as_secs_f64() * 1e3);
            }
        }
        eprintln!("{} segments: {} {:.3} ms, {} {:.3} ms", SEGMENTS, forms[0].0, best[0], forms[1].0, best[1]);
    }
}

/// Micro-bench on the GPU (machine loaded by other agents' builds: an order of magnitude only):
/// CPU time of a frame's small-layer uploads plus the frame's submission, paths of before against
/// the new ones, in a frame where another layer already writes through the ring (its transit
/// command buffer exists anyway, as in the client). "Target": the outline of a still targeted
/// block alone (the usual frame of a game); "mining": that outline and one cracked block whose
/// stage moves every sixteenth frame; "all": every layer, the whole script. The counters are those
/// of the small layers alone.
#[test]
#[ignore = "micro-bench: cargo test --release -p rc-app small_layer_writes_bench -- --ignored --nocapture"]
fn small_layer_writes_bench() {
    const BENCH_FRAMES: u32 = 600;
    let Some((device, queue)) = gpu() else { return };
    let target = Target::new(&device, &queue, test_projection(), Vec3::ZERO);
    let other_layer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 256,
        usage: wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    // Per scene: the lists of a frame, the layers sent (order of `LAYERS`), and whether the window
    // follows the script (it changes the line styles) or stays put.
    type Fill = fn(&mut Scene, u32);
    let (outline, cracks) = (std::array::from_fn(|l| l == 3), std::array::from_fn(|l| l == 2 || l == 3));
    let scenes: [(&str, Fill, [bool; 7], bool); 3] = [
        ("target", |s, _| s.fill_outline(0), outline, false),
        (
            "mining",
            |s, n| {
                s.fill_outline(0);
                s.fill_cracks(n / 8 % 4);
            },
            cracks,
            false,
        ),
        ("all", |s, n| s.fill(n), EVERY_LAYER, true),
    ];
    let empty_frame = || device.create_command_encoder(&Default::default()).finish();
    for (scene_name, fill, sent, window_moves) in scenes {
        for (name, paths) in [("before", OLD), ("ring alone", Paths { ring: true, ..OLD }), ("all new paths", NEW)] {
            let mut scene = Scene::new(&device, &queue, &target, paths);
            // Uploader of the frame's other layer when the small layers write through the queue.
            let mut other_ring = Uploader::new(&device, true);
            let mut times = Vec::new();
            for n in 0..BENCH_FRAMES {
                fill(&mut scene, n);
                let frame_uploader = if paths.ring { &mut scene.up } else { &mut other_ring };
                frame_uploader.write(&queue, &other_layer, 0, &[7; 256]);
                let t = std::time::Instant::now();
                scene.upload_layers(&device, &queue, if window_moves { n } else { 0 }, sent);
                let frame_uploader = if paths.ring { &mut scene.up } else { &mut other_ring };
                let submitted = frame_uploader.submit(&queue, empty_frame());
                times.push(t.elapsed().as_secs_f64() * 1e6);
                if n % 8 == 7 {
                    let wait = wgpu::PollType::Wait { submission_index: Some(submitted), timeout: None };
                    device.poll(wait).expect("GPU wait");
                }
            }
            times.sort_by(f64::total_cmp);
            let s = scene.up.take_stats();
            // Through the ring the frame's other write is counted with the small layers: taken out.
            let other = u64::from(paths.ring) * u64::from(BENCH_FRAMES);
            let per_frame = |count: u64| count as f64 / f64::from(BENCH_FRAMES);
            eprintln!(
                "{scene_name}, {name}: median {:.1} µs, best {:.1} µs per frame; {:.2} writes, {:.2} staging buffers, {:.0} bytes per frame",
                times[times.len() / 2],
                times[0],
                per_frame(s.writes - other),
                per_frame(s.transits),
                per_frame(s.bytes - other * 256)
            );
        }
    }
}
