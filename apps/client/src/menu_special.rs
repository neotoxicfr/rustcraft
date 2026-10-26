//! Special screen menus (`docs/specs/special-screens.md`, `inventories.md` § 5.8–5.9):
//! slot layout, acceptance and quick move. The rest (clicks, drag,
//! gathering) follows the common logic of `menu.rs`.

use rc_proto_v777::item::ItemStack;

use crate::menu::{Accept, Menu, Rules, SlotDef};

/// Kind of a mount (screen textures and sprites).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountKind {
    Horse,
    Llama,
    Nautilus,
}

/// Mount: active slots and chest columns (§ 2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mount {
    pub kind: MountKind,
    pub saddle: bool,
    pub body: bool,
    pub cols: u8,
}

/// Special screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sp {
    Merchant,
    Mount(Mount),
    Grindstone,
    Stonecutter,
    Loom,
    Cartography,
    Smithing,
    Enchant,
    Anvil,
    Beacon,
    Crafter,
    Lectern,
    Shulker,
}

/// Recipe data and item tags received from the server (prediction and lists).
#[derive(Default)]
pub struct Known {
    /// Item tags (`update_tags`), with the `minecraft:` namespace kept.
    pub item_tags: hashbrown::HashMap<String, Vec<i32>>,
    /// Tags of the `banner_pattern` registry (received order).
    pub pattern_tags: hashbrown::HashMap<String, Vec<i32>>,
    /// Item sets of the recipes (`smithing_template`…).
    pub sets: hashbrown::HashMap<String, hashbrown::HashSet<i32>>,
    /// Stonecutter recipes: accepted items (`None` = unknown tag), displayed result.
    pub stonecutter: Vec<(Vec<i32>, Option<ItemStack>)>,
    raw_stonecutter: Vec<(rc_proto_v777::special::Holders, Option<ItemStack>)>,
}

pub static KNOWN: std::sync::RwLock<Option<Known>> = std::sync::RwLock::new(None);

fn resolve(k: &mut Known) {
    use rc_proto_v777::special::Holders;
    k.stonecutter = k
        .raw_stonecutter
        .iter()
        .map(|(h, s)| {
            let ids = match h {
                Holders::Items(v) => v.clone(),
                Holders::Tag(t) => {
                    let t = if t.contains(':') { t.clone() } else { format!("minecraft:{t}") };
                    k.item_tags.get(&t).cloned().unwrap_or_default()
                }
            };
            (ids, s.clone())
        })
        .collect();
}

pub fn set_tags(tags: &rc_proto_v777::play::Tags) {
    let named = |r: &str| {
        tags.iter().find(|(n, _)| n == r).map(|(_, l)| {
            l.iter()
                .map(|(n, v)| (if n.contains(':') { n.clone() } else { format!("minecraft:{n}") }, v.clone()))
                .collect::<hashbrown::HashMap<_, _>>()
        })
    };
    let mut g = KNOWN.write().unwrap_or_else(|p| p.into_inner());
    let k = g.get_or_insert_with(Default::default);
    if let Some(t) = named("minecraft:item") {
        k.item_tags = t;
    }
    if let Some(t) = named("minecraft:banner_pattern") {
        k.pattern_tags = t;
    }
    resolve(k);
}

/// Patterns (registry indices) of a `banner_pattern` tag, in received order.
pub fn pattern_tag(tag: &str) -> Vec<usize> {
    let g = KNOWN.read().unwrap_or_else(|p| p.into_inner());
    g.as_ref()
        .and_then(|k| k.pattern_tags.get(tag))
        .map_or_else(Vec::new, |v| v.iter().filter_map(|&i| usize::try_from(i).ok()).collect())
}

pub fn set_recipes(r: rc_proto_v777::special::Recipes) {
    let mut g = KNOWN.write().unwrap_or_else(|p| p.into_inner());
    let k = g.get_or_insert_with(Default::default);
    k.sets = r.sets.into_iter().map(|(n, v)| (n, v.into_iter().collect())).collect();
    k.raw_stonecutter = r.stonecutter;
    resolve(k);
}

/// Item in a recipe set (`None`: unknown set).
fn in_set(name: &str, item: i32) -> Option<bool> {
    let g = KNOWN.read().unwrap_or_else(|p| p.into_inner());
    let set = g.as_ref()?.sets.get(name)?;
    Some(set.contains(&item))
}

/// Item in an item tag (`None`: unknown tag).
pub fn in_tag(tag: &str, item: i32) -> Option<bool> {
    let g = KNOWN.read().unwrap_or_else(|p| p.into_inner());
    Some(g.as_ref()?.item_tags.get(tag)?.contains(&item))
}

