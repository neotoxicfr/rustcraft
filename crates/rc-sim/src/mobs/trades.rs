//! Merchant offers (F05): trade sets from the data pack (`trade_set`,
//! `villager_trade`), drawn with the game's named random sequence, prices (demand, special
//! difference, reputation), uses and restocking, `Offers` reading and writing,
//! network form (`merchant_offers`).

use rc_proto::nbt::Nbt;
use serde_json::Value;

use crate::items::Items;
use crate::player::Stack;
use crate::rng::Rng;
use crate::server::Server;

/// Cost: item, base quantity, required components (exact predicate).
#[derive(Debug, Clone, PartialEq)]
pub struct Cost {
    pub id: String,
    pub count: i32,
    pub components: Option<Nbt>,
}

impl Cost {
    pub fn stack(&self, count: i32) -> Stack {
        Stack { id: self.id.clone(), count, components: self.components.clone() }
    }

    /// Same item and required components present identically.
    pub fn test(&self, s: Option<&Stack>) -> bool {
        let Some(s) = s else { return false };
        if s.id != self.id {
            return false;
        }
        match &self.components {
            Some(Nbt::Compound(want)) => want
                .iter()
                .all(|(k, v)| crate::items::component(s, k).is_some_and(|have| crate::items::nbt_eq(have, v))),
            _ => true,
        }
    }

    fn to_nbt(&self) -> Nbt {
        let mut f = vec![("id".into(), Nbt::String(self.id.clone())), ("count".into(), Nbt::Int(self.count))];
        if let Some(c) = &self.components {
            f.push(("components".into(), c.clone()));
        }
        Nbt::Compound(f)
    }

    fn from_nbt(v: &Nbt) -> Option<Cost> {
        Some(Cost {
            id: v.get("id")?.as_str()?.to_owned(),
            count: crate::nbtio::int(v, "count").unwrap_or(1) as i32,
            components: v.get("components").cloned(),
        })
    }
}

/// Offer.
#[derive(Debug, Clone, PartialEq)]
pub struct Offer {
    pub cost_a: Cost,
    pub cost_b: Option<Cost>,
    pub result: Stack,
    pub uses: i32,
    pub max_uses: i32,
    pub reward_exp: bool,
    pub special: i32,
    pub demand: i32,
    pub mult: f32,
    pub xp: i32,
}

impl Offer {
    /// Quantity requested for the first cost: base + demand × multiplier + difference, bounded.
    pub fn cost_a_count(&self, items: &Items) -> i32 {
        let base = self.cost_a.count;
        let demand = ((base * self.demand) as f32 * self.mult).floor() as i32;
        let max = items.max_stack(&self.cost_a.stack(1));
        (base + demand.max(0) + self.special).clamp(1, max)
    }

    pub fn out_of_stock(&self) -> bool {
        self.uses >= self.max_uses
    }

    pub fn needs_restock(&self) -> bool {
        self.uses > 0
    }

    pub fn update_demand(&mut self) {
        self.demand = self.demand + self.uses - (self.max_uses - self.uses);
    }

    /// Payment A (adjusted quantity), then B (base quantity) or empty slot.
    pub fn satisfied_by(&self, items: &Items, a: Option<&Stack>, b: Option<&Stack>) -> bool {
        if !self.cost_a.test(a) || a.map_or(0, |s| s.count) < self.cost_a_count(items) {
            return false;
        }
        match &self.cost_b {
            None => b.is_none(),
            Some(c) => c.test(b) && b.map_or(0, |s| s.count) >= c.count,
        }
    }

    /// `take`: removes the payment; false if it is not enough.
    pub fn take(&self, items: &Items, a: &mut Option<Stack>, b: &mut Option<Stack>) -> bool {
        if !self.satisfied_by(items, a.as_ref(), b.as_ref()) {
            return false;
        }
        let n = self.cost_a_count(items);
        shrink(a, n);
        if let Some(c) = &self.cost_b {
            shrink(b, c.count);
        }
        true
    }

