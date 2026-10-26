//! Wither (`server-mobs.md`, rendering): body, three heads, invulnerability texture,
//! energy armor at half health, summoning scale.
//!
//! Entry points (called by `entity_render.rs` before its own rules):
//! - `layers`: body (cutout) and, at half health, armor ("energy swirl" pass);
//! - `skin`: bit 0 = invulnerability texture, bit 1 = armor;
//! - `animate`: rib cage, tail (oscillation), heads;
//! - `scale`: 2, reduced during the summoning;
//! - `swirl_offset`: texture offset of the armor;
//! - `track`: aim of the side heads (client state per tick).
//!
//! Client state: `extra[0..2]` = pitch of the right and left heads, `extra[2..4]` = absolute yaw
//! (degrees). Metadata: 16, 17, 18 = targets of the three heads (network ids), 19 = invulnerability
//! ticks; health = 9.

use super::*;
use crate::entities::Entities;

const KIND: &str = "wither";
const HEALTH_MAX: f32 = 300.0;
const DEG: f32 = PI / 180.0;

pub(super) fn layers(
    _r: &EntityRenderer,
    k: &str,
    _baby: bool,
    skin: u32,
) -> Option<Vec<(String, String, Pass, Wool)>> {
    if k != KIND {
        return None;
    }
    let tex = if skin & 1 != 0 { "entity/wither/wither_invulnerable" } else { "entity/wither/wither" };
    let mut v = vec![("minecraft:wither#main".to_owned(), tex.to_owned(), Pass::Cutout, Wool::None)];
    if skin & 2 != 0 {
        v.push(("minecraft:wither#armor".to_owned(), "entity/wither/wither_armor".to_owned(), Pass::Swirl, Wool::None));
    }
    Some(v)
}

/// Remaining invulnerability ticks (0 outside the summoning).
fn invulnerable(e: &Entity) -> i32 {
    e.meta_int(19).unwrap_or(0).max(0)
}

fn powered(e: &Entity) -> bool {
    matches!(e.meta.get(&9), Some(rc_proto_v777::entity::Meta::Float(h)) if *h <= HEALTH_MAX / 2.0)
}

pub(super) fn skin(_r: &EntityRenderer, e: &Entity) -> Option<u32> {
    if e.kind != KIND {
        return None;
    }
    // The game tests `floor(ticks − partial)`: one tick less over almost the whole frame.
    let n = (invulnerable(e) - 1).max(0);
    let invulnerable_texture = n > 0 && (n > 80 || n / 5 % 2 != 1);
    Some(u32::from(invulnerable_texture) | u32::from(powered(e)) << 1)
}

pub(super) fn shadow(k: &str) -> Option<(f32, f32)> {
    (k == KIND).then_some((1.0, 1.0))
}

pub(super) fn scale(e: &Entity, pt: f32, scale: &mut Vec3) {
    if e.kind != KIND {
        return;
    }
    let ticks = invulnerable(e) as f32 - pt;
    let mut s = 2.0;
    if ticks > 0.0 {
        s -= ticks / 220.0 * 0.5;
    }
    *scale *= s;
}

pub(super) fn animate(p: &mut Pose<'_>, e: &Entity, s: &AnimState, pt: f32) -> bool {
    if e.kind != KIND {
        return false;
    }
    let body = rot_lerp(pt, e.body_o, e.body);
    for (i, name) in ["right_head", "left_head"].into_iter().enumerate() {
        let (x, y) = (e.extra[i], e.extra[2 + i]);
        p.set(name, |h| {
            h[3] = x * DEG;
            h[4] = (y - body) * DEG;
        });
    }
    let anim = (s.age * 0.1).cos();
    let rib = (0.065 + 0.05 * anim) * PI;
    p.set("ribcage", |r| r[3] = rib);
    p.set("tail", |t| {
        t[1] = 6.9 + rib.cos() * 10.0;
        t[2] = -0.5 + rib.sin() * 10.0;
        t[3] = (0.265 + 0.1 * anim) * PI;
    });
    p.set("center_head", |h| {
        h[4] = s.yaw;
        h[3] = s.pitch;
    });
    true
}

/// (u, v) offset of the armor texture, in [0, 1[: `(cos(0.02 t) × 3, 0.01 t)` modulo 1.
pub(super) fn swirl_offset(t: f32) -> (f32, f32) {
    ((t * 0.02).cos() * 3.0 % 1.0, t * 0.01 % 1.0)
}

fn wrap(a: f32) -> f32 {
    let a = a % 360.0;
    if a >= 180.0 {
        a - 360.0
    } else if a < -180.0 {
        a + 360.0
    } else {
        a
    }
}

/// Moves `a` towards `b` by at most `max` degrees.
fn rotlerp(a: f32, b: f32, max: f32) -> f32 {
    a + wrap(b - a).clamp(-max, max)
}

/// One aiming tick of the side heads of each wither: pitch 40°/tick and yaw 10°/tick
/// towards the target (eyes), otherwise yaw towards the body direction.
pub(super) fn track(ents: &mut Entities) {
    let mut updates: Vec<(i32, [f32; 4])> = Vec::new();
    for (&id, e) in &ents.map {
        if e.kind != KIND {
            continue;
        }
        let mut x = [e.extra[0], e.extra[1]];
        let mut y = [e.extra[2], e.extra[3]];
        let target_of = |i: usize| {
            let tid = e.meta_int(17 + i as u8).filter(|&t| t > 0)?;
            if tid == ents.local {
                return ents.me.as_ref();
            }
            ents.map.get(&tid)
        };
        for i in 0..2 {
            match target_of(i) {
                Some(t) => {
                    let a = (e.body + 180.0 * i as f32) * DEG;
                    let hx = e.pos[0] + f64::from(a.cos()) * 1.3;
                    let hy = e.pos[1] + 2.2;
                    let hz = e.pos[2] + f64::from(a.sin()) * 1.3;
                    let eye = f64::from(t.dims(&ents.dims)[1]);
                    let (xd, yd, zd) = (t.pos[0] - hx, t.pos[1] + eye - hy, t.pos[2] - hz);
                    let sd = (xd * xd + zd * zd).sqrt();
                    let yaw_d = (zd.atan2(xd) as f32).to_degrees() - 90.0;
                    let pitch_d = -(yd.atan2(sd) as f32).to_degrees();
                    x[i] = rotlerp(x[i], pitch_d, 40.0);
                    y[i] = rotlerp(y[i], yaw_d, 10.0);
                }
                None => y[i] = rotlerp(y[i], e.body, 10.0),
            }
        }
        updates.push((id, [x[0], x[1], y[0], y[1]]));
    }
    for (id, v) in updates {
        if let Some(e) = ents.map.get_mut(&id) {
            e.extra[..4].copy_from_slice(&v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotlerp_is_clamped_and_wraps() {
        assert_eq!(rotlerp(0.0, 100.0, 10.0), 10.0);
        assert_eq!(rotlerp(0.0, -100.0, 40.0), -40.0);
        // By the shortest path: 170 → -170 goes through 180.
        assert_eq!(rotlerp(170.0, -170.0, 10.0), 180.0);
    }

    #[test]
    fn swirl_offset_stays_in_range() {
        for t in 0..2000 {
            let (u, v) = swirl_offset(t as f32 * 0.37);
            assert!((-1.0..1.0).contains(&u) && (0.0..1.0).contains(&v));
        }
    }
}
