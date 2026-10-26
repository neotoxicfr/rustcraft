//! Ender Dragon (`entities-animals.md` § 15).
//!
//! Entry points (called by `entity_render.rs` before its own rules):
//! - `layers`: body (cutout) and eyes (emissive);
//! - `skin`: single key (no variant or baby);
//! - `animate`: neck, head, jaw, wings, legs and tail taken from the flight history;
//! - `rotations`: dragon-specific stack in place of "Y(180 − body)";
//! - `shadow`: shadow radius 0.5;
//! - `tick`: flight history (64 samples) and wing flap.
//!
//! Client state: `history` = ring of 64 [y, yaw, 0]; `extra[0]` = index of the last
//! sample, `extra[1]` / `extra[2]` = current / previous wing flap.

use super::*;

const KIND: &str = "ender_dragon";
const RING: usize = 64;
const DEG: f32 = PI / 180.0;

pub(super) fn layers(
    _r: &EntityRenderer,
    k: &str,
    _baby: bool,
    _skin: u32,
) -> Option<Vec<(String, String, Pass, Wool)>> {
    if k != KIND {
        return None;
    }
    let model = "minecraft:ender_dragon#main";
    Some(vec![
        (model.to_owned(), "entity/enderdragon/dragon".to_owned(), Pass::Cutout, Wool::None),
        (model.to_owned(), "entity/enderdragon/dragon_eyes".to_owned(), Pass::Eyes, Wool::None),
    ])
}

pub(super) fn skin(_r: &EntityRenderer, e: &Entity) -> Option<u32> {
    (e.kind == KIND).then_some(0)
}

/// Angle in degrees brought into [−180, 180).
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

fn wrap64(a: f64) -> f64 {
    let a = a % 360.0;
    if a >= 180.0 {
        a - 360.0
    } else if a < -180.0 {
        a + 360.0
    } else {
        a
    }
}

/// Dying: the history and the wing flap are frozen.
fn dying(e: &Entity) -> bool {
    e.death_time > 0
}

/// No AI (flag 0x01 of the mob byte, index 15).
fn no_ai(e: &Entity) -> bool {
    e.meta_byte(15) & 1 != 0
}

/// Interpolated sample k (y, yaw): recorded k ticks earlier; empty history → zeros.
fn sample(e: &Entity, k: usize, pt: f32) -> (f64, f32) {
    if e.history.len() < RING {
        return (0.0, 0.0);
    }
    let head = e.extra[0] as usize;
    let at = |k: usize| e.history[(head + RING - (k & (RING - 1))) & (RING - 1)];
    let (a, b) = (at(k + 1), at(k));
    let t = if dying(e) { 0.0 } else { pt };
    (a[0] + f64::from(t) * (b[0] - a[0]), rot_lerp(t, a[1] as f32, b[1] as f32))
}

pub(super) fn tick(e: &mut Entity) {
    if e.kind != KIND {
        return;
    }
    // Client death counter: starts as soon as health (meta 9) drops to 0 — the server's
    // death phase sets it without a death event.
    if e.death_time == 0 && matches!(e.meta.get(&9), Some(rc_proto_v777::entity::Meta::Float(h)) if *h <= 0.0) {
        e.death_time = 1;
    }
    e.extra[2] = e.extra[1];
    if dying(e) {
        return;
    }
    let [vx, vy, vz] = e.velocity;
    let f = 0.2 / ((vx * vx + vz * vz).sqrt() * 10.0 + 1.0) * 2f64.powf(vy);
    let sitting = matches!(e.meta_int(16), Some(5 | 6 | 7 | 10));
    // Slowdown inside a wall (× 0.5): the module does not see the world.
    e.extra[1] += if sitting { 0.1 } else { f as f32 };
    if no_ai(e) {
        e.extra[1] = 0.5;
        return;
    }
    let s = [e.pos[1], f64::from(wrap(e.yaw)), 0.0];
    if e.history.len() < RING {
        // First recording: the whole ring is filled.
        e.history.clear();
        e.history.resize(RING, s);
        e.extra[0] = 0.0;
    } else {
        let head = (e.extra[0] as usize + 1) & (RING - 1);
        e.history[head] = s;
        e.extra[0] = head as f32;
    }
}

