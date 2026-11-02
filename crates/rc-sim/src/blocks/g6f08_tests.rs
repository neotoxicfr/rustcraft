//! G6-F08: nature ticks and turtle eggs (hatching, trampling, landing, breaking), eyeblossoms
//! that follow the night, fluid transfer of pointed dripstone (cauldrons, mud), pitcher crop,
//! mushroom spread, redstone ore lit by a click, grass under a shaped block, copper doors and
//! chests that age at the game's pace, sugar cane and cactus age kept off the network, and the
//! box scan shared by vines, mushrooms and eyeblossoms. Fresh flat world from G6-F01: a missing
//! 26.3 cache or client jar makes the test FAIL.

use rc_client::Event;

use super::g6f01_tests::{W, Y, stack, world};
use super::{Dir, P};
use crate::mobs::species::Kind as MobKind;

fn level_events(evs: &[Event], kind: i32) -> Vec<([i32; 3], i32)> {
    evs.iter()
        .filter_map(|e| match e {
            Event::LevelEvent { kind: k, pos, data, .. } if *k == kind => Some((*pos, *data)),
            _ => None,
        })
        .collect()
}

fn set_day(w: &mut W, t: i64) {
    match w.p.s.clocks.iter_mut().find(|(n, _)| n == "minecraft:overworld") {
        Some(c) => c.1 = t,
        None => w.p.s.time = t,
    }
}

fn rtick(w: &mut W, at: P) {
    let s = w.p.s.get(at);
    w.p.s.random_tick(s, at);
}

fn eggs(w: &W, at: P) -> i32 {
    let s = w.p.s.get(at);
    if w.p.s.v().block(s) != w.p.s.bs.t.id.turtle_egg { 0 } else { w.p.s.v().get_int(s, "eggs") }
}

/// Hatch chance of the day timeline: 1 from 21,062 to 21,904 in the overworld, 0.002 at the
/// edges and in the other dimensions.
#[test]
fn turtle_egg_hatch_chance_follows_the_day() {
    let mut w = world("g6f08-chance");
    for (t, want) in [(21_061, 0.002), (21_062, 1.0), (21_904, 1.0), (21_905, 0.002), (6000, 0.002)] {
        set_day(&mut w, t);
        assert_eq!(w.p.s.turtle_egg_hatch_chance(), want, "time {t}");
    }
    set_day(&mut w, 21_500);
    w.p.s.dim.name = "minecraft:the_nether".into();
    assert_eq!(w.p.s.turtle_egg_hatch_chance(), 0.002, "no day timeline in the Nether");
}

/// In the hatch window, on sand: two cracks (sound, hatch +1, "block changed" event), then the
/// hatch: block removed, one baby turtle per egg (age −24,000, home at the nest, 0.3 + 0.2·i),
/// one 2001 per egg. On stone nothing happens; placed on sand: event 2012.
#[test]
fn turtle_eggs_hatch_on_sand() {
    let mut w = world("g6f08-hatch");
    w.cmd("gamerule random_tick_speed 0");
    set_day(&mut w, 21_100);
    let nest = [4, Y, 4];
    w.set([4, Y - 1, 4], "minecraft:sand");
    let _ = w.events();
    w.set(nest, "minecraft:turtle_egg[eggs=3]");
    let evs = w.events();
    assert_eq!(level_events(&evs, 2012), vec![(nest, 15)], "growth particles on sand");
    w.p.s.mobs.rec.vib.log.clear();
    for stage in 1..=2 {
        rtick(&mut w, nest);
        let s = w.p.s.get(nest);
        assert_eq!(w.p.s.v().get_int(s, "hatch"), stage, "crack {stage}");
    }
    let evs = w.events();
    assert_eq!(w.sounds(&evs, "minecraft:entity.turtle.egg_crack"), 2);
    let changes = w.p.s.mobs.rec.vib.log.iter().filter(|e| e.0 == "minecraft:block_change").count();
    assert_eq!(changes, 2, "block_change per crack");
    assert_eq!(level_events(&evs, 2012).len(), 2, "each crack on sand re-emits the growth particles");
    let before = w.p.s.mobs.list.iter().flatten().filter(|m| m.kind == MobKind::Turtle).count();
    rtick(&mut w, nest);
    let evs = w.events();
    assert_eq!(w.p.s.state_name(w.p.s.get(nest)), "minecraft:air", "hatched nest removed");
    assert_eq!(w.sounds(&evs, "minecraft:entity.turtle.egg_hatch"), 1);
    assert_eq!(level_events(&evs, 2001).len(), 3, "one 2001 per egg");
    let babies: Vec<_> = w.p.s.mobs.list.iter().flatten().filter(|m| m.kind == MobKind::Turtle).collect();
    assert_eq!(babies.len() - before, 3, "one turtle per egg");
    for (i, t) in babies.iter().enumerate() {
        assert_eq!(t.age, -24_000, "baby");
        assert_eq!(t.surf.home, nest, "home at the nest");
        let x = 4.0 + 0.3 + i as f64 * 0.2;
        assert!((t.pos[0] - x).abs() < 1e-9 && (t.pos[2] - 4.3).abs() < 1e-9 && t.pos[1] == f64::from(Y), "{:?}", t.pos);
    }
    // On stone: never.
    let rock = [8, Y, 4];
    w.set([8, Y - 1, 4], "minecraft:stone");
    w.set(rock, "minecraft:turtle_egg");
    for _ in 0..50 {
        rtick(&mut w, rock);
    }
    assert_eq!(w.p.s.v().get_int(w.p.s.get(rock), "hatch"), 0, "egg on stone hatched");
}

