//! Real loop turns (`Server::run_turn`) with the end-of-tick relight on the light thread
//! (`light_async.rs`) against the same turns with the relight on the server thread: the same
//! light, block and chat packets in the same order (a command reads the light), the same light
//! written by each save, the same layers and dirty marks at the end. Every lot of the threaded
//! side waits for its join before it even starts (`LightAsync::hold`): each phase that joins meets
//! it unfinished and must wait for it. The scenario makes each phase the first reader in turn
//! (head of the next tick, a packet, a chunk sent again, the automatic save, the pause, chunks
//! read then generated, closing) and checks where the lot was joined. Sabotages: each of those
//! joins skipped in turn is caught.
//! Fresh flat world: the 26.3 data cache and the client jar are required (absent: FAILURE).

use std::sync::Arc;
use std::time::{Duration, Instant};

use rc_client::Event;
use rc_testdata::need;

use crate::blocks::probe::Probe;
use crate::generation::Out;
use crate::light_async::{SKIPPED_JOIN, Site, light_digest};
use crate::newworld::{NewWorld, Preset, create_world};
use crate::server::Server;
use crate::tickrate::Schedule;

fn version_data() -> Arc<rc_data::VersionData> {
    let data = Arc::new(rc_testdata::version_data().expect("light thread tests: 26.3 data cache missing"));
    assert!(crate::registries::client_jar(&data).is_some(), "light thread tests: 26.3 client jar missing");
    data
}

fn cmd(s: &mut Server, text: &str) {
    let src = s.server_source();
    assert!(s.perform(src, text).0, "command refused: {text}");
}

/// One server of the comparison, driven turn by turn like `Server::run`.
struct Side {
    p: Probe,
    clock: Schedule,
    last: Instant,
    /// The generator's own output channel once a turn reads a relay instead (kept open).
    generator_out: Option<crossbeam_channel::Receiver<Out>>,
}

/// Flat world, chunks −3..=3 loaded and known to the client, view centred (no tracking change),
/// automatic save off until asked. Under a stone roof (the light a command reads is the block
/// light), a sticky piston fed by an observer clock pushes a glowstone block back and forth in
/// chunk (2, 0): its light changes every few ticks, inside the tick, over chunks 1 to 3.
fn side(tag: &str, threaded: bool) -> Side {
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!(
        "rc-light-async-{tag}-{}-{}",
        u8::from(threaded),
        std::process::id()
    )));
    let mut w = NewWorld::new(tag);
    w.seed = 1;
    w.preset = Preset::Flat;
    w.allow_commands = true;
    create_world(&dir, &version_data(), &w).expect("world creation");
    let mut p = Probe::open(&dir, version_data());
    p.s.pos = [0.5, -60.0, 0.5];
    p.s.loaded = true;
    p.s.teleport = None;
    let around: Vec<(i32, i32)> = (-3..=3).flat_map(|x| (-3..=3).map(move |z| (x, z))).collect();
    p.s.load_now(&around);
    p.s.sent.extend(around);
    p.s.center = Some((0, 0));
    p.s.light_async.on = threaded;
    p.s.light_async.hold = threaded;
    p.s.light_async.saves = Some(Vec::new());
    p.s.turn_without_sleep = true;
    p.s.autosave = u64::MAX;
    for c in [
        "fill 20 -58 -12 63 -58 28 minecraft:stone",
        "setblock 43 -60 8 minecraft:glowstone",
        "setblock 42 -60 8 minecraft:sticky_piston[facing=east]",
        "setblock 40 -60 8 minecraft:observer[facing=east]",
        "setblock 41 -60 8 minecraft:observer[facing=west]",
    ] {
        cmd(&mut p.s, c);
    }
    let _ = p.take_events();
    Side { p, clock: Schedule::new(Instant::now()), last: Instant::now(), generator_out: None }
}

/// The packets the light shows in: light updates, chunks, blocks, sections announced ahead of
/// their light, chat lines (command results).
fn packet(e: &Event) -> Option<String> {
    matches!(
        e,
        Event::LightUpdate { .. }
            | Event::Chunk(_)
            | Event::BlocksAheadOfLight { .. }
            | Event::BlockUpdate { .. }
            | Event::SectionUpdate { .. }
            | Event::Chat { .. }
    )
    .then(|| format!("{e:?}"))
}

