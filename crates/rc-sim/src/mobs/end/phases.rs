//! Dragon phases (F15): synchronized ids, own state of each phase (kept from one activation to
//! the next, like the instances of the game's manager), server tick, flight target, speeds,
//! reaction to damage and to destroyed crystals.

use super::super::mob::{Ent, Mob};
use super::super::mth::{DEG, atan2, cos, sin};
use super::dragon::Dragon;
use super::graph::Path;
use crate::server::Server;

/// Phase (network id and `DragonPhase` NBT).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    HoldingPattern = 0,
    StrafePlayer = 1,
    LandingApproach = 2,
    Landing = 3,
    Takeoff = 4,
    SittingFlaming = 5,
    SittingScanning = 6,
    SittingAttacking = 7,
    ChargingPlayer = 8,
    Dying = 9,
    Hover = 10,
}

impl Phase {
    /// Unknown id: circle around.
    pub fn from_id(id: i32) -> Phase {
        match id {
            1 => Phase::StrafePlayer,
            2 => Phase::LandingApproach,
            3 => Phase::Landing,
            4 => Phase::Takeoff,
            5 => Phase::SittingFlaming,
            6 => Phase::SittingScanning,
            7 => Phase::SittingAttacking,
            8 => Phase::ChargingPlayer,
            9 => Phase::Dying,
            10 => Phase::Hover,
            _ => Phase::HoldingPattern,
        }
    }

    /// "Sitting" phase: hover included.
    pub fn sitting(self) -> bool {
        matches!(self, Phase::Hover | Phase::SittingFlaming | Phase::SittingScanning | Phase::SittingAttacking)
    }

    /// Sitting phase of the game (common class): arrows and wind charges without effect.
    pub fn sitting_class(self) -> bool {
        matches!(self, Phase::SittingFlaming | Phase::SittingScanning | Phase::SittingAttacking)
    }

    /// Flight speed.
    pub fn fly_speed(self) -> f32 {
        match self {
            Phase::Hover => 1.0,
            Phase::Landing => 1.5,
            Phase::ChargingPlayer | Phase::Dying => 3.0,
            _ => 0.6,
        }
    }
}

/// Own state of each phase.
#[derive(Debug, Clone, Default)]
pub struct States {
    pub hold_path: Option<Path>,
    pub hold_target: Option<[f64; 3]>,
    pub hold_cw: bool,
    pub strafe_charge: i32,
    pub strafe_path: Option<Path>,
    pub strafe_target: Option<[f64; 3]>,
    pub strafe_attack: Option<Ent>,
    pub strafe_cw: bool,
    pub approach_path: Option<Path>,
    pub approach_target: Option<[f64; 3]>,
    pub landing_target: Option<[f64; 3]>,
    pub takeoff_first: bool,
    pub takeoff_path: Option<Path>,
    pub takeoff_target: Option<[f64; 3]>,
    pub flame_ticks: i32,
    pub flame_count: i32,
    pub flame: Option<i32>,
    pub scan_time: i32,
    pub attack_ticks: i32,
    pub charge_target: Option<[f64; 3]>,
    pub charge_time: i32,
    pub death_target: Option<[f64; 3]>,
    pub death_time: i32,
    pub hover_target: Option<[f64; 3]>,
}

impl Dragon {
    /// `begin` of the current phase.
    fn begin(&mut self) {
        let s = &mut self.ph;
        match self.phase {
            Phase::HoldingPattern => {
                s.hold_path = None;
                s.hold_target = None;
            }
            Phase::StrafePlayer => {
                s.strafe_charge = 0;
                s.strafe_target = None;
                s.strafe_path = None;
                s.strafe_attack = None;
            }
            Phase::LandingApproach => {
                s.approach_path = None;
                s.approach_target = None;
            }
            Phase::Landing => s.landing_target = None,
            Phase::Takeoff => {
                s.takeoff_first = true;
                s.takeoff_path = None;
                s.takeoff_target = None;
            }
            Phase::SittingFlaming => {
                s.flame_ticks = 0;
                s.flame_count += 1;
            }
            Phase::SittingScanning => s.scan_time = 0,
            Phase::SittingAttacking => s.attack_ticks = 0,
            Phase::ChargingPlayer => {
                s.charge_target = None;
                s.charge_time = 0;
            }
            Phase::Dying => {
                s.death_target = None;
                s.death_time = 0;
            }
            Phase::Hover => s.hover_target = None,
        }
    }

