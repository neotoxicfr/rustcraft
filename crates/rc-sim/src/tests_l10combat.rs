//! L10-COMBAT: the reach of explosions and bursts, their direct entity, the sizes of armor stands
//! and mannequins, the sweep's victims, and the game's vector normalization everywhere. On a copy
//! of the test world (`RUSTCRAFT_SOLO_WORLD`). Absent: tests skipped (failure under
//! `RUSTCRAFT_OPT_REQUIRE=1`).
#![cfg(test)]

use crate::blocks::probe::Probe;
use crate::commands::ents::E;
use crate::mobs::mob::Mob;
use crate::mobs::species::Kind;
use crate::player::Stack;
use crate::projectiles::{PKind, Proj};

const BASE: &str = "fill 100 -60 100 131 -60 131 minecraft:stone\nfill 100 -59 100 131 -40 131 minecraft:air\n";

fn probe(tag: &str) -> Option<(Probe, std::path::PathBuf)> {
    let (world, data) = (crate::tests::test_world()?, rc_testdata::version_data()?);
    let dir = crate::tests::fresh_copy(&world, tag);
    let mut p = Probe::open(&dir, std::sync::Arc::new(data));
    p.run(BASE);
    p.s.pos = [110.5, -59.0, 110.5];
    p.s.loaded = true;
    p.s.game_mode = 0;
    p.s.life.vit.health = 20.0;
    p.s.life.vit.dead = false;
    p.s.life.vit.cooldown = 0;
    p.s.yaw = 0.0;
    for i in 0..p.s.slots.len() {
        p.s.slots[i] = None;
    }
    Some((p, dir))
}

fn st(id: &str) -> Stack {
    Stack { id: id.to_owned(), count: 1, components: None }
}

/// A mob of `kind` with lots of health.
fn tank(p: &mut Probe, kind: Kind, pos: [f64; 3]) -> Mob {
    let mut z = p.s.new_mob(kind, pos, 180.0);
    z.attrs.max_health.base = 500.0;
    z.health = 500.0;
    z
}

/// Advancement tree of the given criteria (one per advancement).
fn tree(p: &mut Probe, crits: Vec<(&str, serde_json::Value)>) {
    let mut raw = std::collections::BTreeMap::new();
    for (id, c) in crits {
        raw.insert(id.to_owned(), serde_json::json!({"criteria": {"c": c}}));
    }
    p.s.prog.reload(std::sync::Arc::new(crate::progress::Tree::build(raw)));
}

fn done(p: &Probe, id: &str) -> bool {
    p.s.prog.tree.index.get(id).is_some_and(|&i| p.s.prog.done(i))
}

/// `player_hurt_entity` on a husk whose damage has a direct entity of `ty`.
fn husk_hurt_with_direct(ty: &str) -> serde_json::Value {
    serde_json::json!({"trigger": "minecraft:player_hurt_entity", "conditions": {
        "entity": {"minecraft:entity_type": "minecraft:husk"},
        "damage": {"type": {"direct_entity": {"minecraft:entity_type": ty}}}}})
}

// ---------- Direct entity of an explosion set off by a projectile ----------

/// An arrow of the player sets off an End crystal next to a husk: the crystal's explosion hurts
/// the husk with the crystal as its direct entity (the game's explosion source: the crystal,
/// caused by the player), not the arrow still being resolved. Before, the arrow was seen.
#[test]
fn crystal_set_off_by_arrow_is_the_direct_entity() {
    use crate::projectiles::physics::{EntityHit, Target};
    let Some((mut p, _dir)) = probe("l10-crystal-direct") else { return };
    p.s.pos = [101.5, -59.0, 101.5];
    tree(
        &mut p,
        vec![("l10:arrow", husk_hurt_with_direct("minecraft:arrow")), ("l10:crystal", husk_hurt_with_direct("minecraft:end_crystal"))],
    );
    let h = tank(&mut p, Kind::Husk, [116.5, -59.0, 110.5]);
    let hid = h.id;
    p.s.add_mob(h);
    let crystal = p.s.spawn_crystal([114.5, -59.0, 110.5], 0.0, false);
    let mut arrow = Proj::new(PKind::Arrow, 900_101, [112.5, -58.5, 110.5], st("minecraft:arrow"));
    arrow.owner = Some(p.s.uuid);
    arrow.damage = 2.0;
    arrow.vel = [3.0, 0.0, 0.0];
    p.s.proj_hit_entity(&mut arrow, EntityHit { ent: Target::Crystal(crystal), loc: [114.5, -58.5, 110.5] });
    assert!(p.s.mobs.end.crystals.iter().all(|c| c.id != crystal || c.removed), "crystal not set off");
    assert!(p.s.mob_ref(hid).is_some_and(|m| m.health < 500.0), "the crystal's explosion missed the husk");
    assert!(!done(&p, "l10:arrow"), "the arrow seen as the direct entity of the crystal's explosion");
    assert!(done(&p, "l10:crystal"), "the crystal is not the direct entity of its explosion");
    assert!(p.s.proj.direct_hit.is_none() && p.s.proj.blast_ent.is_none(), "direct entity left behind");
}

