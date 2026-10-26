//! Llama: spit (`llama_spit` projectile, gravity 0.06, friction 0.99, 1 damage, does not
//! disappear when hitting an entity), caravan behind a leashed llama, trader llama
//! countdown.

use rc_client::{EntityEvent, Event};
use rc_proto_v777::entity::{AddEntity, PositionPath, PositionSync};

use super::super::arrows::ray_t;
use super::super::jrand::JRand;
use super::super::mob::{Ent, Mob};
use super::super::mth::{DEG, atan2, cos, dist2, sin};
use super::super::species::Kind;
use super::is_llama;
use crate::server::Server;

#[derive(Debug, Clone)]
pub struct Spit {
    pub id: i32,
    pub owner: i32,
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub rng: JRand,
    pub removed: bool,
    pub left_owner: bool,
    pub tick: i32,
}

/// Mounts world: mounted player, spits in flight, visual lightning.
#[derive(Debug, Clone, Default)]
pub struct World {
    pub riding: super::riding::PlayerRide,
    pub spits: Vec<Spit>,
    pub bolts: Vec<super::trap::Bolt>,
}

fn rot_of(v: [f64; 3]) -> (f32, f32) {
    let k = f64::from(180.0f32 / std::f32::consts::PI);
    let yaw = (atan2(v[0], v[2]) * k) as f32;
    let pitch = (atan2(v[1], (v[0] * v[0] + v[2] * v[2]).sqrt()) * k) as f32;
    (yaw, pitch)
}

fn spit_box(p: [f64; 3]) -> rc_physics::Aabb {
    rc_physics::Aabb { min: [p[0] - 0.125, p[1], p[2] - 0.125], max: [p[0] + 0.125, p[1] + 0.25, p[2] + 0.125] }
}

impl Server {
    /// Start in front of the mouth, aim at a third of the target's height,
    /// speed 1.5, inaccuracy 10; spit sound (llama's random).
    pub(crate) fn llama_spit(&mut self, m: &mut Mob, target: Ent) {
        let (Some(tp), Some(tb)) = (self.ent_pos(target), self.ent_bbox(target)) else { return };
        self.next_entity += 1;
        let id = self.next_entity;
        let mut rng = JRand::from_time(i64::from(id));
        let reach = f64::from(m.width() + 1.0) * 0.5;
        let start = [
            m.pos[0] - reach * f64::from(sin(m.body_yaw * DEG)),
            m.eye_y() - f64::from(0.1f32),
            m.pos[2] + reach * f64::from(cos(m.body_yaw * DEG)),
        ];
        let xd = tp[0] - m.pos[0];
        let yd = tb.min[1] + (tb.max[1] - tb.min[1]) * 0.333_333_333_333_333_3 - start[1];
        let zd = tp[2] - m.pos[2];
        let yo = (xd * xd + zd * zd).sqrt() * f64::from(0.2f32);
        let dir = [xd, yd + yo, zd];
        let l = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
        let mut v = dir.map(|c| if l < 1.0e-4 { 0.0 } else { c / l });
        for c in &mut v {
            *c += rng.triangle(0.0, 0.017_227_5 * 10.0);
        }
        let v = v.map(|c| c * f64::from(1.5f32));
        let (yaw, pitch) = rot_of(v);
        let s =
            Spit { id, owner: m.id, pos: start, vel: v, yaw, pitch, rng, removed: false, left_owner: false, tick: 0 };
        let add = AddEntity {
            id,
            uuid: (id as u128) << 64 | 0x4C00_0000_0000_0000,
            kind: self.entity_kind_id("minecraft:llama_spit"),
            pos: start,
            velocity: super::super::track::lp_quantize(v),
            pitch,
            yaw,
            head_yaw: 0.0,
            data: m.id,
        };
        let _ = self.send(Event::Entity(EntityEvent::Add(Box::new(add))));
        self.mobs.mounts.spits.push(s);
        let p = 1.0 + (m.rng.next_float() - m.rng.next_float()) * 0.2;
        self.mount_sound(m, "minecraft:entity.llama.spit", 1.0, p);
        m.mount.did_spit = true;
    }