/// One loop turn; false once the world closed.
fn turn(s: &mut Side) -> (bool, Vec<String>) {
    let open = s.p.s.run_turn(&mut s.clock, &mut s.last);
    (open, s.p.take_events().iter().filter_map(packet).collect())
}

/// What happens before a turn, on both sides alike.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Step {
    /// Nothing: the head of the next tick is the first reader.
    Quiet,
    /// A command packet that reads the light beside the glowstone.
    Packet,
    /// Chunk (3, 0), whose light the lots compute, sent again in the turn.
    Resend,
    /// Automatic save due in the turn.
    Autosave,
    /// Pause (its save), lifted at the next step.
    Pause,
    Unpause,
    /// A chunk beside chunk 3 read (absent, generated), the generator's outputs for it handed to
    /// the turn at once: the read meets the lot of the turn before.
    Read,
    /// The same, the generator's outputs meeting the lot of the turn's own tick.
    Generate,
    /// The client leaves: closing.
    Close,
}

impl Step {
    /// The phase that must join a lot during this step, if it meets one; whether a lot must be in
    /// flight before the step (`true`: the turn before's, met first by this phase or by the step's
    /// action; otherwise the turn's own tick launches the lot this phase meets).
    fn site(self) -> Option<(Site, bool)> {
        match self {
            Step::Quiet => Some((Site::Tick, true)),
            Step::Packet => Some((Site::Packets, true)),
            Step::Resend => Some((Site::Send, false)),
            Step::Autosave => Some((Site::Save, false)),
            Step::Pause => Some((Site::Save, true)),
            Step::Unpause => None,
            Step::Read => Some((Site::Adopt, true)),
            Step::Generate => Some((Site::Generator, false)),
            Step::Close => Some((Site::Close, true)),
        }
    }
}

/// Before the turn: the step's action.
fn act(s: &mut Side, step: Step, attempt: i32) {
    match step {
        Step::Quiet | Step::Close => {}
        Step::Packet => {
            // Every lot moves the glowstone off or onto x = 43 or 44: one of these reads changes.
            for x in [43, 44] {
                let c = format!(
                    "execute positioned {x} -60 7 if predicate \
                     {{condition:\"minecraft:location_check\",predicate:{{light:{{light:{{min:14}}}}}}}}"
                );
                s.p.session.link.send_command(&c).expect("command sent");
            }
        }
        Step::Resend => {
            s.p.s.sent.remove(&(3, 0));
            s.p.s.queue.push((3, 0));
        }
        Step::Autosave => s.p.s.autosave = 1,
        Step::Pause => s.p.s.paused.store(true, std::sync::atomic::Ordering::Release),
        Step::Unpause => s.p.s.paused.store(false, std::sync::atomic::Ordering::Release),
        Step::Read | Step::Generate => {
            // A new chunk for each attempt, the generated ones first beside the chunks the lots
            // write (3, −1..=1).
            let n = attempt + if step == Step::Read { 40 } else { 0 };
            let col = (4 + n / 3, n % 3 - 1);
            s.p.s.ensure_loaded(&[col]);
            let Some(g) = s.p.s.generator.as_mut() else { panic!("no generator") };
            let (relay, rx) = crossbeam_channel::unbounded();
            let own = std::mem::replace(&mut g.rx, rx);
            let own = s.generator_out.take().unwrap_or(own);
            // Outputs up to the end of the batch that returns the chunk, in their order; what the
            // thread computes later stays on its channel until the next attempt, on both sides alike.
            let mut full = false;
            loop {
                let o = own.recv_timeout(Duration::from_secs(300)).expect("generator output");
                let end = full && matches!(o, Out::Batch(_));
                full |= matches!(&o, Out::Full(c, _) if (c.x, c.z) == col);
                relay.send(o).expect("relay");
                if end {
                    break;
                }
            }
            s.generator_out = Some(own);
        }
    }
    if step == Step::Close {
        // The client quits: the next turn closes the world.
        s.p.session.link.disconnect();
    }
}

