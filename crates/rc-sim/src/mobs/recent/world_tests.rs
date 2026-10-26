//! Family F14 end to end on a copy of the test world (`RUSTCRAFT_SOLO_WORLD`, zone
//! 200..231) driven by the probe: creaking linked to its heart, breeze and wind charges, warden
//! and vibrations, shrieker, happy ghast (harness, mounting, dismounting), dried ghast, save.

use rc_proto::nbt::Nbt;

use super::super::mob::{Ent, Mob};
use super::super::species::Kind;
use crate::blocks::probe::{Probe, state_string};
use crate::input::Input;

/// Stone floor at −60, air above, survival player (normal difficulty).
fn probe(tag: &str) -> Option<(Probe, std::path::PathBuf)> {
    let (world, data) = (crate::tests::test_world(), rc_testdata::version_data());
    let (Some(world), Some(data)) = (world.clone(), data) else {
        eprintln!("test world {world:?} or data missing: test skipped");
        return None;
    };
    let dir = super::super::tests::prep(&world, tag, 0, "normal", &[]);
    let mut p = Probe::open(&dir, std::sync::Arc::new(data));
    p.run(
        "fill 200 -60 200 231 -60 231 minecraft:stone\nfill 200 -59 200 231 -45 231 minecraft:air\nplayer 216.5 -59 202.5 0 0\n",
    );
    if state_string(&p.s, p.s.get([216, -60, 216])) != "minecraft:stone" {
        eprintln!("zone 200..231 missing from the copy");
        return None;
    }
    p.s.game_mode = 0;
    p.s.view = 8;
    p.s.loaded = true;
    p.s.on_ground = true;
    Some((p, dir))
}

fn step(p: &mut Probe, n: i32) {
    for _ in 0..n {
        p.tick();
        p.s.mobs_tick();
        p.s.projectiles_tick();
    }
}

fn spawn(p: &mut Probe, k: Kind, pos: [f64; 3]) -> i32 {
    let mut m = p.s.new_mob(k, pos, 0.0);
    // Like `/summon`: finalized mob (warden's dig cooldown…).
    let mut g = super::super::spawn::Group::default();
    p.s.finalize_spawn(&mut m, super::super::spawn::Reason::SpawnEgg, &mut g);
    let id = m.id;
    p.s.add_mob(m);
    id
}

fn mob(p: &Probe, id: i32) -> Option<&Mob> {
    p.s.mob_ref(id)
}

fn mob_mut(p: &mut Probe, id: i32) -> Option<&mut Mob> {
    let i = p.s.mob_index(id)?;
    p.s.mobs.list[i].as_deref_mut()
}

fn set_day_time(p: &mut Probe, t: i64) {
    match p.s.clocks.iter_mut().find(|(n, _)| n == "minecraft:overworld") {
        Some(c) => c.1 = t,
        None => p.s.clocks.push(("minecraft:overworld".into(), t)),
    }
    p.s.time = t;
}

/// The player looks at a point (the game's yaw and pitch).
fn look_at(p: &mut Probe, t: [f64; 3]) {
    let eye = [p.s.pos[0], p.s.pos[1] + 1.62, p.s.pos[2]];
    let (dx, dy, dz) = (t[0] - eye[0], t[1] - eye[1], t[2] - eye[2]);
    p.s.yaw = (dz.atan2(dx).to_degrees() - 90.0) as f32;
    p.s.pitch = (-(dy.atan2((dx * dx + dz * dz).sqrt())).to_degrees()) as f32;
}

/// Player step: movement sent as by the client (vibrations, shrieker).
fn walk(p: &mut Probe, to: [f64; 3]) {
    p.s.handle(Input::Move { pos: Some(to), rot: None, on_ground: true });
}

