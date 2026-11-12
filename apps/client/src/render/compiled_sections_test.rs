//! Proofs of the compiled sections the entity and block entity tests read (`compiled_sections.rs`):
//! the game's rules over scripted walks. Every instant is given by the test from one base instant,
//! so no verdict depends on how fast the machine runs.

use std::time::{Duration, Instant};

use hashbrown::HashMap;
use rc_mesh::visibility::{VIS_ALL, VIS_NONE};

use super::*;
use crate::render::frame_visibility::FrameVisibility;
use crate::terrain_shader::{FadeRing, FadeWrite, fade_slot, fade_visibility};

const SECTIONS: usize = 6;
/// The default fade (`chunkSectionFadeInTime`): shown from 225 ms on.
const FADE: f64 = 0.75;

fn at(base: Instant, ms: u64) -> Instant {
    base + Duration::from_millis(ms)
}

fn no_unmeshed() -> HashMap<(i32, i32), Connectivity> {
    HashMap::new()
}

/// Compiled sections whose square of render distance 8 is centred on column (0, 0).
fn around_the_origin(base: Instant) -> CompiledSections {
    let mut compiled = CompiledSections::new(base);
    compiled.reposition(ColumnSquare { center: (0, 0), radius: 8 }, |_| {});
    compiled
}

/// A section of a meshed column is compiled from the first walk that reaches it and stays so when
/// later walks no longer reach it (behind a hill now); it shows from 0.3 of its fade on.
#[test]
fn reached_sections_of_meshed_columns_stay_compiled() {
    let base = Instant::now();
    let mut compiled = around_the_origin(base);
    let meshed = |k: (i32, i32)| k == (0, 0);
    compiled.mark_reached(&[(0, 2, 0), (0, 3, 0)], SECTIONS, meshed, &no_unmeshed(), at(base, 1000), |_| {});
    assert!(!compiled.shown((0, 0), 2, at(base, 1224), FADE), "shown before 0.3 of its fade");
    assert!(compiled.shown((0, 0), 2, at(base, 1225), FADE));
    assert!(!compiled.shown((0, 0), 4, at(base, 5000), FADE), "never reached: not compiled");
    compiled.mark_reached(&[(0, 4, 0)], SECTIONS, meshed, &no_unmeshed(), at(base, 2000), |_| {});
    assert!(compiled.shown((0, 0), 3, at(base, 1300), FADE), "no longer reached: still compiled, from its first time");
    assert!(!compiled.shown((0, 0), 4, at(base, 2100), FADE) && compiled.shown((0, 0), 4, at(base, 2225), FADE));
    assert!(compiled.shown((0, 0), 4, at(base, 2000), 0.0), "without a fade, shown at once");
    compiled.clear();
    assert!(!compiled.shown((0, 0), 3, at(base, 9000), FADE), "another world keeps nothing");
}

/// The game holds its sections in a square of render distance around the camera's column: outside
/// it nothing is shown, whatever was compiled; a section that left it starts its fade again when it
/// is compiled anew; a new render distance resets everything. Distance 4: the server still sends
/// column (5, 0) after the camera went back from column (1, 0) to column (0, 0).
#[test]
fn sections_are_held_in_the_square_of_render_distance_around_the_camera() {
    let base = Instant::now();
    let mut compiled = CompiledSections::new(base);
    let far = (5, 0);
    compiled.reposition(ColumnSquare { center: (1, 0), radius: 4 }, |_| {});
    compiled.mark(far, SECTIONS, [3].into_iter(), at(base, 0));
    compiled.mark((1, 0), SECTIONS, [3].into_iter(), at(base, 0));
    assert!(compiled.shown(far, 3, at(base, 1000), FADE), "in the square around column (1, 0)");
    compiled.reposition(ColumnSquare { center: (0, 0), radius: 4 }, |_| {});
    assert!(!compiled.shown(far, 3, at(base, 1000), FADE), "out of the square around column (0, 0)");
    compiled.mark(far, SECTIONS, [3].into_iter(), at(base, 1000));
    assert!(!compiled.shown(far, 3, at(base, 5000), FADE), "nothing is compiled out of the square");
    compiled.reposition(ColumnSquare { center: (1, 0), radius: 4 }, |_| {});
    assert!(!compiled.shown(far, 3, at(base, 5000), FADE), "back in the square: reset, not compiled yet");
    assert!(compiled.shown((1, 0), 3, at(base, 5000), FADE), "a column the square kept keeps its instant");
    compiled.mark(far, SECTIONS, [3].into_iter(), at(base, 6000));
    assert!(!compiled.shown(far, 3, at(base, 6224), FADE), "compiled again: its fade starts over");
    assert!(compiled.shown(far, 3, at(base, 6225), FADE));
    compiled.reposition(ColumnSquare { center: (1, 0), radius: 5 }, |_| {});
    assert!(!compiled.shown((1, 0), 3, at(base, 9000), FADE), "a new render distance resets every section");
    assert!(!CompiledSections::new(base).shown((0, 0), 3, at(base, 0), 0.0), "before the first frame, no square");
}

