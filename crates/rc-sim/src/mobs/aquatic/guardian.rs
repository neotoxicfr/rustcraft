//! Guardian and elder guardian (`server-mobs.md` § 4): undulating swim control, jumps
//! out of the water, beam (lock, magic damage then hit), thorns, restarted wandering, residence
//! zone and mining fatigue of the elder guardian.

use super::super::mob::{Ent, Mob, MoveOp};
use super::super::mth::{dist2, rotlerp_360, yaw_to};
use super::super::species::Kind;
use super::AquaGoal;
use crate::server::Server;

/// Tick of the guardian's movement control (the "go to" operation is never put back on hold).
pub fn move_control(s: &Server, m: &mut Mob) {
    if m.move_ctl.op == MoveOp::MoveTo && !m.nav.is_done() {
        let w = m.move_ctl.wanted;
        let d = [w[0] - m.pos[0], w[1] - m.pos[1], w[2] - m.pos[2]];
        let length = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let (xd, yd, zd) = (d[0] / length, d[1] / length, d[2] / length);
        let yr = yaw_to(d[0], d[2]);
        m.yaw = rotlerp_360(m.yaw, yr, 90.0);
        m.body_yaw = m.yaw;
        let target = (m.move_ctl.speed_mod * m.movement_speed()) as f32;
        let speed = m.speed + 0.125 * (target - m.speed);
        m.speed = speed;
        m.zza = speed;
        let phase = f64::from(m.tick_count + m.id);
        let push = (phase * 0.5).sin() * 0.05;
        let rad = f64::from(m.yaw * (std::f32::consts::PI / 180.0));
        let (cos, sin) = (rad.cos(), rad.sin());
        let y_push = (phase * 0.75).sin() * 0.05;
        m.vel[0] += push * cos;
        m.vel[1] += y_push * (sin + cos) * 0.25 + f64::from(speed) * yd * 0.1;
        m.vel[2] += push * sin;
        // Smoothed gaze toward the front; the target pitch divides twice by the length (game).
        let new = [m.pos[0] + xd * 2.0, m.eye_y() + yd / length, m.pos[2] + zd * 2.0];
        let old = if m.look.cooldown > 0 { m.look.wanted } else { new };
        let lerp = |a: f64, b: f64| a + 0.125 * (b - a);
        let p = [lerp(old[0], new[0]), lerp(old[1], new[1]), lerp(old[2], new[2])];
        s.look_at(m, p, 10.0, 40.0);
        m.aqua.moving = true;
    } else {
        m.speed = 0.0;
        m.zza = 0.0;
        m.aqua.moving = false;
    }
}

/// Start of the AI step: full air in water, random jump on dry land, body turned
/// toward the head during the beam.
pub fn pre_ai_step(m: &mut Mob) {
    if !m.alive() {
        return;
    }
    if m.in_water {
        m.air = 300;
    } else if m.on_ground {
        let x = f64::from((m.rng.next_float() * 2.0 - 1.0) * 0.4);
        let z = f64::from((m.rng.next_float() * 2.0 - 1.0) * 0.4);
        m.vel = [m.vel[0] + x, m.vel[1] + 0.5, m.vel[2] + z];
        m.yaw = m.rng.next_float() * 360.0;
        m.on_ground = false;
        m.needs_sync = true;
    }
    if m.aqua.beam.is_some() {
        m.yaw = m.head_yaw;
    }
}

/// Beam duration.
fn attack_duration(k: Kind) -> i32 {
    if k == Kind::ElderGuardian { 60 } else { 80 }
}

/// Residence zone: is the position inside it?
fn within_home(m: &Mob, p: [i32; 3]) -> bool {
    match m.aqua.home {
        None => true,
        Some((c, r)) => {
            let d = [i64::from(p[0] - c[0]), i64::from(p[1] - c[1]), i64::from(p[2] - c[2])];
            d[0] * d[0] + d[1] * d[1] + d[2] * d[2] < i64::from(r) * i64::from(r)
        }
    }
}

/// The zone restriction test.
fn restricted(m: &Mob, h: f64) -> bool {
    m.aqua.home.is_some_and(|(c, r)| {
        let lim = f64::from(r) + h + 1.0;
        let cc = [f64::from(c[0]) + 0.5, f64::from(c[1]) + 0.5, f64::from(c[2]) + 0.5];
        dist2(cc, m.pos) < lim * lim
    })
}

