//! Ice: blue ice, icebergs, ice spikes (`generation-decoration.md` § 6.6).

use crate::decor::Ctx;
use crate::decor_blocks::Dir;
use crate::decor_blocks::State;
use crate::decor_pred::Pred;
use crate::decor_region::{Region, add};
use crate::decor_rng::DecoRng;

pub fn blue_ice(cx: &mut Ctx, rng: &mut DecoRng, o: [i32; 3]) -> bool {
    let r = &mut *cx.r;
    let b = r.b;
    if o[1] > r.sea_level - 1 {
        return false;
    }
    if !b.is(r.get_p(o), b.k.water) && !b.is(r.get_p(add(o, Dir::Down)), b.k.water) {
        return false;
    }
    if !Dir::ALL.iter().any(|d| *d != Dir::Down && b.is(r.get_p(add(o, *d)), b.k.packed_ice)) {
        return false;
    }
    let blue = b.default_state(b.k.blue_ice);
    r.set_p(o, blue, 2);
    for _ in 0..200 {
        let a = rng.next_int_bounded(5);
        let y_off = a - rng.next_int_bounded(6);
        let mut xz = 3;
        if y_off < 2 {
            xz += y_off / 2;
        }
        if xz >= 1 {
            let dx = rng.next_int_bounded(xz) - rng.next_int_bounded(xz);
            let dz = rng.next_int_bounded(xz) - rng.next_int_bounded(xz);
            let p = [o[0] + dx, o[1] + y_off, o[2] + dz];
            let s = r.get_p(p);
            if (b.is_air(s) || b.is(s, b.k.water) || b.is(s, b.k.packed_ice) || b.is(s, b.k.ice))
                && Dir::ALL.iter().any(|d| b.is(r.get_p(add(p, *d)), b.k.blue_ice))
            {
                r.set_p(p, blue, 2);
            }
        }
    }
    true
}

struct Berg {
    ellipse: bool,
    snow_on_top: bool,
    angle: f64,
    /// sin and cos of the angle computed once (RUSTCRAFT_OPT_BERG_TRIG), otherwise at each cell.
    trig: Option<(f64, f64)>,
    ea: i32,
    ec: i32,
    main: State,
}

fn iceberg_state(b: &crate::decor_blocks::Blocks, s: State) -> bool {
    b.is(s, b.k.packed_ice) || b.is(s, b.k.snow_block) || b.is(s, b.k.blue_ice)
}

fn trig(angle: f64) -> (f64, f64) {
    crate::jmath::sincos(angle)
}

fn ellipse_dist(xo: i32, zo: i32, ox: i32, oz: i32, a: i32, c: i32, (sa, ca): (f64, f64)) -> f64 {
    let (dx, dz) = (f64::from(xo - ox), f64::from(zo - oz));
    let u = (dx * ca - dz * sa) / f64::from(a);
    let v = (dx * sa + dz * ca) / f64::from(c);
    u * u + v * v - 1.0
}

fn circle_dist(rng: &mut DecoRng, xo: i32, zo: i32, radius: i32) -> f64 {
    let off = 10.0f32 * crate::decor_pred::jclamp_f(rng.next_float(), 0.2, 0.8) / radius as f32;
    let (x, z, r) = (f64::from(xo), f64::from(zo), f64::from(radius));
    f64::from(off) + x * x + z * z - r * r
}

fn radius_round(rng: &mut DecoRng, y: i32, h: i32, w: i32) -> i32 {
    let k = 3.5f32 - rng.next_float();
    let mut scale = (1.0f32 - f64::from(y).powf(2.0) as f32 / (h as f32 * k)) * w as f32;
    if h > 15 + rng.next_int_bounded(5) {
        let ty = if y < 3 + rng.next_int_bounded(6) { y / 2 } else { y };
        scale = (1.0f32 - ty as f32 / (h as f32 * k * 0.4)) * w as f32;
    }
    (scale / 2.0).ceil() as i32
}

fn radius_ellipse(y: i32, h: i32, w: i32) -> i32 {
    let scale = (1.0f32 - f64::from(y).powf(2.0) as f32 / (h as f32 * 1.0)) * w as f32;
    (scale / 2.0).ceil() as i32
}

fn radius_steep(rng: &mut DecoRng, y: i32, h: i32, w: i32) -> i32 {
    let k = 1.0f32 + rng.next_float() / 2.0;
    let scale = (1.0f32 - y as f32 / (h as f32 * k)) * w as f32;
    (scale / 2.0).ceil() as i32
}

