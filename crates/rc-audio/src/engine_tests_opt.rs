//! Equivalence proofs of the engine optimisations (C4-M03), on a synthetic table: they
//! depend neither on the game installation nor on a device (simulated output).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::*;
use crate::mixer::Mixer;

const JSON: &str = r#"{
  "test.mono": {"subtitle": "subtitles.test.mono", "sounds": ["test/mono"]},
  "test.loop": {"sounds": ["test/loop"]},
  "test.stereo": {"sounds": [{"name": "test/stereo", "volume": 0.8}]}
}"#;

/// Minimal resources in `target/test-audio/<name>` (the `.ogg` files are never decoded:
/// their PCM is put in the cache in advance).
fn assets(name: &str) -> Arc<Assets> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-audio").join(name);
    let snd = dir.join("assets/minecraft/sounds/test");
    std::fs::create_dir_all(&snd).unwrap();
    for f in ["mono", "loop", "stereo"] {
        std::fs::write(snd.join(format!("{f}.ogg")), b"x").unwrap();
    }
    std::fs::write(dir.join("assets/minecraft/sounds.json"), JSON).unwrap();
    Arc::new(Assets::new(vec![rc_assets::Source::dir(&dir)]))
}

fn pcm(channels: u16, frames: usize, k: usize) -> Arc<Pcm> {
    let data = (0..frames * usize::from(channels)).map(|i| (((i * k) % 2000) as i16 - 1000) * 12).collect();
    Arc::new(Pcm { channels, rate: 44_100, data })
}

fn fill(cache: &Cache) {
    let mut c = cache.lock().unwrap();
    c.insert("assets/minecraft/sounds/test/mono.ogg".into(), pcm(1, 4800, 7));
    c.insert("assets/minecraft/sounds/test/loop.ogg".into(), pcm(1, 2000, 13));
    c.insert("assets/minecraft/sounds/test/stereo.ogg".into(), pcm(2, 3000, 5));
}

fn table(a: &Assets) -> SoundTable {
    let t = SoundTable::load(&[a]);
    assert_eq!(t.len(), 3, "synthetic table incomplete");
    t
}

pub(super) fn headless(opt: Opt, name: &str) -> (AudioEngine, Mixer) {
    let a = assets(name);
    let t = table(&a);
    let (e, m) = AudioEngine::headless_with(a, t, 48_000, opt);
    fill(&e.cache);
    (e, m)
}

fn mono_at(p: [f64; 3], seed: i64) -> SoundRequest {
    SoundRequest::new("test.mono", Category::Block).at(p).seed(Some(seed))
}

pub(super) fn loop_at(p: [f64; 3], seed: i64) -> SoundRequest {
    SoundRequest::new("test.loop", Category::Ambient).at(p).seed(Some(seed)).looping(0)
}

/// f32 output (bits) of 1,000 blocks with a listener that moves and turns on every block.
fn listener_run(opt: Opt) -> Vec<u32> {
    let (mut e, mut m) = headless(opt, if opt.listener { "listener-new" } else { "listener-old" });
    e.set_listener([0.0, 64.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]);
    for i in 0..6 {
        let p = [f64::from(i) * 3.0 - 8.0, 64.0 + f64::from(i % 3), f64::from(i) * -2.0];
        e.play_request(loop_at(p, i64::from(i))).expect("loop launched");
    }
    e.play_request(mono_at([4.0, 64.0, 4.0], 9)).expect("sound launched");
    e.play_request(SoundRequest::new("test.stereo", Category::Ui).seed(Some(3))).expect("stereo launched");
    let mut out = vec![0.0f32; 2 * 480];
    let mut bits = Vec::with_capacity(1000 * out.len());
    for b in 0..1000 {
        let a = b as f32 * 0.013;
        e.set_listener([f64::from(b) * 0.02, 64.0, f64::from(b) * -0.01], [a.sin(), 0.1, -a.cos()], [0.0, 1.0, 0.0]);
        m.process(&mut out);
        bits.extend(out.iter().map(|x| x.to_bits()));
        if b % 3 == 0 {
            e.tick();
        }
    }
    bits
}

