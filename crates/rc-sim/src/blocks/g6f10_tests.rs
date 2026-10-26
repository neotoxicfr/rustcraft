//! G6-F10: sculk sensors (normal, calibrated) listening to vibrations, sensor →
//! shrieker chain, forced step, vibration particle, allay that hears the note block through a
//! vibration (travel, wool), arrows and wooden buttons, blocked chest, simple candle
//! cake. Fresh flat world of G6-F01: a missing 26.3 cache or client jar makes the test FAIL.

use rc_client::Event;
use rc_proto_v777::entity::VibDest;

use super::g6f01_tests::{W, Y, particles, world};
use super::*;
use crate::mobs::mob::Ent;
use crate::mobs::recent::vibration::Src;
use crate::mobs::species::Kind as MobKind;

fn game_ticks(w: &mut W, n: i32, out: &mut Vec<Event>) {
    for _ in 0..n {
        w.p.s.game_tick();
        w.p.s.tick += 1;
        w.p.s.flush_block_changes();
        out.extend(w.p.take_events());
    }
}

fn phase(w: &W, at: P) -> String {
    let st = w.p.s.get(at);
    w.p.s.v().get(st, "sculk_sensor_phase").unwrap_or("?").to_owned()
}

fn power(w: &W, at: P) -> i32 {
    let st = w.p.s.get(at);
    w.p.s.v().get_int(st, "power")
}

fn center(p: P) -> [f64; 3] {
    [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5]
}

fn logged(w: &W, event: &str) -> usize {
    w.p.s.mobs.rec.vib.log.iter().filter(|e| e.0 == event).count()
}

/// Chest opening at 3 blocks (frequency 10): selection at the next tick (particle towards the
/// sensor, 3 ticks of travel), reception at the 3rd tick: active, power 15 − ⌊15/8 × 3⌋ = 10,
/// comparator 10, clicking and sound; cooldown 30 ticks later (power 0), inactive
/// 10 ticks after (stop sound).
#[test]
fn sensor_hears_vibration_then_cools_down() {
    let mut w = world("g6f10-sensor");
    let s = [4, Y, 4];
    w.set(s, "minecraft:sculk_sensor");
    let _ = w.events();
    w.p.s.mobs.rec.vib.log.clear();
    let from = [7, Y, 4];
    w.p.s.game_event_src("minecraft:container_open", center(from), Some(Src::Living(Ent::Player)), None);
    let mut evs = Vec::new();
    game_ticks(&mut w, 1, &mut evs);
    let parts = particles(&w, &evs, "minecraft:vibration");
    assert_eq!(parts.len(), 1, "vibration particle at selection");
    assert_eq!(parts[0].pos, center(from), "particle left from the origin");
    assert_eq!(parts[0].particle.vib, Some((VibDest::Block(s), 3)), "towards the sensor, 3 ticks");
    game_ticks(&mut w, 1, &mut evs);
    assert_eq!(phase(&w, s), "inactive", "still on its way");
    evs.clear();
    game_ticks(&mut w, 1, &mut evs);
    assert_eq!(phase(&w, s), "active", "received at the 3rd tick");
    assert_eq!(power(&w, s), 10, "power by distance");
    assert_eq!(w.comparator(s, Dir::North), 10, "comparator = frequency");
    assert_eq!(logged(&w, "minecraft:sculk_sensor_tendrils_clicking"), 1, "clicking emitted");
    assert_eq!(w.sounds(&evs, "minecraft:block.sculk_sensor.clicking"), 1, "clicking sound");
    assert_eq!(w.p.s.be_int(s, "last_vibration_frequency"), 10, "frequency kept in the block entity");
    // Signal: weak everywhere, strong upwards only.
    let st = w.p.s.get(s);
    assert_eq!(w.p.s.weak(st, s, Dir::North), 10);
    game_ticks(&mut w, 29, &mut evs);
    assert_eq!(phase(&w, s), "active", "30 ticks of activity");
    game_ticks(&mut w, 1, &mut evs);
    assert_eq!(phase(&w, s), "cooldown");
    assert_eq!(power(&w, s), 0);
    assert_eq!(w.comparator(s, Dir::North), 0, "comparator off outside the active phase");
    evs.clear();
    game_ticks(&mut w, 10, &mut evs);
    assert_eq!(phase(&w, s), "inactive");
    assert_eq!(w.sounds(&evs, "minecraft:block.sculk_sensor.clicking_stop"), 1, "stop sound");
}

