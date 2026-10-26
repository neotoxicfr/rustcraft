//! Goals of family F11 (`Goal::Wild`): common goals guarded by an own condition
//! (`Gate`), goals of the bee (hive, flower, pollination, crops, wandering), the fox (sleep,
//! stalking, pounce, shelter, berries, items, perch, defense) and the panda (sitting, on its back,
//! sneezing, rolling, look, breeding requiring bamboo, retaliation).

use super::super::goals::{Goal, JUMP, LOOK, MOVE, TARGET, TBase};
use super::super::mob::{Ent, Mob};
use super::super::mth::{dist2, reduced};
use super::super::species::Kind;
use super::{bee, fox, panda};
use crate::server::Server;

/// Own condition set on a common goal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// Bee: attacks only when angry and without having stung.
    BeeAttack,
    /// Bee: targets the player only when angry and without having stung.
    BeeTarget,
    /// Bee: the retaliation continues only when angry.
    BeeHurtBy,
    /// Fox: panic, except when defending.
    FoxPanic,
    /// Fox: breeding (states cleared at the departure of both parents).
    FoxBreed,
    /// Fox: flees the player if not crouching nor trusting, and without defending.
    FoxAvoidPlayer,
    /// Fox: flees wolves and bears without defending.
    FoxNotDefending,
    /// Fox: melee except sitting, asleep, crouching, face planted.
    FoxMelee,
    /// Fox: follows its parent except defending (states cleared at departure).
    FoxFollowParent,
    /// Fox: looks at the player except face planted and interested.
    FoxLook,
    /// Panda: panic stopped when sitting.
    PandaPanic,
    /// Panda: attack available.
    PandaAction,
    /// Worried and available panda: flee.
    PandaAvoid,
}

#[derive(Debug, Clone)]
pub enum WGoal {
    Gate { gate: Gate, inner: Box<Goal> },
    // Bee.
    EnterHive,
    GoToHive { travel: i32, stuck: i32, last: Option<Vec<[i32; 3]>> },
    GoToFlower { travel: i32 },
    GrowCrop,
    Pollinate { success: i32, last_sound: i32, hover: Option<[f64; 3]>, ticks: i32, unreachable: Vec<([i32; 3], i64)> },
    BeeWander,
    ValidateHive { cd: i32, last: i64 },
    ValidateFlower { cd: i32, last: i64 },
    LocateHive,
    // Fox.
    FoxFloat,
    Faceplant { countdown: i32 },
    StalkPrey,
    Pounce,
    SeekShelter { interval: i32, wanted: [f64; 3] },
    Sleep { countdown: i32 },
    StrollVillage { interval: i32, wanted: Option<[i32; 3]> },
    EatBerries { next_start: i32, try_ticks: i32, max_stay: i32, pos: [i32; 3], reached: bool, waited: i32 },
    SearchItems,
    Perch { rel: [f64; 2], look_time: i32, looks: i32 },
    DefendTrusted { base: TBase, by: Option<Ent>, ts: i32, stamp: i32 },
    // Panda.
    PandaBreed { inner: Box<Goal>, cd: i32 },
    PandaSit { cd: i32 },
    LieOnBack { cd: i32 },
    Sneeze,
    PandaLook { at: Option<Ent>, time: i32 },
    Roll,
    PandaHurtBy { base: TBase, stamp: i32 },
}

impl WGoal {
    pub fn flags(&self) -> u8 {
        match self {
            WGoal::Gate { inner, .. } => inner.flags(),
            WGoal::PandaBreed { inner, .. } => inner.flags(),
            WGoal::GoToHive { .. }
            | WGoal::GoToFlower { .. }
            | WGoal::Pollinate { .. }
            | WGoal::BeeWander
            | WGoal::SeekShelter { .. }
            | WGoal::StrollVillage { .. }
            | WGoal::SearchItems
            | WGoal::PandaSit { .. } => MOVE,
            WGoal::FoxFloat => JUMP,
            WGoal::Faceplant { .. } | WGoal::Sleep { .. } | WGoal::Roll => MOVE | LOOK | JUMP,
            WGoal::StalkPrey | WGoal::Perch { .. } => MOVE | LOOK,
            WGoal::Pounce | WGoal::EatBerries { .. } => MOVE | JUMP,
            WGoal::PandaLook { .. } => LOOK,
            WGoal::DefendTrusted { .. } | WGoal::PandaHurtBy { .. } => TARGET,
            _ => 0,
        }
    }

    pub fn every_tick(&self) -> bool {
        match self {
            WGoal::Gate { inner, .. } | WGoal::PandaBreed { inner, .. } => inner.every_tick(),
            WGoal::Pollinate { .. } | WGoal::FoxFloat | WGoal::EatBerries { .. } => true,
            _ => false,
        }
    }

    /// Fox pounce, panda roll.
    pub fn interruptable(&self) -> bool {
        !matches!(self, WGoal::Pounce | WGoal::Roll)
    }

    fn adjusted(&self, t: i32) -> i32 {
        if self.every_tick() { t } else { reduced(t) }
    }
}

impl Server {
    // ---------- Use ----------

