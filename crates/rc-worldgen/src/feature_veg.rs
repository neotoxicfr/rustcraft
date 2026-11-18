//! Vegetation: multiface, vegetation carpets, root systems, piles, pillars, vines,
//! bamboo, giant mushrooms, corals (`generation-decoration.md` § 6.2, 6.5, 6.9).

use crate::decor::Ctx;
use crate::decor_blocks::{Blocks, Dir, State};
use crate::decor_region::{Region, add};
use crate::decor_rng::DecoRng;
use crate::decor_rules::{can_attach_to, face_prop};
use crate::feature::{HugeMushroom, Multiface, PlacedId, RootSystem, VegPatch};

fn air_or_water(b: &Blocks, s: State) -> bool {
    b.is_air(s) || b.is(s, b.k.water)
}

pub fn multiface(cx: &mut Ctx, m: &Multiface, rng: &mut DecoRng, p: [i32; 3]) -> bool {
    let b = cx.r.b;
    if !air_or_water(b, cx.r.get_p(p)) {
        return false;
    }
    // Valid directions without allocation (at most 6).
    let mut valid = [Dir::Up; 6];
    let mut nv = 0;
    let mut push = |d: Dir| {
        valid[nv] = d;
        nv += 1;
    };
    if m.ceiling {
        push(Dir::Up);
    }
    if m.floor {
        push(Dir::Down);
    }
    if m.wall {
        Dir::NESW.iter().for_each(|d| push(*d));
    }
    let mut dirs = valid;
    rng.shuffle(&mut dirs[..nv]);
    let cur = cx.r.get_p(p);
    if multiface_try(cx, m, p, cur, rng, &dirs[..nv]) {
        return true;
    }
    for &sd in &dirs[..nv] {
        let mut pd = [Dir::Up; 6];
        let mut np = 0;
        for &d in &valid[..nv] {
            if d != sd.opposite() {
                pd[np] = d;
                np += 1;
            }
        }
        let pd = &mut pd[..np];
        rng.shuffle(pd);
        for _ in 0..m.search_range {
            let q = add(p, sd);
            let s = cx.r.get_p(q);
            if !air_or_water(b, s) && !b.is(s, m.block) {
                break;
            }
            if multiface_try(cx, m, q, s, rng, pd) {
                return true;
            }
        }
    }
    false
}

fn multiface_try(cx: &mut Ctx, m: &Multiface, p: [i32; 3], old: State, rng: &mut DecoRng, dirs: &[Dir]) -> bool {
    let b = cx.r.b;
    for &d in dirs {
        if m.can_be_placed_on.contains(b.block(cx.r.get_p(add(p, d)))) {
            let sp = crate::feature_sculk::Spreader::new(b, m.block);
            let Some(ns) = sp.state_for(cx.r, old, p, d) else { return false };
            cx.r.set_p(p, ns, 3);
            cx.r.mark_post_process(p);
            if rng.next_float() < m.chance {
                sp.spread_random(cx.r, ns, p, d, rng);
            }
            return true;
        }
    }
    false
}

