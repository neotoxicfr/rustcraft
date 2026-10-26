//! Proofs of the interface optimisations (C4-M14): a scene that goes through all the
//! primitives (tiled backgrounds under clipping and offset, nine-slices, tooltip, styled text,
//! shadowed, bold, italic, underlined, struck, obfuscated, with background, scaled, rotated, clipped,
//! clipped gradient, pie chart, vignette, End portal, smoothed sprite, highlight, boss
//! bars, creative tabs, blur, overlays and interleaved models) is rendered offscreen at
//! 1600 × 900 on the real GPU, old path against each switch alone and combined, at
//! scales 1 to 6: frames equal to the byte. The vertices sent are also decoded on the
//! CPU (index, instances) and compared with the triangles of the old path.
//!
//! Without an installed game or without a GPU, the tests skip themselves; `RUSTCRAFT_OPT_REQUIRE=1` makes that a failure.

use super::*;
use crate::render::upload::Uploader;

const W: u32 = 1600;
const H: u32 = 900;
const ALL: GuiOpts = GuiOpts {
    write_with: true,
    uniform: true,
    index: true,
    instance: true,
    keep: true,
    sprite_ids: true,
    tile_shader: true,
    text_cache: true,
    font_table: true,
};

struct Env {
    device: wgpu::Device,
    queue: wgpu::Queue,
    assets: rc_assets::Assets,
}

fn env() -> Option<Env> {
    let found =
        rc_data::locate("26.3").ok().and_then(|i| rc_assets::Assets::for_install(&i.client_jar, &[], None).ok());
    let assets = rc_testdata::need(found, "installed game 26.3")?;
    crate::render::upload::tests::gpu_serial();
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .ok();
    let adapter = rc_testdata::need(adapter, "GPU adapter")?;
    eprintln!("GPU : {} ({:?})", adapter.get_info().name, adapter.get_info().backend);
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("device");
    Some(Env { device, queue, assets })
}

fn gui(e: &Env, opts: GuiOpts) -> Gui {
    let a = &e.assets;
    let fonts =
        crate::resources::FONT_IDS.iter().map(|id| ((*id).to_owned(), Font::load(a, id, Default::default()))).collect();
    let lang = rc_text::Lang::load(&a.read("assets/minecraft/lang/en_us.json").unwrap_or_default(), None, None);
    let assets = GuiAssets {
        atlas: GuiAtlas::load(a),
        fonts,
        lang,
        vignette: rc_assets::texture::load(a, "minecraft:misc/vignette"),
        items: None,
        special: Default::default(),
    };
    let mut g = Gui::new(&e.device, &e.queue, wgpu::TextureFormat::Rgba8Unorm, assets);
    g.set_opts(opts);
    g
}

fn styled(s: &str) -> Vec<StyledChar> {
    plain(s)
}

/// Tiled background placed by the scene: visible (clipped) area and pattern origin, in GUI units,
/// computed like [`Gui::sprite_tiled`].
struct Tiled {
    name: &'static str,
    clip: [f32; 4],
    origin: [f32; 2],
    tile: [f32; 2],
}

/// [`Gui::sprite_tiled`] on `r` (tiles `t`, offset `uv`) under the clip `clip`, recorded in `out`.
fn tiled(
    g: &mut Gui,
    out: &mut Vec<Tiled>,
    clip: Option<[f32; 4]>,
    name: &'static str,
    r: [f32; 4],
    t: [f32; 2],
    uv: [f32; 2],
) {
    let c = clip.map_or(r, |c| [c[0].max(r[0]), c[1].max(r[1]), c[2].min(r[2]), c[3].min(r[3])]);
    let origin = [r[0] - uv[0].rem_euclid(t[0]), r[1] - uv[1].rem_euclid(t[1])];
    out.push(Tiled { name, clip: c, origin, tile: t });
    g.sprite_tiled(name, r[0], r[1], r[2], r[3], t[0], t[1], uv[0], uv[1]);
}

/// Screen pixels per GUI unit, per axis, of the last frame placed: the interface is projected
/// onto the exact size (window ÷ scale, `proj`), not onto the layout size rounded
/// up (534 in x at scale 3), hence the scale itself on both axes.
fn factor(g: &Gui) -> [f64; 2] {
    [f64::from(W) / f64::from(g.proj[0]), f64::from(H) / f64::from(g.proj[1])]
}

/// Centre of the pixel `(px, py)` in GUI units.
fn centre(k: [f64; 2], px: u32, py: u32) -> [f64; 2] {
    [(f64::from(px) + 0.5) / k[0], (f64::from(py) + 0.5) / k[1]]
}

/// The centre of the pixel `(px, py)` falls exactly (to a thousandth of a pixel) on a tile joint
/// of a visible tiled background: the only place where the tiled quad and the per-tile quad can
/// take different texels. The tiled quad takes an edge of the same sprite there
/// (`tiled_seams_take_own_texels`); the per-tile quad may read beyond the edge of its
/// rectangle in the atlas (texel of the neighbouring sprite, often transparent: pixel not drawn).
fn on_seam(t: &[Tiled], k: [f64; 2], px: u32, py: u32) -> bool {
    let c = centre(k, px, py);
    let eps = [1e-3 / k[0], 1e-3 / k[1]];
    let seam = |a: usize, o: f32, t: f32| {
        let n = (c[a] - f64::from(o)) / f64::from(t);
        (n - n.round()).abs() * f64::from(t) <= eps[a]
    };
    t.iter().any(|d| {
        let inside = (0..2).all(|a| c[a] >= f64::from(d.clip[a]) - eps[a] && c[a] <= f64::from(d.clip[a + 2]) + eps[a]);
        inside && (seam(0, d.origin[0], d.tile[0]) || seam(1, d.origin[1], d.tile[1]))
    })
}