    pub(crate) fn wild_can_use(&mut self, g: &mut WGoal, m: &mut Mob) -> bool {
        match g {
            WGoal::Gate { gate, inner } => self.gate_can_use(*gate, inner, m),
            WGoal::EnterHive => {
                let ok = self.enter_hive_can_use(m);
                ok && !bee::is_angry(self, m)
            }
            WGoal::GoToHive { .. } => self.go_to_hive_ok(m) && !bee::is_angry(self, m),
            WGoal::GoToFlower { .. } => go_to_flower_ok(m) && !bee::is_angry(self, m),
            WGoal::GrowCrop => {
                let ok = self.grow_crop_ok(m);
                ok && !bee::is_angry(self, m)
            }
            WGoal::Pollinate { .. } => {
                let ok = self.pollinate_can_use(g, m);
                ok && !bee::is_angry(self, m)
            }
            WGoal::BeeWander => m.nav.is_done() && m.rng.next_int_bound(10) == 0,
            WGoal::ValidateHive { cd, last } | WGoal::ValidateFlower { cd, last } => {
                self.time > *last + i64::from(*cd) && !bee::is_angry(self, m)
            }
            WGoal::LocateHive => {
                m.wild.hive_cd == 0
                    && m.wild.hive.is_none()
                    && bee::wants_to_enter_hive(self, m)
                    && !bee::is_angry(self, m)
            }
            WGoal::FoxFloat => m.water_height > 0.25 || m.lava_height > 0.0,
            WGoal::Faceplant { .. } => fox::flag(m, fox::FACEPLANTED),
            WGoal::StalkPrey => {
                if fox::flag(m, fox::SLEEPING) {
                    return false;
                }
                let Some(t) = m.target else { return false };
                self.ent_alive(t)
                    && fox::stalkable(self, t)
                    && self.ent_pos(t).is_some_and(|p| dist2(m.pos, p) > 36.0)
                    && !fox::flag(m, fox::CROUCHING)
                    && !fox::flag(m, fox::INTERESTED)
                    && !m.jumping
            }
            WGoal::Pounce => self.pounce_can_use(m),
            WGoal::SeekShelter { interval, wanted } => {
                if fox::flag(m, fox::SLEEPING) || m.target.is_some() {
                    return false;
                }
                let thunder = self.life.weather.thunder_eff() > 0.9;
                if thunder && self.can_see_sky(m.block_pos()) {
                    return self.hide_pos(m, wanted);
                }
                if *interval > 0 {
                    *interval -= 1;
                    return false;
                }
                *interval = 100;
                let p = m.block_pos();
                self.bright_outside() && self.can_see_sky(p) && !self.is_village(p) && self.hide_pos(m, wanted)
            }
            WGoal::Sleep { countdown } => {
                if m.xxa == 0.0 && m.yya == 0.0 && m.zza == 0.0 {
                    self.fox_can_sleep(countdown, m) || fox::flag(m, fox::SLEEPING)
                } else {
                    false
                }
            }
            WGoal::StrollVillage { interval, wanted } => {
                if fox::flag(m, fox::SLEEPING) || fox::flag(m, fox::SITTING) || fox::flag(m, fox::DEFENDING) {
                    return false;
                }
                if m.target.is_some() {
                    return false;
                }
                let i = *interval;
                self.stroll_village_can_use(i, wanted, m)
            }
            WGoal::EatBerries { next_start, pos, .. } => {
                if fox::flag(m, fox::SLEEPING) {
                    return false;
                }
                if *next_start > 0 {
                    *next_start -= 1;
                    return false;
                }
                *next_start = reduced(200 + m.rng.next_int_bound(200));
                match self.find_berries(m) {
                    Some(p) => {
                        *pos = p;
                        true
                    }
                    None => false,
                }
            }
            WGoal::SearchItems => {
                if m.equipment[0].is_some() || m.target.is_some() || m.last_hurt_by_mob.is_some() || !fox::can_move(m) {
                    return false;
                }
                if m.rng.next_int_bound(reduced(10)) != 0 {
                    return false;
                }
                !fox::items_near(self, m).is_empty() && m.equipment[0].is_none()
            }
            WGoal::Perch { .. } => {
                m.last_hurt_by_mob.is_none()
                    && m.rng.next_float() < 0.02
                    && !fox::flag(m, fox::SLEEPING)
                    && m.target.is_none()
                    && m.nav.is_done()
                    && !fox::alertable(self, m)
                    && !fox::flag(m, fox::POUNCING)
                    && !fox::flag(m, fox::CROUCHING)
            }
            WGoal::DefendTrusted { base, by, ts, stamp } => {
                if m.rng.next_int_bound(reduced(10)) != 0 {
                    return false;
                }
                let Some((attacker, t)) = fox::trusted_attacker(self, m) else { return false };
                *by = attacker;
                *ts = t;
                let Some(a) = attacker else { return false };
                if t == *stamp {
                    return false;
                }
                // The selector excludes trusted beings and requires a remembered victim;
                // range: tracking distance, reduced by visibility.
                let trusted = match a {
                    Ent::Player => fox::trusts_player(self, m),
                    Ent::Mob(id) => self.mob_ref(id).is_some_and(|o| fox::trusts(m, o.uuid)),
                };
                !trusted
                    && fox::has_last_hurt_mob(self, a)
                    && self.targeting_ok(m, a, true, m.follow_range(), base.must_see, true)
            }
            WGoal::PandaBreed { inner, cd } => {
                if !self.goal_can_use(inner, m) || m.wild.unhappy != 0 {
                    return false;
                }
                if !panda::can_find_bamboo(self, m) {
                    if *cd <= m.tick_count {
                        m.wild.unhappy = 32;
                        *cd = m.tick_count + 600;
                        let p = self.panda_near_player(m);
                        m.wild.look_set = true;
                        m.wild.look_at_player = p.is_some();
                    }
                    return false;
                }
                true
            }
            WGoal::PandaSit { cd } => {
                if *cd > m.tick_count
                    || m.is_baby()
                    || m.in_water
                    || !panda::can_perform_action(self, m)
                    || m.wild.unhappy > 0
                {
                    return false;
                }
                m.equipment[0].is_some() || !panda::ground_food(self, m, 6.0).is_empty()
            }
            WGoal::LieOnBack { cd } => {
                *cd < m.tick_count
                    && panda::is_lazy(m)
                    && panda::can_perform_action(self, m)
                    && m.rng.next_int_bound(reduced(400)) == 1
            }
            WGoal::Sneeze => {
                if m.is_baby() && panda::can_perform_action(self, m) {
                    (panda::is_weak(m) && m.rng.next_int_bound(reduced(500)) == 1)
                        || m.rng.next_int_bound(reduced(6000)) == 1
                } else {
                    false
                }
            }
            WGoal::PandaLook { at, .. } => {
                Self::panda_look_override(at, m);
                if m.rng.next_float() >= 0.02 {
                    return false;
                }
                if at.is_none() {
                    *at = (!self.carries_player(m) && self.targeting_ok(m, Ent::Player, false, 6.0, true, true))
                        .then_some(Ent::Player);
                }
                panda::can_perform_action(self, m) && at.is_some()
            }
            WGoal::Roll => self.roll_can_use(m),
            WGoal::PandaHurtBy { base, stamp } => {
                let Some((by, ts)) = m.last_hurt_by_mob else { return false };
                if ts == *stamp {
                    return false;
                }
                if by == Ent::Player && self.rule("minecraft:universal_anger", false) {
                    return false;
                }
                self.targeting_ok(m, by, true, -1.0, base.must_see, true)
            }
        }
    }

