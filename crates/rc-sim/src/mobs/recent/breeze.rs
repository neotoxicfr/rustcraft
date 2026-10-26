//! Breeze (`server-mobs.md` § 4.F14): brain (idle: attackable target or author of the
//! damage, slide toward the target; default fight: wind charge shot, long jump behind
//! the target, shot when stuck, slide), own sounds, immune to damage from another
//! breeze, projectile deflection (except wind charges: `wind.rs`). Its particles and
//! whistle are client-side, but their random draws (entity) are made at the server tick.

use super::super::brainy::brain::{Act, Bk, Brain, K, Sn, St, act, at, run_one, seq};
use super::super::jrand::JRand;
use super::super::mob::{Ent, Mob};
use super::super::mth::{DEG, cos, sin};
use super::xbrain::{Find, Hook, Stop, XBk, XSn};
use super::{POSE_INHALING, POSE_LONG_JUMPING, POSE_SHOOTING, POSE_STANDING};
use crate::server::Server;

pub fn brain(rng: &mut JRand) -> Brain {
    let core_nodes = seq(0, Act::Core, vec![Bk::Swim { chance: 0.8 }, Bk::LookSink { lo: 45, hi: 90 }]);
    let slide_sink = Bk::MoveSink { lo: 20, hi: 40, cd: 0, path: None, last: None, speed: 0.0, armadillo: false };
    let idle = vec![
        at(0, Act::Idle, Bk::X(XBk::StartAttack(Find::Attackable))),
        at(1, Act::Idle, Bk::X(XBk::StartAttack(Find::HurtBy))),
        at(2, Act::Idle, Bk::X(XBk::Wrap { inner: Box::new(slide_sink), hook: Hook::Slide })),
        at(
            3,
            Act::Idle,
            run_one(
                &[],
                0x4252_455a,
                vec![(Bk::DoNothing { lo: 20, hi: 100 }, 1), (Bk::Stroll { speed: 0.6, from_water: true }, 2)],
            ),
        ),
    ];
    let fight = vec![
        at(0, Act::Fight, Bk::X(XBk::StopAttack { why: Stop::BreezeForgot, positives: 0 })),
        at(1, Act::Fight, Bk::X(XBk::BreezeShoot)),
        at(2, Act::Fight, Bk::X(XBk::BreezeJump)),
        at(3, Act::Fight, Bk::X(XBk::BreezeStuck)),
        at(4, Act::Fight, Bk::X(XBk::BreezeSlide)),
    ];
    let _ = seq;
    let mut b = Brain::new(
        rng,
        &[Sn::Living, Sn::HurtBy, Sn::Players, Sn::X(XSn::BreezeAttack)],
        vec![
            (super::super::brainy::brain::core(), core_nodes),
            (act(Act::Idle, &[], &[]), idle),
            (act(Act::Fight, &[(K::Attack, St::Present), (K::Walk, St::Absent)], &[]), fight),
        ],
    );
    // Default activity: fight (then back to the default activity).
    b.act = Act::Fight;
    b
}

/// Fight, otherwise idle.
pub fn update_activity(b: &mut Brain) {
    b.first_valid(&[Act::Fight, Act::Idle]);
}

fn pose(m: &Mob) -> i32 {
    super::super::brainy::pose(m)
}

fn set_pose(m: &mut Mob, p: i32) {
    super::super::brainy::set_pose(m, p);
}

/// A breeze's tick (before the common tick): draws for the ground particles (`next_int(1)`) and
/// the whistle (delay 1 to 80, then pitch and volume) on the entity random.
pub fn pre_tick(_s: &mut Server, m: &mut Mob) {
    match pose(m) {
        POSE_SHOOTING | POSE_INHALING | POSE_STANDING => {
            m.rec.jump_trail = 0;
            let _ = m.rng.next_int_bound(1);
        }
        POSE_LONG_JUMPING => m.rec.jump_trail += 1,
        _ => {}
    }
    m.rec.sound_tick = if m.rec.sound_tick == 0 { m.rng.next_int_bound(80) + 1 } else { m.rec.sound_tick - 1 };
    if m.rec.sound_tick == 0 {
        let _pitch = 0.7 + 0.4 * m.rng.next_float();
        let _volume = 0.8 + 0.2 * m.rng.next_float();
    }
}

