//! Proofs of the bounded PCM cache (`RUSTCRAFT_OPT_PCMBUDGET`, I-05): buffer held by a voice
//! never forgotten, sound forgotten then re-read bit-for-bit identical, common sounds never delayed.

use std::time::Instant;

use super::tests_opt::{headless, loop_at};
use super::*;
use crate::rng::JavaRandom;

fn long_pcm(seconds: usize, k: usize) -> Arc<Pcm> {
    let data = (0..seconds * 44_100).map(|i| (((i * k) % 2000) as i16 - 1000) * 9).collect();
    Arc::new(Pcm { channels: 1, rate: 44_100, data })
}

const LOOP: &str = "assets/minecraft/sounds/test/loop.ogg";
const MONO: &str = "assets/minecraft/sounds/test/mono.ogg";

/// A playing voice keeps its buffer: the cache, even beyond the budget, does not forget it; once
/// the voice is finished (end polled by `pump`), the buffer becomes a candidate again.
#[test]
fn held_buffer_never_evicted() {
    let (mut e, mut m) = headless(Opt::NEW, "pcm-held");
    {
        let mut c = e.cache.lock().unwrap();
        c.insert(LOOP.into(), long_pcm(2, 3));
        c.insert(MONO.into(), long_pcm(2, 5));
        c.budget = Some(1);
    }
    let h = e.play_request(loop_at([0.0, 64.0, 0.0], 1)).expect("loop launched");
    assert_eq!(e.instances[&h.0].state, State::Playing, "launched from the cache");
    let mut out = vec![0.0f32; 2 * 480];
    m.process(&mut out);
    e.cache.lock().unwrap().insert("x".into(), long_pcm(2, 7));
    {
        let c = e.cache.lock().unwrap();
        assert!(c.contains(LOOP), "buffer held by a voice forgotten");
        assert!(!c.contains(MONO), "free and old sound kept beyond the budget");
    }
    e.stop_handle(h);
    for _ in 0..4 {
        m.process(&mut out);
        e.tick();
    }
    e.cache.lock().unwrap().insert("y".into(), long_pcm(2, 11));
    assert!(!e.cache.lock().unwrap().contains(LOOP), "voice finished: buffer forgettable");
    assert_eq!(e.cache_evicted(), 3, "mono, then loop and x");
}

/// 26.3 game resources (installation absent: test skipped with a message).
fn real() -> Option<(Arc<Assets>, SoundTable)> {
    let install = rc_testdata::need(rc_data::locate("26.3").ok(), "installed 26.3 game")?;
    let index = Some(rc_testdata::need(install.asset_index(), "26.3 sound files and fonts (asset index)")?);
    let assets = rc_testdata::need(
        Assets::for_install(&install.client_jar, &[], index.as_ref().map(|(i, o)| (i.as_path(), o.as_path()))).ok(),
        "26.3 game resources",
    )?;
    let table = SoundTable::load(&[&assets]);
    Some((Arc::new(assets), table))
}