    pub(crate) fn wild_can_continue(&mut self, g: &mut WGoal, m: &mut Mob) -> bool {
        match g {
            WGoal::Gate { gate, inner } => self.gate_can_continue(*gate, inner, m),
            WGoal::EnterHive | WGoal::LocateHive | WGoal::ValidateHive { .. } | WGoal::ValidateFlower { .. } => false,
            WGoal::GoToHive { .. } => self.go_to_hive_ok(m) && !bee::is_angry(self, m),
            WGoal::GoToFlower { .. } => go_to_flower_ok(m) && !bee::is_angry(self, m),
            WGoal::GrowCrop => {
                let ok = self.grow_crop_ok(m);
                ok && !bee::is_angry(self, m)
            }
            WGoal::Pollinate { success, .. } => {
                let ok = if !m.wild.pollinating || m.wild.flower.is_none() || bee::raining(self) {
                    false
                } else if *success > 400 {
                    m.rng.next_float() < 0.2
                } else {
                    true
                };
                ok && !bee::is_angry(self, m)
            }
            WGoal::BeeWander => m.nav.path.as_ref().is_some_and(|p| !p.is_done()),
            WGoal::FoxFloat => self.wild_can_use(g, m),
            WGoal::Faceplant { countdown } => fox::flag(m, fox::FACEPLANTED) && *countdown > 0,
            WGoal::StalkPrey => self.wild_can_use(g, m),
            WGoal::Pounce => {
                let Some(t) = m.target else { return false };
                if !self.ent_alive(t) {
                    return false;
                }
                let yd = m.vel[1];
                (yd * yd >= f64::from(0.05f32) || m.pitch.abs() >= 15.0 || !m.on_ground)
                    && !fox::flag(m, fox::FACEPLANTED)
            }
            WGoal::SeekShelter { .. } | WGoal::SearchItems => !m.nav.is_done(),
            WGoal::Sleep { countdown } => self.fox_can_sleep(countdown, m),
            WGoal::StrollVillage { wanted, .. } => {
                let fox_ok = !fox::flag(m, fox::SLEEPING)
                    && !fox::flag(m, fox::SITTING)
                    && !fox::flag(m, fox::DEFENDING)
                    && m.target.is_none();
                wanted.is_some() && !m.nav.is_done() && m.nav.target_pos == *wanted && fox_ok
            }
            WGoal::EatBerries { try_ticks, max_stay, pos, .. } => {
                *try_ticks >= -*max_stay && *try_ticks <= 1200 && fox::berry_target(self, *pos)
            }
            WGoal::Perch { looks, .. } => *looks > 0,
            WGoal::DefendTrusted { base, .. } => self.target_continue(base, m),
            WGoal::PandaBreed { inner, .. } => self.goal_can_continue(inner, m),
            WGoal::PandaSit { .. } | WGoal::LieOnBack { .. } => {
                if !m.in_water && (panda::is_lazy(m) || m.rng.next_int_bound(reduced(600)) != 1) {
                    m.rng.next_int_bound(reduced(2000)) != 1
                } else {
                    false
                }
            }
            WGoal::Sneeze | WGoal::Roll => false,
            WGoal::PandaLook { at, time } => {
                Self::panda_look_override(at, m);
                at.is_some_and(|e| {
                    self.ent_alive(e) && self.ent_pos(e).is_some_and(|p| dist2(m.pos, p) <= 36.0) && *time > 0
                })
            }
            WGoal::PandaHurtBy { base, .. } => {
                if m.wild.got_bamboo || m.wild.did_bite {
                    m.target = None;
                    return false;
                }
                self.target_continue(base, m)
            }
        }
    }

    pub(crate) fn wild_start(&mut self, g: &mut WGoal, m: &mut Mob) {
        match g {
            WGoal::Gate { gate, inner } => {
                match gate {
                    Gate::FoxBreed => {
                        fox::clear_states(m);
                        if let Goal::Breed { partner: Some(p), .. } = **inner
                            && let Some(i) = self.mob_index(p)
                            && let Some(o) = self.mobs.list[i].as_mut()
                        {
                            fox::clear_states(o);
                        }
                    }
                    Gate::FoxFollowParent => fox::clear_states(m),
                    Gate::FoxMelee => fox::set(m, fox::INTERESTED, false),
                    _ => {}
                }
                self.goal_start(inner, m);
            }
            WGoal::EnterHive => {
                if let Some(h) = bee::hive_entity(self, m) {
                    super::hive::add_occupant(self, h, m);
                }
            }
            WGoal::GoToHive { travel, stuck, .. } => {
                *travel = 0;
                *stuck = 0;
            }
            WGoal::GoToFlower { travel } => *travel = 0,
            WGoal::Pollinate { success, last_sound, ticks, .. } => {
                *success = 0;
                *ticks = 0;
                *last_sound = 0;
                m.wild.pollinating = true;
                m.wild.no_nectar_ticks = 0;
            }
            WGoal::BeeWander => self.bee_wander_start(m),
            WGoal::ValidateHive { last, .. } => {
                if m.wild.hive.is_some_and(|h| self.chunk_loaded(h)) && !bee::hive_valid(self, m) {
                    bee::drop_hive(&mut m.wild);
                }
                *last = self.time;
            }
            WGoal::ValidateFlower { last, .. } => {
                if let Some(f) = m.wild.flower
                    && self.chunk_loaded(f)
                    && !bee::attracts_bees(self, f)
                {
                    bee::drop_flower(m);
                }
                *last = self.time;
            }
            WGoal::LocateHive => self.locate_hive(m),
            WGoal::FoxFloat => fox::clear_states(m),
            WGoal::Faceplant { countdown } => *countdown = 40 / 2,
            WGoal::StalkPrey => {
                fox::set(m, fox::SITTING, false);
                fox::set(m, fox::FACEPLANTED, false);
            }
            WGoal::Pounce => {
                m.jumping = true;
                fox::set(m, fox::POUNCING, true);
                fox::set(m, fox::INTERESTED, false);
                if let Some(t) = m.target
                    && let (Some(p), Some(eye)) = (self.ent_pos(t), self.ent_eye(t))
                {
                    self.look_at(m, [p[0], eye, p[2]], 60.0, 30.0);
                    let d = [p[0] - m.pos[0], p[1] - m.pos[1], p[2] - m.pos[2]];
                    let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                    let u = if l < 1.0e-4 { [0.0; 3] } else { d.map(|v| v / l) };
                    m.vel = [m.vel[0] + u[0] * 0.8, m.vel[1] + 0.9, m.vel[2] + u[2] * 0.8];
                    m.needs_sync = true;
                }
                m.nav.stop();
            }
            WGoal::SeekShelter { wanted, .. } => {
                fox::clear_states(m);
                let w = *wanted;
                self.move_to_pos(m, w, 1.25);
            }
            WGoal::Sleep { .. } => {
                fox::set(m, fox::SITTING, false);
                fox::set(m, fox::CROUCHING, false);
                fox::set(m, fox::INTERESTED, false);
                m.jumping = false;
                fox::set(m, fox::SLEEPING, true);
                m.nav.stop();
                m.move_ctl.wanted = m.pos;
                m.move_ctl.speed_mod = 0.0;
                m.move_ctl.op = super::super::mob::MoveOp::MoveTo;
            }
            WGoal::StrollVillage { .. } => fox::clear_states(m),
            WGoal::EatBerries { try_ticks, max_stay, pos, waited, .. } => {
                *waited = 0;
                fox::set(m, fox::SITTING, false);
                let p = *pos;
                self.move_to_pos(
                    m,
                    [f64::from(p[0]) + 0.5, f64::from(p[1] + 1), f64::from(p[2]) + 0.5],
                    f64::from(1.2f32),
                );
                *try_ticks = 0;
                let inner = m.rng.next_int_bound(1200) + 1200;
                *max_stay = m.rng.next_int_bound(inner) + 1200;
            }
            WGoal::SearchItems => {
                if let Some(p) = fox::items_near(self, m).first().copied() {
                    self.move_to_entity(m, p, f64::from(1.2f32));
                }
            }
            WGoal::Perch { rel, look_time, looks } => {
                Self::perch_reset(m, rel, look_time);
                *looks = 2 + m.rng.next_int_bound(3);
                fox::set(m, fox::SITTING, true);
                m.nav.stop();
            }
            WGoal::DefendTrusted { base, by, ts, stamp } => {
                *stamp = *ts;
                fox::set_target(m, *by);
                base.target_mob = *by;
                self.mob_sound(m, "minecraft:entity.fox.aggro", 1.0, 1.0);
                fox::set(m, fox::DEFENDING, true);
                fox::set(m, fox::SLEEPING, false);
                Self::target_start(base);
            }
            WGoal::PandaBreed { inner, .. } => self.goal_start(inner, m),
            WGoal::PandaSit { cd } => {
                if m.equipment[0].is_none() {
                    if let Some(p) = panda::ground_food(self, m, 8.0).first().copied() {
                        self.move_to_entity(m, p, f64::from(1.2f32));
                    }
                } else {
                    panda::try_to_sit(m);
                }
                *cd = 0;
            }
            WGoal::LieOnBack { cd } => {
                m.wild.set_flag(panda::ON_BACK, true);
                *cd = 0;
            }
            WGoal::Sneeze => m.wild.set_flag(panda::SNEEZE, true),
            WGoal::PandaLook { time, .. } => *time = reduced(40 + m.rng.next_int_bound(40)),
            WGoal::Roll => m.wild.set_flag(panda::ROLL, true),
            WGoal::PandaHurtBy { base, stamp } => {
                m.target = m.last_hurt_by_mob.map(|h| h.0);
                base.target_mob = m.target;
                *stamp = m.last_hurt_by_mob.map_or(0, |h| h.1);
                base.unseen_memory = 300;
                self.panda_alert_others(m);
                Self::target_start(base);
            }
            _ => {}
        }
    }

