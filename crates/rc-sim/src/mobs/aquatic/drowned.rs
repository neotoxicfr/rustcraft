//! Drowned (`server-mobs.md` § 4, `minecraft:drowned`): urge to swim, swim control,
//! goals (go to water, trident, filtered melee, beach, surface), trident throw, equipment
//! and mount at spawn.

use super::super::mob::{Ent, Mob, MoveOp};
use super::super::mth::{dist2, rotlerp_360, yaw_to};
use super::super::spawn::Reason;
use super::AquaGoal;
use crate::player::Stack;
use crate::server::Server;

/// Is the target in water?
fn target_in_water(s: &Server, t: Ent) -> bool {
    match t {
        Ent::Player => s.player_in_water(),
        Ent::Mob(id) => s.mob_ref(id).is_some_and(|o| o.in_water),
    }
}

/// Looks for land, or its target is in water.
pub fn wants_to_swim(s: &Server, m: &Mob) -> bool {
    m.aqua.searching_for_land || m.target.is_some_and(|t| target_in_water(s, t))
}

/// Always at night, by day only a target in water.
pub fn ok_target(s: &Server, t: Option<Ent>) -> bool {
    match t {
        Some(t) => !s.bright_outside() || target_in_water(s, t),
        None => false,
    }
}

/// Tick of the drowned's movement control; true if it replaces the common control.
pub fn move_control(s: &Server, m: &mut Mob) -> bool {
    if wants_to_swim(s, m) && m.in_water {
        let higher = m.target.and_then(|t| s.ent_pos(t)).is_some_and(|p| p[1] > m.pos[1]);
        if higher || m.aqua.searching_for_land {
            m.vel[1] += 0.002;
        }
        // The operation stays "go to": only the navigation stopping cuts the speed.
        if m.move_ctl.op != MoveOp::MoveTo || m.nav.is_done() {
            m.speed = 0.0;
            m.zza = 0.0;
            return true;
        }
        let w = m.move_ctl.wanted;
        let (xd, mut yd, zd) = (w[0] - m.pos[0], w[1] - m.pos[1], w[2] - m.pos[2]);
        let dd = (xd * xd + yd * yd + zd * zd).sqrt();
        yd /= dd;
        let yr = yaw_to(xd, zd);
        m.yaw = rotlerp_360(m.yaw, yr, 90.0);
        m.body_yaw = m.yaw;
        let target = (m.move_ctl.speed_mod * m.movement_speed()) as f32;
        let speed = m.speed + 0.125 * (target - m.speed);
        m.speed = speed;
        m.zza = speed;
        // Raw horizontal deltas (not normalized), vertical normalized: like the game.
        let sp = f64::from(speed);
        m.vel[0] += sp * xd * 0.005;
        m.vel[1] += sp * yd * 0.1;
        m.vel[2] += sp * zd * 0.005;
        return true;
    }
    if !m.on_ground {
        m.vel[1] -= 0.008;
    }
    false
}

/// The path's target is within 2 blocks.
fn close_to_next_pos(m: &Mob) -> bool {
    m.nav.path.as_ref().is_some_and(|p| {
        let t = p.target;
        dist2(m.pos, [f64::from(t[0]), f64::from(t[1]), f64::from(t[2])]) < 4.0
    })
}

/// The center proximity test (block center).
fn near_center(b: [i32; 3], p: [f64; 3], d: f64) -> bool {
    let c = [f64::from(b[0]) + 0.5, f64::from(b[1]) + 0.5, f64::from(b[2]) + 0.5];
    dist2(c, p) < d * d
}

/// Valid beach: two air cells above a block one can stand on.
fn beach_valid(s: &Server, p: [i32; 3]) -> bool {
    let a = [p[0], p[1] + 1, p[2]];
    s.is_air(a) && s.is_air([a[0], a[1] + 1, a[2]]) && s.data.state(s.st(p)).is_some_and(|i| i.sturdy & 2 != 0)
}

/// The nearest-block search (search 8, vertical 2, start 0), the game's traversal order.
fn find_beach(s: &Server, m: &Mob) -> Option<[i32; 3]> {
    let (h, v) = (8, 2);
    let b = m.block_pos();
    let mut y = 0;
    while y <= v {
        for r in 0..h {
            let mut x = 0;
            while x <= r {
                let mut z = if x < r && x > -r { r } else { 0 };
                while z <= r {
                    let p = [b[0] + x, b[1] + y - 1, b[2] + z];
                    if beach_valid(s, p) {
                        return Some(p);
                    }
                    z = if z > 0 { -z } else { 1 - z };
                }
                x = if x > 0 { -x } else { 1 - x };
            }
        }
        y = if y > 0 { -y } else { 1 - y };
    }
    None
}

