//! Simple features: simple block, column, ores, disks, lakes, springs, underwater magma,
//! monster rooms, surface freezing (`generation-decoration.md` § 6.2–6.4).

use crate::decor::Ctx;
use crate::decor_blocks::{Blocks, Dir, State};
use crate::decor_pred::Provider;
use crate::decor_region::{BeData, Hm, Region, add};
use crate::decor_rng::DecoRng;
use crate::decor_rules::{Kind, can_survive};
use crate::feature::{BlockColumn, Disk, Lake, Ore, Spring};
use crate::math::round_half_up;

/// Identical fluid states (type, source, level, falling).
pub fn same_fluid(b: &Blocks, a: State, c: State) -> bool {
    let fa = b.facts(a);
    let fc = b.facts(c);
    if fa.fluid == 0 && fc.fluid == 0 {
        return true;
    }
    fa.fluid == fc.fluid
        && b.is_source(a) == b.is_source(c)
        && fa.fluid_amount == fc.fluid_amount
        && b.has_flag(a, rc_data::flags::FLUID_FALLING) == b.has_flag(c, rc_data::flags::FLUID_FALLING)
}

/// Waterlogged if water (any water fluid) is at p.
pub fn copy_waterlogged(r: &Region, p: [i32; 3], s: State) -> State {
    let b = r.b;
    if b.has(s, b.p.waterlogged) { b.with_bool(s, b.p.waterlogged, b.fluid(r.get_p(p)) == 1) } else { s }
}

pub fn is_double_plant(b: &Blocks, s: State) -> bool {
    matches!(b.kind(b.block(s)), Kind::DoublePlant | Kind::TallSeagrass | Kind::SmallDripleaf)
}

/// Placement of a double plant (lower then upper halves).
pub fn place_double(r: &mut Region, s: State, p: [i32; 3], flags: u32) {
    let b = r.b;
    let lower = copy_waterlogged(r, p, b.with_str(s, b.p.half, "lower"));
    r.set_p(p, lower, flags);
    let up = [p[0], p[1] + 1, p[2]];
    let upper = copy_waterlogged(r, up, b.with_str(s, b.p.half, "upper"));
    r.set_p(up, upper, flags);
}

pub fn simple_block(cx: &mut Ctx, provider: &Provider, schedule_tick: bool, rng: &mut DecoRng, p: [i32; 3]) -> bool {
    let r = &mut *cx.r;
    let b = r.b;
    let Some(s) = provider.get_opt(r, rng, p) else { return false };
    if !can_survive(r, s, p) {
        return false;
    }
    if is_double_plant(b, s) {
        let above = r.get(p[0], p[1] + 1, p[2]);
        if !b.is_air(above) && (!same_fluid(b, s, above) || !b.replaceable(above)) {
            return false;
        }
        place_double(r, s, p, 2);
    } else if b.is(s, b.k.pale_moss_carpet) {
        crate::feature_veg::place_mossy_carpet(cx, p);
    } else {
        r.set_p(p, s, 2);
    }
    if schedule_tick {
        // Block re-read after the placement; delay 1 (saved 0 in a proto-chunk, like the game).
        let block = b.block(cx.r.get_p(p));
        cx.r.schedule_block_tick(p, block, 1);
    }
    true
}

pub fn block_column(cx: &mut Ctx, c: &BlockColumn, rng: &mut DecoRng, p: [i32; 3]) -> bool {
    let r = &mut *cx.r;
    let n = c.layers.len();
    let mut heights = [0i32; 8];
    let mut total = 0;
    for (i, l) in c.layers.iter().enumerate() {
        heights[i] = l.height.sample(rng);
        total += heights[i];
    }
    if total == 0 {
        return false;
    }
    let mut next = add(p, c.dir);
    for y in 0..total {
        if !c.allowed.test(r, next) {
            let mut remove = total - y;
            let idx: Box<dyn Iterator<Item = usize>> =
                if c.prioritize_tip { Box::new(0..n) } else { Box::new((0..n).rev()) };
            for i in idx {
                if remove <= 0 {
                    break;
                }
                let k = heights[i].min(remove);
                remove -= k;
                heights[i] -= k;
            }
            break;
        }
        next = add(next, c.dir);
    }
    let mut q = p;
    for (i, l) in c.layers.iter().enumerate() {
        for _ in 0..heights[i] {
            let s = l.provider.get(r, rng, q);
            r.set_p(q, s, 2);
            q = add(q, c.dir);
        }
    }
    true
}

