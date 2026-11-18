//! Diagnostic of the output opening: N engine creations on the default device,
//! steps written by the engine ("audio: steps" / "audio: output opened"), time handed
//! back to the caller, time until the output is ready, and release.
//! `cargo run --release -p rc-audio --example open_steps -- <launches> <pause_s> [<hold_s>]`
//! (`RUSTCRAFT_OPT_AUDIOFOND=0`: opening on the calling thread, old path).

use std::sync::Arc;
use std::time::{Duration, Instant};

fn main() {
    let mut args = std::env::args().skip(1);
    let runs: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(6);
    let pause: f64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(0.0);
    // Duration (s) the output stays open before release.
    let hold: f64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(0.0);
    let install = rc_data::locate("26.3").expect("installation 26.3");
    let index = install.asset_index();
    let assets = rc_assets::Assets::for_install(
        &install.client_jar,
        &[],
        index.as_ref().map(|(i, o)| (i.as_path(), o.as_path())),
    )
    .expect("resources");
    let assets = Arc::new(assets);
    for run in 0..runs {
        let t = Instant::now();
        let mut e = rc_audio::AudioEngine::new(assets.clone());
        let back = t.elapsed();
        while e.is_opening() {
            std::thread::sleep(Duration::from_millis(1));
            e.tick();
        }
        let ready = t.elapsed();
        std::thread::sleep(Duration::from_secs_f64(hold));
        let t2 = Instant::now();
        drop(e);
        println!(
            "launch {run}: rendered in {:.1} ms, output ready in {:.1} ms, released in {:.1} ms",
            back.as_secs_f64() * 1e3,
            ready.as_secs_f64() * 1e3,
            t2.elapsed().as_secs_f64() * 1e3
        );
        std::thread::sleep(Duration::from_secs_f64(pause));
    }
}