/// Listener in a shared cell: mixer output bit-for-bit identical to the listener sent as a
/// command, over 1,000 blocks with a moving listener (call order fixed).
#[test]
fn listener_slot_bit_exact() {
    let old = listener_run(Opt::OLD);
    let new = listener_run(Opt { listener: true, ..Opt::OLD });
    assert!(old.iter().any(|&b| b != 0), "silent output: empty proof");
    assert_eq!(old.len(), new.len());
    let first = old.iter().zip(&new).position(|(a, b)| a != b);
    assert_eq!(first, None, "first difference at sample {first:?}");
    assert_eq!(new, listener_run(Opt::NEW), "full combination");
}

/// Listener cell: a read always returns a whole written value (never a mix
/// of two writes), under concurrent writes.
#[test]
fn listener_slot_never_torn() {
    let slot = Arc::new(ListenerSlot::default());
    slot.store(Listener { pos: [0.0; 3], forward: [0.0; 3], up: [0.0; 3] });
    let stop = Arc::new(AtomicBool::new(false));
    let (s, st) = (slot.clone(), stop.clone());
    let w = std::thread::spawn(move || {
        let mut i = 0u32;
        while !st.load(Ordering::Relaxed) {
            let v = f64::from(i);
            let f = i as f32;
            s.store(Listener { pos: [v, v, v], forward: [f, f, f], up: [f, f, f] });
            i = i.wrapping_add(1);
        }
    });
    let (mut seen, mut skipped) = (0u32, 0u32);
    let t = Instant::now();
    while t.elapsed() < Duration::from_millis(200) {
        match slot.load() {
            Some(l) => {
                let v = l.pos[0];
                assert!(l.pos.iter().all(|&x| x == v), "{l:?}");
                let f = l.forward[0];
                assert!(l.forward.iter().chain(&l.up).all(|&x| x == f), "{l:?}");
                assert!((f64::from(f) - v).abs() <= v * 1e-6, "{l:?}");
                seen += 1;
            }
            None => skipped += 1,
        }
    }
    stop.store(true, Ordering::Relaxed);
    w.join().unwrap();
    assert!(seen > 0, "no complete read ({skipped} abandoned)");
}

/// Audio thread suspended, channel saturated: the next launch, the stop of a loop and the total
/// stop still apply, no orphan "playing" instance. False on the old
/// path (commands lost by `try_send`).
fn saturation(opt: Opt, name: &str) -> Result<(), String> {
    let (mut e, mut m) = headless(opt, name);
    let mut out = vec![0.0f32; 2 * 480];
    let a = e.play_request(loop_at([1.0, 0.0, 0.0], 1)).expect("loop A");
    m.process(&mut out);
    // Audio thread frozen: 5,000 volume changes and 5,000 listener frames.
    for i in 0..5000 {
        e.set_sound_params(a, if i % 2 == 0 { 0.5 } else { 0.6 }, 1.0);
        e.set_listener([f64::from(i), 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]);
    }
    let b = e.play_request(mono_at([2.0, 0.0, 0.0], 2)).expect("sound B");
    e.stop_handle(a);
    for _ in 0..200 {
        m.process(&mut out);
        e.tick();
    }
    if m.active() != 0 {
        return Err(format!("{} voices still in the mixer (stop of A lost?)", m.active()));
    }
    if e.is_playing(b) {
        return Err("B 'playing' forever (launch lost)".into());
    }
    let c = e.play_request(loop_at([0.0; 3], 3)).expect("loop C");
    for _ in 0..5000 {
        e.set_sound_params(c, 0.7, 1.0);
        e.set_sound_params(c, 0.9, 1.0);
    }
    e.stop_all();
    for _ in 0..30 {
        m.process(&mut out);
        e.tick();
    }
    if m.active() != 0 || e.active() != 0 {
        return Err(format!("total stop lost: mixer {}, channels {}", m.active(), e.active()));
    }
    Ok(())
}

