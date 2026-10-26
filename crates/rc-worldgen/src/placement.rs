//! Placement modifiers and placement of a placed feature (`generation-decoration.md` § 3).

use crate::Error;
use crate::decor::Ctx;
use crate::decor_blocks::Dir;
use crate::decor_pred::{HeightP, IntP, ParseCtx, Pred, get_i32, opt_bool, opt_i32, vec3};
use crate::decor_region::Hm;
use crate::decor_rng::DecoRng;
use crate::feature::PlacedId;
use crate::json::Json;

#[derive(Debug, Clone)]
pub enum Modifier {
    Count(IntP),
    NoiseBasedCount { ratio: i32, factor: f64, offset: f64 },
    NoiseThresholdCount { level: f64, below: i32, above: i32 },
    CountOnEveryLayer(IntP),
    Rarity(i32),
    RandomChance(f32),
    InSquare,
    HeightRange(HeightP),
    Heightmap(Hm),
    Offset(IntP, IntP, IntP),
    Cuboid { xz: IntP, y: IntP, edges: bool, interior: bool },
    EnvScan { dir: Dir, target: Pred, allowed: Pred, max: i32 },
    SurfaceRelative { hm: Hm, min: i64, max: i64 },
    SurfaceWaterDepth(i32),
    Filter(Pred),
    Biome,
    RandomlySelected(Vec<Modifier>),
    Fixed(Vec<[i32; 3]>),
}

fn bad(w: impl Into<String>) -> Error {
    Error::Format(w.into())
}

impl Modifier {
    pub fn parse(j: &Json, c: &ParseCtx) -> Result<Modifier, Error> {
        let t = j.get("type").and_then(Json::as_str).ok_or_else(|| bad("modifier without type"))?;
        let t = t.strip_prefix("minecraft:").unwrap_or(t);
        let int = |k: &str| IntP::parse(j.get(k).ok_or_else(|| bad(k.to_owned()))?);
        let hm = |k: &str| {
            Hm::from_name(j.get(k).and_then(Json::as_str).ok_or_else(|| bad(k.to_owned()))?)
                .ok_or_else(|| bad("height map"))
        };
        let f64_of = |k: &str, d: Option<f64>| -> Result<f64, Error> {
            match j.get(k).and_then(Json::as_f64) {
                Some(v) => Ok(v),
                None => d.ok_or_else(|| bad(k.to_owned())),
            }
        };
        Ok(match t {
            "count" => Modifier::Count(int("count")?),
            "noise_based_count" => Modifier::NoiseBasedCount {
                ratio: get_i32(j, "noise_to_count_ratio")?,
                factor: f64_of("noise_factor", None)?,
                offset: f64_of("noise_offset", Some(0.0))?,
            },
            "noise_threshold_count" => Modifier::NoiseThresholdCount {
                level: f64_of("noise_level", None)?,
                below: get_i32(j, "below_noise")?,
                above: get_i32(j, "above_noise")?,
            },
            "count_on_every_layer" => Modifier::CountOnEveryLayer(int("count")?),
            "rarity_filter" => Modifier::Rarity(get_i32(j, "chance")?),
            "random_chance" => {
                Modifier::RandomChance(j.get("chance").and_then(Json::as_f32).ok_or_else(|| bad("chance"))?)
            }
            "in_square" => Modifier::InSquare,
            "height_range" => Modifier::HeightRange(HeightP::parse(j.get("height").ok_or_else(|| bad("height"))?)?),
            "heightmap" => Modifier::Heightmap(hm("heightmap")?),
            "offset" => Modifier::Offset(
                j.get("x").map_or(Ok(IntP::Const(0)), IntP::parse)?,
                j.get("y").map_or(Ok(IntP::Const(0)), IntP::parse)?,
                j.get("z").map_or(Ok(IntP::Const(0)), IntP::parse)?,
            ),
            "cuboid" => Modifier::Cuboid {
                xz: int("xz_size")?,
                y: int("y_size")?,
                edges: opt_bool(j, "include_edges", true),
                interior: opt_bool(j, "include_interior", true),
            },
            "environment_scan" => Modifier::EnvScan {
                dir: Dir::from_name(j.get("direction_of_search").and_then(Json::as_str).unwrap_or(""))
                    .ok_or_else(|| bad("direction_of_search"))?,
                target: Pred::parse(j.get("target_condition").ok_or_else(|| bad("target_condition"))?, c)?,
                allowed: match j.get("allowed_search_condition") {
                    Some(p) => Pred::parse(p, c)?,
                    None => Pred::True,
                },
                max: get_i32(j, "max_steps")?,
            },
            "surface_relative_threshold_filter" => Modifier::SurfaceRelative {
                hm: hm("heightmap")?,
                min: j.get("min_inclusive").and_then(Json::as_i32).map_or(i64::MIN / 4, i64::from),
                max: j.get("max_inclusive").and_then(Json::as_i32).map_or(i64::MAX / 4, i64::from),
            },
            "surface_water_depth_filter" => Modifier::SurfaceWaterDepth(get_i32(j, "max_water_depth")?),
            "block_predicate_filter" => {
                Modifier::Filter(Pred::parse(j.get("predicate").ok_or_else(|| bad("predicate"))?, c)?)
            }
            "biome" => Modifier::Biome,
            "randomly_selected" => Modifier::RandomlySelected(
                j.get("placements")
                    .and_then(Json::as_array)
                    .ok_or_else(|| bad("placements"))?
                    .iter()
                    .map(|m| Modifier::parse(m, c))
                    .collect::<Result<_, _>>()?,
            ),
            "fixed_placement" => Modifier::Fixed(
                j.get("positions")
                    .and_then(Json::as_array)
                    .ok_or_else(|| bad("positions"))?
                    .iter()
                    .map(|p| vec3(Some(p)))
                    .collect::<Result<_, _>>()?,
            ),
            t => return Err(bad(format!("modifier {t}"))),
        })
    }

