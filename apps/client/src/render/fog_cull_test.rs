//! Fog cull: no section with a point short of the fog's end is hidden, whatever the eye of the
//! 8-block cell, model overflow included (the shader's own fog formula is the judge); the sky dome
//! is plain fog colour under the rim slope (the shader's fan, rebuilt here); a walk cut at its
//! sight limit only skips sections in plain fog for as long as it serves; the frame rules (sky,
//! clouds, fog-piercing draws, improved transparency, switches), the steps of the reach and the
//! list the draw lists read. Each faulty reach, limit or bound is caught. Micro-bench (ignored):
//! `fog_cull_bench`.
//!
//! What these tests cannot see: that every draw of the world pass is either turned into the fog
//! colour by plain fog or reported as fog-piercing. `every_world_shader_and_blend_is_classified`
//! keeps that inventory (docs/specs/visibility.md) from going stale; the reports themselves are
//! tested where they are made (`entity_render::tests_additive_draws`, `crack`).

use rc_env::Fog;
use rc_mesh::visibility::{Graph, VIS_ALL, VIS_NONE, beyond_sight};

use super::*;
use crate::sky::{CloudFrame, SkyFrame};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn unit(&mut self) -> f64 {
        (self.next() % 1_000_000) as f64 / 1_000_000.0
    }
    fn below(&mut self, n: u64) -> i32 {
        (self.next() % n) as i32
    }
}

const MIN_Y: i32 = -64;
const SECTIONS: i32 = 24;
/// How far a model may stick out of its section on each axis (the back-face test's margin).
const MODEL_OVERFLOW: f64 = 2.0;
/// Diagonal of the 8-block cell of the eye.
const CELL_DIAGONAL: f64 = 13.856_406_460_551_018;

const DEFAULT_PATHS: FogCullPaths = FogCullPaths {
    over_plain_background: true,
    below_sky_rim: false,
    against_sky: false,
    cuts_walk: false,
    under_improved_transparency: false,
};
const BELOW_SKY_RIM: FogCullPaths = FogCullPaths { below_sky_rim: true, ..DEFAULT_PATHS };
const AGAINST_SKY: FogCullPaths = FogCullPaths { against_sky: true, ..DEFAULT_PATHS };

/// A frame whose only draws that matter to the cull are its terrain and its sky.
const TERRAIN_ALONE: FrameDraws = FrameDraws { farthest_fog_piercing_draw: 0.0, improved_transparency: false };

/// A frame with a fog-piercing draw reaching `blocks` from the eye.
fn piercing_draw_at(blocks: f64) -> FrameDraws {
    FrameDraws { farthest_fog_piercing_draw: blocks, ..TERRAIN_ALONE }
}

fn bodies(clouds: Option<CloudFrame>) -> SkyFrame {
    SkyFrame {
        end: None,
        sun_deg: 0.0,
        moon_deg: 180.0,
        star_deg: 0.0,
        star_brightness: 0.0,
        rain: 0.0,
        moon_phase: 0,
        sunrise: [0.0; 4],
        clouds,
    }
}

fn clouds_fading_at(cloud_end: f32) -> CloudFrame {
    CloudFrame { color: [1.0; 4], height: 192.0, game_time: 0, pt: 0.0, range_chunks: 128, cloud_end, flat: false }
}

/// Fog of the environment `(start, end)` at a render distance in blocks, as `rc_env::fog` lays it out.
fn fog(environment: (f32, f32), render_distance: f32) -> Fog {
    let span = (render_distance / 10.0).clamp(4.0, 64.0);
    Fog {
        color: [0.5, 0.6, 0.7, 1.0],
        env_start: environment.0,
        env_end: environment.1,
        render_start: render_distance - span,
        render_end: render_distance,
        sky_end: render_distance.min(512.0),
        cloud_end: 2048.0,
    }
}

/// A frame where only the fog-coloured background shows behind the terrain (the Nether).
fn frame_without_sky(fog: Fog) -> FrameEnv {
    FrameEnv { fog, sky: [0.0; 4], dome: false, disc: false, bodies: bodies(None), far: 2048.0, fluid: false }
}

/// A frame of the Overworld above the horizon: sky dome and clouds drawn.
fn frame_under_sky(fog: Fog) -> FrameEnv {
    FrameEnv { dome: true, bodies: bodies(Some(clouds_fading_at(fog.cloud_end))), ..frame_without_sky(fog) }
}

/// The fog step of the world shaders (`linear_fog`).
fn linear_fog(distance: f32, start: f32, end: f32) -> f32 {
    if distance <= start {
        0.0
    } else if distance >= end {
        1.0
    } else {
        (distance - start) / (end - start)
    }
}

/// Fog the world shaders give the point `p` seen from `eye` (1: plain fog colour).
fn shader_fog(fog: &Fog, eye: [f64; 3], p: [f64; 3]) -> f32 {
    let d: [f32; 3] = std::array::from_fn(|axis| (p[axis] - eye[axis]) as f32);
    let spherical = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let cylindrical = (d[0] * d[0] + d[2] * d[2]).sqrt().max(d[1].abs());
    linear_fog(spherical, fog.env_start, fog.env_end).max(linear_fog(cylindrical, fog.render_start, fog.render_end))
}

