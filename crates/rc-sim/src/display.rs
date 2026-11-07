//! Entities without AI that only carry data (`entities.md` § 3): display entities (block,
//! item, text) and the interaction box. Their own fields are read from the kept compound the
//! way the game loads them (any numeric tag accepted, a field that does not decode falls back
//! to its default) and sent at the official indices: on spawn the values that differ from the
//! defaults, after `/data` the changed ones plus the interpolation start, which the game always
//! marks as changed.
//!
//! A transformation given as a 4×4 matrix (16 numbers, row by row) is decomposed like the game:
//! eigenvectors of MᵀM by 5 Jacobi sweeps, then three Givens QR steps. Single precision, no
//! fused multiply-add, the game's operation order: results identical to the bit.

use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use crate::commands::ents::E;
use crate::server::Server;

// ---------- Numbers read like the game ----------

/// Integer value of a numeric tag: decimals rounded down, a long cut to its low 32 bits.
fn int_of(t: &Nbt) -> Option<i32> {
    Some(match *t {
        Nbt::Byte(v) => i32::from(v),
        Nbt::Short(v) => i32::from(v),
        Nbt::Int(v) => v,
        Nbt::Long(v) => v as i32,
        Nbt::Float(v) => v.floor() as i32,
        Nbt::Double(v) => v.floor() as i32,
        _ => return None,
    })
}

fn float_of(t: &Nbt) -> Option<f32> {
    Some(match *t {
        Nbt::Byte(v) => f32::from(v),
        Nbt::Short(v) => f32::from(v),
        Nbt::Int(v) => v as f32,
        Nbt::Long(v) => v as f32,
        Nbt::Float(v) => v,
        Nbt::Double(v) => v as f32,
        _ => return None,
    })
}

fn get_int(e: &Nbt, k: &str, d: i32) -> i32 {
    e.get(k).and_then(int_of).unwrap_or(d)
}

fn get_float(e: &Nbt, k: &str, d: f32) -> f32 {
    e.get(k).and_then(float_of).unwrap_or(d)
}

/// Byte value: low 8 bits of the integer value.
fn get_byte(e: &Nbt, k: &str, d: i8) -> i8 {
    e.get(k).and_then(int_of).map_or(d, |v| v as i8)
}

fn get_bool(e: &Nbt, k: &str) -> bool {
    get_byte(e, k, 0) != 0
}

/// Exactly `N` numbers (list or numeric array), otherwise nothing.
fn floats<const N: usize>(t: &Nbt) -> Option<[f32; N]> {
    let v: Vec<f32> = match t {
        Nbt::List(l) => l.iter().map(float_of).collect::<Option<_>>()?,
        Nbt::ByteArray(a) => a.iter().map(|&b| f32::from(b as i8)).collect(),
        Nbt::IntArray(a) => a.iter().map(|&i| i as f32).collect(),
        Nbt::LongArray(a) => a.iter().map(|&i| i as f32).collect(),
        _ => return None,
    };
    v.try_into().ok()
}

// ---------- Single-precision algebra (column-major 3×3, quaternions [x, y, z, w]) ----------

/// 3×3 matrix stored by columns: `m[column][row]`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct M3([[f32; 3]; 3]);

impl M3 {
    fn transpose(self) -> M3 {
        M3(std::array::from_fn(|c| std::array::from_fn(|r| self.0[r][c])))
    }

    /// `self × b`, each term summed as a0·b0 + (a1·b1 + a2·b2).
    fn mul(self, b: M3) -> M3 {
        let a = self.0;
        M3(std::array::from_fn(|c| {
            std::array::from_fn(|r| a[0][r] * b.0[c][0] + (a[1][r] * b.0[c][1] + a[2][r] * b.0[c][2]))
        }))
    }

    /// `self × rotation(q)`, each term summed as (a0·r0 + a1·r1) + a2·r2.
    fn rotate(self, q: [f32; 4]) -> M3 {
        let [x, y, z, w] = q;
        let (w2, x2, y2, z2) = (w * w, x * x, y * y, z * z);
        let (zw, xy, xz, yw, yz, xw) = (z * w, x * y, x * z, y * w, y * z, x * w);
        let (dzw, dxy, dxz, dyw, dyz, dxw) = (zw + zw, xy + xy, xz + xz, yw + yw, yz + yz, xw + xw);
        let rm = [
            [w2 + x2 - z2 - y2, dxy + dzw, dxz - dyw],
            [dxy - dzw, y2 - z2 + w2 - x2, dyz + dxw],
            [dyw + dxz, dyz - dxw, z2 - y2 - x2 + w2],
        ];
        let a = self.0;
        M3(std::array::from_fn(|c| {
            std::array::from_fn(|r| a[0][r] * rm[c][0] + a[1][r] * rm[c][1] + a[2][r] * rm[c][2])
        }))
    }
}

/// Quaternion product `a × q`, the game's nesting of the sums.
fn qmul(a: [f32; 4], q: [f32; 4]) -> [f32; 4] {
    let [x, y, z, w] = a;
    let [qx, qy, qz, qw] = q;
    [
        w * qx + (x * qw + (y * qz + -z * qy)),
        w * qy + (-x * qz + (y * qw + z * qx)),
        w * qz + (x * qy + (-y * qx + z * qw)),
        w * qw + (-x * qx + (-y * qy + -z * qz)),
    ]
}

