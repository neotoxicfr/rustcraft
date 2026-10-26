//! Spear seen in third person: pose of the arm holding it (raise, sway, lowering,
//! raise again as it is used), movement of the held item during use, and thrust attack
//! (arm and item). The curves follow the first-person pose (`hand.rs`).

use std::f32::consts::PI;

use glam::Mat4;

use crate::hand::{Kinetic, around, ease, hit_feedback, progress};

const DEG: f32 = PI / 180.0;

fn in_quad(x: f32) -> f32 {
    x * x
}

fn in_out_expo(x: f32) -> f32 {
    if x < 0.5 {
        if x == 0.0 { 0.0 } else { (2f64.powf(20.0 * f64::from(x) - 10.0) / 2.0) as f32 }
    } else if x == 1.0 {
        1.0
    } else {
        ((2.0 - 2f64.powf(-20.0 * f64::from(x) + 10.0)) / 2.0) as f32
    }
}

fn out_back(x: f32) -> f32 {
    1.0 + 2.70158 * (x - 1.0).powi(3) + 1.70158 * (x - 1.0).powi(2)
}

/// Progress of the use pose after `time` ticks.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct UseParams {
    pub raise: f32,
    pub raise_start: f32,
    pub raise_mid: f32,
    pub raise_end: f32,
    pub sway: f32,
    pub lower: f32,
    pub back: f32,
    pub intensity: f32,
    pub slow: f32,
    pub fast: f32,
}

pub fn use_params(k: Kinetic, time: f32) -> UseParams {
    let raised = k.delay;
    let sway_end = k.dismount + raised;
    let sway_start = sway_end - 20;
    let lower_end = k.knockback + raised;
    let lower_start = lower_end - 40;
    let back_end = k.damage + raised;
    let raise = progress(time, 0.0, raised as f32);
    let sway = progress(time, sway_start as f32, sway_end as f32);
    let back = progress(time, (back_end - 5) as f32, back_end as f32);
    let intensity = 2.0 * ease::out_circ(sway) - 2.0 * ease::in_circ(back);
    UseParams {
        raise,
        raise_start: progress(raise, 0.0, 0.5),
        raise_mid: progress(raise, 0.5, 0.8),
        raise_end: progress(raise, 0.8, 1.0),
        sway,
        lower: ease::out_cubic(ease::in_out_elastic(progress(time - 20.0, lower_start as f32, lower_end as f32))),
        back,
        intensity,
        slow: crate::sky_mesh::table_sin(time * 19.0 * DEG) * intensity,
        fast: crate::sky_mesh::table_sin(time * 30.0 * DEG) * intensity,
    }
}

/// Arm holding a spear ("spear" pose): arm angles (x, y, z) derived from those of the head.
/// `using`: (ticks of use, spear parameters) when this arm is using it.
pub fn hand_pose(
    arm: [f32; 3],
    head: (f32, f32),
    right: bool,
    flying_or_swimming: bool,
    using: Option<(f32, Kinetic)>,
) -> [f32; 3] {
    let inv = if right { 1.0 } else { -1.0 };
    let mut y = -0.1 * inv + head.1;
    let mut x = -PI / 2.0 + head.0 + 0.8;
    let mut z = arm[2];
    if flying_or_swimming {
        x -= 0.959_931_1;
    }
    y = DEG * (y / DEG).clamp(-60.0, 60.0);
    x = DEG * (x / DEG).clamp(-120.0, 30.0);
    if let Some((ticks, k)) = using
        && ticks > 0.0
    {
        let p = use_params(k, ticks);
        y += -inv * p.fast * DEG * p.intensity;
        z += -inv * p.slow * DEG * p.intensity * 0.5;
        x += DEG
            * (-40.0 * p.raise_start
                + 30.0 * p.raise_mid
                + -20.0 * p.raise_end
                + 20.0 * p.lower
                + 10.0 * p.back
                + 0.6 * p.slow * p.intensity);
    }
    [x, y, z]
}