pub fn vegetation_patch(cx: &mut Ctx, v: &VegPatch, rng: &mut DecoRng, o: [i32; 3]) -> bool {
    let b = cx.r.b;
    let xr = v.xz_radius.sample(rng) + 1;
    let zr = v.xz_radius.sample(rng) + 1;
    let inward = if v.ceiling { Dir::Up } else { Dir::Down };
    let outward = inward.opposite();
    let mut surface = crate::tree::JSet::default();
    for dx in -xr..=xr {
        let xe = dx == -xr || dx == xr;
        for dz in -zr..=zr {
            let ze = dz == -zr || dz == zr;
            let edge = xe || ze;
            let corner = xe && ze;
            let edge_only = edge && !corner;
            if corner || (edge_only && (v.extra_edge == 0.0 || rng.next_float() > v.extra_edge)) {
                continue;
            }
            let mut p = [o[0] + dx, o[1], o[2] + dz];
            let mut k = 0;
            while b.is_air(cx.r.get_p(p)) && k < v.vertical_range {
                p = add(p, inward);
                k += 1;
            }
            k = 0;
            while !b.is_air(cx.r.get_p(p)) && k < v.vertical_range {
                p = add(p, outward);
                k += 1;
            }
            let below = add(p, inward);
            let bs = cx.r.get_p(below);
            if b.is_air(cx.r.get_p(p)) && b.sturdy(bs, outward) {
                let extra = v.extra_bottom > 0.0 && rng.next_float() < v.extra_bottom;
                let depth = v.depth.sample(rng) + i32::from(extra);
                if veg_ground(cx, v, rng, below, depth, inward) {
                    surface.insert(below);
                }
            }
        }
    }
    let list: Vec<[i32; 3]> = if v.waterlogged {
        let all: Vec<[i32; 3]> = surface.iter().collect();
        let mut water = crate::tree::JSet::default();
        for &s in &all {
            let exposed = [Dir::North, Dir::East, Dir::South, Dir::West, Dir::Down]
                .iter()
                .any(|d| !b.sturdy(cx.r.get_p(add(s, *d)), d.opposite()));
            if !exposed {
                water.insert(s);
            }
        }
        let w = b.default_state(b.k.water);
        let list: Vec<[i32; 3]> = water.iter().collect();
        for &s in &list {
            cx.r.set_p(s, w, 2);
        }
        list
    } else {
        surface.iter().collect()
    };
    for &s in &list {
        if v.vegetation_chance > 0.0 && rng.next_float() < v.vegetation_chance {
            if v.waterlogged {
                let at = add(add(s, Dir::Down), outward);
                if crate::placement::place(cx, v.vegetation, rng, at, None) {
                    let placed = cx.r.get_p(s);
                    if b.get_bool(placed, b.p.waterlogged) == Some(false) {
                        cx.r.set_p(s, b.with_bool(placed, b.p.waterlogged, true), 2);
                    }
                }
            } else {
                crate::placement::place(cx, v.vegetation, rng, add(s, outward), None);
            }
        }
    }
    !list.is_empty()
}

fn veg_ground(cx: &mut Ctx, v: &VegPatch, rng: &mut DecoRng, mut p: [i32; 3], depth: i32, inward: Dir) -> bool {
    let b = cx.r.b;
    for i in 0..depth {
        let s = v.ground.get(cx.r, rng, p);
        let cur = cx.r.get_p(p);
        if b.block(s) != b.block(cur) {
            if !b.in_tag(cur, v.replaceable) {
                return i != 0;
            }
            cx.r.set_p(p, s, 2);
            p = add(p, inward);
        }
    }
    true
}

pub fn block_blob(
    cx: &mut Ctx,
    state: State,
    on: &crate::decor_pred::Pred,
    rng: &mut DecoRng,
    mut o: [i32; 3],
) -> bool {
    while o[1] > cx.r.min_y + 3 && !on.test(cx.r, add(o, Dir::Down)) {
        o = add(o, Dir::Down);
    }
    if o[1] <= cx.r.min_y + 3 {
        return false;
    }
    for _ in 0..3 {
        let xr = rng.next_int_bounded(2);
        let yr = rng.next_int_bounded(2);
        let zr = rng.next_int_bounded(2);
        let tr = (xr + yr + zr) as f32 * 0.333 + 0.5;
        let t2 = f64::from(tr * tr);
        for z in o[2] - zr..=o[2] + zr {
            for y in o[1] - yr..=o[1] + yr {
                for x in o[0] - xr..=o[0] + xr {
                    let (dx, dy, dz) = (f64::from(x - o[0]), f64::from(y - o[1]), f64::from(z - o[2]));
                    if dx * dx + dy * dy + dz * dz <= t2 {
                        cx.r.set(x, y, z, state, 3);
                    }
                }
            }
        }
        let a = -1 + rng.next_int_bounded(2);
        let c = -rng.next_int_bounded(2);
        let d = -1 + rng.next_int_bounded(2);
        o = [o[0] + a, o[1] + c, o[2] + d];
    }
    true
}