/// One frame of the terrain fade at `now`: its writes applied to `shader_copy` (the buffer the
/// shader reads), then the visibility of column `k` as the shader computes it from that copy.
fn terrain_fade(fade: &mut FadeRing, gpu: bool, shader_copy: &mut [u32], k: (i32, i32), now: Instant) -> f32 {
    match fade.frame(now, FADE) {
        FadeWrite::None => {}
        FadeWrite::All(ring) => shader_copy.copy_from_slice(ring),
        FadeWrite::Slots { slots, ring } => slots.iter().for_each(|&s| shader_copy[s as usize] = ring[s as usize]),
    }
    let slot = shader_copy[fade_slot(k)];
    if gpu { fade_visibility(fade.clock(now), slot, (FADE * 1e6) as f32) } else { f32::from_bits(slot) }
}

/// The terrain's fade follows the compiled sections, as the game's mesh and fade of a section follow
/// its square: a meshed column is drawn at 0 until a walk reaches it, which starts its fade; the
/// square forgets it when it leaves (drawn at 0 again), and the walk that reaches it once back starts
/// its fade over. The case of the review: distance 4, camera from column (1, 0) to (0, 0) and back,
/// column (5, 0) still loaded and meshed. A new render distance forgets every column.
#[test]
fn the_terrain_fade_starts_over_when_the_square_compiles_a_column_again() {
    for gpu in [false, true] {
        let base = Instant::now();
        let mut compiled = CompiledSections::new(base);
        let mut fade = FadeRing::new(gpu, base);
        let mut shader_copy = fade.ring().to_vec();
        let (far, near) = ((5, 0), (1, 0));
        let square = |x: i32, radius: i32| ColumnSquare { center: (x, 0), radius };
        let walk = |compiled: &mut CompiledSections, fade: &mut FadeRing, now: Instant| {
            let reached = [(1, 3, 0), (5, 3, 0)];
            compiled.mark_reached(&reached, SECTIONS, |_| true, &no_unmeshed(), now, |k| fade.birth(k, now));
        };
        let mut seen = |fade: &mut FadeRing, k: (i32, i32), ms: u64| terrain_fade(fade, gpu, &mut shader_copy, k, at(base, ms));
        compiled.reposition(square(1, 4), |k| fade.forget(k));
        assert_eq!(seen(&mut fade, far, 0), 0.0, "gpu={gpu}: meshed, no walk reached it yet: drawn at 0");
        walk(&mut compiled, &mut fade, at(base, 0));
        assert_eq!(seen(&mut fade, far, 375), 0.5, "gpu={gpu}: halfway through its fade");
        assert_eq!(seen(&mut fade, far, 1000), 1.0, "gpu={gpu}");
        compiled.reposition(square(0, 4), |k| fade.forget(k));
        assert_eq!(seen(&mut fade, far, 1000), 0.0, "gpu={gpu}: out of the square: forgotten");
        assert!(fade.visibility(far, FADE).is_none(), "gpu={gpu}: forgotten, not born");
        compiled.reposition(square(1, 4), |k| fade.forget(k));
        assert_eq!(seen(&mut fade, far, 5000), 0.0, "gpu={gpu}: back in the square, not compiled yet");
        assert_eq!(seen(&mut fade, near, 5000), 1.0, "gpu={gpu}: a column the square kept keeps its fade");
        walk(&mut compiled, &mut fade, at(base, 6000));
        assert_eq!(seen(&mut fade, far, 6000), 0.0, "gpu={gpu}: compiled again: its fade starts over");
        assert_eq!(seen(&mut fade, far, 6375), 0.5, "gpu={gpu}");
        assert_eq!(seen(&mut fade, near, 6375), 1.0, "gpu={gpu}: reached again, its fade does not restart");
        compiled.reposition(square(1, 5), |k| fade.forget(k));
        assert_eq!(seen(&mut fade, near, 9000), 0.0, "gpu={gpu}: a new render distance forgets every column");
        assert!(!compiled.mark((9, 0), SECTIONS, [3].into_iter(), at(base, 9000)), "out of the square: not compiled");
        assert!(!compiled.mark(near, SECTIONS, std::iter::empty(), at(base, 9000)), "no section reached");
    }
}

