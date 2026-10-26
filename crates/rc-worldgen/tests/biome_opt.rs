//! Equivalence of the optimised paths of the biome tree (`TreePaths`) and of the memoised zoom
//! (`ZoomMemo`) against the old code, and micro-benches
//! (`cargo test --release -p rc-worldgen --test biome_opt -- --ignored --nocapture`).
//! Each checker is also run on a sabotaged version, which must fail.

mod common;

use std::time::Instant;

use rayon::prelude::*;
use rc_worldgen::biome::{BiomeSource, ClimateList, Searcher, SplitPaths, TreePaths, ZoomMemo, quantize, zoom};
use rc_worldgen::{Ctx, Generator, Volume};

/// Small generator for the test draws (splitmix64).
struct Mix(u64);

impl Mix {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + self.below((hi - lo + 1) as u64) as i64
    }
}

/// The 16 combinations of paths.
fn all_paths() -> Vec<TreePaths> {
    (0..16u32)
        .map(|b| TreePaths { early: b & 1 != 0, soa: b & 2 != 0, simd: b & 4 != 0, memo: b & 8 != 0 })
        .collect()
}

/// Synthetic list: boxes and points (depth 0 or 1 like the surface biomes), including
/// box duplicates under another biome (distance ties), varied offsets.
fn synthetic_list(r: &mut Mix, n: usize) -> ClimateList {
    let mut entries = Vec::with_capacity(n);
    for i in 0..n {
        if i > 8 && r.below(6) == 0 {
            let (s, _): &([(i64, i64); 7], String) = &entries[r.below(i as u64) as usize];
            let s = *s;
            entries.push((s, format!("b{}", r.below(40))));
            continue;
        }
        let mut s = [(0i64, 0i64); 7];
        for (d, p) in s.iter_mut().enumerate().take(6) {
            *p = if d == 4 && r.below(2) == 0 {
                let v = r.below(2) as i64 * 10_000;
                (v, v)
            } else {
                let a = r.range(-12_000, 12_000);
                let w = if r.below(4) == 0 { 0 } else { r.range(0, 8_000) };
                (a, (a + w).min(20_000))
            };
        }
        let off = [0, 0, 0, 1_000, 2_500, 3_750][r.below(6) as usize];
        s[6] = (off, off);
        entries.push((s, format!("b{}", r.below(40))));
    }
    ClimateList::new(entries)
}

/// Targets: random, built on the bounds (ties, zero distance, ±1), extremes
/// (edge of the no-overflow regime, 32 bits, 64 bits), and repeated (memo).
fn targets(list: &ClimateList, r: &mut Mix, n: usize) -> Vec<[i64; 6]> {
    let e = &list.entries;
    let mut extremes = vec![
        0,
        1 << 29,
        (1 << 29) + 1,
        -(1 << 29),
        -(1 << 29) - 1,
        1 << 30,
        -(1 << 30),
        i64::from(i32::MAX),
        i64::from(i32::MIN),
    ];
    // i64 overflow: wrapped in release (like the game), panics in debug in the old code.
    if !cfg!(debug_assertions) {
        extremes.extend([1 << 40, -(1 << 40), i64::MAX, i64::MIN, i64::MAX - 1, i64::MIN + 1, 3_037_000_500]);
    }
    let mut out: Vec<[i64; 6]> = Vec::with_capacity(n);
    while out.len() < n {
        let t: [i64; 6] = match r.below(10) {
            0..=3 => [0; 6].map(|_| r.range(-15_000, 15_000)),
            4..=7 => {
                let (s, _) = &e[r.below(e.len() as u64) as usize];
                let (s2, _) = &e[r.below(e.len() as u64) as usize];
                let mut t = [0i64; 6];
                for d in 0..6 {
                    let p = if r.below(4) == 0 { s2[d] } else { s[d] };
                    t[d] = match r.below(6) {
                        0 => p.0,
                        1 => p.1,
                        2 => p.0 - 1,
                        3 => p.1 + 1,
                        4 => (p.0 + p.1) / 2,
                        _ => r.range(p.0, p.1),
                    };
                }
                t
            }
            8 => {
                let mut t = [0; 6].map(|_| r.range(-15_000, 15_000));
                let d = r.below(6) as usize;
                t[d] = extremes[r.below(extremes.len() as u64) as usize];
                t
            }
            _ => match out.last() {
                Some(l) => *l,
                None => [0; 6],
            },
        };
        out.push(t);
    }
    out
}

