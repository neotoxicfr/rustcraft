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

/// `block` particles sent among `events`, summed per landing x (rounded to the block).
fn dust_at_x(p: &Probe, events: &[rc_client::Event], x: f64) -> i32 {
    let block = p.s.data.registry("minecraft:particle_type").and_then(|r| r.entries.iter().position(|e| e == "minecraft:block"));
    let block = block.expect("block particle missing") as i32;
    (events.iter())
        .filter_map(|e| match e {
            rc_client::Event::Particles(q) if q.particle.kind == block && (q.pos[0] - x).abs() < 0.75 => Some(q.count),
            _ => None,
        })
        .sum()
}

/// Sounds named `name` among `events`.
fn sound_count(p: &Probe, events: &[rc_client::Event], name: &str) -> usize {
    let id = p.s.sound_id(name).unwrap_or_else(|| panic!("unknown sound {name}"));
    (events.iter())
        .filter(|e| matches!(e, rc_client::Event::Sound(s) if matches!(s.sound, rc_proto_v777::play::SoundRef::Id(i) if i == id)))
        .count()
}

/// Game events named `name` given off since the log was last cleared, then the log cleared.
fn game_events(p: &mut Probe, name: &str) -> usize {
    let n = p.s.mobs.rec.vib.log.iter().filter(|e| e.0 == name).count();
    p.s.mobs.rec.vib.log.clear();
    n
}

fn motion_of(e: &Nbt) -> [f64; 3] {
    nbtio::motion(e.get("Motion")).unwrap_or_default()
}

/// Landing dust counts the fall from before the last move: dropped 4 blocks, the fall is 3.35
/// before it (power 0: no dust, where the whole 4 blocks would give 40 shards); dropped 10.5
/// blocks, 9.67 before it: power 6, 150 × (0.2 + 6/15) = 90 shards (not 100).
#[test]
fn landing_dust_counts_the_fall_before_the_last_move() {
    let Some((mut p, dir)) = probe("deco-stand-dust") else { return };
    player_aside(&mut p);
    cmd(&mut p, "summon minecraft:armor_stand 104.5 -55 104.5");
    cmd(&mut p, "summon minecraft:armor_stand 108.5 -48.5 104.5");
    let _ = p.take_events();
    ticks(&mut p, 40);
    let events = p.take_events();
    assert_eq!(pos_of(&at_x(&p, DKind::Stand, 104.5).expect("stand").1)[1], -59.0, "landed");
    assert_eq!(dust_at_x(&p, &events, 104.5), 0, "4 blocks: no landing dust");
    assert_eq!(dust_at_x(&p, &events, 108.5), 90, "10.5 blocks: 90 shards");
    done(p, dir);
}

/// An armor stand summoned inside water makes no splash on its first tick (it was never out of
/// the water); one dropped into it from above splashes once, with the `splash` event.
#[test]
fn no_splash_on_the_first_tick_in_water() {
    let Some((mut p, dir)) = probe("deco-stand-splash") else { return };
    player_aside(&mut p);
    cmd(&mut p, "fill 103 -59 103 110 -57 105 minecraft:water");
    let _ = p.take_events();
    let _ = game_events(&mut p, "minecraft:splash");
    cmd(&mut p, "summon minecraft:armor_stand 104.5 -58 104.5");
    ticks(&mut p, 1);
    let events = p.take_events();
    assert_eq!(sound_count(&p, &events, "minecraft:entity.generic.splash"), 0, "no splash sound on the first tick");
    assert_eq!(game_events(&mut p, "minecraft:splash"), 0, "no splash event on the first tick");
    ticks(&mut p, 20);
    cmd(&mut p, "summon minecraft:armor_stand 108.5 -54 104.5");
    let _ = p.take_events();
    ticks(&mut p, 20);
    let events = p.take_events();
    assert_eq!(sound_count(&p, &events, "minecraft:entity.generic.splash"), 1, "dropped in: one splash");
    assert_eq!(game_events(&mut p, "minecraft:splash"), 1, "dropped in: one splash event");
    done(p, dir);
}