/// A fog of every environment: Nether, water, lava, blindness, darkness, a start after the end,
/// a random end, and the open air where only the render distance hides.
fn random_fog(rng: &mut Rng) -> Fog {
    let render_distance = [32.0, 64.0, 128.0, 192.0, 512.0][rng.below(5) as usize];
    let environment = match rng.below(8) {
        0 => (10.0, 96.0),
        1 => (-8.0, 24.0 + rng.unit() as f32 * 72.0),
        2 => (0.25, 1.0),
        3 => (1.25, 5.0),
        4 => (11.25, 15.0),
        5 => (40.0 + rng.unit() as f32 * 20.0, 30.0),
        6 => (0.0, rng.unit() as f32 * 300.0),
        _ => (0.0, 1024.0),
    };
    fog(environment, render_distance)
}

fn random_eye(rng: &mut Rng) -> [f64; 3] {
    [rng.unit() * 400.0 - 200.0, rng.unit() * 480.0 - 80.0, rng.unit() * 400.0 - 200.0]
}

/// Eyes of the 8-block cell `cell`: two random ones and the eight corners.
fn eyes_of_the_cell(rng: &mut Rng, cell: [i64; 3]) -> Vec<[f64; 3]> {
    let low = cell.map(|c| (c * 8) as f64);
    let mut eyes: Vec<[f64; 3]> = (0..2).map(|_| std::array::from_fn(|axis| low[axis] + rng.unit() * 8.0)).collect();
    for corner in 0..8 {
        eyes.push(std::array::from_fn(|axis| low[axis] + if corner >> axis & 1 == 1 { 8.0 - 1e-9 } else { 0.0 }));
    }
    eyes
}

/// Eyes a walk made from `walk_eye` may serve: up to the diagonal of a cell away, in random
/// directions and along the axes and diagonals.
fn eyes_a_walk_serves(rng: &mut Rng, walk_eye: [f64; 3]) -> Vec<[f64; 3]> {
    let mut eyes = vec![walk_eye];
    for _ in 0..6 {
        let towards: [f64; 3] = std::array::from_fn(|_| rng.unit() * 2.0 - 1.0);
        let length = towards.iter().map(|v| v * v).sum::<f64>().sqrt().max(1e-9);
        eyes.push(std::array::from_fn(|axis| walk_eye[axis] + towards[axis] / length * CELL_DIAGONAL));
    }
    for corner in 0..8 {
        let sign = |axis: usize| if corner >> axis & 1 == 1 { 8.0 } else { -8.0 };
        eyes.push(std::array::from_fn(|axis| walk_eye[axis] + sign(axis) * (1.0 - 1e-9)));
    }
    eyes
}

/// Points of section `pos` grown by the model overflow: its eight corners and its point nearest
/// to `seen_from`.
fn points_of_section(pos: ReachedSection, seen_from: [f64; 3]) -> Vec<[f64; 3]> {
    let low = [pos.0 * 16, MIN_Y + pos.1 * 16, pos.2 * 16].map(|v| f64::from(v) - MODEL_OVERFLOW);
    let high = low.map(|v| v + 16.0 + 2.0 * MODEL_OVERFLOW);
    let mut points = vec![std::array::from_fn(|axis| seen_from[axis].clamp(low[axis], high[axis]))];
    for corner in 0..8 {
        points.push(std::array::from_fn(|axis| if corner >> axis & 1 == 1 { high[axis] } else { low[axis] }));
    }
    points
}

/// From one of `eyes`, a point of section `pos` is not plain fog colour, or shows above the
/// steepest slope of `reach` (the frame's real reach).
fn shows(fog: &Fog, reach: &FogReach, eyes: &[[f64; 3]], pos: ReachedSection) -> bool {
    let above_slope = |eye: [f64; 3], p: [f64; 3]| {
        let rise = p[1] - eye[1];
        let run = ((p[0] - eye[0]).powi(2) + (p[2] - eye[2]).powi(2)).sqrt();
        reach.steepest_slope.is_finite() && rise > 0.0 && rise > reach.steepest_slope * run
    };
    eyes.iter()
        .any(|&eye| points_of_section(pos, eye).iter().any(|&p| shader_fog(fog, eye, p) < 1.0 || above_slope(eye, p)))
}

/// Sections left out over random frames, and those among them that should not be.
#[derive(Default)]
struct LeftOut {
    /// Sections left out, without and under a drawn sky.
    sections: [usize; 2],
    /// Left out although a point of theirs shows ([`shows`]).
    wrongly: usize,
}

/// A random section within `columns` columns of the column of `eye`.
fn section_around(rng: &mut Rng, eye: [f64; 3], columns: i32) -> ReachedSection {
    let camera_column = [(eye[0] / 16.0).floor() as i32, (eye[2] / 16.0).floor() as i32];
    let mut around = || rng.below(2 * columns as u64 + 1) - columns;
    let (x, z) = (camera_column[0] + around(), camera_column[1] + around());
    (x, rng.below(SECTIONS as u64), z)
}

/// Runs random frames (fog, sky or not, eye) and random sections around the edge of the fog
/// through the trim of the draw lists; `faulty` alters the reach of each frame (identity: the
/// real one).
fn hidden_over_random_frames(faulty: impl Fn(FogReach, &Fog) -> FogReach) -> LeftOut {
    let mut rng = Rng(0x243f_6a88_85a3_08d3);
    let mut hidden = LeftOut::default();
    for round in 0..300 {
        let fog = random_fog(&mut rng);
        let under_sky = round % 3 == 0;
        let frame = if under_sky { frame_under_sky(fog) } else { frame_without_sky(fog) };
        let real_reach = FogReach::of_frame(&frame, TERRAIN_ALONE, BELOW_SKY_RIM).expect("a reach");
        let reach = faulty(real_reach, &fog);
        let eye = random_eye(&mut rng);
        let cell = cell_of(eye);
        let eyes = eyes_of_the_cell(&mut rng, cell);
        let columns_around = (reach.sphere.min(reach.cylinder) / 16.0) as i32 + 4;
        for _ in 0..2000 {
            let pos = section_around(&mut rng, eye, columns_around);
            if reach.hides(cell, MIN_Y, pos) {
                hidden.sections[usize::from(under_sky)] += 1;
                hidden.wrongly += usize::from(shows(&fog, &real_reach, &eyes, pos));
            }
        }
    }
    hidden
}

