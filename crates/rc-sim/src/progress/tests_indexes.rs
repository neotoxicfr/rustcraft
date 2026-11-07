//! Criteria indexes (`RUSTCRAFT_OPT_ADVINDEX`, `RUSTCRAFT_OPT_RECIPEINDEX`) against the full scan,
//! on a fresh flat world with the 26.3 tree plus a pack of odd criteria: every indexed event is
//! checked against the full scan of its trigger (same hits, same order), then the whole walk is
//! compared with the old paths (criteria granted in the same order, same packets, same file).

use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::paths::ProgressPaths;
use crate::blocks::probe::Probe;
use crate::player::Stack;

fn one(trigger: &str, conditions: Value) -> Value {
    json!({ "criteria": { "c": { "trigger": trigger, "conditions": conditions } } })
}

fn inv(items: Value) -> Value {
    one("minecraft:inventory_changed", json!({ "items": items }))
}

/// Criteria shapes the base game does not write, all reachable by the walks below.
fn odd_pack() -> Vec<(&'static str, Value)> {
    vec![
        ("t:list", inv(json!([{ "items": ["minecraft:stone", "dirt", "minecraft:stone"] }]))),
        ("t:tag_in_list", inv(json!([{ "items": ["#minecraft:logs"] }]))),
        ("t:short", inv(json!([{ "items": "oak_log" }]))),
        ("t:empty_namespace", inv(json!([{ "items": ":birch_log" }]))),
        ("t:tag", inv(json!([{ "items": "#minecraft:planks" }]))),
        ("t:short_tag", inv(json!([{ "items": "#wool" }]))),
        ("t:no_key", inv(json!([{ "count": { "min": 2 } }]))),
        ("t:count", inv(json!([{ "items": "minecraft:cobblestone", "count": { "min": 2 } }]))),
        ("t:components", inv(json!([{ "items": "minecraft:diamond_sword", "components": { "minecraft:damage": 5 } }]))),
        ("t:air", inv(json!([{ "items": "minecraft:air" }]))),
        (
            "t:player",
            one(
                "minecraft:inventory_changed",
                json!({ "items": [{ "items": "minecraft:gold_ingot" }],
                        "player": [{ "condition": "minecraft:entity_properties", "entity": "this",
                                     "predicate": { "flags": { "is_sneaking": false } } }] }),
            ),
        ),
        ("t:two", inv(json!([{ "items": "minecraft:stick" }, { "items": "minecraft:coal" }]))),
        ("t:slots", one("minecraft:inventory_changed", json!({ "slots": { "empty": { "min": 40 } } }))),
        // Only an emptied slot of an emptied inventory reaches it (the criteria tested for every change).
        ("t:all_empty", one("minecraft:inventory_changed", json!({ "slots": { "occupied": 0 } }))),
        ("t:bad_key", inv(json!([{ "items": 5 }]))),
        ("t:empty_list", inv(json!([{ "items": [] }]))),
        ("t:recipe_short", one("minecraft:recipe_unlocked", json!({ "recipes": "oak_planks" }))),
        (
            "t:recipe_list",
            one("minecraft:recipe_unlocked", json!({ "recipes": ["minecraft:stick", "stick", "torch"] })),
        ),
        ("t:recipe_tag", one("minecraft:recipe_unlocked", json!({ "recipes": "#minecraft:planks" }))),
        ("t:recipe_number", one("minecraft:recipe_unlocked", json!({ "recipes": 3 }))),
        ("t:crafted_short", one("minecraft:recipe_crafted", json!({ "recipes": "stick" }))),
        (
            "t:loot_list",
            one("minecraft:player_generates_container_loot", json!({ "loot_tables": ["chests/igloo_chest"] })),
        ),
    ]
}

/// The 26.3 tree with the odd pack.
fn tree_with_pack(p: &Probe) -> std::sync::Arc<super::Tree> {
    let jar = crate::registries::client_jar(&p.s.data).expect("26.3 client jar");
    let jar = crate::registries::Jar::read(&jar).expect("jar");
    let mut raw: BTreeMap<String, Value> = super::read_advancement_files(&jar, false);
    raw.extend(odd_pack().into_iter().map(|(k, v)| (k.to_owned(), v)));
    std::sync::Arc::new(super::Tree::build(raw))
}

/// What a walk left: criteria granted in order, advancement packets (dates as presence), file
/// (dates zeroed).
#[derive(Debug, PartialEq)]
struct Outcome {
    granted: Vec<(usize, usize)>,
    packets: Vec<String>,
    file: String,
}

fn packets(p: &Probe) -> Vec<String> {
    let mut out = Vec::new();
    for e in p.take_events() {
        if let rc_client::Event::Progress(rc_client::progress::Progress::Advancements(u)) = e {
            let added: Vec<&str> = u.added.iter().map(|a| a.id.as_str()).collect();
            let progress: Vec<(String, Vec<(String, bool)>)> = u
                .progress
                .iter()
                .map(|(id, c)| (id.clone(), c.iter().map(|(n, d)| (n.clone(), d.is_some())).collect()))
                .collect();
            out.push(format!("{} {added:?} {:?} {progress:?}", u.reset, u.removed));
        }
    }
    out
}