fn normalize(q: [f32; 4]) -> [f32; 4] {
    let [x, y, z, w] = q;
    let inv = 1.0 / (x * x + (y * y + (z * z + w * w))).sqrt();
    [x * inv, y * inv, z * inv, w * inv]
}

/// Sine in double precision rounded to single, like the game's math library.
fn sin32(a: f32) -> f32 {
    f64::from(a).sin() as f32
}

/// Cosine deduced from the sine: magnitude by Pythagoras, sign by the quadrant of the angle.
fn cos_from_sin(sin: f32, angle: f32) -> f32 {
    use std::f32::consts::{FRAC_PI_2, PI, TAU};
    let cos = (1.0 - sin * sin).sqrt();
    let a = angle + FRAC_PI_2;
    let mut b = a - ((a / TAU) as i32) as f32 * TAU;
    if b < 0.0 {
        b += TAU;
    }
    if b >= PI { -cos } else { cos }
}

/// Rotation of `angle` around `axis` (axis taken as is, never normalised): angle brought back
/// into [0, 2π) first.
fn axis_angle(angle: f32, axis: [f32; 3]) -> [f32; 4] {
    use std::f32::consts::TAU;
    let angle = if f64::from(angle) < 0.0 { TAU + angle % TAU } else { angle } % TAU;
    let half = angle * 0.5;
    let s = sin32(half);
    let c = cos_from_sin(s, half);
    [axis[0] * s, axis[1] * s, axis[2] * s, c]
}

/// Half-angle sine and cosine of a plane rotation.
#[derive(Debug, Clone, Copy)]
struct Givens {
    sh: f32,
    ch: f32,
}

impl Givens {
    fn unnormalized(sh: f32, ch: f32) -> Givens {
        let w = 1.0 / (sh * sh + ch * ch).sqrt();
        Givens { sh: w * sh, ch: w * ch }
    }

    /// A quarter turn (π/4).
    fn quarter() -> Givens {
        let half = std::f32::consts::FRAC_PI_4 / 2.0;
        let sh = sin32(half);
        Givens { sh, ch: cos_from_sin(sh, half) }
    }

    fn inverse(self) -> Givens {
        Givens { sh: -self.sh, ch: self.ch }
    }

    fn cs(self) -> (f32, f32) {
        (self.ch * self.ch - self.sh * self.sh, 2.0 * self.sh * self.ch)
    }

    fn qx(self) -> [f32; 4] {
        [self.sh, 0.0, 0.0, self.ch]
    }

    fn qy(self) -> [f32; 4] {
        [0.0, self.sh, 0.0, self.ch]
    }

    fn qz(self) -> [f32; 4] {
        [0.0, 0.0, self.sh, self.ch]
    }

    fn mx(self) -> M3 {
        let (c, s) = self.cs();
        M3([[1.0, 0.0, 0.0], [0.0, c, s], [0.0, -s, c]])
    }

    fn my(self) -> M3 {
        let (c, s) = self.cs();
        M3([[c, 0.0, -s], [0.0, 1.0, 0.0], [s, 0.0, c]])
    }

    fn mz(self) -> M3 {
        let (c, s) = self.cs();
        M3([[c, s, 0.0], [-s, c, 0.0], [0.0, 0.0, 1.0]])
    }
}

/// Approximate rotation that cancels the off-diagonal term `a12` (quarter turn when it dominates).
fn approx_givens(a11: f32, a12: f32, a22: f32) -> Givens {
    let g = 3.0 + 2.0 * 2.0f32.sqrt();
    let ch = 2.0 * (a11 - a22);
    let sh = a12;
    if g * sh * sh < ch * ch { Givens::unnormalized(sh, ch) } else { Givens::quarter() }
}

/// Rotation of a QR step that cancels `a2` under `a1`.
fn qr_givens(a1: f32, a2: f32) -> Givens {
    let p = f64::from(a1).hypot(f64::from(a2)) as f32;
    let mut sh = if p > 1.0e-6 { a2 } else { 0.0 };
    let mut ch = a1.abs() + p.max(1.0e-6);
    if a1 < 0.0 {
        std::mem::swap(&mut sh, &mut ch);
    }
    Givens::unnormalized(sh, ch)
}

/// `a ← qᵀ × a × q`.
fn similarity(a: &mut M3, q: M3) {
    let aq = a.mul(q);
    *a = q.transpose().mul(aq);
}

/// One Jacobi sweep over the pairs (0, 1), (0, 2), (1, 2) of the symmetric matrix `m`.
fn jacobi_step(m: &mut M3, out: &mut [f32; 4]) {
    let a = m.0;
    if a[0][1] * a[0][1] + a[1][0] * a[1][0] > 1.0e-6 {
        let p = approx_givens(a[0][0], 0.5 * (a[0][1] + a[1][0]), a[1][1]);
        *out = qmul(*out, p.qz());
        similarity(m, p.mz());
    }
    let a = m.0;
    if a[0][2] * a[0][2] + a[2][0] * a[2][0] > 1.0e-6 {
        let p = approx_givens(a[0][0], 0.5 * (a[0][2] + a[2][0]), a[2][2]).inverse();
        *out = qmul(*out, p.qy());
        similarity(m, p.my());
    }
    let a = m.0;
    if a[1][2] * a[1][2] + a[2][1] * a[2][1] > 1.0e-6 {
        let p = approx_givens(a[1][1], 0.5 * (a[1][2] + a[2][1]), a[2][2]);
        *out = qmul(*out, p.qx());
        similarity(m, p.mx());
    }
}