/// Calibrated (radius 16): signal 15 received from the back → only frequency 15 passes; at 12
/// blocks, power 15 − ⌊15/16 × 12⌋ = 4; nothing towards the back. A normal sensor at 12 blocks
/// hears nothing.
#[test]
fn calibrated_sensor_filters_by_back_signal() {
    let mut w = world("g6f10-calibrated");
    let c = [4, Y, 4];
    w.set(c, "minecraft:calibrated_sculk_sensor[facing=north]");
    // Back: the side opposite to `facing` (south).
    w.set([4, Y, 5], "minecraft:redstone_block");
    let n = [12, Y, 12];
    w.set(n, "minecraft:sculk_sensor");
    let _ = w.events();
    let mut evs = Vec::new();
    let from = [4, Y, -8];
    w.p.s.game_event_src("minecraft:container_open", center(from), None, None);
    game_ticks(&mut w, 15, &mut evs);
    assert_eq!(phase(&w, c), "inactive", "frequency 10 refused (back at 15)");
    w.p.s.game_event_src("minecraft:explode", center(from), None, None);
    game_ticks(&mut w, 13, &mut evs);
    assert_eq!(phase(&w, c), "active", "frequency 15 accepted at 12 blocks");
    assert_eq!(power(&w, c), 4);
    let st = w.p.s.get(c);
    assert_eq!(w.p.s.weak(st, c, Dir::North), 0, "nothing towards the back");
    assert_eq!(w.p.s.weak(st, c, Dir::South), 4, "signal towards the front");
    assert_eq!(phase(&w, n), "inactive", "normal sensor: 12 blocks outside its radius 8");
}

/// Sensor triggered by the player: its click sound (player source) makes the neighbouring shrieker shriek.
#[test]
fn sensor_clicking_wakes_shrieker() {
    let mut w = world("g6f10-chain");
    let s = [4, Y, 4];
    let h = [4, Y, 7];
    w.set(s, "minecraft:sculk_sensor");
    w.set(h, "minecraft:sculk_shrieker");
    let _ = w.events();
    let mut evs = Vec::new();
    w.p.s.game_event_src("minecraft:container_open", center([6, Y, 4]), Some(Src::Living(Ent::Player)), None);
    game_ticks(&mut w, 3, &mut evs);
    assert_eq!(phase(&w, s), "active");
    game_ticks(&mut w, 5, &mut evs);
    let hs = w.p.s.get(h);
    assert_eq!(w.p.s.v().get(hs, "shrieking"), Some("true"), "the shrieker shrieks");
}

/// Player on the ground on a sensor: forced step (frequency 1) received with no travel, power 15.
#[test]
fn stepping_on_sensor_activates_it() {
    let mut w = world("g6f10-step");
    let s = [4, Y, 4];
    w.set(s, "minecraft:sculk_sensor");
    let _ = w.events();
    w.p.s.pos = [4.5, f64::from(Y) + 0.5, 4.5];
    w.p.s.on_ground = true;
    let mut evs = Vec::new();
    game_ticks(&mut w, 2, &mut evs);
    assert_eq!(phase(&w, s), "active", "forced step received");
    assert_eq!(power(&w, s), 15);
    assert_eq!(w.comparator(s, Dir::North), 1, "step frequency");
}

