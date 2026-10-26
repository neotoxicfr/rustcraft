//! First-person hand (`docs/specs/hand-camera.md` § 2): hand height per tick
//! (item swap, attack strength) and transform stack of each hand at render time.

use std::f32::consts::PI;

use glam::{Mat4, Vec3};
use rc_proto_v777::item::ItemStack;

use crate::entities::Entity;

/// State of the two hands (0 main, 1 off).
pub struct HandState {
    /// Item displayed.
    shown: [Option<ItemStack>; 2],
    /// Height (0 lowered, 1 raised): old, current.
    h_o: [f32; 2],
    h: [f32; 2],
    /// Ticks since the last main item swap or the last attack.
    swap_ticks: i32,
    last_main: i32,
    /// Component ignored in the comparison: `damage` (the only one that does not animate the swap).
    ignore: Option<u16>,
    /// Network id of `minecraft:crossbow`.
    crossbow: i32,
    /// Network id of `minecraft:bow`.
    bow: i32,
    /// Busy hands (piloted boat, movement key held): the hands go down.
    pub busy: bool,
}

/// What the hand draws: arm (right?) or item, with its matrix (view frame).
pub enum Draw<'a> {
    Arm {
        right: bool,
        m: Mat4,
    },
    Item {
        stack: &'a ItemStack,
        right: bool,
        m: Mat4,
    },
    /// Map (§ 2.9): `m` = 0..128 frame of the map (after C).
    Map {
        stack: &'a ItemStack,
        m: Mat4,
    },
}

/// Local player at hand render time.
pub struct HandCtx<'a> {
    pub me: &'a Entity,
    pub pt: f32,
    /// Swap scale (`swap_animation_scale`) of the displayed item of each hand.
    pub scale: [f32; 2],
    /// The hand's item is a loaded crossbow (aiming).
    pub charged_crossbow: [bool; 2],
    /// The hand's item carries a `map_id` component.
    pub is_map: [bool; 2],
    /// Hands drawn this frame (`hands_drawn`).
    pub draw: [bool; 2],
    /// Interpolated pitch of the player (degrees).
    pub pitch: f32,
    /// Use in progress: hand, animation, remaining ticks, total duration.
    pub using: Option<(usize, UseAnim, f32, i32)>,
    /// Spinning attack (riptide trident) in progress.
    pub spin: bool,
    /// Ticks since the last kinetic hit feedback (spear), fraction included; 0 without.
    pub kinetic_ticks: f32,
}

/// Durations of a kinetic weapon (spear) that set its use pose: raise delay, then maximum
/// durations of the dismount, knockback and damage conditions (0 if absent).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Kinetic {
    pub delay: i32,
    pub dismount: i32,
    pub knockback: i32,
    pub damage: i32,
}

/// Use animation of the hand that moves the item in first person.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UseAnim {
    /// No movement of its own (shield, spyglass, horn…).
    Plain,
    Eat,
    Bow,
    Trident,
    /// Brush: sweep repeated every 10 ticks.
    Brush,
    /// Bundle: arm swing as in an attack (the movement is added in `build`).
    Bundle,
    /// Block of an item that blocks attacks without being a shield.
    Block,
    /// Crossbow charging: charge duration in ticks (quick charge enchantment included).
    Crossbow(i32),
    /// Spear raised.
    Spear(Option<Kinetic>),
}

/// Movement of a hand using its item: placed after the arm offset.
fn use_transform(i: f32, hh: f32, pt: f32, anim: UseAnim, remaining: f32, duration: i32, kinetic_ticks: f32) -> Mat4 {
    match anim {
        UseAnim::Plain | UseAnim::Bundle | UseAnim::Crossbow(_) => arm_offset(i, hh),
        UseAnim::Block => {
            arm_offset(i, hh) * t(i * -0.141_421_36, 0.08, 0.141_421_36) * rx(-102.25) * ry(i * 13.365) * rz(i * 78.05)
        }
        UseAnim::Spear(k) => {
            // Own pose, without the arm height.
            let held = duration as f32 - (remaining - pt + 1.0);
            let m = t(i * 0.56, -0.52, -0.72);
            k.map_or(m, |k| m * spear_use(k, kinetic_ticks, held, i))
        }
        UseAnim::Brush => {
            let cycle = (remaining % 10.0) - pt + 1.0;
            let angle = -15.0 + 75.0 * (((1.0 - cycle / 10.0) * 2.0 * PI).cos());
            let m = arm_offset(i, hh);
            if i > 0.0 {
                m * t(-0.25, 0.22, 0.35) * rx(-80.0) * ry(90.0) * rx(angle)
            } else {
                m * t(0.1, 0.83, 0.35) * rx(-80.0) * ry(-90.0) * rx(angle) * t(-0.3, 0.22, 0.35)
            }
        }
        UseAnim::Eat => {
            let cur = remaining - pt + 1.0;
            let scaled = cur / duration.max(1) as f32;
            let mut m = Mat4::IDENTITY;
            if scaled < 0.8 {
                m *= t(0.0, ((cur / 4.0 * PI).cos() * 0.1).abs(), 0.0);
            }
            let jig = 1.0 - scaled.powf(27.0);
            m * t(jig * 0.6 * i, jig * -0.5, 0.0)
                * ry(i * jig * 90.0)
                * rx(jig * 10.0)
                * rz(i * jig * 30.0)
                * arm_offset(i, hh)
        }
        UseAnim::Bow | UseAnim::Trident => {
            let bow = anim == UseAnim::Bow;
            let mut m = arm_offset(i, hh);
            let held = duration as f32 - (remaining - pt + 1.0);
            let mut power;
            if bow {
                m *= t(i * -0.278_568_2, 0.183_443_87, 0.157_315_31) * rx(-13.935) * ry(i * 35.3) * rz(i * -9.785);
                power = held / 20.0;
                power = ((power * power + power * 2.0) / 3.0).min(1.0);
            } else {
                m *= t(i * -0.5, 0.7, 0.1) * rx(-55.0) * ry(i * 35.3) * rz(i * -9.785);
                power = (held / 10.0).min(1.0);
            }
            if power > 0.1 {
                let shake = ((held - 0.1) * 1.3).sin() * (power - 0.1);
                m *= t(0.0, shake * 0.004, 0.0);
            }
            m *= if bow { t(0.0, 0.0, power * 0.04) } else { t(0.0, 0.0, power * 0.2) };
            m * Mat4::from_scale(Vec3::new(1.0, 1.0, 1.0 + power * 0.2)) * ry(-i * 45.0)
        }
    }
}