/// Decomposed transformation: translation, left rotation, scale, right rotation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Pose {
    pub t: [f32; 3],
    pub l: [f32; 4],
    pub s: [f32; 3],
    pub r: [f32; 4],
}

const IDENTITY: Pose = Pose { t: [0.0; 3], l: [0.0, 0.0, 0.0, 1.0], s: [1.0; 3], r: [0.0, 0.0, 0.0, 1.0] };

/// SVD of a 4×4 matrix given row by row: M = T × U × S × V (homogeneous factor divided out).
pub(crate) fn decompose(m: [f32; 16]) -> Pose {
    let k = 1.0 / m[15];
    let a = M3(std::array::from_fn(|c| std::array::from_fn(|r| m[r * 4 + c] * k)));
    // Eigenvectors of AᵀA (right rotation, still to be conjugated), then QR of A × V.
    let mut sym = a.transpose().mul(a);
    let mut v = [0.0, 0.0, 0.0, 1.0];
    for _ in 0..5 {
        jacobi_step(&mut sym, &mut v);
    }
    let v = normalize(v);
    // Squared column scales compared in double precision (the game's threshold).
    let zero0 = f64::from(sym.0[0][0]) < 1.0e-6;
    let zero1 = f64::from(sym.0[1][1]) < 1.0e-6;
    let u012s = a.rotate(v);
    let p = if zero0 { qr_givens(u012s.0[1][1], -u012s.0[1][0]) } else { qr_givens(u012s.0[0][0], u012s.0[0][1]) };
    let mut u = qmul([0.0, 0.0, 0.0, 1.0], p.qz());
    let u12s = p.mz().transpose().mul(u012s);
    let p = if zero0 { qr_givens(u12s.0[2][2], -u12s.0[2][0]) } else { qr_givens(u12s.0[0][0], u12s.0[0][2]) };
    let p = p.inverse();
    u = qmul(u, p.qy());
    let u2s = p.my().transpose().mul(u12s);
    let p = if zero1 { qr_givens(u2s.0[2][2], -u2s.0[2][1]) } else { qr_givens(u2s.0[1][1], u2s.0[1][2]) };
    u = qmul(u, p.qx());
    let sm = p.mx().transpose().mul(u2s);
    Pose {
        t: [m[3] * k, m[7] * k, m[11] * k],
        l: u,
        s: [sm.0[0][0], sm.0[1][1], sm.0[2][2]],
        r: [-v[0], -v[1], -v[2], v[3]],
    }
}

/// Rotation: 4 components (normalised) or `{angle, axis}`.
fn rotation(t: &Nbt) -> Option<[f32; 4]> {
    if let Some(q) = floats::<4>(t) {
        return Some(normalize(q));
    }
    let angle = t.get("angle").and_then(float_of)?;
    let axis = t.get("axis").and_then(floats::<3>)?;
    Some(axis_angle(angle, axis))
}

/// `transformation`: the four decomposed fields (all required), otherwise a matrix of 16 numbers;
/// identity if neither decodes.
pub(crate) fn transformation(t: Option<&Nbt>) -> Pose {
    let Some(t) = t else { return IDENTITY };
    if let Nbt::Compound(_) = t {
        let parts = (
            t.get("translation").and_then(floats::<3>),
            t.get("left_rotation").and_then(rotation),
            t.get("scale").and_then(floats::<3>),
            t.get("right_rotation").and_then(rotation),
        );
        return match parts {
            (Some(t), Some(l), Some(s), Some(r)) => Pose { t, l, s, r },
            _ => IDENTITY,
        };
    }
    floats::<16>(t).map_or(IDENTITY, decompose)
}

// ---------- Data of the entities ----------

/// Kind of entity handled here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Block,
    Item,
    Text,
    Interaction,
}

pub(crate) fn kind_of(e: &Nbt) -> Option<Kind> {
    Some(match e.get("id").and_then(Nbt::as_str)? {
        "minecraft:block_display" => Kind::Block,
        "minecraft:item_display" => Kind::Item,
        "minecraft:text_display" => Kind::Text,
        "minecraft:interaction" => Kind::Interaction,
        _ => return None,
    })
}

/// Index of a name among `names` (an unknown name falls back to the first, the default).
fn named(e: &Nbt, k: &str, names: &[&str]) -> i8 {
    e.get(k).and_then(Nbt::as_str).and_then(|n| names.iter().position(|x| *x == n)).map_or(0, |i| i as i8)
}

const BILLBOARDS: [&str; 4] = ["fixed", "vertical", "horizontal", "center"];
const ITEM_CONTEXTS: [&str; 10] = [
    "none",
    "thirdperson_lefthand",
    "thirdperson_righthand",
    "firstperson_lefthand",
    "firstperson_righthand",
    "head",
    "gui",
    "ground",
    "fixed",
    "on_shelf",
];

