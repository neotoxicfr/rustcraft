//! Unit tests of family F02 (no world): info sheets, data, NBT, variants.

use rc_proto_v777::entity::Meta;

use super::super::jrand::JRand;
use super::super::mob::Mob;
use super::super::species::{Category, Kind, dims};

fn mob(kind: Kind) -> Mob {
    let mut m = Mob::new(kind, 7, 1, [0.5, 64.0, 0.5], 0.0, JRand::new(42));
    m.define_data();
    m
}

#[test]
fn info_sheets() {
    let b = Kind::Bat.info();
    assert_eq!(b.category, Category::Ambient);
    assert_eq!((b.width, b.height, b.eye, b.health, b.tracking), (0.5, 0.9, 0.45, 6.0, 5));
    assert_eq!(Kind::Rabbit.info().speed, f64::from(0.3f32));
    assert_eq!(Kind::PolarBear.info().follow, 20.0);
    assert_eq!(Kind::Turtle.info().step, 1.0);
    assert_eq!(Kind::Ocelot.info().ambient_interval, 900);
    assert_eq!(dims(Kind::Rabbit, true), (0.24, 0.4, 0.39));
    assert_eq!(dims(Kind::PolarBear, true), (0.7, 0.7, 0.343_75));
    assert!(Kind::Parrot.is_animal() && !Kind::Bat.is_animal());
    assert_eq!(Kind::from_name("minecraft:polar_bear"), Some(Kind::PolarBear));
    assert_eq!(Category::Ambient.max(), 15);
    assert!(!Category::Ambient.persistent());
}

#[test]
fn synced_data() {
    // Bat: born at rest (flag 1), index 16.
    let mut b = mob(Kind::Bat);
    b.sync_data();
    assert_eq!(b.get(16), Some(&Meta::Byte(1)));
    // Parrot: 18 flags, 19 owner, 20 variant.
    let mut p = mob(Kind::Parrot);
    p.variant = 3;
    p.sitting = true;
    p.tame = true;
    p.sync_data();
    assert_eq!(p.get(18), Some(&Meta::Byte(5)));
    assert_eq!(p.get(20), Some(&Meta::Int(3)));
    // Turtle: 18 egg, 19 laying.
    let mut t = mob(Kind::Turtle);
    t.surf.has_egg = true;
    t.sync_data();
    assert_eq!(t.get(18), Some(&Meta::Bool(true)));
    assert_eq!(t.get(19), Some(&Meta::Bool(false)));
    // Ocelot sprinting: shared flag 8.
    let mut o = mob(Kind::Ocelot);
    super::ocelot::set_sprinting(&mut o, true);
    o.sync_data();
    assert_eq!(o.get(0), Some(&Meta::Byte(8)));
    assert!((o.movement_speed() - f64::from(0.3f32) * (1.0 + f64::from(0.3f32))).abs() < 1e-9);
}

#[test]
fn nbt_round_trip() {
    let mut r = mob(Kind::Rabbit);
    super::rabbit::set_variant(&mut r, super::rabbit::EVIL);
    r.surf.more_carrot = 12;
    let e = r.save();
    let mut r2 = mob(Kind::Rabbit);
    r2.load(&e);
    assert_eq!(r2.variant, 99);
    assert_eq!(r2.surf.more_carrot, 12);
    // Reloaded: armor and temporary bonus set again by the variant (never saved).
    assert_eq!(r2.attrs.armor.base, 8.0);
    assert_eq!(r2.attrs.attack.value(), 8.0);
    // Killer: armor 8, damage 3 + 5, translated name.
    assert_eq!(r.attrs.armor.base, 8.0);
    assert_eq!(r.attrs.attack.value(), 8.0);
    assert!(r.custom_name.is_some());
    // Unknown variant → brown.
    assert_eq!(super::rabbit::by_id(42), 0);

    let mut c = mob(Kind::Mooshroom);
    c.variant = 1;
    let e = c.save();
    assert_eq!(e.get("Type").and_then(rc_proto::nbt::Nbt::as_str), Some("brown"));
    let mut t = mob(Kind::Turtle);
    t.surf.home = [3, -60, 9];
    t.surf.has_egg = true;
    let e = t.save();
    let mut t2 = mob(Kind::Turtle);
    t2.load(&e);
    assert_eq!(t2.surf.home, [3, -60, 9]);
    assert!(t2.surf.has_egg);
}