// ---------------------------------------------------------------- Ores

fn adjacent_to_air(r: &Region, p: [i32; 3]) -> bool {
    Dir::ALL.iter().any(|d| r.b.is_air(r.get_p(add(p, *d))))
}

fn skip_air_check(rng: &mut DecoRng, discard: f32) -> bool {
    if discard <= 0.0 {
        true
    } else if discard >= 1.0 {
        false
    } else {
        rng.next_float() >= discard
    }
}

pub fn ore(cx: &mut Ctx, o: &Ore, rng: &mut DecoRng, p: [i32; 3]) -> bool {
    let r = &mut *cx.r;
    let b = r.b;
    let size = o.size;
    let dir = rng.next_float() * std::f32::consts::PI;
    let spread = size as f32 / 8.0;
    let max_r = ((size as f32 / 16.0 * 2.0 + 1.0) / 2.0).ceil() as i32;
    let (sd, cd) = crate::jmath::sincos(f64::from(dir));
    let x0 = f64::from(p[0]) + sd * f64::from(spread);
    let x1 = f64::from(p[0]) - sd * f64::from(spread);
    let z0 = f64::from(p[2]) + cd * f64::from(spread);
    let z1 = f64::from(p[2]) - cd * f64::from(spread);
    let y0 = f64::from(p[1] + rng.next_int_bounded(3) - 2);
    let y1 = f64::from(p[1] + rng.next_int_bounded(3) - 2);
    let sc = spread.ceil() as i32;
    let xs = p[0] - sc - max_r;
    let ys = p[1] - 2 - max_r;
    let zs = p[2] - sc - max_r;
    let sxz = 2 * (sc + max_r);
    let sy = 2 * (2 + max_r);
    let mut ok = false;
    'probe: for x in xs..=xs + sxz {
        for z in zs..=zs + sxz {
            if ys <= r.height_at(Hm::OceanFloorWg, x, z) {
                ok = true;
                break 'probe;
            }
        }
    }
    if !ok {
        return false;
    }
    // Spheres along the segment.
    let n = size.max(0) as usize;
    let mut data = std::mem::take(&mut cx.tree.ore_data);
    data.clear();
    data.resize(n * 4, 0.0);
    for i in 0..n {
        let step = i as f32 / size as f32;
        let st = f64::from(step);
        let xx = x0 + st * (x1 - x0);
        let yy = y0 + st * (y1 - y0);
        let zz = z0 + st * (z1 - z0);
        let ss = rng.next_double() * f64::from(size) / 16.0;
        let rad = ((f64::from(cx.d.sin.sin(f64::from(std::f32::consts::PI * step))) + 1.0) * ss + 1.0) / 2.0;
        data[i * 4] = xx;
        data[i * 4 + 1] = yy;
        data[i * 4 + 2] = zz;
        data[i * 4 + 3] = rad;
    }
    for i1 in 0..n.saturating_sub(1) {
        if data[i1 * 4 + 3] <= 0.0 {
            continue;
        }
        for i2 in i1 + 1..n {
            if data[i2 * 4 + 3] <= 0.0 {
                continue;
            }
            let dx = data[i1 * 4] - data[i2 * 4];
            let dy = data[i1 * 4 + 1] - data[i2 * 4 + 1];
            let dz = data[i1 * 4 + 2] - data[i2 * 4 + 2];
            let dr = data[i1 * 4 + 3] - data[i2 * 4 + 3];
            if dr * dr > dx * dx + dy * dy + dz * dz {
                if dr > 0.0 {
                    data[i2 * 4 + 3] = -1.0;
                } else {
                    data[i1 * 4 + 3] = -1.0;
                }
            }
        }
    }
    let mut tested = std::mem::take(&mut cx.tree.ore_tested);
    tested.clear();
    tested.resize(((sxz * sy * sxz) as usize).div_ceil(64) + 1, 0);
    let r = &mut *cx.r;
    let mut placed = 0;
    for i in 0..n {
        let rad = data[i * 4 + 3];
        if rad < 0.0 {
            continue;
        }
        let (xx, yy, zz) = (data[i * 4], data[i * 4 + 1], data[i * 4 + 2]);
        let x_min = ((xx - rad).floor() as i32).max(xs);
        let y_min = ((yy - rad).floor() as i32).max(ys);
        let z_min = ((zz - rad).floor() as i32).max(zs);
        let x_max = ((xx + rad).floor() as i32).max(x_min);
        let y_max = ((yy + rad).floor() as i32).max(y_min);
        let z_max = ((zz + rad).floor() as i32).max(z_min);
        // Chunk of each column resolved once per x (`RUSTCRAFT_OPT_ORE_COL`); 255 = outside the region.
        let col = crate::opt::ore_col() && z_max - z_min < 32;
        let mut cols = [255u8; 32];
        for x in x_min..=x_max {
            let xd = (f64::from(x) + 0.5 - xx) / rad;
            if xd * xd >= 1.0 {
                continue;
            }
            if col {
                for z in z_min..=z_max {
                    cols[(z - z_min) as usize] = r.column_chunk(x, z).map_or(255, |c| c as u8);
                }
            }
            for y in y_min..=y_max {
                let yd = (f64::from(y) + 0.5 - yy) / rad;
                if xd * xd + yd * yd >= 1.0 {
                    continue;
                }
                for z in z_min..=z_max {
                    let zd = (f64::from(z) + 0.5 - zz) / rad;
                    let inside = xd * xd + yd * yd + zd * zd < 1.0;
                    if !inside || r.outside_height(y) {
                        continue;
                    }
                    let bit = (x - xs + (y - ys) * sxz + (z - zs) * sxz * sy) as usize;
                    let w = &mut tested[bit >> 6];
                    if *w >> (bit & 63) & 1 != 0 {
                        continue;
                    }
                    *w |= 1 << (bit & 63);
                    if col {
                        let ci = cols[(z - z_min) as usize];
                        if ci == 255 {
                            continue;
                        }
                        let ci = usize::from(ci);
                        let cur = r.get_in(ci, x, y, z);
                        for t in &o.targets {
                            if t.rule.test(b, cur, y, rng)
                                && (skip_air_check(rng, o.discard) || !adjacent_to_air(r, [x, y, z]))
                            {
                                r.set_direct_in(ci, x, y, z, t.state);
                                placed += 1;
                                break;
                            }
                        }
                        continue;
                    }
                    if !r.in_write_zone(x, z) {
                        continue;
                    }
                    let cur = r.get(x, y, z);
                    for t in &o.targets {
                        if t.rule.test(b, cur, y, rng)
                            && (skip_air_check(rng, o.discard) || !adjacent_to_air(r, [x, y, z]))
                        {
                            r.set_direct(x, y, z, t.state);
                            placed += 1;
                            break;
                        }
                    }
                }
            }
        }
    }
    cx.tree.ore_data = data;
    cx.tree.ore_tested = tested;
    placed > 0
}

