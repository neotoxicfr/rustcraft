//! Spear held by a mob (kinetic weapon): spear parameters, contact damage over
//! the course of use, piglin brain behaviors (approach, charge, retreat) and zombified
//! piglin goal. Spec: `server-mobs.md` § 4.F13.
//!
//! Known differences: no dismounting of the target, no after-hit effects of enchantments
//! other than sharpness, smite and bane of arthropods, passengers of the same
//! vehicle not excluded.

use super::super::brainy::brain::{Brain, Track};
use super::super::cubes::view_vector;
use super::super::mob::{Ent, Mob};
use super::super::mth::{DEG, cos, dist2, reduced, sin};
use super::brain::{PB, PK};
use crate::server::Server;

/// Contact condition: maximum duration, minimum attacker speed, minimum relative speed.
#[derive(Clone, Copy)]
struct Cond {
    ticks: i32,
    min_speed: f32,
    min_rel: f32,
}

impl Cond {
    /// The test of a behavior condition (factor 0.2 for a mob).
    fn test(self, used: i32, atk: f64, rel: f64, factor: f64) -> bool {
        used <= self.ticks && atk >= f64::from(self.min_speed) * factor && rel >= f64::from(self.min_rel) * factor
    }
}

/// Kinetic parameters of a spear (`Item.Properties.spear`).
#[derive(Clone, Copy)]
pub(crate) struct Kin {
    delay: i32,
    mult: f32,
    dismount: Cond,
    knock: Cond,
    dmg: Cond,
}

impl Kin {
    /// The computation of the use duration from the damage.
    pub(crate) fn use_duration(&self) -> i32 {
        self.delay + self.dmg.ticks
    }
}

/// `spear(mult, delay, dismount (s, v), knockback (s, v), damage (s, v))`, seconds in ticks.
fn spear(mult: f32, delay: f32, d: (f32, f32), k: (f32, f32), g: (f32, f32)) -> Kin {
    let t = |s: f32| (s * 20.0f32) as i32;
    Kin {
        delay: t(delay),
        mult,
        dismount: Cond { ticks: t(d.0), min_speed: d.1, min_rel: 0.0 },
        knock: Cond { ticks: t(k.0), min_speed: k.1, min_rel: 0.0 },
        dmg: Cond { ticks: t(g.0), min_speed: 0.0, min_rel: g.1 },
    }
}

/// Kinetic weapon component of an item (spears only).
pub(crate) fn kinetic(id: &str) -> Option<Kin> {
    Some(match id {
        "minecraft:wooden_spear" => spear(0.7, 0.75, (5.0, 14.0), (10.0, 5.1), (15.0, 4.6)),
        "minecraft:stone_spear" => spear(0.82, 0.7, (4.5, 13.0), (9.0, 5.1), (13.75, 4.6)),
        "minecraft:copper_spear" => spear(0.82, 0.65, (4.0, 12.0), (8.25, 5.1), (12.5, 4.6)),
        "minecraft:iron_spear" => spear(0.95, 0.6, (2.5, 11.0), (6.75, 5.1), (11.25, 4.6)),
        "minecraft:golden_spear" => spear(0.7, 0.7, (3.5, 13.0), (8.5, 5.1), (13.75, 4.6)),
        "minecraft:diamond_spear" => spear(1.075, 0.5, (3.0, 10.0), (6.5, 5.1), (10.0, 4.6)),
        "minecraft:netherite_spear" => spear(1.2, 0.4, (2.5, 9.0), (5.5, 5.1), (8.75, 4.6)),
        _ => return None,
    })
}

/// Kinetic weapon in the main hand.
pub(crate) fn main_kin(m: &Mob) -> Option<Kin> {
    m.equipment[0].as_ref().and_then(|s| kinetic(&s.id))
}

/// The "holds an item" test for a spear: either hand.
pub(crate) fn holds_kinetic(m: &Mob) -> bool {
    m.equipment[0].as_ref().is_some_and(|s| kinetic(&s.id).is_some())
        || m.equipment[1].as_ref().is_some_and(|s| kinetic(&s.id).is_some())
}

/// Charge speed multiplier of a root mount: camel husk 4, zombie horse
/// 1.4 (single precision), any other mob 1; without a mount or on a minecart: 1.
pub(crate) fn charge_speed(root: Option<super::super::species::Kind>) -> f32 {
    use super::super::species::Kind;
    match root {
        Some(Kind::CamelHusk) => 4.0,
        Some(Kind::ZombieHorse) => 1.4,
        _ => 1.0,
    }
}

