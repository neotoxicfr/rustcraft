//! World clocks of the integrated server: every clock of the registry exists and ticks, its pace
//! (rate, pause, fraction of a tick) follows the game's single-precision steps, survives a save
//! and travels in the time packets; a change sends the changed clock alone.
//! Fresh flat world: the 26.3 data cache and the client jar are required (absent: FAILURE).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rc_client::Event;
use rc_proto::nbt::Nbt;

use crate::blocks::probe::Probe;
use crate::newworld::{NewWorld, Preset, create_world};
use crate::world_clocks::{self, ClockPace, SavedClock};

const OVERWORLD: &str = "minecraft:overworld";
const THE_END: &str = "minecraft:the_end";

fn version_data() -> Arc<rc_data::VersionData> {
    let data = Arc::new(rc_testdata::version_data().expect("world clock tests: 26.3 data cache missing"));
    assert!(crate::registries::client_jar(&data).is_some(), "world clock tests: 26.3 client jar missing");
    data
}

fn scratch_folder(tag: &str) -> PathBuf {
    rc_testdata::scratch(std::env::temp_dir().join(format!("rc-world-clocks-{tag}-{}", std::process::id())))
}

fn open(dir: &Path) -> Probe {
    let mut p = Probe::open(dir, version_data());
    p.s.pos = [0.5, -60.0, 0.5];
    p.s.loaded = true;
    p.s.game_mode = 0;
    p.s.teleport = None;
    let around: Vec<(i32, i32)> = (-1..=1).flat_map(|x| (-1..=1).map(move |z| (x, z))).collect();
    p.s.load_now(&around);
    let _ = p.take_events();
    p
}

fn new_world(tag: &str) -> (Probe, PathBuf) {
    let dir = scratch_folder(tag);
    let mut w = NewWorld::new(tag);
    w.seed = 1;
    w.preset = Preset::Flat;
    w.allow_commands = true;
    create_world(&dir, &version_data(), &w).expect("world creation");
    (open(&dir), dir)
}

fn cmd(p: &mut Probe, text: &str) -> i32 {
    let src = p.s.server_source();
    let (ok, result) = p.s.perform(src, text);
    assert!(ok, "command refused: {text}");
    result
}

fn game_ticks(p: &mut Probe, count: usize) {
    for _ in 0..count {
        p.s.game_tick();
        p.s.tick += 1;
    }
}

/// Clock lists of the time packets sent since the last read that carry at least one clock.
fn clock_packets(p: &Probe) -> Vec<Vec<(String, i64, f32, f32)>> {
    p.take_events()
        .into_iter()
        .filter_map(|e| match e {
            Event::Time { clocks, .. } if !clocks.is_empty() => Some(clocks),
            _ => None,
        })
        .collect()
}

fn fraction_bits(p: &Probe, clock: &str) -> u32 {
    p.s.clock_pace(clock).partial_tick.to_bits()
}

/// The game's clock tick written out step by step, in single precision.
fn reference_tick(total_ticks: &mut i64, pace: &mut ClockPace) {
    if pace.paused {
        return;
    }
    let sum: f32 = pace.partial_tick + pace.rate;
    let whole = f64::from(sum).floor() as i32;
    pace.partial_tick = sum - whole as f32;
    *total_ticks += i64::from(whole);
}

/// A new world has one clock per registry entry, in registry order, and every one ticks: the
/// End's clock counts from the creation of the world, `time query` reads it.
#[test]
fn every_registry_clock_exists_and_ticks() {
    let (mut p, _dir) = new_world("registry");
    let names: Vec<&str> = p.s.clocks.iter().map(|c| c.0.as_str()).collect();
    assert_eq!(names, [OVERWORLD, THE_END], "one clock per registry entry");
    let overworld_before = p.s.clock_total(OVERWORLD);
    game_ticks(&mut p, 5);
    assert_eq!(p.s.clock_total(THE_END), 5, "the End's clock ticks from the creation of the world");
    assert_eq!(p.s.clock_total(OVERWORLD), overworld_before + 5);
    assert_eq!(cmd(&mut p, "time of minecraft:the_end query time"), 5);
}