pub fn scattered_ore(cx: &mut Ctx, o: &Ore, rng: &mut DecoRng, p: [i32; 3]) -> bool {
    let r = &mut *cx.r;
    let b = r.b;
    let tries = rng.next_int_bounded(o.size + 1);
    for i in 0..tries {
        let m = i.min(7) as f32;
        let mut off = [0i32; 3];
        for v in &mut off {
            let a = rng.next_float();
            let c = rng.next_float();
            *v = round_half_up((a - c) * m);
        }
        let q = [p[0] + off[0], p[1] + off[1], p[2] + off[2]];
        let cur = r.get_p(q);
        for t in &o.targets {
            if t.rule.test(b, cur, q[1], rng) && (skip_air_check(rng, o.discard) || !adjacent_to_air(r, q)) {
                r.set_p(q, t.state, 2);
                break;
            }
        }
    }
    true
}

// ---------------------------------------------------------------- Disks, lakes, springs

pub fn disk(cx: &mut Ctx, c: &Disk, rng: &mut DecoRng, p: [i32; 3]) -> bool {
    let r = &mut *cx.r;
    let mut any = false;
    let top = p[1] + c.half_height;
    let bottom = p[1] - c.half_height - 1;
    let rad = c.radius.sample(rng);
    for z in p[2] - rad..=p[2] + rad {
        for x in p[0] - rad..=p[0] + rad {
            let (xd, zd) = (x - p[0], z - p[2]);
            if xd * xd + zd * zd > rad * rad {
                continue;
            }
            // The top of each run of placed blocks makes what it carries be marked.
            let mut placed_above = false;
            let mut y = top;
            while y > bottom {
                let q = [x, y, z];
                if c.target.test(r, q) {
                    if let Some(s) = c.provider.get_opt(r, rng, q) {
                        r.set_p(q, s, 2);
                        if !placed_above {
                            r.mark_above(q);
                        }
                        any = true;
                        placed_above = true;
                    }
                } else {
                    placed_above = false;
                }
                y -= 1;
            }
        }
    }
    any
}

