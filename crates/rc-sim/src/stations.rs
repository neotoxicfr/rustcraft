//! Stations with a menu (S7, `mechanics-s7.md` § 2-4): brewing stand, enchanting table, anvil,
//! grindstone, smithing table, stonecutter. Slot layout, placement rules, result
//! computation, result taking, quick move, closing.
//! The generic menu (`menus.rs`) calls these functions; the effects on the world (levels,
//! sounds, anvil wear, orbs) go up to the server through `Env::station_event`.

use rc_proto::nbt::Nbt;
use serde_json::Value;

use crate::datapack::DataPack;
use crate::enchanting::{self, Enchants, TableView};
use crate::items::{Items, component, remove_component, same, set_component};
use crate::menus::{Env, Kind, Menu, Role, SlotDef, Src, place_back};
use crate::player::Stack;
use crate::recipes::{Ingredient, Recipes, ingredient, json_to_nbt};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum St {
    Brewing,
    Enchanting,
    Anvil,
    Grindstone,
    Smithing,
    Stonecutter,
    Loom,
    /// S8: cartography table.
    Cartography,
}

/// Effect of a station to be applied by the server.
#[derive(Debug, Clone, PartialEq)]
pub enum Ev {
    /// Experience levels removed (anvil).
    SpendLevels(i32),
    /// Anvil used: wear drawn (except creative), event 1029/1030.
    AnvilUsed,
    /// Grindstone: experience given back at the block's centre, event 1042.
    Grindstone(i32),
    /// Smithing: event 1044, recipe used.
    Smithed(Option<String>),
    /// Stonecutter: take sound (once per tick).
    StoneCut(Option<String>),
    /// Loom: take sound (once per tick).
    LoomTake,
    /// S8: cartography table, take sound (once per tick).
    CartographyTake,
}

/// State of an open station.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    /// Table: player's seed, bookshelves counted at opening, current view.
    pub seed: i32,
    pub bookcases: i32,
    pub view: TableView,
    /// Anvil: name typed, quantity of repair material, simple rename.
    pub name: Option<String>,
    pub repair_count: i32,
    pub only_renaming: bool,
    /// Stonecutter: tracked input item and recipes offered (indices into `Data::stonecut`).
    pub input: Option<String>,
    pub cut: Vec<usize>,
    /// Smithing, stonecutter: recipe of the current result.
    pub recipe: Option<String>,
    /// Stonecutter: tick of the last take sound.
    pub last_sound: u64,
    /// Loom: patterns offered.
    pub patterns: Vec<String>,
}

/// Brewing recipe (`brewing`): input and reagent (items, accepted potions), product.
#[derive(Debug, Clone)]
pub struct Brew {
    pub input: (Ingredient, Option<Vec<String>>),
    pub reagent: (Ingredient, Option<Vec<String>>),
    pub out: (String, i32, Option<Nbt>),
}

#[derive(Debug, Clone)]
pub enum SmithKind {
    Transform { id: String, count: i32, components: Option<Nbt> },
    Trim { pattern: String },
}

#[derive(Debug, Clone)]
pub struct Smith {
    pub id: String,
    pub template: Option<Ingredient>,
    pub base: Option<Ingredient>,
    pub addition: Option<Ingredient>,
    pub kind: SmithKind,
}

/// Station data, prepared when the world opens.
pub struct Data {
    pub ench: Enchants,
    /// Enchantment index → network identifier of the synchronised registry.
    pub ench_net: Vec<i32>,
    pub brewing: Vec<Brew>,
    pub brew_reagents: Vec<i32>,
    pub brew_inputs: Vec<i32>,
    pub smithing: Vec<Smith>,
    /// Smithing sets: template, base, addition.
    pub smith_sets: [Vec<i32>; 3],
    /// Stonecutter: (ingredient, result, recipe identifier), in the order of the recipes.
    pub stonecut: Vec<(Ingredient, Stack, String)>,
    /// Loom: banner pattern tags (identifier → patterns, file order).
    pub banner_tags: hashbrown::HashMap<String, Vec<String>>,
}

fn potions_of(v: &Value) -> Option<Vec<String>> {
    match &v["potion_contents"]["potions"] {
        Value::String(s) => Some(vec![s.clone()]),
        Value::Array(a) => Some(a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()),
        _ => None,
    }
}

fn union(sets: impl Iterator<Item = Vec<i32>>) -> Vec<i32> {
    let mut v: Vec<i32> = sets.flatten().collect();
    v.sort_unstable();
    v.dedup();
    v
}

