//! Server-side items (`server-gameplay.md` § 3, `inventories.md` § 2.1): default components
//! (reports of the official generator kept in the data cache), item tags, stack
//! rules (same item, same components, maximum size, wear).

use hashbrown::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};

use rc_data::VersionData;
use rc_proto::nbt::Nbt;
use serde_json::Value;

use crate::player::Stack;

/// Crafting remainders fixed by the game (item property, not a component).
const REMAINDERS: [(&str, &str); 5] = [
    ("minecraft:water_bucket", "minecraft:bucket"),
    ("minecraft:lava_bucket", "minecraft:bucket"),
    ("minecraft:milk_bucket", "minecraft:bucket"),
    ("minecraft:dragon_breath", "minecraft:glass_bottle"),
    ("minecraft:honey_bottle", "minecraft:glass_bottle"),
];

pub struct Items {
    pub data: Arc<VersionData>,
    pub names: Vec<String>,
    index: HashMap<String, i32>,
    /// Item tags: name (without `#`) → sorted identifiers.
    pub tags: HashMap<String, Vec<i32>>,
    /// Potion tags (registry identifiers, tag order), for the draws that pick a potion.
    pub potion_tags: Vec<(String, Vec<i32>)>,
}

/// Default component reports (one data set per process) and their cache, shared
/// so that writing a component can compare with the default without an item table.
/// Folder and cache under the same lock: a changed folder (reports produced after a first
/// world, other version) empties the cache, without letting in a value read from the old folder.
struct Reports {
    dir: Option<PathBuf>,
    cache: HashMap<String, Arc<Value>>,
}

static REPORTS: LazyLock<Mutex<Reports>> = LazyLock::new(|| Mutex::new(Reports { dir: None, cache: HashMap::new() }));

/// Folder of the reports of an item table; an absent folder keeps the previous one (another
/// world opened in the process may still use it) and will be searched again.
fn set_reports(dir: Option<PathBuf>) {
    REPORTS.lock().unwrap_or_else(|e| e.into_inner()).set(dir);
}

impl Reports {
    fn set(&mut self, dir: Option<PathBuf>) {
        if dir.is_some() && self.dir != dir {
            self.dir = dir;
            self.cache.clear();
        }
    }
}

/// Default components of an item (generator report), `Null` if unknown.
fn defaults_of(name: &str) -> Arc<Value> {
    let mut r = REPORTS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(v) = r.cache.get(name) {
        return v.clone();
    }
    // First access to this item: reading of the report under the lock (once per item).
    let path = name.strip_prefix("minecraft:").unwrap_or(name);
    let v = r
        .dir
        .as_ref()
        .and_then(|d| std::fs::read(d.join(format!("{path}.json"))).ok())
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .map(|v| v["components"].clone())
        .unwrap_or(Value::Null);
    let v = Arc::new(v);
    r.cache.insert(name.to_owned(), v.clone());
    v
}

/// Equality of two NBT ignoring the field order of the compounds.
pub fn nbt_eq(a: &Nbt, b: &Nbt) -> bool {
    match (a, b) {
        (Nbt::Compound(x), Nbt::Compound(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.iter().any(|(k2, v2)| k == k2 && nbt_eq(v, v2)))
        }
        (Nbt::List(x), Nbt::List(y)) => x.len() == y.len() && x.iter().zip(y).all(|(a, b)| nbt_eq(a, b)),
        // Reals compared like the game's tags: all NaNs equal to each other, 0.0 ≠ -0.0.
        (Nbt::Float(x), Nbt::Float(y)) => x.to_bits() == y.to_bits() || (x.is_nan() && y.is_nan()),
        (Nbt::Double(x), Nbt::Double(y)) => x.to_bits() == y.to_bits() || (x.is_nan() && y.is_nan()),
        _ => a == b,
    }
}

fn components_eq(a: &Option<Nbt>, b: &Option<Nbt>) -> bool {
    let empty = |c: &Option<Nbt>| match c {
        None => true,
        Some(Nbt::Compound(f)) => f.is_empty(),
        _ => false,
    };
    match (a, b) {
        _ if empty(a) && empty(b) => true,
        (Some(x), Some(y)) => nbt_eq(x, y),
        _ => false,
    }
}

/// Same item and same components.
pub fn same(a: &Stack, b: &Stack) -> bool {
    a.id == b.id && components_eq(&a.components, &b.components)
}

