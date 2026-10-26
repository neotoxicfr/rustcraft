//! Game predicates (`docs/specs/advancements-stats.md` § 7): bounds, items, locations, entities,
//! damage sources and "contextual" conditions (loot conditions) read as JSON the way
//! data packs write them. Evaluated on demand against the integrated server's world.

use rc_proto::nbt::Nbt;
use serde_json::Value;

use crate::commands::ents::E;
use crate::mobs::mob::{Ent, Mob};
use crate::player::Stack;
use crate::projectiles::Proj;
use crate::server::Server;

/// Entity targeted by a predicate: designated (commands) or held outside the list (mob
/// being ticked, for example the one dying), or projectile being ticked (taken out of the
/// list while it acts, for example the one hitting a target).
#[derive(Clone, Copy)]
pub enum Who<'a> {
    E(E),
    Mob(&'a Mob),
    Proj(&'a Proj),
}

impl<'a> From<E> for Who<'a> {
    fn from(e: E) -> Self {
        Who::E(e)
    }
}

impl<'a> From<&'a Mob> for Who<'a> {
    fn from(m: &'a Mob) -> Self {
        Who::Mob(m)
    }
}

/// Damage source seen by the predicates: type, responsible entity and direct entity.
#[derive(Clone)]
pub struct Dmg<'a> {
    pub ty: String,
    pub source: Option<Who<'a>>,
    pub direct: Option<Who<'a>>,
    pub pos: Option<[f64; 3]>,
}

impl Dmg<'_> {
    pub fn plain(ty: &str) -> Dmg<'static> {
        Dmg { ty: ty.to_owned(), source: None, direct: None, pos: None }
    }
}

/// Context of a condition (available loot parameters).
#[derive(Default, Clone)]
pub struct Ctx<'a> {
    pub this: Option<Who<'a>>,
    pub origin: Option<[f64; 3]>,
    pub attacker: Option<Who<'a>>,
    pub direct_attacker: Option<Who<'a>>,
    pub attacking_player: Option<Who<'a>>,
    pub target: Option<Who<'a>>,
    pub interacting: Option<Who<'a>>,
    pub damage: Option<Dmg<'a>>,
    pub tool: Option<Option<&'a Stack>>,
    pub block: Option<u32>,
    pub explosion: Option<f32>,
}

// ---------------------------------------------------------------------------------------------
// Bounds.

fn num(v: &Value) -> Option<f64> {
    v.as_f64()
}

/// Integer bounds: a lone number or `{min, max}` (absent: everything).
pub fn ints(v: Option<&Value>, x: i64) -> bool {
    match v {
        None | Some(Value::Null) => true,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|m| m as i64 == x),
        Some(Value::Object(o)) => {
            o.get("min").and_then(num).is_none_or(|m| x as f64 >= m)
                && o.get("max").and_then(num).is_none_or(|m| x as f64 <= m)
        }
        _ => true,
    }
}

/// Real bounds.
pub fn doubles(v: Option<&Value>, x: f64) -> bool {
    match v {
        None | Some(Value::Null) => true,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|m| m == x),
        Some(Value::Object(o)) => {
            o.get("min").and_then(num).is_none_or(|m| x >= m) && o.get("max").and_then(num).is_none_or(|m| x <= m)
        }
        _ => true,
    }
}

/// Real bounds compared to a square.
pub fn doubles_sqr(v: Option<&Value>, x2: f64) -> bool {
    match v {
        None | Some(Value::Null) => true,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|m| m * m == x2),
        Some(Value::Object(o)) => {
            o.get("min").and_then(num).is_none_or(|m| x2 >= m * m)
                && o.get("max").and_then(num).is_none_or(|m| x2 <= m * m)
        }
        _ => true,
    }
}

/// Registry set: id, `#tag` or list; `tag` answers membership.
pub fn in_set(v: &Value, id: &str, tag: &dyn Fn(&str, &str) -> bool) -> bool {
    match v {
        Value::String(s) => match s.strip_prefix('#') {
            Some(t) => tag(&crate::commands::args::id(t), id),
            None => crate::commands::args::id(s) == id,
        },
        Value::Array(a) => a.iter().any(|x| x.as_str().is_some_and(|s| crate::commands::args::id(s) == id)),
        _ => false,
    }
}

fn opt_bool(v: &Value, k: &str) -> Option<bool> {
    v.get(k).and_then(Value::as_bool)
}

fn args_id(s: &str) -> String {
    crate::commands::args::id(s)
}

/// Raw value of a filterable field (`{raw, filtered}` or the lone value).
fn filterable_raw(v: &Nbt) -> &Nbt {
    v.get("raw").unwrap_or(v)
}

/// Expected effect (`{effect: {amplifier, duration, ambient, visible}}`): the instance's effect
/// must appear in it and its bounds pass.
fn effect_test(w: &Value, e: &crate::effects::Inst) -> bool {
    let Value::Object(map) = w else { return false };
    map.iter().find(|(id, _)| args_id(id) == e.id).is_some_and(|(_, pr)| {
        ints(pr.get("amplifier"), i64::from(e.amp))
            && ints(pr.get("duration"), i64::from(e.dur))
            && opt_bool(pr, "ambient").is_none_or(|b| b == e.ambient)
            && opt_bool(pr, "visible").is_none_or(|b| b == e.visible)
    })
}

/// Firework explosion: shape, twinkle, trail.
fn firework_test(p: &Value, c: &Nbt) -> bool {
    let flag = |k: &str| c.get(k).and_then(Nbt::as_i64).is_some_and(|v| v != 0);
    p.get("shape").and_then(Value::as_str).is_none_or(|sh| c.get("shape").and_then(Nbt::as_str) == Some(sh))
        && opt_bool(p, "has_twinkle").is_none_or(|b| flag("has_twinkle") == b)
        && opt_bool(p, "has_trail").is_none_or(|b| flag("has_trail") == b)
}

// ---------------------------------------------------------------------------------------------
// Loose JSON ↔ NBT comparison (exact components, NBT predicates).

/// JSON value of a component equal to its NBT form (numbers compared by value, booleans = bytes).
pub fn json_eq_nbt(j: &Value, n: &Nbt) -> bool {
    match (j, n) {
        (Value::Bool(b), n) => n.as_i64() == Some(i64::from(*b)),
        (Value::Number(a), n) => {
            let x = match n {
                Nbt::Float(f) => f64::from(*f),
                Nbt::Double(d) => *d,
                o => match o.as_i64() {
                    Some(i) => i as f64,
                    None => return false,
                },
            };
            a.as_f64().is_some_and(|y| (x - y).abs() <= 1e-6 * y.abs().max(1.0))
        }
        (Value::String(s), Nbt::String(t)) => s == t || crate::commands::args::id(s) == *t,
        (Value::Array(a), Nbt::List(l)) => a.len() == l.len() && a.iter().zip(l).all(|(x, y)| json_eq_nbt(x, y)),
        (Value::Array(a), Nbt::IntArray(l)) => {
            a.len() == l.len() && a.iter().zip(l).all(|(x, y)| x.as_i64() == Some(i64::from(*y)))
        }
        (Value::Array(a), Nbt::ByteArray(l)) => {
            a.len() == l.len() && a.iter().zip(l).all(|(x, y)| x.as_i64() == Some(i64::from(*y)))
        }
        (Value::Array(a), Nbt::LongArray(l)) => {
            a.len() == l.len() && a.iter().zip(l).all(|(x, y)| x.as_i64() == Some(*y))
        }
        (Value::Object(o), Nbt::Compound(f)) => {
            o.len() == f.len() && o.iter().all(|(k, v)| f.iter().any(|(fk, fv)| fk == k && json_eq_nbt(v, fv)))
        }
        _ => false,
    }
}

