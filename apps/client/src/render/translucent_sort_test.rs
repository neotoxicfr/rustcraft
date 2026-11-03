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
fn pov_resorts_follow_the_written_rule() {
    // Column (5, 5), sections 4 (y 0..16) and 10 (y 96..112) translucent; bottom of the world −64.
    let starts = [[0u32; 5].as_slice(), &[3; 6], &[9; 14]].concat();
    let (col, sy0) = ((5, 5), -4);
    let mut povs = column_povs(None, &starts, col, sy0);
    let mut out = Vec::new();
    let mut step = |eye: [f64; 3], moved: bool| {
        pov_resorts(&mut povs, &starts, eye, col, sy0, moved, &mut out);
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
    let mut mine: Vec<Vec<u8>> = layouts.iter().zip(&cols).map(|(st, &c)| column_povs(None, st, c, sy0)).collect();
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
            pov_resorts(&mut mine[i], &layouts[i], eye, c, sy0, moved, &mut out);
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
fn pov_picks_scan_near_then_window() {
    let keys: Vec<(i32, i32)> = (0..40).flat_map(|x| (0..5).map(move |z| (x, z))).collect();
    let (picks, next) = pov_picks(&keys, 190, (0, 0));
    let picks: Vec<_> = picks.collect();
    assert_eq!(&picks[..25], (-2..=2).flat_map(|x| (-2..=2).map(move |z| (x, z))).collect::<Vec<_>>());
    let window: Vec<_> = (0..25).map(|i| keys[(190 + i) % 200]).filter(|k| k.0 > 2 || k.1 > 2).collect();
    assert_eq!(picks[25..], window);
    assert_eq!(next, 15);
    let few = [(9, 9), (0, 0)];
    let (p, next) = pov_picks(&few, 1, (0, 0));
    assert_eq!(p.skip(25).collect::<Vec<_>>(), [(9, 9)]);
    assert_eq!(next, 1);
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
