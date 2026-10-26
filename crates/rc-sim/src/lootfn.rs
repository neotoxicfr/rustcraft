//! Loot functions of structure chests (S7, `mechanics-s7.md` § 5.2): random or
//! level-based enchantments, wear, potion, suspicious stew, name, goat horn, ominous bottle,
//! filter. The draws follow the game's order on the context's generator.

use rc_proto::nbt::Nbt;
use serde_json::Value;

use crate::enchanting;
use crate::items::set_component;
use crate::loot::{Ctx, Loot};
use crate::player::Stack;
use crate::randsrc::Rand;

/// Instant effects (the stew's duration is not converted into ticks for them).
pub fn instantaneous(effect: &str) -> bool {
    matches!(effect, "minecraft:instant_health" | "minecraft:instant_damage" | "minecraft:saturation")
}

impl Loot<'_> {
    pub(crate) fn apply_s7(&self, ty: &str, f: &Value, mut s: Stack, ctx: &Ctx<'_>, rng: &mut dyn Rand) -> Stack {
        let items = self.items;
        match ty {
            "enchant_randomly" => {
                let e = enchanting::of(self.pack);
                let book = s.id == "minecraft:book";
                let check = !book && f["only_compatible"].as_bool().unwrap_or(true);
                let opts: Vec<usize> = e
                    .options(self.pack, &f["options"])
                    .into_iter()
                    .filter(|&i| !check || e.supports(items, &e.list[i], &s.id))
                    .collect();
                if opts.is_empty() {
                    return s;
                }
                let pick = opts[rng.next_int_bound(opts.len() as i32) as usize];
                let def = &e.list[pick];
                let level = rng.between(1, def.max_level);
                if book {
                    s = Stack { id: "minecraft:enchanted_book".into(), count: 1, components: None };
                }
                enchanting::upgrade(items, &mut s, &def.id, level);
                s
            }
            "enchant_with_levels" => {
                let e = enchanting::of(self.pack);
                let cost = self.int(&f["levels"], ctx, rng);
                let opts = e.options(self.pack, &f["options"]);
                e.enchant_item(items, rng, s, cost, &opts)
            }
            "set_damage" => {
                let max = items.max_damage(&s);
                if items.damageable(&s) && max > 0 {
                    let base = if f["add"].as_bool().unwrap_or(false) {
                        1.0 - items.damage(&s) as f32 / max as f32
                    } else {
                        0.0
                    };
                    let pct = 1.0 - (self.float(&f["damage"], ctx, rng) + base).clamp(0.0, 1.0);
                    let d = (pct * max as f32).floor() as i32;
                    set_damage(&mut s, d);
                }
                s
            }
            "set_enchantments" => {
                if s.id == "minecraft:book" {
                    s.id = "minecraft:enchanted_book".into();
                }
                let mut list = enchanting::enchants_of(items, &s);
                let add = f["add"].as_bool().unwrap_or(false);
                if let Some(m) = f["enchantments"].as_object() {
                    for (id, lv) in m {
                        let cur = list.iter().find(|(e, _)| e == id).map_or(0, |x| x.1);
                        let v = self.int(lv, ctx, rng);
                        let l = if add { (cur + v).clamp(0, 255) } else { v.clamp(0, 255) };
                        match list.iter_mut().find(|(e, _)| e == id) {
                            Some(x) => x.1 = l,
                            None => list.push((id.clone(), l)),
                        }
                    }
                }
                enchanting::set_enchants(items, &mut s, &list);
                s
            }
            "set_potion" => {
                if let Some(p) = f["id"].as_str() {
                    let mut pc = match crate::items::component(&s, "minecraft:potion_contents") {
                        Some(Nbt::Compound(c)) => Nbt::Compound(c.clone()),
                        _ => Nbt::Compound(Vec::new()),
                    };
                    crate::nbtio::set(&mut pc, "potion", Nbt::String(p.to_owned()));
                    set_component(&mut s, "minecraft:potion_contents", pc);
                }
                s
            }
            "set_stew_effect" => {
                let list = f["effects"].as_array().cloned().unwrap_or_default();
                if s.id == "minecraft:suspicious_stew" && !list.is_empty() {
                    let e = &list[rng.next_int_bound(list.len() as i32) as usize];
                    let id = e["type"].as_str().unwrap_or("").to_owned();
                    let mut d = self.int(&e["duration"], ctx, rng);
                    if !instantaneous(&id) {
                        d *= 20;
                    }
                    let mut cur = match crate::items::component(&s, "minecraft:suspicious_stew_effects") {
                        Some(Nbt::List(l)) => l.clone(),
                        _ => Vec::new(),
                    };
                    cur.push(Nbt::Compound(vec![("id".into(), Nbt::String(id)), ("duration".into(), Nbt::Int(d))]));
                    set_component(&mut s, "minecraft:suspicious_stew_effects", Nbt::List(cur));
                }
                s
            }
            "set_name" => {
                if !f["name"].is_null() {
                    let target = match f["target"].as_str() {
                        Some("item_name") => "minecraft:item_name",
                        _ => "minecraft:custom_name",
                    };
                    set_component(&mut s, target, crate::recipes::json_to_nbt(&f["name"]));
                }
                s
            }
            "set_instrument" => {
                let opts: Vec<String> = match &f["options"] {
                    Value::String(t) if t.starts_with('#') => self
                        .pack
                        .get("tags/instrument", t.trim_start_matches('#'))
                        .and_then(|v| v["values"].as_array().cloned())
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|x| x.as_str().map(str::to_owned))
                        .collect(),
                    Value::String(id) => vec![id.clone()],
                    Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect(),
                    _ => Vec::new(),
                };
                if !opts.is_empty() {
                    let k = rng.next_int_bound(opts.len() as i32) as usize;
                    set_component(&mut s, "minecraft:instrument", Nbt::String(opts[k].clone()));
                }
                s
            }
            "set_ominous_bottle_amplifier" => {
                let a = self.int(&f["amplifier"], ctx, rng).clamp(0, 4);
                set_component(&mut s, "minecraft:ominous_bottle_amplifier", Nbt::Int(a));
                s
            }
            "filtered" => {
                let pass = self.item_pred(&f["item_filter"], Some(&s));
                let g = if pass { &f["on_pass"] } else { &f["on_fail"] };
                if g.is_null() { s } else { self.apply(g, s, ctx, rng) }
            }
            // S8: exploration map, marked here then resolved by the server (search for the
            // structure, map created) where the loot is placed (`maps::resolve_exploration`).
            "exploration_map" => {
                mark_exploration(&mut s, f);
                s
            }
            _ => s,
        }
    }
}

