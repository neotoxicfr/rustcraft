//! Camera (`docs/specs/hand-camera.md` § 3 to 5): perspective (F5), third-person
//! distance, smoothed eye height, field of view, view bobbing and hurt shake.

use glam::{Mat4, Vec3};
use rc_data::{Aabb, VersionData};

/// Camera settings (options menu plugged in later).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraSettings {
    /// Vertical field of view from the option, in degrees (30 to 110).
    pub fov: f32,
    /// Scale of the field-of-view effects (0 to 1).
    pub fov_effect_scale: f32,
    /// View bobbing while walking.
    pub bob_view: bool,
    /// Strength of the hurt shake (0 to 1).
    pub damage_tilt: f32,
}

impl Default for CameraSettings {
    fn default() -> Self {
        CameraSettings { fov: 70.0, fov_effect_scale: 1.0, bob_view: true, damage_tilt: 1.0 }
    }
}

/// Perspective (F5 key): first person, third person from behind, third person from the front.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Perspective {
    #[default]
    First,
    Back,
    Front,
}

impl Perspective {
    pub fn next(self) -> Self {
        match self {
            Perspective::First => Perspective::Back,
            Perspective::Back => Perspective::Front,
            Perspective::Front => Perspective::First,
        }
    }

    /// Initial test mode: `RUSTCRAFT_VIEW=0|1|2`.
    pub fn from_env() -> Self {
        match rc_data::env_str!("RUSTCRAFT_VIEW") {
            Some("1") => Perspective::Back,
            Some("2") => Perspective::Front,
            _ => Perspective::First,
        }
    }

    pub fn detached(self) -> bool {
        self != Perspective::First
    }
}

/// What the camera reads from the local player on every tick.
pub struct TickIn {
    pub eye_height: f64,
    pub flying: bool,
    /// `movement_speed` attribute (sprint included).
    pub movement_speed: f64,
    pub on_ground: bool,
    pub swimming: bool,
    pub dead: bool,
    pub riding: bool,
    /// Horizontal length of the tick's actual movement.
    pub moved: f64,
    /// Horizontal speed after the tick.
    pub speed: f64,
    pub yaw: f32,
    pub pitch: f32,
}

/// Camera position and orientation for a frame.
#[derive(Debug, Clone, Copy)]
pub struct Shot {
    pub pos: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
}

/// Projection parameters of the frame, read by the renderer.
#[derive(Debug, Clone, Copy)]
pub struct RenderView {
    /// World field of view (degrees).
    pub fov: f32,
    /// Field of view of the visibility sort: max(fov, option).
    pub cull_fov: f32,
    /// Field of view of the hand.
    pub hud_fov: f32,
    /// Hurt shake then view bobbing, in view space (world and hand).
    pub tilt: Mat4,
    /// Hand drawn (first person).
    pub hand: bool,
}

impl Default for RenderView {
    fn default() -> Self {
        RenderView { fov: 70.0, cull_fov: 70.0, hud_fov: 70.0, tilt: Mat4::IDENTITY, hand: false }
    }
}

/// Aiming direction (game convention: yaw 0 = +z, positive pitch = downward).
pub fn forward(yaw: f32, pitch: f32) -> Vec3 {
    let (yr, pr) = (yaw.to_radians(), pitch.to_radians());
    Vec3::new(-yr.sin() * pr.cos(), -pr.sin(), yr.cos() * pr.cos())
}

/// View rotation (world → camera), without translation. Like the game, the camera is rotated
/// first in yaw then in pitch (camera = rot_y(π − yaw) · rot_x(−pitch)): no
/// singularity at ±90. A "look at" view with a fixed +y up vector degenerates at the vertical
/// (cos(π/2) in f32 is −4e-8: the horizontal look direction flips and the image turns upside down).
pub fn view_rotation(yaw: f32, pitch: f32) -> Mat4 {
    Mat4::from_rotation_x(pitch.to_radians()) * Mat4::from_rotation_y(yaw.to_radians() - std::f32::consts::PI)
}

/// Screen axes in the world: (right, up, look), continuous up to ±90 of pitch.
/// To use instead of `look × +y` (same degeneracy as the "look at" view).
pub fn axes(yaw: f32, pitch: f32) -> (Vec3, Vec3, Vec3) {
    let v = view_rotation(yaw, pitch);
    (v.row(0).truncate(), v.row(1).truncate(), -v.row(2).truncate())
}

/// Integer origin (x, z) of the render frame and view matrix in that frame.
pub fn view_matrix(eye: [f64; 3], yaw: f32, pitch: f32) -> ([i32; 2], Vec3, Mat4) {
    let origin = [eye[0].floor() as i32, eye[2].floor() as i32];
    let local =
        Vec3::new((eye[0] - f64::from(origin[0])) as f32, eye[1] as f32, (eye[2] - f64::from(origin[1])) as f32);
    (origin, local, view_rotation(yaw, pitch) * Mat4::from_translation(-local))
}

