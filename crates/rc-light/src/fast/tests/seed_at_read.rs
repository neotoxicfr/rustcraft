//! Proofs of mission C2-M11 of series 3: the read prepared for the seeding gives the old read's
//! arrays with exactly the solved interior cells opened and the block emitters listed; the sky columns
//! fetched once give every stack the source height of the old descent; the block resolution still
//! puts out old light with no emitter nor lit margin; each switch alone, and the shipped set with each
//! one flipped, give the old path's sheets and touched sections.

use super::super::seed_at_read::{
    SABOTAGE_BLOCK_MARGIN_ROW_OPENED, SABOTAGE_EMITTER_LOST_AT_SECTION_TOP, SABOTAGE_STORED_LIGHT_IGNORED,
};
use super::super::sky_columns::SABOTAGE_HIGHEST_SECTION_OVERLOOKED;
use super::*;

/// The per-section read (`ZONEUNPACK`) with buffers kept, the base of the switches measured here.
fn section_read() -> Opts {
    Opts::OLD.with("ZONEUNPACK", true).with("ZONEBUF", true)
}

/// Random sheets and sections in direct identifiers ([`world`]), and 4,000 random blocks (emitters
/// among them) over every column so that most sections hold a palette.
fn mixed_world(rng: &mut Rng, height: i32) -> World {
    let mut w = world(height, true, rng);
    for _ in 0..4000 {
        let p = [rng.below(64) - 16, rng.below(height as u64), rng.below(64) - 16];
        let s = state(rng);
        set(&mut w, p, s);
    }
    w
}

/// A zone anywhere around the world, and the rows each wanted sheet solves: the whole zone for the
/// sky, a slice of at least three rows (or the whole zone) for the block.
fn zone_and_rows(rng: &mut Rng, height: i32) -> (Geo, Option<SolvedRows>, Option<SolvedRows>) {
    let lo = [rng.below(70) - 30, rng.below(height as u64 + 8) - 4, rng.below(70) - 30];
    let size = [1 + rng.below(50), 1 + rng.below(height as u64 + 6), 1 + rng.below(50)];
    let g = Geo::new(lo, size);
    let block_rows = if size[1] >= 3 && rng.below(2) == 0 {
        let first = rng.below(size[1] as u64 - 2);
        SolvedRows { first, count: 3 + rng.below((size[1] - first - 2) as u64) }
    } else {
        SolvedRows::all(g)
    };
    let (sky, block) = [(true, true), (true, false), (false, true)][rng.below(3) as usize];
    (g, sky.then(|| SolvedRows::all(g)), block.then_some(block_rows))
}

/// Arrays of one read: face, then propagation byte, emission, sky and block (unwanted: empty), and
/// the emitters listed, sorted.
type ReadArrays = (Vec<u32>, [Vec<u8>; 4], Vec<u32>);

fn read_arrays(world: &World, c: &Ctx, w: &mut Work, g: Geo, sky: Option<SolvedRows>, block: Option<SolvedRows>) -> ReadArrays {
    read_zone(world, c, w, g, sky, block);
    let n = g.cells();
    let wanted = |v: &[u8], on: bool| if on { v[..n].to_vec() } else { Vec::new() };
    let (s, b) = (sky.is_some(), block.is_some());
    let mut emitters = w.emitters.clone();
    emitters.sort_unstable();
    (w.face[..n].to_vec(), [w.prop[..n].to_vec(), wanted(&w.emit, b), wanted(&w.sky, s), wanted(&w.blk, b)], emitters)
}

/// Zone coordinates `(dx, dy, dz)` of an array index (the sentinel ring at -1 and `size`).
fn coords(g: Geo, i: usize) -> [i32; 3] {
    let (dy, rest) = (i / g.plane, i % g.plane);
    [(rest % g.ax) as i32 - g.ring, dy as i32 - g.ring, (rest / g.ax) as i32 - g.ring]
}

