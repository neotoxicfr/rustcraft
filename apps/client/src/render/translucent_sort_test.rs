//! Proofs on the order of the translucent quads: the ways of sorting a section, the viewpoint rule that
//! decides which sections are sorted again, and the test of the free order.

use super::super::test_meshes::flat;
use super::*;
use rc_mesh::fluid::FluidQuad;

/// Free order: a double-face quad counts as its two earlier quads.
#[test]
fn order_free_counts_double_faces_as_both() {
    let lake: Vec<FluidQuad> = (0..6).map(|i| flat(i as f32, 0.0, 14.875, true)).collect();
    let old: Vec<FluidQuad> = lake.iter().flat_map(|q| [*q, rc_mesh::fluid::back_face(q)]).collect();
    let new: Vec<FluidQuad> =
        lake.iter().map(|q| FluidQuad { texture: q.texture | rc_mesh::fluid::DOUBLE_FACE, ..*q }).collect();
    assert_eq!(order_free(&old, &[0]), order_free(&new, &[0]));
    assert!(order_free(&new, &[0]));
    // Two double faces in the same cell: not free, as before.
    let twice = [new[0], new[0]];
    let old_twice: Vec<FluidQuad> =
        [lake[0], lake[0]].iter().flat_map(|q| [*q, rc_mesh::fluid::back_face(q)]).collect();
    assert_eq!(order_free(&old_twice, &[0]), order_free(&twice, &[0]));
    // A double face and a lone back face in the same cell: not free.
    assert!(!order_free(&[new[0], rc_mesh::fluid::back_face(&lake[0])], &[0]));
}

/// Test generator with a fixed seed (independent of the clock).
fn lcg(s: &mut u64) -> u64 {
    *s = s.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
    *s >> 33
}

/// Centres on a half-block grid (many equal distances) and camera, with a fixed seed.
fn trans_case(s: &mut u64, n: usize) -> (Vec<[f32; 3]>, [f32; 3]) {
    let mut v = || (lcg(s) % 32) as f32 * 0.5;
    let centers: Vec<[f32; 3]> = (0..n).map(|_| [v(), v(), v()]).collect();
    let eye = [v(), v(), v()];
    (centers, eye)
}

/// Order of a section by each method of `RUSTCRAFT_OPT_TRANSKEY`.
fn section_by(centers: &[[f32; 3]], eye: [f32; 3], how: TransKey) -> Vec<u32> {
    let mut order: Vec<u32> = (0..centers.len() as u32).collect();
    sort_section(centers, &mut order, eye, how, &mut SortScratch::default());
    order
}

/// `RUSTCRAFT_OPT_TRANSKEY`: distances computed once (pairs or integer keys) give, bit for bit,
/// the order of the original stable sort, ties included (sections long enough for the
/// unstable sort to move them), non-finite cameras included.
#[test]
fn trans_key_orders_match_stable_sort() {
    let mut s = 0x5eed_1606u64;
    let mut ties = 0;
    for case in 0..300 {
        let n = [0, 1, 2, 7, 33, 150, 600][case % 7];
        let (centers, mut eye) = trans_case(&mut s, n);
        match case % 50 {
            1 => eye = [f32::NAN, 0.0, 0.0],
            2 => eye = [f32::INFINITY, 1.0, -f32::INFINITY],
            3 => eye = [1e30, -1e30, 0.0],
            _ => {}
        }
        let old = section_by(&centers, eye, TransKey::Old);
        let d =
            |i: u32| (0..3).map(|a| (centers[i as usize][a] - eye[a]) * (centers[i as usize][a] - eye[a])).sum::<f32>();
        ties += old.windows(2).filter(|w| d(w[0]) == d(w[1])).count();
        for how in [TransKey::Pairs, TransKey::Bits] {
            assert_eq!(section_by(&centers, eye, how), old, "case {case} ({n} quads, {how:?})");
        }
    }
    assert!(ties > 1000, "too few ties to test the stability ({ties})");
    // Whole column: sections split by `starts`, indices shifted by `base`.
    let (centers, eye) = trans_case(&mut s, 400);
    let starts = [0, 0, 120, 121, 400];
    let mut expect: Vec<u32> = Vec::new();
    for w in starts.windows(2) {
        let mut sec: Vec<u32> = (w[0]..w[1]).collect();
        sec.sort_by(|&a, &b| {
            let d = |i: u32| {
                (0..3).map(|k| (centers[i as usize][k] - eye[k]) * (centers[i as usize][k] - eye[k])).sum::<f32>()
            };
            d(b).total_cmp(&d(a))
        });
        expect.extend(sec.iter().map(|i| i + 77));
    }
    assert_eq!(translucent_order(&centers, &starts, Some(eye), 77), expect);
    assert_eq!(translucent_order(&centers, &starts, None, 3), (3..403).collect::<Vec<u32>>());
}

