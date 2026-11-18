//! Decoration features placed from templates (`generation-decoration.md` § 6.4, § 6.9): "template"
//! (desert well, sulfur springs) and fossils. The feature's randomness also serves as randomness of the
//! settings (palette, processors).

use crate::Error;
use crate::decor::Ctx;
use crate::decor_blocks::Dir;
use crate::decor_pred::Weighted;
use crate::decor_region::Hm;
use crate::decor_rng::DecoRng;
use crate::json::Json;
use crate::structure_place::{Placing, place_template};
use crate::template::{BBox, Mirror, Rot, Settings, zero_position};

#[derive(Debug, Clone)]
pub struct TemplateCfg {
    templates: Weighted<(String, Vec<Rot>)>,
    processors: Json,
}

fn rot_of(s: &str) -> Option<Rot> {
    Some(match s.strip_prefix("minecraft:").unwrap_or(s) {
        "none" => Rot::None,
        "clockwise_90" => Rot::Cw90,
        "180" | "clockwise_180" => Rot::Cw180,
        "counterclockwise_90" => Rot::Ccw90,
        _ => return None,
    })
}

impl TemplateCfg {
    pub fn parse(j: &Json) -> Result<TemplateCfg, Error> {
        let mut items = Vec::new();
        for e in j.get("templates").and_then(Json::as_array).unwrap_or(&[]) {
            let d = e.get("data").unwrap_or(e);
            let id = crate::data::full_id(d.get("id").and_then(Json::as_str).unwrap_or(""));
            let rots = match d.get("rotations").and_then(Json::as_array) {
                Some(a) => a.iter().filter_map(|r| r.as_str().and_then(rot_of)).collect(),
                None => Rot::ALL.to_vec(),
            };
            items.push(((id, rots), e.get("weight").and_then(Json::as_i32).unwrap_or(1)));
        }
        Ok(TemplateCfg {
            templates: Weighted::new(items),
            processors: j.get("processors").cloned().unwrap_or(Json::Null),
        })
    }
}

#[derive(Debug, Clone)]
pub struct FossilCfg {
    fossils: Vec<String>,
    overlays: Vec<String>,
    fossil_procs: Json,
    overlay_procs: Json,
    max_empty: i32,
}

impl FossilCfg {
    pub fn parse(j: &Json) -> Result<FossilCfg, Error> {
        let list = |k: &str| -> Vec<String> {
            j.get(k)
                .and_then(Json::as_array)
                .unwrap_or(&[])
                .iter()
                .filter_map(Json::as_str)
                .map(crate::data::full_id)
                .collect()
        };
        Ok(FossilCfg {
            fossils: list("fossil_structures"),
            overlays: list("overlay_structures"),
            fossil_procs: j.get("fossil_processors").cloned().unwrap_or(Json::Null),
            overlay_procs: j.get("overlay_processors").cloned().unwrap_or(Json::Null),
            max_empty: j.get("max_empty_corners_allowed").and_then(Json::as_i32).unwrap_or(0),
        })
    }
}

pub fn place_template_feature(cx: &mut Ctx, c: &TemplateCfg, rng: &mut DecoRng, origin: [i32; 3]) -> bool {
    let (Some(templates), Some(procs)) = (cx.d.templates.clone(), cx.d.procs.clone()) else { return false };
    let Some((id, rots)) = c.templates.pick(rng).cloned() else { return false };
    if rots.is_empty() {
        return false;
    }
    let rot = rots[rng.next_int_bounded(rots.len() as i32) as usize];
    let b = cx.r.b;
    let Some(t) = templates.get(&id, b) else { return false };
    let ox = rot.dir(Dir::West).offset();
    let oz = rot.dir(Dir::North).offset();
    let (hx, hz) = (t.size[0] / 2, t.size[2] / 2);
    let pos = [origin[0] + ox[0] * hx + oz[0] * hz, origin[1], origin[2] + ox[2] * hx + oz[2] * hz];
    let s = Placing {
        mirror: Mirror::None,
        rot,
        pivot: [0, 0, 0],
        bbox: None,
        procs: crate::structure_place::ProcChain::of(procs.get(&c.processors, b)),
        waterlog: true,
        seed: None,
        shared: true,
        known_shape: false,
        entities: Some(None),
    };
    place_template(cx.r, b, &templates.transforms, cx.d.seed, &t, pos, pos, &s, rng, 3)
}

pub fn place_fossil(cx: &mut Ctx, c: &FossilCfg, rng: &mut DecoRng, origin: [i32; 3]) -> bool {
    let (Some(templates), Some(procs)) = (cx.d.templates.clone(), cx.d.procs.clone()) else { return false };
    let rot = Rot::from_index(rng.next_int_bounded(4));
    if c.fossils.is_empty() {
        return false;
    }
    let i = rng.next_int_bounded(c.fossils.len() as i32) as usize;
    let b = cx.r.b;
    let (Some(base), Some(over)) =
        (templates.get(&c.fossils[i], b), c.overlays.get(i).and_then(|o| templates.get(o, b)))
    else {
        return false;
    };
    let (ccx, ccz) = (origin[0] >> 4, origin[2] >> 4);
    let zone =
        BBox::new([ccx * 16 - 16, cx.r.min_y, ccz * 16 - 16], [ccx * 16 + 15 + 16, cx.r.max_y(), ccz * 16 + 15 + 16]);
    let size = base.size_rot(rot);
    let low = [origin[0] - size[0] / 2, origin[1], origin[2] - size[2] / 2];
    let mut lowest = origin[1];
    for x in 0..size[0] {
        for z in 0..size[2] {
            lowest = lowest.min(cx.r.height_at(Hm::OceanFloorWg, low[0] + x, low[2] + z));
        }
    }
    let ty = (lowest - 15 - rng.next_int_bounded(10)).max(cx.r.min_y + 10);
    let target = zero_position([low[0], ty, low[2]], base.size, Mirror::None, rot);
    let bb = base.bbox(target, &Settings { mirror: Mirror::None, rot, pivot: [0, 0, 0] });
    let mut empty = 0;
    for x in [bb.min[0], bb.max[0]] {
        for y in [bb.min[1], bb.max[1]] {
            for z in [bb.min[2], bb.max[2]] {
                let s = cx.r.get(x, y, z);
                if b.is_air(s) || b.is(s, b.k.lava) || b.is(s, b.k.water) {
                    empty += 1;
                }
            }
        }
    }
    if empty > c.max_empty {
        return false;
    }
    let mut s = Placing {
        mirror: Mirror::None,
        rot,
        pivot: [0, 0, 0],
        bbox: Some(zone),
        procs: crate::structure_place::ProcChain::of(procs.get(&c.fossil_procs, b)),
        waterlog: true,
        seed: None,
        shared: true,
        known_shape: false,
        entities: Some(None),
    };
    place_template(cx.r, b, &templates.transforms, cx.d.seed, &base, target, target, &s, rng, 260);
    s.procs = crate::structure_place::ProcChain::of(procs.get(&c.overlay_procs, b));
    place_template(cx.r, b, &templates.transforms, cx.d.seed, &over, target, target, &s, rng, 260);
    true
}
