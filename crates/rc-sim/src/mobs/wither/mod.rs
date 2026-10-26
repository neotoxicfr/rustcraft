//! Wither (family F16, `server-mobs.md` § 4.F16): three-headed flying boss. Sheet,
//! own state, metadata, NBT, goals and hooks called by the common engine (`mobs/`). The
//! behavior (summoning, heads, shots, block destruction, boss bar, damage rules)
//! is in `boss.rs`, the block construction in `summon.rs` (patterns of `golems/build.rs`),
//! the shot skulls in `skull.rs` (common projectile engine, kind `WitherSkull`).

pub(crate) mod boss;
pub(crate) mod skull;
pub(crate) mod summon;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod world_tests;

use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use super::goals::{Goal, JUMP, LOOK, MOVE, TBase, Who};
use super::mob::{Ent, Mob, MoveOp};
use super::mth;
use super::species::{Category, Info, Kind, info as base_info};
use crate::nbtio::{self, int};
use crate::server::Server;

// ---------- Species info ----------

/// Flight speed (`flying_speed` 0.6).
pub const FLYING_SPEED: f64 = 0.6f32 as f64;

/// Wither: 0.9 × 3.5 (eyes at 2.975), HP 300, speed 0.6, tracking 40, armor 4, 50 experience,
/// immune to fire, client tracking 10 chunks, no velocity tracking.
pub static WITHER: Info = {
    let mut i = base_info("minecraft:wither", Category::Monster, 0.9, 3.5, 3.5 * 0.85);
    i.health = 300.0;
    i.speed = 0.6f32 as f64;
    i.follow = 40.0;
    i.armor = 4.0;
    i.xp = 50;
    i.tracking = 10;
    i.not_in_peaceful = true;
    i.ambient = Some("minecraft:entity.wither.ambient");
    i.hurt = "minecraft:entity.wither.hurt";
    i.death = "minecraft:entity.wither.death";
    i.fire_immune = true;
    // Tag `fall_damage_immune`: neither damage nor fall sound, nor arming of the destruction.
    i.fall_immune = true;
    i
};

/// Duration of the summoning (ticks).
pub const INVULNERABLE_TICKS: i32 = 220;

/// Type without velocity tracking.
pub fn no_track_deltas(k: Kind) -> bool {
    k == Kind::Wither
}

// ---------- Own state ----------

/// State of the wither (`wither` field of the mob).
#[derive(Debug, Clone)]
pub struct Wither {
    /// Remaining summoning ticks (index 19).
    pub inv: i32,
    /// Targets of the three heads: network ids, 0 without a target (indices 16, 17, 18).
    pub targets: [i32; 3],
    /// Side heads: next choice (tick) and pending "random" shots.
    pub next_head_update: [i32; 2],
    pub idle_head_updates: [i32; 2],
    /// Countdown before destroying the blocks around (after a hit taken).
    pub destroy_blocks_tick: i32,
    /// Rotations of the side heads (computed by the server, never sent).
    pub x_rot_heads: [f32; 2],
    pub y_rot_heads: [f32; 2],
    /// Boss bar: UUID (drawn at construction), name, progress, shown to the player,
    /// custom name already applied.
    pub bar: u128,
    pub bar_name: Nbt,
    pub bar_progress: f32,
    pub bar_shown: bool,
    pub bar_custom: Option<Nbt>,
}

impl Default for Wither {
    fn default() -> Wither {
        Wither {
            inv: 0,
            targets: [0; 3],
            next_head_update: [0; 2],
            idle_head_updates: [0; 2],
            destroy_blocks_tick: 0,
            x_rot_heads: [0.0; 2],
            y_rot_heads: [0.0; 2],
            bar: 0,
            bar_name: Nbt::Compound(Vec::new()),
            bar_progress: 1.0,
            bar_shown: false,
            bar_custom: None,
        }
    }
}

/// World state specific to the family.
#[derive(Debug, Clone, Default)]
pub struct WitherWorld {
    /// Explosion of a blue skull in progress: resistance of destructible blocks capped at 0.8.
    pub dangerous_blast: bool,
    /// Wither being ticked (out of the list): author of its summoning explosion.
    pub ticking: Option<i32>,
}

impl Server {
    /// Resistance of a block to the explosion in progress (blue skull: 0.8 at most if destructible).
    pub(crate) fn wither_blast_resistance(&self, p: [i32; 3], r: f32) -> f32 {
        if self.mobs.wither.dangerous_blast && boss::can_destroy(self, p) { r.min(0.8) } else { r }
    }
}

