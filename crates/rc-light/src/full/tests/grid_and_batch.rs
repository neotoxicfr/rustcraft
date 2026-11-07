//! Property grid (`grid`) and batch tiles (`batch`) = frozen copy, byte for byte, and each
//! deliberate error of theirs caught.

use hashbrown::HashMap;

use super::*;
use crate::full::batch::{SABOTAGE_TILE_TAKES_KEPT_BUFFERS, SABOTAGE_TILE_WITHOUT_MARGIN};
use crate::full::{SABOTAGE_BUFFERS_GROW_BY_DOUBLING, SABOTAGE_QUEUES_KEPT_OVER_THE_MAX};
use crate::full::grid::{
    SABOTAGE_ABSENT_COLUMN_OPEN, SABOTAGE_EMITTERS_IN_FILL_MISS_A_ROW, SABOTAGE_LEVEL_TEST_ASSUMES_OPACITY_2,
    SABOTAGE_NO_EMITTER_EXIT_MISSES_LEVEL_1, SABOTAGE_QUEUE_DIRECTIONS_DROP_FORWARD, SABOTAGE_SKIP_DEPTHS_WITHOUT_MARGIN,
    SABOTAGE_SKY_SEED_FILTER_FORGETS_BELOW,
};

const HEIGHT: i32 = 96;

/// The 256 combinations of the switches the property grid reads (layout on for all but two, so
/// that new buffers are tried too; interning on for half of them).
fn grid_combos() -> Vec<FullOpts> {
    (0..256u32)
        .map(|b| FullOpts {
            skip: b & 1 != 0,
            noemit: b & 2 != 0,
            occlusion_table: b & 4 != 0,
            queue_directions: b & 8 != 0,
            no_emitter_exit: b & 16 != 0,
            sky_seed_filter: b & 32 != 0,
            emitters_in_fill: b & 64 != 0,
            skip_depths: b & 128 != 0,
            layout: b != 21 && b != 234,
            intern_uniform: b.count_ones() % 2 == 1,
            property_grid: true,
            ..FullOpts::OLD
        })
        .collect()
}

/// Scans of the columns of a case.
fn scans_of_case(t: &LightTables, cols: &[Option<Vec<u32>>]) -> Vec<Option<ColumnScan>> {
    cols.iter().map(|c| c.as_deref().map(|c| scan_column(t, c, HEIGHT))).collect()
}

/// Each hand-made case seen by a quarter of the combinations, each random one by 8 (a 32nd), so
/// that the run stays short; every combination sees cases of both kinds.
#[test]
fn property_grid_matches_frozen_reference() {
    let t = tables();
    assert!(t.has_property_grid(), "the test tables must take the property grid");
    for (n, (cols, has_sky)) in cases(HEIGHT as usize).iter().enumerate() {
        let refs: Columns = std::array::from_fn(|i| cols[i].as_deref());
        let want = reference::light_chunk(&t, refs, HEIGHT, *has_sky);
        let owned = scans_of_case(&t, cols);
        let scans: Scans = std::array::from_fn(|i| owned[i].as_ref());
        for (b, o) in grid_combos().into_iter().enumerate() {
            let every = if n < RANDOM_CASES { 32 } else { 4 };
            if (n + b) % every != 0 {
                continue;
            }
            assert_eq!(light_chunk_opt(&t, refs, [None; 9], HEIGHT, *has_sky, o), want, "case {n}, {o:?}, without scan");
            assert_eq!(light_chunk_opt(&t, refs, scans, HEIGHT, *has_sky, o), want, "case {n}, {o:?}, scans");
        }
    }
}

/// Beyond 512 shapes there is no table of shape pairs (the occlusion table switch reads the masks),
/// beyond 4,095 no property grid (the old layout resolves): same sheets.
#[test]
fn property_grid_falls_back_beyond_its_shapes() {
    for (extra, grid) in [(600, true), (4_100, false)] {
        let t = tables_with_shapes(extra);
        assert!(t.occ.is_empty(), "{extra} more shapes: no table of shape pairs");
        assert_eq!(t.has_property_grid(), grid, "{extra} more shapes");
        for (n, (cols, has_sky)) in cases(HEIGHT as usize).iter().enumerate().step_by(4) {
            let refs: Columns = std::array::from_fn(|i| cols[i].as_deref());
            let want = reference::light_chunk(&t, refs, HEIGHT, *has_sky);
            assert_eq!(light_chunk_opt(&t, refs, [None; 9], HEIGHT, *has_sky, FullOpts::ALL), want, "case {n}, {extra} more shapes");
        }
    }
}