/// Identical stack (item, quantity, components).
pub fn matches(a: Option<&Stack>, b: Option<&Stack>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.count == b.count && same(a, b),
        _ => false,
    }
}

/// Component of a stack (disk NBT), full key `minecraft:…`.
pub fn component<'a>(s: &'a Stack, name: &str) -> Option<&'a Nbt> {
    s.components.as_ref()?.get(name)
}

/// Writes a component into the stack's patch: a value equal to the item's default removes
/// the entry (the patch only keeps the differences from the prototype); a `!name` removal is cancelled.
pub fn set_component(s: &mut Stack, name: &str, v: Nbt) {
    let default = &defaults_of(&s.id)[name];
    let c = s.components.get_or_insert_with(|| Nbt::Compound(Vec::new()));
    crate::nbtio::remove(c, &format!("!{name}"));
    if !default.is_null() && crate::progress::pred::json_eq_nbt(default, &v) {
        crate::nbtio::remove(c, name);
    } else {
        crate::nbtio::set(c, name, v);
    }
    if matches!(c, Nbt::Compound(f) if f.is_empty()) {
        s.components = None;
    }
}

/// Patch read (command, disk, network, block copy) brought down to only the differences from the
/// item's prototype; writes one by one go through `set_component`.
pub fn normalize_patch(s: &mut Stack) {
    let Some(Nbt::Compound(f)) = &mut s.components else { return };
    let defaults = defaults_of(&s.id);
    if defaults.is_null() {
        return;
    }
    f.retain(|(k, v)| patch_entry_kept(&defaults, k, v));
    if f.is_empty() {
        s.components = None;
    }
}

/// An entry the reduced patch keeps: removal of a component without a default has nothing to
/// remove, and a value equal to the default is useless.
fn patch_entry_kept(defaults: &Value, key: &str, v: &Nbt) -> bool {
    match key.strip_prefix('!') {
        Some(r) => !defaults[r].is_null(),
        None => defaults[key].is_null() || !crate::progress::pred::json_eq_nbt(&defaults[key], v),
    }
}

/// `normalize_patch` would leave these components of item `id` exactly as they are (nothing
/// dropped, not emptied into none), told without copying them.
pub fn patch_already_reduced(id: &str, components: &Nbt) -> bool {
    let Nbt::Compound(f) = components else { return true };
    let defaults = defaults_of(id);
    defaults.is_null() || (!f.is_empty() && f.iter().all(|(k, v)| patch_entry_kept(&defaults, k, v)))
}

/// Removal of a component through a patch (`!name`): recorded when the item carries it by
/// default, otherwise the entry simply goes (nothing to remove from the prototype).
pub fn unset_component(s: &mut Stack, name: &str) {
    let by_default = !defaults_of(&s.id)[name].is_null();
    let c = s.components.get_or_insert_with(|| Nbt::Compound(Vec::new()));
    crate::nbtio::remove(c, name);
    if by_default {
        crate::nbtio::set(c, &format!("!{name}"), Nbt::Compound(Vec::new()));
    }
    if matches!(c, Nbt::Compound(f) if f.is_empty()) {
        s.components = None;
    }
}

/// Effective value of a component: the patch's, nothing if the patch removes it, otherwise the
/// item's default (report read back as NBT).
pub fn effective_component(s: &Stack, name: &str) -> Option<Nbt> {
    if let Some(v) = component(s, name) {
        return Some(v.clone());
    }
    if component(s, &format!("!{name}")).is_some() {
        return None;
    }
    let d = &defaults_of(&s.id)[name];
    (!d.is_null()).then(|| crate::recipes::json_to_nbt(d))
}

pub fn remove_component(s: &mut Stack, name: &str) {
    if let Some(c) = &mut s.components {
        crate::nbtio::remove(c, name);
        if matches!(c, Nbt::Compound(f) if f.is_empty()) {
            s.components = None;
        }
    }
}

impl Items {
    pub fn new(data: Arc<VersionData>, tags: &crate::registries::Tags) -> Items {
        let names = data.registry("minecraft:item").map(|r| r.entries.clone()).unwrap_or_default();
        let index = names.iter().enumerate().map(|(i, n)| (n.clone(), i as i32)).collect();
        let reports = rc_data::cache_root()
            .join(&data.version)
            .join("generated")
            .join("reports")
            .join("minecraft")
            .join("components")
            .join("item");
        let reports = reports.is_dir().then_some(reports);
        if reports.is_none() {
            eprintln!("singleplayer: item component reports missing (fuels and food unknown)");
        }
        set_reports(reports);
        let mut items = Items { data, names, index, tags: HashMap::new(), potion_tags: Vec::new() };
        items.set_tags(tags);
        items
    }