/// Displayed name: custom name or type name, "entity" hover and
/// UUID insertion.
pub fn display_name(m: &Mob) -> Nbt {
    let name = m
        .custom_name
        .clone()
        .unwrap_or_else(|| Nbt::Compound(vec![("translate".into(), Nbt::String("entity.minecraft.wither".into()))]));
    let mut f = match name.clone() {
        Nbt::Compound(f) => f,
        Nbt::String(s) => vec![("text".into(), Nbt::String(s))],
        other => vec![("text".into(), Nbt::String(format!("{other:?}")))],
    };
    f.push((
        "hover_event".into(),
        Nbt::Compound(vec![
            ("action".into(), Nbt::String("show_entity".into())),
            ("id".into(), Nbt::String("minecraft:wither".into())),
            ("uuid".into(), nbtio::uuid_nbt(m.uuid)),
            ("name".into(), name),
        ]),
    ));
    f.push(("insertion".into(), Nbt::String(nbtio::uuid_text(m.uuid))));
    Nbt::Compound(f)
}

/// Construction (creation of the wither): the bar draws its UUID from the entity's random source (after that
/// of the entity) and takes the display name of the moment.
pub fn on_new(m: &mut Mob) {
    let hi = m.rng.next_long() as u64;
    let lo = m.rng.next_long() as u64;
    let hi = hi & 0xFFFF_FFFF_FFFF_0FFF | 0x4000;
    let lo = lo & 0x3FFF_FFFF_FFFF_FFFF | 0x8000_0000_0000_0000;
    m.wither.bar = u128::from(hi) << 64 | u128::from(lo);
    m.wither.bar_name = display_name(m);
}

/// Flying navigation (closed doors, floats).
pub fn init_nav(m: &mut Mob) {
    if m.kind == Kind::Wither {
        m.nav.fly = true;
        m.nav.can_float = true;
        m.nav.can_open_doors = false;
    }
}

pub fn define_data(m: &mut Mob) {
    // Data of the three targets and the invulnerability time.
    for i in 16..=19 {
        m.define(i, Meta::Int(0));
    }
}

pub fn sync_data(m: &mut Mob) {
    m.set(5, Meta::Bool(m.surf.no_gravity));
    for i in 0..3 {
        m.set(16 + i as u8, Meta::Int(m.wither.targets[i]));
    }
    m.set(19, Meta::Int(m.wither.inv));
}

pub fn load(m: &mut Mob, e: &Nbt) {
    m.wither.inv = int(e, "Invul").unwrap_or(0) as i32;
    m.surf.no_gravity = int(e, "NoGravity").is_some_and(|v| v != 0);
    if m.custom_name.is_some() {
        m.wither.bar_name = display_name(m);
        m.wither.bar_custom = m.custom_name.clone();
    }
}

pub fn save(m: &Mob, e: &mut Nbt) {
    nbtio::set(e, "Invul", Nbt::Int(m.wither.inv));
    if m.surf.no_gravity {
        nbtio::set(e, "NoGravity", Nbt::Byte(1));
    } else {
        nbtio::remove(e, "NoGravity");
    }
    if let Some(Nbt::List(list)) = nbtio::get_mut(e, "attributes") {
        for (id, base) in [("minecraft:flying_speed", FLYING_SPEED), ("minecraft:armor", m.attrs.armor.base)] {
            list.retain(|a| a.get("id").and_then(Nbt::as_str) != Some(id));
            list.push(Nbt::Compound(vec![("id".into(), Nbt::String(id.into())), ("base".into(), Nbt::Double(base))]));
        }
    }
}

/// Mute (`Silent`).
pub fn silent(m: &Mob) -> bool {
    m.nbt.as_ref().and_then(|n| int(n, "Silent")).is_some_and(|v| v != 0)
}

/// Under half of its HP: armor (immune to arrows), particles.
pub fn powered(m: &Mob) -> bool {
    m.health <= m.max_health() / 2.0
}

// ---------- Goals ----------

#[derive(Debug, Clone)]
pub enum WGoal {
    /// The wither's "do nothing" goal: blocks movement, jump and look during the summoning.
    DoNothing,
    /// Center head shot.
    Ranged { target: Option<Ent>, attack_time: i32, see_time: i32 },
    /// The random flight goal away from water (interval 120).
    FlyStroll { wanted: [f64; 3] },
}

impl WGoal {
    pub fn wrap(self) -> Goal {
        Goal::Wither(self)
    }

    pub fn flags(&self) -> u8 {
        match self {
            WGoal::DoNothing => MOVE | JUMP | LOOK,
            WGoal::Ranged { .. } => MOVE | LOOK,
            WGoal::FlyStroll { .. } => MOVE,
        }
    }

    pub fn every_tick(&self) -> bool {
        matches!(self, WGoal::Ranged { .. })
    }
}

