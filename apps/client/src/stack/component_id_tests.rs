//! Proofs of the component id lookups: every path answers like the former walk over the
//! registry names, on the game's registry and on registries built to trip the lookup by full name.

use super::*;

const BY_NAME_WALK: ComponentLookup = ComponentLookup::FORMER;
/// Every combination of the three switches.
fn every_lookup() -> Vec<ComponentLookup> {
    let on_off = [false, true];
    on_off
        .into_iter()
        .flat_map(|by_full_name| on_off.map(move |hashed| (by_full_name, hashed)))
        .flat_map(|(by_full_name, hashed)| {
            on_off.map(move |resolved_table| ComponentLookup { by_full_name, hashed, resolved_table })
        })
        .collect()
}

fn data_with_component_types(names: &[&str]) -> VersionData {
    let entries = names.iter().map(|name| (*name).to_owned()).collect();
    VersionData { registries: vec![rc_data::Registry::new(COMPONENT_TYPE_REGISTRY, entries)], ..Default::default() }
}

/// Names asked of a registry: each entry as is, without and with a second namespace, plus names
/// no registry holds.
fn asked_names(data: &VersionData) -> Vec<String> {
    let entries = data.registry(COMPONENT_TYPE_REGISTRY).map(|r| r.entries.clone()).unwrap_or_default();
    let mut asked: Vec<String> =
        ["", "minecraft:", "absent", "minecraft:absent", "foo:bar", "minecraft:foo:bar"].map(str::to_owned).to_vec();
    for entry in &entries {
        asked.push(entry.clone());
        asked.push(entry.strip_prefix("minecraft:").unwrap_or(entry).to_owned());
        asked.push(format!("minecraft:{entry}"));
    }
    asked
}

/// The walk `ComponentIds::of` did before, written out again as the reference.
fn walk_ignoring_the_namespace(data: &VersionData, name: &str) -> Option<u16> {
    let path = name.strip_prefix("minecraft:").unwrap_or(name);
    let entries = &data.registry(COMPONENT_TYPE_REGISTRY)?.entries;
    entries.iter().position(|e| e.strip_prefix("minecraft:").unwrap_or(e) == path).map(|i| i as u16)
}

fn assert_every_lookup_answers_like_the_walk(data: &VersionData, registry: &str) {
    let former = ComponentIds::with_lookup(data, BY_NAME_WALK);
    for lookup in every_lookup() {
        let ids = ComponentIds::with_lookup(data, lookup);
        for name in asked_names(data) {
            let expected = walk_ignoring_the_namespace(data, &name);
            assert_eq!(ids.of(&name), expected, "{registry}: `{name}` through {lookup:?}");
            let full_name_walk =
                data.registry(COMPONENT_TYPE_REGISTRY).and_then(|r| r.entries.iter().position(|e| *e == name));
            for indexed in [false, true] {
                let found = component_type_id_with(data, &name, indexed);
                assert_eq!(
                    found,
                    full_name_walk.map(|i| i as u16),
                    "{registry}: full name `{name}`, indexed {indexed}"
                );
            }
        }
        for &component in DataComponent::ALL {
            let expected = walk_ignoring_the_namespace(data, component.path());
            assert_eq!(ids.id(component), expected, "{registry}: {component:?} through {lookup:?}");
        }
        // The ids kept as fields were found by full name: same on every path.
        let fields = |ids: &ComponentIds| {
            [
                ids.damage,
                ids.max_damage,
                ids.enchantments,
                ids.custom_name,
                ids.map_id,
                ids.bundle_contents,
                ids.equippable,
            ]
        };
        assert_eq!(fields(&ids), fields(&former), "{registry}: fields through {lookup:?}");
    }
}

#[test]
fn component_ids_of_the_game_registry_match_the_former_walk() {
    let data = rc_testdata::version_data().expect("version data missing (extraction)");
    let names = &data.registry(COMPONENT_TYPE_REGISTRY).expect("component type registry").entries;
    assert!(names.len() > 90, "{} component types", names.len());
    assert_every_lookup_answers_like_the_walk(&data, "game registry");
    // Stored ids of game 26.3 (network order of the registry).
    let ids = ComponentIds::new(&data);
    let stored = [("custom_data", 0), ("max_stack_size", 1), ("max_damage", 2), ("damage", 3)];
    for (name, id) in stored {
        assert_eq!(ids.of(name), Some(id), "{name}");
        assert_eq!(ids.of(&format!("minecraft:{name}")), Some(id), "minecraft:{name}");
    }
    assert_eq!(ids.id(DataComponent::Damage), Some(3));
    assert_eq!(ids.of("no_such_component"), None);
    // Every listed component exists in the game's registry: a misspelt path would stay `None`.
    let unknown: Vec<_> = DataComponent::ALL.iter().filter(|c| ids.id(**c).is_none()).collect();
    assert!(unknown.is_empty(), "listed components missing from the registry: {unknown:?}");
}

