//! Shape updates during generation (templates placed without known shape, § 10.3):
//! connections (fences, panes and bars, walls, stairs, fence gates), linked halves (doors),
//! attached blocks that no longer hold (torches, ladders, signs, banners, lanterns); the
//! rest goes through the decoration rules (`decor_rules::update_shape`).

use crate::decor_blocks::{BlockId, BlockSet, Blocks, Dir, Prop, State};
use crate::decor_region::{Region, add};

const H4: [Dir; 4] = [Dir::North, Dir::East, Dir::South, Dir::West];

fn short(name: &str) -> &str {
    name.strip_prefix("minecraft:").unwrap_or(name)
}

fn in_tag(r: &Region, s: State, t: &str) -> bool {
    r.b.tag(t).is_some_and(|t| r.b.in_tag(s, t))
}

fn exception(r: &Region, s: State) -> bool {
    let n = short(r.b.block_name(r.b.block(s)));
    in_tag(r, s, "minecraft:leaves")
        || matches!(n, "barrier" | "carved_pumpkin" | "jack_o_lantern" | "melon" | "pumpkin")
        || n.ends_with("shulker_box")
}

fn is_pane(r: &Region, s: State) -> bool {
    let n = short(r.b.block_name(r.b.block(s)));
    n == "iron_bars" || n.ends_with("glass_pane") || n.ends_with("_bars")
}

fn facing(r: &Region, s: State) -> Option<Dir> {
    r.b.prop("facing").and_then(|f| r.b.get_dir(s, f))
}

fn get<'a>(r: &'a Region, s: State, k: &str) -> Option<&'a str> {
    r.b.prop(k).and_then(|p| r.b.get_str(s, p))
}

fn set(r: &Region, s: State, k: &str, v: &str) -> State {
    match r.b.prop(k) {
        Some(p) if r.b.has(s, p) => r.b.with_str(s, p, v),
        _ => s,
    }
}

fn gate_aligned(r: &Region, n: State, d: Dir) -> bool {
    in_tag(r, n, "minecraft:fence_gates") && facing(r, n).is_some_and(|f| f.axis() == d.cw().axis())
}

fn legacy_solid(r: &Region, s: State) -> bool {
    r.b.has_flag(s, rc_data::flags::LEGACY_SOLID)
}

fn is_stairs(r: &Region, s: State) -> bool {
    in_tag(r, s, "minecraft:stairs")
}

fn stairs_take(r: &Region, s: State, p: [i32; 3], d: Dir) -> bool {
    let n = r.get_p(add(p, d));
    !is_stairs(r, n) || facing(r, n) != facing(r, s) || get(r, n, "half") != get(r, s, "half")
}

fn stairs_shape(r: &Region, s: State, p: [i32; 3]) -> &'static str {
    let Some(f) = facing(r, s) else { return "straight" };
    let half = get(r, s, "half");
    let behind = r.get_p(add(p, f));
    if is_stairs(r, behind)
        && get(r, behind, "half") == half
        && let Some(bf) = facing(r, behind)
        && bf.axis() != f.axis()
        && stairs_take(r, s, p, bf.opposite())
    {
        return if bf == f.ccw() { "outer_left" } else { "outer_right" };
    }
    let front = r.get_p(add(p, f.opposite()));
    if is_stairs(r, front)
        && get(r, front, "half") == half
        && let Some(ff) = facing(r, front)
        && ff.axis() != f.axis()
        && stairs_take(r, s, p, ff)
    {
        return if ff == f.ccw() { "inner_left" } else { "inner_right" };
    }
    "straight"
}

fn wall_connects(r: &Region, n: State, d: Dir) -> bool {
    in_tag(r, n, "minecraft:walls")
        || (!exception(r, n) && r.b.sturdy(n, d.opposite()))
        || is_pane(r, n)
        || gate_aligned(r, n, d)
}

