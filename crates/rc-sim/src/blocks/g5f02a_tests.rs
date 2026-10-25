//! G5-F02a: shared crossed-block pass (fire, soul fire, cactus, campfire for the player and
//! the mobs), extinguishing by rain and powder snow, extinguish sound.
//! Fresh flat world of `g6f01_tests`: the 26.3 cache and the client jar are required (the test
//! fails without them).

use rc_client::Event;

use super::P;
use super::g6f01_tests::{W, Y, world};
use crate::mobs::mob::Mob;
use crate::mobs::species::Kind;

const EXTINGUISH: &str = "minecraft:entity.generic.extinguish_fire";

/// Player ready, at full health, placed at `pos` (feet).
fn player_at(w: &mut W, pos: [f64; 3]) {
    let s = &mut w.p.s;
    s.loaded = true;
    s.game_mode = 0;
    s.life.vit.health = 20.0;
    s.life.vit.dead = false;
    s.life.vit.cooldown = 0;
    s.life.vit.fire = -20;
    s.pos = pos;
    s.inside_forget_player();
}

fn player_tick(w: &mut W) -> Vec<Event> {
    w.p.s.vitals_tick();
    w.events()
}

/// Cow without AI (motionless) with feet at `pos`, outside the list.
fn cow(w: &mut W, pos: [f64; 3]) -> Mob {
    let mut m = w.p.s.new_mob(Kind::Cow, pos, 0.0);
    m.no_ai = true;
    m.pos = pos;
    m.old = pos;
    m
}

fn mob_tick(w: &mut W, m: &mut Mob) -> Vec<Event> {
    m.old = m.pos;
    w.p.s.tick_mob(m);
    w.events()
}

fn feet(at: P) -> [f64; 3] {
    [f64::from(at[0]) + 0.5, f64::from(at[1]), f64::from(at[2]) + 0.5]
}

/// Player in fire: 20 rest ticks (−20) before catching fire, "in fire" damage
/// from the first tick; soul fire: 2 points.
#[test]
fn player_fire_block_waits_then_ignites() {
    let mut w = world("g5f02a-pfire");
    let at = [4, Y, 4];
    player_at(&mut w, feet(at));
    w.p.s.life.vit.fire = 0;
    let _ = player_tick(&mut w);
    assert_eq!(w.p.s.life.vit.fire, -20, "outside fire: player counter at rest");
    w.set(at, "minecraft:fire");
    player_at(&mut w, feet(at));
    let _ = player_tick(&mut w);
    assert_eq!(w.p.s.life.vit.fire, -19, "first tick in fire: not yet lit");
    assert_eq!(w.p.s.life.vit.health, 19.0, "fire damage (1)");
    for _ in 0..19 {
        w.p.s.life.vit.cooldown = 0;
        w.p.s.life.vit.health = 20.0;
        let _ = player_tick(&mut w);
    }
    assert_eq!(w.p.s.life.vit.fire, 160, "at the 20th tick: 8 s of fire");

    // Soul fire only holds on a block of `#soul_fire_base_blocks`.
    w.set([at[0], at[1] - 1, at[2]], "minecraft:soul_soil");
    w.set(at, "minecraft:soul_fire");
    assert_eq!(w.name(at), "minecraft:soul_fire");
    player_at(&mut w, feet(at));
    let _ = player_tick(&mut w);
    assert_eq!(w.p.s.life.vit.health, 18.0, "soul fire: 2 points");
}