/// A fractional rate goes through the fraction of a tick in single precision, and so does rate 1
/// afterwards: the fraction left by the earlier rate is rounded again at every tick.
#[test]
fn fractional_rate_follows_single_precision_steps() {
    let (mut p, _dir) = new_world("steps");
    let start = p.s.clock_total(OVERWORLD);
    let mut expected = (start, ClockPace { rate: 0.3, ..ClockPace::default() });
    cmd(&mut p, "time rate 0.3");
    for tick in 0..10 {
        p.s.advance_clocks();
        reference_tick(&mut expected.0, &mut expected.1);
        assert_eq!(
            (p.s.clock_total(OVERWORLD), fraction_bits(&p, OVERWORLD)),
            (expected.0, expected.1.partial_tick.to_bits()),
            "tick {tick} at rate 0.3"
        );
    }
    // Stored values: ten ticks at 0.3 are three whole ticks and a rounding residue.
    assert_eq!(p.s.clock_total(OVERWORLD) - start, 3);
    assert_eq!(fraction_bits(&p, OVERWORLD), 0x3480_0000, "fraction after ten ticks at rate 0.3");
    cmd(&mut p, "time rate 0.7");
    p.s.advance_clocks();
    assert_eq!(fraction_bits(&p, OVERWORLD), 0x3f33_3337, "fraction after one more tick at rate 0.7");
    cmd(&mut p, "time rate 1");
    p.s.advance_clocks();
    assert_eq!(p.s.clock_total(OVERWORLD) - start, 4);
    assert_eq!(fraction_bits(&p, OVERWORLD), 0x3f33_3338, "rate 1 rounds the fraction left by rate 0.7 again");
    cmd(&mut p, "time pause");
    p.s.advance_clocks();
    assert_eq!((p.s.clock_total(OVERWORLD) - start, fraction_bits(&p, OVERWORLD)), (4, 0x3f33_3338), "paused");
    // The End's clock never left the default pace: one tick per tick, no fraction.
    assert_eq!((p.s.clock_total(THE_END), fraction_bits(&p, THE_END)), (13, 0));
}

/// `time set` and a time marker restart the fraction of a tick; `time add` keeps it.
#[test]
fn time_set_restarts_the_fraction_and_time_add_keeps_it() {
    let (mut p, _dir) = new_world("fraction");
    cmd(&mut p, "time rate 0.3");
    p.s.advance_clocks();
    let third = 0.3f32.to_bits();
    assert_eq!(fraction_bits(&p, OVERWORLD), third);
    cmd(&mut p, "time add 10");
    assert_eq!(fraction_bits(&p, OVERWORLD), third, "time add keeps the fraction");
    cmd(&mut p, "time set 1000");
    assert_eq!(fraction_bits(&p, OVERWORLD), 0, "time set restarts the fraction");
    assert_eq!(p.s.clock_total(OVERWORLD), 1000);
    p.s.advance_clocks();
    assert_eq!(fraction_bits(&p, OVERWORLD), third);
    cmd(&mut p, "time set minecraft:noon");
    assert_eq!(fraction_bits(&p, OVERWORLD), 0, "a time marker restarts the fraction");
}

/// A change of one clock sends that clock alone, with its fraction of a tick and its rate (0
/// when it stands still); entering the game sends every clock.
#[test]
fn a_change_sends_the_changed_clock_alone_with_its_fraction() {
    let (mut p, _dir) = new_world("packets");
    cmd(&mut p, "time set 100");
    assert_eq!(clock_packets(&p), [vec![(OVERWORLD.to_owned(), 100, 0.0, 1.0)]], "time set");
    cmd(&mut p, "time rate 0.3");
    assert_eq!(clock_packets(&p), [vec![(OVERWORLD.to_owned(), 100, 0.0, 0.3)]], "time rate");
    p.s.advance_clocks();
    cmd(&mut p, "time add 5");
    assert_eq!(clock_packets(&p), [vec![(OVERWORLD.to_owned(), 105, 0.3, 0.3)]], "time add carries the fraction");
    cmd(&mut p, "time pause");
    assert_eq!(clock_packets(&p), [vec![(OVERWORLD.to_owned(), 105, 0.3, 0.0)]], "a paused clock stands still");
    cmd(&mut p, "time of minecraft:the_end set 7");
    assert_eq!(clock_packets(&p), [vec![(THE_END.to_owned(), 7, 0.0, 1.0)]], "the End's clock alone");
    p.s.send_time(true);
    assert_eq!(
        clock_packets(&p),
        [vec![(OVERWORLD.to_owned(), 105, 0.3, 0.0), (THE_END.to_owned(), 7, 0.0, 1.0)]],
        "entering the game: every clock"
    );
}