/// Eggs lost: landing (1 in 3, never a zombie or a turtle; a mob only with mob griefing),
/// trampling (1 in 100, not sneaking), a survival player putting one fewer back after the
/// break, a creative player removing the whole nest.
#[test]
fn turtle_eggs_trampled_landed_and_broken() {
    let mut w = world("g6f08-crush");
    w.cmd("gamerule random_tick_speed 0");
    let at = [4, Y, 4];
    w.set(at, "minecraft:turtle_egg[eggs=4]");
    for _ in 0..300 {
        w.p.s.turtle_egg_fall_on(at, Some(MobKind::Zombie));
        w.p.s.turtle_egg_fall_on(at, Some(MobKind::Husk));
        w.p.s.turtle_egg_fall_on(at, Some(MobKind::Turtle));
        w.p.s.step_on_block(at, false, Some(MobKind::Bat));
        w.p.s.step_on_block(at, true, None);
    }
    assert_eq!(eggs(&w, at), 4, "zombies, turtles, bats and careful steps spare the eggs");
    w.cmd("gamerule mob_griefing false");
    for _ in 0..300 {
        w.p.s.turtle_egg_fall_on(at, Some(MobKind::Pig));
    }
    assert_eq!(eggs(&w, at), 4, "mob without griefing");
    w.cmd("gamerule mob_griefing true");
    let mut n = 0;
    while eggs(&w, at) == 4 {
        w.p.s.turtle_egg_fall_on(at, Some(MobKind::Pig));
        n += 1;
        assert!(n < 200, "landing never breaks");
    }
    assert_eq!(eggs(&w, at), 3);
    let evs = w.events();
    assert_eq!(w.sounds(&evs, "minecraft:entity.turtle.egg_break"), 1);
    assert_eq!(level_events(&evs, 2001).len(), 1, "block particles of the lost egg");
    // Trampled by the player through the step hook.
    let mut n = 0;
    while eggs(&w, at) == 3 {
        w.p.s.step_on_block(at, false, None);
        n += 1;
        assert!(n < 5000, "trampling never breaks");
    }
    assert_eq!(eggs(&w, at), 2);
    // Broken by a survival player: one fewer put back; the last one leaves air.
    w.p.s.game_mode = 0;
    assert!(w.p.s.player_destroy_block(at));
    assert_eq!(eggs(&w, at), 1, "survival break puts back one egg fewer");
    assert!(w.p.s.player_destroy_block(at));
    assert_eq!(w.p.s.state_name(w.p.s.get(at)), "minecraft:air");
    w.set(at, "minecraft:turtle_egg[eggs=3]");
    w.p.s.game_mode = 1;
    assert!(w.p.s.player_destroy_block(at));
    assert_eq!(w.p.s.state_name(w.p.s.get(at)), "minecraft:air", "creative removes the nest");
    // The last egg landed on: the block goes.
    w.p.s.game_mode = 0;
    w.set(at, "minecraft:turtle_egg");
    let mut n = 0;
    while eggs(&w, at) == 1 {
        w.p.s.turtle_egg_fall_on(at, None);
        n += 1;
        assert!(n < 200);
    }
    assert_eq!(w.p.s.state_name(w.p.s.get(at)), "minecraft:air");
}

