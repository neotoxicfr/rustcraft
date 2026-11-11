//! Proofs of the item atlas optimizations (C4-M20): the atlas is rendered off-screen on the
//! real GPU at forced instants (enchanted sword and book, compass, blocks with an animated sprite,
//! chest and banner as special models), texture ticks interleaved; old path against
//! each switch alone and combined: atlases equal to the byte on every frame.
//! A second test counts the atlas passes (frames where a cell is drawn) when cells
//! with a glint appear at different instants.
//!
//! Without an installed game or without a GPU, the tests skip; `RUSTCRAFT_OPT_REQUIRE=1` makes that a failure.

use super::*;
use rc_assets::item::{EvalEnv, StackView};

struct Plain;
impl StackView for Plain {
    fn has(&self, _: &str) -> bool {
        false
    }
}
impl EvalEnv for Plain {}

struct Env {
    device: wgpu::Device,
    queue: wgpu::Queue,
    assets: rc_assets::Assets,
    data: rc_data::VersionData,
}

fn env() -> Option<Env> {
    let found =
        rc_data::locate("26.3").ok().and_then(|i| rc_assets::Assets::for_install(&i.client_jar, &[], None).ok());
    let assets = rc_testdata::need(found, "installed 26.3 game")?;
    let data = rc_testdata::version_data()?;
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
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("device");
    Some(Env { device, queue, assets, data })
}

/// Objects of the scene: id, forced glint.
const ITEMS: [(&str, u8); 8] = [
    ("minecraft:diamond_sword", 1),
    ("minecraft:enchanted_book", 1),
    ("minecraft:compass", 2),
    ("minecraft:magma_block", 0),
    ("minecraft:sea_lantern", 0),
    ("minecraft:stone", 0),
    ("minecraft:chest", 0),
    ("minecraft:white_banner", 1),
];

/// The switches as they were before the animated sprite and index leads: the base their proofs
/// compare with.
const BEFORE_SPRITE_LEADS: AtlasOpts = AtlasOpts {
    visible_sprites_only: false,
    pixel_rects: false,
    blend_buffer: false,
    indexed: false,
    ..AtlasOpts::ALL
};

/// Cells drawn by the pending pass (one clear quad each).
fn pending_cells(a: &ItemAtlas) -> usize {
    a.clears.len() / if a.opts.indexed { 4 } else { 6 }
}

/// Atlas of the scene's objects alone (their sprites only in the sheet) and their layers.
fn atlas(e: &Env, opts: AtlasOpts) -> (ItemAtlas, Vec<(u64, Vec<Layer>)>) {
    atlas_with_models(e, opts, ItemModels::load(&e.assets, ITEMS.iter().map(|i| i.0)))
}

/// Atlas over `models` (which hold at least the scene's objects) and the scene's layers.
fn atlas_with_models(e: &Env, opts: AtlasOpts, models: ItemModels) -> (ItemAtlas, Vec<(u64, Vec<Layer>)>) {
    let mut items = Vec::new();
    for (k, (id, glint)) in ITEMS.iter().enumerate() {
        let (mut layers, mut key) = (Vec::new(), Vec::new());
        models.layers(id, id, &Plain, &Plain, 0, &mut layers, &mut key);
        assert!(!layers.is_empty(), "{id}: no layer");
        for l in &mut layers {
            l.glint = l.glint.max(*glint);
        }
        items.push((k as u64 + 1, layers));
    }
    let animated = items.iter().filter(|(_, l)| l.iter().any(|l| models.baked(l.model).is_some_and(|b| b.animated)));
    assert!(animated.count() >= 2, "scene without animated sprite: proof without object");
    assert!(items.iter().any(|(_, l)| l.iter().any(|l| l.special != 0)), "scene without special model");
    let special = crate::item_special::SpecialAssets::load(&e.assets, &e.data);
    let mut a = ItemAtlas::new(&e.device, &e.queue, models, special);
    a.opts = opts;
    a.fake_now = Some(std::time::Duration::ZERO);
    (a, items)
}

