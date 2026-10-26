//! Happy ghast (`server-mobs.md` § 4.F14): baby ("ghastling", scale 0.2375) driven
//! by a brain (panic, tempting, follows the player or an adult, random flight) and a simple
//! flight control; adult driven by goals (float, tempting without path, random floating
//! away from blocks) and the ghast's "careful" control. Harness (body slot): four-seat
//! mount piloted by the first player passenger; motionless for 10 ticks when a player
//! stands on it; home area of 32 (64 adult without harness); slow healing (fast in the
//! rain or in clouds).

use rc_proto::nbt::Nbt;

use super::super::brainy::brain::{Act, Bk, Brain, K, Sn, Spd, St, act, at, core, move_sink, run_one, seq};
use super::super::goals::{Goal, LOOK, MOVE};
use super::super::jrand::JRand;
use super::super::mob::{Ent, Mob, MoveOp};
use super::super::mounts::BODY;
use super::super::species::Kind;
use super::xbrain::{XBk, XSn};
use crate::nbtio;
use crate::server::Server;

/// Scale of the baby.
pub const BABY_SCALE: f32 = 0.2375;
/// Flight speed (`flying_speed`).
const FLYING_SPEED: f64 = 0.05;

// ---------- Adult goals ----------

#[derive(Debug, Clone)]
pub enum RGoal {
    /// Happy ghast float goal (floats in a liquid, except when motionless).
    Float,
    /// The bait objective (speed 1, stop distance 7).
    Tempt { player: Option<Ent>, calm: i32, running: bool },
    /// "Float randomly" goal of range 16.
    RandomFloat,
}

impl RGoal {
    pub fn wrap(self) -> Goal {
        Goal::Rec(self)
    }

    pub fn flags(&self) -> u8 {
        match self {
            RGoal::Float => super::super::goals::JUMP,
            RGoal::Tempt { .. } => MOVE | LOOK,
            RGoal::RandomFloat => MOVE,
        }
    }

    pub fn every_tick(&self) -> bool {
        false
    }
}

pub fn register_goals(m: &mut Mob) {
    m.goals = super::super::goals::Selector::default();
    m.targets = super::super::goals::Selector::default();
    m.goals.add(3, RGoal::Float.wrap());
    m.goals.add(4, RGoal::Tempt { player: None, calm: 0, running: false }.wrap());
    m.goals.add(5, RGoal::RandomFloat.wrap());
    m.rec.ghast_baby = Some(false);
    // The float objective makes the navigation floating.
    m.nav.can_float = true;
}

/// Brain (only the baby ticks it).
pub fn brain(rng: &mut JRand) -> Brain {
    let core_nodes = seq(
        0,
        Act::Core,
        vec![
            Bk::Swim { chance: 0.8 },
            Bk::Panic { speed: 2.0, armadillo: false, sniffer: false },
            Bk::LookSink { lo: 45, hi: 90 },
            move_sink(),
            Bk::CountDown(K::TemptCd),
        ],
    );
    let idle = vec![
        at(1, Act::Idle, Bk::X(XBk::TemptEye { speed: 1.25, close: 3.0 })),
        at(2, Act::Idle, Bk::X(XBk::FollowPlayer { lo: 3, hi: 16, speed: 1.1 })),
        at(3, Act::Idle, Bk::X(XBk::FollowAdultEye { lo: 3, hi: 16, speed: 1.1 })),
        at(
            4,
            Act::Idle,
            run_one(
                &[],
                0x4748_5354,
                vec![
                    (Bk::StrollFly { speed: 1.0 }, 1),
                    (Bk::WalkFromLook { speed: Spd::Fixed(1.0), close: 3, axolotl: false }, 1),
                ],
            ),
        ),
    ];
    Brain::new(
        rng,
        &[Sn::Living, Sn::HurtBy, Sn::FoodTempt, Sn::X(XSn::AdultAnyType), Sn::Players],
        vec![
            (core(), core_nodes),
            (act(Act::Idle, &[], &[]), idle),
            (act(Act::Panic, &[(K::Panicking, St::Present)], &[]), Vec::new()),
        ],
    )
}