/// Sneaking and motionless player on a shrieker: the step comes from the block effects, at each
/// tick, without looking at sneaking or waiting for a movement packet.
#[test]
fn sneaking_still_player_on_shrieker_shrieks() {
    let mut w = world("g6f10-shriek-step");
    let h = [4, Y, 4];
    w.set(h, "minecraft:sculk_shrieker");
    let _ = w.events();
    w.p.s.pos = [4.5, f64::from(Y) + 0.5, 4.5];
    w.p.s.on_ground = true;
    w.p.s.sneaking = true;
    let mut evs = Vec::new();
    game_ticks(&mut w, 2, &mut evs);
    let hs = w.p.s.get(h);
    assert_eq!(w.p.s.v().get(hs, "shrieking"), Some("true"), "the shrieker shrieks");
}

/// Mob on the ground on a sensor: forced step (frequency 1); the warden does not trigger
/// it. Placed on the ground with no AI at the height of the top (no fall, hence no other vibration).
#[test]
fn grounded_mob_steps_on_sensor_but_not_warden() {
    let mut w = world("g6f10-mob-step");
    let (a, b) = ([4, Y, 4], [24, Y, 4]);
    w.set(a, "minecraft:sculk_sensor");
    w.set(b, "minecraft:sculk_sensor");
    let _ = w.events();
    let top = f64::from(Y) + 0.5;
    w.cmd(&format!("summon minecraft:pig 4.5 {top} 4.5 {{NoAI:1b,OnGround:1b}}"));
    w.cmd(&format!("summon minecraft:warden 24.5 {top} 4.5 {{NoAI:1b,OnGround:1b}}"));
    let mut evs = Vec::new();
    game_ticks(&mut w, 3, &mut evs);
    assert_eq!(phase(&w, a), "active", "forced step of the mob");
    assert_eq!(w.comparator(a, Dir::North), 1, "step frequency");
    assert_eq!(phase(&w, b), "inactive", "the warden does not trigger the sensor");
}

/// Sensor activated next to an amethyst block: resonance event of the received frequency
/// and resonance sound.
#[test]
fn sensor_makes_amethyst_resonate() {
    let mut w = world("g6f10-resonate");
    let s = [4, Y, 4];
    w.set(s, "minecraft:sculk_sensor");
    w.set([4, Y, 3], "minecraft:amethyst_block");
    let _ = w.events();
    w.p.s.mobs.rec.vib.log.clear();
    w.p.s.game_event_src("minecraft:container_open", center([7, Y, 4]), None, None);
    let mut evs = Vec::new();
    game_ticks(&mut w, 3, &mut evs);
    assert_eq!(phase(&w, s), "active");
    assert_eq!(logged(&w, "minecraft:resonate_10"), 1, "resonance of frequency 10");
    assert_eq!(w.sounds(&evs, "minecraft:block.amethyst_block.resonate"), 1, "resonance sound");
}

/// Sensor placed with a power but no scheduled tick (command): reset to 0.
#[test]
fn sensor_placed_powered_is_switched_off() {
    let mut w = world("g6f10-placed");
    let s = [4, Y, 4];
    w.cmd(&format!("setblock 4 {Y} 4 minecraft:sculk_sensor[power=12]"));
    assert_eq!(power(&w, s), 0, "power reset to 0");
}

