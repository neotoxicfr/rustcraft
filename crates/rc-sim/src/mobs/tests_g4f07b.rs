//! G4-F07b: saddled pig steered with a carrot on a stick (saddle, mount, steering, boost, wear,
//! dismount onto a neighbouring cell); llama spit (shared projectile margin, leaving the
//! llama through the stretched and inflated box, smoothed rotation).

use rc_client::Event;
use rc_proto_v777::entity::Meta;
use rc_proto_v777::play::SoundRef;

use super::jrand::JRand;
use super::mob::Mob;
use super::mounts::SADDLE;
use super::mounts::llama::Spit;
use super::species::Kind;
use crate::blocks::probe::{Probe, state_string};
use crate::player::Stack;
use crate::server::Server;

fn probe(tag: &str) -> Option<(Probe, std::path::PathBuf)> {
    let (world, data) = (crate::tests::test_world(), rc_testdata::version_data());
    let (Some(world), Some(data)) = (world.clone(), data) else {
        eprintln!("test world {world:?} or data missing: test skipped");
        return None;
    };
    let dir = super::tests::prep(&world, tag, 0, "normal", &[]);
    let mut p = Probe::open(&dir, std::sync::Arc::new(data));
    p.run("fill 200 -60 200 231 -60 231 minecraft:stone\nfill 200 -59 200 231 -40 231 minecraft:air\nplayer 216.5 -59 202.5 0 0\n");
    assert_eq!(state_string(&p.s, p.s.get([216, -60, 216])), "minecraft:stone", "zone 200..231 missing from the copy");
    p.s.game_mode = 0;
    p.s.loaded = true;
    p.s.life.vit.health = 20.0;
    p.s.life.vit.dead = false;
    Some((p, dir))
}

