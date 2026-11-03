//! Loot functions of structure chests (S7, `mechanics-s7.md` § 5.2): random or
//! level-based enchantments, wear, potion, suspicious stew, name, goat horn, ominous bottle,
//! filter. The draws follow the game's order on the context's generator.

use rc_proto::nbt::Nbt;
use serde_json::Value;

use crate::commands::args;
use crate::enchanting;
use crate::items::{Items, set_component};
use crate::loot::{Ctx, Loot};
use crate::player::Stack;
use crate::randsrc::Rand;

const CUSTOM_DATA: &str = "minecraft:custom_data";
const CUSTOM_NAME: &str = "minecraft:custom_name";
const WRITTEN: &str = "minecraft:written_book_content";

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
                    put_potion(&mut s, p);
                }
                s
            }
            // A potion drawn among the options (`#tag`, id or list), every potion without them.
            "set_random_potion" if crate::loot::loot_context_borrowed() => {
                if let Some(p) = self.draw_random_potion(&f["options"], rng) {
                    put_potion(&mut s, p);
                }
                s
            }
            // Old reading (`RUSTCRAFT_OPT_LOOT_BORROWED_CONTEXT=0`): the options copied into a list.
            "set_random_potion" => {
                let all = || items.data.registry("minecraft:potion").map(|r| r.entries.clone()).unwrap_or_default();
                let list: Vec<String> = match &f["options"] {
                    Value::Null => all(),
                    Value::String(t) => match t.strip_prefix('#') {
                        Some(tag) => {
                            let names = all();
                            let tag = args::id(tag);
                            items
                                .potion_tags
                                .iter()
                                .find(|(n, _)| args::id(n) == tag)
                                .map(|(_, ids)| ids.iter().filter_map(|&i| names.get(i as usize).cloned()).collect())
                                .unwrap_or_default()
                        }
                        None => vec![args::id(t)],
                    },
                    Value::Array(a) => a.iter().filter_map(Value::as_str).map(args::id).collect(),
                    _ => Vec::new(),
                };
                if !list.is_empty() {
                    let p = &list[rng.next_int_bound(list.len() as i32) as usize];
                    put_potion(&mut s, p);
                }
                s
            }
            // Dyes drawn among the 16 colours and mixed with the current colour; one item left.
            "set_random_dyes" => {
                let rolls = self.int(&f["number_of_dyes"], ctx, rng);
                if rolls > 0 {
                    let dyes: Vec<i32> =
                        (0..rolls).map(|_| crate::mobs::trades::DYE_DIFFUSE[rng.next_int_bound(16) as usize]).collect();
                    let cur = crate::items::effective_component(&s, "minecraft:dyed_color")
                        .and_then(|v| v.get("rgb").or(Some(&v)).and_then(Nbt::as_i64))
                        .map(|v| v as i32);
                    s.count = 1;
                    set_component(&mut s, "minecraft:dyed_color", Nbt::Int(crate::mobs::trades::apply_dyes(cur, &dyes)));
                }
                s
            }
            // Item replaced, quantity and patch kept (read again against the new item's
            // defaults); an empty stack stays empty.
            "set_item" => {
                if let Some(id) = f["item"].as_str()
                    && !empty(&s)
                {
                    s.id = args::id(id);
                    crate::items::normalize_patch(&mut s);
                }
                s
            }
            // Compound merged into `custom_data` (SNBT text or compound); empty result = removed.
            "set_custom_data" => {
                let tag = match &f["tag"] {
                    Value::String(t) => crate::commands::snbt::parse(t).ok(),
                    o @ Value::Object(_) => Some(crate::recipes::json_to_nbt(o)),
                    _ => None,
                };
                if let Some(tag @ Nbt::Compound(_)) = tag {
                    let mut cd = crate::items::component(&s, CUSTOM_DATA).cloned().unwrap_or(Nbt::Compound(Vec::new()));
                    crate::commands::cmds_data::merge(&mut cd, &tag);
                    put_custom_data(&mut s, cd);
                }
                s
            }
            "set_attributes" => {
                let replace = f["replace"].as_bool().unwrap_or(true);
                let mut list = if replace { Vec::new() } else { attribute_entries(items, &s) };
                for m in f["modifiers"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
                    let slots: Vec<&str> = match &m["slot"] {
                        Value::String(x) => vec![x.as_str()],
                        Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
                        _ => Vec::new(),
                    };
                    if slots.is_empty() {
                        continue;
                    }
                    // Slot drawn first, then the amount.
                    let slot = slots[rng.next_int_bound(slots.len() as i32) as usize];
                    let amount = self.float(&m["amount"], ctx, rng);
                    let ty = args::id(m["attribute"].as_str().unwrap_or(""));
                    let id = args::id(m["id"].as_str().unwrap_or(""));
                    // Same attribute and same identifier: the old entry is replaced.
                    list.retain(|e| {
                        e.get("type").and_then(Nbt::as_str) != Some(ty.as_str())
                            || e.get("id").and_then(Nbt::as_str) != Some(id.as_str())
                    });
                    let mut e = vec![
                        ("type".to_owned(), Nbt::String(ty)),
                        ("id".to_owned(), Nbt::String(id)),
                        ("amount".to_owned(), Nbt::Double(f64::from(amount))),
                        ("operation".to_owned(), Nbt::String(m["operation"].as_str().unwrap_or("").to_owned())),
                    ];
                    if slot != "any" {
                        e.push(("slot".to_owned(), Nbt::String(slot.to_owned())));
                    }
                    list.push(Nbt::Compound(e));
                }
                set_component(&mut s, "minecraft:attribute_modifiers", Nbt::List(list));
                s
            }
            // Name of an entity of the context or of a named block entity; without a name, the
            // custom name goes.
            "copy_name" => {
                let src = f["source"].as_str().unwrap_or("");
                let name = if src == "block_entity" {
                    ctx.block_entity
                        .filter(|be| crate::loot::be::nameable(be.get("id").and_then(Nbt::as_str).unwrap_or("")))
                        .map(|be| be.get("CustomName").cloned())
                } else {
                    ctx.world.and_then(|w| w.entity_name(src))
                };
                match name {
                    Some(Some(n)) => set_component(&mut s, CUSTOM_NAME, n),
                    Some(None) => crate::items::unset_component(&mut s, CUSTOM_NAME),
                    None => {}
                }
                s
            }
            "fill_player_head" => {
                if s.id == "minecraft:player_head"
                    && let Some(p) = ctx.world.and_then(|w| w.entity_profile(f["entity"].as_str().unwrap_or("")))
                {
                    set_component(&mut s, "minecraft:profile", p);
                }
                s
            }
            "copy_custom_data" => {
                if let Some(src) = nbt_source(&f["source"], ctx) {
                    s = copy_custom_data(f, s, &src);
                }
                s
            }
            // Contents made by every entry (whatever its weight), split into maximum stacks.
            "set_contents" => {
                let key = args::id(f["component"].as_str().unwrap_or(""));
                if !empty(&s) && crate::loot::HOLDERS.contains(&key.as_str()) {
                    let mut made = Vec::new();
                    self.create_all(&f["entries"], ctx, rng, &mut |x: Stack, _: &mut dyn Rand| {
                        let max = items.max_stack(&x);
                        if x.count < max {
                            made.push(x);
                        } else {
                            let mut left = x.count;
                            while left > 0 {
                                let n = left.min(max);
                                made.push(Stack { count: n, ..x.clone() });
                                left -= n;
                            }
                        }
                    });
                    s = store_held(items, s, &key, made);
                }
                s
            }
            // Every held stack through the modifier (empty slots untouched), quantity capped.
            "modify_contents" => {
                let key = args::id(f["component"].as_str().unwrap_or(""));
                if !empty(&s)
                    && crate::loot::HOLDERS.contains(&key.as_str())
                    && crate::items::effective_component(&s, &key).is_some()
                {
                    let held = crate::loot::held_stacks(&s, &key);
                    let mut out = Vec::with_capacity(held.len());
                    for h in held {
                        out.push(match h {
                            Some(x) => {
                                let mut y = self.apply(&f["modifier"], x, ctx, rng);
                                y.count = y.count.min(items.max_stack(&y));
                                y
                            }
                            None => Stack { id: "minecraft:air".into(), count: 0, components: None },
                        });
                    }
                    s = store_held(items, s, &key, out);
                }
                s
            }
            // Sealed loot of a container item: table identifier and seed (0 left out).
            "set_loot_table" => {
                if !empty(&s)
                    && let Some(t) = f["loot_table_id"].as_str()
                {
                    let mut c = vec![("loot_table".to_owned(), Nbt::String(args::id(t)))];
                    if let Some(seed) = f["seed"].as_i64().filter(|&x| x != 0) {
                        c.push(("seed".to_owned(), Nbt::Long(seed)));
                    }
                    set_component(&mut s, "minecraft:container_loot", Nbt::Compound(c));
                }
                s
            }
            "set_lore" => {
                let lines = f["lore"].as_array().map(Vec::as_slice).unwrap_or(&[]);
                if lines.len() <= 256 {
                    let old = list_of(&s, "minecraft:lore");
                    let new = list_op(f, old, lines.iter().map(text_nbt).collect(), 256);
                    set_component(&mut s, "minecraft:lore", Nbt::List(new));
                }
                s
            }
            "set_banner_pattern" => {
                let new: Vec<Nbt> = f["patterns"].as_array().map(|a| a.iter().map(banner_layer).collect()).unwrap_or_default();
                let list = if f["append"].as_bool().unwrap_or(false) {
                    let mut old = list_of(&s, "minecraft:banner_patterns");
                    old.extend(new);
                    old
                } else {
                    new
                };
                set_component(&mut s, "minecraft:banner_patterns", Nbt::List(list));
                s
            }
            "set_fireworks" => {
                let old = crate::items::effective_component(&s, "minecraft:fireworks").unwrap_or(Nbt::Compound(Vec::new()));
                let flight = match f["flight_duration"].as_i64() {
                    Some(x) if (0..=255).contains(&x) => x as i32,
                    Some(_) => return s,
                    None => old.get("flight_duration").and_then(Nbt::as_i64).map_or(0, |x| i32::from(x as u8)),
                };
                let mut ex = crate::nbtio::list(old.get("explosions")).to_vec();
                if let Some(op) = f.get("explosions") {
                    let vals = op["values"].as_array().map(Vec::as_slice).unwrap_or(&[]);
                    ex = list_op(op, ex, vals.iter().map(|v| explosion(v, &Nbt::Compound(Vec::new()))).collect(), usize::MAX);
                }
                let mut c = Vec::new();
                if flight != 0 {
                    c.push(("flight_duration".to_owned(), Nbt::Byte(flight as u8 as i8)));
                }
                if !ex.is_empty() {
                    c.push(("explosions".to_owned(), Nbt::List(ex)));
                }
                set_component(&mut s, "minecraft:fireworks", Nbt::Compound(c));
                s
            }
            "set_firework_explosion" => {
                let old = crate::items::effective_component(&s, "minecraft:firework_explosion")
                    .unwrap_or(Nbt::Compound(Vec::new()));
                set_component(&mut s, "minecraft:firework_explosion", explosion(f, &old));
                s
            }
            "set_book_cover" => {
                let title = f.get("title").map(filterable_raw_string);
                if title.as_ref().is_some_and(|t| t.get("raw").and_then(Nbt::as_str).is_some_and(|r| r.encode_utf16().count() > 32))
                    || f["generation"].as_i64().is_some_and(|g| !(0..=3).contains(&g))
                {
                    return s;
                }
                let old = written_book(&s);
                let mut c = vec![
                    (
                        "title".to_owned(),
                        title.or_else(|| old.get("title").cloned()).unwrap_or_else(|| filterable_raw_string(&Value::from(""))),
                    ),
                    (
                        "author".to_owned(),
                        f["author"].as_str().map(|a| Nbt::String(a.to_owned())).or_else(|| old.get("author").cloned()).unwrap_or(Nbt::String(String::new())),
                    ),
                ];
                let generation = f["generation"].as_i64().or_else(|| old.get("generation").and_then(Nbt::as_i64)).unwrap_or(0);
                if generation != 0 {
                    c.push(("generation".to_owned(), Nbt::Int(generation as i32)));
                }
                for k in ["pages", "resolved"] {
                    if let Some(v) = old.get(k) {
                        c.push((k.to_owned(), v.clone()));
                    }
                }
                set_component(&mut s, WRITTEN, Nbt::Compound(c));
                s
            }
            // New pages by list operation; the book must be resolved again.
            "set_written_book_pages" => {
                let mut c = match written_book(&s) {
                    Nbt::Compound(c) => c,
                    _ => Vec::new(),
                };
                let old = c.iter().find(|(k, _)| k == "pages").map_or_else(Vec::new, |(_, v)| crate::nbtio::list(Some(v)).to_vec());
                let pages = f["pages"].as_array().map(Vec::as_slice).unwrap_or(&[]);
                let pages = list_op(f, old, pages.iter().map(filterable_text).collect(), usize::MAX);
                c.retain(|(k, _)| k != "pages" && k != "resolved");
                if !pages.is_empty() {
                    c.push(("pages".to_owned(), Nbt::List(pages)));
                }
                set_component(&mut s, WRITTEN, Nbt::Compound(c));
                s
            }
            "set_writable_book_pages" => {
                let pages = f["pages"].as_array().map(Vec::as_slice).unwrap_or(&[]);
                if pages.len() <= 100 {
                    let old = crate::items::effective_component(&s, "minecraft:writable_book_content");
                    let old = crate::nbtio::list(old.as_ref().and_then(|o| o.get("pages"))).to_vec();
                    let new = list_op(f, old, pages.iter().map(filterable_raw_string).collect(), 100);
                    let c = if new.is_empty() { Vec::new() } else { vec![("pages".to_owned(), Nbt::List(new))] };
                    set_component(&mut s, "minecraft:writable_book_content", Nbt::Compound(c));
                }
                s
            }
            // Components hidden or shown in the tooltip (order of first hiding kept).
            "toggle_tooltips" => {
                let old = crate::items::effective_component(&s, "minecraft:tooltip_display");
                let hide = old.as_ref().and_then(|o| o.get("hide_tooltip")).and_then(Nbt::as_i64).unwrap_or(0) != 0;
                let mut hidden: Vec<String> = crate::nbtio::list(old.as_ref().and_then(|o| o.get("hidden_components")))
                    .iter()
                    .filter_map(|x| x.as_str().map(str::to_owned))
                    .collect();
                for (k, shown) in f["toggles"].as_object().into_iter().flatten() {
                    let (k, hide_it) = (args::id(k), !shown.as_bool().unwrap_or(true));
                    if hidden.contains(&k) == hide_it {
                        continue;
                    }
                    if hide_it {
                        hidden.push(k);
                    } else {
                        hidden.retain(|x| *x != k);
                    }
                }
                let mut c = Vec::new();
                if hide {
                    c.push(("hide_tooltip".to_owned(), Nbt::Byte(1)));
                }
                if !hidden.is_empty() {
                    c.push(("hidden_components".to_owned(), Nbt::List(hidden.into_iter().map(Nbt::String).collect())));
                }
                set_component(&mut s, "minecraft:tooltip_display", Nbt::Compound(c));
                s
            }
            // Four lists by list operation; values read in order floats, flags, strings, colours.
            "set_custom_model_data" => {
                let old = crate::items::effective_component(&s, "minecraft:custom_model_data");
                let part = |k: &str| crate::nbtio::list(old.as_ref().and_then(|o| o.get(k))).to_vec();
                let mut c = Vec::new();
                for (k, op) in [("floats", &f["floats"]), ("flags", &f["flags"]), ("strings", &f["strings"]), ("colors", &f["colors"])] {
                    let cur = part(k);
                    let new = if op.is_object() {
                        let vals = op["values"].as_array().map(Vec::as_slice).unwrap_or(&[]);
                        let vals: Vec<Nbt> = vals
                            .iter()
                            .map(|v| match k {
                                "floats" => Nbt::Float(self.float(v, ctx, rng)),
                                "flags" => Nbt::Byte(i8::from(v.as_bool().unwrap_or(false))),
                                "strings" => Nbt::String(v.as_str().unwrap_or("").to_owned()),
                                _ => Nbt::Int(match v.as_array() {
                                    Some(rgb) => rgb_color(rgb),
                                    None => self.int(v, ctx, rng),
                                }),
                            })
                            .collect();
                        list_op(op, cur, vals, usize::MAX)
                    } else {
                        cur
                    };
                    if !new.is_empty() {
                        c.push((k.to_owned(), Nbt::List(new)));
                    }
                }
                set_component(&mut s, "minecraft:custom_model_data", Nbt::Compound(c));
                s
            }
            "set_stew_effect" => {
                // Old reading (`RUSTCRAFT_OPT_LOOT_BORROWED_CONTEXT=0`): the effects copied first.
                let copied;
                let list: &[Value] = if crate::loot::loot_context_borrowed() {
                    f["effects"].as_array().map_or(&[], Vec::as_slice)
                } else {
                    copied = f["effects"].as_array().cloned().unwrap_or_default();
                    &copied
                };
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
                    set_component(&mut s, target, text_nbt(&f["name"]));
                }
                s
            }
            "set_instrument" => {
                let opts: std::sync::Arc<[String]> = match &f["options"] {
                    Value::String(t) if t.starts_with('#') => self.pack.tag("instrument", t),
                    Value::String(id) => std::sync::Arc::from([id.clone()]),
                    Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect(),
                    _ => std::sync::Arc::from([]),
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

    /// Potion drawn among `options` (`#tag` of potions, one id or a list), among every
    /// registered potion without them; one draw, none when there is nothing to draw from. Read
    /// in place: the registry and the options are not copied.
    fn draw_random_potion<'s>(&'s self, options: &'s Value, rng: &mut dyn Rand) -> Option<&'s str> {
        let names = self.items.data.registry("minecraft:potion").map_or(&[][..], |r| r.entries.as_slice());
        let pick = |count: usize, rng: &mut dyn Rand| (count > 0).then(|| rng.next_int_bound(count as i32) as usize);
        match options {
            Value::Null => pick(names.len(), rng).map(|k| names[k].as_str()),
            Value::String(t) => match t.strip_prefix('#') {
                Some(tag) => {
                    let members = self.items.potion_tags.iter().find(|(n, _)| same_id(n, tag)).map_or(&[][..], |(_, ids)| ids);
                    let known = || members.iter().filter_map(|&i| names.get(i as usize));
                    pick(known().count(), rng).and_then(|k| known().nth(k)).map(String::as_str)
                }
                None => pick(1, rng).map(|_| t.as_str()),
            },
            Value::Array(a) => {
                let listed = || a.iter().filter_map(Value::as_str);
                pick(listed().count(), rng).and_then(|k| listed().nth(k))
            }
            _ => None,
        }
    }
}

/// `a` and `b` are the same identifier once `minecraft:` is put before a bare one (as
/// `args::id` does), compared without building either.
fn same_id(a: &str, b: &str) -> bool {
    fn full(x: &str) -> (&str, &str) {
        if x.contains(':') { ("", x) } else { ("minecraft:", x) }
    }
    let ((na, pa), (nb, pb)) = (full(a), full(b));
    na.len() + pa.len() == nb.len() + pb.len() && na.bytes().chain(pa.bytes()).eq(nb.bytes().chain(pb.bytes()))
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

/// Empty stack (no quantity or air).
fn empty(s: &Stack) -> bool {
    s.count <= 0 || s.id == "minecraft:air"
}

/// Text component of the data (JSON) in the game's stored form: a bare string for unstyled
/// text, children folded as the game reads them; kept as written if it cannot be read.
fn text_nbt(v: &Value) -> Nbt {
    let n = crate::recipes::json_to_nbt(v);
    rc_text::from_nbt(&n).map_or(n, |c| crate::commands::text::to_nbt(&c))
}

/// Filterable text (`raw`, optional `filtered`), or a lone component as its raw form.
fn filterable_text(v: &Value) -> Nbt {
    match v.get("raw") {
        Some(raw) => {
            let mut c = vec![("raw".to_owned(), text_nbt(raw))];
            if let Some(fl) = v.get("filtered") {
                c.push(("filtered".to_owned(), text_nbt(fl)));
            }
            Nbt::Compound(c)
        }
        None => Nbt::Compound(vec![("raw".to_owned(), text_nbt(v))]),
    }
}

/// Filterable string (`raw`, optional `filtered`), or a lone string as its raw form.
fn filterable_raw_string(v: &Value) -> Nbt {
    let s = |x: &Value| Nbt::String(x.as_str().unwrap_or("").to_owned());
    match v.get("raw") {
        Some(raw) => {
            let mut c = vec![("raw".to_owned(), s(raw))];
            if let Some(fl) = v.get("filtered") {
                c.push(("filtered".to_owned(), s(fl)));
            }
            Nbt::Compound(c)
        }
        None => Nbt::Compound(vec![("raw".to_owned(), s(v))]),
    }
}

/// List carried by a component of the stack (patch or default), empty otherwise.
fn list_of(s: &Stack, key: &str) -> Vec<Nbt> {
    match crate::items::effective_component(s, key) {
        Some(Nbt::List(l)) => l,
        _ => Vec::new(),
    }
}

/// List operation of a function (`mode` with its `offset` and `size`) applied to `old` with
/// the new values; a refused operation (offset past the end, list over `max`, invalid field)
/// leaves the list as it was. Replacing everything ignores `max`, like the game.
pub(crate) fn list_op<T: Clone>(op: &Value, old: Vec<T>, new: Vec<T>, max: usize) -> Vec<T> {
    let num = |k: &str| -> Option<Option<usize>> {
        match op.get(k) {
            None => Some(None),
            Some(v) => v.as_u64().and_then(|x| usize::try_from(x).ok()).map(Some),
        }
    };
    let (Some(offset), Some(size)) = (num("offset"), num("size")) else { return old };
    let offset = offset.unwrap_or(0);
    match op["mode"].as_str().unwrap_or("") {
        "replace_all" => new,
        "replace_section" => {
            if offset > old.len() || size.is_some_and(|z| z > max) {
                return old;
            }
            let resume = offset.saturating_add(size.unwrap_or(new.len()));
            let mut out = old[..offset].to_vec();
            out.extend(new);
            if resume < old.len() {
                out.extend_from_slice(&old[resume..]);
            }
            if out.len() > max { old } else { out }
        }
        "insert" => {
            if offset > old.len() || old.len() + new.len() > max {
                return old;
            }
            let mut out = old[..offset].to_vec();
            out.extend(new);
            out.extend_from_slice(&old[offset..]);
            out
        }
        "append" => {
            if old.len() + new.len() > max {
                return old;
            }
            let mut out = old;
            out.extend(new);
            out
        }
        _ => old,
    }
}

/// Potion of `potion_contents` set, its colour, effects and name kept (`potion` first, the
/// game's field order).
fn put_potion(s: &mut Stack, id: &str) {
    let mut c = match crate::items::effective_component(s, "minecraft:potion_contents") {
        Some(Nbt::Compound(c)) => c,
        _ => Vec::new(),
    };
    c.retain(|(k, _)| k != "potion");
    c.insert(0, ("potion".to_owned(), Nbt::String(args::id(id))));
    set_component(s, "minecraft:potion_contents", Nbt::Compound(c));
}

/// `custom_data` written back: an empty compound removes the component.
fn put_custom_data(s: &mut Stack, cd: Nbt) {
    if matches!(&cd, Nbt::Compound(f) if f.is_empty()) {
        crate::items::unset_component(s, CUSTOM_DATA);
    } else {
        set_component(s, CUSTOM_DATA, cd);
    }
}

/// Attribute modifiers of the stack: the patch's, none if it removes them, otherwise the item's
/// default with its amounts read as doubles.
fn attribute_entries(items: &Items, s: &Stack) -> Vec<Nbt> {
    const K: &str = "minecraft:attribute_modifiers";
    const REMOVED: &str = "!minecraft:attribute_modifiers";
    if let Some(Nbt::List(l)) = crate::items::component(s, K) {
        return l.clone();
    }
    // Old reading (`RUSTCRAFT_OPT_LOOT_BORROWED_CONTEXT=0`): the removal key written out at each call.
    let removed = if crate::loot::loot_context_borrowed() {
        crate::items::component(s, REMOVED).is_some()
    } else {
        crate::items::component(s, &format!("!{K}")).is_some()
    };
    if removed {
        return Vec::new();
    }
    let d = items.defaults(&s.id);
    d[K].as_array()
        .map(|a| {
            a.iter()
                .map(|e| {
                    let mut n = crate::recipes::json_to_nbt(e);
                    if let Some(x) = e["amount"].as_f64() {
                        crate::nbtio::set(&mut n, "amount", Nbt::Double(x));
                    }
                    n
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Source of `copy_custom_data`: an entity of the context (inline target or `context`), the
/// block entity, or a command storage (`storage`: empty compound when absent).
fn nbt_source(v: &Value, ctx: &Ctx<'_>) -> Option<Nbt> {
    let (storage, name) = match v {
        Value::String(t) => (false, t.as_str()),
        _ => match v["type"].as_str().map(|t| t.trim_start_matches("minecraft:")) {
            Some("context") => (false, v["target"].as_str()?),
            Some("storage") => (true, v["source"].as_str()?),
            _ => return None,
        },
    };
    if storage {
        return Some(ctx.world.and_then(|w| w.storage(name)).cloned().unwrap_or(Nbt::Compound(Vec::new())));
    }
    match name {
        "block_entity" => ctx.block_entity.cloned(),
        target => ctx.world?.entity_nbt(target),
    }
}

/// `copy_custom_data`: each operation copies what its source path finds in `src` to its target
/// path in a copy of `custom_data` (replace by the last tag found, append to the lists, merge
/// into the compounds; a failing path is skipped); the copy is written back only once an
/// operation found something.
fn copy_custom_data(f: &Value, mut s: Stack, src: &Nbt) -> Stack {
    use crate::commands::cmds_data as data;
    let mut result: Option<Nbt> = None;
    for op in f["ops"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
        let found = data::path_get(src, op["source"].as_str().unwrap_or(""));
        let Some(last) = found.last() else { continue };
        let t = result
            .get_or_insert_with(|| crate::items::component(&s, CUSTOM_DATA).cloned().unwrap_or(Nbt::Compound(Vec::new())));
        let path = op["target"].as_str().unwrap_or("");
        match op["op"].as_str().unwrap_or("") {
            "replace" => {
                let _ = data::path_set(t, path, last.clone());
            }
            "append" => data::path_each_created(t, path, &Nbt::List(Vec::new()), &mut |x| {
                if let Nbt::List(l) = x {
                    l.extend(found.iter().cloned());
                }
            }),
            "merge" => data::path_each_created(t, path, &Nbt::Compound(Vec::new()), &mut |x| {
                if matches!(x, Nbt::Compound(_)) {
                    for v in found.iter().filter(|v| matches!(v, Nbt::Compound(_))) {
                        data::merge(x, v);
                    }
                }
            }),
            _ => {}
        }
    }
    if let Some(r) = result {
        put_custom_data(&mut s, r);
    }
    s
}

/// Writes the stacks a holding component carries: `container` keeps the positions up to the
/// last non-empty stack (more than 256 slots: nothing changes), a bundle takes what its
/// weight allows, a loaded crossbow its non-empty stacks.
fn store_held(items: &Items, mut s: Stack, key: &str, list: Vec<Stack>) -> Stack {
    match key {
        "minecraft:container" => {
            let n = list.iter().rposition(|x| !empty(x)).map_or(0, |i| i + 1);
            if n > 256 {
                return s;
            }
            let slots = list[..n]
                .iter()
                .enumerate()
                .filter(|(_, x)| !empty(x))
                .map(|(i, x)| Nbt::Compound(vec![("slot".to_owned(), Nbt::Int(i as i32)), ("item".to_owned(), x.to_nbt(None))]))
                .collect();
            set_component(&mut s, key, Nbt::List(slots));
        }
        "minecraft:bundle_contents" => {
            let mut bag = crate::menus::bundle::Bag::of(items, Vec::new());
            for mut x in list.into_iter().filter(|x| !empty(x)) {
                bag.try_insert(items, &mut x);
            }
            bag.store(items, &mut s);
        }
        _ => {
            let l = list.iter().filter(|x| !empty(x)).map(|x| x.to_nbt(None)).collect();
            set_component(&mut s, key, Nbt::List(l));
        }
    }
    s
}

/// Banner layer in the stored form: pattern (identifier or inline), colour.
fn banner_layer(v: &Value) -> Nbt {
    let pattern = match &v["pattern"] {
        Value::String(p) => Nbt::String(args::id(p)),
        o => crate::recipes::json_to_nbt(o),
    };
    Nbt::Compound(vec![
        ("pattern".to_owned(), pattern),
        ("color".to_owned(), Nbt::String(v["color"].as_str().unwrap_or("white").to_owned())),
    ])
}

/// Firework explosion: the fields given in `v`, the others from `old` (default: small ball),
/// in the stored form (empty colour lists and false flags left out).
fn explosion(v: &Value, old: &Nbt) -> Nbt {
    let shape = v["shape"]
        .as_str()
        .map(|x| Nbt::String(x.to_owned()))
        .or_else(|| old.get("shape").cloned())
        .unwrap_or_else(|| Nbt::String("small_ball".to_owned()));
    let mut c = vec![("shape".to_owned(), shape)];
    for k in ["colors", "fade_colors"] {
        let l = match v.get(k) {
            Some(a) => Some(Nbt::List(
                a.as_array().into_iter().flatten().filter_map(Value::as_i64).map(|x| Nbt::Int(x as i32)).collect(),
            )),
            None => old.get(k).cloned(),
        };
        let filled = |l: &Nbt| match l {
            Nbt::List(x) => !x.is_empty(),
            Nbt::IntArray(x) => !x.is_empty(),
            _ => false,
        };
        if let Some(l) = l.filter(filled) {
            c.push((k.to_owned(), l));
        }
    }
    for k in ["has_trail", "has_twinkle"] {
        if v[k].as_bool().or_else(|| old.get(k).and_then(Nbt::as_i64).map(|x| x != 0)).unwrap_or(false) {
            c.push((k.to_owned(), Nbt::Byte(1)));
        }
    }
    Nbt::Compound(c)
}

/// Content of a written book (patch or default), otherwise the game's empty book (no title,
/// no author, resolved).
fn written_book(s: &Stack) -> Nbt {
    crate::items::effective_component(s, WRITTEN).unwrap_or_else(|| {
        Nbt::Compound(vec![
            ("title".to_owned(), Nbt::Compound(vec![("raw".to_owned(), Nbt::String(String::new()))])),
            ("author".to_owned(), Nbt::String(String::new())),
            ("resolved".to_owned(), Nbt::Byte(1)),
        ])
    })
}

/// Colour written as three floats (`[r, g, b]` in 0..1): opaque ARGB integer, each channel
/// floored after scaling to 255.
fn rgb_color(rgb: &[Value]) -> i32 {
    let ch = |i: usize| ((rgb.get(i).and_then(Value::as_f64).unwrap_or(0.0) as f32 * 255.0).floor() as i32) & 255;
    (255 << 24) | (ch(0) << 16) | (ch(1) << 8) | ch(2)
}

/// Wear set (`damage` component, floored at 0). Always written, never removed: an item without a
/// default wear keeps `damage:0` and so stays usable; for an item whose default is 0 the patch
/// write drops the entry, as before.
pub fn set_damage(s: &mut Stack, d: i32) {
    set_component(s, "minecraft:damage", Nbt::Int(d.max(0)));
}

#[cfg(test)]
#[path = "lootfn_tests.rs"]
mod tests;