    /// `end` of the current phase: the sitting breath removes its cloud.
    fn end(&mut self) {
        if self.phase == Phase::SittingFlaming
            && let Some(id) = self.ph.flame.take()
        {
            self.discard_clouds.push(id);
        }
    }

    /// Phase set at construction (hover, without end of the previous phase).
    pub fn set_phase_initial(&mut self) {
        self.phase = Phase::Hover;
        self.begin();
    }

    /// Nothing if it is already the current phase.
    pub fn set_phase(&mut self, p: Phase) {
        if p == self.phase {
            return;
        }
        self.end();
        self.phase = p;
        self.begin();
    }

    /// Flight target of the phase.
    pub fn fly_target(&self) -> Option<[f64; 3]> {
        let s = &self.ph;
        match self.phase {
            Phase::HoldingPattern => s.hold_target,
            Phase::StrafePlayer => s.strafe_target,
            Phase::LandingApproach => s.approach_target,
            Phase::Landing => s.landing_target,
            Phase::Takeoff => s.takeoff_target,
            Phase::ChargingPlayer => s.charge_target,
            Phase::Dying => s.death_target,
            Phase::Hover => s.hover_target,
            Phase::SittingFlaming | Phase::SittingScanning | Phase::SittingAttacking => None,
        }
    }

    /// Turn speed: sharper landing.
    pub fn turn_speed(&self, vel: [f64; 3]) -> f32 {
        let rot = (vel[0] * vel[0] + vel[2] * vel[2]).sqrt() as f32 + 1.0;
        let dist = rot.min(40.0);
        if self.phase == Phase::Landing { dist / rot } else { 0.7 / dist / rot }
    }
}

/// Squared distance from a point to the dragon's position.
fn d2(t: [f64; 3], p: [f64; 3]) -> f64 {
    (t[0] - p[0]).powi(2) + (t[1] - p[1]).powi(2) + (t[2] - p[2]).powi(2)
}

impl Server {
    /// Number of living crystals seen by the dragon (0 without a fight).
    pub(crate) fn dragon_alive_crystals(&self, d: &Dragon) -> Option<i32> {
        if !d.fight {
            return None;
        }
        self.mobs.end.fight.as_ref().map(|f| f.alive_crystals)
    }

    /// Inner nodes only (no fight or no crystal left).
    fn inner_only(&self, d: &Dragon) -> bool {
        self.dragon_alive_crystals(d).is_none_or(|n| n == 0)
    }

    /// Builds the graph on the first call.
    pub(crate) fn dragon_closest_node(&self, m: &Mob, d: &mut Dragon) -> usize {
        if !d.graph.built() {
            d.graph.build(|x, z| self.hm_no_leaves(x, z));
        }
        let inner = self.inner_only(d);
        d.graph.closest(m.pos, inner)
    }

    fn dragon_closest_node_to(&self, d: &Dragon, t: [f64; 3]) -> usize {
        d.graph.closest(t, self.inner_only(d))
    }

    fn dragon_path(&self, d: &mut Dragon, start: usize, end: usize, tail: Option<[i32; 3]>) -> Option<Path> {
        let inner = self.inner_only(d);
        d.graph.find_path(start, end, tail, inner)
    }

    /// Top of the podium.
    pub(crate) fn egg_pos(&self, d: &Dragon) -> [i32; 3] {
        let o = d.origin;
        [o[0], self.hm_no_leaves(o[0], o[2]), o[2]]
    }