/// Navigation at construction: land (adult); the baby is set when it passes.
pub fn setup_nav(m: &mut Mob) {
    m.nav.can_float = true;
}

/// The baby ghast setup / the adult ghast setup on age change.
fn ensure_setup(m: &mut Mob) {
    let baby = m.is_baby();
    if m.rec.ghast_baby == Some(baby) {
        return;
    }
    if baby {
        m.nav.fly = true;
        m.nav.can_open_doors = false;
        m.nav.can_float = true;
        m.nav.required_length = 48.0;
        m.rec.still_timeout = 0;
        m.rec.stays_still = false;
        m.goals = super::super::goals::Selector::default();
        m.targets = super::super::goals::Selector::default();
        m.rec.ghast_baby = Some(true);
    } else {
        m.nav.fly = false;
        m.nav.required_length = 16.0;
        register_goals(m);
        // Brain stopped and memories erased.
        let mut r = JRand::from_time(i64::from(m.id));
        m.bm.brain = Some(Box::new(brain(&mut r)));
    }
}

/// 0 in a block; 10 above a void placed on a block; otherwise 5.
pub fn walk_target_value(s: &Server, p: [i32; 3]) -> f32 {
    if !s.is_air(p) {
        return 0.0;
    }
    if s.is_air([p[0], p[1] - 1, p[2]]) && !s.is_air([p[0], p[1] - 2, p[2]]) { 10.0 } else { 5.0 }
}

fn wearing_harness(m: &Mob) -> bool {
    m.equipment[BODY].is_some()
}

/// Immobile.
pub fn on_still_timeout(m: &Mob) -> bool {
    m.rec.stays_still || m.rec.still_timeout > 0
}

/// Synchronized flag (position resynchronized at the start).
fn set_still_timeout(s: &mut Server, m: &mut Mob, v: i32) {
    if m.rec.still_timeout <= 0 && v > 0 {
        s.sync_position_now(m);
    }
    m.rec.still_timeout = v;
    m.rec.stays_still = v > 0;
}

/// A player (non-spectator, not on a ghast) standing on top.
fn player_above(s: &Server, m: &Mob) -> bool {
    if s.game_mode == 3 || !s.ent_alive(Ent::Player) {
        return false;
    }
    let riding_ghast =
        s.mobs.mounts.riding.vehicle.and_then(|v| s.mob_ref(v)).is_some_and(|v| v.kind == Kind::HappyGhast);
    if riding_ghast {
        return false;
    }
    let b = m.bbox();
    let h = b.max[1] - b.min[1];
    let p = s.pos;
    p[0] >= b.min[0] - 1.0
        && p[0] < b.max[0] + 1.0
        && p[1] >= b.max[1] - f64::from(1.0e-5f32)
        && p[1] < b.max[1] + h / 2.0
        && p[2] >= b.min[2] - 1.0
        && p[2] < b.max[2] + 1.0
}

/// A happy ghast's tick (after the common tick): leash holder, motionless countdown (after 60
/// ticks of existence), player on top.
pub fn post_tick(s: &mut Server, m: &mut Mob) {
    ensure_setup(m);
    if m.rec.leash_time > 0 {
        m.rec.leash_time -= 1;
    }
    m.rec.leash_holder = m.rec.leash_time > 0;
    if m.rec.still_timeout > 0 {
        let mut v = m.rec.still_timeout;
        if m.tick_count > 60 {
            v -= 1;
        }
        set_still_timeout(s, m, v);
    }
    if player_above(s, m) {
        set_still_timeout(s, m, 10);
    }
}

/// The AI step (start): precise position during motionlessness.
pub fn pre_ai_step(m: &mut Mob) {
    ensure_setup(m);
}

/// 1 HP every 20 ticks in the rain or in clouds, otherwise 600.
pub fn continuous_heal(s: &mut Server, m: &mut Mob) {
    if !m.alive() || m.death_time != 0 || m.health >= m.max_health() {
        return;
    }
    let fast = in_clouds(s, m) || s.precipitation_at(m.block_pos());
    if m.tick_count % if fast { 20 } else { 600 } == 0 {
        m.health = (m.health + 1.0).min(m.max_health());
    }
}

