//! Nether features (and block piles): deltas, giant mushrooms, projected squares,
//! neighbour spreading, patch replacement, pillars, column clusters
//! (`generation-decoration.md` § 6, "the rest").

use crate::Error;
use crate::decor::Ctx;
use crate::decor_blocks::{BlockId, Blocks, Dir, State};
use crate::decor_pred::{Blockset, IntP, ParseCtx, Pred, Provider, get_i32, opt_bool};
use crate::decor_region::add;
use crate::decor_rng::DecoRng;
use crate::feature::Pillar;
use crate::json::Json;

#[derive(Debug, Clone)]
pub struct Delta {
    pub contents: State,
    pub rim: State,
    pub size: IntP,
    pub rim_size: IntP,
    pub cannot_replace: Vec<BlockId>,
}

#[derive(Debug, Clone)]
pub struct HugeFungus {
    pub base: BlockId,
    pub stem: State,
    pub hat: State,
    pub decor: State,
    pub replaceable: Pred,
    pub planted: bool,
    pub vines_head: BlockId,
    pub vines_plant: State,
    /// Wart block cap: weeping vines under the cap.
    pub hat_vines: bool,
}

#[derive(Debug, Clone)]
pub struct PatchySquare {
    pub block: Provider,
    pub project_through: Pred,
    pub size: IntP,
    pub max_projection: i32,
}

#[derive(Debug, Clone)]
pub struct NeighborSpread {
    pub block: Provider,
    pub accepted: Blockset,
    pub can_replace: Pred,
    pub attempts: IntP,
    pub xz: IntP,
    pub y: IntP,
}

#[derive(Debug, Clone)]
pub struct ReplaceBlobs {
    pub target: BlockId,
    pub state: State,
    pub radius: IntP,
}

#[derive(Debug, Clone)]
pub struct SteppedColumns {
    pub block: Provider,
    pub continue_through: Pred,
    pub can_replace: Pred,
    pub cannot_place_on: Blockset,
    pub cluster_reach: IntP,
    pub column_count: IntP,
    pub column_reach: IntP,
    pub height: IntP,
}

fn need<'j>(j: &'j Json, k: &str) -> Result<&'j Json, Error> {
    j.get(k).ok_or_else(|| Error::Format(format!("Nether feature: field {k}")))
}

fn block_of(b: &Blocks, j: &Json) -> Result<BlockId, Error> {
    Ok(b.block(b.parse_state(j)?))
}

pub fn parse_delta(j: &Json, c: &mut ParseCtx) -> Result<Delta, Error> {
    let b = c.b;
    let mut cannot_replace = Vec::new();
    for n in
        ["bedrock", "nether_bricks", "nether_brick_fence", "nether_brick_stairs", "nether_wart", "chest", "spawner"]
    {
        cannot_replace.push(b.block_id(n).ok_or_else(|| Error::Missing(format!("block {n}")))?);
    }
    Ok(Delta {
        contents: b.parse_state(need(j, "contents")?)?,
        rim: b.parse_state(need(j, "rim")?)?,
        size: IntP::parse(need(j, "size")?)?,
        rim_size: IntP::parse(need(j, "rim_size")?)?,
        cannot_replace,
    })
}

pub fn parse_fungus(j: &Json, c: &mut ParseCtx) -> Result<HugeFungus, Error> {
    let b = c.b;
    let id = |n: &str| b.block_id(n).ok_or_else(|| Error::Missing(format!("block {n}")));
    let hat = b.parse_state(need(j, "hat_state")?)?;
    Ok(HugeFungus {
        base: block_of(b, need(j, "valid_base_block")?)?,
        stem: b.parse_state(need(j, "stem_state")?)?,
        hat,
        hat_vines: b.block(hat) == id("nether_wart_block")?,
        decor: b.parse_state(need(j, "decor_state")?)?,
        replaceable: Pred::parse(need(j, "replaceable_blocks")?, c)?,
        planted: opt_bool(j, "planted", false),
        vines_head: id("weeping_vines")?,
        vines_plant: b.default_state(id("weeping_vines_plant")?),
    })
}

pub fn parse_patchy(j: &Json, c: &mut ParseCtx) -> Result<PatchySquare, Error> {
    Ok(PatchySquare {
        block: Provider::parse(need(j, "block")?, c)?,
        project_through: Pred::parse(need(j, "project_through")?, c)?,
        size: IntP::parse(need(j, "size")?)?,
        max_projection: get_i32(j, "max_projection_height")?,
    })
}

