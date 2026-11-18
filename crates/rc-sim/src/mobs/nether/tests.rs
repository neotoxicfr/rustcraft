//! Unit tests of the F12 family (sheets, data, boost, fireballs, ghast flight).

use super::super::mob::Mob;
use super::super::species::Kind;
use super::*;

fn mob(kind: Kind) -> Mob {
    let rng = super::super::jrand::JRand::new(42);
    let mut m = Mob::new(kind, 1, 1, [0.5, 64.0, 0.5], 0.0, rng);
    m.define_data();
    m
}

#[test]
fn game_sheets() {
    let b = Kind::Blaze.info();
    assert_eq!((b.width, b.height, b.health, b.attack, b.follow, b.xp), (0.6, 1.8, 20.0, 6.0, 48.0, 10));
    assert_eq!(b.speed, f64::from(0.23f32));
    assert!(b.fire_immune && b.water_sensitive);
    let g = Kind::Ghast.info();
    assert_eq!((g.width, g.height, g.eye, g.health, g.follow, g.volume), (4.0, 4.0, 2.6, 10.0, 100.0, 5.0));
    assert!(g.fall_immune && g.fire_immune && g.not_in_peaceful);
    let w = Kind::WitherSkeleton.info();
    assert_eq!((w.width, w.height, w.eye), (0.7, 2.4, 2.1));
    assert!(Kind::WitherSkeleton.skeleton_like() && Kind::WitherSkeleton.undead());
    let s = Kind::Strider.info();
    assert_eq!((s.width, s.height, s.health), (0.9, 1.7, 20.0));
    assert_eq!(s.speed, f64::from(0.175f32));
    assert_eq!(s.category, Category::Creature);
    assert!(Kind::Strider.is_animal() && Kind::Strider.ageable());
    assert_eq!(Kind::from_name("minecraft:wither_skeleton"), Some(Kind::WitherSkeleton));
    assert_eq!(super::super::species::dims(Kind::Strider, true), (0.45, 0.85, 0.4375));
}

#[test]
fn synced_data() {
    let mut b = mob(Kind::Blaze);
    assert!(matches!(b.get(16), Some(Meta::Byte(0))));
    b.neth.charged = true;
    b.sync_data();
    assert!(matches!(b.get(16), Some(Meta::Byte(1))));
    let g = mob(Kind::Ghast);
    assert!(matches!(g.get(16), Some(Meta::Bool(false))));
    let w = mob(Kind::WitherSkeleton);
    assert!(w.get(16).is_none(), "the wither skeleton has no data of its own");
    let mut s = mob(Kind::Strider);
    assert!(matches!(s.get(18), Some(Meta::Int(0))) && matches!(s.get(19), Some(Meta::Bool(false))));
    s.neth.suffocating = true;
    s.neth.boost_total = 300;
    s.sync_data();
    assert!(matches!(s.get(18), Some(Meta::Int(300))) && matches!(s.get(19), Some(Meta::Bool(true))));
}

#[test]
fn strider_boost() {
    let mut s = mob(Kind::Strider);
    assert!(strider::boost(&mut s));
    let total = s.neth.boost_total;
    assert!((140..=980).contains(&total));
    // Second boost refused while the first lasts.
    assert!(!strider::boost(&mut s));
    let mut peak = 0.0f32;
    for _ in 0..=total + 1 {
        strider::tick_boost(&mut s);
        peak = peak.max(strider::boost_factor(&s));
    }
    assert!(!s.neth.boosting, "boost finished after its duration");
    assert!(peak > 2.0 && peak <= 2.15, "maximum factor 1 + 1.15: {peak}");
    assert_eq!(strider::boost_factor(&s), 1.0);
}

#[test]
fn maluses_and_effects() {
    use super::super::nav::{PathType, malus as nav_malus};
    assert_eq!(nav_malus(Kind::Blaze, PathType::Water), -1.0);
    assert_eq!(nav_malus(Kind::Blaze, PathType::Lava), 8.0);
    assert_eq!(nav_malus(Kind::Blaze, PathType::Fire), 0.0);
    assert_eq!(nav_malus(Kind::WitherSkeleton, PathType::Lava), 8.0);
    assert_eq!(nav_malus(Kind::Strider, PathType::Lava), 0.0);
    assert_eq!(nav_malus(Kind::Strider, PathType::Water), -1.0);
    assert!(!super::super::effects::can_affect(Kind::WitherSkeleton, "minecraft:wither"));
    assert!(super::super::effects::can_affect(Kind::Blaze, "minecraft:wither"));
}

#[test]
fn ghast_view() {
    // Yaw 0, pitch 0: gaze toward +z.
    let v = ghast::view_vector(0.0, 0.0);
    assert!(v[0].abs() < 1.0e-6 && v[1].abs() < 1.0e-6 && (v[2] - 1.0).abs() < 1.0e-6);
    // Yaw 90: gaze toward −x.
    let v = ghast::view_vector(0.0, 90.0);
    assert!((v[0] + 1.0).abs() < 1.0e-3 && v[2].abs() < 1.0e-3);
}

#[test]
fn ghast_floating() {
    let mut g = mob(Kind::Ghast);
    assert!(ghast::float_can_use(&g), "without a wanted point");
    super::super::nav::set_wanted(&mut g, [0.5, 64.0, 10.5], 1.0);
    assert!(!ghast::float_can_use(&g), "point at 10 blocks");
    super::super::nav::set_wanted(&mut g, [0.5, 64.0, 0.6], 1.0);
    assert!(ghast::float_can_use(&g), "point reached");
    super::super::nav::set_wanted(&mut g, [0.5, 64.0, 70.5], 1.0);
    assert!(ghast::float_can_use(&g), "point too far");
}