/// Test scene; `frame` varies part of the content (sending only the changes).
/// Renders the placed tiled backgrounds.
fn scene(g: &mut Gui, q: &wgpu::Queue, scale: u32, frame: u32) -> Vec<Tiled> {
    g.scale_option = scale;
    g.begin(W, H);
    let (w, h) = (g.width, g.height);
    let mut t = Vec::new();
    // Pause background: blur then full-screen tiled background.
    g.blur(3);
    tiled(g, &mut t, None, "bg/inworld_menu_background", [0.0, 0.0, w, h], [32.0, 32.0], [0.0, 0.0]);
    // Header and separators (32 × 2), tabs (16 × 16).
    tiled(g, &mut t, None, "bg/tab_header_background", [0.0, 0.0, w, 24.0], [16.0, 16.0], [0.0, 0.0]);
    tiled(g, &mut t, None, "bg/header_separator", [0.0, 22.0, w * 0.4, 24.0], [32.0, 2.0], [0.0, 0.0]);
    tiled(g, &mut t, None, "bg/footer_separator", [3.0, h - 33.0, w, h - 31.0], [32.0, 2.0], [0.0, 0.0]);
    // Scrolled list (non-integer v, negative offset) under the clip of a parent.
    let clip = Some([20.0, 30.0, w - 20.0, h - 40.0]);
    g.set_clip(clip);
    let list = [10.0, 26.0, w - 10.0, h - 30.0];
    tiled(g, &mut t, clip, "bg/menu_background", list, [32.0, 32.0], [7.0, 13.37 + frame as f32]);
    tiled(g, &mut t, clip, "bg/menu_background", [40.0, 60.0, 140.0, 120.0], [32.0, 32.0], [-5.5, -40.25]);
    let txt = styled("Text clipped in a list that overflows to the right and bottom");
    g.text(q, &txt, 30.0, 34.0, 0xffe0_e0e0, true, 0);
    g.fill_gradient(25.0, 25.0, 90.0, 80.0, 0xc010_1010, 0xd030_3030);
    g.set_clip(None);
    // Pattern offsets 0 to 31.
    for k in 0..32 {
        let x = 4.0 + k as f32 * 9.0;
        let uv = [k as f32, k as f32 * 0.5];
        tiled(g, &mut t, None, "bg/menu_background", [x, 130.0, x + 8.0, 170.0], [32.0, 32.0], uv);
    }
    // Buttons (nine-slices) and centred text.
    for (i, id) in [Pre::BUTTON, Pre::BUTTON_HIGHLIGHTED, Pre::BUTTON_DISABLED].into_iter().enumerate() {
        let y = 50.0 + 24.0 * i as f32;
        g.sprite_pre(id, w / 2.0 - 100.0, y, 200.0, 20.0, 0xffff_ffff);
        g.sprite_pre(id, w / 2.0 + 104.0, y, 98.0, 20.0, 0xffff_ffff);
        g.sprite_pre(id, w / 2.0 - 160.0, y, 20.0, 20.0, 0xffff_ffff);
        g.text(q, &styled("Back to Game"), w / 2.0 - 30.0, y + 6.0, 0xffff_ffff, true, 0);
    }
    g.sprite("widget/slot_frame", 10.0, 200.0, 18.0, 18.0, 0xffff_ffff);
    g.sprite("container/slot", 30.0, 200.0, 18.0, 18.0, 0xffff_ffff);
    g.sprite("widget/text_field", 60.0, 200.0, 120.0, 20.0, 0xffff_ffff);
    // Boss bars and creative tabs.
    for c in 0..BOSS_COLORS.len() {
        let y = 230.0 + 7.0 * c as f32;
        g.sprite_pre(Pre::boss_color(c, false), 10.0, y, 182.0, 5.0, 0xffff_ffff);
        g.sprite_region_pre(Pre::boss_color(c, true), 182.0, 5.0, 0.0, 0.0, 10.0, y, 91.0 + c as f32, 5.0);
        let s = 1 + c % 4;
        g.sprite_pre(Pre::boss_notches(s, false), 10.0, y, 182.0, 5.0, 0xffff_ffff);
        g.sprite_region_pre(Pre::boss_notches(s, true), 182.0, 5.0, 0.0, 0.0, 10.0, y, 60.0, 5.0);
    }
    for col in 0..7 {
        let x = 200.0 + 28.0 * col as f32;
        g.sprite_pre(Pre::creative_tab(true, col % 2 == 0, col), x, 220.0, 26.0, 32.0, 0xffff_ffff);
        g.sprite_pre(Pre::creative_tab(false, col % 2 == 1, col), x, 256.0, 26.0, 32.0, 0xffff_ffff);
    }
    g.sprite_pre(Pre::CREATIVE_SCROLLER, 400.0, 220.0, 12.0, 15.0, 0xffff_ffff);
    g.sprite_pre(Pre::CREATIVE_SCROLLER_DISABLED, 414.0, 220.0, 12.0, 15.0, 0xffff_ffff);
    // F3: text with background, pie chart.
    g.text(q, &styled("XYZ: 12.345 / 64.00000 / -7.250"), 2.0, 300.0, 0xffe0_e0e0, false, 0x9050_5050);
    g.text(q, &styled("Chunk: 3 4 7 [0 0 in r.0.0.mca]"), 2.0, 309.0, 0xffe0_e0e0, false, 0x9050_5050);
    let (cx, cy, r) = (300.0, 340.0, 30.0);
    for k in 0..12 {
        let a0 = k as f32 / 12.0 * std::f32::consts::TAU;
        let a1 = (k + 1) as f32 / 12.0 * std::f32::consts::TAU;
        let p0 = [cx + r * a0.cos(), cy + r * a0.sin()];
        let p1 = [cx + r * a1.cos(), cy + r * a1.sin()];
        g.fill_quad([[cx, cy], p0, p1, p1], 0xff00_0000 | (k * 0x15_0b07));
    }
    // Styled chat with background, at scale 0.5.
    let chat = styled("§l§nBold underlined§r §oitalic§r §mstruck§r §kobfus§r §cRed §e§lYellow");
    g.text(q, &chat, 4.0, 380.0, 0xffff_ffff, true, 0x8000_0000);
    g.pose = 0.5;
    g.text(q, &chat, 8.0, 800.0, 0xffff_ffff, true, 0x8000_0000);
    g.fill(8.0, 810.0, 200.0, 820.0, 0x8020_4060);
    g.pose = 1.0;
    // Rotated splash.
    let m = g.mark();
    g.text(q, &styled("Also try Terraria!"), -40.0, -4.0, 0xffff_ff00, true, 0);
    let (sn, cs) = (-20f32.to_radians().sin(), (-20f32).to_radians().cos());
    g.transform_since(m, [cs, sn, -sn, cs, w * 0.7, 60.0]);
    // Vignette, End portal, smoothed sprite, highlight.
    g.vignette_rgb([0.3, 0.3, 0.3]);
    g.end_portal(w - 120.0, h - 120.0, w - 10.0, h - 50.0);
    g.sprite_smooth([w - 260.0, 10.0, w - 130.0, 42.0], [0.0, 0.0, 0.5, 1.0], 0xffff_ffff);
    g.text(q, &styled("Selection"), 200.0, 420.0, 0xffff_ffff, false, 0);
    g.text_highlight(200.0, 419.0, 240.0, 429.0, true);
    // Camera overlays and interleaved models.
    g.overlays_here();
    g.fill(500.0, 200.0, 560.0, 260.0, 0x80ff_0000);
    g.models_here();
    g.fill(520.0, 220.0, 580.0, 280.0, 0x8000_ff00);
    // Tooltip (content varies with the frame).
    g.tooltip_layer();
    let (tx, ty) = (300.0 + frame as f32 * 3.0, 450.0);
    g.sprite_pre(Pre::TOOLTIP_BACKGROUND, tx - 12.0, ty - 12.0, 140.0, 44.0, 0xffff_ffff);
    g.sprite_pre(Pre::TOOLTIP_FRAME, tx - 12.0, ty - 12.0, 140.0, 44.0, 0xffff_ffff);
    g.text(q, &styled(&format!("§bDiamond Sword {frame}")), tx, ty, 0xffff_ffff, true, 0);
    g.text(q, &styled("§9+7 Attack Damage"), tx, ty + 10.0, 0xffff_ffff, true, 0);
    g.draw_before_tooltips(|g| g.fill(310.0, 455.0, 330.0, 465.0, 0xff40_40ff));
    t
}