#[test]
fn hidden_sections_are_plain_fog_from_every_eye_of_the_cell() {
    let hidden = hidden_over_random_frames(|reach, _| reach);
    assert_eq!(hidden.wrongly, 0, "sections hidden with a point short of the fog's end or above the slope");
    let [without_sky, under_sky] = hidden.sections;
    assert!(without_sky > 100_000 && under_sky > 5_000, "too few hidden sections: {:?}", hidden.sections);
}

#[test]
fn each_faulty_reach_is_caught() {
    type Fault = fn(FogReach, &Fog) -> FogReach;
    let faults: [(&str, Fault); 5] = [
        ("no room for the model overflow", |reach, _| FogReach {
            sphere: reach.sphere - SAFETY_BLOCKS,
            cylinder: reach.cylinder - SAFETY_BLOCKS,
            ..reach
        }),
        ("fog taken as ending at its end even before its start", |reach, fog| FogReach {
            sphere: ladder_step(fog.env_end),
            ..reach
        }),
        ("environment fog end rounded down a step", |reach, _| FogReach { sphere: reach.sphere / 2.0, ..reach }),
        ("render distance one section short", |reach, _| FogReach { cylinder: reach.cylinder - 16.0, ..reach }),
        ("slope twice as steep", |reach, _| FogReach { steepest_slope: reach.steepest_slope * 2.0, ..reach }),
    ];
    for (name, fault) in faults {
        assert!(hidden_over_random_frames(fault).wrongly > 0, "fault not caught: {name}");
    }
}

/// Fog of the sky dome towards slope `slope` and azimuth `azimuth` (radians), as the sky shader
/// computes it: a fan at height 16 whose rim vertices stand every 45° at 512 blocks, fog distances
/// taken at the vertices and interpolated. `None` under the rim, where no dome is drawn.
fn dome_fog(sky_end: f32, slope: f64, azimuth: f64) -> Option<f32> {
    let run = DOME_HEIGHT / slope;
    let sector = ((azimuth.to_degrees() + 180.0) / 45.0).floor();
    let sector_middle = (-180.0 + 45.0 * sector + 22.5).to_radians();
    // Distance from the centre to the rim (a chord between two vertices) along this azimuth.
    let rim = DOME_RADIUS * 22.5f64.to_radians().cos() / (azimuth - sector_middle).cos();
    let share = run / rim;
    if share > 1.0 {
        return None;
    }
    // Fog distances (spherical, cylindrical) of the vertices: centre (16, 16), rim (512.25, 512).
    let rim_spherical = DOME_RADIUS.hypot(DOME_HEIGHT);
    let spherical = (DOME_HEIGHT + (rim_spherical - DOME_HEIGHT) * share) as f32;
    let cylindrical = (DOME_HEIGHT + (DOME_RADIUS - DOME_HEIGHT) * share) as f32;
    Some(linear_fog(spherical, 0.0, sky_end).max(linear_fog(cylindrical, sky_end, sky_end)))
}

#[test]
fn sky_dome_is_plain_fog_under_the_rim_slope() {
    let azimuths = || (0..360).map(|degrees| (f64::from(degrees) - 179.5).to_radians());
    for sky_end in [4.0, 16.0, 20.0, 24.0, 64.0, 96.0, 100.0, 192.0, 256.0, 300.0, 512.0, 600.0] {
        let rim_slope = sky_rim_slope(sky_end);
        let steepest = if rim_slope.is_finite() { rim_slope } else { 1000.0 };
        for step in 1..=200 {
            let slope = steepest * f64::from(step) / 200.0;
            for azimuth in azimuths() {
                let fog = dome_fog(sky_end, slope, azimuth);
                assert!(fog.is_none_or(|v| v == 1.0), "sky end {sky_end}, slope {slope}, azimuth {azimuth}: {fog:?}");
            }
        }
        // The bound is not empty words: well above the slope, the dome shows its own colour.
        if rim_slope.is_finite() && sky_end < 500.0 {
            let clear = azimuths().filter(|&a| dome_fog(sky_end, rim_slope * 8.0, a).is_some_and(|v| v < 1.0)).count();
            assert!(clear > 0, "sky end {sky_end}: dome plain fog even at 8 times the slope");
        }
    }
}

/// Sections a walk cut at `sight_limit_of` the reach skips, over random frames, and those among
/// them that show from an eye the walk may serve.
fn skipped_by_cut_walks(sight_limit_of: impl Fn(&FogReach) -> f64) -> LeftOut {
    let mut rng = Rng(0x1319_8a2e_0370_7344);
    let mut skipped = LeftOut::default();
    for _ in 0..300 {
        let fog = random_fog(&mut rng);
        let reach = FogReach::of_frame(&frame_without_sky(fog), TERRAIN_ALONE, DEFAULT_PATHS).expect("a reach");
        let sight_limit = sight_limit_of(&reach);
        let walk_eye = random_eye(&mut rng);
        let eyes = eyes_a_walk_serves(&mut rng, walk_eye);
        let columns_around = (sight_limit.min(600.0) / 16.0) as i32 + 4;
        for _ in 0..2000 {
            let pos = section_around(&mut rng, walk_eye, columns_around);
            if beyond_sight(walk_eye, MIN_Y, pos, sight_limit) {
                skipped.sections[0] += 1;
                skipped.wrongly += usize::from(shows(&fog, &reach, &eyes, pos));
            }
        }
    }
    skipped
}