/// Stonecutter recipes accepting this item, in received order: displayed result item.
pub fn stonecutter_for(item: i32) -> Vec<Option<ItemStack>> {
    let g = KNOWN.read().unwrap_or_else(|p| p.into_inner());
    let Some(k) = g.as_ref() else { return Vec::new() };
    k.stonecutter.iter().filter(|(ids, _)| ids.contains(&item)).map(|(_, s)| s.clone()).collect()
}

/// `menu` registry type → special screen.
pub fn from_registry(id: i32) -> Option<Sp> {
    Some(match id {
        7 => Sp::Crafter,
        8 => Sp::Anvil,
        9 => Sp::Beacon,
        13 => Sp::Enchant,
        15 => Sp::Grindstone,
        17 => Sp::Lectern,
        18 => Sp::Loom,
        19 => Sp::Merchant,
        20 => Sp::Shulker,
        21 => Sp::Smithing,
        23 => Sp::Cartography,
        24 => Sp::Stonecutter,
        _ => return None,
    })
}

/// Slots specific to the container (before the inventory; crafter: grid + result).
pub fn container_size(s: Sp) -> usize {
    match s {
        Sp::Merchant | Sp::Grindstone | Sp::Cartography | Sp::Anvil => 3,
        Sp::Mount(m) => 2 + 3 * usize::from(m.cols),
        Sp::Stonecutter | Sp::Enchant => 2,
        Sp::Loom | Sp::Smithing => 4,
        Sp::Beacon | Sp::Lectern => 1,
        Sp::Crafter => 10,
        Sp::Shulker => 27,
    }
}

/// Total number of slots of the menu.
pub fn total(s: Sp) -> usize {
    match s {
        Sp::Lectern => 1,
        _ => container_size(s) + 36,
    }
}

/// First slot of the player part (36 slots).
pub fn player_start(s: Sp, n: usize) -> usize {
    match s {
        Sp::Crafter => 9,
        Sp::Lectern => n,
        _ => n.saturating_sub(36),
    }
}

/// Size of the image.
pub fn image_size(s: Sp) -> (i32, i32) {
    match s {
        Sp::Merchant => (276, 166),
        Sp::Beacon => (230, 219),
        _ => (176, 166),
    }
}

/// Own acceptance codes (`Accept::Special`).
pub mod accept {
    pub const LAPIS: u8 = 1;
    pub const GRIND: u8 = 2;
    pub const BANNER: u8 = 3;
    pub const DYE: u8 = 4;
    pub const PATTERN: u8 = 5;
    pub const MAP: u8 = 6;
    pub const MAP_ADD: u8 = 7;
    pub const TEMPLATE: u8 = 8;
    pub const SMITH_BASE: u8 = 9;
    pub const SMITH_ADD: u8 = 10;
    pub const BEACON_PAY: u8 = 11;
    pub const NOT_SHULKER: u8 = 12;
}

/// True if the item fits in a slot of code `code` (prediction; the server corrects).
pub fn accepts(code: u8, rules: &dyn Rules, s: &ItemStack) -> bool {
    use accept::*;
    let n = rules.item_name(s);
    let ends = |e: &[&str]| e.iter().any(|x| n.ends_with(x));
    // Sets and tags received from the server first; names after (approximate).
    let known = match code {
        TEMPLATE => in_set("minecraft:smithing_template", s.item),
        SMITH_BASE => in_set("minecraft:smithing_base", s.item),
        SMITH_ADD => in_set("minecraft:smithing_addition", s.item),
        BEACON_PAY => in_tag("minecraft:beacon_payment_items", s.item),
        DYE => in_tag("minecraft:loom_dyes", s.item),
        PATTERN => in_tag("minecraft:loom_patterns", s.item),
        _ => None,
    };
    if let Some(v) = known {
        return v;
    }
    match code {
        LAPIS => n == "lapis_lazuli",
        GRIND => rules.max_damage(s) > 0 || n == "enchanted_book" || has_enchantments(rules, s),
        BANNER => n.ends_with("_banner"),
        DYE => n.ends_with("_dye"),
        PATTERN => n.ends_with("_banner_pattern"),
        // Any stack that carries a map number (`map_id`), like the server.
        MAP => rules.map_id_component().is_some_and(|c| s.added.iter().any(|a| a.0 == c)),
        MAP_ADD => matches!(n, "paper" | "map" | "glass_pane"),
        TEMPLATE => n.ends_with("_smithing_template"),
        SMITH_BASE => ends(&[
            "_helmet",
            "_chestplate",
            "_leggings",
            "_boots",
            "_sword",
            "_pickaxe",
            "_axe",
            "_shovel",
            "_hoe",
            "_spear",
            "_nautilus_armor",
        ]),
        SMITH_ADD => {
            n.ends_with("_ingot")
                || matches!(n, "redstone" | "lapis_lazuli" | "quartz" | "diamond" | "emerald" | "amethyst_shard")
        }
        BEACON_PAY => matches!(n, "iron_ingot" | "gold_ingot" | "emerald" | "diamond" | "netherite_ingot"),
        NOT_SHULKER => !n.ends_with("shulker_box"),
        _ => true,
    }
}