    /// Applies the modifier to p; the positions produced go into `out`.
    pub fn apply(&self, cx: &mut Ctx, rng: &mut DecoRng, p: [i32; 3], top: Option<PlacedId>, out: &mut Vec<[i32; 3]>) {
        let r = &*cx.r;
        match self {
            Modifier::Count(c) => {
                let n = c.sample(rng);
                for _ in 0..n {
                    out.push(p);
                }
            }
            Modifier::NoiseBasedCount { ratio, factor, offset } => {
                let v = f64::from(cx.d.info_noise.get2(f64::from(p[0]) / factor, f64::from(p[2]) / factor));
                let n = ((v + offset) * f64::from(*ratio)).ceil() as i32;
                for _ in 0..n {
                    out.push(p);
                }
            }
            Modifier::NoiseThresholdCount { level, below, above } => {
                let v = f64::from(cx.d.info_noise.get2(f64::from(p[0]) / 200.0, f64::from(p[2]) / 200.0));
                let n = if v < *level { *below } else { *above };
                for _ in 0..n {
                    out.push(p);
                }
            }
            Modifier::CountOnEveryLayer(c) => {
                let b = r.b;
                let empty = |s| b.is_air(s) || b.is(s, b.k.water) || b.is(s, b.k.lava);
                let mut layer = 0;
                loop {
                    let mut found = false;
                    let mut i = 0;
                    while i < c.sample(rng) {
                        let x = rng.next_int_bounded(16) + p[0];
                        let z = rng.next_int_bounded(16) + p[2];
                        let start = r.height_at(Hm::MotionBlocking, x, z);
                        // Search for the layer "layer".
                        let mut cur_layer = 0;
                        let mut current = r.get(x, start, z);
                        let mut res = None;
                        let mut y = start;
                        while y > r.min_y {
                            let below = r.get(x, y - 1, z);
                            if !empty(below) && empty(current) && !b.is(below, b.k.bedrock) {
                                if cur_layer == layer {
                                    res = Some(y);
                                    break;
                                }
                                cur_layer += 1;
                            }
                            current = below;
                            y -= 1;
                        }
                        if let Some(y) = res {
                            out.push([x, y, z]);
                            found = true;
                        }
                        i += 1;
                    }
                    layer += 1;
                    if !found {
                        break;
                    }
                }
            }
            Modifier::Rarity(c) => {
                if rng.next_float() < 1.0 / *c as f32 {
                    out.push(p);
                }
            }
            Modifier::RandomChance(c) => {
                if rng.next_float() < *c {
                    out.push(p);
                }
            }
            Modifier::InSquare => {
                let x = rng.next_int_bounded(16) + p[0];
                let z = rng.next_int_bounded(16) + p[2];
                out.push([x, p[1], z]);
            }
            Modifier::HeightRange(h) => {
                let y = h.sample(rng, r.gen_ctx());
                out.push([p[0], y, p[2]]);
            }
            Modifier::Heightmap(hm) => {
                let h = r.height_at(*hm, p[0], p[2]);
                if h > r.min_y {
                    out.push([p[0], h, p[2]]);
                }
            }
            Modifier::Offset(x, y, z) => {
                let dx = x.sample(rng);
                let dy = y.sample(rng);
                let dz = z.sample(rng);
                out.push([p[0] + dx, p[1] + dy, p[2] + dz]);
            }
            Modifier::Cuboid { xz, y, edges, interior } => {
                let h = y.sample(rng);
                let w = xz.sample(rng);
                let l = xz.sample(rng);
                for x in 0..=w {
                    for yy in 0..=h {
                        for z in 0..=l {
                            let xe = x == 0 || x == w;
                            let ye = yy == 0 || yy == h;
                            let ze = z == 0 || z == l;
                            if (*edges || !xe || !ye)
                                && (*edges || !ze || !ye)
                                && (*edges || !xe || !ze)
                                && (*interior || xe || ye || ze)
                            {
                                out.push([p[0] + x, p[1] + yy, p[2] + z]);
                            }
                        }
                    }
                }
            }
            Modifier::EnvScan { dir, target, allowed, max } => {
                let mut q = p;
                if !allowed.test(r, q) {
                    return;
                }
                for _ in 0..*max {
                    if target.test(r, q) {
                        out.push(q);
                        return;
                    }
                    q = crate::decor_region::add(q, *dir);
                    if r.outside_height(q[1]) {
                        return;
                    }
                    if !allowed.test(r, q) {
                        break;
                    }
                }
                if target.test(r, q) {
                    out.push(q);
                }
            }
            Modifier::SurfaceRelative { hm, min, max } => {
                let s = i64::from(r.height_at(*hm, p[0], p[2]));
                let y = i64::from(p[1]);
                if s + min <= y && y <= s + max {
                    out.push(p);
                }
            }
            Modifier::SurfaceWaterDepth(m) => {
                let of = r.height_at(Hm::OceanFloor, p[0], p[2]);
                let ws = r.height_at(Hm::WorldSurface, p[0], p[2]);
                if ws - of <= *m {
                    out.push(p);
                }
            }
            Modifier::Filter(pred) => {
                if pred.test(r, p) {
                    out.push(p);
                }
            }
            Modifier::Biome => {
                let Some(top) = top else { return };
                let bi = r.biome(p[0], p[1], p[2]);
                if cx.d.biome_has_feature(bi, top) {
                    out.push(p);
                }
            }
            Modifier::RandomlySelected(list) => {
                let i = rng.next_int_bounded(list.len() as i32) as usize;
                list[i].apply(cx, rng, p, top, out);
            }
            Modifier::Fixed(list) => {
                let (cxx, czz) = (p[0] >> 4, p[2] >> 4);
                for q in list {
                    if q[0] >> 4 == cxx && q[2] >> 4 == czz {
                        out.push(*q);
                    }
                }
            }
        }
    }
}