/// Landing: sound beyond 3 blocks of fall.
pub fn on_fall(s: &mut Server, m: &Mob, distance: f64) {
    if distance > 3.0 {
        s.mob_sound(m, "minecraft:entity.breeze.land", 1.0, 1.0);
    }
}

/// Yaw, pitch and head set directly.
fn look_at_eyes(m: &mut Mob, p: [f64; 3]) {
    let from = [m.pos[0], m.eye_y(), m.pos[2]];
    let (xd, yd, zd) = (p[0] - from[0], p[1] - from[1], p[2] - from[2]);
    let sd = (xd * xd + zd * zd).sqrt();
    let deg = 180.0f32 / std::f32::consts::PI;
    m.pitch = super::super::mth::wrap(-((super::super::mth::atan2(yd, sd) as f32) * deg));
    m.yaw = super::super::mth::wrap((super::super::mth::atan2(zd, xd) as f32) * deg - 90.0);
    m.head_yaw = m.yaw;
}

/// Shot height: middle of the box + 0.3.
pub fn firing_y(m: &Mob) -> f64 {
    m.pos[1] + f64::from(m.height() / 2.0) + f64::from(0.3f32)
}

// ---------- Shot (`Shoot`) ----------

/// Standing, target within 16 blocks (otherwise the shot is forgotten).
pub fn shoot_check(s: &mut Server, m: &mut Mob, b: &mut Brain) -> bool {
    if pose(m) != POSE_STANDING {
        return false;
    }
    let Some(t) = b.mem.attack.get() else { return false };
    let Some(tp) = s.ent_pos(t) else { return false };
    let within = super::super::mth::dist2(m.pos, tp) < 256.0;
    if !within {
        b.mem.x.shoot.erase();
    }
    within
}

pub fn shoot_start(s: &mut Server, m: &mut Mob, b: &mut Brain) {
    if b.mem.attack.has() {
        set_pose(m, POSE_SHOOTING);
    }
    b.mem.x.shoot_charging.set_exp((), 15);
    s.mob_sound(m, "minecraft:entity.breeze.inhale", 1.0, 1.0);
}

/// After the charge (15 ticks): wind charge toward the target (0.3 of its height, 0.8 if it
/// is mounted), inaccuracy 5 − 4 × difficulty, then 4 ticks of recovery.
pub fn shoot_tick(s: &mut Server, m: &mut Mob, b: &mut Brain) {
    let Some(t) = b.mem.attack.get() else { return };
    let Some(tp) = s.ent_pos(t) else { return };
    look_at_eyes(m, tp);
    if b.mem.x.shoot_charging.has() || b.mem.x.shoot_recovering.has() {
        return;
    }
    b.mem.x.shoot_recovering.set_exp((), 4);
    let (Some(tb), riding) = (s.ent_bbox(t), s.ent_riding(t)) else { return };
    let h = tb.max[1] - tb.min[1];
    let xd = tp[0] - m.pos[0];
    let yd = tp[1] + h * if riding { 0.8 } else { 0.3 } - firing_y(m);
    let zd = tp[2] - m.pos[2];
    let unc = (5 - s.level.difficulty_id() * 4) as f32;
    let start = [m.pos[0], firing_y(m), m.pos[2]];
    s.shoot_wind_charge(m.uuid, start, [xd, yd, zd], 0.7, unc);
    s.mob_sound(m, "minecraft:entity.breeze.shoot", 1.5, 1.0);
}

pub fn shoot_stop(m: &mut Mob, b: &mut Brain) {
    if pose(m) == POSE_SHOOTING {
        set_pose(m, POSE_STANDING);
    }
    b.mem.x.shoot_cooldown.set_exp((), 10);
    b.mem.x.shoot.erase();
}

// ---------- Long jump (`LongJump`) ----------

