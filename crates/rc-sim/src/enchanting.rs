//! Enchantments (S7, `docs/specs/mechanics-s7.md` § 3): definitions read from the pack
//! (`enchantment/*.json`, tags `tags/enchantment`), `enchantments` /
//! `stored_enchantments` components, cost of the three rows of the table, draw of an
//! enchantment list (same sequence of draws as the game: the table and loot share the algorithm).

use hashbrown::HashMap;

use rc_proto::nbt::Nbt;
use serde_json::Value;

use crate::datapack::DataPack;
use crate::items::{Items, component, remove_component, set_component};
use crate::player::Stack;
use crate::randsrc::Rand;

/// Definition of an enchantment.
#[derive(Debug, Clone)]
pub struct Ench {
    pub id: String,
    pub weight: i32,
    pub max_level: i32,
    pub min_cost: (i32, i32),
    pub max_cost: (i32, i32),
    pub anvil_cost: i32,
    supported: Value,
    primary: Value,
    exclusive: Vec<String>,
}

impl Ench {
    pub fn min_cost(&self, level: i32) -> i32 {
        self.min_cost.0 + self.min_cost.1 * (level - 1)
    }
    pub fn max_cost(&self, level: i32) -> i32 {
        self.max_cost.0 + self.max_cost.1 * (level - 1)
    }
}

/// Set of items of a definition: `#tag`, identifier, or list of identifiers.
fn holds(items: &Items, set: &Value, item: &str) -> bool {
    match set {
        Value::String(s) => match s.strip_prefix('#') {
            Some(t) => items.has_tag(t, item),
            None => s == item,
        },
        Value::Array(a) => a.iter().any(|v| v.as_str() == Some(item)),
        _ => false,
    }
}

pub struct Enchants {
    /// Registry order (sorted identifiers).
    pub list: Vec<Ench>,
    index: HashMap<String, usize>,
    tags: HashMap<String, Vec<usize>>,
}

fn cost_pair(v: &Value) -> (i32, i32) {
    (v["base"].as_i64().unwrap_or(0) as i32, v["per_level_above_first"].as_i64().unwrap_or(0) as i32)
}

/// Definitions of the pack (built once).
pub fn of(pack: &DataPack) -> &Enchants {
    pack.ench.get_or_init(|| Enchants::load(pack))
}

impl Enchants {
    pub fn load(pack: &DataPack) -> Enchants {
        let mut list = Vec::new();
        for (id, v) in pack.all("enchantment") {
            list.push(Ench {
                id,
                weight: v["weight"].as_i64().unwrap_or(1) as i32,
                max_level: v["max_level"].as_i64().unwrap_or(1) as i32,
                min_cost: cost_pair(&v["min_cost"]),
                max_cost: cost_pair(&v["max_cost"]),
                anvil_cost: v["anvil_cost"].as_i64().unwrap_or(1) as i32,
                supported: v["supported_items"].clone(),
                primary: v["primary_items"].clone(),
                exclusive: Vec::new(),
            });
        }
        let index: HashMap<String, usize> = list.iter().enumerate().map(|(i, e)| (e.id.clone(), i)).collect();
        let mut tags: HashMap<String, Vec<usize>> = HashMap::new();
        fn resolve(
            pack: &DataPack,
            index: &HashMap<String, usize>,
            name: &str,
            memo: &mut HashMap<String, Vec<usize>>,
            depth: u32,
        ) -> Vec<usize> {
            if let Some(v) = memo.get(name) {
                return v.clone();
            }
            let mut out = Vec::new();
            if depth < 16
                && let Some(t) = pack.get("tags/enchantment", name)
            {
                for e in t["values"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
                    let id = e.as_str().or_else(|| e["id"].as_str()).unwrap_or("");
                    let here = match id.strip_prefix('#') {
                        Some(t) => resolve(pack, index, t, memo, depth + 1),
                        None => index.get(id).map(|&i| vec![i]).unwrap_or_default(),
                    };
                    for i in here {
                        if !out.contains(&i) {
                            out.push(i);
                        }
                    }
                }
            }
            memo.insert(name.to_owned(), out.clone());
            out
        }
        // Exclusive sets.
        for i in 0..list.len() {
            let ex = pack.get("enchantment", &list[i].id).map(|v| v["exclusive_set"].clone()).unwrap_or(Value::Null);
            let ids: Vec<usize> = match &ex {
                Value::String(s) => match s.strip_prefix('#') {
                    Some(t) => resolve(pack, &index, t, &mut tags, 0),
                    None => index.get(s.as_str()).map(|&k| vec![k]).unwrap_or_default(),
                },
                Value::Array(a) => a.iter().filter_map(|v| index.get(v.as_str()?).copied()).collect(),
                _ => Vec::new(),
            };
            list[i].exclusive = ids.iter().map(|&k| list[k].id.clone()).collect();
        }
        for t in ["minecraft:in_enchanting_table", "minecraft:on_random_loot", "minecraft:curse"] {
            resolve(pack, &index, t, &mut tags, 0);
        }
        Enchants { list, index, tags }
    }