/// States of the spear state memory.
const APPROACH: u8 = 1;
const CHARGING: u8 = 2;
const RETREAT: u8 = 3;

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn contains(b: &rc_physics::Aabb, p: [f64; 3]) -> bool {
    (0..3).all(|i| p[i] >= b.min[i] && p[i] < b.max[i])
}

/// The start of use of an item (main hand) for a spear.
fn begin_use(m: &mut Mob) {
    m.use_ticks = 0;
    m.pig.stabbed.clear();
}

/// The use stops, the memorized victims are forgotten.
fn end_use(m: &mut Mob) {
    m.use_ticks = -1;
    m.pig.stabbed.clear();
}

/// Known velocity, computed at the start of the base tick.
pub(crate) fn compute_speed(m: &mut Mob) {
    let p = m.pos;
    let last = m.pig.last_known.replace(p).unwrap_or(p);
    m.pig.known_speed = [p[0] - last[0], p[1] - last[1], p[2] - last[2]];
}

impl Server {
    /// Mount of the spear carrier: speed multiplier of the root mount (end of the
    /// chain of vehicles; a minecart is not a mob) and distance added to the
    /// retreats when the carrier is a passenger (2 blocks).
    pub(crate) fn spear_mount(&self, m: &Mob) -> (f64, f64) {
        let extra = if m.mount.vehicle.is_some() { 2.0 } else { 0.0 };
        let mut root = None;
        let mut cur = m.mount.vehicle;
        while let Some(v) = cur {
            let Some(o) = self.mob_ref(v) else {
                root = None;
                break;
            };
            root = Some(o.kind);
            cur = o.mount.vehicle;
        }
        (f64::from(charge_speed(root)), extra)
    }

    /// Known velocity of an entity in blocks per second.
    fn spear_motion(&self, e: Ent) -> [f64; 3] {
        let v = match e {
            Ent::Player => self.mobs.cubes.player_speed,
            Ent::Mob(id) => self.mob_ref(id).map_or([0.0; 3], |o| o.pig.known_speed),
        };
        [v[0] * 20.0, v[1] * 20.0, v[2] * 20.0]
    }

    /// Entities hit by the tip (block: collision).
    fn spear_hits(&self, m: &Mob) -> Vec<Ent> {
        let look = view_vector(m.pitch, m.head_yaw);
        let eye = [m.pos[0], m.pos[1] + m.eye_height(), m.pos[2]];
        // Spear reach (2.0 to 4.5), factor 0.5 for a mob, margin 0.125.
        let (min_r, max_r) = (f64::from(2.0f32 * 0.5), f64::from(4.5f32 * 0.5));
        let margin = f64::from(0.125f32);
        let from = [eye[0] + look[0] * min_r, eye[1] + look[1] * min_r, eye[2] + look[2] * min_r];
        let reach = max_r + dot(m.vel, look).max(0.0);
        let mut to = [eye[0] + look[0] * reach, eye[1] + look[1] * reach, eye[2] + look[2] * reach];
        if let Some(t) = self.clip_t(eye, to) {
            let hit: [f64; 3] = std::array::from_fn(|i| eye[i] + (to[i] - eye[i]) * t);
            if dist2(eye, hit) < dist2(eye, from) {
                return Vec::new();
            }
            to = hit;
        }
        let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
        let mut cands: Vec<Ent> =
            self.mobs.list.iter().flatten().filter(|o| o.id != m.id && o.alive()).map(|o| Ent::Mob(o.id)).collect();
        if self.ent_alive(Ent::Player) && self.game_mode != 3 {
            cands.push(Ent::Player);
        }
        let mut out = Vec::new();
        for e in cands {
            let Some(bb) = self.ent_bbox(e) else { continue };
            if contains(&bb, from) || super::super::arrows::ray_t(&bb, from, d).is_some() {
                out.push(e);
                continue;
            }
            let big = rc_physics::Aabb { min: bb.min.map(|v| v - margin), max: bb.max.map(|v| v + margin) };
            let Some(t) = super::super::arrows::ray_t(&big, from, d) else { continue };
            let outside: [f64; 3] = std::array::from_fn(|i| from[i] + d[i] * t);
            let center: [f64; 3] = std::array::from_fn(|i| (bb.min[i] + bb.max[i]) / 2.0);
            let mut toward = center;
            if let Some(s) = self.clip_t(outside, center) {
                toward = std::array::from_fn(|i| outside[i] + (center[i] - outside[i]) * s);
            }
            let dd = [toward[0] - outside[0], toward[1] - outside[1], toward[2] - outside[2]];
            if super::super::arrows::ray_t(&bb, outside, dd).is_some() {
                out.push(e);
            }
        }
        out
    }

