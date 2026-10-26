//! Parrot (§ 4.3): flight (flying movement control, flying navigation), taming
//! with seeds, death by cookie, player's shoulder, mob imitation, dance.

use rc_proto::nbt::Nbt;

use super::super::goals::Goal;
use super::super::goals::surface_goals::SGoal;
use super::super::mob::{Mob, MoveOp};
use super::super::mth;
use super::super::species::Kind;
use crate::server::Server;

/// Flight speed (`flying_speed`).
const FLYING_SPEED: f64 = 0.4f32 as f64;

/// Imitated sounds (order of the game's table; the game's real iteration order depends on the hash).
pub const IMITATED: [(&str, &str); 42] = [
    ("minecraft:blaze", "blaze"),
    ("minecraft:bogged", "bogged"),
    ("minecraft:breeze", "breeze"),
    ("minecraft:camel_husk", "camel_husk"),
    ("minecraft:cave_spider", "spider"),
    ("minecraft:creaking", "creaking"),
    ("minecraft:creeper", "creeper"),
    ("minecraft:drowned", "drowned"),
    ("minecraft:elder_guardian", "elder_guardian"),
    ("minecraft:ender_dragon", "ender_dragon"),
    ("minecraft:endermite", "endermite"),
    ("minecraft:evoker", "evoker"),
    ("minecraft:ghast", "ghast"),
    ("minecraft:happy_ghast", ""),
    ("minecraft:guardian", "guardian"),
    ("minecraft:hoglin", "hoglin"),
    ("minecraft:husk", "husk"),
    ("minecraft:illusioner", "illusioner"),
    ("minecraft:magma_cube", "magma_cube"),
    ("minecraft:parched", "parched"),
    ("minecraft:phantom", "phantom"),
    ("minecraft:piglin", "piglin"),
    ("minecraft:piglin_brute", "piglin_brute"),
    ("minecraft:pillager", "pillager"),
    ("minecraft:ravager", "ravager"),
    ("minecraft:shulker", "shulker"),
    ("minecraft:silverfish", "silverfish"),
    ("minecraft:skeleton", "skeleton"),
    ("minecraft:slime", "slime"),
    ("minecraft:spider", "spider"),
    ("minecraft:stray", "stray"),
    ("minecraft:vex", "vex"),
    ("minecraft:vindicator", "vindicator"),
    ("minecraft:warden", "warden"),
    ("minecraft:witch", "witch"),
    ("minecraft:wither", "wither"),
    ("minecraft:wither_skeleton", "wither_skeleton"),
    ("minecraft:zoglin", "zoglin"),
    ("minecraft:zombie", "zombie"),
    ("minecraft:zombie_horse", "zombie_horse"),
    ("minecraft:zombie_nautilus", "zombie_nautilus"),
    ("minecraft:zombie_villager", "zombie_villager"),
];

fn imitated_sound(entity: &str) -> Option<String> {
    let (_, s) = IMITATED.iter().find(|(e, _)| *e == entity)?;
    Some(if s.is_empty() { String::new() } else { format!("minecraft:entity.parrot.imitate.{s}") })
}

pub fn register(m: &mut Mob) {
    let g = &mut m.goals;
    g.add(0, super::panic(1.25, "minecraft:panic_causes"));
    g.add(0, Goal::Float);
    g.add(1, super::look_player(8.0));
    g.add(2, Goal::SitOrdered);
    g.add(2, Goal::FollowOwner { speed: 1.0, start: 5.0, stop: 1.0, recalc: 0, old_water: 0.0 });
    g.add(2, Goal::Surf(SGoal::ParrotWander { wanted: [0.0; 3] }));
    g.add(3, Goal::Surf(SGoal::LandOnShoulder { on: false }));
    g.add(3, Goal::Surf(SGoal::FollowMob { following: None, recalc: 0, old_water: 0.0 }));
}