/// Number of cases whose sheets differ from the frozen copy, with or without scans, with the
/// deliberate error `k` on.
fn cases_broken_by(k: u8) -> usize {
    let t = tables();
    SABOTAGE.with(|s| s.set(k));
    let broken = cases(HEIGHT as usize)
        .iter()
        .filter(|(cols, has_sky)| {
            let refs: Columns = std::array::from_fn(|i| cols[i].as_deref());
            let owned = scans_of_case(&t, cols);
            let scans: Scans = std::array::from_fn(|i| owned[i].as_ref());
            let want = reference::light_chunk(&t, refs, HEIGHT, *has_sky);
            [[None; 9], scans].into_iter().any(|s| light_chunk_opt(&t, refs, s, HEIGHT, *has_sky, FullOpts::ALL) != want)
        })
        .count();
    SABOTAGE.with(|s| s.set(0));
    broken
}

#[test]
fn sabotages_break_the_property_grid_proof() {
    let names = [
        (SABOTAGE_LEVEL_TEST_ASSUMES_OPACITY_2, "level test assumes an opacity of 2"),
        (SABOTAGE_ABSENT_COLUMN_OPEN, "absent column open"),
        (SABOTAGE_SKY_SEED_FILTER_FORGETS_BELOW, "sky seed filter forgets the cell below"),
        (SABOTAGE_QUEUE_DIRECTIONS_DROP_FORWARD, "queue directions drop the forward step"),
        (SABOTAGE_NO_EMITTER_EXIT_MISSES_LEVEL_1, "no-emitter exit misses level 1"),
        (SABOTAGE_EMITTERS_IN_FILL_MISS_A_ROW, "emitters in the fill miss a row"),
        (SABOTAGE_SKIP_DEPTHS_WITHOUT_MARGIN, "depths skipped without a margin"),
    ];
    for (k, name) in names {
        let broken = cases_broken_by(k);
        eprintln!("sabotage {k} ({name}): {broken} cases differ");
        assert!(broken > 0, "sabotage {k} ({name}) not caught by the cases");
    }
    assert_eq!(cases_broken_by(0), 0, "no sabotage: every case equal");
}

/// Random columns over `[-5, 3) × [-4, 2)` chunks (some absent), and their scans.
struct World {
    columns: HashMap<(i32, i32), Vec<u32>>,
    scans: HashMap<(i32, i32), ColumnScan>,
}

impl World {
    fn new(t: &LightTables, seed: u64) -> World {
        let mut r = Rng(seed);
        let mut columns = HashMap::new();
        let ground = 20 + r.next(50) as usize;
        for z in -4..2 {
            for x in -5..3 {
                if r.next(9) == 0 {
                    continue;
                }
                let g = (ground as i64 + r.next(30) as i64 - 15).max(0) as usize;
                columns.insert((x, z), terrain(&mut r, HEIGHT as usize, g));
            }
        }
        let scans = columns.iter().map(|(&k, c)| (k, scan_column(t, c, HEIGHT))).collect();
        World { columns, scans }
    }

    fn present(&self) -> Vec<(i32, i32)> {
        let mut v: Vec<(i32, i32)> = self.columns.keys().copied().collect();
        v.sort_unstable();
        v
    }

    /// Batches: every present chunk, an L across two tiles, a sparse one (chunks lit alone), a
    /// single chunk, and one holding an absent chunk.
    fn batches(&self) -> Vec<Vec<(i32, i32)>> {
        let all = self.present();
        let l_shape: Vec<(i32, i32)> = (-5..2).map(|x| (x, -1)).chain((-4..-1).map(|z| (1, z))).collect();
        let sparse: Vec<(i32, i32)> = all.iter().copied().step_by(5).collect();
        let absent = (-5..3).flat_map(|x| (-4..2).map(move |z| (x, z))).find(|k| !self.columns.contains_key(k));
        let mut with_absent: Vec<(i32, i32)> = all.iter().copied().take(12).collect();
        with_absent.extend(absent);
        vec![all.clone(), l_shape, sparse, vec![all[all.len() / 2]], with_absent]
    }

    fn light(&self, t: &LightTables, chunks: &[(i32, i32)], has_sky: bool, scanned: bool, opts: FullOpts) -> Vec<LightSheets> {
        let column = |x: i32, z: i32| self.columns.get(&(x, z)).map(Vec::as_slice);
        let scan = |x: i32, z: i32| if scanned { self.scans.get(&(x, z)) } else { None };
        light_chunks(t, chunks, column, scan, HEIGHT, has_sky, opts)
    }

    /// Frozen copy's sheets of every chunk of the area, absent ones included.
    fn reference(&self, t: &LightTables, has_sky: bool) -> HashMap<(i32, i32), LightSheets> {
        let area = (-5..3).flat_map(|x| (-4..2).map(move |z| (x, z)));
        area.map(|(x, z)| {
            let refs: Columns =
                std::array::from_fn(|i| self.columns.get(&(x + i as i32 % 3 - 1, z + i as i32 / 3 - 1)).map(Vec::as_slice));
            ((x, z), reference::light_chunk(t, refs, HEIGHT, has_sky))
        })
        .collect()
    }