    pub fn get(&self, id: &str) -> Option<&Ench> {
        self.index.get(id).map(|&i| &self.list[i])
    }

    pub fn idx(&self, id: &str) -> Option<usize> {
        self.index.get(id).copied()
    }

    /// Members of a tag (file order, references resolved); empty if it is not loaded.
    pub fn tag(&self, name: &str) -> &[usize] {
        self.tags.get(name.trim_start_matches('#')).map_or(&[], Vec::as_slice)
    }

    pub fn is_curse(&self, id: &str) -> bool {
        self.idx(id).is_some_and(|i| self.tag("minecraft:curse").contains(&i))
    }

    /// Set of options of a function (`#tag`, identifier or list); `None` → the whole registry.
    pub fn options(&self, pack: &DataPack, v: &Value) -> Vec<usize> {
        match v {
            Value::Null => (0..self.list.len()).collect(),
            Value::String(s) => match s.strip_prefix('#') {
                Some(t) => {
                    if let Some(l) = self.tags.get(t) {
                        return l.clone();
                    }
                    // Tag not preloaded: resolved on demand.
                    let mut memo = HashMap::new();
                    fn r(
                        p: &DataPack,
                        ix: &HashMap<String, usize>,
                        n: &str,
                        m: &mut HashMap<String, Vec<usize>>,
                    ) -> Vec<usize> {
                        let mut out = Vec::new();
                        if let Some(t) = p.get("tags/enchantment", n) {
                            for e in t["values"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
                                let id = e.as_str().or_else(|| e["id"].as_str()).unwrap_or("");
                                let here = match id.strip_prefix('#') {
                                    Some(t) if !m.contains_key(t) => {
                                        m.insert(t.to_owned(), Vec::new());
                                        r(p, ix, t, m)
                                    }
                                    Some(_) => Vec::new(),
                                    None => ix.get(id).map(|&i| vec![i]).unwrap_or_default(),
                                };
                                for i in here {
                                    if !out.contains(&i) {
                                        out.push(i);
                                    }
                                }
                            }
                        }
                        out
                    }
                    r(pack, &self.index, t, &mut memo)
                }
                None => self.idx(s).into_iter().collect(),
            },
            Value::Array(a) => a.iter().filter_map(|x| self.idx(x.as_str()?)).collect(),
            _ => Vec::new(),
        }
    }

    pub fn supports(&self, items: &Items, e: &Ench, item: &str) -> bool {
        holds(items, &e.supported, item)
    }

    /// "Primary" item: supported and in the primary items if they are specified.
    pub fn primary(&self, items: &Items, e: &Ench, item: &str) -> bool {
        self.supports(items, e, item) && (e.primary.is_null() || holds(items, &e.primary, item))
    }

    /// Compatible: different and outside each other's exclusive set.
    pub fn compatible(&self, a: &str, b: &str) -> bool {
        if a == b {
            return false;
        }
        let ex = |x: &str, y: &str| self.get(x).is_some_and(|e| e.exclusive.iter().any(|z| z == y));
        !ex(a, b) && !ex(b, a)
    }

    /// Candidates of a cost (highest level whose interval contains the value).
    pub fn available(&self, items: &Items, value: i32, item: &str, source: &[usize]) -> Vec<(usize, i32)> {
        let book = item == "minecraft:book";
        let mut out = Vec::new();
        for &i in source {
            let e = &self.list[i];
            if !(book || self.primary(items, e, item)) {
                continue;
            }
            for level in (1..=e.max_level).rev() {
                if value >= e.min_cost(level) && value <= e.max_cost(level) {
                    out.push((i, level));
                    break;
                }
            }
        }
        out
    }

    fn weighted(&self, rng: &mut dyn Rand, list: &[(usize, i32)]) -> Option<(usize, i32)> {
        let total: i32 = list.iter().map(|(i, _)| self.list[*i].weight).sum();
        if total <= 0 {
            return None;
        }
        let mut r = rng.next_int_bound(total);
        for &(i, l) in list {
            r -= self.list[i].weight;
            if r < 0 {
                return Some((i, l));
            }
        }
        None
    }

