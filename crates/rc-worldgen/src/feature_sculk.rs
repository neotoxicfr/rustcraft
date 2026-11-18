//! Multiface spreading (lichen, sculk veins) and sculk patches (`generation-decoration.md`
//! § 6.9.1–6.9.2).

use crate::decor::Ctx;
use crate::decor_blocks::{BlockId, Blocks, Dir, State};
use crate::decor_region::{Region, add};
use crate::decor_rng::DecoRng;
use crate::decor_rules::{can_attach_to, face_prop, multiface_has};

/// Setting of a multiface spreader.
#[derive(Debug, Clone, Copy)]
pub struct Spreader {
    pub block: BlockId,
    /// Rules of the sculk veins (replacement, other valid source block).
    pub vein: bool,
    /// Only the same cell ("same space" spreader of the veins).
    pub same_only: bool,
}

impl Spreader {
    pub fn new(b: &Blocks, block: BlockId) -> Self {
        Spreader { block, vein: block == b.k.sculk_vein, same_only: false }
    }

    fn other_valid_source(&self, b: &Blocks, s: State) -> bool {
        self.vein && !b.is(s, b.k.sculk_vein)
    }

    fn can_spread_from(&self, b: &Blocks, s: State, face: Dir) -> bool {
        self.other_valid_source(b, s) || multiface_has(b, s, face)
    }

    fn state_replaceable(&self, r: &Region, source: [i32; 3], pos: [i32; 3], face: Dir, ex: State) -> bool {
        let b = r.b;
        if self.vein {
            let against = r.get_p(add(pos, face));
            if b.is(against, b.k.sculk) || b.is(against, b.k.sculk_catalyst) || b.is(against, b.k.moving_piston) {
                return false;
            }
            let man = (source[0] - pos[0]).abs() + (source[1] - pos[1]).abs() + (source[2] - pos[2]).abs();
            if man == 2 {
                let n = add(source, face.opposite());
                if b.sturdy(r.get_p(n), face) {
                    return false;
                }
            }
            let f = b.fluid(ex);
            if f != 0 && f != 1 {
                return false;
            }
            if b.in_tag(ex, b.rules.fire) {
                return false;
            }
            if b.replaceable(ex) {
                return true;
            }
        }
        b.is_air(ex) || b.is(ex, self.block) || (b.is(ex, b.k.water) && b.is_source(ex))
    }

    fn valid_for_placement(&self, r: &Region, ex: State, pos: [i32; 3], face: Dir) -> bool {
        let b = r.b;
        if b.is(ex, self.block) && multiface_has(b, ex, face) {
            return false;
        }
        can_attach_to(b, r.get_p(add(pos, face)), face)
    }

    /// The state at the placement of the multiface block.
    pub fn state_for(&self, r: &Region, old: State, pos: [i32; 3], face: Dir) -> Option<State> {
        if !self.valid_for_placement(r, old, pos, face) {
            return None;
        }
        let b = r.b;
        let base = if b.is(old, self.block) {
            old
        } else if b.water_source(old) {
            b.with_bool(b.default_state(self.block), b.p.waterlogged, true)
        } else {
            b.default_state(self.block)
        };
        Some(b.with_bool(base, face_prop(b, face), true))
    }

    fn spread_target(&self, r: &Region, state: State, pos: [i32; 3], from: Dir, dir: Dir) -> Option<([i32; 3], Dir)> {
        let b = r.b;
        if dir.axis() == from.axis() {
            return None;
        }
        if !(self.other_valid_source(b, state) || (multiface_has(b, state, from) && !multiface_has(b, state, dir))) {
            return None;
        }
        let cands = [(pos, dir), (add(pos, dir), from), (add(add(pos, dir), from), dir.opposite())];
        let n = if self.same_only { 1 } else { 3 };
        for &(q, face) in &cands[..n] {
            let ex = r.get_p(q);
            if self.state_replaceable(r, pos, q, face, ex) && self.valid_for_placement(r, ex, q, face) {
                return Some((q, face));
            }
        }
        None
    }

    /// Placement of a spread face; at generation (only use here), the cell is first marked
    /// for post-processing.
    fn spread_to(&self, r: &mut Region, q: [i32; 3], face: Dir) -> bool {
        let old = r.get_p(q);
        match self.state_for(r, old, q, face) {
            Some(s) => {
                r.mark_post_process(q);
                r.set_p(q, s, 2)
            }
            None => false,
        }
    }

    /// Spreading from a face towards a random direction.
    pub fn spread_random(&self, r: &mut Region, state: State, pos: [i32; 3], from: Dir, rng: &mut DecoRng) -> bool {
        let mut dirs = Dir::ALL;
        rng.shuffle(&mut dirs);
        for d in dirs {
            if let Some((q, face)) = self.spread_target(r, state, pos, from, d)
                && self.spread_to(r, q, face)
            {
                return true;
            }
        }
        false
    }

