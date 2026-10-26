//! Bench of the light relight after block changes: replay of traces (integrated server,
//! `RUSTCRAFT_LIGHTTRACE`) and of a synthetic suite on the columns of the reference
//! capture. Each replay is first compared, relight by relight, to the frozen copy
//! (`rc_light::reference`): same touched sections, same sheets byte for byte.
//!
//! `cargo bench -p rc-light --bench relight`; `--features memstats` for the allocations;
//! `RUSTCRAFT_OPT_REQUIRE=1`: capture, data or traces absent, or unknown data = failure.

mod common;

use common::*;
use criterion::{Criterion, criterion_group, criterion_main};
use rc_light::{LightTables, Touched};

/// Recognised data: (input fingerprint, replay output fingerprint).
const KNOWN: &[(u64, u64)] = &[
    // Synthetic suite v1 on the capture 1791720243814.rccap (400 relights).
    (0xd4ba_5d3b_15e9_0cc6, 0xd57f_8c67_cb3d_83dc),
    // bench_ticks_surface.rclt: `rc-sim` `tests_opt::bench_ticks` (sample world, surface sites,
    // warm-up included) recorded with RUSTCRAFT_LIGHTTRACE on 22/10/2026 (292 MB, 409 relights;
    // columns written: box of each group of changes at ±16).
    (0xade2_f476_f05b_bf2e, 0x4424_c5d6_1f07_c04c),
    // bench_ticks_nether.rclt: same bench, RUSTCRAFT_LIGHTTRACE_DIM=minecraft:the_nether
    // (39 MB, 100 relights, block light only).
    (0xdda0_8952_58eb_62ee, 0xac19_e329_f760_ded7),
];

/// Version of the synthetic suite generator (changes its input fingerprint).
const SYNTH_VERSION: u64 = 1;

type Relight = fn(&mut MapGrid, i32, &LightTables, bool, &[[i32; 3]]) -> Touched;

fn work(g:&mut MapGrid, h: i32, t: &LightTables, sky: bool, c: &[[i32; 3]]) -> Touched {
    rc_light::relight_sections(g, h, t, sky, c)
}

fn frozen(g: &mut MapGrid, h: i32, t: &LightTables, sky: bool, c: &[[i32; 3]]) -> Touched {
    rc_light::reference::relight_sections(g, h, t, sky, c)
}

/// Incremental relight (`LIGHTINC`) forced, with no group size limit.
fn inc(g: &mut MapGrid, h: i32, t: &LightTables, sky: bool, c: &[[i32; 3]]) -> Touched {
    rc_light::relight_sections_inc(g, h, t, sky, c, Some(usize::MAX))
}

/// Incremental for groups of at most 16 changes, area beyond (`LIGHTINCMAX=16`).
fn inc16(g: &mut MapGrid, h: i32, t: &LightTables, sky: bool, c: &[[i32; 3]]) -> Touched {
    rc_light::relight_sections_inc(g, h, t, sky, c, Some(16))
}

/// Relights where `f` differs from the frozen copy at equal input (each relight starts from the state
/// left by the frozen copy): difference of the incremental on inconsistent sheets (protocol A3).
fn gaps(tr: &Trace, t: &LightTables, f: Relight) -> (usize, usize) {
    let mut g = MapGrid::default();
    let (mut n, mut cells) = (0, 0);
    for s in &tr.steps {
        match s {
            Step::Column(k, c) => {
                g.0.entry(*k).or_insert_with(|| (**c).clone());
            }
            Step::Event(e) => {
                let pos = apply_event(&mut g, tr, e);
                let mut h = g.clone();
                let ta = frozen(&mut g, tr.height, t, tr.has_sky, &pos);
                let tb = f(&mut h, tr.height, t, tr.has_sky, &pos);
                let mut diff = 0;
                for (k, (_, sa, ba)) in &g.0 {
                    let (_, sb, bb) = &h.0[k];
                    for (la, lb) in [(sa, sb), (ba, bb)] {
                        for (x, y) in la.iter().zip(lb) {
                            if let (Some(x), Some(y)) = (x, y)
                                && x != y
                            {
                                diff += (0..4096).filter(|&i| (x[i >> 1] ^ y[i >> 1]) >> ((i & 1) * 4) & 15 != 0).count();
                            }
                        }
                    }
                }
                if ta != tb || diff > 0 {
                    n += 1;
                    cells += diff;
                }
            }
        }
    }
    (n, cells)
}