impl Data {
    pub fn load(pack: &DataPack, items: &Items, recipes: &Recipes, ench_reg: &[String]) -> Data {
        let ench = Enchants::load(pack);
        let ench_net =
            ench.list.iter().map(|e| ench_reg.iter().position(|r| *r == e.id).map_or(-1, |i| i as i32)).collect();
        let mut brewing = Vec::new();
        let mut smithing = Vec::new();
        for (id, v) in pack.all("recipe") {
            let ty = v["type"].as_str().unwrap_or("").trim_start_matches("minecraft:");
            match ty {
                "brewing" => {
                    let (Some(i), Some(r)) =
                        (ingredient(items, &v["input"]["item"]), ingredient(items, &v["reagent"]["item"]))
                    else {
                        continue;
                    };
                    let o = &v["output"];
                    brewing.push(Brew {
                        input: (i, potions_of(&v["input"])),
                        reagent: (r, potions_of(&v["reagent"])),
                        out: (
                            o["id"].as_str().unwrap_or("").to_owned(),
                            o["count"].as_i64().unwrap_or(1) as i32,
                            o.get("components").filter(|c| c.is_object()).map(json_to_nbt),
                        ),
                    });
                }
                "smithing_transform" | "smithing_trim" => {
                    let opt = |k: &str| if v[k].is_null() { None } else { ingredient(items, &v[k]) };
                    let kind = if ty == "smithing_trim" {
                        SmithKind::Trim { pattern: v["pattern"].as_str().unwrap_or("").to_owned() }
                    } else {
                        SmithKind::Transform {
                            id: v["result"]["id"].as_str().unwrap_or("").to_owned(),
                            count: v["result"]["count"].as_i64().unwrap_or(1) as i32,
                            components: v["result"].get("components").filter(|c| c.is_object()).map(json_to_nbt),
                        }
                    };
                    smithing.push(Smith {
                        id,
                        template: opt("template"),
                        base: opt("base"),
                        addition: opt("addition"),
                        kind,
                    });
                }
                _ => {}
            }
        }
        let brew_reagents = union(brewing.iter().map(|b| b.reagent.0.clone()));
        let brew_inputs = union(brewing.iter().map(|b| b.input.0.clone()));
        let smith_sets = [
            union(smithing.iter().filter_map(|s| s.template.clone())),
            union(smithing.iter().filter_map(|s| s.base.clone())),
            union(smithing.iter().filter_map(|s| s.addition.clone())),
        ];
        let stonecut = recipes
            .list
            .iter()
            .filter_map(|r| match &r.body {
                crate::recipes::Body::Stonecut { input, result } => Some((input.clone(), result.stack(), r.id.clone())),
                _ => None,
            })
            .collect();
        let mut banner_tags = hashbrown::HashMap::new();
        for (id, v) in pack.all("tags/banner_pattern") {
            let l: Vec<String> = v["values"]
                .as_array()
                .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect())
                .unwrap_or_default();
            banner_tags.insert(id, l);
        }
        Data { ench, ench_net, brewing, brew_reagents, brew_inputs, smithing, smith_sets, stonecut, banner_tags }
    }

    /// Brewing recipe for a bottle and a reagent (first in the order of the recipes).
    pub fn brew(&self, items: &Items, bottle: &Stack, reagent: &Stack) -> Option<&Brew> {
        self.brewing.iter().find(|b| pot_test(items, &b.input, bottle) && pot_test(items, &b.reagent, reagent))
    }

    pub fn is_reagent(&self, items: &Items, s: &Stack) -> bool {
        items.id(&s.id).is_some_and(|i| self.brew_reagents.binary_search(&i).is_ok())
    }

    pub fn is_potion_input(&self, items: &Items, s: &Stack) -> bool {
        items.id(&s.id).is_some_and(|i| self.brew_inputs.binary_search(&i).is_ok())
            || items.has_tag("minecraft:brewing_potion_inputs", &s.id)
    }
}

fn has(items: &Items, ing: &[i32], s: &Stack) -> bool {
    items.id(&s.id).is_some_and(|i| ing.contains(&i))
}

fn pot_test(items: &Items, p: &(Ingredient, Option<Vec<String>>), s: &Stack) -> bool {
    if !has(items, &p.0, s) {
        return false;
    }
    match &p.1 {
        None => true,
        Some(list) => crate::potions::potion_of(s).is_some_and(|x| list.contains(&x)),
    }
}

/// Product of a brewing recipe.
pub fn brew_output(b: &Brew) -> Stack {
    Stack { id: b.out.0.clone(), count: b.out.1, components: b.out.2.clone() }
}

pub fn menu_type(st: St) -> i32 {
    match st {
        St::Anvil => 8,
        St::Brewing => 11,
        St::Enchanting => 13,
        St::Grindstone => 15,
        St::Smithing => 21,
        St::Stonecutter => 24,
        St::Loom => 18,
        St::Cartography => 23,
    }
}

/// Menu of a station: own slots then the player's inventory.
pub fn open(window: i32, st: St, own: Vec<Option<Stack>>) -> Menu {
    let (roles, data): (Vec<Role>, usize) = match st {
        St::Brewing => ((0..5).map(Role::Station).collect(), 4),
        St::Enchanting => (vec![Role::Station(0), Role::Station(1)], 10),
        St::Anvil => (vec![Role::Station(0), Role::Station(1), Role::StationResult], 1),
        St::Grindstone => (vec![Role::Station(0), Role::Station(1), Role::StationResult], 0),
        St::Smithing => (vec![Role::Station(0), Role::Station(1), Role::Station(2), Role::StationResult], 1),
        St::Stonecutter => (vec![Role::Station(0), Role::StationResult], 1),
        St::Loom => (vec![Role::Station(0), Role::Station(1), Role::Station(2), Role::StationResult], 1),
        St::Cartography => (vec![Role::Station(0), Role::Station(1), Role::StationResult], 0),
    };
    let n = roles.len();
    let mut slots: Vec<SlotDef> =
        roles.into_iter().enumerate().map(|(k, role)| SlotDef { src: Src::Own(k), role }).collect();
    Menu::player_slots(&mut slots);
    let mut m = Menu::base(window, Kind::Station(st), slots, n, data);
    for (k, s) in own.into_iter().enumerate().take(n) {
        m.own[k] = s;
    }
    if st == St::Enchanting {
        m.data[4..10].fill(-1);
    }
    if st == St::Stonecutter {
        m.data[0] = -1;
    }
    m
}

fn st_of(m: &Menu) -> Option<St> {
    match m.kind {
        Kind::Station(s) => Some(s),
        _ => None,
    }
}

/// Maximum size specific to an input slot.
pub fn max_slot(m: &Menu, k: u8) -> Option<i32> {
    match (st_of(m)?, k) {
        (St::Brewing, 0..=2) | (St::Enchanting, 0) => Some(1),
        _ => None,
    }
}

