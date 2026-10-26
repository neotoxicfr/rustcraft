//! Unit tests of family F13 (info sheets, brains, ordering of behaviors).

use super::super::brainy::brain::Act;
use super::super::species::{Kind, dims};
use super::*;

#[test]
fn info_sheets_match_the_game() {
    let p = info(Kind::Piglin);
    assert_eq!((p.width, p.height, p.eye), (0.6, 1.95, 1.79));
    assert_eq!(p.health, 16.0);
    assert_eq!(p.attack, 5.0);
    assert!(!p.not_in_peaceful, "the piglin stays in peaceful");
    let b = info(Kind::PiglinBrute);
    assert_eq!((b.health, b.attack, b.follow, b.xp), (50.0, 7.0, 12.0, 20));
    let z = info(Kind::ZombifiedPiglin);
    assert!(z.fire_immune && !z.burns);
    assert_eq!((z.follow, z.armor, z.attack), (35.0, 2.0, 5.0));
    let h = info(Kind::Hoglin);
    assert_eq!((h.width, h.height), (1.396_484_4, 1.4));
    assert_eq!(h.ambient_interval, 120);
    assert!(info(Kind::Zoglin).fire_immune);
    assert_eq!(dims(Kind::Piglin, true), (0.49, 0.98, 0.78));
    assert_eq!(dims(Kind::Hoglin, true), (0.75, 0.85, 0.625));
}

#[test]
fn species_names_resolve() {
    for (n, k) in [
        ("minecraft:piglin", Kind::Piglin),
        ("minecraft:piglin_brute", Kind::PiglinBrute),
        ("minecraft:zombified_piglin", Kind::ZombifiedPiglin),
        ("minecraft:hoglin", Kind::Hoglin),
        ("minecraft:zoglin", Kind::Zoglin),
    ] {
        assert_eq!(Kind::from_name(n), Some(k));
    }
    assert!(Kind::ZombifiedPiglin.zombie_like() && Kind::ZombifiedPiglin.undead());
    assert!(!Kind::Zoglin.zombie_like() && Kind::Zoglin.undead());
    assert!(Kind::Hoglin.is_animal() && Kind::Hoglin.is_monster() && Kind::Hoglin.ageable());
}

#[test]
fn piglin_brain_orders_core_then_activities() {
    let mut rng = test_rng();
    let b = brain::piglin(&mut rng);
    // Core: 9 behaviors at priorities 0 to 8.
    let core: Vec<i32> = b.nodes.iter().filter(|n| n.act == Act::Core).map(|n| n.prio).collect();
    assert_eq!(core, (0..9).collect::<Vec<_>>());
    // Idle: 8 behaviors from 10.
    assert_eq!(b.nodes.iter().filter(|n| n.act == Act::Idle).count(), 8);
    // Sensors: initial delays drawn from the entity random (0 to 19).
    assert_eq!(b.sensors.len(), 5);
    assert!(b.sensors.iter().all(|s| (0..20).contains(&s.1)));
    assert_eq!(b.act, Act::Idle);
}

#[test]
fn brains_of_the_family() {
    let mut rng = test_rng();
    assert_eq!(brain::brute(&mut rng).sensors.len(), 5);
    assert_eq!(brain::hoglin(&mut rng).sensors.len(), 4);
    assert_eq!(brain::zoglin(&mut rng).sensors.len(), 2);
}

#[test]
fn hoglin_age_boundary() {
    let mut m = Mob::new(Kind::Hoglin, 1, 1, [0.0; 3], 0.0, test_rng());
    on_new(&mut m);
    m.age = -24000;
    age_boundary(&mut m);
    assert_eq!((m.attrs.attack.base, m.pig.xp_reward), (0.5, 3));
    m.age = 0;
    age_boundary(&mut m);
    assert_eq!((m.attrs.attack.base, m.pig.xp_reward), (6.0, 5));
    assert_eq!(m.attrs.knockback_resistance.base, f64::from(0.6f32));
}

#[test]
fn piglin_baby_speed_modifier() {
    let mut m = Mob::new(Kind::Piglin, 1, 1, [0.0; 3], 0.0, test_rng());
    on_new(&mut m);
    let base = m.movement_speed();
    set_piglin_baby(&mut m, true);
    assert!((m.movement_speed() - base * 1.2).abs() < 1e-6);
    set_piglin_baby(&mut m, false);
    assert!((m.movement_speed() - base).abs() < 1e-9);
    assert!(m.can_pickup, "piglins pick up by default");
}