fn space_for_tree(r: &Region, v: &RootSystem, p: [i32; 3]) -> bool {
    let b = r.b;
    for i in 1..=v.required_space {
        let s = r.get(p[0], p[1] + i, p[2]);
        if !b.is_air(s) && !(i < v.allowed_water && b.fluid(s) == 1) {
            return false;
        }
    }
    if v.level_test_distance > 0 {
        for d in Dir::H2D {
            let c = crate::decor_region::add_n(p, d, v.level_test_distance);
            let below = r.get(c[0], c[1] - v.max_level_deviation, c[2]);
            let above = r.get(c[0], c[1] + v.max_level_deviation, c[2]);
            if b.is_air(below) || !b.is_air(above) {
                return false;
            }
        }
    }
    true
}

pub fn root_system(cx: &mut Ctx, v: &RootSystem, rng: &mut DecoRng, o: [i32; 3]) -> bool {
    let b = cx.r.b;
    if !b.is_air(cx.r.get_p(o)) {
        return false;
    }
    let mut w = o;
    let mut placed = false;
    for y in 0..v.root_column_max_height {
        w = add(w, Dir::Up);
        if cx.r.height_at(crate::decor_region::Hm::WorldSurface, w[0], w[2]) < w[1] {
            break;
        }
        if v.allowed_tree_position.test(cx.r, w) && space_for_tree(cx.r, v, w) {
            let below = add(w, Dir::Down);
            let bs = cx.r.get_p(below);
            if b.fluid(bs) == 2 || !b.legacy_solid(bs) {
                break;
            }
            if crate::placement::place(cx, v.feature, rng, w, None) {
                for yy in o[1]..o[1] + y {
                    for _ in 0..v.root_attempts {
                        let dx = rng.next_int_bounded(v.root_radius) - rng.next_int_bounded(v.root_radius);
                        let dz = rng.next_int_bounded(v.root_radius) - rng.next_int_bounded(v.root_radius);
                        let q = [o[0] + dx, yy, o[2] + dz];
                        if b.in_tag(cx.r.get_p(q), v.root_replaceable) {
                            let s = v.root_provider.get(cx.r, rng, q);
                            cx.r.set_p(q, s, 2);
                        }
                    }
                }
                placed = true;
                break;
            }
        }
    }
    if placed {
        for _ in 0..v.hanging_attempts {
            let dx = rng.next_int_bounded(v.hanging_radius) - rng.next_int_bounded(v.hanging_radius);
            let dy = rng.next_int_bounded(v.hanging_span) - rng.next_int_bounded(v.hanging_span);
            let dz = rng.next_int_bounded(v.hanging_radius) - rng.next_int_bounded(v.hanging_radius);
            let q = [o[0] + dx, o[1] + dy, o[2] + dz];
            if b.is_air(cx.r.get_p(q)) {
                let s = v.hanging_provider.get(cx.r, rng, q);
                if crate::decor_rules::can_survive(cx.r, s, q) && b.sturdy(cx.r.get_p(add(q, Dir::Up)), Dir::Down) {
                    cx.r.set_p(q, s, 2);
                }
            }
        }
    }
    true
}

pub fn vines(cx: &mut Ctx, p: [i32; 3]) -> bool {
    let b = cx.r.b;
    if !b.is_air(cx.r.get_p(p)) {
        return false;
    }
    for d in Dir::ALL {
        if d != Dir::Down && can_attach_to(b, cx.r.get_p(add(p, d)), d) {
            let s = b.with_bool(b.default_state(b.k.vine), face_prop(b, d), true);
            cx.r.set_p(p, s, 2);
            return true;
        }
    }
    false
}

