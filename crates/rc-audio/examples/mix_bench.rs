//! CPU cost of the mixer (device excluded): N looping voices, varied positions and pitches,
//! all in range, then half of them out of range (zero gain). Then the same mix split across
//! several threads (rayon, a share of the voices per mixer) and Vorbis decoding (µs and bytes
//! allocated per sound).
//! `cargo run --release -p rc-audio --example mix_bench [mix|decode] [rayon]`
//! Switches: `RUSTCRAFT_OPT_MIXMUTE`, `MIXWEIGHTS`, `MIXSIMD`, `DECODECOPY` (0 = old).

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use rayon::prelude::*;
use rc_audio::{AudioEngine, Category, SoundRequest, SoundTable};

/// Counter of allocated bytes (decoding).
struct Counting;

static ALLOCATED: AtomicUsize = AtomicUsize::new(0);

// Bench only: delegates everything to the system allocator, counts the requested bytes.
#[allow(unsafe_code)]
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCATED.fetch_add(l.size(), Ordering::Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        ALLOCATED.fetch_add(new, Ordering::Relaxed);
        unsafe { System.realloc(p, l, new) }
    }
}

#[global_allocator]
static A: Counting = Counting;

const RATE: u32 = 48_000;
const BLOCK: usize = 480; // 10 ms
const SECONDS: usize = 5;
const EVENTS: [&str; 6] = [
    "block.note_block.harp",
    "entity.experience_orb.pickup",
    "block.stone.break",
    "entity.zombie.ambient",
    "ambient.cave",
    "block.fire.ambient",
];

/// Engine and mixer without a device, with `n` looping voices (indices `first..first + n`);
/// every other voice out of range if `far`.
fn scene(assets: &Arc<rc_assets::Assets>, first: usize, n: usize, far: bool) -> (AudioEngine, rc_audio::mixer::Mixer) {
    let (mut engine, mut mixer) = AudioEngine::headless(assets.clone(), SoundTable::load(&[&**assets]), RATE);
    engine.set_listener([0.0; 3], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]);
    for i in first..first + n {
        let a = i as f64 * 0.7;
        let r = if far && i % 2 == 1 { 40.0 } else { 6.0 };
        let req = SoundRequest::new(EVENTS[i % EVENTS.len()], Category::Block)
            .at([a.cos() * r, 0.0, a.sin() * r])
            .pitch(0.5 + (i % 7) as f32 * 0.25)
            .seed(Some(i as i64))
            .looping(0);
        engine.play_request(req);
    }
    let mut out = vec![0.0f32; 2 * BLOCK];
    let wait = Instant::now();
    while mixer.active() < n && wait.elapsed() < Duration::from_secs(20) {
        engine.tick();
        mixer.process(&mut out);
        std::thread::sleep(Duration::from_millis(1));
    }
    (engine, mixer)
}

