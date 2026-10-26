//! Dripstone: pointed dripstone, clusters, large dripstone (`generation-decoration.md` § 6.8).

use crate::decor::Ctx;
use crate::decor_blocks::{BlockId, Blocks, Dir, State};
use crate::decor_pred::{Pred, clamped_map_d, clamped_map_f, jclamp_f};
use crate::decor_region::{Hm, Region, add};
use crate::decor_rng::DecoRng;
use crate::feature::{LargeDripstone, Speleothem, SpeleothemCluster};
use crate::feature_basic::column_scan;

fn in_set(pred: &Pred, b: &Blocks, s: State) -> bool {
    match pred {
        Pred::Blocks(_, set) => set.contains(b.block(s)),
        _ => false,
    }
}

fn empty_or_water(b: &Blocks, s: State) -> bool {
    b.is_air(s) || b.is(s, b.k.water)
}

fn empty_water_lava(b: &Blocks, s: State) -> bool {
    b.is_air(s) || b.is(s, b.k.water) || b.is(s, b.k.lava)
}

fn is_base(b: &Blocks, s: State, base: BlockId, repl: &Pred) -> bool {
    b.is(s, base) || in_set(repl, b, s)
}

fn place_base(r: &mut Region, p: [i32; 3], base: State, repl: &Pred) -> bool {
    let b = r.b;
    if in_set(repl, b, r.get_p(p)) {
        r.set_p(p, base, 2);
        true
    } else {
        false
    }
}

fn pointed(b: &Blocks, block: BlockId, d: Dir, thickness: &str) -> State {
    let s = b.with_dir(b.default_state(block), b.p.vertical_direction, d);
    // Property "vertical_direction" (pointed dripstone); the property "tip_direction" serves the others.
    let s = b.with_dir(s, b.p.tip_direction, d);
    b.with_str(s, b.p.thickness, thickness)
}

#[allow(clippy::too_many_arguments)]
fn grow(
    r: &mut Region,
    start: [i32; 3],
    d: Dir,
    height: i32,
    merged: bool,
    base: BlockId,
    pointed_block: BlockId,
    repl: &Pred,
) {
    let b = r.b;
    if !is_base(b, r.get_p(add(start, d.opposite())), base, repl) {
        return;
    }
    let mut p = start;
    let mut put = |r: &mut Region, thick: &str| {
        let mut s = pointed(b, pointed_block, d, thick);
        s = b.with_bool(s, b.p.waterlogged, b.fluid(r.get_p(p)) == 1);
        r.set_p(p, s, 2);
        p = add(p, d);
    };
    if height >= 3 {
        put(r, "base");
        for _ in 0..height - 3 {
            put(r, "middle");
        }
    }
    if height >= 2 {
        put(r, "frustum");
    }
    if height >= 1 {
        put(r, if merged { "tip_merge" } else { "tip" });
    }
}

pub fn speleothem(cx: &mut Ctx, s: &Speleothem, rng: &mut DecoRng, p: [i32; 3]) -> bool {
    let r = &mut *cx.r;
    let b = r.b;
    let base = b.block(s.base);
    let above = is_base(b, r.get_p(add(p, Dir::Up)), base, &s.replaceable);
    let below = is_base(b, r.get_p(add(p, Dir::Down)), base, &s.replaceable);
    let tip = if above && below {
        if rng.next_bool() { Dir::Down } else { Dir::Up }
    } else if above {
        Dir::Down
    } else if below {
        Dir::Up
    } else {
        return false;
    };
    let root = add(p, tip.opposite());
    place_base(r, root, s.base, &s.replaceable);
    for d in Dir::NESW {
        if rng.next_float() <= s.directional {
            let p1 = add(root, d);
            place_base(r, p1, s.base, &s.replaceable);
            if rng.next_float() <= s.r2 {
                let p2 = add(p1, Dir::ALL[rng.next_int_bounded(6) as usize]);
                place_base(r, p2, s.base, &s.replaceable);
                if rng.next_float() <= s.r3 {
                    let p3 = add(p2, Dir::ALL[rng.next_int_bounded(6) as usize]);
                    place_base(r, p3, s.base, &s.replaceable);
                }
            }
        }
    }
    let height = if rng.next_float() < s.taller && empty_or_water(b, r.get_p(add(p, tip))) { 2 } else { 1 };
    grow(r, p, tip, height, false, base, s.pointed, &s.replaceable);
    true
}

