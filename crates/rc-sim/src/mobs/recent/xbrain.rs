//! Extension of the generic F10 brain (`brainy/brain.rs`) for family F14: memories,
//! sensors and behaviors specific to the warden, the breeze, the creaking and the happy ghast.
//! The F10 engine keeps control (sort by priority, activities, durations drawn on the world random);
//! it delegates the `K::X`, `Bk::X` and `Sn::X` variants here.

use super::super::brainy::brain::{Bk, Brain, Ctl, K, Slot, St, Track, Walk};
use super::super::mob::{Ent, Mob};
use super::super::mth::dist2;
use super::super::species::Kind;
use crate::server::Server;

/// Own memories (all "registered" for the species that use them).
#[derive(Debug, Clone, Default)]
pub struct XMem {
    // Breeze.
    pub jump_cooldown: Slot<()>,
    pub jump_inhaling: Slot<()>,
    pub jump_target: Slot<[i32; 3]>,
    pub shoot: Slot<()>,
    pub shoot_charging: Slot<()>,
    pub shoot_recovering: Slot<()>,
    pub shoot_cooldown: Slot<()>,
    pub leaving_water: Slot<()>,
    // Warden.
    pub dig_cooldown: Slot<()>,
    pub is_emerging: Slot<()>,
    pub roar_target: Slot<Ent>,
    pub disturbance: Slot<[i32; 3]>,
    pub is_sniffing: Slot<()>,
    pub sniff_cooldown: Slot<()>,
    pub roar_sound_delay: Slot<()>,
    pub roar_sound_cooldown: Slot<()>,
    pub sonic_cooldown: Slot<()>,
    pub sonic_sound_cooldown: Slot<()>,
    pub sonic_sound_delay: Slot<()>,
    pub recent_projectile: Slot<()>,
    pub touch_cooldown: Slot<()>,
    pub vibration_cooldown: Slot<()>,
}

/// Keys of the own memories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XK {
    JumpCooldown,
    JumpInhaling,
    JumpTarget,
    Shoot,
    ShootCharging,
    ShootRecovering,
    ShootCooldown,
    LeavingWater,
    DigCooldown,
    IsEmerging,
    RoarTarget,
    Disturbance,
    IsSniffing,
    SniffCooldown,
    RoarSoundDelay,
    RoarSoundCooldown,
    SonicCooldown,
    SonicSoundCooldown,
    SonicSoundDelay,
    RecentProjectile,
    TouchCooldown,
    VibrationCooldown,
}

impl XMem {
    pub fn tick(&mut self) {
        self.jump_cooldown.tick();
        self.jump_inhaling.tick();
        self.jump_target.tick();
        self.shoot.tick();
        self.shoot_charging.tick();
        self.shoot_recovering.tick();
        self.shoot_cooldown.tick();
        self.leaving_water.tick();
        self.dig_cooldown.tick();
        self.is_emerging.tick();
        self.roar_target.tick();
        self.disturbance.tick();
        self.is_sniffing.tick();
        self.sniff_cooldown.tick();
        self.roar_sound_delay.tick();
        self.roar_sound_cooldown.tick();
        self.sonic_cooldown.tick();
        self.sonic_sound_cooldown.tick();
        self.sonic_sound_delay.tick();
        self.recent_projectile.tick();
        self.touch_cooldown.tick();
        self.vibration_cooldown.tick();
    }

