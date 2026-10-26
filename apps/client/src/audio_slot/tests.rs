//! Equivalence proofs for the audio engine joined on first use (C4-M04), on a synthetic table
//! and engines with no output (`AudioEngine::headless_on`: openings counted).

use std::path::PathBuf;
use std::time::Duration;

use rc_audio::SoundRequest;

use super::*;

const JSON: &str = r#"{
  "test.mono": {"sounds": ["test/mono"]},
  "test.ui": {"sounds": ["test/ui"]},
  "ui.button.click": {"sounds": ["test/ui"]}
}"#;

/// Minimal resources in `target/test-audio-client/<name>`.
fn assets(name: &str) -> Arc<Assets> {
    assets_json(name, JSON)
}

/// Like [`assets`], with `json` for `sounds.json`.
fn assets_json(name: &str, json: &str) -> Arc<Assets> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-audio-client").join(name);
    let snd = dir.join("assets/minecraft/sounds/test");
    std::fs::create_dir_all(&snd).unwrap();
    for f in ["mono", "ui"] {
        std::fs::write(snd.join(format!("{f}.ogg")), b"x").unwrap();
    }
    std::fs::write(dir.join("assets/minecraft/sounds.json"), json).unwrap();
    Arc::new(Assets::new(vec![rc_assets::Source::dir(&dir)]))
}

fn headless(a: Arc<Assets>, t: SoundTable, device: &str) -> AudioEngine {
    assert_eq!(t.len(), 3, "incomplete synthetic table");
    AudioEngine::headless_on(a, t, 48_000, device).0
}

/// Slow construction: the join must really wait.
fn slow(a: Arc<Assets>, t: SoundTable, device: &str) -> AudioEngine {
    std::thread::sleep(Duration::from_millis(150));
    headless(a, t, device)
}

const VOLUMES: Volumes = [1.0, 0.5, 1.0, 0.25, 1.0, 1.0, 0.75, 1.0, 1.0, 1.0, 0.4];

#[test]
fn sound_requested_before_join_is_played() {
    let mut slot = Audio::spawn(assets("before-join"), "", VOLUMES, true, slow);
    assert!(slot.is_pending(), "construction finished too early for the test");
    assert!(slot.peek().is_none(), "debug screen: no engine before the join");
    // First sound requested during construction: joined, never lost.
    let a = slot.get().expect("engine joined on first use");
    let h = a.play_request(SoundRequest::new("test.ui", Category::Ui).seed(Some(1)));
    assert!(h.is_some(), "sound requested before the join lost");
    assert_eq!(a.active(), 1);
    assert!(!slot.is_pending());
    for c in Category::ALL {
        assert_eq!(slot.peek().unwrap().volume(c), VOLUMES[c as usize], "volume {c:?} not set");
    }
}

#[test]
fn settings_already_applied_do_not_join() {
    let mut slot = Audio::spawn(assets("no-join"), "Headphones", VOLUMES, true, slow);
    assert!(slot.for_volumes(&VOLUMES).is_none());
    assert!(slot.for_device("Headphones").is_none());
    assert!(slot.is_pending(), "identical settings: join useless at the end of window creation");
    // A different setting joins and returns the engine.
    let mut other = VOLUMES;
    other[1] = 0.0;
    assert!(slot.for_volumes(&other).is_some());
    assert!(!slot.is_pending());
}

/// Startup: engine created then settings applied like `apply_options` / `apply_video_options`.
fn startup_opens(name: &str, direct: bool, device: &str) -> u32 {
    let mut slot = Audio::spawn(assets(name), device, VOLUMES, direct, headless);
    if let Some(a) = slot.for_volumes(&VOLUMES) {
        for c in Category::ALL {
            a.set_volume(c, VOLUMES[c as usize]);
        }
    }
    if let Some(a) = slot.for_device(device)
        && a.device() != device
    {
        // `reload_audio` with `AUDIOREOPEN` (default); called directly so as never to open
        // a real output when the suite runs with the switches at 0.
        a.reopen(device);
    }
    let a = slot.get().unwrap();
    assert_eq!(a.device(), device);
    a.output_opens()
}

#[test]
fn named_device_opened_once() {
    assert_eq!(startup_opens("named-direct", true, "Speakers (Test)"), 1);
    assert_eq!(startup_opens("named-old", false, "Speakers (Test)"), 2, "old path: two openings");
    assert_eq!(startup_opens("default-direct", true, ""), 1);
    assert_eq!(startup_opens("default-old", false, ""), 1);
}