/// Ambient sound (world random): rare imitation outside peaceful.
pub fn ambient(s: &mut Server) -> String {
    if s.level.difficulty_id() != 0 && s.mobs.rng.next_int_bound(1000) == 0 {
        let k = s.mobs.rng.next_int_bound(IMITATED.len() as i32) as usize;
        imitated_sound(IMITATED[k].0).unwrap_or_else(|| "minecraft:entity.parrot.ambient".into())
    } else {
        "minecraft:entity.parrot.ambient".into()
    }
}

/// The sound pitch.
pub fn pitch(r: &mut super::super::jrand::JRand) -> f32 {
    (r.next_float() - r.next_float()) * 0.2 + 1.0
}

/// Imitator: parrot, or player carrying one (alive, not silent, sound category).
pub struct Imitator {
    pub center: [f64; 3],
    pub bbox: rc_physics::Aabb,
    pub alive: bool,
    pub silent: bool,
    pub source: i32,
}

/// The imitation of nearby mobs (world random) around a box; true if a sound was
/// played. Nothing (without a draw) for a dead or silent imitator; a randomly drawn
/// silent mob does nothing (false).
pub fn imitate_nearby(s: &mut Server, who: &Imitator) -> bool {
    if !who.alive || who.silent || s.mobs.rng.next_int_bound(2) != 0 {
        return false;
    }
    let bbox = who.bbox;
    let area = rc_physics::Aabb { min: bbox.min.map(|v| v - 20.0), max: bbox.max.map(|v| v + 20.0) };
    let imitated = |o: &&Mob| IMITATED.iter().any(|(e, _)| *e == o.kind.info().name) && o.touches(&area);
    let n = s.mobs.list.iter().flatten().map(|m| &**m).filter(imitated).count();
    if n == 0 {
        return false;
    }
    let k = s.mobs.rng.next_int_bound(n as i32) as usize;
    let Some(o) = s.mobs.list.iter().flatten().map(|m| &**m).filter(imitated).nth(k) else { return false };
    if super::super::end::silent(o) {
        return false;
    }
    let snd = imitated_sound(o.kind.info().name).unwrap_or_default();
    let p = pitch(&mut s.mobs.rng);
    if !snd.is_empty() {
        s.world_sound(&snd, who.source, who.center, 0.7, p);
    }
    true
}

/// Start of the AI step: imitation. The party near a jukebox exists only client-side
/// (events 1010 / 1011): the server never marks a nearby jukebox.
pub fn pre_ai_step(s: &mut Server, m: &mut Mob) {
    if s.mobs.rng.next_int_bound(400) == 0 {
        let who = Imitator {
            center: m.pos,
            bbox: m.bbox(),
            alive: m.alive(),
            silent: super::super::end::silent(m),
            source: 6,
        };
        imitate_nearby(s, &who);
    }
}

/// End of the AI step: wing flaps and slowed fall.
pub fn post_ai_step(m: &mut Mob) {
    let f = &mut m.surf;
    f.flap_speed += if !m.on_ground { 4.0 } else { -1.0 } * 0.3;
    f.flap_speed = f.flap_speed.clamp(0.0, 1.0);
    if !m.on_ground && f.flapping < 1.0 {
        f.flapping = 1.0;
    }
    f.flapping *= 0.9;
    if !m.on_ground && m.vel[1] < 0.0 {
        m.vel[1] *= 0.6;
    }
}

/// Wing flap: flight sound, next flap.
pub fn on_flap(s: &mut Server, m: &mut Mob) {
    s.mob_sound(m, "minecraft:entity.parrot.fly", 0.15, 1.0);
    m.surf.next_flap = m.surf.fly_dist + m.surf.flap_speed / 2.0;
}

/// Chicken: flap speed (end of the AI step, the slowed fall is in the movement).
pub fn chicken_flaps(m: &mut Mob) {
    let f = &mut m.surf;
    f.flap_speed += if !m.on_ground { 4.0 } else { -1.0 } * 0.3;
    f.flap_speed = f.flap_speed.clamp(0.0, 1.0);
    if !m.on_ground && f.flapping < 1.0 {
        f.flapping = 1.0;
    }
    f.flapping *= 0.9;
}

