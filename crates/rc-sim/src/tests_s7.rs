//! S7: effects, brewing stand, enchanting, anvil, grindstone, smithing table, stonecutter, loot chests
//! (version data and player jar; absent: tests ignored). The reference values
//! come from the official 26.3 server (same seeds), see `mechanics-s7.md` § 7.

use std::sync::{Arc, OnceLock};

use rc_proto::nbt::Nbt;
use rc_proto_v777::container::HashedStack;

use crate::datapack::DataPack;
use crate::effects::Inst;
use crate::items::{Items, component, set_component};
use crate::menus::{Env, Menu, mode};
use crate::player::Stack;
use crate::recipes::Recipes;
use crate::stations::{self, Data, Ev, St};

struct World {
    items: Items,
    pack: DataPack,
    st: Data,
    ench_reg: Vec<String>,
}

fn world() -> Option<&'static World> {
    static W: OnceLock<Option<World>> = OnceLock::new();
    W.get_or_init(|| {
        let data = Arc::new(rc_testdata::version_data()?);
        let jar = crate::registries::client_jar(&data)?;
        let jar = crate::registries::Jar::read(&jar).ok()?;
        let regs = crate::registries::load_jar(&jar, &data);
        let pack = DataPack::from_jar(&jar);
        let items = Items::new(data, &regs.tags);
        let recipes = Recipes::load(&pack, &items);
        let ench_reg = regs.entries("minecraft:enchantment").to_vec();
        let st = Data::load(&pack, &items, &recipes, &ench_reg);
        Some(World { items, pack, st, ench_reg })
    })
    .as_ref()
}

struct TestEnv<'a> {
    w: &'a World,
    creative: bool,
    level: i32,
    events: Vec<Ev>,
    dropped: Vec<Stack>,
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
    fn drop_item(&mut self, s: Stack, _thrown: bool) {
        self.dropped.push(s);
    }
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
    fn hashed(&self, _s: Option<&Stack>) -> Option<HashedStack> {
        None
    }
    fn st_data(&self) -> Option<&Data> {
        Some(&self.w.st)
    }
    fn xp_level(&self) -> i32 {
        self.level
    }
    fn station_event(&mut self, ev: Ev) {
        self.events.push(ev);
    }
}

fn st(id: &str, count: i32) -> Stack {
    Stack { id: format!("minecraft:{id}"), count, components: None }
}

fn with_ench(mut s: Stack, key: &str, list: &[(&str, i32)]) -> Stack {
    let f = list.iter().map(|(e, l)| (format!("minecraft:{e}"), Nbt::Int(*l))).collect();
    set_component(&mut s, key, Nbt::Compound(f));
    s
}

fn potion(id: &str, p: &str) -> Stack {
    let mut s = st(id, 1);
    set_component(&mut s, "minecraft:potion_contents", Nbt::Compound(vec![("potion".into(), Nbt::String(p.into()))]));
    s
}

#[test]
fn enchant_order_matches_registry() {
    let Some(w) = world() else { return };
    let ids: Vec<&str> = w.st.ench.list.iter().map(|e| e.id.as_str()).collect();
    let reg: Vec<&str> = w.ench_reg.iter().map(String::as_str).collect();
    assert_eq!(ids, reg);
    assert_eq!(w.st.ench.tag("minecraft:in_enchanting_table").len(), 36);
}

#[test]
fn brewing_recipes() {
    let Some(w) = world() else { return };
    let water = potion("potion", "minecraft:water");
    let r = w.st.brew(&w.items, &water, &st("nether_wart", 1)).expect("water + wart");
    assert_eq!(crate::potions::potion_of(&stations::brew_output(r)).as_deref(), Some("minecraft:awkward"));
    let awk = potion("potion", "minecraft:awkward");
    let r = w.st.brew(&w.items, &awk, &st("blaze_powder", 1)).expect("awkward + powder");
    assert_eq!(crate::potions::potion_of(&stations::brew_output(r)).as_deref(), Some("minecraft:strength"));
    let r = w.st.brew(&w.items, &water, &st("gunpowder", 1)).expect("water + gunpowder");
    assert_eq!(stations::brew_output(r).id, "minecraft:splash_potion");
    let r = w.st.brew(&w.items, &water, &st("blaze_powder", 1)).expect("water + blaze powder");
    assert_eq!(crate::potions::potion_of(&stations::brew_output(r)).as_deref(), Some("minecraft:mundane"));
    assert!(w.st.brew(&w.items, &awk, &st("nether_wart", 1)).is_none());
    assert!(w.st.is_reagent(&w.items, &st("nether_wart", 3)));
    assert!(w.st.is_potion_input(&w.items, &st("glass_bottle", 1)));
}