/// Before its mesh, only the empty sections a walk reaches of a loaded column are compiled (the
/// game takes them as compiled empty); its other sections, reached or not, wait for the mesh, and a
/// column neither meshed nor loaded gets nothing. The mesh's first walk compiles the rest it reaches.
#[test]
fn only_the_empty_sections_of_a_column_waiting_for_its_mesh_are_compiled() {
    let base = Instant::now();
    let mut compiled = around_the_origin(base);
    let provisional: Connectivity = [VIS_NONE, VIS_NONE, VIS_ALL, VIS_ALL, VIS_ALL, VIS_ALL].into();
    let unmeshed: HashMap<_, _> = [((1, 0), provisional)].into_iter().collect();
    let reached = [(1, 1, 0), (1, 2, 0), (1, 3, 0), (5, 3, 5)];
    compiled.mark_reached(&reached, SECTIONS, |_| false, &unmeshed, at(base, 0), |_| {});
    let later = at(base, 1000);
    assert!(!compiled.shown((1, 0), 1, later, FADE), "a section holding blocks waits for the mesh");
    assert!(compiled.shown((1, 0), 2, later, FADE) && compiled.shown((1, 0), 3, later, FADE));
    assert!(!compiled.shown((1, 0), 4, later, FADE), "an empty section the walk did not reach");
    assert!(!compiled.shown((5, 5), 3, later, FADE), "a column not loaded");
    compiled.mark_reached(&reached[..3], SECTIONS, |k| k == (1, 0), &no_unmeshed(), at(base, 900), |_| {});
    assert!(!compiled.shown((1, 0), 1, at(base, 1000), FADE) && compiled.shown((1, 0), 1, at(base, 1125), FADE));
    assert!(compiled.shown((1, 0), 2, at(base, 300), FADE), "an empty section keeps its first instant");
}

/// A camera at (8.5, 70, 8.5) looking south (+z) at render distance `radius`; the walk reached its
/// section and section 9 of column (0, 2), 32 blocks ahead; section 8 of columns (0, 2) and (0, 6)
/// was compiled long ago.
fn scripted_frame(radius: i32) -> FrameVisibility {
    let reached = [(0, 8, 0), (0, 9, 2)];
    FrameVisibility::scripted_for_tests([8.5, 70.0, 8.5], &reached, &[((0, 2), 8), ((0, 6), 8)], radius).0
}

/// The entities' test is the game's: in the field and in a compiled section that has appeared,
/// whether the walk reaches that section now or not. An entity behind a hill in a section seen
/// once is drawn (the walk-based test of before hid it); one in a section the walk reaches but
/// never compiled is not (the test of before drew it); above or below the world, the field alone;
/// beyond the render distance, nothing.
#[test]
fn entities_need_the_field_and_a_compiled_section_not_the_walk() {
    let visibility = scripted_frame(8);
    // An entity of 1 × 1.8 × 1 blocks at height `y`, at block z `z` ahead.
    let entity_box = |y: f32, z: f32| ([8.0, y, z], [9.0, y + 1.8, z + 1.0]);
    let walk_based = |y: f32| visibility.aabb(entity_box(y, 40.0).0, entity_box(y, 40.0).1);
    let games = |visibility: &FrameVisibility, y: f32, z: f32| {
        let (lo, hi) = entity_box(y, z);
        visibility.entity_shown(false, lo, hi, [8, y.floor() as i32, z.floor() as i32])
    };
    assert!(!walk_based(70.0) && games(&visibility, 70.0, 40.0), "section 8 not reached now, compiled once: drawn");
    assert!(walk_based(81.0) && !games(&visibility, 81.0, 40.0), "section 9 reached, never compiled: not drawn");
    assert!(visibility.section_shown([8, 400, 40]) && visibility.section_shown([8, -65, 40]), "outside the world");
    let behind = ([8.0, 70.0, -30.0], [9.0, 71.8, -29.0]);
    assert!(!visibility.frustum_box(behind.0, behind.1), "behind the camera: out of the field");
    assert!(visibility.entity_shown(true, behind.0, behind.1, [8, 70, 40]), "drawn outside the field");
    // Column (0, 6), 6 columns ahead: drawn at render distance 8, not at 4 (out of the square).
    assert!(games(&visibility, 70.0, 104.0));
    assert!(!games(&scripted_frame(4), 70.0, 104.0), "beyond the render distance: not drawn");
    // Without compiled sections (neutral frames, `RUSTCRAFT_ALLENTITIES`): the field alone.
    let (planes_only, _) = FrameVisibility::scripted_for_tests([8.5, 70.0, 8.5], &[], &[], 8);
    let neutral = planes_only.with_compiled_sections(None);
    assert!(neutral.section_shown([8, 81, 40]));
}