fn same_ignoring(a: &ItemStack, b: &ItemStack, ignore: Option<u16>) -> bool {
    let keep = |t: &u16| ignore != Some(*t);
    a.item == b.item
        && a.count == b.count
        && a.added.iter().filter(|c| keep(&c.0)).eq(b.added.iter().filter(|c| keep(&c.0)))
        && a.removed.iter().filter(|t| keep(t)).eq(b.removed.iter().filter(|t| keep(t)))
}

fn t(x: f32, y: f32, z: f32) -> Mat4 {
    Mat4::from_translation(Vec3::new(x, y, z))
}
fn rx(d: f32) -> Mat4 {
    Mat4::from_rotation_x(d.to_radians())
}
fn ry(d: f32) -> Mat4 {
    Mat4::from_rotation_y(d.to_radians())
}
fn rz(d: f32) -> Mat4 {
    Mat4::from_rotation_z(d.to_radians())
}

/// Item arm A(hh).
fn arm_offset(i: f32, hh: f32) -> Mat4 {
    t(0.56 * i, -0.52 - 0.6 * hh, -0.72)
}

/// Complete empty arm (§ 2.4, steps 1 to 3); step 4 (part of the model) is `Draw::Arm`.
fn full_arm(i: f32, hh: f32, a: f32) -> Mat4 {
    let s = a.sqrt();
    t(i * (-0.3 * (s * PI).sin() + 0.64), 0.4 * (2.0 * PI * s).sin() - 0.6 - 0.6 * hh, -0.4 * (a * PI).sin() - 0.72)
        * ry(45.0 * i)
        * ry(70.0 * i * (s * PI).sin())
        * rz(-20.0 * i * (a * a * PI).sin())
        * t(-i, 3.6, 3.5)
        * rz(120.0 * i)
        * rx(200.0)
        * ry(-135.0 * i)
        * t(5.6 * i, 0.0, 0.0)
}

/// C (§ 2.9): from the hand frame to the map's 0..128 frame.
fn map_frame() -> Mat4 {
    ry(180.0)
        * rz(180.0)
        * Mat4::from_scale(Vec3::splat(0.38))
        * t(-0.5, -0.5, 0.0)
        * Mat4::from_scale(Vec3::splat(1.0 / 128.0))
}

/// "Whack" swing W(a).
fn whack(i: f32, a: f32) -> Mat4 {
    let s = a.sqrt();
    t(-0.4 * i * (s * PI).sin(), 0.2 * (2.0 * PI * s).sin(), -0.2 * (a * PI).sin())
        * ry(i * (45.0 - 20.0 * (a * a * PI).sin()))
        * rz(-20.0 * i * (s * PI).sin())
        * rx(-80.0 * (s * PI).sin())
        * ry(-45.0 * i)
}

/// "Stab" attack (spear, § 2.8).
fn stab(i: f32, a: f32) -> Mat4 {
    let prog = |v: f32, lo: f32, hi: f32| ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
    let in_out_sine = |x: f32| (1.0 - (PI * x).cos()) / 2.0;
    let out_back = |x: f32| 1.0 + 2.70158 * (x - 1.0).powi(3) + 1.70158 * (x - 1.0).powi(2);
    let in_out_expo = |x: f32| {
        if x <= 0.0 {
            0.0
        } else if x >= 1.0 {
            1.0
        } else if x < 0.5 {
            2f32.powf(20.0 * x - 10.0) / 2.0
        } else {
            (2.0 - 2f32.powf(-20.0 * x + 10.0)) / 2.0
        }
    };
    let de = in_out_sine(prog(a, 0.0, 0.05));
    let mi = out_back(prog(a, 0.05, 0.2));
    let fi = in_out_expo(prog(a, 0.4, 1.0));
    t(0.1 * i * (de - mi), -0.075 * (de - fi), 0.65 * (de - mi))
        * rx(-70.0 * (de - fi))
        * t(0.0, 0.0, -0.25 * (fi - mi))
}

/// Crossbow charging: hand stretched towards the string, shaking then stretching according to the
/// charged fraction (`charge` = charge duration in ticks).
fn crossbow_charge(i: f32, pt: f32, remaining: f32, duration: i32, charge: i32) -> Mat4 {
    let mut m = t(i * -0.478_568_2, -0.094_387, 0.057_315_31) * rx(-11.935) * ry(i * 65.3) * rz(i * -9.785);
    let held = duration as f32 - (remaining - pt + 1.0);
    let mut power = held / charge as f32;
    if power > 1.0 {
        power = 1.0;
    }
    if power > 0.1 {
        let shake = crate::sky_mesh::table_sin((held - 0.1) * 1.3) * (power - 0.1);
        m *= t(0.0, shake * 0.004, 0.0);
    }
    m * t(0.0, 0.0, power * 0.04) * Mat4::from_scale(Vec3::new(1.0, 1.0, 1.0 + power * 0.2)) * ry(-i * 45.0)
}