/// Rate, pause and fraction of a tick are written with the world and read back; a field at its
/// default value is left out of the file, like the game does.
#[test]
fn pace_survives_a_save_and_reopen() {
    let (mut p, dir) = new_world("save");
    cmd(&mut p, "time set 500");
    cmd(&mut p, "time rate 0.3");
    p.s.advance_clocks();
    cmd(&mut p, "time of minecraft:the_end pause");
    let saved = p.s.clocks_to_save();
    assert_eq!(saved[0].pace, ClockPace { partial_tick: 0.3, rate: 0.3, paused: false });
    p.s.save(true);
    drop(p);

    let file = crate::nbtio::read_dat(&world_clocks::file_path(&dir)).expect("world_clocks.dat");
    let field_names = |clock: &str| match file.get("data").and_then(|d| d.get(clock)) {
        Some(Nbt::Compound(fields)) => fields.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
        other => panic!("clock {clock} missing from the file: {other:?}"),
    };
    assert_eq!(field_names(OVERWORLD), ["total_ticks", "partial_tick", "rate"]);
    assert_eq!(field_names(THE_END), ["total_ticks", "paused"]);

    let mut p = open(&dir);
    assert_eq!(p.s.clocks_to_save(), saved, "clocks read back");
    p.s.advance_clocks();
    assert_eq!(p.s.clock_total(THE_END), saved[1].total_ticks, "the End's clock is still paused");
    assert_eq!(fraction_bits(&p, OVERWORLD), (0.3f32 + 0.3f32).to_bits(), "the fraction goes on from the saved one");
}

fn write_clock_file(world: &Path, clocks: Vec<(&str, Vec<(&str, Nbt)>)>) {
    let compound = |fields: Vec<(&str, Nbt)>| Nbt::Compound(fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect());
    let data = Nbt::Compound(clocks.into_iter().map(|(name, fields)| (name.to_owned(), compound(fields))).collect());
    let root = Nbt::Compound(vec![("data".to_owned(), data)]);
    let path = world_clocks::file_path(world);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let bytes = crate::nbtio::gzip(&crate::nbtio::encode_root(&root));
    crate::save::replace(&path, &bytes, false).expect("clock file written");
}

/// Reading of the file: the saved state laid over the registry's clocks, a key without its
/// default namespace accepted, a clock unknown to the registry dropped, a refused state (rate
/// not above zero) replaced by a clock at zero.
#[test]
fn saved_clocks_are_laid_over_the_registry() {
    let dir = scratch_folder("read");
    let registry = [OVERWORLD.to_owned(), THE_END.to_owned(), "pack:tides".to_owned()];
    let at_zero = |name: &str| SavedClock { name: name.to_owned(), total_ticks: 0, pace: ClockPace::default() };
    assert_eq!(world_clocks::read(&dir, &registry), [at_zero(OVERWORLD), at_zero(THE_END), at_zero("pack:tides")], "no file");

    write_clock_file(
        &dir,
        vec![
            ("the_end", vec![("total_ticks", Nbt::Long(7))]),
            ("pack:gone", vec![("total_ticks", Nbt::Long(9))]),
            (
                OVERWORLD,
                vec![
                    ("total_ticks", Nbt::Long(5)),
                    ("partial_tick", Nbt::Float(0.25)),
                    ("rate", Nbt::Float(2.5)),
                    ("paused", Nbt::Byte(1)),
                ],
            ),
            ("pack:tides", vec![("total_ticks", Nbt::Long(11)), ("rate", Nbt::Float(0.0))]),
        ],
    );
    let overworld = SavedClock {
        name: OVERWORLD.to_owned(),
        total_ticks: 5,
        pace: ClockPace { partial_tick: 0.25, rate: 2.5, paused: true },
    };
    let the_end = SavedClock { name: THE_END.to_owned(), total_ticks: 7, pace: ClockPace::default() };
    assert_eq!(world_clocks::read(&dir, &registry), [overworld, the_end, at_zero("pack:tides")]);
}