/// Sounds of a fixed sequence: handles, active events, volumes.
fn run(mut a: AudioEngine) -> (Vec<Option<u64>>, Vec<String>, Vec<f32>, String) {
    let mut handles = Vec::new();
    for i in 0..6 {
        let req = if i % 2 == 0 {
            SoundRequest::new("test.mono", Category::Block).at([f64::from(i), 64.0, 0.0]).seed(Some(i64::from(i)))
        } else {
            SoundRequest::new("test.ui", Category::Ui).seed(Some(i64::from(i)))
        };
        handles.push(a.play_request(req).map(|h| h.0));
    }
    let mut ev: Vec<String> = a.events().map(str::to_owned).collect();
    ev.sort();
    (handles, ev, Category::ALL.map(|c| a.volume(c)).to_vec(), a.device().to_owned())
}

#[test]
fn reload_engine_built_off_thread_is_identical() {
    let (a1, a2) = (assets("reload-thread"), assets("reload-main"));
    // "Resources" thread: construction started during baking, joined by this thread.
    let off = std::thread::spawn(move || Audio::spawn(a1, "Headphones", VOLUMES, true, headless).into_engine())
        .join()
        .unwrap()
        .expect("engine built");
    // Old path (main thread), with the same device and the same volumes.
    let main = build(a2, "Headphones", &VOLUMES, true, headless);
    assert_eq!(run(off), run(main));
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn ms(t: std::time::Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

/// Micro-bench (real resources, real device): MAIN THREAD time for the audio engine
/// at startup and on pack reload, old and new path in the same
/// process. `cargo test --release -p rc-app --bin rustcraft bench_audio_main_thread -- --ignored --nocapture`
#[test]
#[ignore = "micro-bench: real 26.3 installation and audio output"]
fn bench_audio_main_thread() {
    let install = rc_data::locate("26.3").expect("26.3 installation");
    let index = install.asset_index();
    let idx = index.as_ref().map(|(i, o)| (i.as_path(), o.as_path()));
    let assets = Arc::new(Assets::for_install(&install.client_jar, &[], idx).expect("resources"));
    // First named device ("soundDevice" setting non-empty); empty without a device.
    let device = rc_audio::output_devices().into_iter().next().unwrap_or_default();
    let make = AudioEngine::with_device;
    let wait_open = |a: &mut AudioEngine| {
        while a.is_opening() {
            std::thread::sleep(Duration::from_millis(1));
            a.tick();
        }
    };
    let (mut old_start, mut new_start, mut new_join, mut old_reload, mut new_reload) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let (mut old_ready, mut new_ready) = (Vec::new(), Vec::new());
    for round in 0..6 {
        // Old startup: table + creation on this thread, then reopening if `soundDevice` is named.
        let t = std::time::Instant::now();
        let mut a = AudioEngine::new(assets.clone());
        if !device.is_empty() {
            a.reopen(&device);
        }
        old_start.push(ms(t));
        wait_open(&mut a);
        // Old F3+T: shutdown, new engine, old one released here (then reopening).
        let t = std::time::Instant::now();
        a.stop_all();
        a = AudioEngine::new(assets.clone());
        if !device.is_empty() {
            a.reopen(&device);
        }
        old_reload.push(ms(t));
        wait_open(&mut a);
        // New startup: launch, 300 ms of window and GPU creation, join.
        let t = std::time::Instant::now();
        let mut slot = Audio::spawn(assets.clone(), &device, VOLUMES, true, make);
        new_start.push(ms(t));
        std::thread::sleep(Duration::from_millis(300));
        let t = std::time::Instant::now();
        let a2 = slot.get().unwrap();
        new_join.push(ms(t));
        wait_open(a2);
        // New F3+T: engine built elsewhere, swap and release in the background.
        let built = std::thread::spawn({
            let assets = assets.clone();
            let device = device.clone();
            move || build(assets, &device, &VOLUMES, true, make)
        })
        .join()
        .unwrap();
        let t = std::time::Instant::now();
        let a2 = slot.get().unwrap();
        a2.stop_all();
        retire(std::mem::replace(a2, built), true);
        new_reload.push(ms(t));
        wait_open(a2);
        drop(a);
        drop(slot);
        std::thread::sleep(Duration::from_millis(100));
        // Output ready (audible sound) since the request: old (two openings if named) and
        // new (a single one), alternating order, 2 s with no stream between the two (the system's audio engine
        // stays "warm" for a few hundred ms after the last stream).
        for new in [round % 2 == 0, round % 2 == 1] {
            std::thread::sleep(Duration::from_secs(2));
            let t = std::time::Instant::now();
            if new {
                let mut slot = Audio::spawn(assets.clone(), &device, VOLUMES, true, make);
                wait_open(slot.get().unwrap());
                new_ready.push(ms(t));
            } else {
                let mut a = AudioEngine::new(assets.clone());
                if !device.is_empty() {
                    wait_open(&mut a);
                    a.reopen(&device);
                }
                wait_open(&mut a);
                old_ready.push(ms(t));
            }
        }
    }
    println!(
        "BENCH output ready (median of 6, ms, device '{device}'): old {:.1} / new {:.1}",
        median(old_ready),
        median(new_ready)
    );
    println!(
        "BENCH main thread (median of 6, ms): old startup {:.2} / new launch {:.2} + join {:.2}; F3+T old {:.2} / new {:.2}",
        median(old_start),
        median(new_start),
        median(new_join),
        median(old_reload),
        median(new_reload)
    );
}

/// True when nothing other than `a` holds the resources any more (the engine's threads drop them
/// shortly after its release), within 2 s.
fn released(a: &Arc<Assets>) -> bool {
    let t = std::time::Instant::now();
    while Arc::strong_count(a) > 1 {
        if t.elapsed() > Duration::from_secs(2) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    true
}

#[test]
fn retired_engine_released_in_background() {
    let a = assets("release");
    let e = build(a.clone(), "", &VOLUMES, true, headless);
    assert!(Arc::strong_count(&a) > 1, "the engine holds the resources");
    retire(e, true).expect("released on a separate thread").join().unwrap();
    assert!(released(&a), "old engine never released (background)");
    let e = build(a.clone(), "", &VOLUMES, true, headless);
    assert!(retire(e, false).is_none(), "old path: released on this thread");
    assert!(released(&a), "old engine never released (here)");
}

/// Forbidden constructor: the engine must not be rebuilt on the main thread.
fn never(_: Arc<Assets>, _: SoundTable, _: &str) -> AudioEngine {
    panic!("engine rebuilt on the main thread although it was built during the load")
}

#[test]
fn reload_uses_engine_built_during_load() {
    let a = assets("reload-wiring");
    // "Resources" thread: hook called at indexing, engine returned with the result.
    let (l, pre) = load_with_engine(Some(("Headphones".to_owned(), VOLUMES, true)), headless, |hook| {
        hook(&a);
        7
    });
    assert_eq!(l, 7);
    let pre = pre.expect("engine built during the load");
    // `apply_loaded`: that very engine, with no rebuild.
    let e = reload_engine(Some(pre), &a, "Headphones", &VOLUMES, true, never);
    assert_eq!((e.device(), e.output_opens()), ("Headphones", 1));
    for c in Category::ALL {
        assert_eq!(e.volume(c), VOLUMES[c as usize], "volume {c:?} not set");
    }
    // `RELOADAUDIO=0`: no engine built during the load.
    let (_, none) = load_with_engine(None, never, |hook| hook(&a));
    assert!(none.is_none());
}

/// F3+T with a named `soundDevice`: engine of the new stack, then settings applied like
/// `apply_options` does after the reload (not the first pass). Returns the sounds played.
fn reload_clicks(name: &str, direct: bool) -> usize {
    let a = assets(name);
    let mut slot = Audio::ready(reload_engine(None, &a, "Headphones", &VOLUMES, direct, headless));
    apply_device(&mut slot, &a, &VOLUMES, "Headphones", false);
    let e = slot.get().unwrap();
    assert_eq!(e.device(), "Headphones");
    e.active()
}

#[test]
fn reload_with_named_device_plays_no_click() {
    // The game reopens its engine on the chosen device, with no UI sound.
    assert_eq!(reload_clicks("click-direct", true), 0);
    assert_eq!(reload_clicks("click-old", false), 1, "old path: click after the reopening");
}

/// Table of an abandoned stack: different from [`JSON`], to recognize the kept engine.
const ABANDONED_JSON: &str = r#"{"test.mono": {"sounds": ["test/mono"]}}"#;

/// No output, whatever the table; slow, so that the replacement arrives in the middle of the
/// construction of the old engine.
fn any_slow(a: Arc<Assets>, t: SoundTable, device: &str) -> AudioEngine {
    std::thread::sleep(Duration::from_millis(60));
    AudioEngine::headless_on(a, t, 48_000, device).0
}

/// Early hook (`AUDIOEARLY`): called on two abandoned stacks (default font
/// failing) then on the retained one. The returned engine is that of the retained stack, identical to that of
/// the old timing (hook on the retained stack only), with no sound played or extra output, and the
/// engines of the abandoned stacks are released.
#[test]
fn early_hook_on_abandoned_stacks_yields_retained_engine() {
    let (bad1, bad2) = (assets_json("abandoned-1", ABANDONED_JSON), assets_json("abandoned-2", ABANDONED_JSON));
    let good = assets("abandoned-retained");
    let cfg = || Some(("Headphones".to_owned(), VOLUMES, true));
    let (l, e) = load_with_engine(cfg(), any_slow, |hook| {
        hook(&bad1);
        hook(&bad2);
        hook(&good);
        3
    });
    assert_eq!(l, 3);
    let e = e.expect("engine built during the load");
    assert_eq!(e.table().len(), 3, "engine of an abandoned stack kept");
    assert_eq!((e.device(), e.output_opens(), e.active()), ("Headphones", 1, 0));
    assert!(released(&bad1) && released(&bad2), "engine of an abandoned stack never released");
    // Old timing: hook called once, on the retained stack.
    let (_, old) = load_with_engine(cfg(), any_slow, |hook| hook(&assets("abandoned-old")));
    assert_eq!(run(e), run(old.expect("engine")));
    // Startup (`main.rs`): same replacement on the client's slot.
    let mut slot = Audio::off();
    slot.respawn(bad1.clone(), "", VOLUMES, true, any_slow);
    slot.respawn(good.clone(), "", VOLUMES, true, any_slow);
    assert_eq!(slot.get().expect("engine").table().len(), 3);
    drop(slot);
    assert!(released(&bad1), "slot: engine of the abandoned stack never released");
}

/// Engine of an abandoned stack: released without the first-use join (no "audio joined"
/// milestone for a discarded engine), whereas the join of the retained engine does set it. Engines
/// identified by a device specific to this test (the other tests join in parallel).
#[test]
fn abandoned_engine_freed_without_join_milestone() {
    let joined = |d: &str| JOINED.lock().unwrap().iter().filter(|x| *x == d).count();
    let bad = assets_json("discarded", ABANDONED_JSON);
    let mut slot = Audio::off();
    slot.respawn(bad.clone(), "Discarded-milestone", VOLUMES, true, any_slow);
    slot.respawn(assets("kept-join"), "Kept-milestone", VOLUMES, true, any_slow);
    assert!(slot.get().is_some());
    assert!(released(&bad), "discarded engine never released");
    assert_eq!(joined("Discarded-milestone"), 0, "milestone 'audio joined' set for a discarded engine");
    assert_eq!(joined("Kept-milestone"), 1, "join of the retained engine without a milestone");
}

/// Micro-bench (real resources): engine ready, counted from the resource index, when its
/// construction starts after the fonts (old timing) or before (in parallel), and the time of the
/// fonts on this thread in both cases (contention on resource reads). No audio
/// output. `cargo test --release -p rc-app --bin rustcraft bench_audio_early -- --ignored --nocapture`
#[test]
#[ignore = "micro-bench: 26.3 installation"]
fn bench_audio_early() {
    let install = rc_data::locate("26.3").expect("26.3 installation");
    let index = install.asset_index();
    let idx = index.as_ref().map(|(i, o)| (i.as_path(), o.as_path()));
    let assets = Arc::new(Assets::for_install(&install.client_jar, &[], idx).expect("resources"));
    let fonts = |a: &Assets| {
        crate::resources::FONT_IDS
            .iter()
            .filter_map(|id| rc_assets::font::Font::try_load(a, id, Default::default(), true))
            .count()
    };
    let make: Make = |a, t, d| AudioEngine::headless_on(a, t, 48_000, d).0;
    // First pass outside the measurement (files in the system cache).
    assert_eq!(fonts(&assets), 4);
    drop(build(assets.clone(), "", &VOLUMES, true, make));
    let (mut old_fonts, mut new_fonts, mut old_ready, mut new_ready) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for round in 0..10 {
        for early in [round % 2 == 0, round % 2 == 1] {
            let t = std::time::Instant::now();
            let mut slot = Audio::off();
            if early {
                slot.respawn(assets.clone(), "", VOLUMES, true, make);
            }
            assert_eq!(fonts(&assets), 4);
            let f = ms(t);
            if !early {
                slot.respawn(assets.clone(), "", VOLUMES, true, make);
            }
            let e = slot.into_engine().expect("engine");
            let r = ms(t);
            drop(e);
            let (fv, rv) = if early { (&mut new_fonts, &mut new_ready) } else { (&mut old_fonts, &mut old_ready) };
            fv.push(f);
            rv.push(r);
        }
    }
    println!(
        "BENCH engine ready since the index (median of 10, ms): old {:.1} / early {:.1}; fonts on this thread: old {:.1} / early {:.1}",
        median(old_ready),
        median(new_ready),
        median(old_fonts),
        median(new_fonts)
    );
}
