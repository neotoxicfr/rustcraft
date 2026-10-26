//! Bee (§ 4.F11): flight (flying control with turn 20, hover), own gaze,
//! pollination, return to the hive, crop growth, persistent anger and sting (slow death
//! after stinging), drowning, food (flowers with an effect).

use super::super::mob::{Ent, Mob, MoveOp};
use super::super::mth::{self, dist2, rotate_if_necessary, rotate_towards};
use super::Wild;
use crate::server::Server;

pub const ROLL: u8 = 2;
pub const STUNG: u8 = 4;
pub const NECTAR: u8 = 8;

/// Flight speed (`flying_speed` 0.6).
pub const FLYING_SPEED: f64 = 0.6f32 as f64;

pub fn has_nectar(m: &Mob) -> bool {
    m.wild.flag(NECTAR)
}

pub fn has_stung(m: &Mob) -> bool {
    m.wild.flag(STUNG)
}

/// The no-nectar counter restarts from zero when the nectar is taken.
pub fn set_nectar(w: &mut Wild, v: bool) {
    if v {
        w.no_nectar_ticks = 0;
    }
    w.set_flag(NECTAR, v);
}

/// Anger end positive and not yet reached.
pub fn is_angry(s: &Server, m: &Mob) -> bool {
    m.anger_end > 0 && m.anger_end - s.time > 0
}

/// The proximity test between positions (distance² between block positions).
pub fn closer_than(m: &Mob, p: [i32; 3], d: i32) -> bool {
    let b = m.block_pos();
    let (dx, dy, dz) = (i64::from(b[0] - p[0]), i64::from(b[1] - p[1]), i64::from(b[2] - p[2]));
    dx * dx + dy * dy + dz * dz < i64::from(d) * i64::from(d)
}

/// Block center within `d` of the position.
pub fn near_center(p: [i32; 3], at: [f64; 3], d: f64) -> bool {
    dist2([f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5], at) < d * d
}

pub fn too_far(m: &Mob, p: [i32; 3]) -> bool {
    !closer_than(m, p, 48)
}

/// Environment attribute "bees stay in the hive": night of the day timeline
/// (12,542 → 23,460) in the normal world, rain or thunderstorm.
pub fn stay_in_hive(s: &Server) -> bool {
    let night = s.dim.name == "minecraft:overworld" && {
        let t = s.day_time().rem_euclid(24000);
        (12542..23460).contains(&t)
    };
    night || s.life.weather.rain_level > 0.0 || s.life.weather.thunder_eff() > 0.0
}

/// World rain (rain level above 0.2).
pub fn raining(s: &Server) -> bool {
    s.life.weather.rain_level > 0.2
}

/// Block entity of the retained hive if valid (neither too far, nor absent).
pub fn hive_entity(s: &Server, m: &Mob) -> Option<[i32; 3]> {
    let h = m.wild.hive?;
    if too_far(m, h) {
        return None;
    }
    super::hive::is_hive_entity(s, h).then_some(h)
}

pub fn hive_valid(s: &Server, m: &Mob) -> bool {
    hive_entity(s, m).is_some()
}

fn hive_near_fire(s: &Server, m: &Mob) -> bool {
    hive_entity(s, m).is_some_and(|h| super::hive::fire_nearby(s, h))
}

/// The wish to enter the hive.
pub fn wants_to_enter_hive(s: &Server, m: &Mob) -> bool {
    let w = &m.wild;
    if w.stay_out <= 0 && !w.pollinating && !has_stung(m) && m.target.is_none() {
        let want = has_nectar(m) || w.no_nectar_ticks > 3600 || stay_in_hive(s);
        want && !hive_near_fire(s, m)
    } else {
        false
    }
}

pub fn drop_hive(w: &mut Wild) {
    w.hive = None;
    w.hive_cd = 200;
}

pub fn drop_flower(m: &mut Mob) {
    m.wild.flower = None;
    m.wild.flower_cd = m.rng.int_between(20, 60);
}