#[test]
fn effect_update_keeps_hidden() {
    let mut a = Inst::new("minecraft:speed", 1000, 0);
    // Stronger but shorter: the old one stays hidden behind.
    assert!(a.update(&Inst::new("minecraft:speed", 200, 1)));
    assert_eq!((a.amp, a.dur), (1, 200));
    assert_eq!(a.hidden.as_ref().map(|h| (h.amp, h.dur)), Some((0, 1000)));
    // Same level longer: duration extended.
    assert!(a.update(&Inst::new("minecraft:speed", 300, 1)));
    assert_eq!(a.dur, 300);
    // Weaker and shorter: nothing.
    assert!(!a.update(&Inst::new("minecraft:speed", 10, 0)));
    let n = a.to_nbt();
    assert_eq!(Inst::from_nbt(&n), Some(a));
}

#[test]
fn potion_effects_scaled() {
    let s = potion("lingering_potion", "minecraft:strong_poison");
    let e = crate::potions::stack_effects(&s, 0.25);
    assert_eq!((e[0].id.as_str(), e[0].dur, e[0].amp), ("minecraft:poison", 108, 1));
}

fn anvil<'a>(w: &'a World, a: Stack, b: Option<Stack>, name: Option<&str>, level: i32) -> (Menu, TestEnv<'a>) {
    let mut env = TestEnv { w, creative: false, level, events: Vec::new(), dropped: Vec::new() };
    let mut m = stations::open(1, St::Anvil, vec![Some(a), b]);
    let mut inv = vec![None; crate::player::MENU_SLOTS];
    m.st.name = name.map(str::to_owned);
    stations::slots_changed(&mut m, &mut env, &mut inv);
    (m, env)
}

#[test]
fn anvil_costs() {
    let Some(w) = world() else { return };
    // Sword + Sharpness III book: 3 levels (book cost halved, at least 1).
    let book = with_ench(st("enchanted_book", 1), "minecraft:stored_enchantments", &[("sharpness", 3)]);
    let (m, _) = anvil(w, st("diamond_sword", 1), Some(book.clone()), None, 30);
    assert_eq!(m.data[0], 3);
    let out = m.own[2].clone().expect("result");
    assert_eq!(w.items.enchantment(&out, "minecraft:sharpness"), 3);
    assert_eq!(component(&out, "minecraft:repair_cost").and_then(Nbt::as_i64), Some(1));
    // With a name: +1; the default English name costs nothing.
    let (m, _) = anvil(w, st("diamond_sword", 1), Some(book), Some("Blade"), 30);
    assert_eq!(m.data[0], 4);
    let (m, _) = anvil(w, st("diamond_sword", 1), None, Some("Diamond Sword"), 30);
    assert_eq!(m.data[0], 0);
    assert!(m.own[2].is_none());
    let (m, _) = anvil(w, st("diamond_sword", 1), None, Some("Blade"), 30);
    assert_eq!(m.data[0], 1);
    // Repair: iron pickaxe (250) worn by 200 + 5 ingots → 4 ingots (62 × 3 then 14).
    let mut pick = st("iron_pickaxe", 1);
    crate::lootfn::set_damage(&mut pick, 200);
    let (m, _) = anvil(w, pick, Some(st("iron_ingot", 5)), None, 30);
    assert_eq!(m.data[0], 4);
    assert_eq!(m.st.repair_count, 4);
    assert_eq!(w.items.damage(m.own[2].as_ref().unwrap()), 0);
    // Two Sharpness II swords: Sharpness III, 1×3 + 0 tax.
    let s2 = with_ench(st("iron_sword", 1), "minecraft:enchantments", &[("sharpness", 2)]);
    let (m, _) = anvil(w, s2.clone(), Some(s2), None, 30);
    assert_eq!(m.data[0], 3);
    // Incompatible (Smite on Sharpness): +1 per conflict, no addition possible → refusal.
    let smite = with_ench(st("enchanted_book", 1), "minecraft:stored_enchantments", &[("smite", 1)]);
    let sharp = with_ench(st("iron_sword", 1), "minecraft:enchantments", &[("sharpness", 1)]);
    let (m, _) = anvil(w, sharp, Some(smite), None, 30);
    assert!(m.own[2].is_none());
}