pub fn parse_spread(j: &Json, c: &mut ParseCtx) -> Result<NeighborSpread, Error> {
    Ok(NeighborSpread {
        block: Provider::parse(need(j, "block")?, c)?,
        accepted: Blockset::parse(need(j, "accepted_neighbors")?, c.b)?,
        can_replace: Pred::parse(need(j, "can_replace")?, c)?,
        attempts: IntP::parse(need(j, "attempts")?)?,
        xz: IntP::parse(need(j, "xz_offset")?)?,
        y: IntP::parse(need(j, "y_offset")?)?,
    })
}

pub fn parse_blobs(j: &Json, c: &mut ParseCtx) -> Result<ReplaceBlobs, Error> {
    let b = c.b;
    Ok(ReplaceBlobs {
        target: block_of(b, need(j, "target")?)?,
        state: b.parse_state(need(j, "state")?)?,
        radius: IntP::parse(need(j, "radius")?)?,
    })
}

pub fn parse_columns(j: &Json, c: &mut ParseCtx) -> Result<SteppedColumns, Error> {
    Ok(SteppedColumns {
        block: Provider::parse(need(j, "block")?, c)?,
        continue_through: Pred::parse(need(j, "continue_through")?, c)?,
        can_replace: Pred::parse(need(j, "can_replace")?, c)?,
        cannot_place_on: Blockset::parse(need(j, "cannot_place_on")?, c.b)?,
        cluster_reach: IntP::parse(need(j, "cluster_reach")?)?,
        column_count: IntP::parse(need(j, "column_count")?)?,
        column_reach: IntP::parse(need(j, "column_reach")?)?,
        height: IntP::parse(need(j, "height")?)?,
    })
}

/// Walk of a box by increasing Manhattan distance (game order: depth layers,
/// increasing x, increasing y, then positive z before its negative mirror).
struct Manhattan {
    o: [i32; 3],
    reach: [i32; 3],
    max_depth: i32,
    depth: i32,
    max_x: i32,
    max_y: i32,
    x: i32,
    y: i32,
    mirror: bool,
    cur: [i32; 3],
}

impl Manhattan {
    fn new(o: [i32; 3], rx: i32, ry: i32, rz: i32) -> Self {
        Manhattan {
            o,
            reach: [rx, ry, rz],
            max_depth: rx + ry + rz,
            depth: 0,
            max_x: 0,
            max_y: 0,
            x: 0,
            y: 0,
            mirror: false,
            cur: o,
        }
    }
}

impl Iterator for Manhattan {
    type Item = [i32; 3];

    fn next(&mut self) -> Option<[i32; 3]> {
        if self.mirror {
            self.mirror = false;
            self.cur[2] = self.o[2] - (self.cur[2] - self.o[2]);
            return Some(self.cur);
        }
        loop {
            if self.y > self.max_y {
                self.x += 1;
                if self.x > self.max_x {
                    self.depth += 1;
                    if self.depth > self.max_depth {
                        return None;
                    }
                    self.max_x = self.reach[0].min(self.depth);
                    self.x = -self.max_x;
                }
                self.max_y = self.reach[1].min(self.depth - self.x.abs());
                self.y = -self.max_y;
            }
            let (xx, yy) = (self.x, self.y);
            let zz = self.depth - xx.abs() - yy.abs();
            self.y += 1;
            if zz <= self.reach[2] {
                self.mirror = zz != 0;
                self.cur = [self.o[0] + xx, self.o[1] + yy, self.o[2] + zz];
                return Some(self.cur);
            }
        }
    }
}

/// The game's Manhattan distance (sum in single floats, truncated).
fn dist_manhattan(a: [i32; 3], b: [i32; 3]) -> i32 {
    let f = |i: usize| (a[i] - b[i]).abs() as f32;
    (f(0) + f(1) + f(2)) as i32
}

fn delta_clear(cx: &Ctx, d: &Delta, p: [i32; 3]) -> bool {
    let b = cx.r.b;
    let s = cx.r.get_p(p);
    if b.is(s, b.block(d.contents)) || d.cannot_replace.contains(&b.block(s)) {
        return false;
    }
    for dir in Dir::ALL {
        let air = b.is_air(cx.r.get_p(add(p, dir)));
        if (air && dir != Dir::Up) || (!air && dir == Dir::Up) {
            return false;
        }
    }
    true
}