/// Note block's note: the allay retains it after the travel of the vibration (not right
/// away); a wool wall between the two prevents it.
#[test]
fn allay_hears_note_block_through_vibrations() {
    let mut w = world("g6f10-allay");
    let nb = [4, Y, 4];
    w.set(nb, "minecraft:note_block");
    // Full wool wall between the block and the second allay (plane x = 7).
    w.cmd(&format!("fill 7 {} 1 7 {} 7 minecraft:white_wool", Y - 1, Y + 4));
    w.cmd(&format!("summon minecraft:allay 4.5 {Y} 10.5 {{NoGravity:1b}}"));
    w.cmd(&format!("summon minecraft:allay 10.5 {Y} 4.5 {{NoGravity:1b}}"));
    let ids: Vec<(i32, f64)> =
        w.p.s.mobs.list.iter().flatten().filter(|m| m.kind == MobKind::Allay).map(|m| (m.id, m.pos[0])).collect();
    assert_eq!(ids.len(), 2);
    let liked = |w: &W, x: f64| {
        let id = ids.iter().find(|e| e.1 == x).unwrap().0;
        w.p.s.mob_ref(id).and_then(|m| m.bm.brain.as_ref()).and_then(|b| b.mem.liked_noteblock.get()).map(|g| g.pos)
    };
    let mut evs = Vec::new();
    // Listeners registered at the allays' first tick.
    game_ticks(&mut w, 1, &mut evs);
    let st = w.p.s.get(nb);
    w.p.s.note_play(nb, st, Some(Ent::Player));
    game_ticks(&mut w, 1, &mut evs);
    assert_eq!(liked(&w, 4.5), None, "not arrived yet (travel)");
    game_ticks(&mut w, 10, &mut evs);
    assert_eq!(liked(&w, 4.5), Some(nb), "note retained after the travel");
    assert_eq!(liked(&w, 10.5), None, "wool: vibration blocked");
}

/// Arrow stuck in a wooden button: pressed as long as it stays, released 30 ticks after
/// its removal; a stone button ignores it.
#[test]
fn arrows_press_wooden_buttons() {
    let mut w = world("g6f10-button");
    let (oak, stone) = ([4, Y, 4], [8, Y, 4]);
    w.set(oak, "minecraft:oak_button[face=floor]");
    w.set(stone, "minecraft:stone_button[face=floor]");
    let _ = w.events();
    for x in ["4.5", "8.5"] {
        w.cmd(&format!("summon minecraft:arrow {x} {} 4.5 {{Motion:[0.0d,-0.2d,0.0d]}}", f64::from(Y) + 0.3));
    }
    let mut evs = Vec::new();
    game_ticks(&mut w, 3, &mut evs);
    let powered = |w: &W, at: P| w.p.s.v().get_bool(w.p.s.get(at), "powered");
    assert!(powered(&w, oak), "wooden button pressed by the arrow");
    assert!(!powered(&w, stone), "stone button insensitive");
    assert!(logged(&w, "minecraft:block_activate") >= 1, "activation event");
    game_ticks(&mut w, 40, &mut evs);
    assert!(powered(&w, oak), "held as long as the arrow stays");
    for a in w.p.s.proj.list.iter_mut() {
        a.removed = true;
    }
    game_ticks(&mut w, 31, &mut evs);
    assert!(!powered(&w, oak), "released after the arrow is removed");
}

/// Blocked chest: conductive block above (stone, soul sand) or sitting cat, on it
/// or on its paired half; glowstone (non-conductive) does not block. Blocked:
/// no menu and comparator at 0.
#[test]
fn chest_blocked_like_the_game() {
    let mut w = world("g6f10-chest");
    let items = "{Items:[{Slot:0b,id:\"minecraft:stone\",count:64}]}";
    let c = [4, Y, 4];
    let opens = |w: &mut W, at: P| {
        w.p.s.life.menu = None;
        w.p.s.use_block(at);
        let open = w.p.s.life.menu.is_some();
        w.p.s.life.menu = None;
        open
    };
    w.cmd(&format!("setblock 4 {Y} 4 minecraft:chest{items}"));
    w.set([4, Y + 1, 4], "minecraft:glowstone");
    assert!(opens(&mut w, c), "glowstone: opens");
    assert!(w.comparator(c, Dir::North) > 0);
    for top in ["minecraft:stone", "minecraft:soul_sand"] {
        w.set([4, Y + 1, 4], top);
        assert!(!opens(&mut w, c), "{top}: blocked");
        assert_eq!(w.comparator(c, Dir::North), 0, "{top}: comparator at 0");
    }
    w.set([4, Y + 1, 4], "minecraft:air");
    w.cmd(&format!("summon minecraft:cat 4.5 {} 4.5 {{Sitting:1b,NoAI:1b}}", Y + 1));
    assert!(!opens(&mut w, c), "sitting cat: blocked");
    for m in w.p.s.mobs.list.iter_mut().flatten() {
        m.removed = true;
    }
    assert!(opens(&mut w, c), "cat gone: opens");
    // Double chest: the blocked neighbouring half blocks the whole.
    let (l, r) = ([10, Y, 4], [11, Y, 4]);
    w.cmd(&format!("setblock 10 {Y} 4 minecraft:chest[facing=north,type=left]{items}"));
    w.set(r, "minecraft:chest[facing=north,type=right]");
    assert!(opens(&mut w, l), "free double");
    w.set([11, Y + 1, 4], "minecraft:stone");
    assert!(!opens(&mut w, l), "neighbouring half blocked");
    assert_eq!(w.comparator(l, Dir::North), 0);
}