pub fn may_place(m: &Menu, env: &dyn Env, k: u8, s: &Stack) -> bool {
    let items = env.items();
    let Some(st) = st_of(m) else { return true };
    let d = env.st_data();
    match (st, k) {
        (St::Brewing, 0..=2) => d.is_some_and(|d| d.is_potion_input(items, s)),
        (St::Brewing, 3) => d.is_some_and(|d| d.is_reagent(items, s)),
        (St::Brewing, _) => {
            component(s, "minecraft:brewing_fuel").is_some()
                || items.default_component(s, "minecraft:brewing_fuel").is_some()
        }
        (St::Enchanting, 1) => s.id == "minecraft:lapis_lazuli",
        (St::Grindstone, _) => items.damageable(s) || enchanting::has_any(items, s),
        (St::Smithing, k @ 0..=2) => {
            d.is_some_and(|d| items.id(&s.id).is_some_and(|i| d.smith_sets[k as usize].binary_search(&i).is_ok()))
        }
        (St::Loom, 0) => is_banner(s),
        (St::Loom, 1) => is_loom_dye(items, s),
        (St::Loom, 2) => is_pattern_item(items, s),
        (St::Cartography, 0) => component(s, "minecraft:map_id").is_some(),
        (St::Cartography, 1) => matches!(s.id.as_str(), "minecraft:paper" | "minecraft:map" | "minecraft:glass_pane"),
        _ => true,
    }
}

fn is_banner(s: &Stack) -> bool {
    s.id.ends_with("_banner") && !s.id.ends_with("wall_banner")
}

fn is_loom_dye(items: &Items, s: &Stack) -> bool {
    items.has_tag("minecraft:loom_dyes", &s.id) && items.dye(s).is_some()
}

fn is_pattern_item(items: &Items, s: &Stack) -> bool {
    items.has_tag("minecraft:loom_patterns", &s.id) && provided_patterns(items, s).is_some()
}

fn provided_patterns(items: &Items, s: &Stack) -> Option<String> {
    component(s, "minecraft:provides_banner_patterns").and_then(Nbt::as_str).map(str::to_owned).or_else(|| {
        items.default_component(s, "minecraft:provides_banner_patterns").and_then(|v| v.as_str().map(str::to_owned))
    })
}

/// Patterns offered by the loom: without an item, those that ask for none; otherwise those of the item.
fn loom_patterns(items: &Items, d: &Data, pattern_item: Option<&Stack>) -> Vec<String> {
    let tag = match pattern_item {
        None => "minecraft:no_item_required".to_owned(),
        Some(s) => match provided_patterns(items, s) {
            Some(t) => t.trim_start_matches('#').to_owned(),
            None => return Vec::new(),
        },
    };
    d.banner_tags.get(&tag).cloned().unwrap_or_default()
}

fn loom_result(m: &mut Menu, env: &dyn Env, pattern: &str) {
    let items = env.items();
    let (Some(b), Some(dye)) = (&m.own[0], &m.own[1]) else {
        return;
    };
    let mut out = None;
    if let Some(color) = items.dye(dye) {
        let mut r = Stack { count: 1, ..b.clone() };
        let mut layers = match component(&r, "minecraft:banner_patterns") {
            Some(Nbt::List(l)) => l.clone(),
            _ => Vec::new(),
        };
        layers.push(Nbt::Compound(vec![
            ("pattern".into(), Nbt::String(pattern.to_owned())),
            ("color".into(), Nbt::String(color)),
        ]));
        set_component(&mut r, "minecraft:banner_patterns", Nbt::List(layers));
        out = Some(r);
    }
    if out != m.own[3] {
        m.own[3] = out;
    }
}

fn loom_changed(m: &mut Menu, env: &dyn Env) {
    let Some(d) = env.st_data() else { return };
    if m.own[0].is_some() && m.own[1].is_some() {
        let sel = m.data[0];
        let valid = sel >= 0 && (sel as usize) < m.st.patterns.len();
        let previous = std::mem::take(&mut m.st.patterns);
        m.st.patterns = loom_patterns(env.items(), d, m.own[2].as_ref());
        let show = if m.st.patterns.len() == 1 {
            m.data[0] = 0;
            Some(m.st.patterns[0].clone())
        } else if !valid {
            m.data[0] = -1;
            None
        } else {
            let value = previous[sel as usize].clone();
            match m.st.patterns.iter().position(|p| *p == value) {
                Some(k) => {
                    m.data[0] = k as i32;
                    Some(value)
                }
                None => {
                    m.data[0] = -1;
                    None
                }
            }
        };
        match show {
            Some(p) => {
                let n = match m.own[0].as_ref().and_then(|b| component(b, "minecraft:banner_patterns")) {
                    Some(Nbt::List(l)) => l.len(),
                    _ => 0,
                };
                if n >= 6 {
                    m.data[0] = -1;
                    m.own[3] = None;
                } else {
                    loom_result(m, env, &p);
                }
            }
            None => m.own[3] = None,
        }
    } else {
        m.own[3] = None;
        m.st.patterns.clear();
        m.data[0] = -1;
    }
    m.own_dirty = true;
}

/// S8: cartography table (setting up the result slot): enlarge (paper, scale
/// < 4, not locked), lock (glass pane), clone (blank map); the post-processing of the
/// taken map is done by the server.
fn cartography_changed(m: &mut Menu, env: &dyn Env) {
    let (map, add, result) = (m.own[0].clone(), m.own[1].clone(), m.own[2].clone());
    m.own_dirty = true;
    let (Some(map), Some(add)) = (map, add) else {
        if result.is_some() {
            m.own[2] = None;
        }
        return;
    };
    let Some(id) = component(&map, "minecraft:map_id").and_then(Nbt::as_i64) else { return };
    let Some((locked, scale)) = env.map_info(id as i32) else { return };
    let items = env.items();
    let out = if add.id == "minecraft:paper" && items.has_tag("minecraft:extendable_maps", &map.id) && !locked && scale < 4 {
        let mut r = Stack { count: 1, ..map.clone() };
        set_component(&mut r, "minecraft:map_post_processing", Nbt::Int(1));
        Some(r)
    } else if add.id == "minecraft:glass_pane" && !locked {
        let mut r = Stack { count: 1, ..map.clone() };
        set_component(&mut r, "minecraft:map_post_processing", Nbt::Int(0));
        Some(r)
    } else if add.id == "minecraft:map" {
        Some(Stack { count: 2, ..map.clone() })
    } else {
        None
    };
    if out != m.own[2] {
        m.own[2] = out;
    }
}