impl Server {
    pub(crate) fn drowned_can_use(&mut self, g: &mut AquaGoal, m: &mut Mob) -> bool {
        match g {
            AquaGoal::GoToWater { wanted } => {
                if !self.bright_outside() || m.in_water {
                    return false;
                }
                let b = m.block_pos();
                for _ in 0..10 {
                    let x = m.rng.next_int_bound(20) - 10;
                    let y = 2 - m.rng.next_int_bound(8);
                    let z = m.rng.next_int_bound(20) - 10;
                    let p = [b[0] + x, b[1] + y, b[2] + z];
                    if self.bname(p) == "minecraft:water" {
                        *wanted = [f64::from(p[0]) + 0.5, f64::from(p[1]), f64::from(p[2]) + 0.5];
                        return true;
                    }
                }
                false
            }
            AquaGoal::Trident { target, .. } => {
                match m.target.filter(|t| self.ent_alive(*t)) {
                    Some(t) => *target = Some(t),
                    None => return false,
                }
                m.equipment[0].as_ref().is_some_and(|s| s.id == "minecraft:trident")
            }
            AquaGoal::GoToBeach { next_start, pos, .. } => {
                if *next_start > 0 {
                    *next_start -= 1;
                    return false;
                }
                *next_start = super::super::mth::reduced(200 + m.rng.next_int_bound(200));
                let Some(p) = find_beach(self, m) else { return false };
                *pos = p;
                let sea = super::super::surface::sea_level(self);
                !self.bright_outside() && m.in_water && m.pos[1] >= f64::from(sea - 3)
            }
            AquaGoal::SwimUp { .. } => {
                let sea = super::super::surface::sea_level(self);
                !self.bright_outside() && m.in_water && m.pos[1] < f64::from(sea - 2)
            }
            _ => false,
        }
    }

    pub(crate) fn drowned_can_continue(&mut self, g: &mut AquaGoal, m: &mut Mob) -> bool {
        match g {
            AquaGoal::GoToWater { .. } => !m.nav.is_done(),
            AquaGoal::Trident { .. } => {
                if self.drowned_can_use(g, m) {
                    return true;
                }
                let AquaGoal::Trident { target, .. } = g else { return false };
                target.is_some_and(|t| self.ent_alive(t)) && !m.nav.is_done()
            }
            AquaGoal::GoToBeach { try_ticks, max_stay, pos, .. } => {
                *try_ticks >= -*max_stay && *try_ticks <= 1200 && beach_valid(self, *pos)
            }
            AquaGoal::SwimUp { stuck } => {
                let st = *stuck;
                self.drowned_can_use(g, m) && !st
            }
            _ => false,
        }
    }

    pub(crate) fn drowned_start(&mut self, g: &mut AquaGoal, m: &mut Mob) {
        match g {
            AquaGoal::GoToWater { wanted } => {
                let w = *wanted;
                self.move_to_pos(m, w, 1.0);
            }
            AquaGoal::Trident { .. } => {
                m.aggressive = true;
                m.use_ticks = 0;
            }
            AquaGoal::GoToBeach { try_ticks, max_stay, pos, .. } => {
                m.aqua.searching_for_land = false;
                let p = [f64::from(pos[0]) + 0.5, f64::from(pos[1] + 1), f64::from(pos[2]) + 0.5];
                self.move_to_pos(m, p, 1.0);
                *try_ticks = 0;
                let inner = m.rng.next_int_bound(1200) + 1200;
                *max_stay = m.rng.next_int_bound(inner) + 1200;
            }
            AquaGoal::SwimUp { stuck } => {
                m.aqua.searching_for_land = true;
                *stuck = false;
            }
            _ => {}
        }
    }

    pub(crate) fn drowned_stop(&mut self, g: &mut AquaGoal, m: &mut Mob) {
        match g {
            AquaGoal::Trident { target, attack_time, see_time } => {
                *target = None;
                *see_time = 0;
                *attack_time = -1;
                m.use_ticks = -1;
                m.aggressive = false;
            }
            AquaGoal::SwimUp { .. } => m.aqua.searching_for_land = false,
            _ => {}
        }
    }

