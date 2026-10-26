//! Attacker mid-tick (taken out of the list): the victim finds it again by identifier
//! (anger of the warden and the nautilus, villager gossip, aimed knockback of the sulfur cube).
//! Test world required (`RUSTCRAFT_SOLO_WORLD`, area 200..231).

use super::mob::{Ent, Mob};
use super::species::Kind;
use crate::blocks::probe::{Probe, state_string};
use crate::player::Stack;
use crate::server::Server;

fn probe(tag: &str) -> Option<(Probe, std::path::PathBuf)> {
    let (world, data) = (crate::tests::test_world(), rc_testdata::version_data());
    let (Some(world), Some(data)) = (world.clone(), data) else {
        eprintln!("test world {world:?} or data absent: test skipped");
        return None;
    };
    let dir = super::tests::prep(&world, tag, 0, "normal", &[]);
    let mut p = Probe::open(&dir, std::sync::Arc::new(data));
    p.run("fill 200 -60 200 231 -60 231 minecraft:stone\nfill 200 -59 200 231 -45 231 minecraft:air\nplayer 216.5 -59 202.5 0 0\n");
    if state_string(&p.s, p.s.get([216, -60, 216])) != "minecraft:stone" {
        eprintln!("area 200..231 absent from the copy");
        return None;
    }
    p.s.game_mode = 0;
    p.s.loaded = true;
    Some((p, dir))
}

fn spawn(p: &mut Probe, k: Kind, pos: [f64; 3]) -> i32 {
    let mut m = p.s.new_mob(k, pos, 0.0);
    let mut g = super::spawn::Group::default();
    p.s.finalize_spawn(&mut m, super::spawn::Reason::SpawnEgg, &mut g);
    let id = m.id;
    p.s.add_mob(m);
    id
}

/// Melee hit by `att` on `victim`, the attacker taken out of the list as during its tick.
fn melee(s: &mut Server, att: i32, victim: i32) {
    let i = s.mob_index(att).expect("attacker");
    let mut m: Box<Mob> = s.mobs.list[i].take().expect("attacker");
    s.mobs.ticking.push((m.uuid, m.id));
    s.mob_attack(&mut m, Ent::Mob(victim));
    s.mobs.ticking.pop();
    s.mobs.list[i] = Some(m);
}

#[test]
fn victim_finds_attacker_mid_tick() {
    let Some((mut p, _dir)) = probe("pattern5") else { return };
    // Warden hit by an iron golem: anger +100 against the golem (known UUID).
    let golem = spawn(&mut p, Kind::IronGolem, [205.5, -59.0, 205.5]);
    let warden = spawn(&mut p, Kind::Warden, [206.5, -59.0, 205.5]);
    let gu = p.s.mob_ref(golem).unwrap().uuid;
    melee(&mut p.s, golem, warden);
    let w = p.s.mob_ref(warden).unwrap();
    let sus = w.rec.anger.suspects.iter().find(|x| x.0 == Ent::Mob(golem)).copied();
    let mut bad = Vec::new();
    if !matches!(sus, Some((_, u, a)) if u == gu && a >= 100) {
        bad.push(format!("warden anger: {sus:?}"));
    }
    assert!(p.s.mobs.ghosts.is_empty());

    // Nautilus hit by a zombie: 400-tick anger on the zombie's UUID.
    let z1 = spawn(&mut p, Kind::Zombie, [210.5, -59.0, 210.5]);
    let naut = spawn(&mut p, Kind::Nautilus, [211.5, -59.0, 210.5]);
    let zu = p.s.mob_ref(z1).unwrap().uuid;
    melee(&mut p.s, z1, naut);
    let angry = p.s.mob_ref(naut).unwrap().sea.brain.as_ref().and_then(|b| b.angry);
    if angry.map(|a| a.0) != Some(zu) {
        bad.push(format!("nautilus anger: {angry:?}"));
    }

    // Villager hit by a zombie: "hurt" gossip (negative minor) on the zombie.
    let z2 = spawn(&mut p, Kind::Zombie, [215.5, -59.0, 215.5]);
    let vil = spawn(&mut p, Kind::Villager, [216.5, -59.0, 215.5]);
    let zu2 = p.s.mob_ref(z2).unwrap().uuid;
    melee(&mut p.s, z2, vil);
    let v = p.s.mob_ref(vil).unwrap();
    let g = v.npc.as_ref().unwrap().gossips.0.iter().find(|e| e.0 == zu2).map(|e| e.1);
    if !g.is_some_and(|g| g[super::villager::G_MINOR_NEG] > 0) {
        bad.push(format!("gossip: {g:?}"));
    }

    // Sulfur cube carrying a block hit by a zombie: aimed knockback, not the common knockback.
    let z3 = spawn(&mut p, Kind::Zombie, [220.5, -59.0, 220.5]);
    let cube = spawn(&mut p, Kind::SulfurCube, [221.5, -59.0, 220.5]);
    let ci = p.s.mob_index(cube).unwrap();
    let c = p.s.mobs.list[ci].as_mut().unwrap();
    c.equipment[6] = Some(Stack { id: "minecraft:stone".into(), count: 1, components: None });
    c.vel = [0.0; 3];
    let mut common = (**c).clone();
    let zpos = p.s.mob_ref(z3).unwrap().pos;
    let (xd, zd) = (zpos[0] - common.pos[0], zpos[2] - common.pos[2]);
    Server::knockback(&mut common, 0.4, xd, zd);
    melee(&mut p.s, z3, cube);
    let got = p.s.mob_ref(cube).unwrap().vel;
    if got == common.vel {
        bad.push("sulfur cube knockback: common knockback applied".into());
    }
    assert!(bad.is_empty(), "{bad:#?}");
}