    pub(crate) fn wild_stop(&mut self, g: &mut WGoal, m: &mut Mob) {
        match g {
            WGoal::Gate { gate, inner } => {
                if *gate == Gate::BeeTarget
                    && let Goal::Nearest { base, .. } = &mut **inner
                {
                    base.target_mob = None;
                }
                self.goal_stop(inner, m);
            }
            WGoal::GoToHive { travel, stuck, .. } => {
                *travel = 0;
                *stuck = 0;
                m.nav.stop();
                bee::reset_visited_multiplier(m);
            }
            WGoal::GoToFlower { travel } => {
                *travel = 0;
                m.nav.stop();
                bee::reset_visited_multiplier(m);
            }
            WGoal::Pollinate { success, .. } => {
                if *success > 400 {
                    bee::set_nectar(&mut m.wild, true);
                }
                m.wild.pollinating = false;
                m.nav.stop();
                m.wild.flower_cd = 200;
            }
            WGoal::Faceplant { .. } => fox::set(m, fox::FACEPLANTED, false),
            WGoal::StalkPrey => {
                let tp = m.target.and_then(|t| self.ent_pos(t).zip(self.ent_eye(t)));
                match tp {
                    Some((p, eye)) if fox::path_clear(self, m, p) => {
                        fox::set(m, fox::INTERESTED, true);
                        fox::set(m, fox::CROUCHING, true);
                        m.nav.stop();
                        self.look_at(m, [p[0], eye, p[2]], 75.0, 40.0);
                    }
                    _ => {
                        fox::set(m, fox::INTERESTED, false);
                        fox::set(m, fox::CROUCHING, false);
                    }
                }
            }
            WGoal::Pounce => {
                fox::set(m, fox::CROUCHING, false);
                m.wild.crouch = 0.0;
                fox::set(m, fox::INTERESTED, false);
                fox::set(m, fox::POUNCING, false);
            }
            WGoal::Sleep { countdown } => {
                *countdown = m.rng.next_int_bound(reduced(140));
                fox::clear_states(m);
            }
            WGoal::Perch { .. } => fox::set(m, fox::SITTING, false),
            WGoal::DefendTrusted { base, .. } | WGoal::PandaHurtBy { base, .. } => {
                if m.kind == Kind::Fox {
                    fox::set_target(m, None);
                } else {
                    m.target = None;
                }
                base.target_mob = None;
            }
            WGoal::PandaBreed { inner, .. } => self.goal_stop(inner, m),
            WGoal::PandaSit { cd } => {
                if let Some(st) = m.equipment[0].take() {
                    self.spawn_at_location(m, st, 0.0);
                    let secs =
                        if panda::is_lazy(m) { m.rng.next_int_bound(50) + 10 } else { m.rng.next_int_bound(150) + 10 };
                    *cd = m.tick_count + secs * 20;
                }
                m.wild.set_flag(panda::SIT, false);
            }
            WGoal::LieOnBack { cd } => {
                m.wild.set_flag(panda::ON_BACK, false);
                *cd = m.tick_count + 200;
            }
            WGoal::PandaLook { at, .. } => *at = None,
            _ => {}
        }
    }