/// Cost of the sort of a section (old, pairs, integer keys), 1 core then all:
/// `cargo test --release -p rc-app trans_key_cost -- --ignored --nocapture`.
#[test]
#[ignore]
fn trans_key_cost() {
    use rayon::prelude::*;
    let mut s = 7u64;
    for n in [8, 64, 512, 4096] {
        let cases: Vec<_> = (0..(200_000 / n).max(50)).map(|_| trans_case(&mut s, n)).collect();
        for how in [TransKey::Old, TransKey::Pairs, TransKey::Bits] {
            for threads in [1, 4, rayon::current_num_threads()] {
                let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().expect("pool");
                let t = std::time::Instant::now();
                let k: u64 = pool.install(|| cases.par_iter().map(|(c, e)| u64::from(section_by(c, *e, how)[0])).sum());
                let ns = t.elapsed().as_secs_f64() * 1e9 / (cases.len() * n) as f64;
                eprintln!("sort {how:?}, sections of {n}: {ns:.1} ns per quad ({threads} threads, {k})");
            }
        }
    }
}

/// Re-sort rule of the game written separately (`docs/specs/visibility.md`): section offset bounded per
/// axis; re-sort if different from the last, or block changed and a zero component.
fn reference_resorts(
    last: &mut [Option<[i32; 3]>],
    starts: &[u32],
    eye: [f64; 3],
    col: (i32, i32),
    sy0: i32,
    moved: bool,
) -> Vec<usize> {
    let mut out = Vec::new();
    for s in 0..starts.len() - 1 {
        if starts[s] == starts[s + 1] {
            continue;
        }
        let rel = |e: f64, sec: i32| (e.floor() as i64).div_euclid(16) as i32 - sec;
        let p = [rel(eye[0], col.0), rel(eye[1], sy0 + s as i32), rel(eye[2], col.1)].map(|v| v.clamp(-1, 1));
        if last[s] != Some(p) || moved && p.contains(&0) {
            last[s] = Some(p);
            out.push(s);
        }
    }
    out
}