/// Sequence of the entries returned by each path, shared `Searcher` in order; first difference.
fn check_paths(
    list: &ClimateList,
    ts: &[[i64; 6]],
    paths: &[TreePaths],
    f: impl Fn(&ClimateList, [i64; 6], &mut Searcher, TreePaths) -> usize + Sync,
) -> Option<String> {
    let mut s = Searcher::default();
    let want: Vec<usize> = ts.iter().map(|t| list.find_entry_with(*t, &mut s, TreePaths::OLD)).collect();
    paths.par_iter().find_map_any(|p| {
        let mut s = Searcher::default();
        ts.iter().zip(&want).enumerate().find_map(|(i, (t, w))| {
            let got = f(list, *t, &mut s, *p);
            (got != *w).then(|| format!("{p:?} target #{i} {t:?}: entry {got} instead of {w}"))
        })
    })
}

fn lists() -> Vec<(String, ClimateList)> {
    let mut r = Mix(11);
    let mut out = vec![
        ("synthetic 600".to_owned(), synthetic_list(&mut r, 600)),
        ("synthetic 7000".to_owned(), synthetic_list(&mut r, 7000)),
    ];
    if let Some(pack) = common::data_pack() {
        for preset in ["overworld", "nether"] {
            out.push((preset.to_owned(), ClimateList::from_report(&pack, preset).unwrap()));
        }
    }
    out
}

#[test]
fn tree_paths_match_old() {
    for (name, list) in lists() {
        let ts = targets(&list, &mut Mix(name.len() as u64), 1_000_000);
        let diff = check_paths(&list, &ts, &all_paths(), |l, t, s, p| l.find_entry_with(t, s, p));
        assert!(diff.is_none(), "{name} : {}", diff.unwrap());
    }
}

#[test]
fn tree_checker_detects_sabotage() {
    for (name, list) in lists() {
        let ts = targets(&list, &mut Mix(name.len() as u64 + 1), 200_000);
        for p in all_paths().into_iter().filter(|p| !p.memo) {
            let diff = check_paths(&list, &ts, &[p], |l, t, s, p| l.find_entry_sabotaged(t, s, p));
            assert!(diff.is_some(), "{name} {p:?}: sabotage (≥) not detected");
        }
    }
}

/// Flattened tree against exhaustive search: minimal distance reached, and same biome
/// when a single biome is at minimal distance.
#[test]
fn flat_tree_agrees_with_brute_force() {
    let all = TreePaths { early: true, soa: true, simd: true, memo: true };
    for (name, list) in lists() {
        let ts = targets(&list, &mut Mix(name.len() as u64 + 2), 20_000);
        let mut s = Searcher::default();
        for t in ts.iter().filter(|t| t.iter().all(|v| v.unsigned_abs() < 1 << 30)) {
            let e = list.find_entry_with(*t, &mut s, all);
            let best = (0..list.entries.len()).map(|i| list.distance(i, *t)).min().unwrap();
            assert_eq!(list.distance(e, *t), best, "{name} {t:?}");
            if list.tied_biomes(*t) == 1 {
                assert_eq!(list.entries[e].1, list.find_brute_force(*t), "{name} {t:?}");
            }
        }
    }
}

/// Real targets of a chunk, in filling order (sections, x, y, z).
fn chunk_targets(g: &Generator, cx: i32, cz: i32) -> Vec<[i64; 6]> {
    let min_qy = g.settings.min_y >> 2;
    let hq = g.settings.height >> 2;
    let v = Volume::stepped(4, hq, 4, cx * 16, min_qy << 2, cz * 16, 4, 4, 4);
    let mut ctx = Ctx::cached();
    let r = g.router;
    let bufs: Vec<Vec<f32>> = [r.temperature, r.vegetation, r.continents, r.erosion, r.depth, r.ridges]
        .iter()
        .map(|s| g.volume(*s, &mut ctx, &v))
        .collect();
    let mut out = Vec::with_capacity((hq * 16) as usize);
    for sec in 0..hq / 4 {
        for x in 0..4 {
            for y in 0..4 {
                for z in 0..4 {
                    let i = v.index(x, sec * 4 + y, z);
                    out.push([0, 1, 2, 3, 4, 5].map(|k| quantize(bufs[k][i])));
                }
            }
        }
    }
    out
}