/// Behind the target's gaze (±45° gaussian), 4 to 8 blocks.
fn point_behind(s: &Server, m: &mut Mob, t: Ent) -> Option<[f64; 3]> {
    let tp = s.ent_pos(t)?;
    let head = match t {
        Ent::Player => s.yaw,
        Ent::Mob(id) => s.mob_ref(id)?.head_yaw,
    };
    let view = head + 180.0 + m.rng.next_gaussian() as f32 * 90.0 / 2.0;
    let r = 4.0 + m.rng.next_float() * (8.0 - 4.0);
    // The direction vector for a yaw angle.
    let yc = cos(-view * DEG - std::f32::consts::PI);
    let ys = sin(-view * DEG - std::f32::consts::PI);
    let dir = [f64::from(-ys), 0.0, f64::from(-yc)];
    Some([tp[0] + dir[0] * f64::from(r), tp[1], tp[2] + dir[2] * f64::from(r)])
}

/// Surface under the point (10 blocks), otherwise above (10 blocks).
fn snap_to_surface(s: &Server, p: [f64; 3]) -> Option<[i32; 3]> {
    let down = [p[0], p[1] - 10.0, p[2]];
    if let Some(t) = s.clip_t(p, down) {
        let hit = [p[0], p[1] - 10.0 * t, p[2]];
        let b = hit.map(|v| v.floor() as i32);
        return Some([b[0], b[1] + 1, b[2]]);
    }
    let up = [p[0], p[1] + 10.0, p[2]];
    s.clip_t(p, up).map(|t| {
        let hit = [p[0], p[1] + 10.0 * t, p[2]];
        let b = hit.map(|v| v.floor() as i32);
        [b[0], b[1] + 1, b[2]]
    })
}

/// From the feet, 50 blocks at most (or the tracking range).
fn has_line_of_sight(s: &Server, m: &Mob, to: [f64; 3]) -> bool {
    let max = 50.0f64.max(m.follow_range());
    let d = super::super::mth::dist2(m.pos, to).sqrt();
    d <= max && !s.clip_blocks(m.pos, to)
}

/// The dangerous-block test for an entity type (breeze not immune to fire).
fn dangerous(s: &Server, p: [i32; 3]) -> bool {
    let n = s.bname(p);
    s.btag_id(p, crate::tag_bits::TagId::Fire)
        || n == "minecraft:lava"
        || n == "minecraft:magma_block"
        || ((n == "minecraft:campfire" || n == "minecraft:soul_campfire")
            && s.prop(p, "lit").as_deref() == Some("true"))
        || matches!(
            n,
            "minecraft:wither_rose" | "minecraft:sweet_berry_bush" | "minecraft:cactus" | "minecraft:powder_snow"
        )
}

/// The long-jump start condition (with its effects: out-of-range target forgotten, jump target set).
pub fn jump_can_run(s: &mut Server, m: &mut Mob, b: &mut Brain) -> bool {
    if !m.on_ground && !m.in_water {
        return false;
    }
    let deep = m.in_water && m.water_height > m.eye_height();
    if deep || m.lava_height > 0.0 {
        return false;
    }
    if b.mem.x.jump_target.has() {
        return true;
    }
    let Some(t) = b.mem.attack.get() else { return false };
    let Some(tp) = s.ent_pos(t) else { return false };
    let d = super::super::mth::dist2(m.pos, tp).sqrt();
    if d >= m.follow_range() {
        b.mem.attack.erase();
        return false;
    }
    if d - 4.0 <= 0.0 {
        return false;
    }
    // Not on honey, 4 cells of air (or water) above.
    let bp = m.block_pos();
    if s.bname(bp) == "minecraft:honey_block" {
        return false;
    }
    for i in 1..=4 {
        let q = [bp[0], bp[1] + i, bp[2]];
        if !s.is_air(q) && s.mob_fluid(q) != super::super::env::Fluid::Water {
            return false;
        }
    }
    let Some(behind) = point_behind(s, m, t) else { return false };
    let Some(target) = snap_to_surface(s, behind) else { return false };
    if dangerous(s, [target[0], target[1] - 1, target[2]]) {
        return false;
    }
    let c = [f64::from(target[0]) + 0.5, f64::from(target[1]) + 0.5, f64::from(target[2]) + 0.5];
    let c4 = [c[0], c[1] + 4.0, c[2]];
    if !has_line_of_sight(s, m, c) && !has_line_of_sight(s, m, c4) {
        return false;
    }
    b.mem.x.jump_target.set(target);
    true
}