    /// The damage of a kinetic weapon to entities, once per tick of use (before the counter advances).
    pub(crate) fn spear_use_tick(&mut self, m: &mut Mob) {
        let Some(kin) = main_kin(m) else { return };
        if m.use_ticks < kin.delay {
            return;
        }
        let used = m.use_ticks - kin.delay;
        let look = view_vector(m.pitch, m.yaw);
        let atk = dot(look, [m.pig.known_speed[0] * 20.0, m.pig.known_speed[1] * 20.0, m.pig.known_speed[2] * 20.0]);
        let factor = f64::from(0.2f32);
        let base = m.attrs.attack.base;
        let mut affected = false;
        for e in self.spear_hits(m) {
            let recent = m.pig.stabbed.iter().any(|&(o, t)| o == e && self.time - t < 10);
            if recent {
                continue;
            }
            m.pig.stabbed.retain(|&(o, _)| o != e);
            m.pig.stabbed.push((e, self.time));
            let rel = (atk - dot(look, self.spear_motion(e))).max(0.0);
            let dd = kin.dmg.test(used, atk, rel, factor);
            let dk = kin.knock.test(used, atk, rel, factor);
            let dm = kin.dismount.test(used, atk, rel, factor);
            if dd || dk || dm {
                let dmg = base as f32 + (rel * f64::from(kin.mult)).floor() as f32;
                affected |= self.spear_stab(m, e, dmg, dd, dk);
            }
        }
        if affected {
            self.mob_event(m, 2);
        }
    }

    /// The thrust attack (without dismounting).
    fn spear_stab(&mut self, m: &mut Mob, e: Ent, base: f32, deals_damage: bool, deals_knockback: bool) -> bool {
        let weapon = m.equipment[0].clone();
        let wl = |s: &Self, n: &str| weapon.as_ref().map_or(0, |w| s.life.items.enchantment(w, n)) as f32;
        let mut dmg = base;
        let sharp = wl(self, "minecraft:sharpness");
        if sharp > 0.0 {
            dmg += 0.5 * sharp + 0.5;
        }
        if let Ent::Mob(id) = e
            && let Some(o) = self.mob_ref(id)
            && let Some(w) = &weapon
        {
            dmg += self.kind_ench_bonus(w, o.kind);
        }
        let kb_ench = wl(self, "minecraft:knockback");
        let ty = "minecraft:spear";
        let mut affected = deals_knockback;
        let mut hit = false;
        if deals_damage {
            let dealt = match e {
                Ent::Player => {
                    let kb = (m.pos[0] - self.pos[0], m.pos[2] - self.pos[2]);
                    self.player_hurt_by_mob(m, ty, dmg, Some(kb))
                }
                Ent::Mob(id) => {
                    let Some(i) = self.mob_index(id) else { return false };
                    let mut o = self.mobs.list[i].take().expect("mob");
                    self.mobs.killer = Some((m.rng, m.kind, m.id, m.block_pos()));
                    let r = self.mob_hurt_by(&mut o, m, ty, dmg);
                    if let Some((rng, _, _, _)) = self.mobs.killer.take() {
                        m.rng = rng;
                    }
                    self.mobs.list[i] = Some(o);
                    r
                }
            };
            affected |= dealt;
            hit = dealt;
        }
        if deals_knockback {
            // 0.4 then the attacker's knockback (enchantment included).
            let own = (m.attrs.attack_knockback.value() as f32 + kb_ench) / 2.0;
            for (k, power) in [0.4f32, own].into_iter().enumerate() {
                if power <= 0.0 {
                    continue;
                }
                let (s, c) = (sin(m.yaw * DEG), cos(m.yaw * DEG));
                match e {
                    Ent::Player => self.player_knockback(f64::from(power), f64::from(s), f64::from(-c)),
                    Ent::Mob(id) => {
                        if let Some(i) = self.mob_index(id) {
                            let mut o = self.mobs.list[i].take().expect("mob");
                            // Victim's own knockback (full sulfur cube: aim).
                            self.ghost_push(m);
                            let (by, eff) = (Some(Ent::Mob(m.id)), k == 1);
                            self.mob_knockback(&mut o, f64::from(power), f64::from(s), f64::from(-c), by, dmg, eff);
                            self.ghost_pop();
                            self.mobs.list[i] = Some(o);
                        }
                    }
                }
                m.vel[0] *= 0.6;
                m.vel[2] *= 0.6;
            }
        }
        // After-hit effects of the weapon (hit dealt): fire aspect, slowness from bane of arthropods.
        if hit && let Some(w) = &weapon {
            let ticks = 80 * self.life.items.enchantment(w, "minecraft:fire_aspect");
            match e {
                Ent::Player => {
                    if ticks > 0 {
                        self.player_ignite(ticks);
                    }
                }
                Ent::Mob(id) => {
                    if let Some(i) = self.mob_index(id) {
                        let mut o = self.mobs.list[i].take().expect("mob");
                        if ticks > 0 && o.fire < ticks {
                            o.fire = ticks;
                        }
                        self.bane_slowness(Some(w), &mut o);
                        self.mobs.list[i] = Some(o);
                    }
                }
            }
        }
        if affected {
            m.last_hurt_mob = Some(e);
        }
        affected
    }