fn real_targets(n_side: i32) -> Option<(ClimateList, Vec<Vec<[i64; 6]>>)> {
    let pack = common::data_pack()?;
    let g = Generator::new(&pack, "overworld", 0).unwrap();
    let (src, _) = BiomeSource::from_preset(&pack, "normal", "overworld").unwrap();
    let BiomeSource::MultiNoise(list) = src else { panic!("multi-noise source expected") };
    let chunks: Vec<(i32, i32)> =
        (0..n_side).flat_map(|x| (0..n_side).map(move |z| (x * 3 - n_side, z * 3 - n_side))).collect();
    let ts = chunks.par_iter().map(|&(cx, cz)| chunk_targets(&g, cx, cz)).collect();
    Some((list, ts))
}

/// Real climates of 64 chunks, one `Searcher` per chunk like the filling.
#[test]
fn tree_paths_match_old_on_real_chunks() {
    let Some((list, chunks)) = real_targets(8) else { return };
    for ts in &chunks {
        let diff = check_paths(&list, ts, &all_paths(), |l, t, s, p| l.find_entry_with(t, s, p));
        assert!(diff.is_none(), "{}", diff.unwrap());
    }
}

// ---------------------------------------------------------------------------------------------
// two phases (`RUSTCRAFT_OPT_BTREE_SPLIT`)

/// Helps of the stateless phase: none, neighbour's bound + 1, neighbour candidate.
fn modes() -> [SplitPaths; 3] {
    let m = |hint, cand| SplitPaths { split: true, hint, cand };
    [m(false, false), m(true, false), m(true, true)]
}

fn split_paths() -> [TreePaths; 2] {
    [TreePaths::OLD, TreePaths { early: true, soa: true, simd: true, memo: false }]
}

/// Chained sequence of the old code against the two phases (with and without neighbour bound);
/// first difference.
fn check_split(
    list: &ClimateList,
    ts: &[[i64; 6]],
    f: impl Fn(&ClimateList, &[[i64; 6]], &mut Searcher, SplitPaths, TreePaths) -> Vec<usize>,
) -> Option<String> {
    let mut s = Searcher::default();
    let want: Vec<usize> = ts.iter().map(|t| list.find_entry_with(*t, &mut s, TreePaths::OLD)).collect();
    for p in split_paths() {
        for m in modes() {
            let got = f(list, ts, &mut Searcher::default(), m, p);
            assert_eq!(got.len(), want.len());
            if let Some(i) = (0..want.len()).find(|&i| got[i] != want[i]) {
                return Some(format!("{p:?} {m:?} target #{i} {:?}: entry {} instead of {}", ts[i], got[i], want[i]));
            }
        }
    }
    None
}

#[test]
fn split_matches_chained() {
    for (name, list) in lists() {
        let ts = targets(&list, &mut Mix(name.len() as u64 + 7), 1_000_000);
        let diff = check_split(&list, &ts, |l, ts, s, h, p| l.find_entries_split(ts, s, h, p));
        assert!(diff.is_none(), "{name} : {}", diff.unwrap());
    }
}

#[test]
fn split_matches_chained_on_real_chunks() {
    // The 1.57 million climates of the bench (1,024 chunks, seed 0), in a single sequence.
    let Some((list, chunks)) = real_targets(32) else { return };
    let ts: Vec<[i64; 6]> = chunks.concat();
    assert_eq!(ts.len(), 1_572_864);
    let diff = check_split(&list, &ts, |l, ts, s, h, p| l.find_entries_split(ts, s, h, p));
    assert!(diff.is_none(), "{}", diff.unwrap());
    let diff = check_split(&list, &ts, |l, ts, s, h, p| l.find_entries_split_sabotaged(ts, s, h, p, 1));
    assert!(diff.is_some(), "sabotage (≥) not detected on the real climates");
}