pub fn delta(cx: &mut Ctx, d: &Delta, rng: &mut DecoRng, o: [i32; 3]) -> bool {
    let mut any = false;
    let spawn_rim = rng.next_double() < 0.9;
    let rim_x = if spawn_rim { d.rim_size.sample(rng) } else { 0 };
    let rim_z = if spawn_rim { d.rim_size.sample(rng) } else { 0 };
    let has_rim = spawn_rim && rim_x != 0 && rim_z != 0;
    let rx = d.size.sample(rng);
    let rz = d.size.sample(rng);
    let limit = rx.max(rz);
    for p in Manhattan::new(o, rx, 0, rz) {
        if dist_manhattan(p, o) > limit {
            break;
        }
        if delta_clear(cx, d, p) {
            if has_rim {
                any = true;
                cx.r.set_p(p, d.rim, 3);
            }
            let q = [p[0] + rim_x, p[1], p[2] + rim_z];
            if delta_clear(cx, d, q) {
                any = true;
                cx.r.set_p(q, d.contents, 3);
            }
        }
    }
    any
}

fn fungus_replaceable(cx: &Ctx, f: &HugeFungus, p: [i32; 3], plants: bool) -> bool {
    cx.r.b.replaceable(cx.r.get_p(p)) || (plants && f.replaceable.test(cx.r, p))
}

pub fn huge_fungus(cx: &mut Ctx, f: &HugeFungus, rng: &mut DecoRng, o: [i32; 3]) -> bool {
    let b = cx.r.b;
    if !b.is(cx.r.get_p(add(o, Dir::Down)), f.base) {
        return false;
    }
    let mut total = rng.between(4, 13);
    if rng.next_int_bounded(12) == 0 {
        total *= 2;
    }
    if f.planted {
        // Planted variant (bone meal): outside natural decoration.
        cx.note_missing("huge_fungus (planted)");
        return false;
    }
    if o[1] + total + 1 >= cx.r.generator_depth {
        return false;
    }
    let huge = rng.next_float() < 0.06;
    cx.r.set_p(o, b.default_state(b.k.air), 260);
    // Stem.
    let sr = i32::from(huge);
    for dx in -sr..=sr {
        for dz in -sr..=sr {
            let corner = huge && dx.abs() == sr && dz.abs() == sr;
            for dy in 0..total {
                let p = [o[0] + dx, o[1] + dy, o[2] + dz];
                if fungus_replaceable(cx, f, p, true) {
                    if corner {
                        if rng.next_float() < 0.1 {
                            cx.r.set_p(p, f.stem, 3);
                        }
                    } else {
                        cx.r.set_p(p, f.stem, 3);
                    }
                }
            }
        }
    }
    // Cap.
    let vines = f.hat_vines;
    let hat_h = (rng.next_int_bounded(1 + total / 3) + 5).min(total);
    let start = total - hat_h;
    for dy in start..=total {
        let mut radius = if dy < total - rng.next_int_bounded(3) { 2 } else { 1 };
        if hat_h > 8 && dy < start + 4 {
            radius = 3;
        }
        if huge {
            radius += 1;
        }
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                let ex = dx == -radius || dx == radius;
                let ez = dz == -radius || dz == radius;
                let inside = !ex && !ez && dy != total;
                let corner = ex && ez;
                let bottom = dy < start + 3;
                let p = [o[0] + dx, o[1] + dy, o[2] + dz];
                if !fungus_replaceable(cx, f, p, false) {
                    continue;
                }
                if bottom {
                    if !inside {
                        hat_drop(cx, f, rng, p, vines);
                    }
                } else if inside {
                    hat_block(cx, f, rng, p, 0.1, 0.2, if vines { 0.1 } else { 0.0 });
                } else if corner {
                    hat_block(cx, f, rng, p, 0.01, 0.7, if vines { 0.083 } else { 0.0 });
                } else {
                    hat_block(cx, f, rng, p, 5.0E-4, 0.98, if vines { 0.07 } else { 0.0 });
                }
            }
        }
    }
    true
}

fn hat_block(cx: &mut Ctx, f: &HugeFungus, rng: &mut DecoRng, p: [i32; 3], decor: f32, hat: f32, vines: f32) {
    if rng.next_float() < decor {
        cx.r.set_p(p, f.decor, 3);
    } else if rng.next_float() < hat {
        cx.r.set_p(p, f.hat, 3);
        if rng.next_float() < vines {
            weeping_vines(cx, f, rng, p);
        }
    }
}

fn hat_drop(cx: &mut Ctx, f: &HugeFungus, rng: &mut DecoRng, p: [i32; 3], vines: bool) {
    let b = cx.r.b;
    if b.is(cx.r.get_p(add(p, Dir::Down)), b.block(f.hat)) {
        cx.r.set_p(p, f.hat, 3);
    } else if f64::from(rng.next_float()) < 0.15 {
        cx.r.set_p(p, f.hat, 3);
        if vines && rng.next_int_bounded(11) == 0 {
            weeping_vines(cx, f, rng, p);
        }
    }
}