/// The position draw toward a direction (shifted toward the residence if there is one).
fn toward(m: &mut Mob, xz: f64, d: [i32; 3]) -> [i32; 3] {
    let (mut xt, mut zt) = (f64::from(d[0]), f64::from(d[2]));
    if let Some((c, _)) = m.aqua.home
        && xz > 1.0
    {
        if m.pos[0] > f64::from(c[0]) {
            xt -= m.rng.next_double() * xz / 2.0;
        } else {
            xt += m.rng.next_double() * xz / 2.0;
        }
        if m.pos[2] > f64::from(c[2]) {
            zt -= m.rng.next_double() * xz / 2.0;
        } else {
            zt += m.rng.next_double() * xz / 2.0;
        }
    }
    [(xt + m.pos[0]).floor() as i32, (f64::from(d[1]) + m.pos[1]).floor() as i32, (zt + m.pos[2]).floor() as i32]
}

impl Server {
    fn guardian_pos_ok(&self, m: &Mob, restrict: bool, p: [i32; 3]) -> bool {
        let h = self.dim.height;
        let inside = p[1] >= h.min_y && p[1] < h.min_y + h.sections as i32 * 16;
        inside && !(restrict && !within_home(m, p)) && self.nav_stable(m, p) && !self.pos_has_malus(m, p)
    }

    /// 10 tries, best walk value (strictly greater).
    fn guardian_best(
        &self,
        m: &mut Mob,
        mut generate: impl FnMut(&Server, &mut Mob) -> Option<[i32; 3]>,
    ) -> Option<[f64; 3]> {
        let mut best: Option<(f32, [i32; 3])> = None;
        for _ in 0..10 {
            if let Some(p) = generate(self, m) {
                let w = self.walk_target_value(m, p);
                if w > best.map_or(f32::NEG_INFINITY, |b| b.0) {
                    best = Some((w, p));
                }
            }
        }
        best.map(|b| [f64::from(b.1[0]) + 0.5, f64::from(b.1[1]), f64::from(b.1[2]) + 0.5])
    }

    /// The default position draw with the residence zone.
    fn guardian_random_pos(&self, m: &mut Mob, h: i32, v: i32) -> Option<[f64; 3]> {
        let restrict = restricted(m, f64::from(h));
        self.guardian_best(m, |s, m| {
            let x = m.rng.next_int_bound(2 * h + 1) - h;
            let y = m.rng.next_int_bound(2 * v + 1) - v;
            let z = m.rng.next_int_bound(2 * h + 1) - h;
            let p = toward(m, f64::from(h), [x, y, z]);
            s.guardian_pos_ok(m, restrict, p).then_some(p)
        })
    }

    /// The position draw toward a point with the residence zone.
    fn guardian_pos_towards(&self, m: &mut Mob, h: i32, v: i32, to: [f64; 3], rad: f64) -> Option<[f64; 3]> {
        let (dx, dz) = (to[0] - m.pos[0], to[2] - m.pos[2]);
        let restrict = restricted(m, f64::from(h));
        self.guardian_best(m, |s, m| {
            let center = super::super::mth::atan2(dz, dx) - f64::from(std::f32::consts::FRAC_PI_2);
            let yr = center + f64::from(2.0 * m.rng.next_float() - 1.0) * rad;
            let t = m.rng.next_double().sqrt();
            let dist = super::super::mth::rpos_dist(t, 0.0, f64::from(h));
            let xt = -dist * yr.sin();
            let zt = dist * yr.cos();
            if xt.abs() > f64::from(h) || zt.abs() > f64::from(h) {
                return None;
            }
            let yt = m.rng.next_int_bound(2 * v + 1) - v;
            let d = [xt.floor() as i32, yt, zt.floor() as i32];
            let p = toward(m, f64::from(h), d);
            s.guardian_pos_ok(m, restrict, p).then_some(p)
        })
    }

