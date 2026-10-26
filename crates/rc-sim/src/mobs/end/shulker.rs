//! Shulker (F15): attached to a block face, immobile (position snapped to the block center),
//! opening 0..100 that grows the box outward and pushes entities, armor + 20 when it closes,
//! homing bullet shooting, teleportation (5 tries at ±8), duplication when a bullet hits it
//! open, gaze computed in the plane of the attach face.

use super::super::goals::{Goal, TBase, Who};
use super::super::mob::{Ent, Mob};
use super::super::mth::{atan2, rotate_towards, sin};
use super::super::species::Kind;
use super::goals::EGoal;
use crate::blocks::Dir;
use crate::server::Server;

/// Step of a direction (game order: down, up, north, south, west, east).
pub fn step(d: i32) -> [i32; 3] {
    Dir::from_index(d as usize).step()
}

/// Opposite direction.
pub fn opposite(d: i32) -> i32 {
    d ^ 1
}

/// Physical opening: 0.5 − sin((0.5 + a)·π)·0.5.
pub fn physical_peek(a: f32) -> f32 {
    0.5 - sin((0.5 + a) * std::f32::consts::PI) * 0.5
}

/// Box between two openings (size 1).
#[allow(clippy::needless_range_loop)]
pub fn progress_delta_box(dir: i32, from: f32, to: f32, pos: [f64; 3]) -> rc_physics::Aabb {
    let s = step(dir).map(f64::from);
    let (max, min) = (f64::from(from.max(to)), f64::from(from.min(to)));
    let mut b = rc_physics::Aabb { min: [-0.5, 0.0, -0.5], max: [0.5, 1.0, 0.5] };
    for k in 0..3 {
        let e = s[k] * max;
        if e < 0.0 {
            b.min[k] += e;
        } else {
            b.max[k] += e;
        }
        let c = -s[k] * (1.0 + min);
        if c < 0.0 {
            b.min[k] -= c;
        } else if c > 0.0 {
            b.max[k] -= c;
        }
    }
    rc_physics::Aabb {
        min: std::array::from_fn(|k| b.min[k] + pos[k]),
        max: std::array::from_fn(|k| b.max[k] + pos[k]),
    }
}

/// Box for a given opening.
pub fn progress_box(dir: i32, to: f32, pos: [f64; 3]) -> rc_physics::Aabb {
    progress_delta_box(dir, -1.0, to, pos)
}

/// Current box.
pub fn bbox(m: &Mob) -> rc_physics::Aabb {
    progress_box(opposite(m.end.attach), physical_peek(m.end.peek_amount), m.pos)
}

/// Eye height: 0.5, stretched by the opening when it is placed on the ground.
pub fn eye_height(m: &Mob) -> f64 {
    if m.end.attach == 0 && m.end.peek_amount > 0.0 { f64::from(0.5 * (1.0 + m.end.peek_amount)) } else { 0.5 }
}

/// Snapped position: block center, y rounded.
pub fn snap(p: [f64; 3]) -> [f64; 3] {
    [p[0].floor() + 0.5, (p[1] + 0.5).floor(), p[2].floor() + 0.5]
}

pub fn register_goals(m: &mut Mob) {
    let g = &mut m.goals;
    let t = &mut m.targets;
    g.add(1, EGoal::LookAtPlayer { at: None, time: 0 }.wrap());
    g.add(4, EGoal::Attack { time: 0 }.wrap());
    g.add(7, EGoal::Peek { time: 0 }.wrap());
    g.add(8, super::super::surface::look_around());
    t.add(1, Goal::HurtBy { base: TBase::new(true, false), alert: true, stamp: 0 });
    t.add(2, super::super::surface::nearest(Who::Player, true, 10));
    // Shulker defense goal: only in a team (never here).
}

