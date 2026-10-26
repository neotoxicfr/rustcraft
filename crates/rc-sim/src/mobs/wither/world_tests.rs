//! Wither end to end on a copy of the test world (`RUSTCRAFT_SOLO_WORLD`, generated area
//! 200..231) driven by the probe: construction at the block, summoning (healing, explosion), skull
//! shots and wither effect on the player, damage rules, Nether star, wither rose.

use super::super::mob::Ent;
use super::super::species::Kind;
use crate::blocks::probe::{Probe, state_string};
use crate::projectiles::PKind;

/// Stone floor at −60, air above, player at the center (survival, normal difficulty).
fn probe(tag: &str) -> Option<(Probe, std::path::PathBuf)> {
    let (world, data) = (crate::tests::test_world()?, rc_testdata::version_data()?);
    let dir = super::super::tests::prep(&world, tag, 0, "normal", &[]);
    let mut p = Probe::open(&dir, std::sync::Arc::new(data));
    p.run(
        "fill 200 -60 200 231 -60 231 minecraft:stone\nfill 200 -59 200 231 -50 231 minecraft:air\nplayer 216.5 -59 202.5 0 0\n",
    );
    if name_at(&p, [216, -60, 216]) != "minecraft:stone" {
        eprintln!("area 200..231 missing from the copy");
        return None;
    }
    p.s.game_mode = 0;
    p.s.view = 8;
    // Player ready (damage taken).
    p.s.loaded = true;
    Some((p, dir))
}

fn step(p: &mut Probe, n: i32) {
    for _ in 0..n {
        p.tick();
        p.s.mobs_tick();
        p.s.projectiles_tick();
    }
}

fn name_at(p: &Probe, at: [i32; 3]) -> String {
    state_string(&p.s, p.s.get(at))
}

fn withers(p: &Probe) -> Vec<i32> {
    p.s.mobs.list.iter().flatten().filter(|m| m.kind == Kind::Wither && !m.removed).map(|m| m.id).collect()
}

