//! Equivalence proofs of the village, golem, raid and illager search paths: the scenarios of
//! `search_scenarios` give the same trace, tick by tick, with every new path off (the old code)
//! and on, and the same POI registry at the end (insertion ranks, chunks to save); the ominous
//! banner test and the raider species agree with their old forms.

use rc_proto::nbt::Nbt;

use super::super::species::Kind;
use super::search_scenarios::{SCENARIOS, record, walking_layer};
use crate::player::Stack;

fn old_searches() {
    crate::poi::FORCE_OLD_SEARCHES.set(true);
}

/// First tick where two traces differ, with both snapshots.
fn first_difference(a: &[String], b: &[String]) -> Option<(usize, String, String)> {
    let at = a.iter().zip(b).position(|(x, y)| x != y).or_else(|| (a.len() != b.len()).then(|| a.len().min(b.len())))?;
    Some((at, a.get(at).cloned().unwrap_or_default(), b.get(at).cloned().unwrap_or_default()))
}

/// New paths run by the last "new" scenario thread (read at its end, in that thread).
static NEW_PATHS_RUN: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

#[test]
fn scenarios_run_the_same_with_the_old_and_new_searches() {
    let registry = |s: &crate::server::Server| s.mobs.poi.fingerprint() + &walking_layer(s);
    let registry_and_paths = |s: &crate::server::Server| {
        *NEW_PATHS_RUN.lock().unwrap_or_else(std::sync::PoisonError::into_inner) =
            format!("{:?}", crate::poi::search_path_uses());
        s.mobs.poi.fingerprint() + &walking_layer(s)
    };
    for sc in &SCENARIOS {
        let Some(old) = record(sc, "old", old_searches, registry) else { return };
        let Some(new) = record(sc, "new", crate::poi::start_noting_search_paths, registry_and_paths) else { return };
        let paths = NEW_PATHS_RUN.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        eprintln!("scenario {}: new paths run {paths}", sc.name);
        if let Some((tick, a, b)) = first_difference(&old, &new) {
            panic!("scenario {}: first difference at tick {tick}\nold:\n{a}\nnew:\n{b}", sc.name);
        }
        for name in sc.exercises {
            assert!(paths.contains(&format!("{name:?}")), "scenario {} never went through {name:?}", sc.name);
        }
        eprintln!("scenario {}: {} ticks identical", sc.name, old.len());
    }
}

/// The comparison sees a change of a single mob (sabotage of a trace).
#[test]
fn trace_comparison_catches_one_changed_mob() {
    let a = vec!["t 1\n1 Zombie at [1, 2, 3]\n".to_owned(), "t 2\n1 Zombie at [1, 2, 3]\n".to_owned()];
    let mut b = a.clone();
    b[1] = b[1].replace("[1, 2, 3]", "[1, 2, 4]");
    assert_eq!(first_difference(&a, &b).map(|d| d.0), Some(1));
    assert_eq!(first_difference(&a, &a), None);
    assert_eq!(first_difference(&a, &a[..1]).map(|d| d.0), Some(1));
}

#[test]
fn raider_kinds_are_the_raiders() {
    for k in Kind::ALL {
        assert_eq!(super::RAIDER_KINDS.contains(&k), super::is_raider(k), "{k:?}");
    }
}

/// Stacks around the ominous banner: the kept reference answers like the one built at each call.
#[test]
fn ominous_banner_test_agrees_with_the_built_reference() {
    let banner = super::ominous_banner();
    let mut cases: Vec<(&str, Stack)> = vec![("banner", banner.clone())];
    let mut two = banner.clone();
    two.count = 2;
    cases.push(("count 2", two));
    let mut black = banner.clone();
    black.id = "minecraft:black_banner".into();
    cases.push(("other banner", black));
    let mut bare = banner.clone();
    bare.components = None;
    cases.push(("no components", bare));
    let Some(Nbt::Compound(comps)) = banner.components.clone() else { panic!("components") };
    let mut reordered = comps.clone();
    reordered.reverse();
    cases.push(("top-level keys reordered", Stack { components: Some(Nbt::Compound(reordered)), ..banner.clone() }));
    let mut renamed = comps.clone();
    renamed[2].1 = Nbt::Compound(vec![("translate".into(), Nbt::String("block.minecraft.white_banner".into()))]);
    cases.push(("other name", Stack { components: Some(Nbt::Compound(renamed)), ..banner.clone() }));
    let mut nested = comps.clone();
    if let Nbt::List(patterns) = &mut nested[0].1 {
        patterns.swap(0, 1);
    }
    cases.push(("patterns swapped", Stack { components: Some(Nbt::Compound(nested)), ..banner.clone() }));
    let mut extra = comps.clone();
    extra.push(("minecraft:repair_cost".into(), Nbt::Int(1)));
    cases.push(("extra component", Stack { components: Some(Nbt::Compound(extra)), ..banner.clone() }));
    cases.push(("missing component", Stack { components: Some(Nbt::Compound(comps[..3].to_vec())), ..banner.clone() }));
    cases.push(("stone", Stack { id: "minecraft:stone".into(), count: 1, components: None }));
    let expected = [true, false, false, false, true, false, false, false, false, false];
    for ((name, s), want) in cases.iter().zip(expected) {
        let old = super::is_ominous_banner_building_reference(s);
        assert_eq!(super::is_ominous_banner_against_kept_reference(s), old, "{name}");
        assert_eq!(old, want, "{name}");
    }
    // Sabotage: a fast test that forgot the count is caught by the count-2 case.
    let forgot_count = |s: &Stack| {
        s.id == banner.id && super::top_level_components_eq(s.components.as_ref(), banner.components.as_ref())
    };
    assert_ne!(forgot_count(&cases[1].1), super::is_ominous_banner_building_reference(&cases[1].1));
}

/// Micro-bench: ominous banner test, old against kept reference (banner, other stack).
/// `cargo test --release -p rc-sim ominous_banner_bench -- --ignored --nocapture`.
#[test]
#[ignore = "measurement bench"]
fn ominous_banner_bench() {
    use std::hint::black_box;
    let banner = super::ominous_banner();
    let stone = Stack { id: "minecraft:stone".into(), count: 1, components: None };
    for (name, s) in [("banner", &banner), ("stone", &stone)] {
        let time = |f: fn(&Stack) -> bool| {
            let t = std::time::Instant::now();
            let n = 200_000;
            let mut hits = 0;
            for _ in 0..n {
                hits += usize::from(f(black_box(s)));
            }
            black_box(hits);
            t.elapsed().as_secs_f64() * 1e9 / f64::from(n)
        };
        let old = time(super::is_ominous_banner_building_reference);
        let new = time(super::is_ominous_banner_against_kept_reference);
        eprintln!("ominous banner test on {name}: built reference {old:.1} ns, kept reference {new:.1} ns");
    }
}