/// Forced light `{block, sky}` (0 to 15 each) packed as block × 16 + sky × 2²⁰, −1 if absent.
fn brightness(e: &Nbt) -> i32 {
    let Some(b) = e.get("brightness") else { return -1 };
    let level = |k: &str| b.get(k).and_then(int_of).filter(|v| (0..=15).contains(v));
    match (level("block"), level("sky")) {
        (Some(bl), Some(sky)) => bl << 4 | sky << 20,
        _ => -1,
    }
}

/// Text style flags: shadow 1, see-through 2, default background 4, left 8 / right 16.
fn text_flags(e: &Nbt) -> i8 {
    let mut f = 0i8;
    for (k, bit) in [("shadow", 1), ("see_through", 2), ("default_background", 4)] {
        if get_bool(e, k) {
            f |= bit;
        }
    }
    match e.get("alignment").and_then(Nbt::as_str) {
        Some("left") => f | 8,
        Some("right") => f | 16,
        _ => f,
    }
}

/// Width, height and eye height of a compound the blast pushes although it never moves by
/// itself: displays and markers have no size and their eyes at their feet, an interaction its
/// own size; `None` for any other entity.
fn still_size(e: &Nbt) -> Option<(f64, f64, f64)> {
    match kind_of(e) {
        Some(Kind::Interaction) => {
            let h = get_float(e, "height", 1.0);
            Some((f64::from(get_float(e, "width", 1.0)), f64::from(h), f64::from(h * 0.85)))
        }
        Some(_) => Some((0.0, 0.0, 0.0)),
        None if e.get("id").and_then(Nbt::as_str) == Some("minecraft:marker") => Some((0.0, 0.0, 0.0)),
        None => None,
    }
}

/// Same value for the game's change test: decimals compared bit for bit, a non-empty stack
/// never equal (each load builds a new one).
fn same(a: &Meta, b: &Meta) -> bool {
    let bits3 = |v: &[f32; 3]| v.map(f32::to_bits);
    let bits4 = |v: &[f32; 4]| v.map(f32::to_bits);
    match (a, b) {
        (Meta::Float(x), Meta::Float(y)) => x.to_bits() == y.to_bits(),
        (Meta::Vector3(x), Meta::Vector3(y)) => bits3(x) == bits3(y),
        (Meta::Quaternion(x), Meta::Quaternion(y)) => bits4(x) == bits4(y),
        (Meta::Item(Some(_)), _) | (_, Meta::Item(Some(_))) => false,
        _ => a == b,
    }
}

impl Server {
    /// Own data of a display or interaction entity, every index of its type in order, value
    /// read from `e` (default when absent).
    fn data_values(&self, k: Kind, e: &Nbt) -> Vec<(u8, Meta)> {
        if k == Kind::Interaction {
            return vec![
                (8, Meta::Float(get_float(e, "width", 1.0))),
                (9, Meta::Float(get_float(e, "height", 1.0))),
                (10, Meta::Bool(get_bool(e, "response"))),
            ];
        }
        let p = transformation(e.get("transformation"));
        let mut v = vec![
            (8, Meta::Int(get_int(e, "start_interpolation", 0))),
            (9, Meta::Int(get_int(e, "interpolation_duration", 0))),
            (10, Meta::Int(get_int(e, "teleport_duration", 0).clamp(0, 59))),
            (11, Meta::Vector3(p.t)),
            (12, Meta::Vector3(p.s)),
            (13, Meta::Quaternion(p.l)),
            (14, Meta::Quaternion(p.r)),
            (15, Meta::Byte(named(e, "billboard", &BILLBOARDS))),
            (16, Meta::Int(brightness(e))),
            (17, Meta::Float(get_float(e, "view_range", 1.0))),
            (18, Meta::Float(get_float(e, "shadow_radius", 0.0))),
            (19, Meta::Float(get_float(e, "shadow_strength", 1.0))),
            (20, Meta::Float(get_float(e, "width", 0.0))),
            (21, Meta::Float(get_float(e, "height", 0.0))),
            (22, Meta::Int(get_int(e, "glow_color_override", -1))),
        ];
        match k {
            Kind::Block => {
                let st = e.get("block_state").and_then(|s| self.states.state_of(&self.data, s)).unwrap_or(0);
                v.push((23, Meta::State(st as i32)));
            }
            Kind::Item => {
                let s = e.get("item").and_then(crate::player::Stack::from_nbt);
                v.push((23, Meta::Item(self.net(s.as_ref()))));
                v.push((24, Meta::Byte(named(e, "item_display", &ITEM_CONTEXTS))));
            }
            Kind::Text => {
                let text = e.get("text").cloned().unwrap_or(Nbt::String(String::new()));
                v.push((23, Meta::Component(text)));
                v.push((24, Meta::Int(get_int(e, "line_width", 200))));
                v.push((25, Meta::Int(get_int(e, "background", 0x4000_0000))));
                v.push((26, Meta::Byte(get_byte(e, "text_opacity", -1))));
                v.push((27, Meta::Byte(text_flags(e))));
            }
            Kind::Interaction => {}
        }
        v
    }

    /// Own data to send for entity `e`: on spawn (`old` absent) those that differ from the
    /// defaults; after a modification those that changed, plus the interpolation start of a
    /// display (always resent).
    pub(crate) fn display_meta(&self, e: &Nbt, old: Option<&Nbt>, out: &mut Vec<(u8, Meta)>) {
        let Some(k) = kind_of(e) else { return };
        let now = self.data_values(k, e);
        let before = match old {
            Some(o) if kind_of(o) == Some(k) => self.data_values(k, o),
            _ => self.data_values(k, &Nbt::Compound(Vec::new())),
        };
        let forced = |i: u8| old.is_some() && k != Kind::Interaction && i == 8;
        out.extend(now.into_iter().zip(before).filter(|((i, a), (_, b))| forced(*i) || !same(a, b)).map(|(a, _)| a));
    }