    /// Draw of an enchantment list for a cost (`enchantable` of the required item).
    pub fn select(
        &self,
        items: &Items,
        rng: &mut dyn Rand,
        s: &Stack,
        cost: i32,
        source: &[usize],
    ) -> Vec<(usize, i32)> {
        let mut results = Vec::new();
        let Some(ench_value) = enchantable(items, s) else { return results };
        let mut cost = cost + 1 + rng.next_int_bound(ench_value / 4 + 1) + rng.next_int_bound(ench_value / 4 + 1);
        let span = (rng.next_float() + rng.next_float() - 1.0) * 0.15;
        cost = java_round(cost as f32 + cost as f32 * span).max(1);
        let mut cands = self.available(items, cost, &s.id, source);
        if cands.is_empty() {
            return results;
        }
        if let Some(x) = self.weighted(rng, &cands) {
            results.push(x);
        }
        while rng.next_int_bound(50) <= cost {
            if let Some(&(last, _)) = results.last() {
                let lid = self.list[last].id.clone();
                cands.retain(|(i, _)| self.compatible(&lid, &self.list[*i].id));
            }
            if cands.is_empty() {
                break;
            }
            if let Some(x) = self.weighted(rng, &cands) {
                results.push(x);
            }
            cost /= 2;
        }
        results
    }

    /// Enchants a stack: the book becomes an enchanted book.
    pub fn enchant_item(&self, items: &Items, rng: &mut dyn Rand, s: Stack, cost: i32, source: &[usize]) -> Stack {
        let list = self.select(items, rng, &s, cost, source);
        let mut s = s;
        if s.id == "minecraft:book" {
            s = Stack { id: "minecraft:enchanted_book".into(), count: 1, components: None };
        }
        for (i, l) in list {
            upgrade(items, &mut s, &self.list[i].id, l);
        }
        s
    }
}

/// Game rounding on a float (rounds towards +∞ at 0.5).
pub fn java_round(v: f32) -> i32 {
    (v + 0.5).floor() as i32
}

/// `enchantable` value of the item.
pub fn enchantable(items: &Items, s: &Stack) -> Option<i32> {
    if let Some(v) = component(s, "minecraft:enchantable") {
        return v.get("value").and_then(Nbt::as_i64).map(|v| v as i32);
    }
    let v = items.default_component(s, "minecraft:enchantable")?;
    v["value"].as_i64().map(|v| v as i32)
}

/// Component used: `stored_enchantments` for the enchanted book, otherwise `enchantments`.
pub fn ench_key(s: &Stack) -> &'static str {
    if s.id == "minecraft:enchanted_book" { "minecraft:stored_enchantments" } else { "minecraft:enchantments" }
}

/// The enchantments component exists on the stack.
pub fn can_store(items: &Items, s: &Stack) -> bool {
    let k = ench_key(s);
    component(s, k).is_some() || items.default_component(s, k).is_some()
}

/// Enchantments of the stack ("crafting" form: stored ones for an enchanted book), in order.
pub fn enchants_of(items: &Items, s: &Stack) -> Vec<(String, i32)> {
    read_comp(items, s, ench_key(s))
}

/// Enchantments of a given component (stack patch, otherwise default value).
pub fn read_comp(items: &Items, s: &Stack, k: &str) -> Vec<(String, i32)> {
    let from_nbt = |n: &Nbt| -> Vec<(String, i32)> {
        let map = n.get("levels").unwrap_or(n);
        match map {
            Nbt::Compound(f) => f.iter().filter_map(|(k, v)| Some((k.clone(), v.as_i64()? as i32))).collect(),
            _ => Vec::new(),
        }
    };
    if let Some(n) = component(s, k) {
        return from_nbt(n);
    }
    if component(s, &format!("!{k}")).is_some() {
        return Vec::new();
    }
    match items.default_component(s, k) {
        Some(Value::Object(o)) => o.iter().filter_map(|(k, v)| Some((k.clone(), v.as_i64()? as i32))).collect(),
        _ => Vec::new(),
    }
}

/// All enchantments carried (both components), the "has an enchantment" test.
pub fn has_any(items: &Items, s: &Stack) -> bool {
    for k in ["minecraft:enchantments", "minecraft:stored_enchantments"] {
        let n = component(s, k);
        let non_empty = match n {
            Some(Nbt::Compound(f)) => {
                let m = f.iter().find(|(k, _)| k == "levels").map(|(_, v)| v);
                match m {
                    Some(Nbt::Compound(g)) => !g.is_empty(),
                    _ => !f.is_empty(),
                }
            }
            _ => false,
        };
        if non_empty {
            return true;
        }
        if n.is_none()
            && let Some(Value::Object(o)) = items.default_component(s, k)
            && !o.is_empty()
        {
            return true;
        }
    }
    false
}