fn weeping_vines(cx: &mut Ctx, f: &HugeFungus, rng: &mut DecoRng, hat: [i32; 3]) {
    let b = cx.r.b;
    let mut p = add(hat, Dir::Down);
    if !b.is_air(cx.r.get_p(p)) {
        return;
    }
    let mut goal = rng.between(1, 5);
    if rng.next_int_bounded(7) == 0 {
        goal *= 2;
    }
    for h in 0..=goal {
        if b.is_air(cx.r.get_p(p)) {
            if h == goal || !b.is_air(cx.r.get_p(add(p, Dir::Down))) {
                let age = rng.between(23, 25);
                let s = b.with_int(b.default_state(f.vines_head), b.p.age, age);
                cx.r.set_p(p, s, 2);
                break;
            }
            cx.r.set_p(p, f.vines_plant, 2);
        }
        p = add(p, Dir::Down);
    }
}

pub fn patchy_square(cx: &mut Ctx, c: &PatchySquare, rng: &mut DecoRng, o: [i32; 3]) -> bool {
    let size = c.size.sample(rng);
    let bound = size * size + 1;
    for dx in -size..=size {
        for dz in -size..=size {
            let prob = dx.abs() * dz.abs();
            if rng.next_int_bounded(bound) < bound - prob {
                let mut base = [o[0] + dx, o[1], o[2] + dz];
                let mut drop = c.max_projection;
                while c.project_through.test(cx.r, add(base, Dir::Down)) {
                    base = add(base, Dir::Down);
                    drop -= 1;
                    if drop <= 0 {
                        break;
                    }
                }
                if let Some(s) = c.block.get_opt(cx.r, rng, base) {
                    cx.r.set_p(base, s, 2);
                }
            }
        }
    }
    true
}

pub fn neighbor_spread(cx: &mut Ctx, c: &NeighborSpread, rng: &mut DecoRng, o: [i32; 3]) -> bool {
    let b = cx.r.b;
    let s = c.block.get(cx.r, rng, o);
    cx.r.set_p(o, s, 2);
    let attempts = c.attempts.sample(rng);
    for _ in 0..attempts {
        let dx = c.xz.sample(rng);
        let dy = c.y.sample(rng);
        let dz = c.xz.sample(rng);
        let p = [o[0] + dx, o[1] + dy, o[2] + dz];
        if c.can_replace.test(cx.r, p) {
            let mut n = 0;
            for d in Dir::ALL {
                if c.accepted.contains(b.block(cx.r.get_p(add(p, d)))) {
                    n += 1;
                }
                if n > 1 {
                    break;
                }
            }
            if n == 1 {
                let s = c.block.get(cx.r, rng, p);
                cx.r.set_p(p, s, 2);
            }
        }
    }
    true
}

pub fn replace_blobs(cx: &mut Ctx, c: &ReplaceBlobs, rng: &mut DecoRng, o: [i32; 3]) -> bool {
    let b = cx.r.b;
    let (lo, hi) = (cx.r.min_y + 1, cx.r.max_y());
    let mut cur = [o[0], o[1].clamp(lo, hi), o[2]];
    let center = loop {
        if cur[1] <= lo {
            return false;
        }
        if b.is(cx.r.get_p(cur), c.target) {
            break cur;
        }
        cur[1] -= 1;
    };
    let rx = c.radius.sample(rng);
    let ry = c.radius.sample(rng);
    let rz = c.radius.sample(rng);
    let max = rx.max(ry.max(rz));
    let mut any = false;
    for p in Manhattan::new(center, rx, ry, rz) {
        if dist_manhattan(p, center) > max {
            break;
        }
        if b.is(cx.r.get_p(p), c.target) {
            cx.r.set_p(p, c.state, 3);
            any = true;
        }
    }
    any
}

pub fn pillar(cx: &mut Ctx, c: &Pillar, rng: &mut DecoRng, o: [i32; 3]) -> bool {
    let mut p = o;
    while c.can_replace.test(cx.r, p) && rng.next_float() < c.chance && !cx.r.outside_height(p[1]) {
        let s = c.block.get(cx.r, rng, p);
        cx.r.set_p(p, s, 2);
        p = add(p, c.dir);
    }
    p = add(p, c.dir.opposite());
    if let Some(cap) = c.cap {
        crate::placement::place(cx, cap, rng, p, None);
    }
    true
}