/// Vertical offset of a neck or head part (in "degrees" before × 7.5).
fn head_offset(e: &Entity, i: usize, base: (f64, f32), point: (f64, f32)) -> f64 {
    match e.meta_int(16) {
        Some(3 | 4) => {
            // Distance to the egg approximated: column (0, 0), height ignored (world not accessible here).
            let (x, z) = (e.pos[0] - 0.5, e.pos[2] - 0.5);
            i as f64 / ((x * x + z * z) / 4.0).max(1.0)
        }
        Some(5 | 6 | 7 | 10) => i as f64,
        _ if i == 6 => 0.0,
        _ => point.0 - base.0,
    }
}

pub(super) fn animate(p: &mut Pose<'_>, e: &Entity, _s: &AnimState, pt: f32) -> bool {
    if e.kind != KIND {
        return false;
    }
    let flap = e.extra[2] + pt * (e.extra[1] - e.extra[2]);
    let phi = flap * 2.0 * PI;
    p.set("jaw", |x| x[3] = (phi.sin() + 1.0) * 0.2);
    let b = (phi - 1.0).sin() + 1.0;
    let b = (b * b + 2.0 * b) * 0.05;
    let root = |x: &mut [f32; 9]| {
        x[1] = (b - 2.0) * 16.0;
        x[2] = -48.0;
        x[3] = 2.0 * b * DEG;
    };
    if p.has("root") {
        p.set("root", root);
    } else if let Some(x) = p.parts.first_mut() {
        root(x);
    }

    // Neck.
    let [mut x, mut y, mut z] = p.get("neck0").map_or([0.0, 20.0, -12.0], |n| [n[0], n[1], n[2]]);
    let s = sample(e, 6, pt);
    let s5 = sample(e, 5, pt);
    let r2 = wrap(s5.1 - sample(e, 10, pt).1);
    let r = wrap(s5.1 + r2 / 2.0);
    const NECK: [&str; 5] = ["neck0", "neck1", "neck2", "neck3", "neck4"];
    for (i, name) in NECK.iter().enumerate() {
        let q = sample(e, 5 - i, pt);
        let yr = wrap(q.1 - s.1) * DEG * 1.5;
        let xr = (0.45 * i as f32 + phi).cos() * 0.15 + head_offset(e, i, s, q) as f32 * DEG * 7.5;
        let zr = -wrap(q.1 - r) * DEG * 1.5;
        p.set(name, |n| *n = [x, y, z, xr, yr, zr, n[6], n[7], n[8]]);
        x -= yr.sin() * xr.cos() * 10.0;
        y += xr.sin() * 10.0;
        z -= yr.cos() * xr.cos() * 10.0;
    }

    // Head.
    let c = sample(e, 0, pt);
    let yr = wrap(c.1 - s.1) * DEG;
    let xr = wrap64(head_offset(e, 6, s, c)) as f32 * DEG * 7.5;
    let zr = -wrap(c.1 - r) * DEG;
    p.set("head", |h| *h = [x, y, z, xr, yr, zr, h[6], h[7], h[8]]);

    p.set("body", |x| x[5] = -r2 * 1.5 * DEG);

    // Wings.
    let wx = 0.125 - phi.cos() * 0.2;
    let wz = -(phi.sin() + 0.125) * 0.8;
    let tip = ((phi + 2.0).sin() + 0.5) * 0.75;
    p.set("left_wing", |w| {
        w[3] = wx;
        w[4] = -0.25;
        w[5] = wz;
    });
    p.set("left_wing_tip", |w| w[5] = tip);
    p.set("right_wing", |w| {
        w[3] = wx;
        w[4] = 0.25;
        w[5] = -wz;
    });
    p.set("right_wing_tip", |w| w[5] = -tip);

    // Legs.
    let k = 0.1 * b;
    for (part, v) in [
        ("left_hind_leg", 1.0 + k),
        ("left_hind_leg_tip", 0.5 + k),
        ("left_hind_foot", 0.75 + k),
        ("left_front_leg", 1.3 + k),
        ("left_front_leg_tip", -0.5 - k),
        ("left_front_foot", 0.75 + k),
        ("right_hind_leg", 1.0 + k),
        ("right_hind_leg_tip", 0.5 + k),
        ("right_hind_foot", 0.75 + k),
        ("right_front_leg", 1.3 + k),
        ("right_front_leg_tip", -0.5 - k),
        ("right_front_foot", 0.75 + k),
    ] {
        p.set(part, |x| x[3] = v);
    }

    // Tail.
    const TAIL: [&str; 12] =
        ["tail0", "tail1", "tail2", "tail3", "tail4", "tail5", "tail6", "tail7", "tail8", "tail9", "tail10", "tail11"];
    let [mut x, mut y, mut z] = p.get("tail0").map_or([0.0, 10.0, 60.0], |n| [n[0], n[1], n[2]]);
    let s = sample(e, 11, pt);
    let mut t = 0.0f32;
    for (i, name) in TAIL.iter().enumerate() {
        let q = sample(e, 12 + i, pt);
        t += (0.45 * i as f32 + phi).sin() * 0.05;
        let yr = (wrap(q.1 - s.1) * 1.5 + 180.0) * DEG;
        let xr = t + (q.0 - s.0) as f32 * DEG * 7.5;
        let zr = wrap(q.1 - r) * DEG * 1.5;
        p.set(name, |n| *n = [x, y, z, xr, yr, zr, n[6], n[7], n[8]]);
        y += xr.sin() * 10.0;
        z -= yr.cos() * xr.cos() * 10.0;
        x -= yr.sin() * xr.cos() * 10.0;
    }
    true
}