fn cleanup(p: Probe, dir: std::path::PathBuf) {
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Heart between two pale oak logs, at night: the creaking appears, linked to the heart; looked at
/// from close up it freezes; hit, it loses nothing (the heart suffers); heart broken, it tears down.
#[test]
fn creaking_bound_to_heart() {
    let Some((mut p, dir)) = probe("f14creak") else { return };
    set_day_time(&mut p, 18000);
    p.run(
        "setblock 216 -59 222 minecraft:pale_oak_log[axis=y]\nsetblock 216 -58 222 minecraft:creaking_heart[axis=y,natural=true]\nsetblock 216 -57 222 minecraft:pale_oak_log[axis=y]\n",
    );
    let heart = [216, -58, 222];
    let mut found = None;
    for _ in 0..200 {
        step(&mut p, 1);
        found = p.s.mobs.list.iter().flatten().find(|m| m.kind == Kind::Creaking).map(|m| m.id);
        if found.is_some() {
            break;
        }
    }
    eprintln!("heart state {:?}", p.s.heart_state_value(heart));
    let c = found.expect("creaking never appeared");
    assert_eq!(mob(&p, c).unwrap().rec.home, Some(heart), "creaking not linked to the heart");
    // Player right next to it, looking at it: activated then frozen.
    let cp = mob(&p, c).unwrap().pos;
    p.run(&format!("player {} -59 {} 0 0\n", cp[0], cp[2] - 4.0));
    let mut frozen = false;
    for _ in 0..60 {
        let eye = mob(&p, c).unwrap().eye_y();
        let cp = mob(&p, c).unwrap().pos;
        look_at(&mut p, [cp[0], eye, cp[2]]);
        step(&mut p, 1);
        let m = mob(&p, c).unwrap();
        if m.rec.active && !m.rec.can_move {
            frozen = true;
            break;
        }
    }
    assert!(frozen, "creaking looked at never frozen");
    // Player hit: no damage, heart distress call.
    let hp = mob(&p, c).unwrap().health;
    p.s.player_attack(c);
    let m = mob(&p, c).unwrap();
    assert_eq!(m.health, hp, "linked creaking hurt");
    assert!(m.rec.invul_anim > 0, "invulnerability animation missing");
    assert!(p.s.mobs.rec.hearts.map.get(&heart).is_some_and(|h| h.emitter > 0), "heart without distress call");
    // Heart broken: death with effects then breaking apart.
    p.run(&format!("break {} {} {}\n", heart[0], heart[1], heart[2]));
    step(&mut p, 60);
    assert!(mob(&p, c).is_none(), "creaking still there after the heart is broken");
    cleanup(p, dir);
}

/// Player's wind charge: thrown (stack consumed, cooldown), burst against the wall, which
/// pushes a neighboring mob back and opens a wooden trapdoor.
#[test]
fn player_wind_charge_bursts() {
    use crate::projectiles::PKind;
    let Some((mut p, dir)) = probe("f14wind") else { return };
    p.run(
        "fill 214 -59 210 218 -55 210 minecraft:stone\nsetblock 215 -58 209 minecraft:oak_trapdoor[facing=south,half=bottom,open=false]\nplayer 216.5 -59 204.5 0 0\n",
    );
    let pig = spawn(&mut p, Kind::Pig, [217.5, -59.0, 208.5]);
    if let Some(m) = mob_mut(&mut p, pig) {
        m.no_ai = true;
    }
    p.s.slots[36 + p.s.held as usize] =
        Some(crate::player::Stack { id: "minecraft:wind_charge".into(), count: 2, components: None });
    look_at(&mut p, [216.5, -57.8, 210.0]);
    assert!(p.s.proj_use_item(0), "wind charge not thrown");
    assert_eq!(p.s.slots[36 + p.s.held as usize].as_ref().map(|s| s.count), Some(1), "stack not consumed");
    assert!(p.s.proj.list.iter().any(|q| q.kind == PKind::WindCharge), "no charge in flight");
    let mut gone = None;
    let mut kick = [0.0; 3];
    for t in 0..40 {
        step(&mut p, 1);
        if gone.is_none() && !p.s.proj.list.iter().any(|q| q.kind == PKind::WindCharge && !q.removed) {
            gone = Some(t);
            // Mob without AI: the push stays in its velocity (it does not move).
            kick = mob(&p, pig).unwrap().vel;
        }
    }
    let door = state_string(&p.s, p.s.get([215, -58, 209]));
    eprintln!("burst {gone:?}, pig push {kick:?}, trapdoor {door}");
    assert!(gone.is_some(), "charge never burst");
    assert!(dist(kick, [0.0; 3]) > 0.05, "pig not pushed back");
    assert!(door.contains("open=true"), "trapdoor not open: {door}");
    cleanup(p, dir);
}

/// The breeze deflects an arrow (sound, U-turn) instead of being hit.
#[test]
fn breeze_deflects_arrow() {
    use crate::projectiles::{PKind, Proj};
    let Some((mut p, dir)) = probe("f14deflect") else { return };
    let b = spawn(&mut p, Kind::Breeze, [216.5, -59.0, 214.5]);
    if let Some(m) = mob_mut(&mut p, b) {
        m.no_ai = true;
    }
    let id = p.s.proj_new_id();
    let item = crate::player::Stack { id: "minecraft:arrow".into(), count: 1, components: None };
    let mut a = Proj::new(PKind::Arrow, id, [216.5, -58.2, 210.5], item);
    a.shoot([0.0, 0.0, 1.0], 1.5, 0.0);
    p.s.proj_add(a);
    let hp = mob(&p, b).unwrap().health;
    let mut back = false;
    for _ in 0..10 {
        step(&mut p, 1);
        if p.s.proj.list.iter().any(|q| q.id == id && q.vel[2] < 0.0) {
            back = true;
            break;
        }
    }
    assert!(back, "arrow not deflected");
    assert_eq!(mob(&p, b).unwrap().health, hp, "breeze hit");
    cleanup(p, dir);
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Breeze 10 blocks from a survival player: it shoots a wind charge that pushes the player back.
#[test]
fn breeze_shoots_wind_charge() {
    let Some((mut p, dir)) = probe("f14breeze") else { return };
    p.run("player 216.5 -59 212.5 0 0\n");
    let b = spawn(&mut p, Kind::Breeze, [216.5, -59.0, 222.5]);
    let (mut shot, mut pushed) = (None, None);
    for t in 0..400 {
        step(&mut p, 1);
        if shot.is_none() && p.s.proj.list.iter().any(|q| q.kind == crate::projectiles::PKind::BreezeWindCharge) {
            shot = Some(t);
        }
        if shot.is_some() && pushed.is_none() && p.s.mobs.player_vel.iter().any(|v| v.abs() > 0.1) {
            pushed = Some(t);
            break;
        }
    }
    eprintln!("shot {shot:?}, push {pushed:?}, breeze {:?}", mob(&p, b).map(|m| m.pos));
    assert!(shot.is_some(), "no wind charge shot");
    assert!(pushed.is_some(), "player never pushed back");
    cleanup(p, dir);
}

/// Warden: the player's steps (vibrations) anger it against them.
#[test]
fn warden_hears_player_steps() {
    let Some((mut p, dir)) = probe("f14warden") else { return };
    p.run("player 216.5 -59 210.5 0 0\n");
    let w = spawn(&mut p, Kind::Warden, [216.5, -59.0, 220.5]);
    step(&mut p, 5);
    let mut x = 210.5;
    for _ in 0..80 {
        x += 0.25;
        walk(&mut p, [x, -59.0, 210.5]);
        step(&mut p, 1);
    }
    let m = mob(&p, w).expect("warden");
    let anger = m.rec.anger.suspects.iter().find(|s| s.0 == Ent::Player).map_or(0, |s| s.2);
    eprintln!(
        "anger {anger}, suspects {:?}, steps {} / {}, listeners {}, player {:?}, vib {:?}",
        m.rec.anger.suspects,
        p.s.mobs.rec.player_move_dist,
        p.s.mobs.rec.player_next_step,
        p.s.mobs.rec.vib.listeners.len(),
        p.s.pos,
        m.rec.vib
    );
    assert!(anger > 0, "the warden heard nothing");
    // Repeated steps near it: anger, roar, then hit (30 on normal).
    let start = p.s.life.vit.health;
    let mut hit = None;
    let mut roared = false;
    for t in 0..1200 {
        let dx = if (t / 20) % 2 == 0 { 0.2 } else { -0.2 };
        let to = [p.s.pos[0] + dx, -59.0, 214.5];
        walk(&mut p, to);
        step(&mut p, 1);
        let Some(m) = mob(&p, w) else { break };
        roared |= super::super::brainy::pose(m) == super::POSE_ROARING;
        if p.s.life.vit.health < start || p.s.life.vit.dead {
            hit = Some(t);
            break;
        }
    }
    let m = mob(&p, w).expect("warden");
    eprintln!("roar {roared}, hit {hit:?}, anger {:?}, HP {}", m.rec.anger.suspects, p.s.life.vit.health);
    assert!(roared, "no roar");
    assert!(hit.is_some(), "the warden never hit");
    cleanup(p, dir);
}

/// Shrieker that can summon: the player walks on it, warning raised; at level 4, a
/// warden emerges at the end of the shriek.
#[test]
fn shrieker_summons_warden() {
    let Some((mut p, dir)) = probe("f14shriek") else { return };
    p.run("setblock 216 -60 210 minecraft:sculk_shrieker[can_summon=true]\nplayer 216.5 -59 210.5 0 0\n");
    step(&mut p, 2);
    walk(&mut p, [216.5, -59.0, 210.6]);
    step(&mut p, 2);
    let lvl = p.s.mobs.rec.warden_tracker.warning_level;
    assert_eq!(lvl, 1, "first warning");
    // Level 3 and empty cooldown: the next shriek summons a warden.
    p.s.mobs.rec.warden_tracker.warning_level = 3;
    p.s.mobs.rec.warden_tracker.cooldown = 0;
    step(&mut p, 100);
    walk(&mut p, [216.5, -59.0, 210.7]);
    step(&mut p, 100);
    let w = p.s.mobs.list.iter().flatten().find(|m| m.kind == Kind::Warden);
    eprintln!("level {}, warden {:?}", p.s.mobs.rec.warden_tracker.warning_level, w.map(|m| m.pos));
    assert!(w.is_some(), "no warden summoned");
    cleanup(p, dir);
}

/// Happy ghast: harness placed, the player mounts; on dismounting, motionless for 10 ticks.
#[test]
fn happy_ghast_harness_and_ride() {
    let Some((mut p, dir)) = probe("f14ghast") else { return };
    let g = spawn(&mut p, Kind::HappyGhast, [216.5, -55.0, 206.5]);
    step(&mut p, 2);
    p.run("hold minecraft:white_harness\n");
    assert!(p.s.player_interact(g, 0), "harness refused");
    assert!(mob(&p, g).unwrap().equipment[super::super::mounts::BODY].is_some(), "harness not worn");
    p.s.slots[36 + p.s.held as usize] = None;
    assert!(p.s.player_interact(g, 0), "mounting refused");
    assert_eq!(p.s.mobs.mounts.riding.vehicle, Some(g), "player not mounted");
    step(&mut p, 5);
    p.s.player_stop_riding();
    let m = mob(&p, g).unwrap();
    assert_eq!(m.rec.still_timeout, 10, "motionlessness on dismount");
    assert!(m.mount.passengers.is_empty());
    cleanup(p, dir);
}

/// Dried ghast waterlogged at the end of its rehydration: a happy ghastling hatches.
#[test]
fn dried_ghast_hatches() {
    let Some((mut p, dir)) = probe("f14dried") else { return };
    p.run("setblock 216 -59 216 minecraft:dried_ghast[hydration=3,waterlogged=true,facing=north]\n");
    let pos = [216, -59, 216];
    let st = p.s.get(pos);
    assert!(p.s.rec_block_tick(st, pos), "dried ghast tick not handled");
    let g = p.s.mobs.list.iter().flatten().find(|m| m.kind == Kind::HappyGhast);
    assert!(g.is_some_and(|m| m.is_baby()), "no ghastling hatched");
    assert!(state_string(&p.s, p.s.get(pos)).starts_with("minecraft:water"), "block not replaced by water");
    cleanup(p, dir);
}

/// Save: warden anger, happy ghast motionlessness, creaking heart.
#[test]
fn recent_saved_and_reloaded() {
    let Some((mut p, dir)) = probe("f14save") else { return };
    let w = spawn(&mut p, Kind::Warden, [210.5, -59.0, 210.5]);
    let g = spawn(&mut p, Kind::HappyGhast, [214.5, -55.0, 210.5]);
    let uuid = 0x1234_5678_9abc_def0_1234_5678_9abc_def0u128;
    if let Some(m) = mob_mut(&mut p, w) {
        m.no_ai = true;
        m.rec.anger.by_uuid.push((uuid, 60));
    }
    if let Some(m) = mob_mut(&mut p, g) {
        m.no_ai = true;
        m.rec.still_timeout = 7;
    }
    step(&mut p, 1);
    p.run("save\n");
    let list = super::super::tests::saved(&dir, 210 >> 4, 210 >> 4);
    let wn = list.iter().find(|e| e.get("id").and_then(Nbt::as_str) == Some("minecraft:warden")).expect("warden");
    let gn = list.iter().find(|e| e.get("id").and_then(Nbt::as_str) == Some("minecraft:happy_ghast")).expect("ghast");
    eprintln!("warden {:?}", wn.get("anger"));
    assert!(wn.get("anger").is_some(), "anger not saved");
    let mem = wn.get("Brain").and_then(|b| b.get("memories")).expect("warden memories");
    assert!(mem.get("minecraft:dig_cooldown").is_some(), "dig cooldown not saved: {mem:?}");
    assert_eq!(crate::nbtio::int(gn, "still_timeout"), Some(7));
    let mut back = p.s.new_mob(Kind::Warden, [0.0; 3], 0.0);
    back.load(wn);
    assert!(back.rec.anger.by_uuid.iter().any(|&(u, a)| u == uuid && a == 60), "anger not read back");
    assert!(back.bm.brain.as_ref().is_some_and(|b| b.mem.x.dig_cooldown.has()), "dig cooldown not read back");
    cleanup(p, dir);
}

/// Demo world (screenshots): warden, breeze, creaking and its heart, harnessed and baby happy ghast,
/// dried ghast, shrieker. `RUSTCRAFT_F14_PREP` = destination,
/// `RUSTCRAFT_F14_SCENE` = `static` (no AI, creative, day) or `live` (AI, survival, night).
#[test]
#[ignore]
fn prep_recent_world() {
    let (Ok(dest), Some(world), Some(data)) =
        (std::env::var("RUSTCRAFT_F14_PREP"), crate::tests::test_world(), rc_testdata::version_data())
    else {
        return;
    };
    let live = std::env::var("RUSTCRAFT_F14_SCENE").is_ok_and(|s| s == "live");
    let dir = super::super::tests::prep(&world, "f14prep", if live { 0 } else { 1 }, "normal", &[]);
    {
        let mut p = Probe::open(&dir, std::sync::Arc::new(data));
        p.run(
            "fill 200 -60 200 231 -60 231 minecraft:stone\n\
             fill 200 -59 200 231 -45 231 minecraft:air\n\
             setblock 213 -59 214 minecraft:pale_oak_log[axis=y]\n\
             setblock 213 -58 214 minecraft:creaking_heart[axis=y,natural=true]\n\
             setblock 213 -57 214 minecraft:pale_oak_log[axis=y]\n\
             setblock 219 -59 210 minecraft:dried_ghast[facing=west]\n\
             setblock 210 -60 209 minecraft:sculk_shrieker[can_summon=true]\n\
             player 216.5 -59 202.5 0 5\n",
        );
        p.s.view = 8;
        set_day_time(&mut p, if live { 18000 } else { 6000 });
        spawn(&mut p, Kind::Warden, [211.5, -59.0, 216.5]);
        spawn(&mut p, Kind::Breeze, [216.5, -59.0, 213.5]);
        spawn(&mut p, Kind::Creaking, [215.0, -59.0, 211.0]);
        let g = spawn(&mut p, Kind::HappyGhast, [221.5, -59.0, 218.5]);
        if let Some(m) = mob_mut(&mut p, g) {
            let body = super::super::mounts::BODY;
            m.equipment[body] =
                Some(crate::player::Stack { id: "minecraft:red_harness".into(), count: 1, components: None });
        }
        let baby = spawn(&mut p, Kind::HappyGhast, [219.5, -58.0, 207.5]);
        if let Some(m) = mob_mut(&mut p, baby) {
            m.age = -24000;
        }
        for m in p.s.mobs.list.iter_mut().flatten() {
            m.yaw = 180.0;
            m.body_yaw = 180.0;
            m.head_yaw = 180.0;
            m.no_ai = !live;
            m.persistent = true;
        }
        step(&mut p, 2);
        p.run("save\n");
    }
    super::super::tests::move_player(&dir, [216.5, -59.0, 202.5]);
    let dest = std::path::PathBuf::from(dest);
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::rename(&dir, &dest).unwrap();
    eprintln!("F14 world prepared: {}", dest.display());
}