/// Every step of the scenario, in order; each one that joins is tried again (at most 40 turns)
/// until the threaded side meets a lot there.
const STEPS: [Step; 12] = [
    Step::Quiet,
    Step::Packet,
    Step::Quiet,
    Step::Resend,
    Step::Autosave,
    Step::Pause,
    Step::Unpause,
    Step::Read,
    Step::Generate,
    Step::Packet,
    Step::Quiet,
    Step::Close,
];

/// Plays the scenario on both sides, `skipped`: that join skipped on the threaded side. Err: the
/// first difference between the sides.
fn scenario(tag: &str, skipped: Option<Site>) -> Result<(), String> {
    let mut a = side(tag, false);
    let mut b = side(tag, true);
    // Setup at rest (the roof's light, the clock started), both sides alike.
    for _ in 0..20 {
        let (_, pa) = turn(&mut a);
        let (_, pb) = turn(&mut b);
        if pa != pb {
            return Err("packets differ during the setup".into());
        }
    }
    SKIPPED_JOIN.with(|s| s.set(skipped));
    let r = play(&mut a, &mut b);
    SKIPPED_JOIN.with(|s| s.set(None));
    r
}

fn play(a: &mut Side, b: &mut Side) -> Result<(), String> {
    let mut seen = Vec::new();
    for (k, step) in STEPS.into_iter().enumerate() {
        let mut met = false;
        for attempt in 0..40 {
            let wanted = step.site();
            // A step that joins a lot already in flight needs one.
            if wanted.is_some_and(|(_, before)| before) && !b.p.s.light_async.in_flight() {
                let (_, pa) = turn(a);
                let (_, pb) = turn(b);
                if pa != pb {
                    return Err(format!("step {k} ({step:?}), waiting turn: packets differ"));
                }
                continue;
            }
            let joins = b.p.s.light_async.joins.len();
            act(a, step, attempt);
            act(b, step, attempt);
            let (open_a, pa) = turn(a);
            let (open_b, pb) = turn(b);
            if pa != pb {
                return Err(format!("step {k} ({step:?}), attempt {attempt}: packets differ"));
            }
            if open_a != open_b {
                return Err(format!("step {k} ({step:?}): one side closed"));
            }
            let new = &b.p.s.light_async.joins[joins..];
            met = wanted.is_none_or(|(site, _)| new.contains(&site));
            seen.extend_from_slice(new);
            if met || !open_b {
                break;
            }
        }
        if !met {
            return Err(format!("step {k} ({step:?}): no lot met there"));
        }
    }
    if a.p.s.light_async.saves != b.p.s.light_async.saves {
        return Err("the light written by a save differs".into());
    }
    if light_digest(&a.p.s, false) != light_digest(&b.p.s, false) {
        return Err("layers or dirty marks differ at the end".into());
    }
    for site in [Site::Tick, Site::Packets, Site::Send, Site::Save, Site::Adopt, Site::Generator, Site::Close] {
        assert!(seen.contains(&site), "no lot joined at {site:?}: {seen:?}");
    }
    Ok(())
}

/// Every phase that reads or writes the light's outputs first joins the lot, and the threaded side
/// sends, saves and holds what the server thread's relight gives.
#[test]
fn threaded_relight_gives_the_server_thread_s_outputs_at_every_join() {
    if let Err(e) = scenario("same", None) {
        panic!("{e}");
    }
}

/// Each join skipped is caught: a packet, a save or the final state differs, or a guard sees the
/// lot in flight (head of the tick: the broadcast's guard).
#[test]
fn every_skipped_join_is_caught() {
    for site in [Site::Tick, Site::Packets, Site::Send, Site::Save, Site::Generator] {
        let r =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| scenario(&format!("skip-{site:?}"), Some(site))));
        SKIPPED_JOIN.with(|s| s.set(None));
        match r {
            Ok(Ok(())) => panic!("join at {site:?} skipped: not caught"),
            Ok(Err(e)) => eprintln!("join at {site:?} skipped: {e}"),
            Err(p) => {
                // Only a guard's panic counts: any other failure of the scenario proves nothing.
                let text =
                    p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()));
                let text = text.unwrap_or_default();
                assert!(text.contains("light lot in flight"), "join at {site:?} skipped: unexpected panic: {text}");
                eprintln!("join at {site:?} skipped: {text}");
            }
        }
    }
}

