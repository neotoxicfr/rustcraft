//! Golem unit tests (no world): cracks, patterns, oxidation, sounds.

use super::super::jrand::JRand;
use super::super::mob::Mob;
use super::super::species::Kind;

fn golem(kind: Kind) -> Mob {
    let mut m = Mob::new(kind, 1, 1, [0.5, 0.0, 0.5], 0.0, JRand::new(42));
    m.define_data();
    m
}

#[test]
fn iron_golem_cracks_by_health_fraction() {
    let mut m = golem(Kind::IronGolem);
    assert_eq!(m.max_health(), 100.0);
    let cases = [(100.0, 0), (75.0, 0), (74.9, 1), (50.0, 1), (49.0, 2), (25.0, 2), (24.0, 3), (1.0, 3)];
    for (h, c) in cases {
        m.health = h;
        assert_eq!(super::iron::crackiness(&m), c, "HP {h}");
    }
}

#[test]
fn golem_attributes_and_dimensions() {
    let iron = golem(Kind::IronGolem);
    assert_eq!(iron.attrs.knockback_resistance.value(), 1.0);
    assert_eq!(iron.attrs.attack.value(), 15.0);
    assert_eq!(iron.step_height(), 1.0);
    assert!((iron.eye_height() - f64::from(2.7f32 * 0.85)).abs() < 1.0e-6);
    let snow = golem(Kind::SnowGolem);
    assert_eq!(snow.max_health(), 4.0);
    assert!(snow.golem.pumpkin);
    assert_eq!(snow.width(), 0.7);
    let copper = golem(Kind::CopperGolem);
    assert_eq!(copper.max_health(), 12.0);
    assert!(copper.persistent);
    assert!(copper.can_breathe_underwater());
    // Transport cooldown drawn at construction: 60 to 99.
    let cd = copper.golem.brain.transport_cd.unwrap();
    assert!((60..100).contains(&cd));
}

#[test]
fn metadata_defaults() {
    use rc_proto_v777::entity::Meta;
    let snow = golem(Kind::SnowGolem);
    assert_eq!(snow.get(16), Some(&Meta::Byte(16)));
    let copper = golem(Kind::CopperGolem);
    assert_eq!(copper.get(16), Some(&Meta::Variant(0, 38)));
    assert_eq!(copper.get(17), Some(&Meta::Variant(0, 37)));
    let mut iron = golem(Kind::IronGolem);
    iron.golem.player_created = true;
    iron.sync_data();
    assert_eq!(iron.get(16), Some(&Meta::Byte(1)));
}

#[test]
fn copper_sounds_follow_oxidation() {
    assert_eq!(super::copper::sounds(0)[1], "minecraft:entity.copper_golem.hurt");
    assert_eq!(super::copper::sounds(1)[2], "minecraft:entity.copper_golem.death");
    assert_eq!(super::copper::sounds(2)[3], "minecraft:entity.copper_golem_weathered.step");
    assert_eq!(super::copper::sounds(3)[0], "minecraft:entity.copper_golem_oxidized.spin");
}

#[test]
fn copper_age_of_blocks() {
    use super::build::copper_age;
    assert_eq!(copper_age("minecraft:copper_block"), 0);
    assert_eq!(copper_age("minecraft:waxed_exposed_copper"), 1);
    assert_eq!(copper_age("minecraft:weathered_copper"), 2);
    assert_eq!(copper_age("minecraft:waxed_oxidized_copper"), 3);
}

#[test]
fn nbt_round_trip() {
    let mut iron = golem(Kind::IronGolem);
    iron.golem.player_created = true;
    iron.anger_end = 1234;
    let e = iron.save();
    let mut back = golem(Kind::IronGolem);
    back.load(&e);
    assert!(back.golem.player_created);
    assert_eq!(back.anger_end, 1234);
    let mut snow = golem(Kind::SnowGolem);
    snow.golem.pumpkin = false;
    let mut back = golem(Kind::SnowGolem);
    back.load(&snow.save());
    assert!(!back.golem.pumpkin);
    let mut cu = golem(Kind::CopperGolem);
    cu.golem.weather = 2;
    cu.golem.next_weather = -2;
    cu.golem.brain.visited = vec![[1, 2, 3]];
    cu.golem.brain.visited_ttl = 500;
    let mut back = golem(Kind::CopperGolem);
    back.load(&cu.save());
    assert_eq!(back.golem.weather, 2);
    assert_eq!(back.golem.next_weather, -2);
    assert_eq!(back.golem.brain.visited, vec![[1, 2, 3]]);
    assert_eq!(back.golem.brain.visited_ttl, 500);
    // Quirk 26.3: the constructor's cooldown is lost on reload.
    assert_eq!(back.golem.brain.transport_cd, None);
}

#[test]
fn closest_section_takes_first_minimum() {
    use super::iron::closest_section;
    // Village to the east: decreasing distance in x; two minimums (x+2, y−2 and y+2) → the first.
    let d = |s: [i32; 3]| 10 - s[0] - i32::from(s[1].abs() == 2);
    assert_eq!(closest_section([0, 0, 0], 2, d), [2, -2, -2]);
    // No closer section: the center.
    assert_eq!(closest_section([0, 0, 0], 2, |_| 6), [0, 0, 0]);
}

/// Stopping the copper golem in place cuts the walk and the horizontal speed but leaves the
/// movement control's multiplier (a "go to" armed this tick still advances).
#[test]
fn copper_stop_in_place_keeps_move_speed_modifier() {
    use super::super::mob::MoveOp;
    let mut m = golem(Kind::CopperGolem);
    m.move_ctl.op = MoveOp::MoveTo;
    m.move_ctl.speed_mod = 1.0;
    m.speed = 0.2;
    m.zza = 0.2;
    m.xxa = 0.1;
    m.vel = [0.3, -0.08, 0.4];
    crate::server::Server::stop_in_place(&mut m);
    assert_eq!(m.move_ctl.speed_mod, 1.0);
    assert!(matches!(m.move_ctl.op, MoveOp::MoveTo));
    assert_eq!((m.speed, m.zza, m.xxa), (0.0, 0.0, 0.0));
    assert_eq!(m.vel, [0.0, -0.08, 0.0]);
}