// ---------- Sizes of armor stands and mannequins ----------

/// Entity summoned with `/summon`'s path; its network identifier.
fn summon(p: &mut Probe, ty: &str, pos: [f64; 3], nbt: &str) -> i32 {
    let src = p.s.player_source();
    let n = crate::commands::snbt::parse(nbt).expect("snbt");
    match p.s.summon_entity(&src, ty, pos, Some(n)) {
        Ok(E::Other(id)) => id,
        r => panic!("summon {ty}: {r:?}"),
    }
}

/// Saved motion of a decoration.
fn motion(p: &Probe, id: i32) -> [f64; 3] {
    crate::nbtio::motion(p.s.deco_nbt(id).expect("decoration").get("Motion")).unwrap_or_default()
}

/// Box of width `w` and height `h` standing at `pos`.
fn around(pos: [f64; 3], w: f64, h: f64) -> rc_physics::Aabb {
    rc_physics::Aabb { min: [pos[0] - w / 2.0, pos[1], pos[2] - w / 2.0], max: [pos[0] + w / 2.0, pos[1] + h, pos[2] + w / 2.0] }
}

/// The game's burst knockback of an entity at `pos` with its eyes at `eye` and box `b`.
fn burst_kb(p: &Probe, center: [f64; 3], radius: f32, mult: f64, pos: [f64; 3], eye: f64, b: &rc_physics::Aabb) -> [f64; 3] {
    let dist = crate::mobs::mth::dist2(pos, center).sqrt() / f64::from(radius * 2.0);
    let dir = crate::mobs::mth::normalize([pos[0] - center[0], pos[1] + eye - center[1], pos[2] - center[2]]);
    let kp = (1.0 - dist) * f64::from(p.s.seen_percent(center, b)) * mult;
    dir.map(|v| v * kp)
}

/// A Wind Burst pushes a swimming mannequin from its eyes at 0.4 with its 0.6 × 0.6 box, and a
/// marker stand from its feet with no size (the game's sizes); before, the standing size and eyes
/// at 1.62 or 1.7775 were used. A stand scaled ×2 is twice as large.
#[test]
fn burst_uses_pose_marker_and_scale_sizes() {
    let Some((mut p, _dir)) = probe("l10-deco-sizes") else { return };
    p.s.pos = [130.5, -59.0, 130.5];
    let swim = [112.5, -59.0, 110.5];
    let man = summon(&mut p, "minecraft:mannequin", swim, "{pose:\"swimming\",NoGravity:1b}");
    let mark = [110.5, -59.0, 108.5];
    let marker = summon(&mut p, "minecraft:armor_stand", mark, "{Marker:1b,NoGravity:1b}");
    let center = [110.5, -58.5, 110.5];
    let sound = "minecraft:entity.wind_charge.wind_burst";
    p.s.wind_burst_with(None, None, center, 3.5, 1.2, sound, &mut []);
    let want = burst_kb(&p, center, 3.5, 1.2, swim, f64::from(0.4f32), &around(swim, f64::from(0.6f32), f64::from(0.6f32)));
    assert_eq!(motion(&p, man), want, "swimming mannequin");
    let want = burst_kb(&p, center, 3.5, 1.2, mark, 0.0, &around(mark, 0.0, 0.0));
    assert_eq!(motion(&p, marker), want, "marker stand");
    // Scale ×2: box 1 × 3.95, eyes at 3.555 (single precision products).
    let big = summon(&mut p, "minecraft:armor_stand", [120.5, -59.0, 120.5], "{NoGravity:1b,attributes:[{id:\"minecraft:scale\",base:2d}]}");
    let n = p.s.deco_nbt(big).cloned().expect("stand");
    assert_eq!(p.s.deco_box(&n), Some(around([120.5, -59.0, 120.5], 1.0, f64::from(1.975f32 * 2.0))), "scaled box");
    assert_eq!(p.s.ent_eye_height(E::Other(big)), f64::from(1.7775f32 * 2.0), "scaled eyes");
    assert_eq!(p.s.ent_eye_height(E::Other(marker)), 0.0, "marker eyes");
}

