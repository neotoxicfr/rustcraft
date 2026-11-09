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
use rc_light::{IncrementalBench, LightTables, Touched};

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
    // bench_ticks_redstone.rclt: same bench, RUSTCRAFT_BENCH_SITES=redstone (128 observer clocks of
    // `perf:redstone`), recorded on 03/11/2026 (57 MB, 297 relights, 80,228 changes).
    (0xbe0e_0924_13d2_6c9d, 0x8836_ba9a_3e52_d5c2),
];

/// Version of the synthetic suite generator (changes its input fingerprint).
const SYNTH_VERSION: u64 = 1;

type Relight = fn(&mut MapGrid, i32, &LightTables, bool, &[[i32; 3]]) -> Touched;

fn work(g:&mut MapGrid, h: i32, t: &LightTables, sky: bool, c: &[[i32; 3]]) -> Touched {
    rc_light::relight_sections(g, h, t, sky, c)
}

/// Working path with the perf log's queue counters on (their cost in the scene bench).
fn counted(g: &mut MapGrid, h: i32, t: &LightTables, sky: bool, c: &[[i32; 3]]) -> Touched {
    rc_light::stats::force_queue_counting(Some(true));
    let out = rc_light::relight_sections(g, h, t, sky, c);
    rc_light::stats::force_queue_counting(Some(false));
    out
}

/// The C2-M02 switches (frontier seeding, step order, parallel step) set off.
const PROPAGATION_OFF: [(&str, bool); 8] = [
    ("LIGHTSKYFRONTIER", false),
    ("LIGHTMARGINFRONTIER", false),
    ("LIGHTSEEDROWS", false),
    ("LIGHTLEVELFIRST", false),
    ("LIGHTSPREADINLINE", false),
    ("LIGHTATOMICVIEW", false),
    ("LIGHTSPREADBUFS", false),
    ("LIGHTSPREADAFTERSEED", false),
];

/// Working path without the C2-M02 switches (the working path of before).
fn before_propagation(g: &mut MapGrid, h: i32, t: &LightTables, sky: bool, c: &[[i32; 3]]) -> Touched {
    rc_light::relight_sections_with(g, h, t, sky, c, &PROPAGATION_OFF)
}

/// The C2-M10 switches (zone read by rows, uniform rows filled, plane scan, column kept) set off.
const ZONE_IO_OFF: [(&str, bool); 4] =
    [("LIGHTREADROWS", false), ("LIGHTREADFILL", false), ("LIGHTSCANPLANES", false), ("LIGHTWRITECOLUMN", false)];

/// Working path without the C2-M10 switches.
fn before_zone_io(g: &mut MapGrid, h: i32, t: &LightTables, sky: bool, c: &[[i32; 3]]) -> Touched {
    rc_light::relight_sections_with(g, h, t, sky, c, &ZONE_IO_OFF)
}

/// The C2-M11 switches of series 3 (seeding prepared by the read, sky columns fetched once) set off.
const SEED_AND_SKY_COLUMNS_OFF: [(&str, bool); 2] = [("LIGHTSEEDATREAD", false), ("LIGHTSKYCOLUMNCACHE", false)];

/// Working path without the C2-M11 switches of series 3.
fn before_seed(g: &mut MapGrid, h: i32, t: &LightTables, sky: bool, c: &[[i32; 3]]) -> Touched {
    rc_light::relight_sections_with(g, h, t, sky, c, &SEED_AND_SKY_COLUMNS_OFF)
}

fn frozen(g: &mut MapGrid, h: i32, t: &LightTables, sky: bool, c: &[[i32; 3]]) -> Touched {
    rc_light::reference::relight_sections(g, h, t, sky, c)
}

/// Every group by the zone (`LIGHTINC=0`): the working path of before C2-M12.
fn zone(g: &mut MapGrid, h: i32, t: &LightTables, sky: bool, c: &[[i32; 3]]) -> Touched {
    rc_light::relight_sections_inc(g, h, t, sky, c, IncrementalBench::Off)
}