#[test]
fn anvil_take_spends_levels() {
    let Some(w) = world() else { return };
    let book = with_ench(st("enchanted_book", 1), "minecraft:stored_enchantments", &[("unbreaking", 3)]);
    let (mut m, mut env) = anvil(w, st("diamond_pickaxe", 1), Some(book), None, 2);
    // Unbreaking: cost 2 → book 1 × 3 = 3 levels; 2 levels are not enough.
    assert_eq!(m.data[0], 3);
    let mut inv = vec![None; crate::player::MENU_SLOTS];
    m.click(&mut env, &mut inv, 2, 0, mode::PICKUP);
    assert!(m.carried.is_none());
    env.level = 3;
    m.click(&mut env, &mut inv, 2, 0, mode::PICKUP);
    assert!(m.carried.is_some());
    assert!(m.own[0].is_none() && m.own[1].is_none());
    assert!(env.events.contains(&Ev::SpendLevels(3)) && env.events.contains(&Ev::AnvilUsed));
}

#[test]
fn grindstone_removes_enchants() {
    let Some(w) = world() else { return };
    let s = with_ench(st("diamond_sword", 1), "minecraft:enchantments", &[("sharpness", 3), ("vanishing_curse", 1)]);
    let out = stations::grindstone_result(&w.items, Some(&w.st), Some(&s), None).unwrap();
    assert_eq!(w.items.enchantment(&out, "minecraft:sharpness"), 0);
    assert_eq!(w.items.enchantment(&out, "minecraft:vanishing_curse"), 1);
    assert_eq!(component(&out, "minecraft:repair_cost").and_then(Nbt::as_i64), Some(1));
    let book = with_ench(st("enchanted_book", 1), "minecraft:stored_enchantments", &[("mending", 1)]);
    let out = stations::grindstone_result(&w.items, Some(&w.st), Some(&book), None).unwrap();
    assert_eq!(out.id, "minecraft:book");
}

#[test]
fn smithing_netherite_and_trim() {
    let Some(w) = world() else { return };
    let mut env = TestEnv { w, creative: false, level: 0, events: Vec::new(), dropped: Vec::new() };
    let mut inv = vec![None; crate::player::MENU_SLOTS];
    let sword = with_ench(st("diamond_sword", 1), "minecraft:enchantments", &[("sharpness", 5)]);
    let mut m = stations::open(
        1,
        St::Smithing,
        vec![Some(st("netherite_upgrade_smithing_template", 1)), Some(sword), Some(st("netherite_ingot", 2))],
    );
    stations::slots_changed(&mut m, &mut env, &mut inv);
    let out = m.own[3].clone().expect("netherite sword");
    assert_eq!(out.id, "minecraft:netherite_sword");
    assert_eq!(w.items.enchantment(&out, "minecraft:sharpness"), 5);
    m.click(&mut env, &mut inv, 3, 0, mode::PICKUP);
    assert_eq!(m.own[2].as_ref().map(|s| s.count), Some(1));
    assert!(m.own[0].is_none() && m.own[1].is_none());
    // Trim.
    let mut m = stations::open(
        1,
        St::Smithing,
        vec![
            Some(st("coast_armor_trim_smithing_template", 1)),
            Some(st("iron_chestplate", 1)),
            Some(st("gold_ingot", 1)),
        ],
    );
    stations::slots_changed(&mut m, &mut env, &mut inv);
    let out = m.own[3].clone().expect("trimmed chestplate");
    let trim = component(&out, "minecraft:trim").unwrap();
    assert_eq!(trim.get("pattern").and_then(Nbt::as_str), Some("minecraft:coast"));
    assert_eq!(trim.get("material").and_then(Nbt::as_str), Some("minecraft:gold"));
}