    /// Item and potion tags taken from the registry tags (world opening, data reload).
    pub fn set_tags(&mut self, tags: &crate::registries::Tags) {
        let of_registry = |registry: &str| tags.iter().find(|(r, _)| r == registry).map(|(_, t)| t);
        self.tags = of_registry("minecraft:item")
            .map(|t| {
                t.iter()
                    .map(|(n, ids)| {
                        let mut v = ids.clone();
                        v.sort_unstable();
                        (n.clone(), v)
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.potion_tags = of_registry("minecraft:potion").cloned().unwrap_or_default();
    }

    pub fn id(&self, name: &str) -> Option<i32> {
        self.index.get(name).copied()
    }

    pub fn name(&self, id: i32) -> Option<&str> {
        self.names.get(usize::try_from(id).ok()?).map(String::as_str)
    }

    fn info(&self, name: &str) -> Option<&rc_data::ItemInfo> {
        self.data.items.get(self.id(name)? as usize)
    }

    /// Default components of an item (generator report), `Null` if unknown.
    pub fn defaults(&self, name: &str) -> Arc<Value> {
        defaults_of(name)
    }

    /// Default component (`minecraft:food`…) of the item, unless it is removed from the stack.
    pub fn default_component(&self, s: &Stack, name: &str) -> Option<Value> {
        if component(s, &format!("!{name}")).is_some() {
            return None;
        }
        let v = self.defaults(&s.id)[name].clone();
        (!v.is_null()).then_some(v)
    }

    /// Dye colour of the stack (`minecraft:dye` component, of the stack or by default).
    pub fn dye(&self, s: &Stack) -> Option<String> {
        component(s, "minecraft:dye")
            .and_then(Nbt::as_str)
            .map(str::to_owned)
            .or_else(|| self.default_component(s, "minecraft:dye").and_then(|v| v.as_str().map(str::to_owned)))
    }

    pub fn has_tag(&self, tag: &str, item: &str) -> bool {
        let tag = tag.strip_prefix('#').unwrap_or(tag);
        let Some(id) = self.id(item) else { return false };
        self.tags.get(tag).is_some_and(|v| v.binary_search(&id).is_ok())
    }

    /// Maximum size of a stack (`max_stack_size` component); a patch that removes it leaves
    /// the game's fallback, 1.
    pub fn max_stack(&self, s: &Stack) -> i32 {
        self.max_stack_of(&s.id, s.components.as_ref())
    }

    /// `max_stack` of item `id` with the patch `components`, without a stack to build.
    pub fn max_stack_of(&self, id: &str, components: Option<&Nbt>) -> i32 {
        // One pass over the patch for the value and the removal (called for every stack moved).
        if let Some(Nbt::Compound(f)) = components {
            for (k, v) in f {
                match k.as_str() {
                    "minecraft:max_stack_size" => {
                        if let Some(n) = v.as_i64() {
                            return n.clamp(1, 99) as i32;
                        }
                    }
                    "!minecraft:max_stack_size" => return 1,
                    _ => {}
                }
            }
        }
        self.info(id).map_or(64, |i| i32::from(i.max_stack_size.max(1)))
    }

    pub fn max_damage(&self, s: &Stack) -> i32 {
        if let Some(v) = component(s, "minecraft:max_damage").and_then(Nbt::as_i64) {
            return v as i32;
        }
        if component(s, "!minecraft:max_damage").is_some() {
            return 0;
        }
        self.info(&s.id).map_or(0, |i| i.max_damage as i32)
    }

    pub fn damage(&self, s: &Stack) -> i32 {
        component(s, "minecraft:damage").and_then(Nbt::as_i64).unwrap_or(0) as i32
    }

    /// Usable: durability, wear tracked, not unbreakable (§ 3.1). Wear tracked = a `damage`
    /// component, on the stack or by default (items with a durability carry one) and not removed.
    pub fn damageable(&self, s: &Stack) -> bool {
        if s.components.is_none() {
            return self.info(&s.id).is_some_and(|i| i.max_damage > 0);
        }
        self.max_damage(s) > 0
            && component(s, "minecraft:unbreakable").is_none()
            && (component(s, "minecraft:damage").is_some()
                || component(s, "!minecraft:damage").is_none() && self.info(&s.id).is_some_and(|i| i.max_damage > 0))
    }

    /// Equipment slot: 0 main hand, 1 feet, 2 legs, 3 chest, 4 head, 5 off hand,
    /// 6 body, 7 saddle; `None` without an `equippable` component (removed by the patch included).
    pub fn equip_slot(&self, s: &Stack) -> Option<u8> {
        if let Some(e) = component(s, "minecraft:equippable") {
            let slot = e.get("slot").and_then(Nbt::as_str)?;
            return ["mainhand", "feet", "legs", "chest", "head", "offhand", "body", "saddle"]
                .iter()
                .position(|n| *n == slot)
                .map(|i| i as u8);
        }
        if component(s, "!minecraft:equippable").is_some() {
            return None;
        }
        self.info(&s.id).and_then(|i| i.equip_slot.checked_sub(1))
    }

    pub fn remainder(&self, id: &str) -> Option<Stack> {
        REMAINDERS.iter().find(|(a, _)| *a == id).map(|(_, b)| Stack {
            id: (*b).to_owned(),
            count: 1,
            components: None,
        })
    }

    /// Level of an enchantment carried by the stack (`enchantments`).
    pub fn enchantment(&self, s: &Stack, ench: &str) -> i32 {
        let Some(e) = component(s, "minecraft:enchantments") else { return 0 };
        // 26.3 form: { id: level }; old form: { levels: { id: level } }.
        let map = e.get("levels").unwrap_or(e);
        map.get(ench).and_then(Nbt::as_i64).unwrap_or(0) as i32
    }

    /// Strict check of a stack after a component patch: not both damageable and stackable,
    /// quantity within the maximum, every stack held by a container, bundle or loaded
    /// crossbow within its own maximum, a bundle weight that can be computed.
    pub fn valid_strict(&self, s: &Stack) -> bool {
        if self.max_damage(s) > 0 && self.max_stack(s) > 1 {
            return false;
        }
        if s.count > self.max_stack(s) {
            return false;
        }
        let fits = |l: &[Nbt], item: fn(&Nbt) -> Option<&Nbt>| {
            l.iter().filter_map(item).filter_map(Stack::from_nbt).all(|x| x.count <= self.max_stack(&x))
        };
        if let Some(Nbt::List(l)) = effective_component(s, "minecraft:container").as_ref()
            && !fits(l, |e| e.get("item"))
        {
            return false;
        }
        for k in ["minecraft:bundle_contents", "minecraft:charged_projectiles"] {
            if let Some(Nbt::List(l)) = effective_component(s, k).as_ref()
                && !fits(l, |e| Some(e))
            {
                return false;
            }
        }
        crate::menus::bundle::contents(self, s).is_none() || crate::menus::bundle::weight(self, s).is_some()
    }

    /// `food` component of the stack (its patch, otherwise its item's): nutrition, saturation,
    /// can always be eaten.
    pub fn food(&self, s: &Stack) -> Option<(i32, f32, bool)> {
        let f = match component(s, "minecraft:food") {
            Some(patched) => crate::commands::cmds_s9b::nbt_to_json(patched),
            None => self.default_component(s, "minecraft:food")?,
        };
        Some((
            f["nutrition"].as_i64().unwrap_or(0) as i32,
            f["saturation"].as_f64().unwrap_or(0.0) as f32,
            f["can_always_eat"].as_bool().unwrap_or(false),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reports absent at the first world then produced: the second finds them, and the unknown defaults
    /// cached in the meantime are forgotten; a world without reports keeps the folder.
    #[test]
    fn replaceable_reports_folder() {
        let mut r = Reports { dir: None, cache: HashMap::new() };
        r.cache.insert("minecraft:stick".into(), Arc::new(Value::Null));
        r.set(None);
        assert_eq!(r.dir, None);
        let d = PathBuf::from("reports");
        r.set(Some(d.clone()));
        assert_eq!(r.dir.as_ref(), Some(&d));
        assert!(r.cache.is_empty(), "unknown default forgotten");
        r.cache.insert("minecraft:stick".into(), Arc::new(Value::Null));
        r.set(None);
        assert_eq!(r.dir.as_ref(), Some(&d), "folder kept");
        assert_eq!(r.cache.len(), 1, "same folder: cache kept");
    }
}
