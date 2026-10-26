//! Crosshair targeting (`docs/specs/targeting.md`): block or entity under the crosshair, within reach.

use rc_data::Aabb;

/// Faces: 0 down, 1 up, 2 north, 3 south, 4 west, 5 east (the game's direction order).
pub const DOWN: u8 = 0;
pub const UP: u8 = 1;
pub const NORTH: u8 = 2;
pub const SOUTH: u8 = 3;
pub const WEST: u8 = 4;
pub const EAST: u8 = 5;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hit {
    Miss,
    Block { pos: [i32; 3], face: u8, at: [f64; 3], inside: bool },
    Entity { id: i32, at: [f64; 3] },
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dist2(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = sub(a, b);
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
}

/// Face closest to a vector (first maximum of the dot product, face order).
fn nearest_face(d: [f64; 3]) -> u8 {
    let normals: [(u8, [f64; 3]); 6] = [
        (DOWN, [0.0, -1.0, 0.0]),
        (UP, [0.0, 1.0, 0.0]),
        (NORTH, [0.0, 0.0, -1.0]),
        (SOUTH, [0.0, 0.0, 1.0]),
        (WEST, [-1.0, 0.0, 0.0]),
        (EAST, [1.0, 0.0, 0.0]),
    ];
    let mut best = (NORTH, f64::NEG_INFINITY);
    for (f, n) in normals {
        let v = d[0] * n[0] + d[1] * n[1] + d[2] * n[2];
        if v > best.1 {
            best = (f, v);
        }
    }
    best.0
}

fn opposite(f: u8) -> u8 {
    f ^ 1
}

/// Entry of a segment into a box (min faces if delta > 0, max otherwise): updates the
/// best fraction `s` and returns the entry face.
#[allow(clippy::too_many_arguments)]
fn clip_point(
    s_best: &mut f64,
    face: Option<u8>,
    da: f64,
    db: f64,
    dc: f64,
    plane: f64,
    (min_b, max_b): (f64, f64),
    (min_c, max_c): (f64, f64),
    new_face: u8,
    (fa, fb, fc): (f64, f64, f64),
) -> Option<u8> {
    let s = (plane - fa) / da;
    let pb = fb + s * db;
    let pc = fc + s * dc;
    const E: f64 = 1.0e-7;
    if 0.0 < s && s < *s_best && min_b - E < pb && pb < max_b + E && min_c - E < pc && pc < max_c + E {
        *s_best = s;
        Some(new_face)
    } else {
        face
    }
}

fn box_face(b: &Aabb, from: [f64; 3], s: &mut f64, mut face: Option<u8>, d: [f64; 3]) -> Option<u8> {
    const E: f64 = 1.0e-7;
    let (x, y, z) = (from[0], from[1], from[2]);
    if d[0] > E {
        face = clip_point(
            s,
            face,
            d[0],
            d[1],
            d[2],
            b.min[0],
            (b.min[1], b.max[1]),
            (b.min[2], b.max[2]),
            WEST,
            (x, y, z),
        );
    } else if d[0] < -E {
        face = clip_point(
            s,
            face,
            d[0],
            d[1],
            d[2],
            b.max[0],
            (b.min[1], b.max[1]),
            (b.min[2], b.max[2]),
            EAST,
            (x, y, z),
        );
    }
    if d[1] > E {
        face = clip_point(
            s,
            face,
            d[1],
            d[2],
            d[0],
            b.min[1],
            (b.min[2], b.max[2]),
            (b.min[0], b.max[0]),
            DOWN,
            (y, z, x),
        );
    } else if d[1] < -E {
        face =
            clip_point(s, face, d[1], d[2], d[0], b.max[1], (b.min[2], b.max[2]), (b.min[0], b.max[0]), UP, (y, z, x));
    }
    if d[2] > E {
        face = clip_point(
            s,
            face,
            d[2],
            d[0],
            d[1],
            b.min[2],
            (b.min[0], b.max[0]),
            (b.min[1], b.max[1]),
            NORTH,
            (z, x, y),
        );
    } else if d[2] < -E {
        face = clip_point(
            s,
            face,
            d[2],
            d[0],
            d[1],
            b.max[2],
            (b.min[0], b.max[0]),
            (b.min[1], b.max[1]),
            SOUTH,
            (z, x, y),
        );
    }
    face
}

fn contains(b: &Aabb, p: [f64; 3]) -> bool {
    (0..3).all(|i| p[i] >= b.min[i] && p[i] < b.max[i])
}

/// Shape box offset by `shift` (random block offset) then placed at block `pos`, in
/// the game's addition order (shape moved, then block).
fn offset(b: &Aabb, pos: [i32; 3], shift: [f64; 3]) -> Aabb {
    let o = pos.map(f64::from);
    Aabb { min: std::array::from_fn(|i| b.min[i] + shift[i] + o[i]), max: std::array::from_fn(|i| b.max[i] + shift[i] + o[i]) }
}

/// Shape of a block for targeting: boxes of the shape and offset to apply to them.
pub type Shape<'a> = (&'a [Aabb], [f64; 3]);