/// Sides and post of a wall (connections north, east, south, west) from the block above.
fn wall_shape(r: &Region, s: State, top: State, c: [bool; 4]) -> State {
    let zones = r.b.facts(top).wall_down;
    let mut ns = s;
    for (i, d) in H4.iter().enumerate() {
        let side = if !c[i] {
            "none"
        } else if zones >> i & 1 != 0 {
            "tall"
        } else {
            "low"
        };
        ns = set(r, ns, d.name(), side);
    }
    let up = if in_tag(r, top, "minecraft:walls") && get(r, top, "up") == Some("true") {
        true
    } else {
        let g = |d: Dir| get(r, ns, d.name()).unwrap_or("none");
        let (n, so, e, w) = (g(Dir::North), g(Dir::South), g(Dir::East), g(Dir::West));
        let (nn, sn, en, wn) = (n == "none", so == "none", e == "none", w == "none");
        if (nn && sn && wn && en) || nn != sn || wn != en {
            true
        } else if (n == "tall" && so == "tall") || (e == "tall" && w == "tall") {
            false
        } else {
            in_tag(r, top, "minecraft:wall_post_override") || zones & 16 != 0
        }
    };
    set(r, ns, "up", if up { "true" } else { "false" })
}

/// New state of `s` at `p` when the neighbour in direction `d` is `n`: families by
/// precomputed sets (`RUSTCRAFT_OPT_SHAPE_KEYS`, default) or by names (old path).
#[inline]
pub fn update(r: &Region, s: State, p: [i32; 3], d: Dir, n: State) -> State {
    if r.b.shape.opt { keyed::update(r, s, p, d, n) } else { update_by_name(r, s, p, d, n) }
}

/// New path, called directly (equivalence proofs).
#[cfg(test)]
pub(crate) fn update_keyed(r: &Region, s: State, p: [i32; 3], d: Dir, n: State) -> State {
    keyed::update(r, s, p, d, n)
}

/// Old path: tags and properties looked up by name at each call.
pub(crate) fn update_by_name(r: &Region, s: State, p: [i32; 3], d: Dir, n: State) -> State {
    let b = r.b;
    if b.is_air(s) {
        return s;
    }
    let air = b.default_state(b.k.air);
    let name = short(b.block_name(b.block(s)));
    if in_tag(r, s, "minecraft:fences") {
        if !d.is_horizontal() {
            return s;
        }
        let wooden = in_tag(r, s, "minecraft:wooden_fences");
        let same = in_tag(r, n, "minecraft:fences") && in_tag(r, n, "minecraft:wooden_fences") == wooden;
        let c = (!exception(r, n) && b.sturdy(n, d.opposite())) || same || gate_aligned(r, n, d.opposite());
        return set(r, s, d.name(), if c { "true" } else { "false" });
    }
    if is_pane(r, s) {
        if !d.is_horizontal() {
            return s;
        }
        let c = (!exception(r, n) && b.sturdy(n, d.opposite())) || is_pane(r, n) || in_tag(r, n, "minecraft:walls");
        return set(r, s, d.name(), if c { "true" } else { "false" });
    }
    if in_tag(r, s, "minecraft:walls") {
        let cur = H4.map(|dd| get(r, s, dd.name()).is_some_and(|v| v != "none"));
        return match d {
            Dir::Down => s,
            Dir::Up => wall_shape(r, s, n, cur),
            _ => {
                let mut c = cur;
                let i = H4.iter().position(|&x| x == d).unwrap_or(0);
                c[i] = wall_connects(r, n, d.opposite());
                wall_shape(r, s, r.get_p(add(p, Dir::Up)), c)
            }
        };
    }
    if is_stairs(r, s) {
        return if d.is_horizontal() { set(r, s, "shape", stairs_shape(r, s, p)) } else { s };
    }
    if in_tag(r, s, "minecraft:fence_gates") {
        let Some(f) = facing(r, s) else { return s };
        if f.cw().axis() != d.axis() {
            return s;
        }
        let o = r.get_p(add(p, d.opposite()));
        let w = in_tag(r, n, "minecraft:walls") || in_tag(r, o, "minecraft:walls");
        return set(r, s, "in_wall", if w { "true" } else { "false" });
    }
    if in_tag(r, s, "minecraft:doors") {
        let half = get(r, s, "half").unwrap_or("lower");
        let lower = half == "lower";
        if d.axis() != 1 || lower != (d == Dir::Up) {
            if lower && d == Dir::Down && !b.sturdy(r.get_p(add(p, Dir::Down)), Dir::Up) {
                return air;
            }
            return s;
        }
        if b.block(n) == b.block(s) && get(r, n, "half") != Some(half) {
            return set(r, n, "half", half);
        }
        return air;
    }
    match name {
        "torch" | "soul_torch" | "redstone_torch" | "copper_torch" => {
            if d == Dir::Down && b.facts(n).center & (1 << Dir::Up as u8) == 0 {
                return air;
            }
            return s;
        }
        "wall_torch" | "soul_wall_torch" | "redstone_wall_torch" | "copper_wall_torch" | "ladder" => {
            if let Some(f) = facing(r, s)
                && d.opposite() == f
                && !b.sturdy(n, f)
            {
                return air;
            }
            return s;
        }
        "lantern" | "soul_lantern" | "copper_lantern" => {
            let hanging = get(r, s, "hanging") == Some("true");
            let c = if hanging { Dir::Up } else { Dir::Down };
            if d == c {
                let ok = if hanging {
                    b.facts(n).center & (1 << Dir::Down as u8) != 0
                } else {
                    b.facts(n).center & (1 << Dir::Up as u8) != 0
                };
                if !ok {
                    return air;
                }
            }
            return s;
        }
        _ => {}
    }
    if name.ends_with("_wall_banner") || name.ends_with("_wall_sign") {
        if let Some(f) = facing(r, s)
            && d == f.opposite()
            && !legacy_solid(r, n)
        {
            return air;
        }
        return s;
    }
    if (name.ends_with("_banner") || name.ends_with("_sign")) && !name.contains("hanging") {
        if d == Dir::Down && !legacy_solid(r, n) {
            return air;
        }
        return s;
    }
    crate::decor_rules::update_shape(r, s, p, d, n)
}

