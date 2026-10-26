//! Menu clicks and recipes (version data and player's jar; absent: tests ignored).

use std::sync::{Arc, OnceLock};

use super::*;
use crate::items::Items;
use crate::recipes::Recipes;

struct World {
    items: Items,
    recipes: Recipes,
}

fn world() -> Option<&'static World> {
    static W: OnceLock<Option<World>> = OnceLock::new();
    W.get_or_init(|| {
        let Some(data) = rc_testdata::version_data() else {
            eprintln!("version data absent (cache {})", rc_data::cache_root().display());
            return None;
        };
        let data = Arc::new(data);
        let Some(jar) = crate::registries::client_jar(&data) else {
            eprintln!("player's jar not found");
            return None;
        };
        let jar = crate::registries::Jar::read(&jar).ok()?;
        let regs = crate::registries::load_jar(&jar, &data);
        let pack = crate::datapack::DataPack::from_jar(&jar);
        let items = Items::new(data, &regs.tags);
        let recipes = Recipes::load(&pack, &items);
        Some(World { items, recipes })
    })
    .as_ref()
}

struct TestEnv<'a> {
    w: &'a World,
    creative: bool,
    dropped: Vec<(Stack, bool)>,
    crafted: i32,
}

impl Env for TestEnv<'_> {
    fn items(&self) -> &Items {
        &self.w.items
    }
    fn creative(&self) -> bool {
        self.creative
    }
    fn selected(&self) -> usize {
        0
    }
    fn drop_item(&mut self, s: Stack, thrown: bool) {
        self.dropped.push((s, thrown));
    }
    fn craft(&mut self, grid: &[Option<Stack>], w: usize) -> Option<Stack> {
        self.w.recipes.craft(&self.w.items, grid, w, &|_| None).map(|r| r.1)
    }
    fn remaining(&mut self, grid: &[Option<Stack>], _w: usize) -> Vec<Option<Stack>> {
        self.w.recipes.remaining(&self.w.items, grid, &|_| None)
    }
    fn crafted(&mut self, _result: &Stack, count: i32) {
        self.crafted += count;
    }
    fn smelted(&mut self, _count: i32) {}
    fn is_fuel(&self, s: &Stack) -> bool {
        self.w.items.default_component(s, "minecraft:cooking_fuel").is_some()
    }
    fn can_smelt(&self, s: &Stack, _kind: u8) -> bool {
        self.w.recipes.cooking(&self.w.items, crate::recipes::CookKind::Smelting, s).is_some()
    }
    fn hashed(&self, _s: Option<&Stack>) -> Option<HashedStack> {
        None
    }
}

fn st(id: &str, count: i32) -> Option<Stack> {
    Some(Stack { id: format!("minecraft:{id}"), count, components: None })
}

fn count(s: &Option<Stack>) -> i32 {
    s.as_ref().map_or(0, |s| s.count)
}

fn setup() -> Option<(&'static World, Vec<Option<Stack>>)> {
    Some((world()?, vec![None; crate::player::MENU_SLOTS]))
}

#[test]
fn pickup_left_right_and_swap() {
    let Some((w, mut inv)) = setup() else { return };
    let mut env = TestEnv { w, creative: false, dropped: Vec::new(), crafted: 0 };
    let mut m = Menu::inventory();
    inv[9] = st("cobblestone", 33);
    inv[10] = st("dirt", 5);
    // Right click: half rounded up.
    m.click(&mut env, &mut inv, 9, 1, mode::PICKUP);
    assert_eq!(count(&m.carried), 17);
    assert_eq!(count(&inv[9]), 16);
    // Right click on an empty slot: a single item placed.
    m.click(&mut env, &mut inv, 11, 1, mode::PICKUP);
    assert_eq!(count(&inv[11]), 1);
    assert_eq!(count(&m.carried), 16);
    // Left click on the same stack: everything poured back.
    m.click(&mut env, &mut inv, 9, 0, mode::PICKUP);
    assert_eq!(count(&inv[9]), 32);
    assert!(m.carried.is_none());
    // Take the dirt then click on the stone: swap.
    m.click(&mut env, &mut inv, 10, 0, mode::PICKUP);
    m.click(&mut env, &mut inv, 9, 0, mode::PICKUP);
    assert_eq!(inv[9].as_ref().unwrap().id, "minecraft:dirt");
    assert_eq!(m.carried.as_ref().unwrap().id, "minecraft:cobblestone");
    // Outside the window, right click: one item dropped.
    m.click(&mut env, &mut inv, -999, 1, mode::PICKUP);
    assert_eq!(env.dropped.len(), 1);
    assert_eq!(count(&m.carried), 31);
}