/// JSON → NBT (SNBT text of an `nbt` predicate: parsed string).
fn nbt_of_json(v: &Value) -> Option<Nbt> {
    match v {
        Value::String(s) => crate::commands::snbt::parse(s).ok(),
        o => Some(crate::recipes::json_to_nbt(o)),
    }
}

// ---------------------------------------------------------------------------------------------

impl Server {
    // ---- Registries and tags ----------------------------------------------------------------

    /// Member of a tag of any registry (items: the item table).
    pub(crate) fn p_tag(&self, reg: &str, tag: &str, id: &str) -> bool {
        let tag = crate::commands::args::id(tag.trim_start_matches('#'));
        if reg == "minecraft:item" {
            return self.life.items.has_tag(&tag, id);
        }
        let Some((_, tags)) = self.regs.tags.iter().find(|(r, _)| r == reg) else { return false };
        let Some((_, ids)) = tags.iter().find(|(n, _)| *n == tag) else { return false };
        let entries: &[String] = match self.data.registry(reg) {
            Some(r) => &r.entries,
            None => self.regs.entries(reg),
        };
        let Some(i) = entries.iter().position(|e| e == id) else { return false };
        ids.contains(&(i as i32))
    }

    pub(crate) fn p_set(&self, reg: &str, v: &Value, id: &str) -> bool {
        in_set(v, id, &|t, x| self.p_tag(reg, t, x))
    }

    // ---- Items ------------------------------------------------------------------------------

    /// Effective component of a stack (patch, otherwise the item's default) as NBT.
    pub(crate) fn p_component(&self, s: &Stack, name: &str) -> Option<Nbt> {
        if let Some(c) = &s.components {
            if c.get(&format!("!{name}")).is_some() {
                return None;
            }
            if let Some(v) = c.get(name) {
                return Some(v.clone());
            }
        }
        self.life.items.default_component(s, name).map(|v| crate::recipes::json_to_nbt(&v))
    }

    /// Item predicate (`items`, `count`, exact `components`, partial `predicates`).
    pub(crate) fn p_item(&self, p: &Value, s: Option<&Stack>) -> bool {
        let air = Stack { id: "minecraft:air".into(), count: 0, components: None };
        let s = s.unwrap_or(&air);
        if let Some(items) = p.get("items")
            && !self.p_set("minecraft:item", items, &s.id)
        {
            return false;
        }
        if !ints(p.get("count"), i64::from(s.count)) {
            return false;
        }
        self.p_components(p, s)
    }

    /// `components` (exact equality) and `predicates` (partial predicates) of a stack.
    fn p_components(&self, p: &Value, s: &Stack) -> bool {
        if let Some(Value::Object(exact)) = p.get("components") {
            for (k, want) in exact {
                let k = crate::commands::args::id(k.trim_start_matches('!'));
                match self.p_component(s, &k) {
                    Some(have) if json_eq_nbt(want, &have) => {}
                    _ => return false,
                }
            }
        }
        if let Some(Value::Object(partial)) = p.get("predicates") {
            for (k, want) in partial {
                if !self.p_partial(&crate::commands::args::id(k), want, s) {
                    return false;
                }
            }
        }
        true
    }

    /// Partial predicate of an item component.
    pub(crate) fn p_partial(&self, kind: &str, p: &Value, s: &Stack) -> bool {
        let comp = |n: &str| self.p_component(s, n);
        match kind {
            "minecraft:damage" => {
                // False without an effective `damage` component (non-usable item); maximum durability 0
                // without `max_damage`.
                let Some(dmg) = comp(kind).and_then(|d| d.as_i64()).map(|d| d as i32) else { return false };
                let max = comp("minecraft:max_damage").and_then(|d| d.as_i64()).map_or(0, |d| d as i32);
                ints(p.get("durability"), i64::from(max.wrapping_sub(dmg))) && ints(p.get("damage"), i64::from(dmg))
            }
            "minecraft:enchantments" | "minecraft:stored_enchantments" => {
                let list = comp(kind);
                let levels = |id: &str| -> i32 {
                    let Some(l) = &list else { return 0 };
                    let src = l.get("levels").unwrap_or(l);
                    src.get(id).and_then(Nbt::as_i64).unwrap_or(0) as i32
                };
                let all: Vec<(String, i32)> = match &list {
                    Some(l) => match l.get("levels").unwrap_or(l) {
                        Nbt::Compound(f) => {
                            f.iter().map(|(k, v)| (k.clone(), v.as_i64().unwrap_or(0) as i32)).collect()
                        }
                        _ => Vec::new(),
                    },
                    None => Vec::new(),
                };
                let arr: Vec<&Value> = match p {
                    Value::Array(a) => a.iter().collect(),
                    o => vec![o],
                };
                arr.into_iter().all(|e| match e.get("enchantments") {
                    Some(set) => all.iter().any(|(id, lvl)| {
                        self.p_set("minecraft:enchantment", set, id) && ints(e.get("levels"), i64::from(*lvl))
                    }),
                    None => match e.get("levels") {
                        Some(lv) => all.iter().any(|(_, lvl)| ints(Some(lv), i64::from(*lvl))),
                        None => !all.is_empty() || levels("") > 0,
                    },
                })
            }
            "minecraft:potion_contents" => {
                // `{potions, effects}`: base potion in the set (absent = failure), then
                // effects (potion then added, durations not scaled) in a collection.
                let Some(c) = comp(kind) else { return false };
                let potion = match &c {
                    Nbt::String(s) => Some(s.clone()),
                    c => c.get("potion").and_then(Nbt::as_str).map(str::to_owned),
                };
                if let Some(set) = p.get("potions")
                    && !potion.as_deref().is_some_and(|id| self.p_set("minecraft:potion", set, &args_id(id)))
                {
                    return false;
                }
                let Some(eff) = p.get("effects") else { return true };
                let all = crate::potions::contents_effects(&c);
                self.p_coll(Some(eff), &all, effect_test)
            }
            "minecraft:custom_data" => {
                let Some(have) = comp(kind) else { return false };
                nbt_of_json(p).is_some_and(|want| crate::commands::select::nbt_matches(&want, &have))
            }
            "minecraft:container" | "minecraft:bundle_contents" => {
                let items: Vec<Stack> = match comp(kind) {
                    Some(Nbt::List(l)) => {
                        l.iter().filter_map(|e| Stack::from_nbt(e.get("item").unwrap_or(e))).collect()
                    }
                    _ => Vec::new(),
                };
                self.p_collection(p.get("items"), &items)
            }
            "minecraft:jukebox_playable" => {
                let Some(c) = comp(kind) else { return false };
                let song = c.get("song").and_then(Nbt::as_str).or(c.as_str()).unwrap_or("").to_owned();
                p.get("song").is_none_or(|set| self.p_set("minecraft:jukebox_song", set, &song))
            }
            "minecraft:trim" => {
                let Some(c) = comp(kind) else { return false };
                let mat = c.get("material").and_then(Nbt::as_str).unwrap_or("").to_owned();
                let pat = c.get("pattern").and_then(Nbt::as_str).unwrap_or("").to_owned();
                p.get("material").is_none_or(|set| self.p_set("minecraft:trim_material", set, &mat))
                    && p.get("pattern").is_none_or(|set| self.p_set("minecraft:trim_pattern", set, &pat))
            }
            "minecraft:villager/variant" => {
                let Some(c) = comp(kind) else { return false };
                c.as_str().is_some_and(|v| self.p_set("minecraft:villager_type", p, v))
            }
            "minecraft:writable_book_content" => {
                // Pages (raw text of each page) in a collection.
                let Some(c) = comp(kind) else { return false };
                let pages: Vec<&Nbt> = crate::nbtio::list(c.get("pages")).iter().map(filterable_raw).collect();
                self.p_coll(p.get("pages"), &pages, |w, pg| w.as_str().is_some_and(|t| pg.as_str() == Some(t)))
            }
            "minecraft:written_book_content" => {
                let Some(c) = comp(kind) else { return false };
                if let Some(a) = p.get("author").and_then(Value::as_str)
                    && c.get("author").and_then(Nbt::as_str) != Some(a)
                {
                    return false;
                }
                if let Some(t) = p.get("title").and_then(Value::as_str)
                    && c.get("title").map(filterable_raw).and_then(Nbt::as_str) != Some(t)
                {
                    return false;
                }
                if !ints(p.get("generation"), c.get("generation").and_then(Nbt::as_i64).unwrap_or(0)) {
                    return false;
                }
                if let Some(r) = opt_bool(p, "resolved")
                    && c.get("resolved").and_then(Nbt::as_i64).is_some_and(|v| v != 0) != r
                {
                    return false;
                }
                // Pages: raw component of each page equal to the expected component.
                let pages: Vec<Option<rc_text::Component>> = crate::nbtio::list(c.get("pages"))
                    .iter()
                    .map(|pg| rc_text::from_nbt(filterable_raw(pg)))
                    .collect();
                self.p_coll(p.get("pages"), &pages, |w, pg| {
                    pg.is_some() && rc_text::from_nbt(&crate::recipes::json_to_nbt(w)) == *pg
                })
            }
            "minecraft:firework_explosion" => {
                let Some(c) = comp(kind) else { return false };
                firework_test(p, &c)
            }
            "minecraft:fireworks" => {
                let Some(c) = comp(kind) else { return false };
                let explosions: Vec<&Nbt> = crate::nbtio::list(c.get("explosions")).iter().collect();
                self.p_coll(p.get("explosions"), &explosions, |w, e| firework_test(w, e))
                    && ints(p.get("flight_duration"), c.get("flight_duration").and_then(Nbt::as_i64).unwrap_or(0))
            }
            "minecraft:attribute_modifiers" => {
                // List `{type, id, amount, operation, slot}` (old form `{modifiers: […]}` also read).
                let Some(c) = comp(kind) else { return false };
                let list = match &c {
                    Nbt::List(l) => l.as_slice(),
                    c => crate::nbtio::list(c.get("modifiers")),
                };
                let entries: Vec<&Nbt> = list.iter().collect();
                self.p_coll(p.get("modifiers"), &entries, |w, e| self.modifier_test(w, e))
            }
            // Others: presence of the component.
            other => comp(other).is_some(),
        }
    }

