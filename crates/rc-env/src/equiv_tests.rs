//! Equivalence proofs of the switches `RUSTCRAFT_OPT_ENV_NOALLOC`, `_ENV_KEYCACHE` and
//! `_FOG_MEMO`: two environments driven step by step, old and new path, compared bit for
//! bit (values, clocks, fog). Real data required under `RUSTCRAFT_OPT_REQUIRE=1`.

use super::*;

const DIMS: [&str; 3] = ["minecraft:overworld", "minecraft:the_nether", "minecraft:the_end"];
const LEVELS: [f32; 3] = [0.0, 0.5, 1.0];

fn biomes() -> Vec<String> {
    ["plains", "desert", "snowy_plains", "swamp", "crimson_forest", "basalt_deltas", "the_end", "pale_garden"]
        .iter()
        .map(|b| format!("minecraft:{b}"))
        .collect()
}

fn bits(v: V) -> [u32; 4] {
    v.map(f32::to_bits)
}

fn fog_bits(f: &Fog) -> [u32; 10] {
    let c = f.color;
    [c[0], c[1], c[2], c[3], f.env_start, f.env_end, f.render_start, f.render_end, f.sky_end, f.cloud_end]
        .map(f32::to_bits)
}

/// Bit-for-bit comparison, with the context in case of a difference.
fn same(what: &str, old: V, new: V) {
    assert_eq!(bits(old), bits(new), "{what}: old {old:?}, new {new:?}");
}

struct Pair {
    old: Env,
    new: Env,
    po: Probe,
    pn: Probe,
}

impl Pair {
    fn new(d: &VersionData, dim: &str, biomes: &[String]) -> Pair {
        let mut old = Env::new(d, dim, biomes);
        old.opts = EnvOpts::OLD;
        let mut new = Env::new(d, dim, biomes);
        new.opts = EnvOpts::NEW;
        Pair { old, new, po: Probe::default(), pn: Probe::default() }
    }

    fn both(&mut self, f: impl Fn(&mut Env)) {
        f(&mut self.old);
        f(&mut self.new);
    }

    fn clocks_equal(&self, ctx: &str) {
        let (a, b) = (&self.old.clocks, &self.new.clocks);
        assert_eq!((a.game_time, a.last), (b.game_time, b.last), "{ctx}: time");
        assert_eq!(a.clocks, b.clocks, "{ctx}: clocks (birth or advance)");
    }

    /// All the reads of a tick: `eval` (mixed weights), `eval_global`, `eval_in_biome`,
    /// then the probe at three tick fractions.
    fn compare_tick(&mut self, ctx: &str) {
        let w = [(0u32, 1000.0f32), (2, 2000.0), (5, 1096.0)];
        for a in Attr::ALL {
            same(&format!("{ctx} eval {a:?}"), self.old.eval(a, &w), self.new.eval(a, &w));
            same(&format!("{ctx} global {a:?}"), self.old.eval_global(a), self.new.eval_global(a));
            // Second global read: served by the cache on the new side.
            same(&format!("{ctx} global bis {a:?}"), self.old.eval_global(a), self.new.eval_global(a));
            same(&format!("{ctx} biome {a:?}"), self.old.eval_in_biome(a, 3), self.new.eval_in_biome(a, 3));
        }
        self.po.tick(w.to_vec());
        self.pn.tick(w.to_vec());
        for pt in [0.0, 0.37, 0.999] {
            for a in Attr::ALL {
                same(
                    &format!("{ctx} probe {a:?} pt {pt}"),
                    self.po.get(&mut self.old, a, pt),
                    self.pn.get(&mut self.new, a, pt),
                );
            }
        }
        self.clocks_equal(ctx);
    }
}

/// 24,000 ticks × rain and thunder {0; 0.5; 1} × 3 dimensions × all attributes. Clocks
/// born at the first evaluation (no time packet), then time packet halfway through with
/// a fractional speed.
#[test]
fn eval_sweep_matches_old_path() {
    let Some(d) = rc_testdata::version_data() else { return };
    let biomes = biomes();
    for dim in DIMS {
        for rain in LEVELS {
            for thunder in LEVELS {
                let mut p = Pair::new(&d, dim, &biomes);
                assert!(!p.old.clock_names.is_empty() || dim != DIMS[0], "{dim}: no timeline");
                p.both(|e| {
                    e.weather.event(7, rain);
                    e.weather.event(8, thunder);
                });
                for tick in 0..24_000i64 {
                    if tick == 12_000 {
                        let names = p.old.clock_names.clone();
                        let updates: Vec<(String, i64, f32, f32)> =
                            names.into_iter().map(|n| (n, 30_017, 0.25, 0.5)).collect();
                        p.both(|e| e.clocks.set_time(e.clocks.game_time + 3, &updates));
                    }
                    p.both(|e| e.clocks.tick());
                    p.compare_tick(&format!("{dim} rain {rain} thunder {thunder} tick {tick}"));
                }
            }
        }
    }
}