/// "Forward" and "right" vectors of the attach face plane (rotation of the opposite face
/// applied to south, normal × forward).
fn frame(attach: i32) -> ([f64; 3], [f64; 3]) {
    match opposite(attach) {
        1 => ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        0 => ([0.0, 0.0, -1.0], [1.0, 0.0, 0.0]),
        3 => ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0]),
        2 => ([0.0, -1.0, 0.0], [-1.0, 0.0, 0.0]),
        4 => ([0.0, -1.0, 0.0], [0.0, 0.0, 1.0]),
        _ => ([0.0, -1.0, 0.0], [0.0, 0.0, -1.0]),
    }
}

/// Shulker gaze control: yaw computed in the attach plane, pitch always 0, head never brought
/// back toward the body.
pub fn look_control(m: &mut Mob) {
    m.pitch = 0.0;
    if m.look.cooldown > 0 {
        m.look.cooldown -= 1;
        let w = m.look.wanted;
        let out = [w[0] - m.pos[0], w[1] - m.eye_y(), w[2] - m.pos[2]];
        let (fwd, right) = frame(m.end.attach);
        let dr = (right[0] * out[0] + right[1] * out[1] + right[2] * out[2]) as f32;
        let df = (fwd[0] * out[0] + fwd[1] * out[1] + fwd[2] * out[2]) as f32;
        if dr.abs() > 1.0e-5 || df.abs() > 1.0e-5 {
            let y =
                (atan2(f64::from(-dr), f64::from(df)) * f64::from(180.0f32) / f64::from(std::f32::consts::PI)) as f32;
            m.head_yaw = rotate_towards(m.head_yaw, y, m.look.y_max);
        }
        m.pitch = rotate_towards(m.pitch, 0.0, m.look.x_max);
    } else {
        m.head_yaw = rotate_towards(m.head_yaw, m.body_yaw, 10.0);
    }
}

impl Server {
    /// Full face of a state in a direction (collision shape).
    fn face_full(&self, s: u32, d: Dir) -> bool {
        let info = self.v().info(s);
        let m = self.bs.t.face_masks.get(info.collision as usize).map_or([0; 4], |m| m[d as usize]);
        m.iter().all(|&w| w == u64::MAX)
    }

    /// No block or shulker collision (other than `me`) in the box.
    fn shulker_free(&self, me: i32, b: &rc_physics::Aabb) -> bool {
        rc_physics::is_free(&self.world(), b)
            && !self.mobs.of_kind(Kind::Shulker).any(|o| o.id != me && o.alive() && bbox(o).intersects(b))
    }

    /// Free cell, full support face on the attach side, room to open
    /// completely.
    pub(crate) fn shulker_can_stay(&self, m: &Mob, target: [i32; 3], face: i32) -> bool {
        if !self.is_air(target) {
            let blocked = !(self.bname(target) == "minecraft:moving_piston" && target == m.block_pos());
            if blocked {
                return false;
            }
        }
        let s = step(face);
        let support = [target[0] + s[0], target[1] + s[1], target[2] + s[2]];
        if !self.chunks.get(&(support[0] >> 4, support[2] >> 4)).is_some_and(Option::is_some) {
            return false;
        }
        if !self.face_full(self.get(support), Dir::from_index(opposite(face) as usize)) {
            return false;
        }
        let c = [f64::from(target[0]) + 0.5, f64::from(target[1]), f64::from(target[2]) + 0.5];
        let mut b = progress_box(opposite(face), 1.0, c);
        b.min = b.min.map(|v| v + 1.0e-6);
        b.max = b.max.map(|v| v - 1.0e-6);
        self.shulker_free(m.id, &b)
    }

    /// First face (game order) where it can stay.
    fn shulker_attachable(&self, m: &Mob, target: [i32; 3]) -> Option<i32> {
        (0..6).find(|&d| self.shulker_can_stay(m, target, d))
    }

    /// Requested opening: "covered" armor (+20) when it closes,
    /// opening and closing sounds.
    pub(crate) fn shulker_set_peek(&mut self, m: &mut Mob, amount: i32) {
        m.attrs.armor.remove_mod("minecraft:covered");
        if amount == 0 {
            m.attrs.armor.set_perm_mod("minecraft:covered", 20.0, 0);
            self.mob_sound(m, "minecraft:entity.shulker.close", 1.0, 1.0);
        } else {
            self.mob_sound(m, "minecraft:entity.shulker.open", 1.0, 1.0);
        }
        m.end.peek = amount;
    }

