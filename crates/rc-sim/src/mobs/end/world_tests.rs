//! Family F15 end to end on a copy of the test world (`RUSTCRAFT_SOLO_WORLD`, flat map):
//! shulker that opens and shoots (levitation), summoned dragon hit on a part, crystal that
//! explodes, End fight (bar, dragon, podium) then victory by `kill` (active portal, egg,
//! gateway), fight save.

use std::time::Duration;

use rc_client::{EntityEvent, Event};
use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;
use rc_proto_v777::play::BossOp;

use super::super::tests::{drain_until, kind_id, prep};
use crate::tests::{start_test, test_world};

/// Copy of the world, commands allowed.
fn end_prep(world: &std::path::Path, tag: &str, mode: i32, items: &[(&str, i32)]) -> std::path::PathBuf {
    let dir = prep(world, tag, mode, "normal", items);
    let lp = dir.join("level.dat");
    let mut root = crate::nbtio::read_dat(&lp).unwrap();
    if let Some(d) = crate::nbtio::get_mut(&mut root, "Data") {
        crate::nbtio::set(d, "allowCommands", Nbt::Byte(1));
    }
    std::fs::write(&lp, crate::nbtio::gzip(&crate::nbtio::encode_root(&root))).unwrap();
    dir
}

fn cmd(s: &rc_client::Session, c: &str) {
    s.link.send_command(c).unwrap();
    std::thread::sleep(Duration::from_millis(150));
}

fn added(s: &rc_client::Session, data: &rc_data::VersionData, kind: &str, secs: u64) -> Option<(i32, [f64; 3])> {
    let k = kind_id(data, kind);
    let mut id = None;
    drain_until(s, secs, |e| {
        if let Event::Entity(EntityEvent::Add(a)) = e
            && a.kind == k
        {
            id = Some((a.id, a.pos));
            return true;
        }
        false
    });
    id
}