#[test]
fn split_checker_detects_sabotage() {
    for (name, list) in lists() {
        let ts = targets(&list, &mut Mix(name.len() as u64 + 8), 200_000);
        let diff = check_split(&list, &ts, |l, ts, s, h, p| l.find_entries_split_sabotaged(ts, s, h, p, 1));
        assert!(diff.is_some(), "{name}: sabotage (≥) of the stateless phase not detected");
    }
}

/// Last leaf rule: ignoring it changes nothing on the synthetic list of 600
/// entries nor on the 200 real searches (measured on 24/10). Constructed case: B (far from the centre, hence after A in the tree)
/// kept alone, then a target in A ∩ B: the chained search keeps B, F(t) is A.
#[test]
fn split_keeps_last_leaf_on_ties() {
    let mut a = [(-100i64, 100i64); 7];
    let mut b = [(0i64, 1000i64); 7];
    a[6] = (0, 0);
    b[6] = (0, 0);
    let list = ClimateList::new(vec![(a, "a".to_owned()), (b, "b".to_owned())]);
    let ts = vec![[500; 6], [50; 6], [50; 6], [-50; 6], [50; 6]];
    let mut s = Searcher::default();
    let want: Vec<usize> = ts.iter().map(|t| list.find_entry_with(*t, &mut s, TreePaths::OLD)).collect();
    assert_eq!(want, [1, 1, 1, 0, 0]);
    let diff = check_split(&list, &ts, |l, ts, s, h, p| l.find_entries_split(ts, s, h, p));
    assert!(diff.is_none(), "{}", diff.unwrap());
    let diff = check_split(&list, &ts, |l, ts, s, h, p| l.find_entries_split_sabotaged(ts, s, h, p, 2));
    assert!(diff.is_some(), "sabotage (last leaf ignored) not detected");
}

/// Points of a biome search around `o` (columns of step 32 over a square of side
/// 2r + 1, 7 heights), in quarts.
fn locate_points(o: (i32, i32), r: i32) -> (usize, impl Fn(usize) -> [i32; 3] + Sync) {
    const YS: [i32; 7] = [64, 0, 128, -64, 192, 256, 316];
    let side = (2 * r + 1) as usize;
    let at = move |i: usize| {
        let c = i / YS.len();
        let (dx, dz) = ((c % side) as i32 - r, (c / side) as i32 - r);
        [(o.0 + dx * 32) >> 2, YS[i % YS.len()] >> 2, (o.1 + dz * 32) >> 2]
    };
    (side * side * YS.len(), at)
}

type Locate = dyn Fn(&BiomeSource, &Generator, (i32, i32), &[bool], &mut Searcher) -> Option<(usize, u16)>;

/// 200 chained searches (same `Searcher`, like successive commands): biome of a
/// point drawn at random in the radius, and one in ten without an accepted biome (full walk).
fn check_locate(f: &Locate) -> Option<Option<String>> {
    let pack = common::data_pack()?;
    let g = Generator::new(&pack, "overworld", 0).unwrap();
    let (src, _) = BiomeSource::from_preset(&pack, "normal", "overworld").unwrap();
    Some(check_locate_on(&src, &g, f))
}

/// Like [`check_locate`], on a given source and generator.
fn check_locate_on(src: &BiomeSource, g: &Generator, f: &Locate) -> Option<String> {
    let nb = src.names().len();
    let mut r = Mix(21);
    let cases: Vec<((i32, i32), Vec<bool>)> = (0..200)
        .map(|k| {
            let o = (r.range(-20_000, 20_000) as i32, r.range(-20_000, 20_000) as i32);
            let mut ok = vec![false; nb];
            if k % 10 != 0 {
                let (x, z) = (o.0 + r.range(-380, 380) as i32, o.1 + r.range(-380, 380) as i32);
                let b = src.biome_at(g, &mut Ctx::cached(), &mut Searcher::default(), x >> 2, 16, z >> 2);
                ok[usize::from(b)] = true;
            }
            (o, ok)
        })
        .collect();
    let (mut a, mut b) = (Searcher::default(), Searcher::default());
    cases.iter().enumerate().find_map(|(k, (o, ok))| {
        let (n, at) = locate_points(*o, 12);
        let want = src.first_match(g, n, &at, ok, &mut a, SplitPaths::OLD);
        let got = f(src, g, *o, ok, &mut b);
        (got != want).then(|| format!("search #{k} around {o:?}: {got:?} instead of {want:?}"))
    })
}

