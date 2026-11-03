//! Entity lookup by network identifier: the direct search of the entity lists against the
//! sorted list of every entity (`RUSTCRAFT_OPT_DIRECT_ENTITY_LOOKUP`). Test world required
//! (`RUSTCRAFT_SOLO_WORLD`).

use crate::commands::ents::E;
use crate::server::Server;

fn open(tag: &str) -> Option<Server> {
    let (Some(world), Some(data)) = (crate::tests::test_world(), rc_testdata::version_data()) else {
        eprintln!("test world or data missing: test skipped");
        return None;
    };
    let dir = crate::tests::fresh_copy(&world, tag);
    let mut s = crate::portal_tests::open_world(&dir, std::sync::Arc::new(data));
    s.loaded = true;
    s.teleport = None;
    let c = ((s.pos[0].floor() as i32) >> 4, (s.pos[2].floor() as i32) >> 4);
    let around: Vec<(i32, i32)> = (-1..=1).flat_map(|dx| (-1..=1).map(move |dz| (c.0 + dx, c.1 + dz))).collect();
    s.load_now(&around);
    Some(s)
}

fn summon(s: &mut Server, command: &str) {
    let src = s.server_source();
    let (ok, _) = s.perform(src, command);
    assert!(ok, "command refused: {command}");
}

/// One entity of every list `all_ents` joins: mob, ground item, orb, decoration, end crystal,
/// minecart.
fn summon_each_kind(s: &mut Server) {
    for c in [
        "summon minecraft:pig ~2 ~ ~",
        "summon minecraft:item ~1 ~ ~ {Item:{id:\"minecraft:stone\",count:1}}",
        "summon minecraft:experience_orb ~ ~ ~1",
        "summon minecraft:armor_stand ~ ~ ~2",
        "summon minecraft:end_crystal ~ ~ ~3",
        "summon minecraft:minecart ~3 ~ ~",
    ] {
        summon(s, c);
    }
}

#[test]
fn direct_entity_lookup_finds_what_the_sorted_list_found() {
    let Some(mut s) = open("entity-lookup") else { return };
    summon_each_kind(&mut s);
    let ents = s.all_ents();
    for kind in ["Mob", "Item", "Orb", "Other"] {
        assert!(ents.iter().any(|e| format!("{e:?}").starts_with(kind)), "no {kind} in {ents:?}");
    }
    let ids: Vec<i32> = ents.iter().map(|e| s.ent_id(*e)).collect();
    let (low, high) = (ids.iter().min().copied().unwrap_or(0), ids.iter().max().copied().unwrap_or(0));
    let mut found = 0;
    for id in (low - 3..=high + 3).chain([i32::MIN, -1, i32::MAX]) {
        let direct = s.e_of_net_direct(id);
        assert_eq!(direct, s.e_of_net_sorted(id), "identifier {id}");
        found += usize::from(direct.is_some_and(|e| e != E::Player));
    }
    assert_eq!(found, ents.iter().filter(|e| **e != E::Player).count(), "every entity found once");
}

/// Micro-bench (loaded machine figures in `docs/perf.md`):
/// `cargo test --release -p rc-sim entity_lookup_bench -- --ignored --nocapture`.
#[test]
#[ignore]
fn entity_lookup_bench() {
    let Some(mut s) = open("entity-lookup-bench") else { return };
    for _ in 0..200 {
        summon_each_kind(&mut s);
    }
    let ids: Vec<i32> = s.all_ents().iter().map(|e| s.ent_id(*e)).collect();
    let count = ids.len();
    for trial in 0..3 {
        for (name, direct) in [("sorted list", false), ("direct search", true)] {
            let t = std::time::Instant::now();
            for &id in &ids {
                let e = if direct { s.e_of_net_direct(id) } else { s.e_of_net_sorted(id) };
                std::hint::black_box(e);
            }
            let ns = t.elapsed().as_nanos() as f64 / count as f64;
            eprintln!("trial {trial}: {name}, {count} entities: {ns:.0} ns per lookup");
        }
    }
}