    pub(crate) fn wild_tick(&mut self, g: &mut WGoal, m: &mut Mob) {
        match g {
            WGoal::Gate { inner, .. } => self.goal_tick(inner, m),
            WGoal::GoToHive { .. } => self.go_to_hive_tick(g, m),
            WGoal::GoToFlower { travel } => {
                let Some(f) = m.wild.flower else { return };
                *travel += 1;
                if *travel > reduced(2400) {
                    bee::drop_flower(m);
                } else if m.nav.path.as_ref().is_none_or(|p| p.is_done()) {
                    if bee::too_far(m, f) {
                        bee::drop_flower(m);
                    } else {
                        bee::pathfind_randomly_towards(self, m, f);
                    }
                }
            }
            WGoal::GrowCrop => {
                let n = g.adjusted(30);
                if m.rng.next_int_bound(n) == 0 {
                    self.grow_crops_below(m);
                }
            }
            WGoal::Pollinate { .. } => self.pollinate_tick(g, m),
            WGoal::FoxFloat => {
                if m.rng.next_float() < 0.8 {
                    m.jump_ctl = true;
                }
            }
            WGoal::Faceplant { countdown } => *countdown -= 1,
            WGoal::StalkPrey => {
                let Some(t) = m.target else { return };
                let (Some(p), Some(eye)) = (self.ent_pos(t), self.ent_eye(t)) else { return };
                self.look_at(m, [p[0], eye, p[2]], 75.0, 40.0);
                if dist2(m.pos, p) <= 36.0 {
                    fox::set(m, fox::INTERESTED, true);
                    fox::set(m, fox::CROUCHING, true);
                    m.nav.stop();
                } else {
                    self.move_to_entity(m, p, 1.5);
                }
            }
            WGoal::Pounce => self.pounce_tick(m),
            WGoal::StrollVillage { wanted, .. } => {
                let Some(w) = *wanted else { return };
                if m.nav.is_done() && !bee::near_center(w, m.pos, 10.0) {
                    self.stroll_village_step(m, w);
                }
            }
            WGoal::EatBerries { .. } => self.eat_berries_tick(g, m),
            WGoal::SearchItems => {
                let items = fox::items_near(self, m);
                if m.equipment[0].is_none()
                    && let Some(p) = items.first().copied()
                {
                    self.move_to_entity(m, p, f64::from(1.2f32));
                }
            }
            WGoal::Perch { rel, look_time, looks } => {
                *look_time -= 1;
                if *look_time <= 0 {
                    *looks -= 1;
                    Self::perch_reset(m, rel, look_time);
                }
                let eye = m.eye_y();
                let to = [m.pos[0] + rel[0], eye, m.pos[2] + rel[1]];
                self.look_at(m, to, 75.0, 40.0);
            }
            WGoal::PandaBreed { inner, .. } => self.goal_tick(inner, m),
            WGoal::PandaSit { .. } => {
                if !m.wild.flag(panda::SIT) && m.equipment[0].is_some() {
                    panda::try_to_sit(m);
                }
            }
            WGoal::PandaLook { at, time } => {
                Self::panda_look_override(at, m);
                if let Some(e) = *at
                    && self.ent_alive(e)
                    && let (Some(p), Some(eye)) = (self.ent_pos(e), self.ent_eye(e))
                {
                    self.look_at(m, [p[0], eye, p[2]], 10.0, 40.0);
                    *time -= 1;
                }
            }
            _ => {}
        }
    }

    // ---------- Guards ----------

    fn gate_can_use(&mut self, gate: Gate, inner: &mut Goal, m: &mut Mob) -> bool {
        match gate {
            Gate::BeeAttack => self.goal_can_use(inner, m) && bee::is_angry(self, m) && !bee::has_stung(m),
            Gate::BeeTarget => bee::is_angry(self, m) && !bee::has_stung(m) && self.goal_can_use(inner, m),
            Gate::BeeHurtBy => self.goal_can_use(inner, m),
            Gate::FoxPanic => !fox::flag(m, fox::DEFENDING) && self.goal_can_use(inner, m),
            Gate::FoxBreed => self.goal_can_use(inner, m),
            Gate::FoxAvoidPlayer => {
                !fox::flag(m, fox::DEFENDING)
                    && !fox::trusts_player(self, m)
                    && !self.sneaking
                    && self.goal_can_use(inner, m)
            }
            Gate::FoxNotDefending => !fox::flag(m, fox::DEFENDING) && self.goal_can_use(inner, m),
            Gate::FoxMelee => {
                !fox::flag(m, fox::SITTING)
                    && !fox::flag(m, fox::SLEEPING)
                    && !fox::flag(m, fox::CROUCHING)
                    && !fox::flag(m, fox::FACEPLANTED)
                    && self.goal_can_use(inner, m)
            }
            Gate::FoxFollowParent => !fox::flag(m, fox::DEFENDING) && self.goal_can_use(inner, m),
            Gate::FoxLook => {
                self.goal_can_use(inner, m) && !fox::flag(m, fox::FACEPLANTED) && !fox::flag(m, fox::INTERESTED)
            }
            Gate::PandaPanic => self.goal_can_use(inner, m),
            Gate::PandaAction => panda::can_perform_action(self, m) && self.goal_can_use(inner, m),
            Gate::PandaAvoid => {
                panda::is_worried(m) && panda::can_perform_action(self, m) && self.goal_can_use(inner, m)
            }
        }
    }

    fn gate_can_continue(&mut self, gate: Gate, inner: &mut Goal, m: &mut Mob) -> bool {
        match gate {
            Gate::BeeAttack => self.goal_can_continue(inner, m) && bee::is_angry(self, m) && !bee::has_stung(m),
            Gate::BeeTarget => {
                if bee::is_angry(self, m) && !bee::has_stung(m) && m.target.is_some() {
                    return self.goal_can_continue(inner, m);
                }
                if let Goal::Nearest { base, .. } = inner {
                    base.target_mob = None;
                }
                false
            }
            Gate::BeeHurtBy => bee::is_angry(self, m) && self.goal_can_continue(inner, m),
            Gate::FoxFollowParent => !fox::flag(m, fox::DEFENDING) && self.goal_can_continue(inner, m),
            Gate::FoxLook => {
                self.goal_can_continue(inner, m) && !fox::flag(m, fox::FACEPLANTED) && !fox::flag(m, fox::INTERESTED)
            }
            Gate::PandaPanic => {
                if m.wild.flag(panda::SIT) {
                    m.nav.stop();
                    return false;
                }
                self.goal_can_continue(inner, m)
            }
            _ => self.goal_can_continue(inner, m),
        }
    }

    // ---------- Bee ----------

    fn enter_hive_can_use(&mut self, m: &mut Mob) -> bool {
        let Some(h) = m.wild.hive else { return false };
        if bee::wants_to_enter_hive(self, m) && bee::near_center(h, m.pos, 2.0) && bee::hive_entity(self, m).is_some() {
            if !super::hive::is_full(self, h) {
                return true;
            }
            m.wild.hive = None;
        }
        false
    }

    fn go_to_hive_ok(&self, m: &Mob) -> bool {
        let Some(h) = m.wild.hive else { return false };
        !bee::too_far(m, h)
            && bee::wants_to_enter_hive(self, m)
            && !bee::closer_than(m, h, 2)
            && !path_reached(m, h)
            && self.btag_id(h, crate::tag_bits::TagId::Beehives)
    }

