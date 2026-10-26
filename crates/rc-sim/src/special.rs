//! Special crafting-table recipes (S7, `mechanics-s7.md` § 6): firework rocket and star,
//! star fade, written book copy, shield decoration, banner copy,
//! decorated pot. Ingredients read from the pack (`crafting_special_*`), result and leftovers computed
//! like the game.

use rc_proto::nbt::Nbt;
use serde_json::Value;

use crate::items::{Items, component, set_component};
use crate::player::Stack;
use crate::recipes::{Ingredient, Template, ingredient, json_to_nbt};

#[derive(Debug, Clone)]
pub enum Spec {
    Rocket {
        shell: Ingredient,
        fuel: Ingredient,
        star: Ingredient,
        result: Template,
    },
    Star {
        dye: Ingredient,
        fuel: Ingredient,
        trail: Ingredient,
        twinkle: Ingredient,
        shapes: Vec<(&'static str, Ingredient)>,
        result: Template,
    },
    StarFade {
        dye: Ingredient,
        target: Ingredient,
        result: Template,
    },
    BookCloning {
        source: Ingredient,
        material: Ingredient,
        result: Template,
    },
    Shield {
        banner: Ingredient,
        target: Ingredient,
        result: Template,
    },
    BannerDuplicate {
        banner: Ingredient,
        result: Template,
    },
    Pot {
        sides: [Ingredient; 4],
        result: Template,
    },
    /// S8: map extension (`crafting_special_mapextending`, 8 papers around).
    MapExtend {
        map: Ingredient,
        material: Ingredient,
    },
}

/// Firework colours of the 16 dyes (the game's order).
const FIREWORK: [(&str, i32); 16] = [
    ("white", 15790320),
    ("orange", 15435844),
    ("magenta", 12801229),
    ("light_blue", 6719955),
    ("yellow", 14602026),
    ("lime", 4312372),
    ("pink", 14188952),
    ("gray", 4408131),
    ("light_gray", 11250603),
    ("cyan", 2651799),
    ("purple", 8073150),
    ("blue", 2437522),
    ("brown", 5320730),
    ("green", 3887386),
    ("red", 11743532),
    ("black", 1973019),
];

fn template(v: &Value) -> Option<Template> {
    Some(Template {
        id: v["id"].as_str()?.to_owned(),
        count: v["count"].as_i64().unwrap_or(1) as i32,
        components: v.get("components").filter(|c| c.is_object()).map(json_to_nbt),
    })
}

pub fn parse(items: &Items, ty: &str, v: &Value) -> Option<Spec> {
    let ing = |k: &str| ingredient(items, &v[k]);
    let res = || template(&v["result"]);
    Some(match ty {
        "crafting_special_firework_rocket" => {
            Spec::Rocket { shell: ing("shell")?, fuel: ing("fuel")?, star: ing("star")?, result: res()? }
        }
        "crafting_special_firework_star" => {
            let mut shapes = Vec::new();
            for name in ["large_ball", "star", "creeper", "burst"] {
                if let Some(i) = ingredient(items, &v["shapes"][name]) {
                    shapes.push((name, i));
                }
            }
            Spec::Star {
                dye: ing("dye")?,
                fuel: ing("fuel")?,
                trail: ing("trail")?,
                twinkle: ing("twinkle")?,
                shapes,
                result: res()?,
            }
        }
        "crafting_special_firework_star_fade" => {
            Spec::StarFade { dye: ing("dye")?, target: ing("target")?, result: res()? }
        }
        "crafting_special_bookcloning" => {
            Spec::BookCloning { source: ing("source")?, material: ing("material")?, result: res()? }
        }
        "crafting_special_shielddecoration" => {
            Spec::Shield { banner: ing("banner")?, target: ing("target")?, result: res()? }
        }
        "crafting_special_bannerduplicate" => Spec::BannerDuplicate { banner: ing("banner")?, result: res()? },
        "crafting_decorated_pot" => {
            Spec::Pot { sides: [ing("back")?, ing("left")?, ing("right")?, ing("front")?], result: res()? }
        }
        "crafting_special_mapextending" => Spec::MapExtend { map: ing("map")?, material: ing("material")? },
        _ => return None,
    })
}

fn has(items: &Items, ing: &[i32], s: &Stack) -> bool {
    items.id(&s.id).is_some_and(|i| ing.contains(&i))
}

pub(crate) fn firework_color(dye: &str) -> i32 {
    FIREWORK.iter().find(|(n, _)| *n == dye).map_or(FIREWORK[0].1, |c| c.1)
}

fn patterns(s: &Stack) -> usize {
    match component(s, "minecraft:banner_patterns") {
        Some(Nbt::List(l)) => l.len(),
        _ => 0,
    }
}

fn banner_color(id: &str) -> Option<&str> {
    id.strip_prefix("minecraft:")?.strip_suffix("_banner")
}

fn generation(s: &Stack) -> i64 {
    component(s, "minecraft:written_book_content").and_then(|c| c.get("generation")).and_then(Nbt::as_i64).unwrap_or(0)
}

/// Result with the original components of a stack.
fn with_original(result: &Template, from: &Stack, extra: i32) -> Stack {
    let mut out = Stack { id: result.id.clone(), count: result.count + extra, components: from.components.clone() };
    if let Some(Nbt::Compound(f)) = &result.components {
        for (k, v) in f {
            set_component(&mut out, k, v.clone());
        }
    }
    out
}

/// Assembly: `None` if the grid does not match. `map_scale`: scale of a map
/// known to the world (map extension).
pub fn assemble(
    items: &Items,
    spec: &Spec,
    input: &[Option<&Stack>],
    tw: usize,
    th: usize,
    stacks: &[&Stack],
    map_scale: crate::recipes::MapScale<'_>,
) -> Option<Stack> {
    let n = stacks.len();
    match spec {
        Spec::Rocket { shell, fuel, star, result } => {
            if n < 2 {
                return None;
            }
            let mut has_shell = false;
            let mut fuel_n = 0;
            let mut explosions = Vec::new();
            for s in stacks {
                if has(items, shell, s) {
                    if has_shell {
                        return None;
                    }
                    has_shell = true;
                } else if has(items, fuel, s) {
                    fuel_n += 1;
                    if fuel_n > 3 {
                        return None;
                    }
                } else if has(items, star, s) {
                    if let Some(e) = component(s, "minecraft:firework_explosion") {
                        explosions.push(e.clone());
                    }
                } else {
                    return None;
                }
            }
            if !has_shell || fuel_n < 1 {
                return None;
            }
            let mut out = result.stack();
            set_component(
                &mut out,
                "minecraft:fireworks",
                Nbt::Compound(vec![
                    ("flight_duration".into(), Nbt::Byte(fuel_n as i8)),
                    ("explosions".into(), Nbt::List(explosions)),
                ]),
            );
            Some(out)
        }
        Spec::Star { dye, fuel, trail, twinkle, shapes, result } => {
            if n < 2 {
                return None;
            }
            let (mut f, mut d, mut sh, mut tr, mut tw_) = (false, false, false, false, false);
            let shape_of = |s: &Stack| shapes.iter().find(|(_, i)| has(items, i, s)).map(|(n, _)| *n);
            for s in stacks {
                if has(items, twinkle, s) {
                    if tw_ {
                        return None;
                    }
                    tw_ = true;
                } else if has(items, trail, s) {
                    if tr {
                        return None;
                    }
                    tr = true;
                } else if has(items, fuel, s) {
                    if f {
                        return None;
                    }
                    f = true;
                } else if has(items, dye, s) && items.dye(s).is_some() {
                    d = true;
                } else {
                    shape_of(s)?;
                    if sh {
                        return None;
                    }
                    sh = true;
                }
            }
            if !(f && d) {
                return None;
            }
            let mut shape = "small_ball";
            let mut colors = Vec::new();
            for s in stacks {
                if let Some(x) = shape_of(s) {
                    shape = x;
                } else if has(items, twinkle, s) || has(items, trail, s) {
                } else if has(items, dye, s) {
                    colors.push(firework_color(&items.dye(s).unwrap_or_else(|| "white".into())));
                }
            }
            let mut out = result.stack();
            set_component(
                &mut out,
                "minecraft:firework_explosion",
                Nbt::Compound(vec![
                    ("shape".into(), Nbt::String(shape.into())),
                    ("colors".into(), Nbt::IntArray(colors)),
                    ("fade_colors".into(), Nbt::IntArray(Vec::new())),
                    ("has_trail".into(), Nbt::Byte(i8::from(tr))),
                    ("has_twinkle".into(), Nbt::Byte(i8::from(tw_))),
                ]),
            );
            Some(out)
        }
        Spec::StarFade { dye, target, result } => {
            if n < 2 {
                return None;
            }
            let mut tgt = None;
            let mut colors = Vec::new();
            for s in stacks {
                if has(items, dye, s) && items.dye(s).is_some() {
                    colors.push(firework_color(&items.dye(s).unwrap_or_default()));
                } else if has(items, target, s) && tgt.is_none() {
                    tgt = Some(*s);
                } else {
                    return None;
                }
            }
            let t = tgt?;
            if colors.is_empty() {
                return None;
            }
            let mut out = with_original(result, t, 0);
            let mut e = component(t, "minecraft:firework_explosion")
                .cloned()
                .unwrap_or_else(|| Nbt::Compound(vec![("shape".into(), Nbt::String("small_ball".into()))]));
            crate::nbtio::set(&mut e, "fade_colors", Nbt::IntArray(colors));
            set_component(&mut out, "minecraft:firework_explosion", e);
            Some(out)
        }
        Spec::BookCloning { source, material, result } => {
            if n < 2 {
                return None;
            }
            let mut src = None;
            let mut count = 0;
            for s in stacks {
                if has(items, source, s) {
                    if component(s, "minecraft:written_book_content").is_none() || generation(s) >= 2 || src.is_some() {
                        return None;
                    }
                    src = Some(*s);
                } else if has(items, material, s) {
                    count += 1;
                } else {
                    return None;
                }
            }
            let src = src?;
            if count == 0 {
                return None;
            }
            let mut out = with_original(result, src, count - 1);
            let mut c = component(src, "minecraft:written_book_content").cloned()?;
            crate::nbtio::set(&mut c, "generation", Nbt::Int((generation(src) + 1) as i32));
            set_component(&mut out, "minecraft:written_book_content", c);
            Some(out)
        }
        Spec::Shield { banner, target, result } => {
            if n != 2 {
                return None;
            }
            let (mut b, mut t) = (None, None);
            for s in stacks {
                if has(items, banner, s) && banner_color(&s.id).is_some() {
                    if b.is_some() {
                        return None;
                    }
                    b = Some(*s);
                } else if has(items, target, s) && t.is_none() && patterns(s) == 0 {
                    t = Some(*s);
                } else {
                    return None;
                }
            }
            let (b, t) = (b?, t?);
            let mut out = with_original(result, t, 0);
            match component(b, "minecraft:banner_patterns") {
                Some(p) => set_component(&mut out, "minecraft:banner_patterns", p.clone()),
                None => crate::items::remove_component(&mut out, "minecraft:banner_patterns"),
            }
            set_component(&mut out, "minecraft:base_color", Nbt::String(banner_color(&b.id)?.to_owned()));
            Some(out)
        }
        Spec::BannerDuplicate { banner, result } => {
            if n != 2 {
                return None;
            }
            let mut color: Option<&str> = None;
            let (mut src, mut tgt) = (None, false);
            for s in stacks {
                if !has(items, banner, s) {
                    return None;
                }
                let c = banner_color(&s.id)?;
                match color {
                    None => color = Some(c),
                    Some(x) if x != c => return None,
                    _ => {}
                }
                let p = patterns(s);
                if p > 6 {
                    return None;
                }
                if p > 0 {
                    if src.is_some() {
                        return None;
                    }
                    src = Some(*s);
                } else {
                    if tgt {
                        return None;
                    }
                    tgt = true;
                }
            }
            let s = src.filter(|_| tgt)?;
            Some(with_original(result, s, 0))
        }
        Spec::Pot { sides, result } => {
            if tw != 3 || th != 3 || n != 4 {
                return None;
            }
            let at = |x: usize, y: usize| input[x + y * 3];
            let cells = [at(1, 0), at(0, 1), at(2, 1), at(1, 2)];
            let mut deco = Vec::new();
            for (k, name) in ["back", "left", "right", "front"].iter().enumerate() {
                let s = cells[k]?;
                if !has(items, &sides[k], s) {
                    return None;
                }
                deco.push(((*name).to_owned(), Nbt::Compound(vec![("id".into(), Nbt::String(s.id.clone()))])));
            }
            let mut out = result.stack();
            set_component(&mut out, "minecraft:pot_decorations", Nbt::Compound(deco));
            Some(out)
        }
        // S8: map in the centre, paper around; a map unknown to the world or already at
        // maximum scale (4) gives no result.
        Spec::MapExtend { map, material } => {
            if tw != 3 || th != 3 || n != 9 {
                return None;
            }
            let mut src = None;
            for (i, c) in input.iter().enumerate() {
                let s = (*c)?;
                if i == 4 {
                    let id = component(s, "minecraft:map_id").and_then(Nbt::as_i64)?;
                    if !has(items, map, s) || map_scale(id as i32).is_none_or(|scale| scale >= 4) {
                        return None;
                    }
                    src = Some(s);
                } else if !has(items, material, s) {
                    return None;
                }
            }
            let mut out = Stack { count: 1, ..src?.clone() };
            set_component(&mut out, "minecraft:map_post_processing", Nbt::Int(1));
            Some(out)
        }
    }
}

/// Particular leftovers: the copied book and the copied banner stay in the grid.
pub fn remaining(items: &Items, spec: &Spec, grid: &[Option<Stack>]) -> Option<Vec<Option<Stack>>> {
    match spec {
        Spec::BookCloning { .. } => {
            let mut out = vec![None; grid.len()];
            for (i, s) in grid.iter().enumerate() {
                let Some(s) = s else { continue };
                if let Some(r) = items.remainder(&s.id) {
                    out[i] = Some(r);
                } else if component(s, "minecraft:written_book_content").is_some() {
                    out[i] = Some(Stack { count: 1, ..s.clone() });
                    break;
                }
            }
            Some(out)
        }
        Spec::BannerDuplicate { .. } => Some(
            grid.iter()
                .map(|s| {
                    let s = s.as_ref()?;
                    items.remainder(&s.id).or_else(|| (patterns(s) > 0).then(|| Stack { count: 1, ..s.clone() }))
                })
                .collect(),
        ),
        _ => None,
    }
}
