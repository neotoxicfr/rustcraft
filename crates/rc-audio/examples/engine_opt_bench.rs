//! Micro-benches of the engine paths (main thread): per-frame listener, device listing,
//! device change. The measured path follows the switches
//! `RUSTCRAFT_OPT_AUDIOLISTENER`, `AUDIOREOPEN`, `AUDIOFOND` (run twice, 0 then 1).
//! `cargo run --release -p rc-audio --example engine_opt_bench`

use std::sync::Arc;
use std::time::{Duration, Instant};

use rc_audio::{AudioEngine, Category, SoundRequest, SoundTable};

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

    // Listener: 1,000,000 frames, one mixed block every 35 frames (3,500 fps, 100 blocks/s).
    let (mut e, mut m) = AudioEngine::headless(assets.clone(), SoundTable::load(&[&assets]), 48_000);
    for i in 0..32 {
        let req = SoundRequest::new("block.fire.ambient", Category::Block).at([f64::from(i), 0.0, 0.0]).looping(0);
        e.play_request(req);
    }
    let mut out = vec![0.0f32; 2 * 480];
    let (mut main_t, mut mix_t) = (Duration::ZERO, Duration::ZERO);
    for f in 0..1_000_000u32 {
        let a = f as f32 * 1e-4;
        let t = Instant::now();
        e.set_listener([f64::from(f) * 1e-3, 64.0, 0.0], [a.sin(), 0.0, -a.cos()], [0.0, 1.0, 0.0]);
        main_t += t.elapsed();
        if f % 35 == 0 {
            let t = Instant::now();
            m.process(&mut out);
            mix_t += t.elapsed();
        }
    }
    println!(
        "listener: {:.1} ns/frame (main thread), {:.2} µs/block (audio thread, 32 voices)",
        main_t.as_secs_f64() * 1e9 / 1e6,
        mix_t.as_secs_f64() * 1e6 / (1e6 / 35.0)
    );

    // Device listing: full list (names) versus identifiers only.
    let n = 20;
    let t = Instant::now();
    for _ in 0..n {
        std::hint::black_box(rc_audio::devices::DeviceList::query());
    }
    let full = t.elapsed().as_secs_f64() * 1e3 / f64::from(n);
    let t = Instant::now();
    for _ in 0..n {
        std::hint::black_box(rc_audio::devices::output_ids_count());
    }
    let ids = t.elapsed().as_secs_f64() * 1e3 / f64::from(n);
    println!("devices: full list {full:.2} ms, identifiers {ids:.2} ms");

    // Device change (same device), time on the main thread.
    let mut a = AudioEngine::new(assets.clone());
    while a.is_opening() {
        std::thread::sleep(Duration::from_millis(1));
        a.tick();
    }
    for round in 0..4 {
        let t = Instant::now();
        if rc_audio::reopen_enabled() {
            a.reopen("");
        } else {
            a.stop_all();
            let table = SoundTable::load(&[&*assets]);
            a = AudioEngine::with_device(assets.clone(), table, "");
        }
        let main = t.elapsed();
        while a.is_opening() {
            std::thread::sleep(Duration::from_millis(1));
            a.tick();
        }
        println!(
            "reopening {round}: main thread {:.1} ms, output ready in {:.1} ms",
            main.as_secs_f64() * 1e3,
            t.elapsed().as_secs_f64() * 1e3
        );
        std::thread::sleep(Duration::from_millis(500));
    }
}