/// Unchecked incremental relight (`LIGHTINCVERIFY=0`), with no group size limit.
fn unchecked(g: &mut MapGrid, h: i32, t: &LightTables, sky: bool, c: &[[i32; 3]]) -> Touched {
    rc_light::relight_sections_inc(g, h, t, sky, c, IncrementalBench::Unchecked { max: usize::MAX })
}

/// Checked incremental with no group size limit (shipped: groups of at most 100 changes).
fn checked(g: &mut MapGrid, h: i32, t: &LightTables, sky: bool, c: &[[i32; 3]]) -> Touched {
    rc_light::relight_sections_inc(g, h, t, sky, c, IncrementalBench::Checked { max: usize::MAX })
}

/// Checked for groups of at most 16 changes, zone beyond (`LIGHTINCMAX=16`).
fn checked16(g: &mut MapGrid, h: i32, t: &LightTables, sky: bool, c: &[[i32; 3]]) -> Touched {
    rc_light::relight_sections_inc(g, h, t, sky, c, IncrementalBench::Checked { max: 16 })
}

/// Every group checked in order, the overlapping ones included (`LIGHTINCOVERLAP=1`).
fn in_order(g: &mut MapGrid, h: i32, t: &LightTables, sky: bool, c: &[[i32; 3]]) -> Touched {
    rc_light::relight_sections_inc(g, h, t, sky, c, IncrementalBench::CheckedInOrder)
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

/// Queue entries of a replay per kind of light: seeded (at level 15 for the sky sources and lit
/// margins), pushed by the propagation, left unspread at level 1, cells resolved and entries per
/// cell (how dense the propagation is), per relight.
fn print_queues(name: &str, relights: usize, q: &rc_light::stats::Queues) {
    let n = relights.max(1) as f64;
    for (kind, k) in [("block", rc_light::stats::BLOCK), ("sky", rc_light::stats::SKY)] {
        let (seeded, queued): (u64, u64) = (q.seeded[k].iter().sum(), q.queued[k].iter().sum());
        eprintln!(
            "{name}: {kind} queues per relight: seeded {:.0} (at 15: {:.0}), pushed {:.0}, left at level 1 {:.0}; \
             cells {:.0}, entries per cell {:.3}",
            seeded as f64 / n,
            q.seeded[k][15] as f64 / n,
            queued.saturating_sub(seeded) as f64 / n,
            q.queued[k][1] as f64 / n,
            q.cells[k] as f64 / n,
            queued as f64 / q.cells[k].max(1) as f64
        );
    }
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
        let queues_before = rc_light::stats::queues();
        let (ia, ib) = (IN_ALLOCS.load(Relaxed), IN_BYTES.load(Relaxed));
        // Propagation queues counted on this replay only (deterministic counts, any machine).
        rc_light::stats::force_queue_counting(Some(true));
        let (ga, ta, da) = replay(tr, &t, work);
        rc_light::stats::force_queue_counting(Some(false));
        let (allocs_a, bytes_a) = (IN_ALLOCS.load(Relaxed) - ia, IN_BYTES.load(Relaxed) - ib);
        let st = rc_light::stats::totals().since(&before);
        print_queues(&tr.name, tr.events(), &rc_light::stats::queues().since(&queues_before));
        let (ia, ib) = (IN_ALLOCS.load(Relaxed), IN_BYTES.load(Relaxed));
        let (gb, tb, db) = replay(tr, &t, frozen);
        let (allocs_b, bytes_b) = (IN_ALLOCS.load(Relaxed) - ia, IN_BYTES.load(Relaxed) - ib);
        // The working path of before (C2-M02 off): its queues, and the same output.
        let queues_before = rc_light::stats::queues();
        let (ia, ib) = (IN_ALLOCS.load(Relaxed), IN_BYTES.load(Relaxed));
        rc_light::stats::force_queue_counting(Some(true));
        let (gc, tc, _) = replay(tr, &t, before_propagation);
        rc_light::stats::force_queue_counting(Some(false));
        let (allocs_c, bytes_c) = (IN_ALLOCS.load(Relaxed) - ia, IN_BYTES.load(Relaxed) - ib);
        print_queues(&format!("{} before C2-M02", tr.name), tr.events(), &rc_light::stats::queues().since(&queues_before));
        let (gd, td, _) = replay(tr, &t, before_zone_io);
        let (ge, te, _) = replay(tr, &t, before_seed);
        for (g, touched, label) in
            [(&ga, &ta, "work"), (&gc, &tc, "before C2-M02"), (&gd, &td, "before C2-M10"), (&ge, &te, "before C2-M11")]
        {
            assert_eq!(touched.len(), tb.len());
            for (i, (a, b)) in touched.iter().zip(&tb).enumerate() {
                assert_eq!(a, b, "{} ({label}): touched sections of relight {i}", tr.name);
            }
            for (k, (_, sa, ba)) in &g.0 {
                let (_, sb, bb) = &gb.0[k];
                assert!(sa == sb && ba == bb, "{} ({label}): sheets of column {k:?} differ from the reference", tr.name);
            }
        }
        check_known(&tr.name, tr.digest, digest(&ga, &ta), KNOWN);
        drop((gc, tc, gd, td, ge, te));
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
                "{}: allocations per relight work {:.1} / {:.0} KiB, before C2-M02 {:.1} / {:.0} KiB, reference {:.1} / {:.0} KiB",
                tr.name,
                allocs_a as f64 / n,
                bytes_a as f64 / n / 1024.0,
                allocs_c as f64 / n,
                bytes_c as f64 / n / 1024.0,
                allocs_b as f64 / n,
                bytes_b as f64 / n / 1024.0
            );
        }
        drop((ga, gb, ta, tb));
        // The unchecked incremental differs on inconsistent sheets (protocol A3); checked, it may not
        // differ at all, on any trace.
        for (label, f, must_match) in [
            ("LIGHTINCVERIFY=0", unchecked as Relight, false),
            ("LIGHTINC (checked)", checked, true),
            ("LIGHTINCMAX=16 (checked)", checked16, true),
            ("LIGHTINCOVERLAP=1", in_order, true),
        ] {
            let (n, cells) = gaps(tr, &t, f);
            eprintln!("{}: {label} differs from the reference on {n} relights at equal input ({cells} cells)", tr.name);
            assert!(!must_match || n == 0, "{}: {label} differs from the reference on {n} relights", tr.name);
        }
        for (label, f) in [("LIGHTINC (checked)", checked as Relight), ("LIGHTINCOVERLAP=1", in_order)] {
            let before = rc_light::stats::checked_incremental();
            replay(tr, &t, f);
            let after = rc_light::stats::checked_incremental();
            let (kept, fallbacks) = (after.0 - before.0, after.1 - before.1);
            let rate = 100.0 * fallbacks as f64 / (kept + fallbacks).max(1) as f64;
            eprintln!("{}: {label}: {kept} resolutions kept, {fallbacks} handed back to the zone ({rate:.1} %)", tr.name);
        }
        // Order A B B A: each path runs once before and once after the other, and two
        // passes of the same path give the bench's own spread (order, frequency, cache).
        // A gain is judged against the old working path (main), never against the frozen
        // copy: it has no counters nor `bounded` parameter and its machine code differs.
        // `work`: shipped (checked incremental, C2-M12), `zone`: every group by the zone (the working
        // path of before C2-M12), `unchecked`: incremental of C2-M14, `checked16`: checked for the
        // groups of at most 16 changes, `in_order`: every group checked in order, `counted`: queue
        // counters on (C2-M01), `before`: the C2-M02 switches off, `before_io`: the C2-M10 ones,
        // `before_seed`: the C2-M11 ones of series 3, same order mirrored.
        for (label, f) in [
            ("work_a", work as Relight),
            ("zone_a", zone),
            ("unchecked_a", unchecked),
            ("checked16_a", checked16),
            ("in_order_a", in_order),
            ("before_seed_a", before_seed),
            ("before_io_a", before_zone_io),
            ("before_a", before_propagation),
            ("counted_a", counted),
            ("reference_a", frozen),
            ("reference_b", frozen),
            ("counted_b", counted),
            ("before_b", before_propagation),
            ("before_io_b", before_zone_io),
            ("before_seed_b", before_seed),
            ("in_order_b", in_order),
            ("checked16_b", checked16),
            ("unchecked_b", unchecked),
            ("zone_b", zone),
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
