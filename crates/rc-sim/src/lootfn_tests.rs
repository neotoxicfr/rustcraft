//! G6-F17: loot functions missing until now (item, data, attributes, names, contents, lore,
//! heads, banners, dyes, potions, fireworks, books, tooltips, model data), the `!` removal and
//! the strict check of `set_components`, `copy_components` from the tool and the entities, the
//! score / storage / enchantment level providers, value checks, the modifiers of composite
//! entries, nested sets (list, tag, expand), the refusal of a table already running, the
//! `slots` entry, and the command context of `/item modify` and `/loot`. Expected values follow
//! the game's rules (official sources read, in our words in `target/notes`).

use std::sync::Arc;

use rc_proto::nbt::Nbt;
use serde_json::{Value, json};

use crate::commands::ents::E;
use crate::commands::snbt;
use crate::datapack::DataPack;
use crate::loot::{Ctx, Loot};
use crate::player::Stack;
use crate::progress::pred::{Ctx as PCtx, Who};

fn world(tag: &str) -> crate::blocks::probe::Probe {
    let data = Arc::new(rc_testdata::version_data().expect("version data 26.3 missing"));
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-g6f17-{tag}-{}", std::process::id())));
    let mut w = crate::newworld::NewWorld::new(tag);
    w.preset = crate::newworld::Preset::Flat;
    w.allow_commands = true;
    crate::newworld::create_world(&dir, &data, &w).expect("world creation");
    crate::blocks::probe::Probe::open(&dir, data)
}

fn stack(id: &str, n: i32) -> Stack {
    Stack { id: id.into(), count: n, components: None }
}

fn with(id: &str, n: i32, comps: &str) -> Stack {
    let mut s = Stack { id: id.into(), count: n, components: Some(snbt::parse(comps).expect("snbt")) };
    crate::items::normalize_patch(&mut s);
    s
}

/// Component `k` of `s` equal to the SNBT `want` (`-` = absent), field order aside.
fn has(s: &Stack, k: &str, want: &str) -> bool {
    match (crate::items::component(s, k), want) {
        (None, "-") => true,
        (Some(v), w) if w != "-" => crate::items::nbt_eq(v, &snbt::parse(w).expect("snbt")),
        _ => false,
    }
}

fn show(s: &Stack) -> String {
    format!("{} x{} {}", s.id, s.count, s.components.as_ref().map_or_else(|| "-".into(), snbt::to_snbt))
}

