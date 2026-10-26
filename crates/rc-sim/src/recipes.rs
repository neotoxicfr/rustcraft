//! Recipes (`server-gameplay.md` § 4.3-4.4) read from the jar's base pack: crafting (shaped,
//! shapeless, transmutation, dyeing, imbuing, repair), cooking (furnace,
//! blast furnace, smoker, campfire), stonecutting. Order: sorted identifiers.

use rc_proto::nbt::Nbt;
use serde_json::Value;

use crate::datapack::DataPack;
use crate::items::{Items, same};
use crate::player::Stack;

/// Ingredient: accepted item identifiers (sorted).
pub type Ingredient = Vec<i32>;

#[derive(Debug, Clone)]
pub struct Template {
    pub id: String,
    pub count: i32,
    pub components: Option<Nbt>,
}

impl Template {
    pub fn stack(&self) -> Stack {
        Stack { id: self.id.clone(), count: self.count, components: self.components.clone() }
    }
}

#[derive(Debug, Clone)]
pub enum Craft {
    Shaped { w: usize, h: usize, cells: Vec<Option<Ingredient>>, symmetric: bool, result: Template },
    Shapeless { ings: Vec<Ingredient>, result: Template },
    Transmute { input: Ingredient, material: Ingredient, min: i32, max: i32, result: Template, add_count: bool },
    Dye { target: Ingredient, dye: Ingredient, result: Template },
    Imbue { source: Ingredient, material: Ingredient, result: Template },
    Repair,
    /// S7: special recipes (`special.rs`).
    Special(Box<crate::special::Spec>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CookKind {
    Smelting,
    Blasting,
    Smoking,
    Campfire,
}

#[derive(Debug, Clone)]
pub struct Cooking {
    pub kind: CookKind,
    pub ingredient: Ingredient,
    pub result: Template,
    pub xp: f32,
    pub time: i32,
}

#[derive(Debug, Clone)]
pub enum Body {
    Craft(Craft),
    Cook(Cooking),
    Stonecut {
        input: Ingredient,
        result: Template,
    },
    /// Recipe recognised but not simulated (smithing, hard-coded special recipes).
    Other(String),
}

#[derive(Debug, Clone)]
pub struct Recipe {
    pub id: String,
    pub body: Body,
    pub group: Option<String>,
    pub category: Option<String>,
    pub notify: bool,
}

pub struct Recipes {
    pub list: Vec<Recipe>,
    /// Recipe book: first display number of each recipe (a recipe shows
    /// zero, one or several: transmutation, one per number of materials), plus the total.
    display_start: Vec<u32>,
    group_ids: Vec<Option<i32>>,
    /// Rank of a recipe by identifier.
    by_id: hashbrown::HashMap<String, usize>,
}

/// Scale of a known map of the world (`None`: unknown map).
pub type MapScale<'a> = &'a dyn Fn(i32) -> Option<u8>;

/// Number of displays of a recipe in the book: none for special recipes.
fn display_count(b: &Body) -> u32 {
    match b {
        Body::Craft(Craft::Transmute { min, max, .. }) => (max - min + 1).max(0) as u32,
        Body::Craft(Craft::Repair | Craft::Special(_)) => 0,
        Body::Craft(_) | Body::Cook(_) | Body::Stonecut { .. } => 1,
        Body::Other(t) => u32::from(t == "smithing_transform" || t == "smithing_trim"),
    }
}

/// JSON (result components) → approximate save NBT: integers as `int`,
/// decimals as `float`, booleans as byte.
pub fn json_to_nbt(v: &Value) -> Nbt {
    match v {
        Value::Null => Nbt::Compound(Vec::new()),
        Value::Bool(b) => Nbt::Byte(i8::from(*b)),
        Value::Number(n) => match n.as_i64() {
            Some(i) if i32::try_from(i).is_ok() => Nbt::Int(i as i32),
            Some(i) => Nbt::Long(i),
            None => Nbt::Float(n.as_f64().unwrap_or(0.0) as f32),
        },
        Value::String(s) => Nbt::String(s.clone()),
        Value::Array(a) => Nbt::List(a.iter().map(json_to_nbt).collect()),
        Value::Object(o) => Nbt::Compound(o.iter().map(|(k, v)| (k.clone(), json_to_nbt(v))).collect()),
    }
}

/// JSON ingredient (`"minecraft:x"`, `"#tag"` or list) → identifiers.
pub fn ingredient(items: &Items, v: &Value) -> Option<Ingredient> {
    let mut out = Vec::new();
    let mut add = |s: &str| {
        if let Some(tag) = s.strip_prefix('#') {
            if let Some(ids) = items.tags.get(tag) {
                out.extend_from_slice(ids);
            }
        } else if let Some(id) = items.id(s) {
            out.push(id);
        }
    };
    match v {
        Value::String(s) => add(s),
        Value::Array(a) => a.iter().filter_map(Value::as_str).for_each(&mut add),
        _ => return None,
    }
    out.sort_unstable();
    out.dedup();
    (!out.is_empty()).then_some(out)
}

fn template(v: &Value) -> Option<Template> {
    let id = v["id"].as_str()?.to_owned();
    let components = v.get("components").filter(|c| c.is_object()).map(json_to_nbt);
    Some(Template { id, count: v["count"].as_i64().unwrap_or(1) as i32, components })
}

fn test(items: &Items, ing: &Ingredient, s: &Stack) -> bool {
    items.id(&s.id).is_some_and(|id| ing.binary_search(&id).is_ok())
}

/// Trimmed crafting grid: width, height, left, top.
pub fn trim(grid: &[Option<Stack>], w: usize) -> Option<(usize, usize, usize, usize)> {
    let h = grid.len() / w.max(1);
    let filled = |x: usize, y: usize| grid[x + y * w].is_some();
    let xs: Vec<usize> = (0..w).filter(|&x| (0..h).any(|y| filled(x, y))).collect();
    let ys: Vec<usize> = (0..h).filter(|&y| (0..w).any(|x| filled(x, y))).collect();
    let (x0, x1) = (*xs.first()?, *xs.last()?);
    let (y0, y1) = (*ys.first()?, *ys.last()?);
    Some((x1 - x0 + 1, y1 - y0 + 1, x0, y0))
}

/// Bipartite matching: each ingredient has its own stack.
fn bipartite(items: &Items, ings: &[Ingredient], stacks: &[&Stack]) -> bool {
    fn assign(
        items: &Items,
        ings: &[Ingredient],
        stacks: &[&Stack],
        i: usize,
        seen: &mut [bool],
        owner: &mut [Option<usize>],
    ) -> bool {
        for (s, st) in stacks.iter().enumerate() {
            if seen[s] || !test(items, &ings[i], st) {
                continue;
            }
            seen[s] = true;
            if owner[s].is_none_or(|o| assign(items, ings, stacks, o, seen, owner)) {
                owner[s] = Some(i);
                return true;
            }
        }
        false
    }
    let mut owner = vec![None; stacks.len()];
    (0..ings.len()).all(|i| {
        let mut seen = vec![false; stacks.len()];
        assign(items, ings, stacks, i, &mut seen, &mut owner)
    })
}

impl Recipes {
    pub fn load(pack: &DataPack, items: &Items) -> Recipes {
        let mut list = Vec::new();
        for (id, v) in pack.all("recipe") {
            let ty = v["type"].as_str().unwrap_or("").trim_start_matches("minecraft:").to_owned();
            let ing = |k: &str| ingredient(items, &v[k]);
            let res = || template(&v["result"]);
            let body = match ty.as_str() {
                "crafting_shaped" => (|| {
                    let rows: Vec<&str> = v["pattern"].as_array()?.iter().filter_map(Value::as_str).collect();
                    let key = v["key"].as_object()?;
                    // Pattern trimmed of its empty rows and columns.
                    let h0 = rows.len();
                    let w0 = rows.iter().map(|r| r.chars().count()).max().unwrap_or(0);
                    let cell = |x: usize, y: usize| rows[y].chars().nth(x).unwrap_or(' ');
                    let ys: Vec<usize> = (0..h0).filter(|&y| (0..w0).any(|x| cell(x, y) != ' ')).collect();
                    let xs: Vec<usize> = (0..w0).filter(|&x| (0..h0).any(|y| cell(x, y) != ' ')).collect();
                    let (y0, y1, x0, x1) = (*ys.first()?, *ys.last()?, *xs.first()?, *xs.last()?);
                    let (w, h) = (x1 - x0 + 1, y1 - y0 + 1);
                    let mut cells = Vec::with_capacity(w * h);
                    for y in y0..=y1 {
                        for x in x0..=x1 {
                            let c = cell(x, y);
                            cells.push(if c == ' ' { None } else { Some(ingredient(items, &key[&c.to_string()])?) });
                        }
                    }
                    let symmetric = (0..h).all(|y| (0..w / 2).all(|x| cells[x + y * w] == cells[w - 1 - x + y * w]));
                    Some(Body::Craft(Craft::Shaped { w, h, cells, symmetric, result: res()? }))
                })(),
                "crafting_shapeless" => (|| {
                    let ings =
                        v["ingredients"].as_array()?.iter().map(|i| ingredient(items, i)).collect::<Option<_>>()?;
                    Some(Body::Craft(Craft::Shapeless { ings, result: res()? }))
                })(),
                "crafting_transmute" => (|| {
                    let (min, max) = match &v["material_count"] {
                        Value::Number(n) => (n.as_i64()? as i32, n.as_i64()? as i32),
                        Value::Object(o) => (
                            o.get("min").and_then(Value::as_i64).unwrap_or(1) as i32,
                            o.get("max").and_then(Value::as_i64).unwrap_or(8) as i32,
                        ),
                        _ => (1, 1),
                    };
                    let result = Template {
                        id: v["result"]["id"].as_str().unwrap_or("").to_owned(),
                        count: v["result"]["count"].as_i64().unwrap_or(1) as i32,
                        components: v["result"].get("components").filter(|c| c.is_object()).map(json_to_nbt),
                    };
                    Some(Body::Craft(Craft::Transmute {
                        input: ing("input")?,
                        material: ing("material")?,
                        min,
                        max,
                        result,
                        add_count: v["add_material_count_to_result"].as_bool().unwrap_or(false),
                    }))
                })(),
                "crafting_dye" => {
                    (|| Some(Body::Craft(Craft::Dye { target: ing("target")?, dye: ing("dye")?, result: res()? })))()
                }
                "crafting_imbue" => (|| {
                    Some(Body::Craft(Craft::Imbue {
                        source: ing("source")?,
                        material: ing("material")?,
                        result: res()?,
                    }))
                })(),
                "crafting_special_repairitem" => Some(Body::Craft(Craft::Repair)),
                "smelting" | "blasting" | "smoking" | "campfire_cooking" => (|| {
                    let kind = match ty.as_str() {
                        "smelting" => CookKind::Smelting,
                        "blasting" => CookKind::Blasting,
                        "smoking" => CookKind::Smoking,
                        _ => CookKind::Campfire,
                    };
                    let default_time = match kind {
                        CookKind::Smelting => 200,
                        CookKind::Campfire => 100,
                        _ => 100,
                    };
                    Some(Body::Cook(Cooking {
                        kind,
                        ingredient: ing("ingredient")?,
                        result: res()?,
                        xp: v["experience"].as_f64().unwrap_or(0.0) as f32,
                        time: v["cookingtime"].as_i64().unwrap_or(default_time) as i32,
                    }))
                })(),
                "stonecutting" => (|| Some(Body::Stonecut { input: ing("ingredient")?, result: res()? }))(),
                t if t.starts_with("crafting_special_") || t == "crafting_decorated_pot" => match crate::special::parse(items, t, &v) {
                    Some(s) => Some(Body::Craft(Craft::Special(Box::new(s)))),
                    None => Some(Body::Other(ty.clone())),
                },
                _ => Some(Body::Other(ty.clone())),
            };
            let Some(body) = body else { continue };
            list.push(Recipe {
                id,
                body,
                group: v["group"].as_str().filter(|g| !g.is_empty()).map(str::to_owned),
                category: v["category"].as_str().map(str::to_owned),
                notify: v["show_notification"].as_bool().unwrap_or(true),
            });
        }
        let mut display_start = Vec::with_capacity(list.len() + 1);
        let mut n = 0u32;
        for r in &list {
            display_start.push(n);
            n += display_count(&r.body);
        }
        display_start.push(n);
        // Book groups numbered once (stonecutting and smithing have none).
        let grouped = |r: &Recipe| match r.body {
            Body::Stonecut { .. } | Body::Other(_) => None,
            _ => r.group.clone(),
        };
        let mut names: Vec<String> = list.iter().filter_map(grouped).collect();
        names.sort_unstable();
        names.dedup();
        let group_ids =
            list.iter().map(|r| grouped(r).and_then(|g| names.binary_search(&g).ok()).map(|i| i as i32)).collect();
        // First rank of each identifier (like the earlier linear search).
        let mut by_id = hashbrown::HashMap::with_capacity(list.len());
        for (i, r) in list.iter().enumerate() {
            by_id.entry(r.id.clone()).or_insert(i);
        }
        Recipes { list, display_start, group_ids, by_id }
    }

    /// Book group number of recipe `i`.
    pub fn group_id(&self, i: usize) -> Option<i32> {
        self.group_ids.get(i).copied().flatten()
    }

    pub fn index(&self, id: &str) -> Option<usize> {
        self.by_id.get(id).copied()
    }

    /// Book display numbers of recipe `i`.
    pub fn displays(&self, i: usize) -> std::ops::Range<i32> {
        match (self.display_start.get(i), self.display_start.get(i + 1)) {
            (Some(&a), Some(&b)) => a as i32..b as i32,
            _ => 0..0,
        }
    }

    /// Recipe of a book display number, and the rank of the display within the recipe.
    pub fn of_display(&self, d: i32) -> Option<(usize, usize)> {
        let d = u32::try_from(d).ok()?;
        if d >= *self.display_start.last()? {
            return None;
        }
        // Last recipe whose first number is ≤ d (recipes without a display have the
        // same first number as the next one: we take the one that has one).
        let i = self.display_start.partition_point(|&s| s <= d) - 1;
        Some((i, (d - self.display_start[i]) as usize))
    }

    /// Crafting recipe matching the grid (`w`×`w`), and its result; `maps`:
    /// scale of the known maps of the world.
    pub fn craft(&self, items: &Items, grid: &[Option<Stack>], w: usize, maps: MapScale<'_>) -> Option<(usize, Stack)> {
        let (tw, th, x0, y0) = trim(grid, w)?;
        let input: Vec<Option<&Stack>> = (0..th)
            .flat_map(|y| (0..tw).map(move |x| (x, y)))
            .map(|(x, y)| grid[x0 + x + (y0 + y) * w].as_ref())
            .collect();
        let stacks: Vec<&Stack> = input.iter().flatten().copied().collect();
        for (i, r) in self.list.iter().enumerate() {
            let Body::Craft(c) = &r.body else { continue };
            if let Some(out) = Self::assemble(items, c, &input, tw, th, &stacks, maps) {
                return Some((i, out));
            }
        }
        None
    }

    fn assemble(
        items: &Items,
        c: &Craft,
        input: &[Option<&Stack>],
        tw: usize,
        th: usize,
        stacks: &[&Stack],
        maps: MapScale<'_>,
    ) -> Option<Stack> {
        match c {
            Craft::Special(s) => crate::special::assemble(items, s, input, tw, th, stacks, maps),
            Craft::Shaped { w, h, cells, symmetric, result } => {
                if *w != tw || *h != th || cells.iter().flatten().count() != stacks.len() {
                    return None;
                }
                let test_at = |flip: bool| {
                    (0..*h).all(|y| {
                        (0..*w).all(|x| {
                            let e = if flip { &cells[w - x - 1 + y * w] } else { &cells[x + y * w] };
                            match (e, input[x + y * w]) {
                                (None, None) => true,
                                (Some(i), Some(s)) => test(items, i, s),
                                _ => false,
                            }
                        })
                    })
                };
                ((!symmetric && test_at(true)) || test_at(false)).then(|| result.stack())
            }
            Craft::Shapeless { ings, result } => {
                if ings.len() != stacks.len() {
                    return None;
                }
                let ok =
                    if stacks.len() == 1 { test(items, &ings[0], stacks[0]) } else { bipartite(items, ings, stacks) };
                ok.then(|| result.stack())
            }
            Craft::Transmute { input: inp, material, min, max, result, add_count } => {
                let n = stacks.len() as i32;
                if n < min + 1 || n > max + 1 {
                    return None;
                }
                let mut found: Option<&Stack> = None;
                let mut mats = 0;
                for s in stacks {
                    if test(items, inp, s) {
                        if found.is_some() {
                            return None;
                        }
                        found = Some(s);
                    } else if test(items, material, s) {
                        mats += 1;
                        if mats > *max {
                            return None;
                        }
                    } else {
                        return None;
                    }
                }
                let found = found?;
                if mats < *min || mats > *max {
                    return None;
                }
                let count = if *add_count { mats + result.count } else { result.count };
                let id = if result.id.is_empty() { found.id.clone() } else { result.id.clone() };
                let mut out = Stack { id, count, components: found.components.clone() };
                if let Some(Nbt::Compound(f)) = &result.components {
                    for (k, v) in f {
                        crate::items::set_component(&mut out, k, v.clone());
                    }
                }
                if count == 1 && same(&out, found) {
                    return None;
                }
                Some(out)
            }
            Craft::Dye { target, dye, result } => {
                if stacks.len() < 2 {
                    return None;
                }
                let mut tgt: Option<&Stack> = None;
                let mut colors = Vec::new();
                for s in stacks {
                    if test(items, target, s) && tgt.is_none() {
                        tgt = Some(s);
                    } else if test(items, dye, s) {
                        let c = items.dye(s)?;
                        colors.push(dye_rgb(&c)?);
                    } else {
                        return None;
                    }
                }
                let tgt = tgt?;
                let current =
                    crate::items::component(tgt, "minecraft:dyed_color").and_then(Nbt::as_i64).map(|v| v as i32);
                let rgb = mix_colors(current, &colors);
                let mut out = Stack { id: result.id.clone(), count: result.count, components: tgt.components.clone() };
                crate::items::set_component(&mut out, "minecraft:dyed_color", Nbt::Int(rgb));
                Some(out)
            }
            Craft::Imbue { source, material, result } => {
                if tw != 3 || th != 3 || input.iter().any(Option::is_none) {
                    return None;
                }
                let centre = input[4]?;
                if !test(items, source, centre) {
                    return None;
                }
                if (0..9).filter(|&i| i != 4).any(|i| !input[i].is_some_and(|s| test(items, material, s))) {
                    return None;
                }
                let mut out = result.stack();
                if let Some(p) = crate::items::component(centre, "minecraft:potion_contents") {
                    crate::items::set_component(&mut out, "minecraft:potion_contents", p.clone());
                }
                Some(out)
            }
            Craft::Repair => {
                if stacks.len() != 2 {
                    return None;
                }
                let (a, b) = (stacks[0], stacks[1]);
                // Two copies of the same item, alone, each carrying a durability and a
                // wear (components present, even at their default value).
                let has = |s: &Stack, c: &str| {
                    crate::items::component(s, c).is_some() || items.default_component(s, c).is_some()
                };
                let ok = a.id == b.id
                    && a.count == 1
                    && b.count == 1
                    && has(a, "minecraft:max_damage")
                    && has(b, "minecraft:max_damage")
                    && has(a, "minecraft:damage")
                    && has(b, "minecraft:damage");
                if !ok {
                    return None;
                }
                let max = items.max_damage(a).max(items.max_damage(b));
                let rest = |s: &Stack| items.max_damage(s) - items.damage(s);
                let remaining = rest(a) + rest(b) + max * 5 / 100;
                let damage = (max - remaining).max(0);
                let mut out = Stack { id: a.id.clone(), count: 1, components: None };
                // Durability set on the result: only appears if it differs from that of
                // the new item (a value equal to the default is not kept).
                if max != items.max_damage(&out) {
                    crate::items::set_component(&mut out, "minecraft:max_damage", Nbt::Int(max));
                }
                if damage > 0 {
                    crate::items::set_component(&mut out, "minecraft:damage", Nbt::Int(damage));
                }
                // Only curses are kept, at the highest level of the two.
                let mut curses: Vec<(String, Nbt)> = Vec::new();
                for s in [a, b] {
                    let Some(Nbt::Compound(f)) = crate::items::component(s, "minecraft:enchantments") else { continue };
                    for (k, v) in f {
                        if !k.ends_with("_curse") {
                            continue;
                        }
                        let lvl = v.as_i64().unwrap_or(0);
                        match curses.iter_mut().find(|c| &c.0 == k) {
                            Some(c) => {
                                if lvl > c.1.as_i64().unwrap_or(0) {
                                    c.1 = Nbt::Int(lvl as i32);
                                }
                            }
                            None => curses.push((k.clone(), Nbt::Int(lvl as i32))),
                        }
                    }
                }
                if !curses.is_empty() {
                    crate::items::set_component(&mut out, "minecraft:enchantments", Nbt::Compound(curses));
                }
                Some(out)
            }
        }
    }

    /// Remainders per grid slot (crafting remainder of each item).
    pub fn remaining(&self, items: &Items, grid: &[Option<Stack>], maps: MapScale<'_>) -> Vec<Option<Stack>> {
        // S7: remainders specific to special recipes (copied book, copied banner).
        let w = (grid.len() as f64).sqrt() as usize;
        if let Some((i, _)) = self.craft(items, grid, w, maps)
            && let Body::Craft(Craft::Special(s)) = &self.list[i].body
            && let Some(r) = crate::special::remaining(items, s, grid)
        {
            return r;
        }
        grid.iter().map(|s| s.as_ref().and_then(|s| items.remainder(&s.id))).collect()
    }

    /// Cooking recipe of the type for this item.
    pub fn cooking(&self, items: &Items, kind: CookKind, s: &Stack) -> Option<(usize, &Cooking)> {
        self.list.iter().enumerate().find_map(|(i, r)| match &r.body {
            Body::Cook(c) if c.kind == kind && test(items, &c.ingredient, s) => Some((i, c)),
            _ => None,
        })
    }
}

/// Diffuse colour of a dye (game values, the dye colour).
pub fn dye_rgb(name: &str) -> Option<i32> {
    Some(match name {
        "white" => 0xF9FFFE,
        "orange" => 0xF9801D,
        "magenta" => 0xC74EBD,
        "light_blue" => 0x3AB3DA,
        "yellow" => 0xFED83D,
        "lime" => 0x80C71F,
        "pink" => 0xF38BAA,
        "gray" => 0x474F52,
        "light_gray" => 0x9D9D97,
        "cyan" => 0x169C9C,
        "purple" => 0x8932B8,
        "blue" => 0x3C44AA,
        "brown" => 0x835432,
        "green" => 0x5E7C16,
        "red" => 0xB02E26,
        "black" => 0x1D1D21,
        _ => return None,
    })
}

/// Dye mixing (§ 4.3, `crafting_dye`).
pub fn mix_colors(current: Option<i32>, dyes: &[i32]) -> i32 {
    let (mut r, mut g, mut b, mut max_sum, mut n) = (0, 0, 0, 0, 0);
    for c in current.into_iter().chain(dyes.iter().copied()) {
        let (cr, cg, cb) = ((c >> 16) & 255, (c >> 8) & 255, c & 255);
        max_sum += cr.max(cg).max(cb);
        r += cr;
        g += cg;
        b += cb;
        n += 1;
    }
    if n == 0 {
        return 0;
    }
    let (ar, ag, ab) = (r / n, g / n, b / n);
    let avg_max = max_sum as f32 / n as f32;
    let max = ar.max(ag).max(ab) as f32;
    let scale = |v: i32| if max == 0.0 { v } else { (v as f32 * avg_max / max) as i32 };
    (scale(ar) << 16) | (scale(ag) << 8) | scale(ab)
}