pub fn bamboo(cx: &mut Ctx, prob: f32, rng: &mut DecoRng, o: [i32; 3]) -> bool {
    let r = &mut *cx.r;
    let b = r.b;
    if !b.is_air(r.get_p(o)) {
        return false;
    }
    let bamboo = b.default_state(b.k.bamboo);
    if crate::decor_rules::can_survive(r, bamboo, o) {
        let height = rng.next_int_bounded(12) + 5;
        if rng.next_float() < prob {
            let rad = rng.next_int_bounded(4) + 1;
            let tag = if crate::opt::decor_ids() {
                b.rules.beneath_bamboo
            } else {
                b.tag("beneath_bamboo_podzol_replaceable").unwrap_or(u16::MAX)
            };
            let podzol = b.default_state(b.k.podzol);
            for xx in o[0] - rad..=o[0] + rad {
                for zz in o[2] - rad..=o[2] + rad {
                    let (xd, zd) = (xx - o[0], zz - o[2]);
                    if xd * xd + zd * zd <= rad * rad {
                        let y = r.height_at(crate::decor_region::Hm::WorldSurface, xx, zz) - 1;
                        if tag != u16::MAX && b.in_tag(r.get(xx, y, zz), tag) {
                            r.set(xx, y, zz, podzol, 2);
                        }
                    }
                }
            }
        }
        let trunk = b.with_int(b.with_str(b.with_int(bamboo, b.p.age, 1), b.p.leaves, "none"), b.p.stage, 0);
        let mut p = o;
        let mut i = 0;
        while i < height && b.is_air(r.get_p(p)) {
            r.set_p(p, trunk, 2);
            p = add(p, Dir::Up);
            i += 1;
        }
        if p[1] - o[1] >= 3 {
            let final_large = b.with_int(b.with_str(trunk, b.p.leaves, "large"), b.p.stage, 1);
            r.set_p(p, final_large, 2);
            p = add(p, Dir::Down);
            r.set_p(p, b.with_str(trunk, b.p.leaves, "large"), 2);
            p = add(p, Dir::Down);
            r.set_p(p, b.with_str(trunk, b.p.leaves, "small"), 2);
        }
    }
    true
}

fn mushroom_block(r: &mut Region, p: [i32; 3], s: State) {
    let b = r.b;
    let cur = r.get_p(p);
    if b.is_air(cur) || b.in_tag(cur, b.rules.replaceable_by_mushrooms) {
        r.set_p(p, s, 3);
    }
}

