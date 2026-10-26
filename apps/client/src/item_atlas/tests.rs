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

fn atlas(e: &Env, opts: AtlasOpts) -> (ItemAtlas, Vec<(u64, Vec<Layer>)>) {
    let models = ItemModels::load(&e.assets, ITEMS.iter().map(|i| i.0));
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
/// under the same key, like a chest full of the same enchanted book), atlas pass.
/// Returns the number of clear vertices (6 per drawn cell) and the cells' rectangles.
fn frame(e: &Env, a: &mut ItemAtlas, items: &[(u64, Vec<Layer>)], ms: u64) -> (usize, Vec<[f32; 4]>) {
    a.fake_now = Some(std::time::Duration::from_millis(ms));
    a.begin(&e.device, 3);
    let uvs = items.iter().map(|(k, l)| a.cell_for(&e.device, *k, l).expect("case")).collect();
    for (k, l) in items {
        a.cell_for(&e.device, *k, l).expect("case");
    }
    let clears = a.clears.len();
    let mut enc = e.device.create_command_encoder(&Default::default());
    a.encode(&e.device, &e.queue, &mut enc, None);
    e.queue.submit([enc.finish()]);
    (clears, uvs)
}

/// Instants (ms) of the frames and texture ticks played just before each one; frames aligned
/// on the clock of the two paths (all cells created at 0 ms). A tick is played only before
/// a frame where the old clock also redraws: between two beats, `anim_dirty` shows the
/// sprite's new frame up to 4 ms earlier than the old path (only difference, ahead of time).
const SCHEDULE: [(u64, u32); 8] =
    [(0, 0), (1, 0), (300, 7), (301, 0), (305, 1), (900, 20), (1500, 0), (2000, 9)];

/// Atlas read after each frame, clear vertices of each frame, cells' rectangles.
fn run(e: &Env, opts: AtlasOpts) -> (Vec<Vec<u8>>, Vec<usize>, Vec<[f32; 4]>) {
    let (mut a, items) = atlas(e, opts);
    let (mut shots, mut clears) = (Vec::new(), Vec::new());
    let mut uvs = Vec::new();
    for (ms, ticks) in SCHEDULE {
        for _ in 0..ticks {
            a.tick_textures(&e.queue);
        }
        let (c, u) = frame(e, &mut a, &items, ms);
        assert!(c <= 6 * items.len(), "{opts:?} at {ms} ms: {c} clear vertices, a cell drawn twice");
        clears.push(c);
        uvs = u;
        shots.push(read(e, &a));
    }
    (shots, clears, uvs)
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
    let (old, old_clears, uvs) = run(&e, AtlasOpts::OLD);
    let side = 512;
    assert_eq!(old[0].len(), (side * side * 4) as usize);
    // The scene moves: the glint scrolls and the animated sprites change frame.
    let (sword, magma) = (uvs[0], uvs[3]);
    assert_ne!(cell_bytes(&old[1], side, sword), cell_bytes(&old[2], side, sword), "glint motionless");
    let m: Vec<Vec<u8>> = old.iter().map(|s| cell_bytes(s, side, magma)).collect();
    assert!(m.windows(2).any(|w| w[0] != w[1]), "animated sprite motionless");
    let variants = [
        AtlasOpts { borrow: true, ..AtlasOpts::OLD },
        AtlasOpts { glint_clock: true, ..AtlasOpts::OLD },
        AtlasOpts { anim_dirty: true, ..AtlasOpts::OLD },
        AtlasOpts { glint_clock: true, anim_dirty: true, ..AtlasOpts::OLD },
        AtlasOpts::ALL,
    ];
    let mut failed = Vec::new();
    for v in variants {
        let (new, clears, _) = run(&e, v);
        // `anim_dirty` legitimately skips the redraws without a tick: counts compared without it.
        if !v.anim_dirty && clears != old_clears {
            failed.push(format!("{v:?}: clear vertices {clears:?} instead of {old_clears:?}"));
        }
        for (i, (a, b)) in old.iter().zip(&new).enumerate() {
            let diff = a.iter().zip(b).filter(|(x, y)| x != y).count();
            if diff != 0 {
                failed.push(format!("{v:?}: frame {i} ({} ms), {diff} bytes differ", SCHEDULE[i].0));
            }
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
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
            a.encode(&e.device, &e.queue, &mut enc, None);
            e.queue.submit([enc.finish()]);
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
                if f % 100 == 99 {
                    e.queue.submit([]);
                }
            }
            best = best.min(t.elapsed().as_secs_f64() * 1e6 / 2000.0);
        }
        eprintln!("{name}: {best:.2} µs per frame (1 core, main thread)");
    }
}
