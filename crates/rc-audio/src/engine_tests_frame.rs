//! Equivalence proofs of the per-frame and command savings (C4-M29): listener written only
//! when it changes, idle polls skipped, per-frame poll rate limited, duplicate commands
//! dropped. Synthetic table, simulated output, simulated clock: nothing depends on the machine.

use std::cell::Cell;

use super::tests_opt::{headless, loop_at};
use super::*;

thread_local! {
    /// Simulated time of the rate-limit proofs, in nanoseconds since [`clock_origin`].
    static FAKE_NANOS: Cell<u64> = const { Cell::new(0) };
}

fn clock_origin() -> Instant {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    *ORIGIN.get_or_init(Instant::now)
}

fn fake_now() -> Instant {
    clock_origin() + Duration::from_nanos(FAKE_NANOS.with(Cell::get))
}

/// Old frame paths (listener always written, full poll every frame, every command sent).
const FRAME_OLD: Opt =
    Opt { listener_diff: false, pump_skip_idle: false, pump_rate_limit: false, command_dedup: false, ..Opt::NEW };

/// Mixer output, channel states after each tick, listener writes and per-frame polls.
struct FrameRun {
    bits: Vec<u32>,
    ticks: Vec<String>,
    writes: u64,
    frame_pumps: u64,
}

/// Frames every 0.25 ms (simulated clock), a mixer block of 10 ms every 40 frames, a tick
/// every 200 frames; listener still for the first 500 blocks, then turning and moving.
/// Loops, sounds repeated with a delay (their relaunch tick is set by the poll that sees
/// their end) and one-shot sounds.
fn frame_run(opt: Opt, name: &str) -> FrameRun {
    let (mut e, mut m) = headless(opt, name);
    e.clock = fake_now;
    FAKE_NANOS.with(|c| c.set(0));
    e.last_pump = fake_now();
    e.set_subtitles(true);
    for i in 0..4 {
        e.play_request(loop_at([f64::from(i) * 3.0 - 4.0, 64.0, -2.0], i64::from(i))).expect("loop");
    }
    for i in 0..3u32 {
        let req = SoundRequest::new("test.mono", Category::Neutral).at([2.0, 64.0, f64::from(i)]).seed(Some(i64::from(i)));
        e.play_request(req.looping(3 + i)).expect("repeated sound");
    }
    let mut out = vec![0.0f32; 2 * 480];
    let mut run = FrameRun { bits: Vec::new(), ticks: Vec::new(), writes: 0, frame_pumps: 0 };
    for f in 0..40_000u32 {
        FAKE_NANOS.with(|c| c.set(c.get() + 250_000));
        let a = f as f32 * 0.003;
        if f < 20_000 {
            e.set_listener([0.0, 64.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]);
        } else {
            // Turning at every frame, moving every 8th: a write that missed either would show.
            let x = f64::from(f / 8) * 8e-3;
            e.set_listener([x, 64.0, 0.0], [a.sin(), 0.1, -a.cos()], [0.0, 1.0, 0.0]);
        }
        if f % 40 == 39 {
            m.process(&mut out);
            run.bits.extend(out.iter().map(|x| x.to_bits()));
        }
        if f % 200 == 199 {
            if f % 1400 == 199 {
                e.play_request(SoundRequest::new("test.mono", Category::Block).at([-3.0, 64.0, 1.0]).seed(Some(i64::from(f))));
            }
            e.tick();
            let mut state: Vec<_> = e.instances.iter().map(|(id, i)| (*id, i.state, i.start_tick)).collect();
            state.sort_by_key(|x| x.0);
            let mut due: Vec<_> = e.delayed.iter().map(|d| (d.id, d.due)).collect();
            due.sort_unstable();
            let mut subs: Vec<String> = e.take_subtitles().into_iter().map(|s| format!("{s:?}")).collect();
            subs.sort_unstable();
            run.ticks.push(format!("{state:?} {due:?} {subs:?}"));
        }
    }
    run.writes = e.slot.writes();
    run.frame_pumps = e.frame_pumps;
    run
}