#[test]
fn quick_move_and_number_keys() {
    let Some((w, mut inv)) = setup() else { return };
    let mut env = TestEnv { w, creative: false, dropped: Vec::new(), crafted: 0 };
    let mut m = Menu::inventory();
    inv[36] = st("cobblestone", 40);
    inv[9] = st("cobblestone", 50);
    // Shift-click from the hotbar: tops up the bag's stack, the rest in the next free slot.
    m.click(&mut env, &mut inv, 36, 0, mode::QUICK_MOVE);
    assert_eq!(count(&inv[9]), 64);
    assert_eq!(count(&inv[10]), 26);
    assert!(inv[36].is_none());
    // Armour: shift-click equips it.
    inv[20] = st("iron_helmet", 1);
    m.click(&mut env, &mut inv, 20, 0, mode::QUICK_MOVE);
    assert_eq!(inv[5].as_ref().unwrap().id, "minecraft:iron_helmet");
    // Key 3 on slot 10: swap with the hotbar (slot 2).
    m.click(&mut env, &mut inv, 10, 2, mode::SWAP);
    assert_eq!(count(&inv[38]), 26);
    assert!(inv[10].is_none());
    // F key: off hand.
    m.click(&mut env, &mut inv, 38, 40, mode::SWAP);
    assert_eq!(count(&inv[45]), 26);
}

#[test]
fn drag_split_and_gather() {
    let Some((w, mut inv)) = setup() else { return };
    let mut env = TestEnv { w, creative: false, dropped: Vec::new(), crafted: 0 };
    let mut m = Menu::inventory();
    m.carried = st("oak_log", 64);
    // Left drag over 3 slots: 21 each, 1 stays carried.
    m.click(&mut env, &mut inv, -999, 0, mode::QUICK_CRAFT);
    for s in [9, 10, 11] {
        m.click(&mut env, &mut inv, s, 1, mode::QUICK_CRAFT);
    }
    m.click(&mut env, &mut inv, -999, 2, mode::QUICK_CRAFT);
    assert_eq!([count(&inv[9]), count(&inv[10]), count(&inv[11])], [21, 21, 21]);
    assert_eq!(count(&m.carried), 1);
    // Right drag: one per slot.
    m.carried = st("oak_log", 10);
    m.click(&mut env, &mut inv, -999, 4, mode::QUICK_CRAFT);
    m.click(&mut env, &mut inv, 12, 5, mode::QUICK_CRAFT);
    m.click(&mut env, &mut inv, 13, 5, mode::QUICK_CRAFT);
    m.click(&mut env, &mut inv, -999, 6, mode::QUICK_CRAFT);
    assert_eq!([count(&inv[12]), count(&inv[13])], [1, 1]);
    assert_eq!(count(&m.carried), 8);
    // Out-of-sequence phase: reset, nothing moves.
    m.click(&mut env, &mut inv, 14, 1, mode::QUICK_CRAFT);
    assert!(inv[14].is_none());
    // Double click: gathers up to 64 (non-full stacks first).
    m.click(&mut env, &mut inv, 20, 0, mode::PICKUP_ALL);
    assert_eq!(count(&m.carried), 64);
}