/// Loom button: pattern chosen, result placed.
pub fn loom_select(m: &mut Menu, env: &mut dyn Env, button: i32) -> bool {
    if button >= 0 && (button as usize) < m.st.patterns.len() {
        m.data[0] = button;
        let p = m.st.patterns[button as usize].clone();
        loom_result(m, env, &p);
        m.own_dirty = true;
        return true;
    }
    false
}

pub fn may_pickup(m: &Menu, env: &dyn Env) -> bool {
    match st_of(m) {
        Some(St::Anvil) => (env.creative() || env.xp_level() >= m.data[0]) && m.data[0] > 0,
        _ => true,
    }
}

/// Gathering (double click) ignores the result of the smithing table and the stonecutter.
pub fn no_pick_all(m: &Menu) -> bool {
    matches!(st_of(m), Some(St::Smithing | St::Stonecutter | St::Cartography))
}

/// The rest of a quick move of the result is thrown away (stonecutter).
pub fn drops_rest(m: &Menu) -> bool {
    st_of(m) == Some(St::Stonecutter)
}

/// After a click: inputs modified → result recomputed.
pub fn after_click(m: &mut Menu, env: &mut dyn Env, inv: &mut [Option<Stack>], before: &[Option<Stack>]) {
    let inputs = |o: &[Option<Stack>]| -> Vec<Option<Stack>> {
        match st_of(m) {
            Some(St::Anvil | St::Grindstone | St::Cartography) => o[..2.min(o.len())].to_vec(),
            Some(St::Smithing | St::Loom) => o[..3.min(o.len())].to_vec(),
            Some(St::Stonecutter) => o[..1.min(o.len())].to_vec(),
            _ => o.to_vec(),
        }
    };
    if inputs(&m.own) != inputs(before) {
        slots_changed(m, env, inv);
    }
}

/// Result recomputed.
pub fn slots_changed(m: &mut Menu, env: &mut dyn Env, inv: &mut [Option<Stack>]) {
    let _ = inv;
    m.own_dirty = true;
    match st_of(m) {
        Some(St::Enchanting) => {
            let Some(d) = env.st_data() else { return };
            let v = d.ench.table_view(env.items(), m.st.seed, m.st.bookcases, m.own[0].as_ref());
            m.st.view = v;
            for i in 0..3 {
                m.data[i] = v.costs[i];
                m.data[4 + i] =
                    if v.clue[i] >= 0 { d.ench_net.get(v.clue[i] as usize).copied().unwrap_or(-1) } else { -1 };
                m.data[7 + i] = v.level[i];
            }
            m.data[3] = m.st.seed;
        }
        Some(St::Anvil) => anvil_result(m, env),
        Some(St::Grindstone) => {
            m.own[2] = grindstone_result(env.items(), env.st_data(), m.own[0].as_ref(), m.own[1].as_ref());
        }
        Some(St::Smithing) => {
            smithing_result(m, env);
        }
        Some(St::Loom) => loom_changed(m, env),
        Some(St::Cartography) => cartography_changed(m, env),
        Some(St::Stonecutter) => {
            let cur = m.own[0].as_ref().map(|s| s.id.clone());
            if cur != m.st.input {
                m.st.input = cur;
                m.data[0] = -1;
                m.own[1] = None;
                m.st.cut = match (&m.own[0], env.st_data()) {
                    (Some(s), Some(d)) => {
                        (0..d.stonecut.len()).filter(|&i| has(env.items(), &d.stonecut[i].0, s)).collect()
                    }
                    _ => Vec::new(),
                };
            }
        }
        _ => {}
    }
}

/// Stonecutter button: recipe chosen, result placed.
pub fn stonecutter_select(m: &mut Menu, env: &mut dyn Env, button: i32) -> bool {
    if m.data[0] == button {
        return false;
    }
    if button >= 0 && (button as usize) < m.st.cut.len() {
        m.data[0] = button;
        stonecutter_result(m, env);
    }
    true
}

fn stonecutter_result(m: &mut Menu, env: &dyn Env) {
    let idx = m.data[0];
    let r = (idx >= 0 && !m.st.cut.is_empty())
        .then(|| m.st.cut.get(idx as usize).copied())
        .flatten()
        .and_then(|i| env.st_data().map(|d| d.stonecut[i].clone()));
    match r {
        Some((_, out, id)) => {
            m.own[1] = Some(out);
            m.st.recipe = Some(id);
        }
        None => {
            m.own[1] = None;
            m.st.recipe = None;
        }
    }
    m.own_dirty = true;
}