/// Real landings: a pig and the player falling onto a nest end up breaking eggs (1 in 3 per
/// landing), a zombie never does.
#[test]
fn turtle_eggs_broken_by_real_landings() {
    let mut w = world("g6f08-landing");
    w.cmd("gamerule random_tick_speed 0");
    let top = f64::from(Y) + 7.0 / 16.0;
    let land = |w: &mut W, k: Option<MobKind>, x: i32| {
        let at = [x, Y, 4];
        w.set(at, "minecraft:turtle_egg[eggs=4]");
        for _ in 0..60 {
            if eggs(w, at) < 4 {
                break;
            }
            let pos = [f64::from(x) + 0.5, top + 0.05, 4.5];
            match k {
                Some(k) => {
                    let mut m = w.p.s.new_mob(k, pos, 0.0);
                    m.fall_distance = 3.0;
                    m.vel = [0.0, -0.3, 0.0];
                    w.p.s.mob_move(&mut m, [0.0, -0.3, 0.0]);
                    assert!(m.on_ground, "{k:?} not landed");
                }
                None => {
                    w.p.s.pos = [pos[0], top, pos[2]];
                    w.p.s.on_ground = true;
                    w.p.s.life.vit.fall = 2.9;
                    w.p.s.after_move([pos[0], top + 0.1, pos[2]], false);
                }
            }
        }
        eggs(w, at)
    };
    assert!(land(&mut w, Some(MobKind::Pig), 4) < 4, "a pig landing 60 times broke nothing");
    assert_eq!(land(&mut w, Some(MobKind::Zombie), 8), 4, "zombie landings break eggs");
    w.p.s.game_mode = 0;
    assert!(land(&mut w, None, 12) < 4, "the player landing 60 times broke nothing");
}

/// A turning eyeblossom sends one trail particle (L9): from the block's centre to the point its
/// world draws give (lifetime 0.5 + d, speed (d − 0.5, d + 1, d − 0.5)), reached in 20 ticks per
/// second of lifetime, orange when it opens, grey when it closes, not forced; the potted one too.
#[test]
fn eyeblossom_trail_particle() {
    let mut w = world("l9-eye-trail");
    w.cmd("gamerule random_tick_speed 0");
    let c = [8, Y, 8];
    let pot = [2, Y, 2];
    w.set(c, "minecraft:closed_eyeblossom");
    w.set(pot, "minecraft:potted_closed_eyeblossom");
    let expect = |w: &W, at: P| {
        let mut r = w.p.s.bs.rng.clone();
        let life = 0.5 + r.next_double();
        let v = [r.next_double() - 0.5, r.next_double() + 1.0, r.next_double() - 0.5];
        let s = at.map(|k| f64::from(k) + 0.5);
        (s, [s[0] + v[0] * life, s[1] + v[1] * life, s[2] + v[2] * life], (20.0 * life) as i32)
    };
    for (day, at, color) in [(13_000, c, 0xFC_7812), (13_000, pot, 0xFC_7812), (23_401, c, 0x5F_5F5F)] {
        set_day(&mut w, day);
        let _ = w.events();
        let (start, target, ticks) = expect(&w, at);
        rtick(&mut w, at);
        let evs = w.events();
        let sent = super::g6f01_tests::particles(&w, &evs, "minecraft:trail");
        assert_eq!(sent.len(), 1, "one trail particle at {at:?}");
        let p = &sent[0];
        assert_eq!((p.pos, p.particle.trail, p.particle.color), (start, Some((target, ticks)), color));
        assert_eq!((p.count, p.force, p.always_visible, p.delta, p.speed), (1, false, false, [0.0; 3], [0.0; 3]));
        assert!((10..30).contains(&ticks));
    }
}