    /// End of the tick (a shulker's tick after `super.tick`): new attach if needed,
    /// opening advancing by 0.05 toward the target, entities pushed.
    pub(crate) fn shulker_post_tick(&mut self, m: &mut Mob) {
        if m.mount.vehicle.is_none() && !self.shulker_can_stay(m, m.block_pos(), m.end.attach) {
            match self.shulker_attachable(m, m.block_pos()) {
                Some(d) => m.end.attach = d,
                None => {
                    self.shulker_teleport(m);
                }
            }
        }
        m.end.peek_o = m.end.peek_amount;
        let target = m.end.peek as f32 * 0.01;
        let cur = m.end.peek_amount;
        if cur == target {
            return;
        }
        m.end.peek_amount =
            if cur > target { (cur - 0.05).clamp(target, 1.0) } else { (cur + 0.05).clamp(0.0, target) };
        // Push of entities (not shulkers) in the open zone.
        let (now, old) = (physical_peek(m.end.peek_amount), physical_peek(m.end.peek_o));
        let dir = opposite(m.end.attach);
        let push = now - old;
        if push <= 0.0 {
            return;
        }
        let area = progress_delta_box(dir, old, now, m.pos);
        let s = step(dir).map(|v| f64::from(v) * f64::from(push));
        let ids: Vec<i32> = self
            .mobs
            .list
            .iter()
            .flatten()
            .filter(|o| o.id != m.id && o.kind != Kind::Shulker && o.alive() && o.touches(&area))
            .map(|o| o.id)
            .collect();
        for id in ids {
            if let Some(i) = self.mob_index(id)
                && let Some(o) = self.mobs.list[i].as_mut()
            {
                o.pos = [o.pos[0] + s[0], o.pos[1] + s[1], o.pos[2] + s[2]];
                o.needs_sync = true;
            }
        }
    }

    /// 5 tries at ±8 on each axis (entity random); empty cell,
    /// above the bottom of the world, without collision, with an attachable face outside
    /// `shulker_does_not_teleport_to`. True if teleported.
    pub(crate) fn shulker_teleport(&mut self, m: &mut Mob) -> bool {
        if m.no_ai || !m.alive() {
            return false;
        }
        let old = m.block_pos();
        for _ in 0..5 {
            let dx = m.rng.next_int_bound(17) - 8;
            let dy = m.rng.next_int_bound(17) - 8;
            let dz = m.rng.next_int_bound(17) - 8;
            let t = [old[0] + dx, old[1] + dy, old[2] + dz];
            if t[1] <= self.dim.height.min_y || !Self::block_in_border(self.border_bounds(), t) || !self.is_air(t) {
                continue;
            }
            let b = rc_physics::Aabb {
                min: [f64::from(t[0]) + 1.0e-6, f64::from(t[1]) + 1.0e-6, f64::from(t[2]) + 1.0e-6],
                max: [f64::from(t[0] + 1) - 1.0e-6, f64::from(t[1] + 1) - 1.0e-6, f64::from(t[2] + 1) - 1.0e-6],
            };
            if !self.shulker_free(m.id, &b) {
                continue;
            }
            let Some(d) = self.shulker_attachable(m, t) else { continue };
            let s = step(d);
            let support = [t[0] + s[0], t[1] + s[1], t[2] + s[2]];
            if self.tag_id(crate::tag_bits::TagId::ShulkerDoesNotTeleportTo, self.v().block(self.get(support))) {
                continue;
            }
            m.mount.vehicle = None;
            m.end.attach = d;
            self.mob_sound(m, "minecraft:entity.shulker.teleport", 1.0, 1.0);
            m.pos = snap([f64::from(t[0]) + 0.5, f64::from(t[1]), f64::from(t[2]) + 0.5]);
            self.game_event_block("minecraft:teleport", old, Some(Ent::Mob(m.id)), None);
            let n = m.block_pos();
            let packed =
                (((n[0] - old[0] + 8) & 255) << 16) | (((n[1] - old[1] + 8) & 255) << 8) | ((n[2] - old[2] + 8) & 255);
            self.level_event(2016, old, packed);
            m.end.peek = 0;
            m.target = None;
            m.needs_sync = true;
            return true;
        }
        false
    }