    /// Number of faces placed.
    pub fn spread_all(&self, r: &mut Region, state: State, pos: [i32; 3]) -> u32 {
        let b = r.b;
        let mut n = 0;
        for face in Dir::ALL {
            if !self.can_spread_from(b, state, face) {
                continue;
            }
            for d in Dir::ALL {
                if let Some((q, f)) = self.spread_target(r, state, pos, face, d)
                    && self.spread_to(r, q, f)
                {
                    n += 1;
                }
            }
        }
        n
    }
}

// ---------------------------------------------------------------- Sculk patches

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Beh {
    Default,
    Sculk,
    Vein,
}

fn behaviour(b: &Blocks, s: State) -> Beh {
    if b.is(s, b.k.sculk) {
        Beh::Sculk
    } else if b.is(s, b.k.sculk_vein) {
        Beh::Vein
    } else {
        Beh::Default
    }
}

#[derive(Debug, Clone, Copy)]
struct Cursor {
    pos: [i32; 3],
    charge: i32,
    update_delay: i32,
    decay_delay: i32,
    /// Known faces (bits per direction); `None` = unknown.
    facings: Option<u8>,
}

const GROWTH_COST: i32 = 50;
const NO_GROWTH_RADIUS: i32 = 1;
const DECAY_RATE: i32 = 5;
const ADDITIONAL_DECAY: i32 = 10;

fn faces_of(b: &Blocks, s: State) -> u8 {
    let mut m = 0;
    if b.is(s, b.k.sculk_vein) || b.is(s, b.k.glow_lichen) {
        for d in Dir::ALL {
            if multiface_has(b, s, d) {
                m |= 1 << d as u8;
            }
        }
    }
    m
}

fn vein_spreader(b: &Blocks) -> Spreader {
    Spreader { block: b.k.sculk_vein, vein: true, same_only: false }
}

fn regrow(r: &mut Region, pos: [i32; 3], existing: State, faces: u8) -> bool {
    let b = r.b;
    let mut st = b.default_state(b.k.sculk_vein);
    let mut any = false;
    for d in Dir::ALL {
        if faces >> d as u8 & 1 != 0 && can_attach_to(b, r.get_p(add(pos, d)), d) {
            st = b.with_bool(st, face_prop(b, d), true);
            any = true;
        }
    }
    if !any {
        return false;
    }
    if b.fluid(existing) != 0 {
        st = b.with_bool(st, b.p.waterlogged, true);
    }
    r.set_p(pos, st, 3);
    true
}

fn attempt_spread_vein(r: &mut Region, beh: Beh, pos: [i32; 3], state: State, facings: Option<u8>) -> bool {
    let b = r.b;
    match beh {
        Beh::Default => match facings {
            None => {
                let mut sp = vein_spreader(b);
                sp.same_only = true;
                let cur = r.get_p(pos);
                sp.spread_all(r, cur, pos) > 0
            }
            Some(f) if f != 0 => {
                if !b.is_air(state) && b.fluid(state) != 1 {
                    false
                } else {
                    regrow(r, pos, state, f)
                }
            }
            Some(_) => vein_spreader(b).spread_all(r, state, pos) > 0,
        },
        _ => vein_spreader(b).spread_all(r, state, pos) > 0,
    }
}

fn on_discharged(r: &mut Region, beh: Beh, mut state: State, pos: [i32; 3]) {
    let b = r.b;
    if beh != Beh::Vein || !b.is(state, b.k.sculk_vein) {
        return;
    }
    for d in Dir::ALL {
        if multiface_has(b, state, d) && b.is(r.get_p(add(pos, d)), b.k.sculk) {
            state = b.with_bool(state, face_prop(b, d), false);
        }
    }
    if !Dir::ALL.iter().any(|d| multiface_has(b, state, *d)) {
        state = if b.fluid(r.get_p(pos)) == 0 { b.default_state(b.k.air) } else { b.default_state(b.k.water) };
    }
    r.set_p(pos, state, 3);
}

fn dist_sqr(a: [i32; 3], c: [i32; 3]) -> f64 {
    let dx = f64::from(a[0] - c[0]);
    let dy = f64::from(a[1] - c[1]);
    let dz = f64::from(a[2] - c[2]);
    dx * dx + dy * dy + dz * dz
}