    pub fn to_nbt(&self) -> Nbt {
        let mut f = vec![("buy".into(), self.cost_a.to_nbt())];
        if let Some(b) = &self.cost_b {
            f.push(("buyB".into(), b.to_nbt()));
        }
        f.push(("sell".into(), self.result.to_nbt(None)));
        f.push(("uses".into(), Nbt::Int(self.uses)));
        f.push(("maxUses".into(), Nbt::Int(self.max_uses)));
        f.push(("rewardExp".into(), Nbt::Byte(i8::from(self.reward_exp))));
        f.push(("specialPrice".into(), Nbt::Int(self.special)));
        f.push(("demand".into(), Nbt::Int(self.demand)));
        f.push(("priceMultiplier".into(), Nbt::Float(self.mult)));
        f.push(("xp".into(), Nbt::Int(self.xp)));
        Nbt::Compound(f)
    }

    pub fn from_nbt(v: &Nbt) -> Option<Offer> {
        let geti = |k: &str, d: i32| crate::nbtio::int(v, k).map_or(d, |x| x as i32);
        Some(Offer {
            cost_a: Cost::from_nbt(v.get("buy")?)?,
            cost_b: v.get("buyB").and_then(Cost::from_nbt),
            result: v.get("sell").and_then(Stack::from_nbt)?,
            uses: geti("uses", 0),
            max_uses: geti("maxUses", 4),
            reward_exp: geti("rewardExp", 1) != 0,
            special: geti("specialPrice", 0),
            demand: geti("demand", 0),
            mult: match v.get("priceMultiplier") {
                Some(Nbt::Float(f)) => *f,
                Some(Nbt::Double(d)) => *d as f32,
                _ => 0.0,
            },
            xp: geti("xp", 1),
        })
    }
}

fn shrink(s: &mut Option<Stack>, n: i32) {
    if let Some(x) = s {
        x.count -= n;
        if x.count <= 0 {
            *s = None;
        }
    }
}

/// `Offers` (`Recipes`) of an entity.
pub fn offers_to_nbt(offers: &[Offer]) -> Nbt {
    Nbt::Compound(vec![("Recipes".into(), Nbt::List(offers.iter().map(Offer::to_nbt).collect()))])
}

pub fn offers_from_nbt(v: &Nbt) -> Vec<Offer> {
    crate::nbtio::list(v.get("Recipes")).iter().filter_map(Offer::from_nbt).collect()
}

/// The indicated offer (index > 0 only), otherwise the first one satisfied.
pub fn recipe_for(items: &Items, offers: &[Offer], a: Option<&Stack>, b: Option<&Stack>, hint: i32) -> Option<usize> {
    if hint > 0 && (hint as usize) < offers.len() {
        return offers[hint as usize].satisfied_by(items, a, b).then_some(hint as usize);
    }
    offers.iter().position(|o| o.satisfied_by(items, a, b))
}

/// Dye colours (diffuse colour of the texture), identifier order.
const DYE_DIFFUSE: [i32; 16] = [
    16383998, 16351261, 13061821, 3847130, 16701501, 8439583, 15961002, 4673362, 10329495, 1481884, 8991416, 3949738,
    8606770, 6192150, 11546150, 1908001,
];

/// Context of an offer draw: trader (villager type), random of the sequence.
pub struct TradeCtx<'a> {
    pub villager_type: &'a str,
    pub rng: &'a mut Rng,
}

/// Reading of the trade data and the required registries.
pub struct TradeData<'a> {
    pub pack: &'a crate::datapack::DataPack,
    pub items: &'a Items,
    pub loot: &'a crate::loot::Loot<'a>,
    /// Enchantment and potion tags: name → identifiers (tag order).
    pub ench_tags: &'a [(String, Vec<i32>)],
    pub ench_names: &'a [String],
    pub potion_tags: &'a [(String, Vec<i32>)],
    pub potion_names: &'a [String],
    pub effect_instant: &'a dyn Fn(&str) -> bool,
}