/// Weather and time changed without a clock packet (public fields): the global cache key must
/// follow them; clocks replaced by a clone or by default: indices and cache re-read.
#[test]
fn global_cache_follows_weather_time_and_clock_swaps() {
    let Some(d) = rc_testdata::version_data() else { return };
    let biomes = biomes();
    let mut p = Pair::new(&d, DIMS[0], &biomes);
    for tick in 0..6_000i64 {
        let r = LEVELS[(tick / 7 % 3) as usize];
        let t = LEVELS[(tick / 11 % 3) as usize];
        p.both(|e| {
            e.weather.rain = r;
            e.weather.thunder_raw = t;
        });
        if tick % 13 == 0 {
            p.both(|e| e.clocks.game_time += 5);
        }
        if tick == 3_000 {
            p.both(|e| e.clocks = Clocks::default());
        }
        if tick == 4_000 {
            let saved = p.old.clocks.clone();
            p.both(|e| e.clocks = saved.clone());
        }
        p.both(|e| e.clocks.tick());
        p.compare_tick(&format!("swaps tick {tick}"));
        // Without a tick: rain, then thunder, changed between two global reads.
        for (k, (r2, t2)) in [(LEVELS[(tick % 3) as usize], t), (r, LEVELS[(tick % 3) as usize])].into_iter().enumerate() {
            p.both(|e| {
                e.weather.rain = r2;
                e.weather.thunder_raw = t2;
            });
            for a in Attr::ALL {
                same(&format!("weather {k} tick {tick} {a:?}"), p.old.eval_global(a), p.new.eval_global(a));
            }
        }
    }
}

fn fog_input(kind: FogKind, pt: f32, render_chunks: i32, forward_x: f32, variant: u32) -> FogInput {
    FogInput {
        kind,
        pt,
        render_chunks,
        eye_y: [64.0, -70.0, -40.0][(variant % 3) as usize],
        min_y: -64,
        flat: variant.is_multiple_of(5),
        forward_x,
        water_vision: [0.0, 0.3, 1.0][(variant % 3) as usize],
        sky_light: [15, 9, 0][(variant / 3 % 3) as usize],
        biome_rain: variant.is_multiple_of(2),
        dt_ticks: 0.4,
        blindness: [None, Some(-1), Some(10)][(variant / 2 % 3) as usize],
        darkness_blend: [None, Some(0.5)][(variant / 7 % 2) as usize],
        boss_darken: [0.0, 0.6][(variant / 4 % 2) as usize],
        boss_fog: variant.is_multiple_of(11),
        night_vision: [0.0, 1.0][(variant / 9 % 2) as usize],
        spectator: variant.is_multiple_of(13),
        fire_resistance: variant.is_multiple_of(17),
        cloud_range: [128, 32][(variant / 5 % 2) as usize],
    }
}

