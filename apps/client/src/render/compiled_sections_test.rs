//! Proofs of the compiled sections the entity and block entity tests read (`compiled_sections.rs`):
//! the game's rules over scripted walks. Every instant is given by the test from one base instant,
//! so no verdict depends on how fast the machine runs.

use std::sync::Arc;
use std::time::{Duration, Instant};

use hashbrown::HashMap;
use rc_mesh::visibility::{VIS_ALL, VIS_NONE};

use super::*;
use crate::render::frame_visibility::{CompiledView, FrameVisibility, VisibilityPaths};
use crate::render::frustum::{FrameView, frustum_planes};
use crate::render::lists::Reach;

const SECTIONS: usize = 6;
/// The default fade (`chunkSectionFadeInTime`): shown from 225 ms on.
const FADE: f64 = 0.75;

fn at(base: Instant, ms: u64) -> Instant {
    base + Duration::from_millis(ms)
}

fn no_unmeshed() -> HashMap<(i32, i32), Connectivity> {
    HashMap::new()
}

/// A section of a meshed column is compiled from the first walk that reaches it and stays so when
/// later walks no longer reach it (behind a hill now); it shows from 0.3 of its fade on.
#[test]
fn reached_sections_of_meshed_columns_stay_compiled() {
    let base = Instant::now();
    let mut compiled = CompiledSections::new(base);
    let meshed = |k: (i32, i32)| k == (0, 0);
    compiled.mark_reached(&[(0, 2, 0), (0, 3, 0)], SECTIONS, meshed, &no_unmeshed(), at(base, 1000));
    assert!(!compiled.shown((0, 0), 2, at(base, 1224), FADE), "shown before 0.3 of its fade");
    assert!(compiled.shown((0, 0), 2, at(base, 1225), FADE));
    assert!(!compiled.shown((0, 0), 4, at(base, 5000), FADE), "never reached: not compiled");
    compiled.mark_reached(&[(0, 4, 0)], SECTIONS, meshed, &no_unmeshed(), at(base, 2000));
    assert!(compiled.shown((0, 0), 3, at(base, 1300), FADE), "no longer reached: still compiled, from its first time");
    assert!(!compiled.shown((0, 0), 4, at(base, 2100), FADE) && compiled.shown((0, 0), 4, at(base, 2225), FADE));
    assert!(compiled.shown((0, 0), 4, at(base, 2000), 0.0), "without a fade, shown at once");
    compiled.forget_column((0, 0));
    assert!(!compiled.shown((0, 0), 3, at(base, 9000), FADE), "a forgotten column keeps nothing");
}

/// Before its mesh, only the empty sections a walk reaches of a loaded column are compiled (the
/// game takes them as compiled empty); its other sections, reached or not, wait for the mesh, and a
/// column neither meshed nor loaded gets nothing. The mesh's first walk compiles the rest it reaches.
#[test]
fn only_the_empty_sections_of_a_column_waiting_for_its_mesh_are_compiled() {
    let base = Instant::now();
    let mut compiled = CompiledSections::new(base);
    let provisional: Connectivity = [VIS_NONE, VIS_NONE, VIS_ALL, VIS_ALL, VIS_ALL, VIS_ALL].into();
    let unmeshed: HashMap<_, _> = [((1, 0), provisional)].into_iter().collect();
    let reached = [(1, 1, 0), (1, 2, 0), (1, 3, 0), (5, 3, 5)];
    compiled.mark_reached(&reached, SECTIONS, |_| false, &unmeshed, at(base, 0));
    let later = at(base, 1000);
    assert!(!compiled.shown((1, 0), 1, later, FADE), "a section holding blocks waits for the mesh");
    assert!(compiled.shown((1, 0), 2, later, FADE) && compiled.shown((1, 0), 3, later, FADE));
    assert!(!compiled.shown((1, 0), 4, later, FADE), "an empty section the walk did not reach");
    assert!(!compiled.shown((5, 5), 3, later, FADE), "a column not loaded");
    compiled.mark_reached(&reached[..3], SECTIONS, |k| k == (1, 0), &no_unmeshed(), at(base, 900));
    assert!(!compiled.shown((1, 0), 1, at(base, 1000), FADE) && compiled.shown((1, 0), 1, at(base, 1125), FADE));
    assert!(compiled.shown((1, 0), 2, at(base, 300), FADE), "an empty section keeps its first instant");
}

/// A camera at (8.5, 70, 8.5) looking south (+z); the walk reached its section and section 9 of
/// column (0, 2), 32 blocks ahead.
fn view_of_a_scripted_walk() -> FrameView {
    let eye = [8.5, 70.0, 8.5];
    let (origin, _, view_m) = crate::camera::view_matrix(eye, 0.0, 0.0);
    let planes = frustum_planes(crate::render::perspective(70.0, 16.0 / 9.0, 0.05, 512.0) * view_m);
    let reached = Reach::Set([(0, 8, 0), (0, 9, 2)].into_iter().collect());
    FrameView { planes, origin, min_y: -64, sections: 24, reached: Arc::new(reached), occlusion: true }
}

/// The entities' test is the game's: in the field and in a compiled section that has appeared,
/// whether the walk reaches that section now or not. An entity behind a hill in a section seen
/// once is drawn (the walk-based test of before hid it); one in a section the walk reaches but
/// never compiled is not (the test of before drew it); above or below the world, the field alone.
#[test]
fn entities_need_the_field_and_a_compiled_section_not_the_walk() {
    let base = Instant::now();
    let mut compiled = CompiledSections::new(base);
    // Column (0, 2): section 8 (y 64 to 79) compiled long ago, section 9 (y 80 to 95) never.
    compiled.mark((0, 2), 24, [8].into_iter(), base);
    let compiled = CompiledView { sections: Arc::new(compiled), now: at(base, 10_000), fade_secs: FADE };
    let visibility =
        FrameVisibility::new(view_of_a_scripted_walk(), VisibilityPaths::OLD).with_compiled_sections(Some(compiled));
    // An entity of 1 × 1.8 × 1 blocks at height `y`, 32 blocks ahead.
    let entity_box = |y: f32| ([8.0, y, 40.0], [9.0, y + 1.8, 41.0]);
    let walk_based = |y: f32| visibility.aabb(entity_box(y).0, entity_box(y).1);
    let games = |y: f32| {
        let (lo, hi) = entity_box(y);
        visibility.frustum_box(lo, hi) && visibility.section_shown([8, y.floor() as i32, 40])
    };
    assert!(!walk_based(70.0) && games(70.0), "section 8 not reached now, compiled once: drawn");
    assert!(walk_based(81.0) && !games(81.0), "section 9 reached, never compiled: not drawn");
    assert!(visibility.section_shown([8, 400, 40]) && visibility.section_shown([8, -65, 40]), "outside the world");
    let behind = ([8.0, 70.0, -30.0], [9.0, 71.8, -29.0]);
    assert!(!visibility.frustum_box(behind.0, behind.1), "behind the camera: out of the field");
    // Without compiled sections (neutral frames, `RUSTCRAFT_ALLENTITIES`): the field alone.
    let neutral = FrameVisibility::new(view_of_a_scripted_walk(), VisibilityPaths::OLD);
    assert!(neutral.section_shown([8, 81, 40]));
}