    fn go_to_hive_tick(&mut self, g: &mut WGoal, m: &mut Mob) {
        let WGoal::GoToHive { travel, stuck, last } = g else { return };
        let Some(h) = m.wild.hive else { return };
        *travel += 1;
        if *travel > reduced(2400) {
            drop_and_blacklist(m);
            return;
        }
        if m.nav.path.as_ref().is_some_and(|p| !p.is_done()) {
            return;
        }
        if !bee::closer_than(m, h, 16) {
            if bee::too_far(m, h) {
                bee::drop_hive(&mut m.wild);
            } else {
                bee::pathfind_randomly_towards(self, m, h);
            }
            return;
        }
        // Direct path: range 1 very close, otherwise 2; tenfold search.
        let reach = if bee::closer_than(m, h, 3) { 1 } else { 2 };
        bee::set_visited_multiplier(m, 10.0);
        let path = self.create_path(m, h, reach);
        self.move_to_path(m, path, 1.0);
        let reaches = m.nav.path.as_ref().is_some_and(|p| p.reached);
        if !reaches {
            drop_and_blacklist(m);
            return;
        }
        let nodes: Option<Vec<[i32; 3]>> = m.nav.path.as_ref().map(|p| p.nodes.iter().map(|n| n.0).collect());
        if last.is_some() && nodes == *last {
            *stuck += 1;
            if *stuck > 60 {
                bee::drop_hive(&mut m.wild);
                *stuck = 0;
            }
        } else {
            *last = nodes;
        }
    }

    fn locate_hive(&mut self, m: &mut Mob) {
        m.wild.hive_cd = 200;
        let center = m.block_pos();
        let recs = self.poi_in_range(
            |t| matches!(t, crate::poi::PoiType::Beehive | crate::poi::PoiType::BeeNest),
            center,
            20,
            crate::poi::Occupancy::Any,
        );
        let mut with_space: Vec<[i32; 3]> = recs
            .iter()
            .map(|r| r.pos)
            .filter(|p| super::hive::is_hive_entity(self, *p) && !super::hive::is_full(self, *p))
            .collect();
        with_space.sort_by_key(|p| crate::poi::dist2i(*p, center));
        if with_space.is_empty() {
            return;
        }
        for p in &with_space {
            if !m.wild.blacklist.contains(p) {
                m.wild.hive = Some(*p);
                return;
            }
        }
        m.wild.blacklist.clear();
        m.wild.hive = Some(with_space[0]);
    }

    fn grow_crop_ok(&mut self, m: &mut Mob) -> bool {
        if m.wild.crops >= 10 {
            return false;
        }
        if m.rng.next_float() < 0.3 {
            return false;
        }
        bee::has_nectar(m) && bee::hive_valid(self, m)
    }

    /// Crop growth under the bee (one and two cells below): crops +1 age, stems
    /// +1, berry bush +1, cave vines (bone meal); event 2011.
    fn grow_crops_below(&mut self, m: &mut Mob) {
        for i in 1..=2 {
            let b = m.block_pos();
            let p = [b[0], b[1] - i, b[2]];
            if !self.btag_id(p, crate::tag_bits::TagId::BeeGrowables) {
                continue;
            }
            let st = self.st(p);
            let name = self.bname(p).to_owned();
            let age = |s: &Server| {
                s.data.state_properties(st).find(|(k, _)| *k == "age").and_then(|(_, v)| v.parse::<i32>().ok())
            };
            let grown = if name.ends_with("_stem") && !name.contains("attached") {
                age(self).filter(|a| *a < 7).and_then(|a| self.with_prop(st, "age", &(a + 1).to_string()))
            } else if name == "minecraft:sweet_berry_bush" {
                age(self).filter(|a| *a < 3).and_then(|a| self.with_prop(st, "age", &(a + 1).to_string()))
            } else if name == "minecraft:cave_vines" || name == "minecraft:cave_vines_plant" {
                let berries = self.data.state_properties(st).any(|kv| kv == ("berries", "true"));
                if berries { None } else { self.with_prop(st, "berries", "true") }
            } else if name == "minecraft:pitcher_crop" {
                // Double plant, not a simple crop: the bee does not make it grow.
                None
            } else if name == "minecraft:torchflower_crop" {
                // Age 0 -> 1; age 2 (maximal) is the adult flower, a separate block.
                age(self).filter(|a| *a < 1).map_or_else(
                    || age(self).filter(|a| *a == 1).map(|_| self.default_state("minecraft:torchflower")),
                    |a| self.with_prop(st, "age", &(a + 1).to_string()),
                )
            } else {
                // Crop: max age according to the plant.
                let max = crop_max_age(&name);
                age(self).filter(|a| *a < max).and_then(|a| self.with_prop(st, "age", &(a + 1).to_string()))
            };
            if let Some(ns) = grown {
                self.level_event(2011, p, 15);
                self.set_block(p, ns, 3);
                m.wild.crops += 1;
            }
        }
    }

    fn pollinate_can_use(&mut self, g: &mut WGoal, m: &mut Mob) -> bool {
        if m.wild.flower_cd > 0 || bee::has_nectar(m) || bee::raining(self) {
            return false;
        }
        let WGoal::Pollinate { unreachable, .. } = g else { return false };
        match self.find_flower(m, unreachable) {
            Some(f) => {
                m.wild.flower = Some(f);
                let to = [f64::from(f[0]) + 0.5, f64::from(f[1]) + 0.5, f64::from(f[2]) + 0.5];
                self.move_to_pos(m, to, f64::from(1.2f32));
                true
            }
            None => {
                m.wild.flower_cd = m.rng.int_between(20, 60);
                false
            }
        }
    }

    /// Manhattan cube 5, reachable attractive flowers (path to 1);
    /// the unreachable ones are discarded for 600 ticks.
    fn find_flower(&mut self, m: &mut Mob, cache: &mut Vec<([i32; 3], i64)>) -> Option<[i32; 3]> {
        let mut temp: Vec<([i32; 3], i64)> = Vec::new();
        let mut found = None;
        for p in bee::manhattan_box(m.block_pos(), 5, 5, 5) {
            if !bee::attracts_bees(self, p) {
                continue;
            }
            let until = cache.iter().find(|(q, _)| *q == p).map_or(i64::MIN, |c| c.1);
            if self.time < until {
                temp.push((p, until));
                continue;
            }
            let path = self.create_path(m, p, 1);
            if path.as_ref().is_some_and(|q| q.reached) {
                found = Some(p);
                break;
            }
            temp.push((p, self.time + 600));
        }
        *cache = temp;
        found
    }