/// Velocity packets of entity `id` among the events sent since the last read.
fn motion_packets(p: &Probe, id: i32) -> Vec<[f64; 3]> {
    p.take_events()
        .into_iter()
        .filter_map(|e| match e {
            rc_client::Event::Entity(rc_client::EntityEvent::Motion { id: i, velocity }) if i == id => Some(velocity),
            _ => None,
        })
        .collect()
}

/// A weightless armor stand's push is sent by its next tick's tracking, then a further push by a
/// hair (1e-4) sends nothing: the tracking compares with the last velocity sent (1e-7 squared),
/// like the game. A stand with gravity standing in water (not moved by us) still sends its pushed
/// velocity. Before, the weightless stand sent every push at once and the one in water never.
#[test]
fn still_stand_velocity_goes_through_tracking() {
    let Some((mut p, _dir)) = probe("l10-stand-sync") else { return };
    let still = summon(&mut p, "minecraft:armor_stand", [112.5, -59.0, 110.5], "{NoGravity:1b}");
    p.s.deco_tick();
    let _ = p.take_events();
    p.s.deco_push(still, [0.25, 0.0, 0.0]);
    assert!(motion_packets(&p, still).is_empty(), "sent at once, outside the tracking");
    p.s.deco_tick();
    assert_eq!(motion_packets(&p, still), vec![crate::mobs::track::lp_quantize([0.25, 0.0, 0.0])], "push not sent by the tracking");
    p.s.deco_push(still, [1.0e-4, 0.0, 0.0]);
    for _ in 0..3 {
        p.s.deco_tick();
    }
    assert!(motion_packets(&p, still).is_empty(), "a change below the tracking threshold was sent");
    // In water: no movement simulated, the tracking runs.
    p.run("setblock 114 -59 114 minecraft:water");
    let wet = summon(&mut p, "minecraft:armor_stand", [114.5, -59.0, 114.5], "{}");
    p.s.deco_tick();
    let _ = p.take_events();
    p.s.deco_push(wet, [0.0, 0.5, 0.0]);
    p.s.deco_tick();
    assert_eq!(motion_packets(&p, wet).len(), 1, "the velocity of a stand in water never went out");
}

// ---------- Reach of explosions and bursts ----------

/// Saved motion of any kept compound (display, marker, interaction).
fn saved_motion(p: &Probe, id: i32) -> [f64; 3] {
    crate::nbtio::motion(p.s.ent_nbt(E::Other(id)).get("Motion")).unwrap_or_default()
}

