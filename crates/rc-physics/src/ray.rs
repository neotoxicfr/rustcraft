//! Ray cast of the objects (`docs/specs/interactions.md` § 3.5): walk of the blocks between two
//! points, outline of each block and, depending on the mode, the shape of its fluid. Shared by the client
//! (prediction) and the integrated server (boats, buckets, bottles, lily pads, frog spawn).

use rc_data::Aabb;

/// Fluids that the ray can touch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fluids {
    /// None (full bucket).
    None,
    /// Sources only (empty bucket, bottle, lily pad, frog spawn).
    SourceOnly,
    /// Any non-empty fluid (boats and rafts).
    Any,
}

/// Faces: 0 down, 1 up, 2 north, 3 south, 4 west, 5 east (order of the game's directions).
pub const FACES: [[i32; 3]; 6] = [[0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]];

/// Result: block (or fluid) hit, or nothing (end point, approached face, block that
/// contains it — used as is by lily pads).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RayHit {
    pub miss: bool,
    pub pos: [i32; 3],
    pub face: u8,
    pub at: [f64; 3],
    pub inside: bool,
}

/// World as seen by the ray.
pub trait RayWorld {
    /// Outline of the block (selection shape).
    fn outline(&self, p: [i32; 3]) -> &[Aabb];
    /// Fluid of the block: `None` if empty, otherwise (source, height of its shape).
    fn fluid(&self, p: [i32; 3]) -> Option<(bool, f64)>;
    /// Interaction shape of the block (`interaction_shape`), empty for almost all.
    fn interaction(&self, _p: [i32; 3]) -> &[Aabb] {
        &[]
    }
}

const fn px(x0: f64, y0: f64, z0: f64, x1: f64, y1: f64, z1: f64) -> Aabb {
    Aabb { min: [x0 / 16.0, y0 / 16.0, z0 / 16.0], max: [x1 / 16.0, y1 / 16.0, z1 / 16.0] }
}

const CUBE: [Aabb; 1] = [px(0.0, 0.0, 0.0, 16.0, 16.0, 16.0)];
const CAULDRON_INSIDE: [Aabb; 1] = [px(2.0, 4.0, 2.0, 14.0, 16.0, 14.0)];
const HOPPER_INSIDE: Aabb = px(2.0, 11.0, 2.0, 14.0, 16.0, 14.0);
/// Interior of the hopper, plus its side spout: north, south, west, east.
const HOPPER_SIDE: [[Aabb; 2]; 4] = [
    [HOPPER_INSIDE, px(6.0, 8.0, 0.0, 10.0, 10.0, 4.0)],
    [HOPPER_INSIDE, px(6.0, 8.0, 12.0, 10.0, 10.0, 16.0)],
    [HOPPER_INSIDE, px(0.0, 8.0, 6.0, 4.0, 10.0, 10.0)],
    [HOPPER_INSIDE, px(12.0, 8.0, 6.0, 16.0, 10.0, 10.0)],
];

/// Second shape whose face a ray takes when it hits it before the outline: interior of the
/// cauldrons, cube of the composter and the scaffolding, interior and spout of the hopper. Empty for
/// the other blocks (only looked up once per ray, at the block hit).
pub fn interaction_shape(data: &rc_data::VersionData, state: u32) -> &'static [Aabb] {
    let Some(block) = data.block_of(state) else { return &[] };
    let name = block.name.strip_prefix("minecraft:").unwrap_or(&block.name);
    match name {
        "cauldron" | "water_cauldron" | "lava_cauldron" | "powder_snow_cauldron" => &CAULDRON_INSIDE,
        "composter" | "scaffolding" => &CUBE,
        "hopper" => match data.state_properties(state).find(|p| p.0 == "facing").map(|p| p.1) {
            Some("north") => &HOPPER_SIDE[0],
            Some("south") => &HOPPER_SIDE[1],
            Some("west") => &HOPPER_SIDE[2],
            Some("east") => &HOPPER_SIDE[3],
            _ => std::slice::from_ref(&HOPPER_INSIDE),
        },
        _ => &[],
    }
}

/// Outline hit, with the face of the interaction shape if that one is hit closer
/// (point and "inside" unchanged).
pub fn with_interaction_face(hit: Option<RayHit>, interaction: &[Aabb], from: [f64; 3], to: [f64; 3]) -> Option<RayHit> {
    let mut hit = hit?;
    if let Some(i) = clip_shape(interaction, from, to, hit.pos)
        && dist2(i.at, from) < dist2(hit.at, from)
    {
        hit.face = i.face;
    }
    Some(hit)
}

