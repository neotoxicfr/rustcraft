//! Proofs of mission C2-M10: the zone read by rows gives the arrays of the cell-by-cell read, the
//! plane scan gives the changes of the stack scan, and keeping a column's sheets while writing gives
//! the same sheets and touched sections, inserted in the same order.

use super::super::change_scan::{Changes, PlaneScan, apply_changes, find_changes};
use super::super::zone_read::{ROWS_MET, RowKind};
use super::*;

/// The per-section read of before C2-M10 (`ZONEUNPACK` alone, cell by cell).
fn old_read() -> Opts {
    Opts::OLD.with("ZONEUNPACK", true)
}

/// World of the read proof: random sheets (shared uniform, random, absent), a section in direct
/// identifiers, two absent columns, and 3,000 random blocks in the columns of x below 16 so that
/// their sections hold a palette (the others keep a single state).
fn read_world(rng: &mut Rng, height: i32) -> World {
    let mut w = world(height, true, rng);
    for _ in 0..3000 {
        let p = [rng.below(32) - 16, rng.below(height as u64), rng.below(64) - 16];
        let s = state(rng);
        set(&mut w, p, s);
    }
    w
}

/// Zone arrays (face; propagation byte, emission, sky, block) of one read with the switches `o`
/// into the buffers `w` (fresh, or kept from earlier reads); unwanted sheets left empty.
#[allow(clippy::too_many_arguments)]
fn read_arrays(
    world: &World,
    t: &LightTables,
    height: i32,
    o: Opts,
    w: &mut Work,
    g: Geo,
    sky: bool,
    blk: bool,
) -> (Vec<u32>, [Vec<u8>; 4]) {
    let c = Ctx { height, t, o, count_queues: false };
    read(world, &c, w, g, sky, blk);
    let n = g.cells();
    let wanted = |v: &[u8], on: bool| if on { v[..n].to_vec() } else { Vec::new() };
    (w.face[..n].to_vec(), [w.prop[..n].to_vec(), wanted(&w.emit, blk), wanted(&w.sky, sky), wanted(&w.blk, blk)])
}

/// A zone anywhere around the world: origins even and odd, below and above the world, over
/// absent columns, from one cell to several chunks wide.
fn random_zone(rng: &mut Rng, height: i32) -> Geo {
    let lo = [rng.below(70) - 30, rng.below(height as u64 + 8) - 4, rng.below(70) - 30];
    let size = [1 + rng.below(50), 1 + rng.below(height as u64 + 6), 1 + rng.below(50)];
    Geo::new(lo, size)
}

/// First zone whose arrays differ between the cell-by-cell read (fresh zeroed buffers) and `o`
/// (buffers kept from one zone to the next, so stale values must all be overwritten or zeroed), over
/// `zones` random zones (wanted sheets in turn: both, sky only, block only).
fn first_read_difference(
    world: &World,
    t: &LightTables,
    height: i32,
    seed: u64,
    zones: usize,
    o: Opts,
) -> Option<String> {
    let mut rng = Rng(seed);
    let mut kept = Work::default();
    (0..zones).find_map(|zone| {
        let g = random_zone(&mut rng, height);
        let (sky, blk) = [(true, true), (true, false), (false, true)][zone % 3];
        let old = read_arrays(world, t, height, old_read(), &mut Work::default(), g, sky, blk);
        let new = read_arrays(world, t, height, o, &mut kept, g, sky, blk);
        (old != new).then(|| format!("zone {zone} {:?} + {:?}, sky {sky}, block {blk}, {o:?}", g.lo, g.size))
    })
}