/// Shape without offset.
pub fn plain(boxes: &[Aabb]) -> Shape<'_> {
    (boxes, [0.0; 3])
}

/// Segment against a shape placed at block `pos`.
fn clip_shape((boxes, shift): Shape<'_>, from: [f64; 3], to: [f64; 3], pos: [i32; 3]) -> Option<Hit> {
    if boxes.is_empty() {
        return None;
    }
    let d = sub(to, from);
    if d[0] * d[0] + d[1] * d[1] + d[2] * d[2] < 1.0e-7 {
        return None;
    }
    let test = [from[0] + d[0] * 0.001, from[1] + d[1] * 0.001, from[2] + d[2] * 0.001];
    if boxes.iter().any(|b| contains(&offset(b, pos, shift), test)) {
        return Some(Hit::Block { pos, face: opposite(nearest_face(d)), at: test, inside: true });
    }
    let mut s = 1.0;
    let mut face = None;
    for b in boxes {
        face = box_face(&offset(b, pos, shift), from, &mut s, face, d);
    }
    face.map(|face| Hit::Block {
        pos,
        face,
        at: [from[0] + s * d[0], from[1] + s * d[1], from[2] + s * d[2]],
        inside: false,
    })
}

/// Interaction shape of a block (`rc_physics::ray::interaction_shape`): its face replaces that of the
/// outline when hit closer.
pub type Interaction<'a, 'b> = &'b dyn Fn(i32, i32, i32) -> &'a [Aabb];

/// No interaction shape (rays where only the position matters).
pub fn no_interaction<'a>(_: i32, _: i32, _: i32) -> &'a [Aabb] {
    &[]
}

/// Walk over the blocks from `from` to `to` (outline shape of each block).
pub fn clip_blocks<'a>(from: [f64; 3], to: [f64; 3], shape: &dyn Fn(i32, i32, i32) -> Shape<'a>) -> Option<Hit> {
    clip_blocks_with(from, to, shape, &no_interaction)
}

/// Outline hit `h`, with the face of the interaction shape hit closer.
fn interaction_face(h: Hit, boxes: &[Aabb], from: [f64; 3], to: [f64; 3]) -> Hit {
    match h {
        Hit::Block { pos, face, at, inside } => {
            let face = match clip_shape(plain(boxes), from, to, pos) {
                Some(Hit::Block { face: f, at: i, .. }) if dist2(i, from) < dist2(at, from) => f,
                _ => face,
            };
            Hit::Block { pos, face, at, inside }
        }
        other => other,
    }
}

/// Walk over the blocks, interaction shape included (crosshair targeting).
pub fn clip_blocks_with<'a>(
    from: [f64; 3],
    to: [f64; 3],
    shape: &dyn Fn(i32, i32, i32) -> Shape<'a>,
    interaction: Interaction<'a, '_>,
) -> Option<Hit> {
    if from == to {
        return None;
    }
    let lerp = |t: f64, a: f64, b: f64| a + t * (b - a);
    let t = [lerp(-1.0e-7, to[0], from[0]), lerp(-1.0e-7, to[1], from[1]), lerp(-1.0e-7, to[2], from[2])];
    let f = [lerp(-1.0e-7, from[0], to[0]), lerp(-1.0e-7, from[1], to[1]), lerp(-1.0e-7, from[2], to[2])];
    let mut c = f.map(|v| v.floor() as i32);
    let test = |c: [i32; 3]| {
        clip_shape(shape(c[0], c[1], c[2]), from, to, c)
            .map(|h| interaction_face(h, interaction(c[0], c[1], c[2]), from, to))
    };
    if let Some(h) = test(c) {
        return Some(h);
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
        if let Some(h) = test(c) {
            return Some(h);
        }
    }
    None
}

