//! Random draws of the block behaviours that read the block view on both sides of a random draw
//! or of a world change (vines, bamboo, cave vines, bone meal on netherrack, weeping vines,
//! lichen, sea pickles and water plants, crops, frosted ice, moss carpets). For many seeds, the
//! blocks left in the area, the value returned and the next random number are digested per
//! scenario and must match the digests recorded on the code where the view was still an owned
//! copy. The world fingerprints of `tests_opt` do not reach these paths: an extra draw in the vine
//! tick or in the bamboo growth passes them. Fresh flat world of `g6f01_tests` (the 26.3 cache and
//! the client jar are required).

use super::g6f01_tests::{W, Y, world};
use super::*;

/// Seeds tried per scenario.
const SEEDS: i64 = 64;

/// Seed of run `i`, spread over 64 bits: the first draws of the Java generator barely differ
/// between small consecutive seeds (seeds 0 to 31 all fail the vine's 1-in-4 draw).
fn seed(i: i64) -> i64 {
    i.wrapping_mul(0x9E37_79B9_7F4A_7C15_u64 as i64)
}

/// Digest per scenario (first 16 hexadecimal digits of the SHA-256), recorded on main 34e1be81
/// (owned view); an extra draw in the vine tick, or the bamboo stage drawn every time, changes
/// the first and third.
const EXPECTED: &[(&str, &str)] = &[
    ("vine spreading", "19e721033a8f15a6"),
    ("tall bamboo with bone meal", "1e22926f1289e3c6"),
    ("bamboo sapling and short bamboo ticks", "56a121f51859e2c3"),
    ("cave vines tick and shape", "9d50fa3e5f9c2110"),
    ("netherrack between two nyliums", "78da0cf5c84af1b7"),
    ("weeping vines with bone meal", "a03803e4eb308433"),
    ("glow lichen with bone meal", "aba1aea544cdedc1"),
    ("sea pickles on coral and water plants", "ab99ec06500010b8"),
    ("crops tick", "7ee36e5375137c44"),
    ("frosted ice placed", "cdbb40b2c68977ea"),
    ("pale moss carpet placed", "8892f8a671fabf4d"),
];

/// Air over a grass floor in the 16 × 16 columns of chunk 0, from the floor to `Y + 14`.
fn clear(w: &mut W) {
    w.p.run(&format!("fill 0 {} 0 15 {} 15 minecraft:grass_block\n", Y - 1, Y - 1));
    w.p.run(&format!("fill 0 {Y} 0 15 {} 15 minecraft:air\n", Y + 14));
    settle(w);
}

fn settle(w: &mut W) {
    w.p.s.flush_block_changes();
    let _ = w.p.take_events();
}

fn state(w: &W, name: &str) -> u32 {
    w.p.s.default_state(name)
}

/// One scenario: area cleared, `setup` then `act` each run from the same seed; digest of what
/// every seed leaves (blocks of the area, value returned by `act`, next random number).
fn digest_runs(w: &mut W, setup: fn(&mut W), act: fn(&mut W) -> u32) -> String {
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    for i in 0..SEEDS {
        clear(w);
        w.p.s.bs.rng = JavaRandom::new(seed(i));
        setup(w);
        settle(w);
        w.p.s.bs.rng = JavaRandom::new(seed(i));
        let result = act(w);
        settle(w);
        ctx.update(&result.to_le_bytes());
        ctx.update(&w.p.s.bs.rng.next_int().to_le_bytes());
        for y in Y - 1..=Y + 14 {
            for z in 0..16 {
                for x in 0..16 {
                    ctx.update(&w.p.s.get([x, y, z]).to_le_bytes());
                }
            }
        }
    }
    ctx.finish().as_ref().iter().take(8).map(|b| format!("{b:02x}")).collect()
}

fn random_tick_at(w: &mut W, at: P) -> u32 {
    let s = w.p.s.get(at);
    w.p.s.random_tick(s, at);
    0
}

fn bone_meal_at(w: &mut W, at: P) -> u32 {
    u32::from(w.p.s.grow_with_bone_meal(at))
}

