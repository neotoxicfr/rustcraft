//! G1-F02: fields of the advancement triggers, added callers (golem, honey, effects with
//! source), persistent Nether departure, rewards and advancement validation, on a
//! copy of the test world (`RUSTCRAFT_SOLO_WORLD`). Absent: tests ignored (except validation).

use std::collections::BTreeMap;

use serde_json::json;

use super::id_in;
use crate::blocks::probe::Probe;
use crate::mobs::species::Kind;
use crate::player::Stack;

const BASE: &str = "fill 100 -60 100 131 -60 131 minecraft:stone\nfill 100 -59 100 131 -40 131 minecraft:air\n";

fn probe(tag: &str) -> Option<(Probe, std::path::PathBuf)> {
    let (world, data) = (crate::tests::test_world()?, rc_testdata::version_data()?);
    let dir = crate::tests::fresh_copy(&world, tag);
    let mut p = Probe::open(&dir, std::sync::Arc::new(data));
    p.run(BASE);
    p.s.pos = [110.5, -59.0, 110.5];
    p.s.loaded = true;
    p.s.game_mode = 0;
    p.s.life.vit.health = 20.0;
    p.s.life.vit.dead = false;
    Some((p, dir))
}

fn done(p: Probe, dir: std::path::PathBuf) {
    drop(p);
    let _ = std::fs::remove_dir_all(dir);
}

fn adv(p: &Probe, id: &str) -> bool {
    let i = *p.s.prog.tree.index.get(id).unwrap_or_else(|| panic!("advancement {id} missing"));
    p.s.prog.done(i)
}

/// `recipe_unlocked`, `recipe_crafted` and `player_generates_container_loot` read `recipes` and
/// `loot_tables` (single identifier or list).
#[test]
fn trigger_fields_recipes_and_loot_tables() {
    let Some((mut p, dir)) = probe("g1f02-fields") else { return };
    p.s.trig_recipes_unlocked(&["minecraft:beacon".to_owned()]);
    assert!(adv(&p, "minecraft:recipes/misc/beacon"), "recipe_unlocked: recipes field ignored");
    p.s.trig_recipe_crafted(
        "minecraft:recipe_crafted",
        "minecraft:bolt_armor_trim_smithing_template_smithing_trim",
        &[],
    );
    assert!(adv(&p, "minecraft:adventure/trim_with_any_armor_pattern"), "recipe_crafted: recipes field ignored");
    p.s.trig_container_loot("minecraft:chests/bastion_treasure");
    assert!(adv(&p, "minecraft:nether/loot_bastion"), "container_loot: loot_tables field ignored");
    // List of identifiers.
    assert!(id_in(Some(&json!(["minecraft:a", "b"])), "minecraft:b"));
    assert!(!id_in(Some(&json!("minecraft:a")), "minecraft:b"));
    done(p, dir);
}

/// `bee_nest_destroyed` tests `blocks`: a block other than the nest does not count.
#[test]
fn bee_nest_checks_blocks() {
    let Some((mut p, dir)) = probe("g1f02-nest") else { return };
    p.run("setblock 120 -59 120 minecraft:bee_nest\nsetblock 121 -59 120 minecraft:beehive");
    let (nest, hive) = (p.s.get([120, -59, 120]), p.s.get([121, -59, 120]));
    let ench = rc_proto::nbt::Nbt::Compound(vec![("minecraft:silk_touch".into(), rc_proto::nbt::Nbt::Int(1))]);
    let tool = Stack {
        id: "minecraft:diamond_pickaxe".into(),
        count: 1,
        components: Some(rc_proto::nbt::Nbt::Compound(vec![("minecraft:enchantments".into(), ench)])),
    };
    p.s.trig_bee_nest(hive, Some(&tool), 3);
    assert!(!adv(&p, "minecraft:husbandry/silk_touch_nest"), "beehive accepted for a nest");
    p.s.trig_bee_nest(nest, Some(&tool), 3);
    assert!(adv(&p, "minecraft:husbandry/silk_touch_nest"), "nest refused");
    done(p, dir);
}

/// Iron golem built near the player: `summoned_entity` ("Hired Help").
#[test]
fn iron_golem_build_summons() {
    let Some((mut p, dir)) = probe("g1f02-golem") else { return };
    p.run(
        "setblock 113 -59 110 minecraft:iron_block\nsetblock 113 -58 110 minecraft:iron_block\n\
         setblock 112 -58 110 minecraft:iron_block\nsetblock 114 -58 110 minecraft:iron_block\n\
         setblock 113 -57 110 minecraft:carved_pumpkin",
    );
    assert!(p.s.mobs.list.iter().flatten().any(|m| m.kind == Kind::IronGolem), "golem not built");
    assert!(adv(&p, "minecraft:adventure/summon_iron_golem"), "summoned_entity missing");
    done(p, dir);
}

/// Regeneration given then renewed (same level, longer) by an axolotl:
/// `effects_changed` with its source; the same without a source does not count.
#[test]
fn effects_changed_carries_source_and_renewal() {
    let Some((mut p, dir)) = probe("g1f02-axolotl") else { return };
    let a = p.s.new_mob(Kind::Axolotl, [112.5, -59.0, 110.5], 0.0);
    p.s.give_player_effect("minecraft:regeneration", 100, 0);
    assert!(!adv(&p, "minecraft:husbandry/kill_axolotl_target"), "obtained without a source");
    // Renewal: same level, longer duration (old sweep blind to this case).
    assert!(p.s.give_player_effect_from("minecraft:regeneration", 200, 0, Some(&a)));
    assert!(adv(&p, "minecraft:husbandry/kill_axolotl_target"), "axolotl source ignored");
    done(p, dir);
}