#[test]
fn saturated_channel_keeps_play_and_stop() {
    saturation(Opt::NEW, "sat-new").unwrap();
    saturation(Opt { outbox: true, ..Opt::OLD }, "sat-outbox").unwrap();
    // The test does discriminate: the old path loses commands.
    assert!(saturation(Opt::OLD, "sat-old").is_err(), "the old path should lose commands");
}

/// Launch commands (without identifier) of a fixed series of requests.
fn plays(e: &mut AudioEngine, m: &mut Mixer) -> Vec<String> {
    let reqs = [
        mono_at([1.0, 2.0, 3.0], 11),
        loop_at([-4.0, 0.0, 9.0], 12),
        SoundRequest::new("test.stereo", Category::Record).volume(0.5).pitch(1.3).seed(Some(13)),
        mono_at([0.0; 3], 14).volume(2.5).pitch(0.7),
    ];
    for r in reqs {
        e.play_request(r).expect("sound launched");
    }
    m.drain_log().into_iter().filter(|l| l.starts_with("play")).collect()
}

/// Reopening: table and cache kept as they are (same addresses, same content), channels
/// released, and the same requests give the same launches as a new engine.
#[test]
fn reopen_keeps_table_and_cache() {
    let (mut e, mut m) = headless(Opt::NEW, "reopen");
    let ev = e.table.get("minecraft:test.mono").unwrap() as *const crate::sounds::Event;
    let cache = e.cache.clone();
    let (len, bytes) = (e.cache_len(), e.cache_bytes());
    e.set_volume(Category::Block, 0.4);
    let h = e.play_request(loop_at([0.0; 3], 1)).unwrap();
    m.process(&mut [0.0; 960]);
    let mut m2 = e.reopen_headless();
    assert_eq!(m.drain_log().first().map(String::as_str), Some("stop_all"), "old output stopped");
    assert!(std::ptr::eq(ev, e.table.get("minecraft:test.mono").unwrap()), "table re-read");
    assert!(Arc::ptr_eq(&cache, &e.cache), "cache replaced");
    assert_eq!((e.cache_len(), e.cache_bytes()), (len, bytes), "cache emptied");
    assert_eq!(e.active(), 0, "channels not released");
    assert!(!e.is_active(h));
    assert_eq!(e.volume(Category::Block), 0.4, "volume lost");
    let reopened = plays(&mut e, &mut m2);
    // Old path: new engine, same table, same volumes, cache rebuilt (same PCM).
    let (mut f, mut fm) = headless(Opt::OLD, "reopen-fresh");
    f.set_volume(Category::Block, 0.4);
    let fresh = plays(&mut f, &mut fm);
    assert_eq!(reopened.len(), 4);
    assert_eq!(reopened, fresh);
}

/// Fake device: a thread that runs the mixer every millisecond as long as the
/// "stream" lives; `marker` proves its release.
struct FakeDevice {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
    _marker: Arc<()>,
}

impl Drop for FakeDevice {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            h.join().unwrap();
        }
    }
}