#[test]
fn stonecutter_select_and_take() {
    let Some(w) = world() else { return };
    let mut env = TestEnv { w, creative: false, level: 0, events: Vec::new(), dropped: Vec::new() };
    let mut inv = vec![None; crate::player::MENU_SLOTS];
    let mut m = stations::open(1, St::Stonecutter, Vec::new());
    inv[36] = Some(st("stone", 3));
    // Quick move from the hotbar to the input.
    m.click(&mut env, &mut inv, 29, 0, mode::QUICK_MOVE);
    assert_eq!(m.own[0].as_ref().map(|s| s.count), Some(3));
    assert!(!m.st.cut.is_empty());
    let k = m.st.cut.iter().position(|&i| w.st.stonecut[i].1.id == "minecraft:stone_slab").unwrap() as i32;
    assert!(stations::stonecutter_select(&mut m, &mut env, k));
    assert_eq!(m.own[1].as_ref().map(|s| (s.id.as_str(), s.count)), Some(("minecraft:stone_slab", 2)));
    m.click(&mut env, &mut inv, 1, 0, mode::QUICK_MOVE);
    // Three stones → six slabs, input empty, result emptied.
    let slabs: i32 = inv.iter().flatten().filter(|s| s.id == "minecraft:stone_slab").map(|s| s.count).sum();
    assert_eq!(slabs, 6);
    assert!(m.own[0].is_none() && m.own[1].is_none());
}

#[test]
fn special_crafting() {
    let Some(w) = world() else { return };
    let recipes = Recipes::load(&w.pack, &w.items);
    // Star: powder + red dye + gold nugget (star shape) + diamond (trail).
    let grid =
        vec![Some(st("gunpowder", 1)), Some(st("red_dye", 1)), Some(st("gold_nugget", 1)), Some(st("diamond", 1))];
    let (_, star) = recipes.craft(&w.items, &grid, 2, &|_| None).expect("star");
    let e = component(&star, "minecraft:firework_explosion").unwrap();
    assert_eq!(e.get("shape").and_then(Nbt::as_str), Some("star"));
    assert_eq!(e.get("colors"), Some(&Nbt::IntArray(vec![11743532])));
    // Rocket: paper + 2 powders + star → 3 flight 2 rockets.
    let grid = vec![Some(st("paper", 1)), Some(st("gunpowder", 1)), Some(st("gunpowder", 1)), Some(star)];
    let (_, rocket) = recipes.craft(&w.items, &grid, 2, &|_| None).expect("rocket");
    assert_eq!(rocket.count, 3);
    let f = component(&rocket, "minecraft:fireworks").unwrap();
    assert_eq!(f.get("flight_duration").and_then(Nbt::as_i64), Some(2));
    // Banner copy: the patterned banner stays in the grid.
    let mut src = st("red_banner", 1);
    set_component(
        &mut src,
        "minecraft:banner_patterns",
        Nbt::List(vec![Nbt::Compound(vec![
            ("pattern".into(), Nbt::String("minecraft:stripe_top".into())),
            ("color".into(), Nbt::String("white".into())),
        ])]),
    );
    let grid = vec![Some(src.clone()), Some(st("red_banner", 1)), None, None];
    let (_, out) = recipes.craft(&w.items, &grid, 2, &|_| None).expect("banner copy");
    assert_eq!(component(&out, "minecraft:banner_patterns"), component(&src, "minecraft:banner_patterns"));
    let rem = recipes.remaining(&w.items, &grid, &|_| None);
    assert_eq!(rem[0].as_ref().map(|s| s.id.as_str()), Some("minecraft:red_banner"));
    assert!(rem[1].is_none());
}