    pub(crate) fn guardian_can_use(&mut self, g: &mut AquaGoal, m: &mut Mob) -> bool {
        match g {
            AquaGoal::Beam { .. } => m.target.is_some_and(|t| self.ent_alive(t)),
            AquaGoal::TowardsHome { wanted } => {
                if within_home(m, m.block_pos()) {
                    return false;
                }
                let Some((c, _)) = m.aqua.home else { return false };
                let to = [f64::from(c[0]) + 0.5, f64::from(c[1]), f64::from(c[2]) + 0.5];
                match self.guardian_pos_towards(m, 16, 7, to, f64::from(std::f32::consts::FRAC_PI_2)) {
                    Some(p) => {
                        *wanted = p;
                        true
                    }
                    None => false,
                }
            }
            AquaGoal::Stroll { interval, wanted } => {
                if !m.aqua.stroll_trigger {
                    if m.no_action_time >= 100 {
                        return false;
                    }
                    if m.rng.next_int_bound(super::super::mth::reduced(*interval)) != 0 {
                        return false;
                    }
                }
                match self.guardian_random_pos(m, 10, 7) {
                    Some(p) => {
                        *wanted = p;
                        m.aqua.stroll_trigger = false;
                        true
                    }
                    None => false,
                }
            }
            AquaGoal::LookAtGuardian { at, .. } => {
                if m.rng.next_float() >= 0.01 {
                    return false;
                }
                // Closest guardian (or elder guardian) in the 12 × 3 × 12 inflated box.
                let bb = m.bbox();
                let area = rc_physics::Aabb {
                    min: [bb.min[0] - 12.0, bb.min[1] - 3.0, bb.min[2] - 12.0],
                    max: [bb.max[0] + 12.0, bb.max[1] + 3.0, bb.max[2] + 12.0],
                };
                let eye = [m.pos[0], m.eye_y(), m.pos[2]];
                let cands: Vec<(i32, [f64; 3])> = self
                    .mobs
                    .list
                    .iter()
                    .flatten()
                    .filter(|o| super::is_guardian(o.kind) && o.alive() && o.touches(&area))
                    .map(|o| (o.id, o.pos))
                    .collect();
                let mut best: Option<(f64, i32)> = None;
                for (id, p) in cands {
                    if !self.targeting_ok(m, Ent::Mob(id), false, 12.0, true, true) {
                        continue;
                    }
                    let d = dist2(eye, p);
                    if best.is_none_or(|b| d < b.0) {
                        best = Some((d, id));
                    }
                }
                *at = best.map(|b| b.1);
                at.is_some()
            }
            _ => false,
        }
    }

    pub(crate) fn guardian_can_continue(&mut self, g: &mut AquaGoal, m: &mut Mob) -> bool {
        match g {
            AquaGoal::Beam { .. } => {
                let use_ok = m.target.is_some_and(|t| self.ent_alive(t));
                let close_ok = m.kind == Kind::ElderGuardian
                    || m.target.and_then(|t| self.ent_pos(t)).is_some_and(|p| dist2(m.pos, p) > 9.0);
                use_ok && close_ok
            }
            AquaGoal::TowardsHome { .. } | AquaGoal::Stroll { .. } => !m.nav.is_done(),
            AquaGoal::LookAtGuardian { at, time } => {
                let Some(id) = *at else { return false };
                let Some(o) = self.mob_ref(id).filter(|o| o.alive()) else { return false };
                dist2(m.pos, o.pos) <= 144.0 && *time > 0
            }
            _ => false,
        }
    }

    pub(crate) fn guardian_start(&mut self, g: &mut AquaGoal, m: &mut Mob) {
        match g {
            AquaGoal::Beam { attack_time } => {
                *attack_time = -10;
                m.nav.stop();
                if let Some(t) = m.target
                    && let (Some(p), Some(eye)) = (self.ent_pos(t), self.ent_eye(t))
                {
                    self.look_at(m, [p[0], eye, p[2]], 90.0, 90.0);
                }
                m.needs_sync = true;
            }
            AquaGoal::TowardsHome { wanted } | AquaGoal::Stroll { wanted, .. } => {
                let w = *wanted;
                self.move_to_pos(m, w, 1.0);
            }
            AquaGoal::LookAtGuardian { time, .. } => {
                *time = super::super::mth::reduced(40 + m.rng.next_int_bound(40));
            }
            _ => {}
        }
    }

    pub(crate) fn guardian_stop(&mut self, g: &mut AquaGoal, m: &mut Mob) {
        match g {
            AquaGoal::Beam { .. } => {
                m.aqua.beam = None;
                m.target = None;
                m.aqua.stroll_trigger = true;
            }
            AquaGoal::Stroll { .. } => m.nav.stop(),
            AquaGoal::LookAtGuardian { at, .. } => *at = None,
            _ => {}
        }
    }