pub fn lake(cx: &mut Ctx, c: &Lake, rng: &mut DecoRng, p: [i32; 3]) -> bool {
    let r = &mut *cx.r;
    let b = r.b;
    if p[1] <= r.min_y + 4 {
        return false;
    }
    let o = [p[0] - 8, p[1] - 4, p[2] - 8];
    let mut grid = [false; 2048];
    let spots = rng.next_int_bounded(4) + 4;
    for _ in 0..spots {
        let xr = rng.next_double() * 6.0 + 3.0;
        let yr = rng.next_double() * 4.0 + 2.0;
        let zr = rng.next_double() * 6.0 + 3.0;
        let xp = rng.next_double() * (16.0 - xr - 2.0) + 1.0 + xr / 2.0;
        let yp = rng.next_double() * (8.0 - yr - 4.0) + 2.0 + yr / 2.0;
        let zp = rng.next_double() * (16.0 - zr - 2.0) + 1.0 + zr / 2.0;
        for xx in 1..15 {
            for zz in 1..15 {
                for yy in 1..7 {
                    let xd = (f64::from(xx) - xp) / (xr / 2.0);
                    let yd = (f64::from(yy) - yp) / (yr / 2.0);
                    let zd = (f64::from(zz) - zp) / (zr / 2.0);
                    if xd * xd + yd * yd + zd * zd < 1.0 {
                        grid[((xx * 16 + zz) * 8 + yy) as usize] = true;
                    }
                }
            }
        }
    }
    let g = |x: i32, z: i32, y: i32| grid[((x * 16 + z) * 8 + y) as usize];
    let edge = |x: i32, z: i32, y: i32| {
        !g(x, z, y)
            && (x < 15 && g(x + 1, z, y)
                || x > 0 && g(x - 1, z, y)
                || z < 15 && g(x, z + 1, y)
                || z > 0 && g(x, z - 1, y)
                || y < 7 && g(x, z, y + 1)
                || y > 0 && g(x, z, y - 1))
    };
    let fluid = c.fluid.get(r, rng, o);
    for xx in 0..16 {
        for zz in 0..16 {
            for yy in 0..8 {
                if edge(xx, zz, yy) {
                    let q = [o[0] + xx, o[1] + yy, o[2] + zz];
                    let s = r.get_p(q);
                    if yy >= 4 && b.has_flag(s, rc_data::flags::LIQUID) {
                        return false;
                    }
                    if yy < 4 && !b.legacy_solid(s) && s != fluid {
                        return false;
                    }
                    if !c.can_place.test(r, q) {
                        return false;
                    }
                }
            }
        }
    }
    let cave_air = b.default_state(b.k.cave_air);
    for xx in 0..16 {
        for zz in 0..16 {
            for yy in 0..8 {
                if g(xx, zz, yy) {
                    let q = [o[0] + xx, o[1] + yy, o[2] + zz];
                    if c.can_replace_air_fluid.test(r, q) {
                        let air = yy >= 4;
                        r.set_p(q, if air { cave_air } else { fluid }, 2);
                        if air {
                            // Block tick of the air (no effect in game, but saved) then marking.
                            r.schedule_block_tick(q, b.k.cave_air, 0);
                            r.mark_above(q);
                        }
                    }
                }
            }
        }
    }
    let barrier = c.barrier.get(r, rng, o);
    if !b.is_air(barrier) {
        for xx in 0..16 {
            for zz in 0..16 {
                for yy in 0..8 {
                    if edge(xx, zz, yy) && (yy < 4 || rng.next_int_bounded(2) != 0) {
                        let q = [o[0] + xx, o[1] + yy, o[2] + zz];
                        if b.legacy_solid(r.get_p(q)) && c.can_replace_barrier.test(r, q) {
                            r.set_p(q, barrier, 2);
                            r.mark_above(q);
                        }
                    }
                }
            }
        }
    }
    if b.fluid(fluid) == 1 {
        let ice = b.default_state(b.k.ice);
        for xx in 0..16 {
            for zz in 0..16 {
                let q = [o[0] + xx, o[1] + 4, o[2] + zz];
                if should_freeze(cx.d, r, r.biome(q[0], q[1], q[2]), q) && c.can_replace_air_fluid.test(r, q) {
                    r.set_p(q, ice, 2);
                }
            }
        }
    }
    true
}