/// An ordinary explosion pushes every entity, not only mobs: an arrow in flight (marked hurt:
/// its velocity sent back), a block display, a marker and an interaction (their saved motion,
/// from their eyes at their feet or at 0.85 of the interaction's height), an effect cloud, an
/// eye of ender; a burst pushes a shulker bullet without hurting it. Before, none moved.
#[test]
fn explosion_pushes_every_entity() {
    use crate::mobs::explode::Blast;
    let Some((mut p, _dir)) = probe("l10-blast-reach") else { return };
    p.s.pos = [130.5, -59.0, 130.5];
    let center = [110.5, -59.0, 110.5];
    let display = summon(&mut p, "minecraft:block_display", [112.5, -59.0, 110.5], "{}");
    let marker = summon(&mut p, "minecraft:marker", [110.5, -59.0, 112.5], "{}");
    let inter = summon(&mut p, "minecraft:interaction", [108.5, -59.0, 110.5], "{width:1f,height:1f}");
    let arrow_id = p.s.proj_new_id();
    let arrow = Proj::new(PKind::Arrow, arrow_id, [110.5, -58.0, 108.5], st("minecraft:arrow"));
    p.s.proj.list.push(arrow);
    let eye_id = p.s.proj_new_id();
    p.s.proj.list.push(Proj::new(PKind::EyeOfEnder, eye_id, [112.5, -58.0, 112.5], st("minecraft:ender_eye")));
    let cloud_id = p.s.proj_new_id();
    p.s.cloud_add(crate::projectiles::cloud::Cloud::new(cloud_id, [108.5, -59.0, 108.5]));
    p.s.explode_with(None, None, None, center, 2.0, false, "minecraft:explosion", Blast::None(false));
    let want = burst_kb(&p, center, 2.0, 1.0, [112.5, -59.0, 110.5], 0.0, &around([112.5, -59.0, 110.5], 0.0, 0.0));
    assert!(want[0] > 0.0);
    assert_eq!(saved_motion(&p, display), want, "block display");
    assert!(saved_motion(&p, marker)[2] > 0.0, "marker not pushed: {:?}", saved_motion(&p, marker));
    let ip = [108.5, -59.0, 110.5];
    let want = burst_kb(&p, center, 2.0, 1.0, ip, f64::from(1.0f32 * 0.85), &around(ip, 1.0, 1.0));
    assert_eq!(saved_motion(&p, inter), want, "interaction");
    let a = p.s.proj.list.iter().find(|q| q.id == arrow_id).expect("arrow");
    assert!(a.vel[2] < 0.0 && a.sync_velocity, "arrow not pushed nor marked hurt: {:?}", a.vel);
    let e = p.s.proj.list.iter().find(|q| q.id == eye_id).expect("eye");
    assert!(e.vel[0] > 0.0 && !e.sync_velocity, "eye of ender: pushed, never hurt: {:?}", e.vel);
    let c = p.s.proj.clouds.iter().find(|c| c.id == cloud_id).expect("cloud");
    assert!(c.vel[0] < 0.0 && c.vel[2] < 0.0, "cloud not pushed: {:?}", c.vel);
    assert_eq!(crate::nbtio::motion(c.to_nbt().get("Motion")), Some(c.vel), "cloud motion not saved");
    // A burst (no damage) pushes a shulker bullet and does not mark it hurt.
    let bullet_id = p.s.proj_new_id();
    p.s.proj.list.push(Proj::new(PKind::ShulkerBullet, bullet_id, [111.5, -58.5, 110.5], st("minecraft:air")));
    p.s.wind_burst_with(None, None, center, 2.0, 1.0, "minecraft:entity.wind_charge.wind_burst", &mut []);
    let b = p.s.proj.list.iter().find(|q| q.id == bullet_id).expect("bullet");
    assert!(b.vel[0] > 0.0 && !b.sync_velocity, "shulker bullet: {:?}", b.vel);
}