#[allow(clippy::too_many_arguments)]
fn berg_block(
    r: &mut Region,
    g: &Berg,
    rng: &mut DecoRng,
    o: [i32; 3],
    h: i32,
    xo: i32,
    y: i32,
    zo: i32,
    radius: i32,
    a: i32,
) {
    let c = if g.ellipse {
        let mut c = g.ec;
        if y > 0 && h - y <= 3 {
            c -= 4 - (h - y);
        }
        c
    } else {
        0
    };
    let d = if g.ellipse { ellipse_dist(xo, zo, 0, 0, a, c, g.trig.unwrap_or_else(|| trig(g.angle))) } else { circle_dist(rng, xo, zo, radius) };
    // NaN discarded like `d >= 0`: at the top of an elliptical iceberg, the minor axis can drop to 0
    // and the central column gives 0/0 = NaN, which the game discards (comparison `< 0` false).
    if d.is_nan() || d >= 0.0 {
        return;
    }
    let p = [o[0] + xo, o[1] + y, o[2] + zo];
    let cmp = if g.ellipse { -0.5 } else { f64::from(-6 - rng.next_int_bounded(3)) };
    if d > cmp && rng.next_double() > 0.9 {
        return;
    }
    let b = r.b;
    let s = r.get_p(p);
    if b.is_air(s) || b.is(s, b.k.snow_block) || b.is(s, b.k.ice) || b.is(s, b.k.water) {
        let randomness = !g.ellipse || rng.next_double() > 0.05;
        let div = if g.ellipse { 3 } else { 2 };
        let hd = h - y;
        let snow = g.snow_on_top
            && !b.is(s, b.k.water)
            && f64::from(hd) <= f64::from(rng.next_int_bounded((h / div).max(1))) + f64::from(h) * 0.6
            && randomness;
        r.set_p(p, if snow { b.default_state(b.k.snow_block) } else { g.main }, 3);
    }
}

pub fn iceberg(cx: &mut Ctx, main: State, rng: &mut DecoRng, p: [i32; 3]) -> bool {
    let r = &mut *cx.r;
    let b = r.b;
    let o = [p[0], r.sea_level, p[2]];
    let snow_on_top = rng.next_double() > 0.7;
    let angle = rng.next_double() * 2.0 * std::f64::consts::PI;
    let ea = 11 - rng.next_int_bounded(5);
    let ec = 3 + rng.next_int_bounded(3);
    let ellipse = rng.next_double() > 0.7;
    let mut over = if ellipse { rng.next_int_bounded(6) + 6 } else { rng.next_int_bounded(15) + 3 };
    if !ellipse && rng.next_double() > 0.9 {
        over += rng.next_int_bounded(19) + 7;
    }
    let under = (over + rng.next_int_bounded(11)).min(18);
    let wa = rng.next_int_bounded(7);
    let width = (over + wa - rng.next_int_bounded(5)).min(11);
    let a = if ellipse { ea } else { 11 };
    let hoist = crate::opt::berg_trig();
    let g = Berg { ellipse, snow_on_top, angle, trig: hoist.then(|| trig(angle)), ea, ec, main };
    for xo in -a..a {
        for zo in -a..a {
            for y in 0..over {
                let radius = if ellipse { radius_ellipse(y, over, width) } else { radius_round(rng, y, over, width) };
                if ellipse || xo < radius {
                    berg_block(r, &g, rng, o, over, xo, y, zo, radius, a);
                }
            }
        }
    }
    // Smoothing.
    let sa = if ellipse { ea } else { width / 2 };
    let air = b.default_state(b.k.air);
    for x in -sa..=sa {
        for z in -sa..=sa {
            for y in 0..=over {
                let q = [o[0] + x, o[1] + y, o[2] + z];
                let s = r.get_p(q);
                if iceberg_state(b, s) || b.is(s, b.k.snow) {
                    if b.is_air(r.get_p(add(q, Dir::Down))) {
                        r.set_p(q, air, 3);
                        r.set_p(add(q, Dir::Up), air, 3);
                    } else if iceberg_state(b, s) {
                        let n = [Dir::West, Dir::East, Dir::North, Dir::South]
                            .iter()
                            .filter(|d| !iceberg_state(b, r.get_p(add(q, **d))))
                            .count();
                        if n >= 3 {
                            r.set_p(q, air, 3);
                        }
                    }
                }
            }
        }
    }
    for xo in -a..a {
        for zo in -a..a {
            let mut y = -1;
            while y > -under {
                let na = if ellipse {
                    (a as f32 * (1.0f32 - f64::from(y).powf(2.0) as f32 / (under as f32 * 8.0))).ceil() as i32
                } else {
                    a
                };
                let radius = radius_steep(rng, -y, under, width);
                if xo < radius {
                    berg_block(r, &g, rng, o, under, xo, y, zo, radius, na);
                }
                y -= 1;
            }
        }
    }
    let cut = if ellipse { rng.next_double() > 0.1 } else { rng.next_double() > 0.7 };
    if cut {
        let sx = if rng.next_bool() { -1 } else { 1 };
        let sz = if rng.next_bool() { -1 } else { 1 };
        let mut xoff = rng.next_int_bounded((width / 2 - 2).max(1));
        if rng.next_bool() {
            xoff = width / 2 + 1 - rng.next_int_bounded((width - width / 2 - 1).max(1));
        }
        let mut zoff = rng.next_int_bounded((width / 2 - 2).max(1));
        if rng.next_bool() {
            zoff = width / 2 + 1 - rng.next_int_bounded((width - width / 2 - 1).max(1));
        }
        if ellipse {
            xoff = rng.next_int_bounded((ea - 5).max(1));
            zoff = xoff;
        }
        let lo = [sx * xoff, 0, sz * zoff];
        let cangle =
            if ellipse { angle + std::f64::consts::FRAC_PI_2 } else { rng.next_double() * 2.0 * std::f64::consts::PI };
        let ctrig = hoist.then(|| trig(cangle));
        let mut y = 0;
        while y < over - 3 {
            let radius = radius_round(rng, y, over, width);
            carve(r, &g, radius, y, o, false, (cangle, ctrig), lo);
            y += 1;
        }
        let mut y = -1;
        while y > -over + rng.next_int_bounded(5) {
            let radius = radius_steep(rng, -y, over, width);
            carve(r, &g, radius, y, o, true, (cangle, ctrig), lo);
            y -= 1;
        }
    }
    true
}