#[test]
fn a_cut_walk_only_skips_sections_in_plain_fog() {
    let skipped = skipped_by_cut_walks(FogReach::walk_sight_limit);
    assert_eq!(skipped.wrongly, 0, "sections skipped by the walk with a point short of the fog's end");
    assert!(skipped.sections[0] > 100_000, "too few skipped sections: {}", skipped.sections[0]);
    // A slope spares far sections above it: the walk is not cut then.
    let under_sky = FogReach::of_frame(&frame_under_sky(fog((10.0, 96.0), 512.0)), TERRAIN_ALONE, BELOW_SKY_RIM);
    assert_eq!(under_sky.expect("a reach").walk_sight_limit(), f64::INFINITY);
    // The cut is its own switch.
    let reach = FogReach::of_frame(&frame_without_sky(fog((10.0, 96.0), 512.0)), TERRAIN_ALONE, DEFAULT_PATHS);
    assert_eq!(walk_sight_limit(reach, DEFAULT_PATHS), f64::INFINITY);
    let cut = walk_sight_limit(reach, FogCullPaths { cuts_walk: true, ..DEFAULT_PATHS });
    assert_eq!(cut, 96.0 + SAFETY_BLOCKS + WALK_EYE_TRAVEL);
    assert_eq!(walk_sight_limit(None, FogCullPaths { cuts_walk: true, ..DEFAULT_PATHS }), f64::INFINITY);
}

#[test]
fn each_faulty_sight_limit_is_caught() {
    type FaultyLimit = fn(&FogReach) -> f64;
    let faults: [(&str, FaultyLimit); 2] = [
        ("eye taken as still", |reach| reach.walk_sight_limit() - WALK_EYE_TRAVEL),
        ("render distance taken as a sphere", |reach| {
            reach.sphere.min(reach.cylinder) + SAFETY_BLOCKS + WALK_EYE_TRAVEL
        }),
    ];
    for (name, fault) in faults {
        assert!(skipped_by_cut_walks(fault).wrongly > 0, "fault not caught: {name}");
    }
}

#[test]
fn only_a_plain_background_lets_sections_go_by_default() {
    let nether = fog((10.0, 96.0), 512.0);
    let reach = |frame: &FrameEnv, paths| FogReach::of_frame(frame, TERRAIN_ALONE, paths);
    let plain = frame_without_sky(nether);
    let expected = FogReach { sphere: 96.0, cylinder: 512.0, steepest_slope: f64::INFINITY };
    assert_eq!(reach(&plain, DEFAULT_PATHS), Some(expected));
    // Switch off, fog colour that does not cover: nothing goes.
    assert_eq!(reach(&plain, FogCullPaths { over_plain_background: false, ..DEFAULT_PATHS }), None);
    let thin = FrameEnv { fog: Fog { color: [0.5, 0.6, 0.7, 0.9], ..nether }, ..plain };
    assert_eq!(reach(&thin, DEFAULT_PATHS), None);
    // Clouds: still visible past the fog's end (their own fog ends farther), or gone before it.
    let clouds = |cloud_end| FrameEnv { bodies: bodies(Some(clouds_fading_at(cloud_end))), ..plain };
    assert_eq!(reach(&clouds(2048.0), DEFAULT_PATHS), None);
    assert_eq!(reach(&clouds(f32::NAN), DEFAULT_PATHS), None);
    assert_eq!(reach(&clouds(96.0), DEFAULT_PATHS), Some(expected));
    // A drawn sky (dome or End sky) keeps every section by default.
    let dome = FrameEnv { dome: true, ..plain };
    let end_sky = FrameEnv { bodies: SkyFrame { end: Some((0.0, 0.0, 0.0)), ..bodies(None) }, ..plain };
    assert_eq!(reach(&dome, DEFAULT_PATHS), None);
    assert_eq!(reach(&end_sky, DEFAULT_PATHS), None);
}

/// Fogs and paths under which sections go when the terrain is alone: the Nether by default, water
/// under a sky dome with each sky switch.
fn frames_that_cull() -> [(FrameEnv, FogCullPaths); 3] {
    let nether = frame_without_sky(fog((10.0, 96.0), 512.0));
    let water = Fog { sky_end: 96.0, ..fog((-8.0, 96.0), 512.0) };
    let under_sky = FrameEnv { dome: true, ..frame_without_sky(water) };
    [(nether, DEFAULT_PATHS), (under_sky, BELOW_SKY_RIM), (under_sky, AGAINST_SKY)]
}