    /// Item collection predicate: `contains`, `count`, `size`.
    fn p_collection(&self, p: Option<&Value>, items: &[Stack]) -> bool {
        self.p_coll(p, items, |w, s| self.p_item(w, Some(s)))
    }

    /// Predicate on any collection (`contains`, `count`, `size`); `test` judges an element.
    fn p_coll<T>(&self, p: Option<&Value>, items: &[T], test: impl Fn(&Value, &T) -> bool) -> bool {
        let Some(p) = p else { return true };
        if let Some(Value::Array(want)) = p.get("contains") {
            // Each element removes all the tests it satisfies.
            let mut left: Vec<&Value> = want.iter().collect();
            for s in items {
                if left.is_empty() {
                    break;
                }
                left.retain(|w| !test(w, s));
            }
            if !left.is_empty() {
                return false;
            }
        }
        if let Some(Value::Array(counts)) = p.get("count") {
            for c in counts {
                let n = items.iter().filter(|s| test(&c["test"], s)).count();
                if !ints(c.get("count"), n as i64) {
                    return false;
                }
            }
        }
        ints(p.get("size"), items.len() as i64)
    }

    /// Attribute modifier (`type`, `id`, `amount`, `operation`, `slot`: default `any`).
    fn modifier_test(&self, w: &Value, e: &Nbt) -> bool {
        let s = |k: &str| e.get(k).and_then(Nbt::as_str).map(str::to_owned);
        if let Some(set) = w.get("attribute")
            && !s("type").is_some_and(|t| self.p_set("minecraft:attribute", set, &args_id(&t)))
        {
            return false;
        }
        if let Some(id) = w.get("id").and_then(Value::as_str)
            && s("id").map(|x| args_id(&x)) != Some(args_id(id))
        {
            return false;
        }
        let amount = match e.get("amount") {
            Some(Nbt::Double(d)) => *d,
            Some(Nbt::Float(f)) => f64::from(*f),
            Some(o) => o.as_i64().unwrap_or(0) as f64,
            None => 0.0,
        };
        if !doubles(w.get("amount"), amount) {
            return false;
        }
        if let Some(op) = w.get("operation").and_then(Value::as_str)
            && s("operation").as_deref() != Some(op)
        {
            return false;
        }
        w.get("slot").and_then(Value::as_str).is_none_or(|slot| s("slot").as_deref().unwrap_or("any") == slot)
    }

    // ---- Locations ------------------------------------------------------------------------------

    /// Location predicate at `(x, y, z)` in the current dimension.
    pub(crate) fn p_location(&self, p: &Value, x: f64, y: f64, z: f64) -> bool {
        if let Some(pos) = p.get("position")
            && !(doubles(pos.get("x"), x) && doubles(pos.get("y"), y) && doubles(pos.get("z"), z))
        {
            return false;
        }
        if let Some(d) = p.get("dimension").and_then(Value::as_str)
            && crate::commands::args::id(d) != self.dim.name
        {
            return false;
        }
        let bp = [x.floor() as i32, y.floor() as i32, z.floor() as i32];
        let loaded = self.is_loaded(bp);
        if let Some(b) = p.get("biomes")
            && !(loaded && self.p_set("minecraft:worldgen/biome", b, &self.biome_name(bp)))
        {
            return false;
        }
        if let Some(st) = p.get("structures")
            && !(loaded && self.p_structure_at(bp, st))
        {
            return false;
        }
        if let Some(sm) = opt_bool(p, "smokey")
            && !(loaded && sm == crate::mobs::wild::hive::smokey(self, bp))
        {
            return false;
        }
        if let Some(l) = p.get("light")
            && !(loaded && ints(l.get("light"), i64::from(self.p_local_brightness(bp))))
        {
            return false;
        }
        if let Some(b) = p.get("block")
            && !self.p_block(b, bp)
        {
            return false;
        }
        if let Some(f) = p.get("fluid")
            && !self.p_fluid(f, bp)
        {
            return false;
        }
        if let Some(sky) = opt_bool(p, "can_see_sky")
            && sky != self.p_can_see_sky(bp)
        {
            return false;
        }
        true
    }

    /// Raw local brightness: darkened sky, block.
    fn p_local_brightness(&self, p: [i32; 3]) -> i32 {
        let sky = self.light_at(p, true) - self.sky_darken();
        sky.max(self.light_at(p, false))
    }

    /// Visible sky: above the "motion blocking" height map.
    fn p_can_see_sky(&self, p: [i32; 3]) -> bool {
        if p[1] >= self.dim.height.min_y + self.dim.height.sections as i32 * 16 {
            return true;
        }
        self.light_at(p, true) >= 15
    }