fn outcome(p: &mut Probe) -> Outcome {
    p.s.adv_flush(true);
    let packets = packets(p);
    for crits in &mut p.s.prog.got {
        for d in crits.iter_mut().filter(|d| d.is_some()) {
            *d = Some(0);
        }
    }
    Outcome { granted: std::mem::take(&mut p.s.prog.grant_log), packets, file: p.s.prog.to_json(4000) }
}

/// Fresh flat world on `paths`, the tree with the pack, every other advancement already done when
/// `half_done`.
fn open(tag: &str, paths: ProgressPaths, half_done: bool) -> (Probe, std::path::PathBuf) {
    let (mut p, dir) = ProgressPaths::forced(paths, || crate::tests_g3f13::open(tag));
    assert_eq!(p.s.prog.paths, paths);
    let tree = tree_with_pack(&p);
    p.s.prog.reload(tree);
    if half_done {
        for a in (0..p.s.prog.tree.list.len()).step_by(2) {
            for c in 0..p.s.prog.tree.list[a].criteria.len() {
                p.s.prog.grant(a, c);
            }
        }
    }
    p.s.adv_flush(true);
    let _ = p.take_events();
    p.s.prog.grant_log.clear();
    p.s.prog.inv_seen = p.s.slots.clone();
    (p, dir)
}

/// Every item of the registry put in the hotbar (some by two, some taken back at once), a damaged
/// sword, gold, stick and coal together, then the whole inventory emptied (death, `/clear`).
fn inventory_walk(p: &mut Probe) {
    let mut items = p.s.data.registry("minecraft:item").map(|r| r.entries.clone()).unwrap_or_default();
    assert!(items.len() > 1_000, "item registry: {}", items.len());
    // An air stack is an empty slot in the game: never put in a slot.
    items.retain(|id| id != "minecraft:air");
    for (k, id) in items.iter().enumerate() {
        let slot = 36 + k % 9;
        p.s.slots[slot] = Some(Stack { id: id.clone(), count: 1 + (k % 3 == 0) as i32, components: None });
        p.s.trig_inventory_scan();
        if k % 5 == 0 {
            p.s.slots[slot] = None;
            p.s.trig_inventory_scan();
        }
    }
    let damaged = rc_proto::nbt::Nbt::Compound(vec![("minecraft:damage".into(), rc_proto::nbt::Nbt::Int(5))]);
    let extra = [
        Stack { id: "minecraft:diamond_sword".into(), count: 1, components: Some(damaged) },
        Stack { id: "minecraft:gold_ingot".into(), count: 3, components: None },
        Stack { id: "minecraft:stick".into(), count: 1, components: None },
        Stack { id: "minecraft:coal".into(), count: 1, components: None },
        Stack { id: "minecraft:cobblestone".into(), count: 2, components: None },
    ];
    for (i, s) in extra.into_iter().enumerate() {
        p.s.slots[9 + i] = Some(s);
        p.s.trig_inventory_scan();
    }
    for s in p.s.slots.iter_mut() {
        *s = None;
    }
    p.s.trig_inventory_scan();
}

/// `/recipe give @s *`, `/recipe take @s *`, then every recipe again by batches of 100; crafted
/// recipes and generated loot, odd forms included.
fn recipe_walk(p: &mut Probe) {
    let mut src = p.s.player_source();
    src.level = 2;
    assert!(p.s.perform_as(&src, "recipe give @s *").0, "recipe give refused");
    assert!(p.s.perform_as(&src, "recipe take @s *").0, "recipe take refused");
    let ids: Vec<String> = p.s.life.recipes.list.iter().map(|r| r.id.clone()).collect();
    assert!(ids.len() > 1_000, "recipes: {}", ids.len());
    for batch in ids.chunks(100) {
        p.s.award_recipes(batch);
    }
    for id in ["minecraft:stick", "minecraft:bolt_armor_trim_smithing_template_smithing_trim", "stick"] {
        p.s.trig_recipe_crafted("minecraft:recipe_crafted", id, &[]);
    }
    for table in ["minecraft:chests/igloo_chest", "minecraft:chests/bastion_treasure", "chests/igloo_chest"] {
        p.s.trig_container_loot(table);
    }
}

fn run(tag: &str, paths: ProgressPaths, half_done: bool, walk: fn(&mut Probe)) -> Outcome {
    let (mut p, dir) = open(tag, paths, half_done);
    ProgressPaths::checked(|| walk(&mut p));
    let out = outcome(&mut p);
    crate::tests_g3f13::finish(p, dir);
    out
}

fn same_walk_both_ways(tag: &str, walk: fn(&mut Probe), at_least: usize) {
    if rc_testdata::version_data().is_none() {
        return;
    }
    for half_done in [false, true] {
        let new = run(&format!("{tag}-new-{half_done}"), ProgressPaths::NEW, half_done, walk);
        let old = run(&format!("{tag}-old-{half_done}"), ProgressPaths::OLD, half_done, walk);
        assert!(new.granted.len() >= at_least, "{tag}: only {} criteria granted", new.granted.len());
        assert_eq!(new.granted, old.granted, "{tag} (half done {half_done}): criteria granted");
        assert_eq!(new.packets, old.packets, "{tag} (half done {half_done}): packets");
        assert_eq!(new.file, old.file, "{tag} (half done {half_done}): file");
    }
}

