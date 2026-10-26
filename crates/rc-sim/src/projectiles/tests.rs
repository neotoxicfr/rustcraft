//! Projectile tests (S8): pure formulas; trajectories are compared with the official
//! server in `tests_s8.rs`.

use super::physics::{aabb_clip, margin, nearest_face};
use super::weapons::bow_power;

#[test]
fn bow_power_curve() {
    assert_eq!(bow_power(0), 0.0);
    assert!((bow_power(10) - (0.25 + 1.0) / 3.0).abs() < 1e-6);
    assert_eq!(bow_power(20), 1.0);
    assert_eq!(bow_power(100), 1.0);
}

/// Arrow hit: horizontal direction normalised like the game, zero only below 1e-5
/// (single precision); a horizontal speed of 5e-5 still pushes.
#[test]
fn arrow_knockback_threshold() {
    use super::hits::arrow_knockback_push;
    assert_eq!(arrow_knockback_push([5.0e-5, -1.0, 0.0], 0.6), Some([0.6, 0.1, 0.0]));
    assert_eq!(arrow_knockback_push([0.0, -1.0, -3.0e-5], 1.2), Some([0.0, 0.1, -1.2]));
    assert_eq!(arrow_knockback_push([5.0e-6, -1.0, 0.0], 0.6), None);
    assert_eq!(arrow_knockback_push([3.0, 0.0, 4.0], 1.0), Some([0.6, 0.1, 0.8]));
}

/// Knockback of a mob hurt by a projectile: the direction is the projectile's velocity
/// as is; passing it back through a position (source = position + direction) rounds it.
#[test]
fn projectile_hurt_direction_exact() {
    use crate::mobs::combat::hurt_knockback_dir;
    let mut rounded = 0;
    for (x, kx) in [(230.7, -1.234_567_890_123_456_7), (-1021.3, 0.317_2), (12.9, 2.718_281_7)] {
        let pos = [x, -59.0, x * 0.5];
        let kb = (kx, -kx * 0.7);
        let src = [pos[0] + kb.0, pos[1], pos[2] + kb.1];
        if hurt_knockback_dir(None, Some(src), pos) != kb {
            rounded += 1;
        }
        assert_eq!(hurt_knockback_dir(Some(kb), Some(src), pos), kb);
    }
    assert!(rounded > 0, "expected rounding case");
}

#[test]
fn entity_margin() {
    assert_eq!(margin(1), 0.0);
    assert_eq!(margin(2), 0.0);
    assert!((margin(3) - f64::from(0.05f32)).abs() < 1e-12);
    assert!((margin(100) - f64::from(0.3f32)).abs() < 1e-12);
}

#[test]
fn clip_faces() {
    let b = rc_physics::Aabb { min: [0.0, 0.0, 0.0], max: [1.0, 1.0, 1.0] };
    let (t, f) = aabb_clip(&b, [-1.0, 0.5, 0.5], [2.0, 0.5, 0.5]).unwrap();
    assert!((t - 1.0 / 3.0).abs() < 1e-12);
    assert_eq!(f, 4); // west
    let (_, f) = aabb_clip(&b, [0.5, 2.0, 0.5], [0.5, -1.0, 0.5]).unwrap();
    assert_eq!(f, 1); // up
    assert!(aabb_clip(&b, [0.5, 0.5, 0.5], [0.5, 3.0, 0.5]).is_none());
    assert_eq!(nearest_face([0.1, -1.0, 0.2]), 0);
}

#[test]
fn target_block_strength() {
    use super::physics::BlockHit;
    let hit = |loc: [f64; 3], face| BlockHit { loc, pos: [0, 0, 0], face, inside: false, border: false };
    assert_eq!(super::hits::target_strength(hit([0.5, 1.0, 0.5], 1)), 15);
    assert_eq!(super::hits::target_strength(hit([0.0, 1.0, 0.0], 1)), 1);
}