    /// Closest player to a point according to a fight targeting rule
    /// (the closest-player search): alive, visible, non-creative; optional range
    /// (with invisibility) and sight; own filter.
    pub(crate) fn dragon_target_player(
        &self,
        m: &mut Mob,
        range: f64,
        sight: bool,
        filter: impl Fn(&Server) -> bool,
    ) -> bool {
        if !self.ent_alive(Ent::Player) || self.game_mode == 3 || self.game_mode == 1 {
            return false;
        }
        if !filter(self) {
            return false;
        }
        if range > 0.0 {
            let modi = self.visibility(Ent::Player, m);
            let vis = (range * modi).max(2.0);
            if d2(self.pos, m.pos) > vis * vis {
                return false;
            }
        }
        !sight || self.line_of_sight(m, Ent::Player)
    }

    /// Next waypoint: next node, height + random 0..20 (never below).
    fn next_waypoint(m: &mut Mob, path: &mut Option<Path>) -> Option<[f64; 3]> {
        let p = path.as_mut()?;
        if p.is_done() {
            return None;
        }
        let c = p.next_pos();
        p.advance();
        let mut y;
        loop {
            y = f64::from(c[1]) + f64::from(m.rng.next_float() * 20.0);
            if y >= f64::from(c[1]) {
                break;
            }
        }
        Some([f64::from(c[0]), y, f64::from(c[2])])
    }

    /// Target index of a circling flight (outer ring modulo 12 if there is a fight according to
    /// `test`, otherwise inner ring 12..19).
    fn ring_target(idx: i32, outer: bool) -> usize {
        if outer {
            let mut t = idx % 12;
            if t < 0 {
                t += 12;
            }
            t as usize
        } else {
            (((idx - 12) & 7) + 12) as usize
        }
    }

    /// Server tick of the current phase.
    pub(crate) fn phase_tick(&mut self, m: &mut Mob, d: &mut Dragon) {
        match d.phase {
            Phase::HoldingPattern => self.holding_tick(m, d),
            Phase::StrafePlayer => self.strafe_tick(m, d),
            Phase::LandingApproach => self.approach_tick(m, d),
            Phase::Landing => {
                if d.ph.landing_target.is_none() {
                    let e = self.egg_pos(d);
                    d.ph.landing_target = Some([f64::from(e[0]) + 0.5, f64::from(e[1]), f64::from(e[2]) + 0.5]);
                }
                if d.ph.landing_target.is_some_and(|t| d2(t, m.pos) < 1.0) {
                    d.ph.flame_count = 0;
                    d.set_phase(Phase::SittingScanning);
                }
            }
            Phase::Takeoff => self.takeoff_tick(m, d),
            Phase::SittingFlaming => self.flaming_tick(m, d),
            Phase::SittingScanning => self.scanning_tick(m, d),
            Phase::SittingAttacking => {
                let t = d.ph.attack_ticks;
                d.ph.attack_ticks += 1;
                if t >= 40 {
                    d.set_phase(Phase::SittingFlaming);
                }
            }
            Phase::ChargingPlayer => {
                match d.ph.charge_target {
                    None => d.set_phase(Phase::HoldingPattern),
                    Some(_)
                        if d.ph.charge_time > 0 && {
                            let t = d.ph.charge_time;
                            d.ph.charge_time += 1;
                            t >= 10
                        } =>
                    {
                        d.set_phase(Phase::HoldingPattern);
                    }
                    Some(t) => {
                        // Collisions always false: the dragon passes through blocks.
                        let dd = d2(t, m.pos);
                        if !(100.0..=22_500.0).contains(&dd) {
                            d.ph.charge_time += 1;
                        }
                    }
                }
            }
            Phase::Dying => {
                d.ph.death_time += 1;
                if d.ph.death_target.is_none() {
                    let o = d.origin;
                    let y = self.hm_motion_blocking(o[0], o[2]);
                    d.ph.death_target = Some([f64::from(o[0]) + 0.5, f64::from(y), f64::from(o[2]) + 0.5]);
                }
                let t = d.ph.death_target.unwrap_or(m.pos);
                let dd = d2(t, m.pos);
                m.health = if !(100.0..=22_500.0).contains(&dd) { 0.0 } else { 1.0 };
            }
            Phase::Hover => {
                if d.ph.hover_target.is_none() {
                    d.ph.hover_target = Some(m.pos);
                }
            }
        }
    }