/// Not warm enough and water (liquid block).
pub fn should_freeze(d: &crate::decor::Decorator, r: &Region, biome: u16, q: [i32; 3]) -> bool {
    if d.warm_enough_to_rain(biome, q, r.sea_level) {
        return false;
    }
    if r.outside_height(q[1]) {
        return false;
    }
    let s = r.get_p(q);
    r.b.is(s, r.b.k.water)
}

pub fn spring(cx: &mut Ctx, c: &Spring, p: [i32; 3]) -> bool {
    let r = &mut *cx.r;
    let b = r.b;
    let valid = |s: State| c.valid.contains(b.block(s));
    let up = r.get(p[0], p[1] + 1, p[2]);
    if !valid(up) {
        return false;
    }
    let down = r.get(p[0], p[1] - 1, p[2]);
    if c.requires_below && !valid(down) {
        return false;
    }
    let cur = r.get_p(p);
    if !b.is_air(cur) && !valid(cur) {
        return false;
    }
    let mut rocks = 0;
    let mut holes = 0;
    for d in [Dir::West, Dir::East, Dir::North, Dir::South, Dir::Down] {
        let s = r.get_p(add(p, d));
        if valid(s) {
            rocks += 1;
        }
    }
    for d in [Dir::West, Dir::East, Dir::North, Dir::South, Dir::Down] {
        if b.is_air(r.get_p(add(p, d))) {
            holes += 1;
        }
    }
    if rocks == c.rock_count && holes == c.hole_count {
        r.set_p(p, c.state, 2);
        // Tick of the spring's fluid (not read back: placed even if the placement was refused).
        r.schedule_fluid_tick(p, c.state);
        return true;
    }
    false
}