#[test]
fn a_fog_piercing_draw_as_far_as_a_hidden_section_keeps_every_section() {
    for (frame, paths) in frames_that_cull() {
        let reach = |draws| FogReach::of_frame(&frame, draws, paths);
        let alone = reach(TERRAIN_ALONE).expect("a reach");
        assert_eq!((alone.sphere, alone.cylinder), (96.0, 512.0));
        // Nearer than any hidden section (a crack under the crosshair, a wither fought at close
        // range): it cannot stand behind one, the cull goes on.
        for near in [4.5, 60.0, 95.999] {
            assert_eq!(reach(piercing_draw_at(near)), Some(alone), "piercing draw at {near}");
        }
        // As far as a hidden section can stand, or not measured (border wall, debug faces,
        // lightning), or not a number: every section is drawn, under the sky switches too.
        for far in [96.0, 113.0, f64::INFINITY, f64::NAN] {
            assert_eq!(reach(piercing_draw_at(far)), None, "piercing draw at {far}");
        }
    }
    // The bound follows the nearer of the two fogs: lava hides from 16 blocks on.
    let lava = frame_without_sky(fog((0.25, 1.0), 512.0));
    assert!(FogReach::of_frame(&lava, piercing_draw_at(15.9), DEFAULT_PATHS).is_some());
    assert_eq!(FogReach::of_frame(&lava, piercing_draw_at(16.0), DEFAULT_PATHS), None);
}

/// Over random frames, the least room between the bound `bound_of` gives a reach and the nearest
/// point of a section that reach hides, model overflow included, from the eyes of the cell:
/// negative when a hidden section stands nearer than the bound says.
fn least_room_before_a_hidden_section(bound_of: impl Fn(&FogReach) -> f64) -> f64 {
    let mut rng = Rng(0x4528_21e6_38d0_1377);
    let mut least_room = f64::INFINITY;
    for _ in 0..300 {
        let fog = random_fog(&mut rng);
        let reach = FogReach::of_frame(&frame_without_sky(fog), TERRAIN_ALONE, DEFAULT_PATHS).expect("a reach");
        let eye = random_eye(&mut rng);
        let cell = cell_of(eye);
        let eyes = eyes_of_the_cell(&mut rng, cell);
        let columns_around = (reach.sphere.min(reach.cylinder) / 16.0) as i32 + 4;
        for _ in 0..2000 {
            let pos = section_around(&mut rng, eye, columns_around);
            if !reach.hides(cell, MIN_Y, pos) {
                continue;
            }
            for &eye in &eyes {
                let nearest = points_of_section(pos, eye)[0];
                let distance = (0..3).map(|axis| (nearest[axis] - eye[axis]).powi(2)).sum::<f64>().sqrt();
                least_room = least_room.min(distance - bound_of(&reach));
            }
        }
    }
    least_room
}

#[test]
fn no_hidden_section_stands_nearer_than_the_fog_piercing_bound() {
    let room = least_room_before_a_hidden_section(FogReach::nearest_hidden_point);
    assert!(room >= 0.0 && room.is_finite(), "room before the nearest hidden section: {room}");
    // The bound may not count the safety blocks: the model overflow takes most of them back.
    let room_past_safety = least_room_before_a_hidden_section(|reach| reach.nearest_hidden_point() + SAFETY_BLOCKS);
    assert!(room_past_safety < 0.0, "fault not caught: bound taken past the safety blocks ({room_past_safety})");
}

#[test]
fn improved_transparency_keeps_every_section_until_its_switch() {
    let composed = FrameDraws { improved_transparency: true, ..TERRAIN_ALONE };
    for (frame, paths) in frames_that_cull() {
        let alone = FogReach::of_frame(&frame, TERRAIN_ALONE, paths);
        assert!(alone.is_some());
        assert_eq!(FogReach::of_frame(&frame, composed, paths), None);
        let switched_on = FogCullPaths { under_improved_transparency: true, ..paths };
        assert_eq!(FogReach::of_frame(&frame, composed, switched_on), alone);
        // The switch adds no path of its own, and a fog-piercing draw still keeps everything.
        assert_eq!(FogReach::of_frame(&frame, TERRAIN_ALONE, switched_on), alone);
        let piercing = FrameDraws { farthest_fog_piercing_draw: f64::INFINITY, ..composed };
        assert_eq!(FogReach::of_frame(&frame, piercing, switched_on), None);
    }
}

#[test]
fn the_sky_switches_let_sections_go_under_a_drawn_sky() {
    let water = Fog { sky_end: 96.0, ..fog((-8.0, 96.0), 512.0) };
    let dome = FrameEnv { dome: true, ..frame_without_sky(water) };
    let reach = |frame: &FrameEnv, paths| FogReach::of_frame(frame, TERRAIN_ALONE, paths);
    // Below the rim: the slope under which the dome is plain fog (16 × 496 / (512 × 80)).
    let below_rim = reach(&dome, BELOW_SKY_RIM).expect("a reach");
    assert_eq!((below_rim.sphere, below_rim.cylinder), (96.0, 512.0));
    assert!((below_rim.steepest_slope - 0.193_75).abs() < 1e-12, "{}", below_rim.steepest_slope);
    // The dark disc and the End sky are fog-coloured nowhere: that path keeps everything.
    assert_eq!(reach(&FrameEnv { disc: true, ..dome }, BELOW_SKY_RIM), None);
    let end_sky =
        FrameEnv { bodies: SkyFrame { end: Some((0.0, 0.0, 0.0)), ..bodies(None) }, ..frame_without_sky(water) };
    assert_eq!(reach(&end_sky, BELOW_SKY_RIM), None);
    // Against the sky: every hidden section goes, under any sky.
    for frame in [dome, FrameEnv { disc: true, ..dome }, end_sky, frame_under_sky(water)] {
        let expected = FogReach { sphere: 96.0, cylinder: 512.0, steepest_slope: f64::INFINITY };
        assert_eq!(reach(&frame, AGAINST_SKY), Some(expected));
    }
}