/// Fraction of `x` between `lo` and `hi`, clamped to [0, 1].
pub(crate) fn progress(x: f32, lo: f32, hi: f32) -> f32 {
    ((x - lo) / (hi - lo)).clamp(0.0, 1.0)
}

/// Rotation of `m` around the point (x, y, z).
pub(crate) fn around(m: Mat4, x: f32, y: f32, z: f32) -> Mat4 {
    t(x, y, z) * m * t(-x, -y, -z)
}

/// The game's easing curves used by the spear.
pub(crate) mod ease {
    use super::PI;
    use crate::sky_mesh::table_cos;

    pub fn in_out_sine(x: f32) -> f32 {
        -(table_cos(PI * x) - 1.0) / 2.0
    }
    pub fn out_quart(x: f32) -> f32 {
        let s = (1.0 - x) * (1.0 - x);
        1.0 - s * s
    }
    pub fn out_cubic(x: f32) -> f32 {
        1.0 - (1.0 - x).powi(3)
    }
    pub fn out_circ(x: f32) -> f32 {
        (1.0 - (x - 1.0) * (x - 1.0)).sqrt()
    }
    pub fn in_circ(x: f32) -> f32 {
        -(f64::from(1.0 - x * x).sqrt() as f32) + 1.0
    }
    pub fn in_out_elastic(x: f32) -> f32 {
        if x == 0.0 {
            return 0.0;
        }
        if x == 1.0 {
            return 1.0;
        }
        let c5 = f64::from(PI * 4.0 / 9.0);
        let x = f64::from(x);
        let sin = ((20.0 * x - 11.125) * c5).sin();
        if x < 0.5 {
            (-(2f64.powf(20.0 * x - 10.0) * sin) / 2.0) as f32
        } else {
            (2f64.powf(-20.0 * x + 10.0) * sin / 2.0 + 1.0) as f32
        }
    }
    pub fn in_out_back(x: f32) -> f32 {
        if x < 0.5 {
            return 4.0 * x * x * (7.189_819 * x - 2.594_909_4) / 2.0;
        }
        let dt = 2.0 * x - 2.0;
        (dt * dt * (3.594_909_4 * dt + 2.594_909_4) + 2.0) / 2.0
    }
}

/// Item recoil on a kinetic hit feedback (rises in 2 ticks, falls back until the 10th).
pub(crate) fn hit_feedback(ticks: f32) -> f32 {
    0.4 * (ease::out_quart(progress(ticks, 1.0, 3.0)) - ease::in_out_sine(progress(ticks, 3.0, 10.0)))
}

/// Pose of a spear held for `time` ticks: raise, swing, lowering then raise again according to
/// the weapon's durations.
fn spear_use(k: Kinetic, kinetic_ticks: f32, time: f32, i: f32) -> Mat4 {
    let raised = k.delay;
    let sway_end = k.dismount + raised;
    let sway_start = sway_end - 20;
    let lower_end = k.knockback + raised;
    let lower_start = lower_end - 40;
    let back_end = k.damage + raised;
    let raise = progress(time, 0.0, raised as f32);
    let raise_start = progress(raise, 0.0, 0.5);
    let raise_mid = progress(raise, 0.5, 0.8);
    let raise_end = progress(raise, 0.8, 1.0);
    let sway = progress(time, sway_start as f32, sway_end as f32);
    let lower = ease::out_cubic(ease::in_out_elastic(progress(time - 20.0, lower_start as f32, lower_end as f32)));
    let back = progress(time, (back_end - 5) as f32, back_end as f32);
    let intensity = 2.0 * ease::out_circ(sway) - 2.0 * ease::in_circ(back);
    let slow = crate::sky_mesh::table_sin(time * 19.0 * (PI / 180.0)) * intensity;
    let fast = crate::sky_mesh::table_sin(time * 30.0 * (PI / 180.0)) * intensity;
    t(
        i * (raise * 0.15 + raise_end * -0.05 + sway * -0.1 + slow * 0.005),
        raise * -0.075 + raise_mid * 0.075 + fast * 0.01,
        (f64::from(raise_start) * 0.05 + f64::from(raise_end) * -0.05 + f64::from(slow * 0.005)) as f32,
    ) * around(rx(-65.0 * ease::in_out_back(raise) - 35.0 * lower + 100.0 * back + -0.5 * fast), 0.0, 0.1, 0.0)
        * around(ry(-(i * (-90.0 * progress(raise, 0.5, 0.55) + 90.0 * sway + 2.0 * slow))), i * 0.15, 0.0, 0.0)
        * t(0.0, -hit_feedback(kinetic_ticks), 0.0)
}

/// Durations of the game's spears (delay and conditions in ticks), for the tests.
#[cfg(test)]
fn spear_defaults(name: &str) -> Option<Kinetic> {
    let (delay, dismount, knockback, damage) = match name {
        "minecraft:wooden_spear" => (15, 100, 200, 300),
        "minecraft:stone_spear" => (14, 90, 180, 275),
        "minecraft:copper_spear" => (13, 80, 165, 250),
        "minecraft:iron_spear" => (12, 50, 135, 225),
        "minecraft:golden_spear" => (14, 70, 170, 275),
        "minecraft:diamond_spear" => (10, 60, 130, 200),
        "minecraft:netherite_spear" => (8, 50, 110, 175),
        _ => return None,
    };
    Some(Kinetic { delay, dismount, knockback, damage })
}

