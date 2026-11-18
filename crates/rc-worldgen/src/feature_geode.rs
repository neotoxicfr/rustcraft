//! Geodes (`generation-decoration.md` § 6.4).

use crate::Error;
use crate::decor::Ctx;
use crate::decor_blocks::{Dir, State};
use crate::decor_pred::{IntP, ParseCtx, Provider};
use crate::decor_region::add;
use crate::decor_rng::DecoRng;
use crate::feature::Geode;
use crate::json::Json;
use crate::noise::{NoiseStack, NormalNoise, Normalize};
use crate::rng::Rng;

pub fn parse(j: &Json, c: &mut ParseCtx) -> Result<Geode, Error> {
    let bad = |w: &str| Error::Format(format!("geode: {w}"));
    let blocks = j.get("blocks").ok_or_else(|| bad("blocks"))?;
    let layers = j.get("layers").ok_or_else(|| bad("layers"))?;
    let crack = j.get("crack").ok_or_else(|| bad("crack"))?;
    let b = c.b;
    let prov = |k: &str, c: &mut ParseCtx| Provider::parse(blocks.get(k).ok_or_else(|| bad(k))?, c);
    let tag = |k: &str| -> Result<u16, Error> {
        let n = blocks.get(k).and_then(Json::as_str).ok_or_else(|| bad(k))?;
        b.tag(n).ok_or_else(|| Error::Missing(format!("tag {n}")))
    };
    let f64_or = |o: &Json, k: &str, d: f64| o.get(k).and_then(Json::as_f64).unwrap_or(d);
    let int_or = |k: &str, d: IntP| -> Result<IntP, Error> { j.get(k).map_or(Ok(d), IntP::parse) };
    let mut inner_placements = Vec::new();
    for s in blocks.get("inner_placements").and_then(Json::as_array).ok_or_else(|| bad("inner_placements"))? {
        inner_placements.push(b.parse_state(s)?);
    }
    Ok(Geode {
        filling: prov("filling_provider", c)?,
        inner: prov("inner_layer_provider", c)?,
        alternate_inner: prov("alternate_inner_layer_provider", c)?,
        middle: prov("middle_layer_provider", c)?,
        outer: prov("outer_layer_provider", c)?,
        inner_placements,
        cannot_replace: tag("cannot_replace")?,
        invalid_blocks: tag("invalid_blocks")?,
        filling_l: f64_or(layers, "filling", 1.7),
        inner_l: f64_or(layers, "inner_layer", 2.2),
        middle_l: f64_or(layers, "middle_layer", 3.2),
        outer_l: f64_or(layers, "outer_layer", 4.2),
        crack_chance: f64_or(crack, "generate_crack_chance", 1.0),
        base_crack_size: f64_or(crack, "base_crack_size", 2.0),
        crack_point_offset: crack.get("crack_point_offset").and_then(Json::as_i32).unwrap_or(2),
        use_potential: f64_or(j, "use_potential_placements_chance", 0.35),
        use_alternate: f64_or(j, "use_alternate_layer0_chance", 0.0),
        require_alternate: j.get("placements_require_layer0_alternate").and_then(Json::as_bool).unwrap_or(true),
        outer_wall: int_or("outer_wall_distance", IntP::Uniform(4, 5))?,
        distribution: int_or("distribution_points", IntP::Uniform(3, 4))?,
        point_offset: int_or("point_offset", IntP::Uniform(1, 2))?,
        min_gen: j.get("min_gen_offset").and_then(Json::as_i32).unwrap_or(-16),
        max_gen: j.get("max_gen_offset").and_then(Json::as_i32).unwrap_or(16),
        noise_multiplier: f64_or(j, "noise_multiplier", 0.05),
        invalid_threshold: j
            .get("invalid_blocks_threshold")
            .and_then(Json::as_i32)
            .ok_or_else(|| bad("invalid_blocks_threshold"))?,
    })
}