#[test]
fn table_costs_follow_seed() {
    let Some(w) = world() else { return };
    let sword = st("diamond_sword", 1);
    let v = w.st.ench.table_view(&w.items, 12345, 15, Some(&sword));
    assert!(v.costs[2] >= 30 && v.costs[0] >= 1, "{v:?}");
    assert!(v.clue.iter().all(|&c| c >= 0));
    // Official server reading (player with seed 12345, 26 bookshelves): costs 6/20/30,
    // indices Sharpness I, Bane of Arthropods III, Knockback II.
    let names: Vec<&str> = v.clue.iter().map(|&c| w.st.ench.list[c as usize].id.as_str()).collect();
    assert_eq!(v.costs, [6, 20, 30]);
    assert_eq!(names, ["minecraft:sharpness", "minecraft:bane_of_arthropods", "minecraft:knockback"]);
    assert_eq!(v.level, [1, 3, 2]);
    // Button 3: full list drawn (official: Sharpness III, Looting III, Sweeping Edge III, Knockback II,
    // Fire Aspect II).
    let mut got: Vec<(String, i32)> =
        w.st.ench
            .table_list(&w.items, 12345, 2, 30, &sword)
            .iter()
            .map(|&(i, l)| (w.st.ench.list[i].id.clone(), l))
            .collect();
    got.sort();
    let mut want: Vec<(String, i32)> =
        [("sharpness", 3), ("looting", 3), ("sweeping_edge", 3), ("knockback", 2), ("fire_aspect", 2)]
            .iter()
            .map(|(e, l)| (format!("minecraft:{e}"), *l))
            .collect();
    want.sort();
    assert_eq!(got, want);
    // Same seed, same view; book accepted, already enchanted item refused.
    assert_eq!(v, w.st.ench.table_view(&w.items, 12345, 15, Some(&sword)));
    let book = st("book", 1);
    assert!(w.st.ench.table_view(&w.items, 7, 0, Some(&book)).costs[0] > 0);
    let done = with_ench(st("diamond_sword", 1), "minecraft:enchantments", &[("sharpness", 1)]);
    assert_eq!(w.st.ench.table_view(&w.items, 7, 15, Some(&done)).costs, [0; 3]);
    if std::env::var_os("RUSTCRAFT_S7_DUMP").is_some() {
        for seed in [0, 1, 12345, -987_654_321] {
            for b in [0, 8, 15] {
                let v = w.st.ench.table_view(&w.items, seed, b, Some(&sword));
                let names: Vec<String> = v
                    .clue
                    .iter()
                    .map(|&c| if c < 0 { "-".into() } else { w.st.ench.list[c as usize].id.clone() })
                    .collect();
                eprintln!(
                    "table seed {seed} bookshelves {b}: costs {:?} indices {names:?} levels {:?}",
                    v.costs, v.level
                );
            }
        }
    }
}

/// Fixed-seed chests: printed reading (`RUSTCRAFT_S7_DUMP`) for comparison with the
/// official server; consistency check otherwise.
#[test]
fn seeded_chest_loot() {
    let Some(w) = world() else { return };
    let tag_fn = |_: &str, _: &str| false;
    let loot = crate::loot::Loot { pack: &w.pack, items: &w.items, block_tag: &tag_fn };
    let ctx = crate::loot::Ctx::default();
    for (table, seed) in [
        ("minecraft:chests/simple_dungeon", 42i64),
        ("minecraft:chests/end_city_treasure", 7),
        ("minecraft:chests/stronghold_library", 123_456_789),
        ("minecraft:chests/buried_treasure", -5),
        ("minecraft:chests/ancient_city", 99),
    ] {
        let mut rng = crate::mobs::jrand::JRand::new(seed);
        let stacks = loot.drops(table, &ctx, &mut rng);
        let mut items = vec![None; 27];
        crate::lootchest::fill(&mut items, stacks, &mut rng);
        assert!(items.iter().flatten().count() > 0, "{table}");
        // Readings of the official 26.3 server (same seed, `execute if items` then `data get`).
        let expect: &[(usize, &str, i32)] = match seed {
            42 => &[
                (2, "leather", 1),
                (3, "gunpowder", 1),
                (6, "string", 1),
                (10, "rotten_flesh", 2),
                (16, "leather", 2),
                (19, "string", 3),
                (22, "name_tag", 1),
                (26, "rotten_flesh", 1),
            ],
            7 => &[(9, "iron_pickaxe", 1), (10, "iron_shovel", 1), (16, "diamond_chestplate", 1)],
            _ => &[],
        };
        for (slot, id, n) in expect {
            let s = items[*slot].as_ref().unwrap_or_else(|| panic!("{table} slot {slot} empty"));
            assert_eq!((s.id.as_str(), s.count), (format!("minecraft:{id}").as_str(), *n), "{table} slot {slot}");
        }
        if seed == 7 {
            let shovel = items[10].as_ref().unwrap();
            for (e, l) in [("efficiency", 3), ("unbreaking", 3), ("silk_touch", 1)] {
                assert_eq!(w.items.enchantment(shovel, &format!("minecraft:{e}")), l);
            }
            assert_eq!(w.items.enchantment(items[16].as_ref().unwrap(), "minecraft:protection"), 4);
        }
        if std::env::var_os("RUSTCRAFT_S7_DUMP").is_some() {
            eprintln!("== {table} {seed}");
            for (i, s) in items.iter().enumerate() {
                if let Some(s) = s {
                    eprintln!("  {i}: {} x{} {:?}", s.id, s.count, s.components);
                }
            }
        }
    }
}