/// Sinking in still water against a wall, near the surface: the climb-out room is the box lifted
/// from where the move started (-57.25), 0.6 up with the new speed (-0.325): its bottom at
/// -56.975 is out of the water, so it is lifted at 0.3. From where it arrived (0.4 lower) the
/// room would still be in the water.
#[test]
fn armor_stand_climbs_out_of_water_from_where_it_started() {
    let Some((mut p, dir)) = probe("deco-stand-ledge") else { return };
    player_aside(&mut p);
    cmd(&mut p, "fill 103 -59 103 105 -58 105 minecraft:water");
    cmd(&mut p, "fill 106 -59 103 106 -58 105 minecraft:stone");
    cmd(&mut p, "summon minecraft:armor_stand 105.6 -57.25 104.5 {Motion:[0.3d,-0.4d,0.0d]}");
    ticks(&mut p, 1);
    let (_, e) = at_x(&p, DKind::Stand, 105.75).expect("stand");
    assert!((pos_of(&e)[1] + 57.65).abs() < 1.0e-9, "sank 0.4: {:?}", pos_of(&e));
    assert_eq!(motion_of(&e)[1], f64::from(0.3f32), "lifted out against the wall");
    done(p, dir);
}

/// An armor stand falling into a current that flows against a wall, while it moves into that
/// wall: the current's push lands on the speed it moved with, then the blocked axis stops, so it
/// keeps no speed into the wall (pushed after the stop, it would keep the current's push).
#[test]
fn current_push_before_the_wall_stops_it() {
    let Some((mut p, dir)) = probe("deco-stand-current-wall") else { return };
    player_aside(&mut p);
    cmd(&mut p, "fill 104 -59 103 104 -56 105 minecraft:stone");
    cmd(&mut p, "setblock 100 -59 104 minecraft:water");
    for level in 1..=3 {
        cmd(&mut p, &format!("setblock {} -59 104 minecraft:water[level={level}]", 100 + level));
    }
    cmd(&mut p, "summon minecraft:armor_stand 103.7 -58.3 104.5 {Motion:[0.2d,-0.3d,0.0d]}");
    ticks(&mut p, 1);
    let (_, e) = at_x(&p, DKind::Stand, 103.75).expect("stand");
    assert!((pos_of(&e)[0] - 103.75).abs() < 1.0e-9, "stopped by the wall: {:?}", pos_of(&e));
    assert!(pos_of(&e)[1] < -58.45, "in the water now: {:?}", pos_of(&e));
    assert_eq!(motion_of(&e)[0], 0.0, "no speed into the wall");
    done(p, dir);
}

/// A mannequin falling 20 blocks down a ladder shaft: on the ladders its fall is forgotten each
/// tick and its descent held to 0.15 a tick, so it lands unhurt (free fall would deal 17).
#[test]
fn mannequin_slides_down_a_ladder_unhurt() {
    let Some((mut p, dir)) = probe("deco-mannequin-ladder") else { return };
    player_aside(&mut p);
    cmd(&mut p, "fill 104 -59 105 104 -38 105 minecraft:stone");
    cmd(&mut p, "fill 104 -59 104 104 -38 104 minecraft:ladder[facing=north]");
    cmd(&mut p, "summon minecraft:mannequin 104.5 -39 104.5");
    ticks(&mut p, 20);
    let y = pos_of(&at_x(&p, DKind::Mannequin, 104.5).expect("mannequin").1)[1];
    assert!(y > -43.0, "held to 0.15 a tick: {y}");
    ticks(&mut p, 200);
    let (_, e) = at_x(&p, DKind::Mannequin, 104.5).expect("mannequin");
    assert_eq!(pos_of(&e)[1], -59.0, "at the bottom");
    assert_eq!(health_of(&e), 20.0, "unhurt");
    done(p, dir);
}

/// An armor stand dropped 3 blocks onto a slime block bounces back up (with the `bounce` event)
/// instead of stopping dead.
#[test]
fn armor_stand_bounces_on_slime() {
    let Some((mut p, dir)) = probe("deco-stand-slime-bounce") else { return };
    player_aside(&mut p);
    cmd(&mut p, "setblock 104 -60 104 minecraft:slime_block");
    cmd(&mut p, "summon minecraft:armor_stand 104.5 -56 104.5");
    let _ = game_events(&mut p, "minecraft:bounce");
    let mut highest = f64::MIN;
    for _ in 0..30 {
        ticks(&mut p, 1);
        highest = highest.max(motion_of(&at_x(&p, DKind::Stand, 104.5).expect("stand").1)[1]);
    }
    assert!(highest > 0.3, "bounced up: highest vertical speed {highest}");
    assert!(game_events(&mut p, "minecraft:bounce") >= 1, "bounce event");
    done(p, dir);
}