/// Block families of the shape updates and own properties, resolved once at the
/// construction of the blocks (the tags are in `Blocks::rules`).
#[derive(Debug, Clone)]
pub struct ShapeKeys {
    /// `RUSTCRAFT_OPT_SHAPE_KEYS` (read at construction; the proofs switch both paths).
    pub(crate) opt: bool,
    /// Blocks never connected through their full face (except foliage, tested by tag).
    exception: BlockSet,
    pane: BlockSet,
    torch: BlockSet,
    /// Wall torches and ladders.
    wall_torch: BlockSet,
    lantern: BlockSet,
    /// Wall banners and signs.
    wall_attached: BlockSet,
    /// Banners and signs placed on the ground (except hanging signs).
    standing: BlockSet,
    /// All the families handled here: a block outside this set goes straight to the decoration rules.
    any: BlockSet,
    /// Blocks that no shape update changes (outside the families, general rule without effect).
    inert: BlockSet,
    shape: Prop,
    in_wall: Prop,
}

impl ShapeKeys {
    /// Block of a family handled here (equivalence proofs).
    #[cfg(test)]
    pub(crate) fn in_family(&self, b: BlockId) -> bool {
        self.any.contains(b)
    }

    /// No family (while the blocks are being built).
    pub fn empty(blocks: usize) -> ShapeKeys {
        ShapeKeys {
            opt: crate::opt::shape_keys(),
            exception: BlockSet::with_capacity(blocks),
            pane: BlockSet::with_capacity(blocks),
            torch: BlockSet::with_capacity(blocks),
            wall_torch: BlockSet::with_capacity(blocks),
            lantern: BlockSet::with_capacity(blocks),
            wall_attached: BlockSet::with_capacity(blocks),
            standing: BlockSet::with_capacity(blocks),
            any: BlockSet::with_capacity(blocks),
            inert: BlockSet::with_capacity(blocks),
            shape: Prop(u8::MAX),
            in_wall: Prop(u8::MAX),
        }
    }

