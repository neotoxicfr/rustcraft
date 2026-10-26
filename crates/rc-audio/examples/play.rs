//! Plays a sound event on the default output.
//! `cargo run --release -p rc-audio --example play -- block.note_block.harp [pitch] [x]`
//! `x`: abscissa of the source (listener at the origin, looking towards −Z); absent = relative.

use std::sync::Arc;
use std::time::{Duration, Instant};

use rc_audio::{AudioEngine, Category};

fn main() {
    let mut args = std::env::args().skip(1);
    let name = args.next().unwrap_or_else(|| "block.note_block.harp".into());
    let pitch: f32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(1.0);
    let x: Option<f64> = args.next().and_then(|s| s.parse().ok());

    let install = rc_data::locate("26.3").expect("installation 26.3");
    let index = install.asset_index();
    let assets = rc_assets::Assets::for_install(
        &install.client_jar,
        &[],
        index.as_ref().map(|(i, o)| (i.as_path(), o.as_path())),
    )
    .expect("resources");
    let t = Instant::now();
    let assets = Arc::new(assets);
    let mut engine = AudioEngine::new(assets.clone());
    println!(
        "{} events, output {} Hz ({}), ready in {:.0} ms",
        engine.table().len(),
        engine.sample_rate(),
        if engine.has_output() { "opened" } else { "absent" },
        t.elapsed().as_secs_f64() * 1e3
    );
    engine.set_listener([0.0; 3], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]);
    let pos = x.map(|x| [x, 0.0, 0.0]);
    // Same seed for display and playback: same variant.
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as i64);
    let id = rc_audio::sounds::full_id(&name);
    if let Some(r) = engine.table().pick(&id, &mut rc_audio::JavaRandom::new(seed)) {
        let info = assets
            .read(&r.path)
            .and_then(|b| rc_audio::decode::decode_all(b).ok())
            .map(|p| format!("{} channel(s), {} Hz, {:.2} s", p.channels, p.rate, p.seconds()));
        println!("variant {} (stream: {}): {}", r.path, r.stream, info.unwrap_or_default());
    }
    let Some(h) = engine.play(&name, Category::Master, pos, 1.0, pitch, Some(seed)) else {
        eprintln!("nothing to play for {name}");
        return;
    };
    // Tick loop at 20 Hz until the sound ends (30 s maximum).
    let start = Instant::now();
    while engine.is_playing(h) && start.elapsed() < Duration::from_secs(30) {
        std::thread::sleep(Duration::from_millis(50));
        engine.tick();
    }
    println!("done after {:.2} s", start.elapsed().as_secs_f64());
}