    fn pollinate_tick(&mut self, g: &mut WGoal, m: &mut Mob) {
        let WGoal::Pollinate { success, last_sound, hover, ticks, .. } = g else { return };
        let Some(f) = m.wild.flower else { return };
        *ticks += 1;
        if *ticks > 600 {
            bee::drop_flower(m);
            m.wild.pollinating = false;
            m.wild.flower_cd = 200;
            return;
        }
        let fp = [f64::from(f[0]) + 0.5, f64::from(f[1]) + f64::from(0.6f32), f64::from(f[2]) + 0.5];
        if dist2(fp, m.pos).sqrt() > 1.0 {
            *hover = Some(fp);
            set_wanted(m, fp);
            return;
        }
        let h = *hover.get_or_insert(fp);
        let arrived = dist2(m.pos, h).sqrt() <= 0.1;
        let mut set_pos = true;
        if !arrived && *ticks > 600 {
            bee::drop_flower(m);
            return;
        }
        if arrived {
            if m.rng.next_int_bound(25) == 0 {
                let ox = (m.rng.next_float() * 2.0 - 1.0) * 0.333_333_34;
                let oz = (m.rng.next_float() * 2.0 - 1.0) * 0.333_333_34;
                *hover = Some([fp[0] + f64::from(ox), fp[1], fp[2] + f64::from(oz)]);
                m.nav.stop();
            } else {
                set_pos = false;
            }
            self.look_at(m, fp, 10.0, 40.0);
        }
        if set_pos && let Some(h) = *hover {
            set_wanted(m, h);
        }
        *success += 1;
        if m.rng.next_float() < 0.05 && *success > *last_sound + 60 {
            *last_sound = *success;
            self.mob_sound(m, "minecraft:entity.bee.pollinate", 1.0, 1.0);
        }
    }

    /// Start of the bee's wander goal: toward the hive if it is far, otherwise straight ahead; hover
    /// above the ground, otherwise aerial position.
    fn bee_wander_start(&mut self, m: &mut Mob) {
        let threshold = 48 - if m.wild.hive.is_none() && m.wild.flower.is_none() { 16 } else { 24 };
        let dir = match bee::hive_entity(self, m) {
            Some(h) if !bee::closer_than(m, h, threshold) => {
                let c = [f64::from(h[0]) + 0.5, f64::from(h[1]) + 0.5, f64::from(h[2]) + 0.5];
                let d = [c[0] - m.pos[0], c[1] - m.pos[1], c[2] - m.pos[2]];
                let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                if l < 1.0e-4 { [0.0; 3] } else { d.map(|v| v / l) }
            }
            _ => view_vector(m),
        };
        let half_pi = std::f32::consts::FRAC_PI_2;
        let p = super::rpos::hover_pos(self, m, 8, 7, dir[0], dir[2], half_pi, 3, 1)
            .or_else(|| super::rpos::air_and_water_pos(self, m, 8, 4, -2, dir[0], dir[2], f64::from(half_pi)));
        if let Some(p) = p {
            let path = self.create_path(m, super::super::mth::block_of(p), 1);
            self.move_to_path(m, path, 1.0);
        }
    }

    fn chunk_loaded(&self, p: [i32; 3]) -> bool {
        self.chunks.get(&(p[0] >> 4, p[2] >> 4)).is_some_and(Option::is_some)
    }

    // ---------- Fox ----------

    fn fox_can_sleep(&mut self, countdown: &mut i32, m: &mut Mob) -> bool {
        if *countdown > 0 {
            *countdown -= 1;
            return false;
        }
        self.bright_outside()
            && fox::has_shelter(self, m)
            && !fox::alertable(self, m)
            && self.bname(m.block_pos()) != "minecraft:powder_snow"
    }

    /// Ten tries at ±10/±3/±10 without sky and negative walk value.
    fn hide_pos(&mut self, m: &mut Mob, wanted: &mut [f64; 3]) -> bool {
        let b = m.block_pos();
        for _ in 0..10 {
            let p = [
                b[0] + m.rng.next_int_bound(20) - 10,
                b[1] + m.rng.next_int_bound(6) - 3,
                b[2] + m.rng.next_int_bound(20) - 10,
            ];
            if !self.can_see_sky(p) && self.walk_target_value(m, p) < 0.0 {
                *wanted = [f64::from(p[0]) + 0.5, f64::from(p[1]), f64::from(p[2]) + 0.5];
                return true;
            }
        }
        false
    }

    fn pounce_can_use(&mut self, m: &mut Mob) -> bool {
        if m.wild.crouch != 5.0 {
            return false;
        }
        let Some(t) = m.target else { return false };
        if !self.ent_alive(t) {
            return false;
        }
        // Movement direction == look direction for any living being: never blocking.
        let Some(tp) = self.ent_pos(t) else { return false };
        let clear = fox::path_clear(self, m, tp);
        if !clear {
            let _ = self.create_path(m, super::super::mth::block_of(tp), 0);
            fox::set(m, fox::CROUCHING, false);
            fox::set(m, fox::INTERESTED, false);
        }
        clear
    }

    fn pounce_tick(&mut self, m: &mut Mob) {
        let target = m.target;
        if let Some(t) = target
            && let (Some(p), Some(eye)) = (self.ent_pos(t), self.ent_eye(t))
        {
            self.look_at(m, [p[0], eye, p[2]], 60.0, 30.0);
        }
        if !fox::flag(m, fox::FACEPLANTED) {
            let v = m.vel;
            if v[1] * v[1] < f64::from(0.03f32) && m.pitch != 0.0 {
                m.pitch = super::super::mth::rotlerp(m.pitch, 0.0, 0.2);
            } else {
                let horiz = (v[0] * v[0] + v[2] * v[2]).sqrt();
                let bias = if m.jumping && v[1] > 0.0 { 6.5f32 } else { 1.0 };
                let by = v[1] * f64::from(bias);
                let len = (horiz * horiz + by * by).sqrt();
                if len > f64::from(1.0e-5f32) {
                    let r =
                        (-by).signum() * (horiz / len).acos() * f64::from(180.0f32) / f64::from(std::f32::consts::PI);
                    m.pitch = r as f32;
                }
            }
        }
        let close = target.and_then(|t| self.ent_pos(t)).is_some_and(|p| (dist2(m.pos, p).sqrt() as f32) <= 2.0);
        if let Some(t) = target
            && close
        {
            self.mob_attack(m, t);
        } else if m.pitch > 0.0 && m.on_ground && (m.vel[1] as f32) != 0.0 && fox::in_snow(self, m) {
            m.pitch = 60.0;
            fox::set_target(m, None);
            fox::set(m, fox::FACEPLANTED, true);
        }
    }

    fn stroll_village_can_use(&mut self, interval: i32, wanted: &mut Option<[i32; 3]>, m: &mut Mob) -> bool {
        if self.bright_outside() {
            return false;
        }
        if m.rng.next_int_bound(interval) != 0 {
            return false;
        }
        let p = m.block_pos();
        if self.sections_to_village(p) > 6 {
            return false;
        }
        let land = super::rpos::land_pos(self, m, 15, 7, |s, _, q| {
            // Score "minus the number of sections to the village": the cell closest to a village.
            -f64::from(s.mobs.poi.sections_to_village((q[0] >> 4, q[1] >> 4, q[2] >> 4)))
        });
        *wanted = land.map(super::super::mth::block_of);
        wanted.is_some()
    }