/// Every function type the pack never uses, applied as `/item modify` and the data packs do.
#[test]
fn missing_loot_functions() {
    let mut p = world("g6f17fn");
    let defaults = p.s.life.items.defaults("minecraft:diamond_sword");
    assert!(!defaults["minecraft:attribute_modifiers"].is_null(), "item component reports missing");
    p.s.cmd.storage.insert("test:s".into(), snbt::parse("{v:{w:3},l:[1,2]}").unwrap());
    let s = &p.s;
    let no_tag = |_: &str, _: &str| false;
    let loot = Loot { pack: &s.life.pack, items: &s.life.items, block_tag: &no_tag };
    let view = s.loot_world(PCtx { this: Some(Who::E(E::Player)), origin: Some(s.pos), ..PCtx::default() });
    let ctx = Ctx { world: Some(&view), ..Ctx::default() };
    let mut rng = crate::mobs::jrand::JRand::new(5);
    let mut fails: Vec<String> = Vec::new();
    let mut check = |name: &str, ok: bool, s: &Stack| {
        if !ok {
            fails.push(format!("{name}: {}", show(s)));
        }
    };
    let mut ap = |f: Value, s: Stack| loot.apply(&f, s, &ctx, &mut rng);

    let r = ap(json!({"type": "minecraft:set_item", "item": "minecraft:diamond"}), with("minecraft:stone", 5, "{\"minecraft:custom_name\":\"n\"}"));
    check("set_item", r.id == "minecraft:diamond" && r.count == 5 && has(&r, "minecraft:custom_name", "\"n\""), &r);

    let r = ap(
        json!({"type": "minecraft:set_custom_data", "tag": "{a:1b,b:{c:2}}"}),
        with("minecraft:stone", 1, "{\"minecraft:custom_data\":{b:{d:3}}}"),
    );
    check("set_custom_data", has(&r, "minecraft:custom_data", "{a:1b,b:{c:2,d:3}}"), &r);

    let add = json!({"type": "minecraft:set_attributes", "replace": false, "modifiers": [
        {"attribute": "luck", "id": "test:x", "amount": 2.5, "operation": "add_value", "slot": "mainhand"}]});
    let r = ap(add, stack("minecraft:diamond_sword", 1));
    let list = crate::nbtio::list(crate::items::component(&r, "minecraft:attribute_modifiers")).to_vec();
    let last = list.last().cloned().unwrap_or(Nbt::Byte(0));
    let want = snbt::parse("{type:\"minecraft:luck\",id:\"test:x\",amount:2.5d,operation:\"add_value\",slot:\"mainhand\"}").unwrap();
    check("set_attributes (added to the defaults)", list.len() == 3 && crate::items::nbt_eq(&last, &want), &r);
    let twice = json!({"type": "minecraft:set_attributes", "modifiers": [
        {"attribute": "minecraft:luck", "id": "test:x", "amount": 1, "operation": "add_value", "slot": ["any"]},
        {"attribute": "minecraft:luck", "id": "test:x", "amount": 3, "operation": "add_value", "slot": "any"}]});
    let r = ap(twice, stack("minecraft:stick", 1));
    check(
        "set_attributes (same id replaced, slot any left out)",
        has(&r, "minecraft:attribute_modifiers", "[{type:\"minecraft:luck\",id:\"test:x\",amount:3.0d,operation:\"add_value\"}]"),
        &r,
    );

    let r = ap(json!({"type": "minecraft:copy_name", "source": "this"}), with("minecraft:stone", 1, "{\"minecraft:custom_name\":\"n\"}"));
    check("copy_name from the unnamed player", has(&r, "minecraft:custom_name", "-"), &r);

    let r = ap(
        json!({"type": "minecraft:set_contents", "component": "container", "entries": [
            {"type": "minecraft:item", "name": "minecraft:stone", "modifier": {"type": "minecraft:set_count", "count": 70}},
            {"type": "minecraft:item", "name": "minecraft:dirt", "weight": 0}]}),
        stack("minecraft:shulker_box", 1),
    );
    let want = "[{slot:0,item:{id:\"minecraft:stone\",count:64}},{slot:1,item:{id:\"minecraft:stone\",count:6}},{slot:2,item:{id:\"minecraft:dirt\",count:1}}]";
    check("set_contents (split, weight 0 taken)", has(&r, "minecraft:container", want), &r);
    let r = ap(
        json!({"type": "minecraft:modify_contents", "component": "minecraft:container",
            "modifier": {"type": "minecraft:set_count", "count": 2}}),
        r,
    );
    let want = "[{slot:0,item:{id:\"minecraft:stone\",count:2}},{slot:1,item:{id:\"minecraft:stone\",count:2}},{slot:2,item:{id:\"minecraft:dirt\",count:2}}]";
    check("modify_contents", has(&r, "minecraft:container", want), &r);
    let r = ap(
        json!({"type": "minecraft:set_contents", "component": "minecraft:bundle_contents", "entries": [
            {"type": "minecraft:item", "name": "minecraft:stone", "modifier": {"type": "minecraft:set_count", "count": 70}}]}),
        stack("minecraft:bundle", 1),
    );
    check("set_contents (bundle weight: 64 of 70)", has(&r, "minecraft:bundle_contents", "[{id:\"minecraft:stone\",count:64}]"), &r);

    let r = ap(json!({"type": "minecraft:set_loot_table", "loot_table_id": "chests/simple_dungeon", "seed": 5}), stack("minecraft:chest", 1));
    check("set_loot_table", has(&r, "minecraft:container_loot", "{loot_table:\"minecraft:chests/simple_dungeon\",seed:5L}"), &r);

    let lore = |mode: Value| {
        let mut f = json!({"type": "minecraft:set_lore", "lore": ["a", {"text": "b", "color": "red"}, {"text": "c"}]});
        if let (Some(o), Value::Object(m)) = (f.as_object_mut(), mode) {
            o.extend(m);
        }
        f
    };
    let base = || with("minecraft:stone", 1, "{\"minecraft:lore\":[\"x\",\"y\"]}");
    let r = ap(lore(json!({"mode": "append"})), base());
    check("set_lore append", has(&r, "minecraft:lore", "[\"x\",\"y\",\"a\",{text:\"b\",color:\"red\"},\"c\"]"), &r);
    let r = ap(lore(json!({"mode": "insert", "offset": 1})), base());
    check("set_lore insert", has(&r, "minecraft:lore", "[\"x\",\"a\",{text:\"b\",color:\"red\"},\"c\",\"y\"]"), &r);
    let r = ap(lore(json!({"mode": "replace_section", "offset": 0, "size": 1})), base());
    check("set_lore replace_section", has(&r, "minecraft:lore", "[\"a\",{text:\"b\",color:\"red\"},\"c\",\"y\"]"), &r);
    let r = ap(lore(json!({"mode": "insert", "offset": 5})), base());
    check("set_lore offset past the end", has(&r, "minecraft:lore", "[\"x\",\"y\"]"), &r);

    let r = ap(json!({"type": "minecraft:fill_player_head", "entity": "this"}), stack("minecraft:player_head", 1));
    let want = Nbt::Compound(vec![
        ("id".into(), crate::nbtio::uuid_nbt(rc_client::offline_uuid("S3"))),
        ("name".into(), Nbt::String("S3".into())),
    ]);
    let ok = crate::items::component(&r, "minecraft:profile").is_some_and(|v| crate::items::nbt_eq(v, &want));
    check("fill_player_head", ok, &r);
    let r = ap(json!({"type": "minecraft:fill_player_head", "entity": "this"}), stack("minecraft:stone", 1));
    check("fill_player_head (not a head)", r.components.is_none(), &r);

    let r = ap(
        json!({"type": "minecraft:copy_custom_data", "source": {"type": "minecraft:storage", "source": "test:s"}, "ops": [
            {"source": "v", "target": "x.y", "op": "replace"},
            {"source": "l[]", "target": "arr", "op": "append"},
            {"source": "v", "target": "m", "op": "merge"},
            {"source": "missing", "target": "z", "op": "replace"}]}),
        stack("minecraft:stone", 1),
    );
    check("copy_custom_data (storage)", has(&r, "minecraft:custom_data", "{x:{y:{w:3}},arr:[1,2],m:{w:3}}"), &r);
    let r = ap(
        json!({"type": "minecraft:copy_custom_data", "source": "this", "ops": [{"source": "Health", "target": "h", "op": "replace"}]}),
        stack("minecraft:stone", 1),
    );
    check("copy_custom_data (this)", has(&r, "minecraft:custom_data", "{h:20.0f}"), &r);

    let layer = json!({"type": "minecraft:set_banner_pattern", "append": true, "patterns": [{"pattern": "stripe_top", "color": "red"}]});
    let r = ap(layer.clone(), stack("minecraft:white_banner", 1));
    let r = ap(layer, r);
    let two = "[{pattern:\"minecraft:stripe_top\",color:\"red\"},{pattern:\"minecraft:stripe_top\",color:\"red\"}]";
    check("set_banner_pattern append", has(&r, "minecraft:banner_patterns", two), &r);

    let r = ap(json!({"type": "minecraft:set_random_dyes", "number_of_dyes": 1}), stack("minecraft:leather_helmet", 3));
    let dyed = crate::items::component(&r, "minecraft:dyed_color").and_then(Nbt::as_i64).map(|x| x as i32);
    check("set_random_dyes", r.count == 1 && dyed.is_some_and(|c| crate::mobs::trades::DYE_DIFFUSE.contains(&c)), &r);
    let r = ap(json!({"type": "minecraft:set_random_potion", "options": ["minecraft:swiftness"]}), stack("minecraft:potion", 1));
    check("set_random_potion", has(&r, "minecraft:potion_contents", "{potion:\"minecraft:swiftness\"}"), &r);
    let r = ap(json!({"type": "minecraft:set_random_potion", "options": "#minecraft:tradeable"}), stack("minecraft:potion", 1));
    let tag_ok = s.life.items.potion_tags.iter().any(|t| t.0.ends_with("tradeable"));
    check("set_random_potion (tag)", !tag_ok || crate::items::component(&r, "minecraft:potion_contents").is_some(), &r);

    let r = ap(
        json!({"type": "minecraft:set_fireworks", "flight_duration": 3, "explosions": {"values": [
            {"shape": "star", "colors": [16711680], "has_trail": true}], "mode": "append"}}),
        stack("minecraft:firework_rocket", 1),
    );
    let want = "{flight_duration:3b,explosions:[{shape:\"star\",colors:[16711680],has_trail:1b}]}";
    check("set_fireworks", has(&r, "minecraft:fireworks", want), &r);
    let r = ap(json!({"type": "minecraft:set_firework_explosion", "shape": "burst", "fade_colors": [1, 2]}), stack("minecraft:firework_star", 1));
    check("set_firework_explosion", has(&r, "minecraft:firework_explosion", "{shape:\"burst\",fade_colors:[1,2]}"), &r);

    let r = ap(json!({"type": "minecraft:set_book_cover", "title": "T", "author": "A"}), stack("minecraft:written_book", 1));
    check("set_book_cover", has(&r, "minecraft:written_book_content", "{title:{raw:\"T\"},author:\"A\",resolved:1b}"), &r);
    let r2 = ap(json!({"type": "minecraft:set_book_cover", "generation": 5}), r.clone());
    check("set_book_cover (generation out of range)", crate::items::matches(Some(&r2), Some(&r)), &r2);
    let r = ap(
        json!({"type": "minecraft:set_written_book_pages", "mode": "replace_all", "pages": ["p1", {"text": "p2", "bold": true}]}),
        r,
    );
    let want = "{title:{raw:\"T\"},author:\"A\",pages:[{raw:\"p1\"},{raw:{text:\"p2\",bold:1b}}]}";
    check("set_written_book_pages", has(&r, "minecraft:written_book_content", want), &r);
    let r = ap(
        json!({"type": "minecraft:set_writable_book_pages", "mode": "append", "pages": ["a", {"raw": "b", "filtered": "c"}]}),
        stack("minecraft:writable_book", 1),
    );
    check("set_writable_book_pages", has(&r, "minecraft:writable_book_content", "{pages:[{raw:\"a\"},{raw:\"b\",filtered:\"c\"}]}"), &r);

    let r = ap(
        json!({"type": "minecraft:toggle_tooltips", "toggles": {"minecraft:attribute_modifiers": false, "enchantments": false}}),
        stack("minecraft:diamond_sword", 1),
    );
    let r = ap(json!({"type": "minecraft:toggle_tooltips", "toggles": {"minecraft:enchantments": true}}), r);
    check("toggle_tooltips", has(&r, "minecraft:tooltip_display", "{hidden_components:[\"minecraft:attribute_modifiers\"]}"), &r);

    let r = ap(
        json!({"type": "minecraft:set_custom_model_data",
            "floats": {"values": [1.5, {"type": "minecraft:uniform", "min": 2, "max": 2}], "mode": "append"},
            "flags": {"values": [true], "mode": "replace_all"},
            "strings": {"values": ["s"], "mode": "append"},
            "colors": {"values": [255, [1.0, 0.0, 0.0]], "mode": "append"}}),
        stack("minecraft:stick", 1),
    );
    let want = "{floats:[1.5f,2.0f],flags:[1b],strings:[\"s\"],colors:[255,-65536]}";
    check("set_custom_model_data", has(&r, "minecraft:custom_model_data", want), &r);

    // `set_components`: `!x` removal against the defaults, strict check of the result.
    let r = ap(json!({"type": "minecraft:set_components", "components": {"!minecraft:food": {}}}), stack("minecraft:cooked_beef", 1));
    check("set_components !food (default removed)", has(&r, "!minecraft:food", "{}"), &r);
    let r = ap(json!({"type": "minecraft:set_components", "components": {"!food": {}}}), stack("minecraft:stone", 1));
    check("set_components !food (nothing to remove)", r.components.is_none(), &r);
    let r = ap(json!({"type": "minecraft:set_components", "components": {"max_stack_size": 64}}), stack("minecraft:diamond_sword", 1));
    check("set_components damageable and stackable refused", r.components.is_none(), &r);
    let r = ap(json!({"type": "minecraft:set_components", "components": {"max_stack_size": 16}}), stack("minecraft:stone", 64));
    check("set_components quantity over the maximum refused", r.components.is_none(), &r);

    // `copy_components` from the tool (its value, defaults included) and the block entity name.
    let sword = stack("minecraft:diamond_sword", 1);
    let tool_ctx = Ctx { tool: Some(Some(&sword)), world: Some(&view), ..Ctx::default() };
    let f = json!({"type": "minecraft:copy_components", "source": "tool", "include": ["max_damage"]});
    let r = loot.apply(&f, stack("minecraft:stick", 1), &tool_ctx, &mut rng);
    check("copy_components (tool)", has(&r, "minecraft:max_damage", "1561"), &r);
    let be = snbt::parse("{id:\"minecraft:chest\",CustomName:\"Box\"}").unwrap();
    let be_ctx = Ctx { block_entity: Some(&be), ..Ctx::default() };
    let r = loot.apply(&json!({"type": "minecraft:copy_name", "source": "block_entity"}), stack("minecraft:chest", 1), &be_ctx, &mut rng);
    check("copy_name (block entity)", has(&r, "minecraft:custom_name", "\"Box\""), &r);
    drop(view);
    assert!(fails.is_empty(), "{fails:#?}");
}