/// Flying movement control (turn 10, no hover).
pub fn move_control(m: &mut Mob) {
    if m.move_ctl.op == MoveOp::MoveTo {
        m.move_ctl.op = MoveOp::Wait;
        m.surf.no_gravity = true;
        let w = m.move_ctl.wanted;
        let (xd, yd, zd) = (w[0] - m.pos[0], w[1] - m.pos[1], w[2] - m.pos[2]);
        let dd = xd * xd + yd * yd + zd * zd;
        if dd < f64::from(2.500_000_3e-7f32) {
            m.yya = 0.0;
            m.zza = 0.0;
            return;
        }
        let yr = (mth::atan2(zd, xd) * f64::from(180.0f32) / f64::from(std::f32::consts::PI)) as f32 - 90.0;
        m.yaw = mth::rotlerp(m.yaw, yr, 90.0);
        let speed = if m.on_ground {
            (m.move_ctl.speed_mod * m.movement_speed()) as f32
        } else {
            (m.move_ctl.speed_mod * FLYING_SPEED) as f32
        };
        m.speed = speed;
        m.zza = speed;
        let sd = (xd * xd + zd * zd).sqrt();
        if yd.abs() > f64::from(1.0e-5f32) || sd.abs() > f64::from(1.0e-5f32) {
            let xr = -(mth::atan2(yd, sd) * f64::from(180.0f32) / f64::from(std::f32::consts::PI)) as f32;
            m.pitch = mth::rotlerp(m.pitch, xr, 10.0);
            m.yya = if yd > 0.0 { speed } else { -speed };
        }
    } else {
        m.surf.no_gravity = false;
        m.yya = 0.0;
        m.zza = 0.0;
    }
}

pub fn interact(s: &mut Server, m: &mut Mob, slot: usize, item: &str) -> Option<bool> {
    let consume = |s: &mut Server| {
        if s.game_mode != 1
            && let Some(st) = &mut s.slots[slot]
        {
            st.count -= 1;
            if st.count <= 0 {
                s.slots[slot] = None;
            }
        }
    };
    if !m.tame && s.item_tag(item, "minecraft:parrot_food") {
        consume(s);
        // Silent: neither sound nor pitch draws.
        if !m.x.silent {
            let p = 1.0 + (m.rng.next_float() - m.rng.next_float()) * 0.2;
            s.mob_sound(m, "minecraft:entity.parrot.eat", 1.0, p);
        }
        if m.rng.next_int_bound(10) == 0 {
            m.tame = true;
            m.owner = Some(s.uuid);
            s.mob_event(m, 7);
            s.s9_tamed(m); // S9
        } else {
            s.mob_event(m, 6);
        }
        return Some(true);
    }
    if !s.item_tag(item, "minecraft:parrot_poisonous_food") {
        if m.on_ground && m.tame && m.owner == Some(s.uuid) {
            m.sitting = !m.sitting;
            return Some(true);
        }
        return None;
    }
    consume(s);
    // Poison 900 ticks (mob effects: S7), then the player's infinite damage.
    s.give_mob_effect(m, "minecraft:poison", 900, 0);
    s.mob_hurt(m, "minecraft:player_attack", f32::MAX, Some(super::super::mob::Ent::Player), Some(s.pos));
    Some(true)
}

/// Tick of the tamed panic: joins its owner if it is too far.
pub fn panic_tick(s: &mut Server, m: &mut Mob) {
    let owner_here = m.tame && m.owner == Some(s.uuid) && s.game_mode != 3;
    if m.sitting || !owner_here {
        return;
    }
    if mth::dist2(m.pos, s.pos) >= 144.0 {
        s.teleport_to_owner_pub(m);
    }
}

// ---------- Player's shoulder ----------

/// The parrot lands on the shoulder: saved in the player.
pub fn set_on_shoulder(s: &mut Server, m: &mut Mob) -> bool {
    if !s.on_ground || s.player_in_water() {
        return false;
    }
    let side = if s.mobs.shoulder[0].is_none() {
        0
    } else if s.mobs.shoulder[1].is_none() {
        1
    } else {
        return false;
    };
    let mut tag = m.save();
    if let Nbt::Compound(f) = &mut tag {
        f.retain(|(k, _)| k != "id");
        f.push(("id".into(), Nbt::String("minecraft:parrot".into())));
    }
    s.mobs.shoulder[side] = Some(tag);
    s.mobs.shoulder_time = s.time;
    send_shoulders(s);
    m.removed = true;
    true
}

