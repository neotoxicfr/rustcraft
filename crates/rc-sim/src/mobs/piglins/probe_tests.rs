//! Piglins and spear on a copy of the test world (`RUSTCRAFT_SOLO_WORLD`, zone 200..231):
//! mount speed and extended retreats of the mounted spear carrier, nemesis (wither
//! included), blocking item kept in the off-hand.

use super::super::brainy::brain::{Brain, Visible};
use super::super::mob::{Ent, Mob};
use super::super::species::Kind;
use super::brain::{PB, PSn};
use super::spear::SpearGoal;
use crate::blocks::probe::{Probe, state_string};
use crate::player::Stack;
use rc_proto::nbt::Nbt;

fn probe(tag: &str) -> Option<Probe> {
    let (world, data) = (crate::tests::test_world(), rc_testdata::version_data());
    let (Some(world), Some(data)) = (world.clone(), data) else {
        eprintln!("test world {world:?} or data missing: test skipped");
        return None;
    };
    let dir = super::super::tests::prep(&world, tag, 0, "normal", &[]);
    let mut p = Probe::open(&dir, std::sync::Arc::new(data));
    p.run("fill 200 -60 200 231 -60 231 minecraft:stone\nfill 200 -59 200 231 -45 231 minecraft:air\nplayer 216.5 -59 202.5 0 0\n");
    assert_eq!(state_string(&p.s, p.s.get([216, -60, 216])), "minecraft:stone", "zone 200..231 missing from the copy");
    p.s.game_mode = 0;
    p.s.loaded = true;
    Some(p)
}

fn spawn(p: &mut Probe, k: Kind, pos: [f64; 3]) -> i32 {
    let m = p.s.new_mob(k, pos, 0.0);
    let id = m.id;
    p.s.add_mob(m);
    id
}

/// `rider` mounts `vehicle` (first passenger).
fn ride(p: &mut Probe, rider: i32, vehicle: i32) {
    p.s.mobs.get_mut(rider).expect("rider").mount.vehicle = Some(vehicle);
    p.s.mobs.get_mut(vehicle).expect("mount").mount.passengers.insert(0, Ent::Mob(rider));
}

fn spear() -> Option<Stack> {
    Some(Stack { id: "minecraft:golden_spear".into(), count: 1, components: None })
}

/// Mob taken out of the list (as during its tick).
fn with_mob<R>(p: &mut Probe, id: i32, f: impl FnOnce(&mut crate::server::Server, &mut Mob) -> R) -> R {
    let i = p.s.mob_index(id).expect("mob");
    let mut m = p.s.mobs.list[i].take().expect("mob");
    let r = f(&mut p.s, &mut m);
    p.s.mobs.list[i] = Some(m);
    r
}

/// Mob and brain taken out (as during its tick).
fn with_brain<R>(p: &mut Probe, id: i32, f: impl FnOnce(&mut crate::server::Server, &mut Mob, &mut Brain) -> R) -> R {
    with_mob(p, id, |s, m| {
        let mut b = m.bm.brain.take().expect("brain");
        let r = f(s, m, &mut b);
        m.bm.brain = Some(b);
        r
    })
}

/// Multiplier of the ROOT mount (camel husk 4, zombie horse 1.4 in single
/// precision, other 1) and +2 blocks on retreats as soon as the carrier is a passenger.
#[test]
fn spear_mount_speed_and_distance() {
    let Some(mut p) = probe("f13mount") else { return };
    let husk = spawn(&mut p, Kind::Husk, [210.5, -59.0, 210.5]);
    assert_eq!(p.s.spear_mount(p.s.mob_ref(husk).unwrap()), (1.0, 0.0), "on foot");
    let camel = spawn(&mut p, Kind::CamelHusk, [210.5, -59.0, 210.5]);
    ride(&mut p, husk, camel);
    assert_eq!(p.s.spear_mount(p.s.mob_ref(husk).unwrap()), (4.0, 2.0), "camel husk");
    let zomb = spawn(&mut p, Kind::Zombie, [214.5, -59.0, 210.5]);
    let zh = spawn(&mut p, Kind::ZombieHorse, [214.5, -59.0, 210.5]);
    ride(&mut p, zomb, zh);
    assert_eq!(p.s.spear_mount(p.s.mob_ref(zomb).unwrap()), (f64::from(1.4f32), 2.0), "zombie horse");
    // Chain: the direct mount (horse) counts less than the root (camel husk).
    let rider = spawn(&mut p, Kind::Piglin, [218.5, -59.0, 210.5]);
    let horse = spawn(&mut p, Kind::Horse, [218.5, -59.0, 210.5]);
    let root = spawn(&mut p, Kind::CamelHusk, [218.5, -59.0, 210.5]);
    ride(&mut p, rider, horse);
    ride(&mut p, horse, root);
    assert_eq!(p.s.spear_mount(p.s.mob_ref(rider).unwrap()), (4.0, 2.0), "root mount");
    let pig = spawn(&mut p, Kind::Piglin, [222.5, -59.0, 210.5]);
    let hog = spawn(&mut p, Kind::Hoglin, [222.5, -59.0, 210.5]);
    ride(&mut p, pig, hog);
    assert_eq!(p.s.spear_mount(p.s.mob_ref(pig).unwrap()), (1.0, 2.0), "hoglin: speed 1, retreats +2");
}