/// Candidate entity: id, box, pick radius (box inflation for normal
/// targeting), same root mount as the camera.
pub struct Candidate {
    pub id: i32,
    pub bb: Aabb,
    pub radius: f64,
    pub same_vehicle: bool,
}

impl Candidate {
    fn picked_box(&self) -> Aabb {
        let r = self.radius;
        Aabb { min: self.bb.min.map(|v| v - r), max: self.bb.max.map(|v| v + r) }
    }
}

/// Entry point of the segment into a box.
fn clip_box(b: &Aabb, from: [f64; 3], to: [f64; 3]) -> Option<[f64; 3]> {
    let d = sub(to, from);
    let mut s = 1.0;
    box_face(b, from, &mut s, None, d).map(|_| [from[0] + s * d[0], from[1] + s * d[1], from[2] + s * d[2]])
}

/// Full targeting: eyes `eye`, unit view vector `dir`, block and entity ranges.
pub fn pick<'a>(
    eye: [f64; 3],
    dir: [f64; 3],
    block_range: f64,
    entity_range: f64,
    shape: &dyn Fn(i32, i32, i32) -> Shape<'a>,
    interaction: Interaction<'a, '_>,
    entities: impl Iterator<Item = Candidate>,
) -> Hit {
    let mut max = block_range.max(entity_range);
    let mut max2 = max * max;
    let end = [eye[0] + dir[0] * max, eye[1] + dir[1] * max, eye[2] + dir[2] * max];
    let block = clip_blocks_with(eye, end, shape, interaction);
    let block_at = match block {
        Some(Hit::Block { at, .. }) => at,
        _ => end,
    };
    let block_d2 = dist2(block_at, eye);
    if block.is_some() {
        max2 = block_d2;
        max = max2.sqrt();
    }
    let to = [eye[0] + dir[0] * max, eye[1] + dir[1] * max, eye[2] + dir[2] * max];
    let mut nearest = max2;
    let mut hovered: Option<(i32, [f64; 3])> = None;
    for c in entities {
        let bb = c.picked_box();
        let clip = clip_box(&bb, eye, to);
        if contains(&bb, eye) {
            if nearest >= 0.0 {
                hovered = Some((c.id, clip.unwrap_or(eye)));
                nearest = 0.0;
            }
        } else if let Some(p) = clip {
            let dd = dist2(p, eye);
            if dd < nearest || nearest == 0.0 {
                if c.same_vehicle {
                    if nearest == 0.0 {
                        hovered = Some((c.id, p));
                    }
                } else {
                    hovered = Some((c.id, p));
                    nearest = dd;
                }
            }
        }
    }
    let filter = |h: Hit, at: [f64; 3], range: f64| if dist2(at, eye) < range * range { h } else { Hit::Miss };
    match hovered {
        Some((id, at)) if dist2(at, eye) < block_d2 => filter(Hit::Entity { id, at }, at, entity_range),
        _ => match block {
            Some(h) => filter(h, block_at, block_range),
            None => Hit::Miss,
        },
    }
}

/// Attack range of an item (`attack_range`): minimum and maximum ranges of the game mode,
/// box margin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AttackReach {
    pub min: f64,
    pub max: f64,
    pub margin: f64,
}