/// At night a closed eyeblossom opens on its random tick (long sound, "block changed" with the
/// old state) and wakes the closed ones within ±3/±2/±3 (scheduled tick, short sound), not
/// the farther one; by day it closes again. A potted one turns too. Outside the overworld, no
/// change.
#[test]
fn eyeblossoms_follow_the_night() {
    let mut w = world("g6f08-eye");
    w.cmd("gamerule random_tick_speed 0");
    let c = [8, Y, 8];
    let near = [[10, Y, 9], [5, Y, 8], [8, Y, 11]];
    // Beyond ±3 of every flower of the chain (the woken ones wake their own neighbours).
    let far = [8, Y, 15];
    for p in std::iter::once(c).chain(near).chain([far]) {
        w.set(p, "minecraft:closed_eyeblossom");
    }
    let pot = [2, Y, 2];
    w.set(pot, "minecraft:potted_closed_eyeblossom");
    let _ = w.events();
    set_day(&mut w, 6000);
    rtick(&mut w, c);
    assert_eq!(w.p.s.state_name(w.p.s.get(c)), "minecraft:closed_eyeblossom", "opened by day");
    set_day(&mut w, 13_000);
    w.p.s.mobs.rec.vib.log.clear();
    rtick(&mut w, c);
    assert_eq!(w.p.s.state_name(w.p.s.get(c)), "minecraft:open_eyeblossom");
    let closed = w.p.s.v().default_state(w.p.s.bs.t.id.closed_eyeblossom);
    let ev = w.p.s.mobs.rec.vib.log.iter().find(|e| e.0 == "minecraft:block_change").expect("block_change");
    assert_eq!(ev.3, Some(closed), "event carries the old state");
    let b = w.p.s.bs.t.id.closed_eyeblossom;
    for q in near {
        assert!(w.p.s.has_scheduled_tick(q, b), "neighbour {q:?} not woken");
    }
    assert!(!w.p.s.has_scheduled_tick(far, b), "too far to be woken");
    let mut evs = w.events();
    assert_eq!(w.sounds(&evs, "minecraft:block.eyeblossom.open_long"), 1);
    // The farthest neighbour is at distance 3: delay at most 30.
    let set_time = w.p.s.time;
    w.ticks(31, &mut evs);
    w.p.s.time = set_time;
    for q in near {
        assert_eq!(w.p.s.state_name(w.p.s.get(q)), "minecraft:open_eyeblossom", "neighbour {q:?}");
    }
    assert_eq!(w.sounds(&evs, "minecraft:block.eyeblossom.open"), 3, "short sound of the woken ones");
    assert_eq!(w.p.s.state_name(w.p.s.get(far)), "minecraft:closed_eyeblossom");
    rtick(&mut w, pot);
    assert_eq!(w.p.s.state_name(w.p.s.get(pot)), "minecraft:potted_open_eyeblossom");
    set_day(&mut w, 23_401);
    rtick(&mut w, c);
    rtick(&mut w, pot);
    assert_eq!(w.p.s.state_name(w.p.s.get(c)), "minecraft:closed_eyeblossom", "closes at dawn");
    assert_eq!(w.p.s.state_name(w.p.s.get(pot)), "minecraft:potted_closed_eyeblossom");
    w.p.s.dim.name = "minecraft:the_nether".into();
    set_day(&mut w, 13_000);
    rtick(&mut w, c);
    assert_eq!(w.p.s.state_name(w.p.s.get(c)), "minecraft:closed_eyeblossom", "no timeline in the Nether");
}

/// Random ticks of a stalactite until its fluid transfer (event 1504): returns the tip.
fn drip_until_transfer(w: &mut W, top: P) -> P {
    for _ in 0..600 {
        rtick(w, top);
        let evs = w.events();
        if let Some(&(tip, _)) = level_events(&evs, 1504).first() {
            return tip;
        }
    }
    panic!("no fluid transfer at {top:?}");
}

