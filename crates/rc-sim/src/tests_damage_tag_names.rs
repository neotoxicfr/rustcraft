//! Damage type tags and sets written with a short name (`is_fire`, `#is_fire`, `:is_fire`,
//! `in_fire`), as a `damage_resistant` or `blocks_attacks` component may hold them: the game reads
//! a missing or empty namespace as `minecraft`. On a copy of the test world
//! (`RUSTCRAFT_SOLO_WORLD`) for the registries; absent: tests skipped (failure under
//! `RUSTCRAFT_OPT_REQUIRE=1`), except the pure identifier test.
#![cfg(test)]

use rc_proto::nbt::Nbt;
use serde_json::json;

use crate::blocks::probe::Probe;
use crate::mobs::combat::same_identifier;
use crate::player::Stack;

fn probe(tag: &str) -> Option<Probe> {
    let (world, data) = (crate::tests::test_world()?, rc_testdata::version_data()?);
    let dir = crate::tests::fresh_copy(&world, tag);
    Some(Probe::open(&dir, std::sync::Arc::new(data)))
}

/// A stack whose `damage_resistant` patch holds `types`.
fn resistant(id: &str, types: Nbt) -> Stack {
    let mut s = Stack { id: id.to_owned(), count: 1, components: None };
    crate::items::set_component(&mut s, "minecraft:damage_resistant", Nbt::Compound(vec![("types".to_owned(), types)]));
    s
}

fn text(s: &str) -> Nbt {
    Nbt::String(s.to_owned())
}

#[test]
fn identifiers_without_namespace_are_in_minecraft() {
    assert!(same_identifier("minecraft:is_fire", "is_fire"));
    assert!(same_identifier("minecraft:is_fire", ":is_fire"));
    assert!(same_identifier("minecraft:is_fire", "minecraft:is_fire"));
    assert!(!same_identifier("minecraft:is_fire", "other:is_fire"));
    assert!(!same_identifier("other:is_fire", "is_fire"));
    assert!(!same_identifier("minecraft:is_fire", "is_fall"));
    assert!(!same_identifier("minecraft:is_fire", "#is_fire"));
    // The completion used by damage predicates and weapon damage types follows the same rule.
    for (written, full) in [("is_fire", "minecraft:is_fire"), (":is_fire", "minecraft:is_fire"), ("pack:x", "pack:x")] {
        assert_eq!(crate::commands::args::id(written), full);
    }
}

#[test]
fn short_tag_names_resolve_like_full_ones() {
    let Some(p) = probe("dmg-tag-short") else { return };
    let fire = "minecraft:in_fire";
    for tag in ["minecraft:is_fire", "is_fire", ":is_fire"] {
        assert!(p.s.dmg_tag(fire, tag), "{tag} holds in_fire");
    }
    assert!(!p.s.dmg_tag(fire, "is_explosion"), "is_explosion does not hold in_fire");
    assert!(!p.s.dmg_tag(fire, "other:is_fire"), "another namespace is another tag");
    assert!(!p.s.dmg_tag("in_fire", "is_fire"), "the damage type itself is always complete");
}

#[test]
fn damage_sets_accept_short_tags_and_ids() {
    let Some(p) = probe("dmg-set-short") else { return };
    let fire = "minecraft:in_fire";
    for entry in ["#is_fire", "#minecraft:is_fire", "in_fire", "minecraft:in_fire", ":in_fire"] {
        assert!(p.s.damage_set_entry_has(entry, fire), "{entry}");
        assert!(crate::item_use::damage_set_has(&p.s, &json!(entry), fire), "{entry} (component json)");
    }
    assert!(!p.s.damage_set_entry_has("#is_explosion", fire));
    assert!(!p.s.damage_set_entry_has("lava", fire));
    assert!(crate::item_use::damage_set_has(&p.s, &json!(["lava", "in_fire"]), fire), "list of ids");
    assert!(!crate::item_use::damage_set_has(&p.s, &json!(["#is_fire"]), fire), "a list never holds tags");
}

/// `damage_resistant` patches: a short tag, a single damage type and a list all protect the item
/// and spare the worn piece, like the game (before, only the full tag name worked for items).
#[test]
fn damage_resistant_component_reads_every_set_form() {
    let Some(p) = probe("dmg-resistant-forms") else { return };
    let fire = "minecraft:in_fire";
    let forms = [text("#is_fire"), text("#minecraft:is_fire"), text("in_fire"), Nbt::List(vec![text("lava"), text("in_fire")])];
    for types in forms {
        let ingot = resistant("minecraft:iron_ingot", types.clone());
        assert!(p.s.item_resists(&ingot, fire), "{types:?} protects the item");
        assert!(!p.s.item_resists(&ingot, "minecraft:explosion"), "{types:?} only covers fire");
        let chest = resistant("minecraft:iron_chestplate", types.clone());
        assert!(!p.s.armor_wears(&chest, fire), "{types:?} spares the worn piece");
        assert!(p.s.armor_wears(&chest, "minecraft:explosion"), "{types:?}: other damage wears it");
    }
    let plain = Stack { id: "minecraft:iron_ingot".to_owned(), count: 1, components: None };
    assert!(!p.s.item_resists(&plain, fire), "no component, no resistance");
    let netherite = Stack { id: "minecraft:netherite_ingot".to_owned(), count: 1, components: None };
    assert!(p.s.item_resists(&netherite, fire), "prototype component of netherite");
}