/// Visible cloud color and body between the bottom of the clouds and 4 blocks above.
fn in_clouds(s: &Server, m: &Mob) -> bool {
    let Some(h) = s.dim_attribute(&s.dim.name, "minecraft:visual/cloud_height").and_then(|v| v.as_f64()) else {
        return false;
    };
    let colored = s.dim_attribute(&s.dim.name, "minecraft:visual/cloud_color").is_some();
    if !colored {
        return false;
    }
    let bottom = h as f32;
    if m.pos[1] + f64::from(m.height()) < f64::from(bottom) {
        return false;
    }
    m.pos[1] <= f64::from(bottom + 4.0)
}

/// Baby brain, home area.
pub fn custom_ai_step(s: &mut Server, m: &mut Mob, b: &mut Brain) {
    if m.is_baby() {
        s.bz_brain_tick(m, b);
        b.first_valid(&[Act::Panic, Act::Idle]);
    }
    // The restriction control (neither leashed nor ridden).
    if m.mount.passengers.is_empty() {
        let radius = if !m.is_baby() && !wearing_harness(m) { 64 } else { 32 };
        let bp = m.block_pos();
        let keep = m.rec.restriction.is_some_and(|(h, r)| {
            let d = (0..3).map(|i| f64::from(h[i] - bp[i]).powi(2)).sum::<f64>();
            d < f64::from((radius + 16) * (radius + 16)) && r == radius
        });
        if !keep {
            m.rec.restriction = Some((bp, radius));
        }
    }
}

// ---------- Movement ----------

/// Movement control: baby — flight; adult — careful control of the
/// ghast (motionless: wait and stop in place).
pub fn move_control(s: &Server, m: &mut Mob) -> bool {
    if m.is_baby() {
        fly_move_control(m, 180.0);
        return true;
    }
    if on_still_timeout(m) {
        m.move_ctl.op = MoveOp::Wait;
        m.nav.stop();
        m.xxa = 0.0;
        m.yya = 0.0;
        m.zza = 0.0;
        m.speed = 0.0;
        m.vel = [0.0; 3];
    }
    if m.move_ctl.op != MoveOp::MoveTo {
        return true;
    }
    let old = m.rec.float_duration;
    m.rec.float_duration -= 1;
    if old > 0 {
        return true;
    }
    m.rec.float_duration += m.rng.next_int_bound(5) + 2;
    let w = m.move_ctl.wanted;
    let travel = [w[0] - m.pos[0], w[1] - m.pos[1], w[2] - m.pos[2]];
    if careful_can_reach(s, m, travel) {
        let l = (travel[0] * travel[0] + travel[1] * travel[1] + travel[2] * travel[2]).sqrt();
        let n = if l < 1.0e-5 { [0.0; 3] } else { travel.map(|c| c / l) };
        let k = FLYING_SPEED * 5.0 / 3.0;
        m.vel = [m.vel[0] + n[0] * k, m.vel[1] + n[1] * k, m.vel[2] + n[2] * k];
    } else {
        m.move_ctl.op = MoveOp::Wait;
    }
    true
}

/// The baby's flight movement control (flight speed 0.05).
fn fly_move_control(m: &mut Mob, max_turn: f32) {
    use super::super::mth::{atan2, rotlerp};
    if m.move_ctl.op == MoveOp::MoveTo {
        m.move_ctl.op = MoveOp::Wait;
        let w = m.move_ctl.wanted;
        let (xd, yd, zd) = (w[0] - m.pos[0], w[1] - m.pos[1], w[2] - m.pos[2]);
        let dd = xd * xd + yd * yd + zd * zd;
        if dd < f64::from(2.500_000_3e-7f32) {
            m.yya = 0.0;
            m.zza = 0.0;
            return;
        }
        let deg = f64::from(180.0f32) / f64::from(std::f32::consts::PI);
        let yr = (atan2(zd, xd) * deg) as f32 - 90.0;
        m.yaw = rotlerp(m.yaw, yr, 90.0);
        let speed = if m.on_ground {
            (m.move_ctl.speed_mod * m.movement_speed()) as f32
        } else {
            (m.move_ctl.speed_mod * FLYING_SPEED) as f32
        };
        m.speed = speed;
        m.zza = speed;
        let sd = (xd * xd + zd * zd).sqrt();
        if yd.abs() > f64::from(1.0e-5f32) || sd.abs() > f64::from(1.0e-5f32) {
            let xr = -(atan2(yd, sd) * deg) as f32;
            m.pitch = rotlerp(m.pitch, xr, max_turn);
            m.yya = if yd > 0.0 { speed } else { -speed };
        }
    } else {
        m.yya = 0.0;
        m.zza = 0.0;
    }
}