    pub fn new(b: &Blocks) -> ShapeKeys {
        let n = b.block_count();
        let mut k = ShapeKeys {
            shape: b.prop("shape").unwrap_or(Prop(u8::MAX)),
            in_wall: b.prop("in_wall").unwrap_or(Prop(u8::MAX)),
            ..ShapeKeys::empty(n)
        };
        // Same name tests as the old path, done once per block.
        for i in 0..n {
            let id = i as BlockId;
            let name = short(b.block_name(id));
            if matches!(name, "barrier" | "carved_pumpkin" | "jack_o_lantern" | "melon" | "pumpkin")
                || name.ends_with("shulker_box")
            {
                k.exception.insert(id);
            }
            let sets = [
                (&mut k.pane, name == "iron_bars" || name.ends_with("glass_pane") || name.ends_with("_bars")),
                (&mut k.torch, matches!(name, "torch" | "soul_torch" | "redstone_torch" | "copper_torch")),
                (
                    &mut k.wall_torch,
                    matches!(
                        name,
                        "wall_torch" | "soul_wall_torch" | "redstone_wall_torch" | "copper_wall_torch" | "ladder"
                    ),
                ),
                (&mut k.lantern, matches!(name, "lantern" | "soul_lantern" | "copper_lantern")),
                (&mut k.wall_attached, name.ends_with("_wall_banner") || name.ends_with("_wall_sign")),
                (
                    &mut k.standing,
                    (name.ends_with("_banner") || name.ends_with("_sign")) && !name.contains("hanging"),
                ),
            ];
            let mut hit = false;
            for (set, yes) in sets {
                if yes {
                    set.insert(id);
                    hit = true;
                }
            }
            let t = &b.rules;
            if hit
                || [t.fences, t.walls, t.stairs, t.fence_gates, t.doors]
                    .iter()
                    .any(|&tg| tg != u16::MAX && b.tag_contains(tg, id))
            {
                k.any.insert(id);
            } else if crate::decor_rules::shape_inert(b, id) {
                k.inert.insert(id);
            }
        }
        k
    }

    /// Block that no shape update changes.
    #[inline]
    pub fn inert(&self, b: BlockId) -> bool {
        self.inert.contains(b)
    }
}

/// Shape updates without lookup by name: same tests, in the same order, as
/// [`update_by_name`].
mod keyed {
    use super::{H4, ShapeKeys};
    use crate::decor_blocks::{Blocks, Dir, State};
    use crate::decor_region::{Region, add};
    use crate::decor_rules::face_prop;

    #[inline]
    fn tg(b: &Blocks, s: State, t: u16) -> bool {
        t != u16::MAX && b.in_tag(s, t)
    }

    #[inline]
    fn k(b: &Blocks) -> &ShapeKeys {
        &b.shape
    }

    fn exception(b: &Blocks, s: State) -> bool {
        tg(b, s, b.rules.leaves) || k(b).exception.contains(b.block(s))
    }

    fn is_pane(b: &Blocks, s: State) -> bool {
        k(b).pane.contains(b.block(s))
    }

    fn facing(b: &Blocks, s: State) -> Option<Dir> {
        b.get_dir(s, b.p.facing)
    }

    fn half(b: &Blocks, s: State) -> Option<&str> {
        b.get_str(s, b.p.half)
    }

    fn gate_aligned(b: &Blocks, n: State, d: Dir) -> bool {
        tg(b, n, b.rules.fence_gates) && facing(b, n).is_some_and(|f| f.axis() == d.cw().axis())
    }

    fn is_stairs(b: &Blocks, s: State) -> bool {
        tg(b, s, b.rules.stairs)
    }

    fn stairs_take(r: &Region, s: State, p: [i32; 3], d: Dir) -> bool {
        let b = r.b;
        let n = r.get_p(add(p, d));
        !is_stairs(b, n) || facing(b, n) != facing(b, s) || half(b, n) != half(b, s)
    }