pub fn huge_mushroom(cx: &mut Ctx, m: &HugeMushroom, rng: &mut DecoRng, o: [i32; 3]) -> bool {
    let b = cx.r.b;
    let mut h = rng.next_int_bounded(3) + 4;
    if rng.next_int_bounded(12) == 0 {
        h *= 2;
    }
    // Valid position ("red" radius always zero: quirk of the game, height passed as −1).
    if o[1] < cx.r.min_y + 1 || o[1] + h + 1 > cx.r.max_y() {
        return false;
    }
    if !m.can_place_on.test(cx.r, add(o, Dir::Down)) {
        return false;
    }
    let leaves = b.rules.leaves;
    for dy in 0..=h {
        let rad = if !m.red && dy > 3 { m.foliage_radius } else { 0 };
        for dx in -rad..=rad {
            for dz in -rad..=rad {
                let s = cx.r.get(o[0] + dx, o[1] + dy, o[2] + dz);
                if !b.is_air(s) && !b.in_tag(s, leaves) {
                    return false;
                }
            }
        }
    }
    let r = &mut *cx.r;
    let fr = m.foliage_radius;
    let (w, e, n, s_, u) = (b.p.west, b.p.east, b.p.north, b.p.south, b.p.up);
    if m.red {
        for dy in h - 3..=h {
            let rad = if dy < h { fr } else { fr - 1 };
            let center = fr - 2;
            for dx in -rad..=rad {
                for dz in -rad..=rad {
                    let xe = dx == -rad || dx == rad;
                    let ze = dz == -rad || dz == rad;
                    if dy >= h || xe != ze {
                        let mut st = m.cap.get(r, rng, o);
                        if b.has(st, w) && b.has(st, e) && b.has(st, n) && b.has(st, s_) && b.has(st, u) {
                            st = b.with_bool(st, u, dy >= h - 1);
                            st = b.with_bool(st, w, dx < -center);
                            st = b.with_bool(st, e, dx > center);
                            st = b.with_bool(st, n, dz < -center);
                            st = b.with_bool(st, s_, dz > center);
                        }
                        mushroom_block(r, [o[0] + dx, o[1] + dy, o[2] + dz], st);
                    }
                }
            }
        }
    } else {
        for dx in -fr..=fr {
            for dz in -fr..=fr {
                let (minx, maxx, minz, maxz) = (dx == -fr, dx == fr, dz == -fr, dz == fr);
                let xe = minx || maxx;
                let ze = minz || maxz;
                if !xe || !ze {
                    let west = minx || (ze && dx == 1 - fr);
                    let east = maxx || (ze && dx == fr - 1);
                    let north = minz || (xe && dz == 1 - fr);
                    let south = maxz || (xe && dz == fr - 1);
                    let mut st = m.cap.get(r, rng, o);
                    if b.has(st, w) && b.has(st, e) && b.has(st, n) && b.has(st, s_) {
                        st = b.with_bool(st, w, west);
                        st = b.with_bool(st, e, east);
                        st = b.with_bool(st, n, north);
                        st = b.with_bool(st, s_, south);
                    }
                    mushroom_block(r, [o[0] + dx, o[1] + h, o[2] + dz], st);
                }
            }
        }
    }
    for dy in 0..h {
        let st = m.stem.get(r, rng, o);
        mushroom_block(r, [o[0], o[1] + dy, o[2]], st);
    }
    true
}

fn coral_part(cx: &mut Ctx, f: PlacedId, rng: &mut DecoRng, p: [i32; 3]) -> bool {
    crate::placement::place(cx, f, rng, p, None)
}

pub fn coral_tree(cx: &mut Ctx, f: PlacedId, rng: &mut DecoRng, o: [i32; 3]) -> bool {
    let mut p = o;
    let trunk = rng.next_int_bounded(3) + 1;
    for _ in 0..trunk {
        if !coral_part(cx, f, rng, p) {
            return true;
        }
        p = add(p, Dir::Up);
    }
    let top = p;
    let n = rng.next_int_bounded(3) + 2;
    let mut dirs = Dir::NESW;
    rng.shuffle(&mut dirs);
    for &d in dirs.iter().take(n as usize) {
        p = add(top, d);
        let h = rng.next_int_bounded(5) + 2;
        let mut seg = 0;
        let mut j = 0;
        while j < h && coral_part(cx, f, rng, p) {
            seg += 1;
            p = add(p, Dir::Up);
            if j == 0 || (seg >= 2 && rng.next_float() < 0.25) {
                p = add(p, d);
                seg = 0;
            }
            j += 1;
        }
    }
    true
}

pub fn coral_claw(cx: &mut Ctx, f: PlacedId, rng: &mut DecoRng, o: [i32; 3]) -> bool {
    if !coral_part(cx, f, rng, o) {
        return false;
    }
    let claw = Dir::NESW[rng.next_int_bounded(4) as usize];
    let n = rng.next_int_bounded(2) + 2;
    let mut dirs = [claw, claw.cw(), claw.ccw()];
    rng.shuffle(&mut dirs);
    for &bd in dirs.iter().take(n as usize) {
        let mut p = o;
        let side = rng.next_int_bounded(2) + 1;
        p = add(p, bd);
        let (seg, inway) = if bd == claw {
            (claw, rng.next_int_bounded(3) + 2)
        } else {
            p = add(p, Dir::Up);
            let s = [bd, Dir::Up][rng.next_int_bounded(2) as usize];
            (s, rng.next_int_bounded(3) + 3)
        };
        let mut i = 0;
        while i < side && coral_part(cx, f, rng, p) {
            p = add(p, seg);
            i += 1;
        }
        p = add(p, seg.opposite());
        p = add(p, Dir::Up);
        for _ in 0..inway {
            p = add(p, claw);
            if !coral_part(cx, f, rng, p) {
                break;
            }
            if rng.next_float() < 0.25 {
                p = add(p, Dir::Up);
            }
        }
    }
    true
}