/// Tag `bee_attractive`, not waterlogged; sunflower: upper half only.
pub fn attracts_bees(s: &Server, p: [i32; 3]) -> bool {
    if !s.btag_id(p, crate::tag_bits::TagId::BeeAttractive) {
        return false;
    }
    let st = s.st(p);
    let prop = |k: &str| s.data.state_properties(st).find(|(n, _)| *n == k).map(|(_, v)| v.to_owned());
    if prop("waterlogged").as_deref() == Some("true") {
        return false;
    }
    s.bname(p) != "minecraft:sunflower" || prop("half").as_deref() == Some("upper")
}

// ---------- Ticks ----------

/// End of `tick`: nectar drops (client particles, but two doubles of the world random
/// drawn per particle server-side), the particle count removed at each loop turn.
pub fn post_tick(s: &mut Server, m: &mut Mob) {
    if has_nectar(m) && m.wild.crops < 10 && m.rng.next_float() < 0.05 {
        let mut i = 0;
        while i < m.rng.next_int_bound(2) + 1 {
            let _ = s.mobs.rng.next_double();
            let _ = s.mobs.rng.next_double();
            i += 1;
        }
    }
}

/// Drowning (> 20 ticks in water), slow death after the sting, no-nectar counter,
/// persistent anger.
pub fn custom_ai_step(s: &mut Server, m: &mut Mob) {
    let stung = has_stung(m);
    if m.in_water {
        m.wild.under_water += 1;
    } else {
        m.wild.under_water = 0;
    }
    if m.wild.under_water > 20 {
        s.mob_hurt(m, "minecraft:drown", 1.0, None, None);
    }
    if stung {
        m.wild.since_sting += 1;
        let bound = (1200 - m.wild.since_sting).clamp(1, 1200);
        if m.wild.since_sting % 5 == 0 && m.rng.next_int_bound(bound) == 0 {
            let h = m.health;
            s.mob_hurt(m, "minecraft:generic", h, None, None);
        }
    }
    if !has_nectar(m) {
        m.wild.no_nectar_ticks += 1;
    }
    update_anger(s, m, false);
}

/// UUID of an entity (player or loaded mob).
fn uuid_of(s: &Server, e: Ent) -> Option<u128> {
    match e {
        Ent::Player => Some(s.uuid),
        Ent::Mob(id) => s.mob_ref(id).map(|o| o.uuid),
    }
}

/// Entity designated by a UUID (player, loaded mob).
pub fn ent_of(s: &Server, u: u128) -> Option<Ent> {
    if u == s.uuid {
        return Some(Ent::Player);
    }
    s.mobs.get_uuid(u).map(|o| Ent::Mob(o.id))
}

/// Valid player target (neither creative, nor spectator, nor peaceful).
fn valid_player_target(s: &Server, e: Ent) -> bool {
    e == Ent::Player && s.game_mode != 1 && s.game_mode != 3 && s.level.difficulty_id() != 0
}

/// 20 to 39 s.
fn start_anger_timer(s: &Server, m: &mut Mob) {
    let t = m.rng.int_between(400, 780);
    m.anger_end = s.time + i64::from(t);
}

/// The persistent anger update.
pub fn update_anger(s: &mut Server, m: &mut Mob, stay_if_target: bool) {
    // Target read back from a save: found by its UUID as soon as it is loaded.
    if m.anger_target.is_none()
        && let Some(u) = m.wild.angry_at
        && let Some(e) = ent_of(s, u)
    {
        m.anger_target = Some(e);
        if m.target.is_none() {
            m.target = Some(e);
        }
    }
    let prev = m.target;
    let persistent = m.wild.angry_at;
    if let Some(t) = prev
        && !s.ent_alive(t)
        && persistent.is_some()
        && uuid_of(s, t) == persistent
        && matches!(t, Ent::Mob(_))
    {
        m.stop_being_angry();
        return;
    }
    if let Some(t) = m.target {
        let u = uuid_of(s, t);
        let new = persistent.is_none() || u != persistent;
        if new {
            m.wild.angry_at = u;
            m.anger_target = Some(t);
        }
        if new || stay_if_target {
            start_anger_timer(s, m);
        }
    }
    if persistent.is_some()
        && !is_angry(s, m)
        && (m.target.is_none_or(|t| !valid_player_target(s, t)) || !stay_if_target)
    {
        m.stop_being_angry();
    }
    if persistent == Some(s.uuid) && (s.game_mode == 1 || s.game_mode == 3 || s.level.difficulty_id() == 0) {
        m.stop_being_angry();
    }
}