    /// Before damage: closed, immune to arrows.
    pub(crate) fn shulker_before_hurt(&mut self, m: &Mob, ty: &str) -> Option<bool> {
        (m.end.peek == 0 && matches!(ty, "minecraft:arrow" | "minecraft:trident")).then_some(false)
    }

    /// After damage dealt: below half HP, 1 chance in 4 to teleport; otherwise
    /// hit by a shulker bullet: teleportation and duplication.
    pub(crate) fn shulker_after_hurt(&mut self, m: &mut Mob, ty: &str) {
        // Even dead, the "1 in 4" draw happens (the teleportation then fails).
        if m.health < m.max_health() * 0.5 && m.rng.next_int_bound(4) == 0 {
            self.shulker_teleport(m);
        } else if self.dmg_tag(ty, "minecraft:is_projectile")
            && self.mobs.end.direct == Some("minecraft:shulker_bullet")
        {
            self.shulker_hit_by_bullet(m);
        }
    }

    /// Open and teleported → n living shulkers within ±8 of the old
    /// box, failure with probability (n − 1)/5 (world random), otherwise a new shulker of
    /// the same color at the old position.
    fn shulker_hit_by_bullet(&mut self, m: &mut Mob) {
        let old_pos = m.pos;
        let old_box = bbox(m);
        if m.end.peek == 0 || !self.shulker_teleport(m) {
            return;
        }
        let area = rc_physics::Aabb { min: old_box.min.map(|v| v - 8.0), max: old_box.max.map(|v| v + 8.0) };
        let mut count = self
            .mobs
            .list
            .iter()
            .flatten()
            .filter(|o| o.kind == Kind::Shulker && o.alive() && o.touches(&area))
            .count();
        if bbox(m).intersects(&area) {
            count += 1;
        }
        let failure = (count as f32 - 1.0) / 5.0;
        if self.mobs.rng.next_float() < failure {
            return;
        }
        let mut baby = self.new_mob(Kind::Shulker, snap(old_pos), 0.0);
        baby.end.color = m.end.color;
        self.add_mob(baby);
    }

    /// Tick of the shulker attack goal: shoots every 20..110 ticks if the target is within 20
    /// blocks (otherwise lost).
    pub(crate) fn shulker_attack_tick(&mut self, m: &mut Mob, time: &mut i32) {
        if self.level.difficulty_id() == 0 {
            return;
        }
        *time -= 1;
        let Some(t) = m.target else { return };
        let (Some(p), Some(eye)) = (self.ent_pos(t), self.ent_eye(t)) else { return };
        self.look_at(m, [p[0], eye, p[2]], 180.0, 180.0);
        if super::super::mth::dist2(m.pos, p) < 400.0 {
            if *time <= 0 {
                *time = 20 + m.rng.next_int_bound(10) * 20 / 2;
                let axis = m.end.attach / 2;
                self.shoot_bullet(m, t, axis);
                let pitch = (m.rng.next_float() - m.rng.next_float()) * 0.2 + 1.0;
                self.mob_sound(m, "minecraft:entity.shulker.shoot", 2.0, pitch);
            }
        } else {
            m.target = None;
        }
    }
}

/// The shulker ignores damage from another shulker for its retaliation.
pub fn ignores_attacker(s: &Server, m: &Mob, by: Ent) -> bool {
    m.kind == Kind::Shulker && matches!(by, Ent::Mob(id) if s.mob_ref(id).is_some_and(|o| o.kind == Kind::Shulker))
}
