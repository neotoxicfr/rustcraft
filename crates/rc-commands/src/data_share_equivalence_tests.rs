//! Equivalence proof of `RUSTCRAFT_OPT_CMDDATASHARE` on the client side: command data that reads
//! the registries in the version data and shares its block table parses and suggests every
//! truncation of the corpus like the copied data of the former path (fingerprint of the former
//! path checked), answers every lookup like the copied sets, and holds the same blocks and sounds.
//! Two command data made from one version data share their tables; a deliberate error (a
//! registry read in the version data finds nothing) must be seen by the corpus.

use std::sync::Arc;

use rc_data::VersionData;

use crate::Data;
use crate::data::{SABOTAGE_VERSION_LOOKUP, force_shared_command_data};
use crate::suggest_equivalence_tests::{
    FORMER_CORPUS_FINGERPRINT, corpus_traced, fingerprint, real_trees_and_version, with_players_and_target,
};

/// `f` run with `RUSTCRAFT_OPT_CMDDATASHARE` forced to `on` on this thread.
fn with_share<T>(on: bool, f: impl FnOnce() -> T) -> T {
    force_shared_command_data(Some(on));
    let out = f();
    force_shared_command_data(None);
    out
}

/// The client's command data made from `version` on the path `shared`.
fn command_data(version: &Arc<VersionData>, shared: bool) -> Data {
    with_share(shared, || {
        let mut d = Data::from_shared_version(version);
        with_players_and_target(&mut d);
        d
    })
}

#[test]
fn shared_data_parses_and_suggests_like_the_copy() {
    let (trees, version) = real_trees_and_version();
    let copied = command_data(&version, false);
    let shared = command_data(&version, true);
    let former = with_share(false, || corpus_traced(&trees, &copied));
    assert_eq!(fingerprint(&former), FORMER_CORPUS_FINGERPRINT, "former path changed");
    let now = with_share(true, || corpus_traced(&trees, &shared));
    assert_eq!(now.len(), former.len());
    for (n, f) in now.iter().zip(&former) {
        assert_eq!(n, f);
    }
    SABOTAGE_VERSION_LOOKUP.with(|s| s.set(true));
    let wrong = with_share(true, || corpus_traced(&trees, &shared));
    SABOTAGE_VERSION_LOOKUP.with(|s| s.set(false));
    assert_ne!(wrong, former, "a failing version lookup is not seen by the corpus");
}

#[test]
fn shared_lookups_blocks_and_sounds_match_the_copy() {
    let (_, version) = real_trees_and_version();
    let copied = command_data(&version, false);
    let shared = command_data(&version, true);
    let mut looked_up = 0;
    for r in &version.registries {
        assert_eq!(shared.registry(&r.name), copied.registry(&r.name), "{}", r.name);
        assert_eq!(shared.has_registry(&r.name), copied.has_registry(&r.name));
        let absent = ["", "minecraft:", "minecraft:nope", "stone", "Minecraft:stone"];
        for id in r.entries.iter().map(String::as_str).chain(absent) {
            assert_eq!(shared.contains(&r.name, id), copied.contains(&r.name, id), "{} {id}", r.name);
            looked_up += 1;
        }
    }
    assert!(looked_up > 5_000, "only {looked_up} lookups");
    assert!(!shared.contains("minecraft:nope", "minecraft:stone") && !shared.has_registry("minecraft:nope"));
    assert!(!shared.sound_ids().is_empty());
    assert_eq!(shared.sound_ids(), copied.sound_ids());
    assert_eq!(shared.blocks.len(), copied.blocks.len());
    for (id, def) in copied.blocks.iter() {
        let s = shared.blocks.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert_eq!(format!("{s:?}"), format!("{def:?}"), "{id}");
    }
    let mut ids: Vec<String> = copied.blocks.keys().cloned().collect();
    ids.sort();
    assert_eq!(shared.blocks.ids_in_order(), &ids[..]);
}

#[test]
fn data_from_one_version_share_their_tables() {
    let (_, version) = real_trees_and_version();
    let a = command_data(&version, true);
    let b = command_data(&version, true);
    assert!(Arc::ptr_eq(&a.blocks, &b.blocks), "two block tables for one version data");
    let items = version.registry("minecraft:item").expect("item registry");
    let read = a.registry("minecraft:item").expect("item registry in the command data");
    assert!(std::ptr::eq(read.as_ptr(), items.entries.as_ptr()), "item registry copied");
    let copied = command_data(&version, false);
    assert!(!Arc::ptr_eq(&a.blocks, &copied.blocks));
    // Found by identity, never by content: two equal version data have their own tables.
    let (empty_a, empty_b) = (Arc::new(VersionData::default()), Arc::new(VersionData::default()));
    let (ea, eb) = (command_data(&empty_a, true), command_data(&empty_b, true));
    assert!(!Arc::ptr_eq(&ea.blocks, &eb.blocks));
    // The table goes with the last command data that holds it.
    let table = Arc::downgrade(&a.blocks);
    drop((a, b));
    assert!(table.upgrade().is_none(), "block table kept alive by the sharing");
}