/// Targeting of an item within attack range, tried before normal targeting: gaze segment from
/// `min` to `max` (lengthened by the part of the movement `along` in the gaze direction), cut at the first
/// block or at the world border `border`; a block closer than `min` is returned as is.
/// Entities hit on the segment (exact box, or box inflated by the margin then brought back onto
/// its surface if no block separates it from it, or the start of the segment inside the box): the closest
/// to the eyes. Otherwise the possible block, at most `block_range` from the eyes; `Miss` leaves room for
/// normal targeting.
#[allow(clippy::too_many_arguments)]
pub fn pick_attack<'a>(
    eye: [f64; 3],
    dir: [f64; 3],
    reach: AttackReach,
    along: f64,
    block_range: f64,
    border: [f64; 4],
    shape: &dyn Fn(i32, i32, i32) -> Shape<'a>,
    interaction: Interaction<'a, '_>,
    entities: impl Iterator<Item = Candidate>,
) -> Hit {
    let at = |k: f64| [eye[0] + dir[0] * k, eye[1] + dir[1] * k, eye[2] + dir[2] * k];
    let from = at(reach.min);
    let mut to = at(reach.max + along.max(0.0));
    let block = clip_with_border(eye, to, shape, interaction, border);
    if let Some(Hit::Block { at: hit, .. }) = block {
        to = hit;
        if dist2(eye, to) < dist2(eye, from) {
            return filter_block(block, eye, block_range);
        }
    }
    let m = reach.margin;
    let mut best: Option<(f64, i32, [f64; 3])> = None;
    for c in entities {
        let bb = c.bb;
        let p = if contains(&bb, from) {
            Some(from)
        } else if let Some(p) = clip_box(&bb, from, to) {
            Some(p)
        } else if m > 0.0 {
            let wide = Aabb { min: bb.min.map(|v| v - m), max: bb.max.map(|v| v + m) };
            clip_box(&wide, from, to).and_then(|outside| {
                let centre: [f64; 3] = std::array::from_fn(|i| (bb.min[i] + bb.max[i]) / 2.0);
                let towards = match clip_with_border(outside, centre, shape, interaction, border) {
                    Some(Hit::Block { at, .. }) => at,
                    _ => centre,
                };
                clip_box(&bb, outside, towards)
            })
        } else {
            None
        };
        if let Some(p) = p {
            let d = dist2(eye, p);
            if best.is_none_or(|b| d < b.0) {
                best = Some((d, c.id, p));
            }
        }
    }
    match best {
        Some((_, id, at)) => Hit::Entity { id, at },
        None => filter_block(block, eye, block_range),
    }
}

/// Segment cut at the blocks then at the border `b` (x min, x max, z min, z max): start inside the
/// bounds and hit point (or end of the segment) outside → border block, at the point brought back into the
/// bounds (minus 1e-5 in single float on the maximum side), face according to the segment direction.
fn clip_with_border<'a>(
    from: [f64; 3],
    to: [f64; 3],
    shape: &dyn Fn(i32, i32, i32) -> Shape<'a>,
    interaction: Interaction<'a, '_>,
    b: [f64; 4],
) -> Option<Hit> {
    let hit = clip_blocks_with(from, to, shape, interaction);
    let end = match hit {
        Some(Hit::Block { at, .. }) => at,
        _ => to,
    };
    let within = |p: [f64; 3]| p[0] >= b[0] && p[0] < b[1] && p[2] >= b[2] && p[2] < b[3];
    if !within(from) || within(end) {
        return hit;
    }
    let e = f64::from(1.0e-5_f32);
    let p = [end[0].max(b[0]).min(b[1] - e), end[1], end[2].max(b[2]).min(b[3] - e)];
    Some(Hit::Block { pos: p.map(|v| v.floor() as i32), face: nearest_face(sub(end, from)), at: p, inside: false })
}

/// Targeting of an item along the gaze (brush): blocks from `eye` to `to` (shapes given, border
/// `border` included), then the entity whose exact box cuts the segment shortened to the block, the
/// closest one (start inside a box: ignored). Otherwise the block, or `Miss`.
pub fn pick_along<'a>(
    eye: [f64; 3],
    to: [f64; 3],
    shape: &dyn Fn(i32, i32, i32) -> Shape<'a>,
    interaction: Interaction<'a, '_>,
    border: [f64; 4],
    entities: impl Iterator<Item = Candidate>,
) -> Hit {
    let block = clip_with_border(eye, to, shape, interaction, border);
    let end = match block {
        Some(Hit::Block { at, .. }) => at,
        _ => to,
    };
    let mut best: Option<(f64, i32, [f64; 3])> = None;
    for c in entities {
        if contains(&c.bb, eye) {
            continue;
        }
        if let Some(p) = clip_box(&c.bb, eye, end) {
            let d = dist2(eye, p);
            if best.is_none_or(|b| d < b.0) {
                best = Some((d, c.id, p));
            }
        }
    }
    match (best, block) {
        (Some((_, id, at)), _) => Hit::Entity { id, at },
        (None, Some(h)) => h,
        (None, None) => Hit::Miss,
    }
}

