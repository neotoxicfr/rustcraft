//! Allocations of the evaluation: zero per frame on the new path (`RUSTCRAFT_OPT_ENV_NOALLOC`),
//! at least one per call on the old one (proof that the counter does see allocations). Per-thread
//! counter: the other tests in the binary do not skew it.
//!
//! Micro-bench (ignored by default, 1 core: this work is sequential, once per frame):
//! `cargo-file.ps1 test --release -p rc-env --test noalloc -- --ignored --nocapture`.

// Counting allocator: delegates to the system (same precedent as `apps/client/src/alloc_trace.rs`).
#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::time::Instant;

use rc_env::{Attr, Env, EnvOpts, FogInput, FogKind, Probe, fog};

struct Counting;

thread_local! {
    static COUNT: Cell<u64> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        COUNT.with(|c| c.set(c.get() + 1));
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        COUNT.with(|c| c.set(c.get() + 1));
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static A: Counting = Counting;

fn allocs() -> u64 {
    COUNT.with(Cell::get)
}

fn input(pt: f32) -> FogInput {
    FogInput {
        kind: FogKind::Atmospheric,
        pt,
        render_chunks: 12,
        eye_y: 70.0,
        min_y: -64,
        flat: false,
        forward_x: 0.7,
        water_vision: 0.0,
        sky_light: 15,
        biome_rain: true,
        dt_ticks: 0.4,
        blindness: None,
        darkness_blend: None,
        boss_darken: 0.0,
        boss_fog: false,
        night_vision: 0.0,
        spectator: false,
        fire_resistance: false,
        cloud_range: 128,
    }
}

/// A "frame": what `environ::frame`, the lightmap and the shadows read in `rc-env`.
fn frame(env: &mut Env, probe: &mut Probe, rain_fog: &mut f32, pt: f32) -> f32 {
    let f = fog(env, probe, rain_fog, &input(pt));
    let mut acc = f.env_end + f.color[0];
    for a in [Attr::SkyColor, Attr::SunAngle, Attr::MoonAngle, Attr::StarAngle, Attr::CloudColor] {
        acc += probe.get(env, a, pt)[0];
    }
    acc += env.eval_global(Attr::SkyLightLevel)[0];
    acc += env.eval_global(Attr::SkyLightLevel)[0];
    acc += env.eval_global(Attr::SunAngle)[0];
    acc
}

fn setup(opts: EnvOpts) -> Option<(Env, Probe)> {
    let d = rc_testdata::version_data()?;
    let biomes = vec!["minecraft:plains".to_owned(), "minecraft:desert".to_owned()];
    let mut env = Env::new(&d, "minecraft:overworld", &biomes);
    env.opts = opts;
    env.clocks.set_time(0, &[("minecraft:overworld".into(), 6000, 0.0, 1.0)]);
    let mut probe = Probe::default();
    probe.tick(vec![(0, 3000.0), (1, 1096.0)]);
    Some((env, probe))
}

#[test]
fn new_path_allocates_nothing_per_frame() {
    let Some((mut env, mut probe)) = setup(EnvOpts::NEW) else { return };
    let mut rain_fog = 0.0;
    // First frame: birth of the clocks, memos filled.
    frame(&mut env, &mut probe, &mut rain_fog, 0.0);
    let before = allocs();
    let mut sink = 0.0;
    for _ in 0..200 {
        env.clocks.tick();
        // The probe receives its weights outside the frame (client tick): reuses an existing Vec.
        let w = std::mem::take(&mut probe.weights);
        probe.tick(w);
        for k in 0..5 {
            sink += frame(&mut env, &mut probe, &mut rain_fog, k as f32 / 5.0);
            sink += env.eval(Attr::FogColor, &[(0, 4096.0)])[0];
        }
    }
    assert!(sink.is_finite());
    assert_eq!(allocs() - before, 0, "allocations on the new path");

    // Old path: the counter sees the allocations (otherwise this test would prove nothing).
    let Some((mut env, mut probe)) = setup(EnvOpts::OLD) else { return };
    frame(&mut env, &mut probe, &mut rain_fog, 0.0);
    let before = allocs();
    for _ in 0..10 {
        env.clocks.tick();
        sink += frame(&mut env, &mut probe, &mut rain_fog, 0.5);
    }
    assert!(sink.is_finite());
    assert!(allocs() - before >= 10, "the old path should allocate ({})", allocs() - before);
}

/// Audio attributes of the tick (`RUSTCRAFT_OPT_SOUNDIDS`): borrowed, zero allocations; copied
/// (old path), several per tick. Figures: bench, loaded machine.
#[test]
fn audio_attributes_borrowed_allocate_nothing() {
    let Some(d) = rc_testdata::version_data() else { return };
    let biomes = vec!["minecraft:crimson_forest".to_owned(), "minecraft:plains".to_owned()];
    let env = Env::new(&d, "minecraft:overworld", &biomes);
    let mut sink = 0usize;
    let before = allocs();
    for i in 0..1000u32 {
        let a = env.ambient_sounds_ref(i % 2);
        sink += a.additions.len() + a.loop_sound.as_ref().map_or(0, String::len);
        sink += env.background_music_ref(i % 2).default.as_ref().map_or(0, |t| t.sound.len());
    }
    assert_eq!(allocs() - before, 0, "allocations of borrowed reads");
    let before = allocs();
    let t = Instant::now();
    for i in 0..1000u32 {
        let a = env.ambient_sounds(i % 2);
        sink += a.additions.len() + a.loop_sound.as_ref().map_or(0, String::len);
        sink += env.background_music(i % 2).default.as_ref().map_or(0, |t| t.sound.len());
    }
    let copied = allocs() - before;
    println!("copy of audio attributes: {:.1} alloc/tick, {:.0} ns/tick", copied as f64 / 1000.0, t.elapsed().as_nanos() as f64 / 1000.0);
    assert!(copied >= 3000, "the old path should allocate ({copied})");
    assert!(sink > 0);
}

fn bench(label: &str, opts: EnvOpts) {
    let Some((mut env, mut probe)) = setup(opts) else { return };
    let mut rain_fog = 0.0;
    let mut sink = 0.0;
    // Global reads only (3 per frame in the client), then a full frame.
    let n = 2_000_000u32;
    let t = Instant::now();
    for i in 0..n {
        if i.is_multiple_of(64) {
            env.clocks.tick();
        }
        sink += env.eval_global(Attr::SkyLightLevel)[0];
    }
    let global = t.elapsed().as_nanos() as f64 / f64::from(n);
    let t = Instant::now();
    for _ in 0..n {
        sink += env.eval(Attr::SkyColor, &[(0, 3000.0), (1, 1096.0)])[0];
    }
    let eval = t.elapsed().as_nanos() as f64 / f64::from(n);
    let m = 200_000u32;
    let t = Instant::now();
    for i in 0..m {
        if i.is_multiple_of(64) {
            env.clocks.tick();
            let w = std::mem::take(&mut probe.weights);
            probe.tick(w);
        }
        sink += frame(&mut env, &mut probe, &mut rain_fog, (i % 64) as f32 / 64.0);
    }
    let img = t.elapsed().as_nanos() as f64 / f64::from(m);
    let a0 = allocs();
    frame(&mut env, &mut probe, &mut rain_fog, 0.3);
    let per_frame = allocs() - a0;
    println!(
        "{label:>7}: eval_global {global:6.1} ns, eval (2 biomes) {eval:6.1} ns, frame {img:7.1} ns, {per_frame} alloc/frame (sum {sink})"
    );
}

#[test]
#[ignore = "micro-bench"]
fn microbench_eval_and_fog() {
    for _ in 0..3 {
        bench("old", EnvOpts::OLD);
        bench("noalloc", EnvOpts { noalloc: true, keycache: false, fog_memo: false });
        bench("keycache", EnvOpts { noalloc: false, keycache: true, fog_memo: false });
        bench("fogmemo", EnvOpts { noalloc: false, keycache: false, fog_memo: true });
        bench("without fog", EnvOpts { noalloc: true, keycache: true, fog_memo: false });
        bench("new", EnvOpts::NEW);
    }
}