/// Visual shape class of a block (§ 3.3).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Visual {
    Collision,
    Empty,
    Full,
    Outline,
}

fn visual_of(name: &str) -> Visual {
    let n = name.strip_prefix("minecraft:").unwrap_or(name);
    if n == "glass"
        || n == "tinted_glass"
        || n.ends_with("_stained_glass")
        || n.ends_with("glass_pane")
        || n == "iron_bars"
        || n.contains("copper_bars")
        || n.contains("copper_grate")
        || n == "powder_snow"
    {
        Visual::Empty
    } else if n == "mud" || n == "soul_sand" {
        Visual::Full
    } else if n.ends_with("_fence") || n == "snow" {
        Visual::Outline
    } else {
        Visual::Collision
    }
}

static FULL_CUBE: [Aabb; 1] = [Aabb { min: [0.0; 3], max: [1.0; 3] }];

pub struct Camera {
    pub settings: CameraSettings,
    pub view: Perspective,
    /// Walking speed from the abilities (`player_abilities` packet).
    pub walking_speed: f32,
    eye: (f64, f64),
    eye_set: bool,
    fov_mod: (f32, f32),
    bob: (f32, f32),
    walk: (f32, f32),
    /// Hand tilt: smoothed pitch and yaw (old, current).
    sway: [f32; 4],
    /// Death time of the local player (ticks, 0 alive).
    pub death_time: i32,
    /// Visual shape per block, filled on first use.
    visual: Vec<Visual>,
    /// First-person hands (created on the first tick).
    pub hand: Option<crate::hand::HandState>,
    /// Item in use that changes the field of view: drawn bow (use ticks) or spyglass.
    pub use_fov: UseFov,
}

/// Item in use taken into account by the field of view (§ 4.1).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum UseFov {
    #[default]
    None,
    /// Bow drawn for this many ticks.
    Bow(i32),
    Spyglass,
}

impl Default for Camera {
    fn default() -> Self {
        Camera {
            settings: CameraSettings::default(),
            view: Perspective::from_env(),
            walking_speed: 0.1,
            eye: (1.62, 1.62),
            eye_set: false,
            fov_mod: (1.0, 1.0),
            bob: (0.0, 0.0),
            walk: (0.0, 0.0),
            sway: [0.0; 4],
            death_time: 0,
            visual: Vec::new(),
            hand: None,
            use_fov: UseFov::None,
        }
    }
}

fn lerp(t: f32, a: f32, b: f32) -> f32 {
    a + t * (b - a)
}

impl Camera {
    /// One tick of the local player (§ 3.2, 4.2, 5.1, 2.2).
    pub fn tick(&mut self, t: &TickIn) {
        if !self.eye_set {
            self.eye = (t.eye_height, t.eye_height);
            self.eye_set = true;
            self.sway = [t.pitch, t.pitch, t.yaw, t.yaw];
        }
        self.eye.0 = self.eye.1;
        self.eye.1 += (t.eye_height - self.eye.1) * 0.5;
        // Field of view.
        let mut m = 1.0f32;
        if t.flying {
            m *= 1.1;
        }
        let v = self.walking_speed;
        if v != 0.0 {
            m *= ((t.movement_speed as f32 / v) + 1.0) / 2.0;
        }
        // Bow: narrowed by 15% × (min(ticks / 20, 1))²; spyglass in first person: 0.1
        // without the effects scale.
        let target = match self.use_fov {
            UseFov::Spyglass if self.view == Perspective::First => 0.1,
            UseFov::Bow(t) => {
                let s = (t as f32 / 20.0).min(1.0);
                lerp(self.settings.fov_effect_scale, 1.0, m * (1.0 - s * s * 0.15))
            }
            _ => lerp(self.settings.fov_effect_scale, 1.0, m),
        };
        self.fov_mod.0 = self.fov_mod.1;
        self.fov_mod.1 = (self.fov_mod.1 + (target - self.fov_mod.1) * 0.5).clamp(0.1, 1.5);
        // View bobbing.
        self.bob.0 = self.bob.1;
        if t.riding {
            self.bob.1 = 0.0;
        } else {
            let b = if t.on_ground && !t.dead && !t.swimming { (t.speed as f32).min(0.1) } else { 0.0 };
            self.bob.1 += (b - self.bob.1) * 0.4;
        }
        self.walk.0 = self.walk.1;
        self.walk.1 += 0.6 * t.moved as f32;
        // Hand tilt.
        self.sway[0] = self.sway[1];
        self.sway[2] = self.sway[3];
        self.sway[1] += (t.pitch - self.sway[1]) * 0.5;
        self.sway[3] += (t.yaw - self.sway[3]) * 0.5;
        self.death_time = if t.dead { self.death_time + 1 } else { 0 };
    }