    /// Block predicate (`BlockPredicate`) at `p`.
    pub(crate) fn p_block(&self, b: &Value, p: [i32; 3]) -> bool {
        if !self.is_loaded(p) {
            return false;
        }
        let s = self.get(p);
        if !self.p_block_state(b, s) {
            return false;
        }
        if let Some(n) = b.get("nbt") {
            let Some(be) = self.be_get(p) else { return false };
            let Some(want) = nbt_of_json(n) else { return false };
            if !crate::commands::select::nbt_matches(&want, be) {
                return false;
            }
        }
        true
    }

    /// "State" part of a block predicate.
    pub(crate) fn p_block_state(&self, b: &Value, s: u32) -> bool {
        let name = self.data.block_of(s).map_or("", |b| b.name.as_str()).to_owned();
        if let Some(set) = b.get("blocks")
            && !self.p_set("minecraft:block", set, &name)
        {
            return false;
        }
        if let Some(Value::Object(props)) = b.get("state") {
            let v = self.v();
            for (k, want) in props {
                let Some(have) = v.get(s, k) else { return false };
                if !prop_match(want, have) {
                    return false;
                }
            }
        }
        true
    }

    fn p_fluid(&self, f: &Value, p: [i32; 3]) -> bool {
        if !self.is_loaded(p) {
            return false;
        }
        let fl = self.fluid_at(p);
        let name = match (fl.kind, fl.source) {
            (1, true) => "minecraft:water",
            (1, false) => "minecraft:flowing_water",
            (2, true) => "minecraft:lava",
            (2, false) => "minecraft:flowing_lava",
            _ => "minecraft:empty",
        };
        if let Some(set) = f.get("fluids")
            && !self.p_set("minecraft:fluid", set, name)
        {
            return false;
        }
        if let Some(Value::Object(props)) = f.get("state") {
            for (k, want) in props {
                let have = match k.as_str() {
                    "level" => (if fl.source { 8 } else { i32::from(fl.amount) }).to_string(),
                    "falling" => fl.falling.to_string(),
                    _ => return false,
                };
                if !prop_match(want, &have) {
                    return false;
                }
            }
        }
        true
    }