    pub(crate) fn guardian_tick(&mut self, g: &mut AquaGoal, m: &mut Mob) {
        match g {
            AquaGoal::Beam { attack_time } => {
                let Some(t) = m.target else { return };
                m.nav.stop();
                if let (Some(p), Some(eye)) = (self.ent_pos(t), self.ent_eye(t)) {
                    self.look_at(m, [p[0], eye, p[2]], 90.0, 90.0);
                }
                if !self.line_of_sight(m, t) {
                    m.target = None;
                    return;
                }
                *attack_time += 1;
                if *attack_time == 0 {
                    m.aqua.beam = Some(t);
                    m.aqua.beam_id = match t {
                        Ent::Player => self.player_id,
                        Ent::Mob(id) => id,
                    };
                    self.mob_event(m, 21);
                } else if *attack_time >= attack_duration(m.kind) {
                    let mut dmg = 1.0;
                    if self.level.difficulty_id() == 3 {
                        dmg += 2.0;
                    }
                    if m.kind == Kind::ElderGuardian {
                        dmg += 2.0;
                    }
                    self.guardian_magic(m, t, dmg);
                    self.mob_attack(m, t);
                    m.target = None;
                }
            }
            AquaGoal::LookAtGuardian { at, time } => {
                let Some(o) = at.and_then(|id| self.mob_ref(id)).filter(|o| o.alive()).map(|o| (o.pos, o.eye_y()))
                else {
                    return;
                };
                let xm = super::super::goals::max_head_x(m);
                self.look_at(m, [o.0[0], o.1, o.0[2]], 10.0, xm);
                *time -= 1;
            }
            _ => {}
        }
    }

    /// Indirect magic damage of the beam (source: the guardian).
    fn guardian_magic(&mut self, m: &mut Mob, t: Ent, dmg: f32) {
        let ty = "minecraft:indirect_magic";
        match t {
            Ent::Player => {
                let kb = (m.pos[0] - self.pos[0], m.pos[2] - self.pos[2]);
                self.player_hurt_by_mob(m, ty, dmg, Some(kb));
            }
            Ent::Mob(id) => {
                let Some(i) = self.mob_index(id) else { return };
                let mut o = self.mobs.list[i].take().expect("mob");
                self.mob_hurt_by(&mut o, m, ty, dmg);
                self.mobs.list[i] = Some(o);
            }
        }
    }
}

/// Thorns (2 damage) to the living direct attacker if the guardian does not move, then restarted
/// wandering; before the damage itself.
pub fn before_hurt(s: &mut Server, m: &mut Mob, ty: &str, by: Option<Ent>) {
    let direct_living = matches!(
        ty,
        "minecraft:mob_attack" | "minecraft:mob_attack_no_aggro" | "minecraft:player_attack" | "minecraft:sting"
    );
    if !m.aqua.moving
        && direct_living
        && !s.dmg_tag(ty, "minecraft:avoids_guardian_thorns")
        && ty != "minecraft:thorns"
        && let Some(cause) = by
    {
        match cause {
            Ent::Player => {
                let kb = (m.pos[0] - s.pos[0], m.pos[2] - s.pos[2]);
                s.player_hurt_by_mob(m, "minecraft:thorns", 2.0, Some(kb));
            }
            Ent::Mob(id) => match s.mob_index(id) {
                Some(i) => {
                    let mut o = s.mobs.list[i].take().expect("mob");
                    s.mob_hurt_by(&mut o, m, "minecraft:thorns", 2.0);
                    s.mobs.list[i] = Some(o);
                }
                // Attacker mid-tick (taken out of the list): thorns applied on return.
                None => s.mobs.aqua.thorns.push((id, m.id, m.pos)),
            },
        }
    }
    m.aqua.stroll_trigger = true;
}

/// Mining fatigue III (6000 ticks) to players in
/// survival within 50 blocks, every 1200 ticks; residence zone set when needed.
pub fn elder_custom_ai(s: &mut Server, m: &mut Mob) {
    if (m.tick_count + m.id) % 1200 == 0 {
        let survival = s.game_mode == 0 || s.game_mode == 2;
        let close = dist2(m.pos, s.pos) < 50.0 * 50.0;
        let fx = s.fx().get("minecraft:mining_fatigue").cloned();
        let needs = match &fx {
            None => true,
            Some(e) => e.amp < 2 || (!e.infinite() && e.dur <= 1199),
        };
        if survival && close && needs {
            s.add_effect_from(
                crate::effects::Inst::new("minecraft:mining_fatigue", 6000, 2),
                Some(crate::progress::pred::Who::Mob(m)),
            );
            let _ = s.send(rc_client::Event::GameEvent { kind: 10, param: 1.0 });
        }
    }
    if m.aqua.home.is_none() {
        m.aqua.home = Some((m.block_pos(), 16));
    }
}

impl Server {
    /// Thorns owed to a mob that was attacking (called after its hit, attacker present).
    pub(crate) fn aqua_flush_thorns(&mut self, attacker: &mut Mob) {
        if self.mobs.aqua.thorns.is_empty() {
            return;
        }
        let pending: Vec<(i32, i32, [f64; 3])> = std::mem::take(&mut self.mobs.aqua.thorns);
        for (id, guardian, pos) in pending {
            if id == attacker.id {
                self.mob_hurt(attacker, "minecraft:thorns", 2.0, Some(Ent::Mob(guardian)), Some(pos));
            }
        }
    }
}