/// Soul sand T (x axis) at 209..211, skulls placed one by one: the last one summons the wither
/// (pattern erased, foot of the T + 0.55, yaw 90), a third of its HP; 220 ticks later it has
/// regained its HP, exploded (crater) and targets the player.
#[test]
fn wither_built_and_summoned() {
    let Some((mut p, dir)) = probe("f16build") else { return };
    p.run(
        "setblock 210 -59 216 minecraft:soul_sand\nsetblock 209 -58 216 minecraft:soul_soil\nsetblock 210 -58 216 minecraft:soul_sand\nsetblock 211 -58 216 minecraft:soul_sand\nsetblock 209 -57 216 minecraft:wither_skeleton_skull\nsetblock 210 -57 216 minecraft:wither_skeleton_skull\n",
    );
    // Two skulls: nothing.
    p.s.wither_check_spawn([210, -57, 216]);
    assert!(withers(&p).is_empty());
    assert!(p.s.wither_can_spawn_mob([211, -57, 216]), "base pattern (dispenser)");
    p.run("setblock 211 -57 216 minecraft:wither_skeleton_wall_skull[facing=south]\n");
    p.s.wither_check_spawn([211, -57, 216]);
    let w = withers(&p);
    assert_eq!(w.len(), 1, "wither not summoned");
    let m = p.s.mob_ref(w[0]).unwrap();
    assert_eq!(m.pos, [210.5, -59.0 + 0.55, 216.5]);
    assert_eq!((m.yaw, m.body_yaw), (90.0, 90.0));
    assert_eq!((m.wither.inv, m.health), (220, 100.0));
    for q in [[210, -59, 216], [209, -58, 216], [211, -58, 216], [209, -57, 216], [211, -57, 216]] {
        assert_eq!(name_at(&p, q), "minecraft:air", "{q:?}");
    }
    let id = w[0];
    // Chunks sent to the player (probe's network tracking).
    for cx in 12..=14 {
        for cz in 12..=14 {
            p.s.sent.insert((cx, cz));
        }
    }
    step(&mut p, 1);
    {
        let m = p.s.mob_ref(id).unwrap();
        assert_eq!(m.wither.inv, 219);
        assert!(m.wither.bar_shown, "boss bar shown to the player");
        assert!((m.wither.bar_progress - (1.0 - 219.0 / 220.0)).abs() < 1e-6);
    }
    step(&mut p, 219);
    let m = p.s.mob_ref(id).unwrap();
    eprintln!("after the summoning: HP {}, inv {}, target {:?}", m.health, m.wither.inv, m.target);
    assert_eq!(m.wither.inv, 0);
    assert_eq!(m.health, 300.0);
    assert_eq!(name_at(&p, [210, -60, 216]), "minecraft:air", "explosion crater");
    step(&mut p, 120);
    let m = p.s.mob_ref(id).unwrap();
    let skulls = p.s.proj.list.iter().filter(|s| s.kind == PKind::WitherSkull).count();
    eprintln!("target {:?}, heads {:?}, skulls {skulls}, HP {}", m.target, m.wither.targets, p.s.life.vit.health);
    assert_eq!(m.target, Some(Ent::Player), "the wither does not target the player");
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Active wither (egg): skulls shot at the player, hit with the level-2 wither effect (10 s on
/// normal difficulty).
#[test]
fn wither_skulls_wither_the_player() {
    let Some((mut p, dir)) = probe("f16skulls") else { return };
    let m = p.s.new_mob(Kind::Wither, [216.5, -55.0, 212.5], 0.0);
    p.s.add_mob(m);
    let (mut seen, mut hit) = (0, false);
    let mut ids = hashbrown::HashSet::new();
    let mut trace: std::collections::BTreeMap<i32, ([f64; 3], [f64; 3], i32)> = Default::default();
    for _ in 0..400 {
        step(&mut p, 1);
        for s in p.s.proj.list.iter().filter(|s| s.kind == PKind::WitherSkull) {
            ids.insert(s.id);
            let e = trace.entry(s.id).or_insert((s.pos, s.pos, 0));
            e.1 = s.pos;
            e.2 += 1;
        }
        seen = ids.len();
        hit |= p.s.life.fx.list.iter().any(|e| e.id == "minecraft:wither" && e.amp == 1);
        if hit && seen >= 2 {
            break;
        }
    }
    eprintln!("skulls {seen}, player withered {hit}, HP {}", p.s.life.vit.health);
    for (id, t) in trace.iter().take(6) {
        eprintln!("skull {id}: {t:?}");
    }
    if let Some(w) = p.s.mobs.list.iter().flatten().find(|m| m.kind == Kind::Wither) {
        eprintln!("wither {:?} target {:?} heads {:?}", w.pos, w.target, w.wither.targets);
    }
    assert!(seen >= 1, "no skull");
    assert!(hit, "wither effect missing");
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Damage rules: summoning (refused except "killed"), armor against arrows, fire and
/// drowning refused; an accepted hit arms block destruction (20 ticks).
#[test]
fn wither_damage_rules() {
    let Some((mut p, dir)) = probe("f16hurt") else { return };
    let mut m = p.s.new_mob(Kind::Wither, [216.5, -59.0, 212.5], 0.0);
    m.wither.inv = 100;
    assert!(!p.s.mob_hurt(&mut m, "minecraft:player_attack", 5.0, Some(Ent::Player), None));
    assert!(!p.s.mob_hurt(&mut m, "minecraft:in_fire", 5.0, None, None));
    assert_eq!(m.wither.destroy_blocks_tick, 0);
    m.wither.inv = 0;
    assert!(!p.s.mob_hurt(&mut m, "minecraft:drown", 5.0, None, None));
    assert!(p.s.mob_hurt(&mut m, "minecraft:player_attack", 5.0, Some(Ent::Player), None));
    assert_eq!(m.wither.destroy_blocks_tick, 20);
    assert_eq!(m.wither.idle_head_updates, [3, 3]);
    m.health = 120.0;
    m.damage_cooldown = 0;
    assert!(!p.s.mob_hurt(&mut m, "minecraft:arrow", 5.0, Some(Ent::Player), None), "armor against the arrow");
    assert!(p.s.mob_hurt(&mut m, "minecraft:player_attack", 5.0, Some(Ent::Player), None));
    // Effects refused.
    assert!(!p.s.give_mob_effect(&mut m, "minecraft:regeneration", 100, 0));
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Fall without effect (fall-immune tag); melee hit from an undead refused even
/// when the attacker is out of the list (only `killer` describes it).
#[test]
fn wither_fall_and_undead_melee() {
    let Some((mut p, dir)) = probe("f16hurt2") else { return };
    let mut m = p.s.new_mob(Kind::Wither, [216.5, -59.0, 212.5], 0.0);
    m.wither.inv = 0;
    let health = m.health;
    assert!(!p.s.mob_hurt(&mut m, "minecraft:fall", 5.0, None, None));
    assert_eq!((m.health, m.wither.destroy_blocks_tick, m.wither.idle_head_updates), (health, 0, [0, 0]));
    let rng = super::super::jrand::JRand::new(1);
    p.s.mobs.killer = Some((rng, Kind::Zombie, 9999, [216, -59, 213]));
    assert!(!p.s.mob_hurt(&mut m, "minecraft:mob_attack", 5.0, Some(Ent::Mob(9999)), None));
    assert_eq!((m.health, m.wither.destroy_blocks_tick), (health, 0));
    p.s.mobs.killer = Some((rng, Kind::Creeper, 9999, [216, -59, 213]));
    assert!(p.s.mob_hurt(&mut m, "minecraft:mob_attack", 5.0, Some(Ent::Mob(9999)), None));
    p.s.mobs.killer = None;
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Death: Nether star with a long lifetime; a pig killed by the wither leaves a wither rose
/// on the grass.
#[test]
fn wither_star_and_rose() {
    let Some((mut p, dir)) = probe("f16loot") else { return };
    let w = p.s.new_mob(Kind::Wither, [216.5, -59.0, 220.5], 0.0);
    let wid = w.id;
    p.s.add_mob(w);
    p.run("setblock 212 -60 212 minecraft:grass_block\n");
    let mut pig = p.s.new_mob(Kind::Pig, [212.5, -59.0, 212.5], 0.0);
    assert!(p.s.mob_hurt(&mut pig, "minecraft:wither_skull", 100.0, Some(Ent::Mob(wid)), None));
    assert!(pig.dead);
    assert_eq!(name_at(&p, [212, -59, 212]), "minecraft:wither_rose");
    let i = p.s.mob_index(wid).unwrap();
    let mut w = p.s.mobs.list[i].take().unwrap();
    w.last_hurt_by_player = 100;
    p.s.mob_hurt(&mut w, "minecraft:player_attack", 1000.0, Some(Ent::Player), None);
    assert!(w.dead);
    let star = p.s.life.drops.iter().find(|d| d.stack.id == "minecraft:nether_star").expect("Nether star");
    assert_eq!(star.age, -6000);
    p.s.mobs.list[i] = Some(w);
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}