fn can_place_growth(r: &Region, pos: [i32; 3]) -> bool {
    let b = r.b;
    let above = r.get_p(add(pos, Dir::Up));
    if !(b.is_air(above) || (b.is(above, b.k.water) && b.fluid(above) == 1)) {
        return false;
    }
    let mut n = 0;
    for x in pos[0] - 4..=pos[0] + 4 {
        for y in pos[1]..=pos[1] + 2 {
            for z in pos[2] - 4..=pos[2] + 4 {
                if b.in_tag(r.get(x, y, z), b.rules.sculk_growth_inhibitors) {
                    n += 1;
                    if n > 2 {
                        return false;
                    }
                }
            }
        }
    }
    true
}

fn attempt_place_sculk(r: &mut Region, pos: [i32; 3], rng: &mut DecoRng) -> bool {
    let b = r.b;
    let state = r.get_p(pos);
    let mut dirs = Dir::ALL;
    rng.shuffle(&mut dirs);
    for support in dirs {
        if !multiface_has(b, state, support) {
            continue;
        }
        let sp = add(pos, support);
        if b.in_tag(r.get_p(sp), b.rules.sculk_replaceable_world_gen) {
            let sculk = b.default_state(b.k.sculk);
            r.set_p(sp, sculk, 3);
            vein_spreader(b).spread_all(r, sculk, sp);
            let skip = support.opposite();
            for d in Dir::ALL {
                if d == skip {
                    continue;
                }
                let vp = add(sp, d);
                let vs = r.get_p(vp);
                if b.is(vs, b.k.sculk_vein) {
                    on_discharged(r, Beh::Vein, vs, vp);
                }
            }
            return true;
        }
    }
    false
}

fn use_charge(r: &mut Region, beh: Beh, c: &Cursor, origin: [i32; 3], rng: &mut DecoRng, veins: bool) -> i32 {
    let b = r.b;
    match beh {
        Beh::Default => {
            if c.decay_delay > 0 {
                c.charge
            } else {
                0
            }
        }
        Beh::Vein => {
            if veins && attempt_place_sculk(r, c.pos, rng) {
                c.charge - 1
            } else if rng.next_int_bounded(DECAY_RATE) == 0 {
                (c.charge as f32 * 0.5).floor() as i32
            } else {
                c.charge
            }
        }
        Beh::Sculk => {
            let charge = c.charge;
            if charge != 0 && rng.next_int_bounded(DECAY_RATE) == 0 {
                let p = c.pos;
                let close = dist_sqr(p, origin) < f64::from(NO_GROWTH_RADIUS * NO_GROWTH_RADIUS);
                if !close && can_place_growth(r, p) {
                    if rng.next_int_bounded(GROWTH_COST) < charge {
                        let gp = add(p, Dir::Up);
                        let mut st = if rng.next_int_bounded(11) == 0 {
                            b.with_bool(b.default_state(b.k.sculk_shrieker), b.p.can_summon, true)
                        } else {
                            b.default_state(b.k.sculk_sensor)
                        };
                        if b.has(st, b.p.waterlogged) && b.fluid(r.get_p(gp)) != 0 {
                            st = b.with_bool(st, b.p.waterlogged, true);
                        }
                        r.set_p(gp, st, 3);
                    }
                    0.max(charge - GROWTH_COST)
                } else if rng.next_int_bounded(ADDITIONAL_DECAY) != 0 {
                    charge
                } else {
                    let pen = if close {
                        1
                    } else {
                        let outer = (dist_sqr(p, origin).sqrt() as f32 - NO_GROWTH_RADIUS as f32).powi(2);
                        let reach = ((24 - NO_GROWTH_RADIUS) * (24 - NO_GROWTH_RADIUS)) as f32;
                        let factor = (outer / reach).min(1.0);
                        1.max((charge as f32 * factor * 0.5) as i32)
                    };
                    charge - pen
                }
            } else {
                charge
            }
        }
    }
}

/// "Cornerless" offsets of the 3×3×3 cuboid (x, then y, then z order).
fn non_corner() -> Vec<[i32; 3]> {
    let mut v = Vec::with_capacity(18);
    for z in -1..=1 {
        for y in -1..=1 {
            for x in -1..=1 {
                if (x == 0 || y == 0 || z == 0) && !(x == 0 && y == 0 && z == 0) {
                    v.push([x, y, z]);
                }
            }
        }
    }
    v
}

fn unobstructed(r: &Region, from: [i32; 3], d: Dir) -> bool {
    let t = add(from, d);
    !r.b.sturdy(r.get_p(t), d.opposite())
}

fn movement_ok(r: &Region, from: [i32; 3], to: [i32; 3]) -> bool {
    let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    if d[0].abs() + d[1].abs() + d[2].abs() == 1 {
        return true;
    }
    let dx = if d[0] < 0 { Dir::West } else { Dir::East };
    let dy = if d[1] < 0 { Dir::Down } else { Dir::Up };
    let dz = if d[2] < 0 { Dir::North } else { Dir::South };
    if d[0] == 0 {
        unobstructed(r, from, dy) || unobstructed(r, from, dz)
    } else if d[1] == 0 {
        unobstructed(r, from, dx) || unobstructed(r, from, dz)
    } else {
        unobstructed(r, from, dx) || unobstructed(r, from, dy)
    }
}