pub fn speleothem_cluster(cx: &mut Ctx, s: &SpeleothemCluster, rng: &mut DecoRng, o: [i32; 3]) -> bool {
    let b = cx.r.b;
    if !empty_or_water(b, cx.r.get_p(o)) {
        return false;
    }
    let height = s.height.sample(rng);
    let wetness = s.wetness.sample(rng);
    let density = s.density.sample(rng);
    let xr = s.radius.sample(rng);
    let zr = s.radius.sample(rng);
    for dx in -xr..=xr {
        for dz in -zr..=zr {
            let edge = (xr - dx.abs()).min(zr - dz.abs());
            let chance = f64::from(clamped_map_f(edge as f32, 0.0, s.max_edge as f32, s.chance_at_max, 1.0));
            cluster_column(cx, s, rng, [o[0] + dx, o[1], o[2] + dz], dx, dz, wetness, chance, height, density);
        }
    }
    true
}

#[allow(clippy::too_many_arguments)]
fn cluster_column(
    cx: &mut Ctx,
    s: &SpeleothemCluster,
    rng: &mut DecoRng,
    p: [i32; 3],
    dx: i32,
    dz: i32,
    wet: f32,
    chance: f64,
    cluster_h: i32,
    density: f32,
) {
    let r = &mut *cx.r;
    let b = r.b;
    let base = b.block(s.base);
    let Some((ceiling, base_floor)) =
        column_scan(r, p, s.search_range, |st| empty_or_water(b, st), |st| !b.is_air(st) && !b.is(st, b.k.water))
    else {
        return;
    };
    if ceiling.is_none() && base_floor.is_none() {
        return;
    }
    let want_pool = rng.next_float() < wet;
    let mut floor = base_floor;
    if want_pool
        && let Some(f) = base_floor
        && can_place_pool(r, s, [p[0], f, p[2]])
    {
        floor = Some(f - 1);
        r.set(p[0], f, p[2], b.default_state(b.k.water), 2);
    }
    let want_stalactite = rng.next_double() < chance;
    let lava = |r: &Region, y: i32| b.is(r.get(p[0], y, p[2]), b.k.lava);
    let stalactite_h = match ceiling {
        Some(c) if want_stalactite && !lava(r, c) => {
            let t = s.layer_thickness.sample(rng);
            replace_with_base(r, [p[0], c, p[2]], t, Dir::Up, s);
            let max_h = match floor {
                Some(f) => cluster_h.min(c - f),
                None => cluster_h,
            };
            spel_height(s, rng, dx, dz, density, max_h)
        }
        _ => 0,
    };
    let want_stalagmite = rng.next_double() < chance;
    let stalagmite_h = match floor {
        Some(f) if want_stalagmite && !lava(r, f) => {
            let t = s.layer_thickness.sample(rng);
            replace_with_base(r, [p[0], f, p[2]], t, Dir::Down, s);
            if ceiling.is_some() {
                0.max(stalactite_h + rng.between(-s.max_diff, s.max_diff))
            } else {
                spel_height(s, rng, dx, dz, density, cluster_h)
            }
        }
        _ => 0,
    };
    let (mut act_c, mut act_f) = (stalactite_h, stalagmite_h);
    if let (Some(c), Some(f)) = (ceiling, floor)
        && c - stalactite_h <= f + stalagmite_h
    {
        let lowest = (c - stalactite_h).max(f + 1);
        let highest = (f + stalagmite_h).min(c - 1);
        let bottom = rng.between(lowest, highest + 1);
        act_c = c - bottom;
        act_f = bottom - 1 - f;
    }
    let col_h = match (ceiling, floor) {
        (Some(c), Some(f)) => Some(c - f - 1),
        _ => None,
    };
    let merge = rng.next_bool() && act_c > 0 && act_f > 0 && col_h.is_some_and(|h| act_c + act_f == h);
    if let Some(c) = ceiling {
        grow(r, [p[0], c - 1, p[2]], Dir::Down, act_c, merge, base, s.pointed, &s.replaceable);
    }
    if let Some(f) = floor {
        grow(r, [p[0], f + 1, p[2]], Dir::Up, act_f, merge, base, s.pointed, &s.replaceable);
    }
}