    /// Structure (set) one of whose pieces contains `p`, from the
    /// references and starts of the loaded chunks.
    pub(crate) fn p_structure_at(&self, p: [i32; 3], set: &Value) -> bool {
        let Some(Some(c)) = self.chunks.get(&(p[0] >> 4, p[2] >> 4)) else { return false };
        let refs = c.root.get("structures").and_then(|s| s.get("References"));
        let Some(Nbt::Compound(refs)) = refs else { return false };
        // Structure tags read from the game data (they are not part of the
        // tags sent to the client).
        let in_tag = |t: &str, x: &str| self.structure_tag_members(&format!("#{t}")).iter().any(|m| m == x);
        for (name, starts) in refs {
            if !in_set(set, name, &in_tag) {
                continue;
            }
            // References read in place (no more copy of the array at each question).
            let longs: Box<dyn Iterator<Item = i64> + '_> = match starts {
                Nbt::LongArray(l) => Box::new(l.iter().copied()),
                Nbt::List(l) => Box::new(l.iter().filter_map(Nbt::as_i64)),
                _ => Box::new(std::iter::empty()),
            };
            for l in longs {
                let (sx, sz) = ((l & 0xFFFF_FFFF) as i32, (l >> 32) as i32);
                let Some(Some(sc)) = self.chunks.get(&(sx, sz)) else { continue };
                let start = sc.root.get("structures").and_then(|s| s.get("starts")).and_then(|s| s.get(name));
                let Some(start) = start else { continue };
                for piece in crate::nbtio::list(start.get("Children")) {
                    if let Some(Nbt::IntArray(bb)) = piece.get("BB")
                        && bb.len() == 6
                        && (bb[0]..=bb[3]).contains(&p[0])
                        && (bb[1]..=bb[4]).contains(&p[1])
                        && (bb[2]..=bb[5]).contains(&p[2])
                    {
                        return true;
                    }
                }
            }
        }
        false
    }

    // ---- Entities ---------------------------------------------------------------------------

    pub(crate) fn w_type(&self, w: Who<'_>) -> String {
        match w {
            Who::E(e) => self.ent_type(e),
            Who::Mob(m) => m.kind.info().name.to_owned(),
            Who::Proj(p) => p.kind.name().to_owned(),
        }
    }

    pub(crate) fn w_pos(&self, w: Who<'_>) -> [f64; 3] {
        match w {
            Who::E(e) => self.e_pos(e),
            Who::Mob(m) => m.pos,
            Who::Proj(p) => p.pos,
        }
    }

    fn w_mob<'a>(&'a self, w: Who<'a>) -> Option<&'a Mob> {
        match w {
            Who::Mob(m) => Some(m),
            Who::E(E::Mob(i)) => self.mobs.mob_ref_any(i),
            Who::E(_) | Who::Proj(_) => None,
        }
    }

    /// Entity NBT as the `nbt` predicates and the selectors' `nbt` option compare it:
    /// save without the type id `id`, plus the held item for the player.
    pub(crate) fn w_nbt(&self, w: Who<'_>) -> Nbt {
        let mut n = match w {
            Who::E(e) => self.ent_nbt(e),
            Who::Mob(m) => self.mob_nbt(m),
            Who::Proj(p) => self.proj_to_nbt(p),
        };
        if let Nbt::Compound(f) = &mut n {
            f.retain(|(k, _)| k != "id");
        }
        if matches!(w, Who::E(E::Player))
            && let Some(s) = &self.slots[36 + self.held as usize]
        {
            crate::nbtio::set(&mut n, "SelectedItem", s.to_nbt(None));
        }
        n
    }

    fn w_living(&self, w: Who<'_>) -> bool {
        match w {
            Who::E(E::Player) | Who::Mob(_) => true,
            Who::E(e) => self.ent_is_living(e),
            Who::Proj(_) => false,
        }
    }

    /// Equipment: head, chest, legs, feet, body, hand, offhand.
    fn w_equipment(&self, w: Who<'_>, slot: &str) -> Option<Stack> {
        match w {
            Who::E(E::Player) => {
                let i = match slot {
                    "head" => 5,
                    "chest" => 6,
                    "legs" => 7,
                    "feet" => 8,
                    "mainhand" => 36 + self.held as usize,
                    "offhand" => 45,
                    _ => return None,
                };
                self.slots.get(i).cloned().flatten()
            }
            _ => match self.w_mob(w) {
                Some(m) => crate::mobs::mob::slot_index(slot).and_then(|i| m.equipment[i].clone()),
                None => {
                    let n = self.w_nbt(w);
                    n.get("equipment").and_then(|e| e.get(slot)).and_then(Stack::from_nbt)
                }
            },
        }
    }

    /// Active effects: (id, amplifier, duration, ambient, visible).
    fn w_effects(&self, w: Who<'_>) -> Vec<(String, i32, i32, bool, bool)> {
        match w {
            Who::E(E::Player) => {
                self.life.fx.list.iter().map(|i| (i.id.clone(), i.amp, i.dur, i.ambient, i.visible)).collect()
            }
            _ => {
                let id = self.w_id(w);
                self.cmd
                    .mob_effects
                    .get(&id)
                    .map(|l| {
                        l.iter().map(|e| (e.id.clone(), e.amplifier, e.duration, e.ambient, e.particles)).collect()
                    })
                    .unwrap_or_default()
            }
        }
    }

    fn w_flag(&self, w: Who<'_>, flag: &str) -> bool {
        match w {
            Who::E(E::Player) => match flag {
                "on_ground" => self.on_ground,
                "on_fire" => self.life.vit.fire > 0,
                "sneaking" => self.sneaking,
                "sprinting" => self.life.vit.sprinting,
                // Flight: elytra glide or ability flight.
                "flying" => self.flying || self.proj_player.fall_flying,
                "fall_flying" => self.proj_player.fall_flying,
                "swimming" => self.mobs.sea.player_swimming,
                "in_water" => {
                    self.fluid_at([self.pos[0].floor() as i32, self.pos[1].floor() as i32, self.pos[2].floor() as i32])
                        .kind
                        == 1
                }
                _ => false,
            },
            _ => match self.w_mob(w) {
                Some(m) => match flag {
                    "on_ground" => m.on_ground,
                    "on_fire" => m.on_fire(),
                    "baby" => m.is_baby(),
                    "in_water" => m.in_water,
                    _ => false,
                },
                None => {
                    let n = self.w_nbt(w);
                    match flag {
                        "on_ground" => n.get("OnGround").and_then(Nbt::as_i64).is_some_and(|v| v != 0),
                        "on_fire" => n.get("Fire").and_then(Nbt::as_i64).is_some_and(|v| v > 0),
                        "baby" => n.get("Age").and_then(Nbt::as_i64).is_some_and(|v| v < 0),
                        _ => false,
                    }
                }
            },
        }
    }

    /// Known movement: last move received from the client for the player, velocity otherwise.
    fn w_velocity(&self, w: Who<'_>) -> [f64; 3] {
        match w {
            Who::E(E::Player) => self.proj_player.known_move,
            Who::Proj(p) => p.vel,
            _ => match self.w_mob(w) {
                Some(m) => m.vel,
                None => crate::nbtio::f64s::<3>(self.w_nbt(w).get("Motion")).unwrap_or_default(),
            },
        }
    }

    fn w_fall(&self, w: Who<'_>) -> f64 {
        match w {
            Who::E(E::Player) => self.life.vit.fall,
            _ => self.w_mob(w).map_or(0.0, |m| m.fall_distance),
        }
    }

    /// Block under the feet at `off` (common rule of the supporting block).
    fn w_on_pos(&self, w: Who<'_>, off: f32) -> [i32; 3] {
        let support = match w {
            Who::E(E::Player) => self.player_support(),
            Who::Mob(m) => m.support,
            Who::E(E::Mob(i)) => self.mobs.mob_ref_any(i).and_then(|m| m.support),
            Who::E(e) => self.cart_of(e).and_then(|c| c.support),
            Who::Proj(_) => None,
        };
        self.on_pos(self.w_pos(w), support, off)
    }

    /// Entity predicate (named sub-predicates); `None`: false.
    pub(crate) fn p_entity(&self, p: &Value, w: Option<Who<'_>>, origin: Option<[f64; 3]>) -> bool {
        let Some(w) = w else { return false };
        let Value::Object(parts) = p else { return true };
        // Game order: type first, NBT last; the result is the same (AND).
        for (k, v) in parts {
            let k = crate::commands::args::id(k);
            if !self.p_entity_part(&k, v, w, origin) {
                return false;
            }
        }
        true
    }

    fn p_entity_part(&self, k: &str, v: &Value, w: Who<'_>, origin: Option<[f64; 3]>) -> bool {
        match k {
            "minecraft:entity_type" | "minecraft:type" => self.p_set("minecraft:entity_type", v, &self.w_type(w)),
            "minecraft:location" => {
                let [x, y, z] = self.w_pos(w);
                self.p_location(v, x, y, z)
            }
            "minecraft:stepping_on" => {
                if !self.w_flag(w, "on_ground") {
                    return false;
                }
                let b = self.w_on_pos(w, 1.0e-5);
                self.p_location(v, f64::from(b[0]) + 0.5, f64::from(b[1]) + 0.5, f64::from(b[2]) + 0.5)
            }
            "minecraft:movement_affected_by" => {
                let b = self.w_on_pos(w, 0.500_001);
                self.p_location(v, f64::from(b[0]) + 0.5, f64::from(b[1]) + 0.5, f64::from(b[2]) + 0.5)
            }
            "minecraft:distance" => {
                let Some(o) = origin else { return false };
                let e = self.w_pos(w);
                p_distance(v, o, e)
            }
            "minecraft:movement" => {
                let m = self.w_velocity(w);
                let (x, y, z) = (m[0] * 20.0, m[1] * 20.0, m[2] * 20.0);
                doubles(v.get("x"), x)
                    && doubles(v.get("y"), y)
                    && doubles(v.get("z"), z)
                    && doubles_sqr(v.get("speed"), x * x + y * y + z * z)
                    && doubles_sqr(v.get("horizontal_speed"), x * x + z * z)
                    && doubles(v.get("vertical_speed"), y.abs())
                    && doubles(v.get("fall_distance"), self.w_fall(w))
            }
            "minecraft:effects" => {
                let have = self.w_effects(w);
                if !self.w_living(w) {
                    return false;
                }
                let Value::Object(want) = v else { return true };
                want.iter().all(|(id, pr)| {
                    let id = crate::commands::args::id(id);
                    have.iter().find(|e| e.0 == id).is_some_and(|e| {
                        ints(pr.get("amplifier"), i64::from(e.1))
                            && ints(pr.get("duration"), i64::from(e.2))
                            && opt_bool(pr, "ambient").is_none_or(|b| b == e.3)
                            && opt_bool(pr, "visible").is_none_or(|b| b == e.4)
                    })
                })
            }
            "minecraft:nbt" => {
                nbt_of_json(v).is_some_and(|want| crate::commands::select::nbt_matches(&want, &self.w_nbt(w)))
            }
            "minecraft:flags" => {
                let chk = |key: &str, flag: &str| opt_bool(v, key).is_none_or(|b| b == self.w_flag(w, flag));
                let living = self.w_living(w);
                chk("is_on_ground", "on_ground")
                    && chk("is_on_fire", "on_fire")
                    && chk("is_sneaking", "sneaking")
                    && chk("is_sprinting", "sprinting")
                    && chk("is_swimming", "swimming")
                    && opt_bool(v, "is_flying").is_none_or(|b| b == (living && self.w_flag(w, "flying")))
                    && chk("is_in_water", "in_water")
                    && opt_bool(v, "is_fall_flying").is_none_or(|b| !living || b == self.w_flag(w, "fall_flying"))
                    && opt_bool(v, "is_baby").is_none_or(|b| !living || b == self.w_flag(w, "baby"))
            }
            "minecraft:equipment" => {
                if !self.w_living(w) {
                    return false;
                }
                ["head", "chest", "legs", "feet", "body", "mainhand", "offhand"]
                    .iter()
                    .all(|s| v.get(*s).is_none_or(|ip| self.p_item(ip, self.w_equipment(w, s).as_ref())))
            }
            "minecraft:periodic_tick" => {
                let n = v.as_i64().unwrap_or(1).max(1);
                let tc = match self.w_mob(w) {
                    Some(m) => i64::from(m.tick_count),
                    None => self.tick as i64,
                };
                tc % n == 0
            }
            // W8c: direct vehicle; passenger: any one of the passengers.
            "minecraft:vehicle" => {
                let veh = match w {
                    Who::E(E::Player) => self.mobs.mounts.riding.vehicle.or(self.pcart.vehicle),
                    _ => self.w_mob(w).and_then(|m| m.mount.vehicle),
                };
                // G5-F06: the vehicle can be a minecart.
                self.p_entity(v, veh.map(|i| Who::E(self.vehicle_e(i))), origin)
            }
            "minecraft:passenger" => self.w_mob(w).is_some_and(|m| {
                m.mount.passengers.iter().any(|p| {
                    let e = match *p {
                        crate::mobs::mob::Ent::Player => E::Player,
                        crate::mobs::mob::Ent::Mob(i) => E::Mob(i),
                    };
                    self.p_entity(v, Some(Who::E(e)), origin)
                })
            }),
            "minecraft:targeted_entity" => match self.w_mob(w).and_then(|m| m.target) {
                Some(Ent::Player) => self.p_entity(v, Some(Who::E(E::Player)), origin),
                Some(Ent::Mob(i)) => self.p_entity(v, Some(Who::E(E::Mob(i))), origin),
                None => false,
            },
            "minecraft:team" => {
                let holder = match w {
                    Who::E(e) => self.ent_score_name(e),
                    Who::Mob(m) => crate::nbtio::uuid_text(m.uuid),
                    Who::Proj(p) => crate::nbtio::uuid_text(p.uuid),
                };
                self.cmd.scores.team_of(&holder).is_some_and(|t| Some(t.name.as_str()) == v.as_str())
            }
            "minecraft:slots" => {
                let Value::Object(slots) = v else { return true };
                slots.iter().all(|(range, ip)| self.w_slots(w, range).iter().any(|s| self.p_item(ip, s.as_ref())))
            }
            "minecraft:components" => {
                let Value::Object(want) = v else { return true };
                want.iter().all(|(k, val)| {
                    let k = crate::commands::args::id(k);
                    self.w_component(w, &k).is_some_and(|have| match (&have, val) {
                        (Value::String(a), Value::String(b)) => {
                            crate::commands::args::id(a) == crate::commands::args::id(b)
                        }
                        (a, b) => a == b,
                    })
                })
            }
            "minecraft:predicates" => {
                // Partial predicates evaluated on the value of the entity's component; only the
                // villager variant carries one (the others: presence).
                let Value::Object(want) = v else { return true };
                want.iter().all(|(k, pv)| {
                    let k = crate::commands::args::id(k);
                    let Some(have) = self.w_component(w, &k) else { return false };
                    match k.as_str() {
                        "minecraft:villager/variant" => {
                            have.as_str().is_some_and(|t| self.p_set("minecraft:villager_type", pv, t))
                        }
                        _ => true,
                    }
                })
            }
            "minecraft:entity_tags" => {
                let tags = match w {
                    Who::E(e) => self.ent_tags(e),
                    Who::Mob(_) | Who::Proj(_) => crate::nbtio::list(self.w_nbt(w).get("Tags"))
                        .iter()
                        .filter_map(|t| t.as_str().map(str::to_owned))
                        .collect(),
                };
                let has = |key: &str| -> Vec<String> {
                    v.get(key)
                        .and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect())
                        .unwrap_or_default()
                };
                let any = has("any_of");
                let all = has("all_of");
                let none = has("none_of");
                (v.get("any_of").is_none() || any.iter().any(|t| tags.contains(t)))
                    && (v.get("none_of").is_none() || !none.iter().any(|t| tags.contains(t)))
                    && all.iter().all(|t| tags.contains(t))
            }
            "minecraft:type_specific/player" => self.p_player(v, w),
            "minecraft:type_specific/cube_mob" => {
                let size = self.w_nbt(w).get("Size").and_then(Nbt::as_i64);
                let cube = matches!(self.w_type(w).as_str(), "minecraft:slime" | "minecraft:magma_cube");
                cube && ints(v.get("size"), size.unwrap_or(0) + 1)
            }
            "minecraft:type_specific/sheep" => match self.w_mob(w) {
                Some(m) if m.kind == crate::mobs::species::Kind::Sheep => {
                    opt_bool(v, "sheared").is_none_or(|b| b == m.sheared)
                }
                _ => false,
            },
            // Pillager: absent fields = false; current or active raid at its position; captain =
            // ominous banner on the head and patrol leader.
            "minecraft:type_specific/raider" => self.w_mob(w).is_some_and(|m| {
                crate::mobs::illagers::is_raider(m.kind)
                    && self.raider_has_raid(m) == opt_bool(v, "has_raid").unwrap_or(false)
                    && crate::mobs::illagers::is_captain(m) == opt_bool(v, "is_captain").unwrap_or(false)
            }),
            // Bobber: without `in_open_water`, true for any entity.
            "minecraft:type_specific/fishing_hook" => opt_bool(v, "in_open_water").is_none_or(|want| {
                let id = self.w_id(w);
                self.proj.hook.as_ref().is_some_and(|h| h.p.id == id && h.open_water == want)
            }),
            "minecraft:type_specific/lightning" => false,
            _ => false,
        }
    }

    /// Slots of a named range (`container.*`, `hotbar.*`, `armor.*`, `weapon.*`…).
    fn w_slots(&self, w: Who<'_>, range: &str) -> Vec<Option<Stack>> {
        let (base, idx) = match range.split_once('.') {
            Some((b, i)) => (b, i),
            None => (range, "*"),
        };
        let pick = |all: Vec<Option<Stack>>| -> Vec<Option<Stack>> {
            if idx == "*" {
                all
            } else {
                idx.parse::<usize>().ok().and_then(|i| all.get(i).cloned()).into_iter().collect()
            }
        };
        match (w, base) {
            (Who::E(E::Player), "container") => pick((0..36).map(|i| self.slots[inv_to_menu(i)].clone()).collect()),
            (Who::E(E::Player), "hotbar") => pick((0..9).map(|i| self.slots[36 + i].clone()).collect()),
            (Who::E(E::Player), "inventory") => pick((9..36).map(|i| self.slots[i].clone()).collect()),
            (Who::E(E::Player), "enderchest") => pick(self.life.vit.ender.clone()),
            (_, "weapon") => match idx {
                "mainhand" | "*" => vec![self.w_equipment(w, "mainhand")],
                "offhand" => vec![self.w_equipment(w, "offhand")],
                _ => Vec::new(),
            },
            (_, "armor") => match idx {
                "*" => ["feet", "legs", "chest", "head", "body"].iter().map(|s| self.w_equipment(w, s)).collect(),
                s => vec![self.w_equipment(w, s)],
            },
            _ => Vec::new(),
        }
    }

    /// Data component of an entity (variants), as JSON: read from its NBT.
    fn w_component(&self, w: Who<'_>, k: &str) -> Option<Value> {
        let n = self.w_nbt(w);
        let ty = self.w_type(w);
        let short = ty.trim_start_matches("minecraft:");
        let path = k.trim_start_matches("minecraft:");
        let (owner, field) = path.split_once('/')?;
        if owner != short && !(owner == "villager" && short == "zombie_villager") {
            return None;
        }
        let v = match field {
            "variant" => n
                .get("variant")
                .or_else(|| n.get("Variant"))
                .or_else(|| n.get("VillagerData").and_then(|d| d.get("type")))?,
            "color" => n.get("Color").or_else(|| n.get("CollarColor"))?,
            "collar" => n.get("CollarColor")?,
            "sound_variant" => n.get("sound_variant")?,
            other => n.get(other)?,
        };
        Some(match v {
            Nbt::String(s) => Value::String(s.clone()),
            o => match o.as_i64() {
                Some(i) if field == "color" || field == "collar" => {
                    Value::String(DYES.get(i as usize).copied().unwrap_or("white").to_owned())
                }
                Some(i) => Value::from(i),
                None => return None,
            },
        })
    }

    /// "Player" sub-predicate.
    fn p_player(&self, v: &Value, w: Who<'_>) -> bool {
        if !matches!(w, Who::E(E::Player)) {
            return false;
        }
        let vit = &self.life.vit;
        if !ints(v.get("level"), i64::from(vit.xp_level)) {
            return false;
        }
        if let Some(f) = v.get("food")
            && !(ints(f.get("level"), i64::from(vit.food)) && doubles(f.get("saturation"), f64::from(vit.saturation)))
        {
            return false;
        }
        if let Some(Value::Array(modes)) = v.get("gamemode") {
            let names = ["survival", "creative", "adventure", "spectator"];
            let cur = names.get(self.game_mode as usize).copied().unwrap_or("survival");
            if !modes.iter().any(|m| m.as_str() == Some(cur)) {
                return false;
            }
        }
        if let Some(Value::Array(stats)) = v.get("stats") {
            for s in stats {
                let ty = crate::commands::args::id(s["type"].as_str().unwrap_or(""));
                let key = crate::commands::args::id(s["stat"].as_str().unwrap_or(""));
                if !ints(s.get("value"), i64::from(self.stat_value(&ty, &key))) {
                    return false;
                }
            }
        }
        if let Some(Value::Object(recipes)) = v.get("recipes") {
            for (id, want) in recipes {
                if self.life.known.contains(&crate::commands::args::id(id)) != want.as_bool().unwrap_or(true) {
                    return false;
                }
            }
        }
        if let Some(Value::Object(advs)) = v.get("advancements") {
            for (id, want) in advs {
                if !self.adv_predicate(&crate::commands::args::id(id), want) {
                    return false;
                }
            }
        }
        if let Some(look) = v.get("looking_at") {
            match self.adv_looking_at() {
                Some(e) => {
                    if !self.p_entity(look, Some(Who::E(e)), Some(self.pos)) || !self.player_sees(e) {
                        return false;
                    }
                }
                None => return false,
            }
        }
        if let Some(inp) = v.get("input") {
            let k = self.prog.keys;
            let bits = [
                ("forward", 1),
                ("backward", 2),
                ("left", 4),
                ("right", 8),
                ("jump", 16),
                ("sneak", 32),
                ("sprint", 64),
            ];
            for (name, bit) in bits {
                if let Some(b) = opt_bool(inp, name)
                    && b != (k & bit != 0)
                {
                    return false;
                }
            }
        }
        true
    }

    /// Entity targeted by the player's gaze at 100 blocks (`looking_at`): nearest entry
    /// of the eye segment in a box (no margin; a box that already contains the eyes
    /// is not hit), spectators excluded (the player themselves, only player in the list).
    pub(crate) fn adv_looking_at(&self) -> Option<E> {
        let from = [self.pos[0], self.pos[1] + self.ent_eye_height(E::Player), self.pos[2]];
        let d = rc_physics::view_vector(self.pitch, self.yaw);
        let to = [from[0] + d[0] * 100.0, from[1] + d[1] * 100.0, from[2] + d[2] * 100.0];
        let mut best: Option<(f64, E)> = None;
        for e in self.all_ents() {
            if e == E::Player {
                continue;
            }
            if let Some((t, _)) = crate::projectiles::physics::aabb_clip(&self.e_bbox(e), from, to)
                && best.is_none_or(|(bt, _)| t < bt)
            {
                best = Some((t, e));
            }
        }
        best.map(|(_, e)| e)
    }

    /// Line of sight of the player to `e`: from their eyes to the target's eyes (column of their feet),
    /// "collision" blocks, fluids ignored; beyond 128 blocks, false.
    fn player_sees(&self, e: E) -> bool {
        let from = [self.pos[0], self.pos[1] + self.ent_eye_height(E::Player), self.pos[2]];
        let p = self.e_pos(e);
        let to = [p[0], p[1] + self.ent_eye_height(e), p[2]];
        let d2: f64 = (0..3).map(|k| (to[k] - from[k]) * (to[k] - from[k])).sum();
        d2.sqrt() <= 128.0 && !self.clip_blocks(from, to)
    }

    // ---- Damage -----------------------------------------------------------------------------

    /// Damage source predicate.
    pub(crate) fn p_damage_source(&self, p: &Value, d: &Dmg<'_>, origin: Option<[f64; 3]>) -> bool {
        if let Some(Value::Array(tags)) = p.get("tags") {
            for t in tags {
                let id = t["id"].as_str().unwrap_or("");
                let expected = t["expected"].as_bool().unwrap_or(true);
                let has = match id.strip_prefix('#') {
                    Some(tag) => self.p_tag("minecraft:damage_type", tag, &d.ty),
                    None => crate::commands::args::id(id) == d.ty,
                };
                if has != expected {
                    return false;
                }
            }
        }
        if let Some(de) = p.get("direct_entity")
            && !self.p_entity(de, d.direct, origin)
        {
            return false;
        }
        if let Some(se) = p.get("source_entity")
            && !self.p_entity(se, d.source, origin)
        {
            return false;
        }
        if let Some(b) = opt_bool(p, "is_direct") {
            let direct = match (d.source, d.direct) {
                (Some(a), Some(b)) => self.w_same(a, b),
                (None, None) => true,
                _ => false,
            };
            if b != direct {
                return false;
            }
        }
        true
    }

    fn w_id(&self, w: Who<'_>) -> i32 {
        match w {
            Who::E(e) => self.ent_id(e),
            Who::Mob(m) => m.id,
            Who::Proj(p) => p.id,
        }
    }

    fn w_same(&self, a: Who<'_>, b: Who<'_>) -> bool {
        self.w_id(a) == self.w_id(b)
    }

    // ---- Contextual conditions -----------------------------------------------------------

    /// "Contextual" condition of a criterion: object = entity predicate (wrapped in
    /// `entity_properties` on "this"), list = loot conditions all true.
    pub(crate) fn p_context(&self, v: &Value, ctx: &Ctx<'_>) -> bool {
        match v {
            Value::Null => true,
            Value::Array(list) => list.iter().all(|c| self.p_condition(c, ctx)),
            Value::Object(o) if !o.contains_key("type") && !o.contains_key("condition") => {
                self.p_entity(v, ctx.this, ctx.origin)
            }
            o => self.p_condition(o, ctx),
        }
    }

    /// Loot condition in a context.
    pub(crate) fn p_condition(&self, v: &Value, ctx: &Ctx<'_>) -> bool {
        if let Value::String(id) = v {
            return match self.life.pack.get("predicate", &crate::commands::args::id(id)) {
                Some(p) => self.p_condition(&p, ctx),
                None => false,
            };
        }
        if let Value::Array(a) = v {
            return a.iter().all(|c| self.p_condition(c, ctx));
        }
        let ty = v.get("type").or_else(|| v.get("condition")).and_then(Value::as_str).unwrap_or("");
        let target = |name: Option<&str>| -> Option<Who<'_>> {
            match name.unwrap_or("this") {
                "this" => ctx.this,
                "attacker" | "killer" => ctx.attacker,
                "direct_attacker" | "direct_killer" => ctx.direct_attacker,
                "attacking_player" | "killer_player" => ctx.attacking_player,
                "target_entity" => ctx.target,
                "interacting_entity" => ctx.interacting,
                _ => None,
            }
        };
        match crate::commands::args::id(ty).as_str() {
            "minecraft:inverted" => !self.p_condition(&v["term"], ctx),
            "minecraft:any_of" => v["terms"].as_array().is_some_and(|t| t.iter().any(|c| self.p_condition(c, ctx))),
            "minecraft:all_of" => v["terms"].as_array().is_none_or(|t| t.iter().all(|c| self.p_condition(c, ctx))),
            "minecraft:reference" => {
                let id = crate::commands::args::id(v["name"].as_str().unwrap_or(""));
                self.life.pack.get("predicate", &id).is_some_and(|p| self.p_condition(&p, ctx))
            }
            "minecraft:entity_properties" => {
                let who = target(v["entity"].as_str());
                match v.get("predicate") {
                    Some(p) => ctx.origin.is_some() && self.p_entity(p, who, ctx.origin),
                    None => true,
                }
            }
            "minecraft:location_check" => {
                let Some(o) = ctx.origin else { return false };
                // W8c: offsets at the condition's first level (`offsetX/Y/Z`).
                let off = |k: &str| v[k].as_i64().unwrap_or(0) as f64;
                match v.get("predicate") {
                    Some(p) => self.p_location(p, o[0] + off("offsetX"), o[1] + off("offsetY"), o[2] + off("offsetZ")),
                    None => true,
                }
            }
            "minecraft:killed_by_player" => ctx.attacking_player.is_some(),
            "minecraft:damage_source_properties" => match (&ctx.damage, ctx.origin) {
                (Some(d), Some(o)) => v.get("predicate").is_none_or(|p| self.p_damage_source(p, d, Some(o))),
                _ => false,
            },
            "minecraft:match_tool" => match ctx.tool {
                Some(t) => v.get("predicate").is_none_or(|p| self.p_item(p, t)),
                None => false,
            },
            "minecraft:match_block" => match ctx.block {
                Some(s) => {
                    let mut b = v.clone();
                    if let Some(o) = b.as_object_mut()
                        && let Some(bl) = o.remove("block")
                    {
                        o.insert("blocks".into(), bl);
                    }
                    if let Some(o) = b.as_object_mut()
                        && let Some(pr) = o.remove("properties")
                    {
                        o.insert("state".into(), pr);
                    }
                    self.p_block_state(&b, s)
                }
                None => false,
            },
            "minecraft:entity_scores" => {
                let Some(w) = target(v["entity"].as_str()) else { return false };
                let holder = match w {
                    Who::E(e) => self.ent_score_name(e),
                    Who::Mob(m) => crate::nbtio::uuid_text(m.uuid),
                    Who::Proj(p) => crate::nbtio::uuid_text(p.uuid),
                };
                let Some(Value::Object(scores)) = v.get("scores") else { return true };
                scores.iter().all(|(obj, range)| {
                    self.cmd.scores.objective(obj).is_some()
                        && self.cmd.scores.get(&holder, obj).is_some_and(|s| self.p_int_range(range, s))
                })
            }
            "minecraft:weather_check" => {
                let w = &self.life.weather;
                opt_bool(v, "raining").is_none_or(|b| b == w.raining)
                    && opt_bool(v, "thundering").is_none_or(|b| b == w.thundering)
            }
            "minecraft:time_check" => {
                let clock = crate::commands::args::id(v["clock"].as_str().unwrap_or("minecraft:overworld"));
                let mut t = self.clocks.iter().find(|c| c.0 == clock).map_or(self.time, |c| c.1);
                if let Some(p) = v["period"].as_i64()
                    && p != 0
                {
                    t %= p;
                }
                self.p_int_range(&v["value"], t as i32)
            }
            "minecraft:random_chance" => {
                let c = v["chance"].as_f64().unwrap_or(0.0) as f32;
                rand_float() < c
            }
            "minecraft:survives_explosion" => ctx.explosion.is_none_or(|r| rand_float() <= 1.0 / r),
            "minecraft:enchantment_active_check" => false,
            "minecraft:int_value_check" => {
                let n = v["value"].as_i64().unwrap_or(0) as i32;
                self.p_int_range(&v["test"], n)
            }
            "minecraft:float_value_check" => {
                let n = v["value"].as_f64().unwrap_or(0.0);
                doubles(v.get("test"), n)
            }
            "minecraft:table_bonus" | "minecraft:random_chance_with_enchanted_bonus" => false,
            _ => false,
        }
    }

    /// W8c: full evaluator of the draw-free loot conditions (location, entity
    /// properties, damage source…), lent to the loot tables.
    pub(crate) fn loot_world<'s>(&'s self, ctx: Ctx<'s>) -> impl Fn(&Value) -> bool + 's {
        move |v| self.p_condition(v, &ctx)
    }

    /// Integer range (`IntRange`: constant bounds or simple providers).
    fn p_int_range(&self, r: &Value, x: i32) -> bool {
        match r {
            Value::Number(n) => n.as_i64() == Some(i64::from(x)),
            Value::Object(o) => {
                let b = |k: &str| o.get(k).and_then(|v| v.as_f64().or_else(|| v.get("value").and_then(Value::as_f64)));
                b("min").is_none_or(|m| f64::from(x) >= m) && b("max").is_none_or(|m| f64::from(x) <= m)
            }
            _ => true,
        }
    }
}