/// End of the AI step: delays, roll (angry at a target within 2 blocks), hive
/// checked every 20 ticks.
pub fn post_ai_step(s: &mut Server, m: &mut Mob) {
    let w = &mut m.wild;
    if w.stay_out > 0 {
        w.stay_out -= 1;
    }
    if w.hive_cd > 0 {
        w.hive_cd -= 1;
    }
    if w.flower_cd > 0 {
        w.flower_cd -= 1;
    }
    let roll =
        is_angry(s, m) && !has_stung(m) && m.target.and_then(|t| s.ent_pos(t)).is_some_and(|p| dist2(p, m.pos) < 4.0);
    m.wild.set_flag(ROLL, roll);
    if m.tick_count % 20 == 0 && !hive_valid(s, m) {
        m.wild.hive = None;
    }
}

// ---------- Controls ----------

/// The flight movement control (turn 20, hover: gravity stays off when stopped).
pub fn move_control(m: &mut Mob) {
    if m.move_ctl.op == MoveOp::MoveTo {
        m.move_ctl.op = MoveOp::Wait;
        m.surf.no_gravity = true;
        let w = m.move_ctl.wanted;
        let (xd, yd, zd) = (w[0] - m.pos[0], w[1] - m.pos[1], w[2] - m.pos[2]);
        let dd = xd * xd + yd * yd + zd * zd;
        if dd < f64::from(2.500_000_3e-7f32) {
            m.yya = 0.0;
            m.zza = 0.0;
            return;
        }
        let yr = (mth::atan2(zd, xd) * f64::from(180.0f32) / f64::from(std::f32::consts::PI)) as f32 - 90.0;
        m.yaw = mth::rotlerp_360(m.yaw, yr, 90.0);
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
            m.pitch = mth::rotlerp_360(m.pitch, xr, 20.0);
            m.yya = if yd > 0.0 { speed } else { -speed };
        }
    } else {
        m.yya = 0.0;
        m.zza = 0.0;
    }
}

/// Bee gaze control: no gaze when angry; pitch kept during pollination.
pub fn look_control(s: &Server, m: &mut Mob) -> bool {
    if is_angry(s, m) {
        return true;
    }
    look_tick(m, !m.wild.pollinating);
    true
}

/// The gaze control with optional pitch reset to zero.
pub fn look_tick(m: &mut Mob, reset_pitch: bool) {
    if reset_pitch {
        m.pitch = 0.0;
    }
    if m.look.cooldown > 0 {
        m.look.cooldown -= 1;
        let w = m.look.wanted;
        let (xd, zd) = (w[0] - m.pos[0], w[2] - m.pos[2]);
        if zd.abs() > f64::from(1.0e-5f32) || xd.abs() > f64::from(1.0e-5f32) {
            m.head_yaw = rotate_towards(m.head_yaw, mth::yaw_to(xd, zd), m.look.y_max);
        }
        let yd = w[1] - m.eye_y();
        let sd = (xd * xd + zd * zd).sqrt();
        if yd.abs() > f64::from(1.0e-5f32) || sd.abs() > f64::from(1.0e-5f32) {
            m.pitch = rotate_towards(m.pitch, mth::pitch_to(yd, sd), m.look.x_max);
        }
    } else {
        m.head_yaw = rotate_towards(m.head_yaw, m.body_yaw, 10.0);
    }
    if !m.nav.is_done() {
        m.head_yaw = rotate_if_necessary(m.head_yaw, m.body_yaw, super::super::goals::max_head_y(m.kind));
    }
}

// ---------- Combat ----------