    // ---------- Circling flight ----------

    fn holding_tick(&mut self, m: &mut Mob, d: &mut Dragon) {
        let dd = d.ph.hold_target.map_or(0.0, |t| d2(t, m.pos));
        if dd < 100.0 || dd > 22_500.0 {
            self.holding_new_target(m, d);
        }
    }

    fn holding_new_target(&mut self, m: &mut Mob, d: &mut Dragon) {
        if d.ph.hold_path.as_ref().is_some_and(Path::is_done) {
            let egg = self.egg_pos(d);
            let crystals = self.dragon_alive_crystals(d).unwrap_or(0);
            if m.rng.next_int_bound(crystals + 3) == 0 {
                d.set_phase(Phase::LandingApproach);
                return;
            }
            let eggc = [f64::from(egg[0]) + 0.5, f64::from(egg[1]) + 0.5, f64::from(egg[2]) + 0.5];
            let player = self.dragon_target_player(m, -1.0, false, |_| true);
            let dist = if player { d2(eggc, self.pos) / 512.0 } else { 64.0 };
            if player && (m.rng.next_int_bound((dist + 2.0) as i32) == 0 || m.rng.next_int_bound(crystals + 2) == 0) {
                self.strafe_player(m, d, Ent::Player);
                return;
            }
        }
        if d.ph.hold_path.as_ref().is_none_or(Path::is_done) {
            let cur = self.dragon_closest_node(m, d) as i32;
            let mut target = cur;
            if m.rng.next_int_bound(8) == 0 {
                d.ph.hold_cw = !d.ph.hold_cw;
                target += 6;
            }
            target += if d.ph.hold_cw { 1 } else { -1 };
            // "Crystals ≥ 0": always true with a fight (outer ring).
            let outer = self.dragon_alive_crystals(d).is_some_and(|n| n >= 0);
            let t = Self::ring_target(target, outer);
            d.ph.hold_path = self.dragon_path(d, cur as usize, t, None);
            if let Some(p) = d.ph.hold_path.as_mut() {
                p.advance();
            }
        }
        if let Some(t) = Self::next_waypoint(m, &mut d.ph.hold_path) {
            d.ph.hold_target = Some(t);
        }
    }

    /// Switches to strafing a target (the low-level attack then the target choice).
    pub(crate) fn strafe_player(&mut self, m: &mut Mob, d: &mut Dragon, target: Ent) {
        d.set_phase(Phase::StrafePlayer);
        let Some(tp) = self.ent_pos(target) else { return };
        d.ph.strafe_attack = Some(target);
        let cur = self.dragon_closest_node(m, d);
        let to = self.dragon_closest_node_to(d, tp);
        let fx = tp[0].floor() as i32;
        let fz = tp[2].floor() as i32;
        let xd = f64::from(fx) - m.pos[0];
        let zd = f64::from(fz) - m.pos[2];
        let sd = (xd * xd + zd * zd).sqrt();
        let ho = (f64::from(0.4f32) + sd / 80.0 - 1.0).min(10.0);
        let fy = (tp[1] + ho).floor() as i32;
        d.ph.strafe_path = self.dragon_path(d, cur, to, Some([fx, fy, fz]));
        if let Some(p) = d.ph.strafe_path.as_mut() {
            p.advance();
            if let Some(t) = Self::next_waypoint(m, &mut d.ph.strafe_path) {
                d.ph.strafe_target = Some(t);
            }
        }
    }

    // ---------- Strafing ----------