/// An ordinary explosion hurts a mannequin like a living entity (the explosion's damage on its
/// health, its hurt time) and pushes it; an invulnerable armor stand, or a marker, is not broken
/// but still pushed. Before, mannequins were ignored and a stand left standing never moved.
#[test]
fn explosion_hurts_mannequins_and_pushes_standing_stands() {
    use crate::mobs::explode::Blast;
    let Some((mut p, _dir)) = probe("l10-blast-mannequin") else { return };
    p.s.pos = [130.5, -59.0, 130.5];
    let center = [110.5, -59.0, 110.5];
    let mp = [112.5, -59.0, 110.5];
    let man = summon(&mut p, "minecraft:mannequin", mp, "{NoGravity:1b}");
    let hard = summon(&mut p, "minecraft:armor_stand", [110.5, -59.0, 112.5], "{NoGravity:1b,Invulnerable:1b}");
    let mark = summon(&mut p, "minecraft:armor_stand", [108.5, -59.0, 110.5], "{NoGravity:1b,Marker:1b}");
    let b = around(mp, f64::from(0.6f32), f64::from(1.8f32));
    let exposure = f64::from(p.s.seen_percent(center, &b));
    p.s.explode_with(None, None, None, center, 2.0, false, "minecraft:explosion", Blast::None(false));
    let dist = 2.0 / 4.0;
    let pow = (1.0 - dist) * exposure;
    let dmg = ((pow * pow + pow) / 2.0 * 7.0 * 4.0 + 1.0) as f32;
    let n = p.s.deco_nbt(man).cloned().expect("mannequin");
    assert_eq!(n.get("Health"), Some(&rc_proto::nbt::Nbt::Float(20.0 - dmg)), "explosion damage on the mannequin");
    assert_eq!(n.get("HurtTime"), Some(&rc_proto::nbt::Nbt::Short(10)));
    assert_eq!(motion(&p, man), burst_kb(&p, center, 2.0, 1.0, mp, f64::from(1.62f32), &b), "mannequin push");
    assert!(p.s.deco_nbt(hard).is_some() && motion(&p, hard)[2] > 0.0, "invulnerable stand: {:?}", motion(&p, hard));
    assert!(p.s.deco_nbt(mark).is_some() && motion(&p, mark)[0] < 0.0, "marker stand: {:?}", motion(&p, mark));
}

/// The sweep of a sword reaches an armor stand (it wobbles, knocked back along the player's
/// yaw) and a mannequin (hurt, `player_hurt_entity` fired) next to the target; a marker stand is
/// left out. Before, only mobs were swept.
#[test]
fn sweep_reaches_stands_and_mannequins() {
    let Some((mut p, _dir)) = probe("l10-sweep-deco") else { return };
    tree(
        &mut p,
        vec![(
            "l10:hurt_mannequin",
            serde_json::json!({"trigger": "minecraft:player_hurt_entity",
                "conditions": {"entity": {"minecraft:entity_type": "minecraft:mannequin"}}}),
        )],
    );
    p.s.slots[36 + p.s.held as usize] = Some(st("minecraft:diamond_sword"));
    p.s.equipment_tick();
    p.s.on_ground = true;
    p.s.mobs.attack_ticker = 100;
    p.s.life.vit.sprinting = false;
    p.s.life.vit.fall = 0.0;
    p.s.proj_player.known_move = [0.0; 3];
    p.s.time = 100;
    let mut z = tank(&mut p, Kind::Zombie, [110.5, -59.0, 111.5]);
    let stand = summon(&mut p, "minecraft:armor_stand", [111.3, -59.0, 111.5], "{}");
    let man = summon(&mut p, "minecraft:mannequin", [109.7, -59.0, 111.5], "{}");
    let mark = summon(&mut p, "minecraft:armor_stand", [110.5, -59.0, 112.3], "{Marker:1b}");
    let _ = p.take_events();
    p.s.player_attack_mob(&mut z);
    assert!(z.health < 500.0, "main hit without damage");
    let wobble = p.take_events().into_iter().any(|e| {
        matches!(e, rc_client::Event::Entity(rc_client::EntityEvent::Event { id, event: 32 }) if id == stand)
    });
    assert!(wobble, "the swept stand did not wobble");
    assert!(motion(&p, stand)[2] > 0.0, "swept stand not knocked back: {:?}", motion(&p, stand));
    let h = p.s.deco_nbt(man).and_then(|n| n.get("Health").cloned());
    assert!(matches!(h, Some(rc_proto::nbt::Nbt::Float(v)) if v < 20.0), "swept mannequin not hurt: {h:?}");
    assert!(done(&p, "l10:hurt_mannequin"), "the mannequin's hurt did not trigger player_hurt_entity");
    assert_eq!(motion(&p, mark), [0.0; 3], "a marker stand was swept");
}

// ---------- Explosions with a player author ----------