#[allow(clippy::too_many_arguments)]
fn carve(r: &mut Region, g: &Berg, radius: i32, y: i32, o: [i32; 3], under: bool, angle: (f64, Option<(f64, f64)>), lo: [i32; 3]) {
    let b = r.b;
    let a = radius + 1 + g.ea / 3;
    let c = (radius - 3).min(3) + g.ec / 2 - 1;
    for xo in -a..a {
        for zo in -a..a {
            if ellipse_dist(xo, zo, lo[0], lo[2], a, c, angle.1.unwrap_or_else(|| trig(angle.0))) < 0.0 {
                let p = [o[0] + xo, o[1] + y, o[2] + zo];
                let s = r.get_p(p);
                if iceberg_state(b, s) || b.is(s, b.k.snow_block) {
                    if under {
                        r.set_p(p, b.default_state(b.k.water), 3);
                    } else {
                        r.set_p(p, b.default_state(b.k.air), 3);
                        let up = add(p, Dir::Up);
                        if b.is(r.get_p(up), b.k.snow) {
                            r.set_p(up, b.default_state(b.k.air), 3);
                        }
                    }
                }
            }
        }
    }
}

pub fn spike(cx: &mut Ctx, state: State, on: &Pred, replace: &Pred, rng: &mut DecoRng, p: [i32; 3]) -> bool {
    let r = &mut *cx.r;
    let b = r.b;
    let mut o = p;
    while b.is_air(r.get_p(o)) && o[1] > r.min_y + 2 {
        o = add(o, Dir::Down);
    }
    if !on.test(r, o) {
        return false;
    }
    o[1] += rng.next_int_bounded(4);
    let height = rng.next_int_bounded(4) + 7;
    let width = height / 4 + rng.next_int_bounded(2);
    if width > 1 && rng.next_int_bounded(60) == 0 {
        o[1] += 10 + rng.next_int_bounded(30);
    }
    for y in 0..height {
        let scale = (1.0f32 - y as f32 / height as f32) * width as f32;
        let nw = scale.ceil() as i32;
        for xo in -nw..=nw {
            let dx = xo.abs() as f32 - 0.25;
            for zo in -nw..=nw {
                let dz = zo.abs() as f32 - 0.25;
                let inside = (xo == 0 && zo == 0) || dx * dx + dz * dz <= scale * scale;
                if !inside {
                    continue;
                }
                let border = xo == -nw || xo == nw || zo == -nw || zo == nw;
                if border && rng.next_float() > 0.75 {
                    continue;
                }
                let q = [o[0] + xo, o[1] + y, o[2] + zo];
                if b.is_air(r.get_p(q)) || replace.test(r, q) {
                    r.set_p(q, state, 3);
                }
                if y != 0 && nw > 1 {
                    let q = [o[0] + xo, o[1] - y, o[2] + zo];
                    if b.is_air(r.get_p(q)) || replace.test(r, q) {
                        r.set_p(q, state, 3);
                    }
                }
            }
        }
    }
    let pw = (width - 1).clamp(0, 1);
    for xo in -pw..=pw {
        for zo in -pw..=pw {
            let mut c = [o[0] + xo, o[1] - 1, o[2] + zo];
            let mut run = 50;
            if xo.abs() == 1 && zo.abs() == 1 {
                run = rng.next_int_bounded(5);
            }
            while c[1] > 50 {
                let s = r.get_p(c);
                if !b.is_air(s) && !replace.test(r, c) && s != state {
                    break;
                }
                r.set_p(c, state, 3);
                c[1] -= 1;
                run -= 1;
                if run <= 0 {
                    c[1] -= rng.next_int_bounded(5) + 1;
                    run = rng.next_int_bounded(5);
                }
            }
        }
    }
    true
}