#[test]
fn locate_split_matches_serial() {
    for m in modes() {
        let f = move |src: &BiomeSource, g: &Generator, o, ok: &[bool], s: &mut Searcher| {
            let (n, at) = locate_points(o, 12);
            src.first_match(g, n, at, ok, s, m)
        };
        let Some(d) = check_locate(&f) else { return };
        assert!(d.is_none(), "{m:?} : {}", d.unwrap());
    }
    // Sabotage: batches read back out of order (points of a task permuted). Ties between
    // leaves, for their part, never change the returned biome on these 200 searches (measured on
    // 24/10): their sabotage is checked at the level of the leaves (`split_checker_*`).
    let sab = |src: &BiomeSource, g: &Generator, o, ok: &[bool], s: &mut Searcher| {
        let (n, at) = locate_points(o, 12);
        let run = rc_worldgen::biome::SPLIT_RUN;
        let swapped = |i: usize| {
            let j = i ^ 1;
            if i / run == j / run && j < n { at(j) } else { at(i) }
        };
        src.first_match(g, n, swapped, ok, s, modes()[2])
    };
    let d = check_locate(&sab).unwrap();
    assert!(d.is_some(), "sabotaged search (points permuted within a batch) not detected");
}

/// Branch of the sources without last leaf (End, checkerboard): same 200 searches, 4,375 points
/// per walk (neither a multiple of 64 nor of 64 × threads: partial last batch and last task),
/// in reserves of 1, 3 and all the threads (batches of 64, 192, 64 × threads points). Checkerboard of 40
/// biomes in cells of 4 columns: the accepted biome often only appears after several batches.
#[test]
fn locate_split_matches_serial_stateless_sources() {
    let Some(pack) = common::data_pack() else { return };
    let (end, _) = BiomeSource::from_preset(&pack, "normal", "the_end").unwrap();
    let g_end = Generator::new(&pack, "end", 0).unwrap();
    let names: Vec<String> = (0..40).map(|k| format!("minecraft:b{k}")).collect();
    let checker = BiomeSource::checkerboard(&names, 3).unwrap();
    let g_over = Generator::new(&pack, "overworld", 0).unwrap();
    let split = |m: SplitPaths| {
        move |src: &BiomeSource, g: &Generator, o, ok: &[bool], s: &mut Searcher| {
            let (n, at) = locate_points(o, 12);
            src.first_match(g, n, at, ok, s, m)
        }
    };
    let sab = |src: &BiomeSource, g: &Generator, o, ok: &[bool], s: &mut Searcher| {
        let (n, at) = locate_points(o, 12);
        let run = rc_worldgen::biome::SPLIT_RUN;
        let swapped = |i: usize| {
            let j = i ^ 1;
            if i / run == j / run && j < n { at(j) } else { at(i) }
        };
        src.first_match(g, n, swapped, ok, s, modes()[2])
    };
    let all = std::thread::available_parallelism().map_or(4, |n| n.get());
    for threads in [1, 3, all] {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
        pool.install(|| {
            for (name, src, g) in [("the_end", &end, &g_end), ("checkerboard", &checker, &g_over)] {
                for m in modes() {
                    let d = check_locate_on(src, g, &split(m));
                    assert!(d.is_none(), "{name} {threads} threads {m:?}: {}", d.unwrap());
                }
                assert!(check_locate_on(src, g, &sab).is_some(), "{name} {threads} threads: sabotage not detected");
            }
        });
    }
}

// ---------------------------------------------------------------------------------------------
// zoom