fn substrate_access(r: &Region, s: State, pos: [i32; 3]) -> bool {
    let b = r.b;
    if !b.is(s, b.k.sculk_vein) {
        return false;
    }
    Dir::ALL.iter().any(|d| multiface_has(b, s, *d) && b.in_tag(r.get_p(add(pos, *d)), b.rules.sculk_replaceable))
}

fn movement_pos(
    r: &Region,
    pos: [i32; 3],
    origin: [i32; 3],
    rng: &mut DecoRng,
    offsets: &mut [[i32; 3]],
) -> Option<[i32; 3]> {
    let b = r.b;
    rng.shuffle(offsets);
    let mut best = pos;
    for o in offsets.iter() {
        let n = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
        let dsq = (origin[0] - n[0]).pow(2) + (origin[2] - n[2]).pow(2);
        if dsq > 144 {
            continue;
        }
        let t = r.get_p(n);
        if behaviour(b, t) != Beh::Default && movement_ok(r, pos, n) {
            best = n;
            if substrate_access(r, t, n) {
                break;
            }
        }
    }
    if best == pos { None } else { Some(best) }
}

#[allow(clippy::too_many_arguments)]
fn update_cursor(
    r: &mut Region,
    c: &mut Cursor,
    origin: [i32; 3],
    rng: &mut DecoRng,
    veins: bool,
    offsets: &mut [[i32; 3]],
) {
    let b = r.b;
    if c.charge <= 0 {
        return;
    }
    if c.update_delay > 0 {
        c.update_delay -= 1;
        return;
    }
    let mut state = r.get_p(c.pos);
    let mut beh = behaviour(b, state);
    if veins && attempt_spread_vein(r, beh, c.pos, state, c.facings) && beh != Beh::Sculk {
        state = r.get_p(c.pos);
        beh = behaviour(b, state);
    }
    c.charge = use_charge(r, beh, c, origin, rng, veins);
    if c.charge <= 0 {
        on_discharged(r, beh, state, c.pos);
        return;
    }
    match movement_pos(r, c.pos, origin, rng, offsets) {
        Some(t) => {
            on_discharged(r, beh, state, c.pos);
            c.pos = t;
            state = r.get_p(t);
        }
        None => {
            on_discharged(r, beh, state, c.pos);
            c.charge = 0;
            return;
        }
    }
    if behaviour(b, state) != Beh::Default {
        c.facings = Some(faces_of(b, state));
    }
    c.decay_delay = if beh == Beh::Default { (c.decay_delay - 1).max(0) } else { 1 };
    c.update_delay = 1;
}

pub fn sculk_patch(cx: &mut Ctx, cfg: &crate::feature::SculkPatch, rng: &mut DecoRng, o: [i32; 3]) -> bool {
    let r = &mut *cx.r;
    let b = r.b;
    let start = r.get_p(o);
    let ok = if behaviour(b, start) != Beh::Default {
        true
    } else if !b.is_air(start) && !(b.is(start, b.k.water) && b.is_source(start)) {
        false
    } else {
        Dir::ALL.iter().any(|d| b.facts(r.get_p(add(o, *d))).collision_full)
    };
    if !ok {
        return false;
    }
    let mut offsets = non_corner();
    let base = non_corner();
    let mut cursors: Vec<Cursor> = Vec::with_capacity(32);
    for round in 0..cfg.spread_rounds + cfg.growth_rounds {
        for _ in 0..cfg.charge_count {
            let mut charge = cfg.amount_per_charge;
            while charge > 0 {
                let cur = charge.min(1000);
                if cursors.len() < 32 {
                    cursors.push(Cursor { pos: o, charge: cur, update_delay: 0, decay_delay: 1, facings: None });
                }
                charge -= cur;
            }
        }
        let veins = round < cfg.spread_rounds;
        for _ in 0..cfg.spread_attempts {
            if cursors.is_empty() {
                continue;
            }
            for c in cursors.iter_mut() {
                let far = (c.pos[0] - o[0]).abs().max((c.pos[1] - o[1]).abs()).max((c.pos[2] - o[2]).abs()) > 1024;
                if far {
                    c.charge = 0;
                    continue;
                }
                offsets.copy_from_slice(&base);
                update_cursor(r, c, o, rng, veins, &mut offsets);
            }
            cursors.retain(|c| c.charge > 0);
        }
        cursors.clear();
    }
    true
}