fn state(d: &rc_data::VersionData, name: &str) -> u32 {
    let short = name.trim_start_matches("minecraft:");
    d.blocks
        .iter()
        .find(|b| b.name == name || b.name == short)
        .unwrap_or_else(|| panic!("block {name} absent from the data"))
        .default_state
}

/// Synthetic suite on the capture: 200 "place then restore" pairs at random
/// (fixed seed) on the inner columns: torch placed on the ground, stone roof (shadow),
/// three-block hole open to the sky, buried glowing stone, row of six leaves.
fn synthetic(cap: &Capture, d: &rc_data::VersionData) -> Trace {
    // Air, cave air, void air.
    let air = |s: u32| {
        d.states.get(s as usize).and_then(|i| d.blocks.get(i.block as usize)).is_none_or(|b| b.name.ends_with("air"))
    };
    let (torch, stone, glow, leaves) = (
        state(d, "minecraft:torch"),
        state(d, "minecraft:stone"),
        state(d, "minecraft:glowstone"),
        state(d, "minecraft:oak_leaves"),
    );
    let mut keys: Vec<(i32, i32)> = cap.cols.keys().copied().collect();
    keys.sort_unstable();
    let mut steps = Vec::new();
    for k in &keys {
        let (s, sky, block) = &cap.cols[k];
        let (mut sky, mut block) = (sky.clone(), block.clone());
        fill_implied(&mut sky, &mut block);
        steps.push(Step::Column(*k, Box::new((s.clone(), sky, block))));
    }
    let inner: Vec<(i32, i32)> = keys
        .iter()
        .copied()
        .filter(|&(x, z)| (-1..=1).all(|dx| (-1..=1).all(|dz| cap.cols.contains_key(&(x + dx, z + dz)))))
        .collect();
    let at = |x: i32, y: i32, z: i32| -> u32 {
        cap.cols
            .get(&(x >> 4, z >> 4))
            .and_then(|(s, _, _)| s.get((y >> 4) as usize))
            .map_or(0, |s| s.blocks.get(rc_world::block_index((x & 15) as usize, (y & 15) as usize, (z & 15) as usize)))
    };
    let mut seed = 0x5eed_1157_u64;
    let mut rnd = |n: u64| {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (seed >> 33) % n
    };
    let h = cap.height;
    assert!(!inner.is_empty(), "capture without a column surrounded by its 8 neighbours");
    for _ in 0..200 {
        let (cx, cz) = inner[rnd(inner.len() as u64) as usize];
        let (x, z) = (cx * 16 + rnd(16) as i32, cz * 16 + rnd(16) as i32);
        let surface = (0..h).rev().find(|&y| !air(at(x, y, z))).unwrap_or(0);
        let ps: Vec<([i32; 3], u32)> = match rnd(5) {
            0 => vec![([x, surface + 1, z], torch)],
            1 => vec![([x, surface + 4, z], stone)],
            2 => (0..3).map(|i| ([x, surface - i, z], 0)).collect(),
            3 => vec![([x, (surface - 12).max(0), z], glow)],
            _ => (0..6).map(|i| ([x + i, surface + 2, z], leaves)).collect(),
        };
        let ps: Vec<_> = ps.into_iter().filter(|(p, _)| (0..h).contains(&p[1])).collect();
        if ps.is_empty() {
            continue;
        }
        let back: Event = ps.iter().map(|&(p, _)| (p, at(p[0], p[1], p[2]))).collect();
        steps.push(Step::Event(ps));
        steps.push(Step::Event(back));
    }
    let mut dg = Fnv(cap.digest);
    dg.u64(SYNTH_VERSION);
    Trace { name: "synthetic".into(), digest: dg.0, height: h, has_sky: true, direct_bits: cap.direct_bits, steps }
}