    pub fn has(&self, k: XK) -> bool {
        match k {
            XK::JumpCooldown => self.jump_cooldown.has(),
            XK::JumpInhaling => self.jump_inhaling.has(),
            XK::JumpTarget => self.jump_target.has(),
            XK::Shoot => self.shoot.has(),
            XK::ShootCharging => self.shoot_charging.has(),
            XK::ShootRecovering => self.shoot_recovering.has(),
            XK::ShootCooldown => self.shoot_cooldown.has(),
            XK::LeavingWater => self.leaving_water.has(),
            XK::DigCooldown => self.dig_cooldown.has(),
            XK::IsEmerging => self.is_emerging.has(),
            XK::RoarTarget => self.roar_target.has(),
            XK::Disturbance => self.disturbance.has(),
            XK::IsSniffing => self.is_sniffing.has(),
            XK::SniffCooldown => self.sniff_cooldown.has(),
            XK::RoarSoundDelay => self.roar_sound_delay.has(),
            XK::RoarSoundCooldown => self.roar_sound_cooldown.has(),
            XK::SonicCooldown => self.sonic_cooldown.has(),
            XK::SonicSoundCooldown => self.sonic_sound_cooldown.has(),
            XK::SonicSoundDelay => self.sonic_sound_delay.has(),
            XK::RecentProjectile => self.recent_projectile.has(),
            XK::TouchCooldown => self.touch_cooldown.has(),
            XK::VibrationCooldown => self.vibration_cooldown.has(),
        }
    }

    pub fn erase(&mut self, k: XK) {
        match k {
            XK::JumpCooldown => self.jump_cooldown.erase(),
            XK::JumpInhaling => self.jump_inhaling.erase(),
            XK::JumpTarget => self.jump_target.erase(),
            XK::Shoot => self.shoot.erase(),
            XK::ShootCharging => self.shoot_charging.erase(),
            XK::ShootRecovering => self.shoot_recovering.erase(),
            XK::ShootCooldown => self.shoot_cooldown.erase(),
            XK::LeavingWater => self.leaving_water.erase(),
            XK::DigCooldown => self.dig_cooldown.erase(),
            XK::IsEmerging => self.is_emerging.erase(),
            XK::RoarTarget => self.roar_target.erase(),
            XK::Disturbance => self.disturbance.erase(),
            XK::IsSniffing => self.is_sniffing.erase(),
            XK::SniffCooldown => self.sniff_cooldown.erase(),
            XK::RoarSoundDelay => self.roar_sound_delay.erase(),
            XK::RoarSoundCooldown => self.roar_sound_cooldown.erase(),
            XK::SonicCooldown => self.sonic_cooldown.erase(),
            XK::SonicSoundCooldown => self.sonic_sound_cooldown.erase(),
            XK::SonicSoundDelay => self.sonic_sound_delay.erase(),
            XK::RecentProjectile => self.recent_projectile.erase(),
            XK::TouchCooldown => self.touch_cooldown.erase(),
            XK::VibrationCooldown => self.vibration_cooldown.erase(),
        }
    }
}

/// Own sensors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XSn {
    /// Nearby entities, then the first attackable (neither creative nor
    /// spectator).
    BreezeAttack,
    /// Nearby entities, then the closest targetable player, failing that
    /// another targetable entity.
    WardenEntity,
    /// The adult sensor of any type (happy ghast).
    AdultAnyType,
}

impl XSn {
    pub fn period(self) -> i32 {
        20
    }
}

/// Own condition added to an F10 behavior (overridden).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hook {
    /// Creaking: only if it can move.
    CanMove,
    /// Breeze: slide (sound, slide pose, shot allowed when stopped).
    Slide,
}

/// Target search of `StartAttacking`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Find {
    /// Creaking: active, closest visible attackable player.
    CreakingPlayer,
    /// Breeze: memory of the closest attackable target.
    Attackable,
    /// Breeze: author of the last damage, if alive.
    HurtBy,
}

/// Stop condition of the attack stop when the target becomes invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// Creaking: the target is no longer a visible attackable player.
    CreakingUnreachable,
    /// Breeze: not attackable for 100 evaluations.
    BreezeForgot,
    /// Warden: no longer angry, or target not targetable (anger erased), without tiredness.
    Warden,
}