/// `RUSTCRAFT_OPT_TRANSPOV`: hand-written cases, then scripted camera path (fixed seed)
/// over several columns, sections re-sorted at each step == written rule.
#[test]
fn view_cell_resorts_follow_the_written_rule() {
    // Column (5, 5), sections 4 (y 0..16) and 10 (y 96..112) translucent; bottom of the world −64.
    let starts = [[0u32; 5].as_slice(), &[3; 6], &[9; 14]].concat();
    let (col, sy0) = ((5, 5), -4);
    let mut cells = column_view_cells(None, &starts, col, sy0);
    let mut out = Vec::new();
    let mut step = |eye: [f64; 3], moved: bool| {
        view_cell_resorts(&mut cells, &starts, camera_section(eye), col, sy0, moved, &mut out);
        out.clone()
    };
    assert_eq!(step([8.5, 100.5, 8.5], true), [4, 10], "never sorted");
    assert_eq!(step([9.5, 100.5, 8.5], true), [10], "block changed: only the section on the axis (y)");
    assert_eq!(step([9.7, 100.5, 8.5], false), [] as [usize; 0], "same block, same viewpoint");
    assert_eq!(step([10.5, 140.5, 8.5], true), [10], "above: viewpoint of 10 changed");
    assert_eq!(step([80.5, 140.5, 8.5], true), [4, 10], "within the x slab of the column");
    assert_eq!(step([81.5, 140.5, 8.5], true), [4, 10], "on the x axis: re-sorted at each block");
    assert_eq!(step([81.5, 140.5, 8.5], false), [] as [usize; 0]);
    assert_eq!(step([200.0, 300.0, 300.0], true), [4, 10]);
    assert_eq!(step([201.0, 300.0, 300.0], true), [] as [usize; 0], "far away, off axis: never");
    // Scripted path: flights, falls, section jumps, over 9 columns around the origin.
    let mut s = 0x1606_0001u64;
    let cols: Vec<(i32, i32)> = (-1..=1).flat_map(|x| (-1..=1).map(move |z| (x * 3, z * 2))).collect();
    let layouts: Vec<Vec<u32>> = cols
        .iter()
        .map(|_| {
            let mut at = 0;
            (0..=24)
                .map(|_| {
                    let next = at + u32::from(lcg(&mut s).is_multiple_of(3)) * 4;
                    std::mem::replace(&mut at, next)
                })
                .collect()
        })
        .collect();
    let mut mine: Vec<Vec<u8>> =
        layouts.iter().zip(&cols).map(|(st, &c)| column_view_cells(None, st, c, sy0)).collect();
    let mut theirs: Vec<Vec<Option<[i32; 3]>>> = layouts.iter().map(|st| vec![None; st.len() - 1]).collect();
    let (mut eye, mut last_block) = ([0.5f64, 70.0, 0.5], None);
    let mut resorted = 0;
    for t in 0..2000 {
        let r = |s: &mut u64, k: f64| (lcg(s) % 2001) as f64 / 1000.0 * k - k;
        let k = if t % 97 == 0 { 40.0 } else { 0.7 };
        eye = [eye[0] + r(&mut s, k), (eye[1] + r(&mut s, k)).clamp(-80.0, 330.0), eye[2] + r(&mut s, k)];
        let block = eye.map(|v| v.floor() as i64);
        let moved = last_block != Some(block);
        last_block = Some(block);
        for (i, &c) in cols.iter().enumerate() {
            view_cell_resorts(&mut mine[i], &layouts[i], camera_section(eye), c, sy0, moved, &mut out);
            let want = reference_resorts(&mut theirs[i], &layouts[i], eye, c, sy0, moved);
            assert_eq!(out, want, "step {t}, column {c:?}, camera {eye:?}");
            resorted += out.len();
        }
    }
    assert!(resorted > 1000, "path too poor ({resorted} re-sorts)");
}

/// Columns examined: the 25 nearby first, then the rotation without them, which resumes where it
/// stopped and covers max(n / 8, 15) keys.
#[test]
fn sweep_picks_scan_near_then_window() {
    let keys: Vec<(i32, i32)> = (0..40).flat_map(|x| (0..5).map(move |z| (x, z))).collect();
    let near = near_cells((0, 0));
    let (picks, next) = sweep_picks(&near, &keys, 190, (0, 0));
    let picks: Vec<_> = picks.collect();
    assert_eq!(&picks[..25], (-2..=2).flat_map(|x| (-2..=2).map(move |z| (x, z))).collect::<Vec<_>>());
    let window: Vec<_> = (0..25).map(|i| keys[(190 + i) % 200]).filter(|k| k.0 > 2 || k.1 > 2).collect();
    assert_eq!(picks[25..], window);
    assert_eq!(next, 15);
    let few = [(9, 9), (0, 0)];
    let (p, next) = sweep_picks(&near, &few, 1, (0, 0));
    assert_eq!(p.skip(25).collect::<Vec<_>>(), [(9, 9)]);
    assert_eq!(next, 1);
}

/// `RUSTCRAFT_OPT_TRANSLUCENT_NEAR_LIST`: the nearby columns found in the sorted list are the 25
/// cells around the camera that the list holds, in the same order; random lists and cameras
/// (fixed seed), the camera's column inside and outside the list's span.
#[test]
fn near_list_holds_the_listed_cells_in_their_order() {
    let mut s = 0x0ea5_1157u64;
    let mut out = Vec::new();
    let mut found = 0;
    for _ in 0..500 {
        let mut keys: Vec<(i32, i32)> =
            (0..lcg(&mut s) % 120).map(|_| ((lcg(&mut s) % 13) as i32 - 6, (lcg(&mut s) % 13) as i32 - 6)).collect();
        keys.sort_unstable();
        keys.dedup();
        let cam = ((lcg(&mut s) % 17) as i32 - 8, (lcg(&mut s) % 17) as i32 - 8);
        near_in_list(&keys, cam, &mut out);
        let want: Vec<(i32, i32)> = near_cells(cam).into_iter().filter(|k| keys.binary_search(k).is_ok()).collect();
        assert_eq!(out, want, "camera {cam:?}, list {keys:?}");
        found += out.len();
    }
    assert!(found > 2000, "too few nearby columns to test anything ({found})");
}