/// Listener written only on change, idle polls skipped, rate-limited poll: same mixer output
/// bit for bit, same ends, relaunches and subtitles at every tick, alone and together.
#[test]
fn frame_savings_bit_exact() {
    let old = frame_run(FRAME_OLD, "frame-old");
    assert!(old.bits.iter().any(|&b| b != 0), "silent output: empty proof");
    let relaunches = old.ticks.windows(2).filter(|w| w[0] != w[1]).count();
    assert!(relaunches > 50, "channel state barely changes: empty proof ({relaunches})");
    let variants = [
        ("listener_diff", Opt { listener_diff: true, ..FRAME_OLD }),
        ("pump_skip_idle", Opt { pump_skip_idle: true, ..FRAME_OLD }),
        ("pump_rate_limit", Opt { pump_rate_limit: true, ..FRAME_OLD }),
        ("all", Opt { listener_diff: true, pump_skip_idle: true, pump_rate_limit: true, ..FRAME_OLD }),
    ];
    for (name, opt) in variants {
        let new = frame_run(opt, &format!("frame-{name}"));
        let first = old.bits.iter().zip(&new.bits).position(|(a, b)| a != b);
        assert_eq!(first, None, "{name}: first difference at sample {first:?}");
        assert_eq!(old.bits.len(), new.bits.len(), "{name}");
        let tick = old.ticks.iter().zip(&new.ticks).position(|(a, b)| a != b);
        assert_eq!(tick, None, "{name}: first tick that differs: {:?}", tick.map(|t| (&old.ticks[t], &new.ticks[t])));
    }
}

/// The savings do save: a still listener is written once (creation), a moving one at every
/// frame; the rate-limited poll runs once per simulated millisecond (every 4th frame).
#[test]
fn frame_savings_skip_the_work() {
    let old = frame_run(FRAME_OLD, "frame-count-old");
    let diff = frame_run(Opt { listener_diff: true, ..FRAME_OLD }, "frame-count-diff");
    assert_eq!(old.writes, 1 + 40_000, "old path: one write per frame");
    assert_eq!(diff.writes, 1 + 1 + 20_000, "still: written once; moving: every frame");
    assert_eq!(old.frame_pumps, 40_000);
    let limited = frame_run(Opt { pump_rate_limit: true, ..FRAME_OLD }, "frame-count-limit");
    assert_eq!(limited.frame_pumps, 10_000, "one per simulated millisecond");
}

/// Loads in flight counted: a sound absent from the cache is answered (here: a load failure,
/// the test file is not a real Ogg) and its channel released, the counter back at zero.
/// Sabotage (counter not incremented): the answer is never read, the sound stays loading.
#[test]
fn idle_poll_still_receives_loads() {
    let (mut e, _m) = headless(Opt::NEW, "pump-loads");
    e.clear_cache();
    let h = e.play_request(loop_at([0.0; 3], 1)).expect("sound queued for loading");
    assert_eq!(e.instances[&h.0].state, State::Loading);
    let t = Instant::now();
    while e.is_playing(h) && t.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(1));
        e.set_listener([0.0; 3], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]);
    }
    assert!(!e.is_playing(h), "load answer never received");
    assert_eq!(e.loads_in_flight, 0);
}

/// Commands of a run: per block, attached sounds moved (half of them stand still), volumes
/// re-applied every 50 blocks (mostly unchanged, the 11 sliders together), music fade, total
/// stop, reopening. `log`: commands drained instead of mixed; otherwise mixer output.
fn command_run(opt: Opt, name: &str, log: bool) -> (Vec<String>, Vec<u32>) {
    let (mut e, mut m) = headless(opt, name);
    let mut out = vec![0.0f32; 2 * 480];
    let (mut lines, mut bits) = (Vec::new(), Vec::new());
    let launch = |e: &mut AudioEngine| -> Vec<SoundHandle> {
        (0..6).map(|i| e.play_request(loop_at([f64::from(i), 64.0, 2.0], i64::from(i))).expect("loop")).collect()
    };
    let mut handles = launch(&mut e);
    for b in 0..2000usize {
        for (k, h) in handles.iter().enumerate() {
            let x = k as f64 + if k % 2 == 0 { 0.0 } else { b as f64 * 0.05 };
            e.set_position(*h, [x, 64.0, 2.0]);
        }
        if b % 50 == 0 {
            for c in Category::ALL {
                let v = if c == Category::Ambient && b % 200 == 100 { 0.5 } else { 1.0 };
                e.set_volume(c, v);
            }
        }
        if b % 70 == 0 {
            e.set_category_fade(Category::Ambient, (b % 3) as f32 * 0.4);
        }
        if b == 900 {
            e.stop_all();
            handles = launch(&mut e);
        }
        if b == 1500 {
            // The commands the reopening sends to the retired mixer (total stop) leave with it.
            if log {
                lines.extend(m.drain_log_ids(true));
                lines.push("reopen".into());
            }
            m = e.reopen_headless();
            handles = launch(&mut e);
        }
        e.set_listener([0.0, 64.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]);
        if log {
            lines.extend(m.drain_log_ids(true));
        } else {
            m.process(&mut out);
            bits.extend(out.iter().map(|x| x.to_bits()));
        }
        if b % 5 == 4 {
            e.tick();
        }
    }
    (lines, bits)
}