/// Own behaviors.
#[derive(Debug, Clone)]
pub enum XBk {
    Wrap {
        inner: Box<Bk>,
        hook: Hook,
    },
    StartAttack(Find),
    StopAttack {
        why: Stop,
        positives: i32,
    },
    /// The look-target choice (warden): looks at its attack target within tracking range.
    LookAtTarget,
    /// The behavior of the baby following the adult toward the closest visible player (happy ghastling).
    FollowPlayer {
        lo: i32,
        hi: i32,
        speed: f32,
    },
    /// The behavior of the baby following the adult toward the adult, followed target at eye height.
    FollowAdultEye {
        lo: i32,
        hi: i32,
        speed: f32,
    },
    /// The bait tracking at eye height.
    TemptEye {
        speed: f32,
        close: f64,
    },
    // Breeze.
    BreezeShoot,
    BreezeJump,
    BreezeStuck,
    BreezeSlide,
    // Warden.
    WardenLook,
    Emerging,
    ForceUnmount,
    Digging,
    DigCooldown,
    SetRoarTarget,
    TryToSniff,
    Roar,
    Sniffing,
    SonicBoom,
    GoToDisturbance,
}

use K::X as KX;
use XK::*;

impl XBk {
    pub(crate) fn ctl(&self) -> Ctl {
        match self {
            XBk::Wrap { inner, .. } => inner.ctl(),
            XBk::TemptEye { .. } => Ctl::Timed(60, 60),
            XBk::BreezeShoot => Ctl::Timed(20, 20),
            XBk::BreezeJump => Ctl::Timed(200, 200),
            XBk::BreezeStuck | XBk::BreezeSlide | XBk::ForceUnmount => Ctl::Timed(60, 60),
            XBk::Emerging => Ctl::Timed(super::warden::EMERGE_DURATION, super::warden::EMERGE_DURATION),
            XBk::Digging => Ctl::Timed(super::warden::DIGGING_DURATION, super::warden::DIGGING_DURATION),
            XBk::Roar => Ctl::Timed(super::warden::ROAR_DURATION, super::warden::ROAR_DURATION),
            XBk::Sniffing => Ctl::Timed(super::warden::SNIFFING_DURATION, super::warden::SNIFFING_DURATION),
            XBk::SonicBoom => Ctl::Timed(super::warden::SONIC_DURATION, super::warden::SONIC_DURATION),
            _ => Ctl::OneShot,
        }
    }

    /// The behavior never expires (overridden).
    pub fn never_times_out(&self) -> bool {
        matches!(self, XBk::TemptEye { .. })
    }