/// Positions: columns walked from top to bottom, positions scattered around a chunk,
/// neighbourhood of i32::MIN / MAX, arbitrary values.
fn zoom_positions(r: &mut Mix, n: usize) -> Vec<(i32, i32, i32)> {
    let mut out = Vec::with_capacity(n);
    let edge = |r: &mut Mix| {
        let k = r.range(0, 40) as i32;
        if r.below(2) == 0 { i32::MIN.wrapping_add(k) } else { i32::MAX.wrapping_sub(k) }
    };
    while out.len() < n {
        match r.below(4) {
            0 => {
                let (x, z) = (r.range(-3000, 3000) as i32, r.range(-3000, 3000) as i32);
                for y in (-64..320).rev() {
                    out.push((x, y, z));
                }
            }
            1 => {
                let (cx, cz) = (r.range(-200, 200) as i32 * 16, r.range(-200, 200) as i32 * 16);
                for _ in 0..200 {
                    out.push((cx + r.range(-16, 32) as i32, r.range(-64, 320) as i32, cz + r.range(-16, 32) as i32));
                }
            }
            2 => {
                let mut c = [0; 3].map(|_| r.range(-100, 100) as i32);
                c[r.below(3) as usize] = edge(r);
                if r.below(2) == 0 {
                    c[r.below(3) as usize] = edge(r);
                }
                for y in 0..8 {
                    out.push((c[0], c[1].wrapping_sub(y), c[2]));
                }
            }
            _ => out.push((r.next() as i32, r.next() as i32, r.next() as i32)),
        }
    }
    out
}

fn check_zoom(seed: i64, ps: &[(i32, i32, i32)], sabotaged: bool) -> Option<String> {
    let mut m = ZoomMemo::new(seed);
    ps.iter().find_map(|&(x, y, z)| {
        let got = if sabotaged { m.zoom_sabotaged(x, y, z) } else { m.zoom(x, y, z) };
        let want = zoom(seed, x, y, z);
        (got != want).then(|| format!("seed {seed} block {x},{y},{z}: {got:?} instead of {want:?}"))
    })
}

#[test]
fn zoom_memo_matches_direct() {
    let mut r = Mix(5);
    let ps = zoom_positions(&mut r, 1_000_000);
    for seed in [0i64, -6_214_586_474_519_478_921, i64::MIN, i64::MAX] {
        let d = check_zoom(seed, &ps, false);
        assert!(d.is_none(), "{}", d.unwrap());
        assert!(check_zoom(seed, &ps, true).is_some(), "seed {seed}: sabotaged memo not detected");
    }
}

/// Memoised zoom against the Java probe (reference outside the repository).
#[test]
fn zoom_memo_matches_game() {
    let Some(lines) = common::read_lines("zoom.txt") else { return };
    let mut memo: Option<Box<ZoomMemo>> = None;
    for l in &lines {
        let v: Vec<i64> = l.split(' ').map(|s| s.parse().unwrap()).collect();
        let seed = rc_worldgen::rng::biome_zoom_seed(v[0]);
        let m = match &mut memo {
            Some(m) if m.seed() == seed => m,
            _ => memo.insert(ZoomMemo::new(seed)),
        };
        let q = m.zoom(v[1] as i32, v[2] as i32, v[3] as i32);
        assert_eq!((i64::from(q.0), i64::from(q.1), i64::from(q.2)), (v[4], v[5], v[6]), "{l}");
    }
    assert!(lines.len() > 4000);
}

// ---------------------------------------------------------------------------------------------
// micro-benches

fn time<T>(label: &str, reps: u32, mut f: impl FnMut() -> T) -> f64 {
    let _ = std::hint::black_box(f());
    let mut best = f64::MAX;
    for _ in 0..reps {
        let t = Instant::now();
        std::hint::black_box(f());
        best = best.min(t.elapsed().as_secs_f64());
    }
    eprintln!("{label}: {:.3} ms (best of {reps})", best * 1e3);
    best
}

fn named() -> Vec<(&'static str, TreePaths)> {
    let p = |early, soa, simd, memo| TreePaths { early, soa, simd, memo };
    vec![
        ("old", TreePaths::OLD),
        ("early", p(true, false, false, false)),
        ("soa", p(false, true, false, false)),
        ("soa+simd", p(false, true, true, false)),
        ("memo", p(false, false, false, true)),
        ("early+soa", p(true, true, false, false)),
        ("early+soa+simd", p(true, true, true, false)),
        ("all", p(true, true, true, true)),
    ]
}

