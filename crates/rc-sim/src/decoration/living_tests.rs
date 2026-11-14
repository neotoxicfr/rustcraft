//! Armor stands and mannequins as the living entities the game makes of them, on a copy of the
//! test world (`RUSTCRAFT_SOLO_WORLD`): fire and lava, currents and deep water, plates, tripwire
//! and turtle eggs, the player's hits, falls and drowning. Expected values worked out from the
//! game's rules (none read off our own code). World absent: tests skipped.

use rc_data::PropKey;
use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use super::DKind;
use super::tests::{decos, done, dropped, meta_of, probe, ticks};
use crate::blocks::probe::Probe;
use crate::nbtio;

fn cmd(p: &mut Probe, c: &str) {
    let mut src = p.s.player_source();
    src.level = 4;
    let (ok, _) = p.s.perform(src, c);
    assert!(ok || !c.starts_with("summon"), "command refused: {c}");
}

/// The player out of the way (it would press the plates itself).
fn player_aside(p: &mut Probe) {
    p.s.pos = [97.5, -59.0, 97.5];
}

/// The one decoration of kind `k` at x `x`: identifier and compound.
fn at_x(p: &Probe, k: DKind, x: f64) -> Option<(i32, Nbt)> {
    decos(p, k).into_iter().find(|(_, e)| nbtio::f64s::<3>(e.get("Pos")).is_some_and(|q| (q[0] - x).abs() < 0.75))
}

fn pos_of(e: &Nbt) -> [f64; 3] {
    nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default()
}

fn health_of(e: &Nbt) -> f32 {
    match e.get("Health") {
        Some(Nbt::Float(h)) => *h,
        _ => 20.0,
    }
}

fn prop(p: &Probe, at: [i32; 3], k: PropKey) -> Option<String> {
    p.s.view().get(p.s.st(at), k).map(str::to_owned)
}

/// Armor stand on a lit campfire: the campfire sets it alight (5 s), then, burning, takes 0.15 a
/// tick and its own fire 4 every 20 ticks: shown burning, it breaks within 6 s, dropping its
/// helmet but never itself.
#[test]
fn armor_stand_on_a_campfire_burns_and_breaks() {
    let Some((mut p, dir)) = probe("deco-stand-campfire") else { return };
    player_aside(&mut p);
    cmd(&mut p, "setblock 104 -59 104 minecraft:campfire[lit=true]");
    cmd(&mut p, "summon minecraft:armor_stand 104.5 -58.5 104.5 {equipment:{head:{id:\"minecraft:iron_helmet\",count:1}}}");
    let (id, _) = at_x(&p, DKind::Stand, 104.5).expect("armor stand");
    let _ = p.take_events();
    ticks(&mut p, 3);
    let (_, e) = at_x(&p, DKind::Stand, 104.5).expect("still standing after 3 ticks");
    assert!(super::mannequin::fire_ticks(&e) > 90, "set alight by the campfire: {:?}", e.get("Fire"));
    assert!(health_of(&e) < 20.0, "burning: health {}", health_of(&e));
    assert!(meta_of(&p, id).contains(&(0, Meta::Byte(1))), "shown burning to the clients");
    ticks(&mut p, 120);
    assert!(at_x(&p, DKind::Stand, 104.5).is_none(), "burnt down within 6 s");
    assert_eq!(dropped(&p, "minecraft:iron_helmet"), 1, "its equipment dropped");
    assert_eq!(dropped(&p, "minecraft:armor_stand"), 0, "never the stand itself");
    done(p, dir);
}

/// Armor stand dropped into lava: set alight for 15 s; lava itself does not hurt it, and its fire
/// only hurts it out of lava (the game's rule), so it stands at full health inside.
#[test]
fn armor_stand_in_lava_catches_fire() {
    let Some((mut p, dir)) = probe("deco-stand-lava") else { return };
    player_aside(&mut p);
    cmd(&mut p, "setblock 104 -59 104 minecraft:lava");
    cmd(&mut p, "summon minecraft:armor_stand 104.5 -58.5 104.5");
    ticks(&mut p, 30);
    let (_, e) = at_x(&p, DKind::Stand, 104.5).expect("armor stand");
    let fire = super::mannequin::fire_ticks(&e);
    assert!((270..=300).contains(&fire), "set alight for 15 s: {fire}");
    assert_eq!(health_of(&e), 20.0, "no damage while in lava");
    assert!(pos_of(&e)[1] < -58.5, "sunk into the lava: {:?}", pos_of(&e));
    done(p, dir);
}

