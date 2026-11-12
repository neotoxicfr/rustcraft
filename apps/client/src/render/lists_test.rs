//! Proofs of equivalence of the draw lists (`RUSTCRAFT_OPT_VISORDER`, `RUSTCRAFT_OPT_ARGSKIP`,
//! `RUSTCRAFT_OPT_REACHGRID`) and micro-bench of the construction.

use super::*;
use std::sync::Arc;

/// Deterministic generator (64-bit LCG, high bits).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }

    fn below(&mut self, n: u32) -> u32 {
        self.next() % n
    }
}

/// Fixed seed of the trials (no dependence on the clock).
const SEED: u64 = 0x5eed_c306;
const SECTIONS: i32 = 24;
const MIN_Y: i32 = -64;

/// Mesh version of the next test column: each upload has its own, like the real meshes.
fn next_version() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static VERSION: AtomicU64 = AtomicU64::new(1);
    VERSION.fetch_add(1, Ordering::Relaxed)
}

/// Test column: random faces, fluids and translucents per layer, group and section,
/// absolute instances contiguous within their region (`cursor`), like the real upload.
fn column(
    rng: &mut Lcg,
    k: (i32, i32),
    region_div: i32,
    cursor: &mut HashMap<(i32, i32), u32>,
    trans: &mut u32,
) -> ColumnGpu {
    let region = (k.0.div_euclid(region_div), k.1.div_euclid(region_div));
    let face_start = *cursor.get(&region).unwrap_or(&0);
    let s1 = SECTIONS as usize + 1;
    let cum = |rng: &mut Lcg, n: usize, at: &mut u32, empty: bool| {
        let mut v = Vec::with_capacity(n);
        for i in 0..n {
            v.push(*at);
            // Last entry of a group = end (no faces after).
            if i % s1 != s1 - 1 && !empty && rng.below(3) != 0 {
                *at += rng.below(6);
            }
        }
        v
    };
    let at = cursor.entry(region).or_insert(0);
    let mut starts: [Vec<u32>; 3] = Default::default();
    let mut ranges: [Range<u32>; 3] = Default::default();
    for layer in 0..3 {
        let a0 = *at;
        let empty = rng.below(4) == 0;
        starts[layer] = cum(rng, BUCKETS * s1, at, empty);
        ranges[layer] = a0..*at;
    }
    let mut fluid_starts: [Vec<u32>; 3] = Default::default();
    let mut fluid_ranges: [Range<u32>; 3] = Default::default();
    for layer in 0..2 {
        let a0 = *at;
        let empty = rng.below(2) == 0;
        fluid_starts[layer] = cum(rng, s1, at, empty);
        fluid_ranges[layer] = a0..*at;
    }
    let t0 = *trans;
    let empty = rng.below(3) == 0;
    let order_starts = cum(rng, s1, trans, empty);
    fluid_ranges[2] = t0..*trans;
    fluid_starts[2] = order_starts.clone();
    ColumnGpu {
        digest: 0,
        region,
        face_alloc: (face_start, 0),
        fluid_alloc: (0, 0),
        trans_alloc: (0, 0),
        ranges,
        fluid_ranges,
        starts,
        fluid_starts,
        vis: Arc::from(Vec::new()),
        trans_centers: Vec::new(),
        trans_starts: Vec::new(),
        order_alloc: (t0, *trans - t0),
        order_starts,
        sorted_at: None,
        view_cells: Vec::new(),
        view_epoch: 0,
        version: next_version(),
        trans_slot: None,
    }
}

/// Loaded columns and reached sections.
type World = (HashMap<(i32, i32), ColumnGpu>, Vec<(i32, i32, i32)>);

/// Test world of radius `r` around (0, 0): loaded columns (a few are missing: the
/// list must ignore them) and reached sections (`density` out of 8), stored like the walk.
fn world(rng: &mut Lcg, r: i32, density: u32) -> World {
    let (mut cols, mut reached) = (HashMap::new(), Vec::new());
    let (mut cursor, mut trans) = (HashMap::new(), 0);
    for x in -r..=r {
        for z in -r..=r {
            if rng.below(16) != 0 {
                cols.insert((x, z), column(rng, (x, z), 4, &mut cursor, &mut trans));
            }
        }
    }
    reach(rng, r, density, &mut reached);
    (cols, reached)
}

fn reach(rng: &mut Lcg, r: i32, density: u32, out: &mut Vec<(i32, i32, i32)>) {
    out.clear();
    for x in -r..=r {
        for z in -r..=r {
            for sy in 0..SECTIONS {
                if rng.below(8) < density {
                    out.push((x, sy, z));
                }
            }
        }
    }
    out.sort_unstable_by_key(|&(x, sy, z)| (x, z, sy));
}

/// Frame of trajectory `f`: eye (including heights at the exact middle of a section and on a
/// boundary), yaw, pitch.
fn camera(f: u32) -> ([f64; 3], f32, f32) {
    let t = f64::from(f);
    let y = match f % 5 {
        // Exact middle of a section: two sections at equal distance on either side.
        0 => f64::from(MIN_Y + 16 * (4 + (f / 5 % 8) as i32) + 8),
        // Boundary between two sections.
        1 => f64::from(MIN_Y + 16 * (5 + (f / 5 % 6) as i32)),
        _ => 70.0 + (t * 0.37).sin() * 30.0,
    };
    // Horizontal position held for 4 frames (rotation in place: the cached order is used); the
    // height does not change the origin.
    let p = f64::from(f / 4);
    let eye = [(p * 0.9).sin() * 40.0 + p * 0.25, y, (p * 0.6).cos() * 40.0 - p * 0.1];
    (eye, ((t * 7.3) % 360.0 - 180.0) as f32, ((t * 1.7).sin() * 80.0) as f32)
}

fn planes(eye: [f64; 3], yaw: f32, pitch: f32, far: f32) -> ([i32; 2], [Vec4; 6]) {
    let (origin, _, view_m) = crate::camera::view_matrix(eye, yaw, pitch);
    (origin, super::super::frustum::frustum_planes(super::super::perspective(70.0, 16.0 / 9.0, 0.05, far) * view_m))
}