/// Block returned by attack targeting: missed beyond the block range.
fn filter_block(block: Option<Hit>, eye: [f64; 3], range: f64) -> Hit {
    match block {
        Some(h @ Hit::Block { at, .. }) if dist2(eye, at) < range * range => h,
        _ => Hit::Miss,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: [Aabb; 1] = [Aabb { min: [0.0; 3], max: [1.0; 3] }];
    /// Distant border (no effect).
    const OPEN: [f64; 4] = [-3.0e7, 3.0e7, -3.0e7, 3.0e7];

    /// Spear, player 1 block from the border (z max = 1), cow 3.8 blocks behind: the segment
    /// stops at the border, block (0, 64, 0) south face returned (the old code targeted the cow).
    #[test]
    fn attack_range_stops_at_world_border() {
        let reach = AttackReach { min: 2.0, max: 4.5, margin: 0.125 };
        let cow = || Candidate {
            id: 7,
            bb: Aabb { min: [-0.45, 64.0, 3.8], max: [0.45, 65.4, 4.7] },
            radius: 0.0,
            same_vehicle: false,
        };
        let empty = |_: i32, _: i32, _: i32| plain(&[]);
        let eye = [0.0, 64.62, 0.0];
        let border = [-10.0, 10.0, -10.0, 1.0];
        let h = pick_attack(eye, [0.0, 0.0, 1.0], reach, 0.0, 4.5, border, &empty, &no_interaction, std::iter::once(cow()));
        match h {
            Hit::Block { pos: [0, 64, 0], face: SOUTH, at, inside: false } => {
                assert_eq!(at[2], 1.0 - f64::from(1.0e-5_f32), "{at:?}");
            }
            _ => panic!("{h:?}"),
        }
        // Start already outside the bounds: border ignored, the cow is targeted.
        let h = pick_attack([0.0, 64.62, 2.0], [0.0, 0.0, 1.0], reach, 0.0, 4.5, border, &empty, &no_interaction, std::iter::once(cow()));
        assert!(matches!(h, Hit::Entity { id: 7, .. }), "{h:?}");
    }
    const SLAB: [Aabb; 1] = [Aabb { min: [0.0; 3], max: [1.0, 0.5, 1.0] }];

    fn floor_at(y0: i32) -> impl Fn(i32, i32, i32) -> Shape<'static> {
        move |_, y, _| plain(if y <= y0 { &FULL[..] } else { &[] })
    }

    #[test]
    fn looks_down_at_floor() {
        let h = pick([0.5, 65.62, 0.5], [0.0, -1.0, 0.0], 4.5, 3.0, &floor_at(63), &no_interaction, std::iter::empty());
        match h {
            Hit::Block { pos, face, at, inside } => {
                assert_eq!((pos, face, inside), ([0, 63, 0], UP, false));
                assert!((at[1] - 64.0).abs() < 1e-9);
            }
            _ => panic!("{h:?}"),
        }
    }

    #[test]
    fn out_of_reach_is_miss() {
        let h = pick([0.5, 70.0, 0.5], [0.0, -1.0, 0.0], 4.5, 3.0, &floor_at(63), &no_interaction, std::iter::empty());
        assert_eq!(h, Hit::Miss);
    }

    #[test]
    fn slab_top_face() {
        let shape = |x: i32, y: i32, z: i32| plain(if (x, y, z) == (2, 64, 0) { &SLAB[..] } else { &[][..] });
        let d = [2.0f64, -1.0, 0.0];
        let n = (d[0] * d[0] + d[1] * d[1]).sqrt();
        let h = pick([0.5, 65.62, 0.5], [d[0] / n, d[1] / n, 0.0], 4.5, 3.0, &shape, &no_interaction, std::iter::empty());
        assert!(matches!(h, Hit::Block { pos: [2, 64, 0], face: UP, .. }), "{h:?}");
    }

    #[test]
    fn entity_in_front_of_block() {
        let cow =
            Candidate { id: 7, bb: Aabb { min: [-0.45, 64.0, 1.5], max: [0.45, 65.4, 2.4] }, radius: 0.0, same_vehicle: false };
        let shape = |_: i32, _: i32, z: i32| plain(if z == 3 { &FULL[..] } else { &[][..] });
        let h = pick([0.0, 64.62, 0.0], [0.0, 0.0, 1.0], 4.5, 3.0, &shape, &no_interaction, std::iter::once(cow));
        assert!(matches!(h, Hit::Entity { id: 7, .. }), "{h:?}");
    }

    #[test]
    fn entity_beyond_entity_range_is_miss_not_block() {
        // Entity at 3.5 (beyond 3) in front of a block at 4: the block is behind, nothing is targeted.
        let cow =
            Candidate { id: 7, bb: Aabb { min: [-0.45, 64.0, 3.5], max: [0.45, 65.4, 3.9] }, radius: 0.0, same_vehicle: false };
        let shape = |_: i32, _: i32, z: i32| plain(if z == 4 { &FULL[..] } else { &[][..] });
        let h = pick([0.0, 64.62, 0.0], [0.0, 0.0, 1.0], 4.5, 3.0, &shape, &no_interaction, std::iter::once(cow));
        assert_eq!(h, Hit::Miss);
    }

    /// Spear (range 2 to 4.5, margin 0.125): a cow at 4 blocks, outside the entity range (3)
    /// of normal targeting, is targeted; a cow at 1 block, too close, is not.
    #[test]
    fn attack_range_reaches_past_entity_range() {
        let reach = AttackReach { min: 2.0, max: 4.5, margin: 0.125 };
        let far = || Candidate {
            id: 7,
            bb: Aabb { min: [-0.45, 64.0, 3.8], max: [0.45, 65.4, 4.7] },
            radius: 0.0,
            same_vehicle: false,
        };
        let eye = [0.0, 64.62, 0.0];
        let empty = |_: i32, _: i32, _: i32| plain(&[]);
        assert_eq!(pick(eye, [0.0, 0.0, 1.0], 4.5, 3.0, &empty, &no_interaction, std::iter::once(far())), Hit::Miss);
        let h = pick_attack(eye, [0.0, 0.0, 1.0], reach, 0.0, 4.5, OPEN, &empty, &no_interaction, std::iter::once(far()));
        match h {
            Hit::Entity { id: 7, at } => assert!((at[2] - 3.8).abs() < 1e-9, "{at:?}"),
            _ => panic!("{h:?}"),
        }
        let near = Candidate {
            id: 8,
            bb: Aabb { min: [-0.45, 64.0, 0.5], max: [0.45, 65.4, 1.4] },
            radius: 0.0,
            same_vehicle: false,
        };
        let h = pick_attack(eye, [0.0, 0.0, 1.0], reach, 0.0, 4.5, OPEN, &empty, &no_interaction, std::iter::once(near));
        assert_eq!(h, Hit::Miss);
    }

    /// Margin: the gaze passes 0.1 beside the box; the box inflated by 0.125 is hit and
    /// the point is brought back onto the surface of the real box, toward its center.
    #[test]
    fn attack_margin_projects_on_surface() {
        let reach = AttackReach { min: 0.0, max: 4.5, margin: 0.125 };
        let c = Candidate {
            id: 3,
            bb: Aabb { min: [0.1, 64.0, 2.0], max: [1.0, 65.0, 3.0] },
            radius: 0.0,
            same_vehicle: false,
        };
        let empty = |_: i32, _: i32, _: i32| plain(&[]);
        let h = pick_attack([0.0, 64.5, 0.0], [0.0, 0.0, 1.0], reach, 0.0, 4.5, OPEN, &empty, &no_interaction, std::iter::once(c));
        match h {
            Hit::Entity { id: 3, at } => {
                // Entry into the inflated box at z = 1.875, then toward the center (0.55; 64.5; 2.5):
                // north face of the real box at a fifth of the way (x = 0.11).
                assert!((at[2] - 2.0).abs() < 1e-9 && (at[0] - 0.11).abs() < 1e-9, "{at:?}");
            }
            _ => panic!("{h:?}"),
        }
    }

    /// Cauldron at (0, 64, 0): the gaze enters through the opening and hits the east wall; the
    /// targeted face is the top one (inside hit earlier), the point stays on the wall.
    #[test]
    fn cauldron_inside_gives_the_aimed_face() {
        const WALLS: [Aabb; 2] = [
            Aabb { min: [0.875, 0.0, 0.0], max: [1.0, 1.0, 1.0] },
            Aabb { min: [0.125, 0.0, 0.125], max: [0.875, 0.25, 0.875] },
        ];
        const INSIDE: [Aabb; 1] = [Aabb { min: [0.125, 0.25, 0.125], max: [0.875, 1.0, 0.875] }];
        let shape = |x: i32, y: i32, z: i32| plain(if (x, y, z) == (0, 64, 0) { &WALLS[..] } else { &[][..] });
        let inside = |x: i32, y: i32, z: i32| if (x, y, z) == (0, 64, 0) { &INSIDE[..] } else { &[][..] };
        let (dx, dy) = (0.65f64, -1.0f64);
        let n = (dx * dx + dy * dy).sqrt();
        let dir = [dx / n, dy / n, 0.0];
        let h = pick([0.3, 65.5, 0.5], dir, 4.5, 3.0, &shape, &inside, std::iter::empty());
        match h {
            Hit::Block { pos: [0, 64, 0], face: UP, at, inside: false } => assert!((at[0] - 0.875).abs() < 1e-9, "{at:?}"),
            _ => panic!("{h:?}"),
        }
        let h = pick([0.3, 65.5, 0.5], dir, 4.5, 3.0, &shape, &no_interaction, std::iter::empty());
        assert!(matches!(h, Hit::Block { face: WEST, .. }), "{h:?}");
    }

    /// Brush targeting: the entity in front of the block wins; an entity that contains the eyes is
    /// ignored; with nothing, `Miss`.
    #[test]
    fn along_view_entities_then_block() {
        let shape = |_: i32, _: i32, z: i32| plain(if z == 3 { &FULL[..] } else { &[][..] });
        let cow = |id, z0: f64, z1: f64| Candidate {
            id,
            bb: Aabb { min: [-0.45, 64.0, z0], max: [0.45, 65.4, z1] },
            radius: 0.0,
            same_vehicle: false,
        };
        let (eye, to) = ([0.0, 64.62, 0.0], [0.0, 64.62, 4.5]);
        let h = pick_along(eye, to, &shape, &no_interaction, OPEN, std::iter::once(cow(7, 1.5, 2.4)));
        assert!(matches!(h, Hit::Entity { id: 7, at } if (at[2] - 1.5).abs() < 1e-9), "{h:?}");
        let h = pick_along(eye, to, &shape, &no_interaction, OPEN, std::iter::once(cow(8, -0.5, 0.5)));
        assert!(matches!(h, Hit::Block { pos: [0, 64, 3], face: NORTH, .. }), "{h:?}");
        let h = pick_along(eye, to, &shape, &no_interaction, OPEN, std::iter::once(cow(9, 3.2, 4.0)));
        assert!(matches!(h, Hit::Block { .. }), "entity behind the block: {h:?}");
        let empty = |_: i32, _: i32, _: i32| plain(&[]);
        assert_eq!(pick_along(eye, to, &empty, &no_interaction, OPEN, std::iter::empty()), Hit::Miss);
    }

    /// Flower offset toward +x: a ray passing to the left of the model (at the center of the unshifted block)
    /// misses it, a ray inside the shifted shape hits it.
    #[test]
    fn shifted_shape_follows_offset() {
        // Centered flower shape (like the game before offset): 0.3125 to 0.6875.
        const FLOWER: [Aabb; 1] = [Aabb { min: [0.3125, 0.0, 0.3125], max: [0.6875, 0.625, 0.6875] }];
        let shift = [0.25, 0.0, 0.0];
        let shape = |x: i32, y: i32, z: i32| if (x, y, z) == (0, 64, 2) { (&FLOWER[..], shift) } else { plain(&[]) };
        // Horizontal ray at x = 0.4: inside the centered shape, outside the shifted shape (0.5625..0.9375).
        let miss = pick([0.4, 64.3, 0.5], [0.0, 0.0, 1.0], 4.5, 3.0, &shape, &no_interaction, std::iter::empty());
        assert_eq!(miss, Hit::Miss);
        let hit = pick([0.8, 64.3, 0.5], [0.0, 0.0, 1.0], 4.5, 3.0, &shape, &no_interaction, std::iter::empty());
        match hit {
            Hit::Block { pos, face, at, .. } => {
                assert_eq!((pos, face), ([0, 64, 2], NORTH));
                assert!((at[2] - 2.3125).abs() < 1e-9, "{at:?}");
            }
            _ => panic!("{hit:?}"),
        }
    }
}