/// Taking the result.
pub fn on_take(m: &mut Menu, env: &mut dyn Env, inv: &mut [Option<Stack>], _taken: &Stack) {
    if matches!(st_of(m), Some(St::Smithing | St::Stonecutter)) {
        env.made(_taken, _taken.count); // S9: the reaction to crafting
    }
    match st_of(m) {
        Some(St::Anvil) => {
            if !env.creative() {
                env.station_event(Ev::SpendLevels(m.data[0]));
            }
            if m.st.repair_count > 0 {
                match &mut m.own[1] {
                    Some(a) if a.count > m.st.repair_count => a.count -= m.st.repair_count,
                    other => *other = None,
                }
            } else if !m.st.only_renaming {
                m.own[1] = None;
            }
            m.data[0] = 0;
            m.own[0] = None;
            env.station_event(Ev::AnvilUsed);
            slots_changed(m, env, inv);
        }
        Some(St::Grindstone) => {
            let xp = grindstone_xp(env, m.own[0].as_ref()) + grindstone_xp(env, m.own[1].as_ref());
            let xp = if xp > 0 {
                let half = (f64::from(xp) / 2.0).ceil() as i32;
                half + env.level_int(half)
            } else {
                0
            };
            env.station_event(Ev::Grindstone(xp));
            m.own[0] = None;
            m.own[1] = None;
            slots_changed(m, env, inv);
        }
        Some(St::Smithing) => {
            for k in 0..3 {
                if let Some(s) = &mut m.own[k] {
                    s.count -= 1;
                    if s.count <= 0 {
                        m.own[k] = None;
                    }
                }
            }
            env.station_event(Ev::Smithed(m.st.recipe.clone()));
            slots_changed(m, env, inv);
        }
        Some(St::Stonecutter) => {
            let recipe = m.st.recipe.clone();
            let had = m.own[0].is_some();
            if let Some(s) = &mut m.own[0] {
                s.count -= 1;
                if s.count <= 0 {
                    m.own[0] = None;
                }
            }
            slots_changed(m, env, inv);
            if had {
                stonecutter_result(m, env);
            }
            env.station_event(Ev::StoneCut(recipe));
        }
        Some(St::Loom) => {
            for k in [0, 1] {
                if let Some(s) = &mut m.own[k] {
                    s.count -= 1;
                    if s.count <= 0 {
                        m.own[k] = None;
                    }
                }
                loom_changed(m, env);
            }
            if m.own[0].is_none() || m.own[1].is_none() {
                m.data[0] = -1;
            }
            env.station_event(Ev::LoomTake);
        }
        // S8: a map and an addition consumed, take sound.
        Some(St::Cartography) => {
            for k in [0, 1] {
                if let Some(s) = &mut m.own[k] {
                    s.count -= 1;
                    if s.count <= 0 {
                        m.own[k] = None;
                    }
                }
                cartography_changed(m, env);
            }
            env.station_event(Ev::CartographyTake);
        }
        _ => {}
    }
    m.own_dirty = true;
}

fn grindstone_xp(env: &dyn Env, s: Option<&Stack>) -> i32 {
    let (Some(s), Some(d)) = (s, env.st_data()) else { return 0 };
    let mut n = 0;
    for (e, l) in enchanting::enchants_of(env.items(), s) {
        if !d.ench.is_curse(&e)
            && let Some(def) = d.ench.get(&e)
        {
            n += def.min_cost(l);
        }
    }
    n
}

/// Quick move specific to each station; true if something moved.
pub fn quick_move(m: &mut Menu, env: &mut dyn Env, inv: &mut [Option<Stack>], i: usize, stack: &mut Stack) -> bool {
    let Some(st) = st_of(m) else { return false };
    let n = m.len();
    let own = m.own.len();
    let (inv0, inv1, use1) = (own, own + 27, own + 36);
    let _ = n;
    match st {
        St::Brewing => {
            let d = env.st_data();
            let fuel = component(stack, "minecraft:brewing_fuel").is_some()
                || env.items().default_component(stack, "minecraft:brewing_fuel").is_some();
            let reagent = d.is_some_and(|d| d.is_reagent(env.items(), stack));
            let potion = d.is_some_and(|d| d.is_potion_input(env.items(), stack));
            if i >= 5 {
                if fuel {
                    // Fuel first: placed, the move stops there (no take); otherwise
                    // reagent if it is one.
                    if m.move_to(env, inv, stack, 4, 5, false) {
                        let rest = (stack.count > 0).then(|| stack.clone());
                        m.set(inv, i, rest);
                        return false;
                    }
                    !reagent || m.move_to(env, inv, stack, 3, 4, false)
                } else if reagent {
                    m.move_to(env, inv, stack, 3, 4, false)
                } else if potion {
                    m.move_to(env, inv, stack, 0, 3, false)
                } else if (inv0..inv1).contains(&i) {
                    m.move_to(env, inv, stack, inv1, use1, false)
                } else if (inv1..use1).contains(&i) {
                    m.move_to(env, inv, stack, inv0, inv1, false)
                } else {
                    m.move_to(env, inv, stack, inv0, use1, false)
                }
            } else {
                m.move_to(env, inv, stack, inv0, use1, true)
            }
        }
        St::Enchanting => {
            if i == 0 || i == 1 {
                m.move_to(env, inv, stack, inv0, use1, true)
            } else if stack.id == "minecraft:lapis_lazuli" {
                m.move_to(env, inv, stack, 1, 2, true)
            } else {
                if m.own[0].is_some() || !m.may_place(env, 0, stack) {
                    return false;
                }
                m.own[0] = Some(Stack { count: 1, ..stack.clone() });
                m.own_dirty = true;
                stack.count -= 1;
                // The choice by a player on the input: the table recomputes right away.
                slots_changed(m, env, inv);
                true
            }
        }
        St::Cartography => {
            if i == 2 {
                m.move_to(env, inv, stack, inv0, use1, true)
            } else if i > 2 {
                if component(stack, "minecraft:map_id").is_some() {
                    m.move_to(env, inv, stack, 0, 1, false)
                } else if matches!(stack.id.as_str(), "minecraft:paper" | "minecraft:map" | "minecraft:glass_pane") {
                    m.move_to(env, inv, stack, 1, 2, false)
                } else if (inv0..inv1).contains(&i) {
                    m.move_to(env, inv, stack, inv1, use1, false)
                } else {
                    (inv1..use1).contains(&i) && m.move_to(env, inv, stack, inv0, inv1, false)
                }
            } else {
                m.move_to(env, inv, stack, inv0, use1, false)
            }
        }
        St::Loom => {
            let items = env.items();
            if i == 3 {
                m.move_to(env, inv, stack, inv0, use1, true)
            } else if i > 3 {
                if is_banner(stack) {
                    m.move_to(env, inv, stack, 0, 1, false)
                } else if is_loom_dye(items, stack) {
                    m.move_to(env, inv, stack, 1, 2, false)
                } else if is_pattern_item(items, stack) {
                    m.move_to(env, inv, stack, 2, 3, false)
                } else if (inv0..inv1).contains(&i) {
                    m.move_to(env, inv, stack, inv1, use1, false)
                } else {
                    (inv1..use1).contains(&i) && m.move_to(env, inv, stack, inv0, inv1, false)
                }
            } else {
                m.move_to(env, inv, stack, inv0, use1, false)
            }
        }
        St::Anvil | St::Grindstone | St::Smithing | St::Stonecutter => {
            let result = own - 1;
            if i == result {
                m.move_to(env, inv, stack, inv0, use1, true)
            } else if i < result {
                m.move_to(env, inv, stack, inv0, use1, false)
            } else if st == St::Grindstone {
                if m.own[0].is_some() && m.own[1].is_some() {
                    if (inv0..inv1).contains(&i) {
                        m.move_to(env, inv, stack, inv1, use1, false)
                    } else {
                        (inv1..use1).contains(&i) && m.move_to(env, inv, stack, inv0, inv1, false)
                    }
                } else {
                    m.move_to(env, inv, stack, 0, 2, false)
                }
            } else if st == St::Stonecutter {
                let accepts =
                    env.st_data().is_some_and(|d| d.stonecut.iter().any(|(ing, ..)| has(env.items(), ing, stack)));
                if accepts {
                    m.move_to(env, inv, stack, 0, 1, false)
                } else if (inv0..inv1).contains(&i) {
                    m.move_to(env, inv, stack, inv1, use1, false)
                } else {
                    (inv1..use1).contains(&i) && m.move_to(env, inv, stack, inv0, inv1, false)
                }
            } else {
                let into_inputs = match st {
                    St::Smithing => smithing_accepts(m, env, stack),
                    _ => true,
                };
                if into_inputs {
                    m.move_to(env, inv, stack, 0, result, false)
                } else if (inv0..inv1).contains(&i) {
                    m.move_to(env, inv, stack, inv1, use1, false)
                } else {
                    (inv1..use1).contains(&i) && m.move_to(env, inv, stack, inv0, inv1, false)
                }
            }
        }
    }
}