/// The player killed by its own primed TNT is "blown up by" itself (the attacker named: both
/// arguments are the player), counts `killed_by` player and no player kill; with `pvp` off its TNT
/// pushes it without hurting it. Before, the key went out with the victim alone.
#[test]
fn own_tnt_names_the_player_and_follows_pvp() {
    use crate::mobs::mob::Ent;
    use rc_proto::nbt::Nbt;
    let Some((mut p, _dir)) = probe("l10-own-tnt") else { return };
    let center = [112.5, -59.0, 110.5];
    crate::nbtio::set(&mut p.s.rules, "minecraft:pvp", Nbt::Byte(0));
    p.s.mobs.player_vel = [0.0; 3];
    p.s.tnt_explode(-1, center, 4.0, Some(Ent::Player), false, false);
    assert_eq!(p.s.life.vit.health, 20.0, "own TNT hurt the player with pvp off");
    assert!(p.s.mobs.player_vel[0] < 0.0, "own TNT did not push: {:?}", p.s.mobs.player_vel);
    crate::nbtio::set(&mut p.s.rules, "minecraft:pvp", Nbt::Byte(1));
    p.s.life.vit.health = 1.0;
    let _ = p.take_events();
    p.s.tnt_explode(-1, center, 4.0, Some(Ent::Player), false, false);
    let m = p
        .take_events()
        .into_iter()
        .find_map(|e| match e {
            rc_client::Event::Died { message: Some(m), .. } => Some(m),
            _ => None,
        })
        .expect("no death");
    let rc_text::Content::Translate { key, with, .. } = &m.content else { panic!("{m:?}") };
    assert_eq!(key, "death.attack.explosion.player");
    assert_eq!(with.len(), 2, "{m:?}");
    assert_eq!(format!("{:?}", with[0]), format!("{:?}", with[1]), "the attacker is not the player itself");
    assert_eq!(p.s.stat_value("minecraft:killed_by", "minecraft:player"), 1, "killed_by player");
    assert_eq!(p.s.stat_value("minecraft:custom", "minecraft:player_kills"), 0, "credited with its own death");
}

/// A mannequin hurt and killed by the player's explosion emits its damage and death events itself
/// (no player behind them: a shrieker would not answer), and its death credits the player:
/// `killed` mannequin, `mob_kills`, `totalKillCount`, `player_killed_entity`. Before, both events
/// came from the player and the kill counted nowhere.
#[test]
fn mannequin_events_are_its_own_and_its_kill_is_credited() {
    use crate::mobs::explode::Blast;
    use crate::mobs::mob::Ent;
    use crate::mobs::recent::vibration::Src;
    let Some((mut p, _dir)) = probe("l10-mannequin-kill") else { return };
    p.s.pos = [130.5, -59.0, 130.5];
    tree(
        &mut p,
        vec![(
            "l10:kill_mannequin",
            serde_json::json!({"trigger": "minecraft:player_killed_entity",
                "conditions": {"entity": {"minecraft:entity_type": "minecraft:mannequin"}}}),
        )],
    );
    let mut src = p.s.player_source();
    src.level = 4;
    assert!(p.s.perform(src, "scoreboard objectives add total totalKillCount").0);
    let mp = [112.5, -59.0, 110.5];
    summon(&mut p, "minecraft:mannequin", mp, "{NoGravity:1b,Health:1f}");
    p.s.mobs.rec.vib.log.clear();
    let center = [110.5, -59.0, 110.5];
    p.s.explode_with(None, Some(Ent::Player), None, center, 2.0, false, "minecraft:player_explosion", Blast::None(false));
    for ev in ["minecraft:entity_damage", "minecraft:entity_die"] {
        let srcs: Vec<_> = p.s.mobs.rec.vib.log.iter().filter(|l| l.0 == ev && l.1 == mp).map(|l| l.2).collect();
        assert_eq!(srcs, vec![Some(Src::Other)], "{ev}");
    }
    assert_eq!(p.s.stat_value("minecraft:killed", "minecraft:mannequin"), 1, "killed stat");
    assert_eq!(p.s.stat_value("minecraft:custom", "minecraft:mob_kills"), 1, "mob_kills");
    let name = p.s.name.clone();
    assert_eq!(p.s.cmd.scores.get(&name, "total"), Some(1), "totalKillCount");
    assert!(done(&p, "l10:kill_mannequin"), "player_killed_entity");
}