/// A translucent column of the scripted worlds: its sections (24, bottom of the world −64), centres
/// and place in the order buffer.
#[derive(Clone)]
struct ColumnSpec {
    key: (i32, i32),
    starts: Vec<u32>,
    centers: Vec<[f32; 3]>,
    /// First place in the order buffer, which is also the first translucent quad.
    at: u32,
}

/// Column with 24 sections, a third of them translucent (a few quads each), a quarter of the
/// columns free of any sort (no centre kept, like a lake surface).
fn column_spec(s: &mut u64, key: (i32, i32), at: u32) -> ColumnSpec {
    let mut starts = vec![0u32];
    for _ in 0..24 {
        let quads = if lcg(s).is_multiple_of(3) { 1 + (lcg(s) % 6) as u32 } else { 0 };
        starts.push(starts.last().unwrap() + quads);
    }
    let free = lcg(s).is_multiple_of(4);
    let n = *starts.last().unwrap() as usize;
    let mut v = |k: u64| (lcg(s) % k) as f32 * 0.5;
    let centers = if free { Vec::new() } else { (0..n).map(|_| [v(32), v(768), v(32)]).collect() };
    ColumnSpec { key, starts, centers, at }
}

/// A renderer's columns and translucent re-sort, without the GPU.
struct SweepWorld {
    columns: HashMap<(i32, i32), ColumnGpu>,
    sweep: TranslucentSweep,
    columns_rev: u64,
    sort_eye: Option<[f64; 3]>,
    sort_min_y: i32,
}

impl SweepWorld {
    fn new(paths: SweepPaths) -> Self {
        SweepWorld {
            columns: HashMap::new(),
            sweep: TranslucentSweep::new(paths),
            columns_rev: 0,
            sort_eye: None,
            sort_min_y: 0,
        }
    }

    /// What the upload does for the re-sort: view cells of the last frame's camera, noted for its
    /// view epoch, and the list told.
    fn upload(&mut self, spec: &ColumnSpec) {
        let old = self.columns.remove(&spec.key);
        let old_sorted = old.filter(|o| !o.trans_centers.is_empty()).map(|o| o.view_epoch);
        let view_epoch = self.sweep.view_epoch();
        let sorted = !spec.centers.is_empty();
        let view_cells = if sorted {
            column_view_cells(self.sort_eye.map(camera_section), &spec.starts, spec.key, self.sort_min_y.div_euclid(16))
        } else {
            Vec::new()
        };
        let n = spec.starts.last().copied().unwrap_or(0);
        let mut fluid_ranges: [std::ops::Range<u32>; 3] = Default::default();
        fluid_ranges[2] = spec.at..spec.at + n;
        let column = ColumnGpu {
            trans_centers: spec.centers.clone(),
            trans_starts: spec.starts.clone(),
            trans_alloc: (spec.at, n),
            order_alloc: (spec.at, n),
            fluid_ranges,
            view_cells,
            view_epoch,
            ..ColumnGpu::default()
        };
        self.sweep.column_replaced(spec.key, old_sorted, sorted.then_some(view_epoch));
        self.columns.insert(spec.key, column);
        self.columns_rev += 1;
    }

    fn remove(&mut self, key: (i32, i32)) {
        if let Some(c) = self.columns.remove(&key) {
            self.sweep.column_replaced(key, (!c.trans_centers.is_empty()).then_some(c.view_epoch), None);
            self.columns_rev += 1;
        }
    }

    /// One frame: the runs written (column, place, order), the columns examined.
    fn frame(&mut self, camera: SweepCamera) -> (Vec<RunWritten>, usize) {
        self.sort_eye = Some(camera.eye);
        self.sort_min_y = camera.min_y;
        let mut writes = Vec::new();
        let examined = self.sweep.run(&mut self.columns, self.columns_rev, camera, |k, at, order| {
            writes.push((k, at, order.to_vec()));
        });
        (writes, examined)
    }