    fn stairs_shape(r: &Region, s: State, p: [i32; 3]) -> &'static str {
        let b = r.b;
        let Some(f) = facing(b, s) else { return "straight" };
        let h = half(b, s);
        let behind = r.get_p(add(p, f));
        if is_stairs(b, behind)
            && half(b, behind) == h
            && let Some(bf) = facing(b, behind)
            && bf.axis() != f.axis()
            && stairs_take(r, s, p, bf.opposite())
        {
            return if bf == f.ccw() { "outer_left" } else { "outer_right" };
        }
        let front = r.get_p(add(p, f.opposite()));
        if is_stairs(b, front)
            && half(b, front) == h
            && let Some(ff) = facing(b, front)
            && ff.axis() != f.axis()
            && stairs_take(r, s, p, ff)
        {
            return if ff == f.ccw() { "inner_left" } else { "inner_right" };
        }
        "straight"
    }

    fn wall_connects(b: &Blocks, n: State, d: Dir) -> bool {
        tg(b, n, b.rules.walls)
            || (!exception(b, n) && b.sturdy(n, d.opposite()))
            || is_pane(b, n)
            || gate_aligned(b, n, d)
    }

    fn wall_shape(b: &Blocks, s: State, top: State, c: [bool; 4]) -> State {
        let zones = b.facts(top).wall_down;
        let mut ns = s;
        for (i, d) in H4.iter().enumerate() {
            let side = if !c[i] {
                "none"
            } else if zones >> i & 1 != 0 {
                "tall"
            } else {
                "low"
            };
            ns = b.with_str(ns, face_prop(b, *d), side);
        }
        let up = if tg(b, top, b.rules.walls) && b.get_bool(top, b.p.up) == Some(true) {
            true
        } else {
            let g = |d: Dir| b.get_str(ns, face_prop(b, d)).unwrap_or("none");
            let (n, so, e, w) = (g(Dir::North), g(Dir::South), g(Dir::East), g(Dir::West));
            let (nn, sn, en, wn) = (n == "none", so == "none", e == "none", w == "none");
            if (nn && sn && wn && en) || nn != sn || wn != en {
                true
            } else if (n == "tall" && so == "tall") || (e == "tall" && w == "tall") {
                false
            } else {
                tg(b, top, b.rules.wall_post_override) || zones & 16 != 0
            }
        };
        b.with_bool(ns, b.p.up, up)
    }

    pub(super) fn update(r: &Region, s: State, p: [i32; 3], d: Dir, n: State) -> State {
        let b = r.b;
        if b.is_air(s) {
            return s;
        }
        let bl = b.block(s);
        let k = k(b);
        if !k.any.contains(bl) {
            return crate::decor_rules::update_shape(r, s, p, d, n);
        }
        let air = b.default_state(b.k.air);
        let t = &b.rules;
        if tg(b, s, t.fences) {
            if !d.is_horizontal() {
                return s;
            }
            let wooden = tg(b, s, t.wooden_fences);
            let same = tg(b, n, t.fences) && tg(b, n, t.wooden_fences) == wooden;
            let c = (!exception(b, n) && b.sturdy(n, d.opposite())) || same || gate_aligned(b, n, d.opposite());
            return b.with_bool(s, face_prop(b, d), c);
        }
        if k.pane.contains(bl) {
            if !d.is_horizontal() {
                return s;
            }
            let c = (!exception(b, n) && b.sturdy(n, d.opposite())) || is_pane(b, n) || tg(b, n, t.walls);
            return b.with_bool(s, face_prop(b, d), c);
        }
        if tg(b, s, t.walls) {
            let cur = H4.map(|dd| b.get_str(s, face_prop(b, dd)).is_some_and(|v| v != "none"));
            return match d {
                Dir::Down => s,
                Dir::Up => wall_shape(b, s, n, cur),
                _ => {
                    let mut c = cur;
                    let i = H4.iter().position(|&x| x == d).unwrap_or(0);
                    c[i] = wall_connects(b, n, d.opposite());
                    wall_shape(b, s, r.get_p(add(p, Dir::Up)), c)
                }
            };
        }
        if is_stairs(b, s) {
            return if d.is_horizontal() { b.with_str(s, k.shape, stairs_shape(r, s, p)) } else { s };
        }
        if tg(b, s, t.fence_gates) {
            let Some(f) = facing(b, s) else { return s };
            if f.cw().axis() != d.axis() {
                return s;
            }
            let o = r.get_p(add(p, d.opposite()));
            let w = tg(b, n, t.walls) || tg(b, o, t.walls);
            return b.with_bool(s, k.in_wall, w);
        }
        if tg(b, s, t.doors) {
            let h = half(b, s).unwrap_or("lower");
            let lower = h == "lower";
            if d.axis() != 1 || lower != (d == Dir::Up) {
                if lower && d == Dir::Down && !b.sturdy(r.get_p(add(p, Dir::Down)), Dir::Up) {
                    return air;
                }
                return s;
            }
            if b.block(n) == bl && half(b, n) != Some(h) {
                return b.with_str(n, b.p.half, h);
            }
            return air;
        }
        if k.torch.contains(bl) {
            if d == Dir::Down && b.facts(n).center & (1 << Dir::Up as u8) == 0 {
                return air;
            }
            return s;
        }
        if k.wall_torch.contains(bl) {
            if let Some(f) = facing(b, s)
                && d.opposite() == f
                && !b.sturdy(n, f)
            {
                return air;
            }
            return s;
        }
        if k.lantern.contains(bl) {
            let hanging = b.get_bool(s, b.p.hanging) == Some(true);
            let c = if hanging { Dir::Up } else { Dir::Down };
            if d == c {
                let ok = if hanging {
                    b.facts(n).center & (1 << Dir::Down as u8) != 0
                } else {
                    b.facts(n).center & (1 << Dir::Up as u8) != 0
                };
                if !ok {
                    return air;
                }
            }
            return s;
        }
        if k.wall_attached.contains(bl) {
            if let Some(f) = facing(b, s)
                && d == f.opposite()
                && !b.legacy_solid(n)
            {
                return air;
            }
            return s;
        }
        if k.standing.contains(bl) {
            if d == Dir::Down && !b.legacy_solid(n) {
                return air;
            }
            return s;
        }
        crate::decor_rules::update_shape(r, s, p, d, n)
    }
}

