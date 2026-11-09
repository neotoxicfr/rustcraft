//! Advancements file read in one pass (`RUSTCRAFT_OPT_ADVLOADFAST`): reference files (none, medium,
//! complete, unknown keys, shuffled order, a key written twice) read by both paths give the same
//! criteria, order, packets and rewritten file; a file we wrote is rewritten to the byte; the tree
//! parsed on every core equals the one parsed in order.

use std::path::Path;
use std::sync::Arc;

use super::{Progress, ProgressPaths, Tree};

fn base_tree() -> Option<Arc<Tree>> {
    let data = rc_testdata::version_data()?;
    let jar = rc_testdata::need(crate::registries::client_jar(&data), "26.3 client jar")?;
    let jar = crate::registries::Jar::read(&jar).expect("jar");
    Some(Arc::new(Tree::load(&jar)))
}

fn file(world: &Path) -> std::path::PathBuf {
    world.join("players").join("advancements").join(format!("{}.json", crate::nbtio::uuid_text(7)))
}

fn read(tree: &Arc<Tree>, world: &Path, paths: ProgressPaths) -> Progress {
    ProgressPaths::forced(paths, || Progress::new(tree.clone(), world, 7))
}

/// The file our progress writes after granting `picks` (advancement, criterion) in that order.
fn written(tree: &Arc<Tree>, world: &Path, picks: &[(usize, usize)]) -> String {
    let _ = std::fs::remove_file(file(world));
    let mut p = read(tree, world, ProgressPaths::NEW);
    for &(a, c) in picks {
        p.grant(a, c);
    }
    p.to_json(4000)
}

/// What a reading gave: criteria (with dates), order, first packet, rewritten file.
fn state(p: &mut Progress) -> (Vec<Vec<Option<i64>>>, Vec<usize>, String, String) {
    let flush = format!("{:?}", p.flush());
    (p.got.clone(), p.order.clone(), flush, p.to_json(4000))
}

fn same_reading(tree: &Arc<Tree>, world: &Path, text: &str, what: &str) -> Progress {
    std::fs::create_dir_all(file(world).parent().unwrap()).unwrap();
    std::fs::write(file(world), text).unwrap();
    let mut new = read(tree, world, ProgressPaths::NEW);
    let mut old = read(tree, world, ProgressPaths::OLD);
    assert_eq!(state(&mut new), state(&mut old), "{what}: the two readings differ");
    // Then the same grants on both: same order kept (flag against scan).
    for (a, adv) in tree.list.iter().enumerate().step_by(97) {
        for c in 0..adv.criteria.len() {
            new.grant(a, c);
            old.grant(a, c);
        }
    }
    assert_eq!(new.order, old.order, "{what}: order after grants");
    assert_eq!(format!("{:?}", new.flush()), format!("{:?}", old.flush()), "{what}: packet after grants");
    read(tree, world, ProgressPaths::NEW)
}

#[test]
fn one_pass_reading_matches_the_key_search() {
    let Some(tree) = base_tree() else { return };
    let world = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-adv-load-{}", std::process::id())));
    let n = tree.list.len();
    // No file.
    let mut none_new = read(&tree, &world, ProgressPaths::NEW);
    let mut none_old = read(&tree, &world, ProgressPaths::OLD);
    assert_eq!(state(&mut none_new), state(&mut none_old));
    // Medium: every third advancement, granted from the last one down (file order ≠ sorted order).
    let medium: Vec<(usize, usize)> = (0..n).rev().step_by(3).map(|a| (a, 0)).collect();
    let text = written(&tree, &world, &medium);
    let back = same_reading(&tree, &world, &text, "medium");
    assert_eq!(back.to_json(4000), text, "medium file not rewritten to the byte");
    // Complete, shuffled by a fixed stride.
    let mut all: Vec<(usize, usize)> =
        tree.list.iter().enumerate().flat_map(|(a, adv)| (0..adv.criteria.len()).map(move |c| (a, c))).collect();
    let len = all.len();
    all = (0..len).map(|i| all[(i * 7919) % len]).collect();
    let text = written(&tree, &world, &all);
    let back = same_reading(&tree, &world, &text, "complete");
    assert_eq!(back.to_json(4000), text, "complete file not rewritten to the byte");
    assert!(back.order.len() == n, "complete: {} of {n}", back.order.len());
    // Unknown advancement and criterion, a key written twice (the last value at the first place).
    let medium_text = written(&tree, &world, &medium);
    let first = &tree.list[medium[0].0];
    let odd = medium_text.replacen(
        "{\n",
        &format!(
            "{{\n  \"minecraft:does/not_exist\": {{\n    \"criteria\": {{\n      \"x\": \"2025-10-09 08:53:20 +0000\"\n    }},\n    \
             \"done\": true\n  }},\n  \"{}\": {{\n    \"criteria\": {{\n      \"nope\": \"2025-10-09 08:53:20 +0000\"\n    }},\n    \
             \"done\": false\n  }},\n",
            first.id
        ),
        1,
    );
    same_reading(&tree, &world, &odd, "unknown keys and a key written twice");
}