/// Face of a block "visible from the outside" (occlusion of the non-full face).
pub fn visible_from_outside(b: &Blocks, s: State, d: Dir) -> bool {
    !(b.has_flag(s, rc_data::flags::CAN_OCCLUDE) && b.facts(s).occl_faces >> d as u8 & 1 != 0)
}

/// Vertical sweep: (ceiling, floor) if p is in the column.
pub fn column_scan(
    r: &Region,
    p: [i32; 3],
    range: i32,
    inside: impl Fn(State) -> bool,
    edge: impl Fn(State) -> bool,
) -> Option<(Option<i32>, Option<i32>)> {
    if !inside(r.get_p(p)) {
        return None;
    }
    let scan = |dy: i32| {
        let mut y = p[1];
        let mut i = 1;
        while i < range && inside(r.get(p[0], y, p[2])) {
            y += dy;
            i += 1;
        }
        if edge(r.get(p[0], y, p[2])) { Some(y) } else { None }
    };
    let ceiling = scan(1);
    let floor = scan(-1);
    Some((ceiling, floor))
}

pub fn underwater_magma(cx: &mut Ctx, range: i32, radius: i32, prob: f32, rng: &mut DecoRng, p: [i32; 3]) -> bool {
    let r = &mut *cx.r;
    let b = r.b;
    let water = b.k.water;
    let Some((_, Some(floor))) = column_scan(r, p, range, |s| b.is(s, water), |s| !b.is(s, water)) else {
        return false;
    };
    let f = [p[0], floor, p[2]];
    let magma = b.default_state(b.k.magma_block);
    let mut n = 0;
    // Walk of the box: x fastest, then y, then z.
    for z in f[2] - radius..=f[2] + radius {
        for y in f[1] - radius..=f[1] + radius {
            for x in f[0] - radius..=f[0] + radius {
                if rng.next_float() >= prob {
                    continue;
                }
                let q = [x, y, z];
                let s = r.get_p(q);
                if b.is(s, water) || b.is_air(s) {
                    continue;
                }
                if visible_from_outside(b, r.get(x, y - 1, z), Dir::Up) {
                    continue;
                }
                let mut ok = true;
                for d in Dir::NESW {
                    if visible_from_outside(b, r.get_p(add(q, d)), d.opposite()) {
                        ok = false;
                        break;
                    }
                }
                if ok {
                    r.set_p(q, magma, 2);
                    n += 1;
                }
            }
        }
    }
    n > 0
}

// ---------------------------------------------------------------- Monster room

/// Reorientation of a chest against the walls (§ 6.4.1).
pub fn reorient_chest(r: &Region, p: [i32; 3], chest: State) -> State {
    let b = r.b;
    let mut solid: Option<Dir> = None;
    for d in Dir::NESW {
        let s = r.get_p(add(p, d));
        if b.is(s, b.k.chest) {
            return chest;
        }
        if b.solid_render(s) {
            if solid.is_some() {
                solid = None;
                break;
            }
            solid = Some(d);
        }
    }
    if let Some(d) = solid {
        return b.with_dir(chest, b.p.facing, d.opposite());
    }
    let mut f = b.get_dir(chest, b.p.facing).unwrap_or(Dir::North);
    if b.solid_render(r.get_p(add(p, f))) {
        f = f.opposite();
    }
    if b.solid_render(r.get_p(add(p, f))) {
        f = f.cw();
    }
    if b.solid_render(r.get_p(add(p, f))) {
        f = f.opposite();
    }
    b.with_dir(chest, b.p.facing, f)
}