fn side_prop(b: &Blocks, d: Dir) -> crate::decor_blocks::Prop {
    face_prop(b, d)
}

/// Update of the state of a moss carpet.
fn mossy_updated(r: &Region, mut s: State, p: [i32; 3], mut create: bool) -> State {
    let b = r.b;
    let base = b.get_bool(s, b.p.bottom) == Some(true);
    create |= base;
    let carpet = b.k.pale_moss_carpet;
    for d in Dir::NESW {
        let prop = side_prop(b, d);
        let support = can_attach_to(b, r.get_p(add(p, d)), d);
        let mut side = if support { if create { "low" } else { b.get_str(s, prop).unwrap_or("none") } } else { "none" };
        if side == "low" {
            let above = r.get_p(add(p, Dir::Up));
            if b.is(above, carpet)
                && b.get_str(above, prop) != Some("none")
                && b.get_bool(above, b.p.bottom) == Some(false)
            {
                side = "tall";
            }
            if !base {
                let below = r.get_p(add(p, Dir::Down));
                if b.is(below, carpet) && b.get_str(below, prop) == Some("none") {
                    side = "none";
                }
            }
        }
        s = b.with_str(s, prop, side);
    }
    s
}

fn mossy_has_faces(b: &Blocks, s: State) -> bool {
    b.get_bool(s, b.p.bottom) == Some(true) || Dir::NESW.iter().any(|d| b.get_str(s, side_prop(b, *d)) != Some("none"))
}

/// Placement of a moss carpet (sides drawn with the region's randomness).
pub fn place_mossy_carpet(cx: &mut Ctx, p: [i32; 3]) {
    let r = &mut *cx.r;
    let b = r.b;
    let carpet = b.k.pale_moss_carpet;
    let simple = b.default_state(carpet);
    let adjusted = mossy_updated(r, simple, p, true);
    r.set_p(p, adjusted, 2);
    // Top layer.
    let up = add(p, Dir::Up);
    let prev = r.get_p(up);
    let is_carpet = b.is(prev, carpet);
    let mut top = b.default_state(b.k.air);
    if (!is_carpet || b.get_bool(prev, b.p.bottom) != Some(true)) && (is_carpet || b.replaceable(prev)) {
        let no_base = b.with_bool(simple, b.p.bottom, false);
        let mut st = mossy_updated(r, no_base, up, true);
        for d in Dir::NESW {
            let prop = side_prop(b, d);
            if b.get_str(st, prop) != Some("none") && !r.region_rng.next_bool() {
                st = b.with_str(st, prop, "none");
            }
        }
        if mossy_has_faces(b, st) && st != prev {
            top = st;
        }
    }
    if !b.is_air(top) {
        r.set_p(up, top, 2);
        let again = mossy_updated(r, adjusted, p, true);
        r.set_p(p, again, 2);
    }
}

pub fn place_pale_moss_patch(cx: &mut Ctx, rng: &mut DecoRng, p: [i32; 3]) {
    let f = if crate::opt::decor_ids() { cx.d.pale_moss_patch } else { cx.d.reg.feature_id("minecraft:pale_moss_patch") };
    if let Some(f) = f {
        crate::feature::place_feature(cx, f, rng, p);
    }
}