/// `find` on the climates of 1,024 real chunks (≈ 1.6 million searches), 1 thread then
/// powers of 2 up to the number of cores; rate of early exit and of memo.
#[test]
#[ignore = "benchmark"]
fn bench_tree() {
    let Some((list, chunks)) = real_targets(32) else { return };
    let n: usize = chunks.iter().map(Vec::len).sum();
    let (mut zero, mut repeat) = (0usize, 0usize);
    for ts in &chunks {
        let mut s = Searcher::default();
        let mut prev: Option<([i64; 6], usize)> = None;
        for t in ts {
            if let Some((pt, pe)) = prev {
                zero += usize::from(list.distance(pe, *t) == 0);
                repeat += usize::from(pt == *t);
            }
            prev = Some((*t, list.find_entry_with(*t, &mut s, TreePaths::OLD)));
        }
    }
    eprintln!("{n} searches: last leaf at zero distance {zero}, target identical to the previous one {repeat}");
    let run = |p: TreePaths| {
        chunks
            .iter()
            .map(|ts| {
                let mut s = Searcher::default();
                ts.iter().map(|t| list.find_entry_with(*t, &mut s, p)).sum::<usize>()
            })
            .sum::<usize>()
    };
    eprintln!("-- 1 thread");
    for (name, p) in named() {
        let dt = time(name, 5, || run(p));
        eprintln!("   {name}: {:.1} ns / search", dt * 1e9 / n as f64);
    }
    let max = std::thread::available_parallelism().map_or(1, |n| n.get());
    for (name, p) in [named()[0], named()[6]] {
        let mut t = 1;
        while t <= max {
            let pool = rayon::ThreadPoolBuilder::new().num_threads(t).build().unwrap();
            let par = || {
                chunks
                    .par_iter()
                    .map(|ts| {
                        let mut s = Searcher::default();
                        ts.iter().map(|t| list.find_entry_with(*t, &mut s, p)).sum::<usize>()
                    })
                    .sum::<usize>()
            };
            pool.install(|| time(&format!("{name}, {t} threads"), 5, par));
            t *= 2;
        }
    }
}

/// Leaves only: chained sequence against two phases (without bound, with neighbour bound) on the
/// climates of 1,024 chunks, 1 thread then powers of 2 up to the number of cores.
#[test]
#[ignore = "benchmark"]
fn bench_split_leaves() {
    let Some((list, chunks)) = real_targets(32) else { return };
    let ts: Vec<[i64; 6]> = chunks.concat();
    let p = TreePaths::current();
    eprintln!("{} targets, paths {p:?}", ts.len());
    time("chained", 5, || {
        let mut s = Searcher::default();
        ts.iter().map(|t| list.find_entry_with(*t, &mut s, p)).sum::<usize>()
    });
    let max = std::thread::available_parallelism().map_or(1, |n| n.get());
    for m in modes() {
        let mut t = 1;
        while t <= max {
            let pool = rayon::ThreadPoolBuilder::new().num_threads(t).build().unwrap();
            pool.install(|| {
                time(&format!("two phases, help {}/{}, {t} threads", m.hint, m.cand), 5, || {
                    list.find_entries_split(&ts, &mut Searcher::default(), m, p).iter().sum::<usize>()
                })
            });
            t *= 2;
        }
    }
}

/// `/locate biome` without an accepted biome (full walk, like a rare biome): square of radius
/// 50 columns × 7 heights (71,407 points), series against two phases.
#[test]
#[ignore = "benchmark"]
fn bench_locate() {
    let Some(pack) = common::data_pack() else { return };
    let g = Generator::new(&pack, "overworld", 0).unwrap();
    let (src, _) = BiomeSource::from_preset(&pack, "normal", "overworld").unwrap();
    let ok = vec![false; src.names().len()];
    let (n, at) = locate_points((0, 0), 50);
    let run = |p: SplitPaths| src.first_match(&g, n, &at, &ok, &mut Searcher::default(), p);
    time("series", 3, || run(SplitPaths::OLD));
    let max = std::thread::available_parallelism().map_or(1, |n| n.get());
    for m in modes() {
        let mut t = 1;
        while t <= max {
            let pool = rayon::ThreadPoolBuilder::new().num_threads(t).build().unwrap();
            pool.install(|| time(&format!("two phases, help {}/{}, {t} threads", m.hint, m.cand), 3, || run(m)));
            t *= 2;
        }
    }
}