impl TradeData<'_> {
    fn int(&self, v: &Value, rng: &mut Rng) -> i32 {
        let ty = v["type"].as_str().unwrap_or("").trim_start_matches("minecraft:");
        if ty == "add" && v["inputs"].is_array() {
            return v["inputs"].as_array().into_iter().flatten().map(|x| self.int(x, rng)).sum();
        }
        self.loot.int(v, &crate::loot::Ctx::default(), rng)
    }

    fn float(&self, v: &Value, rng: &mut Rng) -> f32 {
        self.loot.float(v, &crate::loot::Ctx::default(), rng)
    }

    /// Ordered list of a trade tag (`tags/villager_trade/…`), recursive.
    fn trade_tag(&self, name: &str, out: &mut Vec<String>, depth: u32) {
        let name = name.trim_start_matches('#');
        let Some(t) = self.pack.get("tags/villager_trade", name) else { return };
        for e in t["values"].as_array().into_iter().flatten() {
            let id = e.as_str().or_else(|| e["id"].as_str()).unwrap_or("");
            if let Some(sub) = id.strip_prefix('#') {
                if depth < 16 {
                    self.trade_tag(sub, out, depth + 1);
                }
            } else if !out.iter().any(|x| x == id) {
                out.push(id.to_owned());
            }
        }
    }

    fn trade_list(&self, v: &Value) -> Vec<String> {
        let mut out = Vec::new();
        match v {
            Value::String(s) if s.starts_with('#') => self.trade_tag(s, &mut out, 0),
            Value::String(s) => out.push(s.clone()),
            Value::Array(a) => out.extend(a.iter().filter_map(Value::as_str).map(str::to_owned)),
            _ => {}
        }
        out
    }

    fn ench_set(&self, v: &Value) -> Vec<String> {
        match v {
            Value::String(s) if s.starts_with('#') => {
                let t = s.trim_start_matches('#');
                self.ench_tags
                    .iter()
                    .find(|(n, _)| n == t)
                    .map(|(_, ids)| ids.iter().filter_map(|&i| self.ench_names.get(i as usize).cloned()).collect())
                    .unwrap_or_default()
            }
            Value::String(s) => vec![s.clone()],
            Value::Array(a) => a.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
            _ => self.ench_names.to_vec(),
        }
    }

    fn ench(&self, id: &str) -> Option<std::sync::Arc<Value>> {
        self.pack.get("enchantment", id)
    }

    fn item_in(&self, set: &Value, item: &str) -> bool {
        match set {
            Value::String(s) if s.starts_with('#') => self.items.has_tag(s, item),
            Value::String(s) => s == item,
            Value::Array(a) => a.iter().any(|x| x.as_str() == Some(item)),
            _ => false,
        }
    }

    fn compatible(&self, a: &str, b: &str) -> bool {
        if a == b {
            return false;
        }
        let excl = |x: &str, y: &str| {
            self.ench(x).is_some_and(|e| {
                !e["exclusive_set"].is_null() && self.ench_set(&e["exclusive_set"]).iter().any(|n| n == y)
            })
        };
        !excl(a, b) && !excl(b, a)
    }

    fn cost(v: &Value, level: i32) -> i32 {
        v["base"].as_i64().unwrap_or(0) as i32 + v["per_level_above_first"].as_i64().unwrap_or(0) as i32 * (level - 1)
    }

    /// Cost adjusted by enchantability, successive weighted draws.
    fn select_enchantments(&self, rng: &mut Rng, item: &str, mut cost: i32, source: &[String]) -> Vec<(String, i32)> {
        let mut results = Vec::new();
        let ench = self.items.defaults(item)["minecraft:enchantable"]["value"].as_i64();
        let Some(value) = ench.map(|v| v as i32) else { return results };
        cost += 1 + rng.next_int_bound(value / 4 + 1) + rng.next_int_bound(value / 4 + 1);
        let span = (rng.next_float() + rng.next_float() - 1.0) * 0.15;
        cost = ((cost as f32 + cost as f32 * span).round() as i32).max(1);
        let book = item == "minecraft:book";
        let mut avail: Vec<(String, i32, i32)> = Vec::new();
        for id in source {
            let Some(e) = self.ench(id) else { continue };
            let primary = if e["primary_items"].is_null() { &e["supported_items"] } else { &e["primary_items"] };
            if !book && !self.item_in(primary, item) {
                continue;
            }
            let max = e["max_level"].as_i64().unwrap_or(1) as i32;
            for level in (1..=max).rev() {
                if cost >= Self::cost(&e["min_cost"], level) && cost <= Self::cost(&e["max_cost"], level) {
                    avail.push((id.clone(), level, e["weight"].as_i64().unwrap_or(1) as i32));
                    break;
                }
            }
        }
        if avail.is_empty() {
            return results;
        }
        let pick = |rng: &mut Rng, list: &[(String, i32, i32)]| -> Option<(String, i32)> {
            let total: i32 = list.iter().map(|e| e.2).sum();
            if total <= 0 {
                return None;
            }
            let mut r = rng.next_int_bound(total);
            for e in list {
                r -= e.2;
                if r < 0 {
                    return Some((e.0.clone(), e.1));
                }
            }
            None
        };
        if let Some(e) = pick(rng, &avail) {
            results.push(e);
        }
        while rng.next_int_bound(50) <= cost {
            if let Some(last) = results.last() {
                let last = last.0.clone();
                avail.retain(|e| self.compatible(&last, &e.0));
            }
            if avail.is_empty() {
                break;
            }
            if let Some(e) = pick(rng, &avail) {
                results.push(e);
            }
            cost /= 2;
        }
        results
    }

    /// Maximum level kept; book → enchanted book (stored enchantments).
    fn enchant(s: &mut Stack, id: &str, level: i32) {
        let key =
            if s.id == "minecraft:enchanted_book" { "minecraft:stored_enchantments" } else { "minecraft:enchantments" };
        let mut map: Vec<(String, Nbt)> = match crate::items::component(s, key) {
            Some(Nbt::Compound(f)) => f.clone(),
            _ => Vec::new(),
        };
        match map.iter_mut().find(|(k, _)| k == id) {
            Some((_, v)) => {
                let cur = v.as_i64().unwrap_or(0) as i32;
                *v = Nbt::Int(cur.max(level));
            }
            None => map.push((id.to_owned(), Nbt::Int(level))),
        }
        crate::items::set_component(s, key, Nbt::Compound(map));
    }

    fn ench_keys(s: &Stack, key: &str) -> Vec<String> {
        match crate::items::component(s, key) {
            Some(Nbt::Compound(f)) => f.iter().map(|(k, _)| k.clone()).collect(),
            _ => Vec::new(),
        }
    }

    /// Item predicate of `filtered` (items, enchantments/stored/non-empty map).
    fn item_filter(&self, f: &Value, s: &Stack) -> bool {
        if !f["items"].is_null() && !self.item_in(&f["items"], &s.id) {
            return false;
        }
        if let Some(p) = f["predicates"].as_object() {
            for (k, _) in p {
                let ok = match k.trim_start_matches("minecraft:") {
                    "enchantments" => !Self::ench_keys(s, "minecraft:enchantments").is_empty(),
                    "stored_enchantments" => !Self::ench_keys(s, "minecraft:stored_enchantments").is_empty(),
                    "map_id" => crate::items::component(s, "minecraft:map_id").is_some(),
                    _ => true,
                };
                if !ok {
                    return false;
                }
            }
        }
        true
    }

    /// Item functions of the trades, in order (empty stack = discarded).
    fn modify(&self, f: &Value, mut s: Stack, rng: &mut Rng) -> Stack {
        if let Some(a) = f.as_array() {
            for x in a {
                s = self.modify(x, s, rng);
            }
            return s;
        }
        let ty = f["type"].as_str().or_else(|| f["function"].as_str()).unwrap_or("").trim_start_matches("minecraft:");
        match ty {
            "discard" => s.count = 0,
            "filtered" => {
                let next = if self.item_filter(&f["item_filter"], &s) { &f["on_pass"] } else { &f["on_fail"] };
                if !next.is_null() {
                    s = self.modify(next, s, rng);
                }
            }
            "enchant_randomly" => {
                let book = s.id == "minecraft:book";
                let check = !book && f["only_compatible"].as_bool().unwrap_or(true);
                let list: Vec<String> = self
                    .ench_set(&f["options"])
                    .into_iter()
                    .filter(|e| !check || self.ench(e).is_some_and(|d| self.item_in(&d["supported_items"], &s.id)))
                    .collect();
                if list.is_empty() {
                    return s;
                }
                let e = list[rng.next_int_bound(list.len() as i32) as usize].clone();
                let max = self.ench(&e).and_then(|d| d["max_level"].as_i64()).unwrap_or(1) as i32;
                let level = if max <= 1 { 1 } else { 1 + rng.next_int_bound(max) };
                if book {
                    s = Stack { id: "minecraft:enchanted_book".into(), count: s.count, components: None };
                }
                Self::enchant(&mut s, &e, level);
                if f["include_additional_cost_component"].as_bool().unwrap_or(false) {
                    let c = 2 + rng.next_int_bound(5 + level * 10) + 3 * level;
                    crate::items::set_component(&mut s, "minecraft:additional_trade_cost", Nbt::Int(c));
                }
            }
            "enchant_with_levels" => {
                let levels = self.int(&f["levels"], rng);
                let source = self.ench_set(&f["options"]);
                let chosen = self.select_enchantments(rng, &s.id, levels, &source);
                if s.id == "minecraft:book" {
                    s = Stack { id: "minecraft:enchanted_book".into(), count: s.count, components: None };
                }
                for (e, l) in chosen {
                    Self::enchant(&mut s, &e, l);
                }
                if f["include_additional_cost_component"].as_bool().unwrap_or(false) && s.count > 0 && levels > 0 {
                    crate::items::set_component(&mut s, "minecraft:additional_trade_cost", Nbt::Int(levels));
                }
            }
            "set_random_dyes" => {
                let rolls = self.int(&f["number_of_dyes"], rng);
                if rolls > 0 {
                    let dyes: Vec<i32> = (0..rolls).map(|_| DYE_DIFFUSE[rng.next_int_bound(16) as usize]).collect();
                    let cur = crate::items::component(&s, "minecraft:dyed_color")
                        .and_then(|v| v.get("rgb").or(Some(v)).and_then(Nbt::as_i64))
                        .map(|v| v as i32);
                    let rgb = apply_dyes(cur, &dyes);
                    s.count = 1;
                    crate::items::set_component(&mut s, "minecraft:dyed_color", Nbt::Int(rgb));
                }
            }
            "set_potion" => {
                if let Some(id) = f["id"].as_str() {
                    set_potion(&mut s, id);
                }
            }
            "set_random_potion" => {
                let list: Vec<String> = match f["options"].as_str() {
                    Some(t) if t.starts_with('#') => {
                        let t = t.trim_start_matches('#');
                        self.potion_tags
                            .iter()
                            .find(|(n, _)| n == t)
                            .map(|(_, ids)| {
                                ids.iter().filter_map(|&i| self.potion_names.get(i as usize).cloned()).collect()
                            })
                            .unwrap_or_default()
                    }
                    Some(id) => vec![id.to_owned()],
                    None => self.potion_names.to_vec(),
                };
                if !list.is_empty() {
                    let p = list[rng.next_int_bound(list.len() as i32) as usize].clone();
                    set_potion(&mut s, &p);
                }
            }
            "set_stew_effect" => {
                if s.id == "minecraft:suspicious_stew"
                    && let Some(list) = f["effects"].as_array().filter(|l| !l.is_empty())
                {
                    let e = &list[rng.next_int_bound(list.len() as i32) as usize];
                    let id = e["type"].as_str().unwrap_or("");
                    let mut d = self.int(&e["duration"], rng);
                    if !(self.effect_instant)(id) {
                        d *= 20;
                    }
                    let mut effects = match crate::items::component(&s, "minecraft:suspicious_stew_effects") {
                        Some(Nbt::List(l)) => l.clone(),
                        _ => Vec::new(),
                    };
                    effects.push(Nbt::Compound(vec![
                        ("id".into(), Nbt::String(id.to_owned())),
                        ("duration".into(), Nbt::Int(d)),
                    ]));
                    crate::items::set_component(&mut s, "minecraft:suspicious_stew_effects", Nbt::List(effects));
                }
            }
            // S8: exploration map marked, resolved by the server (`resolve_offer_maps`).
            "exploration_map" => crate::lootfn::mark_exploration(&mut s, f),
            _ => {
                let ctx = crate::loot::Ctx::default();
                s = self.loot.apply_fn(f, s, &ctx, rng);
            }
        }
        s
    }

    /// `merchant_predicate`: properties of the entity (villager type).
    fn predicate(&self, p: &Value, ctx: &TradeCtx<'_>) -> bool {
        if p.is_null() {
            return true;
        }
        let want = &p["predicate"]["minecraft:predicates"]["minecraft:villager/variant"];
        let ty = format!("minecraft:{}", ctx.villager_type);
        match want {
            Value::String(s) => *s == ty,
            Value::Array(a) => a.iter().any(|x| x.as_str() == Some(ty.as_str())),
            _ => true,
        }
    }

    /// Generation of a villager offer; `None` if the offer is refused.
    pub fn offer(&self, id: &str, ctx: &mut TradeCtx<'_>) -> Option<Offer> {
        let t = self.pack.get("villager_trade", id)?;
        if !self.predicate(&t["merchant_predicate"], ctx) {
            return None;
        }
        let g = &t["gives"];
        let mut result = Stack {
            id: g["id"].as_str()?.to_owned(),
            count: g["count"].as_i64().unwrap_or(1) as i32,
            components: g["components"]
                .as_object()
                .map(|c| Nbt::Compound(c.iter().map(|(k, v)| (k.clone(), crate::recipes::json_to_nbt(v))).collect())),
        };
        let mut additional = 0;
        if !t["given_item_modifier"].is_null() {
            result = self.modify(&t["given_item_modifier"], result, ctx.rng);
            if result.count <= 0 {
                return None;
            }
        }
        if let Some(c) = crate::items::component(&result, "minecraft:additional_trade_cost").and_then(Nbt::as_i64) {
            additional += c as i32;
            crate::items::remove_component(&mut result, "minecraft:additional_trade_cost");
        }
        if !t["double_trade_price_enchantments"].is_null() {
            let set = self.ench_set(&t["double_trade_price_enchantments"]);
            if Self::ench_keys(&result, "minecraft:stored_enchantments").iter().any(|e| set.contains(e)) {
                additional *= 2;
            }
        }
        let cost = |v: &Value, extra: i32, rng: &mut Rng| -> Option<Cost> {
            let id = v["id"].as_str()?.to_owned();
            let n = if v["count"].is_null() { 1 } else { self.int(&v["count"], rng) };
            let max = self.items.max_stack(&Stack { id: id.clone(), count: 1, components: None });
            Some(Cost {
                id,
                count: (n + extra).clamp(0, max),
                components: v["components"].as_object().map(|c| {
                    Nbt::Compound(c.iter().map(|(k, v)| (k.clone(), crate::recipes::json_to_nbt(v))).collect())
                }),
            })
        };
        let a = cost(&t["wants"], additional, ctx.rng)?;
        if a.count < 1 {
            return None;
        }
        let b = if t["additional_wants"].is_null() { None } else { Some(cost(&t["additional_wants"], 0, ctx.rng)?) };
        if b.as_ref().is_some_and(|b| b.count < 1) {
            return None;
        }
        let max_uses = if t["max_uses"].is_null() { 4 } else { self.int(&t["max_uses"], ctx.rng) };
        let xp = if t["xp"].is_null() { 1 } else { self.int(&t["xp"], ctx.rng) };
        let disc =
            if t["reputation_discount"].is_null() { 0.0 } else { self.float(&t["reputation_discount"], ctx.rng) };
        Some(Offer {
            cost_a: a,
            cost_b: b,
            result,
            uses: 0,
            max_uses: max_uses.max(1),
            reward_exp: true,
            special: 0,
            demand: 0,
            mult: disc.max(0.0),
            xp: xp.max(0),
        })
    }

    /// Number drawn, then draws without replacement (or with duplicates).
    pub fn add_from_set(&self, set: &str, offers: &mut Vec<Offer>, ctx: &mut TradeCtx<'_>) {
        let Some(ts) = self.pack.get("trade_set", set) else { return };
        let n = self.int(&ts["amount"], ctx.rng);
        let dup = ts["allow_duplicates"].as_bool().unwrap_or(false);
        let mut list = self.trade_list(&ts["trades"]);
        let mut found = 0;
        while found < n && !list.is_empty() {
            let roll = ctx.rng.next_int_bound(list.len() as i32) as usize;
            if dup {
                match self.offer(&list[roll], ctx) {
                    Some(o) => {
                        offers.push(o);
                        found += 1;
                    }
                    None => {
                        list.remove(roll);
                    }
                }
            } else {
                let id = list.remove(roll);
                if let Some(o) = self.offer(&id, ctx) {
                    offers.push(o);
                    found += 1;
                }
            }
        }
    }
}