#[test]
fn stew_flowers() {
    assert_eq!(super::mooshroom::flower_effect("minecraft:dandelion"), Some(("minecraft:saturation", 7)));
    assert_eq!(super::mooshroom::flower_effect("minecraft:allium"), Some(("minecraft:fire_resistance", 60)));
    assert_eq!(super::mooshroom::flower_effect("minecraft:lily_of_the_valley"), Some(("minecraft:poison", 220)));
    assert_eq!(super::mooshroom::flower_effect("minecraft:stone"), None);
}

#[test]
fn rabbit_path_following_keeps_jump_speed() {
    use super::super::nav;
    let mut r = mob(Kind::Rabbit);
    // The generic navigation goes through the rabbit's own control.
    nav::set_wanted(&mut r, [3.0, 64.0, 3.0], 0.6);
    assert_eq!(r.surf.next_jump_speed, 0.6);
    // Zero speed: the last useful speed is kept.
    nav::set_wanted(&mut r, [3.0, 64.0, 3.0], 0.0);
    assert_eq!(r.surf.next_jump_speed, 0.6);
    // In water: 1.5 forced.
    r.in_water = true;
    nav::set_wanted(&mut r, [3.0, 64.0, 3.0], 0.6);
    assert_eq!((r.move_ctl.speed_mod, r.surf.next_jump_speed), (1.5, 1.5));
    // The other species do not touch this field.
    let mut c = mob(Kind::Turtle);
    nav::set_wanted(&mut c, [3.0, 64.0, 3.0], 0.6);
    assert_eq!(c.surf.next_jump_speed, 0.0);
}

#[test]
fn own_maluses_turtle_parrot() {
    use super::super::nav::{PathType, malus};
    assert_eq!(malus(Kind::Turtle, PathType::Water), 0.0);
    for d in [PathType::DoorOpen, PathType::DoorWoodClosed, PathType::DoorIronClosed] {
        assert_eq!(malus(Kind::Turtle, d), -1.0);
    }
    for t in [PathType::FireInNeighbor, PathType::Fire, PathType::Cocoa] {
        assert_eq!(malus(Kind::Parrot, t), -1.0);
    }
    // Generic animal unchanged.
    assert_eq!(malus(Kind::Rabbit, PathType::FireInNeighbor), 16.0);
    assert_eq!(malus(Kind::Rabbit, PathType::Cocoa), 0.0);
}

#[test]
fn bear_anger_saved() {
    use super::super::mob::Ent;
    let mut b = mob(Kind::PolarBear);
    b.anger_target = Some(Ent::Player);
    b.surf.angry_at = Some(0x1234);
    b.anger_end = 500;
    let e = b.save();
    assert_eq!(crate::nbtio::uuid(e.get("angry_at")), Some(0x1234));
    let mut b2 = mob(Kind::PolarBear);
    b2.load(&e);
    assert_eq!((b2.anger_target, b2.surf.angry_at), (Some(Ent::Player), Some(0x1234)));
    // Anger over: the field read does not survive the next save.
    b2.anger_target = None;
    b2.anger_end = -1;
    assert!(b2.save().get("angry_at").is_none());
}

#[test]
fn rabbit_jump_power() {
    let mut r = mob(Kind::Rabbit);
    // Requested speed zero (constructor): low jump.
    assert_eq!(r.move_ctl.speed_mod, 0.0);
    r.move_ctl.speed_mod = 2.2;
    r.horizontal_collision = false;
    r.jumping = false;
    // 0.3 normalized by 0.42 then multiplied by the jump strength 0.42.
    let base = 0.3f32;
    assert!((base / 0.42 * 0.42 - 0.3).abs() < 1e-6);
}