/// Empties the pass for an interleaved draw: rebinds another vertex buffer (the interface
/// must redo its bindings afterwards).
fn spoil(p: &mut wgpu::RenderPass<'_>, junk: &wgpu::Buffer, calls: &std::cell::Cell<u32>) {
    p.set_vertex_buffer(0, junk.slice(..));
    p.set_index_buffer(junk.slice(..), wgpu::IndexFormat::Uint32);
    calls.set(calls.get() + 1);
}

/// Renders the image of `g` (before and after the blur, like the engine) and returns its RGBA pixels.
fn render(e: &Env, g: &mut Gui, up: &mut Uploader) -> Vec<u8> {
    let (d, q) = (&e.device, &e.queue);
    g.upload(d, q, up);
    let target = d.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let depth = d.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let (view, dview) = (target.create_view(&Default::default()), depth.create_view(&Default::default()));
    let junk = d.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 1 << 16,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::INDEX,
        mapped_at_creation: false,
    });
    let calls = std::cell::Cell::new(0);
    let mut enc = d.create_command_encoder(&Default::default());
    for after in [false, true] {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: if after {
                        wgpu::LoadOp::Load
                    } else {
                        wgpu::LoadOp::Clear(wgpu::Color { r: 0.2, g: 0.4, b: 0.6, a: 1.0 })
                    },
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &dview,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        g.draw_blur_part_models(&mut pass, after, |p| spoil(p, &junk, &calls), |p| spoil(p, &junk, &calls));
    }
    let out = d.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(W * H * 4),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    enc.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &out,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(W * 4), rows_per_image: Some(H) },
        },
        wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
    );
    up.submit(q, enc.finish());
    assert_eq!(calls.get(), 2, "overlays and interleaved models once each");
    let slice = out.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("image read"));
    d.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("GPU wait");
    let px = slice.get_mapped_range().expect("projected image").to_vec();
    out.unmap();
    px
}

fn diff(a: &[u8], b: &[u8]) -> usize {
    a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0).filter(|(x, y)| x != y).count()
}

/// Positions (x, y) of the differing pixels.
fn diff_at(a: &[u8], b: &[u8]) -> Vec<(u32, u32)> {
    a.as_chunks::<4>()
        .0
        .iter()
        .zip(b.as_chunks::<4>().0)
        .enumerate()
        .filter(|(_, (x, y))| x != y)
        .map(|(i, _)| (i as u32 % W, i as u32 / W))
        .collect()
}

/// Pixels: first positions and bounding rectangle.
fn where_diff(px: &[(u32, u32)]) -> String {
    let (x0, x1) = (px.iter().map(|p| p.0).min(), px.iter().map(|p| p.0).max());
    let (y0, y1) = (px.iter().map(|p| p.1).min(), px.iter().map(|p| p.1).max());
    format!("x {x0:?}..{x1:?} y {y0:?}..{y1:?}, first {:?}", &px[..px.len().min(6)])
}