/// Replaces the enchantments (component suited to the item); empty list = empty component.
pub fn set_enchants(items: &Items, s: &mut Stack, list: &[(String, i32)]) {
    let k = ench_key(s);
    let fields: Vec<(String, Nbt)> =
        list.iter().filter(|(_, l)| *l > 0).map(|(e, l)| (e.clone(), Nbt::Int(*l))).collect();
    let default_empty = match items.default_component(s, k) {
        Some(Value::Object(o)) => o.is_empty(),
        None => false,
        _ => true,
    };
    if fields.is_empty() && default_empty {
        remove_component(s, k);
    } else {
        set_component(s, k, Nbt::Compound(fields));
    }
}

/// `upgrade`: level raised to the maximum of the two.
pub fn upgrade(items: &Items, s: &mut Stack, ench: &str, level: i32) {
    let mut l = enchants_of(items, s);
    match l.iter_mut().find(|(e, _)| e == ench) {
        Some(x) => x.1 = x.1.max(level),
        None => l.push((ench.to_owned(), level)),
    }
    set_enchants(items, s, &l);
}

/// The item can go on the table: `enchantable` present and enchantments empty.
pub fn is_enchantable(items: &Items, s: &Stack) -> bool {
    if enchantable(items, s).is_none() {
        return false;
    }
    let present = component(s, "minecraft:enchantments").is_some()
        || (component(s, "!minecraft:enchantments").is_none()
            && items.default_component(s, "minecraft:enchantments").is_some());
    present && read_comp(items, s, "minecraft:enchantments").is_empty()
}

/// Cost of a row of the table (`slot` 0..3) according to the bookshelves.
pub fn table_cost(items: &Items, rng: &mut dyn Rand, slot: i32, bookcases: i32, s: &Stack) -> i32 {
    if enchantable(items, s).is_none() {
        return 0;
    }
    let b = bookcases.min(15);
    let selected = rng.next_int_bound(8) + 1 + (b >> 1) + rng.next_int_bound(b + 1);
    match slot {
        0 => (selected / 3).max(1),
        1 => selected * 2 / 3 + 1,
        _ => selected.max(b * 2),
    }
}

/// State of the table: costs, indices (enchantment, level) shown for each row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableView {
    pub costs: [i32; 3],
    pub clue: [i32; 3],
    pub level: [i32; 3],
}

impl Default for TableView {
    fn default() -> Self {
        TableView { costs: [0; 3], clue: [-1; 3], level: [-1; 3] }
    }
}

impl Enchants {
    /// List of a row of the table: generator reset to `seed + row`, a book loses a randomly
    /// drawn enchantment if it has several.
    pub fn table_list(&self, items: &Items, seed: i32, slot: i32, cost: i32, s: &Stack) -> Vec<(usize, i32)> {
        let mut rng = crate::mobs::jrand::JRand::new(i64::from(seed.wrapping_add(slot)));
        let src = self.tag("minecraft:in_enchanting_table").to_vec();
        if src.is_empty() {
            return Vec::new();
        }
        let mut list = self.select(items, &mut rng, s, cost, &src);
        if s.id == "minecraft:book" && list.len() > 1 {
            let k = Rand::next_int_bound(&mut rng, list.len() as i32) as usize;
            list.remove(k);
        }
        list
    }

    /// Recomputation of the three rows for the item placed (on the table).
    pub fn table_view(&self, items: &Items, seed: i32, bookcases: i32, s: Option<&Stack>) -> TableView {
        let mut v = TableView::default();
        let Some(s) = s.filter(|s| is_enchantable(items, s)) else { return v };
        let mut rng = crate::mobs::jrand::JRand::new(i64::from(seed));
        for i in 0..3 {
            v.costs[i] = table_cost(items, &mut rng, i as i32, bookcases, s);
            if v.costs[i] < i as i32 + 1 {
                v.costs[i] = 0;
            }
        }
        for i in 0..3 {
            if v.costs[i] > 0 {
                // The draw of the list resets the shared generator: the sequence
                // continues on that seed for the choice of the index.
                let mut r2 = crate::mobs::jrand::JRand::new(i64::from(seed.wrapping_add(i as i32)));
                let src = self.tag("minecraft:in_enchanting_table").to_vec();
                let mut list = self.select(items, &mut r2, s, v.costs[i], &src);
                if s.id == "minecraft:book" && list.len() > 1 {
                    let k = Rand::next_int_bound(&mut r2, list.len() as i32) as usize;
                    list.remove(k);
                }
                if !list.is_empty() {
                    let k = Rand::next_int_bound(&mut r2, list.len() as i32) as usize;
                    v.clue[i] = list[k].0 as i32;
                    v.level[i] = list[k].1;
                }
            }
        }
        v
    }
}