#[test]
fn throw_from_slot() {
    let Some((w, mut inv)) = setup() else { return };
    let mut env = TestEnv { w, creative: false, dropped: Vec::new(), crafted: 0 };
    let mut m = Menu::inventory();
    inv[9] = st("dirt", 5);
    m.click(&mut env, &mut inv, 9, 0, mode::THROW);
    assert_eq!(count(&inv[9]), 4);
    m.click(&mut env, &mut inv, 9, 1, mode::THROW);
    assert!(inv[9].is_none());
    assert_eq!(env.dropped.iter().map(|d| d.0.count).sum::<i32>(), 5);
    // Cloning refused outside creative.
    inv[9] = st("dirt", 5);
    m.click(&mut env, &mut inv, 9, 2, mode::CLONE);
    assert!(m.carried.is_none());
    env.creative = true;
    m.click(&mut env, &mut inv, 9, 2, mode::CLONE);
    assert_eq!(count(&m.carried), 64);
}

#[test]
fn craft_planks_sticks_and_table() {
    let Some((w, mut inv)) = setup() else { return };
    let mut env = TestEnv { w, creative: false, dropped: Vec::new(), crafted: 0 };
    let mut m = Menu::inventory();
    // Log in the 2×2 grid: 4 planks.
    m.carried = st("oak_log", 3);
    m.click(&mut env, &mut inv, 1, 0, mode::PICKUP);
    assert_eq!(inv[0].as_ref().map(|s| (s.id.as_str(), s.count)), Some(("minecraft:oak_planks", 4)));
    // Shift-click on the result: the 3 logs become 12 planks.
    m.click(&mut env, &mut inv, 0, 0, mode::QUICK_MOVE);
    assert!(inv[1].is_none() && inv[0].is_none());
    let planks: i32 = inv.iter().flatten().filter(|s| s.id == "minecraft:oak_planks").map(|s| s.count).sum();
    assert_eq!(planks, 12);
    assert_eq!(env.crafted, 12);
    // Crafting table: 4 planks in a square.
    let mut m = Menu::crafting(1);
    let mut inv2 = vec![None; crate::player::MENU_SLOTS];
    for s in [1, 2, 4, 5] {
        m.own[s] = st("oak_planks", 1);
    }
    m.update_result(&mut env, &mut inv2);
    assert_eq!(m.own[0].as_ref().map(|s| s.id.as_str()), Some("minecraft:crafting_table"));
    // Same pattern shifted to the bottom right.
    m.own = vec![None; 10];
    for s in [5, 6, 8, 9] {
        m.own[s] = st("oak_planks", 1);
    }
    m.update_result(&mut env, &mut inv2);
    assert_eq!(m.own[0].as_ref().map(|s| s.id.as_str()), Some("minecraft:crafting_table"));
    // Taking the result consumes one plank per slot.
    m.click(&mut env, &mut inv2, 0, 0, mode::PICKUP);
    assert_eq!(m.carried.as_ref().map(|s| s.id.as_str()), Some("minecraft:crafting_table"));
    assert!(m.own[5].is_none() && m.own[0].is_none());
}

#[test]
fn shaped_mirror_and_remainders() {
    let Some((w, mut inv)) = setup() else { return };
    let mut env = TestEnv { w, creative: false, dropped: Vec::new(), crafted: 0 };
    let mut m = Menu::crafting(1);
    // Mirrored axe: planks on the right.
    m.own[2] = st("oak_planks", 1);
    m.own[3] = st("oak_planks", 1);
    m.own[6] = st("oak_planks", 1);
    m.own[5] = st("stick", 1);
    m.own[8] = st("stick", 1);
    m.update_result(&mut env, &mut inv);
    assert_eq!(m.own[0].as_ref().map(|s| s.id.as_str()), Some("minecraft:wooden_axe"));
    // Cake: milk buckets leave empty buckets.
    m.own = vec![None; 10];
    for s in 1..4 {
        m.own[s] = st("milk_bucket", 1);
    }
    m.own[4] = st("sugar", 1);
    m.own[5] = st("egg", 1);
    m.own[6] = st("sugar", 1);
    for s in 7..10 {
        m.own[s] = st("wheat", 1);
    }
    m.update_result(&mut env, &mut inv);
    assert_eq!(m.own[0].as_ref().map(|s| s.id.as_str()), Some("minecraft:cake"));
    m.click(&mut env, &mut inv, 0, 0, mode::PICKUP);
    for s in 1..4 {
        assert_eq!(m.own[s].as_ref().map(|s| s.id.as_str()), Some("minecraft:bucket"));
    }
    assert!(m.own[4].is_none() && m.own[0].is_none());
    // Closing: grid returned to the inventory.
    m.close(&mut env, &mut inv);
    let buckets: i32 = inv.iter().flatten().filter(|s| s.id == "minecraft:bucket").map(|s| s.count).sum();
    assert_eq!(buckets, 3);
    assert_eq!(inv[36].as_ref().map(|s| s.id.as_str()), Some("minecraft:cake"), "carried returned first, held slot");
}