/// Zone read by rows, with and without the uniform fill and the ring-only zeroing, sequential and in
/// parallel bands (`LIGHTPARSOLVE`, threshold 0): the arrays of the cell-by-cell read, cell for cell
/// and ring included, for 600 zones; every kind of row is met (fixed state, palette, direct values,
/// cells looked up, rows outside the world or of an absent column, shared uniform sheets, unpacked
/// sheets); a nibble shifted on the rows that start on an odd cell, and a ring left stale, are seen.
#[test]
fn zone_read_by_rows_matches_cell_by_cell() {
    let t = tables();
    let height = 64;
    let mut rng = Rng(2026);
    let world = read_world(&mut rng, height);
    let rows = old_read().with("LIGHTREADROWS", true);
    let kept_buffers = |o: Opts| o.with("ZONEBUF", true).with("LIGHTZEROONLYRING", true);
    let new_paths = [
        rows,
        rows.with("LIGHTREADFILL", true),
        kept_buffers(rows.with("LIGHTREADFILL", true)),
        kept_buffers(old_read()),
    ];
    ROWS_MET.with(|m| m.set([0; 7]));
    for o in new_paths {
        if let Some(e) = first_read_difference(&world, &t, height, 2027, 600, o) {
            panic!("{e}");
        }
    }
    for (sabotage, o, what) in [(40, new_paths[1], "nibble shifted on odd rows"), (42, new_paths[3], "ring left stale")]
    {
        SABOTAGE.with(|s| s.set(sabotage));
        let caught = first_read_difference(&world, &t, height, 2027, 600, o);
        SABOTAGE.with(|s| s.set(0));
        assert!(caught.is_some(), "{what} not seen");
    }
    let met = ROWS_MET.with(|m| m.get());
    let kinds = [
        RowKind::FixedState,
        RowKind::Palette,
        RowKind::Values,
        RowKind::Cells,
        RowKind::Outside,
        RowKind::UniformSheet,
        RowKind::UnpackedSheet,
    ];
    for kind in kinds {
        assert!(met[kind as usize] > 0, "no row of kind {} met: {met:?}", kind as usize);
    }
    // Parallel bands of 16 planes (threshold 0): same arrays.
    let par = |o: Opts| Opts { par_min: 0, ..o.with("LIGHTPARSOLVE", true) };
    pool(4).install(|| {
        for o in new_paths {
            if let Some(e) = first_read_difference(&world, &t, height, rng.0, 200, par(o)) {
                panic!("parallel bands: {e}");
            }
        }
    });
}

/// Each C2-M10 switch alone (the read ones on the per-section read they replace, the ring-only
/// zeroing on kept buffers), the read pair, and the shipped set with each one flipped: bit for bit
/// the old path, sheets and touched sections.
#[test]
fn each_zone_io_switch_alone_matches_old() {
    let t = tables();
    let names = ["LIGHTREADROWS", "LIGHTREADFILL", "LIGHTSCANPLANES", "LIGHTWRITECOLUMN", "LIGHTZEROONLYRING"];
    let base = old_read().with("ZONEBUF", true);
    let mut paths: Vec<(String, Opts)> = names.iter().map(|n| (format!("{n} alone"), base.with(n, true))).collect();
    paths.push(("rows and fill".into(), base.with("LIGHTREADROWS", true).with("LIGHTREADFILL", true)));
    let shipped = Opts::from_bits(!super::super::DEFAULT_OFF & ((1 << NAMES.len()) - 1) & !PAR_BITS);
    for n in names {
        // Off by default (`LIGHTZEROONLYRING`): added to the shipped set; on: taken out of it.
        let on = shipped.with(n, true) == shipped;
        paths.push((format!("shipped, {n} set to {}", !on), shipped.with(n, !on)));
    }
    for (label, o) in paths {
        for (seed, garbage) in [(71u64, false), (72, true)] {
            let case = Case { t: &t, height: 64, has_sky: true, garbage, steps: 300, seed, state: &state };
            compare(&case, |_| o).unwrap_or_else(|e| panic!("{label}, seed {seed}: {e}"));
        }
    }
}

/// Random levels over random sheet bytes (value, sheet bit, open bit), from no change to every
/// cell changed: the plane scan finds the cells of the stack scan, grouped and ordered alike, with
/// its scratch reused from one zone to the next; a grouping that loses its last stack is seen.
#[test]
fn plane_scan_finds_the_cells_of_the_stack_scan() {
    let found = compare_scans(17, 400).unwrap_or_else(|e| panic!("{e}"));
    assert!(found > 100_000, "too few changed cells to prove anything: {found}");
    SABOTAGE.with(|s| s.set(41));
    let caught = compare_scans(17, 400);
    SABOTAGE.with(|s| s.set(0));
    assert!(caught.is_err(), "last stack lost by the grouping not seen");
}