/// Order of the neighbours of the game's shape updates.
pub const ORDER: [Dir; 6] = [Dir::West, Dir::East, Dir::North, Dir::South, Dir::Down, Dir::Up];

/// Shape recomputed from the six neighbours (game order).
pub fn from_neighbours(r: &Region, s: State, p: [i32; 3]) -> State {
    let mut s = s;
    for d in ORDER {
        s = update(r, s, p, d, r.get_p(add(p, d)));
    }
    s
}

/// Shape updates of a template placed without known shape: faces of the placed volume, then each
/// placed block from its neighbours.
pub fn after_template(r: &mut Region, placed: &[[i32; 3]]) {
    if placed.is_empty() {
        return;
    }
    let mut lo = placed[0];
    let mut hi = placed[0];
    for p in placed {
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    let size = [hi[0] - lo[0] + 1, hi[1] - lo[1] + 1, hi[2] - lo[2] + 1];
    let idx = |p: [i32; 3]| ((p[0] - lo[0]) * size[1] + (p[1] - lo[1])) * size[2] + (p[2] - lo[2]);
    let mut filled = vec![false; (size[0] * size[1] * size[2]) as usize];
    for p in placed {
        filled[idx(*p) as usize] = true;
    }
    let inside = |q: [i32; 3]| (0..3).all(|i| q[i] >= lo[i] && q[i] <= hi[i]);
    // Faces of the volume (axes x, y, z; negative side then positive).
    for (axis, dirs) in [(0, [Dir::West, Dir::East]), (1, [Dir::Down, Dir::Up]), (2, [Dir::North, Dir::South])] {
        let _ = axis;
        for x in lo[0]..=hi[0] {
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    let p = [x, y, z];
                    if !filled[idx(p) as usize] {
                        continue;
                    }
                    for d in dirs {
                        let q = add(p, d);
                        if inside(q) && filled[idx(q) as usize] {
                            continue;
                        }
                        let s = r.get_p(p);
                        let ns = r.get_p(q);
                        let s2 = update(r, s, p, d, ns);
                        if s2 != s {
                            r.set_p(p, s2, 2);
                        }
                        let n2 = update(r, ns, q, d.opposite(), s2);
                        if n2 != ns {
                            r.set_p(q, n2, 2);
                        }
                    }
                }
            }
        }
    }
    for p in placed {
        let s = r.get_p(*p);
        let n = from_neighbours(r, s, *p);
        if n != s {
            r.set_p(*p, n, 2);
        }
    }
}