/// Combinations compared with the old path.
fn configs() -> Vec<(&'static str, GuiOpts)> {
    let o = GuiOpts::OLD;
    vec![
        ("direct write", GuiOpts { write_with: true, ..o }),
        ("kept uniform", GuiOpts { uniform: true, ..o }),
        ("index", GuiOpts { index: true, ..o }),
        ("instances", GuiOpts { instance: true, ..o }),
        ("changes only", GuiOpts { keep: true, ..o }),
        ("resolved sprites", GuiOpts { sprite_ids: true, ..o }),
        ("tiles in shader", GuiOpts { tile_shader: true, ..o }),
        ("cached text", GuiOpts { text_cache: true, ..o }),
        ("font table", GuiOpts { font_table: true, ..o }),
        ("index + direct write + tiles", GuiOpts { index: true, write_with: true, tile_shader: true, ..o }),
        ("changes only + index", GuiOpts { keep: true, index: true, ..o }),
        ("changes only + instances + tiles", GuiOpts { keep: true, instance: true, tile_shader: true, ..o }),
        ("all", ALL),
        ("all except changes only", GuiOpts { keep: false, ..ALL }),
        ("all as index", GuiOpts { instance: false, ..ALL }),
    ]
}

/// Frames of the scene, scales 1 to 6 and three successive frames (one identical to the
/// previous): each combination equals the old path to the byte, except, with the tiles
/// in the shader, on the only pixels whose centre falls exactly on a tile joint (their
/// content is checked by `tiled_seams_take_own_texels`). The paths that go through the
/// transit are rendered with and without ring (`RUSTCRAFT_OPT_UPLOADRING=0`: queue write view).
#[test]
fn gui_paths_render_identical_frames() {
    let Some(e) = env() else { return };
    let mut old = gui(&e, GuiOpts::OLD);
    let mut up_old = Uploader::new(&e.device, true);
    let frames: Vec<(u32, u32)> = (1..=6).flat_map(|s| [(s, 0), (s, 1), (s, 1)]).collect();
    let reference: Vec<(Vec<u8>, Vec<Tiled>, [f64; 2])> = frames
        .iter()
        .map(|&(s, f)| {
            let t = scene(&mut old, &e.queue, s, f);
            (render(&e, &mut old, &mut up_old), t, factor(&old))
        })
        .collect();
    assert!(diff(&reference[0].0, &reference[3].0) > 0, "scales 1 and 2 distinct (empty scene?)");
    let mut failures = Vec::new();
    let mut ties = Vec::new();
    for (name, opts) in configs() {
        let rings: &[bool] = if opts.write_with || opts.keep { &[true, false] } else { &[true] };
        for &ring in rings {
            let mut g = gui(&e, opts);
            let mut up = Uploader::new(&e.device, ring);
            for (k, &(s, f)) in frames.iter().enumerate() {
                scene(&mut g, &e.queue, s, f);
                let px = render(&e, &mut g, &mut up);
                let (want, tiles, kf) = &reference[k];
                let differing = diff_at(want, &px);
                let (seam, bad): (Vec<_>, Vec<_>) =
                    differing.into_iter().partition(|&(x, y)| opts.tile_shader && on_seam(tiles, *kf, x, y));
                if !seam.is_empty() {
                    ties.push(format!("{name} scale {s}: {}", seam.len()));
                }
                if !bad.is_empty() {
                    let ring = if ring { "" } else { " (no ring)" };
                    failures.push(format!(
                        "{name}{ring}: scale {s}, frame {f}: {} differing pixels off a seam ({})",
                        bad.len(),
                        where_diff(&bad)
                    ));
                }
            }
        }
    }
    eprintln!("pixels on an exact seam: {}", ties.join(", "));
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Candidate texels on an axis for a pixel centre `c` (GUI units): the one that contains it,
/// and also its neighbour (same sprite, modulo the tile) if `c` falls exactly on a texel edge.
fn texel_candidates(c: f64, origin: f32, tile: f32, n: u32, k: f64) -> Vec<u32> {
    debug_assert!(n > 0);
    let a = (c - f64::from(origin)) / f64::from(tile);
    let tex = (a - a.floor()) * f64::from(n);
    let r = tex.round();
    if (tex - r).abs() * f64::from(tile) / f64::from(n) * k <= 1e-3 {
        let r = r as i64;
        vec![(r - 1).rem_euclid(i64::from(n)) as u32, r.rem_euclid(i64::from(n)) as u32]
    } else {
        vec![(tex.floor() as u32).min(n - 1)]
    }
}

/// Tiled backgrounds alone (4 sprites, integer, half-integer and arbitrary offsets), scales 1
/// to 6, tiles in the shader alone and all: each interior pixel shows, mixed with the clear
/// colour, a candidate texel OF THE SAME SPRITE (read in the atlas on the CPU), including
/// on the exact joints: never a texel of the neighbouring sprite of the atlas (the game repeats a
/// texture of its own). The old path (one quad per tile) validates the model off the
/// joints; on an exact joint it reads beyond the sprite's edge (see [`on_seam`]).
#[test]
fn tiled_seams_take_own_texels() {
    let Some(e) = env() else { return };
    let sprites: [(&'static str, [f32; 2]); 4] = [
        ("bg/menu_background", [32.0, 32.0]),
        ("bg/inworld_menu_background", [32.0, 32.0]),
        ("bg/tab_header_background", [16.0, 16.0]),
        ("bg/header_separator", [32.0, 2.0]),
    ];
    let offsets = [[0.0, 0.0], [0.5, 0.5], [-5.5, -40.25], [7.0, 13.37], [13.5, 2.5], [31.5, 17.0]];
    let clear = [51.0, 102.0, 153.0];
    // CPU copy of the atlas (the interface's one is emptied after the upload to the GPU).
    let atlas = GuiAtlas::load(&e.assets);
    let mut failures = Vec::new();
    for (label, opts) in
        [("old", GuiOpts::OLD), ("tiles in shader", GuiOpts { tile_shader: true, ..GuiOpts::OLD }), ("all", ALL)]
    {
        let mut g = gui(&e, opts);
        let mut up = Uploader::new(&e.device, true);
        for scale in 1..=6 {
            g.scale_option = scale;
            g.begin(W, H);
            let mut t = Vec::new();
            for (i, &(name, tile)) in sprites.iter().enumerate() {
                for (j, &uv) in offsets.iter().enumerate() {
                    let (x, y) = (4.0 + 76.0 * j as f32, 4.0 + 58.0 * i as f32);
                    tiled(&mut g, &mut t, None, name, [x, y, x + 70.0, y + 50.0], tile, uv);
                }
            }
            let k = factor(&g);
            let px = render(&e, &mut g, &mut up);
            let img = &atlas.image;
            let (mut checked, mut seams, mut bad) = (0u32, 0u32, Vec::new());
            for d in &t {
                let s = *atlas.sprites.get(d.name).unwrap_or_else(|| panic!("{} missing from the atlas", d.name));
                let (x0, x1) =
                    ((f64::from(d.clip[0]) * k[0]).floor() as u32, (f64::from(d.clip[2]) * k[0]).ceil() as u32);
                let (y0, y1) =
                    ((f64::from(d.clip[1]) * k[1]).floor() as u32, (f64::from(d.clip[3]) * k[1]).ceil() as u32);
                for py in y0..y1 {
                    for pxx in x0..x1 {
                        let [cx, cy] = centre(k, pxx, py);
                        // Outer edges excluded (quad coverage, off topic here).
                        let (mx, my) = (1e-3 / k[0], 1e-3 / k[1]);
                        if cx <= f64::from(d.clip[0]) + mx
                            || cx >= f64::from(d.clip[2]) - mx
                            || cy <= f64::from(d.clip[1]) + my
                            || cy >= f64::from(d.clip[3]) - my
                            || !opts.tile_shader && on_seam(std::slice::from_ref(d), k, pxx, py)
                        {
                            continue;
                        }
                        let tu = texel_candidates(cx, d.origin[0], d.tile[0], s.width, k[0]);
                        let tv = texel_candidates(cy, d.origin[1], d.tile[1], s.height, k[1]);
                        let at = (py * W + pxx) as usize * 4;
                        let got = &px[at..at + 3];
                        let ok = tu.iter().any(|&u| {
                            tv.iter().any(|&v| {
                                let o = (((s.y + v) * img.width + s.x + u) * 4) as usize;
                                let c = &img.rgba[o..o + 4];
                                let a = f64::from(c[3]) / 255.0;
                                (0..3).all(|ch| {
                                    let want = if c[3] == 0 {
                                        clear[ch]
                                    } else {
                                        f64::from(c[ch]) * a + clear[ch] * (1.0 - a)
                                    };
                                    (f64::from(got[ch]) - want).abs() <= 2.0
                                })
                            })
                        });
                        checked += 1;
                        seams += u32::from(on_seam(std::slice::from_ref(d), k, pxx, py));
                        if !ok {
                            bad.push((pxx, py));
                        }
                    }
                }
            }
            assert!(checked > 10_000, "{label} scale {scale}: too few pixels checked ({checked})");
            if opts.tile_shader && scale <= 2 {
                assert!(seams > 100, "{label} scale {scale}: exact seams not exercised ({seams})");
            }
            if !bad.is_empty() {
                failures.push(format!(
                    "{label} scale {scale}: {} pixels outside the sprite ({})",
                    bad.len(),
                    where_diff(&bad)
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Interface bigger than a chunk of the transit (more than 1 MiB sent): large-send branch
/// of the ring and queue write view, image equal to the old path to the byte.
#[test]
fn large_gui_frame_uploads_identical() {
    let Some(e) = env() else { return };
    let grid = |g: &mut Gui| {
        g.scale_option = 1;
        g.begin(W, H);
        for i in 0..160u32 {
            for j in 0..90u32 {
                let (x, y) = (10.0 * i as f32, 10.0 * j as f32);
                g.fill(x, y, x + 8.0, y + 8.0, 0xff00_0000 | (i * 0x01_0307 + j * 0x05_0b00));
            }
        }
    };
    let mut old = gui(&e, GuiOpts::OLD);
    grid(&mut old);
    let want = render(&e, &mut old, &mut Uploader::new(&e.device, true));
    let o = GuiOpts::OLD;
    for (name, opts) in [
        ("direct write", GuiOpts { write_with: true, ..o }),
        ("instances + direct write", GuiOpts { instance: true, write_with: true, ..o }),
        ("all except changes only", GuiOpts { keep: false, ..ALL }),
        ("all", ALL),
    ] {
        for ring in [true, false] {
            let mut g = gui(&e, opts);
            let mut up = Uploader::new(&e.device, ring);
            grid(&mut g);
            let px = render(&e, &mut g, &mut up);
            let sent = up.take_stats().bytes;
            assert!(sent > 1 << 20, "{name} ring {ring}: {sent} B sent, large send not exercised");
            let d = diff_at(&want, &px);
            assert!(d.is_empty(), "{name} ring {ring}: {} differing pixels ({})", d.len(), where_diff(&d));
        }
    }
}

/// Triangles of 6 vertices of the quads sent, decoded from the send format (independently of the
/// shader): static indices on 4 corners, or instance (corners, uv rectangle, colour per row).
fn decoded(g: &Gui) -> Vec<Vertex> {
    let mut bytes = Vec::new();
    let fmt = g.opts.format();
    Gui::encode(&g.layers, fmt, &mut |b| bytes.extend_from_slice(b));
    match fmt {
        Format::Tri => bytemuck::cast_slice(&bytes).to_vec(),
        Format::Indexed => bytemuck::cast_slice::<u8, Vertex>(&bytes)
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|q| [q[0], q[1], q[2], q[2], q[3], q[0]])
            .collect(),
        Format::Instanced => bytemuck::cast_slice::<u8, Inst>(&bytes)
            .iter()
            .flat_map(|i| {
                let [u0, v0, u1, v1] = i.uv;
                let c = |k: usize, uv: [f32; 2], color: [f32; 4]| Vertex {
                    pos: [i.p[2 * k], i.p[2 * k + 1]],
                    uv,
                    color,
                    mode: i.mode,
                };
                let q =
                    [c(0, [u0, v0], i.top), c(1, [u0, v1], i.bottom), c(2, [u1, v1], i.bottom), c(3, [u1, v0], i.top)];
                [q[0], q[1], q[2], q[2], q[3], q[0]]
            })
            .collect(),
    }
}

/// The vertices sent as index or as instances, redeployed into triangles, are those of the old
/// path to the bit (tiles on the CPU on both sides).
#[test]
fn gui_formats_decode_to_old_triangles() {
    let Some(e) = env() else { return };
    for opts in [GuiOpts { index: true, ..GuiOpts::OLD }, GuiOpts { instance: true, ..GuiOpts::OLD }] {
        // Two new interfaces: same sequence of draws for the obfuscated text.
        let (mut old, mut g) = (gui(&e, GuiOpts::OLD), gui(&e, opts));
        for s in 1..=6 {
            scene(&mut old, &e.queue, s, 0);
            scene(&mut g, &e.queue, s, 0);
            let (a, b) = (decoded(&old), decoded(&g));
            assert_eq!(a.len(), b.len(), "{opts:?} scale {s}: vertex count");
            let bad = a.iter().zip(&b).position(|(x, y)| bytemuck::bytes_of(x) != bytemuck::bytes_of(y));
            if let Some(i) = bad {
                let q = i / 6 * 6;
                panic!("{opts:?} scale {s}: vertex {i} differs\nold {:?}\nnew {:?}", &a[q..q + 6], &b[q..q + 6]);
            }
        }
    }
}

/// Each pre-resolved sprite is the one the name lookup gives, and the ranks of [`Pre`]
/// designate the names the callers used to write.
#[test]
fn pre_resolved_sprites_match_lookup() {
    let Some(e) = env() else { return };
    let g = gui(&e, GuiOpts::OLD);
    let names = pre_names();
    assert_eq!(names.len(), Pre::COUNT);
    let mut found = 0;
    for (i, n) in names.iter().enumerate() {
        let by_name = g.atlas.sprites.get(n.as_str()).copied();
        assert_eq!(g.pre[i], by_name, "{n}");
        found += usize::from(by_name.is_some());
    }
    assert_eq!(found, names.len(), "all frequent sprites exist in the game atlas");
    let name = |p: Pre| names[p.0 as usize].as_str();
    assert_eq!(name(Pre::BUTTON_HIGHLIGHTED), "widget/button_highlighted");
    assert_eq!(name(Pre::TOOLTIP_FRAME), "tooltip/frame");
    assert_eq!(name(Pre::CREATIVE_SCROLLER_DISABLED), "container/creative_inventory/scroller_disabled");
    for (c, col) in BOSS_COLORS.iter().enumerate() {
        assert_eq!(name(Pre::boss_color(c, false)), format!("boss_bar/{col}_background"));
        assert_eq!(name(Pre::boss_color(c, true)), format!("boss_bar/{col}_progress"));
    }
    for (s, n) in BOSS_NOTCHES.iter().enumerate().skip(1) {
        assert_eq!(name(Pre::boss_notches(s, false)), format!("boss_bar/{n}_background"));
        assert_eq!(name(Pre::boss_notches(s, true)), format!("boss_bar/{n}_progress"));
    }
    for (top, row) in [(true, "top"), (false, "bottom")] {
        for (sel, s) in [(false, "unselected"), (true, "selected")] {
            for col in 0..7 {
                let want = format!("container/creative_inventory/tab_{row}_{s}_{}", col + 1);
                assert_eq!(name(Pre::creative_tab(top, sel, col)), want);
            }
        }
    }
    assert_eq!(g.screen_rects, g.screen_rects_by_name());
    assert_eq!(g.smooth, g.atlas.sprites.get(SMOOTH_SPRITE).copied());
}

/// Text scene (C4-M15): chat of 20 styled lines with background, shadow every other frame, blended
/// colour, alt/uniform fonts, field without suspect glyph, clipping, half scale, visual order
/// (Arabic), typed text, underlines cut by a click action change, obfuscated, line that
/// changes every frame; `frame` varies what must vary.
fn text_scene(g: &mut Gui, q: &wgpu::Queue, frame: u32) {
    g.scale_option = 3;
    g.begin(W, H);
    let styles = ["§l", "§o", "§n", "§m", "§c", "§e§l", "§9§o", "§r", "§a§n§m", ""];
    for i in 0..20u32 {
        let s = styles[i as usize % styles.len()];
        let line = styled(&format!("{s}<Player{i}> message no. {i} §bwith§r some style, é à ç ñ ß {}", i * 37));
        let bg = if i % 3 == 0 { 0 } else { 0x8000_0000 };
        g.text(q, &line, 2.0, 20.0 + 9.0 * i as f32, 0xffff_ffff, true, bg);
    }
    // Shadow every other frame, same text at the same place.
    g.text(q, &styled("Alternating shadow"), 300.0, 20.0, 0xffff_ffff, frame.is_multiple_of(2), 0);
    // Fade (changing alpha) then fixed.
    let a = if frame < 3 { 0x40 + frame * 0x30 } else { 0xff };
    g.text(q, &styled("Title that fades in"), 300.0, 30.0, a << 24 | 0x00ff_ff00, true, 0);
    // Fonts.
    let mut alt = styled("Galactic alt");
    let mut uni = styled("Uniform ¿ € ∑");
    // Fonts mixed in the line (font change along the style runs).
    for (i, c) in alt.iter_mut().enumerate() {
        if i % 3 != 0 {
            c.style.font = "minecraft:alt".into();
        }
    }
    for c in &mut uni {
        c.style.font = "minecraft:uniform".into();
    }
    g.text(q, &alt, 300.0, 40.0, 0xffff_ffff, true, 0);
    g.text(q, &uni, 300.0, 50.0, 0xffff_ffff, true, 0);
    // Chat field: suspect glyphs discarded.
    g.filter_fishy.set(true);
    g.text_forward(q, &styled("input in progress_"), 4.0, 220.0, 0xffe0_e0e0, true);
    g.filter_fishy.set(false);
    g.text_forward(q, &styled("input in progress_"), 4.0, 230.0, 0xffe0_e0e0, true);
    // Clipping and half scale.
    g.set_clip(Some([300.0, 60.0, 340.0, 75.0]));
    g.text(q, &styled("§nText clipped on the right"), 300.0, 62.0, 0xffff_ffff, true, 0);
    g.set_clip(None);
    g.pose = 0.5;
    g.text(q, &styled("§lsmall"), 600.0, 140.0, 0xffff_ffff, true, 0x8000_0000);
    g.pose = 1.0;
    // Visual order.
    g.text(q, &styled("مرحبا بالعالم 123"), 300.0, 80.0, 0xffff_ffff, true, 0);
    // Underlined: two different click actions cut the run (start offset by 1).
    let mut link = styled("§nlink1link2");
    let (c1, c2) = (
        Some(std::sync::Arc::new(rc_text::ClickEvent { action: "open_url".into(), value: "a".into(), nbt: None })),
        Some(std::sync::Arc::new(rc_text::ClickEvent { action: "open_url".into(), value: "b".into(), nbt: None })),
    );
    let k = if frame.is_multiple_of(2) { 3 } else { 5 };
    for (i, c) in link.iter_mut().enumerate() {
        c.style.click = if i < k { c1.clone() } else { c2.clone() };
    }
    g.text(q, &link, 300.0, 90.0, 0xffff_ffff, true, 0);
    // Obfuscated, and line that changes every frame.
    g.text(q, &styled("§kobfuscated§r clear"), 300.0, 100.0, 0xffff_ffff, true, 0);
    g.text(q, &styled(&format!("XYZ: {}.125", frame * 7)), 300.0, 110.0, 0xffe0_e0e0, false, 0x9050_5050);
}

/// Bytes of the placed layers (shapes and text, in order).
fn layer_bytes(g: &Gui) -> Vec<u8> {
    let mut out = Vec::new();
    for (blend, shapes, text) in &g.layers {
        out.push(*blend as u8);
        out.extend_from_slice(&(shapes.len() as u64).to_le_bytes());
        out.extend_from_slice(bytemuck::cast_slice(shapes));
        out.extend_from_slice(&(text.len() as u64).to_le_bytes());
        out.extend_from_slice(bytemuck::cast_slice(text));
    }
    out
}

/// Cached text and font table (alone and together, two formats): vertices placed equal to
/// the old path to the bit, frame by frame (8 frames, fonts replaced at the 5th: new
/// keys), and the cache really is used (lines copied).
#[test]
fn text_paths_emit_identical_vertices() {
    let Some(e) = env() else { return };
    let o = GuiOpts::OLD;
    let reload = |g: &mut Gui| {
        let a = &e.assets;
        let fonts = crate::resources::FONT_IDS
            .iter()
            .map(|id| ((*id).to_owned(), Font::load(a, id, Default::default())))
            .collect();
        g.set_fonts(fonts);
    };
    for (name, opts) in [
        ("cached text", GuiOpts { text_cache: true, ..o }),
        ("font table", GuiOpts { font_table: true, ..o }),
        ("both", GuiOpts { text_cache: true, font_table: true, ..o }),
        ("all (instances)", ALL),
    ] {
        // Reference: same options, text through the old path (same vertex format).
        let mut old = gui(&e, GuiOpts { text_cache: false, font_table: false, ..opts });
        let mut want = Vec::new();
        for f in 0..8 {
            if f == 4 {
                reload(&mut old);
            }
            text_scene(&mut old, &e.queue, f);
            want.push(layer_bytes(&old));
        }
        assert!(want[0] != want[1], "the scene varies from one frame to the next");
        let mut g = gui(&e, opts);
        for f in 0..8 {
            if f == 4 {
                reload(&mut g);
            }
            text_scene(&mut g, &e.queue, f);
            let got = layer_bytes(&g);
            let bad = want[f as usize].iter().zip(&got).position(|(a, b)| a != b);
            assert!(
                got.len() == want[f as usize].len() && bad.is_none(),
                "{name} frame {f}: vertices differ (byte {bad:?}, {} against {} bytes)",
                got.len(),
                want[f as usize].len()
            );
        }
        if opts.text_cache {
            assert!(g.text_hits >= 40, "{name}: only {} lines copied", g.text_hits);
        }
    }
}

/// Text micro-bench (loaded machine possible): 1,000 styled ASCII then Unicode lines,
/// placed 50 times, old path against each switch.
/// `cargo test --release -p rc-app text_bench -- --ignored --nocapture`.
#[test]
#[ignore]
fn text_bench() {
    let Some(e) = env() else { return };
    let ascii: Vec<_> =
        (0..1000).map(|i| styled(&format!("§l<Player{i}>§r ordinary chat message number {i}, fairly long"))).collect();
    let uni: Vec<_> = (0..1000).map(|i| styled(&format!("§cEnchanted Sword {i}: Sharpness V, Ñ ß ¿ ∑ → ★"))).collect();
    let o = GuiOpts::OLD;
    for (label, lines) in [("ASCII", &ascii), ("Unicode", &uni)] {
        for (name, opts) in [
            ("old", o),
            ("font table", GuiOpts { font_table: true, ..o }),
            ("cached text", GuiOpts { text_cache: true, ..o }),
            ("both", GuiOpts { text_cache: true, font_table: true, ..o }),
        ] {
            let mut g = gui(&e, opts);
            let mut t = std::time::Duration::ZERO;
            let mut width = std::time::Duration::ZERO;
            for k in 0..55 {
                g.scale_option = 3;
                g.begin(W, H);
                let t0 = std::time::Instant::now();
                for (i, l) in lines.iter().enumerate() {
                    g.text(&e.queue, l, 2.0, (i % 100) as f32 * 9.0, 0xffff_ffff, true, 0);
                }
                let t1 = std::time::Instant::now();
                let mut sum = 0;
                for l in lines.iter() {
                    sum += rc_text::width(&g, l);
                }
                std::hint::black_box(sum);
                if k >= 5 {
                    t += t1 - t0;
                    width += t1.elapsed();
                }
            }
            let us = |d: std::time::Duration| d.as_secs_f64() * 1e6 / 50.0;
            eprintln!("{label:<8} {name:<20} build {:>8.1} µs  width {:>7.1} µs  (1,000 lines)", us(t), us(width));
        }
    }
}

#[test]
fn changed_range_bounds() {
    let a: Vec<u8> = (0..1000u32).map(|i| i as u8).collect();
    assert_eq!(changed_range(&a, &a), None);
    let mut b = a.clone();
    b[130] ^= 1;
    b[517] ^= 1;
    assert_eq!(changed_range(&a, &b), Some((128, 520)));
    assert_eq!(changed_range(&a[..600], &a), Some((600, 1000)));
    assert_eq!(changed_range(&a, &a[..600]), None);
    assert_eq!(changed_range(&[], &a), Some((0, 1000)));
    let mut c = a.clone();
    c[999] ^= 1;
    assert_eq!(changed_range(&a, &c), Some((996, 1000)));
    c[0] ^= 1;
    assert_eq!(changed_range(&a, &c), Some((0, 1000)));
}

/// Micro-bench (loaded machine possible): scene placement and upload, per combination, at
/// scale 3 (pause at 1600 × 900). `cargo test --release -p rc-app gui_bench -- --ignored --nocapture`.
#[test]
#[ignore]
fn gui_bench() {
    let Some(e) = env() else { return };
    let o = GuiOpts::OLD;
    let all = ALL;
    let cases = [
        ("old", o),
        ("direct write", GuiOpts { write_with: true, ..o }),
        ("kept uniform", GuiOpts { uniform: true, ..o }),
        ("index", GuiOpts { index: true, ..o }),
        ("instances", GuiOpts { instance: true, ..o }),
        ("index + direct write", GuiOpts { index: true, write_with: true, ..o }),
        ("instances + direct write", GuiOpts { instance: true, write_with: true, ..o }),
        ("changes only (static frame)", GuiOpts { keep: true, ..o }),
        ("changes only + instances (static frame)", GuiOpts { keep: true, instance: true, ..o }),
        ("resolved sprites", GuiOpts { sprite_ids: true, ..o }),
        ("tiles in shader", GuiOpts { tile_shader: true, ..o }),
        ("all except changes only", GuiOpts { keep: false, ..all }),
        ("all", all),
    ];
    const N: u32 = 400;
    for (name, opts) in cases {
        let mut g = gui(&e, opts);
        let mut up = Uploader::new(&e.device, true);
        let (mut build, mut send) = (std::time::Duration::ZERO, std::time::Duration::ZERO);
        for k in 0..N + 20 {
            let t = std::time::Instant::now();
            scene(&mut g, &e.queue, 3, 0);
            let t1 = std::time::Instant::now();
            g.upload(&e.device, &e.queue, &mut up);
            // The ring pays for closing the chunk, its command buffer and the copy at the
            // submit: the upload counts up to and including the submit, for all paths.
            up.submit(&e.queue, e.device.create_command_encoder(&Default::default()).finish());
            let t2 = std::time::Instant::now();
            if k % 16 == 0 {
                e.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("GPU wait");
            }
            if k >= 20 {
                build += t1 - t;
                send += t2 - t1;
            }
        }
        let bytes = up.take_stats().bytes / u64::from(N + 20);
        let us = |d: std::time::Duration| d.as_secs_f64() * 1e6 / f64::from(N);
        eprintln!(
            "{name:<45} build {:>7.1} µs  send+submit {:>6.1} µs  {:>7} B/frame  {} elements",
            us(build),
            us(send),
            bytes,
            g.frame_vertices
        );
    }
}