/// S8: `exploration_map` parameters kept in `custom_data` (`rustcraft:explore`).
pub fn mark_exploration(s: &mut Stack, f: &Value) {
    let mut p = vec![
        ("destination".to_owned(), Nbt::String(f["destination"].as_str().unwrap_or("#minecraft:on_treasure_maps").to_owned())),
        ("decoration".to_owned(), Nbt::String(f["decoration"].as_str().unwrap_or("minecraft:mansion").to_owned())),
        ("zoom".to_owned(), Nbt::Int(f["zoom"].as_i64().unwrap_or(2) as i32)),
        ("search_radius".to_owned(), Nbt::Int(f["search_radius"].as_i64().unwrap_or(50) as i32)),
    ];
    p.push(("skip_existing_chunks".to_owned(), Nbt::Byte(i8::from(f["skip_existing_chunks"].as_bool().unwrap_or(true)))));
    let mut cd = match crate::items::component(s, "minecraft:custom_data") {
        Some(Nbt::Compound(c)) => c.clone(),
        _ => Vec::new(),
    };
    cd.retain(|(k, _)| k != "rustcraft:explore");
    cd.push(("rustcraft:explore".to_owned(), Nbt::Compound(p)));
    set_component(s, "minecraft:custom_data", Nbt::Compound(cd));
}

/// Wear set (`damage` component, removed at 0 like the default value).
pub fn set_damage(s: &mut Stack, d: i32) {
    if d <= 0 {
        crate::items::remove_component(s, "minecraft:damage");
    } else {
        set_component(s, "minecraft:damage", Nbt::Int(d));
    }
}