/// Survival: a shulker 5 blocks away opens (index 17 = 100), shoots a bullet that hits the player
/// (health drops, levitation).
#[test]
fn shulker_opens_and_shoots() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = end_prep(&world, "f15shulker", 0, &[]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    std::thread::sleep(Duration::from_millis(2500));
    cmd(
        &s,
        &format!("summon minecraft:shulker {} {} {} {{PersistenceRequired:1b}}", p.pos[0] + 5.0, p.pos[1], p.pos[2]),
    );
    let (sh, _) = added(&s, &data, "minecraft:shulker", 10).expect("shulker did not appear");
    let bullet = kind_id(&data, "minecraft:shulker_bullet");
    let lev = data
        .registry("minecraft:mob_effect")
        .unwrap()
        .entries
        .iter()
        .position(|e| e == "minecraft:levitation")
        .unwrap() as i32;
    let (mut open, mut balls, mut hurt, mut levitating) = (false, 0, false, false);
    drain_until(&s, 25, |e| {
        match e {
            Event::Entity(EntityEvent::Data { id, values }) if *id == sh => {
                open |= values.iter().any(|(i, v)| *i == 17 && matches!(v, Meta::Byte(100)));
            }
            Event::Entity(EntityEvent::Add(a)) if a.kind == bullet => balls += 1,
            Event::Health { health, .. } if *health < 20.0 => hurt = true,
            Event::Entity(EntityEvent::Effect { effect, .. }) if *effect == lev => levitating = true,
            _ => {}
        }
        open && balls > 0 && hurt && levitating
    });
    eprintln!("shulker: open {open}, {balls} bullets, hit {hurt}, levitation {levitating}");
    assert!(open, "the shulker does not open");
    assert!(balls > 0, "no bullet");
    assert!(hurt && levitating, "bullet without effect");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Survival, sword: summoned dragon (hover, phase 10), hit on its body (part 3: id + 3): it is
/// hurt (body damage: /4 + 1).
#[test]
fn dragon_body_part_hit() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = end_prep(&world, "f15dragon", 0, &[("diamond_sword", 1)]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    std::thread::sleep(Duration::from_millis(2500));
    cmd(&s, &format!("summon minecraft:ender_dragon {} {} {}", p.pos[0] + 2.0, p.pos[1], p.pos[2]));
    let (d, _) = added(&s, &data, "minecraft:ender_dragon", 10).expect("dragon did not appear");
    std::thread::sleep(Duration::from_millis(1200));
    let mut health = 200.0f32;
    let mut hurt = false;
    for _ in 0..4 {
        s.link.send_attack(d + 3).unwrap();
        drain_until(&s, 1, |e| {
            match e {
                Event::Entity(EntityEvent::Hurt { id, .. }) if *id == d => hurt = true,
                Event::Entity(EntityEvent::Data { id, values }) if *id == d => {
                    for (i, v) in values {
                        if *i == 9
                            && let Meta::Float(h) = v
                        {
                            health = *h;
                        }
                    }
                }
                _ => {}
            }
            false
        });
        if hurt && health < 200.0 {
            break;
        }
    }
    eprintln!("dragon {d}: hurt {hurt}, health {health}");
    assert!(hurt && health < 200.0, "hit on the body without effect");
    assert!(health > 190.0, "body damage not reduced ({health})");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Creative: a summoned crystal explodes when hit.
#[test]
fn crystal_explodes() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = end_prep(&world, "f15crystal", 1, &[]);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    std::thread::sleep(Duration::from_millis(2500));
    cmd(&s, &format!("summon minecraft:end_crystal {} {} {}", p.pos[0] + 2.0, p.pos[1], p.pos[2]));
    let (c, _) = added(&s, &data, "minecraft:end_crystal", 10).expect("crystal did not appear");
    s.link.send_attack(c).unwrap();
    let (mut boom, mut gone) = (false, false);
    drain_until(&s, 5, |e| {
        match e {
            Event::Explode(_) => boom = true,
            Event::Entity(EntityEvent::Remove(l)) if l.contains(&c) => gone = true,
            _ => {}
        }
        boom && gone
    });
    assert!(boom && gone, "the crystal does not explode ({boom}, {gone})");
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Creative: arrival in the End (pink boss bar, dragon created, pillar crystals), then `kill` of
/// the dragon: bar hidden, active exit portal (block entity), egg, gateway (event 3000); the
/// fight state is saved.
#[test]
fn end_fight_victory() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = end_prep(&world, "f15fight", 1, &[]);
    let data = std::sync::Arc::new(data);
    let (s, handle, _) = start_test(&dir, data.clone());
    std::thread::sleep(Duration::from_millis(2500));
    cmd(&s, "execute in minecraft:the_end run tp @s 0 90 40");
    let dragon_kind = kind_id(&data, "minecraft:ender_dragon");
    let crystal_kind = kind_id(&data, "minecraft:end_crystal");
    let (mut bar, mut dragon, mut crystals) = (false, None, 0);
    drain_until(&s, 60, |e| {
        match e {
            Event::Boss(b) if matches!(b.op, BossOp::Add { color: 0, flags: 6, .. }) => bar = true,
            Event::Entity(EntityEvent::Add(a)) if a.kind == dragon_kind => dragon = Some(a.id),
            Event::Entity(EntityEvent::Add(a)) if a.kind == crystal_kind => crystals += 1,
            _ => {}
        }
        bar && dragon.is_some() && crystals >= 1
    });
    eprintln!("End: bar {bar}, dragon {dragon:?}, {crystals} crystals");
    assert!(bar, "no boss bar");
    assert!(dragon.is_some(), "dragon missing");
    cmd(&s, "kill @e[type=minecraft:ender_dragon]");
    let egg_state = data.blocks.iter().find(|b| b.name == "minecraft:dragon_egg").map_or(0, |b| b.default_state);
    let (mut hidden, mut gateway, mut egg) = (false, false, false);
    drain_until(&s, 10, |e| {
        match e {
            Event::Boss(b) if matches!(b.op, BossOp::Remove) => hidden = true,
            Event::LevelEvent { kind: 3000, .. } => gateway = true,
            Event::BlockUpdate { state, .. } if *state == egg_state => egg = true,
            Event::SectionUpdate { changes, .. } if changes.iter().any(|c| c.1 == egg_state) => egg = true,
            _ => {}
        }
        hidden && gateway && egg
    });
    eprintln!("victory: bar hidden {hidden}, gateway {gateway}, egg {egg}");
    assert!(hidden && gateway, "incomplete victory");
    s.link.disconnect();
    handle.wait();
    let fight = dir.join("dimensions/minecraft/the_end/data/minecraft/ender_dragon_fight.dat");
    let root = crate::nbtio::read_dat(&fight).expect("fight not saved");
    let d = root.get("data").expect("data");
    assert_eq!(crate::nbtio::int(d, "dragon_killed"), Some(1));
    assert_eq!(crate::nbtio::int(d, "previously_killed"), Some(1));
    assert!(egg, "no egg");
    let _ = std::fs::remove_dir_all(&dir);
}