/// Soul sand slows a pushed armor stand (speed factor 0.4 before the air's drag); a slime block
/// it stands on holds back its sideways speed (× 0.4 + |vy| × 0.2).
#[test]
fn soul_sand_and_slime_slow_a_pushed_armor_stand() {
    let Some((mut p, dir)) = probe("deco-stand-slow-blocks") else { return };
    player_aside(&mut p);
    cmd(&mut p, "fill 100 -60 104 103 -60 104 minecraft:soul_sand");
    cmd(&mut p, "fill 108 -60 104 112 -60 104 minecraft:slime_block");
    cmd(&mut p, "summon minecraft:armor_stand 100.5 -59.125 104.5 {Motion:[0.5d,0.0d,0.0d]}");
    cmd(&mut p, "summon minecraft:armor_stand 108.5 -59 104.5 {Motion:[0.3d,-0.0784d,0.0d],OnGround:1b}");
    ticks(&mut p, 1);
    let sand = motion_of(&at_x(&p, DKind::Stand, 101.0).expect("stand on soul sand").1);
    assert_eq!(sand[0], 0.5 * f64::from(0.4f32) * f64::from(0.91f32), "soul sand factor then the air's drag");
    let slime = motion_of(&at_x(&p, DKind::Stand, 108.8).expect("stand on slime").1);
    let vy = (0.0 - 0.08) * f64::from(0.98f32);
    assert_eq!(slime[1], vy, "resting on the slime: no bounce");
    assert_eq!(slime[0], 0.3 * f64::from(0.8f32 * 0.91f32) * (0.4 + vy.abs() * 0.2), "slime friction then its step");
    done(p, dir);
}

/// A mannequin in powder snow freezes, and the clients are told (shared index 7) so they draw it
/// shaking once fully frozen.
#[test]
fn mannequin_freezing_is_shown_to_the_clients() {
    let Some((mut p, dir)) = probe("deco-mannequin-frozen") else { return };
    player_aside(&mut p);
    cmd(&mut p, "setblock 104 -59 104 minecraft:powder_snow");
    cmd(&mut p, "setblock 104 -58 104 minecraft:powder_snow");
    cmd(&mut p, "summon minecraft:mannequin 104.5 -59 104.5");
    let (id, _) = at_x(&p, DKind::Mannequin, 104.5).expect("mannequin");
    let _ = p.take_events();
    ticks(&mut p, 10);
    let frozen = nbtio::int(&at_x(&p, DKind::Mannequin, 104.5).expect("mannequin").1, "TicksFrozen").unwrap_or(0) as i32;
    assert!(frozen > 0, "freezing in powder snow");
    assert!(meta_of(&p, id).contains(&(7, Meta::Int(frozen))), "its freezing sent to the clients ({frozen})");
    done(p, dir);
}

/// Under the world a mannequin takes 4 points whenever its hurt cooldown allows until it dies, an
/// armor stand is killed at once (`entity_die`), instead of both vanishing.
#[test]
fn living_decorations_under_the_world_are_hurt() {
    let Some((mut p, dir)) = probe("deco-under-world") else { return };
    player_aside(&mut p);
    cmd(&mut p, "summon minecraft:mannequin 104.5 -140 104.5 {NoGravity:1b}");
    cmd(&mut p, "summon minecraft:armor_stand 108.5 -140 104.5 {NoGravity:1b}");
    let _ = game_events(&mut p, "minecraft:entity_die");
    ticks(&mut p, 1);
    let (_, e) = at_x(&p, DKind::Mannequin, 104.5).expect("the mannequin is still there");
    assert_eq!(health_of(&e), 16.0, "4 points under the world");
    assert!(at_x(&p, DKind::Stand, 108.5).is_none(), "the armor stand is killed");
    assert_eq!(game_events(&mut p, "minecraft:entity_die"), 1, "the armor stand's death event");
    ticks(&mut p, 60);
    let dying = at_x(&p, DKind::Mannequin, 104.5).map_or(0.0, |(_, e)| health_of(&e));
    assert!(dying <= 0.0, "dead within 3 s: health {dying}");
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