    pub fn entry(&self) -> &'static [(K, St)] {
        use St::*;
        match self {
            XBk::Wrap { inner, .. } => inner.entry(),
            XBk::StartAttack(_) => &[(K::Attack, Absent), (K::CantReach, Registered)],
            XBk::StopAttack { .. } => &[(K::Attack, Present), (K::CantReach, Registered)],
            XBk::LookAtTarget => &[(K::Look, Absent), (K::Visible, Present)],
            XBk::FollowPlayer { .. } => &[(K::VisiblePlayer, Present), (K::Look, Registered), (K::Walk, Absent)],
            XBk::FollowAdultEye { .. } => &[(K::Adult, Present), (K::Look, Registered), (K::Walk, Absent)],
            XBk::TemptEye { .. } => &[
                (K::Look, Registered),
                (K::Walk, Registered),
                (K::TemptCd, Absent),
                (K::Tempting, Present),
                (K::Breed, Absent),
                (K::Panicking, Absent),
            ],
            XBk::BreezeShoot => &[
                (K::Attack, Present),
                (KX(ShootCooldown), Absent),
                (KX(ShootCharging), Absent),
                (KX(ShootRecovering), Absent),
                (KX(Shoot), Present),
                (K::Walk, Absent),
                (KX(JumpTarget), Absent),
            ],
            XBk::BreezeJump => &[
                (K::Attack, Present),
                (KX(JumpCooldown), Absent),
                (KX(JumpInhaling), Registered),
                (KX(JumpTarget), Registered),
                (KX(Shoot), Absent),
                (K::Walk, Absent),
                (KX(LeavingWater), Registered),
            ],
            XBk::BreezeStuck => &[
                (K::Attack, Present),
                (KX(JumpInhaling), Absent),
                (KX(JumpTarget), Absent),
                (K::Walk, Absent),
                (KX(Shoot), Absent),
            ],
            XBk::BreezeSlide => {
                &[(K::Attack, Present), (K::Walk, Absent), (KX(JumpCooldown), Absent), (KX(Shoot), Absent)]
            }
            XBk::WardenLook => &[
                (K::Look, Registered),
                (KX(Disturbance), Registered),
                (KX(RoarTarget), Registered),
                (K::Attack, Absent),
            ],
            XBk::Emerging => &[(KX(IsEmerging), Present), (K::Walk, Absent), (K::Look, Registered)],
            XBk::ForceUnmount => &[],
            XBk::Digging => &[(K::Attack, Absent), (K::Walk, Absent)],
            XBk::DigCooldown => &[(KX(DigCooldown), Registered)],
            XBk::SetRoarTarget => &[(KX(RoarTarget), Absent), (K::Attack, Absent), (K::CantReach, Registered)],
            XBk::TryToSniff => &[
                (KX(IsSniffing), Registered),
                (K::Walk, Registered),
                (KX(SniffCooldown), Absent),
                (K::Attackable, Present),
                (KX(Disturbance), Absent),
            ],
            XBk::Roar => &[
                (KX(RoarTarget), Present),
                (K::Attack, Absent),
                (KX(RoarSoundCooldown), Registered),
                (KX(RoarSoundDelay), Registered),
            ],
            XBk::Sniffing => &[
                (KX(IsSniffing), Present),
                (K::Attack, Absent),
                (K::Walk, Absent),
                (K::Look, Registered),
                (K::Attackable, Registered),
                (KX(Disturbance), Registered),
                (KX(SniffCooldown), Registered),
            ],
            XBk::SonicBoom => &[
                (K::Attack, Present),
                (KX(SonicCooldown), Absent),
                (KX(SonicSoundCooldown), Registered),
                (KX(SonicSoundDelay), Registered),
            ],
            XBk::GoToDisturbance => {
                &[(KX(Disturbance), Present), (K::Attack, Absent), (K::Walk, Absent), (K::Look, Registered)]
            }
        }
    }
}

/// Entity targetable by the species: breeze limited to the player and the iron golem,
/// warden according to the targeting test.
pub(crate) fn can_attack(s: &Server, m: &Mob, e: Ent) -> bool {
    match m.kind {
        Kind::Breeze => {
            let ok_kind = match e {
                Ent::Player => true,
                Ent::Mob(id) => s.mob_ref(id).is_some_and(|o| o.kind == Kind::IronGolem),
            };
            ok_kind && s.bz_mob_can_attack(m, e)
        }
        Kind::Warden => super::warden::can_target(s, m, e),
        _ => s.bz_mob_can_attack(m, e),
    }
}

impl Server {
    /// Own sensors.
    pub(crate) fn x_sense(&mut self, m: &mut Mob, b: &mut Brain, x: XSn) {
        use super::super::brainy::brain::Sn;
        if x == XSn::AdultAnyType {
            // Closest visible adult of the friendly-mob tag
            // to follow.
            if b.mem.visible.has() {
                let f = |s: &Server, e: Ent| match e {
                    Ent::Mob(id) => s.mob_ref(id).is_some_and(|o| {
                        !o.is_baby() && s.entity_in(o.kind, &["#minecraft:followable_friendly_mobs".to_owned()])
                    }),
                    Ent::Player => false,
                };
                let a =
                    self.bz_visible_closest(m, b, &f).and_then(|e| if let Ent::Mob(id) = e { Some(id) } else { None });
                b.mem.adult.set_opt(a);
            }
            return;
        }
        // The two other sensors inherit from the nearby-entities sensor (list recomputed).
        self.bz_sense(m, b, Sn::Living);
        let list: Vec<Ent> = b.mem.living.v.clone().unwrap_or_default();
        match x {
            XSn::BreezeAttack => {
                let mut found = None;
                for e in list {
                    if e == Ent::Player && matches!(self.game_mode, 1 | 3) {
                        continue;
                    }
                    if self.bz_brain_attackable(m, b, e) {
                        found = Some(e);
                        break;
                    }
                }
                b.mem.attackable.set_opt(found);
            }
            XSn::WardenEntity => {
                let ok = |s: &Server, e: Ent| super::warden::can_target(s, m, e);
                let found = list
                    .iter()
                    .copied()
                    .find(|&e| e == Ent::Player && ok(self, e))
                    .or_else(|| list.iter().copied().find(|&e| e != Ent::Player && ok(self, e)));
                b.mem.attackable.set_opt(found);
            }
            XSn::AdultAnyType => {}
        }
    }