#[test]
fn piglin_metadata_and_nbt_round_trip() {
    let mut m = Mob::new(Kind::Piglin, 1, 1, [0.0; 3], 0.0, test_rng());
    on_new(&mut m);
    m.define_data();
    m.pig.immune = true;
    m.pig.dancing = true;
    set_piglin_baby(&mut m, true);
    m.pig.inventory.push(crate::player::Stack { id: "minecraft:gold_nugget".into(), count: 5, components: None });
    if let Some(b) = m.bm.brain.as_mut() {
        b.mem.pig.hunted_recently.set_exp(true, 1234);
        b.mem.pig.angry_at.set_exp(42, 600);
    }
    m.sync_data();
    assert_eq!(m.get(16), Some(&rc_proto_v777::entity::Meta::Bool(true)));
    assert_eq!(m.get(17), Some(&rc_proto_v777::entity::Meta::Bool(true)));
    assert_eq!(m.get(19), Some(&rc_proto_v777::entity::Meta::Bool(true)));
    let e = m.save();
    let mut n = Mob::new(Kind::Piglin, 2, 2, [0.0; 3], 0.0, test_rng());
    on_new(&mut n);
    n.load(&e);
    assert!(n.baby && n.pig.immune);
    assert_eq!(n.pig.inventory.len(), 1);
    let b = n.bm.brain.as_ref().expect("brain");
    assert_eq!(b.mem.pig.hunted_recently.ttl, 1234);
    assert_eq!(b.mem.pig.angry_at.get(), Some(42));
}

#[test]
fn hoglin_throw_uses_radians() {
    // 10 radians, not degrees.
    let a = 10.0f32;
    let (c, s) = (super::super::mth::cos(a), super::super::mth::sin(a));
    assert!(c < -0.8 && s < -0.5);
}

#[test]
fn brute_brain_has_no_hunted_recently_slot() {
    // The brute's brain does not register the memory: read ignored, never written.
    let mut p = Mob::new(Kind::Piglin, 1, 1, [0.0; 3], 0.0, test_rng());
    on_new(&mut p);
    if let Some(b) = p.bm.brain.as_mut() {
        b.mem.pig.hunted_recently.set_exp(true, 500);
    }
    let e = p.save();
    let mut brute = Mob::new(Kind::PiglinBrute, 2, 2, [0.0; 3], 0.0, test_rng());
    on_new(&mut brute);
    brute.load(&e);
    assert!(!brute.bm.brain.as_ref().expect("brain").mem.pig.hunted_recently.has());
    if let Some(b) = brute.bm.brain.as_mut() {
        b.mem.pig.hunted_recently.set_exp(true, 500);
    }
    let saved = brute.save();
    let text = format!("{saved:?}");
    assert!(!text.contains("hunted_recently"), "the brute does not save this memory");
}

#[test]
fn golden_spear_kinetic_parameters() {
    use super::spear::{kinetic, main_kin};
    let k = kinetic("minecraft:golden_spear").expect("golden spear");
    // Delay 0.7 s, damage up to 13.75 s: 14 + 275 ticks of use.
    assert_eq!(k.use_duration(), 14 + 275);
    assert!(kinetic("minecraft:golden_sword").is_none());
    let mut m = Mob::new(Kind::Piglin, 1, 1, [0.0; 3], 0.0, test_rng());
    on_new(&mut m);
    assert!(main_kin(&m).is_none() && !super::spear::holds_kinetic(&m));
    m.equipment[1] = Some(crate::player::Stack { id: "minecraft:golden_spear".into(), count: 1, components: None });
    assert!(main_kin(&m).is_none() && super::spear::holds_kinetic(&m), "off-hand: non-melee only");
}

#[test]
fn known_speed_is_the_last_tick_displacement() {
    let mut m = Mob::new(Kind::Piglin, 1, 1, [0.0; 3], 0.0, test_rng());
    super::spear::compute_speed(&mut m);
    assert_eq!(m.pig.known_speed, [0.0; 3]);
    m.pos = [0.5, 0.0, -0.25];
    super::spear::compute_speed(&mut m);
    assert_eq!(m.pig.known_speed, [0.5, 0.0, -0.25]);
}