#[test]
fn pickup_goes_to_first_free_slot() {
    let Some((w, mut inv)) = setup() else { return };
    inv[36] = st("stone_pickaxe", 1);
    assert!(inventory_add(&mut inv, &w.items, 0, st("dirt", 1).unwrap(), false).is_none());
    assert_eq!(inv[37].as_ref().map(|s| s.id.as_str()), Some("minecraft:dirt"));
    assert!(inventory_add(&mut inv, &w.items, 0, st("dirt", 1).unwrap(), false).is_none());
    assert_eq!(count(&inv[37]), 2);
    assert_eq!(inv[36].as_ref().map(|s| s.id.as_str()), Some("minecraft:stone_pickaxe"));
}

#[test]
fn synchronisation_sends_only_differences() {
    let Some((w, mut inv)) = setup() else { return };
    let mut env = TestEnv { w, creative: false, dropped: Vec::new(), crafted: 0 };
    let mut m = Menu::inventory();
    inv[9] = st("dirt", 5);
    m.send_all(&inv);
    assert!(matches!(m.out.last(), Some(Out::Content { .. })));
    m.out.clear();
    m.broadcast(&env, &inv);
    assert!(m.out.is_empty());
    m.click(&mut env, &mut inv, 9, 0, mode::PICKUP);
    m.broadcast(&env, &inv);
    let slots: Vec<i16> =
        m.out.iter().filter_map(|o| if let Out::Slot { slot, .. } = o { Some(*slot) } else { None }).collect();
    assert_eq!(slots, vec![9]);
    assert!(m.out.iter().any(|o| matches!(o, Out::Cursor(Some(_)))));
}

fn bag(w: &World, s: &Option<Stack>) -> Vec<(String, i32)> {
    super::bundle::contents(&w.items, s.as_ref().unwrap()).unwrap().into_iter().map(|s| (s.id, s.count)).collect()
}