/// Cactus touched by the player's box (against its inset shape): 1 "cactus" point; lit
/// campfire under the feet: 1 point, unlit: nothing.
#[test]
fn player_cactus_and_campfire_hurt() {
    let mut w = world("g5f02a-pcactus");
    w.set([9, Y, 4], "minecraft:cactus");
    player_at(&mut w, [8.75, f64::from(Y), 4.5]);
    let _ = player_tick(&mut w);
    assert_eq!(w.p.s.life.vit.health, 19.0, "cactus: 1 point");
    assert_eq!(w.p.s.life.vit.last_damage.as_ref().map(|d| d.0.as_str()), Some("minecraft:cactus"));

    w.set([4, Y, 8], "minecraft:campfire[lit=true]");
    player_at(&mut w, [4.5, f64::from(Y) + 0.4375, 8.5]);
    let _ = player_tick(&mut w);
    assert_eq!(w.p.s.life.vit.health, 19.0, "lit campfire: 1 point");
    assert_eq!(w.p.s.life.vit.last_damage.as_ref().map(|d| d.0.as_str()), Some("minecraft:campfire"));

    w.set([4, Y, 8], "minecraft:campfire[lit=false]");
    player_at(&mut w, [4.5, f64::from(Y) + 0.4375, 8.5]);
    let _ = player_tick(&mut w);
    assert_eq!(w.p.s.life.vit.health, 20.0, "unlit campfire: nothing");
}

/// Mob in fire: set alight for 8 s and hurt; against a cactus: hurt.
#[test]
fn mob_fire_block_and_cactus() {
    let mut w = world("g5f02a-mfire");
    let at = [4, Y, 4];
    w.set(at, "minecraft:fire");
    let mut m = cow(&mut w, feet(at));
    let hp = m.health;
    let _ = mob_tick(&mut w, &mut m);
    assert_eq!(m.fire, 160, "mob in fire: 8 s");
    assert_eq!(m.health, hp - 1.0, "fire damage (1)");

    w.set([10, Y, 4], "minecraft:cactus");
    // Box (0.9 wide) that overflows by 0.05 into the cactus cell (its shape is inset).
    let mut c = cow(&mut w, [10.0 - 0.45 + 0.05, f64::from(Y), 4.5]);
    let hp = c.health;
    let _ = mob_tick(&mut w, &mut c);
    assert_eq!(c.health, hp - 1.0, "cactus: 1 point");
}

/// Rain in the open: the fire of the player and of a mob goes out, with the extinguish sound
/// (once); without rain, the fire continues.
#[test]
fn rain_extinguishes_with_sound() {
    let mut w = world("g5f02a-rain");
    player_at(&mut w, feet([4, Y, 4]));
    w.p.s.life.vit.fire = 100;
    let evs = player_tick(&mut w);
    assert_eq!(w.p.s.life.vit.fire, 99, "clear weather: the fire continues");
    assert_eq!(w.sounds(&evs, EXTINGUISH), 0);

    w.cmd("weather rain");
    w.p.s.life.weather.rain_level = 1.0; // rain level reached (without waiting for the rise)
    assert!(w.p.s.in_rain(feet([4, Y, 4]), f64::from(Y) + 1.8), "rain above the player");
    let evs = player_tick(&mut w);
    assert!(w.p.s.life.vit.fire <= 0, "rain: player put out ({})", w.p.s.life.vit.fire);
    assert_eq!(w.sounds(&evs, EXTINGUISH), 1, "player extinguish sound");

    let mut m = cow(&mut w, feet([8, Y, 8]));
    m.fire = 100;
    let evs = mob_tick(&mut w, &mut m);
    assert_eq!(m.fire, 0, "rain: mob put out");
    assert_eq!(w.sounds(&evs, EXTINGUISH), 1, "mob extinguish sound");
}

/// Water: fire put out in the pass, with the sound.
#[test]
fn water_extinguish_plays_sound() {
    let mut w = world("g5f02a-water");
    let at = [4, Y, 4];
    w.set(at, "minecraft:water");
    player_at(&mut w, feet(at));
    w.p.s.life.vit.fire = 100;
    let evs = player_tick(&mut w);
    assert!(w.p.s.life.vit.fire <= 0, "water: player put out");
    assert_eq!(w.sounds(&evs, EXTINGUISH), 1, "extinguish sound (water)");
}