/// The cell is inside the zone's interior stacks and strictly between the margin rows of `rows`.
fn solved_interior(g: Geo, i: usize, rows: SolvedRows) -> bool {
    let [dx, dy, dz] = coords(g, i);
    (1..g.size[0] - 1).contains(&dx) && (1..g.size[2] - 1).contains(&dz) && (rows.first + 1..rows.first + rows.count - 1).contains(&dy)
}

/// What the prepared read must give, from the arrays of the plain read: the solved interior cells
/// with a sheet opened, and the solved interior cells that emit listed.
fn expected_from_plain(g: Geo, plain: &ReadArrays, sky: Option<SolvedRows>, block: Option<SolvedRows>) -> ReadArrays {
    let (face, [prop, emit, sky_values, block_values], _) = plain.clone();
    let open = |values: Vec<u8>, rows: Option<SolvedRows>| -> Vec<u8> {
        let Some(rows) = rows else { return values };
        values
            .iter()
            .enumerate()
            .map(|(i, &b)| if b & DATA != 0 && solved_interior(g, i, rows) { b | OPEN } else { b })
            .collect()
    };
    let emitters = match block {
        Some(rows) => (0..emit.len()).filter(|&i| emit[i] > 0 && solved_interior(g, i, rows)).map(|i| i as u32).collect(),
        None => Vec::new(),
    };
    let (sky_values, block_values) = (open(sky_values, sky), open(block_values, block));
    (face, [prop, emit, sky_values, block_values], emitters)
}

/// First zone, over `zones` random ones, where the read prepared by `LIGHTSEEDATREAD` on top of `o`
/// is not the plain read of `o` with the solved interior opened and the emitters listed; the opened
/// cells and the emitters seen, otherwise.
fn first_prepared_read_difference(world: &World, t: &LightTables, seed: u64, zones: usize, o: Opts) -> Result<[usize; 2], String> {
    let height = 64;
    let mut rng = Rng(seed);
    let (mut plain_buffers, mut prepared_buffers) = (Work::default(), Work::default());
    let plain_ctx = Ctx { height, t, o: o.with("LIGHTSEEDATREAD", false), count_queues: false };
    let prepared_ctx = Ctx { o: o.with("LIGHTSEEDATREAD", true), ..plain_ctx };
    let mut seen = [0usize; 2];
    for zone in 0..zones {
        let (g, sky, block) = zone_and_rows(&mut rng, height);
        let plain = read_arrays(world, &plain_ctx, &mut plain_buffers, g, sky, block);
        let prepared = read_arrays(world, &prepared_ctx, &mut prepared_buffers, g, sky, block);
        if prepared != expected_from_plain(g, &plain, sky, block) {
            return Err(format!("zone {zone} {:?} + {:?}, sky {sky:?}, block {block:?}, {o:?}", g.lo, g.size));
        }
        let [_, _, s, b] = &prepared.1;
        seen[0] += s.iter().chain(b).filter(|&&v| v & OPEN != 0).count();
        seen[1] += prepared.2.len();
    }
    Ok(seen)
}

