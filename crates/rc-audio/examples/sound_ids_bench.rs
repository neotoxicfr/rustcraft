//! Micro-bench of sound launching (C4-M24): variant draw (old recursive versus
//! indexed, in the same process), then full launch by the engine, warm PCM cache, on
//! a mix of village and crowd sounds. Allocations counted on the main thread.
//! The launch follows `RUSTCRAFT_OPT_SOUNDIDS` (run twice, 0 then 1).
//! `cargo-file.ps1 run --release -p rc-audio --example sound_ids_bench`
//! Main thread only: a sound is launched where it is requested, nothing to split up.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rc_audio::{AudioEngine, Category, JavaRandom, SoundTable};

struct Counting;

thread_local! {
    static COUNT: Cell<u64> = const { Cell::new(0) };
}

// Bench only: delegates everything to the system allocator, counts the current thread's allocations.
#[allow(unsafe_code)]
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

/// Sounds of a village and crowd scene (mobs, steps, bell, doors).
const MIX: [&str; 12] = [
    "entity.villager.ambient",
    "entity.villager.yes",
    "entity.chicken.ambient",
    "entity.cow.ambient",
    "entity.pig.ambient",
    "entity.sheep.ambient",
    "entity.zombie.ambient",
    "entity.skeleton.step",
    "block.grass.step",
    "block.wooden_door.open",
    "block.bell.use",
    "entity.generic.eat",
];

fn main() {
    let install = rc_data::locate("26.3").expect("installation 26.3");
    let index = install.asset_index();
    let assets = rc_assets::Assets::for_install(
        &install.client_jar,
        &[],
        index.as_ref().map(|(i, o)| (i.as_path(), o.as_path())),
    )
    .expect("resources");
    let assets = Arc::new(assets);
    let t = Instant::now();
    let table = SoundTable::load(&[&assets]);
    let load_ms = t.elapsed().as_secs_f64() * 1e3;
    let t = Instant::now();
    table.prepare();
    println!("table: read {load_ms:.1} ms, compilation {:.2} ms", t.elapsed().as_secs_f64() * 1e3);

    // 1. Draw only, all events.
    let mut ids: Vec<String> = table.ids().map(str::to_owned).collect();
    ids.sort_unstable();
    for round in 0..3 {
        let mut sink = 0usize;
        let (a0, t) = (allocs(), Instant::now());
        for (k, id) in ids.iter().enumerate() {
            for s in 0..200 {
                let mut rng = JavaRandom::new((k * 200 + s) as i64);
                sink += table.pick(id, &mut rng).map_or(0, |r| r.path.len());
            }
        }
        let n = (ids.len() * 200) as f64;
        let (old_ns, old_a) = (t.elapsed().as_secs_f64() * 1e9 / n, (allocs() - a0) as f64 / n);
        let (a0, t) = (allocs(), Instant::now());
        for (k, id) in ids.iter().enumerate() {
            for s in 0..200 {
                let mut rng = JavaRandom::new((k * 200 + s) as i64);
                sink += table.pick_fast(id, &mut rng).map_or(0, |r| r.file as usize);
            }
        }
        let (new_ns, new_a) = (t.elapsed().as_secs_f64() * 1e9 / n, (allocs() - a0) as f64 / n);
        println!(
            "draw {round}: recursive {old_ns:.1} ns ({old_a:.2} alloc), indexed {new_ns:.1} ns ({new_a:.2} alloc) [{sink}]"
        );
    }

    // 2. Full launch, warm cache.
    let (mut e, mut m) = AudioEngine::headless(assets.clone(), table, 48_000);
    let mut out = vec![0.0f32; 2 * 480];
    for _ in 0..3 {
        for ev in MIX {
            e.play(ev, Category::Neutral, Some([1.0, 64.0, 1.0]), 1.0, 1.0, None);
        }
        let t = Instant::now();
        while t.elapsed() < Duration::from_millis(1500) {
            m.process(&mut out);
            e.tick();
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    println!("cache: {} sounds, {:.1} MB", e.cache_len(), e.cache_bytes() as f64 / 1e6);
    let ids = rc_audio::sound_ids_enabled();
    for round in 0..3 {
        let (mut plays, mut started, mut allocated, mut spent) = (0u64, 0u64, 0u64, Duration::ZERO);
        for tick in 0..2000u32 {
            let a0 = allocs();
            let t = Instant::now();
            for k in 0..3u32 {
                let ev = MIX[((tick * 3 + k) % MIX.len() as u32) as usize];
                let pos = [f64::from(k) * 2.0, 64.0, f64::from(tick % 16)];
                started += u64::from(e.play(ev, Category::Neutral, Some(pos), 1.0, 1.0, Some(i64::from(tick * 3 + k))).is_some());
                plays += 1;
            }
            spent += t.elapsed();
            allocated += allocs() - a0;
            m.process(&mut out);
            e.tick();
        }
        println!(
            "launch {round} (SOUNDIDS={}): {:.0} ns/sound, {:.2} alloc/sound, {started}/{plays} launched",
            u8::from(ids),
            spent.as_secs_f64() * 1e9 / plays as f64,
            allocated as f64 / plays as f64
        );
    }
}