fn columns_can_place_at(cx: &Ctx, c: &SteppedColumns, p: [i32; 3]) -> bool {
    if !c.can_replace.test(cx.r, p) {
        return false;
    }
    let b = cx.r.b;
    let s = cx.r.get_p(add(p, Dir::Down));
    !b.is_air(s) && !c.cannot_place_on.contains(b.block(s))
}

pub fn stepped_columns(cx: &mut Ctx, c: &SteppedColumns, rng: &mut DecoRng, o: [i32; 3]) -> bool {
    if !columns_can_place_at(cx, c, o) {
        return false;
    }
    let height = c.height.sample(rng);
    let reach = height.min(c.cluster_reach.sample(rng));
    let count = c.column_count.sample(rng);
    let width = 2 * reach + 1;
    let mut placed = false;
    for _ in 0..count.max(0) {
        let x = o[0] - reach + rng.next_int_bounded(width);
        let y = o[1] + rng.next_int_bounded(1);
        let z = o[2] - reach + rng.next_int_bounded(width);
        let p = [x, y, z];
        let left = height - dist_manhattan(p, o);
        if left >= 0 {
            let cr = c.column_reach.sample(rng);
            placed |= column(cx, c, rng, p, left, cr);
        }
    }
    placed
}

fn column(cx: &mut Ctx, c: &SteppedColumns, rng: &mut DecoRng, o: [i32; 3], height: i32, reach: i32) -> bool {
    let b = cx.r.b;
    let mut any = false;
    for z in o[2] - reach..=o[2] + reach {
        for x in o[0] - reach..=o[0] + reach {
            let p = [x, o[1], z];
            let step = dist_manhattan(p, o);
            let found = if c.can_replace.test(cx.r, p) {
                // Surface: descend as long as one cannot place.
                let mut q = p;
                let mut limit = step;
                let mut r = None;
                while q[1] > cx.r.min_y + 1 && limit > 0 {
                    limit -= 1;
                    if columns_can_place_at(cx, c, q) {
                        r = Some(q);
                        break;
                    }
                    q[1] -= 1;
                }
                r
            } else {
                // Air: climb to the first air (stop on a forbidden block).
                let mut q = p;
                let mut limit = step;
                let mut r = None;
                while q[1] <= cx.r.max_y() && limit > 0 {
                    limit -= 1;
                    let s = cx.r.get_p(q);
                    if c.cannot_place_on.contains(b.block(s)) {
                        break;
                    }
                    if b.is_air(s) {
                        r = Some(q);
                        break;
                    }
                    q[1] += 1;
                }
                r
            };
            if let Some(mut q) = found {
                let mut left = height - step / 2;
                while left >= 0 {
                    if c.can_replace.test(cx.r, q) {
                        let s = c.block.get(cx.r, rng, q);
                        cx.r.set_p(q, s, 3);
                        q[1] += 1;
                        any = true;
                    } else {
                        if !c.continue_through.test(cx.r, q) {
                            break;
                        }
                        q[1] += 1;
                    }
                    left -= 1;
                }
            }
        }
    }
    any
}

pub fn block_pile(cx: &mut Ctx, pr: &Provider, rng: &mut DecoRng, o: [i32; 3]) -> bool {
    let b = cx.r.b;
    if o[1] < cx.r.min_y + 5 {
        return false;
    }
    let xr = 2 + rng.next_int_bounded(2);
    let zr = 2 + rng.next_int_bounded(2);
    let path = if crate::opt::decor_ids() {
        Some(b.k.dirt_path).filter(|&k| k != crate::decor_blocks::BlockId::MAX)
    } else {
        b.block_id("dirt_path")
    };
    for z in o[2] - zr..=o[2] + zr {
        for y in o[1]..=o[1] + 1 {
            for x in o[0] - xr..=o[0] + xr {
                let (xd, zd) = (o[0] - x, o[2] - z);
                let lim = rng.next_float() * 10.0 - rng.next_float() * 6.0;
                let place = if (xd * xd + zd * zd) as f32 <= lim { true } else { f64::from(rng.next_float()) < 0.031 };
                if place {
                    let p = [x, y, z];
                    if b.is_air(cx.r.get_p(p)) {
                        let below = cx.r.get_p(add(p, Dir::Down));
                        let ok = if Some(b.block(below)) == path { rng.next_bool() } else { b.sturdy(below, Dir::Up) };
                        if ok {
                            let s = pr.get(cx.r, rng, p);
                            cx.r.set_p(p, s, 260);
                        }
                    }
                }
            }
        }
    }
    true
}