    /// Text of a text display resolved like the game when it loads the entity (selector, score
    /// and nbt contents), source = the entity at `at` with game master rights. On failure, or
    /// without a `text` field, the text stays `previous` (the one the entity had, absent at
    /// creation).
    pub(crate) fn display_resolve_text(&mut self, id: i32, at: [f64; 3], previous: Option<Nbt>) {
        let found = self.entities.values().flatten().find(|(i, _)| *i == id).map(|(_, n)| n);
        let Some(n) = found.filter(|n| kind_of(n) == Some(Kind::Text)) else { return };
        let value = match n.get("text").cloned() {
            None if previous.is_none() => return,
            None => previous,
            Some(text) => {
                let [yaw, pitch] = crate::nbtio::f32s::<2>(n.get("Rotation")).unwrap_or_default();
                let base = self.server_source();
                let mut src = self.with_entity(&base, E::Other(id));
                src.pos = at;
                src.rot = [pitch, yaw];
                src.dim = self.dim.name.clone();
                src.level = 2;
                src.silent = true;
                self.resolve_component(&src, &text, Some(E::Other(id)), 0).ok().or(previous)
            }
        };
        for (c, l) in self.entities.iter_mut() {
            if let Some((_, n)) = l.iter_mut().find(|(i, _)| *i == id) {
                match value {
                    Some(v) => crate::nbtio::set(n, "text", v),
                    None => crate::nbtio::remove(n, "text"),
                }
                self.life.drop_chunks.insert(*c);
                return;
            }
        }
    }

    /// Entities that never move by themselves in an explosion or a burst, pushed like any entity
    /// (× `mult`; the damage does nothing to them): displays, markers and interactions kept as
    /// compounds (motion saved; a display or an interaction sends it when it differs from the
    /// motion before beyond the tracking's threshold, a marker is never sent), then the effect
    /// clouds. Only the compounds of those types are looked at.
    pub(crate) fn blast_push_still(&mut self, center: [f64; 3], radius: f32, mult: f64) {
        if radius < 1.0e-5 {
            return;
        }
        let diameter = f64::from(radius * 2.0);
        let push = |s: &Server, pos: [f64; 3], eye: f64, b: &rc_physics::Aabb| -> Option<[f64; 3]> {
            let dist = crate::mobs::mth::dist2(pos, center).sqrt() / diameter;
            if dist > 1.0 {
                return None;
            }
            let dir = crate::mobs::mth::normalize([pos[0] - center[0], pos[1] + eye - center[1], pos[2] - center[2]]);
            let exposure = if mult == 0.0 { 0.0 } else { f64::from(s.seen_percent(center, b)) };
            let strength = (1.0 - dist) * exposure * mult;
            Some(dir.map(|v| v * strength))
        };
        let distance_first = self.deco.switches.still_distance_first;
        let out_of_reach = |p: [f64; 3]| crate::mobs::mth::dist2(p, center).sqrt() / diameter > 1.0;
        let pos_of = |e: &Nbt| crate::nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
        let mut hits: Vec<((i32, i32), usize, [f64; 3])> = Vec::new();
        // Every kept compound: one teleported keeps the list of the chunk it was read in.
        for (&c, list) in &self.entities {
            for (k, (_, e)) in list.iter().enumerate() {
                // Distance first (switch): a far compound is dropped without its type compared.
                if distance_first && out_of_reach(pos_of(e)) {
                    continue;
                }
                let Some((w, h, eye)) = still_size(e) else { continue };
                let p = pos_of(e);
                let b = rc_physics::Aabb { min: [p[0] - w / 2.0, p[1], p[2] - w / 2.0], max: [p[0] + w / 2.0, p[1] + h, p[2] + w / 2.0] };
                if let Some(kb) = push(self, p, eye, &b) {
                    hits.push((c, k, kb));
                }
            }
        }
        for (c, k, kb) in hits {
            let Some((id, n)) = self.entities.get_mut(&c).and_then(|l| l.get_mut(k)) else { continue };
            let id = *id;
            let old = crate::nbtio::motion(n.get("Motion")).unwrap_or_default();
            let new = [old[0] + kb[0], old[1] + kb[1], old[2] + kb[2]];
            crate::nbtio::set(n, "Motion", Nbt::List(new.iter().map(|&v| Nbt::Double(v)).collect()));
            let visible = crate::decoration::client_visible(n);
            self.life.drop_chunks.insert(c);
            if visible && crate::mobs::track::velocity_changed(old, new) {
                let velocity = crate::mobs::track::lp_quantize(new);
                let _ = self.send(rc_client::Event::Entity(rc_client::EntityEvent::Motion { id, velocity }));
            }
        }
        for i in 0..self.proj.clouds.len() {
            let c = &self.proj.clouds[i];
            if c.removed {
                continue;
            }
            // Box 2 × radius wide, 0.5 high: eyes at 0.85 of it.
            let Some(kb) = push(self, c.pos, f64::from(0.5f32 * 0.85), &c.bbox()) else { continue };
            let c = &mut self.proj.clouds[i];
            c.vel = [c.vel[0] + kb[0], c.vel[1] + kb[1], c.vel[2] + kb[2]];
            if crate::mobs::track::velocity_changed(c.sent_vel, c.vel) {
                c.sent_vel = c.vel;
                let (id, velocity) = (c.id, crate::mobs::track::lp_quantize(c.vel));
                if c.seen {
                    let _ = self.send(rc_client::Event::Entity(rc_client::EntityEvent::Motion { id, velocity }));
                }
            }
        }
    }