#[test]
fn the_reach_moves_in_steps_and_never_under_the_fog_end() {
    let ladder = [16.0, 24.0, 32.0, 48.0, 64.0, 96.0, 128.0, 192.0, 256.0, 384.0, 512.0, 768.0, 1024.0, 1536.0, 2048.0];
    let mut rng = Rng(0xa409_3822_299f_31d0);
    for _ in 0..20_000 {
        let blocks = (rng.unit() * 2300.0 - 100.0) as f32;
        let (step, sections) = (ladder_step(blocks), whole_sections(blocks));
        assert!(step >= f64::from(blocks) && sections >= f64::from(blocks), "{blocks}: {step}, {sections}");
        if blocks > 2048.0 {
            assert!(step.is_infinite() && sections.is_infinite(), "{blocks}");
            continue;
        }
        assert!(ladder.contains(&step) && step < f64::from(blocks.max(16.0)) * 1.5 + 1e-9, "{blocks}: {step}");
        assert!(sections % 16.0 == 0.0 && sections < f64::from(blocks.max(0.0)) + 16.0 + 1e-9, "{blocks}: {sections}");
    }
    assert_eq!((ladder_step(96.0), ladder_step(96.5), ladder_step(-8.0)), (96.0, 128.0, 16.0));
    assert_eq!((whole_sections(192.0), whole_sections(1.0), whole_sections(-3.0)), (192.0, 16.0, 16.0));
    for odd in [f32::NAN, f32::INFINITY, 1.0e9] {
        assert!(ladder_step(odd).is_infinite() && whole_sections(odd).is_infinite(), "{odd}");
    }
    // A blindness wearing off sweeps the fog end from 5 to 512 blocks: eleven steps, not one per frame.
    let mut steps: Vec<f64> = (0..=1000).map(|i| ladder_step((5.0 + 507.0 * i as f32 / 1000.0).min(512.0))).collect();
    steps.dedup();
    assert_eq!(steps.len(), 11, "{steps:?}");
}

#[test]
fn the_drawn_list_follows_the_walk_the_fog_and_the_cell_of_the_eye() {
    let mut rng = Rng(0x0822_0f3a_52c8_91e7);
    let eye = [3.5, 70.2, -5.25];
    // Sorted by column then height, like the walk's result.
    let mut reached: Vec<ReachedSection> = (0..4000).map(|_| section_around(&mut rng, eye, 12)).collect();
    reached.sort_unstable_by_key(|&(x, sy, z)| (x, z, sy));
    reached.dedup();
    let nether = FogReach::of_frame(&frame_without_sky(fog((10.0, 96.0), 512.0)), TERRAIN_ALONE, DEFAULT_PATHS);
    let lava = FogReach::of_frame(&frame_without_sky(fog((0.25, 1.0), 512.0)), TERRAIN_ALONE, DEFAULT_PATHS);
    let water = Fog { sky_end: 96.0, ..fog((-8.0, 96.0), 512.0) };
    let under_sky_rim =
        FogReach::of_frame(&FrameEnv { dome: true, ..frame_without_sky(water) }, TERRAIN_ALONE, BELOW_SKY_RIM);
    let in_sight = |reach: Option<FogReach>, eye: [f64; 3]| -> Vec<ReachedSection> {
        let hidden = |pos| reach.is_some_and(|reach| reach.hides(cell_of(eye), MIN_Y, pos));
        reached.iter().copied().filter(|&pos| !hidden(pos)).collect()
    };
    let mut drawn = DrawnSections::default();
    let mut generations = vec![drawn.generation()];
    let mut list_after = |drawn: &mut DrawnSections, walk, eye, reach| {
        drawn.refresh(&reached, walk, eye, MIN_Y, reach);
        generations.push(drawn.generation());
        drawn.sections(&reached).to_vec()
    };
    // No fog reach: the walk's own list. A fog: the sections it leaves in sight, in the same order.
    assert_eq!(list_after(&mut drawn, 1, eye, None), reached);
    let under_nether_fog = list_after(&mut drawn, 1, eye, nether);
    assert_eq!(under_nether_fog, in_sight(nether, eye));
    assert!(under_nether_fog.len() < reached.len() && !under_nether_fog.is_empty());
    // Same walk, fog and cell (the eye moved within its cell): the list is kept as it is.
    assert_eq!(list_after(&mut drawn, 1, [0.5, 71.9, -7.9], nether), under_nether_fog);
    // Another cell, another fog, another walk result: remade each time.
    let next_cell = [eye[0] + 8.0, eye[1], eye[2]];
    assert_eq!(list_after(&mut drawn, 1, next_cell, nether), in_sight(nether, next_cell));
    let under_lava_fog = list_after(&mut drawn, 1, next_cell, lava);
    assert_eq!(under_lava_fog, in_sight(lava, next_cell));
    assert!(under_lava_fog.len() < under_nether_fog.len());
    assert_eq!(list_after(&mut drawn, 2, next_cell, lava), under_lava_fog);
    // Under a slope, far columns keep the sections that stand above it.
    let under_the_rim = list_after(&mut drawn, 2, next_cell, under_sky_rim);
    assert_eq!(under_the_rim, in_sight(under_sky_rim, next_cell));
    let any_slope = under_sky_rim.map(|reach| FogReach { steepest_slope: f64::INFINITY, ..reach });
    assert!(under_the_rim.len() > in_sight(any_slope, next_cell).len() + 100, "the slope spared too few sections");
    // The fog lifts: the walk's own list again.
    assert_eq!(list_after(&mut drawn, 2, next_cell, None), reached);
    let changed: Vec<bool> = generations.windows(2).map(|pair| pair[1] != pair[0]).collect();
    assert_eq!(changed, [true, true, false, true, true, true, true, true]);
}