/// The careful reachability test: arrival box inflated by 1 without an avoided block (`happy_ghast_avoids`)
/// or collision shape or fluid; path without collision or fluid (except the one we are in).
fn careful_can_reach(s: &Server, m: &Mob, travel: [f64; 3]) -> bool {
    let bb = m.bbox();
    let at = bb.offset(travel[0], travel[1], travel[2]);
    let lo = at.min.map(|v| (v - 1.0).floor() as i32);
    let hi = at.max.map(|v| (v + 1.0).floor() as i32);
    for x in lo[0]..=hi[0] {
        for y in lo[1]..=hi[1] {
            for z in lo[2]..=hi[2] {
                if !traversal_possible(s, m, None, [x, y, z], false, false) {
                    return false;
                }
            }
        }
    }
    let start = m.pos;
    let end = [start[0] + travel[0], start[1] + travel[1], start[2] + travel[2]];
    let (water, lava) = (m.in_water, m.lava_height > 0.0);
    super::super::nether::ghast::each_cell_between(start, end, &at, &mut |p| {
        let unit = rc_physics::Aabb {
            min: [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])],
            max: [f64::from(p[0] + 1), f64::from(p[1] + 1), f64::from(p[2] + 1)],
        };
        bb.intersects(&unit) || traversal_possible(s, m, Some(travel), p, water, lava)
    })
}

/// The traversal-possible test (careful).
fn traversal_possible(s: &Server, m: &Mob, travel: Option<[f64; 3]>, p: [i32; 3], water: bool, lava: bool) -> bool {
    if s.is_air(p) {
        return true;
    }
    let mut shapes = Vec::new();
    s.phys.collision(s.st(p), &mut shapes);
    let shapes: Vec<rc_physics::Aabb> =
        shapes.iter().map(|b| b.offset(f64::from(p[0]), f64::from(p[1]), f64::from(p[2]))).collect();
    let no_collision = match travel {
        Some(t) => !super::super::nether::ghast::collided_along(&m.bbox(), t, &shapes),
        None => shapes.is_empty(),
    };
    if s.btag_id(p, crate::tag_bits::TagId::HappyGhastAvoids) {
        return false;
    }
    let fluid = s.mob_fluid(p);
    if fluid != super::super::env::Fluid::None {
        let touched = match travel {
            None => true,
            Some(t) => {
                let h = s.fluid_height_at(p);
                let fb = rc_physics::Aabb {
                    min: [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])],
                    max: [f64::from(p[0] + 1), f64::from(p[1]) + h, f64::from(p[2] + 1)],
                };
                super::super::nether::ghast::collided_along(&m.bbox(), t, &[fb])
            }
        };
        if touched {
            return match fluid {
                super::super::env::Fluid::Water => water,
                _ => lava,
            };
        }
    }
    no_collision
}

/// Gaze: motionless, yaw brought to the nearest quarter turn; requested gaze, straight toward
/// the point; otherwise facing the movement.
pub fn look_control(s: &Server, m: &mut Mob) -> bool {
    if m.is_baby() {
        return false;
    }
    if on_still_timeout(m) {
        let close = wrap_degrees_90(m.yaw);
        m.yaw -= close;
        m.head_yaw = m.yaw;
    } else if m.look.cooldown > 0 {
        m.look.cooldown -= 1;
        let (xd, zd) = (m.look.wanted[0] - m.pos[0], m.look.wanted[2] - m.pos[2]);
        let deg = 180.0f32 / std::f32::consts::PI;
        m.yaw = -(super::super::mth::atan2(xd, zd) as f32) * deg;
        m.body_yaw = m.yaw;
        m.head_yaw = m.body_yaw;
    } else {
        super::super::nether::ghast::face_movement(s, m);
    }
    true
}