    /// View cells of every column, by key.
    fn view_cells(&self) -> Vec<((i32, i32), Vec<u8>)> {
        let mut cells: Vec<_> = self.columns.iter().map(|(&k, c)| (k, c.view_cells.clone())).collect();
        cells.sort_unstable();
        cells
    }

    /// The list and the count of columns not settled, against a rebuild from every column.
    fn assert_kept_state_matches_a_rebuild(&self, t: usize) {
        let mut rebuilt: Vec<(i32, i32)> =
            self.columns.iter().filter(|(_, c)| !c.trans_centers.is_empty()).map(|(&k, _)| k).collect();
        rebuilt.sort_unstable();
        assert_eq!(self.sweep.translucent_columns, rebuilt, "frame {t}: list");
        let epoch = self.sweep.view_epoch;
        let unsettled = rebuilt.iter().filter(|k| self.columns[*k].view_epoch != epoch).count();
        assert_eq!(self.sweep.unsettled, unsettled, "frame {t}: columns not settled");
    }
}

/// Camera of the scripted path: long stops, steps inside a block, block and section changes, jumps.
fn next_eye(s: &mut u64, t: usize, eye: [f64; 3]) -> [f64; 3] {
    let r = |s: &mut u64, k: f64| (lcg(s) % 2001) as f64 / 1000.0 * k - k;
    match t % 120 {
        // Still: the frames the skip is for.
        0..=49 => eye,
        // Within the block mostly, sometimes over its edge.
        50..=79 => [eye[0] + r(s, 0.2), eye[1], eye[2] + r(s, 0.2)],
        // Walking and flying: blocks and sections change.
        80..=118 => [eye[0] + r(s, 1.5), (eye[1] + r(s, 1.5)).clamp(-80.0, 330.0), eye[2] + r(s, 1.5)],
        // Jump.
        _ => [r(s, 40.0), (eye[1] + r(s, 60.0)).clamp(-80.0, 330.0), r(s, 40.0)],
    }
}

/// A scripted run: frames of camera path with uploads, removals, columns turning free and back, and
/// an improved transparency round trip; returns, per frame, the runs written, the rotation and the
/// view cells of every column, checking the kept list against a rebuild at every frame.
fn scripted_sweep(paths: SweepPaths, columns: usize, frames: usize) -> (Vec<SweepFrameLog>, usize) {
    let mut s = 0x5eed_c305u64;
    let side = (columns as f64).sqrt().ceil() as i32;
    let keys: Vec<(i32, i32)> = (0..columns as i32).map(|i| (i % side - side / 2, i / side - side / 2)).collect();
    let mut at = 0;
    let mut specs: Vec<ColumnSpec> = keys
        .iter()
        .map(|&k| {
            let spec = column_spec(&mut s, k, at);
            at += spec.starts.last().unwrap();
            spec
        })
        .collect();
    let mut world = SweepWorld::new(paths);
    for spec in &specs {
        world.upload(spec);
    }
    let (mut eye, mut log, mut examined) = ([3.5f64, 70.2, -2.5], Vec::new(), 0);
    for t in 0..frames {
        eye = next_eye(&mut s, t, eye);
        // Columns change in the middle of the path, while still and while walking.
        if t % 37 == 11 {
            let i = (lcg(&mut s) % specs.len() as u64) as usize;
            let mut spec = column_spec(&mut s, specs[i].key, at);
            at += spec.starts.last().unwrap();
            if t % 3 == 0 {
                spec.centers.clear();
            }
            specs[i] = spec;
            world.upload(&specs[i]);
        }
        if t % 53 == 20 {
            let i = (lcg(&mut s) % specs.len() as u64) as usize;
            world.remove(specs[i].key);
        }
        if t % 53 == 45 {
            let i = (lcg(&mut s) % specs.len() as u64) as usize;
            world.upload(&specs[i].clone());
        }
        let improved_transparency = (300..340).contains(&(t % 700));
        let cam_col = (eye[0].floor() as i32 >> 4, eye[2].floor() as i32 >> 4);
        let camera = SweepCamera { eye, min_y: -64, column: cam_col, improved_transparency };
        let (writes, n) = world.frame(camera);
        examined += n;
        world.assert_kept_state_matches_a_rebuild(t);
        log.push(SweepFrameLog { writes, cursor: world.sweep.cursor, view_cells: world.view_cells() });
    }
    (log, examined)
}