    /// Instant triggers.
    pub(crate) fn x_trigger(&mut self, m: &mut Mob, b: &mut Brain, x: &mut XBk, t: i64) -> bool {
        match x {
            XBk::Wrap { inner, hook } => self.x_hook_ok(m, *hook) && self.bz_trigger(m, b, inner, t),
            XBk::StartAttack(find) => {
                let target = match find {
                    Find::CreakingPlayer => {
                        if !m.rec.active {
                            return false;
                        }
                        b.mem.attackable_player.get()
                    }
                    Find::Attackable => b.mem.attackable.get(),
                    Find::HurtBy => {
                        if b.mem.hurt_by.has() {
                            m.last_hit.as_ref().and_then(|h| h.by)
                        } else {
                            None
                        }
                    }
                };
                let Some(target) = target else { return false };
                if !can_attack(self, m, target) {
                    return false;
                }
                b.mem.attack.set(target);
                b.mem.cant_reach.erase();
                true
            }
            XBk::StopAttack { why, positives } => {
                let Some(target) = b.mem.attack.get() else { return true };
                let tired = b.mem.cant_reach.get().is_some_and(|c| self.time - c > 200);
                let grows_tired = *why != Stop::Warden;
                let mut ok = can_attack(self, m, target) && !(grows_tired && tired) && self.ent_alive(target);
                if ok {
                    let stop = match why {
                        Stop::CreakingUnreachable => {
                            !(target == Ent::Player && b.mem.attackable_player.get() == Some(Ent::Player))
                        }
                        Stop::BreezeForgot => {
                            // Counter specific to the
                            // behavior, reset to 100 on each positive test.
                            if self.bz_brain_attackable(m, b, target) {
                                *positives = 100;
                                false
                            } else {
                                *positives -= 1;
                                *positives < 0
                            }
                        }
                        Stop::Warden => {
                            !super::warden::anger_level(m).angry() || !super::warden::can_target(self, m, target)
                        }
                    };
                    ok = !stop;
                }
                if ok {
                    return true;
                }
                if *why == Stop::Warden {
                    super::warden::on_target_invalid(self, m, b, target);
                }
                b.mem.attack.erase();
                true
            }
            XBk::LookAtTarget => {
                let Some(target) = b.mem.attack.get() else { return false };
                let range = m.follow_range();
                let max = range * range;
                let me = m.pos;
                let passengers = m.mount.passengers.clone();
                let f = move |s: &Server, e: Ent| {
                    e == target && s.ent_pos(e).is_some_and(|p| dist2(p, me) <= max) && !passengers.contains(&e)
                };
                let Some(e) = self.bz_visible_closest(m, b, &f) else { return false };
                b.mem.look.set(Track::ent(e, true));
                true
            }
            XBk::FollowPlayer { lo, hi, speed } => {
                let target = b.mem.visible_player.get();
                self.x_follow(m, b, target, *lo, *hi, *speed)
            }
            XBk::FollowAdultEye { lo, hi, speed } => {
                let target = b.mem.adult.get().map(Ent::Mob);
                self.x_follow(m, b, target, *lo, *hi, *speed)
            }
            XBk::WardenLook => super::warden::look_trigger(self, m, b),
            XBk::DigCooldown => {
                if b.mem.x.dig_cooldown.has() {
                    b.mem.x.dig_cooldown.set_exp((), 1200);
                }
                true
            }
            XBk::SetRoarTarget => super::warden::set_roar_target(self, m, b),
            XBk::TryToSniff => super::warden::try_to_sniff(self, m, b),
            XBk::GoToDisturbance => {
                let Some(loc) = b.mem.x.disturbance.get() else { return false };
                let bp = m.block_pos();
                // Squared distance of block corners < 4.
                let d2 = (0..3).map(|i| f64::from(loc[i] - bp[i]).powi(2)).sum::<f64>();
                if d2 >= 4.0 {
                    let r = &mut self.mobs.rng;
                    let dx = r.next_int_bound(3) - 1;
                    let dz = r.next_int_bound(3) - 1;
                    let p = [loc[0] + dx, loc[1], loc[2] + dz];
                    // The writing of the walk and look memories.
                    b.mem.look.set(Track::block(p));
                    b.mem.walk.set(Walk { target: Track::block(p), speed: 0.7, close: 2 });
                }
                true
            }
            _ => false,
        }
    }