fn smithing_accepts(m: &Menu, env: &dyn Env, s: &Stack) -> bool {
    let Some(d) = env.st_data() else { return false };
    let Some(id) = env.items().id(&s.id) else { return false };
    (0..3).any(|k| d.smith_sets[k].binary_search(&id).is_ok() && m.own[k].is_none())
}

/// Closing: inputs given back to the player (except the brewing stand, whose contents stay in the block).
pub fn close(m: &mut Menu, env: &mut dyn Env, inv: &mut [Option<Stack>]) {
    let Some(st) = st_of(m) else { return };
    if st == St::Brewing {
        return;
    }
    let inputs = match st {
        St::Enchanting => 2,
        St::Anvil | St::Grindstone => 2,
        St::Smithing => 3,
        St::Stonecutter => 1,
        St::Loom => 3,
        St::Cartography => 2,
        St::Brewing => 0,
    };
    let n = m.own.len();
    for k in inputs..n {
        m.own[k] = None;
    }
    for k in 0..inputs {
        if let Some(s) = m.own[k].take() {
            place_back(inv, env, s);
        }
    }
    m.own_dirty = true;
}

// --- Anvil ---

fn repair_cost(items: &Items, s: Option<&Stack>) -> i32 {
    let Some(s) = s else { return 0 };
    component(s, "minecraft:repair_cost")
        .and_then(Nbt::as_i64)
        .or_else(|| items.default_component(s, "minecraft:repair_cost").and_then(|v| v.as_i64()))
        .unwrap_or(0) as i32
}

fn set_damage(items: &Items, s: &mut Stack, d: i32) {
    let max = items.max_damage(s);
    crate::lootfn::set_damage(s, d.clamp(0, max.max(0)));
}

/// Valid repair material (`repairable` of the item).
pub(crate) fn valid_repair(items: &Items, input: &Stack, add: &Stack) -> bool {
    let Some(r) = items.default_component(input, "minecraft:repairable") else { return false };
    match &r["items"] {
        Value::String(t) => match t.strip_prefix('#') {
            Some(tag) => items.has_tag(tag, &add.id),
            None => *t == add.id,
        },
        Value::Array(a) => a.iter().any(|x| x.as_str() == Some(add.id.as_str())),
        _ => false,
    }
}

/// Displayed name (server English): custom name, otherwise item name.
pub fn hover_name(items: &Items, s: &Stack) -> String {
    let text = component(s, "minecraft:custom_name")
        .cloned()
        .or_else(|| component(s, "minecraft:item_name").cloned())
        .or_else(|| items.default_component(s, "minecraft:item_name").map(|v| json_to_nbt(&v)));
    let Some(c) = text.as_ref().and_then(rc_text::from_nbt) else { return s.id.clone() };
    rc_text::plain_string(&c, en_us(items))
}

pub(crate) fn en_us(items: &Items) -> &'static rc_text::Lang {
    static LANG: std::sync::OnceLock<rc_text::Lang> = std::sync::OnceLock::new();
    LANG.get_or_init(|| {
        let bytes = crate::registries::client_jar(&items.data).and_then(|p| {
            // Shared jar index (already built when the world opens); `RUSTCRAFT_OPT_JARINDEX=0`:
            // the jar is reopened.
            if rc_data::jar::enabled() {
                return Some(rc_data::jar::index(&p).ok()?.get(rc_data::jar::EN_US)?.as_bytes().to_vec());
            }
            let f = std::fs::File::open(p).ok()?;
            let mut z = zip::ZipArchive::new(f).ok()?;
            let mut e = z.by_name("assets/minecraft/lang/en_us.json").ok()?;
            let mut v = Vec::new();
            std::io::Read::read_to_end(&mut e, &mut v).ok()?;
            Some(v)
        });
        rc_text::Lang::load(bytes.as_deref().unwrap_or(b"{}"), None, None)
    })
}