/// Value of a state property: exact, or range `{min, max}` (integers compared as numbers).
fn prop_match(want: &Value, have: &str) -> bool {
    match want {
        Value::String(s) => s == have,
        Value::Bool(b) => b.to_string() == have,
        Value::Number(n) => n.as_i64().map(|i| i.to_string()).as_deref() == Some(have),
        Value::Object(r) => {
            let cmp = |x: &Value| -> Option<std::cmp::Ordering> {
                let s = x.as_str().map(str::to_owned).or_else(|| x.as_i64().map(|i| i.to_string()))?;
                match (have.parse::<i64>(), s.parse::<i64>()) {
                    (Ok(a), Ok(b)) => Some(a.cmp(&b)),
                    _ => Some(have.cmp(s.as_str())),
                }
            };
            r.get("min").is_none_or(|m| cmp(m).is_some_and(|o| o != std::cmp::Ordering::Less))
                && r.get("max").is_none_or(|m| cmp(m).is_some_and(|o| o != std::cmp::Ordering::Greater))
        }
        _ => false,
    }
}

/// Distance: deltas in single-precision floats, like the game.
pub fn p_distance(v: &Value, a: [f64; 3], b: [f64; 3]) -> bool {
    let xd = (a[0] - b[0]) as f32;
    let yd = (a[1] - b[1]) as f32;
    let zd = (a[2] - b[2]) as f32;
    doubles(v.get("x"), f64::from(xd.abs()))
        && doubles(v.get("y"), f64::from(yd.abs()))
        && doubles(v.get("z"), f64::from(zd.abs()))
        && doubles_sqr(v.get("horizontal"), f64::from(xd * xd + zd * zd))
        && doubles_sqr(v.get("absolute"), f64::from(xd * xd + yd * yd + zd * zd))
}

/// Inventory menu slot index for an inventory index (0–8 hotbar, 9–35 main inventory,
/// 36–39 feet→head, 40 offhand).
pub fn inv_to_menu(i: usize) -> usize {
    match i {
        0..=8 => 36 + i,
        9..=35 => i,
        36 => 8,
        37 => 7,
        38 => 6,
        39 => 5,
        40 => 45,
        _ => usize::MAX,
    }
}

/// Dye colors in the order of their id.
const DYES: [&str; 16] = [
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "light_gray",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
];

#[cfg(test)]
#[path = "tests_pred.rs"]
mod tests;

/// Draw of a random condition outside loot (criteria, `execute if predicate`).
fn rand_float() -> f32 {
    use std::cell::Cell;
    thread_local!(static S: Cell<u64> = Cell::new(0x2545_F491_4F6C_DD1D ^ std::process::id() as u64));
    S.with(|s| {
        let mut x = s.get();
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        s.set(x);
        (x >> 40) as f32 / (1u64 << 24) as f32
    })
}