pub fn monster_room(cx: &mut Ctx, rng: &mut DecoRng, p: [i32; 3]) -> bool {
    let r = &mut *cx.r;
    let b = r.b;
    let cannot = if crate::opt::decor_ids() {
        b.rules.features_cannot_replace
    } else {
        b.tag("features_cannot_replace").unwrap_or(u16::MAX)
    };
    let can_replace = |s: State| cannot == u16::MAX || !b.in_tag(s, cannot);
    let xr = rng.next_int_bounded(2) + 2;
    let (min_x, max_x) = (-xr - 1, xr + 1);
    let zr = rng.next_int_bounded(2) + 2;
    let (min_z, max_z) = (-zr - 1, zr + 1);
    let mut holes = 0;
    for dx in min_x..=max_x {
        for dy in -1..=4 {
            for dz in min_z..=max_z {
                let q = [p[0] + dx, p[1] + dy, p[2] + dz];
                let solid = b.legacy_solid(r.get_p(q));
                if (dy == -1 || dy == 4) && !solid {
                    return false;
                }
                if (dx == min_x || dx == max_x || dz == min_z || dz == max_z)
                    && dy == 0
                    && b.is_air(r.get_p(q))
                    && b.is_air(r.get(q[0], q[1] + 1, q[2]))
                {
                    holes += 1;
                }
            }
        }
    }
    if !(1..=5).contains(&holes) {
        return false;
    }
    let cave_air = b.default_state(b.k.cave_air);
    let mossy = b.default_state(b.k.mossy_cobblestone);
    let cobble = b.default_state(b.k.cobblestone);
    for dx in min_x..=max_x {
        for dy in (-1..=3).rev() {
            for dz in min_z..=max_z {
                let q = [p[0] + dx, p[1] + dy, p[2] + dz];
                let s = r.get_p(q);
                if dx == min_x || dy == -1 || dz == min_z || dx == max_x || dy == 4 || dz == max_z {
                    if q[1] >= r.min_y && !b.legacy_solid(r.get(q[0], q[1] - 1, q[2])) {
                        r.set_p(q, cave_air, 2);
                    } else if b.legacy_solid(s) && !b.is(s, b.k.chest) {
                        let place = if dy == -1 && rng.next_int_bounded(4) != 0 { mossy } else { cobble };
                        if can_replace(r.get_p(q)) {
                            r.set_p(q, place, 2);
                        }
                    }
                } else if !b.is(s, b.k.chest) && !b.is(s, b.k.spawner) && can_replace(s) {
                    r.set_p(q, cave_air, 2);
                }
            }
        }
    }
    let chest = b.default_state(b.k.chest);
    for _ in 0..2 {
        for _ in 0..3 {
            let xc = p[0] + rng.next_int_bounded(xr * 2 + 1) - xr;
            let zc = p[2] + rng.next_int_bounded(zr * 2 + 1) - zr;
            let q = [xc, p[1], zc];
            if b.is_air(r.get_p(q)) {
                let walls = Dir::NESW.iter().filter(|d| b.legacy_solid(r.get_p(add(q, **d)))).count();
                if walls == 1 {
                    let st = reorient_chest(r, q, chest);
                    if can_replace(r.get_p(q)) {
                        r.set_p(q, st, 2);
                    }
                    if r.has_block_entity(q[0], q[1], q[2]) {
                        let seed = rng.next_long();
                        r.note_block_entity(q, BeData::Loot { table: "minecraft:chests/simple_dungeon", seed });
                    }
                    break;
                }
            }
        }
    }
    let spawner = b.default_state(b.k.spawner);
    if can_replace(r.get_p(p)) {
        r.set_p(p, spawner, 2);
    }
    if r.has_block_entity(p[0], p[1], p[2]) && b.is(r.get_p(p), b.k.spawner) {
        const MOBS: [&str; 4] = ["minecraft:skeleton", "minecraft:zombie", "minecraft:zombie", "minecraft:spider"];
        let entity = MOBS[rng.next_int_bounded(4) as usize];
        r.note_block_entity(p, BeData::Spawner { entity });
    }
    true
}

// ---------------------------------------------------------------- Surface freezing

