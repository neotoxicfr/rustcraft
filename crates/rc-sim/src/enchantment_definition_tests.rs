//! Enchantment effects read from their definition, so that a pack's own enchantment acts like
//! the game's: `damage_immunity` (Frost Walker's hot floors), `block_experience` (Silk Touch's
//! zero) and the enchantment tags that keep ice, infested blocks, nests and pots whole.

use std::sync::Arc;

use rc_proto::nbt::Nbt;

use crate::blocks::probe::Probe;
use crate::player::Stack;

/// A pack enchantment for the feet: `effects` is its effect map, as JSON.
fn enchantment(slots: &str, effects: &str) -> String {
    format!(
        r##"{{"description":"test","supported_items":"#minecraft:enchantable/durability","weight":1,"max_level":1,
        "min_cost":{{"base":1,"per_level_above_first":1}},"max_cost":{{"base":1,"per_level_above_first":1}},
        "anvil_cost":1,"slots":["{slots}"],"effects":{effects}}}"##
    )
}

fn open(tag: &str) -> (Probe, std::path::PathBuf) {
    let data = Arc::new(rc_testdata::version_data().expect("version data 26.3 missing"));
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-ench-def-{tag}-{}", std::process::id())));
    let mut w = crate::newworld::NewWorld::new("ench-def");
    w.preset = crate::newworld::Preset::Flat;
    w.seed = 0;
    w.generate_structures = false;
    crate::newworld::create_world(&dir, &data, &w).expect("world creation");
    let mut p = Probe::open(&dir, data);
    p.run("setblock 4 -60 4 minecraft:stone\n"); // loads the chunk the tests use
    p.s.pos = [4.5, -60.0, 6.5];
    p.s.loaded = true;
    p.s.game_mode = 0;
    p.s.life.vit.health = 20.0;
    p.s.life.vit.dead = false;
    p.s.life.vit.cooldown = 0;
    for slot in p.s.slots.iter_mut() {
        *slot = None;
    }
    (p, dir)
}

fn enchanted(id: &str, enchantment: &str) -> Stack {
    let mut s = Stack { id: id.into(), count: 1, components: None };
    let levels = Nbt::Compound(vec![(enchantment.into(), Nbt::Int(1))]);
    crate::items::set_component(&mut s, "minecraft:enchantments", levels);
    s
}

/// A pack enchantment whose `damage_immunity` names fall damage spares the wearer; Frost
/// Walker's own definition still spares hot floors.
#[test]
fn damage_immunity_is_read_from_the_definition() {
    let (mut p, _dir) = open("immunity");
    let effects = r##"{"minecraft:damage_immunity":[{"requirements":{"type":"minecraft:damage_source_properties",
        "predicate":{"tags":[{"id":"#minecraft:is_fall","expected":true}]}},"effect":{}}]}"##;
    p.s.life.pack.insert_test_file("test:enchantment/feather_guard", &enchantment("feet", effects));
    p.s.slots[8] = Some(enchanted("minecraft:iron_boots", "test:feather_guard"));
    assert!(!p.s.hurt("minecraft:fall", 5.0), "fall damage landed through the immunity");
    assert_eq!(p.s.life.vit.health, 20.0);
    p.s.slots[8] = Some(enchanted("minecraft:iron_boots", "minecraft:frost_walker"));
    assert!(!p.s.hurt("minecraft:hot_floor", 1.0), "Frost Walker let the hot floor burn");
    assert!(p.s.hurt("minecraft:fall", 5.0), "Frost Walker spared a fall");
    assert_eq!(p.s.life.vit.health, 15.0);
}

/// Total value of the orbs lying in the world.
fn orb_value(p: &Probe) -> i32 {
    p.s.life.orbs.iter().map(|o| o.value * o.count).sum()
}

/// A pack enchantment adding 5 by `block_experience` gives sculk 6 experience; Silk Touch, from
/// its own definition, gives none.
#[test]
fn block_experience_is_read_from_the_definition() {
    let (mut p, _dir) = open("experience");
    let effects = r#"{"minecraft:block_experience":[{"effect":{"type":"minecraft:add","value":5}}]}"#;
    p.s.life.pack.insert_test_file("test:enchantment/wisdom", &enchantment("mainhand", effects));
    let wise = enchanted("minecraft:iron_pickaxe", "test:wisdom");
    p.s.drop_block_xp([4, -59, 4], "minecraft:sculk", Some(&wise));
    assert_eq!(orb_value(&p), 6, "sculk experience with the pack enchantment");
    let silk = enchanted("minecraft:iron_pickaxe", "minecraft:silk_touch");
    p.s.drop_block_xp([4, -59, 4], "minecraft:sculk", Some(&silk));
    assert_eq!(orb_value(&p), 6, "Silk Touch gave experience");
}

/// A pack enchantment added to `prevents_ice_melting` keeps broken ice from turning to water,
/// like Silk Touch; a plain pickaxe melts it.
#[test]
fn ice_melting_follows_the_enchantment_tag() {
    let (mut p, _dir) = open("ice");
    p.s.life.pack.insert_test_file("test:enchantment/cold_hands", &enchantment("mainhand", "{}"));
    p.s.life.pack.insert_test_file(
        "tags/enchantment/prevents_ice_melting",
        r#"{"values":["minecraft:silk_touch","test:cold_hands"]}"#,
    );
    // Water under the ice: broken without the tag, it melts.
    p.run("setblock 4 -60 4 minecraft:water\n");
    let ice = p.s.data.default_state_of("minecraft:ice").expect("ice");
    let air = p.s.data.default_state_of("minecraft:air").expect("air");
    let broken = |p: &mut Probe, tool: Option<&Stack>| {
        p.s.set_block([4, -59, 4], air, crate::blocks::flag::ALL);
        p.s.player_destroyed([4, -59, 4], ice, tool);
        p.s.state_name(p.s.get([4, -59, 4])).to_owned()
    };
    assert_eq!(broken(&mut p, Some(&enchanted("minecraft:iron_pickaxe", "test:cold_hands"))), "minecraft:air");
    assert_eq!(broken(&mut p, Some(&Stack { id: "minecraft:iron_pickaxe".into(), count: 1, components: None })), "minecraft:water");
}