/// A weightless armor stand cancels its small speeds each tick like any living entity
/// (|v| < 0.003): its saved motion drops to zero and nothing residual is sent. Before, the
/// residual motion stayed saved for good and went out with the tracking.
#[test]
fn still_stand_cancels_small_speeds() {
    let Some((mut p, _dir)) = probe("l10-stand-small") else { return };
    let id = summon(&mut p, "minecraft:armor_stand", [112.5, -59.0, 110.5], "{NoGravity:1b}");
    p.s.deco_tick();
    let _ = p.take_events();
    p.s.deco_push(id, [0.002, 0.0, -0.001]);
    for _ in 0..3 {
        p.s.deco_tick();
    }
    assert_eq!(motion(&p, id).map(f64::to_bits), [0.0f64; 3].map(f64::to_bits), "residual motion kept");
    assert!(motion_packets(&p, id).iter().all(|v| *v == [0.0; 3]), "residual velocity sent");
}

/// The bobber's tracking sends its velocity only when it changed (a burst that does not push it
/// sends nothing), and an ordinary explosion marks it hurt: its velocity goes out as is after the
/// tracking, even unpushed. Before, every tracking sent it and the hurt was never seen.
#[test]
fn bobber_velocity_tracked_and_sent_when_hurt() {
    let Some((mut p, _dir)) = probe("l10-bobber") else { return };
    p.s.slots[36] = Some(st("minecraft:fishing_rod"));
    p.s.held = 0;
    assert!(p.s.fishing_rod_use(0), "rod cast");
    let h = p.s.own_hook_mut().expect("bobber");
    h.p.pos = [111.5, -59.0, 110.5];
    h.p.vel = [0.0; 3];
    let id = h.p.id;
    for _ in 0..10 {
        p.s.own_hook_tick();
    }
    assert_eq!(p.s.own_hook().expect("bobber").p.vel, [0.0; 3], "bobber not at rest");
    let _ = p.take_events();
    let center = [110.5, -59.0, 110.5];
    p.s.explosion_projectiles(center, 4.0, None, 0.0, false);
    p.s.own_hook_tick();
    assert!(motion_packets(&p, id).is_empty(), "unchanged velocity sent by the tracking");
    p.s.explosion_projectiles(center, 4.0, None, 0.0, true);
    p.s.own_hook_tick();
    assert_eq!(motion_packets(&p, id), vec![[0.0; 3]], "hurt bobber's velocity not sent");
}

/// Bench (ignored): cost of the new explosion passes and of the still stands' tracking.
/// `cargo test --release -p rc-sim bench_l10_combat -- --ignored --nocapture`.
#[test]
#[ignore]
fn bench_l10_combat() {
    use rc_proto::nbt::Nbt;
    let Some((mut p, _dir)) = probe("l10-bench") else { return };
    p.s.pos = [130.5, -59.0, 130.5];
    let time = |p: &mut Probe, n: u32, f: &dyn Fn(&mut Probe)| {
        let t = std::time::Instant::now();
        for _ in 0..n {
            f(p);
        }
        t.elapsed().as_secs_f64() * 1e6 / f64::from(n)
    };
    let center = [110.5, -59.0, 110.5];
    let still = |p: &mut Probe| p.s.blast_push_still(center, 4.0, 1.0);
    let projs = |p: &mut Probe| p.s.explosion_projectiles(center, 4.0, None, 1.0, true);
    let empty = (time(&mut p, 200, &still), time(&mut p, 200, &projs));
    // 4,000 kept compounds out of range (markers spread over 400 chunks) and 200 arrows.
    for k in 0..4000 {
        let c = (20 + (k % 20), 20 + (k / 20) % 20);
        let pos = [f64::from(c.0 * 16) + 0.5, -59.0, f64::from(c.1 * 16) + 0.5];
        let n = Nbt::Compound(vec![
            ("id".into(), Nbt::String("minecraft:marker".into())),
            ("Pos".into(), Nbt::List(pos.iter().map(|&v| Nbt::Double(v)).collect())),
        ]);
        p.s.entities.entry(c).or_default().push((3_000_000 + k, n));
    }
    for k in 0..200 {
        let id = p.s.proj_new_id();
        let pos = [300.5 + f64::from(k % 20), -58.0, 300.5 + f64::from(k / 20)];
        p.s.proj.list.push(Proj::new(PKind::Arrow, id, pos, st("minecraft:arrow")));
    }
    let full = (time(&mut p, 200, &still), time(&mut p, 200, &projs));
    eprintln!("still pass: 0 compounds {:.2} us, 4000 far {:.2} us; projectiles: 0 {:.2} us, 200 far {:.2} us", empty.0, full.0, empty.1, full.1);
    // Deco tick: 400 weightless stands (tracking only) against 400 stands with gravity.
    let tick = |p: &mut Probe| p.s.deco_tick();
    let base = time(&mut p, 50, &tick);
    for i in 0..400 {
        let (x, z) = (100.5 + f64::from(i % 20) * 1.5, 100.5 + f64::from(i / 20) * 1.5);
        summon(&mut p, "minecraft:armor_stand", [x, -59.0, z], "{NoGravity:1b,equipment:{head:{id:\"minecraft:iron_helmet\",count:1}}}");
    }
    let weightless = time(&mut p, 50, &tick);
    eprintln!("deco tick: empty {base:.1} us, 400 weightless stands with a helmet {weightless:.1} us");
}