fn digest(p: &Pcm) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for x in [u32::from(p.channels), p.rate].into_iter().chain(p.data.iter().map(|&s| u32::from(s as u16))) {
        h = (h ^ u64::from(x)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn job(path: &str, tick: u64) -> LoadJob {
    LoadJob { id: 1, path: Name::Owned(path.to_owned()), stream: false, looping: false, tick, pin: false }
}

fn loaded_pcm(l: Loaded) -> Arc<Pcm> {
    match l {
        Loaded::Ready(_, Source::Buffer(p)) => p,
        Loaded::Ready(..) => panic!("unexpected stream"),
        Loaded::Failed(_, e) => panic!("loading: {e}"),
    }
}

/// Play, forget, replay: the re-read PCM is bit-for-bit identical to the first decoding.
#[test]
fn evicted_sound_reloads_bit_identical() {
    let Some((assets, _)) = real() else { return };
    let cache: Cache = Arc::new(Mutex::new(PcmCache::new(Some(1))));
    let paths: Vec<String> =
        (1..=4).map(|i| format!("assets/minecraft/sounds/ambient/cave/cave{i}.ogg")).collect();
    let first: Vec<u64> = paths.iter().map(|p| digest(&loaded_pcm(load(&assets, &cache, job(p, 0))))).collect();
    let c = cache.lock().unwrap();
    assert!(!c.contains(&paths[0]) && c.contains(&paths[3]), "long caves: forgotten beyond the budget");
    assert_eq!(c.evicted, 3);
    drop(c);
    for (p, d) in paths.iter().zip(&first) {
        let again = loaded_pcm(load(&assets, &cache, job(p, 10)));
        assert_eq!(digest(&again), *d, "{p} re-read differs");
        let fresh = decode::decode_all(assets.read(p).expect("sound present")).expect("decoded");
        assert_eq!(digest(&fresh), *d, "{p}: cache differs from a fresh decoding");
    }
}

/// Polls the engine until `done`; `false` only past the anti-hang guard (300 s: real decodings on
/// a saturated machine), never the normal path of a test.
fn pump_until(e: &mut AudioEngine, done: impl Fn(&AudioEngine) -> bool) -> bool {
    let t = Instant::now();
    while t.elapsed().as_secs() < 300 {
        e.pump();
        if done(e) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    false
}

/// Files of an event in the cache.
fn cached_files(e: &AudioEngine, event: &str) -> (usize, usize) {
    let ev = e.table.event_index(event).expect("event");
    let files = e.table.buffered_files(ev);
    let c = e.cache.lock().unwrap();
    (files.iter().filter(|&&f| c.contains(e.table.file_path(f))).count(), files.len())
}

/// `AUDIO_PRELOAD_VARIANTS`: after the first playback of a short sound, every variant of its event
/// reaches the cache, decoded bit for bit like a direct decoding; a long sound (cave ambience)
/// preloads nothing, and neither does the old path.
#[test]
fn first_playback_preloads_the_other_variants() {
    let Some((assets, _)) = real() else { return };
    let (mut e, _m) = AudioEngine::headless_with(assets.clone(), SoundTable::load(&[&assets]), 48_000, Opt::NEW);
    let step = "minecraft:block.stone.step";
    e.play_request(SoundRequest::new(step, Category::Block).at([1.0, 64.0, 0.0]).seed(Some(1))).expect("step");
    assert!(pump_until(&mut e, |e| { let (n, all) = cached_files(e, step); n == all }), "variants never all decoded");
    let (n, _) = cached_files(&e, step);
    assert!(n >= 4, "{n} variants: empty proof");
    let ev = e.table.event_index(step).expect("event");
    for f in e.table.buffered_files(ev) {
        let path = e.table.file_path(f);
        let kept = e.cache.lock().unwrap().get(path, 0).expect("cached");
        let fresh = decode::decode_all(assets.read(path).expect("file")).expect("decoded");
        assert_eq!(digest(&kept), digest(&fresh), "{path}");
    }
    assert!(pump_until(&mut e, |e| e.loads_in_flight == 0), "loads never answered");
    assert!(e.variant_jobs > 0, "short sound: no variant preload counted");
    // Preloads are decided on the engine thread when the first load answers: once the cave load
    // is answered, a preload would have been sent. Counted, instead of a 300 ms sleep that a
    // loaded machine makes too short to see one.
    let jobs = e.variant_jobs;
    let cave = "minecraft:ambient.cave";
    e.play_request(SoundRequest::new(cave, Category::Ambient).at([1.0, 64.0, 0.0]).seed(Some(2))).expect("cave");
    assert!(pump_until(&mut e, |e| e.loads_in_flight == 0 && cached_files(e, cave).0 == 1), "cave never decoded");
    assert_eq!(e.variant_jobs, jobs, "long sound: variants preloaded");
    assert_eq!(cached_files(&e, cave).0, 1, "long sound: variants preloaded");
    let (mut old, _m) = AudioEngine::headless_with(
        assets.clone(),
        SoundTable::load(&[&assets]),
        48_000,
        Opt { preload_variants: false, ..Opt::NEW },
    );
    old.play_request(SoundRequest::new(step, Category::Block).at([1.0, 64.0, 0.0]).seed(Some(1))).expect("step");
    assert!(pump_until(&mut old, |e| e.loads_in_flight == 0), "load never answered");
    assert_eq!(old.variant_jobs, 0, "old path preloaded");
    assert_eq!(cached_files(&old, step).0, 1, "old path preloaded");
}

/// Village-like burst: the first playbacks of 50 common events at once (cold cache), then 3
/// sounds per tick for 400 ticks. Time until the burst is decoded, launches that waited for a
/// decoding.
fn village_entry(assets: &Arc<Assets>, opt: Opt, loaders: usize) -> (f64, u32) {
    let (mut e, mut m) = AudioEngine::build_with_loaders(assets.clone(), SoundTable::load(&[&**assets]), 48_000, opt, loaders);
    e.headless = true;
    let t = Instant::now();
    for (k, ev) in COMMON.iter().enumerate() {
        e.play_request(SoundRequest::new(ev, Category::Neutral).at([2.0, 64.0, 2.0]).seed(Some(k as i64)));
    }
    assert!(pump_until(&mut e, |e| e.instances.values().all(|i| i.state != State::Loading)), "burst never decoded");
    let burst = t.elapsed().as_secs_f64() * 1e3;
    let mut rng = JavaRandom::new(7);
    let (mut waited, mut out) = (0, vec![0.0f32; 2 * 480]);
    for _ in 0..400 {
        for _ in 0..3 {
            let ev = COMMON[rng.next_int_bounded(50) as usize];
            let req = SoundRequest::new(ev, Category::Neutral).at([2.0, 64.0, 2.0]).seed(Some(i64::from(rng.next_int())));
            if let Some(h) = e.play_request(req) {
                waited += u32::from(e.instances[&h.0].state == State::Loading);
            }
        }
        for _ in 0..5 {
            m.process(&mut out);
        }
        e.tick();
        // A tick lasts 50 ms in game: decodings started in the tick are done by the next one.
        pump_until(&mut e, |e| e.loads_in_flight == 0);
    }
    (burst, waited)
}

/// Bench (ignored), loaded machine figures only: decoding threads 1, 2, 4, 8 on a burst of first
/// sounds, and the launches that wait for a decoding with and without the variant preloading.
/// `cargo-file.ps1 test --release -p rc-audio loader_bench -- --ignored --nocapture`
#[test]
#[ignore = "bench"]
fn loader_bench() {
    let Some((assets, _)) = real() else { return };
    for pass in 0..2 {
        for loaders in [1, 2, 4, 8] {
            let (burst, _) = village_entry(&assets, Opt { preload_variants: false, ..Opt::NEW }, loaders);
            println!("pass {pass}: {loaders} loading thread(s): burst of 50 first sounds decoded in {burst:.1} ms");
        }
        for preload in [false, true] {
            let (burst, waited) = village_entry(&assets, Opt { preload_variants: preload, ..Opt::NEW }, 2);
            println!("pass {pass}: preload {preload}: burst {burst:.1} ms, {waited} of 1,200 launches waited for a decoding");
        }
    }
}

/// Common sounds of a game (step, hits, doors, mobs, interface).
const COMMON: [&str; 50] = [
    "block.stone.step",
    "block.stone.break",
    "block.stone.place",
    "block.stone.hit",
    "block.grass.step",
    "block.grass.break",
    "block.grass.place",
    "block.grass.hit",
    "block.gravel.step",
    "block.gravel.break",
    "block.sand.step",
    "block.sand.break",
    "block.wood.step",
    "block.wood.break",
    "block.wood.place",
    "block.wood.hit",
    "block.deepslate.step",
    "block.deepslate.break",
    "block.deepslate.place",
    "block.deepslate.hit",
    "block.chest.open",
    "block.chest.close",
    "block.wooden_door.open",
    "block.wooden_door.close",
    "block.ladder.step",
    "block.wool.step",
    "block.lava.pop",
    "entity.player.hurt",
    "entity.player.attack.strong",
    "entity.player.attack.sweep",
    "entity.player.attack.weak",
    "entity.player.splash",
    "entity.generic.eat",
    "entity.item.pickup",
    "entity.experience_orb.pickup",
    "entity.arrow.shoot",
    "entity.arrow.hit",
    "entity.zombie.ambient",
    "entity.zombie.hurt",
    "entity.skeleton.ambient",
    "entity.skeleton.step",
    "entity.creeper.primed",
    "entity.spider.ambient",
    "entity.spider.step",
    "entity.cow.ambient",
    "entity.pig.ambient",
    "entity.sheep.ambient",
    "entity.chicken.ambient",
    "entity.villager.ambient",
    "ui.button.click",
];

/// Long and rare sounds (ambiences): the only ones the budget makes forget.
const LONG: [&str; 6] = [
    "ambient.cave",
    "ambient.underwater.loop.additions",
    "ambient.underwater.loop.additions.rare",
    "block.sand.idle",
    "entity.generic.explode",
    "ambient.basalt_deltas.additions",
];

#[derive(Debug, Default)]
struct Run {
    /// Common sounds launched "loading" although their file had already been loaded.
    common_waits: u32,
    /// Launches that waited for a decoding (first playback of a file included).
    loading_launches: u32,
    long_reloads: u32,
    plays: u32,
    peak: usize,
    end: usize,
    evicted: u64,
}

/// Simulated game: 1.5 common sounds per tick, one long sound every 20 ticks, mixer and `tick`
/// at the game's pace (2,400 frames per tick at 48 kHz).
fn session(assets: Arc<Assets>, table: SoundTable, budget: Option<usize>, ticks: u32, opt: Opt) -> Run {
    let (mut e, mut m) = AudioEngine::headless_with(assets, table, 48_000, opt);
    e.cache.lock().unwrap().budget = budget;
    let mut rng = JavaRandom::new(42);
    let mut seen: HashSet<Arc<str>> = HashSet::new();
    let mut pending: Vec<(u64, bool)> = Vec::new();
    let mut run = Run::default();
    let mut out = vec![0.0f32; 2 * 480];
    for t in 0..ticks {
        let mut wanted: Vec<(&str, bool)> = vec![(COMMON[rng.next_int_bounded(50) as usize], true)];
        if t % 2 == 0 {
            wanted.push((COMMON[rng.next_int_bounded(50) as usize], true));
        }
        if t % 20 == 0 {
            wanted.push((LONG[rng.next_int_bounded(LONG.len() as i32) as usize], false));
        }
        for (ev, common) in wanted {
            let seed = i64::from(rng.next_int());
            let req = SoundRequest::new(ev, Category::Neutral).at([2.0, 64.0, 2.0]).seed(Some(seed));
            let Some(h) = e.play_request(req) else { continue };
            run.plays += 1;
            let inst = &e.instances[&h.0];
            if inst.state == State::Loading {
                run.loading_launches += 1;
                if seen.contains(&*inst.path) {
                    if common {
                        run.common_waits += 1;
                        eprintln!("waiting: {ev} ({})", &*inst.path);
                    } else {
                        run.long_reloads += 1;
                    }
                }
                pending.push((h.0, common));
            } else {
                seen.insert(inst.path.to_string().into());
            }
        }
        for _ in 0..5 {
            m.process(&mut out);
        }
        e.tick();
        // As in game (decoding of a few ms, 50 ms tick): loads finished before the next
        // tick; the wait depends on no clock, only on the end of the decodings (300 s: anti-hang
        // guard only, a silent cap would change the counts on a saturated machine).
        let t0 = Instant::now();
        while e.instances.values().any(|i| i.state == State::Loading) {
            assert!(t0.elapsed().as_secs() < 300, "anti-hang guard: loads of tick {t} never answered");
            std::thread::sleep(std::time::Duration::from_millis(1));
            e.pump();
        }
        // Loads finished: files now "already loaded".
        pending.retain(|(id, _)| match e.instances.get(id) {
            Some(i) if i.state == State::Loading => true,
            Some(i) => {
                seen.insert(i.path.to_string().into());
                false
            }
            None => false,
        });
        run.peak = run.peak.max(e.cache_bytes());
    }
    run.end = e.cache_bytes();
    run.evicted = e.cache_evicted();
    run
}

/// 30 min of game at the default budget: none of the 50 common sounds waits for a reload,
/// each starts from the cache in the call itself, so at the next mixer block, as without budget.
/// (Budget of 4 MiB, under the ~13 MB of protected sounds: 14 waits of the 2.1 s zombie cry,
/// forgotten at every long sound. Sabotage checked: unprotected short sounds -> failure.)
#[test]
fn common_sounds_never_wait() {
    let Some((assets, table)) = real() else { return };
    for ev in COMMON.iter().chain(&LONG) {
        assert!(table.get(&full_id(ev)).is_some(), "event {ev} absent");
    }
    let r = session(assets, table, Some(pcm_cache::BUDGET), 36_000, Opt::NEW);
    assert!(r.evicted > 0, "budget never reached: pointless test ({r:?})");
    assert_eq!(r.common_waits, 0, "common sound delayed by forgetting ({r:?})");
}

/// Bench (ignored): 30 min of simulated game, unbounded cache versus default budget.
/// `cargo-file.ps1 test --release -p rc-audio pcm_budget_bench -- --ignored --nocapture`
#[test]
#[ignore = "bench"]
fn pcm_budget_bench() {
    let Some((assets, _)) = real() else { return };
    let runs = [
        (None, Opt { preload_variants: false, ..Opt::NEW }),
        (Some(pcm_cache::BUDGET), Opt { preload_variants: false, ..Opt::NEW }),
        (None, Opt::NEW),
        (Some(pcm_cache::BUDGET), Opt::NEW),
    ];
    for (budget, opt) in runs {
        let table = SoundTable::load(&[&assets]);
        let t = Instant::now();
        let r = session(assets.clone(), table, budget, 36_000, opt);
        println!(
            "preload {}, budget {:?} MiB: end {:.1} MB, peak {:.1} MB, {} forgotten, {} re-reads of long sounds, {} waits for common sounds, {} launches waiting for a decoding, {} sounds, {:.1} s",
            opt.preload_variants,
            budget.map(|b| b >> 20),
            r.end as f64 / 1e6,
            r.peak as f64 / 1e6,
            r.evicted,
            r.long_reloads,
            r.common_waits,
            r.loading_launches,
            r.plays,
            t.elapsed().as_secs_f64()
        );
    }
}
