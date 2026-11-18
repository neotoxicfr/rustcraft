//! Location paths (`RUSTCRAFT_OPT_LOCATIONFAST`) against the old ones: block tags looked up by the
//! block's index, entity types lent, and the `location` trigger walked over the test world
//! (`RUSTCRAFT_SOLO_WORLD`) on both paths: criteria granted in the same order.

use std::sync::Arc;

use super::paths::ProgressPaths;
use crate::blocks::probe::Probe;
use crate::mobs::species::Kind;

/// Every block against every block tag (full and short names, unknown tag and block): the tag
/// table gives the answer of the three scans.
#[test]
fn block_tags_by_index_match_the_scans() {
    if rc_testdata::version_data().is_none() {
        return;
    }
    let (mut p, dir) = crate::tests_g3f13::open("location-block-tags");
    let mut tags: Vec<String> =
        p.s.regs
            .tags
            .iter()
            .find(|(r, _)| r == "minecraft:block")
            .map(|(_, t)| t.iter().map(|(n, _)| n.clone()).collect())
            .unwrap_or_default();
    assert!(tags.len() > 100, "block tags: {}", tags.len());
    tags.push("minecraft:not_a_tag".into());
    let mut blocks: Vec<String> = p.s.data.blocks.iter().map(|b| b.name.clone()).collect();
    blocks.push("minecraft:not_a_block".into());
    let mut members = 0;
    for tag in &tags {
        let short = tag.strip_prefix("minecraft:").unwrap_or(tag).to_owned();
        for asked in [format!("#{tag}"), tag.clone(), short] {
            for block in &blocks {
                p.s.prog.paths.location_fast = false;
                let old = p.s.p_tag("minecraft:block", &asked, block);
                p.s.prog.paths.location_fast = true;
                assert_eq!(p.s.p_tag("minecraft:block", &asked, block), old, "{asked} {block}");
                members += usize::from(old);
            }
        }
    }
    assert!(members > 3_000, "too few members: {members}");
    crate::tests_g3f13::finish(p, dir);
}

/// A mob's type lent from its kind is the copied one.
#[test]
fn lent_entity_type_matches_the_copy() {
    if rc_testdata::version_data().is_none() {
        return;
    }
    let (mut p, dir) = crate::tests_g3f13::open("location-entity-type");
    for kind in [Kind::Pig, Kind::Zombie, Kind::Slime, Kind::MagmaCube, Kind::Villager] {
        let m = p.s.new_mob(kind, [0.5, -60.0, 0.5], 0.0);
        p.s.prog.paths.location_fast = false;
        let old = p.s.w_type(super::pred::Who::Mob(&m)).into_owned();
        p.s.prog.paths.location_fast = true;
        let new = p.s.w_type(super::pred::Who::Mob(&m));
        assert!(matches!(new, std::borrow::Cow::Borrowed(_)));
        assert_eq!(new, old);
    }
    crate::tests_g3f13::finish(p, dir);
}

/// The `location` criteria tested at points of the test world, on each path: same grants.
fn location_walk(paths: ProgressPaths, tag: &str) -> Option<Vec<(usize, usize)>> {
    let (world, data) = (crate::tests::test_world()?, rc_testdata::version_data()?);
    let dir = crate::tests::fresh_copy(&world, tag);
    let mut p = ProgressPaths::forced(paths, || Probe::open(&dir, Arc::new(data)));
    p.s.loaded = true;
    p.s.game_mode = 0;
    let around: Vec<(i32, i32)> = (-4..=4).flat_map(|x| (-4..=4).map(move |z| (x, z))).collect();
    p.s.load_now(&around);
    p.s.prog.grant_log.clear();
    for x in (-60..60).step_by(9) {
        for z in (-60..60).step_by(11) {
            for y in [-50.0, 0.0, 64.0, 70.0, 120.0] {
                p.s.pos = [f64::from(x) + 0.5, y, f64::from(z) + 0.5];
                p.s.trigger("minecraft:location", &|_, _| true);
            }
        }
    }
    let log = std::mem::take(&mut p.s.prog.grant_log);
    drop(p);
    Some(log)
}

#[test]
fn location_trigger_grants_the_same_on_both_paths() {
    let Some(new) = location_walk(ProgressPaths::NEW, "location-new") else { return };
    let old = location_walk(ProgressPaths::OLD, "location-old").expect("second copy");
    assert!(!new.is_empty(), "no location criterion reached");
    assert_eq!(new, old);
}