fn has_enchantments(rules: &dyn Rules, s: &ItemStack) -> bool {
    rules.enchantments_component().is_some_and(|c| s.added.iter().any(|a| a.0 == c))
}

/// Standard inventory slot `p` (0..36) starting from `(x0, y0)`.
fn standard(p: usize, x0: i32, y0: i32) -> SlotDef {
    let p = p as i32;
    if p < 27 { at(x0 + 18 * (p % 9), y0 + 18 * (p / 9)) } else { at(x0 + 18 * (p - 27), y0 + 58) }
}

fn at(x: i32, y: i32) -> SlotDef {
    SlotDef { x, y, max: 64, accept: Accept::Any, craft_result: false, empty_sprite: None }
}

fn result(x: i32, y: i32) -> SlotDef {
    SlotDef { accept: Accept::Nothing, ..at(x, y) }
}

fn only(x: i32, y: i32, code: u8) -> SlotDef {
    SlotDef { accept: Accept::Special(code), ..at(x, y) }
}

/// Hidden slot (inactive): off screen, never hovered.
const HIDDEN: i32 = -10_000;

/// Slot `i` of a special menu of `n` slots.
pub fn slot_def(s: Sp, i: usize, n: usize) -> SlotDef {
    use accept::*;
    let c = container_size(s);
    match s {
        Sp::Crafter => {
            return match i {
                0..=8 => at(26 + 18 * (i as i32 % 3), 17 + 18 * (i as i32 / 3)),
                45 => result(134, 35),
                _ => standard(i - 9, 8, 84),
            };
        }
        Sp::Lectern => return at(HIDDEN, HIDDEN),
        _ if i >= c => {
            let (x0, y0) = match s {
                Sp::Merchant => (108, 84),
                Sp::Beacon => (36, 137),
                _ => (8, 84),
            };
            return standard(i - c, x0, y0);
        }
        _ => {}
    }
    let _ = n;
    match s {
        Sp::Merchant => match i {
            0 => at(136, 37),
            1 => at(162, 37),
            _ => result(220, 37),
        },
        Sp::Mount(m) => match i {
            0 if m.saddle => {
                SlotDef { max: 1, accept: Accept::Armor(8), empty_sprite: Some("container/slot/saddle"), ..at(8, 18) }
            }
            1 if m.body => SlotDef {
                max: 1,
                accept: Accept::Armor(7),
                empty_sprite: Some(match m.kind {
                    MountKind::Horse => "container/slot/horse_armor",
                    MountKind::Llama => "container/slot/llama_armor",
                    MountKind::Nautilus => "container/slot/nautilus_armor_inventory",
                }),
                ..at(8, 36)
            },
            0 | 1 => SlotDef { accept: Accept::Nothing, ..at(HIDDEN, HIDDEN) },
            _ => {
                let k = (i - 2) as i32;
                let cols = i32::from(m.cols.max(1));
                at(80 + 18 * (k % cols), 18 + 18 * (k / cols))
            }
        },
        Sp::Grindstone => match i {
            0 => only(49, 19, GRIND),
            1 => only(49, 40, GRIND),
            _ => result(129, 34),
        },
        Sp::Stonecutter => match i {
            0 => at(20, 33),
            _ => result(143, 33),
        },
        Sp::Loom => match i {
            0 => SlotDef { empty_sprite: Some("container/slot/banner"), ..only(13, 26, BANNER) },
            1 => SlotDef { empty_sprite: Some("container/slot/dye"), ..only(33, 26, DYE) },
            2 => SlotDef { empty_sprite: Some("container/slot/banner_pattern"), ..only(23, 45, PATTERN) },
            _ => result(143, 57),
        },
        Sp::Cartography => match i {
            0 => only(15, 15, MAP),
            1 => only(15, 52, MAP_ADD),
            _ => result(145, 39),
        },
        Sp::Smithing => match i {
            0 => only(8, 48, TEMPLATE),
            1 => only(26, 48, SMITH_BASE),
            2 => only(44, 48, SMITH_ADD),
            _ => result(98, 48),
        },
        Sp::Enchant => match i {
            0 => SlotDef { max: 1, ..at(15, 47) },
            _ => SlotDef { empty_sprite: Some("container/slot/lapis_lazuli"), ..only(35, 47, LAPIS) },
        },
        Sp::Anvil => match i {
            0 => at(27, 47),
            1 => at(76, 47),
            _ => result(134, 47),
        },
        Sp::Beacon => SlotDef { max: 1, ..only(136, 110, BEACON_PAY) },
        Sp::Shulker => only(8 + 18 * (i as i32 % 9), 18 + 18 * (i as i32 / 9), NOT_SHULKER),
        Sp::Crafter | Sp::Lectern => unreachable!("handled above"),
    }
}