    /// Eye height smoothed at render time.
    pub fn eye_height(&self, pt: f32) -> f64 {
        self.eye.0 + f64::from(pt) * (self.eye.1 - self.eye.0)
    }

    /// Camera position (§ 3.2, 3.3): `feet` = interpolated feet, `distance` = scale ×
    /// `camera_distance` attribute, `state` = world block state.
    #[allow(clippy::too_many_arguments)]
    pub fn place(
        &mut self,
        data: &VersionData,
        pt: f32,
        feet: [f64; 3],
        yaw: f32,
        pitch: f32,
        distance: f64,
        state: &dyn Fn(i32, i32, i32) -> u32,
    ) -> Shot {
        let eye = [feet[0], feet[1] + self.eye_height(pt), feet[2]];
        let (yaw, pitch) = match self.view {
            Perspective::First => return Shot { pos: eye, yaw, pitch },
            Perspective::Back => (yaw, pitch),
            Perspective::Front => (yaw + 180.0, -pitch),
        };
        if self.visual.is_empty() {
            self.visual = data.blocks.iter().map(|b| visual_of(&b.name)).collect();
        }
        let fwd = forward(yaw, pitch).as_dvec3();
        let mut d = distance;
        let visual = &self.visual;
        let shape = |x: i32, y: i32, z: i32| -> crate::pick::Shape<'_> {
            let s = state(x, y, z);
            let Some(info) = data.state(s) else { return crate::pick::plain(&[]) };
            // Shapes that follow the block's offset (bamboo, dripstones): offset here too.
            match visual.get(usize::from(info.block)).copied().unwrap_or(Visual::Collision) {
                Visual::Collision => (data.shape(info.collision), crate::crack::shape_shift(data, s, [x, y, z])),
                Visual::Empty => crate::pick::plain(&[]),
                Visual::Full => crate::pick::plain(&FULL_CUBE),
                Visual::Outline => (data.shape(info.outline), crate::crack::shape_shift(data, s, [x, y, z])),
            }
        };
        for i in 0..8 {
            let c = [(i & 1) as f64, ((i >> 1) & 1) as f64, ((i >> 2) & 1) as f64].map(|v| v * 0.2 - 0.1);
            let from = [eye[0] + c[0], eye[1] + c[1], eye[2] + c[2]];
            let to = [from[0] - fwd.x * d, from[1] - fwd.y * d, from[2] - fwd.z * d];
            if let Some(crate::pick::Hit::Block { at, .. }) = crate::pick::clip_blocks(from, to, &shape) {
                let d2: f64 = (0..3).map(|k| (at[k] - eye[k]).powi(2)).sum();
                if d2 < d * d {
                    d = d2.sqrt();
                }
            }
        }
        Shot { pos: [eye[0] - fwd.x * d, eye[1] - fwd.y * d, eye[2] - fwd.z * d], yaw, pitch }
    }

    /// Projection of the frame (§ 4.3, 5): `fluid` = water or lava in the camera, `hurt` =
    /// (remaining hurt time − pt, direction) of the local player.
    pub fn render_view(&self, pt: f32, fluid: bool, hurt: Option<(f32, f32)>) -> RenderView {
        let s = &self.settings;
        let mut fov = s.fov * lerp(pt, self.fov_mod.0, self.fov_mod.1);
        let mut hud = 70.0f32;
        let dying = self.death_time > 0;
        let d = (self.death_time as f32 + pt).min(20.0);
        if dying {
            let k = (1.0 - 500.0 / (d + 500.0)) * 2.0 + 1.0;
            fov /= k;
            hud /= k;
        }
        if fluid {
            let k = lerp(s.fov_effect_scale, 1.0, 6.0 / 7.0);
            fov *= k;
            hud *= k;
        }
        // Hurt shake then view bobbing (view space).
        let mut tilt = Mat4::IDENTITY;
        if dying {
            tilt *= Mat4::from_rotation_z((40.0 - 8000.0 / (d + 200.0)).to_radians());
        }
        if let Some((h, dir)) = hurt
            && h >= 0.0
        {
            let h = ((h / 10.0).powi(4) * std::f32::consts::PI).sin();
            tilt *= Mat4::from_rotation_y((-dir).to_radians())
                * Mat4::from_rotation_z((-h * 14.0 * s.damage_tilt).to_radians())
                * Mat4::from_rotation_y(dir.to_radians());
        }
        if s.bob_view {
            let w = -(self.walk.1 + (self.walk.1 - self.walk.0) * pt);
            let b = lerp(pt, self.bob.0, self.bob.1);
            let pi = std::f32::consts::PI;
            tilt *= Mat4::from_translation(Vec3::new((w * pi).sin() * b * 0.5, -((w * pi).cos() * b).abs(), 0.0))
                * Mat4::from_rotation_z(((w * pi).sin() * b * 3.0).to_radians())
                * Mat4::from_rotation_x((((w * pi - 0.2).cos() * b).abs() * 5.0).to_radians());
        }
        RenderView { fov, cull_fov: fov.max(s.fov), hud_fov: hud, tilt, hand: self.view == Perspective::First }
    }

    /// Hand tilt according to the rotation (§ 2.2), view = yaw and pitch of the frame.
    pub fn hand_sway(&self, pt: f32, yaw: f32, pitch: f32) -> Mat4 {
        let x = lerp(pt, self.sway[0], self.sway[1]);
        let y = lerp(pt, self.sway[2], self.sway[3]);
        Mat4::from_rotation_x(((pitch - x) * 0.1).to_radians()) * Mat4::from_rotation_y(((yaw - y) * 0.1).to_radians())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycle() {
        let p = Perspective::First;
        assert_eq!(p.next(), Perspective::Back);
        assert_eq!(p.next().next(), Perspective::Front);
        assert_eq!(p.next().next().next(), Perspective::First);
    }

    #[test]
    fn visual_classes() {
        assert!(visual_of("minecraft:glass") == Visual::Empty);
        assert!(visual_of("minecraft:red_stained_glass_pane") == Visual::Empty);
        assert!(visual_of("minecraft:oak_fence") == Visual::Outline);
        assert!(visual_of("minecraft:oak_fence_gate") == Visual::Collision);
        assert!(visual_of("minecraft:soul_sand") == Visual::Full);
        assert!(visual_of("minecraft:stone") == Visual::Collision);
    }

    #[test]
    fn fov_sprint() {
        let mut c = Camera::default();
        let t = TickIn {
            eye_height: 1.62,
            flying: false,
            movement_speed: 0.13,
            on_ground: true,
            swimming: false,
            dead: false,
            riding: false,
            moved: 0.0,
            speed: 0.0,
            yaw: 0.0,
            pitch: 0.0,
        };
        for _ in 0..40 {
            c.tick(&t);
        }
        // Sprint: (0.13 / 0.1 + 1) / 2 = 1.15.
        assert!((c.render_view(1.0, false, None).fov - 70.0 * 1.15).abs() < 0.01);
        // Bow drawn 20 ticks: × 0.85; spyglass in first person: 0.1.
        c.use_fov = UseFov::Bow(20);
        for _ in 0..40 {
            c.tick(&t);
        }
        assert!((c.render_view(1.0, false, None).fov - 70.0 * 1.15 * 0.85).abs() < 0.01);
        c.use_fov = UseFov::Spyglass;
        c.view = Perspective::First;
        for _ in 0..40 {
            c.tick(&t);
        }
        assert!((c.render_view(1.0, false, None).fov - 7.0).abs() < 0.01);
    }

    /// Screen up and screen right expressed in the world (rows 1 and 0 of the view).
    fn screen_axes(yaw: f32, pitch: f32) -> (Vec3, Vec3) {
        let v = view_matrix([0.5, 64.0, 0.5], yaw, pitch).2;
        (v.row(1).truncate(), v.row(0).truncate())
    }

    /// Looking straight up or down does not flip the image: at the ±90 stops, the screen
    /// axes continuously extend those of 89.9, whatever the yaw, without NaN.
    #[test]
    fn no_flip_at_vertical_limits() {
        for yaw in [0.0f32, 37.0, 90.0, 180.0, -135.0, 1234.5] {
            for (near, edge) in [(89.9f32, 90.0f32), (-89.9, -90.0)] {
                let (up_a, right_a) = screen_axes(yaw, near);
                let (up_b, right_b) = screen_axes(yaw, edge);
                assert!(up_b.is_finite() && right_b.is_finite(), "NaN at {yaw}/{edge}");
                assert!(up_a.dot(up_b) > 0.999, "up flipped at {yaw}/{edge}: {up_a} → {up_b}");
                assert!(right_a.dot(right_b) > 0.999, "right flipped at {yaw}/{edge}: {right_a} → {right_b}");
                // The look direction stays that of `forward` (third row = −look direction).
                let f = view_matrix([0.5, 64.0, 0.5], yaw, edge).2.row(2).truncate();
                assert!((-f).dot(forward(yaw, edge)) > 0.999);
            }
            // The screen right depends only on the yaw (no roll).
            let (_, r) = screen_axes(yaw, 90.0);
            let yr = yaw.to_radians();
            assert!(r.dot(Vec3::new(-yr.cos(), 0.0, -yr.sin())) > 0.999, "right at {yaw}: {r}");
        }
        // Local frame kept: the eye projects to the view origin.
        let (_, local, v) = view_matrix([10.25, 70.0, -3.75], 20.0, 90.0);
        assert!(v.transform_point3(local).length() < 1e-4);
    }
}