fn read(e: &Env, a: &ItemAtlas) -> Vec<u8> {
    let (tex, _, _) = a.target.as_ref().expect("atlas created");
    let s = a.size;
    let out = e.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(s * s * 4),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = e.device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        tex.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &out,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(s * 4), rows_per_image: Some(s) },
        },
        wgpu::Extent3d { width: s, height: s, depth_or_array_layers: 1 },
    );
    e.queue.submit([enc.finish()]);
    let slice = out.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("atlas read"));
    e.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("GPU wait");
    let px = slice.get_mapped_range().expect("atlas mapped").to_vec();
    out.unmap();
    px
}

/// A frame at instant `ms`: begin, all requested cells twice (identical stacks
/// under the same key, like a chest full of the same enchanted book), atlas pass, its writes
/// through the uploader's ring (`ring`) or through the queue.
/// Returns the number of drawn cells and the cells' rectangles.
fn frame(e: &Env, a: &mut ItemAtlas, items: &[(u64, Vec<Layer>)], ms: u64, ring: bool) -> (usize, Vec<[f32; 4]>) {
    a.fake_now = Some(std::time::Duration::from_millis(ms));
    a.begin(&e.device, 3);
    let uvs = items.iter().map(|(k, l)| a.cell_for(&e.device, *k, l).expect("case")).collect();
    for (k, l) in items {
        a.cell_for(&e.device, *k, l).expect("case");
    }
    let cells = pending_cells(a);
    let mut up = crate::render::upload::Uploader::new(&e.device, ring);
    let mut enc = e.device.create_command_encoder(&Default::default());
    a.encode(&e.device, &e.queue, &mut up, &mut enc, None);
    up.submit(&e.queue, enc.finish());
    (cells, uvs)
}

/// Instants (ms) of the frames and texture ticks played just before each one; frames aligned
/// on the clock of the two paths (all cells created at 0 ms). A tick is played only before
/// a frame where the old clock also redraws: between two beats, `anim_dirty` shows the
/// sprite's new frame up to 4 ms earlier than the old path (only difference, ahead of time).
const SCHEDULE: [(u64, u32); 8] =
    [(0, 0), (1, 0), (300, 7), (301, 0), (305, 1), (900, 20), (1500, 0), (2000, 9)];

/// Atlas read after each frame, cells drawn by each frame, cells' rectangles.
fn run(e: &Env, opts: AtlasOpts, ring: bool) -> (Vec<Vec<u8>>, Vec<usize>, Vec<[f32; 4]>) {
    let (mut a, items) = atlas(e, opts);
    let (mut shots, mut drawn) = (Vec::new(), Vec::new());
    let mut uvs = Vec::new();
    for (ms, ticks) in SCHEDULE {
        for _ in 0..ticks {
            a.tick_textures(&e.queue);
        }
        let (cells, u) = frame(e, &mut a, &items, ms, ring);
        assert!(cells <= items.len(), "{opts:?} at {ms} ms: {cells} cells drawn, one of them twice");
        drawn.push(cells);
        uvs = u;
        shots.push(read(e, &a));
    }
    (shots, drawn, uvs)
}

/// Bytes of cell `uv` in an atlas of side `side`.
fn cell_bytes(px: &[u8], side: u32, uv: [f32; 4]) -> Vec<u8> {
    let f = side as f32;
    let (x0, y0, w, h) = ((uv[0] * f) as usize, (uv[1] * f) as usize, (uv[2] * f) as usize, (uv[3] * f) as usize);
    (y0..y0 + h).flat_map(|y| px[(y * side as usize + x0) * 4..(y * side as usize + x0 + w) * 4].to_vec()).collect()
}