/// W8c: bundle carried onto a stack (left click: insertion limited by weight), right click on
/// an empty slot (a stack comes out), bundle placed in a slot (left and right clicks).
#[test]
fn bundle_clicks() {
    let Some((w, mut inv)) = setup() else { return };
    let mut env = TestEnv { w, creative: false, dropped: Vec::new(), crafted: 0 };
    let mut m = Menu::inventory();
    inv[9] = st("bundle", 1);
    inv[10] = st("cobblestone", 40);
    inv[11] = st("ender_pearl", 16);
    m.click(&mut env, &mut inv, 9, 0, mode::PICKUP);
    m.click(&mut env, &mut inv, 10, 0, mode::PICKUP);
    assert_eq!(bag(w, &m.carried), vec![("minecraft:cobblestone".into(), 40)]);
    assert!(inv[10].is_none());
    // 40/64 taken: 24/64 left = 6 pearls (1/16 each).
    m.click(&mut env, &mut inv, 11, 0, mode::PICKUP);
    assert_eq!(count(&inv[11]), 10);
    assert_eq!(bag(w, &m.carried)[0], ("minecraft:ender_pearl".into(), 6));
    // Bag full: nothing moves.
    m.click(&mut env, &mut inv, 11, 0, mode::PICKUP);
    assert_eq!(count(&inv[11]), 10);
    // Right click on an empty slot: the last inserted (at the head) comes out.
    m.click(&mut env, &mut inv, 12, 1, mode::PICKUP);
    assert_eq!(inv[12].as_ref().map(|s| (s.id.as_str(), s.count)), Some(("minecraft:ender_pearl", 6)));
    // Bundle placed; stone carried, left click on the bundle: poured into it.
    m.click(&mut env, &mut inv, 13, 0, mode::PICKUP);
    inv[14] = st("stone", 30);
    m.click(&mut env, &mut inv, 14, 0, mode::PICKUP);
    m.click(&mut env, &mut inv, 13, 0, mode::PICKUP);
    assert_eq!(count(&m.carried), 6, "24/64 left: 24 stones go in");
    assert_eq!(bag(w, &inv[13])[0], ("minecraft:stone".into(), 24));
    // Right click on the bundle, cursor empty: a stack onto the cursor.
    m.click(&mut env, &mut inv, 15, 0, mode::PICKUP);
    m.click(&mut env, &mut inv, 13, 1, mode::PICKUP);
    assert_eq!(m.carried.as_ref().map(|s| (s.id.as_str(), s.count)), Some(("minecraft:stone", 24)));
    // A shulker box does not go in.
    m.click(&mut env, &mut inv, 15, 0, mode::PICKUP);
    m.carried = st("shulker_box", 1);
    m.click(&mut env, &mut inv, 13, 0, mode::PICKUP);
    assert_eq!(count(&m.carried), 1);
    assert_eq!(bag(w, &inv[13]), vec![("minecraft:cobblestone".into(), 40)]);
}

/// Stack chosen with the wheel (`bundle_item_selected`): right click removes that one, the
/// selection falls back after use; same index twice or index outside the bundle = deselection.
#[test]
fn bundle_selected_item_is_removed() {
    let Some((w, mut inv)) = setup() else { return };
    let mut env = TestEnv { w, creative: false, dropped: Vec::new(), crafted: 0 };
    let mut m = Menu::inventory();
    inv[9] = st("bundle", 1);
    inv[10] = st("cobblestone", 10);
    inv[11] = st("ender_pearl", 4);
    inv[12] = st("stone", 3);
    for k in [10, 11, 12] {
        m.click(&mut env, &mut inv, 9, 0, mode::PICKUP); // bundle onto the cursor
        m.click(&mut env, &mut inv, k, 0, mode::PICKUP); // item poured
        m.click(&mut env, &mut inv, 9, 0, mode::PICKUP); // bundle put back
    }
    // Order in the bundle: stone, pearls, cobble (the last inserted at the head).
    assert_eq!(
        bag(w, &inv[9]).iter().map(|b| b.0.as_str()).collect::<Vec<_>>(),
        ["minecraft:stone", "minecraft:ender_pearl", "minecraft:cobblestone"]
    );
    m.select_bundle(&w.items, &inv, 9, 2);
    // Bundle placed in slot 9, cursor empty: right click removes the chosen stack.
    m.click(&mut env, &mut inv, 9, 1, mode::PICKUP);
    assert_eq!(m.carried.as_ref().map(|s| s.id.as_str()), Some("minecraft:cobblestone"));
    // Selection fell back: the next take is the first stack.
    m.carried = None;
    m.click(&mut env, &mut inv, 9, 1, mode::PICKUP);
    assert_eq!(m.carried.as_ref().map(|s| s.id.as_str()), Some("minecraft:stone"));
    // Toggle: same index twice, or outside the bundle.
    m.carried = None;
    m.select_bundle(&w.items, &inv, 9, 0);
    m.select_bundle(&w.items, &inv, 9, 0);
    assert!(m.bundle_sel.is_none());
    m.select_bundle(&w.items, &inv, 9, 5);
    assert!(m.bundle_sel.is_none());
}