fn set_potion(s: &mut Stack, id: &str) {
    let mut c = match crate::items::component(s, "minecraft:potion_contents") {
        Some(Nbt::Compound(f)) => f.clone(),
        _ => Vec::new(),
    };
    c.retain(|(k, _)| k != "potion");
    c.insert(0, ("potion".into(), Nbt::String(id.to_owned())));
    crate::items::set_component(s, "minecraft:potion_contents", Nbt::Compound(c));
}

/// Average of the colours, intensity kept.
pub fn apply_dyes(cur: Option<i32>, dyes: &[i32]) -> i32 {
    let (mut r, mut g, mut b, mut inten, mut n) = (0i32, 0i32, 0i32, 0i32, 0i32);
    let mut add = |c: i32| {
        let (cr, cg, cb) = ((c >> 16) & 255, (c >> 8) & 255, c & 255);
        inten += cr.max(cg).max(cb);
        r += cr;
        g += cg;
        b += cb;
        n += 1;
    };
    if let Some(c) = cur {
        add(c);
    }
    for &d in dyes {
        add(d);
    }
    let (mut r, mut g, mut b) = (r / n, g / n, b / n);
    let avg = inten as f32 / n as f32;
    let res = r.max(g).max(b) as f32;
    r = (r as f32 * avg / res) as i32;
    g = (g as f32 * avg / res) as i32;
    b = (b as f32 * avg / res) as i32;
    (r << 16) | (g << 8) | b
}

