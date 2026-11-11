//! Equivalence of the visibility paths on the real sections of the reference capture (outside the
//! repository): face communication of every non-empty section through each dilation path against
//! the cell-by-cell fill, walks with or without the per-axis ray against the reference walk,
//! with frozen fingerprints, and walks stopped at a sight limit against the full walk cut there.
//! Data absent: skipped, or failure under `RUSTCRAFT_OPT_REQUIRE=1`.

mod common;

use hashbrown::HashMap;

use common::{MIN_Y, capture_solid_masks as capture_masks};
use rc_mesh::visibility::{
    DilationPaths, Graph, VIS_ALL, Vis, WalkPaths, beyond_sight, section_vis_fill, section_vis_with, visible_sections,
};

/// Sections per column, walk radius of the bench.
const SECTIONS: i32 = 24;
const RADIUS: i32 = 32;

fn fnv(words: impl Iterator<Item = u64>) -> u64 {
    words.fold(0xcbf2_9ce4_8422_2325, |h, w| (h ^ w).wrapping_mul(0x100_0000_01b3))
}

/// Communications of the 13,768 non-empty sections, cell-by-cell fill (frozen with the dilation
/// of before, which gives the same).
const EXPECTED_SECTION_VIS: u64 = 0x7ffc_c1bb_5216_0bc4;
/// Walk nodes (section, arrivals, steps) from the eyes of `walk_paths_match_on_the_capture`.
const EXPECTED_WALK_NODES: u64 = 0xe1a6_faea_273b_68ce;

#[test]
fn dilation_paths_match_the_fill_on_the_capture() {
    let Some(cols) = capture_masks() else { return };
    let sections: Vec<[u64; 64]> = cols.iter().flat_map(|(_, m)| m.iter().flatten().copied()).collect();
    let fill: Vec<Vis> = sections.iter().map(section_vis_fill).collect();
    for p in DilationPaths::every_combination() {
        for (i, (s, f)) in sections.iter().zip(&fill).enumerate() {
            assert_eq!(section_vis_with(s, p), *f, "section {i}, {p:?}");
        }
    }
    let partial = fill.iter().filter(|&&v| v != VIS_ALL && v != 0).count();
    eprintln!("{} non-empty sections, {partial} with partial communications", sections.len());
    assert_eq!(fnv(fill.iter().copied()), EXPECTED_SECTION_VIS, "communications of the capture");
}

#[test]
fn walk_paths_match_on_the_capture() {
    let Some(cols) = capture_masks() else { return };
    let vis: HashMap<(i32, i32), Vec<Vis>> = cols
        .iter()
        .map(|(k, m)| (*k, m.iter().map(|s| s.as_ref().map_or(VIS_ALL, section_vis_fill)).collect()))
        .collect();
    let n = vis.len() as i32;
    let (sx, sz) = vis.keys().fold((0, 0), |a, k| (a.0 + k.0, a.1 + k.1));
    let (cx, cz) = (f64::from(sx / n * 16), f64::from(sz / n * 16));
    // Ground, cave, high in the air, above the world, off-centre.
    let eyes = [
        [cx + 8.3, 80.6, cz + 8.7],
        [cx + 3.5, -20.2, cz + 12.1],
        [cx + 8.3, 190.4, cz + 8.7],
        [cx + 1.1, 330.0, cz + 2.9],
        [cx + 150.6, 70.3, cz - 120.4],
    ];
    let column = |x, z| vis.get(&(x, z)).map(Vec::as_slice);
    let mut graphs = WalkPaths::every_combination().map(Graph::with_paths);
    let mut digest = Vec::new();
    for (k, eye) in eyes.into_iter().enumerate() {
        let reference = visible_sections(column, eye, MIN_Y, SECTIONS, RADIUS, true);
        for (g, paths) in graphs.iter_mut().zip(WalkPaths::every_combination()) {
            assert_eq!(g.run(column, eye, MIN_Y, SECTIONS, RADIUS, true), reference, "eye {k}, {paths:?}");
        }
        let [old, others @ ..] = &graphs;
        for (g, paths) in others.iter().zip(&WalkPaths::every_combination()[1..]) {
            assert!(old.nodes().eq(g.nodes()), "eye {k}, {paths:?}: arrivals or steps differ");
        }
        eprintln!("eye {k}: {} sections reached", reference.len());
        digest.extend(old.nodes().flat_map(|((x, y, z), sources, step)| {
            [x as u64, y as u64, z as u64, u64::from(sources), u64::from(step)]
        }));
    }
    assert_eq!(fnv(digest.into_iter()), EXPECTED_WALK_NODES, "walk nodes of the capture");
}

/// A walk stopped at a sight limit reaches, on the capture, the sections of the full walk that
/// have a point short of the limit: same order, same arrivals and steps, for every ray path.
#[test]
fn walk_within_sight_is_the_full_walk_cut_on_the_capture() {
    let Some(cols) = capture_masks() else { return };
    let vis: HashMap<(i32, i32), Vec<Vis>> = cols
        .iter()
        .map(|(k, m)| (*k, m.iter().map(|s| s.as_ref().map_or(VIS_ALL, section_vis_fill)).collect()))
        .collect();
    let n = vis.len() as i32;
    let (sx, sz) = vis.keys().fold((0, 0), |a, k| (a.0 + k.0, a.1 + k.1));
    let (cx, cz) = (f64::from(sx / n * 16), f64::from(sz / n * 16));
    let eyes = [
        [cx + 8.3, 80.6, cz + 8.7],
        [cx + 3.5, -20.2, cz + 12.1],
        [cx + 8.3, 190.4, cz + 8.7],
        [cx + 1.1, 330.0, cz + 2.9],
        [cx + 150.6, 70.3, cz - 120.4],
    ];
    let column = |x, z| vis.get(&(x, z)).map(Vec::as_slice);
    let mut full = Graph::with_paths(WalkPaths::OLD);
    let mut cut_walks = WalkPaths::every_combination().map(Graph::with_paths);
    for (k, eye) in eyes.into_iter().enumerate() {
        full.run(column, eye, MIN_Y, SECTIONS, RADIUS, true);
        // Limits of the fogs that hide the most (lava to the Nether, with their margin) and beyond.
        for limit in [37.9, 69.9, 117.9, 149.9, 277.9, 533.9] {
            let kept: Vec<_> = full.nodes().filter(|node| !beyond_sight(eye, MIN_Y, node.0, limit)).collect();
            for walk in &mut cut_walks {
                let reached = walk.run_within(column, eye, MIN_Y, SECTIONS, RADIUS, true, limit).to_vec();
                assert!(reached.iter().eq(kept.iter().map(|node| &node.0)), "eye {k}, limit {limit}: sections");
                assert!(walk.nodes().eq(kept.iter().copied()), "eye {k}, limit {limit}: arrivals or steps");
            }
            eprintln!("eye {k}, limit {limit}: {} of {} sections kept", kept.len(), full.nodes().count());
        }
    }
}