    /// Interaction entity `id` reachable by the player (eyes within the interaction range + 3 of
    /// its box, block inside the border): its compound's position, otherwise nothing.
    fn interaction_target(&self, id: i32) -> Option<((i32, i32), usize, bool)> {
        let (c, i) = self
            .entities
            .iter()
            .find_map(|(c, l)| l.iter().position(|e| e.0 == id).map(|i| (*c, i)))?;
        let e = &self.entities[&c][i].1;
        if kind_of(e) != Some(Kind::Interaction) {
            return None;
        }
        let p = crate::nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
        let (w, h) = (f64::from(get_float(e, "width", 1.0)), f64::from(get_float(e, "height", 1.0)));
        let b = rc_physics::Aabb {
            min: [p[0] - w / 2.0, p[1], p[2] - w / 2.0],
            max: [p[0] + w / 2.0, p[1] + h, p[2] + w / 2.0],
        };
        let range = self.player_attribute("minecraft:entity_interaction_range") + 3.0;
        let eye = [self.pos[0], self.pos[1] + self.ent_eye_height(E::Player), self.pos[2]];
        let d2: f64 = (0..3).map(|a| (eye[a] - eye[a].clamp(b.min[a], b.max[a])).powi(2)).sum();
        let block = crate::mobs::mth::block_of(p);
        let ok = d2 < range * range && Self::block_in_border(self.border_bounds(), block);
        Some((c, i, ok))
    }

    /// Records `{player, timestamp}` (player's UUID, game time) under `key`; chunk to rewrite.
    fn interaction_record(&mut self, c: (i32, i32), i: usize, key: &str) {
        let action = Nbt::Compound(vec![
            ("player".into(), crate::nbtio::uuid_nbt(self.uuid)),
            ("timestamp".into(), Nbt::Long(self.time)),
        ]);
        if let Some((_, n)) = self.entities.get_mut(&c).and_then(|l| l.get_mut(i)) {
            crate::nbtio::set(n, key, action);
            self.life.drop_chunks.insert(c);
        }
    }

    /// Player hit on entity `id`; true if it is an interaction entity (handled). The hit is
    /// recorded and `player_hurt_entity` fires (generic damage, 1 dealt and taken); without
    /// `response` the attack stops there, otherwise it goes on as on a target that takes no
    /// damage (charge spent, knockback sound when sprinting at full charge, "no damage" sound).
    pub(crate) fn interaction_attacked(&mut self, id: i32) -> bool {
        let Some((c, i, ok)) = self.interaction_target(id) else { return false };
        if !ok || self.game_mode == 3 || self.life.vit.dead {
            return true;
        }
        self.interaction_record(c, i, "attack");
        let dmg = crate::progress::pred::Dmg::plain("minecraft:generic");
        self.trig_player_hurt_entity(Some(crate::progress::pred::Who::E(E::Other(id))), &dmg, 1.0, 1.0, false);
        if !get_bool(&self.entities[&c][i].1, "response") {
            return true;
        }
        // Damage of a charge-weighted hit (enchantment bonus only counts when the base is
        // zero, never for a player whose base attack damage is 1).
        let speed = self.player_attribute("minecraft:attack_speed");
        let delay = (1.0 / speed * 20.0) as f32;
        let scale = ((self.mobs.attack_ticker as f32 + 0.5) / delay).clamp(0.0, 1.0);
        let base = self.player_attribute("minecraft:attack_damage") as f32 * (0.2 + scale * scale * 0.8);
        self.mobs.attack_ticker = 0;
        if base <= 0.0 {
            return true;
        }
        if self.life.vit.sprinting && scale > 0.9 {
            self.world_sound("minecraft:entity.player.attack.knockback", 7, self.pos, 1.0, 1.0);
        }
        self.world_sound("minecraft:entity.player.attack.nodamage", 7, self.pos, 1.0, 1.0);
        true
    }

    /// Player right click on entity `id`; true if it is an interaction entity (handled): use
    /// recorded, `player_interacted_with_entity` with the stack held before (no swing, the game
    /// consumes the action). A spectator does nothing.
    pub(crate) fn interaction_used(&mut self, id: i32, hand: i32) -> bool {
        let Some((c, i, ok)) = self.interaction_target(id) else { return false };
        if !ok || self.game_mode == 3 || self.life.vit.dead {
            return true;
        }
        let slot = if hand == 0 { 36 + self.held as usize } else { 45 };
        let held = self.slots[slot].clone();
        self.interaction_record(c, i, "interaction");
        self.trig_entity_interact(held.as_ref(), crate::progress::pred::Who::E(E::Other(id)));
        true
    }

