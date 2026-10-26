//! Projectile advancements on a new flat world: `target_hit` with a condition on the
//! projectile (Bullseye), `killed_by_arrow` accumulated by a piercing arrow (Arbalistic)
//! and victim conditions consumed one by one (Two Birds, One Arrow).

use std::path::PathBuf;
use std::sync::Arc;

use super::physics::{EntityHit, Target};
use super::{PKind, Proj};
use crate::blocks::probe::Probe;
use crate::mobs::mob::Ent;
use crate::mobs::species::Kind;
use crate::newworld::{NewWorld, Preset, create_world};
use crate::player::Stack;

fn open(tag: &str) -> (Probe, PathBuf) {
    let data = Arc::new(rc_testdata::version_data().expect("26.3 version data absent"));
    assert!(crate::registries::client_jar(&data).is_some(), "jar client 26.3 absent");
    let dir = std::env::temp_dir().join(format!("rc-projprog-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut w = NewWorld::new(tag);
    w.seed = 1;
    w.preset = Preset::Flat;
    create_world(&dir, &data, &w).expect("world creation");
    let mut p = Probe::open(&dir, data);
    p.s.pos = [0.5, -60.0, 0.5];
    (p, dir)
}

fn adv(p: &Probe, id: &str) -> usize {
    *p.s.prog.tree.index.get(id).unwrap_or_else(|| panic!("advancement {id} absent from the tree"))
}

fn stack(id: &str) -> Stack {
    Stack { id: id.into(), count: 1, components: None }
}

fn mob(p: &mut Probe, k: Kind, pos: [f64; 3], health: f32) -> i32 {
    let mut m = p.s.new_mob(k, pos, 0.0);
    m.persistent = true;
    m.health = health;
    let id = m.id;
    p.s.add_mob(m);
    id
}

fn finish(p: Probe, dir: PathBuf) {
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Bullseye: signal 15 AND projectile at least 30 blocks (horizontally) from the player.
#[test]
fn bullseye_reads_projectile_condition() {
    let (mut p, dir) = open("bull");
    let a = adv(&p, "minecraft:adventure/bullseye");
    let near = Proj::new(PKind::Arrow, 900_001, [29.4, -40.0, 0.5], stack("minecraft:arrow"));
    p.s.s8_target_hit(15, &near);
    assert!(!p.s.prog.done(a), "projectile at 28.9 blocks: no advancement");
    let far = Proj::new(PKind::Arrow, 900_002, [30.6, -60.0, 0.5], stack("minecraft:arrow"));
    p.s.s8_target_hit(14, &far);
    assert!(!p.s.prog.done(a), "signal 14: no advancement");
    p.s.s8_target_hit(15, &far);
    assert!(p.s.prog.done(a), "signal 15 at 30.1 blocks: advancement obtained");
    finish(p, dir);
}

/// Two Birds, One Arrow: each "phantom" condition consumes a distinct victim.
#[test]
fn two_birds_consumes_victims() {
    let (mut p, dir) = open("birds");
    let a = adv(&p, "minecraft:adventure/two_birds_one_arrow");
    let ph1 = mob(&mut p, Kind::Phantom, [3.5, -50.0, 3.5], 20.0);
    let ph2 = mob(&mut p, Kind::Phantom, [4.5, -50.0, 3.5], 20.0);
    let z = mob(&mut p, Kind::Zombie, [5.5, -60.0, 3.5], 20.0);
    let cb = stack("minecraft:crossbow");
    p.s.s8_killed_by_arrow(&[Ent::Mob(ph1)], Some(&cb));
    assert!(!p.s.prog.done(a), "a single phantom");
    p.s.s8_killed_by_arrow(&[Ent::Mob(ph1), Ent::Mob(z)], Some(&cb));
    assert!(!p.s.prog.done(a), "phantom + zombie");
    p.s.s8_killed_by_arrow(&[Ent::Mob(ph1), Ent::Mob(ph2)], Some(&stack("minecraft:bow")));
    assert!(!p.s.prog.done(a), "bow instead of crossbow");
    p.s.s8_killed_by_arrow(&[Ent::Mob(ph1), Ent::Mob(ph2)], Some(&cb));
    assert!(p.s.prog.done(a), "two phantoms with the crossbow");
    finish(p, dir);
}

/// Arbalistic: a piercing arrow (level 4) kills five different types; the list
/// of those killed is accumulated on the arrow and passed on at each hit.
#[test]
fn arbalistic_accumulates_pierced_kills() {
    let (mut p, dir) = open("arba");
    let a = adv(&p, "minecraft:adventure/arbalistic");
    let kinds = [Kind::Zombie, Kind::Skeleton, Kind::Creeper, Kind::Spider, Kind::Pig];
    let ids: Vec<i32> =
        kinds.iter().enumerate().map(|(i, &k)| mob(&mut p, k, [3.5 + i as f64 * 2.0, -60.0, 3.5], 0.5)).collect();
    let mut arrow = Proj::new(PKind::Arrow, 900_003, [1.5, -59.0, 3.5], stack("minecraft:arrow"));
    arrow.owner = Some(p.s.uuid);
    arrow.weapon = Some(stack("minecraft:crossbow"));
    arrow.pierce = 4;
    arrow.damage = 2.0;
    arrow.vel = [3.0, 0.0, 0.0];
    for (n, &id) in ids.iter().enumerate() {
        assert!(!p.s.prog.done(a), "advancement obtained before the fifth victim ({n})");
        let pos = p.s.mob_ref(id).expect("mob").pos;
        p.s.proj_hit_entity(&mut arrow, EntityHit { ent: Target::Living(Ent::Mob(id)), loc: pos });
        assert!(!p.s.ent_alive(Ent::Mob(id)), "victim {n} killed");
    }
    assert_eq!(arrow.pierce_killed.as_ref().map(Vec::len), Some(5), "five victims accumulated");
    assert!(p.s.prog.done(a), "five types killed by the same arrow");
    finish(p, dir);
}