/// Face whose normal is the most aligned with `d` (first maximum in the face order).
pub fn nearest_face(d: [f64; 3]) -> u8 {
    let mut best = (2u8, f64::NEG_INFINITY);
    for (f, n) in FACES.iter().enumerate() {
        let v = d[0] * f64::from(n[0]) + d[1] * f64::from(n[1]) + d[2] * f64::from(n[2]);
        if v > best.1 {
            best = (f as u8, v);
        }
    }
    best.0
}

/// View vector of a pitch and a yaw in degrees (the game's single-precision computation).
pub fn view_vector(pitch: f32, yaw: f32) -> [f64; 3] {
    let rx = pitch * (std::f32::consts::PI / 180.0);
    let ry = -yaw * (std::f32::consts::PI / 180.0);
    let (yc, ys) = (crate::cos_t(ry), crate::sin_t(ry));
    let (xc, xs) = (crate::cos_t(rx), crate::sin_t(rx));
    [f64::from(ys * xc), f64::from(-xs), f64::from(yc * xc)]
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dist2(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = sub(a, b);
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
}

/// Entry of the segment through a plane of axis `a` (value `plane`) within the bounds of the other two axes.
fn clip_plane(s: &mut f64, d: [f64; 3], from: [f64; 3], a: usize, plane: f64, b: &Aabb) -> bool {
    let t = (plane - from[a]) / d[a];
    let (u, v) = ((a + 1) % 3, (a + 2) % 3);
    let pu = from[u] + t * d[u];
    let pv = from[v] + t * d[v];
    const E: f64 = 1.0e-7;
    if 0.0 < t && t < *s && b.min[u] - E < pu && pu < b.max[u] + E && b.min[v] - E < pv && pv < b.max[v] + E {
        *s = t;
        return true;
    }
    false
}

/// Best entry in a box: minimal faces if the segment advances, maximal otherwise.
fn box_face(b: &Aabb, from: [f64; 3], s: &mut f64, mut face: Option<u8>, d: [f64; 3]) -> Option<u8> {
    const E: f64 = 1.0e-7;
    // Game order: x, y, z; entry face = side opposite to the direction of travel.
    let sides: [(usize, u8, u8); 3] = [(0, 4, 5), (1, 0, 1), (2, 2, 3)];
    for (a, low, high) in sides {
        if d[a] > E {
            if clip_plane(s, d, from, a, b.min[a], b) {
                face = Some(low);
            }
        } else if d[a] < -E && clip_plane(s, d, from, a, b.max[a], b) {
            face = Some(high);
        }
    }
    face
}

fn contains(b: &Aabb, p: [f64; 3]) -> bool {
    (0..3).all(|i| p[i] >= b.min[i] && p[i] < b.max[i])
}

fn offset(b: &Aabb, pos: [i32; 3]) -> Aabb {
    let o = pos.map(f64::from);
    Aabb {
        min: [b.min[0] + o[0], b.min[1] + o[1], b.min[2] + o[2]],
        max: [b.max[0] + o[0], b.max[1] + o[1], b.max[2] + o[2]],
    }
}

/// Segment against a shape placed at block `pos` (test point at 0.001 from the start: "inside").
pub fn clip_shape(boxes: &[Aabb], from: [f64; 3], to: [f64; 3], pos: [i32; 3]) -> Option<RayHit> {
    if boxes.is_empty() {
        return None;
    }
    let d = sub(to, from);
    if d[0] * d[0] + d[1] * d[1] + d[2] * d[2] < 1.0e-7 {
        return None;
    }
    let test = [from[0] + d[0] * 0.001, from[1] + d[1] * 0.001, from[2] + d[2] * 0.001];
    if boxes.iter().any(|b| contains(&offset(b, pos), test)) {
        return Some(RayHit { miss: false, pos, face: nearest_face(d) ^ 1, at: test, inside: true });
    }
    let mut s = 1.0;
    let mut face = None;
    for b in boxes {
        face = box_face(&offset(b, pos), from, &mut s, face, d);
    }
    face.map(|face| RayHit {
        miss: false,
        pos,
        face,
        at: [from[0] + s * d[0], from[1] + s * d[1], from[2] + s * d[2]],
        inside: false,
    })
}

/// Ray from `from` to `to`: for each block crossed, the nearest of the outline and the fluid
/// (tie: the block), the first block hit wins.
pub fn clip(w: &dyn RayWorld, from: [f64; 3], to: [f64; 3], mode: Fluids) -> RayHit {
    let cell = |c: [i32; 3]| -> Option<RayHit> {
        let block = clip_shape(w.outline(c), from, to, c);
        let block = block.and_then(|b| with_interaction_face(Some(b), w.interaction(c), from, to));
        let liquid = match (mode, w.fluid(c)) {
            (Fluids::None, _) | (_, None) | (Fluids::SourceOnly, Some((false, _))) => None,
            (_, Some((_, h))) => clip_shape(&[Aabb { min: [0.0; 3], max: [1.0, h, 1.0] }], from, to, c),
        };
        let bd = block.map_or(f64::MAX, |h| dist2(from, h.at));
        let ld = liquid.map_or(f64::MAX, |h| dist2(from, h.at));
        if bd <= ld { block } else { liquid }
    };
    let miss = || {
        let d = sub(from, to);
        RayHit { miss: true, pos: to.map(|v| v.floor() as i32), face: nearest_face(d), at: to, inside: false }
    };
    if from == to {
        return miss();
    }
    let lerp = |t: f64, a: f64, b: f64| a + t * (b - a);
    let t = [lerp(-1.0e-7, to[0], from[0]), lerp(-1.0e-7, to[1], from[1]), lerp(-1.0e-7, to[2], from[2])];
    let f = [lerp(-1.0e-7, from[0], to[0]), lerp(-1.0e-7, from[1], to[1]), lerp(-1.0e-7, from[2], to[2])];
    let mut c = f.map(|v| v.floor() as i32);
    if let Some(h) = cell(c) {
        return h;
    }
    let d = sub(t, f);
    let sign = d.map(|v| {
        if v > 0.0 {
            1
        } else if v < 0.0 {
            -1
        } else {
            0
        }
    });
    let delta: [f64; 3] = std::array::from_fn(|i| if sign[i] == 0 { f64::MAX } else { f64::from(sign[i]) / d[i] });
    let frac = |v: f64| v - v.floor();
    let mut tt: [f64; 3] = std::array::from_fn(|i| delta[i] * if sign[i] > 0 { 1.0 - frac(f[i]) } else { frac(f[i]) });
    while tt[0] <= 1.0 || tt[1] <= 1.0 || tt[2] <= 1.0 {
        let axis = if tt[0] < tt[1] {
            if tt[0] < tt[2] { 0 } else { 2 }
        } else if tt[1] < tt[2] {
            1
        } else {
            2
        };
        c[axis] += sign[axis];
        tt[axis] += delta[axis];
        if let Some(h) = cell(c) {
            return h;
        }
    }
    miss()
}

/// Ray of the objects from the eyes, along the view, over the block interaction range.
pub fn item_clip(w: &dyn RayWorld, eye: [f64; 3], pitch: f32, yaw: f32, range: f64, mode: Fluids) -> RayHit {
    let v = view_vector(pitch, yaw);
    let to = [eye[0] + v[0] * range, eye[1] + v[1] * range, eye[2] + v[2] * range];
    clip(w, eye, to, mode)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: [Aabb; 1] = [Aabb { min: [0.0; 3], max: [1.0; 3] }];

    /// Basin: stone up to y = 61, source water of height 8/9 at y = 62 for z ≥ 2.
    struct Pond {
        depth_top: i32,
    }

    impl RayWorld for Pond {
        fn outline(&self, p: [i32; 3]) -> &[Aabb] {
            if p[1] < self.depth_top || (p[1] <= 62 && p[2] < 2) { &FULL } else { &[] }
        }
        fn fluid(&self, p: [i32; 3]) -> Option<(bool, f64)> {
            (p[2] >= 2 && (self.depth_top..=62).contains(&p[1])).then_some((true, 8.0 / 9.0))
        }
    }

    #[test]
    fn fluid_modes() {
        let w = Pond { depth_top: 60 };
        let eye = [0.5, 64.62, 0.5];
        // Towards the basin, 35° downwards: water in "all" mode, the bottom or nothing otherwise.
        let any = item_clip(&w, eye, 35.0, 0.0, 4.5, Fluids::Any);
        assert!(!any.miss && any.pos[1] == 62 && any.face == 1, "{any:?}");
        assert!((any.at[1] - (62.0 + 8.0 / 9.0)).abs() < 1e-6);
        let src = item_clip(&w, eye, 35.0, 0.0, 4.5, Fluids::SourceOnly);
        assert_eq!(src.pos, any.pos);
        let none = item_clip(&w, eye, 35.0, 0.0, 4.5, Fluids::None);
        assert!(none.miss, "{none:?}");
    }

    #[test]
    fn miss_reports_end_block_and_face() {
        let w = Pond { depth_top: -100 };
        let h = item_clip(&w, [0.5, 200.0, 0.5], 0.0, 0.0, 4.5, Fluids::Any);
        assert!(h.miss);
        assert_eq!(h.pos, [0, 200, 5]);
        assert_eq!(h.face, 2); // towards the start: north
    }

    /// Cauldron at (0, 0, 0): four walls and a 4-pixel bottom, open interior.
    struct Cauldron {
        interaction: bool,
    }

    const WALLS: [Aabb; 5] = [
        px(0.0, 0.0, 0.0, 2.0, 16.0, 16.0),
        px(14.0, 0.0, 0.0, 16.0, 16.0, 16.0),
        px(2.0, 0.0, 0.0, 14.0, 16.0, 2.0),
        px(2.0, 0.0, 14.0, 14.0, 16.0, 16.0),
        px(2.0, 0.0, 2.0, 14.0, 4.0, 14.0),
    ];

    impl RayWorld for Cauldron {
        fn outline(&self, p: [i32; 3]) -> &[Aabb] {
            if p == [0, 0, 0] { &WALLS } else { &[] }
        }
        fn fluid(&self, _p: [i32; 3]) -> Option<(bool, f64)> {
            None
        }
        fn interaction(&self, p: [i32; 3]) -> &[Aabb] {
            if self.interaction && p == [0, 0, 0] { &CAULDRON_INSIDE } else { &[] }
        }
    }

    #[test]
    fn interaction_shape_gives_its_face() {
        // Ray that enters through the opening then hits the east wall: point on the wall, top face
        // (that of the interior, hit earlier).
        let (from, to) = ([0.3, 1.5, 0.5], [0.95, 0.5, 0.5]);
        let h = clip(&Cauldron { interaction: true }, from, to, Fluids::None);
        assert_eq!((h.miss, h.pos, h.face, h.inside), (false, [0, 0, 0], 1, false), "{h:?}");
        assert!((h.at[0] - 0.875).abs() < 1e-9 && (h.at[1] - (1.5 - 0.575 / 0.65)).abs() < 1e-9, "{h:?}");
        // Without interaction shape: west face of the wall.
        let h = clip(&Cauldron { interaction: false }, from, to, Fluids::None);
        assert_eq!(h.face, 4);
        // Ray that hits the wall from the outside: the interior is not closer, face kept.
        let h = clip(&Cauldron { interaction: true }, [-1.0, 0.5, 0.5], [2.0, 0.5, 0.5], Fluids::None);
        assert_eq!((h.face, h.at[0]), (4, 0.0));
    }

    #[test]
    fn interaction_shapes_of_the_game_blocks() {
        let d = rc_testdata::version_data().expect("version data absent (extraction)");
        let state = |name: &str, facing: Option<&str>| -> u32 {
            let b = d.blocks.iter().find(|b| b.name == name).expect(name);
            (b.first_state..b.first_state + b.state_count)
                .find(|&s| facing.is_none_or(|f| d.state_properties(s).any(|p| p == ("facing", f))))
                .expect("state")
        };
        assert_eq!(interaction_shape(&d, state("minecraft:water_cauldron", None)), &CAULDRON_INSIDE);
        assert_eq!(interaction_shape(&d, state("minecraft:composter", None)), &CUBE);
        assert_eq!(interaction_shape(&d, state("minecraft:scaffolding", None)), &CUBE);
        assert_eq!(interaction_shape(&d, state("minecraft:hopper", Some("down"))), &[HOPPER_INSIDE]);
        let east = interaction_shape(&d, state("minecraft:hopper", Some("east")));
        assert_eq!(east[1], px(12.0, 8.0, 6.0, 16.0, 10.0, 10.0));
        assert!(interaction_shape(&d, state("minecraft:stone", None)).is_empty());
    }

    #[test]
    fn flowing_water_ignored_by_source_only() {
        struct Flow;
        impl RayWorld for Flow {
            fn outline(&self, p: [i32; 3]) -> &[Aabb] {
                if p[1] < 60 { &FULL } else { &[] }
            }
            fn fluid(&self, p: [i32; 3]) -> Option<(bool, f64)> {
                (p[1] == 60).then_some((false, 0.5))
            }
        }
        let h = item_clip(&Flow, [0.5, 62.0, 0.5], 90.0, 0.0, 4.5, Fluids::SourceOnly);
        assert_eq!((h.miss, h.pos), (false, [0, 59, 0]));
        let h = item_clip(&Flow, [0.5, 62.0, 0.5], 90.0, 0.0, 4.5, Fluids::Any);
        assert_eq!((h.pos, h.face), ([0, 60, 0], 1));
        assert!((h.at[1] - 60.5).abs() < 1e-9);
    }
}