/// Powder snow: a burning entity is put out and the snow melts (player always; mob
/// only with `mob_griefing`), extinguish sound.
#[test]
fn powder_snow_extinguishes_and_melts() {
    let mut w = world("g5f02a-snow");
    let at = [4, Y, 4];
    w.set(at, "minecraft:powder_snow");
    player_at(&mut w, feet(at));
    w.p.s.life.vit.fire = 100;
    let evs = player_tick(&mut w);
    assert!(w.p.s.life.vit.fire <= 0, "powder snow: player put out");
    assert_eq!(w.name(at), "minecraft:air", "the snow melts under the burning player");
    assert_eq!(w.sounds(&evs, EXTINGUISH), 1, "extinguish sound (snow)");

    let b = [8, Y, 8];
    w.set(b, "minecraft:powder_snow");
    let mut m = cow(&mut w, feet(b));
    m.fire = 100;
    let _ = mob_tick(&mut w, &mut m);
    assert_eq!(m.fire, 0, "mob put out");
    assert_eq!(w.name(b), "minecraft:air", "mob_griefing: the snow melts");

    w.cmd("gamerule minecraft:mob_griefing false");
    w.set(b, "minecraft:powder_snow");
    let mut m = cow(&mut w, feet(b));
    m.fire = 100;
    let _ = mob_tick(&mut w, &mut m);
    assert_eq!(m.fire, 0, "mob put out without mob_griefing");
    assert_eq!(w.name(b), "minecraft:powder_snow", "without mob_griefing: the snow stays");
}

/// Micro-bench (ignored by default): cost of a pass for a motionless mob and for a
/// mob moving 0.2 per tick, compared with the old name-based "powder snow" sweep
/// that it replaces. `cargo test --release -p rc-sim g5f02a_bench -- --ignored --nocapture`.
#[test]
#[ignore]
fn g5f02a_bench() {
    use crate::inside_blocks::Who;
    let mut w = world("g5f02a-bench");
    let n = 200_000u32;
    let mut m = cow(&mut w, feet([4, Y, 4]));
    let t = std::time::Instant::now();
    for _ in 0..n {
        m.old = m.pos;
        std::hint::black_box(w.p.s.inside_blocks(&mut Who::Mob(&mut m)));
    }
    let still = t.elapsed().as_nanos() as f64 / f64::from(n);
    let t = std::time::Instant::now();
    for i in 0..n {
        let from = [4.5 + f64::from(i % 10) * 0.2, f64::from(Y), 4.5];
        m.old = from;
        m.pos = [from[0] + 0.2, from[1], from[2]];
        m.inside.record(from, m.pos, [0.2, 0.0, 0.0]);
        std::hint::black_box(w.p.s.inside_blocks(&mut Who::Mob(&mut m)));
    }
    let moving = t.elapsed().as_nanos() as f64 / f64::from(n);
    // Reference: the old name-based sweep of the box cells (only one of the covered effects).
    let s = &w.p.s;
    let t = std::time::Instant::now();
    for _ in 0..n {
        let b = m.bbox();
        let lo = b.min.map(|v| (v + 1.0e-5).floor() as i32);
        let hi = b.max.map(|v| (v - 1.0e-5).floor() as i32);
        let hit = (lo[0]..=hi[0])
            .any(|x| (lo[1]..=hi[1]).any(|y| (lo[2]..=hi[2]).any(|z| s.bname([x, y, z]) == "minecraft:powder_snow")));
        std::hint::black_box(hit);
    }
    let old_scan = t.elapsed().as_nanos() as f64 / f64::from(n);
    eprintln!("still pass {still:.0} ns, moving {moving:.0} ns; old snow sweep {old_scan:.0} ns");
}

/// Traversal: a one-block movement to the east numbers the arrival layer as step 1 and
/// revisits no cell.
#[test]
fn stepped_traversal_numbers_layers() {
    let at = rc_physics::Aabb { min: [1.2, 0.1, 0.2], max: [1.8, 1.7, 0.8] };
    let mut seen = Vec::new();
    let mut got = Vec::new();
    crate::inside_blocks::each_cell_stepped([0.5, 0.1, 0.5], [1.5, 0.1, 0.5], &at, &mut seen, &mut |p, s| {
        got.push((p, s));
        true
    });
    assert_eq!(got, vec![([0, 0, 0], 0), ([0, 1, 0], 0), ([1, 0, 0], 1), ([1, 1, 0], 1)]);
}