    fn strafe_tick(&mut self, m: &mut Mob, d: &mut Dragon) {
        let Some(target) = d.ph.strafe_attack else {
            d.set_phase(Phase::HoldingPattern);
            return;
        };
        let Some(tp) = self.ent_pos(target) else {
            d.set_phase(Phase::HoldingPattern);
            return;
        };
        if d.ph.strafe_path.as_ref().is_some_and(Path::is_done) {
            let (xd, zd) = (tp[0] - m.pos[0], tp[2] - m.pos[2]);
            let dist = (xd * xd + zd * zd).sqrt();
            let ho = (f64::from(0.4f32) + dist / 80.0 - 1.0).min(10.0);
            d.ph.strafe_target = Some([tp[0], tp[1] + ho, tp[2]]);
        }
        let dd = d.ph.strafe_target.map_or(0.0, |t| d2(t, m.pos));
        if dd < 100.0 || dd > 22_500.0 {
            self.strafe_new_target(m, d);
        }
        if d2(tp, m.pos) < 4096.0 {
            if self.line_of_sight(m, target) {
                d.ph.strafe_charge += 1;
                let aim = norm([tp[0] - m.pos[0], 0.0, tp[2] - m.pos[2]]);
                let dir = norm([f64::from(sin(m.yaw * DEG)), 0.0, f64::from(-cos(m.yaw * DEG))]);
                let dot = (dir[0] * aim[0] + dir[1] * aim[1] + dir[2] * aim[2]) as f32;
                let angle =
                    (f64::from(dot).acos() * f64::from(180.0f32) / f64::from(std::f32::consts::PI)) as f32 + 0.5;
                if d.ph.strafe_charge >= 5 && (0.0..10.0).contains(&angle) {
                    let view = super::super::nether::ghast::view_vector(m.pitch, m.yaw);
                    let head = d.parts[0];
                    let start = [head[0] - view[0], head[1] + 0.5 + 0.5, head[2] - view[2]];
                    let teye = self.ent_pos(target).map_or(tp[1], |p| p[1]) + self.ent_half_height(target);
                    let dir = [tp[0] - start[0], teye - start[1], tp[2] - start[2]];
                    if !super::silent(m) {
                        self.level_event(1017, m.block_pos(), 0);
                    }
                    self.shoot_dragon_fireball(m, start, norm(dir));
                    d.ph.strafe_charge = 0;
                    if let Some(p) = d.ph.strafe_path.as_mut() {
                        while !p.is_done() {
                            p.advance();
                        }
                    }
                    d.set_phase(Phase::HoldingPattern);
                }
            } else if d.ph.strafe_charge > 0 {
                d.ph.strafe_charge -= 1;
            }
        } else if d.ph.strafe_charge > 0 {
            d.ph.strafe_charge -= 1;
        }
    }

    fn strafe_new_target(&mut self, m: &mut Mob, d: &mut Dragon) {
        if d.ph.strafe_path.as_ref().is_none_or(Path::is_done) {
            let cur = self.dragon_closest_node(m, d) as i32;
            let mut target = cur;
            if m.rng.next_int_bound(8) == 0 {
                d.ph.strafe_cw = !d.ph.strafe_cw;
                target += 6;
            }
            target += if d.ph.strafe_cw { 1 } else { -1 };
            // Here "crystals > 0" (and not ≥ 0 like the circling flight).
            let outer = self.dragon_alive_crystals(d).is_some_and(|n| n > 0);
            let t = Self::ring_target(target, outer);
            d.ph.strafe_path = self.dragon_path(d, cur as usize, t, None);
            if let Some(p) = d.ph.strafe_path.as_mut() {
                p.advance();
            }
        }
        if let Some(t) = Self::next_waypoint(m, &mut d.ph.strafe_path) {
            d.ph.strafe_target = Some(t);
        }
    }

    // ---------- Approach and takeoff ----------