/// A key written with a space before its colon (not the game's own writing): the former search
/// does not find it and places the advancement by its date, the one pass keeps the written order,
/// as the game's ordered table does (difference faithful to the game).
#[test]
fn one_pass_reading_keeps_the_written_order() {
    let Some(tree) = base_tree() else { return };
    let world = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-adv-order-{}", std::process::id())));
    let (a, b) = (tree.get("minecraft:story/root").unwrap(), tree.get("minecraft:adventure/root").unwrap());
    let (na, nb) = (&tree.list[a].criteria[0].name, &tree.list[b].criteria[0].name);
    let text = format!(
        "{{\n  \"minecraft:story/root\" : {{\"criteria\": {{\"{na}\": \"2025-10-09 08:53:20 +0000\"}}, \"done\": true}},\n  \
         \"minecraft:adventure/root\": {{\"criteria\": {{\"{nb}\": \"2025-10-09 08:53:19 +0000\"}}, \"done\": true}},\n  \
         \"DataVersion\": 4000\n}}"
    );
    std::fs::create_dir_all(file(&world).parent().unwrap()).unwrap();
    std::fs::write(file(&world), text).unwrap();
    assert_eq!(read(&tree, &world, ProgressPaths::NEW).order, [a, b], "written order");
    assert_eq!(read(&tree, &world, ProgressPaths::OLD).order, [b, a], "former search (by date)");
}

/// The tree parsed on every core: same advancements, links, criteria, positions and indexes.
#[test]
fn parallel_tree_matches_the_sequential_one() {
    let Some(data) = rc_testdata::version_data() else { return };
    let Some(jar) = rc_testdata::need(crate::registries::client_jar(&data), "26.3 client jar") else { return };
    let jar = crate::registries::Jar::read(&jar).expect("jar");
    let summary = |t: &Tree| -> Vec<String> {
        let mut v: Vec<String> = t
            .list
            .iter()
            .map(|a| {
                let crits: Vec<(&str, &str)> =
                    a.criteria.iter().map(|c| (c.name.as_str(), c.trigger.as_str())).collect();
                format!("{} {:?} {:?} {:?} {} {} {crits:?}", a.id, a.parent, a.children, a.req, a.x, a.y)
            })
            .collect();
        let mut triggers: Vec<_> = t.by_trigger.iter().collect();
        triggers.sort();
        v.push(format!("{triggers:?}"));
        v.push(format!("{:?}", t.indexed[super::criteria_index::INVENTORY_CHANGED].sizes()));
        v
    };
    let parallel = ProgressPaths::forced(ProgressPaths::NEW, || Tree::load(&jar));
    let sequential = ProgressPaths::forced(ProgressPaths::OLD, || Tree::load(&jar));
    assert!(parallel.list.len() > 1_800);
    let expected = summary(&sequential);
    assert_eq!(summary(&parallel), expected);
    // Shares of uneven sizes, more threads than shares.
    for threads in [2, 3, 32] {
        assert_eq!(summary(&Tree::load_on(&jar, threads)), expected, "{threads} threads");
    }
}