fn spel_height(s: &SpeleothemCluster, rng: &mut DecoRng, dx: i32, dz: i32, density: f32, max_h: i32) -> i32 {
    if rng.next_float() > density {
        return 0;
    }
    let d = dx.abs() + dz.abs();
    let mean = clamped_map_d(f64::from(d), 0.0, f64::from(s.max_center), f64::from(max_h) / 2.0, 0.0) as f32;
    let v = mean + rng.next_gaussian() as f32 * s.height_deviation as f32;
    jclamp_f(v, 0.0, max_h as f32) as i32
}

fn can_place_pool(r: &Region, s: &SpeleothemCluster, p: [i32; 3]) -> bool {
    let b = r.b;
    let st = r.get_p(p);
    if b.is(st, b.k.water) || b.is(st, b.block(s.base)) || b.is(st, s.pointed) {
        return false;
    }
    if b.fluid(r.get_p(add(p, Dir::Up))) == 1 {
        return false;
    }
    let ok = |q: [i32; 3]| {
        let n = r.get_p(q);
        b.in_tag(n, b.rules.base_stone_overworld) || b.fluid(n) == 1
    };
    Dir::NESW.iter().all(|d| ok(add(p, *d))) && ok(add(p, Dir::Down))
}

fn replace_with_base(r: &mut Region, first: [i32; 3], n: i32, d: Dir, s: &SpeleothemCluster) {
    let mut p = first;
    for _ in 0..n {
        if !place_base(r, p, s.base, &s.replaceable) {
            return;
        }
        p = add(p, d);
    }
}

// ---------------------------------------------------------------- Large dripstone

fn spel_shape(d: f64, radius: f64, scale: f64, blunt: f64) -> f64 {
    let d = if d < blunt { blunt } else { d };
    let r = d / radius * 0.384;
    let p1 = 0.75 * r.powf(1.333_333_333_333_333_3);
    let p2 = r.powf(0.666_666_666_666_666_6);
    let p3 = 0.333_333_333_333_333_3 * r.ln();
    let h = (scale * (p1 - p2 - p3)).max(0.0);
    h / 0.384 * radius
}

struct Drip {
    root: [i32; 3],
    up: bool,
    radius: i32,
    blunt: f64,
    scale: f64,
}

struct Wind {
    origin_y: i32,
    speed: Option<(f64, f64)>,
    max: i32,
}

impl Wind {
    fn offset(&self, p: [i32; 3]) -> [i32; 3] {
        let Some((vx, vz)) = self.speed else { return p };
        let dy = f64::from(self.origin_y - p[1]);
        let dx = ((vx * dy).floor() as i32).clamp(-self.max, self.max);
        let dz = ((vz * dy).floor() as i32).clamp(-self.max, self.max);
        [p[0] + dx, p[1], p[2] + dz]
    }
}

impl Drip {
    fn height_at(&self, r: f32) -> i32 {
        spel_shape(f64::from(r), f64::from(self.radius), self.scale, self.blunt) as i32
    }

    fn anchor(&mut self, cx: &Ctx, wind: &Wind) -> bool {
        let r = &*cx.r;
        let b = r.b;
        while self.radius > 1 {
            let mut root = self.root;
            let tries = 10.min(self.height_at(0.0));
            for _ in 0..tries {
                if b.is(r.get_p(root), b.k.lava) {
                    return false;
                }
                if circle_embedded(cx, wind.offset(root), self.radius) {
                    self.root = root;
                    return true;
                }
                root = add(root, if self.up { Dir::Down } else { Dir::Up });
            }
            self.radius /= 2;
        }
        false
    }