/// What plain fog does to what a shader file draws in the world pass.
enum InPlainFog {
    /// Every fragment is mixed towards the fog colour (the file holds the world fog formula) and
    /// the pipelines that use it blend opaque or alpha: nothing shows behind a hidden section.
    FogColoured,
    /// Some draw of the file is not turned into the fog colour: the frame rule that covers it.
    Condition(&'static str),
    /// Nothing the terrain can hide, or judged with another file: why.
    Elsewhere(&'static str),
}

/// Every shader file of the client, classified for the fog cull.
const SHADERS_IN_PLAIN_FOG: [(&str, InPlainFog); 42] = {
    use InPlainFog::{Condition, Elsewhere, FogColoured};
    const STAGES: InPlainFog = Elsewhere("improved transparency stages: every section is drawn under that option");
    const SCREEN: InPlainFog = Elsewhere("interface or screen effect, drawn after the world");
    [
        ("block_fx.wgsl", FogColoured),
        ("block_fx_oit.wgsl", STAGES),
        ("block_text.wgsl", FogColoured),
        ("blur.wgsl", SCREEN),
        ("border.wgsl", Condition("no fog: the border wall is a fog-piercing draw")),
        ("border_oit.wgsl", STAGES),
        ("crack.wgsl", Condition("multiplied into the pixel: fog-piercing, measured by `Cracks::farthest_vertex`")),
        ("crack_oit.wgsl", STAGES),
        ("debug_lines.wgsl", Condition("filled faces without fog: fog-piercing (`draws_filled_faces`)")),
        ("debug_lines_oit.wgsl", STAGES),
        ("debug_text.wgsl", FogColoured),
        ("debug_text_oit.wgsl", STAGES),
        ("entity.wgsl", Condition("lightning, energy swirl and dragon rays are added: `farthest_additive_draw`")),
        ("entity_oit.wgsl", STAGES),
        ("entity_outline.wgsl", Elsewhere("glowing outlines are drawn without the world's depth")),
        ("gui.wgsl", SCREEN),
        ("item.wgsl", SCREEN),
        ("leash.wgsl", FogColoured),
        ("nametag.wgsl", FogColoured),
        ("nametag_oit.wgsl", STAGES),
        ("oit.wgsl", STAGES),
        ("oit_bounds_compact.wgsl", STAGES),
        ("oit_bounds_rgba.wgsl", STAGES),
        ("oit_passes.wgsl", STAGES),
        ("oit_wavelet_loops.wgsl", STAGES),
        ("oit_wavelet_unrolled.wgsl", STAGES),
        ("outline.wgsl", FogColoured),
        ("outline_oit.wgsl", STAGES),
        ("overlays.wgsl", SCREEN),
        ("panorama.wgsl", SCREEN),
        ("particles.wgsl", FogColoured),
        ("particles_oit.wgsl", STAGES),
        ("post_fx.wgsl", SCREEN),
        ("sky.wgsl", Condition("sun, moon, stars, End sky, clouds: the sky and cloud rules of the frame")),
        ("sky_oit.wgsl", STAGES),
        ("tech_cells.wgsl", Elsewhere("vertex stage only: its fragments are the fogged segments of debug_lines.wgsl")),
        ("terrain.wgsl", Condition("the sky dome it also draws: the sky rules of the frame")),
        ("terrain_oit.wgsl", STAGES),
        ("text_display.wgsl", FogColoured),
        ("text_display_oit.wgsl", STAGES),
        ("weather.wgsl", FogColoured),
        ("weather_oit.wgsl", STAGES),
    ]
};

/// Source files that write a blend out by its factors (anything but the stock opaque and alpha
/// blends), with the number of factors written and what was checked of them.
const BLENDS_WRITTEN_BY_FACTORS: [(&str, usize, &str); 8] = [
    ("crack.rs", 4, "cracks, multiplied: fog-piercing"),
    ("entity_render.rs", 6, "lightning blend (lightning, swirl, rays): fog-piercing; glint: fades to nothing in fog"),
    ("item_atlas.rs", 4, "item icons of the interface"),
    ("oit.rs", 6, "improved transparency stages and composition"),
    ("overlays.rs", 2, "screen overlays"),
    ("render_gpu_test.rs", 2, "a test"),
    ("sky.rs", 4, "sun, moon, stars, sunrise: drawn with the sky dome only"),
    ("weather_render.rs", 8, "rain and snow: alpha; border wall: fog-piercing"),
];

/// Files under `dir` (recursively) whose name ends with `extension`, as (name, contents).
fn sources_under(dir: &std::path::Path, extension: &str, found: &mut Vec<(String, String)>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let path = entry.expect("directory entry").path();
        if path.is_dir() {
            sources_under(&path, extension, found);
        } else if path.extension().is_some_and(|ext| ext == extension) {
            let name = path.file_name().expect("file name").to_string_lossy().into_owned();
            found.push((name, std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))));
        }
    }
}