    // ---------- Brain behaviors (piglin) ----------

    fn spear_able(m: &Mob, b: &Brain) -> bool {
        b.mem.attack.has() && main_kin(m).is_some()
    }

    pub(crate) fn spear_check(&mut self, m: &mut Mob, b: &mut Brain, p: &mut PB) -> bool {
        let using = m.use_ticks >= 0;
        match p {
            PB::SpearApproach => Self::spear_able(m, b) && !using,
            PB::SpearAttack { .. } => {
                b.mem.pig.spear_status.get() == Some(CHARGING) && Self::spear_able(m, b) && !using
            }
            PB::SpearRetreat { away, .. } => {
                if !(Self::spear_able(m, b) && !using) || b.mem.pig.spear_status.get() != Some(RETREAT) {
                    return false;
                }
                let Some(tp) = b.mem.attack.get().and_then(|t| self.ent_pos(t)) else { return false };
                let dist = dist2(m.pos, tp).sqrt();
                let (_, md) = self.spear_mount(m);
                *away = self.pig_land_pos_away(m, (9.0 + md - dist).max(0.0), (11.0 + md - dist).max(1.0), 7, tp);
                away.is_some()
            }
            _ => true,
        }
    }

    pub(crate) fn spear_start(&mut self, m: &mut Mob, b: &mut Brain, p: &mut PB) {
        match p {
            PB::SpearApproach => {
                m.aggressive = true;
                b.mem.pig.spear_status.set(APPROACH);
            }
            PB::SpearAttack { engage, charge } => {
                m.aggressive = true;
                *engage = main_kin(m).map_or(0, |k| k.use_duration());
                *charge = None;
                begin_use(m);
            }
            PB::SpearRetreat { time, .. } => {
                m.aggressive = true;
                *time = 0;
            }
            _ => {}
        }
    }

    pub(crate) fn spear_continue(&mut self, m: &mut Mob, b: &mut Brain, p: &mut PB) -> bool {
        let able = Self::spear_able(m, b);
        match p {
            PB::SpearApproach => {
                let far = b.mem.attack.get().and_then(|t| self.ent_pos(t)).is_some_and(|tp| dist2(m.pos, tp) > 100.0);
                able && far
            }
            PB::SpearAttack { engage, .. } => *engage > 0 && able,
            PB::SpearRetreat { time, away } => *time < 100 && away.is_some() && !m.nav.is_done() && able,
            _ => false,
        }
    }

    pub(crate) fn spear_tick(&mut self, m: &mut Mob, b: &mut Brain, p: &mut PB) {
        let Some(target) = b.mem.attack.get() else { return };
        let Some(tp) = self.ent_pos(target) else { return };
        b.mem.look.set(Track::ent(target, true));
        // Brain speeds (1.0) multiplied by that of the mount.
        let (sp, md) = self.spear_mount(m);
        match p {
            PB::SpearApproach => {
                self.move_to_entity(m, tp, sp);
            }
            PB::SpearAttack { engage, charge } => {
                let d2 = dist2(m.pos, tp);
                *engage -= 1;
                if let Some(c) = *charge {
                    self.move_to_pos(m, c, sp);
                    if m.nav.is_done() {
                        *charge = None;
                    }
                } else {
                    self.move_to_entity(m, tp, sp);
                    if d2 < 4.0 || m.nav.is_done() {
                        let dist = d2.sqrt();
                        *charge = self.pig_land_pos_away(m, 6.0 + md - dist, 7.0 + md - dist, 7, tp);
                    }
                }
            }
            PB::SpearRetreat { time, away } => {
                *time += 1;
                if let Some(a) = *away {
                    self.move_to_pos(m, a, sp);
                }
            }
            _ => {}
        }
    }