// ---------- Vector normalization (the game's threshold: 1e-5 in single precision) ----------

/// The common normalization: zero only below 1e-5 widened from single precision.
#[test]
fn normalize_threshold_is_single_precision() {
    use crate::mobs::mth::normalize;
    let just_above = 9.999_999_9e-6; // above 1e-5f widened, below 1e-5
    assert!(just_above > f64::from(1.0e-5f32) && just_above < 1.0e-5);
    assert_eq!(normalize([just_above, 0.0, 0.0]), [1.0, 0.0, 0.0]);
    assert_eq!(normalize([9.999_99e-6, 0.0, 0.0]), [0.0; 3]);
    assert_eq!(normalize([5.0e-5, 0.0, 0.0]), [1.0, 0.0, 0.0]);
}

/// A shot along a direction just above the game's threshold keeps its power (the local threshold
/// of 1e-5 in double precision zeroed it).
#[test]
fn shot_direction_just_above_threshold() {
    let mut a = Proj::new(PKind::Arrow, 900_102, [0.0; 3], st("minecraft:arrow"));
    a.shoot([9.999_999_9e-6, 0.0, 0.0], 2.0, 0.0);
    assert_eq!(a.vel, [2.0, 0.0, 0.0]);
}

/// A burst whose centre lies 5e-5 from a wind charge still redirects it along that offset, like
/// the game (the local threshold of 1e-4 left it in place).
#[test]
fn burst_redirects_charge_close_to_centre() {
    let Some((mut p, _dir)) = probe("l10-redirect") else { return };
    let center = [110.5, -58.0, 110.5];
    let mut c = Proj::new(PKind::WindCharge, 900_103, [center[0] + 5.0e-5, center[1], center[2]], st("minecraft:wind_charge"));
    c.vel = [0.0; 3];
    p.s.proj.list.push(c);
    p.s.explosion_projectiles(center, 1.2, None, 1.22, false);
    let v = p.s.proj.list.iter().find(|q| q.id == 900_103).expect("charge").vel;
    assert!(v[0] > 1.0 && v[1] == 0.0 && v[2] == 0.0, "charge not pushed along its offset: {v:?}");
}

/// The common item throw of a mob (villager, piglin): 0.3 in single precision under the eyes and
/// for the speed, along the unit vector even for a target 5e-5 away.
#[test]
fn mob_item_throw_in_single_precision() {
    let Some((mut p, _dir)) = probe("l10-throw") else { return };
    let v = p.s.new_mob(Kind::Villager, [110.5, -59.0, 112.5], 0.0);
    let k = f64::from(0.3f32);
    let before = p.s.life.drops.len();
    p.s.throw_item(&v, st("minecraft:bread"), [110.5, -59.0, 115.5]);
    let d = p.s.life.drops.last().expect("thrown item");
    assert_eq!(p.s.life.drops.len(), before + 1);
    assert_eq!(d.vel, [0.0, 0.0, k], "throw speed");
    assert_eq!(d.pos[1], v.eye_y() - k, "throw height");
    p.s.throw_item(&v, st("minecraft:bread"), [110.5 + 5.0e-5, -59.0, 112.5]);
    assert_eq!(p.s.life.drops.last().expect("thrown item").vel, [k, 0.0, 0.0], "close target not thrown at");
}