/// Measurement (not a proof), step 0 of the plan: what the launch's copy of the columns costs on
/// the server thread, per tick and per lot, in the scene bench's flight (`perf_fly`: 2 blocks
/// per tick from the model world's plains, view 12), ocean and redstone sites; real loop turns at
/// 20 ticks per second, 100 warm-up then 400 measured; and the mean tick as the game counts it
/// (outside the sleep) with the relight on the server thread then on its own. Run alone, quiet
/// machine: `cargo test --release -p rc-sim bench_light_async -- --ignored --nocapture`.
#[test]
#[ignore = "measurement bench"]
fn bench_light_async() {
    use std::sync::atomic::Ordering::Relaxed;
    let Some(model) = need(crate::tests_opt::model_dir(), "bench model world") else { return };
    let Some(data) = need(rc_testdata::version_data(), "26.3 data cache") else { return };
    crate::mobs::index::set_verify(false);
    crate::mobs::mob::cross_checks::turn_off_for_a_bench();
    let data = Arc::new(data);
    if need(crate::registries::client_jar(&data), "jar client 26.3").is_none() {
        return;
    }
    type Spot<'a> = (&'a str, f64, f64, f64, &'a [&'a str]);
    let sites: [Spot; 3] = [
        ("flight", 1600.5, 96.0, -447.5, &["tag @s add perf_fly"]),
        ("ocean", -479.5, 72.0, -703.5, &[]),
        ("redstone", 2240.5, 73.0, 166.5, &["function perf:redstone"]),
    ];
    let only = std::env::var("RUSTCRAFT_BENCH_SITES").ok();
    for (name, x, y, z, rig) in
        sites.into_iter().filter(|s| only.as_deref().is_none_or(|o| o.split(',').any(|n| n == s.0)))
    {
        for threaded in [false, true] {
            let dir = crate::tests::fresh_copy(&model, &format!("lightasync-{name}-{}", u8::from(threaded)));
            let (mut s, session) = crate::portal_tests::open_world_session(&dir, data.clone());
            crate::tests_opt::setup(&mut s);
            s.sim_dist = 6;
            s.sim_req.store(6, Relaxed);
            s.view = 12;
            s.light_async.on = threaded;
            crate::tests_opt::go(&mut s, crate::dims::OVERWORLD, x, Some(y), z, false);
            for c in rig {
                let src = s.server_source();
                let _ = s.perform(src, c);
            }
            let (mut clock, mut last) = (Schedule::new(Instant::now()), Instant::now());
            let stats = || {
                use crate::light_async::stats::{COLUMNS, COPY_NS, LAUNCHES};
                (LAUNCHES.load(Relaxed), COLUMNS.load(Relaxed), COPY_NS.load(Relaxed))
            };
            let (mut l0, mut c0, mut n0) = (0, 0, 0);
            let mut tick_ns = 0u64;
            // 100 warm-up turns, then 400 measured.
            for i in 0..500 {
                if i == 100 {
                    (l0, c0, n0) = stats();
                }
                assert!(s.run_turn(&mut clock, &mut last), "world closed");
                while session.events.try_recv().is_ok() {}
                if i >= 100 && (i + 1) % 100 == 0 {
                    tick_ns += s.cmd.ticking.average_tick_nanos();
                }
            }
            let (l1, c1, n1) = stats();
            let (lots, cols, copy) = (l1 - l0, c1 - c0, (n1 - n0) as f64);
            eprintln!(
                "bench_light_async {name} {}: mean tick {:.3} ms; lots {lots}, columns copied per lot {:.1}, copy {:.4} ms per tick, {:.1} µs per lot",
                if threaded { "thread" } else { "server" },
                tick_ns as f64 / 4.0 / 1e6,
                cols as f64 / lots.max(1) as f64,
                copy / 400.0 / 1e6,
                copy / lots.max(1) as f64 / 1e3
            );
            s.stop_gen();
            s.stop_parked();
            drop(s);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