/// Flowing water (source at x 100, levels 1 to 7 eastward): the current carries an armor stand
/// east, instead of leaving it stuck where it stood.
#[test]
fn armor_stand_carried_by_the_current() {
    let Some((mut p, dir)) = probe("deco-stand-current") else { return };
    player_aside(&mut p);
    cmd(&mut p, "setblock 100 -59 104 minecraft:water");
    for level in 1..=7 {
        cmd(&mut p, &format!("setblock {} -59 104 minecraft:water[level={level}]", 100 + level));
    }
    cmd(&mut p, "summon minecraft:armor_stand 101.5 -59 104.5");
    ticks(&mut p, 40);
    let (_, e) = at_x(&p, DKind::Stand, 103.0).or_else(|| decos(&p, DKind::Stand).into_iter().next()).expect("stand");
    assert!(pos_of(&e)[0] > 102.0, "carried east by the current: {:?}", pos_of(&e));
    done(p, dir);
}

/// Deep water: an armor stand summoned inside it sinks slowly to the bottom instead of hanging in
/// the water.
#[test]
fn armor_stand_sinks_in_deep_water() {
    let Some((mut p, dir)) = probe("deco-stand-deep") else { return };
    player_aside(&mut p);
    cmd(&mut p, "fill 103 -59 103 105 -55 105 minecraft:water");
    cmd(&mut p, "summon minecraft:armor_stand 104.5 -56.5 104.5");
    ticks(&mut p, 10);
    let y10 = pos_of(&at_x(&p, DKind::Stand, 104.5).expect("stand").1)[1];
    assert!(y10 < -56.5 && y10 > -59.0, "sinking slowly: {y10}");
    ticks(&mut p, 200);
    let e = at_x(&p, DKind::Stand, 104.5).expect("stand").1;
    assert_eq!(pos_of(&e)[1], -59.0, "at the bottom");
    done(p, dir);
}

/// Plates: an armor stand presses a stone plate; two press a light weighted plate to 2; a marker
/// presses nothing.
#[test]
fn armor_stands_press_and_weigh_on_plates() {
    let Some((mut p, dir)) = probe("deco-stand-plates") else { return };
    player_aside(&mut p);
    cmd(&mut p, "setblock 104 -59 104 minecraft:stone_pressure_plate");
    cmd(&mut p, "setblock 106 -59 104 minecraft:light_weighted_pressure_plate");
    cmd(&mut p, "setblock 108 -59 104 minecraft:stone_pressure_plate");
    cmd(&mut p, "summon minecraft:armor_stand 104.5 -59 104.5");
    cmd(&mut p, "summon minecraft:armor_stand 106.3 -59 104.5");
    cmd(&mut p, "summon minecraft:armor_stand 106.7 -59 104.5");
    cmd(&mut p, "summon minecraft:armor_stand 108.5 -59 104.5 {Marker:1b}");
    ticks(&mut p, 2);
    assert_eq!(prop(&p, [104, -59, 104], PropKey::Powered).as_deref(), Some("true"), "stone plate pressed");
    assert_eq!(prop(&p, [106, -59, 104], PropKey::Power).as_deref(), Some("2"), "two stands weighed");
    assert_eq!(prop(&p, [108, -59, 104], PropKey::Powered).as_deref(), Some("false"), "a marker presses nothing");
    done(p, dir);
}

/// Tripwire: an armor stand standing on the string powers it.
#[test]
fn armor_stand_trips_the_wire() {
    let Some((mut p, dir)) = probe("deco-stand-tripwire") else { return };
    player_aside(&mut p);
    cmd(&mut p, "setblock 104 -59 104 minecraft:tripwire");
    cmd(&mut p, "summon minecraft:armor_stand 104.5 -59 104.5");
    ticks(&mut p, 2);
    assert_eq!(prop(&p, [104, -59, 104], PropKey::Powered).as_deref(), Some("true"), "string tripped");
    done(p, dir);
}

/// Turtle eggs under an armor stand: one chance in 100 per tick that one cracks (`mob_griefing`);
/// with the rule off they are spared.
#[test]
fn armor_stand_cracks_turtle_eggs() {
    let Some((mut p, dir)) = probe("deco-stand-eggs") else { return };
    player_aside(&mut p);
    cmd(&mut p, "setblock 104 -59 104 minecraft:turtle_egg[eggs=4]");
    cmd(&mut p, "setblock 108 -59 104 minecraft:turtle_egg[eggs=4]");
    cmd(&mut p, "summon minecraft:armor_stand 104.5 -58 104.5");
    ticks(&mut p, 3000);
    assert_ne!(prop(&p, [104, -59, 104], PropKey::Eggs).as_deref(), Some("4"), "eggs cracked under the stand");
    cmd(&mut p, "gamerule mob_griefing false");
    cmd(&mut p, "summon minecraft:armor_stand 108.5 -58 104.5");
    ticks(&mut p, 3000);
    assert_eq!(prop(&p, [108, -59, 104], PropKey::Eggs).as_deref(), Some("4"), "spared without mob_griefing");
    done(p, dir);
}