pub(super) fn rotations(e: &Entity, pt: f32, m: &mut Mat4) -> bool {
    if e.kind != KIND {
        return false;
    }
    let (y5, _) = sample(e, 5, pt);
    let (y10, _) = sample(e, 10, pt);
    let (_, yaw7) = sample(e, 7, pt);
    *m *= Mat4::from_rotation_y(-yaw7 * DEG)
        * Mat4::from_rotation_x(((y5 - y10) * 10.0) as f32 * DEG)
        * Mat4::from_translation(Vec3::new(0.0, 0.0, 1.0));
    // No death tilt (the engine omits it for the dragon).
    true
}

pub(super) fn scale(_e: &Entity, _pt: f32, _scale: &mut Vec3) {}

/// Dissolve mask of the dying dragon.
pub(super) const MASK: &str = "entity/enderdragon/dragon_exploding";

/// Interpolated death time (0..200), `None` if alive.
fn death(e: &Entity, pt: f32) -> Option<f32> {
    (e.kind == KIND && e.death_time > 0).then(|| (e.death_time as f32 - 1.0 + pt).max(0.0))
}

/// Dying: dissolve alpha 1 − death/200 (the body switches to "cutout with dissolve").
pub(super) fn dissolve(e: &Entity, pt: f32) -> Option<f32> {
    death(e, pt).map(|t| (1.0 - t / 200.0).clamp(0.0, 1.0))
}

/// Death rays (§ 15.1) in the model frame `m`: triangle fan, cumulative rotation
/// drawn from a fixed-seed generator 432; RGBA colours (white centre, transparent magenta
/// edge).
pub(super) fn rays(m: Mat4, tau: f32, out: &mut Vec<FreeVertex>) {
    let over = if tau > 0.8 { ((tau - 0.8) / 0.2).min(1.0) } else { 0.0 };
    let inner = 0x00ff_ffff | u32::from(((1.0 - over) * 255.0) as u8) << 24;
    const OUTER: u32 = 0x00ff_00ff;
    let mut r = crate::sky_mesh::JavaRandom::new(432);
    let count = ((tau + tau * tau) / 2.0 * 60.0).floor() as i32;
    let mut pose = m * Mat4::from_translation(Vec3::new(0.0, -1.0, -2.0));
    const TAU: f32 = PI * 2.0;
    const HALF_SQRT_3: f32 = 0.866_025_4;
    let vert = |pose: &Mat4, p: Vec3, color: u32| FreeVertex {
        pos: pose.transform_point3(p).to_array(),
        uv: [0.0; 2],
        rect: [0.0; 4],
        color,
        light: 0xf0f0,
        normal: [0.0, 1.0, 0.0],
    };
    for _ in 0..count {
        let (a, b, c) = (r.float() * TAU, r.float() * TAU, r.float() * TAU);
        let (d, f, g) = (r.float() * TAU, r.float() * TAU, r.float() * TAU + tau * (PI / 2.0));
        let q = glam::Quat::from_rotation_x(a)
            * glam::Quat::from_rotation_y(b)
            * glam::Quat::from_rotation_z(c)
            * glam::Quat::from_rotation_x(d)
            * glam::Quat::from_rotation_y(f)
            * glam::Quat::from_rotation_z(g);
        pose *= Mat4::from_quat(q);
        let len = r.float() * 20.0 + 5.0 + over * 10.0;
        let w = r.float() * 2.0 + 1.0 + over * 2.0;
        let o = vert(&pose, Vec3::ZERO, inner);
        let left = vert(&pose, Vec3::new(-HALF_SQRT_3 * w, len, -0.5 * w), OUTER);
        let right = vert(&pose, Vec3::new(HALF_SQRT_3 * w, len, -0.5 * w), OUTER);
        let bottom = vert(&pose, Vec3::new(0.0, len, w), OUTER);
        out.extend([o, left, right, o, right, bottom, o, bottom, left]);
    }
}