/// Zombie goal (husk on camel husk): the approach passes the mount's speed (4) to
/// the navigation; on foot, 1.
#[test]
fn spear_goal_uses_mount_speed() {
    let Some(mut p) = probe("f13goal") else { return };
    let mut speeds = Vec::new();
    for mounted in [false, true] {
        let husk = spawn(&mut p, Kind::Husk, [216.5, -59.0, 220.5]);
        if mounted {
            let camel = spawn(&mut p, Kind::CamelHusk, [216.5, -59.0, 220.5]);
            ride(&mut p, husk, camel);
        }
        let sp = with_mob(&mut p, husk, |s, m| {
            m.equipment[0] = spear();
            m.target = Some(Ent::Player);
            // Never ticked: placed on the ground so that the navigation computes a path.
            m.on_ground = true;
            let mut g = SpearGoal::new();
            assert!(s.spear_goal_can_use(m), "spear goal unusable");
            s.spear_goal_start(&mut g, m);
            s.spear_goal_tick(&mut g, m);
            assert!(m.nav.path.is_some(), "no path to the player");
            m.nav.speed_mod
        });
        speeds.push(sp);
        p.s.mobs.get_mut(husk).expect("husk").mount.vehicle = None;
    }
    assert_eq!(speeds, vec![1.0, 4.0]);
}

/// Piglin brain (spear approach): speed multiplied by the mount (zombie horse).
#[test]
fn piglin_spear_approach_uses_mount_speed() {
    let Some(mut p) = probe("f13brain") else { return };
    let pig = spawn(&mut p, Kind::Piglin, [216.5, -59.0, 222.5]);
    let zh = spawn(&mut p, Kind::ZombieHorse, [216.5, -59.0, 222.5]);
    ride(&mut p, pig, zh);
    let sp = with_brain(&mut p, pig, |s, m, b| {
        m.equipment[0] = spear();
        b.mem.attack.set(Ent::Player);
        let mut beh = PB::SpearApproach;
        s.spear_tick(m, b, &mut beh);
        assert!(m.nav.path.is_some(), "no path to the player");
        m.nav.speed_mod
    });
    assert_eq!(sp, f64::from(1.4f32));
}

/// Nemesis: wither skeleton OR wither (the boss), for the piglin as for the brute.
#[test]
fn piglin_nemesis_includes_wither_boss() {
    let Some(mut p) = probe("f13nemesis") else { return };
    let wither = spawn(&mut p, Kind::Wither, [212.5, -59.0, 212.5]);
    for (k, sensor) in [(Kind::Piglin, PSn::Piglin), (Kind::PiglinBrute, PSn::Brute)] {
        let id = spawn(&mut p, k, [216.5, -59.0, 212.5]);
        let got = with_brain(&mut p, id, |s, m, b| {
            b.mem.living.set(vec![Ent::Mob(wither)]);
            b.mem.visible.set(Visible { list: vec![Ent::Mob(wither)], cache: vec![Some(true)] });
            s.pig_sense(m, b, sensor);
            b.mem.pig.nemesis.get()
        });
        assert_eq!(got, Some(Ent::Mob(wither)), "{k:?}: wither ignored");
    }
}

/// End of admiring: the off-hand is emptied unless the item blocks (`blocks_attacks` component
/// of the stack or default, not removed), not according to the shield's id.
#[test]
fn piglin_keeps_any_parrying_offhand_item() {
    let Some(mut p) = probe("f13offhand") else { return };
    let comp = |id: &str, key: &str| Stack {
        id: id.to_owned(),
        count: 1,
        components: Some(Nbt::Compound(vec![(key.to_owned(), Nbt::Compound(Vec::new()))])),
    };
    let cases = [
        (Stack { id: "minecraft:shield".into(), count: 1, components: None }, false),
        (comp("minecraft:stick", "minecraft:blocks_attacks"), false),
        (comp("minecraft:shield", "!minecraft:blocks_attacks"), true),
        (Stack { id: "minecraft:stick".into(), count: 1, components: None }, true),
    ];
    for (i, (stack, dropped)) in cases.into_iter().enumerate() {
        let pig = spawn(&mut p, Kind::Piglin, [204.5 + 3.0 * i as f64, -59.0, 226.5]);
        let acted = with_brain(&mut p, pig, |s, m, b| {
            m.equipment[1] = Some(stack.clone());
            let mut beh = Box::new(PB::StopHoldingItem);
            s.pig_trigger(m, b, &mut beh, 0)
        });
        assert_eq!(acted, dropped, "case {i} ({}): off-hand", stack.id);
    }
}