/// Player metadata (19 left, 20 right: parrot variant).
pub fn send_shoulders(s: &Server) {
    let v = |t: &Option<Nbt>| t.as_ref().map(|t| crate::nbtio::int(t, "Variant").map_or(0, |v| (v as i32).clamp(0, 4)));
    let values = vec![
        (19, rc_proto_v777::entity::Meta::OptionalInt(v(&s.mobs.shoulder[0]))),
        (20, rc_proto_v777::entity::Meta::OptionalInt(v(&s.mobs.shoulder[1]))),
    ];
    let _ = s.send(rc_client::Event::Entity(rc_client::EntityEvent::Data { id: s.player_id, values }));
}

/// Tick of the carrying player: sounds, parrots' fall.
pub fn player_tick(s: &mut Server) {
    if !s.mobs.shoulder_loaded {
        s.mobs.shoulder_loaded = true;
        load_shoulders(s);
        if s.mobs.shoulder.iter().any(Option::is_some) {
            send_shoulders(s);
        }
    }
    if s.mobs.shoulder.iter().all(Option::is_none) {
        return;
    }
    for side in 0..2 {
        let Some(t) = s.mobs.shoulder[side].clone() else { continue };
        if crate::nbtio::int(&t, "Silent").is_some_and(|v| v != 0) {
            continue;
        }
        if s.mobs.rng.next_int_bound(200) == 0 {
            let pos = s.pos;
            // The carrying player imitates (never silent in singleplayer), players category.
            let who = Imitator { center: pos, bbox: s.player_aabb(), alive: true, silent: false, source: 7 };
            if !imitate_nearby(s, &who) {
                let snd = ambient(s);
                let p = pitch(&mut s.mobs.rng);
                s.world_sound(&snd, 7, pos, 1.0, p);
            }
        }
    }
    let flying = s.abilities().flying;
    if s.life.vit.fall > 0.5
        || s.player_in_water()
        || flying
        || s.life.vit.sleeping.is_some()
        || s.player_in_powder_snow()
        || s.game_mode == 3
        || s.life.vit.dead
    {
        remove_from_shoulders(s);
    }
}

/// The parrots come back to life above the player (after 20 ticks).
pub fn remove_from_shoulders(s: &mut Server) {
    if s.mobs.shoulder_time + 20 >= s.time || s.mobs.shoulder.iter().all(Option::is_none) {
        return;
    }
    for side in 0..2 {
        let Some(tag) = s.mobs.shoulder[side].take() else { continue };
        let mut m = s.new_mob(Kind::Parrot, s.pos, 0.0);
        let id = m.id;
        m.load(&tag);
        m.id = id;
        m.tame = true;
        m.owner = Some(s.uuid);
        m.pos = [s.pos[0], s.pos[1] + f64::from(0.7f32), s.pos[2]];
        m.old = m.pos;
        s.init_mob(&mut m);
        s.add_mob(m);
    }
    send_shoulders(s);
}

/// Reading the player file (`ShoulderEntityLeft` / `ShoulderEntityRight`).
pub fn load_shoulders(s: &mut Server) {
    for (i, k) in ["ShoulderEntityLeft", "ShoulderEntityRight"].iter().enumerate() {
        s.mobs.shoulder[i] = s.pdata.root.get(k).filter(|t| matches!(t, Nbt::Compound(f) if !f.is_empty())).cloned();
    }
}

/// Writing into the player file.
pub fn save_shoulders(s: &Server, root: &mut Nbt) {
    for (i, k) in ["ShoulderEntityLeft", "ShoulderEntityRight"].iter().enumerate() {
        match &s.mobs.shoulder[i] {
            Some(t) => crate::nbtio::set(root, k, t.clone()),
            None => crate::nbtio::remove(root, k),
        }
    }
}