/// Pointed dripstone under a dripstone block: water above the root fills an empty cauldron
/// (1047) and a water cauldron one level (never past 3), lava gives a lava cauldron (1046),
/// both after 50 + fall height ticks; mud above the root dries into clay at once.
#[test]
fn pointed_dripstone_transfers_fluid() {
    let mut w = world("g6f08-drip");
    w.cmd("gamerule random_tick_speed 0");
    w.p.run(&format!("fill 0 {} 0 15 {} 15 minecraft:stone\n", Y + 5, Y + 6));
    // Root of stone (any block other than the dripstone): the transfer works, the growth (which
    // needs a dripstone block above) cannot move the tip during the test.
    let column = |w: &mut W, x: i32, z: i32, fluid: &str, below: Option<&str>| {
        w.set([x, Y + 5, z], fluid);
        w.set([x, Y + 4, z], "minecraft:stone");
        w.set([x, Y + 3, z], "minecraft:pointed_dripstone[vertical_direction=down,thickness=tip]");
        if let Some(b) = below {
            w.set([x, Y, z], b);
        }
    };
    column(&mut w, 2, 2, "minecraft:water", Some("minecraft:cauldron"));
    column(&mut w, 6, 2, "minecraft:lava", Some("minecraft:cauldron"));
    column(&mut w, 10, 2, "minecraft:mud", None);
    column(&mut w, 2, 6, "minecraft:water", Some("minecraft:water_cauldron[level=1]"));
    column(&mut w, 6, 6, "minecraft:water", Some("minecraft:water_cauldron[level=3]"));
    let _ = w.events();
    // Mud: dried at once.
    w.p.s.mobs.rec.vib.log.clear();
    let tip = drip_until_transfer(&mut w, [10, Y + 3, 2]);
    assert_eq!(tip, [10, Y + 3, 2]);
    assert_eq!(w.p.s.state_name(w.p.s.get([10, Y + 5, 2])), "minecraft:clay", "mud dried");
    let clay = w.p.s.get([10, Y + 5, 2]);
    assert!(
        w.p.s.mobs.rec.vib.log.iter().any(|e| e.0 == "minecraft:block_change" && e.3 == Some(clay)),
        "block_change with the clay"
    );
    // Cauldrons: tick scheduled at 50 + fall height, then filled.
    for (x, z) in [(2, 2), (6, 2), (2, 6), (6, 6)] {
        let tip = drip_until_transfer(&mut w, [x, Y + 3, z]);
        assert_eq!(tip, [x, Y + 3, z]);
        let c = [x, Y, z];
        let b = w.p.s.v().block(w.p.s.get(c));
        assert!(w.p.s.has_scheduled_tick(c, b), "cauldron {c:?} not scheduled");
    }
    let mut evs = Vec::new();
    w.ticks(49 + 2, &mut evs);
    assert_eq!(w.p.s.state_name(w.p.s.get([2, Y, 2])), "minecraft:cauldron", "filled before 50 + fall");
    w.ticks(30, &mut evs);
    assert_eq!(w.p.s.state_name(w.p.s.get([2, Y, 2])), "minecraft:water_cauldron");
    assert_eq!(w.p.s.state_name(w.p.s.get([6, Y, 2])), "minecraft:lava_cauldron");
    assert_eq!(w.p.s.v().get_int(w.p.s.get([2, Y, 6]), "level"), 2, "one level more");
    assert_eq!(w.p.s.v().get_int(w.p.s.get([6, Y, 6]), "level"), 3, "full stays full");
    let fills = level_events(&evs, 1047);
    assert!(fills.contains(&([2, Y, 2], 0)) && fills.contains(&([2, Y, 6], 0)), "{fills:?}");
    assert!(!fills.iter().any(|f| f.0 == [6, Y, 6]), "full cauldron");
    assert_eq!(level_events(&evs, 1046), vec![([6, Y, 2], 0)]);
}

/// The pitcher crop grows at its random ticks up to age 4, a double plant from age 3.
#[test]
fn pitcher_crop_grows() {
    let mut w = world("g6f08-pitcher");
    w.cmd("gamerule random_tick_speed 0");
    let at = [4, Y, 4];
    w.set([4, Y - 1, 4], "minecraft:farmland");
    w.set(at, "minecraft:pitcher_crop[age=0,half=lower]");
    for _ in 0..3000 {
        rtick(&mut w, at);
    }
    let low = w.p.s.get(at);
    let up = w.p.s.get([4, Y + 1, 4]);
    assert_eq!(w.p.s.v().get_int(low, "age"), 4, "ripe");
    assert_eq!(w.p.s.state_name(up), "minecraft:pitcher_crop");
    assert_eq!(w.p.s.v().get(up, "half"), Some("upper"));
    assert_eq!(w.p.s.v().get_int(up, "age"), 4);
}

/// A mushroom on mycelium spreads at its random ticks, never past 5 in its 9×3×9 box.
#[test]
fn mushrooms_spread_up_to_five() {
    let mut w = world("g6f08-mushroom");
    w.cmd("gamerule random_tick_speed 0");
    w.p.run(&format!("fill 0 {} 0 15 {} 15 minecraft:mycelium\n", Y - 1, Y - 1));
    let at = [8, Y, 8];
    w.set(at, "minecraft:brown_mushroom");
    for _ in 0..20_000 {
        rtick(&mut w, at);
    }
    let b = w.p.s.v().block(w.p.s.get(at));
    let mut n = 0;
    for x in -4..=4 {
        for y in -1..=1 {
            for z in -4..=4 {
                n += usize::from(w.p.s.v().block(w.p.s.get([8 + x, Y + y, 8 + z])) == b);
            }
        }
    }
    assert!((2..=5).contains(&n), "{n} mushrooms in the box");
}