/// Spear being used, in hand (after the hand offset, before the display transform):
/// `attack` = progress of the current attack (0 without), `feedback` = ticks since the
/// last hit feedback.
pub fn use_item(k: Kinetic, forward: f32, time: f32, right: bool, attack: f32, feedback: f32) -> Mat4 {
    if time == 0.0 {
        return Mat4::IDENTITY;
    }
    let atk = in_quad(progress(attack, 0.05, 0.2));
    let retract = in_out_expo(progress(attack, 0.4, 1.0));
    let p = use_params(k, time);
    let inv = if right { 1.0 } else { -1.0 };
    let raise_mod = 1.0 - out_back(1.0 - p.raise);
    let fb = hit_feedback(feedback);
    Mat4::from_translation(glam::Vec3::new(0.0, (f64::from(-fb) * 0.4) as f32, -forward * (raise_mod - p.back) + fb))
        * around(
            Mat4::from_rotation_x(-(70.0 * (p.raise - p.back) - 40.0 * (atk - retract)) * DEG),
            0.0,
            -0.03125,
            0.125,
        )
        * around(Mat4::from_rotation_y(inv * 90.0 * (p.raise - p.sway + 3.0 * retract + atk) * DEG), 0.0, 0.0, 0.125)
}

/// Thrust attack: extra x rotation of the striking arm.
pub fn attack_arm(attack: f32) -> f32 {
    let prepare = ease::in_out_sine(progress(attack, 0.0, 0.05));
    let atk = in_quad(progress(attack, 0.05, 0.2));
    let retract = in_out_expo(progress(attack, 0.4, 1.0));
    (90.0 * prepare - 120.0 * atk + 30.0 * retract) * DEG
}

/// Thrust attack: held item (no movement without an attack in progress).
pub fn attack_item(attack: f32, forward: f32) -> Mat4 {
    if attack <= 0.0 {
        return Mat4::IDENTITY;
    }
    let atk = in_quad(progress(attack, 0.05, 0.2));
    let retract = in_out_expo(progress(attack, 0.4, 1.0));
    around(Mat4::from_rotation_x(-70.0 * (atk - retract) * DEG), 0.0, -0.125, 0.125)
        * Mat4::from_translation(glam::Vec3::new(0.0, forward * (atk - retract), 0.0))
}

/// Forward movement of the stack's spear (`kinetic_weapon` component: network patch,
/// otherwise the item's default).
pub fn forward_movement(view: &crate::stack::StackRef<'_>) -> f32 {
    use rc_proto::{Reader, varint::read_varint};
    if let Some(b) = view.patch("minecraft:kinetic_weapon") {
        let r = &mut Reader::new(b);
        let parsed = (|| -> Option<f32> {
            read_varint(r).ok()?;
            read_varint(r).ok()?;
            for _ in 0..3 {
                if r.u8().ok()? != 0 {
                    read_varint(r).ok()?;
                    r.take(8).ok()?;
                }
            }
            Some(f32::from_be_bytes(r.take(4).ok()?.try_into().ok()?))
        })();
        return parsed.unwrap_or(0.0);
    }
    view.info.and_then(|i| i.kinetic_params).map_or(0.0, |k| k.forward_movement)
}

#[cfg(test)]
mod tests {
    use super::*;

    const IRON: Kinetic = Kinetic { delay: 12, dismount: 50, knockback: 135, damage: 225 };

    #[test]
    fn hand_pose_rest_and_clamp() {
        // Head straight: arm raised by -90° + 0.8 rad, no yaw beyond ±60°.
        let [x, y, _] = hand_pose([0.0; 3], (0.0, 0.0), true, false, None);
        assert!((x - (-PI / 2.0 + 0.8)).abs() < 1e-6);
        assert!((y + 0.1).abs() < 1e-6);
        let [x, y, _] = hand_pose([0.0; 3], (-2.0, 3.0), false, true, None);
        assert!((x / DEG + 120.0).abs() < 1e-3, "x={}", x / DEG);
        assert!((y / DEG - 60.0).abs() < 1e-3);
    }

    #[test]
    fn use_params_phases() {
        let p = use_params(IRON, 6.0);
        assert!((p.raise - 0.5).abs() < 1e-6 && p.raise_start == 1.0 && p.raise_mid == 0.0);
        let p = use_params(IRON, 300.0);
        assert_eq!((p.raise, p.sway, p.back), (1.0, 1.0, 1.0));
        assert!(p.intensity.abs() < 1e-6);
    }

    #[test]
    fn attack_curves() {
        assert_eq!(attack_arm(0.0), 0.0);
        // Preparation finished, strike not started: +90°.
        assert!((attack_arm(0.05) - 90.0 * DEG).abs() < 1e-5);
        // End of the attack: 90 - 120 + 30 = 0.
        assert!(attack_arm(1.0).abs() < 1e-5);
        assert_eq!(attack_item(0.0, 0.38), Mat4::IDENTITY);
    }
}