    fn place(&self, cx: &mut Ctx, rng: &mut DecoRng, wind: &Wind) {
        let b = cx.r.b;
        let dripstone = b.default_state(b.k.dripstone_block);
        for dx in -self.radius..=self.radius {
            for dz in -self.radius..=self.radius {
                let cr = ((dx * dx + dz * dz) as f32).sqrt();
                if cr > self.radius as f32 {
                    continue;
                }
                let mut h = self.height_at(cr);
                if h <= 0 {
                    continue;
                }
                if f64::from(rng.next_float()) < 0.2 {
                    let f = rng.next_float() * (1.0 - 0.8) + 0.8;
                    h = (h as f32 * f) as i32;
                }
                let mut p = [self.root[0] + dx, self.root[1], self.root[2] + dz];
                let mut out = false;
                let max_y = if self.up { cx.r.height_at(Hm::WorldSurfaceWg, p[0], p[2]) } else { i32::MAX };
                let mut i = 0;
                while i < h && p[1] < max_y {
                    let w = wind.offset(p);
                    let s = cx.r.get_p(w);
                    if empty_water_lava(b, s) {
                        out = true;
                        cx.r.set_p(w, dripstone, 2);
                    } else if out && b.in_tag(s, b.rules.base_stone_overworld) {
                        break;
                    }
                    p = add(p, if self.up { Dir::Up } else { Dir::Down });
                    i += 1;
                }
            }
        }
    }
}

fn circle_embedded(cx: &Ctx, c: [i32; 3], radius: i32) -> bool {
    let r = &*cx.r;
    let b = r.b;
    if empty_water_lava(b, r.get_p(c)) {
        return false;
    }
    let inc = 6.0f32 / radius as f32;
    let mut a = 0.0f32;
    let two_pi = (std::f64::consts::PI * 2.0) as f32;
    while a < two_pi {
        let dx = (cx.d.sin.cos(f64::from(a)) * radius as f32) as i32;
        let dz = (cx.d.sin.sin(f64::from(a)) * radius as f32) as i32;
        if empty_water_lava(b, r.get(c[0] + dx, c[1], c[2] + dz)) {
            return false;
        }
        a += inc;
    }
    true
}

pub fn large_dripstone(cx: &mut Ctx, s: &LargeDripstone, rng: &mut DecoRng, o: [i32; 3]) -> bool {
    let b = cx.r.b;
    if !empty_or_water(b, cx.r.get_p(o)) {
        return false;
    }
    let drip = b.k.dripstone_block;
    let Some((Some(ceiling), Some(floor))) = column_scan(
        cx.r,
        o,
        s.search_range,
        |st| empty_or_water(b, st),
        |st| b.is(st, drip) || in_set(&s.replaceable, b, st) || b.is(st, b.k.lava),
    ) else {
        return false;
    };
    let height = ceiling - floor - 1;
    if height < 4 {
        return false;
    }
    let by_height = (height as f32 * s.ratio) as i32;
    let max_r = by_height.clamp(s.column_radius.min_value(), s.column_radius.max_value());
    let radius = rng.between(s.column_radius.min_value(), max_r);
    let blunt = f64::from(s.stalactite_bluntness.sample(rng));
    let scale = f64::from(s.height_scale.sample(rng));
    let mut top = Drip { root: [o[0], ceiling - 1, o[2]], up: false, radius, blunt, scale };
    let blunt = f64::from(s.stalagmite_bluntness.sample(rng));
    let scale = f64::from(s.height_scale.sample(rng));
    let mut bottom = Drip { root: [o[0], floor + 1, o[2]], up: true, radius, blunt, scale };
    let suitable = |d: &Drip| d.radius >= s.min_radius_for_wind && d.blunt >= f64::from(s.min_bluntness_for_wind);
    let wind = if suitable(&top) && suitable(&bottom) {
        let speed = s.wind_speed.sample(rng);
        let dir = rng.next_float() * std::f32::consts::PI;
        let vx = f64::from(cx.d.sin.cos(f64::from(dir)) * speed);
        let vz = f64::from(cx.d.sin.sin(f64::from(dir)) * speed);
        Wind { origin_y: o[1], speed: Some((vx, vz)), max: 16 - radius }
    } else {
        Wind { origin_y: 0, speed: None, max: 0 }
    };
    let top_ok = top.anchor(cx, &wind);
    let bottom_ok = bottom.anchor(cx, &wind);
    if top_ok {
        top.place(cx, rng, &wind);
    }
    if bottom_ok {
        bottom.place(cx, rng, &wind);
    }
    true
}