/// The night skipped by the sleepers is a move to the wake-up marker: the fraction of a tick
/// restarts and the overworld clock alone is sent.
#[test]
fn wake_up_restarts_the_fraction() {
    let (mut p, _dir) = new_world("wake-up");
    p.run(concat!(
        "setblock 2 -60 0 minecraft:red_bed[facing=east,part=foot]\n",
        "setblock 3 -60 0 minecraft:red_bed[facing=east,part=head]\n",
    ));
    p.s.pos = [1.5, -60.0, 1.5];
    cmd(&mut p, "time set 18000");
    p.s.clock_paces.insert(OVERWORLD.to_owned(), ClockPace { partial_tick: 0.5, rate: 0.5, paused: false });
    p.s.update_sleeping_list();
    assert!(p.s.use_bed([2, -60, 0]), "lying down");
    p.s.life.vit.sleep_timer = 100;
    let _ = p.take_events();
    p.s.sleep_world_tick();
    assert_eq!(p.s.clock_total(OVERWORLD), 24000, "night skipped at the wake-up marker");
    assert_eq!(fraction_bits(&p, OVERWORLD), 0, "the fraction restarts at the wake-up");
    assert_eq!(clock_packets(&p), [vec![(OVERWORLD.to_owned(), 24000, 0.0, 0.5)]], "the overworld clock alone");
}

/// The clock tick of before this module (pause set, rate map and fraction map looked up by
/// name), kept for the bench.
#[derive(Default)]
struct PreviousClocks {
    clocks: Vec<(String, i64)>,
    paused: std::collections::BTreeSet<String>,
    rate: std::collections::BTreeMap<String, f32>,
    fraction: std::collections::BTreeMap<String, f32>,
}

impl PreviousClocks {
    fn advance(&mut self) {
        for c in &mut self.clocks {
            if self.paused.contains(&c.0) {
                continue;
            }
            let rate = self.rate.get(&c.0).copied().unwrap_or(1.0);
            if rate == 1.0 {
                c.1 += 1;
            } else {
                if !self.fraction.contains_key(&c.0) {
                    self.fraction.insert(c.0.clone(), 0.0);
                }
                let Some(fraction) = self.fraction.get_mut(&c.0) else { continue };
                *fraction += rate;
                let whole = fraction.floor();
                *fraction -= whole;
                c.1 += whole as i64;
            }
        }
    }
}

/// Bench (loaded machine allowed): `cargo test --release -p rc-sim --lib bench_clock_tick --
/// --nocapture`. One tick of the two clocks of a world, at the default pace then with the
/// overworld at a fractional rate, against the tick of before.
#[test]
fn bench_clock_tick() {
    const TICKS: u32 = 2_000_000;
    let (mut p, _dir) = new_world("bench");
    let nanoseconds_per_tick = |tick: &mut dyn FnMut()| {
        let mut samples: Vec<f64> = (0..5)
            .map(|_| {
                let start = std::time::Instant::now();
                for _ in 0..TICKS {
                    tick();
                }
                start.elapsed().as_secs_f64() * 1e9 / f64::from(TICKS)
            })
            .collect();
        samples.sort_by(f64::total_cmp);
        samples[samples.len() / 2]
    };
    let mut previous = PreviousClocks { clocks: p.s.clocks.clone(), ..PreviousClocks::default() };
    let before_default = nanoseconds_per_tick(&mut || std::hint::black_box(&mut previous).advance());
    let after_default = nanoseconds_per_tick(&mut || std::hint::black_box(&mut p.s).advance_clocks());
    previous.rate.insert(OVERWORLD.to_owned(), 0.3);
    cmd(&mut p, "time rate 0.3");
    let before_fractional = nanoseconds_per_tick(&mut || std::hint::black_box(&mut previous).advance());
    let after_fractional = nanoseconds_per_tick(&mut || std::hint::black_box(&mut p.s).advance_clocks());
    eprintln!(
        "[bench] clock tick, two clocks: default pace before {before_default:.1} ns, after {after_default:.1} ns; \
         overworld at rate 0.3 before {before_fractional:.1} ns, after {after_fractional:.1} ns (medians of 5)"
    );
}