fn cleanup(p: Probe, dir: std::path::PathBuf) {
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

fn bare(p: &mut Probe, k: Kind, pos: [f64; 3]) -> i32 {
    let m = p.s.new_mob(k, pos, 0.0);
    let id = m.id;
    p.s.add_mob(m);
    id
}

fn mob(p: &Probe, id: i32) -> &Mob {
    p.s.mob_ref(id).expect("mob")
}

fn tick_one(p: &mut Probe, id: i32) {
    let i = p.s.mob_index(id).expect("mob");
    let mut m = p.s.mobs.list[i].take().expect("mob");
    p.s.tick_mob(&mut m);
    p.s.mobs.list[i] = Some(m);
}

fn sounds(p: &Probe, events: &[Event]) -> Vec<String> {
    let reg = p.s.data.registry("minecraft:sound_event").expect("sound registry");
    events
        .iter()
        .filter_map(|e| match e {
            Event::Sound(s) => Some(match &s.sound {
                SoundRef::Id(i) => reg.entries.get(*i as usize).cloned().unwrap_or_default(),
                SoundRef::Named(n) => n.clone(),
            }),
            _ => None,
        })
        .collect()
}

fn stack(id: &str, count: i32) -> Option<Stack> {
    Some(Stack { id: id.into(), count, components: None })
}

/// Saddle fitted (pig sound), mount, carrot in hand: the player steers; boost of 140 to 980
/// ticks (index 18), stick worn by 7; second use during the boost: no boost, "used"
/// statistic. Without the carrot, the pig is no longer steered.
#[test]
fn saddled_pig_steered_with_carrot() {
    let Some((mut p, dir)) = probe("g4f07b-pig") else { return };
    let pig = bare(&mut p, Kind::Pig, [216.5, -59.0, 206.5]);
    let slot = 36 + p.s.held as usize;
    p.s.slots[slot] = stack("minecraft:saddle", 2);
    let _ = p.take_events();
    assert!(p.s.player_interact(pig, 0), "saddle refused");
    assert!(mob(&p, pig).equipment[SADDLE].is_some(), "saddle not fitted");
    assert_eq!(p.s.slots[slot].as_ref().map(|s| s.count), Some(1), "one saddle removed");
    let evs = p.take_events();
    assert!(sounds(&p, &evs).iter().any(|n| n == "minecraft:entity.pig.saddle"), "pig saddle sound");

    p.s.slots[slot] = stack("minecraft:carrot_on_a_stick", 1);
    assert!(p.s.player_interact(pig, 0), "mount refused");
    assert_eq!(p.s.mobs.mounts.riding.vehicle, Some(pig), "player not mounted");
    tick_one(&mut p, pig);
    assert!(Server::player_controls(mob(&p, pig)), "pig not steered with the carrot");

    p.s.on_use_item(0);
    let total = mob(&p, pig).neth.boost_total;
    assert!((140..=980).contains(&total), "no boost: {total}");
    let st = p.s.slots[slot].clone().expect("stick");
    assert_eq!(p.s.life.items.damage(&st), 7, "carrot on a stick wear");
    let mut m = p.s.mob_ref(pig).cloned().expect("pig");
    m.sync_data();
    assert_eq!(m.get(18), Some(&Meta::Int(total)), "boost duration synced");
    assert_eq!(p.s.prog.stats.get("minecraft:used", "minecraft:carrot_on_a_stick"), 0);
    p.s.on_use_item(0);
    assert_eq!(mob(&p, pig).neth.boost_total, total, "boost restarted during the boost");
    assert_eq!(p.s.prog.stats.get("minecraft:used", "minecraft:carrot_on_a_stick"), 1, "\"used\" statistic");

    p.s.slots[slot] = None;
    tick_one(&mut p, pig);
    assert!(!Server::player_controls(mob(&p, pig)), "steered without the carrot");
    cleanup(p, dir);
}

/// The stick breaks on the last use: a new fishing rod.
#[test]
fn carrot_on_a_stick_breaks() {
    let Some((mut p, dir)) = probe("g4f07b-break") else { return };
    let pig = bare(&mut p, Kind::Pig, [216.5, -59.0, 206.5]);
    let i = p.s.mob_index(pig).unwrap();
    p.s.mobs.list[i].as_mut().unwrap().equipment[SADDLE] = stack("minecraft:saddle", 1);
    let slot = 36 + p.s.held as usize;
    let mut st = Stack { id: "minecraft:carrot_on_a_stick".into(), count: 1, components: None };
    crate::items::set_component(&mut st, "minecraft:damage", rc_proto::nbt::Nbt::Int(20));
    p.s.slots[slot] = Some(st);
    assert!(p.s.player_interact(pig, 0), "mount refused");
    tick_one(&mut p, pig);
    p.s.on_use_item(0);
    let st = p.s.slots[slot].clone().expect("fishing rod");
    assert_eq!(st.id, "minecraft:fishing_rod");
    assert_eq!(p.s.life.items.damage(&st), 0);
    cleanup(p, dir);
}

/// Dismounting a pig facing south: right cell (west) at the centre, otherwise the left one;
/// a chicken follows the same rule.
#[test]
fn pig_dismounts_onto_neighbouring_cell() {
    let Some((mut p, dir)) = probe("g4f07b-dismount") else { return };
    let poses = [(0.6, 1.8), (0.6, 1.5), (0.6, 0.6)];
    for k in [Kind::Pig, Kind::Chicken] {
        let id = bare(&mut p, k, [216.5, -59.0, 206.5]);
        let v = p.s.mob_ref(id).cloned().unwrap();
        assert_eq!(p.s.dismount_location(&v, 0.6, &poses), [215.5, -59.0, 206.5], "{k:?}: west cell");
        p.run("setblock 215 -59 206 minecraft:stone\nsetblock 215 -58 206 minecraft:stone\n");
        assert_eq!(p.s.dismount_location(&v, 0.6, &poses), [217.5, -59.0, 206.5], "{k:?}: east cell");
        p.run("setblock 215 -59 206 minecraft:air\nsetblock 215 -58 206 minecraft:air\n");
        let i = p.s.mob_index(id).unwrap();
        p.s.mobs.list[i] = None;
    }
    cleanup(p, dir);
}

fn spit(owner: i32, pos: [f64; 3], vel: [f64; 3], tick: i32) -> Spit {
    Spit {
        id: 990_000 + tick,
        owner,
        pos,
        vel,
        yaw: 0.0,
        pitch: 0.0,
        rng: JRand::new(7),
        removed: false,
        left_owner: true,
        tick,
    }
}

/// Shared margin: on the first tick (margin 0), a spit grazing a cow at 0.2 does not hit it;
/// on the 10th tick (margin capped at 0.3), it does.
#[test]
fn spit_shared_margin() {
    let Some((mut p, dir)) = probe("g4f07b-margin") else { return };
    let llama = bare(&mut p, Kind::Llama, [205.5, -59.0, 205.5]);
    let cow = bare(&mut p, Kind::Cow, [216.5, -59.0, 210.5]);
    let max = mob(&p, cow).health;
    let half = f64::from(mob(&p, cow).width()) / 2.0;
    let x = 216.5 + half + 0.2;
    p.s.mobs.mounts.spits.push(spit(llama, [x, -58.5, 208.0], [0.0, 0.0, 3.0], 0));
    p.s.spits_tick();
    assert_eq!(mob(&p, cow).health, max, "hit without margin");
    p.s.mobs.mounts.spits.clear();
    p.s.mobs.mounts.spits.push(spit(llama, [x, -58.5, 208.0], [0.0, 0.0, 3.0], 9));
    p.s.spits_tick();
    assert!(mob(&p, cow).health < max, "not hit with the 0.3 margin");
    cleanup(p, dir);
}

/// Leaving the llama: a spit 0.5 from the llama's box has not left it yet (box stretched and
/// inflated by 1); rotation smoothed towards the velocity (a fifth of the difference per tick).
#[test]
fn spit_exit_and_rotation() {
    let Some((mut p, dir)) = probe("g4f07b-exit") else { return };
    let llama = bare(&mut p, Kind::Llama, [210.5, -59.0, 210.5]);
    let half = f64::from(mob(&p, llama).width()) / 2.0;
    let mut s = spit(llama, [210.5 + half + 0.5 + 0.125, -58.0, 210.5], [0.5, 0.0, 0.0], 0);
    s.left_owner = false;
    p.s.mobs.mounts.spits.push(s);
    p.s.spits_tick();
    let s = p.s.mobs.mounts.spits.first().expect("spit removed");
    assert!(!s.left_owner, "left the llama too early");
    assert!((s.yaw - 18.0).abs() < 1.0e-3, "rotation not smoothed: {}", s.yaw);
    assert_eq!(s.pitch, 0.0);
    cleanup(p, dir);
}
