//! Proofs of the C2-M02 seeding and propagation steps: the seeding leaves the same levels and
//! opened cells with fewer queue entries, the propagation reaches the same levels, the relight
//! gives the old path's sheets, and each deliberate error is caught.

use super::super::frontier::{
    SABOTAGE_FORGETS_HIGHER_NEIGHBOUR, SABOTAGE_FORGETS_LOWEST_SOURCE, SABOTAGE_MARGIN_15_NOT_QUEUED, StackSeed,
};
use super::*;

/// The seeding record of a stack stays two 32-bit heights (one per stack of the zone, kept per thread).
#[test]
fn stack_seed_is_two_heights() {
    assert_eq!(size_of::<StackSeed>(), 8);
}

/// Sequential switches all on (without `LIGHTDEDUP` and the multi-core paths).
fn sequential_all() -> Opts {
    Opts::from_bits(!DEDUP & !PAR_BITS)
}

/// Bits of the C2-M02 seeding switches (`LIGHTSKYFRONTIER`, `LIGHTMARGINFRONTIER`, `LIGHTSEEDROWS`).
const SEED_BITS: u32 = 0x7 << 14;

/// One zone seeded with given switches: the sheet bytes after seeding (opened cells), the seeded
/// levels, the entries queued and the levels after propagation.
struct Seeded {
    lay: Vec<u8>,
    seeded: Vec<u8>,
    entries: usize,
    spread: Vec<u8>,
}

/// Zone of the group `cs` (sky zone or block zone) read once, then seeded and propagated with
/// each of `opts` on a copy of the read.
fn seed_zone(world: &World, t: &LightTables, cs: &[[i32; 3]], sky: bool, opts: &[Opts]) -> Vec<Seeded> {
    let first = Ctx { height: 64, t, o: opts[0], count_queues: false };
    let mut w = Work::default();
    let block_geo = block_zone(&first, cs);
    let (sky_geo, _) = sky_zone(world, &first, &mut w.aux, cs, block_geo);
    let g = if sky { sky_geo } else { block_geo };
    read(world, &first, &mut w, g, sky, !sky);
    let Work { face, prop, emit, sky: sky_sheet, blk, aux, .. } = &mut w;
    let z = Cells { face, prop, emit };
    let read_sheet = if sky { sky_sheet.clone() } else { blk.clone() };
    opts.iter()
        .map(|&o| {
            let c = Ctx { o, ..first };
            let (mut lay, mut lane) = (read_sheet.clone(), Lane::default());
            seed(world, &c, &z, &mut lay, &mut lane, sky.then_some(&mut *aux), None, g, 0, g.size[1], sky);
            let entries = lane.buckets.iter().map(Vec::len).sum();
            let seeded = lane.level.clone();
            propagate(&c, &z, &lay, &mut lane, g, 0, g.size[1]);
            Seeded { lay, seeded, entries, spread: lane.level }
        })
        .collect()
}

/// Consistent world with a stone roof (a shadow under full sky), leaves under it, and torches
/// just outside the block zone of a change at (2, 30, 2) (lit margins, x = 18 and z = -14), some
/// of them against a stone wall inside the zone (margins with nothing to raise).
fn shaded_world(t: &LightTables) -> World {
    let mut rng = Rng(17);
    let mut w = world(64, false, &mut rng);
    let mut ps = Vec::new();
    for x in -3..8 {
        for z in -3..8 {
            ps.push(([x, 40, z], 1));
        }
    }
    for y in 28..33 {
        for z in 0..5 {
            ps.push(([17, y, z], 1));
        }
    }
    ps.extend([([2, 33, 2], 2), ([3, 33, 2], 2), ([20, 30, 2], TORCH), ([2, 30, -16], TORCH)]);
    for &(p, s) in &ps {
        set(&mut w, p, s);
    }
    let changed: Vec<[i32; 3]> = ps.iter().map(|&(p, _)| p).collect();
    crate::relight_zones(&mut w, 64, t, true, &changed, true);
    w
}

/// Same seeded levels and opened cells with and without each seeding switch, same levels after
/// propagation; the sky frontier queues far fewer entries (a full-sky zone: only the margins and
/// the edges of the shadow).
#[test]
fn frontier_seeding_keeps_the_levels_with_fewer_entries() {
    let t = tables();
    let world = shaded_world(&t);
    let all = sequential_all();
    let old = Opts::from_bits(all.bits() & !SEED_BITS);
    let variants = [
        old,
        all,
        old.with("LIGHTSKYFRONTIER", true),
        old.with("LIGHTMARGINFRONTIER", true),
        old.with("LIGHTSEEDROWS", true),
    ];
    for sky in [true, false] {
        let runs = seed_zone(&world, &t, &[[2, 30, 2]], sky, &variants);
        for (k, r) in runs.iter().enumerate().skip(1) {
            assert!(r.lay == runs[0].lay, "sky {sky}, variant {k}: opened cells");
            assert!(r.seeded == runs[0].seeded, "sky {sky}, variant {k}: seeded levels");
            assert!(r.spread == runs[0].spread, "sky {sky}, variant {k}: levels after propagation");
            assert!(r.entries <= runs[0].entries, "sky {sky}, variant {k}: more entries");
        }
        let entries: Vec<usize> = runs.iter().map(|r| r.entries).collect();
        eprintln!("sky {sky}: entries old, all, sky frontier, margin frontier, rows: {entries:?}");
        if sky {
            // The margins at 15 stay with the sky frontier alone; both together leave the edges.
            assert!(entries[2] * 3 < entries[0] && entries[1] * 10 < entries[0], "sky frontier: {entries:?}");
            assert!(entries[3] < entries[0], "margin frontier on the sky: {entries:?}");
        } else {
            assert!(entries[3] < entries[0], "margin frontier on the block: {entries:?}");
            assert_eq!(entries[2], entries[0], "the sky frontier does not touch the block");
        }
        assert_eq!(entries[4], entries[0], "rows order: same entries");
    }
}