/// `kinetic_weapon` component of the network patch: delay and durations of the three conditions.
fn decode_kinetic(b: &[u8]) -> Option<Kinetic> {
    use rc_proto::{Reader, varint::read_varint};
    let r = &mut Reader::new(b);
    read_varint(r).ok()?;
    let delay = read_varint(r).ok()?;
    let mut cond = || -> Option<i32> {
        if r.u8().ok()? == 0 {
            return Some(0);
        }
        let ticks = read_varint(r).ok()?;
        r.take(8).ok()?;
        Some(ticks)
    };
    let (dismount, knockback, damage) = (cond()?, cond()?, cond()?);
    Some(Kinetic { delay, dismount, knockback, damage })
}

/// Level of an enchantment (by registry name) in the stack's `enchantments` component.
fn enchantment_level(view: &crate::stack::StackRef<'_>, names: &[String], wanted: &str) -> i32 {
    use rc_proto::{Reader, varint::read_varint};
    let Some(b) = view.patch("minecraft:enchantments") else { return 0 };
    let r = &mut Reader::new(b);
    let n = read_varint(r).unwrap_or(0).max(0);
    for _ in 0..n {
        let (Ok(id), Ok(level)) = (read_varint(r), read_varint(r)) else { break };
        if usize::try_from(id).ok().and_then(|i| names.get(i)).is_some_and(|s| s == wanted) {
            return level;
        }
    }
    0
}

impl HandState {
    pub fn new(data: &rc_data::VersionData) -> Self {
        let id = |name: &str| {
            data.registry("minecraft:data_component_type")
                .and_then(|r| r.entries.iter().position(|e| e == name))
                .and_then(|i| u16::try_from(i).ok())
        };
        let item = |name: &str| {
            data.registry("minecraft:item")
                .and_then(|r| r.entries.iter().position(|e| e == name))
                .map_or(i32::MIN, |i| i as i32)
        };
        HandState {
            shown: [None, None],
            h_o: [0.0; 2],
            h: [0.0; 2],
            swap_ticks: 0,
            last_main: i32::MIN,
            ignore: id("minecraft:damage"),
            crossbow: item("minecraft:crossbow"),
            bow: item("minecraft:bow"),
            busy: false,
        }
    }

    /// Attack or click into the void: the main hand lowers then rises.
    pub fn reset_swap(&mut self) {
        self.swap_ticks = 0;
    }

    /// Use accepted on the client side: the hand concerned restarts from the low position
    /// ("rebound", § 2.1).
    pub fn bounce(&mut self, hand: usize) {
        if let Some(h) = self.h.get_mut(hand) {
            *h = 0.0;
        }
    }

    /// One tick (§ 2.1): `expected` = contents of the hands, `animates` = the item plays the swap
    /// animation (`hand_animation_on_swap`).
    pub fn tick(
        &mut self,
        expected: [Option<&ItemStack>; 2],
        attack_speed: f64,
        animates: &dyn Fn(&ItemStack) -> bool,
    ) {
        self.h_o = self.h;
        self.swap_ticks += 1;
        let main = expected[0].map_or(-1, |s| s.item);
        if main != self.last_main {
            self.swap_ticks = 0;
            self.last_main = main;
        }
        #[allow(clippy::needless_range_loop)]
        for k in 0..2 {
            let replace = match (&self.shown[k], expected[k]) {
                (Some(a), Some(b)) => same_ignoring(a, b, self.ignore) || !animates(b),
                (None, Some(b)) => !animates(b),
                (_, None) => false,
            };
            if replace {
                self.shown[k] = expected[k].cloned();
            }
        }
        let delay = if attack_speed > 0.0 { (20.0 / attack_speed) as f32 } else { 5.0 };
        let e = ((self.swap_ticks as f32 + 1.0) / delay).clamp(0.0, 1.0);
        #[allow(clippy::needless_range_loop)]
        for k in 0..2 {
            let same = self.shown[k].as_ref() == expected[k];
            if self.busy {
                self.h[k] = (self.h[k] - 0.4).clamp(0.0, 1.0);
            } else {
                let target = match (same, k) {
                    (false, _) => 0.0,
                    (true, 0) => e * e * e,
                    (true, _) => 1.0,
                };
                self.h[k] += (target - self.h[k]).clamp(-0.4, 0.4);
            }
            if self.h[k] < 0.1 {
                self.shown[k] = expected[k].cloned();
            }
        }
    }

    pub fn shown(&self, hand: usize) -> Option<&ItemStack> {
        self.shown[hand].as_ref()
    }

    /// Hands drawn (main, off) according to the REAL contents of the hands (`held`:
    /// item and loaded crossbow), not the displayed item, and the use in progress (item, hand). Without
    /// bow or crossbow held: both. Otherwise, without use: the main alone if it holds a
    /// loaded crossbow; bow or crossbow shot: the shooting hand alone; other use of the
    /// main hand with a loaded crossbow in the off hand: the main alone.
    pub fn hands_drawn(&self, held: [Option<(i32, bool)>; 2], using: Option<(i32, usize)>) -> [bool; 2] {
        let holds = |id: i32| held.iter().flatten().any(|h| h.0 == id);
        let charged = |k: usize| held[k].is_some_and(|h| h.0 == self.crossbow && h.1);
        if !holds(self.bow) && !holds(self.crossbow) {
            return [true; 2];
        }
        match using {
            None => [true, !charged(0)],
            Some((item, hand)) if item != self.bow && item != self.crossbow => [true, !(hand == 0 && charged(1))],
            Some((_, hand)) => [hand == 0, hand == 1],
        }
    }