    /// The behavior of the baby following the adult (followed target at eye height).
    fn x_follow(&mut self, m: &mut Mob, b: &mut Brain, target: Option<Ent>, lo: i32, hi: i32, speed: f32) -> bool {
        if !m.is_baby() {
            return false;
        }
        let Some(e) = target else { return false };
        let Some(p) = self.ent_pos(e) else { return false };
        let d = dist2(m.pos, p);
        let (l, h) = (f64::from(lo), f64::from(hi + 1));
        if d < h * h && d >= l * l {
            let tr = Track::Ent { e, eye: true, target_eye: true };
            b.mem.walk.set(Walk { target: tr, speed, close: lo - 1 });
            b.mem.look.set(tr);
            return true;
        }
        false
    }

    fn x_hook_ok(&self, m: &Mob, hook: Hook) -> bool {
        match hook {
            Hook::CanMove => m.rec.can_move,
            Hook::Slide => true,
        }
    }

    /// The additional start conditions.
    pub(crate) fn x_check(&mut self, m: &mut Mob, b: &mut Brain, x: &mut XBk, t: i64) -> bool {
        match x {
            XBk::Wrap { inner, hook } => self.x_hook_ok(m, *hook) && self.bz_bh_check(m, b, inner, t),
            XBk::BreezeShoot => super::breeze::shoot_check(self, m, b),
            XBk::BreezeJump => super::breeze::jump_can_run(self, m, b),
            XBk::BreezeStuck => m.mount.vehicle.is_some() || m.in_water || super::has_effect(m, "minecraft:levitation"),
            XBk::BreezeSlide => m.on_ground && !m.in_water && super::super::brainy::pose(m) == super::POSE_STANDING,
            XBk::ForceUnmount => m.mount.vehicle.is_some(),
            XBk::Digging => m.on_ground || m.in_water || m.lava_height > 0.0,
            XBk::SonicBoom => b
                .mem
                .attack
                .get()
                .and_then(|e| self.ent_pos(e))
                .is_some_and(|p| super::closer_than_xz_y(m.pos, p, 15.0, 20.0)),
            _ => true,
        }
    }

    /// `start`.
    pub(crate) fn x_start(&mut self, m: &mut Mob, b: &mut Brain, x: &mut XBk, t: i64, end: i64) {
        match x {
            XBk::Wrap { inner, hook } => {
                self.bz_bh_start(m, b, inner, t, end);
                if *hook == Hook::Slide {
                    self.mob_sound(m, "minecraft:entity.breeze.slide", 1.0, 1.0);
                    super::super::brainy::set_pose(m, super::POSE_SLIDING);
                }
            }
            XBk::BreezeShoot => super::breeze::shoot_start(self, m, b),
            XBk::BreezeJump => super::breeze::jump_start(self, m, b),
            XBk::BreezeStuck => b.mem.x.shoot.set_exp((), 60),
            XBk::BreezeSlide => super::breeze::slide_start(self, m, b),
            XBk::ForceUnmount => self.x_unride(m),
            XBk::Emerging => super::warden::emerging_start(self, m),
            XBk::Digging => super::warden::digging_start(self, m, b, t, end),
            XBk::Roar => super::warden::roar_start(self, m, b),
            XBk::Sniffing => super::warden::sniffing_start(self, m),
            XBk::SonicBoom => super::warden::sonic_start(self, m, b),
            _ => {}
        }
    }