type Scenario = (&'static str, fn(&mut W), fn(&mut W) -> u32);

const SCENARIOS: &[Scenario] = &[
    (
        "vine spreading",
        |w| {
            w.p.run(&format!("fill 8 {Y} 8 8 {} 8 minecraft:stone\n", Y + 3));
            w.set([7, Y + 1, 8], "minecraft:vine[east=true]");
        },
        |w| random_tick_at(w, [7, Y + 1, 8]),
    ),
    (
        "tall bamboo with bone meal",
        |w| w.p.run(&format!("fill 4 {Y} 4 4 {} 4 minecraft:bamboo\n", Y + 10)),
        |w| bone_meal_at(w, [4, Y, 4]),
    ),
    (
        "bamboo sapling and short bamboo ticks",
        |w| {
            w.set([10, Y, 10], "minecraft:bamboo_sapling");
            w.p.run(&format!("fill 12 {Y} 12 12 {} 12 minecraft:bamboo\n", Y + 2));
        },
        |w| random_tick_at(w, [10, Y, 10]) + random_tick_at(w, [12, Y + 2, 12]),
    ),
    (
        "cave vines tick and shape",
        |w| {
            w.set([4, Y + 5, 4], "minecraft:stone");
            w.set([4, Y + 4, 4], "minecraft:cave_vines[age=0]");
            w.set([6, Y + 5, 6], "minecraft:stone");
            w.set([6, Y + 4, 6], "minecraft:cave_vines_plant");
        },
        |w| {
            random_tick_at(w, [4, Y + 4, 4]);
            let (p, air) = ([6, Y + 4, 6], state(w, "minecraft:air"));
            let s = w.p.s.get(p);
            w.p.s.update_shape(s, p, Dir::Down, below(p), air)
        },
    ),
    (
        "netherrack between two nyliums",
        |w| {
            w.set([8, Y - 1, 8], "minecraft:netherrack");
            w.set([7, Y - 1, 8], "minecraft:crimson_nylium");
            w.set([9, Y - 1, 8], "minecraft:warped_nylium");
        },
        |w| bone_meal_at(w, [8, Y - 1, 8]),
    ),
    (
        "weeping vines with bone meal",
        |w| {
            w.set([3, Y + 12, 3], "minecraft:stone");
            w.set([3, Y + 11, 3], "minecraft:weeping_vines[age=3]");
        },
        |w| bone_meal_at(w, [3, Y + 11, 3]),
    ),
    (
        "glow lichen with bone meal",
        |w| {
            w.p.run(&format!("fill 8 {Y} 6 8 {} 10 minecraft:stone\n", Y + 2));
            w.set([7, Y + 1, 8], "minecraft:glow_lichen[east=true]");
        },
        |w| bone_meal_at(w, [7, Y + 1, 8]),
    ),
    (
        "sea pickles on coral and water plants",
        |w| {
            w.p.run(&format!("fill 0 {} 0 15 {} 15 minecraft:brain_coral_block\n", Y - 1, Y - 1));
            w.p.run(&format!("fill 0 {Y} 0 15 {} 15 minecraft:water\n", Y + 3));
            w.set([8, Y, 8], "minecraft:sea_pickle[waterlogged=true]");
        },
        |w| {
            let pickles = bone_meal_at(w, [8, Y, 8]);
            pickles + 2 * u32::from(w.p.s.grow_water_plant([4, Y, 4], None))
        },
    ),
    (
        "crops tick",
        |w| {
            w.p.run(&format!("fill 2 {} 2 4 {} 2 minecraft:farmland\n", Y - 1, Y - 1));
            w.set([2, Y, 2], "minecraft:wheat[age=6]");
            w.set([3, Y, 2], "minecraft:beetroots[age=2]");
            w.set([4, Y, 2], "minecraft:torchflower_crop[age=1]");
        },
        |w| {
            for _ in 0..8 {
                for x in 2..=4 {
                    random_tick_at(w, [x, Y, 2]);
                }
            }
            0
        },
    ),
    (
        "frosted ice placed",
        |_| {},
        |w| {
            w.set([8, Y, 8], "minecraft:frosted_ice");
            0
        },
    ),
    (
        "pale moss carpet placed",
        |w| {
            w.p.run(&format!("fill 7 {Y} 7 9 {} 9 minecraft:stone\n", Y + 1));
            w.set([8, Y + 2, 8], "minecraft:pale_moss_carpet");
        },
        |w| {
            let (p, s) = ([8, Y + 2, 8], state(w, "minecraft:pale_moss_carpet"));
            w.p.s.moss_carpet_placed(s, p);
            0
        },
    ),
];

#[test]
fn random_draws_around_block_reads_are_unchanged() {
    let mut w = world("draworder");
    let got: Vec<(&str, String)> =
        SCENARIOS.iter().map(|&(name, setup, act)| (name, digest_runs(&mut w, setup, act))).collect();
    for (name, digest) in &got {
        eprintln!("(\"{name}\", \"{digest}\"),");
    }
    assert!(!EXPECTED.is_empty(), "no recorded digests: enter the lines above in EXPECTED");
    let changed: Vec<&str> = got
        .iter()
        .filter(|(name, digest)| EXPECTED.iter().find(|e| e.0 == *name).map(|e| e.1) != Some(digest.as_str()))
        .map(|e| e.0)
        .collect();
    assert!(changed.is_empty(), "random draws changed in: {changed:?}");
}