/// Geode noise: normal "parity" noise (octave −4, amplitude 1), seed = world seed.
pub fn geode_noise(seed: i64) -> NoiseStack {
    // Parity factor brought back to the standard normalisation (creation of the parity).
    let dev = (0.270_224_783_124_521_1f64 * 1.0).abs();
    let new_factor = 1.0 * 0.333_333_333_333_333_3 / ((dev * dev).sqrt() * 2f64.sqrt());
    let old_factor = 1.0 * 0.5 * 0.333_333_333_333_333_3 / (0.1 * 2.0);
    let base = old_factor / new_factor;
    NormalNoise::new(base, -4, 1, Normalize::Enabled, Vec::new()).create(&mut Rng::legacy(seed))
}

fn safe_set(cx: &mut Ctx, g: &Geode, p: [i32; 3], s: State) {
    let b = cx.r.b;
    if !b.in_tag(cx.r.get_p(p), g.cannot_replace) {
        cx.r.set_p(p, s, 2);
    }
}

pub fn place(cx: &mut Ctx, g: &Geode, rng: &mut DecoRng, o: [i32; 3]) -> bool {
    let b = cx.r.b;
    let n = g.distribution.sample(rng);
    let noise = cx.d.geode_noise();
    let adj = f64::from(n) / f64::from(g.outer_wall.max_value());
    let inner_air = 1.0 / g.filling_l.sqrt();
    let innermost = 1.0 / (g.inner_l + adj).sqrt();
    let inner_crust = 1.0 / (g.middle_l + adj).sqrt();
    let outer_crust = 1.0 / (g.outer_l + adj).sqrt();
    let crack_size = 1.0 / (g.base_crack_size + rng.next_double() / 2.0 + if n > 3 { adj } else { 0.0 }).sqrt();
    let crack = f64::from(rng.next_float()) < g.crack_chance;
    let mut invalid = 0;
    let mut points: Vec<([i32; 3], i32)> = Vec::with_capacity(n.max(0) as usize);
    for _ in 0..n {
        let x = g.outer_wall.sample(rng);
        let y = g.outer_wall.sample(rng);
        let z = g.outer_wall.sample(rng);
        let p = [o[0] + x, o[1] + y, o[2] + z];
        let s = cx.r.get_p(p);
        if b.is_air(s) || b.in_tag(s, g.invalid_blocks) {
            invalid += 1;
            if invalid > g.invalid_threshold {
                return false;
            }
        }
        points.push((p, g.point_offset.sample(rng)));
    }
    let mut cracks: Vec<[i32; 3]> = Vec::new();
    if crack {
        let k = rng.next_int_bounded(4);
        let c = n * 2 + 1;
        let (dx, dz) = match k {
            0 => (c, 0),
            1 => (0, c),
            2 => (c, c),
            _ => (0, 0),
        };
        for dy in [7, 5, 1] {
            cracks.push([o[0] + dx, o[1] + dy, o[2] + dz]);
        }
    }
    let dist2 = |a: [i32; 3], c: [i32; 3]| {
        let dx = f64::from(a[0]) - f64::from(c[0]);
        let dy = f64::from(a[1]) - f64::from(c[1]);
        let dz = f64::from(a[2]) - f64::from(c[2]);
        dx * dx + dy * dy + dz * dz
    };
    let mut crystals: Vec<[i32; 3]> = Vec::new();
    let air = b.default_state(b.k.air);
    // Noise bound: a cell whose noise-free sum stays under the crust even with maximal noise
    // is outside the geode (no draw takes place there); we then avoid evaluating the noise.
    let range = noise.range();
    let bound = f64::from(range.min.abs().max(range.max.abs())) * g.noise_multiplier.abs() * points.len() as f64 + 1e-9;
    // 1/√ terms of the first pass kept for the second (`RUSTCRAFT_OPT_GEODE_REUSE`); row
    // in x skipped at once when even its maximum possible stays under the crust
    // (`RUSTCRAFT_OPT_GEODE_ROWSKIP`).
    let reuse = crate::opt::geode_reuse() && points.len() <= 16;
    let rowskip = crate::opt::geode_rowskip();
    let mut terms = [0.0f64; 16];
    let (x_lo, x_hi) = (o[0] + g.min_gen, o[0] + g.max_gen);
    for z in o[2] + g.min_gen..=o[2] + g.max_gen {
        for y in o[1] + g.min_gen..=o[1] + g.max_gen {
            if rowskip {
                // For each point, the cell of the nearest row; exact integer distances,
                // root and division increasing: each term of the row is <= its maximum, and
                // the sum (same order) too.
                let mut max = 0.0;
                for (pt, po) in &points {
                    let cx = pt[0].clamp(x_lo, x_hi);
                    max += 1.0 / (dist2([cx, y, z], *pt) + f64::from(*po)).sqrt();
                }
                if max + bound < outer_crust {
                    continue;
                }
            }
            for x in x_lo..=x_hi {
                let p = [x, y, z];
                let mut raw = 0.0;
                for (i, (pt, po)) in points.iter().enumerate() {
                    let t = 1.0 / (dist2(p, *pt) + f64::from(*po)).sqrt();
                    if reuse {
                        terms[i] = t;
                    }
                    raw += t;
                }
                if raw + bound < outer_crust {
                    continue;
                }
                let off = f64::from(noise.get(f64::from(x), f64::from(y), f64::from(z))) * g.noise_multiplier;
                let mut shell = 0.0;
                if reuse {
                    for t in &terms[..points.len()] {
                        shell += t + off;
                    }
                } else {
                    for (pt, po) in &points {
                        shell += 1.0 / (dist2(p, *pt) + f64::from(*po)).sqrt() + off;
                    }
                }
                if shell < outer_crust {
                    continue;
                }
                if shell >= inner_air {
                    let s = g.filling.get(cx.r, rng, p);
                    safe_set(cx, g, p, s);
                } else {
                    let mut cs = 0.0;
                    for c in &cracks {
                        cs += 1.0 / (dist2(p, *c) + f64::from(g.crack_point_offset)).sqrt() + off;
                    }
                    if crack && cs >= crack_size {
                        safe_set(cx, g, p, air);
                        // The fluids neighbouring the crack flow into it.
                        for d in Dir::ALL {
                            let q = add(p, d);
                            let s = cx.r.get_p(q);
                            cx.r.schedule_fluid_tick(q, s);
                        }
                    } else if shell >= innermost {
                        let alt = f64::from(rng.next_float()) < g.use_alternate;
                        let s = if alt { g.alternate_inner.get(cx.r, rng, p) } else { g.inner.get(cx.r, rng, p) };
                        safe_set(cx, g, p, s);
                        if (!g.require_alternate || alt) && f64::from(rng.next_float()) < g.use_potential {
                            crystals.push(p);
                        }
                    } else if shell >= inner_crust {
                        let s = g.middle.get(cx.r, rng, p);
                        safe_set(cx, g, p, s);
                    } else if shell >= outer_crust {
                        let s = g.outer.get(cx.r, rng, p);
                        safe_set(cx, g, p, s);
                    }
                }
            }
        }
    }
    for c in crystals {
        let mut s = g.inner_placements[rng.next_int_bounded(g.inner_placements.len() as i32) as usize];
        for d in Dir::ALL {
            if b.has(s, b.p.facing) {
                s = b.with_dir(s, b.p.facing, d);
            }
            let q = add(c, d);
            let ps = cx.r.get_p(q);
            if b.has(s, b.p.waterlogged) {
                s = b.with_bool(s, b.p.waterlogged, b.fluid(ps) != 0 && b.is_source(ps));
            }
            if b.is_air(ps) || (b.is(ps, b.k.water) && b.facts(ps).fluid_amount == 8) {
                safe_set(cx, g, q, s);
                break;
            }
        }
    }
    true
}