    fn stroll_village_step(&mut self, m: &mut Mob, w: [i32; 3]) {
        let target = [f64::from(w[0]) + 0.5, f64::from(w[1]), f64::from(w[2]) + 0.5];
        let me = m.pos;
        let dist = [me[0] - target[0], me[1] - target[1], me[2] - target[2]];
        let far = [dist[0] * 0.4 + target[0], dist[1] * 0.4 + target[1], dist[2] * 0.4 + target[2]];
        let d = [far[0] - me[0], far[1] - me[1], far[2] - me[2]];
        let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let n = if l < 1.0e-4 { [0.0; 3] } else { d.map(|v| v / l) };
        let mv = [n[0] * 10.0 + me[0], n[1] * 10.0 + me[1], n[2] * 10.0 + me[2]];
        let pt = super::super::mth::block_of(mv);
        let y = self.surface_y(pt[0], pt[2], 5);
        let goal = [f64::from(pt[0]), f64::from(y), f64::from(pt[2])];
        if !self.move_to_pos(m, goal, 1.0) {
            let b = m.block_pos();
            let x = b[0] - 8 + m.rng.next_int_bound(16);
            let z = b[2] - 8 + m.rng.next_int_bound(16);
            let y = self.surface_y(x, z, 5);
            self.move_to_pos(m, [f64::from(x), f64::from(y), f64::from(z)], 1.0);
        }
    }

    fn find_berries(&mut self, m: &mut Mob) -> Option<[i32; 3]> {
        let o = m.block_pos();
        let (range, vrange) = (12, 1);
        let mut y = 0;
        while y <= vrange {
            for r in 0..range {
                let mut x = 0;
                while x <= r {
                    let mut z = if x < r && x > -r { r } else { 0 };
                    while z <= r {
                        let p = [o[0] + x, o[1] + y - 1, o[2] + z];
                        if fox::berry_target(self, p) {
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

    fn eat_berries_tick(&mut self, g: &mut WGoal, m: &mut Mob) {
        let WGoal::EatBerries { try_ticks, pos, reached, waited, .. } = g else { return };
        if *reached {
            if *waited >= 40 {
                let p = *pos;
                fox::pick_berries(self, m, p);
            } else {
                *waited += 1;
            }
        } else if m.rng.next_float() < 0.05 {
            self.mob_sound(m, "minecraft:entity.fox.sniff", 1.0, 1.0);
        }
        // Target above the block, accepted at 2, path every 100.
        let t = [pos[0], pos[1] + 1, pos[2]];
        if !bee::near_center(t, m.pos, 2.0) {
            *reached = false;
            *try_ticks += 1;
            if *try_ticks % 100 == 0 {
                self.move_to_pos(m, [f64::from(t[0]) + 0.5, f64::from(t[1]), f64::from(t[2]) + 0.5], f64::from(1.2f32));
            }
        } else {
            *reached = true;
            *try_ticks -= 1;
        }
    }

    /// The reset of the perch gaze (reduced delay: the goal does not tick every tick).
    fn perch_reset(m: &mut Mob, rel: &mut [f64; 2], look_time: &mut i32) {
        let a = std::f64::consts::TAU * m.rng.next_double();
        *rel = [a.cos(), a.sin()];
        *look_time = reduced(80 + m.rng.next_int_bound(20));
    }

    // ---------- Panda ----------

    fn roll_can_use(&mut self, m: &mut Mob) -> bool {
        if !((m.is_baby() || panda::is_playful(m)) && m.on_ground) {
            return false;
        }
        if !panda::can_perform_action(self, m) {
            return false;
        }
        let a = m.yaw * super::super::mth::DEG;
        let (xd, zd) = (-super::super::mth::sin(a), super::super::mth::cos(a));
        let xs = if xd.abs() > 0.5 { xd.signum() as i32 } else { 0 };
        let zs = if zd.abs() > 0.5 { zd.signum() as i32 } else { 0 };
        let b = m.block_pos();
        if self.is_air([b[0] + xs, b[1] - 1, b[2] + zs]) {
            return true;
        }
        (panda::is_playful(m) && m.rng.next_int_bound(reduced(60)) == 1) || m.rng.next_int_bound(reduced(500)) == 1
    }

    fn panda_look_override(at: &mut Option<Ent>, m: &mut Mob) {
        if m.wild.look_set {
            m.wild.look_set = false;
            *at = m.wild.look_at_player.then_some(Ent::Player);
        }
    }

    /// Panda retaliation: only aggressive pandas without a target join.
    fn panda_alert_others(&mut self, m: &Mob) {
        let follow = m.follow_range();
        let p = m.pos;
        let area = rc_physics::Aabb {
            min: [p[0] - follow, p[1] - 10.0, p[2] - follow],
            max: [p[0] + 1.0 + follow, p[1] + 1.0 + 10.0, p[2] + 1.0 + follow],
        };
        let Some((by, _)) = m.last_hurt_by_mob else { return };
        for o in self.mobs.list.iter_mut().flatten() {
            if o.kind == Kind::Panda
                && o.id != m.id
                && o.target.is_none()
                && o.touches(&area)
                && panda::is_aggressive(o)
            {
                o.target = Some(by);
            }
        }
    }
}

fn go_to_flower_ok(m: &Mob) -> bool {
    let Some(f) = m.wild.flower else { return false };
    m.wild.no_nectar_ticks > 600 && !bee::closer_than(m, f, 2)
}

/// The current path reaches the hive and is finished.
fn path_reached(m: &Mob, h: [i32; 3]) -> bool {
    if bee::closer_than(m, h, 2) {
        return true;
    }
    m.nav.path.as_ref().is_some_and(|p| p.target == h && p.reached && p.is_done())
}

/// 3 hives retained at most.
fn drop_and_blacklist(m: &mut Mob) {
    if let Some(h) = m.wild.hive {
        m.wild.blacklist.push(h);
        while m.wild.blacklist.len() > 3 {
            m.wild.blacklist.remove(0);
        }
    }
    bee::drop_hive(&mut m.wild);
}

fn set_wanted(m: &mut Mob, p: [f64; 3]) {
    m.move_ctl.wanted = p;
    m.move_ctl.speed_mod = f64::from(0.35f32);
    m.move_ctl.op = super::super::mob::MoveOp::MoveTo;
}

/// Gaze vector.
fn view_vector(m: &Mob) -> [f64; 3] {
    use super::super::mth::{DEG, cos, sin};
    let (xr, yr) = (m.pitch * DEG, -m.yaw * DEG);
    let (yc, ys) = (cos(yr), sin(yr));
    let (xc, xs) = (cos(xr), sin(xr));
    [f64::from(ys * xc), f64::from(-xs), f64::from(yc * xc)]
}

/// Maximum age of a crop.
fn crop_max_age(name: &str) -> i32 {
    match name {
        "minecraft:beetroots" => 3,
        _ => 7,
    }
}