/// Right click on redstone ore lights it: empty hand consumed (nothing placed), a block item
/// still placed next to it. The touch draws two floats of the world random per face whose
/// neighbour is not opaque.
#[test]
fn redstone_ore_lit_by_click() {
    let mut w = world("g6f08-ore");
    w.cmd("gamerule random_tick_speed 0");
    let at = [9, Y, 9];
    w.set(at, "minecraft:redstone_ore");
    w.hold(None);
    w.click(at, Dir::Up, [0.5, 1.0, 0.5]);
    assert!(w.p.s.v().get_bool(w.p.s.get(at), "lit"), "lit by an empty-hand click");
    w.set(at, "minecraft:redstone_ore[lit=false]");
    w.hold(stack("minecraft:stone", 4));
    w.click(at, Dir::Up, [0.5, 1.0, 0.5]);
    assert!(w.p.s.v().get_bool(w.p.s.get(at), "lit"), "lit by a block item click");
    assert_eq!(w.p.s.state_name(w.p.s.get([9, Y + 1, 9])), "minecraft:stone", "the block is still placed");
    // Draws: stone above and grass below are opaque, four air sides: 8 floats.
    w.set(at, "minecraft:redstone_ore[lit=false]");
    let mut want = w.p.s.bs.rng.clone();
    let s = w.p.s.get(at);
    w.p.s.redstone_ore_touch(at, s);
    for _ in 0..8 {
        want.next_float();
    }
    assert_eq!(want.next_float(), w.p.s.bs.rng.next_float(), "particle draws of the touch");
}

/// Grass and mycelium die under a bottom slab, bottom stairs or 2 snow layers, not under a top
/// slab, glass or one snow layer. Nylium only reads the top face: it lives under water, dies under
/// a single snow layer.
#[test]
fn grass_reads_the_shape_above() {
    let mut w = world("g6f08-grass");
    w.cmd("gamerule random_tick_speed 0");
    // The opaque override of the old rule matches the extracted attenuation (always 15).
    for (i, info) in w.p.s.data.states.iter().enumerate() {
        if info.has(rc_data::flags::SOLID_RENDER) {
            assert_eq!(info.light_dampening, 15, "opaque state {i}");
        }
    }
    let cases = [
        ("minecraft:stone_slab[type=bottom]", false),
        ("minecraft:oak_stairs[half=bottom]", false),
        ("minecraft:snow[layers=2]", false),
        ("minecraft:stone_slab[type=top]", true),
        ("minecraft:glass", true),
        ("minecraft:snow[layers=1]", true),
        ("minecraft:stone", false),
    ];
    for (i, (above, alive)) in cases.iter().enumerate() {
        let g = [1 + 2 * i as i32, Y - 1, 2];
        w.set([g[0], Y, g[2]], above);
        rtick(&mut w, g);
        let want = if *alive { "minecraft:grass_block" } else { "minecraft:dirt" };
        assert_eq!(w.p.s.state_name(w.p.s.get(g)), want, "grass under {above}");
    }
    let m = [1, Y - 1, 6];
    w.set(m, "minecraft:mycelium");
    w.set([1, Y, 6], "minecraft:stone_slab[type=bottom]");
    rtick(&mut w, m);
    assert_eq!(w.p.s.state_name(w.p.s.get(m)), "minecraft:dirt", "mycelium under a bottom slab");
    for (x, above, alive) in [(5, "minecraft:water", true), (7, "minecraft:snow[layers=1]", false)] {
        let n = [x, Y - 1, 6];
        w.set(n, "minecraft:crimson_nylium");
        w.set([x, Y, 6], above);
        rtick(&mut w, n);
        let want = if alive { "minecraft:crimson_nylium" } else { "minecraft:netherrack" };
        assert_eq!(w.p.s.state_name(w.p.s.get(n)), want, "nylium under {above}");
    }
}