/// Player going down along a honey block, game tick multiple of 20: slide
/// ("Sticky Situation"); too slow or tick not a multiple of 20: nothing.
#[test]
fn honey_slide_triggers() {
    let Some((mut p, dir)) = probe("g1f02-honey") else { return };
    p.run("fill 112 -59 110 112 -55 110 minecraft:honey_block");
    p.s.pos = [111.75, -57.5, 110.5];
    p.s.on_ground = false;
    let id = "minecraft:adventure/honey_block_slide";
    p.s.time = 40;
    p.s.trig_after_move([111.75, -57.45, 110.5], false);
    assert!(!adv(&p, id), "slide too slow accepted");
    p.s.time = 41;
    p.s.trig_after_move([111.75, -57.37, 110.5], false);
    assert!(!adv(&p, id), "tick not a multiple of 20 accepted");
    p.s.time = 60;
    p.s.trig_after_move([111.75, -57.37, 110.5], false);
    assert!(adv(&p, id), "slide_down_block missing");
    done(p, dir);
}

/// Honey rule: "old" speed = movement of the tick (gravity 0.08 cancelled), slow fall:
/// gravity 0.01.
#[test]
fn honey_old_dy_matches_rule() {
    let Some((mut p, dir)) = probe("g1f02-honeydy") else { return };
    assert!((p.s.honey_old_dy(-0.13) - -0.13).abs() < 1e-12);
    p.s.add_effect(crate::effects::Inst::new("minecraft:slow_falling", 100, 0));
    assert!((p.s.honey_old_dy(-0.13) - (-0.13 - 0.01 + 0.08)).abs() < 1e-12);
    done(p, dir);
}

/// `nether_travel` starts from the entry saved with the player (`entered_nether_pos`).
#[test]
fn nether_travel_uses_saved_entry() {
    let Some((mut p, dir)) = probe("g1f02-nether") else { return };
    p.s.dims.entered_nether = Some([0.5, 64.0, 0.5]);
    p.s.pos = [8000.5, 64.0, 0.5];
    p.s.trig_changed_dimension("minecraft:the_nether", "minecraft:overworld");
    assert!(adv(&p, "minecraft:nether/fast_travel"), "saved departure ignored");
    done(p, dir);
}

/// `entity_struck`: origin context = player (distance measured from them, not the lightning).
#[test]
fn entity_struck_origin_is_player() {
    let Some((p, dir)) = probe("g1f02-struck") else { return };
    let pred = json!({ "minecraft:type_specific/lightning": {
        "entity_struck": { "distance": { "absolute": { "max": 2.0 } } }
    } });
    let far = [p.s.pos[0] + 100.0, p.s.pos[1], p.s.pos[2]];
    assert!(p.s.bolt_pred(&pred, far, 0, &[crate::mobs::mob::Ent::Player]), "origin taken on the lightning");
    done(p, dir);
}

/// Item reward: `this` context = player; unstored item collectable right away,
/// without a thrower.
#[test]
fn reward_loot_context_and_drop() {
    let Some((mut p, dir)) = probe("g1f02-reward") else { return };
    let table = r#"{"pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:diamond"}],
        "condition":{"condition":"minecraft:entity_properties","entity":"this",
        "predicate":{"type":"minecraft:player","location":{"position":{"x":{"min":110.0,"max":111.0}}}}}}]}"#;
    p.s.life.pack = crate::datapack::DataPack::from_files(&[("test:loot_table/t", table)]);
    let got = p.s.roll_loot("test:t");
    assert_eq!(got.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["minecraft:diamond"], "empty context");
    let n = p.s.life.drops.len();
    p.s.drop_reward(Stack { id: "minecraft:diamond".into(), count: 1, components: None });
    assert_eq!(p.s.life.drops.len(), n + 1);
    let d = p.s.life.drops.last().unwrap();
    assert_eq!((d.pickup_delay, d.thrower), (0, None));
    done(p, dir);
}

/// Validation at loading: requirements ≠ criteria, background outside the root, root shown without a
/// background → rejected; a correct advancement stays.
#[test]
fn advancement_validation_rejects() {
    let disp = |bg: bool| {
        let mut d = json!({ "icon": { "id": "minecraft:stone" }, "title": "t", "description": "d" });
        if bg {
            d["background"] = json!("minecraft:block/stone");
        }
        d
    };
    let crit = json!({ "a": { "trigger": "minecraft:impossible" } });
    let raw: BTreeMap<String, serde_json::Value> = [
        ("x:r", json!({ "criteria": crit, "display": disp(true) })),
        ("x:ok", json!({ "parent": "x:r", "criteria": crit, "display": disp(false) })),
        ("x:unknown", json!({ "parent": "x:r", "criteria": crit, "requirements": [["a", "b"]] })),
        (
            "x:missing",
            json!({ "parent": "x:r", "criteria": { "a": crit["a"], "b": crit["a"] }, "requirements": [["a"]] }),
        ),
        ("x:bgchild", json!({ "parent": "x:r", "criteria": crit, "display": disp(true) })),
        ("x:nobg", json!({ "criteria": crit, "display": disp(false) })),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    let t = super::super::Tree::build(raw);
    let mut ids: Vec<&str> = t.list.iter().map(|a| a.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(ids, ["x:ok", "x:r"]);
}