    /// Spit tick (after the mobs); position sent back every 10 ticks.
    pub(crate) fn spits_tick(&mut self) {
        let mut spits = std::mem::take(&mut self.mobs.mounts.spits);
        for s in spits.iter_mut().filter(|s| !s.removed) {
            s.tick += 1;
            // The spit can only hit its llama after leaving it.
            if !s.left_owner {
                s.left_owner = self.mob_ref(s.owner).is_none_or(|o| !o.touches(&spit_box(s.pos)));
            }
            let mv = s.vel;
            let from = s.pos;
            let to = [from[0] + mv[0], from[1] + mv[1], from[2] + mv[2]];
            let block_t = self.clip_t(from, to);
            let mut end = match block_t {
                Some(t) => [from[0] + mv[0] * t, from[1] + mv[1] * t, from[2] + mv[2] * t],
                None => to,
            };
            // Starting from the inside, a path that would end outside the
            // border touches it at the point brought back inside (block hit, spit removed).
            let b = self.border_bounds();
            let inside = |p: [f64; 3]| p[0] >= b[0] && p[0] < b[1] && p[2] >= b[2] && p[2] < b[3];
            let mut block_hit = block_t.is_some();
            if inside(from) && !inside(end) {
                let e = f64::from(1.0e-5f32);
                end = [end[0].clamp(b[0], b[1] - e), end[1], end[2].clamp(b[2], b[3] - e)];
                block_hit = true;
            }
            let seg = [end[0] - from[0], end[1] - from[1], end[2] - from[2]];
            let mut hit: Option<(f64, Ent)> = None;
            for o in self.mobs.list.iter().flatten() {
                if !o.alive() || (o.id == s.owner && !s.left_owner) {
                    continue;
                }
                let bb = o.bbox();
                let g = rc_physics::Aabb { min: bb.min.map(|v| v - 0.3), max: bb.max.map(|v| v + 0.3) };
                if ray_t(&g, from, seg).is_some() {
                    let d = dist2(from, o.pos);
                    if hit.is_none_or(|h| d < h.0) {
                        hit = Some((d, Ent::Mob(o.id)));
                    }
                }
            }
            if self.game_mode != 3 && self.ent_alive(Ent::Player) {
                let bb = self.player_aabb();
                let g = rc_physics::Aabb { min: bb.min.map(|v| v - 0.3), max: bb.max.map(|v| v + 0.3) };
                if ray_t(&g, from, seg).is_some() {
                    let d = dist2(from, self.pos);
                    if hit.is_none_or(|h| d < h.0) {
                        hit = Some((d, Ent::Player));
                    }
                }
            }
            if let Some((_, e)) = hit {
                self.spit_hit(s, e);
            } else if block_hit {
                s.removed = true;
                continue;
            }
            let next = to;
            s.pitch = rot_of(mv).1;
            s.yaw = rot_of(mv).0;
            let air_only = {
                let b = spit_box(s.pos);
                let lo = b.min.map(|v| v.floor() as i32);
                let hi = b.max.map(|v| v.floor() as i32);
                let mut ok = false;
                for x in lo[0]..=hi[0] {
                    for y in lo[1]..=hi[1] {
                        for z in lo[2]..=hi[2] {
                            if self.is_air([x, y, z]) {
                                ok = true;
                            }
                        }
                    }
                }
                ok
            };
            if !air_only || self.mob_fluid(super::super::mth::block_of(s.pos)) == super::super::env::Fluid::Water {
                s.removed = true;
                continue;
            }
            s.vel = s.vel.map(|v| v * f64::from(0.99f32));
            s.vel[1] -= 0.06;
            s.pos = next;
            if s.tick % 10 == 0 {
                let p = PositionSync {
                    id: s.id,
                    path: PositionPath { linear: true, steps: vec![(s.pos, 0)] },
                    yaw: s.yaw,
                    pitch: s.pitch,
                    on_ground: false,
                };
                let _ = self.send(Event::Entity(EntityEvent::Sync(p)));
            }
            if s.tick > 1200 {
                s.removed = true;
            }
        }
        let gone: Vec<i32> = spits.iter().filter(|s| s.removed).map(|s| s.id).collect();
        if !gone.is_empty() {
            let _ = self.send(Event::Entity(EntityEvent::Remove(gone)));
        }
        spits.retain(|s| !s.removed);
        let added = std::mem::take(&mut self.mobs.mounts.spits);
        spits.extend(added);
        self.mobs.mounts.spits = spits;
    }

    /// Spit on an entity: 1 damage (`spit`), the spit continues on its way.
    fn spit_hit(&mut self, s: &Spit, e: Ent) {
        let owner = self.mob_index(s.owner).and_then(|i| self.mobs.list[i].as_ref().map(|m| (**m).clone()));
        let Some(owner) = owner else { return };
        // Projectile: the knockback follows its velocity (opposite, `xd`, `zd` of the game), not its position.
        let kb = (-s.vel[0], -s.vel[2]);
        match e {
            Ent::Player => {
                self.player_hurt_by_mob(&owner, "minecraft:spit", 1.0, Some(kb));
            }
            Ent::Mob(id) => {
                let Some(i) = self.mob_index(id) else { return };
                let Some(mut o) = self.mobs.list[i].take() else { return };
                self.mob_hurt_dir(&mut o, "minecraft:spit", 1.0, Some(Ent::Mob(s.owner)), Some(s.pos), Some(kb));
                self.mobs.list[i] = Some(o);
            }
        }
    }

    // ---------- Caravan ----------

