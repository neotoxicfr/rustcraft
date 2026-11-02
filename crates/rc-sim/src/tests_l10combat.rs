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

/// A weightless armor stand pushed by a hair (1e-4) sends no velocity: its tracking compares with
/// the last velocity sent (1e-7 squared), like the game; a real push is sent by its next tick.
/// A stand with gravity standing in water (not moved by us) still sends its pushed velocity.
/// Before, the weightless stand sent every push at once and the one in water never.
#[test]
fn still_stand_velocity_goes_through_tracking() {
    let Some((mut p, _dir)) = probe("l10-stand-sync") else { return };
    let still = summon(&mut p, "minecraft:armor_stand", [112.5, -59.0, 110.5], "{NoGravity:1b}");
    p.s.deco_tick();
    let _ = p.take_events();
    p.s.deco_push(still, [1.0e-4, 0.0, 0.0]);
    p.s.deco_tick();
    assert!(motion_packets(&p, still).is_empty(), "a push below the tracking threshold was sent");
    p.s.deco_push(still, [0.25, 0.0, 0.0]);
    assert!(motion_packets(&p, still).is_empty(), "sent at once, outside the tracking");
    p.s.deco_tick();
    assert_eq!(motion_packets(&p, still), vec![crate::mobs::track::lp_quantize([0.2501, 0.0, 0.0])], "push not sent by the tracking");
    // In water: no movement simulated, the tracking runs.
    p.run("setblock 114 -59 114 minecraft:water");
    let wet = summon(&mut p, "minecraft:armor_stand", [114.5, -59.0, 114.5], "{}");
    p.s.deco_tick();
    let _ = p.take_events();
    p.s.deco_push(wet, [0.0, 0.5, 0.0]);
    p.s.deco_tick();
    assert_eq!(motion_packets(&p, wet).len(), 1, "the velocity of a stand in water never went out");
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
    p.s.explosion_redirect(center, 1.2, None, 1.22);
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