/// Inhale (10 ticks), charge sound, gaze toward the jump target.
pub fn jump_start(s: &mut Server, m: &mut Mob, b: &mut Brain) {
    if !b.mem.x.jump_inhaling.has() {
        b.mem.x.jump_inhaling.set_exp((), 10);
    }
    set_pose(m, POSE_INHALING);
    // Sound attached to the breeze (it follows it during the inhale).
    s.entity_sound(m.id, "minecraft:entity.breeze.charge", Server::mob_sound_source(m), 1.0, 1.0);
    if let Some(t) = b.mem.x.jump_target.get() {
        look_at_eyes(m, [f64::from(t[0]) + 0.5, f64::from(t[1]) + 0.5, f64::from(t[2]) + 0.5]);
    }
}

/// Jump (angles 40/55/60/75/80 shuffled, speed ≤ 0.0583 × range), then landing:
/// cooldown 10 (2 if hurt) and shot allowed 100 ticks.
pub fn jump_tick(s: &mut Server, m: &mut Mob, b: &mut Brain) {
    let in_water = m.in_water;
    if !in_water && b.mem.x.leaving_water.has() {
        b.mem.x.leaving_water.erase();
    }
    if !b.mem.x.jump_inhaling.has() && pose(m) == POSE_INHALING {
        let v = b.mem.x.jump_target.get().and_then(|t| {
            let target = [f64::from(t[0]) + 0.5, f64::from(t[1]), f64::from(t[2]) + 0.5];
            optimal_jump(m, target)
        });
        let Some(v) = v else {
            set_pose(m, POSE_STANDING);
            return;
        };
        if in_water {
            b.mem.x.leaving_water.set(());
        }
        s.mob_sound(m, "minecraft:entity.breeze.jump", 1.0, 1.0);
        set_pose(m, POSE_LONG_JUMPING);
        m.yaw = m.body_yaw;
        super::super::brainy::set_discard_friction(m, true);
        m.vel = v;
        m.needs_sync = true;
    } else if pose(m) == POSE_LONG_JUMPING && (m.on_ground || (m.in_water && !b.mem.x.leaving_water.has())) {
        s.mob_sound(m, "minecraft:entity.breeze.land", 1.0, 1.0);
        set_pose(m, POSE_STANDING);
        super::super::brainy::set_discard_friction(m, false);
        let hurt = b.mem.hurt_by.has();
        b.mem.x.jump_cooldown.set_exp((), if hurt { 2 } else { 10 });
        b.mem.x.shoot.set_exp((), 100);
    }
}

pub fn jump_stop(m: &mut Mob, b: &mut Brain) {
    let p = pose(m);
    if p == POSE_LONG_JUMPING || p == POSE_INHALING {
        set_pose(m, POSE_STANDING);
    }
    b.mem.x.jump_target.erase();
    b.mem.x.jump_inhaling.erase();
    b.mem.x.leaving_water.erase();
}

/// The optimal jump vector computation (entity random for the angle order), jump bonus.
fn optimal_jump(m: &mut Mob, target: [f64; 3]) -> Option<[f64; 3]> {
    let mut angles = [40, 55, 60, 75, 80];
    for i in (1..angles.len()).rev() {
        let j = m.rng.next_int_bound(i as i32 + 1) as usize;
        angles.swap(i, j);
    }
    for a in angles {
        let max_v = 0.058_333_334f32 * m.follow_range() as f32;
        if let Some(v) = jump_vector(m, target, max_v, a) {
            if let Some(e) = m.x.effects.iter().find(|e| e.id == "minecraft:jump_boost") {
                let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                let ny = if l < 1.0e-5 { 0.0 } else { v[1] / l };
                let power = f64::from(0.1f32 * (e.amp + 1) as f32);
                return Some([v[0], v[1] + ny * power, v[2]]);
            }
            return Some(v);
        }
    }
    None
}