/// Bringing an angle back into [−90, 90).
fn wrap_degrees_90(a: f32) -> f32 {
    let mut r = a % 90.0;
    if r >= 45.0 {
        r -= 90.0;
    }
    if r < -45.0 {
        r += 90.0;
    }
    r
}

/// `travel`: flight movement at 0.05 × 5/3 (water × 0.8, lava × 0.5, air × 0.91).
pub fn travel(s: &mut Server, m: &mut Mob) {
    let speed = (FLYING_SPEED * 5.0 / 3.0) as f32;
    s.move_relative_pub(m, speed);
    let v = m.vel;
    s.mob_move(m, v);
    let k = if m.in_water {
        f64::from(0.8f32)
    } else if m.lava_height > 0.0 {
        0.5
    } else {
        f64::from(0.91f32)
    };
    m.vel = m.vel.map(|x| x * k);
}

// ---------- Mount ----------

/// Piloted by the player: harness, not motionless, player first.
pub fn player_controls(v: &Mob) -> bool {
    v.kind == Kind::HappyGhast
        && wearing_harness(v)
        && !on_still_timeout(v)
        && v.mount.passengers.first() == Some(&Ent::Player)
}

/// Four seats at most.
pub fn max_passengers(v: &Mob) -> Option<usize> {
    (v.kind == Kind::HappyGhast).then_some(4)
}

/// Passenger attachment points (front, left, back, right at 4 high).
pub fn passenger_point(v: &Mob, rank: usize) -> Option<[f64; 3]> {
    if v.kind != Kind::HappyGhast {
        return None;
    }
    let s = f64::from(if v.is_baby() { BABY_SCALE } else { 1.0 });
    let p = match rank {
        0 => [0.0, 4.0, 1.7],
        1 => [-1.7, 4.0, 0.0],
        2 => [0.0, 4.0, -1.7],
        _ => [1.7, 4.0, 0.0],
    };
    Some(p.map(|c| c * s))
}

/// Dismount location: top of the ghast.
pub fn dismount_location(v: &Mob) -> Option<[f64; 3]> {
    (v.kind == Kind::HappyGhast).then(|| [v.pos[0], v.bbox().max[1], v.pos[2]])
}

/// Passenger mounted: goggles lowered at the first; motionlessness lifted if
/// nobody is on top, otherwise set back to 10.
pub fn on_passenger_added(s: &mut Server, m: &mut Mob, was_empty: bool) {
    if m.kind != Kind::HappyGhast {
        return;
    }
    if was_empty {
        s.mob_sound(m, "minecraft:entity.happy_ghast.harness_goggles_down", 1.0, 1.0);
    }
    if !player_above(s, m) {
        set_still_timeout(s, m, 0);
    } else if m.rec.still_timeout > 10 {
        set_still_timeout(s, m, 10);
    }
}

/// Passenger dismounted: motionless 10 ticks; nobody left: area forgotten,
/// goggles raised.
pub fn on_passenger_removed(s: &mut Server, m: &mut Mob) {
    if m.kind != Kind::HappyGhast {
        return;
    }
    set_still_timeout(s, m, 10);
    if m.mount.passengers.is_empty() {
        m.rec.restriction = None;
        s.mob_sound(m, "minecraft:entity.happy_ghast.harness_goggles_up", 1.0, 1.0);
    }
}

/// Yaw that catches up 8% of the difference with the player's gaze, pitch by half.
pub fn tick_ridden(s: &Server, m: &mut Mob) -> bool {
    if m.kind != Kind::HappyGhast {
        return false;
    }
    let diff = super::super::mth::wrap(s.yaw - m.yaw);
    m.yaw += diff * 0.08;
    m.pitch = s.pitch * 0.5;
    m.body_yaw = m.yaw;
    m.head_yaw = m.yaw;
    true
}