fn mix(assets: &Arc<rc_assets::Assets>) {
    let blocks = SECONDS * RATE as usize / BLOCK;
    let mut out = vec![0.0f32; 2 * BLOCK];
    for far in [false, true] {
        for &n in &[1usize, 16, 64, 128, 247] {
            let (mut engine, mut mixer) = scene(assets, 0, n, far);
            let t = Instant::now();
            let mut worst = Duration::ZERO;
            for _ in 0..blocks {
                let b = Instant::now();
                mixer.process(&mut out);
                worst = worst.max(b.elapsed());
            }
            let el = t.elapsed();
            println!(
                "{n:>3} voices{} ({} active): {:.3} % of a core, {:.2} µs / 10 ms block (worst {:.0} µs), {:.2} ns / voice·frame",
                if far { ", half out of range" } else { "" },
                mixer.active(),
                el.as_secs_f64() / SECONDS as f64 * 100.0,
                el.as_secs_f64() * 1e6 / blocks as f64,
                worst.as_secs_f64() * 1e6,
                el.as_secs_f64() * 1e9 / (blocks * BLOCK * mixer.active().max(1)) as f64,
            );
            if n == 247 && !far {
                // Normal reserve full: the next sound is dropped.
                let extra = engine.play("block.stone.break", Category::Block, None, 1.0, 1.0, None);
                println!("PCM cache: {:.1} MiB; 248th sound: {extra:?}", engine.cache_bytes() as f64 / 1048576.0);
            }
        }
    }
    if std::env::args().nth(2).as_deref() != Some("rayon") {
        return;
    }
    // Multi-core: 247 voices split across k mixers rendered in parallel (rayon), summed.
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut k = 1;
    while k <= threads.min(16) {
        let per = 247 / k;
        let mut parts: Vec<_> = (0..k).map(|j| scene(assets, j * per, if j + 1 == k { 247 - j * per } else { per }, false)).collect();
        let mut outs = vec![vec![0.0f32; 2 * BLOCK]; k];
        let t = Instant::now();
        let mut worst = Duration::ZERO;
        for _ in 0..blocks {
            let b = Instant::now();
            parts.par_iter_mut().zip(outs.par_iter_mut()).for_each(|((_, m), o)| m.process(o));
            for (i, s) in out.iter_mut().enumerate() {
                *s = outs.iter().map(|o| o[i]).sum();
            }
            worst = worst.max(b.elapsed());
        }
        let el = t.elapsed();
        println!(
            "rayon {k:>2} mixers (247 voices): {:.2} µs / block (worst {:.0} µs)",
            el.as_secs_f64() * 1e6 / blocks as f64,
            worst.as_secs_f64() * 1e6
        );
        k *= 2;
    }
}

fn decode(assets: &rc_assets::Assets) {
    let old = rc_data::env_str!("RUSTCRAFT_OPT_DECODECOPY") == Some("0");
    let mut all = assets.list("assets/minecraft/sounds/", ".ogg");
    all.sort();
    let files: Vec<Vec<u8>> = all.iter().filter(|p| !p.contains("/music/") && !p.contains("/records/")).filter_map(|p| assets.read(p)).collect();
    let (mut ns, mut bytes, mut samples, mut cap) = (0u128, 0usize, 0usize, 0usize);
    for f in &files {
        let f = f.clone();
        let a0 = ALLOCATED.load(Ordering::Relaxed);
        let t = Instant::now();
        let pcm = if old {
            rc_audio::decode::decode_all(Arc::<[u8]>::from(f)).unwrap()
        } else {
            rc_audio::decode::decode_all(f).unwrap()
        };
        ns += t.elapsed().as_nanos();
        bytes += ALLOCATED.load(Ordering::Relaxed) - a0;
        samples += pcm.data.len();
        cap += pcm.data.capacity();
    }
    let n = files.len().max(1);
    println!(
        "decoding ({}): {n} sounds, {:.0} µs / sound, {:.2} ns / sample, {:.0} KiB allocated / sound, PCM kept {:.1} MiB (capacity {:.1} MiB)",
        if old { "old" } else { "new" },
        ns as f64 / 1e3 / n as f64,
        ns as f64 / samples.max(1) as f64,
        bytes as f64 / 1024.0 / n as f64,
        samples as f64 * 2.0 / 1048576.0,
        cap as f64 * 2.0 / 1048576.0,
    );
}

fn main() {
    let install = rc_data::locate("26.3").expect("installation 26.3");
    let index = install.asset_index();
    let assets = rc_assets::Assets::for_install(
        &install.client_jar,
        &[],
        index.as_ref().map(|(i, o)| (i.as_path(), o.as_path())),
    )
    .expect("resources");
    let t = Instant::now();
    let table = SoundTable::load(&[&assets]);
    println!("sounds.json: {} events in {:.0} ms", table.len(), t.elapsed().as_secs_f64() * 1e3);
    let assets = Arc::new(assets);
    let what = std::env::args().nth(1).unwrap_or_default();
    if what != "decode" {
        mix(&assets);
    }
    if what != "mix" {
        decode(&assets);
    }
}