/// Places a placed feature from `origin` (§ 3.1); `top` = top-level feature (biome
/// check) or nothing for a nested feature.
pub fn place(cx: &mut Ctx, id: PlacedId, rng: &mut DecoRng, origin: [i32; 3], top: Option<PlacedId>) -> bool {
    let d = cx.d;
    let placed = &d.reg.placed[id.0 as usize];
    let feature = placed.feature;
    let mods = &placed.placement;
    if mods.is_empty() {
        return crate::feature::place_feature(cx, feature, rng, origin);
    }
    let mut placed_any = false;
    let mut stack: Vec<([i32; 3], u16)> = cx.take_stack();
    let mut produced: Vec<[i32; 3]> = cx.take_positions();
    stack.push((origin, 0));
    while let Some((p, k)) = stack.pop() {
        produced.clear();
        mods[k as usize].apply(cx, rng, p, top, &mut produced);
        let next = k + 1;
        if (next as usize) < mods.len() {
            for &q in produced.iter().rev() {
                stack.push((q, next));
            }
        } else {
            // Positions produced then placements (the list may be reused by the feature).
            let list: Vec<[i32; 3]> = std::mem::take(&mut produced);
            for &q in &list {
                placed_any |= crate::feature::place_feature(cx, feature, rng, q);
            }
            produced = list;
        }
    }
    cx.give_stack(stack);
    cx.give_positions(produced);
    placed_any
}

/// Height of a default anchor (unused elsewhere).
pub fn default_int(j: &Json, k: &str, d: i32) -> i32 {
    opt_i32(j, k, d)
}