/// Copper door: only the lower half ages (the upper copies it). Copper chest: never the right
/// half nor an open one; a closed single chest ages.
#[test]
fn copper_door_and_chest_age_like_the_game() {
    let mut w = world("g6f08-copper");
    w.cmd("gamerule random_tick_speed 0");
    let low = [4, Y, 4];
    let up = [4, Y + 1, 4];
    w.set(low, "minecraft:copper_door[half=lower]");
    w.set(up, "minecraft:copper_door[half=upper]");
    let before = w.p.s.bs.rng.clone();
    for _ in 0..3000 {
        rtick(&mut w, up);
    }
    assert_eq!(w.p.s.state_name(w.p.s.get(low)), "minecraft:copper_door", "upper half aged the door");
    let mut a = before;
    assert_eq!(a.next_float(), w.p.s.bs.rng.next_float(), "upper half drew the random");
    for _ in 0..3000 {
        rtick(&mut w, low);
    }
    let aged = w.p.s.state_name(w.p.s.get(low)).to_owned();
    assert_ne!(aged, "minecraft:copper_door", "lower half never aged");
    assert_eq!(w.p.s.state_name(w.p.s.get(up)), aged, "upper half follows");
    let right = [8, Y, 4];
    let open = [8, Y, 8];
    let single = [12, Y, 8];
    w.set(right, "minecraft:copper_chest[type=right]");
    w.set(open, "minecraft:copper_chest");
    w.set(single, "minecraft:copper_chest");
    w.p.s.life.openers.insert(open, 1);
    for _ in 0..3000 {
        rtick(&mut w, right);
        rtick(&mut w, open);
        rtick(&mut w, single);
    }
    assert_eq!(w.p.s.state_name(w.p.s.get(right)), "minecraft:copper_chest", "right half aged");
    assert_eq!(w.p.s.state_name(w.p.s.get(open)), "minecraft:copper_chest", "open chest aged");
    assert_ne!(w.p.s.state_name(w.p.s.get(single)), "minecraft:copper_chest", "closed chest never aged");
}

/// The age of sugar cane and cactus changes without being sent to the clients (flags 260).
#[test]
fn cane_and_cactus_age_stays_on_the_server() {
    let mut w = world("g6f08-cane");
    w.cmd("gamerule random_tick_speed 0");
    for (at, name) in [([3, Y, 3], "minecraft:sugar_cane"), ([6, Y, 3], "minecraft:cactus")] {
        w.set(at, name);
        let _ = w.events();
        assert!(w.p.s.changed.is_empty());
        rtick(&mut w, at);
        assert_eq!(w.p.s.v().get_int(w.p.s.get(at), "age"), 1, "{name} aged");
        assert!(!w.p.s.changed.contains(&at), "{name} age queued for the clients");
    }
}