pub fn freeze_top_layer(cx: &mut Ctx, p: [i32; 3]) -> bool {
    let d = cx.d;
    let r = &mut *cx.r;
    let b = r.b;
    let ice = b.default_state(b.k.ice);
    let snow = b.default_state(b.k.snow);
    for dx in 0..16 {
        for dz in 0..16 {
            let x = p[0] + dx;
            let z = p[2] + dz;
            let y = r.height_at(Hm::MotionBlocking, x, z);
            let top = [x, y, z];
            let below = [x, y - 1, z];
            let biome = r.biome(x, y, z);
            // The two heights of a column at once (`RUSTCRAFT_OPT_FREEZE_COL`).
            let warm = if crate::opt::freeze_col() {
                Some(d.warm_column(biome, x, z, [y - 1, y], r.sea_level))
            } else {
                None
            };
            let freeze = match warm {
                Some(w) => !w[0] && !r.outside_height(y - 1) && b.is(r.get_p(below), b.k.water),
                None => should_freeze(d, r, biome, below),
            };
            if freeze {
                r.set_p(below, ice, 2);
            }
            // Snow: precipitation, cold, air or snow, the snow holds.
            let c = d.climate[biome as usize];
            let warm_top = match warm {
                Some(w) => w[1],
                None => c.has_precipitation && d.warm_enough_to_rain(biome, top, r.sea_level),
            };
            if c.has_precipitation && !warm_top && !r.outside_height(y) {
                let s = r.get_p(top);
                if (b.is_air(s) || b.is(s, b.k.snow)) && can_survive(r, snow, top) {
                    r.set_p(top, snow, 2);
                    let bs = r.get_p(below);
                    if b.has(bs, b.p.snowy) {
                        r.set_p(below, b.with_bool(bs, b.p.snowy, true), 2);
                    }
                }
            }
        }
    }
    true
}

/// Fills the layer `height` of the chunk of `o` (above the world bottom) wherever there is
/// air (non-blocking layer of a flat world, placed as the last step).
pub fn fill_layer(cx: &mut Ctx, height: i32, state: State, o: [i32; 3]) -> bool {
    let r = &mut *cx.r;
    let y = r.min_y + height;
    for dx in 0..16 {
        for dz in 0..16 {
            let p = [o[0] + dx, y, o[2] + dz];
            if r.b.is_air(r.get_p(p)) {
                r.set_p(p, state, 2);
            }
        }
    }
    true
}

/// First replacement whose rule accepts the block of `o`.
pub fn replace_single_block(
    cx: &mut Ctx,
    targets: &[crate::feature::OreTarget],
    rng: &mut DecoRng,
    o: [i32; 3],
) -> bool {
    let r = &mut *cx.r;
    let cur = r.get_p(o);
    for t in targets {
        if t.rule.test(r.b, cur, o[1], rng) {
            r.set_p(o, t.state, 2);
            break;
        }
    }
    true
}

/// Starting platform of the void: chunks at Chebyshev distance 1 at most from chunk (0, 0),
/// cells within 16 of (8, 8), height origin + 3, cobblestone at the centre, stone elsewhere.
pub fn void_start_platform(cx: &mut Ctx, o: [i32; 3]) -> bool {
    let (ccx, ccz) = (o[0] >> 4, o[2] >> 4);
    if ccx.abs().max(ccz.abs()) > 1 {
        return true;
    }
    let r = &mut *cx.r;
    let b = r.b;
    let stone = b.default_state(b.block_id("stone").expect("stone"));
    let cobble = b.default_state(b.block_id("cobblestone").expect("cobblestone"));
    let y = o[1] + 3;
    for z in ccz * 16..ccz * 16 + 16 {
        for x in ccx * 16..ccx * 16 + 16 {
            if (x - 8).abs().max((z - 8).abs()) <= 16 {
                r.set_p([x, y, z], if x == 8 && z == 8 { cobble } else { stone }, 2);
            }
        }
    }
    true
}