/// The jump vector computation for an angle without collision control.
fn jump_vector(m: &Mob, target: [f64; 3], max_v: f32, angle: i32) -> Option<[f64; 3]> {
    let p = m.pos;
    let plane = [target[0] - p[0], 0.0, target[2] - p[2]];
    let pl = (plane[0] * plane[0] + plane[2] * plane[2]).sqrt();
    let half = if pl < 1.0e-5 { [0.0; 3] } else { [plane[0] / pl * 0.5, 0.0, plane[2] / pl * 0.5] };
    let tp = [target[0] - half[0], target[1], target[2] - half[2]];
    let d = [tp[0] - p[0], tp[1] - p[1], tp[2] - p[2]];
    let ang = angle as f32 * std::f32::consts::PI / 180.0;
    let xz = d[2].atan2(d[0]);
    let r2 = d[0] * d[0] + d[2] * d[2];
    let r = r2.sqrt();
    let y = d[1];
    let g = m.attrs.gravity.value();
    let sin2 = f64::from(2.0 * ang).sin();
    let cos_sq = f64::from(ang).cos().powi(2);
    let (s_a, c_a) = (f64::from(ang).sin(), f64::from(ang).cos());
    let v0sq = r2 * g / (r * sin2 - 2.0 * y * cos_sq);
    if v0sq < 0.0 {
        return None;
    }
    let v0 = v0sq.sqrt();
    if v0 > f64::from(max_v) {
        return None;
    }
    let (v0r, v0y) = (v0 * c_a, v0 * s_a);
    let k = f64::from(0.95f32);
    Some([v0r * xz.cos() * k, v0y * k, v0r * xz.sin() * k])
}

// ---------- Slide (`Slide`) ----------

/// Slide: in the inner circle (4 blocks), point away from the target if possible;
/// otherwise at random behind the target or in the middle circle (4 to 8 blocks from the target).
pub fn slide_start(s: &mut Server, m: &mut Mob, b: &mut Brain) {
    let Some(t) = b.mem.attack.get() else { return };
    let Some(tp) = s.ent_pos(t) else { return };
    let bp = m.block_pos();
    let c = [f64::from(bp[0]) + 0.5, f64::from(bp[1]) + 0.5, f64::from(bp[2]) + 0.5];
    let inner = {
        let (dx, dy, dz) = (tp[0] - c[0], tp[1] - c[1], tp[2] - c[2]);
        dx * dx + dz * dz < 16.0 && dy.abs() < 10.0
    };
    let mut position = None;
    if inner
        && let Some(p0) = s.default_random_pos_away(m, 5, 5, tp)
        && has_line_of_sight(s, m, p0)
        && super::super::mth::dist2(tp, p0) > super::super::mth::dist2(tp, m.pos)
    {
        position = Some(p0);
    }
    let position = match position {
        Some(p) => p,
        None => {
            if m.rng.next_bool() {
                point_behind(s, m, t).unwrap_or(tp)
            } else {
                let dir = [tp[0] - m.pos[0], tp[1] - m.pos[1], tp[2] - m.pos[2]];
                let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
                let dist = len - (8.0 + m.rng.next_double() * (4.0 - 8.0));
                let n = if len < 1.0e-5 { [0.0; 3] } else { dir.map(|v| v / len) };
                [m.pos[0] + n[0] * dist, m.pos[1] + n[1] * dist, m.pos[2] + n[2] * dist]
            }
        }
    };
    let bpos = position.map(|v| v.floor() as i32);
    b.mem.walk.set(super::super::brainy::brain::Walk {
        target: super::super::brainy::brain::Track::block(bpos),
        speed: 0.6,
        close: 1,
    });
}