impl Server {
    /// Adds the offers of a trade set (random: the game's named sequence if the set
    /// declares one, otherwise the world random).
    pub(crate) fn add_trade_set(&mut self, set: &str, villager_type: &str, offers: &mut Vec<Offer>) {
        let data = self.data.clone();
        let tag_fn = |tag: &str, name: &str| {
            let tags = &self.block_tags;
            let id = data.blocks.iter().position(|b| b.name == name).map(|i| i as u32);
            id.is_some_and(|id| tags.get(tag).is_some_and(|v| v.binary_search(&id).is_ok()))
        };
        let seq = self.life.pack.get("trade_set", set).and_then(|t| t["random_sequence"].as_str().map(str::to_owned));
        let empty: Vec<(String, Vec<i32>)> = Vec::new();
        let ench_tags = self.regs.tags.iter().find(|t| t.0 == "minecraft:enchantment").map_or(&empty, |t| &t.1);
        let potion_tags = self.regs.tags.iter().find(|t| t.0 == "minecraft:potion").map_or(&empty, |t| &t.1);
        let ench_names = self.regs.entries("minecraft:enchantment").to_vec();
        let potion_names: Vec<String> =
            data.registry("minecraft:potion").map(|r| r.entries.clone()).unwrap_or_default();
        let instant =
            |e: &str| matches!(e, "minecraft:instant_health" | "minecraft:instant_damage" | "minecraft:saturation");
        let life = &mut *self.life;
        let loot = crate::loot::Loot { pack: &life.pack, items: &life.items, block_tag: &tag_fn };
        let td = TradeData {
            pack: &life.pack,
            items: &life.items,
            loot: &loot,
            ench_tags,
            ench_names: &ench_names,
            potion_tags,
            potion_names: &potion_names,
            effect_instant: &instant,
        };
        let rng = match &seq {
            Some(s) => life.seqs.get(s),
            None => &mut life.rng,
        };
        let mut ctx = TradeCtx { villager_type, rng };
        td.add_from_set(set, offers, &mut ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dyes_average() {
        // A single red dye: colour of the dye.
        assert_eq!(apply_dyes(None, &[11546150]), 11546150);
        let two = apply_dyes(None, &[16383998, 1908001]);
        assert_eq!(two, apply_dyes(None, &[1908001, 16383998]));
    }

    #[test]
    fn offer_nbt_round_trip() {
        let o = Offer {
            cost_a: Cost { id: "minecraft:wheat".into(), count: 20, components: None },
            cost_b: None,
            result: Stack { id: "minecraft:emerald".into(), count: 1, components: None },
            uses: 3,
            max_uses: 16,
            reward_exp: true,
            special: -2,
            demand: 5,
            mult: 0.05,
            xp: 2,
        };
        let n = offers_to_nbt(std::slice::from_ref(&o));
        assert_eq!(offers_from_nbt(&n), vec![o.clone()]);
        let mut d = o;
        d.update_demand();
        assert_eq!(d.demand, 5 + 3 - 13);
    }
}
