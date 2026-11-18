//! Index of the criteria by identifier: what each criterion shape gives, on a small pack and on
//! the 26.3 tree. The equivalence with the full scan on a live server is in `tests_indexes.rs`.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::{INVENTORY_CHANGED, item_key};
use crate::progress::Tree;

fn one(trigger: &str, conditions: Value) -> Value {
    json!({ "criteria": { "c": { "trigger": trigger, "conditions": conditions } } })
}

fn inv(items: Value) -> Value {
    one(INVENTORY_CHANGED, json!({ "items": items }))
}

/// Criteria of the small pack, by advancement name.
fn pack() -> Tree {
    let advs = [
        ("t:a_id", inv(json!([{ "items": "minecraft:stone" }]))),
        ("t:b_short", inv(json!([{ "items": "oak_log" }]))),
        ("t:c_empty_namespace", inv(json!([{ "items": ":birch_log" }]))),
        ("t:d_list", inv(json!([{ "items": ["minecraft:dirt", "dirt", "c:dust"] }]))),
        ("t:e_tag", inv(json!([{ "items": "#minecraft:planks" }]))),
        ("t:f_tag_in_list", inv(json!([{ "items": ["#minecraft:logs"] }]))),
        ("t:g_no_key", inv(json!([{ "count": 2 }]))),
        ("t:h_two_items", inv(json!([{ "items": "minecraft:stick" }, { "items": "minecraft:coal" }]))),
        ("t:i_no_items", one(INVENTORY_CHANGED, json!({ "slots": { "empty": { "min": 0 } } }))),
        ("t:j_player", one(INVENTORY_CHANGED, json!({ "items": [{ "items": "minecraft:stone" }], "player": [] }))),
        ("t:k_bad_key", inv(json!([{ "items": 5 }]))),
        ("t:l_count", inv(json!([{ "items": "minecraft:stone", "count": { "min": 2 } }]))),
        ("t:m_recipe", one("minecraft:recipe_unlocked", json!({ "recipes": "oak_planks" }))),
        ("t:n_recipes", one("minecraft:recipe_unlocked", json!({ "recipes": ["minecraft:stick", "stick", 3] }))),
        ("t:o_recipe_number", one("minecraft:recipe_unlocked", json!({ "recipes": 3 }))),
    ];
    let raw: BTreeMap<String, Value> = advs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
    Tree::build(raw)
}

/// Advancement names of the candidates for `id` (tag `#minecraft:planks` held by `oak_planks` only).
fn names(t: &Tree, trigger: &str, id: Option<&str>) -> Vec<String> {
    let list = &t.by_trigger[trigger];
    let mut out = Vec::new();
    let has_tag = |tag: &str, item: &str| tag == "minecraft:planks" && item == "minecraft:oak_planks";
    t.indexed[trigger].candidates(id, has_tag, &mut out);
    out.iter().map(|&p| t.list[list[p as usize].0].id.clone()).collect()
}

#[test]
fn each_criterion_shape_lands_in_its_group() {
    let t = pack();
    // Tested for every change: no key, two items, no items, a player condition, a bad key.
    let always = ["t:g_no_key", "t:h_two_items", "t:i_no_items", "t:j_player", "t:k_bad_key"];
    assert_eq!(names(&t, INVENTORY_CHANGED, None), always, "emptied slot");
    let with = |extra: &[&str]| {
        let mut v: Vec<String> = always.iter().chain(extra).map(|s| (*s).to_owned()).collect();
        v.sort();
        v
    };
    assert_eq!(names(&t, INVENTORY_CHANGED, Some("minecraft:stone")), with(&["t:a_id", "t:l_count"]));
    assert_eq!(names(&t, INVENTORY_CHANGED, Some("minecraft:oak_log")), with(&["t:b_short"]));
    assert_eq!(names(&t, INVENTORY_CHANGED, Some("minecraft:birch_log")), with(&["t:c_empty_namespace"]));
    assert_eq!(names(&t, INVENTORY_CHANGED, Some("minecraft:dirt")), with(&["t:d_list"]), "list kept once");
    assert_eq!(names(&t, INVENTORY_CHANGED, Some("c:dust")), with(&["t:d_list"]));
    assert_eq!(names(&t, INVENTORY_CHANGED, Some("minecraft:oak_planks")), with(&["t:e_tag"]));
    // A tag inside a list is an identifier (namespace `#minecraft`): only an item of that name (none).
    assert_eq!(names(&t, INVENTORY_CHANGED, Some("#minecraft:logs")), with(&["t:f_tag_in_list"]));
    assert_eq!(names(&t, INVENTORY_CHANGED, Some("minecraft:oak_wood")), with(&[]));
    // Recipes: one identifier, a list (short form and repeat kept once), anything else never.
    let r = "minecraft:recipe_unlocked";
    assert_eq!(names(&t, r, Some("minecraft:oak_planks")), ["t:m_recipe"]);
    assert_eq!(names(&t, r, Some("minecraft:stick")), ["t:n_recipes"]);
    assert_eq!(names(&t, r, Some("oak_planks")), Vec::<String>::new(), "looked up by the full identifier");
}

#[test]
fn item_keys_are_full_identifiers() {
    assert!(
        matches!(item_key(&json!({"items": [{"items": "stone"}]})), super::Key::Ids(v) if v == ["minecraft:stone"])
    );
    assert!(matches!(item_key(&json!({"items": [{"items": "#c:ores"}]})), super::Key::Tag(t) if t == "c:ores"));
    assert!(matches!(item_key(&json!({"items": []})), super::Key::Always));
}

/// 26.3: the ~1,800 criteria naming one item go to ~540 identifier groups, ~100 to 36 tags, a
/// handful tested for every change; recipes all by identifier.
#[test]
fn base_game_tree_is_mostly_indexed() {
    let Some(data) = rc_testdata::version_data() else { return };
    let Some(jar) = rc_testdata::need(crate::registries::client_jar(&data), "26.3 client jar") else { return };
    let t = Tree::load(&crate::registries::Jar::read(&jar).expect("jar"));
    let (ids, tags, always) = t.indexed[INVENTORY_CHANGED].sizes();
    assert!(ids > 400 && (30..=60).contains(&tags) && always <= 10, "inventory index: {ids} {tags} {always}");
    let (ids, tags, always) = t.indexed["minecraft:recipe_unlocked"].sizes();
    assert!(ids > 1_500 && tags == 0 && always == 0, "recipe index: {ids} {tags} {always}");
}