/// Goals and targets in the game's add order.
pub fn register_goals(m: &mut Mob) {
    let g = &mut m.goals;
    g.add(0, WGoal::DoNothing.wrap());
    g.add(2, WGoal::Ranged { target: None, attack_time: -1, see_time: 0 }.wrap());
    g.add(5, WGoal::FlyStroll { wanted: [0.0; 3] }.wrap());
    g.add(6, Goal::LookAtPlayer { dist: 8.0, prob: 0.02, at: None, time: 0 });
    g.add(7, super::surface::look_around());
    let t = &mut m.targets;
    t.add(1, Goal::HurtBy { base: TBase::new(true, false), alert: false, stamp: 0 });
    // Interval 0 (no draw), view not memorized but checked by the conditions.
    t.add(
        2,
        Goal::Nearest {
            base: TBase::new(false, false),
            who: Who::WitherPrey,
            interval: 0,
            target: None,
            angry_only: false,
            non_tame: false,
        },
    );
}

const RADIUS: f32 = 20.0;
const INTERVAL: i32 = 40;

impl Server {
    pub(crate) fn wither_can_use(&mut self, g: &mut WGoal, m: &mut Mob) -> bool {
        match g {
            WGoal::DoNothing => m.wither.inv > 0,
            WGoal::Ranged { target, .. } => match m.target.filter(|t| self.ent_alive(*t)) {
                Some(t) => {
                    *target = Some(t);
                    true
                }
                None => false,
            },
            WGoal::FlyStroll { wanted } => {
                // Inaction, 1 chance in 120, position.
                if m.no_action_time >= 100 || m.rng.next_int_bound(mth::reduced(120)) != 0 {
                    return false;
                }
                match self.fly_wander_pos(m) {
                    Some(p) => {
                        *wanted = p;
                        true
                    }
                    None => false,
                }
            }
        }
    }

    pub(crate) fn wither_can_continue(&mut self, g: &mut WGoal, m: &mut Mob) -> bool {
        match g {
            WGoal::DoNothing => m.wither.inv > 0,
            WGoal::Ranged { .. } => {
                if self.wither_can_use(g, m) {
                    return true;
                }
                let WGoal::Ranged { target, .. } = g else { return false };
                target.is_some_and(|t| self.ent_alive(t)) && !m.nav.is_done()
            }
            WGoal::FlyStroll { .. } => !m.nav.is_done(),
        }
    }

    pub(crate) fn wither_start(&mut self, g: &mut WGoal, m: &mut Mob) {
        if let WGoal::FlyStroll { wanted } = g {
            let w = *wanted;
            self.move_to_pos(m, w, 1.0);
        }
    }

    pub(crate) fn wither_stop(&mut self, g: &mut WGoal, m: &mut Mob) {
        match g {
            WGoal::Ranged { target, attack_time, see_time } => {
                *target = None;
                *see_time = 0;
                *attack_time = -1;
            }
            WGoal::FlyStroll { .. } => m.nav.stop(),
            WGoal::DoNothing => {}
        }
    }

    /// The ranged attack goal tick (speed 1, interval 40, radius 20).
    pub(crate) fn wither_goal_tick(&mut self, g: &mut WGoal, m: &mut Mob) {
        let WGoal::Ranged { target, attack_time, see_time } = g else { return };
        let Some(t) = *target else { return };
        let (Some(tp), Some(eye)) = (self.ent_pos(t), self.ent_eye(t)) else { return };
        let d2 = mth::dist2(m.pos, tp);
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
            // Power (distance / radius) has no effect on the skull; interval
            // `⌊d × (max − min) + min⌋` = 40 (min = max).
            boss::shoot_at(self, m, 0, t);
            *attack_time = INTERVAL;
        } else if *attack_time < 0 {
            // Interpolation between equal min and max: constant.
            *attack_time = INTERVAL;
        }
    }
}

// ---------- Engine hooks ----------

/// Flight movement control at the own flight speed.
pub fn move_control(m: &mut Mob) -> bool {
    if m.kind != Kind::Wither {
        return false;
    }
    if m.move_ctl.op == MoveOp::MoveTo {
        m.move_ctl.op = MoveOp::Wait;
        m.surf.no_gravity = true;
        let w = m.move_ctl.wanted;
        let (xd, yd, zd) = (w[0] - m.pos[0], w[1] - m.pos[1], w[2] - m.pos[2]);
        let dd = xd * xd + yd * yd + zd * zd;
        if dd < f64::from(2.500_000_3e-7f32) {
            m.yya = 0.0;
            m.zza = 0.0;
            return true;
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
        // Without hovering: gravity returns when stopped.
        m.surf.no_gravity = false;
        m.yya = 0.0;
        m.zza = 0.0;
    }
    true
}

/// The wither refuses every effect (always false).
pub fn refuses_effects(k: Kind) -> bool {
    k == Kind::Wither
}

/// Never despawns outside peaceful, inaction reset to zero; `None`:
/// common rule.
pub fn check_despawn(m: &mut Mob) -> Option<bool> {
    if m.kind != Kind::Wither {
        return None;
    }
    m.no_action_time = 0;
    Some(false)
}