    /// The continuation test.
    pub(crate) fn x_continue(&mut self, m: &mut Mob, b: &mut Brain, x: &mut XBk, t: i64) -> bool {
        match x {
            XBk::Wrap { inner, .. } => self.bz_bh_continue(m, b, inner, t),
            XBk::TemptEye { .. } => b.mem.tempting.has() && !b.mem.breed.has() && !b.mem.panicking.has(),
            XBk::BreezeShoot => b.mem.attack.has() && b.mem.x.shoot.has(),
            XBk::BreezeJump => super::super::brainy::pose(m) != super::POSE_STANDING && !b.mem.x.jump_cooldown.has(),
            XBk::Digging => !m.removed,
            XBk::Emerging | XBk::Roar | XBk::Sniffing | XBk::SonicBoom => true,
            _ => false,
        }
    }

    /// `tick`.
    pub(crate) fn x_tick(&mut self, m: &mut Mob, b: &mut Brain, x: &mut XBk, t: i64, end: i64) {
        match x {
            XBk::Wrap { inner, .. } => self.bz_bh_tick(m, b, inner, t, end),
            XBk::TemptEye { speed, close } => {
                let Some(player) = b.mem.tempting.get() else { return };
                b.mem.look.set(Track::ent(player, true));
                let pp = self.ent_pos(player).unwrap_or(m.pos);
                if dist2(m.pos, pp) < *close * *close {
                    b.mem.walk.erase();
                } else {
                    let tr = Track::Ent { e: player, eye: true, target_eye: true };
                    b.mem.walk.set(Walk { target: tr, speed: *speed, close: 2 });
                }
            }
            XBk::BreezeShoot => super::breeze::shoot_tick(self, m, b),
            XBk::BreezeJump => super::breeze::jump_tick(self, m, b),
            XBk::Roar => super::warden::roar_tick(self, m, b),
            XBk::SonicBoom => super::warden::sonic_tick(self, m, b),
            _ => {}
        }
    }

    /// `stop`.
    pub(crate) fn x_stop(&mut self, m: &mut Mob, b: &mut Brain, x: &mut XBk, t: i64, end: i64) {
        match x {
            XBk::Wrap { inner, hook } => {
                self.bz_bh_stop(m, b, inner, t, end);
                if *hook == Hook::Slide {
                    super::super::brainy::set_pose(m, super::POSE_STANDING);
                    if b.mem.attack.has() {
                        b.mem.x.shoot.set_exp((), 60);
                    }
                }
            }
            XBk::TemptEye { .. } => {
                b.mem.tempt_cd.set(100);
                b.mem.walk.erase();
                b.mem.look.erase();
            }
            XBk::BreezeShoot => super::breeze::shoot_stop(m, b),
            XBk::BreezeJump => super::breeze::jump_stop(m, b),
            XBk::Emerging => {
                if super::super::brainy::pose(m) == super::POSE_EMERGING {
                    super::super::brainy::set_pose(m, super::POSE_STANDING);
                }
            }
            XBk::Digging => {
                if !m.removed {
                    m.removed = true;
                }
            }
            XBk::Roar => super::warden::roar_stop(self, m, b),
            XBk::Sniffing => super::warden::sniffing_stop(self, m, b),
            XBk::SonicBoom => b.mem.x.sonic_cooldown.set_exp((), 40),
            _ => {}
        }
    }

    /// Dismounting from the vehicle.
    fn x_unride(&mut self, m: &mut Mob) {
        let Some(vid) = m.mount.vehicle else { return };
        let Some(i) = self.mob_index(vid) else { return };
        let Some(mut v) = self.mobs.list[i].take() else { return };
        self.mob_dismount_from(m, &mut v);
        self.mobs.list[i] = Some(v);
    }
}