/// Baby — food (growth); adult — harness placed, then mounts.
pub fn interact(s: &mut Server, m: &mut Mob, slot: usize, item: &str) -> Option<bool> {
    let held = s.slots[slot].clone();
    if m.is_baby() {
        if !item.is_empty() && s.item_tag(item, "minecraft:happy_ghast_food") {
            if s.game_mode != 1 {
                s.consume_held(slot);
            }
            let secs = super::super::mounts::feeding_secs(-m.age);
            super::super::mounts::age_up(m, secs, true);
            return Some(true);
        }
        return None;
    }
    if let Some(h) = held.as_ref()
        && let Some(e) = s.equippable(h)
        && e.on_interact
        && e.slot == BODY
        && m.alive()
        && m.equipment[BODY].is_none()
        && e.allowed.as_ref().is_none_or(|l| s.entity_in(m.kind, l))
    {
        let one = crate::player::Stack { count: 1, ..h.clone() };
        if s.game_mode != 1 {
            s.consume_held(slot);
        }
        m.drop_chance[BODY] = 2.0;
        s.mount_set_slot(m, BODY, Some(one));
        return Some(true);
    }
    if wearing_harness(m) && !s.sneaking {
        let was_empty = m.mount.passengers.is_empty();
        if s.player_start_riding_ghast(m) {
            on_passenger_added(s, m, was_empty);
        }
        return Some(true);
    }
    None
}

impl Server {
    /// Mounts a happy ghast (four seats).
    fn player_start_riding_ghast(&mut self, m: &mut Mob) -> bool {
        if m.mount.passengers.len() >= 4 {
            return false;
        }
        self.player_start_riding(m)
    }

    /// Goals of the adult happy ghast.
    pub(crate) fn rec_can_use(&mut self, g: &mut RGoal, m: &mut Mob) -> bool {
        match g {
            RGoal::Float => {
                !on_still_timeout(m)
                    && ((m.in_water && m.water_height > self.fluid_jump_threshold(m)) || m.lava_height > 0.0)
            }
            RGoal::Tempt { player, calm, .. } => {
                if *calm > 0 {
                    *calm -= 1;
                    return false;
                }
                *player = self.ghast_tempting_player(m);
                player.is_some()
            }
            RGoal::RandomFloat => super::super::nether::ghast::float_can_use(m),
        }
    }

    pub(crate) fn rec_can_continue(&mut self, g: &mut RGoal, m: &mut Mob) -> bool {
        match g {
            RGoal::Float => self.rec_can_use(g, m),
            RGoal::Tempt { player, .. } => {
                *player = self.ghast_tempting_player(m);
                player.is_some()
            }
            RGoal::RandomFloat => false,
        }
    }

    pub(crate) fn rec_start(&mut self, g: &mut RGoal, m: &mut Mob) {
        match g {
            RGoal::Tempt { running, .. } => *running = true,
            RGoal::RandomFloat => {
                let p = self.ghast_fly_to(m, 16);
                super::super::nav::set_wanted(m, p, 1.0);
            }
            RGoal::Float => {}
        }
    }

    pub(crate) fn rec_stop(&mut self, g: &mut RGoal, m: &mut Mob) {
        if let RGoal::Tempt { player, calm, running } = g {
            *player = None;
            m.move_ctl.op = MoveOp::Wait;
            *calm = super::super::mth::reduced(100);
            *running = false;
        }
    }

    pub(crate) fn rec_tick(&mut self, g: &mut RGoal, m: &mut Mob) {
        match g {
            RGoal::Float => {
                if m.rng.next_float() < 0.8 {
                    m.jump_ctl = true;
                }
            }
            RGoal::Tempt { player, .. } => {
                let Some(p) = *player else { return };
                let (Some(pp), Some(eye)) = (self.ent_pos(p), self.ent_eye(p)) else { return };
                let y = super::super::goals::max_head_y(m.kind) + 20.0;
                let x = super::super::goals::max_head_x(m);
                self.look_at(m, [pp[0], eye, pp[2]], y, x);
                if super::super::mth::dist2(m.pos, pp) < 49.0 {
                    m.move_ctl.op = MoveOp::Wait;
                } else {
                    let r = m.rng.next_double();
                    let t = [
                        (pp[0] - m.pos[0]) * r + m.pos[0],
                        (eye - m.pos[1]) * r + m.pos[1],
                        (pp[2] - m.pos[2]) * r + m.pos[2],
                    ];
                    super::super::nav::set_wanted(m, t, 1.0);
                }
            }
            RGoal::RandomFloat => {}
        }
    }