    fn approach_tick(&mut self, m: &mut Mob, d: &mut Dragon) {
        let dd = d.ph.approach_target.map_or(0.0, |t| d2(t, m.pos));
        if !(dd < 100.0 || dd > 22_500.0) {
            return;
        }
        if d.ph.approach_path.as_ref().is_none_or(Path::is_done) {
            let cur = self.dragon_closest_node(m, d);
            let egg = self.egg_pos(d);
            let player = self.dragon_target_player(m, -1.0, false, |_| true);
            let to = if player {
                let aim = norm([self.pos[0], 0.0, self.pos[2]]);
                self.dragon_closest_node_to(d, [-aim[0] * 40.0, 105.0, -aim[2] * 40.0])
            } else {
                self.dragon_closest_node_to(d, [40.0, f64::from(egg[1]), 0.0])
            };
            d.ph.approach_path = self.dragon_path(d, cur, to, Some(egg));
            if let Some(p) = d.ph.approach_path.as_mut() {
                p.advance();
            }
        }
        if let Some(t) = Self::next_waypoint(m, &mut d.ph.approach_path) {
            d.ph.approach_target = Some(t);
        }
        if d.ph.approach_path.as_ref().is_some_and(Path::is_done) {
            d.set_phase(Phase::Landing);
        }
    }

    fn takeoff_tick(&mut self, m: &mut Mob, d: &mut Dragon) {
        if !d.ph.takeoff_first && d.ph.takeoff_path.is_some() {
            let egg = self.egg_pos(d);
            let c = [f64::from(egg[0]) + 0.5, f64::from(egg[1]) + 0.5, f64::from(egg[2]) + 0.5];
            if d2(c, m.pos) >= 100.0 {
                d.set_phase(Phase::HoldingPattern);
            }
            return;
        }
        d.ph.takeoff_first = false;
        let cur = self.dragon_closest_node(m, d);
        let look = self.dragon_head_look(m, d);
        let to = self.dragon_closest_node_to(d, [-look[0] * 40.0, 105.0, -look[2] * 40.0]) as i32;
        let outer = self.dragon_alive_crystals(d).is_some_and(|n| n > 0);
        let t = Self::ring_target(to, outer);
        d.ph.takeoff_path = self.dragon_path(d, cur, t, None);
        if let Some(p) = d.ph.takeoff_path.as_mut() {
            p.advance();
            if let Some(t) = Self::next_waypoint(m, &mut d.ph.takeoff_path) {
                d.ph.takeoff_target = Some(t);
            }
        }
    }

    /// Landing and takeoff (tilt according to the distance to
    /// the egg), sitting phases (−45°), otherwise gaze.
    pub(crate) fn dragon_head_look(&self, m: &Mob, d: &Dragon) -> [f64; 3] {
        use super::super::nether::ghast::view_vector;
        if matches!(d.phase, Phase::Landing | Phase::Takeoff) {
            let egg = self.egg_pos(d);
            let c = [f64::from(egg[0]) + 0.5, f64::from(egg[1]) + 0.5, f64::from(egg[2]) + 0.5];
            let dist = ((d2(c, m.pos).sqrt() as f32) / 4.0).max(1.0);
            let y_off = 6.0 / dist;
            view_vector(-y_off * 1.5 * 5.0, m.yaw)
        } else if d.phase.sitting() {
            view_vector(-45.0, m.yaw)
        } else {
            view_vector(m.pitch, m.yaw)
        }
    }

    // ---------- Sitting phases ----------