    pub(crate) fn drowned_tick(&mut self, g: &mut AquaGoal, m: &mut Mob) {
        match g {
            AquaGoal::Trident { target, attack_time, see_time } => {
                const RADIUS: f32 = 10.0;
                let Some(t) = *target else { return };
                let (Some(tp), Some(eye)) = (self.ent_pos(t), self.ent_eye(t)) else { return };
                let d2 = dist2(m.pos, tp);
                let sight = self.line_of_sight(m, t);
                if sight {
                    *see_time += 1;
                } else {
                    *see_time = 0;
                }
                if d2 <= f64::from(RADIUS * RADIUS) && *see_time >= 5 {
                    m.nav.stop();
                } else {
                    self.move_to_entity(m, tp, 1.0);
                }
                self.look_at(m, [tp[0], eye, tp[2]], 30.0, 30.0);
                *attack_time -= 1;
                if *attack_time == 0 {
                    if !sight {
                        return;
                    }
                    // Power has no effect on the trident; interval min = max = 40.
                    self.throw_trident(m, t);
                    *attack_time = 40;
                } else if *attack_time < 0 {
                    *attack_time = 40;
                }
            }
            AquaGoal::GoToBeach { try_ticks, pos, .. } => {
                let target = [pos[0], pos[1] + 1, pos[2]];
                if !near_center(target, m.pos, 1.0) {
                    *try_ticks += 1;
                    if *try_ticks % 40 == 0 {
                        let p = [f64::from(target[0]) + 0.5, f64::from(target[1]), f64::from(target[2]) + 0.5];
                        self.move_to_pos(m, p, 1.0);
                    }
                } else {
                    *try_ticks -= 1;
                }
            }
            AquaGoal::SwimUp { stuck } => {
                let sea = super::super::surface::sea_level(self);
                if m.pos[1] < f64::from(sea - 1) && (m.nav.is_done() || close_to_next_pos(m)) {
                    let to = [m.pos[0], f64::from(sea - 1), m.pos[2]];
                    match self.pos_towards(m, 4, 8, to, std::f32::consts::FRAC_PI_2) {
                        None => *stuck = true,
                        Some(n) => {
                            self.move_to_pos(m, n, 1.0);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Drowned equipment (world random source): 10% weapon,
    /// trident 10/16, fishing rod 6/16; neither zombie armor nor zombie weapon.
    pub(crate) fn drowned_equipment(&mut self, m: &mut Mob) {
        let r = &mut self.mobs.rng;
        if f64::from(r.next_float()) > 0.9 {
            let k = r.next_int_bound(16);
            let id = if k < 10 { "minecraft:trident" } else { "minecraft:fishing_rod" };
            m.equipment[0] = Some(Stack { id: id.into(), count: 1, components: None });
        }
    }

    /// End of a drowned's spawn finalization: nautilus shell (3%, dropped for sure), then the
    /// draw of the mounted zombie nautilus (natural or structure, trident, adult, outside rivers).
    pub(crate) fn drowned_finalize(&mut self, m: &mut Mob, reason: Reason) {
        if m.equipment[1].is_none() && self.mobs.rng.next_float() < 0.03 {
            m.equipment[1] = Some(Stack { id: "minecraft:nautilus_shell".into(), count: 1, components: None });
            m.drop_chance[1] = 2.0;
        }
        let trident = m.equipment[0].as_ref().is_some_and(|s| s.id == "minecraft:trident");
        let structure = reason == Reason::Structure;
        if (reason == Reason::Natural || structure)
            && trident
            && self.mobs.rng.next_float() < 0.5
            && !m.is_baby()
            && !self.biome_tag(m.block_pos(), "minecraft:more_frequent_drowned_spawns")
        {
            // Mount finalized with the drowned's reason, persistent if it comes from a structure.
            self.zombie_nautilus_jockey(m, reason, structure);
        }
    }

    /// Trident aimed at a third of the target's height.
    fn throw_trident(&mut self, m: &mut Mob, target: Ent) {
        let (Some(tp), Some(tb)) = (self.ent_pos(target), self.ent_bbox(target)) else { return };
        let item = match &m.equipment[0] {
            Some(s) if s.id == "minecraft:trident" => s.clone(),
            _ => Stack { id: "minecraft:trident".into(), count: 1, components: None },
        };
        let start = [m.pos[0], m.eye_y() - f64::from(0.1f32), m.pos[2]];
        let xd = tp[0] - m.pos[0];
        let yd = tb.min[1] + (tb.max[1] - tb.min[1]) * 0.333_333_333_333_333_3 - start[1];
        let zd = tp[2] - m.pos[2];
        let dist = (xd * xd + zd * zd).sqrt();
        let unc = (14 - self.level.difficulty_id() * 4) as f32;
        self.spawn_trident(m, start, [xd, yd + dist * f64::from(0.2f32), zd], unc, item);
        let p = 1.0 / (m.rng.next_float() * 0.4 + 0.8);
        self.mob_sound(m, "minecraft:entity.drowned.shoot", 1.0, p);
    }
}