    /// Player tempting the adult (harness or food depending on equipment), range 16.
    fn ghast_tempting_player(&self, m: &Mob) -> Option<Ent> {
        if self.game_mode == 3 || !self.ent_alive(Ent::Player) {
            return None;
        }
        let range = m.attrs.tempt_range.value();
        if super::super::mth::dist2(m.pos, self.pos) > range * range {
            return None;
        }
        let tag = if !wearing_harness(m) && !m.is_baby() {
            "minecraft:happy_ghast_tempt_items"
        } else {
            "minecraft:happy_ghast_food"
        };
        let held = [&self.slots[36 + self.held as usize], &self.slots[45]];
        held.iter().any(|st| st.as_ref().is_some_and(|st| self.item_tag(&st.id, tag))).then_some(Ent::Player)
    }

    /// 64 tries in a ±16 cube in the home area, empty
    /// and within 16 blocks of a block in one of the six directions; otherwise the last one drawn,
    /// brought under the surface if it is above.
    fn ghast_fly_to(&self, m: &mut Mob, dist_blocks: i32) -> [f64; 3] {
        let c = m.pos;
        let mut result: Option<[f64; 3]> = None;
        for _ in 0..64 {
            let t = [
                c[0] + f64::from((m.rng.next_float() * 2.0 - 1.0) * 16.0),
                c[1] + f64::from((m.rng.next_float() * 2.0 - 1.0) * 16.0),
                c[2] + f64::from((m.rng.next_float() * 2.0 - 1.0) * 16.0),
            ];
            let within = m.rec.restriction.is_none_or(|(h, r)| {
                let b = t.map(|v| v.floor() as i32);
                let d = (0..3).map(|i| f64::from(h[i] - b[i]).powi(2)).sum::<f64>();
                r == -1 || d < f64::from(r * r)
            });
            result = within.then_some(t);
            if let Some(t) = result
                && self.ghast_good_target(t, dist_blocks)
            {
                return t;
            }
        }
        let r = match result {
            Some(r) => r,
            None => [
                c[0] + f64::from((m.rng.next_float() * 2.0 - 1.0) * 16.0),
                c[1] + f64::from((m.rng.next_float() * 2.0 - 1.0) * 16.0),
                c[2] + f64::from((m.rng.next_float() * 2.0 - 1.0) * 16.0),
            ],
        };
        let b = r.map(|v| v.floor() as i32);
        let top = self.motion_blocking_height(b[0], b[2]);
        if top < b[1] && top > self.dim.height.min_y {
            return [r[0], m.pos[1] - (m.pos[1] - r[1]).abs(), r[2]];
        }
        r
    }

    /// Empty cell, a block within `d` in one of the six directions.
    fn ghast_good_target(&self, t: [f64; 3], d: i32) -> bool {
        if d <= 0 {
            return true;
        }
        let p = t.map(|v| v.floor() as i32);
        if !self.is_air(p) {
            return false;
        }
        const DIRS: [[i32; 3]; 6] = [[0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]];
        for dir in DIRS {
            for i in 1..d {
                let q = [p[0] + dir[0] * i, p[1] + dir[1] * i, p[2] + dir[2] * i];
                if !self.is_air(q) {
                    return true;
                }
            }
        }
        false
    }
}

// ---------- NBT ----------

pub fn load(m: &mut Mob, e: &Nbt) {
    let v = nbtio::int(e, "still_timeout").unwrap_or(0) as i32;
    m.rec.still_timeout = v;
    m.rec.stays_still = v > 0;
}

pub fn save(m: &Mob, e: &mut Nbt) {
    nbtio::set(e, "still_timeout", Nbt::Int(m.rec.still_timeout));
}