fn fnv(h: &mut u64, bytes: &[u8]) {
    for &b in bytes {
        *h = (*h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
    }
}

/// Batches and commands of a frame in bytes (exact comparison).
fn image_bytes(l: &Lists, drawn: (usize, u64)) -> Vec<u8> {
    let mut v = Vec::new();
    for &(layer, fluid, (rx, rz), first, count) in &l.batches {
        v.extend_from_slice(&(layer as u32).to_le_bytes());
        v.push(u8::from(fluid));
        for n in [rx, rz] {
            v.extend_from_slice(&n.to_le_bytes());
        }
        v.extend_from_slice(&first.to_le_bytes());
        v.extend_from_slice(&count.to_le_bytes());
    }
    v.extend_from_slice(bytemuck::cast_slice(&l.args));
    v.extend_from_slice(&(drawn.0 as u64).to_le_bytes());
    v.extend_from_slice(&drawn.1.to_le_bytes());
    v
}

/// Trajectory of 600 frames: walk redone every 7 frames, columns changed every
/// 13 (one removed, one put back), advancing origin; batches, commands and count identical frame by
/// frame between the old and the new path, and fingerprint of the old path stored.
#[test]
fn draw_lists_match_the_old_build_along_a_trajectory() {
    let mut rng = Lcg(SEED);
    let (mut cols, mut reached) = world(&mut rng, 12, 5);
    let (mut old, mut new) = (Lists::default(), Lists::default());
    let (mut rgen, mut rev) = (1u64, 1u64);
    let mut digest = 0xcbf2_9ce4_8422_2325u64;
    let mut ties = 0;
    let mut merged = 0;
    let mut hits = 0;
    for f in 0..600u32 {
        if f % 7 == 6 {
            reach(&mut rng, 12, 5, &mut reached);
            rgen += 1;
        }
        if f % 13 == 12 {
            let k = ((rng.below(25) as i32) - 12, (rng.below(25) as i32) - 12);
            if let Some(c) = cols.remove(&k) {
                let k2 = (k.1, k.0);
                cols.entry(k2).or_insert(c);
            }
            rev += 1;
        }
        let (eye, yaw, pitch) = camera(f);
        let (origin, pl) = planes(eye, yaw, pitch, 12.0 * 16.0);
        let input = ListIn {
            columns: &cols,
            reached: &reached,
            planes: &pl,
            origin,
            eye,
            min_y: MIN_Y,
            sections: SECTIONS,
            face_vertices: 4,
            split_draws: false,
            sections_generation: rgen,
            columns_rev: rev,
        };
        let a = build(&input, &mut old, ListPaths::OLD, || {});
        let b = build(&input, &mut new, ListPaths { cached_order: true, ..ListPaths::OLD }, || {});
        hits += usize::from(new.order_key == Some((rgen, origin, rev)));
        let (ia, ib) = (image_bytes(&old, a), image_bytes(&new, b));
        assert!(ia == ib, "frame {f}: lists differ (eye {eye:?}, yaw {yaw}, pitch {pitch})");
        fnv(&mut digest, &ia);
        ties += usize::from(f % 5 == 0 && old.batches.iter().any(|b| b.0 == 2 && b.1));
        merged += old.args.iter().filter(|a| a.instance_count > 5).count();
    }
    assert!(ties > 0 && merged > 0, "trajectory without ties or merges: it proves nothing");
    assert!(hits > 200 && hits < 550, "frames served by the cached order: {hits} (both paths must serve)");
    // Fingerprint of the old path on this trajectory (fixed trial seed): changes if the
    // construction itself changes.
    assert_eq!(digest, EXPECTED_DIGEST, "trajectory fingerprint: {digest:#018x}");
}

const EXPECTED_DIGEST: u64 = 0x88c4_1e8e_1e14_3620;

/// Frames of the scripted cameras.
const SCRIPTED_FRAMES: u32 = 700;

/// Scripted camera of the path tests: rotation in place, walk at a fixed height (steps across
/// column edges), flight (every coordinate moving), rotation at a section's exact middle then on a
/// section boundary, and a look straight down from high above (regions wholly in the field).
/// Returns eye, yaw, pitch, far plane.
fn scripted_camera(f: u32) -> ([f64; 3], f32, f32, f32) {
    let t = f as f32;
    let far = 12.0 * 16.0;
    match f / 150 {
        0 => ([3.5, 70.5, -2.5], t * 2.4 - 180.0, (t * 0.07).sin() * 60.0, far),
        1 => {
            let s = f64::from(f - 150) * 0.3;
            ([-20.0 + s, 70.5, 4.0 + s * 0.2], 90.0 + (t * 0.05).sin() * 40.0, 10.0, far)
        }
        2 => {
            let s = f64::from(f - 300);
            ([20.0 - s * 0.8, 60.0 + s * 0.4, -10.0 + s * 0.5], t * 1.3, (t * 0.11).sin() * 70.0, far)
        }
        3 => {
            // Middle of section 9, then the boundary between sections 9 and 10.
            let y = if f < 525 { f64::from(MIN_Y + 16 * 9 + 8) } else { f64::from(MIN_Y + 16 * 10) };
            ([0.5, y, 0.5], t * 3.1, (t * 0.13).sin() * 85.0, far)
        }
        _ => ([0.5, 900.0, 0.5], t, 89.0, 2000.0),
    }
}

/// Test world changed along the scripted cameras like the game changes it: walk redone, a column
/// sent again with a new mesh (often while the camera holds still), the same mesh moved to another
/// place, a column removed then sent back.
struct ScriptedWorld {
    rng: Lcg,
    columns: HashMap<(i32, i32), ColumnGpu>,
    reached: Vec<(i32, i32, i32)>,
    cursor: HashMap<(i32, i32), u32>,
    trans: u32,
    removed: Option<((i32, i32), ColumnGpu)>,
    reached_gen: u64,
    columns_rev: u64,
}

impl ScriptedWorld {
    fn new(seed: u64) -> Self {
        let mut rng = Lcg(seed);
        let (mut columns, mut reached) = (HashMap::new(), Vec::new());
        let (mut cursor, mut trans) = (HashMap::new(), 0);
        for x in -12..=12 {
            for z in -12..=12 {
                if rng.below(16) != 0 {
                    let c = Self::new_column(&mut rng, (x, z), &mut cursor, &mut trans);
                    columns.insert((x, z), c);
                }
            }
        }
        reach(&mut rng, 12, 5, &mut reached);
        ScriptedWorld { rng, columns, reached, cursor, trans, removed: None, reached_gen: 1, columns_rev: 1 }
    }

    /// Column with air above its ground (sections drawing nothing) and, one time in two, a cave
    /// section drawing nothing below it.
    fn new_column(rng: &mut Lcg, k: (i32, i32), cursor: &mut HashMap<(i32, i32), u32>, trans: &mut u32) -> ColumnGpu {
        let mut c = column(rng, k, 4, cursor, trans);
        let ground = 8 + rng.below(10) as usize;
        let cave = (rng.below(2) == 0).then(|| 1 + rng.below(ground as u32 - 2) as usize);
        with_air(&mut c, ground, cave);
        c
    }

    /// Changes of frame `f`; `near`: column of the camera.
    fn step(&mut self, f: u32, near: (i32, i32)) {
        let rng = &mut self.rng;
        let around = |rng: &mut Lcg| (near.0 + rng.below(9) as i32 - 4, near.1 + rng.below(9) as i32 - 4);
        if f % 23 == 22 {
            reach(rng, 12, 5, &mut self.reached);
            self.reached_gen += 1;
        }
        if f % 7 == 3 {
            let k = around(rng);
            let fresh = Self::new_column(rng, k, &mut self.cursor, &mut self.trans);
            self.columns.insert(k, fresh);
            self.columns_rev += 1;
        }
        if f % 17 == 5 {
            // Same mesh (same version) uploaded at another place of its buffers.
            let k = around(rng);
            if let Some(c) = self.columns.get_mut(&k) {
                move_column(c, 7);
                self.columns_rev += 1;
            }
        }
        if f % 11 == 6 {
            // Partial remesh written in place: same places, faces spread otherwise, new version.
            let k = around(rng);
            if let Some(c) = self.columns.get_mut(&k) {
                remesh_in_place(c, rng);
                self.columns_rev += 1;
            }
        }
        if f % 31 == 10 {
            let k = around(rng);
            self.removed = self.columns.remove(&k).map(|c| (k, c));
            self.columns_rev += 1;
        }
        if f % 31 == 20
            && let Some((k, c)) = self.removed.take()
        {
            self.columns.insert(k, c);
            self.columns_rev += 1;
        }
    }

    fn input<'a>(&'a self, planes: &'a [Vec4; 6], origin: [i32; 2], eye: [f64; 3]) -> ListIn<'a> {
        ListIn {
            columns: &self.columns,
            reached: &self.reached,
            planes,
            origin,
            eye,
            min_y: MIN_Y,
            sections: SECTIONS,
            face_vertices: 4,
            split_draws: false,
            sections_generation: self.reached_gen,
            columns_rev: self.columns_rev,
        }
    }
}

/// Makes the sections from `ground` up (air) and the section `cave` draw nothing in every list.
fn with_air(c: &mut ColumnGpu, ground: usize, cave: Option<usize>) {
    let empty = |sy: usize| sy >= ground || Some(sy) == cave;
    let lists = c.starts.iter_mut().chain(c.fluid_starts.iter_mut()).chain([&mut c.order_starts]);
    for starts in lists {
        for group in starts.as_chunks_mut::<{ SECTIONS as usize + 1 }>().0 {
            let counts: Vec<u32> = group.windows(2).map(|w| w[1] - w[0]).collect();
            for (sy, n) in counts.into_iter().enumerate() {
                group[sy + 1] = group[sy] + if empty(sy) { 0 } else { n };
            }
        }
    }
}

/// Places the column `by` instances further in each of its buffers.
fn move_column(c: &mut ColumnGpu, by: u32) {
    for r in c.ranges.iter_mut().chain(c.fluid_ranges.iter_mut()) {
        *r = r.start + by..r.end + by;
    }
    for s in c.starts.iter_mut().chain(c.fluid_starts.iter_mut()).chain([&mut c.order_starts]) {
        s.iter_mut().for_each(|v| *v += by);
    }
    for a in [&mut c.face_alloc, &mut c.fluid_alloc, &mut c.trans_alloc, &mut c.order_alloc] {
        a.0 += by;
    }
}

/// New mesh of the same size written over the old one (`in_place_writes`): the faces of the
/// opaque layer move from one section to the next, the places stay, the version changes.
fn remesh_in_place(c: &mut ColumnGpu, rng: &mut Lcg) {
    let s1 = SECTIONS as usize + 1;
    let starts = &mut c.starts[0];
    for g in 0..BUCKETS {
        let i = g * s1 + 1 + rng.below(SECTIONS as u32 - 1) as usize;
        if starts[i] < starts[i + 1] {
            starts[i] += 1;
        }
    }
    c.version = next_version();
}

/// Every new path at once, on the calling thread.
const ALL_SERIAL: ListPaths = ListPaths {
    cached_order: true,
    column_commands: true,
    hierarchical_frustum: true,
    plane_coherence: true,
    section_search: true,
    reuse_buffers: true,
    ..ListPaths::OLD
};

/// A path compared with the old construction.
type Compared = (String, ListPaths);

/// Each path alone (on the old construction and on the cached order), and all together.
fn compared_paths() -> Vec<Compared> {
    let old = ListPaths::OLD;
    let cached = ListPaths { cached_order: true, ..old };
    let mut v: Vec<Compared> = vec![("cached order".into(), cached), ("all".into(), ALL_SERIAL)];
    for (base_name, base) in [("old", old), ("cached order", cached)] {
        let alone = [
            ("command cache", ListPaths { column_commands: true, ..base }),
            ("hierarchical frustum", ListPaths { hierarchical_frustum: true, ..base }),
            ("plane coherence", ListPaths { plane_coherence: true, ..base }),
            ("section search", ListPaths { section_search: true, ..base }),
            ("section search in the hierarchy", ListPaths { section_search: true, hierarchical_frustum: true, ..base }),
            ("buffer reuse", ListPaths { reuse_buffers: true, ..base }),
        ];
        v.extend(alone.map(|(name, p)| (format!("{base_name} + {name}"), p)));
    }
    v
}

/// Draw settings of each frame: face vertices and split draws.
type DrawSettings = fn(u32) -> (u32, bool);

/// The game's usual settings: faces as 4-vertex strips, consecutive draws merged.
fn usual_settings(_: u32) -> (u32, bool) {
    (4, false)
}

/// Settings changed every 40 frames on the same lists: 6-vertex faces (shader without the strip),
/// split draws (`RUSTCRAFT_SPLITDRAWS`), both, then back to the usual ones.
fn changing_settings(f: u32) -> (u32, bool) {
    [(4, false), (6, false), (6, true), (4, true)][(f / 40 % 4) as usize]
}

/// Lists of each frame of the scripted cameras drawn with `settings`: `on_frame(frame, lists of
/// the old construction)`.
fn along_scripted_cameras(settings: DrawSettings, mut on_frame: impl FnMut(u32, &ListIn<'_>, &[u8])) {
    let mut world = ScriptedWorld::new(SEED ^ 5);
    let mut reference = Lists::default();
    for f in 0..SCRIPTED_FRAMES {
        let (eye, yaw, pitch, far) = scripted_camera(f);
        world.step(f, (eye[0].floor() as i32 >> 4, eye[2].floor() as i32 >> 4));
        let (origin, pl) = planes(eye, yaw, pitch, far);
        let (face_vertices, split_draws) = settings(f);
        let input = ListIn { face_vertices, split_draws, ..world.input(&pl, origin, eye) };
        let drawn = build(&input, &mut reference, ListPaths::OLD, || {});
        on_frame(f, &input, &image_bytes(&reference, drawn));
    }
}

/// Every path of [`compared_paths`] built on its own lists along the scripted cameras with
/// `settings`, compared with the old construction on every frame; returns the columns reused and
/// rebuilt by every serial path during the rotation in place.
fn compare_every_path(settings: DrawSettings) -> (u64, u64) {
    let mut compared: Vec<(Compared, Lists)> = compared_paths().into_iter().map(|c| (c, Lists::default())).collect();
    let (mut reused, mut rebuilt, mut drawn_frames) = (0, 0, 0);
    along_scripted_cameras(settings, |f, input, want| {
        drawn_frames += usize::from(want.len() > 64);
        for ((name, paths), lists) in &mut compared {
            let drawn = build(input, lists, *paths, || {});
            assert!(image_bytes(lists, drawn) == want, "frame {f}: path {name} differs");
            if *paths == ALL_SERIAL && f < 150 {
                let (r, b) = lists.column_command_reuse();
                (reused, rebuilt) = (reused + r, rebuilt + b);
            }
        }
    });
    assert!(drawn_frames > 600, "frames drawing something: {drawn_frames}");
    (reused, rebuilt)
}

/// Every path gives the old construction's batches and commands (order included) on every frame
/// of the scripted cameras, with columns sent, moved, removed and sent back on the way; the cache
/// serves most columns while the camera turns in place.
#[test]
fn every_path_matches_the_old_build_along_scripted_cameras() {
    let (reused, rebuilt) = compare_every_path(usual_settings);
    println!("rotation in place: {reused} columns reused, {rebuilt} rebuilt");
    assert!(reused > rebuilt && rebuilt > 0, "rotation in place: {reused} columns reused, {rebuilt} rebuilt");
}

/// The same with 6-vertex faces and split draws, changed on the way on the same lists: the merges
/// they turn off and the cache emptied at each change.
#[test]
fn every_path_matches_the_old_build_while_the_draw_settings_change() {
    let (reused, rebuilt) = compare_every_path(changing_settings);
    println!("rotation in place, settings changing: {reused} columns reused, {rebuilt} rebuilt");
    assert!(reused > 0 && rebuilt > 0, "rotation in place: {reused} columns reused, {rebuilt} rebuilt");
}

/// The sabotages the tests must see: stale face groups or eye height reused by the cache, a
/// column across a plane accepted whole, runs joined over empty sections, a one-sided section
/// search, the cache kept across a change of the draw settings.
#[test]
fn sabotaged_paths_are_caught() {
    let usual = [
        (Sabotage::StaleFacings, ALL_SERIAL),
        (Sabotage::IgnoredMeshVersion, ALL_SERIAL),
        (Sabotage::IgnoredEyeHeight, ALL_SERIAL),
        (Sabotage::StraddlingColumnAcceptedWhole, ALL_SERIAL),
        (Sabotage::RunsJoinedAcrossEmptySections, ALL_SERIAL),
        (Sabotage::SectionSearchFromBelowOnly, ALL_SERIAL),
    ];
    let changing = [(Sabotage::CacheKeptAcrossSettings, ALL_SERIAL)];
    for (settings, sabotaged) in [(usual_settings as DrawSettings, &usual[..]), (changing_settings, &changing[..])] {
        let mut runs: Vec<(Sabotage, ListPaths, Lists, u32)> = sabotaged
            .iter()
            .map(|&(s, p)| (s, ListPaths { sabotage: Some(s), ..p }, Lists::default(), 0))
            .collect();
        along_scripted_cameras(settings, |_, input, want| {
            for (_, paths, lists, differs) in &mut runs {
                let drawn = build(input, lists, *paths, || {});
                *differs += u32::from(image_bytes(lists, drawn) != want);
            }
        });
        for (s, .., differs) in &runs {
            println!("sabotage {s:?}: {differs} frames differ");
            assert!(*differs > 0, "sabotage {s:?} not seen");
        }
    }
}

/// A cleared terrain restarts its buffers from the first place, and a column of the next world can
/// meet an old entry's key (same coordinates, version and places, other faces): with the cache
/// forgotten, it is drawn from its own mesh; without, stale commands would be drawn.
#[test]
fn forgotten_columns_are_rebuilt_after_the_terrain_is_cleared() {
    let mut world = ScriptedWorld::new(SEED ^ 13);
    let (eye, yaw, pitch, far) = scripted_camera(0);
    let (origin, pl) = planes(eye, yaw, pitch, far);
    let (mut kept, mut forgotten) = (Lists::default(), Lists::default());
    build(&world.input(&pl, origin, eye), &mut kept, ALL_SERIAL, || {});
    build(&world.input(&pl, origin, eye), &mut forgotten, ALL_SERIAL, || {});
    // Next world: other faces at the same places, under the same versions.
    for c in world.columns.values_mut() {
        let version = c.version;
        remesh_in_place(c, &mut world.rng);
        c.version = version;
    }
    forgotten.forget_columns();
    let input = world.input(&pl, origin, eye);
    let want = {
        let mut reference = Lists::default();
        let drawn = build(&input, &mut reference, ListPaths::OLD, || {});
        image_bytes(&reference, drawn)
    };
    let drawn = build(&input, &mut forgotten, ALL_SERIAL, || {});
    assert!(image_bytes(&forgotten, drawn) == want, "forgotten cache: stale commands drawn");
    let drawn = build(&input, &mut kept, ALL_SERIAL, || {});
    assert!(image_bytes(&kept, drawn) != want, "kept cache: the scene does not tell stale commands apart");
}

/// Region box ⊇ column box ⊇ box of a column's reached sections ⊇ section box, and an enclosing
/// box out of the field (or wholly inside) makes every box it holds so: the shortcuts of the
/// hierarchical frustum give the per-section answer. 10^5 random boxes and frusta.
#[test]
fn enclosing_boxes_answer_like_the_boxes_they_hold() {
    let mut rng = Lcg(SEED ^ 11);
    let mut outer = [0usize; 3];
    for case in 0..100_000 {
        let k = (rng.below(81) as i32 - 40, rng.below(81) as i32 - 40);
        let ((eye, yaw, pitch), far) = if case % 4 == 0 {
            // From high above the column's region, looking down: whole regions in the field.
            let mut centre = |c: i32| f64::from((c >> REGION_SHIFT) * 256 + 128) + f64::from(rng.below(64)) - 32.0;
            let (x, z) = (centre(k.0), centre(k.1));
            (
                ([x, 700.0 + f64::from(rng.below(800)), z], rng.below(360) as f32, 89.0 + rng.below(10) as f32 / 10.0),
                3000.0,
            )
        } else {
            let around = |c: i32, rng: &mut Lcg| f64::from(c * 16 + rng.below(600) as i32 - 300) + 0.25;
            let eye = [around(k.0, &mut rng), f64::from(rng.below(400)) - 70.5, around(k.1, &mut rng)];
            let view = (eye, rng.below(3600) as f32 / 10.0 - 180.0, rng.below(1800) as f32 / 10.0 - 90.0);
            (view, 32.0 + rng.below(2000) as f32)
        };
        let (origin, pl) = planes(eye, yaw, pitch, far);
        let culling = ColumnCulling { planes: &pl, origin, min_y: MIN_Y, sections: SECTIONS, paths: ListPaths::OLD };
        let lo = rng.below(SECTIONS as u32) as i32;
        let hi = lo + rng.below((SECTIONS - lo) as u32) as i32;
        let sy = lo + rng.below((hi - lo + 1) as u32) as i32;
        let reached = (section_box(origin, MIN_Y, k, lo).0, section_box(origin, MIN_Y, k, hi).1);
        let boxes = [
            culling.region_box((k.0 >> REGION_SHIFT, k.1 >> REGION_SHIFT)),
            column_box(origin, MIN_Y, SECTIONS, k),
            reached,
            section_box(origin, MIN_Y, k, sy),
        ];
        let verdict = |(min, max): (Vec3, Vec3)| match (aabb_visible(&pl, min, max), aabb_inside(&pl, min, max)) {
            (false, false) => FrustumSide::Outside,
            (true, true) => FrustumSide::Inside,
            (true, false) => FrustumSide::Across,
            (false, true) => panic!("box wholly inside but not visible: {min} {max}"),
        };
        for w in boxes.windows(2) {
            let ((omin, omax), (imin, imax)) = (w[0], w[1]);
            assert!(omin.cmple(imin).all() && omax.cmpge(imax).all(), "box {imin}..{imax} not inside {omin}..{omax}");
            match verdict(w[0]) {
                FrustumSide::Outside => assert_eq!(verdict(w[1]), FrustumSide::Outside),
                FrustumSide::Inside => assert_eq!(verdict(w[1]), FrustumSide::Inside),
                FrustumSide::Across => {}
            }
        }
        outer[verdict(boxes[0]) as usize] += 1;
    }
    assert!(outer.iter().all(|&n| n > 1000), "region verdicts outside / inside / across: {outer:?}");
}

/// The plane remembered by the coherence test changes nothing to the answer, and after a
/// rejection it names a plane that rejects the box.
#[test]
fn remembered_plane_gives_the_same_answer() {
    let mut rng = Lcg(SEED ^ 13);
    let mut rejected = 0;
    for _ in 0..100_000 {
        let eye = [f64::from(rng.below(64)), 70.0, f64::from(rng.below(64))];
        let (origin, pl) = planes(eye, rng.below(360) as f32, rng.below(180) as f32 - 90.0, 300.0);
        let k = (rng.below(41) as i32 - 20, rng.below(41) as i32 - 20);
        let (min, max) = section_box(origin, MIN_Y, k, rng.below(SECTIONS as u32) as i32);
        let mut hint = rng.below(6) as u8;
        let seen = visible_with_hint(&pl, min, max, &mut hint);
        assert_eq!(seen, aabb_visible(&pl, min, max));
        if !seen {
            rejected += 1;
            assert!(!box_reaches_plane(&pl[usize::from(hint)], min, max), "remembered plane {hint} does not reject");
        }
    }
    assert!(rejected > 10_000, "boxes rejected: {rejected}");
}

/// Sections of a column from farthest to nearest: two-pointer merge = stable sort
/// of the old path, eye at the exact middle of a section and on a boundary included.
#[test]
fn far_to_near_matches_the_stable_sort() {
    let mut rng = Lcg(SEED ^ 1);
    let (mut src, mut got, mut want) = (Vec::new(), Vec::new(), Vec::new());
    let mut ties = 0;
    for case in 0..20_000u32 {
        let mut runs: Vec<Range<usize>> = Vec::new();
        let mut sy = rng.below(3) as usize;
        while sy < SECTIONS as usize && runs.len() < 6 {
            let len = 1 + rng.below(4) as usize;
            let end = (sy + len).min(SECTIONS as usize);
            runs.push(sy..end);
            sy = end + 1 + rng.below(3) as usize;
        }
        let ey = match case % 3 {
            0 => f64::from(16 * rng.below(SECTIONS as u32) + 8),
            1 => f64::from(16 * rng.below(SECTIONS as u32 + 1)),
            _ => f64::from(rng.below(SECTIONS as u32 * 16 * 64)) / 64.0 - 20.0,
        };
        got.clear();
        far_to_near(&runs, ey, &mut src, &mut got);
        want.clear();
        want.extend(runs.iter().flat_map(|r| r.clone()).map(|sy| sy..sy + 1));
        want.sort_by(|a: &Range<usize>, b: &Range<usize>| {
            section_dist(b.start, ey).total_cmp(&section_dist(a.start, ey))
        });
        assert_eq!(got, want, "runs {runs:?}, eye {ey}");
        ties += usize::from(want.windows(2).any(|w| section_dist(w[0].start, ey) == section_dist(w[1].start, ey)));
    }
    assert!(ties > 100, "too few ties ({ties}) to separate");
}

/// Range to rewrite: applied on the old contents, it gives back exactly the new
/// commands; nothing when they are identical.
#[test]
fn changed_range_rebuilds_the_new_commands() {
    let mut rng = Lcg(SEED ^ 2);
    let arg = |rng: &mut Lcg| DrawArgs {
        vertex_count: 4 + rng.below(2) * 2,
        instance_count: rng.below(4),
        first_vertex: 0,
        first_instance: rng.below(8),
    };
    let mut skipped = 0;
    for _ in 0..20_000 {
        let old: Vec<DrawArgs> = (0..rng.below(12)).map(|_| arg(&mut rng)).collect();
        let mut new = old.clone();
        match rng.below(4) {
            0 => {}
            1 => new.truncate(rng.below(new.len() as u32 + 1) as usize),
            2 => new.extend((0..1 + rng.below(3)).map(|_| arg(&mut rng))),
            _ => {
                if !new.is_empty() {
                    let i = rng.below(new.len() as u32) as usize;
                    new[i] = arg(&mut rng);
                }
            }
        }
        let mut gpu = old.clone();
        match changed_range(&old, &new) {
            None => {
                skipped += 1;
                assert!(bytemuck::cast_slice::<_, u8>(&old[..new.len()]) == bytemuck::cast_slice::<_, u8>(&new[..]));
            }
            Some(r) => {
                gpu.resize(
                    gpu.len().max(r.end),
                    DrawArgs { vertex_count: 0, instance_count: 0, first_vertex: 0, first_instance: 0 },
                );
                gpu[r.clone()].copy_from_slice(&new[r]);
            }
        }
        // The GPU reads only the first `new.len()`.
        assert!(
            bytemuck::cast_slice::<_, u8>(&gpu[..new.len()]) == bytemuck::cast_slice::<_, u8>(&new[..]),
            "old {} commands, new {}",
            old.len(),
            new.len()
        );
    }
    assert!(skipped > 1000, "no-change cases too rare ({skipped})");
}

/// Meshing view of a frame drawn from `eye` (`MeshingView`, `RUSTCRAFT_OPT_MESHVISIBLEONLY`).
fn meshing_view(eye: [f64; 3], yaw: f32, pitch: f32, far: f32) -> super::super::frustum::MeshingView {
    let (origin, pl) = planes(eye, yaw, pitch, far);
    super::super::frustum::MeshingView::new(pl, origin, eye, crate::camera::axes(yaw, pitch).2, MIN_Y, SECTIONS)
}

/// The meshing's view of a column holds every section the draw lists draw for it, along the
/// trajectory of the draw test, and more (the game's frustum moved back also holds sections behind
/// the camera); columns out of the field, wholly in it and across its edge all met.
#[test]
fn meshing_view_holds_the_drawn_runs() {
    use rc_mesh::wire::Sections;
    let mut rng = Lcg(SEED ^ 7);
    let (cols, mut reached) = world(&mut rng, 12, 5);
    let (mut outside, mut across, mut inside, mut wider) = (0, 0, 0, 0);
    let mut runs = Vec::new();
    for f in 0..240u32 {
        if f % 7 == 6 {
            reach(&mut rng, 12, 5, &mut reached);
        }
        // Far plane at 4 × the render distance, as the game's and ours (`environ`): moving the
        // frustum back never brings it inside the loaded area. The last frames from high above,
        // looking down: whole columns in the field.
        let ((eye, yaw, pitch), far) =
            if f < 200 { (camera(f), 4.0 * 12.0 * 16.0) } else { (([0.5, 900.0, 0.5], f as f32, 89.0), 2000.0) };
        let (origin, pl) = planes(eye, yaw, pitch, far);
        let input = ListIn {
            columns: &cols,
            reached: &reached,
            planes: &pl,
            origin,
            eye,
            min_y: MIN_Y,
            sections: SECTIONS,
            face_vertices: 4,
            split_draws: false,
            sections_generation: 0,
            columns_rev: 0,
        };
        let view = meshing_view(eye, yaw, pitch, far);
        for secs in reached.chunk_by(|a, b| (a.0, a.2) == (b.0, b.2)) {
            let k = (secs[0].0, secs[0].2);
            runs.clear();
            column_runs(&input, k, secs, &mut runs);
            let mut drawn = Sections::default();
            runs.iter().flat_map(Range::clone).for_each(|s| drawn.insert(s as i32));
            let seen = view.sections_in_view(k, secs);
            assert_eq!(seen.intersection(drawn), drawn, "frame {f}, column {k:?}: a drawn section left out");
            wider += usize::from(seen != drawn);
            match drawn.count(SECTIONS as usize) {
                0 => outside += 1,
                n if n == secs.len() => inside += 1,
                _ => across += 1,
            }
        }
    }
    assert!(outside > 1000 && across > 100 && inside > 1000, "cases {outside} / {across} / {inside}");
    assert!(wider > 100, "columns with sections in view behind the camera too rare ({wider})");
}

/// The meshing view's sections found by the section search (`RUSTCRAFT_OPT_SECTION_RANGE_SEARCH`)
/// are those of the per-section test, on every column of the draw test's trajectory; a one-sided
/// search is seen.
#[test]
fn meshing_view_section_search_matches_the_per_section_test() {
    let mut rng = Lcg(SEED ^ 7);
    let (_, mut reached) = world(&mut rng, 12, 5);
    let search = ListPaths { section_search: true, ..ListPaths::OLD };
    let sabotaged = ListPaths { sabotage: Some(Sabotage::SectionSearchFromBelowOnly), ..search };
    let (mut across, mut sabotage_seen) = (0, 0);
    for f in 0..240u32 {
        if f % 7 == 6 {
            reach(&mut rng, 12, 5, &mut reached);
        }
        let ((eye, yaw, pitch), far) =
            if f < 200 { (camera(f), 4.0 * 12.0 * 16.0) } else { (([0.5, 900.0, 0.5], f as f32, 89.0), 2000.0) };
        let view = meshing_view(eye, yaw, pitch, far);
        for secs in reached.chunk_by(|a, b| (a.0, a.2) == (b.0, b.2)) {
            let k = (secs[0].0, secs[0].2);
            let want = view.sections_in_view_with(k, secs, ListPaths::OLD);
            assert_eq!(view.sections_in_view_with(k, secs, search), want, "frame {f}, column {k:?}");
            let n = want.count(SECTIONS as usize);
            across += usize::from(n > 0 && n < secs.len());
            sabotage_seen += usize::from(view.sections_in_view_with(k, secs, sabotaged) != want);
        }
    }
    assert!(across > 100, "columns across an edge of the view too rare ({across})");
    assert!(sabotage_seen > 0, "one-sided section search not seen");
}

/// Game rule of the sections to rebuild: a dirty section just behind the camera (a lamp or a door
/// changing 4 to 6 blocks behind the player) is in the meshing view, so a quick half-turn shows it
/// rebuilt; the draw's frustum leaves it out. Two sections behind, it is out of both.
#[test]
fn section_just_behind_the_camera_is_in_the_meshing_view() {
    // Eye at z 4.5 looking south (+z), at height 70.5: its section is 8 (MIN_Y −64).
    let (eye, yaw, pitch, far) = ([4.5, 70.5, 4.5], 0.0, 0.0, 12.0 * 16.0);
    assert!(crate::camera::axes(yaw, pitch).2.z > 0.99, "yaw 0 looks south");
    let (origin, drawn_planes) = planes(eye, yaw, pitch, far);
    let drawn = super::super::frustum::MeshingView { planes: drawn_planes, origin, min_y: MIN_Y, sections: SECTIONS };
    let view = meshing_view(eye, yaw, pitch, far);
    let behind = [(0, 8, -1)];
    assert!(drawn.sections_in_view((0, -1), &behind).is_empty(), "the draw's frustum keeps it");
    assert!(view.sections_in_view((0, -1), &behind).contains(8), "the section just behind is not rebuilt");
    let far_behind = [(0, 8, -2)];
    assert!(view.sections_in_view((0, -2), &far_behind).is_empty(), "two sections behind is rebuilt");
}

/// The moved-back frustum holds the camera's 8-block cell along the whole trajectory of the draw
/// test (zoomed views and looks straight up and down included), as the game requires.
#[test]
fn meshing_view_holds_the_camera_cell() {
    use glam::Vec3;
    for f in 0..400u32 {
        let (eye, yaw, pitch) = camera(f);
        // Looks straight down and straight up now and then.
        let pitch = match f % 50 {
            0 => 90.0,
            1 => -90.0,
            _ => pitch,
        };
        let (origin, _) = planes(eye, yaw, pitch, 12.0 * 16.0);
        let view = meshing_view(eye, yaw, pitch, 12.0 * 16.0);
        let local = |v: f64, i: usize| (v - [f64::from(origin[0]), 0.0, f64::from(origin[1])][i]) as f32;
        let corner = |round: fn(f64) -> f64| Vec3::from(std::array::from_fn(|i| local(round(eye[i] / 8.0) * 8.0, i)));
        assert!(aabb_inside(&view.planes, corner(f64::floor), corner(f64::ceil)), "frame {f}: cell not held");
    }
}

/// Micro-bench of the meshing's view (`cargo test --release -- --ignored meshing_view_bench
/// --nocapture`): sections in view of every reached column, distance 12 and 32, one core, with the
/// per-section test and with the section search; two alternating passes, the second counts.
#[test]
#[ignore]
fn meshing_view_bench() {
    for r in [12, 32] {
        let mut rng = Lcg(SEED);
        let (_, reached) = world(&mut rng, r, 5);
        let columns: Vec<&[(i32, i32, i32)]> = reached.chunk_by(|a, b| (a.0, a.2) == (b.0, b.2)).collect();
        let frames = 200u32;
        let paths =
            [("per section", ListPaths::OLD), ("section search", ListPaths { section_search: true, ..ListPaths::OLD })];
        for pass in 0..2 {
            for (name, paths) in paths {
                let mut seen = 0usize;
                let t = std::time::Instant::now();
                for f in 0..frames {
                    let eye = [0.5, 70.0, 0.5];
                    let view = meshing_view(eye, f as f32 * 0.9, 10.0, r as f32 * 16.0);
                    for secs in &columns {
                        let s = view.sections_in_view_with((secs[0].0, secs[0].2), secs, paths);
                        seen += s.count(SECTIONS as usize);
                    }
                }
                let ns = t.elapsed().as_secs_f64() * 1e9 / f64::from(frames) / columns.len() as f64;
                if pass == 1 {
                    println!(
                        "meshing view distance {r}, {name}: {ns:.1} ns per column ({} columns, {seen} sections seen)",
                        columns.len()
                    );
                }
            }
        }
    }
}

/// Grid of reached sections = set of the same sections, over the whole box and beyond.
#[test]
fn reach_grid_matches_the_set() {
    let mut rng = Lcg(SEED ^ 3);
    let mut reached = Vec::new();
    for r in [0, 1, 5, 12] {
        for density in [0, 1, 4, 8] {
            reach(&mut rng, r, density, &mut reached);
            // Offset: grid not centred on 0.
            let shift = (rng.below(100) as i32 - 50, rng.below(4) as i32 - 2);
            reached.iter_mut().for_each(|s| (s.0, s.2) = (s.0 + shift.0, s.2 + shift.1));
            let set: hashbrown::HashSet<_> = reached.iter().copied().collect();
            let grid = ReachGrid::new(&reached);
            assert_eq!(grid.is_empty(), set.is_empty());
            let mut hits = 0;
            for x in shift.0 - r - 2..=shift.0 + r + 2 {
                for z in shift.1 - r - 2..=shift.1 + r + 2 {
                    for y in -2..SECTIONS + 2 {
                        assert_eq!(grid.contains(x, y, z), set.contains(&(x, y, z)), "({x}, {y}, {z})");
                        hits += usize::from(set.contains(&(x, y, z)));
                    }
                }
            }
            assert_eq!(hits, set.len());
            for far in [i32::MIN, i32::MAX] {
                assert!(!grid.contains(far, 0, 0) && !grid.contains(0, far, 0) && !grid.contains(0, 0, far));
            }
        }
    }
}

/// Paths of the micro-bench.
fn bench_paths() -> Vec<Compared> {
    let cached = ListPaths { cached_order: true, ..ListPaths::OLD };
    let v: Vec<Compared> = vec![
        ("old".into(), ListPaths::OLD),
        ("cached order (previous default)".into(), cached),
        ("+ command cache".into(), ListPaths { column_commands: true, ..cached }),
        ("+ hierarchical frustum".into(), ListPaths { hierarchical_frustum: true, ..cached }),
        ("+ plane coherence".into(), ListPaths { plane_coherence: true, ..cached }),
        ("+ section search".into(), ListPaths { section_search: true, ..cached }),
        ("+ buffer reuse".into(), ListPaths { reuse_buffers: true, ..cached }),
        ("all but the cache".into(), ListPaths { column_commands: false, ..ALL_SERIAL }),
        ("all but the hierarchy".into(), ListPaths { hierarchical_frustum: false, ..ALL_SERIAL }),
        ("all but the coherence".into(), ListPaths { plane_coherence: false, ..ALL_SERIAL }),
        ("all but the search".into(), ListPaths { section_search: false, ..ALL_SERIAL }),
        ("all but the buffers".into(), ListPaths { reuse_buffers: false, ..ALL_SERIAL }),
        ("all".into(), ALL_SERIAL),
    ];
    v
}

/// Bench world of radius `r` like the game's: ground 7 to 11 sections high, air above (drawing
/// nothing), 7 sections out of 8 reached (the walk crosses the open air).
fn world_with_air(rng: &mut Lcg, r: i32) -> World {
    let (mut cols, reached) = world(rng, r, 7);
    for (&(x, z), c) in cols.iter_mut() {
        with_air(c, 7 + (x * 7 + z * 13).rem_euclid(5) as usize, None);
    }
    (cols, reached)
}

/// Camera motion of the micro-bench: blocks per frame, pause between two frames (the pool's
/// threads fall asleep as between two frames of the game).
struct Motion {
    name: &'static str,
    speed: f64,
    pause: Option<std::time::Duration>,
}

const MOTIONS: [Motion; 4] = [
    Motion { name: "rotation", speed: 0.0, pause: None },
    Motion { name: "rotation, 1 ms between frames", speed: 0.0, pause: Some(std::time::Duration::from_millis(1)) },
    Motion { name: "walk", speed: 0.07, pause: None },
    Motion { name: "fast flight", speed: 1.1, pause: None },
];

/// Builds of `frames` frames of `motion` through `world` (radius `r`); returns µs per build and
/// the columns reused and rebuilt by the cache.
fn bench_motion(
    world: &World,
    r: i32,
    frames: u32,
    motion: &Motion,
    path: &Compared,
) -> (f64, u64, u64) {
    let (cols, reached) = world;
    let (_, p) = path;
    let mut l = Lists::default();
    let (mut reused, mut rebuilt, mut spent) = (0, 0, std::time::Duration::ZERO);
    for f in 0..frames {
        let s = f64::from(f) * motion.speed;
        let eye = [0.5 + s, 70.0, 0.5 + s * 0.3];
        let (origin, pl) = planes(eye, f as f32 * 0.9, 10.0, r as f32 * 16.0);
        let input = ListIn {
            columns: cols,
            reached,
            planes: &pl,
            origin,
            eye,
            min_y: MIN_Y,
            sections: SECTIONS,
            face_vertices: 4,
            split_draws: false,
            sections_generation: 1,
            columns_rev: 1,
        };
        if let Some(pause) = motion.pause {
            std::thread::sleep(pause);
        }
        let t = std::time::Instant::now();
        std::hint::black_box(build(&input, &mut l, *p, || {}));
        spent += t.elapsed();
        let (a, b) = l.column_command_reuse();
        (reused, rebuilt) = (reused + a, rebuilt + b);
    }
    (spent.as_secs_f64() * 1e6 / f64::from(frames), reused, rebuilt)
}

/// Share of the column selection in a build (`cargo test --release -p rc-app --bin rustcraft
/// lists_phase_bench -- --ignored --nocapture`): world with ground and air, rotation in place,
/// distance 32, every serial path on; one core.
#[test]
#[ignore]
fn lists_phase_bench() {
    let (cols, reached) = world_with_air(&mut Lcg(SEED), 32);
    for paths in [ALL_SERIAL, ListPaths { section_search: false, ..ALL_SERIAL }] {
        let (mut whole, mut selection) = (Lists::default(), Lists::default());
        let (mut t_whole, mut t_selection) = (std::time::Duration::ZERO, std::time::Duration::ZERO);
        for f in 0..400u32 {
            let eye = [0.5, 70.0, 0.5];
            let (origin, pl) = planes(eye, f as f32 * 0.9, 10.0, 32.0 * 16.0);
            let input = ListIn {
                columns: &cols,
                reached: &reached,
                planes: &pl,
                origin,
                eye,
                min_y: MIN_Y,
                sections: SECTIONS,
                face_vertices: 4,
                split_draws: false,
                sections_generation: 1,
                columns_rev: 1,
            };
            let t = std::time::Instant::now();
            std::hint::black_box(build(&input, &mut whole, paths, || {}));
            t_whole += t.elapsed();
            let t = std::time::Instant::now();
            std::hint::black_box(select_visible(&input, &mut selection, paths).len());
            t_selection += t.elapsed();
        }
        let us = |d: std::time::Duration| d.as_secs_f64() * 1e6 / 400.0;
        let (columns, bytes) = whole.command_cache_size();
        println!(
            "section search {}: build {:.1} µs/frame, of which column selection {:.1} µs; cache {columns} columns, {} KB",
            paths.section_search,
            us(t_whole),
            us(t_selection),
            bytes / 1024
        );
    }
}

/// Micro-bench (`cargo test --release -p rc-app --bin rustcraft lists_bench -- --ignored
/// --nocapture`): construction only, in the world of the previous series (sections reached at
/// random) and in a world with ground and air like the game's; rotation in place (fixed walk and
/// origin), the same with a pause between frames, walking (origin moving now and then) and fast
/// flight (origin changing on every frame); each path of [`bench_paths`], at distance 12, 32 and
/// 64. Two alternating passes, the second counts.
#[test]
#[ignore]
fn lists_bench() {
    let paths = bench_paths();
    for (r, frames) in [(12, 400u32), (32, 120), (64, 30)] {
        let worlds =
            [("random sections", world(&mut Lcg(SEED), r, 5)), ("ground and air", world_with_air(&mut Lcg(SEED), r))];
        for (world_name, world) in &worlds {
            for motion in &MOTIONS {
                let mut res = vec![(0.0f64, 0u64, 0u64); paths.len()];
                for pass in 0..2 {
                    for (i, path) in paths.iter().enumerate() {
                        let measured = bench_motion(world, r, frames, motion, path);
                        if pass == 1 {
                            res[i] = measured;
                        }
                    }
                }
                let old = res[0].0;
                for ((name, ..), (us, reused, rebuilt)) in paths.iter().zip(&res) {
                    let cache = if reused + rebuilt > 0 {
                        format!(
                            ", {:.0}% of the columns from the cache",
                            100.0 * *reused as f64 / (reused + rebuilt) as f64
                        )
                    } else {
                        String::new()
                    };
                    let against = (us / old - 1.0) * 100.0;
                    println!(
                        "lists {world_name} distance {r} {}: {name}: {us:.1} µs/frame ({against:+.0}% against old{cache})",
                        motion.name
                    );
                }
            }
        }
    }
    // Reached sections: set rebuilt on the main thread against grid.
    for r in [12, 32, 64] {
        let mut rng = Lcg(SEED);
        let mut reached = Vec::new();
        reach(&mut rng, r, 5, &mut reached);
        let n = 20;
        let t = std::time::Instant::now();
        for _ in 0..n {
            std::hint::black_box(reached.iter().copied().collect::<hashbrown::HashSet<_>>());
        }
        let set_us = t.elapsed().as_secs_f64() * 1e6 / f64::from(n);
        let t = std::time::Instant::now();
        for _ in 0..n {
            std::hint::black_box(ReachGrid::new(&reached));
        }
        let grid_us = t.elapsed().as_secs_f64() * 1e6 / f64::from(n);
        let set: hashbrown::HashSet<_> = reached.iter().copied().collect();
        let grid = ReachGrid::new(&reached);
        let q: Vec<(i32, i32, i32)> = (0..200_000)
            .map(|_| {
                (rng.below(2 * r as u32 + 1) as i32 - r, rng.below(24) as i32, rng.below(2 * r as u32 + 1) as i32 - r)
            })
            .collect();
        let t = std::time::Instant::now();
        let a = q.iter().filter(|p| set.contains(*p)).count();
        let set_q = t.elapsed().as_secs_f64() * 1e9 / q.len() as f64;
        let t = std::time::Instant::now();
        let b = q.iter().filter(|p| grid.contains(p.0, p.1, p.2)).count();
        let grid_q = t.elapsed().as_secs_f64() * 1e9 / q.len() as f64;
        assert_eq!(a, b);
        println!(
            "reached distance {r} ({} sections): set construction {set_us:.0} µs, grid {grid_us:.0} µs; query {set_q:.1} ns against {grid_q:.1} ns",
            reached.len()
        );
    }
}