    pub(crate) fn spear_stop(&mut self, m: &mut Mob, b: &mut Brain, p: &mut PB) {
        match p {
            PB::SpearApproach => {
                m.nav.stop();
                b.mem.pig.spear_status.set(CHARGING);
            }
            PB::SpearAttack { engage, charge } => {
                m.nav.stop();
                end_use(m);
                *charge = None;
                *engage = 0;
                b.mem.pig.spear_status.set(RETREAT);
            }
            PB::SpearRetreat { time, away } => {
                m.nav.stop();
                m.aggressive = false;
                end_use(m);
                *time = 0;
                *away = None;
                b.mem.pig.erase(PK::SpearStatus);
            }
            _ => {}
        }
    }
}

// ---------- Zombified piglin goal ----------

/// Goal state: absent while the goal is not running.
#[derive(Debug, Clone, Default)]
pub struct SpearGoal {
    on: bool,
    engage: i32,
    fleeing: i32,
    away: Option<[f64; 3]>,
    done: bool,
}

impl SpearGoal {
    pub fn new() -> SpearGoal {
        SpearGoal { on: false, engage: -1, fleeing: -1, away: None, done: false }
    }
}

impl Server {
    fn spear_goal_able(m: &Mob) -> bool {
        m.target.is_some() && main_kin(m).is_some()
    }

    pub(crate) fn spear_goal_can_use(&mut self, m: &Mob) -> bool {
        Self::spear_goal_able(m) && m.use_ticks < 0
    }

    pub(crate) fn spear_goal_can_continue(&mut self, g: &SpearGoal, m: &Mob) -> bool {
        g.on && !g.done && Self::spear_goal_able(m)
    }

    pub(crate) fn spear_goal_start(&mut self, g: &mut SpearGoal, m: &mut Mob) {
        m.aggressive = true;
        *g = SpearGoal { on: true, engage: -1, fleeing: -1, away: None, done: false };
    }

    pub(crate) fn spear_goal_stop(&mut self, g: &mut SpearGoal, m: &mut Mob) {
        m.nav.stop();
        m.aggressive = false;
        *g = SpearGoal::new();
        end_use(m);
    }

    pub(crate) fn spear_goal_tick(&mut self, g: &mut SpearGoal, m: &mut Mob) {
        if !g.on {
            return;
        }
        let Some(t) = m.target else { return };
        let (Some(tp), Some(eye)) = (self.ent_pos(t), self.ent_eye(t)) else { return };
        let d2 = dist2(m.pos, tp);
        // Goal speeds (1.0) multiplied by that of the mount; extended retreats if passenger.
        let (sp, md) = self.spear_mount(m);
        self.look_at(m, [tp[0], eye, tp[2]], 30.0, 30.0);
        if g.engage < 0 {
            // Approach (10 blocks) then start of use.
            if d2 > 100.0 {
                self.move_to_entity(m, tp, sp);
                return;
            }
            g.engage = reduced(main_kin(m).map_or(0, |k| k.use_duration()));
            begin_use(m);
        }
        if g.engage > 0 {
            g.engage -= 1;
            if g.engage == 0 {
                end_use(m);
                let dist = d2.sqrt();
                g.away = self.pig_land_pos_away(m, (9.0 + md - dist).max(0.0), (11.0 + md - dist).max(1.0), 7, tp);
                g.fleeing = 1;
            }
        }
        let flee_over = if g.fleeing > 0 {
            g.fleeing += 1;
            if g.fleeing > reduced(100) {
                g.done = true;
                true
            } else {
                false
            }
        } else {
            false
        };
        if flee_over {
            return;
        }
        if let Some(a) = g.away {
            self.move_to_pos(m, a, sp);
            if m.nav.is_done() {
                if g.fleeing > 0 {
                    g.done = true;
                    return;
                }
                g.away = None;
            }
        } else {
            self.move_to_entity(m, tp, sp);
            if d2 < 4.0 || m.nav.is_done() {
                let dist = d2.sqrt();
                g.away = self.pig_land_pos_away(m, 6.0 + md - dist, 7.0 + md - dist, 7, tp);
            }
        }
    }
}