    fn flaming_tick(&mut self, m: &mut Mob, d: &mut Dragon) {
        d.ph.flame_ticks += 1;
        if d.ph.flame_ticks >= 200 {
            if d.ph.flame_count >= 4 {
                d.set_phase(Phase::Takeoff);
            } else {
                d.set_phase(Phase::SittingScanning);
            }
        } else if d.ph.flame_ticks == 10 {
            let head = d.parts[0];
            let look = norm([head[0] - m.pos[0], 0.0, head[2] - m.pos[2]]);
            let x = head[0] + look[0] * 5.0 / 2.0;
            let z = head[2] + look[2] * 5.0 / 2.0;
            let initial = head[1] + 0.5;
            let mut y = initial;
            let mut p = [x.floor() as i32, y.floor() as i32, z.floor() as i32];
            while self.is_air(p) {
                y -= 1.0;
                if y < 0.0 {
                    y = initial;
                    break;
                }
                p = [x.floor() as i32, y.floor() as i32, z.floor() as i32];
            }
            let y = y.floor() + 1.0;
            let id = self.spawn_breath_cloud(Some(m.uuid), [x, y, z], 5.0, 200, 0.0, 0);
            d.ph.flame = Some(id);
        }
    }

    fn scanning_tick(&mut self, m: &mut Mob, d: &mut Dragon) {
        d.ph.scan_time += 1;
        let my = m.pos[1];
        let near = self.dragon_target_player(m, 20.0, true, |s| (s.pos[1] - my).abs() <= 10.0);
        if near {
            if d.ph.scan_time > 25 {
                d.set_phase(Phase::SittingAttacking);
            } else {
                let aim = norm([self.pos[0] - m.pos[0], 0.0, self.pos[2] - m.pos[2]]);
                let dir = norm([f64::from(sin(m.yaw * DEG)), 0.0, f64::from(-cos(m.yaw * DEG))]);
                let dot = (dir[0] * aim[0] + dir[2] * aim[2]) as f32;
                let angle =
                    (f64::from(dot).acos() * f64::from(180.0f32) / f64::from(std::f32::consts::PI)) as f32 + 0.5;
                // Undefined angle (NaN): no rotation, like the game's comparisons.
                if angle < 0.0 || angle > 10.0 {
                    let head = d.parts[0];
                    let (xa, za) = (self.pos[0] - head[0], self.pos[2] - head[2]);
                    let delta = wrap_d(
                        180.0
                            - atan2(xa, za) * f64::from(180.0f32) / f64::from(std::f32::consts::PI)
                            - f64::from(m.yaw),
                    )
                    .clamp(-100.0, 100.0);
                    d.y_rot_a *= 0.8;
                    let mut dist = (xa * xa + za * za).sqrt() as f32 + 1.0;
                    let rot = dist;
                    if dist > 40.0 {
                        dist = 40.0;
                    }
                    d.y_rot_a += delta as f32 * (0.7 / dist / rot);
                    m.yaw += d.y_rot_a;
                }
            }
        } else if d.ph.scan_time >= 100 {
            let charge = self.dragon_target_player(m, 150.0, true, |_| true);
            d.set_phase(Phase::Takeoff);
            if charge {
                d.set_phase(Phase::ChargingPlayer);
                d.ph.charge_target = Some(self.pos);
            }
        }
    }

    /// Crystal destroyed (of the phase): in circling flight, strafes the player.
    pub(crate) fn phase_crystal_destroyed(&mut self, m: &mut Mob, d: &mut Dragon, player: Option<Ent>) {
        if d.phase == Phase::HoldingPattern
            && let Some(p) = player
            && self.dragon_target_player(m, -1.0, false, |_| true)
        {
            self.strafe_player(m, d, p);
        }
    }

    /// Half-height of an entity (Y at 0.5 of the height minus Y at the base).
    fn ent_half_height(&self, e: Ent) -> f64 {
        self.ent_bbox(e).map_or(0.9, |b| (b.max[1] - b.min[1]) * 0.5)
    }
}

/// Normalization of a vector (zero below 1e-5).
pub fn norm(v: [f64; 3]) -> [f64; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l < f64::from(1.0e-5f32) { [0.0; 3] } else { [v[0] / l, v[1] / l, v[2] / l] }
}

/// Bringing a float angle back into [−180, 180).
pub fn wrap_d(a: f64) -> f64 {
    let mut d = a % 360.0;
    if d >= 180.0 {
        d -= 360.0;
    }
    if d < -180.0 {
        d += 360.0;
    }
    d
}