/// Micro-bench (ignored; `--ignored bench_g6f08 --nocapture`, palette prefilter cut with
/// `RUSTCRAFT_OPT_SCAN_PALETTE=0`): spread limit of a vine, old walk cell by cell through `get`
/// against the scan by section, on 10 % vines over a 32 × 4 × 32 area; and the grass survival
/// rule, old (plain attenuation) against the attenuation through the top face.
#[test]
#[ignore]
fn bench_g6f08_scan_and_grass() {
    let mut w = world("g6f08-bench");
    w.cmd("gamerule random_tick_speed 0");
    let mut r = 99u32;
    let mut vines = Vec::new();
    for x in -8..24 {
        for y in Y..Y + 4 {
            for z in -8..24 {
                r = r.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                if (r >> 16).is_multiple_of(10) {
                    w.p.s.set_block([x, y, z], w.p.s.default_state("minecraft:vine"), 2);
                    vines.push([x, y, z]);
                }
            }
        }
    }
    let vine = w.p.s.bs.t.id.vine;
    let old = |s: &crate::server::Server, p: P| {
        let mut n = 4;
        for x in -4..=4 {
            for y in -1..=1 {
                for z in -4..=4 {
                    if s.v().block(s.get([p[0] + x, p[1] + y, p[2] + z])) == vine {
                        n -= 1;
                        if n < 0 {
                            return false;
                        }
                    }
                }
            }
        }
        true
    };
    // Also a sparse case: the same number of calls around lone vines in an empty sky band.
    let sky: Vec<P> = (0..64).map(|i| [i % 16, Y + 40, i / 16 * 4]).collect();
    for &p in &sky {
        w.p.s.set_block(p, w.p.s.default_state("minecraft:vine"), 2);
    }
    let s = &w.p.s;
    for (label, set) in [("dense", &vines), ("sparse", &sky)] {
        let mut agree = 0;
        for &p in set.iter() {
            agree += usize::from(old(s, p) == s.at_most_in_box(p, [-4, -1, -4], [4, 1, 4], vine, 4));
        }
        assert_eq!(agree, set.len(), "{label}: results differ");
        let rounds = (200_000 / set.len()).max(1);
        for pass in 0..2 {
            let t = std::time::Instant::now();
            let mut k = 0;
            for _ in 0..rounds {
                for &p in set.iter() {
                    k += usize::from(old(s, p));
                }
            }
            let a = t.elapsed().as_nanos() as f64 / (rounds * set.len()) as f64;
            let t = std::time::Instant::now();
            for _ in 0..rounds {
                for &p in set.iter() {
                    k += usize::from(s.at_most_in_box(p, [-4, -1, -4], [4, 1, 4], vine, 4));
                }
            }
            let b = t.elapsed().as_nanos() as f64 / (rounds * set.len()) as f64;
            println!("[g6f08] {label} pass {pass}: old walk {a:.1} ns, scan {b:.1} ns ({k})");
        }
    }
    // Grass survival rule on the whole state table (above = every state in turn).
    let grass = s.v().default_state(s.bs.t.id.grass_block);
    let n = s.data.states.len() as u32;
    for pass in 0..2 {
        let t = std::time::Instant::now();
        let mut k = 0u32;
        for _ in 0..20 {
            for a in 0..n {
                let i = s.v().info(a);
                let d = if i.has(rc_data::flags::SOLID_RENDER) { 15 } else { i.light_dampening };
                k += u32::from(d < 15);
            }
        }
        let old_ns = t.elapsed().as_nanos() as f64 / f64::from(20 * n);
        let t = std::time::Instant::now();
        for _ in 0..20 {
            for a in 0..n {
                k += u32::from(s.light.dampening_into(grass, a, 1) < 15);
            }
        }
        let new_ns = t.elapsed().as_nanos() as f64 / f64::from(20 * n);
        println!("[g6f08] grass rule pass {pass}: old {old_ns:.2} ns, new {new_ns:.2} ns ({k})");
    }
}

/// The box scan visits in the game's order (sections x, z, y; cells y, z, x) and the spread
/// limit counts like a full walk.
#[test]
fn box_scan_order_and_limit() {
    let mut w = world("g6f08-scan");
    w.cmd("gamerule random_tick_speed 0");
    let mut placed = Vec::new();
    for x in [15, 16] {
        for y in [-49, -48] {
            for z in [15, 16] {
                placed.push([x, y, z]);
                w.set([x, y, z], "minecraft:obsidian");
            }
        }
    }
    let ob = w.p.s.v().block(w.p.s.get([15, -49, 15]));
    let mut seen = Vec::new();
    let v = w.p.s.v();
    w.p.s.scan_box([15, -49, 15], [16, -48, 16], |s| v.block(s) == ob, |q, _| {
        seen.push(q);
        true
    });
    let min_y = w.p.s.dim.height.min_y;
    placed.sort_by_key(|q| (q[0] >> 4, q[2] >> 4, (q[1] - min_y) >> 4, q[1], q[2], q[0]));
    assert_eq!(seen, placed, "scan order");
    // Spread limit against a full walk, on a scattered pattern.
    let mut r = 12_345u32;
    for x in 0..16 {
        for z in 0..16 {
            r = r.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            if r >> 28 == 0 {
                w.set([x, Y + ((r >> 16) & 1) as i32, z], "minecraft:obsidian");
            }
        }
    }
    for p in [[4, Y, 4], [8, Y, 8], [12, Y + 1, 3], [0, Y, 15]] {
        let mut count = 0;
        for x in -4..=4 {
            for y in -1..=1 {
                for z in -4..=4 {
                    count += usize::from(w.p.s.v().block(w.p.s.get([p[0] + x, p[1] + y, p[2] + z])) == ob);
                }
            }
        }
        for n in 0..6 {
            assert_eq!(w.p.s.at_most_in_box(p, [-4, -1, -4], [4, 1, 4], ob, n), count <= n, "{p:?} n={n} count={count}");
        }
    }
}