    /// Who last attacked (`attack`) or used (`interaction`) an interaction entity: the local
    /// player if it is them (the game looks the player up by UUID).
    pub(crate) fn interaction_player(&self, e: &Nbt, key: &str) -> Option<E> {
        if kind_of(e) != Some(Kind::Interaction) {
            return None;
        }
        // Both fields required (otherwise the record does not decode: nobody).
        let action = e.get(key)?;
        action.get("timestamp").and_then(Nbt::as_i64)?;
        let who = crate::nbtio::uuid(action.get("player"))?;
        (who == self.uuid).then_some(E::Player)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(p: &Pose) -> [u32; 14] {
        let mut out = [0u32; 14];
        let all = p.t.iter().chain(&p.l).chain(&p.s).chain(&p.r);
        for (o, v) in out.iter_mut().zip(all) {
            *o = v.to_bits();
        }
        out
    }

    /// Matrices given row by row, decomposed by the game (JOML 1.10.9 and the game's SVD code run
    /// in Java by a small oracle program kept with the mission notes of G5-F05, outside the
    /// repository: game jar and JOML jar on the class path): translation, left rotation,
    /// scale, right rotation, as float bits.
    #[test]
    fn svd_matches_the_game_bit_for_bit() {
        for (m, want) in ORACLE {
            let got = bits(&decompose(*m));
            assert_eq!(&got, want, "matrix {m:?}: {:?}", decompose(*m));
        }
    }

    /// Rotations read like the game: 4 components normalised, axis and angle (axis not
    /// normalised, angle brought back into [0, 2π)).
    #[test]
    fn rotations_match_the_game() {
        for (input, want) in ROT_ORACLE {
            let q = match input {
                RotIn::Quat(q) => normalize(*q),
                RotIn::Axis(a, x) => axis_angle(*a, *x),
            };
            assert_eq!(q.map(f32::to_bits), *want, "{input:?}: {q:?}");
        }
    }

    /// Decomposed and matrix forms of the same transformation agree within the SVD's precision.
    #[test]
    fn matrix_form_recovers_scale_and_translation() {
        let m = [2.0, 0.0, 0.0, 1.0, 0.0, 3.0, 0.0, 2.0, 0.0, 0.0, 4.0, 3.0, 0.0, 0.0, 0.0, 1.0];
        let p = decompose(m);
        assert_eq!(p.t, [1.0, 2.0, 3.0]);
        let mut s = p.s.map(f32::abs);
        s.sort_by(f32::total_cmp);
        for (a, b) in s.iter().zip([2.0, 3.0, 4.0]) {
            assert!((a - b).abs() < 1e-4, "{:?}", p.s);
        }
        let list = Nbt::List(m.iter().map(|&x| Nbt::Float(x)).collect());
        assert_eq!(transformation(Some(&list)), p);
        // Decomposed form: fields as given; one missing → identity, like the game's codec.
        let f = |v: &[f32]| Nbt::List(v.iter().map(|&x| Nbt::Float(x)).collect());
        let c = Nbt::Compound(vec![
            ("translation".into(), f(&[1.0, 2.0, 3.0])),
            ("left_rotation".into(), f(&[0.0, 0.0, 0.0, 2.0])),
            ("scale".into(), f(&[2.0, 2.0, 2.0])),
            ("right_rotation".into(), Nbt::Compound(vec![("angle".into(), Nbt::Float(0.0)), ("axis".into(), f(&[0.0, 1.0, 0.0]))])),
        ]);
        let p = transformation(Some(&c));
        assert_eq!((p.t, p.l, p.s), ([1.0, 2.0, 3.0], [0.0, 0.0, 0.0, 1.0], [2.0; 3]));
        let Nbt::Compound(mut partial) = c else { unreachable!() };
        partial.remove(0);
        assert_eq!(transformation(Some(&Nbt::Compound(partial))), IDENTITY);
    }

    #[derive(Debug)]
    enum RotIn {
        Quat([f32; 4]),
        Axis(f32, [f32; 3]),
    }

    const ORACLE: &[([f32; 16], [u32; 14])] = &[
        ([1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0], [0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x3f800000, 0x3f800000, 0x3f800000, 0x3f800000, 0x80000000, 0x80000000, 0x80000000, 0x3f800000]),
        ([1.0, 0.0, 0.0, 2.0, 0.0, 1.0, 0.0, 3.0, 0.0, 0.0, 1.0, 4.0, 0.0, 0.0, 0.0, 1.0], [0x40000000, 0x40400000, 0x40800000, 0x00000000, 0x00000000, 0x00000000, 0x3f800000, 0x3f800000, 0x3f800000, 0x3f800000, 0x80000000, 0x80000000, 0x80000000, 0x3f800000]),
        ([2.0, 0.0, 0.0, 1.0, 0.0, 3.0, 0.0, 2.0, 0.0, 0.0, 4.0, 3.0, 0.0, 0.0, 0.0, 1.0], [0x3f800000, 0x40000000, 0x40400000, 0x00000000, 0x00000000, 0x00000000, 0x3f800000, 0x40000000, 0x40400000, 0x40800000, 0x80000000, 0x80000000, 0x80000000, 0x3f800000]),
        ([1.0, 0.5, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0], [0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x3ea69d46, 0x3f7210e2, 0x3fa3f07e, 0x3f47e0fa, 0x3f800001, 0x80000000, 0x80000000, 0xbee084d0, 0x3f6612d4]),
        ([0.866, 0.0, 0.5, 0.25, 0.0, 1.0, 0.0, -1.0, -0.5, 0.0, 0.866, 0.0, 0.0, 0.0, 0.0, 1.0], [0x3e800000, 0xbf800000, 0x00000000, 0x00000000, 0x3e8484bc, 0x00000000, 0x3f7746ce, 0x3f7ffe8e, 0x3f7ffffe, 0x3f7ffe90, 0x80000000, 0x80000000, 0x80000000, 0x3f800000]),
        ([1.7320508, 0.0, 1.0, 0.0, 0.0, 0.5, 0.0, 0.0, -1.0, 0.0, 1.7320508, 0.0, 0.0, 0.0, 0.0, 1.0], [0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x3e8483ee, 0x00000000, 0x3f7746e9, 0x3ffffffe, 0x3efffffe, 0x3fffffff, 0x80000000, 0x80000000, 0x80000000, 0x3f800000]),
        ([2.0, 0.0, 0.0, 4.0, 0.0, 2.0, 0.0, 6.0, 0.0, 0.0, 2.0, 8.0, 0.0, 0.0, 0.0, 2.0], [0x40000000, 0x40400000, 0x40800000, 0x00000000, 0x00000000, 0x00000000, 0x3f800000, 0x3f800000, 0x3f800000, 0x3f800000, 0x80000000, 0x80000000, 0x80000000, 0x3f800000]),
        ([0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0], [0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x3f800000, 0x00000000, 0x3f800000, 0x3f800000, 0x80000000, 0x80000000, 0x80000000, 0x3f800000]),
        ([1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0], [0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x3f800000, 0x3f800000, 0x00000000, 0x3f800000, 0x80000000, 0x80000000, 0x80000000, 0x3f800000]),
        ([-1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0], [0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x3f800000, 0x00000000, 0x00000000, 0x3f800000, 0x3f800000, 0xbf800000, 0x80000000, 0x80000000, 0x80000000, 0x3f800000]),
        ([0.3, -1.2, 0.7, 5.0, 0.9, 0.1, -0.4, -2.0, -0.2, 0.8, 1.5, 0.5, 0.0, 0.0, 0.0, 1.0], [0x40a00000, 0xc0000000, 0x3f000000, 0x3e0ac102, 0xbdab1cf6, 0x3f2b6437, 0x3f39be02, 0x3f5ce0e6, 0x3fb63349, 0x3fe383ff, 0xbe50a6dd, 0xbdda358e, 0x3debe160, 0xbf7763c6]),
        ([0.0, -1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0], [0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x3f3504f3, 0x3f3504f3, 0x3f7fffff, 0x3f7fffff, 0x3f800000, 0x80000000, 0x80000000, 0x80000000, 0x3f800000]),
        ([0.1, 0.2, 0.3, 0.0, 0.4, 0.5, 0.6, 0.0, 0.7, 0.8, 0.9, 0.0, 0.0, 0.0, 0.0, 1.0], [0x00000000, 0x00000000, 0x00000000, 0x3f1601ea, 0x3f19c378, 0x3e36a884, 0xbf038cac, 0x3fd7a7de, 0x3ddab943, 0x328c0000, 0xbf0b6390, 0x3db58278, 0xbf009357, 0x3f2a7884]),
        ([3.0, 1.0, 0.0, 0.0, 1.0, 3.0, 0.0, 0.0, 0.0, 0.0, 0.001, 0.0, 0.0, 0.0, 0.0, 1.0], [0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x3ec3ef16, 0x3f6c835f, 0x40800000, 0x40000001, 0x3a83126f, 0x80000000, 0x80000000, 0xbec3ef16, 0x3f6c835e]),
    ];
    const ROT_ORACLE: &[(RotIn, [u32; 4])] = &[
        (RotIn::Quat([1.0, 2.0, 3.0, 4.0]), [0x3e3af4ba, 0x3ebaf4ba, 0x3f0c378c, 0x3f3af4ba]),
        (RotIn::Quat([0.0, 0.0, 0.0, 2.0]), [0x00000000, 0x00000000, 0x00000000, 0x3f800000]),
        (RotIn::Quat([0.1, -0.2, 0.3, -0.9]), [0x3dd21ed0, 0xbe521ed0, 0x3e9d971c, 0xbf6c62aa]),
        (RotIn::Axis(1.5707964, [0.0, 1.0, 0.0]), [0x00000000, 0x3f3504f3, 0x00000000, 0x3f3504f3]),
        (RotIn::Axis(-1.0, [1.0, 1.0, 0.0]), [0x3ef57741, 0x3ef57741, 0x00000000, 0xbf60a941]),
        (RotIn::Axis(7.0, [0.0, 0.0, 1.0]), [0x00000000, 0x00000000, 0x3eb399d9, 0x3f6fbba1]),
        (RotIn::Axis(3.5, [0.6, 0.0, 0.8]), [0x3f1723e7, 0x00000000, 0x3f498534, 0xbe368618]),
        (RotIn::Axis(-20.0, [1.0, 0.0, 0.0]), [0x3f0b44f3, 0x00000000, 0x00000000, 0xbf56cd68]),
        (RotIn::Axis(0.0, [0.0, 1.0, 0.0]), [0x00000000, 0x00000000, 0x00000000, 0x3f800000]),
    ];
}
