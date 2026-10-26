//! Blaze (`server-mobs.md` § 4.2): wanted height drawn every 100 ticks, pushed
//! upward when the target is higher; attack: melee on contact, otherwise bursts of three
//! small fireballs (charge 60 ticks, shots every 6, pause 100).

use super::super::mob::Mob;
use super::super::mth::dist2;
use crate::server::Server;

/// Height offset (entity's random source), vertical push.
pub fn custom_ai_step(s: &mut Server, m: &mut Mob) {
    m.neth.next_height_change -= 1;
    if m.neth.next_height_change <= 0 {
        m.neth.next_height_change = 100;
        m.neth.height_offset = m.rng.triangle(0.5, 6.891) as f32;
    }
    let Some(t) = m.target else { return };
    let Some(eye) = s.ent_eye(t) else { return };
    if eye > m.eye_y() + f64::from(m.neth.height_offset) && super::goals::can_attack(s, t) {
        let vy = m.vel[1];
        m.vel[1] += (f64::from(0.3f32) - vy) * f64::from(0.3f32);
        m.needs_sync = true;
    }
}

/// Wanted position of the movement control (speed 1).
fn want(m: &mut Mob, p: [f64; 3]) {
    super::super::nav::set_wanted(m, p, 1.0);
}

/// Tick of the blaze's attack goal.
pub fn attack_tick(s: &mut Server, m: &mut Mob, step: &mut i32, time: &mut i32, last_seen: &mut i32) {
    *time -= 1;
    let Some(t) = m.target else { return };
    let (Some(tp), Some(tb)) = (s.ent_pos(t), s.ent_bbox(t)) else { return };
    let sight = s.line_of_sight(m, t);
    if sight {
        *last_seen = 0;
    } else {
        *last_seen += 1;
    }
    let d = dist2(m.pos, tp);
    let follow = m.follow_range();
    if d < 4.0 {
        if !sight {
            return;
        }
        if *time <= 0 {
            *time = 20;
            s.mob_attack(m, t);
        }
        want(m, tp);
    } else if d < follow * follow && sight {
        let xd = tp[0] - m.pos[0];
        let yd = (tb.min[1] + (tb.max[1] - tb.min[1]) * 0.5) - (m.pos[1] + f64::from(m.height()) * 0.5);
        let zd = tp[2] - m.pos[2];
        if *time <= 0 {
            *step += 1;
            if *step == 1 {
                *time = 60;
                m.neth.charged = true;
            } else if *step <= 4 {
                *time = 6;
            } else {
                *time = 100;
                *step = 0;
                m.neth.charged = false;
            }
            if *step > 1 {
                let sqd = d.sqrt().sqrt() * 0.5;
                // Event 1018 (shot sound) at the blaze's position.
                s.level_event(1018, m.block_pos(), 0);
                let dir = [m.rng.triangle(xd, 2.297 * sqd), yd, m.rng.triangle(zd, 2.297 * sqd)];
                let y = m.pos[1] + f64::from(m.height()) * 0.5 + 0.5;
                s.shoot_fireball(Some(m.uuid), [m.pos[0], y, m.pos[2]], dir, (m.yaw, m.pitch), false, 1);
            }
        }
        s.look_at(m, [tp[0], s.ent_eye(t).unwrap_or(tp[1]), tp[2]], 10.0, 10.0);
    } else if *last_seen < 5 {
        want(m, tp);
    }
}
