//! Full-screen animation of an activated item (totem of undying, entity event 35 of the local
//! player): 40 ticks, the item emerges from the back of the screen spinning then leaves, randomly offset
//! sideways. Drawn after the screen effects (fire, water, block) in the hand pass
//! (hand projection, shared depth), in view frame with the fixed lights of the
//! 3D items (`entity_render::items_3d_lights`), "fixed" display context, full
//! light; hidden with the interface (F1).

use std::f32::consts::PI;

use glam::{Mat4, Vec3};
use rc_proto_v777::item::ItemStack;

/// Duration of the animation (ticks).
pub const LENGTH: i32 = 40;

/// Index of the "fixed" context in the models' display transforms.
pub const FIXED: usize = 7;

/// Full light (block 15, sky 15) in the instance format.
pub const FULL_LIGHT: u32 = 240 | (240 << 8);

/// Item being activated.
#[derive(Default)]
pub struct ItemActivation {
    item: Option<ItemStack>,
    ticks: i32,
    off: [f32; 2],
    /// The first-person hand is drawn this frame (the pass may exist only for the
    /// animation, in third person).
    pub show_hand: bool,
}

impl ItemActivation {
    /// Starts the animation; `rx`, `ry` in [0, 1) give the lateral offset.
    pub fn activate(&mut self, stack: ItemStack, rx: f32, ry: f32) {
        self.item = Some(stack);
        self.ticks = LENGTH;
        self.off = [rx * 2.0 - 1.0, ry * 2.0 - 1.0];
    }

    /// Client tick (game not suspended).
    pub fn tick(&mut self) {
        if self.ticks > 0 {
            self.ticks -= 1;
            if self.ticks == 0 {
                self.item = None;
            }
        }
    }

    /// World change or disconnection.
    pub fn reset(&mut self) {
        self.item = None;
        self.ticks = 0;
    }

    pub fn active(&self) -> bool {
        self.item.is_some() && self.ticks > 0
    }

    /// Stack and matrix in the camera frame (before the "fixed" display).
    pub fn current(&self, pt: f32, aspect: f32) -> Option<(&ItemStack, Mat4)> {
        let item = self.item.as_ref().filter(|_| self.ticks > 0)?;
        Some((item, transform(self.ticks, pt, self.off, aspect)))
    }
}

/// The game's transform: progress t = (40 − ticks + pt) / 40, smoothed curve
/// 10.25t⁵ − 24.95t⁴ + 25.5t³ − 13.8t² + 4t, depth −10 + 9 sin(πc), lateral offset
/// 0.3 × offset (× width/height in x) × |sin(2πc)|, scale 0.8, Y rotation 900°×|sin(πc)|,
/// small X and Z rotations of 6° × cos(8t).
pub fn transform(ticks: i32, pt: f32, off: [f32; 2], aspect: f32) -> Mat4 {
    let tick = LENGTH - ticks;
    let t = (tick as f32 + pt) / LENGTH as f32;
    let t2 = t * t;
    let t3 = t * t2;
    let smooth = 10.25 * t3 * t2 - 24.95 * t2 * t2 + 25.5 * t3 - 13.8 * t2 + 4.0 * t;
    let a = smooth * PI;
    let side = (a * 2.0).sin().abs();
    let (ox, oy) = (off[0] * 0.3 * aspect, off[1] * 0.3);
    let wob = (6.0 * (t * 8.0).cos()).to_radians();
    Mat4::from_translation(Vec3::new(ox * side, oy * side, -10.0 + 9.0 * a.sin()))
        * Mat4::from_scale(Vec3::splat(0.8))
        * Mat4::from_rotation_y((900.0 * a.sin().abs()).to_radians())
        * Mat4::from_rotation_x(wob)
        * Mat4::from_rotation_z(wob)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stack() -> ItemStack {
        ItemStack { item: 1, count: 1, added: Vec::new(), removed: Vec::new() }
    }

    #[test]
    fn lasts_forty_ticks() {
        let mut a = ItemActivation::default();
        a.activate(stack(), 0.5, 0.5);
        for _ in 0..39 {
            a.tick();
            assert!(a.active());
        }
        a.tick();
        assert!(!a.active());
        assert!(a.current(0.0, 1.0).is_none());
    }

    #[test]
    fn starts_far_and_returns() {
        // Start: at the back (z = −10), no offset; in the middle, very close to the camera.
        let m = transform(LENGTH, 0.0, [1.0, 1.0], 16.0 / 9.0);
        let p = m.transform_point3(Vec3::ZERO);
        assert!((p.z + 10.0).abs() < 1e-5 && p.x.abs() < 1e-5 && p.y.abs() < 1e-5);
        // Curve: 4t − 13.8t² + 25.5t³ − 24.95t⁴ + 10.25t⁵ equals 1 at t = 1 (return to the back).
        let end = transform(0, 0.0, [0.0, 0.0], 1.0).transform_point3(Vec3::ZERO);
        assert!((end.z + 10.0).abs() < 1e-3, "{end:?}");
        let near = (0..=LENGTH)
            .map(|k| transform(k, 0.0, [0.0; 2], 1.0).transform_point3(Vec3::ZERO).z)
            .fold(f32::MIN, f32::max);
        assert!(near > -1.2, "{near}");
    }
}
