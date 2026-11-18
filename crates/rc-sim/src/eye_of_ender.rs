//! G5-F10: Eye of Ender thrown in the air. The item looks for the nearest "eye-located"
//! structure (radius 100 chunks), drops an entity that flies toward it (at most 12 blocks
//! horizontally, 8 higher), without collision, for 80 ticks; then it falls as an item
//! (4 times out of 5) or breaks (event 2003). Rules recorded from game 26.3, in our own words.

use rc_proto_v777::entity::Meta;

use crate::player::Stack;
use crate::projectiles::{PKind, Proj};
use crate::server::Server;

/// Horizontal distance beyond which the eye aims at an intermediate point.
const TOO_FAR: f64 = 12.0;
/// Height added to that intermediate point.
const TOO_FAR_HEIGHT: f64 = 8.0;
/// Flight duration in ticks.
const LIFE: i32 = 80;

/// Flight target from `from` toward structure `feature` (low corner).
pub(crate) fn eye_target(from: [f64; 3], feature: [f64; 3]) -> [f64; 3] {
    let d = [feature[0] - from[0], feature[1] - from[1], feature[2] - from[2]];
    let h = (d[0] * d[0] + d[2] * d[2]).sqrt();
    if h > TOO_FAR {
        [from[0] + d[0] / h * TOO_FAR, from[1] + TOO_FAR_HEIGHT, from[2] + d[2] / h * TOO_FAR]
    } else {
        feature
    }
}

/// New speed of the eye: the horizontal speed tends (2.5 ‰ per tick) toward the horizontal
/// distance to the target, slowed by 20 % within one block; the vertical speed tends
/// (1.5 %) toward ±1 depending on whether the eye is below the target or not.
pub(crate) fn eye_motion(old: [f64; 3], pos: [f64; 3], target: [f64; 3]) -> [f64; 3] {
    let hd = [target[0] - pos[0], target[2] - pos[2]];
    let hl = (hd[0] * hd[0] + hd[1] * hd[1]).sqrt();
    let old_h = (old[0] * old[0] + old[2] * old[2]).sqrt();
    let mut speed = old_h + 0.0025 * (hl - old_h);
    let mut y = old[1];
    if hl < 1.0 {
        speed *= 0.8;
        y *= 0.8;
    }
    // The game's comparison reproduced as is: the position minus the vertical speed.
    let wanted = if pos[1] - old[1] < target[1] { 1.0 } else { -1.0 };
    let k = speed / hl;
    [hd[0] * k, y + (wanted - y) * 0.015, hd[1] * k]
}

impl Server {
    /// Use of the eye in the air (hand `hand`); true if the use is handled. Aiming at a portal
    /// frame, the use passes through (placing on the frame happens on the block click). Otherwise
    /// the prolonged use starts (zero duration: it lasts until release); with no
    /// structure found, nothing else.
    pub(crate) fn ender_eye_use(&mut self, hand: i32) -> bool {
        let slot = self.hand_slot(hand);
        let Some(s) = self.slots[slot].clone() else { return false };
        if let Some((p, _)) = self.pov_hit(false)
            && self.state_name(self.st(p)) == "minecraft:end_portal_frame"
        {
            return false;
        }
        self.start_using(hand, 0);
        let origin = self.pos.map(|v| v.floor() as i32);
        let Some(found) = self.nearest_structure("minecraft:eye_of_ender_located", origin, 100) else {
            return true;
        };
        let h = if self.sneaking { 1.5 } else { 1.8 };
        let at = [self.pos[0], self.pos[1] + h * 0.5, self.pos[2]];
        let id = self.proj_new_id();
        let mut one = s.clone();
        one.count = 1;
        let mut p = Proj::new(PKind::EyeOfEnder, id, at, one.clone());
        let net = self.net(Some(&one));
        p.set_meta(8, Meta::Item(net));
        p.eye_target = Some(eye_target(at, found.map(f64::from)));
        p.life = 0;
        p.eye_survive = p.rng.next_int_bound(5) > 0;
        self.game_event_at("minecraft:projectile_shoot", at, Some(crate::mobs::mob::Ent::Player));
        self.proj_add(p);
        self.trig_used_ender_eye(found);
        let r = self.life.rng.next_float();
        let pitch = 0.33 + r * (0.5 - 0.33);
        self.play_sound("minecraft:entity.ender_eye.launch", 6, self.pos, 1.0, pitch);
        if !self.infinite_materials() {
            let mut rest: Stack = s.clone();
            rest.count -= 1;
            self.slots[slot] = (rest.count > 0).then_some(rest);
            self.use_stack_emptied(slot);
        }
        self.award_stat("minecraft:used", &s.id, 1);
        self.broadcast_menus();
        true
    }

    /// One tick of the eye: base tick (portal, fluids, fire), free movement, new
    /// speed toward the target, end of flight after 80 ticks (sound, item or break).
    pub(crate) fn eye_tick(&mut self, p: &mut Proj) {
        self.proj_portal_tick(p);
        self.proj_base_tick(p, true);
        if p.removed {
            return;
        }
        let new = [p.pos[0] + p.vel[0], p.pos[1] + p.vel[1], p.pos[2] + p.vel[2]];
        if let Some(t) = p.eye_target {
            p.vel = eye_motion(p.vel, new, t);
        }
        p.pos = new;
        p.life += 1;
        if p.life > LIFE {
            self.play_sound("minecraft:entity.ender_eye.death", 6, p.pos, 1.0, 1.0);
            p.removed = true;
            if p.eye_survive {
                // New item without pickup delay, velocity drawn like any item entity.
                let r = &mut self.life.rng;
                let vel = [r.next_double() * 0.2 - 0.1, 0.2, r.next_double() * 0.2 - 0.1];
                self.spawn_item(p.pos, vel, p.item.clone(), 0);
            } else {
                self.level_event(2003, p.block_pos(), 0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Structure 100 blocks to the east: intermediate point at 12 blocks, 8 higher.
    #[test]
    fn far_target_is_clamped() {
        let t = eye_target([0.5, 64.9, 0.5], [100.0, 30.0, 0.0]);
        assert!((t[0] - (0.5 + 99.5 / (99.5f64 * 99.5 + 0.25).sqrt() * 12.0)).abs() < 1e-12);
        assert_eq!(t[1], 72.9);
        let near = eye_target([0.0, 64.0, 0.0], [5.0, 10.0, 5.0]);
        assert_eq!(near, [5.0, 10.0, 5.0]);
    }

    /// First tick since stopping, target 12 blocks east and 8 higher: horizontal
    /// speed 0.0025 × 12 = 0.03, vertical 0.015 upward.
    #[test]
    fn first_motion_step() {
        let v = eye_motion([0.0; 3], [0.0, 64.0, 0.0], [12.0, 72.0, 0.0]);
        assert!((v[0] - 0.03).abs() < 1e-15, "{v:?}");
        assert_eq!(v[2], 0.0);
        assert!((v[1] - 0.015).abs() < 1e-15);
        // A block under the target: 20 % braking; above: downward.
        let v = eye_motion([0.1, 0.2, 0.0], [11.5, 80.0, 0.0], [12.0, 72.0, 0.0]);
        let speed = (0.1 + 0.0025 * (0.5 - 0.1)) * 0.8;
        assert!((v[0] - speed).abs() < 1e-15);
        assert!((v[1] - (0.16 + (-1.0 - 0.16) * 0.015)).abs() < 1e-15);
    }
}