/// Changed cells found by both scans over `zones` random zones; the cells found, or the first
/// difference.
fn compare_scans(seed: u64, zones: usize) -> Result<usize, String> {
    let mut rng = Rng(seed);
    let (stacks, planes) = (Opts::OLD, Opts::OLD.with("LIGHTSCANPLANES", true));
    let mut kept = PlaneScan::default();
    let mut found = 0;
    for zone in 0..zones {
        let lo = [rng.below(40) - 20, rng.below(10), rng.below(40) - 20];
        let size = [3 + rng.below(50), 3 + rng.below(60), 3 + rng.below(50)];
        let g = Geo::new(lo, size);
        let changed_percent = [0, 1, 10, 100][rng.below(4) as usize];
        let n = g.cells();
        let (mut lay, mut level) = (vec![0u8; n], vec![0u8; n]);
        for (b, v) in lay.iter_mut().zip(level.iter_mut()) {
            let value = rng.below(16) as u8;
            *b = value | if rng.below(4) == 0 { 0 } else { DATA } | if rng.below(2) == 0 { OPEN } else { 0 };
            *v = if rng.below(100) < changed_percent { rng.below(16) as u8 } else { value };
        }
        let r0 = rng.below(size[1] as u64 - 1);
        let ny = 2 + rng.below((size[1] - r0 - 1) as u64);
        let (mut old, mut new) = (Changes::default(), Changes::default());
        find_changes(&stacks, &lay, &level, &mut PlaneScan::default(), g, r0, ny, &mut old);
        find_changes(&planes, &lay, &level, &mut kept, g, r0, ny, &mut new);
        if old.stacks != new.stacks || old.cells != new.cells {
            return Err(format!("zone {zone} {lo:?} + {size:?}, rows {r0} + {ny}: changes differ"));
        }
        found += old.cells.len();
    }
    Ok(found)
}

/// A world that counts the write accesses to its sheets.
struct CountedWrites {
    world: World,
    sheet_accesses: usize,
}

impl LightGrid for CountedWrites {
    fn column(&self, cx: i32, cz: i32) -> Option<(&[Section], &[LightLayer], &[LightLayer])> {
        self.world.column(cx, cz)
    }
    fn layers_mut(&mut self, cx: i32, cz: i32, sky: bool) -> Option<&mut [LightLayer]> {
        self.sheet_accesses += 1;
        self.world.layers_mut(cx, cz, sky)
    }
}

/// Random changes in stack order, absent columns included: written with the column kept, the same
/// sheets and touched sections as stack by stack, with one sheet access per run of stacks of a column
/// instead of one per stack. (The map is filled in the same order either way; its iteration order is
/// not observable anyway: each map draws its own hash seed.)
#[test]
fn column_kept_write_gives_the_same_sheets_and_sections_in_order() {
    let mut rng = Rng(23);
    let height = 64;
    let base = world(height, true, &mut rng);
    let (per_stack, kept) = (Opts::OLD, Opts::OLD.with("LIGHTWRITECOLUMN", true));
    for case in 0..200 {
        let mut ch = Changes::default();
        let (x0, z0) = (rng.below(30) - 30, rng.below(30) - 30);
        for dz in 0..1 + rng.below(40) {
            for dx in 0..1 + rng.below(60) {
                if rng.below(3) != 0 {
                    continue;
                }
                let mut y = rng.below(8) - 1;
                while y < height + 1 && rng.below(6) != 0 {
                    ch.cells.push((y, rng.below(16) as u8));
                    y += 1 + rng.below(20);
                }
                if ch.cells.len() > ch.stacks.last().map_or(0, |s| s.2) {
                    ch.stacks.push((x0 + dx, z0 + dz, ch.cells.len()));
                }
            }
        }
        let sky = rng.below(2) == 0;
        let (mut a, mut b) = (
            CountedWrites { world: base.clone(), sheet_accesses: 0 },
            CountedWrites { world: base.clone(), sheet_accesses: 0 },
        );
        let (mut ta, mut tb) = (Touched::new(), Touched::new());
        apply_changes(&mut a, &per_stack, &ch, sky, &mut ta);
        apply_changes(&mut b, &kept, &ch, sky, &mut tb);
        assert!(a.world == b.world, "case {case}: sheets");
        assert_eq!(ta, tb, "case {case}: touched sections");
        let columns = ch.stacks.chunk_by(|p, q| (p.0 >> 4, p.1 >> 4) == (q.0 >> 4, q.1 >> 4)).count();
        assert_eq!((a.sheet_accesses, b.sheet_accesses), (ch.stacks.len(), columns), "case {case}: sheet accesses");
    }
}