/// Simulated opening: `delay` before the reply; `hold`: the stream is kept (otherwise released right
/// away, sabotage); `fail`: opening refused.
fn fake(delay: u64, hold: bool, fail: bool, marker: Arc<()>) -> Opener {
    Box::new(move |mut m: Mixer| {
        std::thread::sleep(Duration::from_millis(delay));
        if fail {
            drop(marker);
            return Err(Error("simulated opening refused".into()));
        }
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        let handle = std::thread::spawn(move || {
            let mut out = vec![0.0f32; 2 * 480];
            while !s.load(Ordering::Relaxed) {
                m.process(&mut out);
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        let dev = FakeDevice { stop, handle: Some(handle), _marker: marker };
        let stream: Box<dyn std::any::Any> = if hold {
            Box::new(dev)
        } else {
            drop(dev);
            Box::new(())
        };
        Ok((stream, 44_100, "fake".into(), Steps::default()))
    })
}

fn opening(opt: Opt, name: &str, opener: Opener) -> AudioEngine {
    let a = assets(name);
    let t = table(&a);
    let (mut e, m) = AudioEngine::build(a, t, 48_000, opt);
    fill(&e.cache);
    e.output = spawn_output(m, opener);
    e
}

/// Ticks the engine until `done` (5 s at most).
fn until(e: &mut AudioEngine, done: impl Fn(&AudioEngine) -> bool) -> bool {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(5) {
        e.tick();
        if done(e) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    false
}

/// `AUDIOFILE=0`: during the opening, as without a device (no sound, no subtitle);
/// afterwards, sounds played to the end; engine released: thread and stream released.
#[test]
fn background_open_without_file_drops_sounds() {
    let marker = Arc::new(());
    let mut e = opening(Opt { file: false, ..Opt::NEW }, "background-a", fake(80, true, false, marker.clone()));
    e.set_subtitles(true);
    assert!(e.is_opening() && !e.has_output());
    assert!(e.play_request(mono_at([0.0; 3], 1)).is_none(), "sound accepted during the opening");
    assert!(e.take_subtitles().is_empty() && e.active() == 0);
    assert!(until(&mut e, |e| !e.is_opening()), "opening never finished");
    assert!(e.has_output());
    assert_eq!((e.sample_rate(), e.current_device_name()), (44_100, "fake"));
    let h = e.play_request(mono_at([0.0; 3], 1)).expect("sound launched");
    assert_eq!(e.take_subtitles().len(), 1);
    assert!(until(&mut e, |e| !e.is_playing(h)), "sound never finished by the mixer");
    drop(e);
    assert_eq!(Arc::strong_count(&marker), 1, "stream or output thread still alive");
}

/// `AUDIOFILE` (default): sound accepted during the opening then played. Sabotage (stream not kept
/// by the output thread): the sound is never played to the end.
#[test]
fn background_open_queues_with_file_option() {
    let file = Opt::NEW;
    let mut e = opening(file, "background-b", fake(80, true, false, Arc::new(())));
    assert!(e.is_opening() && e.has_output());
    let h = e.play_request(mono_at([0.0; 3], 1)).expect("sound queued");
    assert!(until(&mut e, |e| !e.is_playing(h)), "queued sound lost");
    let mut e = opening(file, "background-b-sabotage", fake(80, false, false, Arc::new(())));
    let h = e.play_request(mono_at([0.0; 3], 1)).expect("sound queued");
    assert!(!until(&mut e, |e| !e.is_playing(h)), "the sabotage should lose the sound");
}

/// Opening refused: back to "no output", queued sounds forgotten, thread ended.
#[test]
fn failed_background_open() {
    let marker = Arc::new(());
    let mut e = opening(Opt { file: true, ..Opt::NEW }, "background-failure", fake(30, true, true, marker.clone()));
    let h = e.play_request(mono_at([0.0; 3], 1)).expect("sound queued");
    assert!(until(&mut e, |e| !e.is_opening()));
    assert!(!e.has_output() && e.active() == 0 && !e.is_playing(h));
    assert!(e.play_request(mono_at([0.0; 3], 2)).is_none());
    assert_eq!(Arc::strong_count(&marker), 1, "opening still alive");
    drop(e);
}

/// Background reopening (same call sequence as `reopen`) then an interface click right away,
/// like the "Device" setting: the game reloads synchronously then plays the click.
/// Returns `true` if the click is played to the end by the new output.
fn click_after_reopen(opt: Opt, name: &str) -> bool {
    let mut e = opening(opt, name, fake(10, true, false, Arc::new(())));
    assert!(until(&mut e, |e| !e.is_opening()));
    let mixer = e.rewire();
    e.output = spawn_output(mixer, fake(60, true, false, Arc::new(())));
    let req = SoundRequest::new("test.mono", Category::Ui).volume(0.25);
    let Some(click) = e.play_request(req) else { return false };
    until(&mut e, |e| !e.is_opening() && !e.is_playing(click))
}

#[test]
fn click_after_reopen_is_played() {
    assert!(click_after_reopen(Opt::NEW, "click-reopen"), "click of the setting lost");
    // Without queueing, the click is lost: the test does discriminate.
    assert!(!click_after_reopen(Opt { file: false, ..Opt::NEW }, "click-reopen-no-queue"));
}

/// Background reopening: sound requested during the switch played afterwards; the old stream is
/// kept until the reply of the new output, then released.
#[test]
fn background_reopen_releases_old_output() {
    let old = Arc::new(());
    let mut e = opening(Opt::NEW, "background-reopen", fake(10, true, false, old.clone()));
    assert!(until(&mut e, |e| !e.is_opening()));
    let mixer = e.rewire();
    e.output = spawn_output(mixer, fake(60, true, false, Arc::new(())));
    assert!(Arc::strong_count(&old) > 1, "old output released before the reply of the new one");
    let early = e.play_request(mono_at([0.0; 3], 1)).expect("sound during the switch queued");
    assert!(until(&mut e, |e| !e.is_opening()));
    let h = e.play_request(mono_at([0.0; 3], 1)).expect("sound launched");
    assert!(until(&mut e, |e| !e.is_playing(h) && !e.is_playing(early)));
    let t = Instant::now();
    while Arc::strong_count(&old) > 1 && t.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(Arc::strong_count(&old), 1, "old output orphaned");
}

fn listener_at(x: f64) -> Command {
    Command::Listener(crate::mixer::Listener { pos: [x, 0.0, 0.0], forward: [0.0, 0.0, -1.0], up: [0.0, 1.0, 0.0] })
}

/// Audio thread frozen beyond `OUTBOX_MAX`: the last gain of a voice and the last listener
/// survive the compaction; an outbox full of non-compactable commands does not redo a
/// full walk at every send (doubled threshold).
#[test]
fn overfull_outbox_keeps_latest_state() {
    let (mut e, _m) = headless(Opt { listener: false, ..Opt::NEW }, "full-outbox");
    let a = e.play_request(loop_at([0.0; 3], 1)).expect("loop A");
    for i in 0..CMD_CAP + 10 {
        e.send(listener_at(i as f64));
    }
    assert!(!e.outbox.is_empty(), "channel not full");
    // Gain changed only once: never sent again afterwards.
    e.set_sound_params(a, 0.3, 1.0);
    let gain = e.instances[&a.0].params.gain;
    let last = (OUTBOX_MAX + 10) as f64;
    for i in 0..=OUTBOX_MAX + 10 {
        e.send(listener_at(i as f64));
    }
    let gains: Vec<f32> = e.outbox.iter().filter_map(|c| if let Command::Gain(id, g) = c { (*id == a.0).then_some(*g) } else { None }).collect();
    assert_eq!(gains, [gain], "last gain of the voice lost at compaction");
    let listeners: Vec<f64> = e.outbox.iter().filter_map(|c| if let Command::Listener(l) = c { Some(l.pos[0]) } else { None }).collect();
    // A single listener kept at compaction, then those sent since (fewer than 64 here).
    assert_eq!(listeners.last(), Some(&last), "last listener lost");
    assert!(listeners.len() < 64, "{} listeners kept", listeners.len());
    // Never-compactable commands: amortized cost, not one walk per send.
    let t = Instant::now();
    for _ in 0..3 * OUTBOX_MAX {
        e.send(Command::Stop(u64::MAX));
    }
    assert!(e.outbox_limit >= e.outbox.len(), "threshold not raised after compaction");
    assert!(t.elapsed() < Duration::from_secs(2), "compaction at every send: {:?}", t.elapsed());
}

/// `AUDIOOUTBOX=0`, full channel: the total stop of the reopening is lost, the old output
/// must be released right away (it would replay its sounds while the new one opens).
#[test]
fn reopen_without_outbox_releases_old_output_when_stop_lost() {
    let old = Arc::new(());
    let marker = old.clone();
    // Audio thread frozen: the mixer is kept without ever being run, the channel fills up.
    let frozen: Opener = Box::new(move |m: Mixer| {
        Ok((Box::new((m, marker)) as Box<dyn std::any::Any>, 44_100, "frozen".into(), Steps::default()))
    });
    let mut e = opening(Opt { outbox: false, ..Opt::NEW }, "lost-stop", frozen);
    assert!(until(&mut e, |e| !e.is_opening()));
    let a = e.play_request(loop_at([0.0; 3], 1)).expect("loop A");
    for i in 0..CMD_CAP + 10 {
        e.set_sound_params(a, if i % 2 == 0 { 0.5 } else { 0.6 }, 1.0);
    }
    let mixer = e.rewire();
    e.output = spawn_output(mixer, fake(3000, true, false, Arc::new(())));
    let t = Instant::now();
    while Arc::strong_count(&old) > 1 && t.elapsed() < Duration::from_secs(1) {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(Arc::strong_count(&old), 1, "old output kept although its stop is lost");
}

// --- C4-M24: allocation-free launch (`RUSTCRAFT_OPT_SOUNDIDS`) ---

/// Reserve counters equal to the recount of the instances.
fn check_counts(e: &AudioEngine) {
    let n = e.instances.values().filter(|i| i.stream).count();
    assert_eq!(e.counts, [e.instances.len() - n, n], "reserve counters");
}

/// Mixed scenario: direct launches and requests (loops, repeated with delay, deferred),
/// stops, subtitles. Returns the mixer output (bits), the subtitles and the channel state.
fn ids_run(opt: Opt, name: &str) -> (Vec<u32>, Vec<String>, Vec<String>) {
    let (mut e, mut m) = headless(opt, name);
    e.set_subtitles(true);
    e.set_listener([0.0, 64.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]);
    let mut out = vec![0.0f32; 2 * 480];
    let (mut bits, mut subs, mut state) = (Vec::new(), Vec::new(), Vec::new());
    let mut loops = Vec::new();
    for b in 0..600i64 {
        let p = [(b % 13) as f64 - 6.0, 64.0, (b % 7) as f64 * -2.0];
        match b % 9 {
            0 => {
                e.play("test.mono", Category::Block, Some(p), 0.5 + (b % 3) as f32 * 0.4, 1.2, Some(b));
            }
            1 => {
                e.play("minecraft:test.stereo", Category::Ui, None, 1.0, 0.9, Some(b));
            }
            2 => {
                let r = SoundRequest::new("test.mono", Category::Neutral).at(p).seed(Some(b)).looping(7);
                loops.extend(e.play_request(r));
            }
            3 => {
                loops.extend(e.play_request(loop_at(p, b)));
            }
            4 => {
                e.play_request(mono_at(p, b).delay(3));
            }
            5 => {
                // Unknown, empty, zero volume: refused alike.
                e.play("test.absent", Category::Block, Some(p), 1.0, 1.0, Some(b));
                e.play("intentionally_empty", Category::Block, Some(p), 1.0, 1.0, Some(b));
                e.play("test.mono", Category::Block, Some(p), 0.0, 1.0, Some(b));
            }
            6 if loops.len() > 4 => {
                let h = loops.remove(0);
                e.stop_handle(h);
            }
            7 if b % 90 == 7 => e.stop(Some(Category::Ambient), Some("test.loop")),
            _ => {}
        }
        for _ in 0..2 {
            m.process(&mut out);
            bits.extend(out.iter().map(|x| x.to_bits()));
        }
        e.tick();
        check_counts(&e);
        // Repeated subtitles of the same tick in the order of the channel table (hash with a
        // random seed, already true before): compared sorted by tick.
        let mut batch: Vec<String> = e.take_subtitles().into_iter().map(|s| format!("{s:?}")).collect();
        batch.sort_unstable();
        subs.extend(batch);
        let mut ids: Vec<_> = e.instances.iter().map(|(id, i)| (*id, &*i.event, &*i.path, i.state)).collect();
        ids.sort_by_key(|x| x.0);
        state.push(format!("{ids:?}"));
    }
    (bits, subs, state)
}

/// Same mixer output bit for bit (so same launches), same subtitles (including repeated),
/// same channels, old and new launch.
#[test]
fn sound_ids_bit_exact() {
    let old = ids_run(Opt { ids: false, ..Opt::NEW }, "ids-old");
    let new = ids_run(Opt::NEW, "ids-new");
    assert!(old.0.iter().any(|&b| b != 0), "silent output: empty proof");
    assert!(old.1.iter().any(|s| s.contains("repeated: true")), "no repeated subtitle: empty proof");
    let first = old.0.iter().zip(&new.0).position(|(a, b)| a != b);
    assert_eq!(first, None, "first difference at sample {first:?}");
    assert_eq!(old.1, new.1, "subtitles");
    assert_eq!(old.2, new.2, "channels");
}

/// 26.3 game table, all events, reserves saturated then released: same handles,
/// same voice parameters, same files, same subtitles; counters right at every step.
fn vanilla_launches(opt: Opt) -> Option<Vec<String>> {
    let install = rc_testdata::need(rc_data::locate("26.3").ok(), "installed 26.3 game")?;
    let index = install.asset_index();
    let assets = rc_testdata::need(
        Assets::for_install(&install.client_jar, &[], index.as_ref().map(|(i, o)| (i.as_path(), o.as_path()))).ok(),
        "26.3 game resources",
    )?;
    let table = SoundTable::load(&[&assets]);
    let mut ids: Vec<String> = table.ids().map(str::to_owned).collect();
    ids.sort_unstable();
    let (mut e, _m) = AudioEngine::headless_with(Arc::new(assets), table, 48_000, opt);
    // No loading: every sound stays "loading", the result depends on no thread.
    let (jobs, job_rx) = crossbeam_channel::unbounded();
    e.jobs = jobs;
    e.set_subtitles(true);
    let mut log = Vec::new();
    for (k, id) in ids.iter().enumerate() {
        let short = id.strip_prefix("minecraft:").filter(|_| k % 2 == 0).unwrap_or(id);
        let cat = Category::ALL[k % Category::COUNT];
        let pos = (k % 3 != 0).then_some([k as f64 * 0.5, 64.0, -(k as f64)]);
        let h = if k % 5 == 0 {
            let r = SoundRequest::new(short, cat).seed(Some(k as i64)).looping(if k % 10 == 0 { 0 } else { 4 });
            e.play_request(r)
        } else {
            e.play(short, cat, pos, (k % 4) as f32 * 0.4, 0.7 + (k % 3) as f32 * 0.3, Some(k as i64 * 31))
        };
        let inst = h.and_then(|h| e.instances.get(&h.0));
        log.push(format!("{id} {h:?} {:?}", inst.map(|i| (&*i.event, &*i.path, i.params, i.stream))));
        check_counts(&e);
        if k % 300 == 299 {
            e.stop_all();
            for _ in 0..21 {
                e.tick();
            }
            check_counts(&e);
            assert_eq!(e.active(), 0);
        }
        log.extend(e.take_subtitles().into_iter().map(|s| format!("{s:?}")));
    }
    drop(job_rx);
    Some(log)
}

#[test]
fn sound_ids_vanilla_launches_identical() {
    let Some(old) = vanilla_launches(Opt { ids: false, ..Opt::NEW }) else { return };
    let new = vanilla_launches(Opt::NEW).expect("table loaded twice");
    let refused = old.iter().filter(|l| l.contains(" None None")).count();
    let started = old.iter().filter(|l| l.contains(" Some(SoundHandle(")).count();
    assert!(refused > 100 && started > 500, "empty proof: {started} launched, {refused} refused");
    let first = old.iter().zip(&new).position(|(a, b)| a != b);
    assert_eq!(first, None, "first difference: {:?}", first.map(|i| (&old[i], &new[i])));
    assert_eq!(old.len(), new.len());
}