/// Biome filling of 1,024 chunks (`chunk`, paths from the environment): share of the
/// search in the total.
#[test]
#[ignore = "benchmark"]
fn bench_chunk_biomes() {
    let Some(pack) = common::data_pack() else { return };
    let g = Generator::new(&pack, "overworld", 0).unwrap();
    let (src, _) = BiomeSource::from_preset(&pack, "normal", "overworld").unwrap();
    let chunks: Vec<(i32, i32)> = (0..32).flat_map(|x| (0..32).map(move |z| (x * 3 - 32, z * 3 - 32))).collect();
    eprintln!("paths: {:?}, zoom memo {}", TreePaths::current(), rc_worldgen::opt::zoom_memo());
    time("chunk 1,024 chunks, 1 thread", 3, || {
        chunks.iter().map(|&(x, z)| src.chunk(&g, x, z, &mut Searcher::default()).data[0]).sum::<u16>()
    });
}

/// Zoom: columns of 384 blocks walked from top to bottom (materials) and positions scattered
/// around a chunk (decoration), direct against memoised.
#[test]
#[ignore = "benchmark"]
fn bench_zoom() {
    let seed = rc_worldgen::rng::biome_zoom_seed(0);
    let mut r = Mix(9);
    let cols: Vec<(i32, i32)> = (0..4096).map(|_| (r.range(-5000, 5000) as i32, r.range(-5000, 5000) as i32)).collect();
    let scattered: Vec<(i32, i32, i32)> = (0..1_572_864i32)
        .map(|i| {
            let (cx, cz) = (i / 4096 * 16, i / 4096 * 32);
            (cx + r.range(-8, 24) as i32, r.range(-64, 320) as i32, cz + r.range(-8, 24) as i32)
        })
        .collect();
    let n = cols.len() * 384;
    let sum = |q: (i32, i32, i32)| q.0 ^ q.1 ^ q.2;
    let a = time("columns, direct", 5, || {
        cols.iter().map(|&(x, z)| (-64..320).rev().map(|y| sum(zoom(seed, x, y, z))).fold(0, i32::wrapping_add)).fold(0, i32::wrapping_add)
    });
    let b = time("columns, memo", 5, || {
        let mut m = ZoomMemo::new(seed);
        cols.iter().map(|&(x, z)| (-64..320).rev().map(|y| sum(m.zoom(x, y, z))).fold(0, i32::wrapping_add)).fold(0, i32::wrapping_add)
    });
    let b2 = time("columns, thread memo", 5, || {
        cols.iter()
            .map(|&(x, z)| (-64..320).rev().map(|y| sum(rc_worldgen::biome::zoom_cached(seed, x, y, z))).fold(0, i32::wrapping_add))
            .fold(0, i32::wrapping_add)
    });
    eprintln!(
        "   columns: {:.1} → {:.1} ns / zoom (thread memo {:.1})",
        a * 1e9 / n as f64,
        b * 1e9 / n as f64,
        b2 * 1e9 / n as f64
    );
    let c = time("scattered, direct", 5, || scattered.iter().map(|&(x, y, z)| sum(zoom(seed, x, y, z))).fold(0, i32::wrapping_add));
    let d = time("scattered, memo", 5, || {
        let mut m = ZoomMemo::new(seed);
        scattered.iter().map(|&(x, y, z)| sum(m.zoom(x, y, z))).fold(0, i32::wrapping_add)
    });
    let e = time("scattered, thread memo", 5, || {
        scattered.iter().map(|&(x, y, z)| sum(rc_worldgen::biome::zoom_cached(seed, x, y, z))).fold(0, i32::wrapping_add)
    });
    let k = scattered.len() as f64;
    eprintln!("   scattered: {:.1} → {:.1} ns / zoom (thread memo {:.1})", c * 1e9 / k, d * 1e9 / k, e * 1e9 / k);
}