    /// What the hand draws this frame (§ 2.3 to 2.7); `base` = view frame after shake,
    /// bobbing and tilt.
    pub fn build<'a>(&'a self, base: Mat4, c: &HandCtx<'_>) -> Vec<Draw<'a>> {
        let me = c.me;
        let right_handed = me.meta_int(15).is_none_or(|v| v != 0);
        let invisible = me.flags() & 0x20 != 0;
        let swing_hand = me.swing.map_or(0, |s| s.0.clamp(0, 1) as usize);
        let swing_kind = me.swing.map_or(1, |s| s.1);
        let mut out = Vec::new();
        let hh_of = |k: usize| c.scale[k] * (1.0 - (self.h_o[k] + c.pt * (self.h[k] - self.h_o[k])));
        let swing_of = |k: usize| if swing_hand == k { me.swing_at(c.pt) } else { 0.0 };
        // Two-handed map: map in the main hand, off hand empty (§ 2.9).
        if let (Some(stack), None, true, true) = (self.shown[0].as_ref(), &self.shown[1], c.is_map[0], c.draw[0]) {
            let (hh, a) = (hh_of(0), swing_of(0));
            let s = a.sqrt();
            let k = (1.0 - c.pitch / 45.0 + 0.1).clamp(0.0, 1.0);
            let tilt = 0.5 - 0.5 * (k * PI).cos();
            let m = base
                * t(0.0, 0.1 * (a * PI).sin(), -0.4 * (s * PI).sin())
                * t(0.0, 0.04 - 1.2 * hh - 0.5 * tilt, -0.72)
                * rx(-85.0 * tilt);
            if !invisible {
                for (right, i) in [(true, 1.0), (false, -1.0)] {
                    let arm = m * ry(90.0) * ry(92.0) * rx(45.0) * rz(-41.0 * i) * t(0.3 * i, -1.1, 0.45);
                    out.push(Draw::Arm { right, m: arm });
                }
            }
            let m = m * rx(20.0 * (s * PI).sin()) * Mat4::from_scale(Vec3::splat(2.0)) * map_frame();
            out.push(Draw::Map { stack, m });
            return out;
        }
        for k in 0..2 {
            if !c.draw[k] {
                continue;
            }
            let right = (k == 0) == right_handed;
            let i = if right { 1.0 } else { -1.0 };
            let hh = hh_of(k);
            let a = swing_of(k);
            let Some(stack) = self.shown[k].as_ref() else {
                // Empty arm: main hand only, player visible (§ 2.4).
                if k == 0 && !invisible {
                    out.push(Draw::Arm { right, m: base * full_arm(i, hh, a) });
                }
                continue;
            };
            if c.is_map[k] {
                // One-handed map (§ 2.9): complete arm tilted, map in front.
                let m = base * t(0.125 * i, -0.125, 0.0);
                if !invisible {
                    out.push(Draw::Arm { right, m: m * rz(10.0 * i) * full_arm(i, hh, a) });
                }
                let s = a.sqrt();
                let x = (s * PI).sin();
                let m = m
                    * t(0.51 * i, -0.08 - 1.2 * hh, -0.75)
                    * t(-0.5 * x * i, 0.4 * (2.0 * PI * s).sin() - 0.3 * x, -0.3 * (a * PI).sin())
                    * rx(-45.0 * x)
                    * ry(-30.0 * i * x)
                    * map_frame();
                out.push(Draw::Map { stack, m });
                continue;
            }
            // Use of this hand with ticks remaining (otherwise ordinary pose).
            let using = c.using.filter(|u| u.0 == k && u.2 > 0.0);
            if stack.item == self.crossbow {
                // Crossbow (§ 2.7): charging (not loaded), hand on the string; otherwise "whack"
                // swing whatever the kind, and aiming if loaded.
                let mut m = base * arm_offset(i, hh);
                match using {
                    Some((_, UseAnim::Crossbow(charge), rem, dur)) if !c.charged_crossbow[k] => {
                        m *= crossbow_charge(i, c.pt, rem, dur, charge);
                    }
                    _ => {
                        m *= whack(i, a);
                        if c.charged_crossbow[k] && a < 0.001 && k == 0 {
                            m *= t(-0.641864 * i, 0.0, 0.0) * ry(10.0 * i);
                        }
                    }
                }
                out.push(Draw::Item { stack, right, m });
                continue;
            }
            let mut m = match using {
                Some((_, anim, rem, dur)) => base * use_transform(i, hh, c.pt, anim, rem, dur, c.kinetic_ticks),
                // Spinning attack of the trident: item held to the side, no swing.
                None if c.spin => base * arm_offset(i, hh) * t(i * -0.4, 0.8, 0.3) * ry(i * 65.0) * rz(i * -85.0),
                None => base * arm_offset(i, hh),
            };
            if let Some((_, anim, _, _)) = using {
                // A use in progress replaces the swing (the bundle keeps its arm swing).
                if anim == UseAnim::Bundle {
                    m *= whack(i, a);
                }
            } else if a > 0.0 && !c.spin {
                match swing_kind {
                    1 => m *= whack(i, a),
                    2 => m *= stab(i, a),
                    _ => {}
                }
            }
            out.push(Draw::Item { stack, right, m });
        }
        out
    }
}

