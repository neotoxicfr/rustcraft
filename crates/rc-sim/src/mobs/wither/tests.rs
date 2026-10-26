//! Unit tests of the wither (F16): sheet, metadata, NBT, bar, heads, flight.

use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use super::super::mob::{Mob, MoveOp};
use super::super::species::Kind;
use super::*;

fn wither() -> Mob {
    let rng = super::super::jrand::JRand::new(42);
    let mut m = Mob::new(Kind::Wither, 7, 0x1234, [0.5, 64.0, 0.5], 0.0, rng);
    m.define_data();
    m
}

#[test]
fn sheet() {
    let i = Kind::Wither.info();
    assert_eq!(i.name, "minecraft:wither");
    assert_eq!((i.width, i.height, i.eye), (0.9, 3.5, 3.5 * 0.85));
    assert_eq!((i.health, i.follow, i.armor, i.xp, i.tracking), (300.0, 40.0, 4.0, 50, 10));
    assert_eq!(i.speed, f64::from(0.6f32));
    assert!(i.fire_immune && i.fall_immune && i.not_in_peaceful);
    assert_eq!(i.category, Category::Monster);
    assert!(Kind::Wither.undead() && !Kind::Wither.is_animal());
    assert_eq!(Kind::from_name("minecraft:wither"), Some(Kind::Wither));
    assert!(refuses_effects(Kind::Wither) && !super::super::effects::can_affect(Kind::Wither, "minecraft:speed"));
}

#[test]
fn synced_data() {
    let mut m = wither();
    for i in 16..=19 {
        assert!(matches!(m.get(i), Some(Meta::Int(0))), "index {i}");
    }
    m.wither.inv = 220;
    m.wither.targets = [5, 6, 0];
    m.surf.no_gravity = true;
    m.sync_data();
    assert!(matches!(m.get(16), Some(Meta::Int(5))));
    assert!(matches!(m.get(17), Some(Meta::Int(6))));
    assert!(matches!(m.get(19), Some(Meta::Int(220))));
    assert!(matches!(m.get(5), Some(Meta::Bool(true))));
}

#[test]
fn bar_drawn_at_construction() {
    let m = wither();
    // Version-4 UUID, IETF variant; type name with hover and insertion.
    assert_eq!((m.wither.bar >> 76) & 0xF, 4);
    assert_eq!((m.wither.bar >> 62) & 0x3, 2);
    assert_ne!(m.wither.bar, m.uuid);
    assert_eq!(m.wither.bar_progress, 1.0);
    let n = &m.wither.bar_name;
    assert_eq!(n.get("translate").and_then(Nbt::as_str), Some("entity.minecraft.wither"));
    assert!(n.get("hover_event").is_some() && n.get("insertion").is_some());
    // Same random source: same bar (draws after the entity's UUID).
    assert_eq!(wither().wither.bar, m.wither.bar);
}

#[test]
fn nbt_invul_and_attributes() {
    let mut m = wither();
    m.wither.inv = 123;
    let e = m.save();
    assert!(matches!(e.get("Invul"), Some(Nbt::Int(123))));
    let attrs = match e.get("attributes") {
        Some(Nbt::List(l)) => l.clone(),
        _ => panic!("attributes missing"),
    };
    assert!(attrs.iter().any(|a| a.get("id").and_then(Nbt::as_str) == Some("minecraft:flying_speed")));
    let mut b = wither();
    b.load(&e);
    assert_eq!(b.wither.inv, 123);
}

#[test]
fn head_positions() {
    let mut m = wither();
    m.body_yaw = 0.0;
    assert_eq!(boss::head_x(&m, 0), 0.5);
    assert_eq!(boss::head_y(&m, 0), 67.0);
    assert!((boss::head_x(&m, 1) - 1.8).abs() < 1e-6);
    assert!((boss::head_x(&m, 2) + 0.8).abs() < 1e-6);
    assert!((boss::head_y(&m, 1) - (64.0 + f64::from(2.2f32))).abs() < 1e-9);
    // Index 3 (shot of the second side head): falls back onto head 1.
    assert!((boss::head_x(&m, 3) - boss::head_x(&m, 1)).abs() < 1e-6);
    assert!((boss::head_z(&m, 3) - boss::head_z(&m, 1)).abs() < 1e-6);
}

#[test]
fn flight_without_gravity_then_return() {
    let mut m = wither();
    m.move_ctl.op = MoveOp::MoveTo;
    m.move_ctl.wanted = [10.5, 70.0, 0.5];
    m.move_ctl.speed_mod = 1.0;
    assert!(move_control(&mut m));
    assert!(m.surf.no_gravity);
    assert_eq!(m.zza, f64::from(0.6f32) as f32);
    assert!(m.yya > 0.0);
    // When stopped: gravity restored (no hovering).
    assert!(move_control(&mut m));
    assert!(!m.surf.no_gravity);
    assert_eq!((m.yya, m.zza), (0.0, 0.0));
}

#[test]
fn armor_below_half() {
    let mut m = wither();
    assert!(!powered(&m));
    m.health = 150.0;
    assert!(powered(&m));
}