/// The player's fist on a mannequin: 1 point of damage and the default knockback away from the
/// player, like any living entity.
#[test]
fn mannequin_hit_by_the_player() {
    let Some((mut p, dir)) = probe("deco-mannequin-hit") else { return };
    cmd(&mut p, "summon minecraft:mannequin 105.5 -59 104.5");
    let (id, _) = at_x(&p, DKind::Mannequin, 105.5).expect("mannequin");
    ticks(&mut p, 2);
    p.s.mobs.attack_ticker = 100;
    assert!(p.s.deco_attacked(id));
    let (_, e) = at_x(&p, DKind::Mannequin, 105.5).expect("mannequin");
    assert_eq!(health_of(&e), 19.0, "fist: 1 point");
    let v = nbtio::motion(e.get("Motion")).unwrap_or_default();
    assert!(v[0] > 0.3, "knocked back away from the player: {v:?}");
    done(p, dir);
}

/// A mannequin dropped from 10.5 blocks: floor(10.5 − 3) = 7 points of fall damage.
#[test]
fn mannequin_takes_fall_damage() {
    let Some((mut p, dir)) = probe("deco-mannequin-fall") else { return };
    player_aside(&mut p);
    cmd(&mut p, "summon minecraft:mannequin 104.5 -48.5 104.5");
    ticks(&mut p, 60);
    let (_, e) = at_x(&p, DKind::Mannequin, 104.5).expect("mannequin");
    assert_eq!(pos_of(&e)[1], -59.0, "landed");
    assert_eq!(health_of(&e), 13.0, "7 points of fall damage");
    done(p, dir);
}

/// Bench (`--ignored bench_resting_living_decorations --nocapture`, release build): microseconds
/// per decoration tick for 200 armor stands and 100 mannequins resting on stone (the usual case:
/// fluids, fire, movement, block pass, tracking each tick), and the same over a stone pressure
/// plate field (the plates count them).
#[test]
#[ignore]
fn bench_resting_living_decorations() {
    let Some((mut p, dir)) = probe("deco-bench-resting") else { return };
    player_aside(&mut p);
    for k in 0..300 {
        let (x, z) = (96.5 + f64::from(k % 20), 98.5 + f64::from(k / 20));
        let kind = if k % 3 == 2 { "mannequin" } else { "armor_stand" };
        cmd(&mut p, &format!("summon minecraft:{kind} {x} -59 {z}"));
    }
    ticks(&mut p, 5);
    for (label, plates) in [("on stone", false), ("on plates", true)] {
        if plates {
            cmd(&mut p, "fill 96 -59 98 115 -59 112 minecraft:stone_pressure_plate");
            ticks(&mut p, 5);
        }
        for pass in 0..3 {
            let n = 200;
            let t = std::time::Instant::now();
            ticks(&mut p, n);
            let per = t.elapsed().as_secs_f64() * 1e6 / f64::from(n);
            eprintln!("pass {pass}, {label}: {per:.1} us per decoration tick (300 living decorations)");
            let _ = p.take_events();
        }
    }
    done(p, dir);
}

/// Under water a mannequin uses up its air, one a tick (an armor stand breathes under water).
#[test]
fn mannequin_runs_out_of_air() {
    let Some((mut p, dir)) = probe("deco-mannequin-air") else { return };
    player_aside(&mut p);
    cmd(&mut p, "fill 103 -59 103 106 -55 105 minecraft:water");
    cmd(&mut p, "summon minecraft:mannequin 104.5 -59 104.5");
    cmd(&mut p, "summon minecraft:armor_stand 105.5 -59 104.5");
    ticks(&mut p, 50);
    let air = |e: &Nbt| nbtio::int(e, "Air");
    assert_eq!(air(&at_x(&p, DKind::Mannequin, 104.5).expect("mannequin").1), Some(250), "50 ticks of air used");
    assert_eq!(air(&at_x(&p, DKind::Stand, 105.5).expect("stand").1), Some(300), "the armor stand breathes");
    done(p, dir);
}