    /// Chunks of `chunks` whose sheets differ from `want`.
    fn differing(
        &self,
        t: &LightTables,
        chunks: &[(i32, i32)],
        (has_sky, scanned, opts): (bool, bool, FullOpts),
        want: &HashMap<(i32, i32), LightSheets>,
    ) -> Vec<(i32, i32)> {
        let got = self.light(t, chunks, has_sky, scanned, opts);
        chunks.iter().zip(got).filter(|&(c, ref sheets)| *sheets != want[c]).map(|(&c, _)| c).collect()
    }
}

#[test]
fn union_tiles_match_frozen_reference() {
    let t = tables();
    let variants = [
        FullOpts::ALL,
        FullOpts { skip: false, ..FullOpts::ALL },
        FullOpts { sky_seed_filter: false, queue_directions: false, ..FullOpts::ALL },
        FullOpts { layout: false, intern_uniform: false, ..FullOpts::ALL },
        FullOpts { emitters_in_fill: false, no_emitter_exit: false, ..FullOpts::ALL },
        FullOpts { union_tile_side: 2, ..FullOpts::ALL },
        FullOpts { union_tile_side: 3, ..FullOpts::ALL },
        FullOpts { union_tile_side: 4, ..FullOpts::ALL },
        FullOpts { union_tile_side: 8, ..FullOpts::ALL },
        // Without the property grid the batch is lit chunk by chunk.
        FullOpts { property_grid: false, ..FullOpts::ALL },
    ];
    for seed in 0..6 {
        let world = World::new(&t, 0x0c2_0015 ^ seed);
        for has_sky in [true, false] {
            let want = world.reference(&t, has_sky);
            for (b, batch) in world.batches().iter().enumerate() {
                for (v, &opts) in variants.iter().enumerate() {
                    for scanned in [false, true] {
                        let wrong = world.differing(&t, batch, (has_sky, scanned, opts), &want);
                        assert!(wrong.is_empty(), "seed {seed}, batch {b}, sky {has_sky}, variant {v}, scans {scanned}: {wrong:?}");
                    }
                }
            }
        }
    }
}

/// The union path is really taken: lighting a tile's zone without its margin of one chunk gives
/// other sheets (the deliberate error is only seen from the thread that sets it: one thread).
#[test]
fn sabotaged_tile_without_margin_breaks_the_union() {
    let t = tables();
    let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().expect("thread pool");
    let world = World::new(&t, 77);
    let all = world.present();
    let want = world.reference(&t, true);
    let broken = pool.install(|| {
        SABOTAGE.with(|s| s.set(SABOTAGE_TILE_WITHOUT_MARGIN));
        let wrong = world.differing(&t, &all, (true, true, FullOpts::ALL), &want);
        SABOTAGE.with(|s| s.set(0));
        wrong
    });
    eprintln!("tile without margin: {} chunks of {} differ", broken.len(), all.len());
    assert!(!broken.is_empty(), "tile without margin not caught");
    assert!(pool.install(|| world.differing(&t, &all, (true, true, FullOpts::ALL), &want)).is_empty());
}

/// A tile whose zone exceeds the buffers a thread keeps gets buffers of its own: the 3 × 3 buffers
/// its thread keeps stay in place for the next chunk lit alone (they were thrown away before).
#[test]
fn large_tile_leaves_the_kept_buffers_in_place() {
    let t = tables();
    // Above a 3 × 3 zone and its queues, under a tile of 4 with its margin (6 × 6 columns).
    let cap = 2 << 20;
    assert!(grid::zone_buffer_bytes(3, 3, HEIGHT) <= cap && grid::zone_buffer_bytes(6, 6, HEIGHT) > cap);
    let opts = FullOpts { max_kept_buffer_bytes: cap, union_tile_side: 4, ..FullOpts::ALL };
    let world = World::new(&t, 5);
    // One aligned tile of 4 × 4 chunks.
    let tile: Vec<(i32, i32)> = world.present().into_iter().filter(|&(x, z)| (-4..0).contains(&x) && z < 0).collect();
    assert!(tile.len() >= 5, "the tile is lit as one zone");
    let want = world.reference(&t, true);
    let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().expect("thread pool");
    let kept_before_and_after = |sabotage: u8| {
        pool.install(|| {
            drop(world.light(&t, &tile[..1], true, true, opts));
            let before = kept_buffer_bytes();
            SABOTAGE.with(|s| s.set(sabotage));
            let wrong = world.differing(&t, &tile, (true, true, opts), &want);
            SABOTAGE.with(|s| s.set(0));
            assert!(wrong.is_empty(), "sabotage {sabotage}: {wrong:?}");
            (before, kept_buffer_bytes())
        })
    };
    let (before, after) = kept_before_and_after(0);
    assert!(before > 0, "a chunk lit alone keeps its buffers");
    assert_eq!(after, before, "the tile left the kept buffers in place");
    let (before, after) = kept_before_and_after(SABOTAGE_TILE_TAKES_KEPT_BUFFERS);
    eprintln!("tile taking the kept buffers: {before} bytes kept before, {after} after");
    assert_eq!(after, 0, "sabotage not seen: the tile took the kept buffers and threw them away");
}