/// The read prepared for the seeding, over 500 random zones and every read (cell by cell, by rows,
/// rows filled, kept buffers with only the ring zeroed; sequential then in parallel bands of 16
/// planes): the plain read's arrays with exactly the solved interior cells that have a sheet opened,
/// and exactly the emitting ones listed. An emitter forgotten on a section's top row, and the block's
/// bottom margin row opened, are seen.
#[test]
fn prepared_read_opens_the_solved_interior_and_lists_its_emitters() {
    let t = tables();
    let mut rng = Rng(2611);
    let world = mixed_world(&mut rng, 64);
    let rows_filled = section_read().with("LIGHTREADROWS", true).with("LIGHTREADFILL", true);
    let reads = [
        section_read(),
        section_read().with("LIGHTREADROWS", true),
        rows_filled,
        rows_filled.with("LIGHTZEROONLYRING", true),
        section_read().with("LIGHTOPENAFTERREAD", true),
        rows_filled.with("LIGHTOPENAFTERREAD", true),
        rows_filled.with("LIGHTZEROONLYRING", true).with("LIGHTOPENAFTERREAD", true),
    ];
    for o in reads {
        let [opened, emitters] = first_prepared_read_difference(&world, &t, 2612, 500, o).unwrap_or_else(|e| panic!("{e}"));
        assert!(opened > 100_000 && emitters > 1000, "too little met to prove anything: {opened} opened, {emitters} emitters");
    }
    let bands = |o: Opts| Opts { par_min: 0, ..o.with("LIGHTPARSOLVE", true) };
    pool(4).install(|| {
        for o in reads {
            first_prepared_read_difference(&world, &t, 2613, 200, bands(o)).unwrap_or_else(|e| panic!("parallel bands: {e}"));
        }
    });
    for (sabotage, what) in [
        (SABOTAGE_EMITTER_LOST_AT_SECTION_TOP, "emitter forgotten on a section's top row"),
        (SABOTAGE_BLOCK_MARGIN_ROW_OPENED, "block margin row opened"),
    ] {
        for o in [reads[1], reads[5]] {
            SABOTAGE.with(|s| s.set(sabotage));
            let caught = first_prepared_read_difference(&world, &t, 2612, 500, o);
            SABOTAGE.with(|s| s.set(0));
            assert!(caught.is_err(), "{what} not seen ({o:?})");
        }
    }
}

/// Source height of every interior stack of `zones` random sky zones, the columns looked up per
/// stack and fetched once (`LIGHTSKYCOLUMNCACHE`), with and without the section skip (the memo
/// stays off: shared by both calls, it would hand the second the first one's answer; the relight
/// proofs cover it); the stacks seen, or the first difference.
fn first_source_difference(world: &World, t: &LightTables, seed: u64, zones: usize) -> Result<usize, String> {
    let height = 64;
    let mut rng = Rng(seed);
    let mut stacks = 0;
    for zone in 0..zones {
        let changes: Vec<[i32; 3]> = (0..1 + rng.below(3))
            .map(|_| [rng.below(60) - 14, rng.below(height as u64), rng.below(60) - 14])
            .collect();
        let base = section_read().with("SKYSKIP", rng.below(4) != 0);
        let (looked_up, fetched) = (base.with("LIGHTSKYCOLUMNCACHE", false), base.with("LIGHTSKYCOLUMNCACHE", true));
        let ctx = |o: Opts| Ctx { height, t, o, count_queues: false };
        let mut w = Work::default();
        let block_geo = block_zone(&ctx(looked_up), &changes);
        let (g, _) = sky_zone(world, &ctx(looked_up), &mut w.aux, &changes, block_geo);
        read(world, &ctx(looked_up), &mut w, g, true, false);
        let Work { face, prop, emit, emitters, aux, .. } = &mut w;
        let z = Cells { face, prop, emit, emitters };
        let ny = g.size[1];
        let columns = SkyColumns::new(world, &ctx(fetched), aux, g);
        for dz in 1..g.size[2] - 1 {
            for dx in 1..g.size[0] - 1 {
                let old = column_source(world, &ctx(looked_up), aux, None, &z, g, 0, ny, dx, dz);
                let new = column_source(world, &ctx(fetched), aux, Some(&columns), &z, g, 0, ny, dx, dz);
                if old != new {
                    return Err(format!("zone {zone} ({changes:?}), stack ({dx}, {dz}): {old} against {new}, {base:?}"));
                }
                stacks += 1;
            }
        }
    }
    Ok(stacks)
}

/// Sky columns fetched once: every interior stack of 300 random sky zones gets the source height
/// of the descent from the top of the world (worlds of random blocks at every height, so columns
/// with several sections that are not clear); starting below the highest of them is seen.
#[test]
fn sky_columns_give_every_stack_its_source_height() {
    // A window entry stays a slice and a height (the slice's pointer gives the absent columns' niche).
    assert_eq!(size_of::<Option<super::super::sky_columns::SkyColumn>>(), 24);
    let t = tables();
    let mut rng = Rng(2614);
    let world = mixed_world(&mut rng, 64);
    let stacks = first_source_difference(&world, &t, 2615, 300).unwrap_or_else(|e| panic!("{e}"));
    assert!(stacks > 100_000, "too few stacks: {stacks}");
    SABOTAGE.with(|s| s.set(SABOTAGE_HIGHEST_SECTION_OVERLOOKED));
    let caught = first_source_difference(&world, &t, 2615, 300);
    SABOTAGE.with(|s| s.set(0));
    assert!(caught.is_err(), "descent started below a section that is not clear not seen");
}