/// The cull is exact only while every draw of the world pass is turned into the fog colour by
/// plain fog, or is a condition of the frame rules. A shader or a blend added or changed must be
/// judged against that (docs/specs/visibility.md, "Sections the fog hides whole") before this
/// inventory is brought up to date.
#[test]
fn every_world_shader_and_blend_is_classified() {
    const HOW: &str = "judge it against the fog cull (docs/specs/visibility.md), then update this inventory";
    let sources = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut shaders = Vec::new();
    sources_under(&sources, "wgsl", &mut shaders);
    shaders.sort();
    let found: Vec<&str> = shaders.iter().map(|(name, _)| name.as_str()).collect();
    let classified: Vec<&str> = SHADERS_IN_PLAIN_FOG.iter().map(|(name, _)| *name).collect();
    assert_eq!(found, classified, "shader files changed: {HOW}");
    for ((name, source), (_, in_plain_fog)) in shaders.iter().zip(&SHADERS_IN_PLAIN_FOG) {
        match in_plain_fog {
            InPlainFog::FogColoured => assert!(source.contains("linear_fog("), "{name} has no world fog: {HOW}"),
            InPlainFog::Condition(rule) | InPlainFog::Elsewhere(rule) => assert!(!rule.is_empty()),
        }
    }
    // The needle is split so that this file does not count itself.
    let factor = concat!("Blend", "Factor::");
    let mut code = Vec::new();
    sources_under(&sources, "rs", &mut code);
    code.sort();
    let written: Vec<(&str, usize)> = code
        .iter()
        .map(|(name, source)| (name.as_str(), source.matches(factor).count()))
        .filter(|&(_, factors)| factors > 0)
        .collect();
    let checked: Vec<(&str, usize)> = BLENDS_WRITTEN_BY_FACTORS.iter().map(|&(name, n, _)| (name, n)).collect();
    assert_eq!(written, checked, "blends written by their factors changed: {HOW}");
}

/// Columns the integrated server sends around a player at a view distance (in columns): a disc.
fn tracked_columns(view: i32) -> Vec<(i32, i32)> {
    let gap = |d: i32| (d.abs() - 1).max(0);
    let span = -view - 1..=view + 1;
    let all = span.clone().flat_map(|x| span.clone().map(move |z| (x, z)));
    all.filter(|&(x, z)| gap(x).pow(2) + gap(z).pow(2) < view * view).collect()
}

/// Micro-bench (machine loaded by other agents' builds: an order of magnitude, not a decision):
/// at view distances 12 and 32, over the columns the server tracks (open sky above solid ground,
/// 24 sections), in the fogs that hide the most: sections left in sight, time to remake the list
/// the draw lists read, and time of the walk whole or cut at the fog.
#[test]
#[ignore = "micro-bench: cargo test --release -p rc-app fog_cull_bench -- --ignored --nocapture"]
fn fog_cull_bench() {
    // Solid ground entered but not crossed (sections under y = 64), open air above.
    let column: Vec<_> = (0..SECTIONS).map(|sy| if sy < 8 { VIS_NONE } else { VIS_ALL }).collect();
    let cases: [(&str, (f32, f32), FogCullPaths, bool); 5] = [
        ("Nether, fog end 96", (10.0, 96.0), DEFAULT_PATHS, false),
        ("lava, fog end 1", (0.25, 1.0), DEFAULT_PATHS, false),
        ("water at full vision under the sky, below the sky rim", (-8.0, 96.0), BELOW_SKY_RIM, true),
        ("water at full vision under the sky, against the sky", (-8.0, 96.0), AGAINST_SKY, true),
        ("open air under the sky, against the sky", (0.0, 1024.0), AGAINST_SKY, true),
    ];
    let best_ms = |work: &mut dyn FnMut()| {
        let passes = (0..15).map(|_| {
            let started = std::time::Instant::now();
            work();
            started.elapsed().as_secs_f64() * 1e3
        });
        passes.fold(f64::MAX, f64::min)
    };
    for view in [12, 32] {
        let tracked: hashbrown::HashSet<(i32, i32)> = tracked_columns(view).into_iter().collect();
        let columns = |x: i32, z: i32| tracked.contains(&(x, z)).then_some(&column[..]);
        for eye in [[8.5, 71.6, 8.5], [8.5, 250.3, 8.5]] {
            let mut graph = Graph::default();
            let mut walk = |sight_limit: f64| {
                let mut reached = graph.run_within(columns, eye, MIN_Y, SECTIONS, view + 1, true, sight_limit).to_vec();
                reached.sort_unstable_by_key(|&(x, sy, z)| (x, z, sy));
                reached
            };
            let reached = walk(f64::INFINITY);
            let whole_walk_ms = best_ms(&mut || drop(std::hint::black_box(walk(f64::INFINITY))));
            eprintln!("view {view}, eye y {}: {} sections reached, walk {whole_walk_ms:.3} ms", eye[1], reached.len());
            for (name, environment, paths, under_sky) in cases {
                let fog =
                    Fog { sky_end: environment.1.min(view as f32 * 16.0), ..fog(environment, view as f32 * 16.0) };
                let frame = if under_sky { frame_under_sky(fog) } else { frame_without_sky(fog) };
                let fog_reach = FogReach::of_frame(&frame, TERRAIN_ALONE, paths);
                let mut drawn = DrawnSections::default();
                let mut walk_generation = 0;
                let list_ms = best_ms(&mut || {
                    walk_generation += 1;
                    drawn.refresh(&reached, walk_generation, eye, MIN_Y, fog_reach);
                });
                let in_sight = drawn.sections(&reached).len();
                let sight_limit = walk_sight_limit(fog_reach, FogCullPaths { cuts_walk: true, ..paths });
                let cut_walk_ms = best_ms(&mut || drop(std::hint::black_box(walk(sight_limit))));
                eprintln!(
                    "  {name}: {in_sight} sections in sight ({:.1} %), list remade in {list_ms:.3} ms; walk cut at \
                     {sight_limit:.1}: {} sections, {cut_walk_ms:.3} ms",
                    in_sight as f64 * 100.0 / reached.len() as f64,
                    walk(sight_limit).len(),
                );
            }
        }
    }
}
