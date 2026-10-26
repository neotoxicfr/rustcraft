//! Tests of the F08 family (stat sheets, NBT, metadata, goals, trident).

use super::super::mob::Mob;
use super::super::species::{Kind, dims};
use super::*;

fn mob(kind: Kind) -> Mob {
    let rng = super::super::jrand::JRand::new(42);
    Mob::new(kind, 7, 1, [0.5, 64.0, 0.5], 0.0, rng)
}

#[test]
fn sheets_and_dimensions() {
    assert_eq!(Kind::from_name("minecraft:drowned"), Some(Kind::Drowned));
    assert_eq!(Kind::from_name("minecraft:guardian"), Some(Kind::Guardian));
    assert_eq!(Kind::from_name("minecraft:elder_guardian"), Some(Kind::ElderGuardian));
    assert!(Kind::Drowned.zombie_like() && Kind::Drowned.undead());
    assert!(!Kind::Guardian.undead());
    assert_eq!(dims(Kind::Drowned, true), (0.49, 0.98, 0.775));
    assert_eq!(dims(Kind::Guardian, false), (0.85, 0.85, 0.425));
    let i = Kind::ElderGuardian.info();
    assert_eq!((i.health, i.attack, i.xp, i.ambient_interval), (80.0, 8.0, 10, 160));
    assert_eq!(i.speed, f64::from(0.3f32));
    let d = Kind::Drowned.info();
    assert_eq!((d.step, d.attack, d.armor, d.follow), (1.0, 3.0, 2.0, 35.0));
}

#[test]
fn construction_navigation_and_persistence() {
    let d = mob(Kind::Drowned);
    assert!(d.nav.amphibious && !d.nav.swim);
    let g = mob(Kind::Guardian);
    assert!(g.nav.swim && !g.persistent);
    let e = mob(Kind::ElderGuardian);
    assert!(e.nav.swim && e.persistent);
    assert!(g.can_breathe_underwater() && d.can_breathe_underwater());
    // Zombie: undead, does not drown (it converts into a drowned).
    assert!(mob(Kind::Zombie).can_breathe_underwater());
}

#[test]
fn guardian_and_drowned_metadata() {
    let mut g = mob(Kind::Guardian);
    g.define_data();
    g.aqua.moving = true;
    g.aqua.beam = Some(super::super::mob::Ent::Player);
    g.aqua.beam_id = 1;
    g.sync_data();
    assert_eq!(g.get(16), Some(&Meta::Bool(true)));
    assert_eq!(g.get(17), Some(&Meta::Int(1)));
    let mut d = mob(Kind::Drowned);
    d.define_data();
    d.aqua.swimming = true;
    d.use_ticks = 0;
    d.sync_data();
    assert_eq!(d.get(0), Some(&Meta::Byte(0x10)));
    assert_eq!(d.get(8), Some(&Meta::Byte(1)));
    // Zombie indices: baby (16), type (17), conversion (18).
    assert_eq!(d.get(16), Some(&Meta::Bool(false)));
    assert_eq!(d.get(18), Some(&Meta::Bool(false)));
}

#[test]
fn elder_guardian_residence_nbt() {
    let mut e = mob(Kind::ElderGuardian);
    e.aqua.home = Some(([3, -40, 9], 16));
    let mut n = Nbt::Compound(Vec::new());
    save(&e, &mut n);
    let mut back = mob(Kind::ElderGuardian);
    load(&mut back, &n);
    assert_eq!(back.aqua.home, Some(([3, -40, 9], 16)));
}

#[test]
fn goal_order() {
    let mut d = mob(Kind::Drowned);
    d.goals = Default::default();
    d.targets = Default::default();
    register_goals(&mut d);
    let prios: Vec<i32> = d.goals.goals.iter().map(|w| w.prio).collect();
    assert_eq!(prios, vec![4, 8, 8, 1, 2, 2, 5, 6, 7]);
    let tprios: Vec<i32> = d.targets.goals.iter().map(|w| w.prio).collect();
    assert_eq!(tprios, vec![1, 2, 3, 3, 3, 5]);
    let mut g = mob(Kind::ElderGuardian);
    g.goals = Default::default();
    g.targets = Default::default();
    register_goals(&mut g);
    let prios: Vec<i32> = g.goals.goals.iter().map(|w| w.prio).collect();
    assert_eq!(prios, vec![4, 5, 7, 8, 8, 9]);
    assert!(matches!(g.goals.goals[2].goal, Goal::Aqua(AquaGoal::Stroll { interval: 400, .. })));
    assert_eq!(g.targets.goals.len(), 1);
    // Flags: beam and wandering take movement + look; beam on every tick.
    assert_eq!(g.goals.goals[0].goal.flags(), 3);
    assert!(g.goals.goals[0].goal.every_tick());
    assert_eq!(g.goals.goals[2].goal.flags(), 3);
}

#[test]
fn aimed_trident_silent_in_peaceful_and_speed() {
    let stack = crate::player::Stack { id: "minecraft:trident".into(), count: 1, components: None };
    let mut p = crate::projectiles::Proj::new(crate::projectiles::PKind::Trident, 1, [0.0; 3], stack);
    p.shoot([3.0, 0.0, 4.0], 1.6, 0.0);
    let v = p.vel;
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    assert!((l - f64::from(1.6f32)).abs() < 1e-9, "{l}");
    assert!((v[0] / v[2] - 0.75).abs() < 1e-12);
}