#[test]
fn item_index_gives_the_hits_of_the_full_scan() {
    same_walk_both_ways("adv-index-items", inventory_walk, 300);
}

#[test]
fn recipe_index_gives_the_hits_of_the_full_scan() {
    same_walk_both_ways("adv-index-recipes", recipe_walk, 800);
}

/// The odd pack's criteria were reached by the walks (otherwise their shapes proved nothing).
#[test]
fn odd_criteria_are_reached() {
    if rc_testdata::version_data().is_none() {
        return;
    }
    let (mut p, dir) = open("adv-index-odd", ProgressPaths::NEW, false);
    ProgressPaths::checked(|| {
        inventory_walk(&mut p);
        recipe_walk(&mut p);
    });
    let done = |id: &str| p.s.prog.tree.get(id).is_some_and(|a| p.s.prog.done(a));
    for id in [
        "t:list",
        "t:short",
        "t:empty_namespace",
        "t:tag",
        "t:short_tag",
        "t:no_key",
        "t:count",
        "t:components",
        "t:player",
        "t:two",
        "t:slots",
        "t:all_empty",
        "t:recipe_short",
        "t:recipe_list",
        "t:crafted_short",
        "t:loot_list",
    ] {
        assert!(done(id), "{id} not reached");
    }
    for id in ["t:tag_in_list", "t:bad_key", "t:empty_list", "t:recipe_tag", "t:recipe_number", "t:air"] {
        assert!(!done(id), "{id} reached (never in the game)");
    }
    crate::tests_g3f13::finish(p, dir);
}

/// An empty slot read without building an air stack gives the answers of the air stack.
#[test]
fn empty_slot_predicate_matches_the_air_stack() {
    if rc_testdata::version_data().is_none() {
        return;
    }
    let (p, dir) = open("adv-empty-slot", ProgressPaths::NEW, false);
    let air = Stack { id: "minecraft:air".into(), count: 0, components: None };
    let preds = [
        json!({}),
        json!({ "items": "minecraft:air" }),
        json!({ "items": "air" }),
        json!({ "items": "minecraft:stone" }),
        json!({ "items": "#minecraft:planks" }),
        json!({ "count": 0 }),
        json!({ "count": { "min": 1 } }),
        json!({ "components": { "minecraft:max_stack_size": 64 } }),
        json!({ "components": {} }),
        json!({ "predicates": { "minecraft:damage": {} } }),
        json!({ "items": "minecraft:air", "predicates": {} }),
    ];
    for pred in &preds {
        assert_eq!(p.s.p_empty_item(pred), p.s.p_item_of(pred, &air), "{pred}");
    }
    crate::tests_g3f13::finish(p, dir);
}

/// Several items: the unmatched ones kept as bits give the answers of the list, on inventories
/// holding none, some or all of them (repeats, tags, counts, an item matching two predicates).
#[test]
fn several_items_as_bits_match_the_list() {
    if rc_testdata::version_data().is_none() {
        return;
    }
    let (p, dir) = open("adv-several-items", ProgressPaths::NEW, false);
    let stack = |id: &str, count: i32| Stack { id: format!("minecraft:{id}"), count, components: None };
    let inventories = [
        vec![],
        vec![stack("stick", 1)],
        vec![stack("stick", 1), stack("coal", 1)],
        vec![stack("coal", 2), stack("oak_planks", 1), stack("stick", 1)],
        vec![stack("oak_planks", 1)],
        vec![stack("birch_planks", 3), stack("stone", 1), stack("dirt", 1), stack("stick", 4)],
    ];
    let lists = [
        json!([{ "items": "minecraft:stick" }, { "items": "minecraft:coal" }]),
        json!([{ "items": "stick" }, { "items": "stick" }]),
        json!([{ "items": "#minecraft:planks" }, { "items": "minecraft:oak_planks" }]),
        json!([{ "items": "minecraft:stone" }, { "items": "dirt" }, { "items": "minecraft:stick", "count": { "min": 2 } }]),
        json!([{}, { "items": "minecraft:coal", "count": 2 }]),
        json!((0..64).map(|_| json!({ "items": "minecraft:stick" })).collect::<Vec<_>>()),
        json!((0..65).map(|_| json!({ "items": "minecraft:stick" })).collect::<Vec<_>>()),
    ];
    let mut found = 0;
    for inv in &inventories {
        for list in &lists {
            let items = list.as_array().unwrap();
            let bits = super::trig::all_items_found(&p.s, items, inv.iter(), true);
            assert_eq!(bits, super::trig::all_items_found(&p.s, items, inv.iter(), false), "{list} in {inv:?}");
            found += usize::from(bits);
        }
    }
    assert!((5..30).contains(&found), "found {found}");
    crate::tests_g3f13::finish(p, dir);
}