/// Old command log minus the commands without effect: a position equal to the voice's, a run
/// of category commands reduced to its last one, dropped if the mixer already has it.
fn without_no_ops(old: &[String]) -> Vec<String> {
    let default_cat = format!("cat {:?}", [1.0f32; Category::COUNT]);
    let mut pos: HashMap<String, String> = HashMap::new();
    let mut cat = default_cat.clone();
    let mut out: Vec<String> = Vec::new();
    for (i, line) in old.iter().enumerate() {
        if line == "reopen" {
            // New mixer: default factors, no voice.
            cat = default_cat.clone();
            pos.clear();
            out.push(line.clone());
            continue;
        }
        if let Some(rest) = line.strip_prefix("play ") {
            let id = rest.split(' ').next().unwrap_or_default().to_owned();
            let p = rest.split("pos: ").nth(1).and_then(|s| s.split(']').next()).unwrap_or_default();
            pos.insert(id, format!("{p}]"));
        } else if let Some(rest) = line.strip_prefix("pos ") {
            let (id, p) = rest.split_once(' ').unwrap_or_default();
            if pos.get(id).is_some_and(|q| q == p) {
                continue;
            }
            pos.insert(id.to_owned(), p.to_owned());
        } else if line.starts_with("cat ") {
            if old.get(i + 1).is_some_and(|n| n.starts_with("cat ")) || *line == cat {
                continue;
            }
            cat = line.clone();
        }
        out.push(line.clone());
    }
    out
}

/// Duplicate commands dropped: the new command log is the old one without its no-ops, and
/// the mixer output is the same bit for bit over 2,000 blocks (still and moving sounds).
#[test]
fn command_dedup_drops_only_no_ops() {
    let old = command_run(FRAME_OLD, "dedup-log-old", true).0;
    let new = command_run(Opt { command_dedup: true, ..FRAME_OLD }, "dedup-log-new", true).0;
    let reduced = without_no_ops(&old);
    assert!(new.len() * 3 < old.len() * 2, "nothing dropped: {} -> {}", old.len(), new.len());
    assert!(new.iter().any(|l| l.starts_with("cat ")), "no category command left: empty proof");
    let first = reduced.iter().zip(&new).position(|(a, b)| a != b);
    assert_eq!(first, None, "first difference: {:?}", first.map(|i| (&reduced[i], &new[i])));
    assert_eq!(reduced.len(), new.len());
    let old_bits = command_run(FRAME_OLD, "dedup-mix-old", false).1;
    let new_bits = command_run(Opt { command_dedup: true, ..FRAME_OLD }, "dedup-mix-new", false).1;
    assert!(old_bits.iter().any(|&b| b != 0), "silent output: empty proof");
    let first = old_bits.iter().zip(&new_bits).position(|(a, b)| a != b);
    assert_eq!(first, None, "first difference at sample {first:?}");
}

/// Interned subtitles: every subtitle of an event carries the table's own key (no copy); the
/// old path copies it at every launch. Same text either way.
#[test]
fn subtitle_keys_shared_with_table() {
    for (interned, name) in [(true, "subtitles-shared"), (false, "subtitles-copied")] {
        let (mut e, _m) = headless(Opt { subtitle_interned: interned, ..Opt::NEW }, name);
        e.set_subtitles(true);
        for seed in 0..2 {
            e.play("test.mono", Category::Block, Some([1.0, 64.0, 0.0]), 1.0, 1.0, Some(seed));
        }
        let s = e.take_subtitles();
        assert_eq!(s.len(), 2, "{name}");
        assert!(s.iter().all(|s| &*s.key == "subtitles.test.mono"), "{name}: {s:?}");
        assert_eq!(Arc::ptr_eq(&s[0].key, &s[1].key), interned, "{name}: key shared");
    }
}

/// Decoding threads: 2 on the old path; one per 8 cores between 2 and 4 otherwise.
#[test]
fn loader_threads_follow_the_machine() {
    assert_eq!(loader_threads(Opt { loader_threads: false, ..Opt::NEW }), 2);
    let cores = rc_data::env_str!("RUSTCRAFT_THREADS")
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(2, usize::from));
    assert_eq!(loader_threads(Opt::NEW), (cores / 8).clamp(2, 4), "{cores} cores");
}

/// Main-thread cost of one frame: listener + poll, camera still then moving (real clock).
fn frame_cost(opt: Opt, moving: bool) -> f64 {
    let (mut e, mut m) = headless(opt, "bench-frame");
    for i in 0..32 {
        e.play_request(loop_at([f64::from(i), 64.0, 0.0], i64::from(i)));
    }
    let mut out = vec![0.0f32; 2 * 480];
    let mut spent = Duration::ZERO;
    // 3,500 frames/s: one mixer block every 35 frames, one tick every 175; the clock is read
    // once per block of frames (its own cost is of the order of the measured one).
    const BLOCKS: u32 = 40_000;
    for b in 0..BLOCKS {
        let t = Instant::now();
        for f in b * 35..b * 35 + 35 {
            let x = if moving { f64::from(f) * 1e-4 } else { 0.0 };
            e.set_listener([x, 64.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]);
        }
        spent += t.elapsed();
        m.process(&mut out);
        if b % 5 == 0 {
            e.tick();
        }
    }
    spent.as_secs_f64() * 1e9 / f64::from(BLOCKS * 35)
}