#[test]
fn atlas_paths_render_identical_bytes() {
    let Some(e) = env() else { return };
    let (old, old_drawn, uvs) = run(&e, AtlasOpts::OLD, true);
    let side = 512;
    assert_eq!(old[0].len(), (side * side * 4) as usize);
    // The scene moves: the glint scrolls and the animated sprites change frame.
    let (sword, magma) = (uvs[0], uvs[3]);
    assert_ne!(cell_bytes(&old[1], side, sword), cell_bytes(&old[2], side, sword), "glint motionless");
    let m: Vec<Vec<u8>> = old.iter().map(|s| cell_bytes(s, side, magma)).collect();
    assert!(m.windows(2).any(|w| w[0] != w[1]), "animated sprite motionless");
    // Each switch alone on the old path, then together; `true`: writes through the ring.
    let variants = [
        (AtlasOpts { borrow: true, ..AtlasOpts::OLD }, true),
        (AtlasOpts { glint_clock: true, ..AtlasOpts::OLD }, true),
        (AtlasOpts { anim_dirty: true, ..AtlasOpts::OLD }, true),
        (AtlasOpts { glint_clock: true, anim_dirty: true, ..AtlasOpts::OLD }, true),
        (AtlasOpts { visible_sprites_only: true, ..AtlasOpts::OLD }, true),
        (AtlasOpts { visible_sprites_only: true, ..AtlasOpts::OLD }, false),
        (AtlasOpts { pixel_rects: true, ..AtlasOpts::OLD }, true),
        (AtlasOpts { blend_buffer: true, ..AtlasOpts::OLD }, true),
        (AtlasOpts { indexed: true, ..AtlasOpts::OLD }, true),
        (AtlasOpts::OLD, false),
        (BEFORE_SPRITE_LEADS, true),
        (AtlasOpts::ALL, true),
        (AtlasOpts::ALL, false),
    ];
    let mut failed = Vec::new();
    for (v, ring) in variants {
        let (new, drawn, _) = run(&e, v, ring);
        // `anim_dirty` legitimately skips the redraws without a tick: counts compared without it.
        if !v.anim_dirty && drawn != old_drawn {
            failed.push(format!("{v:?}: cells drawn {drawn:?} instead of {old_drawn:?}"));
        }
        for (i, (a, b)) in old.iter().zip(&new).enumerate() {
            let diff = a.iter().zip(b).filter(|(x, y)| x != y).count();
            if diff != 0 {
                failed.push(format!("{v:?}, ring {ring}: frame {i} ({} ms), {diff} bytes differ", SCHEDULE[i].0));
            }
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// Frames of the hide-and-show proof: instant (ms), texture ticks played just before (200 in all),
/// and whether the cells with an animated sprite are requested by that frame.
const HIDE_AND_SHOW: [(u64, u32, bool); 12] = [
    (0, 0, true),
    (100, 3, true),
    (200, 17, false),
    (300, 40, false),
    (400, 1, true),
    (500, 0, true),
    (600, 25, false),
    (700, 33, true),
    (800, 1, true),
    (900, 60, false),
    (1000, 19, true),
    (1100, 1, true),
];

/// What a hide-and-show run leaves: after each frame, the pixels of the cell of each requested
/// object (in the objects' order), and the bytes of animated frames written into the sheet so far.
struct HideAndShowRun {
    cells: Vec<Vec<Vec<u8>>>,
    sprite_bytes: Vec<u64>,
}

fn hide_and_show(e: &Env, opts: AtlasOpts, ring: bool, skip_stale_sprites: bool) -> HideAndShowRun {
    let (mut a, items) = atlas(e, opts);
    a.sabotage_skip_stale_sprites = skip_stale_sprites;
    let is_animated = |layers: &[Layer]| layers.iter().any(|l| a.baked(l.model).animated);
    let still: Vec<(u64, Vec<Layer>)> = items.iter().filter(|(_, l)| !is_animated(l)).cloned().collect();
    assert!(still.len() + 2 <= items.len(), "scene without cells to hide");
    let mut run = HideAndShowRun { cells: Vec::new(), sprite_bytes: Vec::new() };
    for (ms, ticks, animated_shown) in HIDE_AND_SHOW {
        for _ in 0..ticks {
            a.tick_textures(&e.queue);
        }
        let (_, uvs) = frame(e, &mut a, if animated_shown { &items } else { &still }, ms, ring);
        // Cell by cell: a cell shown again takes any of the freed cells (the order in which the
        // slot map gives them back is not fixed), so whole atlases cannot be compared.
        let shot = read(e, &a);
        run.cells.push(uvs.iter().map(|&uv| cell_bytes(&shot, a.size, uv)).collect());
        run.sprite_bytes.push(a.uploaded_sprite_bytes);
    }
    run
}

/// 200 texture ticks with the animated cells shown, hidden, shown again: writing a sprite only
/// when a drawn cell reads it leaves the same pixels in every requested cell, frame after frame,
/// as writing every changed sprite on every tick; hidden, the animated sprites cost no write.
/// Sabotage: stale sprites left unwritten when their cell is drawn change the cells.
#[test]
fn sprites_written_when_read_leave_the_same_cells() {
    let Some(e) = env() else { return };
    let differing_frames = |one: &HideAndShowRun, other: &HideAndShowRun| -> Vec<usize> {
        (0..HIDE_AND_SHOW.len()).filter(|&i| one.cells[i] != other.cells[i]).collect()
    };
    // The lead on each family of the other switches; `true`: writes through the ring.
    for (others, ring) in [(AtlasOpts::ALL, true), (AtlasOpts::ALL, false), (AtlasOpts::OLD, true)] {
        let eager = hide_and_show(&e, AtlasOpts { visible_sprites_only: false, ..others }, ring, false);
        let lazy = hide_and_show(&e, AtlasOpts { visible_sprites_only: true, ..others }, ring, false);
        assert!(eager.cells.windows(2).any(|w| w[0].len() == w[1].len() && w[0] != w[1]), "motionless scene");
        assert_eq!(differing_frames(&eager, &lazy), Vec::<usize>::new(), "{others:?}, ring {ring}: frames that differ");
        for (i, (_, ticks, animated_shown)) in HIDE_AND_SHOW.iter().enumerate().skip(1) {
            let written = lazy.sprite_bytes[i] - lazy.sprite_bytes[i - 1];
            if !animated_shown {
                assert_eq!(written, 0, "{others:?}: frame {i}, sprites written with their cells hidden");
            } else if *ticks > 0 {
                assert!(written > 0, "{others:?}: frame {i}, no sprite written for the cells shown again");
            }
        }
        let (eager_total, lazy_total) = (eager.sprite_bytes[11], lazy.sprite_bytes[11]);
        eprintln!("{others:?}, ring {ring}: sprite bytes written over 200 ticks {eager_total} -> {lazy_total}");
        assert!(lazy_total * 4 < eager_total, "written when read: {lazy_total} bytes against {eager_total}");
        let sabotaged = hide_and_show(&e, AtlasOpts { visible_sprites_only: true, ..others }, ring, true);
        assert!(!differing_frames(&eager, &sabotaged).is_empty(), "{others:?}: sabotage not detected");
    }
}

/// Atlas passes over 1 s at 2400 frames/s (0.4 ms per frame) when 14 glint cells
/// appear one per frame: the old per-cell clock makes almost one per frame,
/// the shared clock one per 4 ms. Cells with an animated sprite alone only count at ticks.
#[test]
fn glint_clock_counts_passes() {
    let Some(e) = env() else { return };
    let count = |opts: AtlasOpts, glint: bool| {
        let (mut a, mut items) = atlas(&e, opts);
        if !glint {
            items.retain(|(_, l)| l.iter().all(|l| l.glint == 0));
        }
        let mut shown: Vec<(u64, Vec<Layer>)> = Vec::new();
        let mut up = crate::render::upload::Uploader::new(&e.device, true);
        let (mut passes, mut tick) = (0, 0);
        for f in 0..2400u64 {
            let us = f * 400 + 1;
            if let Some(src) = items.get((f as usize) % items.len()).filter(|_| f < 14) {
                shown.push((1000 + f, src.1.clone()));
            }
            if us / 50_000 > tick {
                tick = us / 50_000;
                a.tick_textures(&e.queue);
            }
            a.fake_now = Some(std::time::Duration::from_micros(us));
            a.begin(&e.device, 3);
            for (k, l) in &shown {
                a.cell_for(&e.device, *k, l);
            }
            passes += u32::from(!a.clears.is_empty());
            let mut enc = e.device.create_command_encoder(&Default::default());
            a.encode(&e.device, &e.queue, &mut up, &mut enc, None);
            up.submit(&e.queue, enc.finish());
            if f % 100 == 99 {
                e.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("GPU wait");
            }
        }
        passes
    };
    let (old, clock) = (count(AtlasOpts::OLD, true), count(AtlasOpts { glint_clock: true, ..AtlasOpts::OLD }, true));
    let (anim_old, anim_dirty) =
        (count(AtlasOpts::OLD, false), count(AtlasOpts { anim_dirty: true, ..AtlasOpts::OLD }, false));
    eprintln!("atlas passes / s: glints {old} -> {clock}; animated sprites alone {anim_old} -> {anim_dirty}");
    assert!(clock <= 260, "shared clock: {clock} passes (one per 4 ms expected)");
    assert!(clock * 2 < old, "shared clock without effect: {old} -> {clock}");
    assert!(anim_dirty <= 14 + 20, "animated sprites: {anim_dirty} passes (at ticks only expected)");
}

/// Micro-bench (ignored by default): CPU time of the atlas redraws, old path against
/// each switch, 2000 frames of 8 cells each redrawn (`--ignored --nocapture`).
#[test]
#[ignore]
fn bench_redraw() {
    let Some(e) = env() else { return };
    for (name, opts) in [
        ("old", AtlasOpts::OLD),
        ("borrow", AtlasOpts { borrow: true, ..AtlasOpts::OLD }),
        ("clock", AtlasOpts { glint_clock: true, ..AtlasOpts::OLD }),
        ("dirty", AtlasOpts { anim_dirty: true, ..AtlasOpts::OLD }),
        ("all before the sprite and index leads", BEFORE_SPRITE_LEADS),
        ("indexed", AtlasOpts { indexed: true, ..BEFORE_SPRITE_LEADS }),
        ("all", AtlasOpts::ALL),
        ("old", AtlasOpts::OLD),
    ] {
        let (mut a, items) = atlas(&e, opts);
        let mut best = f64::MAX;
        for pass in 0..3u64 {
            let t = std::time::Instant::now();
            for f in 0..2000u64 {
                // 5 ms per frame: all the glint cells due on every frame.
                a.fake_now = Some(std::time::Duration::from_millis((pass * 2000 + f) * 5));
                a.tick_textures(&e.queue);
                a.begin(&e.device, 3);
                for (k, l) in &items {
                    a.cell_for(&e.device, *k, l);
                }
                a.clears.clear();
                a.solids.clear();
                a.blended.clear();
                a.sprites_to_upload.clear();
                if f % 100 == 99 {
                    e.queue.submit([]);
                }
            }
            best = best.min(t.elapsed().as_secs_f64() * 1e6 / 2000.0);
        }
        eprintln!("{name}: {best:.2} µs per frame (1 core, main thread)");
    }
}

/// Micro-bench (ignored by default, `--ignored --nocapture`): texture ticks and atlas passes with
/// the sprites of every item of the game in the sheet, like in play. 2,000 rounds of one tick
/// then one frame; the frame requests the 8 objects of the scene (`shown`) or only the ones
/// without an animated sprite (`hidden`, the usual case: a hotbar without magma or sea lantern).
/// Times of the main thread, 1 core (GPU-queue work, nothing to spread over cores); best of 3.
#[test]
#[ignore]
fn bench_ticks_and_passes() {
    let Some(e) = env() else { return };
    let all_models = || ItemModels::load(&e.assets, e.data.items.iter().map(|i| i.model.as_str()));
    const ROUNDS: u64 = 2000;
    for (name, opts, ring) in [
        ("before", BEFORE_SPRITE_LEADS, true),
        ("pixel rectangles", AtlasOpts { pixel_rects: true, ..BEFORE_SPRITE_LEADS }, true),
        ("blend buffer", AtlasOpts { blend_buffer: true, ..BEFORE_SPRITE_LEADS }, true),
        ("visible sprites only", AtlasOpts { visible_sprites_only: true, ..BEFORE_SPRITE_LEADS }, true),
        ("visible sprites only, queue", AtlasOpts { visible_sprites_only: true, ..BEFORE_SPRITE_LEADS }, false),
        ("indexed", AtlasOpts { indexed: true, ..BEFORE_SPRITE_LEADS }, true),
        ("all", AtlasOpts::ALL, true),
        ("all, queue", AtlasOpts::ALL, false),
        ("before", BEFORE_SPRITE_LEADS, true),
    ] {
        for animated_shown in [false, true] {
            let (mut a, items) = atlas_with_models(&e, opts, all_models());
            let is_animated = |layers: &[Layer]| layers.iter().any(|l| a.baked(l.model).animated);
            let requested: Vec<(u64, Vec<Layer>)> =
                items.iter().filter(|(_, l)| animated_shown || !is_animated(l)).cloned().collect();
            let (animated, blended) =
                (a.animated_sprites.len(), a.animated_sprites.iter().filter(|s| s.playback.is_blended()).count());
            let mut up = crate::render::upload::Uploader::new(&e.device, ring);
            let (mut best_tick, mut best_frame) = (f64::MAX, f64::MAX);
            let mut vertex_bytes = 0;
            a.uploaded_sprite_bytes = 0;
            for pass in 0..3u64 {
                let (mut in_ticks, mut in_frames) = (std::time::Duration::ZERO, std::time::Duration::ZERO);
                for round in 0..ROUNDS {
                    let start = std::time::Instant::now();
                    a.tick_textures(&e.queue);
                    let ticked = std::time::Instant::now();
                    // 5 ms per frame: all the glint cells due on every frame.
                    a.fake_now = Some(std::time::Duration::from_millis((pass * ROUNDS + round) * 5));
                    a.begin(&e.device, 3);
                    for (k, l) in &requested {
                        a.cell_for(&e.device, *k, l);
                    }
                    let mut enc = e.device.create_command_encoder(&Default::default());
                    a.encode(&e.device, &e.queue, &mut up, &mut enc, None);
                    vertex_bytes = a.all.len() * std::mem::size_of::<Vertex>();
                    up.submit(&e.queue, enc.finish());
                    in_ticks += ticked - start;
                    in_frames += ticked.elapsed();
                    if round % 100 == 99 {
                        e.device
                            .poll(wgpu::PollType::Wait { submission_index: None, timeout: None })
                            .expect("GPU wait");
                    }
                }
                best_tick = best_tick.min(in_ticks.as_secs_f64() * 1e6 / ROUNDS as f64);
                best_frame = best_frame.min(in_frames.as_secs_f64() * 1e6 / ROUNDS as f64);
            }
            let shown = if animated_shown { "shown" } else { "hidden" };
            eprintln!(
                "{name}, animated cells {shown}: tick {best_tick:.2} µs, frame {best_frame:.2} µs, \
                 {} sprite bytes per tick, {vertex_bytes} vertex bytes per pass \
                 ({animated} animated sprites, {blended} blended)",
                a.uploaded_sprite_bytes / (3 * ROUNDS),
            );
        }
    }
}