/// In a world 384 high, under the default maximum, the 3 × 3 buffers a thread keeps survive a zone
/// higher than the one before: grown exactly, not doubled (doubled after a zone cut short by the
/// void skip, they reached 5.6 MB, over the 4 MiB kept, and were thrown away).
#[test]
fn kept_buffers_survive_a_higher_zone() {
    const TALL: i32 = 384;
    let t = tables();
    assert!(grid::zone_buffer_bytes(3, 3, TALL) <= DEFAULT_KEPT_BUFFER_BYTES);
    let air = vec![AIR; 256 * TALL as usize];
    // One stone in the centre column: the void skip cuts the zone 30 rows above it at most.
    let centre_with_stone_at = |y: usize| {
        let mut c = air.clone();
        c[idx(8, y, 8)] = STONE;
        c
    };
    let (low, high) = (centre_with_stone_at(339), centre_with_stone_at(383));
    let light = |centre: &[u32]| {
        let mut cols: Columns = [Some(&air[..]); 9];
        cols[4] = Some(centre);
        drop(light_chunk_opt(&t, cols, [None; 9], TALL, true, FullOpts::ALL));
    };
    // Each run on a thread of its own: its kept buffers start empty.
    let kept_after_low_then_high = |sabotage: u8| {
        std::thread::scope(|s| {
            s.spawn(|| {
                SABOTAGE.with(|s| s.set(sabotage));
                light(&low);
                let after_low = kept_buffer_bytes();
                light(&high);
                (after_low, kept_buffer_bytes())
            })
            .join()
            .expect("lighting thread")
        })
    };
    let (low_kept, high_kept) = kept_after_low_then_high(0);
    eprintln!("kept after the low zone: {low_kept} bytes, after the higher one: {high_kept}");
    assert!(low_kept > 0 && high_kept >= grid::zone_buffer_bytes(3, 3, TALL), "the higher zone kept its buffers");
    assert!(high_kept <= DEFAULT_KEPT_BUFFER_BYTES);
    let (low_kept, high_kept) = kept_after_low_then_high(SABOTAGE_BUFFERS_GROW_BY_DOUBLING);
    eprintln!("buffers grown by doubling: {low_kept} bytes kept after the low zone, {high_kept} after the higher one");
    assert!(low_kept > 0);
    assert_eq!(high_kept, 0, "sabotage not seen: the doubled buffers went over the maximum and were thrown away");
}

/// A tile that fits the kept maximum but whose queues take its buffers over it gives back its
/// zone buffers without the queues, instead of throwing everything away.
#[test]
fn queues_over_the_kept_maximum_are_released_alone() {
    let t = tables();
    // The zone buffers of a tile of 4 (6 × 6 columns with its margin) fit exactly, its queues do not.
    let cap = grid::zone_buffer_bytes(6, 6, HEIGHT);
    let opts = FullOpts { max_kept_buffer_bytes: cap, union_tile_side: 4, ..FullOpts::ALL };
    let world = World::new(&t, 5);
    let tile: Vec<(i32, i32)> = world.present().into_iter().filter(|&(x, z)| (-4..0).contains(&x) && z < 0).collect();
    let want = world.reference(&t, true);
    // Each run on a pool of its own: its thread's kept buffers start empty.
    let kept_after_the_tile = |sabotage: u8| {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().expect("thread pool");
        pool.install(|| {
            SABOTAGE.with(|s| s.set(sabotage));
            let wrong = world.differing(&t, &tile, (true, true, opts), &want);
            SABOTAGE.with(|s| s.set(0));
            assert!(wrong.is_empty(), "sabotage {sabotage}: {wrong:?}");
            kept_buffer_bytes()
        })
    };
    let kept = kept_after_the_tile(0);
    eprintln!("kept after the tile: {kept} bytes of at most {cap}");
    assert!(kept > 0 && kept <= cap, "the zone buffers are kept without the queues");
    let kept = kept_after_the_tile(SABOTAGE_QUEUES_KEPT_OVER_THE_MAX);
    assert_eq!(kept, 0, "sabotage not seen: the queues kept took the buffers over the maximum");
}