    fn llama_leashed(m: &Mob) -> bool {
        // Leash not simulated in singleplayer: never held.
        let _ = m;
        false
    }

    /// First of the caravan held on a leash, at most 8 links away.
    fn first_is_leashed(&self, m: &Mob, mut counter: i32) -> bool {
        let mut cur = m.mount.caravan_head;
        loop {
            if counter > 8 {
                return false;
            }
            let Some(h) = cur.and_then(|id| self.mob_ref(id)) else { return false };
            if Self::llama_leashed(h) {
                return true;
            }
            counter += 1;
            cur = h.mount.caravan_head;
        }
    }

    pub(crate) fn caravan_can_use(&mut self, m: &mut Mob) -> bool {
        if Self::llama_leashed(m) || m.mount.caravan_head.is_some() {
            return false;
        }
        let bb = m.bbox();
        let area = rc_physics::Aabb {
            min: [bb.min[0] - 9.0, bb.min[1] - 4.0, bb.min[2] - 9.0],
            max: [bb.max[0] + 9.0, bb.max[1] + 4.0, bb.max[2] + 9.0],
        };
        let near: Vec<(i32, f64, bool, bool, bool)> = self
            .mobs
            .list
            .iter()
            .flatten()
            .filter(|o| o.id != m.id && is_llama(o.kind) && o.touches(&area))
            .map(|o| {
                (
                    o.id,
                    dist2(m.pos, o.pos),
                    o.mount.caravan_head.is_some(),
                    o.mount.caravan_tail.is_some(),
                    Self::llama_leashed(o),
                )
            })
            .collect();
        let mut best: Option<(i32, f64)> = None;
        for &(id, d, in_caravan, tail, _) in &near {
            if in_caravan && !tail && best.is_none_or(|b| d <= b.1) {
                best = Some((id, d));
            }
        }
        if best.is_none() {
            for &(id, d, _, tail, leashed) in &near {
                if leashed && !tail && best.is_none_or(|b| d <= b.1) {
                    best = Some((id, d));
                }
            }
        }
        let Some((id, d)) = best else { return false };
        if d < 4.0 {
            return false;
        }
        let Some(c) = self.mob_ref(id) else { return false };
        if !Self::llama_leashed(c) && !self.first_is_leashed(c, 1) {
            return false;
        }
        m.mount.caravan_head = Some(id);
        if let Some(i) = self.mob_index(id)
            && let Some(o) = self.mobs.list[i].as_mut()
        {
            o.mount.caravan_tail = Some(m.id);
        }
        true
    }

    pub(crate) fn caravan_can_continue(&mut self, m: &mut Mob, speed: &mut f64, dist_check: &mut i32) -> bool {
        let Some(head) = m.mount.caravan_head.and_then(|id| self.mob_ref(id)).map(|h| (h.alive(), h.pos)) else {
            return false;
        };
        if !head.0 || !self.first_is_leashed(m, 0) {
            return false;
        }
        if dist2(m.pos, head.1) > 676.0 {
            if *speed <= 3.0 {
                *speed *= 1.2;
                *dist_check = super::super::mth::reduced(40);
                return true;
            }
            if *dist_check == 0 {
                return false;
            }
        }
        if *dist_check > 0 {
            *dist_check -= 1;
        }
        true
    }

    pub(crate) fn caravan_tick(&mut self, m: &mut Mob, speed: f64) {
        let Some(h) = m.mount.caravan_head.and_then(|id| self.mob_ref(id)).map(|h| h.pos) else { return };
        let d = [h[0] - m.pos[0], h[1] - m.pos[1], h[2] - m.pos[2]];
        let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let n = if dist < 1.0e-4 { [0.0; 3] } else { d.map(|c| c / dist) };
        let k = (dist - 2.0).max(0.0);
        let to = [m.pos[0] + n[0] * k, m.pos[1] + n[1] * k, m.pos[2] + n[2] * k];
        self.move_to_pos(m, to, speed);
    }

    /// The caravan's exit.
    pub(crate) fn leave_caravan(&mut self, m: &mut Mob) {
        if let Some(h) = m.mount.caravan_head.take()
            && let Some(i) = self.mob_index(h)
            && let Some(o) = self.mobs.list[i].as_mut()
        {
            o.mount.caravan_tail = None;
        }
    }
}

/// Trader llama: despawn countdown (suspended if tamed, ridden by a player, age-locked or
/// persistent).
pub fn maybe_despawn(s: &Server, m: &mut Mob) {
    if m.kind != Kind::TraderLlama {
        return;
    }
    let one_player = m.mount.passengers.len() == 1 && m.mount.passengers[0] == Ent::Player;
    let can = !super::is_tamed(m) && !one_player && !m.mount.age_locked && !m.persistent;
    let _ = s;
    if can {
        m.mount.despawn_delay -= 1;
        if m.mount.despawn_delay <= 0 {
            m.removed = true;
        }
    }
}