/// Registries the game never has, where a lookup by full name alone would answer differently
/// from the walk: an entry without namespace, the same path with and without it (either order),
/// a foreign namespace, a duplicate.
#[test]
fn component_ids_of_odd_registries_match_the_former_walk() {
    let registries: [(&str, &[&str]); 6] = [
        ("empty", &[]),
        ("entry without namespace", &["minecraft:damage", "lore", "minecraft:trim"]),
        ("bare path before the namespaced one", &["damage", "minecraft:damage", "minecraft:lore"]),
        ("namespaced path before the bare one", &["minecraft:damage", "damage", "minecraft:lore"]),
        ("foreign namespace", &["minecraft:damage", "foo:bar", "foo:lore", "minecraft:lore"]),
        ("duplicate", &["minecraft:lore", "minecraft:damage", "minecraft:lore", "minecraft:minecraft:trim"]),
    ];
    for (label, names) in registries {
        assert_every_lookup_answers_like_the_walk(&data_with_component_types(names), label);
    }
    assert_every_lookup_answers_like_the_walk(&VersionData::default(), "no registry");
}

/// The ids hold the registry's shared name list, not a copy of it.
#[test]
fn component_names_are_shared_with_the_registry() {
    let data = data_with_component_types(&["minecraft:damage", "minecraft:lore"]);
    let shared = data.registry(COMPONENT_TYPE_REGISTRY).expect("registry").shared();
    let (first, second) = (ComponentIds::new(&data), ComponentIds::new(&data));
    assert!(Arc::ptr_eq(&first.names, &shared) && Arc::ptr_eq(&second.names, &shared));
}

/// Nanoseconds per call of `lookup`, best of five passes over `names`.
fn nanoseconds_per_lookup<T>(names: &[T], mut lookup: impl FnMut(&T) -> Option<u16>) -> f64 {
    const ROUNDS: usize = 10_000;
    (0..5)
        .map(|_| {
            let start = std::time::Instant::now();
            for _ in 0..ROUNDS {
                for name in names {
                    std::hint::black_box(lookup(std::hint::black_box(name)));
                }
            }
            start.elapsed().as_nanos() as f64 / (ROUNDS * names.len()) as f64
        })
        .fold(f64::MAX, f64::min)
}

/// Micro-bench (ignored): `cargo test --release -p rc-app bench_component_id_lookup -- --ignored
/// --nocapture`. One lookup of each listed component, on each path, then on every core at once.
#[test]
#[ignore = "micro-bench"]
fn bench_component_id_lookup() {
    let data = rc_testdata::version_data().expect("version data missing (extraction)");
    let paths: Vec<&str> = DataComponent::ALL.iter().map(|c| c.path()).collect();
    let rows = [
        ("walk over the names (former)", ComponentLookup::FORMER),
        ("full name, rc-data walk", ComponentLookup { by_full_name: true, hashed: false, resolved_table: false }),
        ("full name, hashed", ComponentLookup { by_full_name: true, hashed: true, resolved_table: false }),
    ];
    println!("{} names looked up in a registry of {}", paths.len(), ComponentIds::new(&data).names.len());
    for (label, lookup) in rows {
        let ids = ComponentIds::with_lookup(&data, lookup);
        println!("{label:<30} {:>7.1} ns per lookup", nanoseconds_per_lookup(&paths, |path| ids.of(path)));
    }
    let table = ComponentLookup { by_full_name: true, hashed: true, resolved_table: true };
    let ids = ComponentIds::with_lookup(&data, table);
    let per_read = nanoseconds_per_lookup(DataComponent::ALL, |component| ids.id(*component));
    println!("{:<30} {per_read:>7.1} ns per lookup", "table of resolved ids");
    for indexed in [false, true] {
        let label = if indexed { "version data, indexed" } else { "version data, walk (former)" };
        let full: Vec<String> = paths.iter().map(|p| format!("minecraft:{p}")).collect();
        let per_lookup = nanoseconds_per_lookup(&full, |name| component_type_id_with(&data, name, indexed));
        println!("{label:<30} {per_lookup:>7.1} ns per lookup");
    }
    // Every core at once: the index and the table are read-only once built, nothing is locked.
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    let hashed =
        ComponentIds::with_lookup(&data, ComponentLookup { by_full_name: true, hashed: true, resolved_table: false });
    let mut threads = 2;
    while threads <= cores {
        let slowest = std::thread::scope(|scope| {
            let workers: Vec<_> =
                (0..threads).map(|_| scope.spawn(|| nanoseconds_per_lookup(&paths, |path| hashed.of(path)))).collect();
            workers.into_iter().map(|w| w.join().expect("bench thread")).fold(0.0, f64::max)
        });
        println!("full name, hashed, {threads:>2} threads {slowest:>6.1} ns per lookup (slowest thread)");
        threads *= 2;
    }
}