fn bench(c: &mut Criterion) {
    let Some(d) = need(rc_testdata::version_data(), "26.3 data cache") else { return };
    let t = LightTables::new(&d);
    let mut list = Vec::new();
    if let Some(cap) = need(capture(&d), "reference capture") {
        list.push(synthetic(&cap, &d));
    }
    let found = traces();
    if found.is_empty() {
        let what = "relight traces (RUSTCRAFT_LIGHTTRACE, %LOCALAPPDATA%\\rustcraft-re\\traces\\lumiere)";
        assert!(!required(), "{what} absent (RUSTCRAFT_OPT_REQUIRE=1)");
        eprintln!("{what} absent: synthetic suite only");
    }
    list.extend(found);
    let mut g = c.benchmark_group("relight");
    g.sample_size(10);
    for tr in &list {
        // Equivalence first: working path = frozen copy, relight by relight.
        use std::sync::atomic::Ordering::Relaxed;
        let before = rc_light::stats::totals();
        let (ia, ib) = (IN_ALLOCS.load(Relaxed), IN_BYTES.load(Relaxed));
        let (ga, ta, da) = replay(tr, &t, work);
        let (allocs_a, bytes_a) = (IN_ALLOCS.load(Relaxed) - ia, IN_BYTES.load(Relaxed) - ib);
        let st = rc_light::stats::totals().since(&before);
        let (ia, ib) = (IN_ALLOCS.load(Relaxed), IN_BYTES.load(Relaxed));
        let (gb, tb, db) = replay(tr, &t, frozen);
        let (allocs_b, bytes_b) = (IN_ALLOCS.load(Relaxed) - ia, IN_BYTES.load(Relaxed) - ib);
        assert_eq!(ta.len(), tb.len());
        for (i, (a, b)) in ta.iter().zip(&tb).enumerate() {
            assert_eq!(a, b, "{}: touched sections of relight {i}", tr.name);
        }
        for (k, (_, sa, ba)) in &ga.0 {
            let (_, sb, bb) = &gb.0[k];
            assert!(sa == sb && ba == bb, "{}: sheets of column {k:?} differ from the reference", tr.name);
        }
        check_known(&tr.name, tr.digest, digest(&ga, &ta), KNOWN);
        let n = tr.events().max(1) as f64;
        eprintln!(
            "{}: {} relights, {} changes; work {:.3} ms/relight, reference {:.3} ms/relight",
            tr.name,
            tr.events(),
            tr.changes(),
            da.as_secs_f64() * 1e3 / n,
            db.as_secs_f64() * 1e3 / n
        );
        eprintln!(
            "{}: sky resolutions {} ({:.3} ms each), block {} ({:.3} ms each)",
            tr.name,
            st.sky_solves,
            st.sky_ns as f64 / 1e6 / st.sky_solves.max(1) as f64,
            st.block_solves,
            st.block_ns as f64 / 1e6 / st.block_solves.max(1) as f64
        );
        if cfg!(feature = "memstats") {
            eprintln!(
                "{}: allocations per relight work {:.1} / {:.0} KiB, reference {:.1} / {:.0} KiB",
                tr.name,
                allocs_a as f64 / n,
                bytes_a as f64 / n / 1024.0,
                allocs_b as f64 / n,
                bytes_b as f64 / n / 1024.0
            );
        }
        drop((ga, gb, ta, tb));
        for (label, f) in [("LIGHTINC", inc as Relight), ("LIGHTINCMAX=16", inc16)] {
            let (n, cells) = gaps(tr, &t, f);
            eprintln!("{}: {label} differs from the reference on {n} relights at equal input ({cells} cells)", tr.name);
        }
        // Order A B B A: each path runs once before and once after the other, and two
        // passes of the same path give the bench's own spread (order, frequency, cache).
        // A gain is judged against the old working path (main), never against the frozen
        // copy: it has no counters nor `bounded` parameter and its machine code differs.
        // `inc` and `inc16`: incremental relight (C2-M14), same order mirrored.
        for (label, f) in [
            ("work_a", work as Relight),
            ("inc_a", inc),
            ("inc16_a", inc16),
            ("reference_a", frozen),
            ("reference_b", frozen),
            ("inc16_b", inc16),
            ("inc_b", inc),
            ("work_b", work),
        ] {
            g.bench_function(format!("{}_{label}_1t", tr.name), |b| {
                b.iter_custom(|iters| (0..iters).map(|_| replay(tr, &t, f).2).sum())
            });
        }
    }
    g.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