/// Filtering of the typed name: control characters and `§` removed.
pub fn filter_name(s: &str) -> String {
    s.chars().filter(|c| *c != '\u{a7}' && *c >= ' ' && *c != '\u{7f}').collect()
}

fn anvil_result(m: &mut Menu, env: &mut dyn Env) {
    let items = env.items();
    let creative = env.creative();
    let Some(d) = env.st_data() else { return };
    let input = m.own[0].clone();
    m.st.only_renaming = false;
    m.data[0] = 1;
    let mut price: i64 = 0;
    let mut naming = 0;
    let Some(input) = input.filter(|s| enchanting::can_store(items, s)) else {
        m.own[2] = None;
        m.data[0] = 0;
        return;
    };
    let mut result = input.clone();
    let addition = m.own[1].clone();
    let mut ench = enchanting::enchants_of(items, &result);
    let tax = i64::from(repair_cost(items, Some(&input))) + i64::from(repair_cost(items, addition.as_ref()));
    m.st.repair_count = 0;
    let fail = |m: &mut Menu| {
        m.own[2] = None;
        m.data[0] = 0;
    };
    if let Some(add) = &addition {
        let using_book = component(add, "minecraft:stored_enchantments").is_some()
            || (component(add, "!minecraft:stored_enchantments").is_none()
                && items.default_component(add, "minecraft:stored_enchantments").is_some());
        if items.damageable(&result) && valid_repair(items, &input, add) {
            let mut amount = items.damage(&result).min(items.max_damage(&result) / 4);
            if amount <= 0 {
                fail(m);
                return;
            }
            let mut count = 0;
            while amount > 0 && count < add.count {
                let nd = items.damage(&result) - amount;
                set_damage(items, &mut result, nd);
                price += 1;
                amount = items.damage(&result).min(items.max_damage(&result) / 4);
                count += 1;
            }
            m.st.repair_count = count;
        } else {
            if !using_book && (result.id != add.id || !items.damageable(&result)) {
                fail(m);
                return;
            }
            if items.damageable(&result) && !using_book {
                let r1 = items.max_damage(&input) - items.damage(&input);
                let r2 = items.max_damage(add) - items.damage(add);
                let additional = r2 + items.max_damage(&result) * 12 / 100;
                let remaining = r1 + additional;
                let rd = (items.max_damage(&result) - remaining).max(0);
                if rd < items.damage(&result) {
                    set_damage(items, &mut result, rd);
                    price += 2;
                }
            }
            let mut any_ok = false;
            let mut any_bad = false;
            for (e, lvl) in enchanting::enchants_of(items, add) {
                let Some(def) = d.ench.get(&e) else { continue };
                let current = ench.iter().find(|x| x.0 == e).map_or(0, |x| x.1);
                let mut level = if current == lvl { lvl + 1 } else { lvl.max(current) };
                let mut compatible = d.ench.supports(items, def, &input.id);
                if creative || input.id == "minecraft:enchanted_book" {
                    compatible = true;
                }
                for (other, _) in &ench {
                    if *other != e && !d.ench.compatible(&e, other) {
                        compatible = false;
                        price += 1;
                    }
                }
                if !compatible {
                    any_bad = true;
                } else {
                    any_ok = true;
                    level = level.min(def.max_level);
                    match ench.iter_mut().find(|x| x.0 == e) {
                        Some(x) => x.1 = level,
                        None => ench.push((e.clone(), level)),
                    }
                    let mut fee = def.anvil_cost;
                    if using_book {
                        fee = (fee / 2).max(1);
                    }
                    price += i64::from(fee * level);
                    if input.count > 1 {
                        price = 40;
                    }
                }
            }
            if any_bad && !any_ok {
                fail(m);
                return;
            }
        }
    }
    match &m.st.name {
        Some(n) if !n.trim().is_empty() => {
            if *n != hover_name(items, &input) {
                naming = 1;
                price += 1;
                set_component(&mut result, "minecraft:custom_name", Nbt::String(n.clone()));
            }
        }
        _ => {
            if component(&input, "minecraft:custom_name").is_some() {
                naming = 1;
                price += 1;
                remove_component(&mut result, "minecraft:custom_name");
            }
        }
    }
    let final_price = if price <= 0 { 0 } else { (tax + price).clamp(0, i64::from(i32::MAX)) as i32 };
    m.data[0] = final_price;
    let mut out = (price > 0).then_some(result);
    if i64::from(naming) == price && naming > 0 {
        if m.data[0] >= 40 {
            m.data[0] = 39;
        }
        m.st.only_renaming = true;
    }
    if m.data[0] >= 40 && !creative {
        out = None;
    }
    if let Some(r) = &mut out {
        let mut base = repair_cost(items, Some(r)).max(repair_cost(items, addition.as_ref()));
        if i64::from(naming) != price || naming == 0 {
            base = (i64::from(base) * 2 + 1).min(i64::from(i32::MAX)) as i32;
        }
        if base == 0 {
            remove_component(r, "minecraft:repair_cost");
        } else {
            set_component(r, "minecraft:repair_cost", Nbt::Int(base));
        }
        enchanting::set_enchants(items, r, &ench);
    }
    m.own[2] = out;
}

/// Name typed in the anvil (`rename_item`): result renamed and recomputed.
pub fn anvil_rename(m: &mut Menu, env: &mut dyn Env, inv: &mut [Option<Stack>], name: &str) -> bool {
    let v = filter_name(name);
    if v.chars().count() > 50 || m.st.name.as_deref() == Some(v.as_str()) {
        return false;
    }
    m.st.name = Some(v);
    slots_changed(m, env, inv);
    true
}