/// Fog inputs swept over a whole day (step of 37 ticks): `Fog` and rain multiplier
/// equal bit for bit.
#[test]
fn fog_sweep_matches_old_path() {
    let Some(d) = rc_testdata::version_data() else { return };
    let biomes = biomes();
    let kinds = [FogKind::Atmospheric, FogKind::Water, FogKind::Lava, FogKind::PowderSnow];
    for dim in DIMS {
        for rain in LEVELS {
            let mut p = Pair::new(&d, dim, &biomes);
            p.both(|e| {
                e.weather.event(7, rain);
                e.weather.event(8, 1.0 - rain);
            });
            let (mut ro, mut rn) = (0.0f32, 0.0f32);
            let mut variant = 0u32;
            for tick in 0..24_000i64 {
                p.both(|e| e.clocks.tick());
                p.po.tick(vec![(0, 4096.0)]);
                p.pn.tick(vec![(0, 4096.0)]);
                if tick % 37 != 0 {
                    continue;
                }
                for pt in [0.0, 0.5, 0.93] {
                    for kind in kinds {
                        for rc in [2, 4, 7, 12, 32] {
                            for fx in [-1.0, -0.2, 0.0, 0.6, 1.0] {
                                variant = variant.wrapping_add(1);
                                let i = fog_input(kind, pt, rc, fx, variant);
                                let fo = fog(&mut p.old, &mut p.po, &mut ro, &i);
                                let f_n = fog(&mut p.new, &mut p.pn, &mut rn, &i);
                                assert_eq!(
                                    fog_bits(&fo),
                                    fog_bits(&f_n),
                                    "{dim} rain {rain} tick {tick} pt {pt} {kind:?} rc {rc} fx {fx} v {variant}\n{fo:?}\n{f_n:?}"
                                );
                                assert_eq!(ro.to_bits(), rn.to_bits(), "rain multiplier");
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Sign of the sine without a sine: identical to the computation on a dense sweep of ±8 rad, around the
/// zeros and on special values.
#[test]
fn sin_sign_matches_sin() {
    let check = |x: f32| assert_eq!(sin_positive(x, true), x.sin() > 0.0, "theta {x} ({:#x})", x.to_bits());
    for v in [0.0, -0.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY, f32::MIN_POSITIVE, f32::MAX, f32::MIN] {
        check(v);
    }
    let mut x = -8.0f32;
    while x <= 8.0 {
        check(x);
        x += 1.0e-6;
    }
    // Zeros: bits near 0, π and 2π (and opposites).
    for z in [std::f32::consts::PI, 2.0 * std::f32::consts::PI, 0.01, std::f32::consts::PI - 0.01] {
        for s in [1.0f32, -1.0] {
            let c = (z * s).to_bits();
            for k in 0..20_000u32 {
                check(f32::from_bits(c.wrapping_sub(10_000).wrapping_add(k)));
            }
        }
    }
}

/// Probe angles: remainder avoided under one turn, bit-for-bit identical to the computed remainder, including
/// for offsets of one turn or more, ±0, NaN and ±∞.
#[test]
fn angle_without_fmod_matches() {
    let vals = [
        0.0f32, -0.0, 1.0, -1.0, 89.9, 90.0, 179.99, 180.0, -180.0, 359.99, 360.0, -360.0, 360.01, 725.0, -725.0, 1.0e9,
        f32::NAN, f32::INFINITY, f32::NEG_INFINITY, f32::MIN_POSITIVE,
    ];
    let (mut old, mut new) = (Env::default(), Env::default());
    (old.opts, new.opts) = (EnvOpts::OLD, EnvOpts::NEW);
    for o in vals {
        for n in vals {
            for pt in [0.0, 0.3, 1.0] {
                let mut p = Probe::default();
                let i = Attr::SunAngle as usize;
                (p.old[i], p.new[i]) = (Some(float(o)), Some(float(n)));
                let mut q = p.clone();
                let (a, b) = (p.get(&mut old, Attr::SunAngle, pt), q.get(&mut new, Attr::SunAngle, pt));
                assert_eq!(bits(a), bits(b), "o {o} n {n} pt {pt}");
            }
        }
    }
}

/// Track sampling without an array: same values as the old one on real tracks and
/// synthetic tracks (unsorted keys, no period, duplicates).
#[test]
fn track_scan_matches_segments() {
    let mut tracks: Vec<Track> = Vec::new();
    if let Some(d) = rc_testdata::version_data() {
        for dim in DIMS {
            let e = Env::new(&d, dim, &[]);
            tracks.extend(e.tracks.into_iter().flatten());
        }
        assert!(!tracks.is_empty(), "no real track");
    }
    let f = float;
    for period in [0, 24_000, 100] {
        for keys in [
            vec![(10, f(1.0)), (20, f(2.0))],
            vec![(6000, f(360.0)), (6000, f(0.0))],
            vec![(500, f(3.0)), (100, f(1.0)), (900, f(2.0))],
            vec![(0, f(0.5)), (0, f(0.7)), (50, f(1.0)), (50, f(-1.0)), (99, f(4.0))],
        ] {
            for (step, ease) in [(false, Ease::Linear), (true, Ease::Linear), (false, Ease::Constant)] {
                tracks.push(Track { clock: 0, period, step, op: EnvOp::Override, ease, keys: keys.clone() });
            }
        }
    }
    for t in &tracks {
        for ticks in (-30_000i64..60_000).step_by(7).chain([i64::MIN / 2, i64::MAX / 2]) {
            assert_eq!(bits(t.sample(ticks)), bits(t.sample_scan(ticks)), "{t:?} tick {ticks}");
        }
    }
}
