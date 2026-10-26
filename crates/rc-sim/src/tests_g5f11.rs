//! G5-F11: beacon, crafter, recipe book, workbench repair, equipment through
//! the inventory, merchant double-click, map extension. Fresh flat world: no external
//! data other than the 26.3 cache and the client jar, whose absence makes the test FAIL.

use std::path::PathBuf;
use std::sync::Arc;

use rc_client::Event;
use rc_client::recipes::{HolderSet, RecipeDisplay, SlotDisplay};
use rc_proto::nbt::Nbt;

use crate::blocks::probe::Probe;
use crate::menus::{Kind, Menu, Role, SlotDef, Src, mode};
use crate::player::Stack;

struct W {
    p: Probe,
    dir: PathBuf,
}

impl Drop for W {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// First layer of air of the flat world (grass at y = −61).
const Y: i32 = -60;

fn world(tag: &str) -> W {
    let data = rc_testdata::version_data().expect("G5-F11: 26.3 data cache missing");
    let data = Arc::new(data);
    assert!(crate::registries::client_jar(&data).is_some(), "G5-F11: 26.3 client jar missing");
    let dir = std::env::temp_dir().join(format!("rc-g5f11-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut w = crate::newworld::NewWorld::new(tag);
    w.seed = 7;
    w.preset = crate::newworld::Preset::Flat;
    w.allow_commands = true;
    w.generate_structures = false;
    crate::newworld::create_world(&dir, &data, &w).expect("world creation");
    let mut p = Probe::open(&dir, data);
    let want: Vec<(i32, i32)> = (-2..=2).flat_map(|x| (-2..=2).map(move |z| (x, z))).collect();
    p.s.ensure_loaded(&want);
    let t = std::time::Instant::now();
    while !want.iter().all(|c| p.s.chunks.get(c).is_some_and(Option::is_some)) {
        assert!(t.elapsed().as_secs() < 120, "G5-F11: flat world generation too slow");
        p.s.poll_gen();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    p.run(&format!("fill 0 {Y} 0 15 {} 15 minecraft:air\n", Y + 3));
    p.s.pos = [8.5, f64::from(Y), 11.5];
    p.s.game_mode = 0;
    p.s.loaded = true;
    let _ = p.take_events();
    W { p, dir }
}

fn stack(id: &str, count: i32) -> Option<Stack> {
    Some(Stack { id: format!("minecraft:{id}"), count, components: None })
}

impl W {
    fn sounds(&self, evs: &[Event], name: &str) -> usize {
        let id = self.p.s.sound_id(name).expect("known sound");
        evs.iter()
            .filter(
                |e| matches!(e, Event::Sound(s) if matches!(s.sound, rc_proto_v777::play::SoundRef::Id(i) if i == id)),
            )
            .count()
    }

    /// One game tick (time advanced, blocks then block entities).
    fn tick(&mut self, out: &mut Vec<Event>) {
        self.p.s.time += 1;
        self.p.s.tick_world_blocks();
        self.p.s.flush_block_changes();
        out.extend(self.p.take_events());
    }

    /// Real player command; test failure if refused.
    fn cmd(&mut self, text: &str) {
        let src = self.p.s.player_source();
        let (ok, _) = self.p.s.perform(src, text);
        assert!(ok, "command refused: {text}");
    }

    /// Until the next multiple of 80 (included).
    fn until_pulse(&mut self, out: &mut Vec<Event>) {
        loop {
            self.tick(out);
            if self.p.s.time % 80 == 0 {
                return;
            }
        }
    }

    fn adv(&self, id: &str) -> bool {
        let i = *self.p.s.prog.tree.index.get(id).unwrap_or_else(|| panic!("advancement {id} missing"));
        self.p.s.prog.done(i)
    }

    fn effect(&self, key: &str, at: [i32; 3]) -> Option<String> {
        self.p.s.be_get(at).and_then(|b| b.get(key)).and_then(Nbt::as_str).map(str::to_owned)
    }

    fn shift_click(&mut self, slot: i16) {
        let w = self.p.s.current_window();
        let state = self.p.s.life.menu.as_ref().map_or(0, |m| m.state);
        self.p.s.on_click(w, state, slot, 0, mode::QUICK_MOVE, Vec::new(), None);
    }

    fn click(&mut self, slot: i16) {
        let w = self.p.s.current_window();
        let state = self.p.s.life.menu.as_ref().map_or(0, |m| m.state);
        self.p.s.on_click(w, state, slot, 0, mode::PICKUP, Vec::new(), None);
    }
}

/// Beacon on a one-tier pyramid: column examined, levels every 80 ticks, activation
/// sound and `construct_beacon`; menu (payment, allowed choice, refusal that kicks the
/// player, payment dropped on closing); ambient effects to players; turn-off when the
/// pyramid disappears and when the beacon is removed.
#[test]
fn beacon_tick_menu_effects() {
    let mut w = world("beacon");
    let b = [8, Y + 1, 8];
    w.p.run(&format!("fill 7 {Y} 7 9 {Y} 9 minecraft:iron_block\nsetblock 8 {} 8 minecraft:beacon\n", Y + 1));
    let _ = w.p.take_events();
    w.p.s.time = 799 - 2;
    let mut evs = Vec::new();
    // First tick: column examined (beam), levels still 0.
    w.tick(&mut evs);
    assert!(w.p.s.use_block_or_bed(b), "beacon: click not handled");
    let m = w.p.s.life.menu.as_ref().expect("beacon menu open");
    assert_eq!((m.kind, m.menu_type(), m.len()), (Kind::Beacon, 9, 37));
    assert_eq!(m.data, vec![0, 0, 0]);
    // Multiple of 80: one tier, activation, advancement.
    w.until_pulse(&mut evs);
    assert_eq!(w.sounds(&evs, "minecraft:block.beacon.activate"), 1, "activation sound");
    assert!(w.adv("minecraft:nether/create_beacon"), "construct_beacon not triggered");
    assert_eq!(w.p.s.be_get(b).and_then(|e| crate::nbtio::int(e, "Levels")), Some(1), "Levels written");
    assert!(w.p.s.fx().get("minecraft:speed").is_none(), "no effect chosen");
    w.p.s.broadcast_menus();
    assert_eq!(w.p.s.life.menu.as_ref().map(|m| m.data[0]), Some(1), "menu levels");
    // Payment: a single ingot goes into the slot with Shift + click; a stack of 5 goes into the hotbar.
    w.p.s.slots[9] = stack("iron_ingot", 5);
    w.shift_click(1);
    assert!(w.p.s.life.menu.as_ref().is_some_and(|m| m.own[0].is_none()), "stack of 5 refused as payment");
    assert_eq!(w.p.s.slots[36].as_ref().map(|s| s.count), Some(5), "stack moved to the hotbar");
    w.p.s.slots[9] = stack("iron_ingot", 1);
    w.shift_click(1);
    assert_eq!(w.p.s.life.menu.as_ref().and_then(|m| m.own[0].clone()), stack("iron_ingot", 1));
    // Allowed choice: speed; payment consumed, choice sound, effect recorded.
    let mut evs = w.p.take_events();
    w.p.s.beacon_set(Some("minecraft:speed"), None);
    evs.extend(w.p.take_events());
    assert_eq!(w.effect("primary_effect", b).as_deref(), Some("minecraft:speed"));
    assert!(w.p.s.life.menu.as_ref().is_some_and(|m| m.own[0].is_none()), "payment consumed");
    assert_eq!(w.sounds(&evs, "minecraft:block.beacon.power_select"), 1);
    let speed =
        w.p.s.data.registry("minecraft:mob_effect").unwrap().entries.iter().position(|e| e == "minecraft:speed");
    assert_eq!(w.p.s.life.menu.as_ref().map(|m| m.data[1]), speed.map(|i| i as i32 + 1));
    // Refused choice (strength: 3 tiers): player kicked, nothing recorded.
    w.p.s.slots[10] = stack("emerald", 1);
    w.shift_click(2);
    w.p.s.beacon_set(Some("minecraft:strength"), None);
    let evs = w.p.take_events();
    assert!(evs.iter().any(|e| matches!(e, Event::Kicked(_))), "forbidden choice: player kicked");
    assert_eq!(w.effect("primary_effect", b).as_deref(), Some("minecraft:speed"));
    // Effect at the next multiple of 80: speed I, 220 ticks, ambient.
    let mut evs = Vec::new();
    w.until_pulse(&mut evs);
    let e = w.p.s.fx().get("minecraft:speed").cloned().expect("speed given");
    assert_eq!((e.amp, e.dur, e.ambient, e.visible), (0, 220, true, true));
    assert_eq!(w.sounds(&evs, "minecraft:block.beacon.ambient"), 1);
    // Closing: the payment emerald drops.
    w.p.s.close_menu(false);
    let ground: i32 =
        w.p.s.life.drops.iter().filter(|d| d.stack.id == "minecraft:emerald").map(|d| d.stack.count).sum();
    assert_eq!(ground, 1, "payment dropped on closing");
    // Pyramid removed: 0 tiers at the next multiple, turn-off sound.
    w.p.run(&format!("fill 7 {Y} 7 9 {Y} 9 minecraft:air\n"));
    let mut evs = Vec::new();
    w.until_pulse(&mut evs);
    assert_eq!(w.sounds(&evs, "minecraft:block.beacon.deactivate"), 1, "turn-off");
    // Beacon removed: turn-off sound.
    let _ = w.p.take_events();
    w.p.run(&format!("setblock 8 {} 8 minecraft:air\n", Y + 1));
    let evs = w.p.take_events();
    assert_eq!(w.sounds(&evs, "minecraft:block.beacon.deactivate"), 1, "beacon removed");
}

/// Beam cut by an opaque block (not by bedrock); without a beam, neither levels nor effects.
#[test]
fn beacon_beam_blocked() {
    let mut w = world("beam");
    let b = [8, Y + 1, 8];
    w.p.run(&format!(
        "fill 7 {Y} 7 9 {Y} 9 minecraft:iron_block\nsetblock 8 {} 8 minecraft:beacon\nsetblock 8 {} 8 minecraft:stone\n",
        Y + 1,
        Y + 3
    ));
    w.cmd(&format!("data merge block 8 {} 8 {{primary_effect:\"minecraft:haste\"}}", Y + 1));
    let mut evs = Vec::new();
    w.until_pulse(&mut evs);
    w.until_pulse(&mut evs);
    assert_eq!(w.p.s.beacon_data(b)[0], 0, "beam cut: no tier");
    assert!(w.p.s.fx().get("minecraft:haste").is_none());
    // Bedrock lets the beam through.
    w.p.run(&format!("setblock 8 {} 8 minecraft:bedrock\n", Y + 3));
    w.until_pulse(&mut evs);
    w.until_pulse(&mut evs);
    assert_eq!(w.p.s.beacon_data(b)[0], 1, "bedrock crossed");
    assert!(w.p.s.fx().get("minecraft:haste").is_some());
}

/// Beacon choice rule.
#[test]
fn beacon_choice_rules() {
    use crate::beacon::valid_choice as v;
    assert!(v(Some("minecraft:speed"), None, 1));
    assert!(!v(Some("minecraft:strength"), None, 2));
    assert!(v(Some("minecraft:strength"), None, 3));
    assert!(!v(Some("minecraft:speed"), Some("minecraft:regeneration"), 3), "secondary before 4 tiers");
    assert!(v(Some("minecraft:speed"), Some("minecraft:regeneration"), 4));
    assert!(v(Some("minecraft:speed"), Some("minecraft:speed"), 4), "same effect strengthened");
    assert!(!v(Some("minecraft:speed"), Some("minecraft:haste"), 4), "secondary of another level");
    assert!(!v(Some("minecraft:regeneration"), None, 4), "regeneration never as primary");
    assert!(!v(Some("minecraft:poison"), None, 4), "effect outside the list");
    assert!(v(None, None, 0));
}

/// Crafter: menu (type 7, block entity grid, shown result), slots disabled by
/// the packet (only empty ones), placement refused in a disabled slot, result neither taken nor placed.
#[test]
fn crafter_menu() {
    let mut w = world("crafter");
    let c = [8, Y, 10];
    w.p.run(&format!(
        "setblock 8 {Y} 10 minecraft:crafter\ndata merge block 8 {Y} 10 {{Items:[{{Slot:0b,id:\"minecraft:oak_log\",count:1}}]}}\n"
    ));
    assert!(w.p.s.use_block_or_bed(c), "crafter: click not handled");
    let window = w.p.s.current_window();
    let m = w.p.s.life.menu.as_ref().expect("crafter menu");
    assert_eq!((m.kind, m.menu_type(), m.len(), m.data.len()), (Kind::Crafter, 7, 46, 10));
    assert_eq!(m.own[0], stack("oak_log", 1));
    assert_eq!(m.own[9], stack("oak_planks", 4), "shown result");
    // Empty slot disabled; full slot unchanged.
    w.p.s.crafter_slot_state(1, window, false);
    w.p.s.crafter_slot_state(0, window, false);
    assert_eq!(w.p.s.be_get(c).and_then(|b| b.get("disabled_slots")).cloned(), Some(Nbt::IntArray(vec![1])));
    w.p.s.broadcast_menus();
    assert_eq!(w.p.s.life.menu.as_ref().map(|m| m.data[..2].to_vec()), Some(vec![0, 1]));
    // Placement in the disabled slot refused; in slot 2 accepted, result recomputed.
    w.p.s.slots[9] = stack("oak_log", 2);
    w.click(9);
    w.click(1);
    assert!(w.p.s.life.menu.as_ref().is_some_and(|m| m.own[1].is_none()), "disabled slot");
    w.click(2);
    let m = w.p.s.life.menu.as_ref().unwrap();
    assert_eq!(m.own[2], stack("oak_log", 2));
    assert!(m.own[9].is_none(), "two logs: no recipe");
    // Result: neither taken on click nor on quick move.
    w.click(2);
    w.click(9);
    w.p.run(&format!("data merge block 8 {Y} 10 {{Items:[{{Slot:0b,id:\"minecraft:oak_log\",count:1}}]}}\n"));
    w.p.s.broadcast_menus();
    assert_eq!(w.p.s.life.menu.as_ref().map(|m| m.own[9].clone()), Some(stack("oak_planks", 4)), "grid re-read");
    w.click(45);
    w.shift_click(45);
    assert!(w.p.s.life.menu.as_ref().is_some_and(|m| m.carried.is_none() && m.own[9].is_some()));
    // Reopening: the slot becomes free again.
    w.p.s.crafter_slot_state(1, window, true);
    assert_eq!(w.p.s.be_get(c).and_then(|b| b.get("disabled_slots")).cloned(), Some(Nbt::IntArray(Vec::new())));
}

/// Workbench repair: any usable item with wear (bow, without a `repairable` component);
/// the greatest durability is set on the result when it is not the default one.
#[test]
fn repair_any_damageable() {
    let w = world("repair");
    let items = &w.p.s.life.items;
    let bow = |dmg: i32| {
        let mut s = stack("bow", 1).unwrap();
        crate::items::set_component(&mut s, "minecraft:damage", Nbt::Int(dmg));
        Some(s)
    };
    let grid = vec![bow(300), bow(200), None, None];
    let (_, out) = w.p.s.life.recipes.craft(items, &grid, 2, &|_| None).expect("bows repaired");
    // Remaining 84 + 184 + 384 × 5% = 287 → wear 384 − 287 = 97.
    assert_eq!(out.id, "minecraft:bow");
    assert_eq!(crate::items::component(&out, "minecraft:damage"), Some(&Nbt::Int(97)));
    assert!(crate::items::component(&out, "minecraft:max_damage").is_none(), "default durability not set");
    let mut big = bow(380).unwrap();
    crate::items::set_component(&mut big, "minecraft:max_damage", Nbt::Int(1000));
    let grid = vec![Some(big), bow(380), None, None];
    let (_, out) = w.p.s.life.recipes.craft(items, &grid, 2, &|_| None).expect("bows repaired");
    assert_eq!(crate::items::component(&out, "minecraft:max_damage"), Some(&Nbt::Int(1000)));
    // 1000 - (620 + 4 + 50) = 326.
    assert_eq!(crate::items::component(&out, "minecraft:damage"), Some(&Nbt::Int(326)));
    // Without tracked wear: nothing.
    let mut a = bow(10).unwrap();
    crate::items::set_component(&mut a, "!minecraft:damage", Nbt::Compound(Vec::new()));
    crate::items::remove_component(&mut a, "minecraft:damage");
    let grid = vec![Some(a), bow(10), None, None];
    assert!(w.p.s.life.recipes.craft(items, &grid, 2, &|_| None).is_none());
}

/// Map extension: unknown map or already at scale 4 → no result.
#[test]
fn map_extend_max_scale() {
    let mut w = world("mapext");
    let grid = |id: i32| {
        let mut g = vec![stack("paper", 1); 9];
        let mut m = stack("filled_map", 1).unwrap();
        crate::items::set_component(&mut m, "minecraft:map_id", Nbt::Int(id));
        g[4] = Some(m);
        g
    };
    let small = w.p.s.map_create(0.0, 0.0, 3, true, false);
    let big = w.p.s.map_create(0.0, 0.0, 4, true, false);
    let s = &w.p.s;
    let scale = |id: i32| s.maps.scale_of(id);
    assert!(s.life.recipes.craft(&s.life.items, &grid(small), 3, &scale).is_some(), "scale 3 extended");
    assert!(s.life.recipes.craft(&s.life.items, &grid(big), 3, &scale).is_none(), "scale 4 refused");
    assert!(s.life.recipes.craft(&s.life.items, &grid(9999), 3, &scale).is_none(), "unknown map");
}

/// Recipe book: transmutation (one entry per number of materials), dyeing,
/// impregnation, stonecutting, smithing; ingredients shown as item sets;
/// unique display numbers given back to their recipe.
#[test]
fn recipe_book_all_kinds() {
    let w = world("book");
    let s = &w.p.s;
    let one = |id: &str| {
        let e = s.book_entries(id, 0);
        assert_eq!(e.len(), 1, "{id}: one entry");
        e.into_iter().next().unwrap().0
    };
    // Dyed Shulker box: box (tag) + dye (single item, as a set).
    let e = one("minecraft:black_shulker_box");
    let RecipeDisplay::Shapeless { ingredients, .. } = &e.display else { panic!("transmutation: {e:?}") };
    assert_eq!(ingredients[0], SlotDisplay::Tag(HolderSet::Tag("minecraft:shulker_boxes".into())));
    let dye = s.life.items.id("minecraft:black_dye").unwrap();
    assert_eq!(ingredients[1], SlotDisplay::Tag(HolderSet::Ids(vec![dye])));
    // Map copy: 1 to 8 empty maps, 8 entries; the result counts the materials.
    let maps = s.book_entries("minecraft:map_cloning", 0);
    assert_eq!(maps.len(), 8);
    let RecipeDisplay::Shapeless { ingredients, result, .. } = &maps[7].0.display else { panic!() };
    assert_eq!(ingredients.len(), 9);
    let SlotDisplay::Composite(list) = result else { panic!("map copy: {result:?}") };
    assert!(matches!(&list[0], SlotDisplay::Stack(t) if t.count == 9));
    let i = s.life.recipes.index("minecraft:map_cloning").unwrap();
    for (k, (e, _)) in maps.iter().enumerate() {
        assert_eq!(s.life.recipes.of_display(e.id), Some((i, k)), "display number given back");
    }
    assert!(matches!(
        one("minecraft:leather_boots_dyed").display,
        RecipeDisplay::Shapeless { result: SlotDisplay::Dyed(..), .. }
    ));
    assert!(matches!(one("minecraft:tipped_arrow").display, RecipeDisplay::Shaped { width: 3, height: 3, .. }));
    assert!(matches!(
        one("minecraft:andesite_slab_from_andesite_stonecutting").display,
        RecipeDisplay::Stonecutter { .. }
    ));
    assert!(matches!(one("minecraft:netherite_axe_smithing").display, RecipeDisplay::Smithing { .. }));
    assert!(matches!(
        one("minecraft:bolt_armor_trim_smithing_template_smithing_trim").display,
        RecipeDisplay::Smithing { result: SlotDisplay::SmithingTrim(..), .. }
    ));
    assert!(s.book_entries("minecraft:repair_item", 0).is_empty(), "special recipe outside the book");
    // Recipe with shape: ingredients as sets (never a bare item).
    let RecipeDisplay::Shaped { ingredients, .. } = one("minecraft:stick").display else { panic!() };
    assert!(ingredients.iter().all(|x| matches!(x, SlotDisplay::Tag(_) | SlotDisplay::Empty)));
}

/// Armor and off hand placed through the inventory (click, creative mode): equip sound.
#[test]
fn equip_sound_from_inventory() {
    let mut w = world("equip");
    w.p.s.slots[9] = stack("iron_chestplate", 1);
    let _ = w.p.take_events();
    w.shift_click(9);
    assert_eq!(w.p.s.slots[6], stack("iron_chestplate", 1), "chestplate worn");
    let evs = w.p.take_events();
    assert_eq!(w.sounds(&evs, "minecraft:item.armor.equip_iron"), 1, "equip sound");
    w.p.s.game_mode = 1;
    w.p.s.on_creative_slot(5, stack("iron_helmet", 1));
    let evs = w.p.take_events();
    assert_eq!(w.sounds(&evs, "minecraft:item.armor.equip_iron"), 1, "helmet placed in creative");
}

struct Env<'a> {
    items: &'a crate::items::Items,
}

impl crate::menus::Env for Env<'_> {
    fn items(&self) -> &crate::items::Items {
        self.items
    }
    fn creative(&self) -> bool {
        false
    }
    fn selected(&self) -> usize {
        0
    }
    fn drop_item(&mut self, _s: Stack, _thrown: bool) {}
    fn craft(&mut self, _grid: &[Option<Stack>], _w: usize) -> Option<Stack> {
        None
    }
    fn remaining(&mut self, grid: &[Option<Stack>], _w: usize) -> Vec<Option<Stack>> {
        vec![None; grid.len()]
    }
    fn crafted(&mut self, _result: &Stack, _count: i32) {}
    fn smelted(&mut self, _count: i32) {}
    fn is_fuel(&self, _s: &Stack) -> bool {
        false
    }
    fn can_smelt(&self, _s: &Stack, _kind: u8) -> bool {
        false
    }
    fn hashed(&self, _s: Option<&Stack>) -> Option<rc_proto_v777::container::HashedStack> {
        None
    }
}

/// Double-click (gather): nothing in the merchant screen, everything elsewhere.
#[test]
fn merchant_pickup_all_disabled() {
    let w = world("pickall");
    let mut env = Env { items: &w.p.s.life.items };
    for (kind, want) in [(Kind::Merchant, 1), (Kind::Crafting, 21)] {
        let mut slots = vec![SlotDef { src: Src::Own(0), role: Role::Plain }];
        Menu::player_slots(&mut slots);
        let mut m = Menu::base(1, kind, slots, 3, 0);
        let mut inv = vec![None; crate::player::MENU_SLOTS];
        inv[9] = stack("stone", 10);
        inv[10] = stack("stone", 10);
        m.carried = stack("stone", 1);
        m.click(&mut env, &mut inv, 3, 0, mode::PICKUP_ALL);
        assert_eq!(m.carried.as_ref().map(|s| s.count), Some(want), "{kind:?}");
    }
}

/// Micro-bench (ignored by default): cost of the tick of a full-pyramid beacon (4 tiers),
/// off a multiple of 80 (column examination) and at a multiple of 80 (pyramid, effects).
#[test]
#[ignore]
fn bench_beacon_tick() {
    let mut w = world("bench-beacon");
    let y = Y + 4;
    let centers: Vec<i32> = vec![-10, 8, 26];
    for &c in &centers {
        for (step, dy) in (1..=4).zip(1..) {
            w.p.run(&format!(
                "fill {} {} {} {} {} {} minecraft:iron_block\n",
                c - step,
                y - dy,
                8 - step,
                c + step,
                y - dy,
                8 + step
            ));
        }
        w.p.run(&format!("setblock {c} {y} 8 minecraft:beacon\n"));
        w.cmd(&format!("data merge block {c} {y} 8 {{primary_effect:\"minecraft:speed\"}}"));
    }
    let mut evs = Vec::new();
    w.until_pulse(&mut evs);
    w.until_pulse(&mut evs);
    let states: Vec<(crate::blocks::P, crate::beacon::BeaconTick)> = centers
        .iter()
        .map(|&c| {
            let p = [c, y, 8];
            match w.p.s.bs.be_ticks.get(&p) {
                Some((_, crate::blocks::level::BeTick::Beacon(b))) => (p, *b),
                _ => panic!("beacon without ticker"),
            }
        })
        .collect();
    assert!(states.iter().all(|s| s.1.levels == 4), "complete pyramids");
    for (label, time) in [("column", 81i64), ("multiple of 80", 160)] {
        w.p.s.time = time;
        let n = 20_000;
        let t = std::time::Instant::now();
        for _ in 0..n {
            for &(p, b) in &states {
                w.p.s.beacon_tick(p, b);
            }
        }
        let ns = t.elapsed().as_nanos() as f64 / (n as f64 * states.len() as f64);
        let _ = w.p.take_events();
        eprintln!("beacon {label}: {ns:.1} ns/tick");
    }
}

/// Micro-bench (ignored by default): full book (all recipes); precomputed group number
/// against the old computation (group list sorted at each entry).
#[test]
#[ignore]
fn bench_recipe_book() {
    let w = world("bench-book");
    let s = &w.p.s;
    let ids: Vec<String> = s.life.recipes.list.iter().map(|r| r.id.clone()).collect();
    let t = std::time::Instant::now();
    let mut n = 0;
    for _ in 0..5 {
        for id in &ids {
            n += s.book_entries(id, 0).len();
        }
    }
    eprintln!("book: {} entries, {:.2} ms per full book", n / 5, t.elapsed().as_secs_f64() * 1e3 / 5.0);
    let t = std::time::Instant::now();
    let mut sum = 0usize;
    for _ in 0..5 {
        for r in &s.life.recipes.list {
            let Some(g) = &r.group else { continue };
            let mut names: Vec<&String> = s.life.recipes.list.iter().filter_map(|r| r.group.as_ref()).collect();
            names.sort();
            names.dedup();
            sum += names.iter().position(|x| *x == g).unwrap_or(0);
        }
    }
    eprintln!("old group number: {:.2} ms per full book ({sum})", t.elapsed().as_secs_f64() * 1e3 / 5.0);
    let t = std::time::Instant::now();
    for _ in 0..5 {
        for i in 0..s.life.recipes.list.len() {
            sum += s.life.recipes.group_id(i).unwrap_or(0) as usize;
        }
    }
    eprintln!(
        "precomputed group number: {:.4} ms per full book ({sum})",
        t.elapsed().as_secs_f64() * 1e3 / 5.0
    );
}