impl EntityRenderer {
    /// Dragon effects outside the model: death rays (model frame `m`) and beam towards the
    /// nearest End crystal (within the box inflated by 32; the game looks for it once every
    /// 10 ticks, here every frame), from position `rel`.
    pub(super) fn dragon_effects(&mut self, e: &Entity, m: Mat4, rel: [f32; 3], pt: f32, light: u32) {
        if e.kind != KIND {
            return;
        }
        if let Some(t) = death(e, pt) {
            rays(m, t / 200.0, &mut self.free_verts[FREE_RAYS]);
        }
        let p = e.render_pos(pt);
        let [w, h] = {
            let d = e.dims(&self.dims);
            [f64::from(d[0]) / 2.0 + 32.0, f64::from(d[1]) + 32.0]
        };
        let nearest = self
            .crystals
            .iter()
            .filter(|(c, _)| {
                (c[0] - p[0]).abs() <= w && (c[2] - p[2]).abs() <= w && c[1] >= p[1] - 32.0 && c[1] <= p[1] + h
            })
            .min_by(|a, b| {
                let d = |c: &[f64; 3]| (0..3).map(|i| (c[i] - p[i]).powi(2)).sum::<f64>();
                d(&a.0).total_cmp(&d(&b.0))
            })
            .copied();
        if let Some((c, tc)) = nearest {
            let hc = f64::from(objects::crystal_height(tc));
            let d = Vec3::new((c[0] - p[0]) as f32, (c[1] + hc - p[1]) as f32, (c[2] - p[2]) as f32);
            self.push_beam(Mat4::from_translation(Vec3::from(rel)), d, e.age as f32 + pt, light);
        }
    }
}

pub(super) fn shadow(k: &str) -> Option<(f32, f32)> {
    (k == KIND).then_some((0.5, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_bounds() {
        assert_eq!(wrap(180.0), -180.0);
        assert_eq!(wrap(-190.0), 170.0);
        assert_eq!(wrap(540.0), -180.0);
        assert!((wrap64(359.5) + 0.5).abs() < 1e-9);
    }

    #[test]
    fn rays_count_and_fade() {
        let mut v = Vec::new();
        rays(Mat4::IDENTITY, 0.5, &mut v);
        // floor((0.5 + 0.25) / 2 × 60) = 22 rays of 3 triangles.
        assert_eq!(v.len(), 22 * 9);
        let first = v[0].pos;
        v.clear();
        rays(Mat4::IDENTITY, 0.5, &mut v);
        assert_eq!(v[0].pos, first, "reproducible draw");
        v.clear();
        rays(Mat4::IDENTITY, 1.0, &mut v);
        assert_eq!(v.len(), 60 * 9);
        // Full overheat: centre alpha zero, transparent magenta edge.
        assert_eq!(v[0].color >> 24, 0);
        assert_eq!(v[1].color, 0x00ff_00ff);
    }

    #[test]
    fn beam_geometry() {
        let mut v = Vec::new();
        super::objects::beam_quads(Mat4::IDENTITY, Vec3::new(0.0, 10.0, 0.0), 0.0, [0.0; 4], 0, &mut v);
        assert_eq!(v.len(), 8 * 6);
        // Start (0, 2, 0); end 10 blocks above, v1 = 10/32.
        let top = v.iter().map(|x| x.pos[1]).fold(f32::MIN, f32::max);
        assert!((top - 12.0).abs() < 1e-4, "{top}");
        assert!(v.iter().any(|x| (x.uv[1] - 10.0 / 32.0).abs() < 1e-6));
    }
}