/// Swap animation of a stack: (replayed, scale), defaults without a known `item_model`.
pub fn swap_props(
    models: Option<&rc_assets::item::ItemModels>,
    ids: &crate::stack::ComponentIds,
    data: &rc_data::VersionData,
    stack: &ItemStack,
) -> (bool, f32) {
    let info = usize::try_from(stack.item).ok().and_then(|i| data.items.get(i));
    let view = crate::stack::StackRef { stack, info, ids, data, hold: crate::stack::Hold::MAIN };
    match (models, view.model()) {
        (Some(m), Some(id)) => m.swap_animation(&id),
        _ => (true, 1.0),
    }
}

/// Component of the stack: `Some(Some(bytes))` if it is in the patch, `Some(None)` if it comes
/// from the item's defaults, `None` if it is absent or removed.
fn component<'s>(
    ids: &crate::stack::ComponentIds,
    info: Option<&rc_data::ItemInfo>,
    st: &'s ItemStack,
    name: &str,
) -> Option<Option<&'s [u8]>> {
    let c = ids.of(name)?;
    if let Some(b) = st.component(c) {
        return Some(Some(b));
    }
    (info.is_some_and(|i| i.components.contains(&c)) && !st.removed.contains(&c)).then_some(None)
}

/// Use animation of a stack, like the game: that of the item (bow, crossbow, trident,
/// brush, bundle), otherwise that of the `consumable` component, otherwise block (`blocks_attacks`), otherwise
/// spear (`kinetic_weapon`), otherwise none.
fn use_anim(view: &crate::stack::StackRef<'_>, ids: &crate::stack::ComponentIds, ench: &[String]) -> UseAnim {
    let (st, info) = (view.stack, view.info);
    match view.item_name() {
        "minecraft:bow" => return UseAnim::Bow,
        "minecraft:trident" => return UseAnim::Trident,
        "minecraft:brush" => return UseAnim::Brush,
        "minecraft:crossbow" => {
            // 1.25 s minus 0.25 s per quick charge level, at least 0, in ticks.
            let level = enchantment_level(view, ench, "minecraft:quick_charge");
            let seconds = (1.25 - 0.25 * level as f32).max(0.0);
            return UseAnim::Crossbow((seconds * 20.0).floor() as i32);
        }
        // Spyglass and goat horn: no movement of their own in first person.
        "minecraft:spyglass" | "minecraft:goat_horn" => return UseAnim::Plain,
        n if n.ends_with("bundle") => return UseAnim::Bundle,
        _ => {}
    }
    if let Some(c) = component(ids, info, st, "minecraft:consumable") {
        let anim = match c {
            // Duration (float) then animation.
            Some(b) => {
                let r = &mut rc_proto::Reader::new(b);
                r.take(4).ok().and_then(|_| rc_proto::varint::read_varint(r).ok()).unwrap_or(1)
            }
            None => info.and_then(|i| i.consumable.as_ref()).map_or(1, |c| i32::from(c.animation)),
        };
        return match anim {
            1 | 2 => UseAnim::Eat,
            3 if view.item_name() != "minecraft:shield" => UseAnim::Block,
            4 => UseAnim::Bow,
            5 => UseAnim::Trident,
            9 => UseAnim::Brush,
            10 => UseAnim::Bundle,
            11 => UseAnim::Spear(kinetic_of(view, ids)),
            _ => UseAnim::Plain,
        };
    }
    if component(ids, info, st, "minecraft:blocks_attacks").is_some() {
        // The shield keeps its own pose (item model "blocking").
        return if view.item_name() == "minecraft:shield" { UseAnim::Plain } else { UseAnim::Block };
    }
    match kinetic_of(view, ids) {
        Some(k) => UseAnim::Spear(Some(k)),
        None => UseAnim::Plain,
    }
}

/// Durations of the stack's kinetic weapon: network patch, otherwise the item's default component
/// (extracted data).
pub(crate) fn kinetic_of(view: &crate::stack::StackRef<'_>, ids: &crate::stack::ComponentIds) -> Option<Kinetic> {
    match component(ids, view.info, view.stack, "minecraft:kinetic_weapon")? {
        Some(b) => decode_kinetic(b),
        None => view.info.and_then(|i| i.kinetic_params).map(Kinetic::from_params),
    }
}

impl Kinetic {
    pub(crate) fn from_params(p: rc_data::KineticParams) -> Self {
        let t = |i: usize| p.conditions[i].map_or(0, |c| c.0);
        Kinetic { delay: p.delay, dismount: t(0), knockback: t(1), damage: t(2) }
    }
}

/// State of the local player read by the hand besides the entity: names of the enchantments
/// registry (quick charge), spinning attack, ticks since the last kinetic hit.
pub struct HandExtra<'a> {
    pub enchantments: &'a [String],
    pub spin: bool,
    pub kinetic_ticks: f32,
    /// Environment of the local player's items (death location, position, clock, keys).
    pub env: &'a crate::stack::HudEnv,
}