/// A re-sorted run sent to the order buffer: column, place, order.
type RunWritten = ((i32, i32), u32, Vec<u32>);

#[derive(Debug, PartialEq)]
struct SweepFrameLog {
    writes: Vec<RunWritten>,
    cursor: usize,
    view_cells: Vec<((i32, i32), Vec<u8>)>,
}

fn sweep_paths(skip_settled: bool, near_list: bool, incremental_list: bool) -> SweepPaths {
    SweepPaths { by_view_cell: true, skip_settled, near_list, incremental_list, sabotage: None }
}

/// First frame two scripted runs differ, if any.
fn first_difference(a: &[SweepFrameLog], b: &[SweepFrameLog]) -> Option<usize> {
    a.iter().zip(b).position(|(x, y)| x != y)
}

/// `RUSTCRAFT_OPT_TRANSLUCENT_SWEEP_SKIP`, `RUSTCRAFT_OPT_TRANSLUCENT_NEAR_LIST`,
/// `RUSTCRAFT_OPT_INCREMENTAL_TRANSLUCENT_KEYS`, alone and together: over a scripted camera path
/// (fixed seed, long stops, block and section changes, jumps, uploads, removals, columns turning
/// free, an improved transparency round trip) on 9 columns and on 60 (a rotation shorter than the
/// list), every frame writes the same runs, leaves the rotation at the same place and the same view
/// cells as the old path; the skip examines far fewer columns; skipping without moving the rotation
/// and an incremental list that forgets removals are seen.
#[test]
fn sweep_paths_resort_the_same_sections_frame_by_frame() {
    for columns in [9, 60] {
        let frames = 2100;
        let (reference, examined_before) = scripted_sweep(sweep_paths(false, false, false), columns, frames);
        let resorted: usize = reference.iter().map(|f| f.writes.len()).sum();
        assert!(resorted > 300, "{columns} columns: path too poor ({resorted} runs re-sorted)");
        for (skip, near, incremental) in
            [(true, false, false), (false, true, false), (false, false, true), (true, true, true)]
        {
            let (log, examined) = scripted_sweep(sweep_paths(skip, near, incremental), columns, frames);
            assert_eq!(
                first_difference(&reference, &log),
                None,
                "{columns} columns, skip {skip}, near list {near}, incremental {incremental}"
            );
            if skip {
                assert!(
                    examined * 2 < examined_before,
                    "{columns} columns: the skip examined {examined} columns, the old path {examined_before}"
                );
            }
        }
        // With 9 columns the rotation covers the whole list on every frame: a frozen cursor is the
        // same cursor there, so that sabotage is only looked for in the longer list.
        let sabotages: &[SweepSabotage] = if columns > 15 {
            &[SweepSabotage::SkipFreezesCursor, SweepSabotage::RemovalIgnored]
        } else {
            &[SweepSabotage::RemovalIgnored]
        };
        let sabotaged = |sabotage| SweepPaths { sabotage: Some(sabotage), ..sweep_paths(true, true, true) };
        for &sabotage in sabotages {
            let caught = std::panic::catch_unwind(|| {
                let (log, _) = scripted_sweep(sabotaged(sabotage), columns, frames);
                first_difference(&reference, &log)
            });
            assert!(!matches!(caught, Ok(None)), "{columns} columns: sabotage {sabotage:?} not seen");
        }
    }
}

