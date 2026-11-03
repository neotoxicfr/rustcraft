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
    m.set_pos(pos);
    m.old = pos;
    m
}

fn mob_tick(w: &mut W, m: &mut Mob) -> Vec<Event> {
    m.old = m.pos();
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

/// Teleport of the player (absolute, relative, by command) out of the fire: like the game, the
/// path is not swept, no "in fire" damage on the arrival tick.
#[test]
fn player_teleport_forgets_crossed_path() {
    let mut w = world("g5f02a-ptp");
    let at = [4, Y, 4];
    w.set(at, "minecraft:fire");
    for how in 0..3 {
        player_at(&mut w, feet(at));
        let _ = player_tick(&mut w);
        assert_eq!(w.p.s.life.vit.health, 19.0, "in fire (case {how})");
        w.p.s.life.vit.cooldown = 0;
        w.p.s.life.vit.health = 20.0;
        let far = feet([12, Y, 12]);
        match how {
            0 => w.p.s.teleport_to(far),
            1 => w.p.s.send_relative_teleport(far, 0.0, 0.0, 0),
            _ => w.p.s.command_teleport(crate::commands::ents::E::Player, far, [false; 3], None, [false; 2]),
        }
        w.p.s.teleport = None; // confirmation received
        let _ = player_tick(&mut w);
        assert_eq!(w.p.s.life.vit.health, 20.0, "teleported (case {how}): nothing swept");
    }
}

/// `/tp` of a mob: its movements of the tick are forgotten (no ignition by the fire it left).
#[test]
fn mob_command_teleport_forgets_moves() {
    use crate::inside_blocks::Who;
    let mut w = world("g5f02a-mtp");
    let at = [4, Y, 4];
    w.set(at, "minecraft:fire");
    let m = cow(&mut w, feet(at));
    let id = m.id;
    w.p.s.add_mob(m);
    let m = w.p.s.mobs.get_mut(id).expect("cow in the list");
    let from = m.pos();
    m.add_pos_axis(0, 0.1);
    m.inside.record(from, m.pos(), [0.1, 0.0, 0.0]);
    w.p.s.command_teleport(crate::commands::ents::E::Mob(id), feet([12, Y, 12]), [false; 3], None, [false; 2]);
    let mut m = w.p.s.mobs.get_mut(id).expect("cow in the list").clone();
    let _ = w.p.s.inside_blocks(&mut Who::Mob(&mut m));
    assert_eq!(m.fire, 0, "teleported: the fire at the start is not swept");
}

/// `rotate` and directed look of a mob: rotation alone, like the game. The recorded path
/// (pushed through fire) is still swept by the next pass, the old position and the path
/// do not move; the look aligns the body on the head, `rotate` does not. Before, the rotation went
/// through the teleport, which emptied the path, reset the old position and stopped the path.
#[test]
fn mob_rotate_keeps_moves_and_path() {
    use crate::commands::ents::E;
    use crate::inside_blocks::Who;
    let mut w = world("g5f02a-mrot");
    let at = [4, Y, 4];
    w.set(at, "minecraft:fire");
    for look in [false, true] {
        let m = cow(&mut w, feet(at));
        let id = m.id;
        w.p.s.add_mob(m);
        let m = w.p.s.mobs.get_mut(id).expect("cow in the list");
        let from = m.pos();
        m.add_pos_axis(0, 2.0);
        m.inside.record(from, m.pos(), [2.0, 0.0, 0.0]);
        let to = m.pos();
        let body = m.body_yaw;
        m.nav.path = some_path();
        if look {
            w.p.s.command_look_at(E::Mob(id), false, feet([4, Y, 12]), None);
        } else {
            w.p.s.command_rotate(E::Mob(id), [90.0, 0.0], [false, false]);
        }
        let mut m = w.p.s.mobs.get_mut(id).expect("cow in the list").clone();
        let old = m.old;
        m.old = m.pos(); // start of the next tick: only the recorded path goes through the fire
        let _ = w.p.s.inside_blocks(&mut Who::Mob(&mut m));
        assert_eq!(m.fire, 160, "path through the fire swept (look: {look})");
        assert_eq!((old, m.pos()), (from, to), "positions intact (look: {look})");
        assert!(m.nav.path.is_some(), "path kept (look: {look})");
        assert_eq!(m.head_yaw, m.yaw, "head aligned (look: {look})");
        if look {
            assert_eq!(m.body_yaw, m.yaw, "look: body aligned on the head");
        } else {
            assert_eq!((m.yaw, m.body_yaw), (90.0, body), "rotate: yaw set, body unchanged");
        }
    }
}

/// Path in progress (empty, arbitrary target): only its presence counts.
fn some_path() -> Option<crate::mobs::nav::Path> {
    Some(crate::mobs::nav::Path { nodes: Vec::new(), next: 0, target: [9, Y, 4], dist_to_target: 0.0, reached: false })
}

/// Path of a mob: `tp` drops it, `spreadplayers` keeps it, like the game. Before, the shared
/// teleport dropped it for both commands.
#[test]
fn only_tp_stops_the_path() {
    use crate::commands::ents::E;
    let mut w = world("g5f02a-path");
    w.cmd(&format!("summon minecraft:cow 4.5 {Y} 4.5 {{Tags:[\"g5p\"],NoAI:1b}}"));
    let src = w.p.s.player_source();
    let Ok(E::Mob(id)) = w.p.s.arg_entity(&src, "@e[tag=g5p,limit=1]", false) else { panic!("cow summoned") };
    w.p.s.mobs.get_mut(id).expect("cow").nav.path = some_path();
    let walking = |w: &mut W| w.p.s.mobs.get_mut(id).map(|m| m.nav.path.is_some());
    w.cmd("spreadplayers 8 8 1 4 false @e[tag=g5p]");
    assert_eq!(walking(&mut w), Some(true), "spreadplayers: path kept");
    w.cmd(&format!("tp @e[tag=g5p] 6 {Y} 6"));
    assert_eq!(walking(&mut w), Some(false), "tp: path dropped");
}

/// Order of the cells within one step: when backing towards −x, the game starts from the max x
/// edge; the cactus (x + 1) hurts before the campfire (x), which is then blocked by invulnerability.
#[test]
fn cells_follow_travel_direction() {
    let mut w = world("g5f02a-order");
    w.set([9, Y, 4], "minecraft:cactus");
    w.set([8, Y, 4], "minecraft:campfire[lit=true]");
    let before = [8.80, f64::from(Y), 4.5];
    player_at(&mut w, before);
    w.p.s.pos = [8.75, f64::from(Y), 4.5];
    w.p.s.inside_record_player(before);
    let _ = player_tick(&mut w);
    assert_eq!(w.p.s.life.vit.health, 19.0, "a single hit (invulnerability)");
    assert_eq!(w.p.s.life.vit.last_damage.as_ref().map(|d| d.0.as_str()), Some("minecraft:cactus"));

    // Without a path: z in the outer loop, x inside; towards −x: start at the max x edge.
    let at = rc_physics::Aabb { min: [0.2, 0.1, 0.2], max: [1.8, 0.9, 1.8] };
    let mut seen = crate::inside_blocks::Seen::default();
    let mut got = Vec::new();
    crate::inside_blocks::each_cell_stepped([1.0, 0.0, 1.0], [1.0, 0.0, 1.0], &at, &mut seen, &mut |p, _| {
        got.push(p);
        true
    });
    assert_eq!(got, vec![[0, 0, 0], [1, 0, 0], [0, 0, 1], [1, 0, 1]]);
    got.clear();
    crate::inside_blocks::each_cell_stepped([1.0, 0.0, 1.0], [0.95, 0.0, 1.0], &at, &mut seen, &mut |p, _| {
        got.push(p);
        true
    });
    assert_eq!(got[..4], [[1, 0, 0], [1, 0, 1], [0, 0, 0], [0, 0, 1]], "towards −x: max x first, z inside");
}

/// Shared traversal (pass, ghasts, portals): the seen-cells mask returns exactly the same
/// sequence (cell, step) as the list it replaces, over 3,000 random paths (boxes of 0.25 to
/// 4 blocks, paths up to ±20 blocks, plus a few oversized paths), and the stored checksum
/// of the sequence.
#[test]
fn seen_mask_matches_linear_list() {
    use crate::inside_blocks::{Seen, each_cell_stepped};
    let mut r = crate::mobs::jrand::JRand::new(5_020_001);
    let (mut fast, mut slow) = (Seen::default(), Seen::linear());
    let (mut a, mut b) = (Vec::new(), Vec::new());
    let mut sum = 0u64;
    let mut cells = 0usize;
    for i in 0..3000 {
        let reach = if i % 500 == 0 { 120.0 } else { 20.0 };
        let mut v = || (f64::from(r.next_float()) - 0.5) * 2.0;
        let from = [v() * 30.0, v() * 30.0, v() * 30.0];
        let to: [f64; 3] = std::array::from_fn(|k| from[k] + v() * reach);
        let half = [0.125 + v().abs() * 1.875, 0.125 + v().abs() * 1.875, 0.125 + v().abs() * 1.875];
        let at = rc_physics::Aabb {
            min: [to[0] - half[0], to[1], to[2] - half[2]],
            max: [to[0] + half[0], to[1] + 2.0 * half[1], to[2] + half[2]],
        };
        a.clear();
        b.clear();
        each_cell_stepped(from, to, &at, &mut fast, &mut |p, s| {
            a.push((p, s));
            true
        });
        each_cell_stepped(from, to, &at, &mut slow, &mut |p, s| {
            b.push((p, s));
            true
        });
        assert_eq!(a, b, "path {i}: {from:?} -> {to:?}");
        cells += a.len();
        for &(p, s) in &a {
            let h = (i64::from(p[0]) * 73_856_093) ^ (i64::from(p[1]) * 19_349_663) ^ (i64::from(p[2]) * 83_492_791);
            sum = sum.wrapping_mul(31).wrapping_add(h as u64 ^ u64::from(s as u32));
        }
    }
    assert!(cells > 100_000, "paths too short: {cells} cells");
    assert_eq!((cells, sum), EXPECTED_SEEN_SUM, "sequence of cells changed");
}

/// Checksum of `seen_mask_matches_linear_list` (cells, sum), taken on the list.
const EXPECTED_SEEN_SUM: (usize, u64) = (896_844, 17_753_366_349_638_366_043);

/// Silent mob in lava: neither burn sound nor draw of its pitch (its own random
/// stays one value ahead of that of the same non-silent mob).
#[test]
fn silent_mob_lava_keeps_its_rng() {
    use crate::inside_blocks::Who;
    let mut w = world("g5f02a-silent");
    let at = [4, Y, 4];
    w.set(at, "minecraft:lava");
    let mut loud = cow(&mut w, feet(at));
    let mut quiet = loud.clone();
    quiet.x.silent = true;
    let _ = w.p.s.inside_blocks(&mut Who::Mob(&mut loud));
    let evs = w.events();
    assert_eq!(w.sounds(&evs, "minecraft:entity.generic.burn"), 1, "burn sound (not silent)");
    let _ = w.p.s.inside_blocks(&mut Who::Mob(&mut quiet));
    let evs = w.events();
    assert_eq!(w.sounds(&evs, "minecraft:entity.generic.burn"), 0, "silent: no sound");
    assert_eq!(loud.health, quiet.health, "same lava damage");
    let _ = quiet.rng.next_float();
    assert_eq!(loud.rng.next_long(), quiet.rng.next_long(), "silent: one draw fewer");
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
        m.old = m.pos();
        std::hint::black_box(w.p.s.inside_blocks(&mut Who::Mob(&mut m)));
    }
    let still = t.elapsed().as_nanos() as f64 / f64::from(n);
    let t = std::time::Instant::now();
    for i in 0..n {
        let from = [4.5 + f64::from(i % 10) * 0.2, f64::from(Y), 4.5];
        m.old = from;
        m.set_pos([from[0] + 0.2, from[1], from[2]]);
        m.inside.record(from, m.pos(), [0.2, 0.0, 0.0]);
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

    // Traversal of a ghast (4 × 4 × 4): seen-cells mask against the list it replaces.
    use crate::inside_blocks::{Seen, each_cell_stepped};
    for travel in [[14.3, 6.1, -11.7], [5.0, 2.0, 6.0], [1.0, 0.0, 0.0], [0.2, 0.0, 0.0]] {
        let from = [0.5, 70.0, 0.5];
        let to: [f64; 3] = std::array::from_fn(|k| from[k] + travel[k]);
        let at = rc_physics::Aabb { min: [to[0] - 2.0, to[1], to[2] - 2.0], max: [to[0] + 2.0, to[1] + 4.0, to[2] + 2.0] };
        let mut per = [0.0f64; 2];
        for (k, mut seen) in [Seen::default(), Seen::linear()].into_iter().enumerate() {
            let reps = 2_000u32;
            let t = std::time::Instant::now();
            for _ in 0..reps {
                let mut n = 0u32;
                each_cell_stepped(from, to, &at, &mut seen, &mut |p, _| {
                    n += std::hint::black_box(p[0]) as u32 & 1;
                    true
                });
                std::hint::black_box(n);
            }
            per[k] = t.elapsed().as_nanos() as f64 / f64::from(reps);
        }
        eprintln!("ghast {travel:?}: mask {:.0} ns, list {:.0} ns", per[0], per[1]);
    }
}

/// Traversal: a one-block movement to the east numbers the arrival layer as step 1 and
/// revisits no cell.
#[test]
fn stepped_traversal_numbers_layers() {
    let at = rc_physics::Aabb { min: [1.2, 0.1, 0.2], max: [1.8, 1.7, 0.8] };
    let mut seen = crate::inside_blocks::Seen::default();
    let mut got = Vec::new();
    crate::inside_blocks::each_cell_stepped([0.5, 0.1, 0.5], [1.5, 0.1, 0.5], &at, &mut seen, &mut |p, s| {
        got.push((p, s));
        true
    });
    assert_eq!(got, vec![([0, 0, 0], 0), ([0, 1, 0], 0), ([1, 0, 0], 1), ([1, 1, 0], 1)]);
}