/// Hand instances of the frame in the entity engine (after those of the world).
#[allow(clippy::too_many_arguments)]
pub fn push(
    er: &mut crate::entity_render::EntityRenderer,
    models: &rc_assets::item::ItemModels,
    hand: &HandState,
    me: &Entity,
    pt: f32,
    base: Mat4,
    light: u32,
    ids: &crate::stack::ComponentIds,
    data: &rc_data::VersionData,
    extra: &HandExtra<'_>,
) {
    let scale = [0, 1].map(|k| hand.shown(k).map_or(1.0, |s| swap_props(Some(models), ids, data, s).1));
    let is_charged = |s: &ItemStack| {
        use rc_assets::item::StackView;
        let info = usize::try_from(s.item).ok().and_then(|i| data.items.get(i));
        crate::stack::StackRef { stack: s, info, ids, data, hold: crate::stack::Hold::NONE }.charge_type() != "none"
    };
    let charged = [0, 1].map(|k| hand.shown(k).is_some_and(is_charged));
    // Real contents of the hands (copied from the inventory on every tick), not the displayed item.
    let held = [0, 1].map(|k| me.equipment[k].as_ref().map(|s| (s.item, s.item == hand.crossbow && is_charged(s))));
    let draw = hand.hands_drawn(held, crate::stack::using_state().map(|u| (u.0, u.3)));
    let is_map = [0, 1].map(|k| hand.shown(k).is_some_and(|s| crate::maps::map_id(ids, s).is_some()));
    let pitch = me.pitch_o + pt * (me.pitch - me.pitch_o);
    // Use in progress: use animation of the item (`use_anim`).
    let using = crate::stack::using_state().and_then(|(item, used, left, k)| {
        let st = hand.shown(k).filter(|s| s.item == item)?;
        let info = usize::try_from(st.item).ok().and_then(|i| data.items.get(i));
        let view = crate::stack::StackRef { stack: st, info, ids, data, hold: crate::stack::Hold::local_hand(k == 0) };
        Some((k, use_anim(&view, ids, extra.enchantments), left as f32, used + left))
    });
    let ctx = HandCtx {
        me,
        pt,
        scale,
        charged_crossbow: charged,
        is_map,
        draw,
        pitch,
        using,
        spin: extra.spin,
        kinetic_ticks: extra.kinetic_ticks,
    };
    for d in hand.build(base, &ctx) {
        match d {
            Draw::Arm { right, m } => er.push_hand_arm(me, right, m, light),
            Draw::Item { stack, right, m } => {
                // Main hand on the side of the main arm.
                let main = right == me.meta_int(15).is_none_or(|v| v != 0);
                er.push_hand_item(models, stack, right, crate::stack::Hold::local_hand(main), extra.env, m, light)
            }
            Draw::Map { stack, m } => er.push_hand_map(stack, m, light),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stack(item: i32) -> ItemStack {
        ItemStack { item, count: 1, added: Vec::new(), removed: Vec::new() }
    }

    fn state() -> HandState {
        HandState {
            shown: [None, None],
            h_o: [0.0; 2],
            h: [0.0; 2],
            swap_ticks: 0,
            last_main: i32::MIN,
            ignore: Some(1),
            crossbow: -1,
            bow: -2,
            busy: false,
        }
    }

    /// Selection of the hands with bow or crossbow, according to the official game (bow = -2, crossbow =
    /// -1, torch = 7).
    #[test]
    fn bow_like_hand_selection() {
        let h = state();
        let (bow, xbow, xbow_loaded, torch) = (Some((-2, false)), Some((-1, false)), Some((-1, true)), Some((7, false)));
        // Neither bow nor crossbow: both hands, even during a use.
        assert_eq!(h.hands_drawn([torch, torch], Some((7, 1))), [true, true]);
        // Bow held without use: both.
        assert_eq!(h.hands_drawn([bow, torch], None), [true, true]);
        // Bow drawn in the main hand: the main alone; in the off hand: the off hand alone.
        assert_eq!(h.hands_drawn([bow, torch], Some((-2, 0))), [true, false]);
        assert_eq!(h.hands_drawn([torch, bow], Some((-2, 1))), [false, true]);
        // Crossbow loaded in the main hand, without use: the main alone; not loaded: both.
        assert_eq!(h.hands_drawn([xbow_loaded, torch], None), [true, false]);
        assert_eq!(h.hands_drawn([xbow, torch], None), [true, true]);
        // Crossbow loaded in the off hand, without use: both.
        assert_eq!(h.hands_drawn([torch, xbow_loaded], None), [true, true]);
        // Other use (eating) in the main hand with a loaded crossbow in the off hand: main alone;
        // use of the off hand: both.
        assert_eq!(h.hands_drawn([torch, xbow_loaded], Some((7, 0))), [true, false]);
        assert_eq!(h.hands_drawn([xbow_loaded, torch], Some((7, 1))), [true, true]);
        // Crossbow charging in the off hand: the off hand alone.
        assert_eq!(h.hands_drawn([torch, xbow], Some((-1, 1))), [false, true]);
    }

    /// The renderer respects the selection: bow drawn in the main hand, torch in the off hand →
    /// no piece of the off hand.
    #[test]
    fn build_skips_hidden_hand() {
        let mut h = state();
        h.shown = [Some(stack(-2)), Some(stack(7))];
        let me = crate::debug_render::tests::test_entity("player");
        let mut c = HandCtx {
            me: &me,
            pt: 0.0,
            scale: [1.0; 2],
            charged_crossbow: [false; 2],
            is_map: [false; 2],
            draw: [true, false],
            pitch: 0.0,
            using: None,
            spin: false,
            kinetic_ticks: 0.0,
        };
        let main_only = h.build(Mat4::IDENTITY, &c);
        assert_eq!(main_only.len(), 1);
        assert!(matches!(main_only[0], Draw::Item { stack, .. } if stack.item == -2));
        c.draw = [true, true];
        assert_eq!(h.build(Mat4::IDENTITY, &c).len(), 2);
    }

    /// Use without movement of its own (shield): simple arm offset; bow: arm offset then
    /// item tilted towards the centre.
    #[test]
    fn use_transforms() {
        assert_eq!(use_transform(1.0, 0.0, 0.0, UseAnim::Plain, 100.0, 1000, 0.0), arm_offset(1.0, 0.0));
        let bow = use_transform(1.0, 0.0, 0.0, UseAnim::Bow, 100.0, 1000, 0.0);
        assert_ne!(bow, arm_offset(1.0, 0.0));
        // The two hands are symmetrical in x for the bow translation.
        let left = use_transform(-1.0, 0.0, 0.0, UseAnim::Bow, 100.0, 1000, 0.0);
        assert!((bow.w_axis.x + left.w_axis.x).abs() < 1e-5);
    }

    #[test]
    fn raise_after_swap() {
        let mut h = state();
        let sword = stack(5);
        // Sword (speed 1.6: delay 12.5 ticks): hand fully raised after the delay.
        for _ in 0..20 {
            h.tick([Some(&sword), None], 1.6, &|_| true);
        }
        assert_eq!(h.shown(0), Some(&sword));
        assert!((h.h[0] - 1.0).abs() < 1e-6);
        // Item change: the hand lowers first.
        let pick = stack(6);
        h.tick([Some(&pick), None], 1.6, &|_| true);
        assert!(h.h[0] < 0.7);
        assert_eq!(h.shown(0), Some(&sword));
    }

    /// Pose of the iron spear compared with the official client's matrices (same inputs:
    /// ticks since the kinetic hit, ticks held, right then left arm).
    #[test]
    fn spear_use_matches_official() {
        let iron = spear_defaults("minecraft:iron_spear").unwrap();
        #[rustfmt::skip]
        let cases: [(f32, f32, f32, [f32; 12]); 6] = [
            (0.0, 3.5, 1.0, [1.0, 0.0, 0.0, 0.0, 0.995386, 0.095951, 0.0, -0.095951, 0.995386, 0.04375, -0.021414, 0.019572]),
            (0.0, 3.5, -1.0, [1.0, 0.0, 0.0, 0.0, 0.995386, 0.095951, 0.0, -0.095951, 0.995386, -0.04375, -0.021414, 0.019572]),
            (0.0, 12.0, 1.0, [0.0, -0.906308, -0.422618, 0.0, 0.422618, -0.906308, 1.0, 0.0, 0.0, 0.25, 0.193684, 0.154024]),
            (2.0, 40.0, -1.0, [0.0, 0.906308, 0.422618, 0.0, 0.422618, -0.906308, -1.0, 0.0, 0.0, -0.25, 0.035203, 0.493889]),
            (0.0, 140.0, 1.0, [0.998993, 0.040636, 0.019006, 0.0, 0.423668, -0.905818, -0.044861, 0.904906, 0.423241, 0.006579, 0.034218, 0.094159]),
            (5.0, 230.0, -1.0, [0.99857, -0.052495, 0.010077, 0.0, -0.188512, -0.982071, 0.053454, 0.980667, -0.188243, -0.007875, 0.189507, 0.426259]),
        ];
        for (hit, time, i, want) in cases {
            let m = spear_use(iron, hit, time, i);
            let got = [m.x_axis, m.y_axis, m.z_axis, m.w_axis].map(|c| [c.x, c.y, c.z]).concat();
            for (g, w) in got.iter().zip(want) {
                assert!((g - w).abs() < 2e-5, "time {time}, arm {i}: {got:?} against {want:?}");
            }
        }
    }

    /// Default values of the spears read from the network: the patch decodes the same way.
    #[test]
    fn kinetic_patch_decodes() {
        // 10, 12, then three conditions present (50, 135, 225) and their speeds.
        let mut b = vec![10, 12];
        for ticks in [50u8, 135, 225] {
            b.push(1);
            if ticks < 128 {
                b.push(ticks);
            } else {
                b.extend([ticks | 0x80, 1]);
            }
            b.extend([0; 8]);
        }
        b.extend([0; 8]);
        let k = decode_kinetic(&b).unwrap();
        assert_eq!(Some(k), spear_defaults("minecraft:iron_spear"));
    }

    /// Crossbow charging: pose different from aiming and from the resting hand, and complete
    /// charge (stretch of 1.2) once the charge duration has elapsed.
    #[test]
    fn crossbow_charge_pose() {
        let start = crossbow_charge(1.0, 0.0, 71_999.0, 72_000, 25);
        let full = crossbow_charge(1.0, 0.0, 72_000.0 - 40.0, 72_000, 25);
        assert_ne!(start, full);
        // Stretch in z of 1.2 at full charge (volume), none at the start.
        assert!((full.determinant() - 1.2).abs() < 1e-5);
        assert!((start.determinant() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn damage_ignored() {
        let mut a = stack(5);
        let mut b = stack(5);
        a.added.push((1, Box::new([3])));
        b.added.push((1, Box::new([4])));
        assert!(same_ignoring(&a, &b, Some(1)));
        b.added.push((7, Box::new([0])));
        assert!(!same_ignoring(&a, &b, Some(1)));
    }

    /// Only `damage` is ignored: a different `max_damage` replays the swap animation.
    #[test]
    fn max_damage_animates_swap() {
        let entries = ["minecraft:custom_data", "minecraft:max_stack_size", "minecraft:max_damage", "minecraft:damage"];
        let data = rc_data::VersionData {
            registries: vec![rc_data::Registry::new(
                "minecraft:data_component_type",
                entries.map(String::from).to_vec(),
            )],
            ..Default::default()
        };
        let h = HandState::new(&data);
        let (mut a, mut b) = (stack(5), stack(5));
        a.added.push((3, Box::new([3])));
        b.added.push((3, Box::new([4])));
        assert!(same_ignoring(&a, &b, h.ignore));
        let (mut a, mut b) = (stack(5), stack(5));
        a.added.push((2, Box::new([10])));
        b.added.push((2, Box::new([20])));
        assert!(!same_ignoring(&a, &b, h.ignore));
    }
}