/// A torch lit in a consistent world, then taken away: its block zone then holds old light but no
/// emitter, and its margins are dark (16 cells away), so only the stored light keeps the block
/// resolution going; it must put the light out like the old path. Ignoring the stored light is seen.
#[test]
fn removed_torch_light_is_put_out() {
    let t = tables();
    let o = Opts::from_bits(!DEDUP & !PAR_BITS);
    assert!(o.seeds_at_read() && o.block_early);
    let run = || -> bool {
        let mut a = world(64, false, &mut Rng(4));
        let torch = [8, 32, 8];
        set(&mut a, torch, TORCH);
        crate::relight_zones(&mut a, 64, &t, false, &[torch], true);
        let mut b = a.clone();
        set(&mut a, torch, 0);
        set(&mut b, torch, 0);
        let lit_before = a.0[&(0, 0)].2.iter().flatten().any(|l| l.iter().any(|&v| v != 0));
        assert!(lit_before, "the torch lit nothing");
        let ta = crate::relight_zones(&mut a, 64, &t, false, &[torch], true);
        let mut out = [Touched::new()];
        relight_zones(&mut b, 64, &t, false, &[torch], None, o, &mut out);
        a == b && ta == out[0]
    };
    assert!(run(), "light of the removed torch not put out like the old path");
    SABOTAGE.with(|s| s.set(SABOTAGE_STORED_LIGHT_IGNORED));
    let same = run();
    SABOTAGE.with(|s| s.set(0));
    assert!(!same, "early stop that ignores the stored light not seen");
}

/// Each switch alone on the per-section read, both together, and the shipped set with each one
/// turned off: bit for bit the old path, sheets and touched sections, sequential then multi-core
/// (thresholds at 0, 4 threads).
#[test]
fn each_seed_switch_alone_matches_old() {
    let t = tables();
    let names = ["LIGHTSEEDATREAD", "LIGHTSKYCOLUMNCACHE", "LIGHTOPENAFTERREAD"];
    let mut paths: Vec<(String, Opts)> = names.iter().map(|n| (format!("{n} alone"), section_read().with(n, true))).collect();
    paths.push(("seeds prepared by the read, opened after it".into(), section_read().with(names[0], true).with(names[2], true)));
    paths.push(("all three".into(), section_read().with(names[0], true).with(names[1], true).with(names[2], true)));
    let shipped = Opts::from_bits(!super::super::DEFAULT_OFF & ((1 << NAMES.len()) - 1) & !PAR_BITS);
    assert!(shipped.seed_at_read && shipped.sky_column_cache && shipped.open_after_read, "all three on by default");
    for n in names {
        paths.push((format!("shipped without {n}"), shipped.with(n, false)));
    }
    paths.push(("shipped".into(), shipped));
    let multi_core = |o: Opts| Opts { join_min: 0, par_min: 0, chunk: 64, ..o.with("LIGHTJOIN", true).with("LIGHTPARSOLVE", true).with("LIGHTGROUPS", true) };
    let threads = pool(4);
    for (label, o) in paths {
        for (seed, garbage) in [(81u64, false), (82, true)] {
            let case = Case { t: &t, height: 64, has_sky: true, garbage, steps: 300, seed, state: &state };
            compare(&case, |_| o).unwrap_or_else(|e| panic!("{label}, seed {seed}: {e}"));
            // Built on the pool's thread: a case holds a state picker that is not shared.
            threads
                .install(|| {
                    let case = Case { t: &t, height: 64, has_sky: true, garbage, steps: 300, seed, state: &state };
                    compare(&case, |_| multi_core(o))
                })
                .unwrap_or_else(|e| panic!("{label} multi-core, seed {seed}: {e}"));
        }
    }
}