/// Shapes where each frontier rule matters, placed one step at a time on a consistent world and
/// relit by the old path and by `o`: a 3 × 3 layer of leaves under the sky (its centre is only lit
/// from its own lowest source), a 5 × 5 stone roof (the open stacks west of it light its shadow
/// sideways), then a stone 16 blocks west of the roof's east side (the margin of its zone is open
/// sky next to the shadow). Sheets and touched sections compared at every step.
fn frontier_scene(t: &LightTables, o: Opts) -> Result<(), String> {
    let mut rng = Rng(71);
    let mut a = world(64, false, &mut rng);
    let mut b = a.clone();
    let layer = |y: i32, x0: i32, z0: i32, n: i32, s: u32| {
        (0..n * n).map(move |k| ([x0 + k % n, y, z0 + k / n], s)).collect::<Vec<_>>()
    };
    let steps = [layer(50, 19, 19, 3, 2), layer(40, 0, 0, 5, 1), vec![([-11, 20, 2], 1)]];
    for (n, step) in steps.iter().enumerate() {
        for &(p, s) in step {
            set(&mut a, p, s);
            set(&mut b, p, s);
        }
        let ps: Vec<[i32; 3]> = step.iter().map(|&(p, _)| p).collect();
        let ta = crate::relight_zones(&mut a, 64, t, true, &ps, true);
        let mut out = [Touched::new()];
        relight_zones(&mut b, 64, t, true, &ps, None, o, &mut out);
        if a != b || ta != out[0] {
            return Err(format!("scene step {n} differs"));
        }
    }
    Ok(())
}

/// Seeding sabotages seen by the comparison with the old path (sequential, all switches on), on
/// the frontier scene or on random suites: frontier without the lowest source of a stack, frontier
/// that forgets the higher source of its east neighbour, margins at 15 never queued, level
/// prefilter that assumes an opacity of 2.
#[test]
fn frontier_sabotages_are_caught() {
    let t = tables();
    frontier_scene(&t, sequential_all()).expect("frontier scene without sabotage");
    let names = [
        (SABOTAGE_FORGETS_LOWEST_SOURCE, "frontier forgets the lowest source"),
        (SABOTAGE_FORGETS_HIGHER_NEIGHBOUR, "frontier forgets a neighbour that starts higher"),
        (SABOTAGE_MARGIN_15_NOT_QUEUED, "margin at 15 not queued"),
        (super::super::SABOTAGE_PREFILTER_ASSUMES_OPACITY_2, "prefilter assumes opacity 2"),
    ];
    for (k, name) in names {
        SABOTAGE.with(|s| s.set(k));
        let scene = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| frontier_scene(&t, sequential_all())));
        SABOTAGE.with(|s| s.set(0));
        if !matches!(scene, Ok(Ok(()))) {
            eprintln!("sabotage {k} ({name}): frontier scene {scene:?}");
            continue;
        }
        let caught = (0..6u64).any(|seed| {
            SABOTAGE.with(|s| s.set(k));
            let case =
                Case { t: &t, height: 64, has_sky: true, garbage: seed % 2 == 1, steps: 400, seed, state: &state };
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| compare(&case, |_| sequential_all())));
            SABOTAGE.with(|s| s.set(0));
            match &r {
                Ok(Ok(())) => false,
                Ok(Err(e)) => {
                    eprintln!("sabotage {k} ({name}), seed {seed}: {e}");
                    true
                }
                Err(_) => {
                    eprintln!("sabotage {k} ({name}), seed {seed}: panic");
                    true
                }
            }
        });
        assert!(caught, "sabotage {k} ({name}) not detected");
    }
}

/// Each C2-M02 switch alone on the old path, then all of them, on random and consistent data:
/// bit for bit the old path (sheets and sections).
#[test]
fn each_new_switch_alone_matches_old() {
    let t = tables();
    let par = |o: Opts| Opts { join_min: 0, par_min: 0, chunk: 64, ..o };
    for (n, name) in NAMES.iter().enumerate().take(22).skip(14) {
        // The multi-core switches act inside `LIGHTPARSOLVE` only.
        let base = if n >= 18 { par(Opts::from_bits(4096)) } else { Opts::OLD };
        let o = base.with(name, true);
        pool(4).install(|| {
            for (seed, garbage) in [(51u64, false), (52, true)] {
                let case = Case { t: &t, height: 64, has_sky: true, garbage, steps: 300, seed, state: &state };
                compare(&case, |_| o).unwrap_or_else(|e| panic!("{name} alone, seed {seed}: {e}"));
            }
        });
    }
}

/// Sequential propagation after the seeding (`LIGHTSPREADAFTERSEED`) at thresholds 0 (always
/// parallel) and infinite (always sequential): the light of the old path, 1 and 8 threads.
#[test]
fn spread_after_seed_thresholds_give_the_same_light() {
    let t = tables();
    for threads in [1, 8] {
        for spread_min in [0, usize::MAX] {
            let o = Opts { join_min: 0, par_min: 0, chunk: 64, spread_min, ..Opts::from_bits(!DEDUP) };
            pool(threads).install(|| {
                for (seed, garbage) in [(61u64, false), (62, true)] {
                    let case = Case { t: &t, height: 64, has_sky: true, garbage, steps: 300, seed, state: &state };
                    compare(&case, |_| o)
                        .unwrap_or_else(|e| panic!("{threads} threads, threshold {spread_min}, seed {seed}: {e}"));
                }
            });
        }
    }
}