/// Composite modifiers, nested sets, re-entry, `slots` entry, providers and value checks.
#[test]
fn loot_entries_and_providers() {
    let mut p = world("g6f17en");
    p.s.player_command("scoreboard objectives add o dummy");
    p.s.player_command("scoreboard players set S3 o 4");
    p.s.player_command("scoreboard players set X o 6");
    p.s.cmd.storage.insert("minecraft:st".into(), snbt::parse("{n:5,f:2.5d,t:\"x\"}").unwrap());
    p.s.slots[36] = Some(stack("minecraft:diamond", 2));
    p.s.slots[37] = Some(with("minecraft:shulker_box", 1, "{\"minecraft:container\":[{slot:1,item:{id:\"minecraft:apple\",count:3}}]}"));
    p.s.held = 0;
    let pack = DataPack::from_files(&[
        ("test:loot_table/self", r#"{"pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:stone"}]},
            {"rolls":1,"entries":[{"type":"minecraft:loot_table","value":"test:self"}]}]}"#),
        ("test:loot_table/a", r#"{"pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:stone"}]}]}"#),
        ("test:loot_table/b", r#"{"pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:dirt"}]}]}"#),
        ("test:tags/loot_table/t", r##"{"values":["test:a","#test:u"]}"##),
        ("test:tags/loot_table/u", r#"{"values":[{"id":"test:b","required":false}]}"#),
    ]);
    let s = &p.s;
    let no_tag = |_: &str, _: &str| false;
    let mine = Loot { pack: &pack, items: &s.life.items, block_tag: &no_tag };
    let loot = Loot { pack: &s.life.pack, items: &s.life.items, block_tag: &no_tag };
    let view = s.loot_world(PCtx { this: Some(Who::E(E::Player)), origin: Some(s.pos), ..PCtx::default() });
    let ctx = Ctx { world: Some(&view), ..Ctx::default() };
    let mut rng = crate::mobs::jrand::JRand::new(9);
    let mut fails: Vec<String> = Vec::new();
    let names = |v: &[Stack]| v.iter().map(|s| format!("{}x{}", s.id.trim_start_matches("minecraft:"), s.count)).collect::<Vec<_>>();
    let one = |entry: Value| json!({"pools": [{"rolls": 1, "entries": [entry]}]});

    // Leaf modifier first, then the composite's (setting 2 then adding 1 gives 3).
    let t = one(json!({"type": "minecraft:group", "modifier": {"type": "minecraft:set_count", "count": 1, "add": true}, "children": [
        {"type": "minecraft:item", "name": "minecraft:stone", "modifier": {"type": "minecraft:set_count", "count": 2}}]}));
    let out = names(&loot.run(&t, &ctx, &mut rng));
    if out != ["stonex3"] {
        fails.push(format!("composite modifier: {out:?}"));
    }
    let t = one(json!({"type": "minecraft:loot_table", "value": [
        {"pools": [{"rolls": 1, "entries": [{"type": "minecraft:item", "name": "minecraft:stone"}]}]},
        {"pools": [{"rolls": 1, "entries": [{"type": "minecraft:item", "name": "minecraft:dirt"}]}]}]}));
    let out = names(&loot.run(&t, &ctx, &mut rng));
    if out != ["stonex1", "dirtx1"] {
        fails.push(format!("nested list: {out:?}"));
    }
    let mut t = t;
    t["pools"][0]["entries"][0]["expand"] = json!(true);
    let out = names(&loot.run(&t, &ctx, &mut rng));
    if out.len() != 1 {
        fails.push(format!("nested list expanded (one table drawn): {out:?}"));
    }
    let out = names(&mine.run(&one(json!({"type": "minecraft:loot_table", "value": "#test:t"})), &ctx, &mut rng));
    if out != ["stonex1", "dirtx1"] {
        fails.push(format!("nested tag: {out:?}"));
    }
    let out = names(&mine.drops("test:self", &ctx, &mut rng));
    if out != ["stonex1"] {
        fails.push(format!("table nesting itself (refused once running): {out:?}"));
    }
    // Distinct tables nested 64 deep: no depth limit, each level runs (and the test thread's
    // stack holds it).
    let mut deep = one(json!({"type": "minecraft:item", "name": "minecraft:stone"}));
    for _ in 0..63 {
        deep = json!({"pools": [{"rolls": 1, "entries": [{"type": "minecraft:item", "name": "minecraft:stone"}]},
            {"rolls": 1, "entries": [{"type": "minecraft:loot_table", "value": deep}]}]});
    }
    let n = loot.run(&deep, &ctx, &mut rng).len();
    if n != 64 {
        fails.push(format!("64 distinct nested tables: {n} stacks"));
    }
    let t = one(json!({"type": "minecraft:slots", "slot_source": {"type": "minecraft:slot_range", "source": "this", "slots": "hotbar.0"}}));
    let out = names(&loot.run(&t, &ctx, &mut rng));
    if out != ["diamondx2"] {
        fails.push(format!("slots entry: {out:?}"));
    }
    let t = one(json!({"type": "minecraft:slots", "slot_source": {"type": "minecraft:contents", "component": "container",
        "slot_source": {"type": "minecraft:limit_slots", "limit": 1, "slot_source": {"type": "minecraft:filtered",
            "item_filter": {"items": "minecraft:shulker_box"}, "slot_source": [
                {"type": "minecraft:slot_range", "source": "this", "slots": "hotbar.*"}]}}}}));
    let out = names(&loot.run(&t, &ctx, &mut rng));
    if out != ["applex3"] {
        fails.push(format!("slots entry (contents of the first shulker box): {out:?}"));
    }

    // Providers read from the lent world.
    let count = |c: Value| one(json!({"type": "minecraft:item", "name": "minecraft:stone", "modifier": {"type": "minecraft:set_count", "count": c}}));
    let cases = [
        (json!({"type": "minecraft:score", "target": "this", "score": "o"}), 4),
        (json!({"type": "minecraft:score", "target": {"type": "minecraft:fixed", "name": "X"}, "score": "o"}), 6),
        (json!({"type": "minecraft:score", "target": {"type": "minecraft:context", "target": "this"}, "score": "none", "fallback": 9}), 9),
        (json!({"type": "minecraft:storage", "storage": "st", "path": "n"}), 5),
        (json!({"type": "minecraft:storage", "storage": "st", "path": "t", "fallback": 7}), 7),
        (json!({"type": "minecraft:from_float", "input": {"type": "minecraft:storage", "storage": "st", "path": "f"}}), 2),
        (json!({"type": "minecraft:from_float", "input": {"type": "minecraft:enchantment_level",
            "amount": {"type": "minecraft:linear", "base": 3, "per_level_above_first": 1}}}), 2),
    ];
    for (c, want) in cases {
        let out = loot.run(&count(c.clone()), &ctx, &mut rng);
        if out.first().map(|s| s.count) != Some(want) {
            fails.push(format!("provider {c}: {:?}, expected {want}", names(&out)));
        }
    }
    let check = |cond: Value, want: bool, rng: &mut dyn crate::randsrc::Rand| {
        let got = loot.cond(&cond, &ctx, rng);
        (got != want).then(|| format!("condition {cond}: {got}"))
    };
    fails.extend(check(
        json!({"type": "minecraft:int_value_check", "value": {"type": "minecraft:uniform", "min": 3, "max": 3}, "test": {"min": 2, "max": 4}}),
        true,
        &mut rng,
    ));
    fails.extend(check(
        json!({"type": "minecraft:float_value_check", "value": {"type": "minecraft:storage", "storage": "st", "path": "f"}, "test": 2.5}),
        true,
        &mut rng,
    ));
    // Arithmetic fixed by G3-F08, kept: divisor read first (zero: NaN, the dividend not drawn),
    // ceiling through a saturated integer, 32-bit division wrapping, binomial refusing NaN.
    let mut a = crate::mobs::jrand::JRand::new(3);
    let b = crate::mobs::jrand::JRand::new(3);
    let m = json!({"type": "minecraft:mod", "left": {"type": "minecraft:uniform", "min": 0, "max": 1}, "right": 0});
    let nan = loot.float_checked(&m, &ctx, &mut a).is_ok_and(f32::is_nan);
    if !nan || a.clone().next_int() != b.clone().next_int() {
        fails.push("float mod by zero".into());
    }
    if loot.float(&json!({"type": "minecraft:ceil", "input": 3.0e9}), &ctx, &mut a) != 2_147_483_647.0 {
        fails.push("ceil saturated".into());
    }
    if loot.int(&json!({"type": "minecraft:div", "left": i32::MIN, "right": -1}), &ctx, &mut a) != i32::MIN {
        fails.push("int division wrapping".into());
    }
    let bin = json!({"type": "minecraft:binomial", "n": 3, "p": {"type": "minecraft:div", "left": 0, "right": 0}});
    if loot.int_checked(&bin, &ctx, &mut a).is_ok() {
        fails.push("binomial with NaN".into());
    }
    drop(view);
    // Value check of a command predicate: the value is a provider (score of the source).
    let ok = p.s.perform(
        p.s.player_source(),
        "execute if predicate {type:\"minecraft:int_value_check\",value:{type:\"minecraft:score\",target:\"this\",score:\"o\"},test:4}",
    );
    if !ok.0 {
        fails.push(format!("execute if predicate int_value_check: {ok:?}"));
    }
    assert!(fails.is_empty(), "{fails:#?}");
}

/// `/item modify` and `/loot`: the source entity and position are the context.
#[test]
fn command_contexts() {
    let mut p = world("g6f17cmd");
    p.s.player_command("scoreboard objectives add o dummy");
    p.s.player_command("scoreboard players set S3 o 4");
    p.s.held = 0;
    p.s.slots[36] = Some(stack("minecraft:player_head", 1));
    p.s.player_command("item modify entity @s weapon.mainhand {type:\"minecraft:fill_player_head\",entity:\"this\"}");
    let mut fails = Vec::new();
    let head = p.s.slots[36].clone();
    if !head.as_ref().is_some_and(|h| crate::items::component(h, "minecraft:profile").and_then(|v| v.get("name")).and_then(Nbt::as_str) == Some("S3")) {
        fails.push(format!("item modify fill_player_head: {:?}", head.as_ref().map(show)));
    }
    p.s.slots[36] = Some(stack("minecraft:stone", 1));
    p.s.player_command("item modify entity @s weapon.mainhand {type:\"minecraft:set_count\",count:{type:\"minecraft:score\",target:\"this\",score:\"o\"}}");
    if p.s.slots[36].as_ref().map(|s| s.count) != Some(4) {
        fails.push(format!("item modify score of the source: {:?}", p.s.slots[36].as_ref().map(show)));
    }
    p.s.slots[37] = None;
    p.s.player_command(
        "loot replace entity @s hotbar.1 loot {pools:[{rolls:1,entries:[{type:\"minecraft:item\",name:\"minecraft:dirt\",modifier:{type:\"minecraft:set_count\",count:{type:\"minecraft:score\",target:\"this\",score:\"o\"}}}]}]}",
    );
    if p.s.slots[37].as_ref().map(|s| (s.id.as_str(), s.count)) != Some(("minecraft:dirt", 4)) {
        fails.push(format!("loot (chest context, source as this): {:?}", p.s.slots[37].as_ref().map(show)));
    }
    assert!(fails.is_empty(), "{fails:#?}");
}