/// Slot neither takeable nor modifiable (crafter result, inactive mount slots).
pub fn locked(s: Sp, i: usize) -> bool {
    match s {
        Sp::Crafter => i == 45,
        Sp::Mount(m) => (i == 0 && !m.saddle) || (i == 1 && !m.body),
        _ => false,
    }
}

/// The gathering double-click does not use this slot (§ 1.2).
pub fn no_gather(s: Sp, i: usize) -> bool {
    matches!(s, Sp::Merchant) && i < 3
}

/// Quick move (shift-click) of slot `i`, stack `st`; true if something moved.
pub fn quick_move(m: &mut Menu<'_>, s: Sp, i: usize, st: &mut ItemStack) -> bool {
    use accept::*;
    let n = m.slots.len();
    let c = container_size(s);
    // Inventory ↔ hotbar (player part from `p` to `p + 36`).
    let swap_bar = |m: &mut Menu<'_>, st: &mut ItemStack, p: usize| {
        if i < p + 27 { m.move_to(st, p + 27, p + 36, false) } else { m.move_to(st, p, p + 27, false) }
    };
    let rules = m.rules;
    let fits = |code: u8| accepts(code, rules, st);
    match s {
        Sp::Crafter => match i {
            0..=8 => m.move_to(st, 9, 45, true),
            45 => m.move_to(st, 9, 45, true),
            _ => m.move_to(st, 0, 9, false),
        },
        Sp::Lectern => false,
        // Result (or any slot of the container) → inventory, in reverse order for the result.
        _ if i < c => {
            let reverse = slot_def(s, i, n).accept == Accept::Nothing
                || matches!(s, Sp::Shulker | Sp::Mount(_) | Sp::Enchant | Sp::Beacon);
            m.move_to(st, c, n, reverse)
        }
        Sp::Merchant | Sp::Anvil => swap_bar(m, st, c),
        Sp::Mount(mt) => {
            let e = rules.equip_slot(st);
            (mt.body && e == 7 && m.slots[1].is_none() && m.move_to(st, 1, 2, false))
                || (mt.saddle && e == 8 && m.slots[0].is_none() && m.move_to(st, 0, 1, false))
                || (mt.cols > 0 && m.move_to(st, 2, c, false))
                || swap_bar(m, st, c)
        }
        Sp::Grindstone => {
            if m.slots[0].is_none() || m.slots[1].is_none() {
                m.move_to(st, 0, 2, false)
            } else {
                swap_bar(m, st, c)
            }
        }
        Sp::Stonecutter => {
            // Lacking known recipes here, any item goes to the input if it can take it.
            m.move_to(st, 0, 1, false) || swap_bar(m, st, c)
        }
        Sp::Loom => {
            let target = if fits(BANNER) {
                Some(0)
            } else if fits(DYE) {
                Some(1)
            } else if fits(PATTERN) {
                Some(2)
            } else {
                None
            };
            match target {
                Some(t) => m.move_to(st, t, t + 1, false),
                None => swap_bar(m, st, c),
            }
        }
        Sp::Cartography => {
            if fits(MAP) {
                m.move_to(st, 0, 1, false)
            } else if fits(MAP_ADD) {
                m.move_to(st, 1, 2, false)
            } else {
                swap_bar(m, st, c)
            }
        }
        Sp::Smithing => m.move_to(st, 0, 3, false) || swap_bar(m, st, c),
        Sp::Enchant => {
            if fits(LAPIS) {
                m.move_to(st, 1, 2, false)
            } else if m.slots[0].is_none() {
                // A single copy in slot 0.
                let mut one = ItemStack { count: 1, ..st.clone() };
                let moved = m.move_to(&mut one, 0, 1, false);
                if moved {
                    st.count -= 1;
                }
                moved
            } else {
                false
            }
        }
        Sp::Beacon => {
            if m.slots[0].is_none() && fits(BEACON_PAY) && st.count == 1 {
                m.move_to(st, 0, 1, false)
            } else {
                swap_bar(m, st, c)
            }
        }
        Sp::Shulker => m.move_to(st, 0, c, false),
    }
}