/// Candle cake without color: comparator 14 and falls without support, like the dyed ones.
#[test]
fn plain_candle_cake_like_dyed_ones() {
    let mut w = world("g6f10-cake");
    let c = [4, Y, 4];
    w.set(c, "minecraft:candle_cake");
    assert_eq!(w.comparator(c, Dir::North), 14);
    w.set([4, Y - 1, 4], "minecraft:air");
    let mut evs = Vec::new();
    w.ticks(1, &mut evs);
    assert_eq!(w.name(c), "minecraft:air", "without support: destroyed");
}

/// Bench (`cargo test --release -p rc-sim --lib -- --ignored bench_blocked_chest --nocapture`):
/// "blocked chest" test of a double chest (comparator read) with 500 loaded mobs,
/// none sitting.
#[test]
#[ignore]
fn bench_blocked_chest() {
    let mut w = world("g6f10-bench-chest");
    w.cmd(&format!("setblock 10 {Y} 4 minecraft:chest[facing=north,type=left]"));
    w.set([11, Y, 4], "minecraft:chest[facing=north,type=right]");
    for i in 0..500 {
        let kind = if i % 50 == 0 { "cat" } else { "pig" };
        w.cmd(&format!("summon minecraft:{kind} {} {Y} {} {{NoAI:1b}}", 20 + i % 25, 20 + i / 25));
    }
    // Index check (compared sweep, tests only) switched off for the measurement.
    crate::mobs::index::set_verify(false);
    let n = 200_000u32;
    let t = std::time::Instant::now();
    let mut k = 0u32;
    for _ in 0..n {
        k += u32::from(w.p.s.chest_blocked(std::hint::black_box([10, Y, 4])));
    }
    assert_eq!(k, 0);
    crate::mobs::index::set_verify(true);
    eprintln!("double chest, 500 mobs: {:.1} ns/read", t.elapsed().as_nanos() as f64 / f64::from(n));
}

/// Bench (`cargo test --release -p rc-sim --lib -- --ignored bench_step_on_sensor --nocapture`):
/// per-mob cost on the ground and per tick of the "walked on a sensor" test when a vibration
/// block is loaded (otherwise skipped): effect block at 0.2, then block name.
#[test]
#[ignore]
fn bench_step_on_sensor() {
    let mut w = world("g6f10-bench");
    w.set([4, Y, 4], "minecraft:sculk_sensor");
    let support = Some([10, Y - 1, 10]);
    let pos = [10.5, f64::from(Y), 10.5];
    let n = 2_000_000u32;
    let t = std::time::Instant::now();
    for _ in 0..n {
        w.p.s.sensor_entity_on(std::hint::black_box(pos), support, Src::Living(Ent::Mob(1)), false);
    }
    eprintln!("step on sensor (elsewhere): {:.1} ns/mob", t.elapsed().as_nanos() as f64 / f64::from(n));
}