/// Main-thread cost of a tick of attached sounds (48 entities, half still) plus the 11 volume
/// sliders applied, per tick, and the commands sent per tick.
fn command_cost(opt: Opt) -> (f64, f64) {
    let (mut e, mut m) = headless(opt, "bench-commands");
    let handles: Vec<SoundHandle> =
        (0..48).filter_map(|i| e.play_request(loop_at([f64::from(i), 64.0, 0.0], i64::from(i)))).collect();
    let mut out = vec![0.0f32; 2 * 480];
    let (mut spent, mut sent) = (Duration::ZERO, 0usize);
    const TICKS: u32 = 20_000;
    for t in 0..TICKS {
        let start = Instant::now();
        for (k, h) in handles.iter().enumerate() {
            let x = k as f64 + if k % 2 == 0 { 0.0 } else { f64::from(t) * 0.01 };
            e.set_position(*h, [x, 64.0, 0.0]);
        }
        for c in Category::ALL {
            e.set_volume(c, 1.0);
        }
        e.tick();
        spent += start.elapsed();
        sent += m.drain_log().len();
        m.process(&mut out);
    }
    (spent.as_secs_f64() * 1e9 / f64::from(TICKS), sent as f64 / f64::from(TICKS))
}

/// Bench (ignored), loaded machine figures only:
/// `cargo-file.ps1 test --release -p rc-audio frame_bench -- --ignored --nocapture`
#[test]
#[ignore = "bench"]
fn frame_bench() {
    let variants = [
        ("old", FRAME_OLD),
        ("listener_diff", Opt { listener_diff: true, ..FRAME_OLD }),
        ("pump_skip_idle", Opt { pump_skip_idle: true, ..FRAME_OLD }),
        ("pump_rate_limit", Opt { pump_rate_limit: true, ..FRAME_OLD }),
        ("diff+skip", Opt { listener_diff: true, pump_skip_idle: true, ..FRAME_OLD }),
        ("all three", Opt { listener_diff: true, pump_skip_idle: true, pump_rate_limit: true, ..FRAME_OLD }),
    ];
    for pass in 0..2 {
        for (name, opt) in variants {
            println!(
                "pass {pass} {name:>16}: still {:.1} ns/frame, moving {:.1} ns/frame",
                frame_cost(opt, false),
                frame_cost(opt, true)
            );
        }
        for (name, opt) in [("old", FRAME_OLD), ("command_dedup", Opt { command_dedup: true, ..FRAME_OLD })] {
            let (ns, cmds) = command_cost(opt);
            println!("pass {pass} {name:>16}: {ns:.0} ns/tick, {cmds:.1} commands/tick");
        }
        for (name, interned) in [("copied", false), ("interned", true)] {
            let (launch, repeat) = subtitle_cost(Opt { subtitle_interned: interned, ..Opt::NEW });
            println!("pass {pass} subtitles {name:>8}: {launch:.0} ns/launch, {repeat:.0} ns per repeated subtitle");
        }
    }
}

/// Launch with subtitles on (cost per sound, drained every 10 launches like a frame), and the
/// repeated subtitles of 64 audible loops (cost per subtitle, every 10th tick).
fn subtitle_cost(opt: Opt) -> (f64, f64) {
    let (mut e, m) = headless(opt, "bench-subtitles");
    // No mixer: commands are dropped at once instead of piling up in the outbox.
    drop(m);
    e.set_subtitles(true);
    const N: u32 = 200_000;
    let t = Instant::now();
    for k in 0..N {
        let h = e.play("test.mono", Category::Block, Some([1.0, 64.0, 0.0]), 1.0, 1.0, Some(i64::from(k)));
        if let Some(h) = h {
            e.stop_handle(h);
        }
        if k % 10 == 9 {
            std::hint::black_box(e.take_subtitles());
            e.instances.clear();
            e.counts = [0; 2];
        }
    }
    let launch = t.elapsed().as_secs_f64() * 1e9 / f64::from(N);
    for i in 0..64 {
        let req = SoundRequest::new("test.mono", Category::Neutral).at([f64::from(i), 64.0, 0.0]).seed(Some(i64::from(i)));
        e.play_request(req.looping(0));
    }
    e.take_subtitles();
    let ticks = 20_000u32;
    let t = Instant::now();
    for _ in 0..ticks {
        e.tick();
        std::hint::black_box(e.take_subtitles());
    }
    let repeats = f64::from(ticks / 10) * 64.0;
    (launch, t.elapsed().as_secs_f64() * 1e9 / repeats)
}