// --- Grindstone ---

fn remove_non_curses(items: &Items, d: Option<&Data>, mut s: Stack) -> Stack {
    let list: Vec<(String, i32)> =
        enchanting::enchants_of(items, &s).into_iter().filter(|(e, _)| d.is_some_and(|d| d.ench.is_curse(e))).collect();
    enchanting::set_enchants(items, &mut s, &list);
    if s.id == "minecraft:enchanted_book" && list.is_empty() {
        s.id = "minecraft:book".into();
        remove_component(&mut s, "minecraft:stored_enchantments");
    }
    let mut cost = 0i64;
    for _ in 0..list.len() {
        cost = (cost * 2 + 1).min(i64::from(i32::MAX));
    }
    if cost == 0 {
        remove_component(&mut s, "minecraft:repair_cost");
    } else {
        set_component(&mut s, "minecraft:repair_cost", Nbt::Int(cost as i32));
    }
    s
}

pub fn grindstone_result(items: &Items, d: Option<&Data>, a: Option<&Stack>, b: Option<&Stack>) -> Option<Stack> {
    if a.is_none() && b.is_none() {
        return None;
    }
    if a.map_or(0, |s| s.count) > 1 || b.map_or(0, |s| s.count) > 1 {
        return None;
    }
    match (a, b) {
        (Some(x), None) | (None, Some(x)) => {
            enchanting::has_any(items, x).then(|| remove_non_curses(items, d, x.clone()))
        }
        (Some(i), Some(a)) => {
            if i.id != a.id {
                return None;
            }
            let durability = items.max_damage(i).max(items.max_damage(a));
            let r1 = items.max_damage(i) - items.damage(i);
            let r2 = items.max_damage(a) - items.damage(a);
            let remaining = r1 + r2 + durability * 5 / 100;
            let mut count = 1;
            if !items.damageable(i) {
                if items.max_stack(i) < 2 || !crate::items::matches(Some(i), Some(a)) {
                    return None;
                }
                count = 2;
            }
            let mut n = Stack { count, ..i.clone() };
            if items.damageable(&n) {
                if durability == items.max_damage(&Stack { components: None, ..n.clone() }) {
                    remove_component(&mut n, "minecraft:max_damage");
                } else {
                    set_component(&mut n, "minecraft:max_damage", Nbt::Int(durability));
                }
                crate::lootfn::set_damage(&mut n, (durability - remaining).max(0));
            }
            let mut list = enchanting::enchants_of(items, &n);
            for (e, l) in enchanting::enchants_of(items, a) {
                let cur = list.iter().find(|x| x.0 == e).map_or(0, |x| x.1);
                if !d.is_some_and(|d| d.ench.is_curse(&e)) || cur == 0 {
                    match list.iter_mut().find(|x| x.0 == e) {
                        Some(x) => x.1 = x.1.max(l),
                        None => list.push((e, l)),
                    }
                }
            }
            enchanting::set_enchants(items, &mut n, &list);
            Some(remove_non_curses(items, d, n))
        }
        (None, None) => None,
    }
}

// --- Smithing ---

fn test_opt(items: &Items, ing: &Option<Ingredient>, s: Option<&Stack>) -> bool {
    match (ing, s) {
        (None, s) => s.is_none(),
        (Some(i), Some(s)) => has(items, i, s),
        (Some(_), None) => false,
    }
}

fn smithing_result(m: &mut Menu, env: &mut dyn Env) {
    let items = env.items();
    let found = env.st_data().and_then(|d| {
        d.smithing.iter().find(|r| {
            test_opt(items, &r.template, m.own[0].as_ref())
                && test_opt(items, &r.base, m.own[1].as_ref())
                && test_opt(items, &r.addition, m.own[2].as_ref())
        })
    });
    let (out, id) = match found {
        None => (None, None),
        Some(r) => {
            let out = match &r.kind {
                SmithKind::Transform { id, count, components } => m.own[1].as_ref().map(|base| {
                    let mut out = Stack { id: id.clone(), count: *count, components: base.components.clone() };
                    if let Some(Nbt::Compound(f)) = components {
                        for (k, v) in f {
                            set_component(&mut out, k, v.clone());
                        }
                    }
                    out
                }),
                SmithKind::Trim { pattern } => {
                    let material = m.own[2].as_ref().and_then(|a| {
                        component(a, "minecraft:provides_trim_material")
                            .and_then(Nbt::as_str)
                            .map(str::to_owned)
                            .or_else(|| {
                                items
                                    .default_component(a, "minecraft:provides_trim_material")
                                    .and_then(|v| v.as_str().map(str::to_owned))
                            })
                    });
                    match (material, m.own[1].as_ref()) {
                        (Some(mat), Some(base)) => {
                            let trim = Nbt::Compound(vec![
                                ("material".into(), Nbt::String(mat.clone())),
                                ("pattern".into(), Nbt::String(pattern.clone())),
                            ]);
                            let same_trim = component(base, "minecraft:trim").is_some_and(|t| {
                                t.get("material").and_then(Nbt::as_str) == Some(mat.as_str())
                                    && t.get("pattern").and_then(Nbt::as_str) == Some(pattern.as_str())
                            });
                            (!same_trim).then(|| {
                                let mut o = Stack { count: 1, ..base.clone() };
                                set_component(&mut o, "minecraft:trim", trim);
                                o
                            })
                        }
                        _ => None,
                    }
                }
            };
            (out, Some(r.id.clone()))
        }
    };
    m.st.recipe = id;
    m.own[3] = out;
    let err = m.own[0].is_some() && m.own[1].is_some() && m.own[2].is_some() && m.own[3].is_none();
    m.data[0] = i32::from(err);
}

/// Two identical stacks (for the tests).
pub fn same_stack(a: &Stack, b: &Stack) -> bool {
    same(a, b) && a.count == b.count
}