/// Cost of the re-sort per frame for 200 and 2,000 translucent columns, camera still then walking,
/// with 0 or 1 column uploaded per frame, by path (1 core: it runs on the main thread, a few µs):
/// `cargo test --release -p rc-app translucent_sweep_cost -- --ignored --nocapture`.
#[test]
#[ignore]
fn translucent_sweep_cost() {
    let mut s = 0xbe9c_4a11_u64;
    for n in [200usize, 2000] {
        let side = (n as f64).sqrt().ceil() as i32;
        let mut at = 0;
        let specs: Vec<ColumnSpec> = (0..n as i32)
            .map(|i| {
                let mut spec = column_spec(&mut s, (i % side - side / 2, i / side - side / 2), at);
                if spec.centers.is_empty() {
                    let quads = *spec.starts.last().unwrap() as usize;
                    spec.centers = vec![[1.0, 70.0, 1.0]; quads];
                }
                at += spec.starts.last().unwrap();
                spec
            })
            .collect();
        for (walking, uploads) in [(false, 0), (true, 0), (false, 1), (true, 1)] {
            for (name, paths) in [
                ("old", sweep_paths(false, false, false)),
                ("skip", sweep_paths(true, false, false)),
                ("near list", sweep_paths(false, true, false)),
                ("incremental list", sweep_paths(false, false, true)),
                ("all", sweep_paths(true, true, true)),
            ] {
                let mut world = SweepWorld::new(paths);
                specs.iter().for_each(|spec| world.upload(spec));
                let frames = 4000;
                let mut eye = [0.5f64, 70.5, 0.5];
                let mut timed = std::time::Duration::ZERO;
                for t in 0..frames {
                    if walking && t % 4 == 0 {
                        eye[0] += 1.0;
                    }
                    for u in 0..uploads {
                        world.upload(&specs[(t * 7 + u) % n]);
                    }
                    let camera = SweepCamera {
                        eye,
                        min_y: -64,
                        column: (eye[0].floor() as i32 >> 4, eye[2].floor() as i32 >> 4),
                        improved_transparency: false,
                    };
                    let started = std::time::Instant::now();
                    world.sweep.run(&mut world.columns, world.columns_rev, camera, |_, _, order| {
                        std::hint::black_box(order);
                    });
                    timed += started.elapsed();
                }
                let walk = if walking { "walking" } else { "still" };
                eprintln!(
                    "{n} columns, {walk}, {uploads} upload per frame, {name}: {:.2} µs per frame",
                    timed.as_secs_f64() * 1e6 / frames as f64
                );
            }
        }
    }
}

/// Sort centre of a non-planar quad: middle of the 0–2 diagonal (the game), not the mean
/// of the four vertices (sloping water surface).
#[test]
fn quad_center_is_the_v0_v2_midpoint() {
    let p = [[0.0, 1.0, 0.0], [0.0, 0.5, 1.0], [1.0, 0.25, 1.0], [1.0, 0.875, 0.0]];
    assert_eq!(quad_center(&p), [0.5, 0.625, 0.5]);
    let mean: f32 = p.iter().map(|v| v[1]).sum::<f32>() / 4.0;
    assert_ne!(quad_center(&p)[1], mean);
}

#[test]
fn coplanar_surface_needs_no_sort() {
    // Surface of a lake with its underside faces: same plane, a single one per cell and direction.
    let q: Vec<_> =
        (0..4).flat_map(|i| [flat(i as f32, 0.0, 15.875, true), flat(i as f32, 0.0, 15.875, false)]).collect();
    assert!(order_free(&q, &[0]));
    // Two sections, each planar at its own height.
    let q = vec![
        flat(0.0, 0.0, 3.0, true),
        flat(1.0, 0.0, 3.0, true),
        flat(0.0, 0.0, 20.0, true),
        flat(1.0, 0.0, 20.0, true),
    ];
    assert!(order_free(&q, &[0, 2]));
}

#[test]
fn overlap_or_depth_needs_sort() {
    // Two different planes in a section.
    assert!(!order_free(&[flat(0.0, 0.0, 3.0, true), flat(1.0, 0.0, 4.0, true)], &[0]));
    // Same cell, same direction: overlap.
    assert!(!order_free(&[flat(0.0, 0.0, 3.0, true), flat(0.0, 0.0, 3.0, true)], &[0]));
    // Quad straddling two cells.
    let mut wide = flat(0.0, 0.0, 3.0, true);
    wide.corners[2][0] = 2.0;
    wide.corners[3][0] = 2.0;
    assert!(!order_free(&[wide, flat(5.0, 0.0, 3.0, true)], &[0]));
    // Sloping surface (flowing water).
    let mut slope = flat(0.0, 0.0, 3.0, true);
    slope.corners[0][1] = 2.5;
    assert!(!order_free(&[slope, flat(1.0, 0.0, 3.0, true)], &[0]));
}