/// Sting (integer damage), stinger planted, poison by difficulty, anger
/// extinguished, "has stung".
pub fn sting(s: &mut Server, m: &mut Mob, t: Ent) -> bool {
    let dmg = (m.attrs.attack.value() as i32) as f32;
    let ty = "minecraft:sting";
    let hurt = match t {
        Ent::Player => {
            let kb = (m.pos[0] - s.pos[0], m.pos[2] - s.pos[2]);
            s.player_hurt_by_mob(m, ty, dmg, Some(kb))
        }
        Ent::Mob(id) => {
            let Some(i) = s.mob_index(id) else { return false };
            let Some(mut o) = s.mobs.list[i].take() else { return false };
            let r = s.mob_hurt_by(&mut o, m, ty, dmg);
            s.mobs.list[i] = Some(o);
            r
        }
    };
    if hurt {
        let poison = match s.level.difficulty_id() {
            2 => 10,
            3 => 18,
            _ => 0,
        };
        match t {
            Ent::Player => {
                super::hive::add_player_stinger(s);
                if poison > 0 {
                    s.give_player_effect_from("minecraft:poison", poison * 20, 0, Some(&*m));
                }
            }
            Ent::Mob(id) => {
                if let Some(i) = s.mob_index(id)
                    && let Some(mut o) = s.mobs.list[i].take()
                {
                    let n = match o.get(13) {
                        Some(rc_proto_v777::entity::Meta::Int(n)) => *n,
                        _ => 0,
                    };
                    o.set(13, rc_proto_v777::entity::Meta::Int(n + 1));
                    if poison > 0 {
                        s.give_mob_effect(&mut o, "minecraft:poison", poison * 20, 0);
                    }
                    s.mobs.list[i] = Some(o);
                }
            }
        }
        m.wild.set_flag(STUNG, true);
        m.stop_being_angry();
        s.mob_sound(m, "minecraft:entity.bee.sting", 1.0, 1.0);
    }
    hurt
}

/// Right click: flower with an effect (open eyeblossom: poison 25; wither rose:
/// wither 40) consumed; the rest follows animal food.
pub fn interact(s: &mut Server, m: &mut Mob, slot: usize, item: &str) -> Option<bool> {
    if !s.item_tag(item, "minecraft:bee_food") {
        return None;
    }
    let (id, dur) = match item {
        "minecraft:open_eyeblossom" => ("minecraft:poison", 25),
        "minecraft:wither_rose" => ("minecraft:wither", 40),
        _ => return None,
    };
    s.consume_held(slot);
    s.give_mob_effect(m, id, dur, 0);
    Some(true)
}

// ---------- Movements ----------

/// An aerial step toward the target (range by distance), path
/// search limited by half.
pub fn pathfind_randomly_towards(s: &Server, m: &mut Mob, target: [i32; 3]) {
    let tv = [f64::from(target[0]) + 0.5, f64::from(target[1]), f64::from(target[2]) + 0.5];
    let b = m.block_pos();
    let yd = tv[1] as i32 - b[1];
    let y_adjust = if yd > 2 {
        4
    } else if yd < -2 {
        -4
    } else {
        0
    };
    let (mut xz, mut yv) = (6, 8);
    let dist = (b[0] - target[0]).abs() + (b[1] - target[1]).abs() + (b[2] - target[2]).abs();
    if dist < 15 {
        xz = dist / 2;
        yv = dist / 2;
    }
    let p = super::rpos::air_pos_towards(s, m, xz, yv, y_adjust, tv, f64::from(std::f32::consts::PI / 10.0));
    if let Some(p) = p {
        set_visited_multiplier(m, 0.5);
        s.move_to_pos(m, p, 1.0);
    }
}

/// The setting of the visited-nodes multiplier.
pub fn set_visited_multiplier(m: &mut Mob, k: f32) {
    m.nav.max_visited = (m.follow_range() * 16.0 * f64::from(k)).floor() as i32;
}

/// The reset of the visited-nodes multiplier.
pub fn reset_visited_multiplier(m: &mut Mob) {
    set_visited_multiplier(m, 1.0);
}

/// Positions of a cube traversed by increasing Manhattan distance.
pub fn manhattan_box(o: [i32; 3], rx: i32, ry: i32, rz: i32) -> Vec<[i32; 3]> {
    let mut out = Vec::new();
    for depth in 0..=rx + ry + rz {
        let max_x = rx.min(depth);
        for x in -max_x..=max_x {
            let max_y = ry.min(depth - x.abs());
            for y in -max_y..=max_y {
                let z = depth - x.abs() - y.abs();
                if z <= rz {
                    out.push([o[0] + x, o[1] + y, o[2] + z]);
                    if z != 0 {
                        out.push([o[0] + x, o[1] + y, o[2] - z]);
                    }
                }
            }
        }
    }
    out
}
