//! Bat (§ 4.1, `ambient` category): AI coded in the tick (rest under a block,
//! erratic flight toward randomly drawn points), no goals.

use super::super::mob::{Ent, Mob};
use super::super::mth;
use crate::server::Server;

fn resting(m: &Mob) -> bool {
    m.surf.bat_flags & 1 != 0
}

fn set_resting(m: &mut Mob, v: bool) {
    if v {
        m.surf.bat_flags |= 1;
    } else {
        m.surf.bat_flags &= !1;
    }
}

/// "Conductive" block (full opaque) above.
fn conductor(s: &Server, p: [i32; 3]) -> bool {
    s.solid_render(p)
}

/// End of `tick`: stuck to the ceiling at rest, otherwise vertical velocity damped.
pub fn post_tick(m: &mut Mob) {
    if m.removed {
        return;
    }
    if resting(m) {
        m.vel = [0.0; 3];
        m.pos[1] = m.pos[1].floor() + 1.0 - f64::from(m.height());
    } else {
        m.vel[1] *= 0.6;
    }
}

/// The species' own server AI step.
pub fn custom_ai_step(s: &mut Server, m: &mut Mob) {
    let pos = m.block_pos();
    let above = [pos[0], pos[1] + 1, pos[2]];
    if resting(m) {
        if conductor(s, above) {
            if m.rng.next_int_bound(200) == 0 {
                m.head_yaw = m.rng.next_int_bound(360) as f32;
            }
            if s.targeting_ok(m, Ent::Player, false, 4.0, true, true) {
                set_resting(m, false);
                s.level_event(1025, pos, 0);
            }
        } else {
            set_resting(m, false);
            s.level_event(1025, pos, 0);
        }
        return;
    }
    if let Some(t) = m.surf.bat_target
        && (!s.is_air(t) || t[1] <= s.dim.height.min_y)
    {
        m.surf.bat_target = None;
    }
    let renew = match m.surf.bat_target {
        None => true,
        Some(t) => {
            m.rng.next_int_bound(30) == 0 || {
                let c = [f64::from(t[0]) + 0.5, f64::from(t[1]) + 0.5, f64::from(t[2]) + 0.5];
                mth::dist2(c, m.pos) < 4.0
            }
        }
    };
    if renew {
        let x = m.pos[0] + f64::from(m.rng.next_int_bound(7)) - f64::from(m.rng.next_int_bound(7));
        let y = m.pos[1] + f64::from(m.rng.next_int_bound(6)) - 2.0;
        let z = m.pos[2] + f64::from(m.rng.next_int_bound(7)) - f64::from(m.rng.next_int_bound(7));
        m.surf.bat_target = Some(mth::block_of([x, y, z]));
    }
    let t = m.surf.bat_target.unwrap_or(pos);
    let dx = f64::from(t[0]) + 0.5 - m.pos[0];
    let dy = f64::from(t[1]) + 0.1 - m.pos[1];
    let dz = f64::from(t[2]) + 0.5 - m.pos[2];
    let v = m.vel;
    let k = f64::from(0.1f32);
    let nv = [
        v[0] + (dx.signum0() * 0.5 - v[0]) * k,
        v[1] + (dy.signum0() * f64::from(0.7f32) - v[1]) * k,
        v[2] + (dz.signum0() * 0.5 - v[2]) * k,
    ];
    m.vel = nv;
    let yrd = (mth::atan2(nv[2], nv[0]) * f64::from(180.0f32) / f64::from(std::f32::consts::PI)) as f32 - 90.0;
    let diff = mth::wrap(yrd - m.yaw);
    m.zza = 0.5;
    m.yaw += diff;
    if m.rng.next_int_bound(100) == 0 && conductor(s, above) {
        set_resting(m, true);
    }
}

/// Sign of a float (0 for 0).
trait Signum0 {
    fn signum0(self) -> f64;
}

impl Signum0 for f64 {
    fn signum0(self) -> f64 {
        if self > 0.0 {
            1.0
        } else if self < 0.0 {
            -1.0
        } else {
            self
        }
    }
}

/// Spawn rule (world random): below the surface, one time in two, dark enough.
pub fn spawn_rules(s: &mut Server, p: [i32; 3]) -> bool {
    if p[1] >= s.surface_y(p[0], p[2], 1) {
        return false;
    }
    if s.mobs.rng.next_bool() {
        return false;
    }
    if s.mob_brightness(p) > s.mobs.rng.next_int_bound(4) {
        return false;
    }
    let below = [p[0], p[1] - 1, p[2]];
    s.btag_id(below, crate::tag_bits::TagId::BatsSpawnableOn) && s.valid_spawn_on(below, super::super::species::Kind::Bat)
}
