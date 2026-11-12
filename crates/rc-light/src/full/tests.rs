//! New paths of the full light = frozen copy (`reference`), byte for byte, for
//! all combinations of switches, with and without column scans, chunk by chunk and by tiles
//! of a batch; each deliberate error of the new paths must break the proof.

use super::*;
use crate::reference;

mod grid_and_batch;

/// Air, stone (full), leaves (attenuation 1), torch (14), slab (bottom face), glass
/// (neutral), sea lantern (15, full), lava (15, attenuation 1), cave air (neutral), mushroom
/// (1, no face).
fn tables() -> LightTables {
    tables_with_shapes(0)
}

/// [`tables`] with `extra` unused face shapes more: beyond 512 shapes there is no table of shape
/// pairs, beyond 4,095 no property grid.
fn tables_with_shapes(extra: usize) -> LightTables {
    const F: [u64; 4] = [u64::MAX; 4];
    let mut masks = vec![[[0; 4]; 6], [F; 6], [F, [0; 4], [0; 4], [0; 4], [0; 4], [0; 4]]];
    masks.extend((0..extra).map(|k| [[k as u64 + 1, 0, 0, 0]; 6]));
    LightTables::from_parts(
        vec![0, 0, 0, 14, 0, 0, 15, 15, 0, 1],
        vec![0, 15, 1, 0, 0, 0, 15, 1, 0, 0],
        vec![0, 1, 0, 0, 2, 0, 1, 0, 0, 0],
        masks,
    )
}

const AIR: u32 = 0;
const STONE: u32 = 1;
const TORCH: u32 = 3;
const LAVA: u32 = 7;
const MUSHROOM: u32 = 9;
/// State outside the tables (neither neutral nor known).
const UNKNOWN: u32 = 200;

struct Rng(u64);
impl Rng {
    fn next(&mut self, n: u64) -> u64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % n
    }
}

type Case = (Vec<Option<Vec<u32>>>, bool);

fn idx(x: usize, y: usize, z: usize) -> usize {
    (y << 8) | (z << 4) | x
}

/// Random terrain: solid ground up to a height per column, caves, sources, slabs…
fn terrain(r: &mut Rng, height: usize, ground: usize) -> Vec<u32> {
    let mut v = vec![AIR; 256 * height];
    let g = ground.min(height);
    for (k, s) in v.iter_mut().enumerate() {
        let y = k >> 8;
        if y >= g + (k & 7) {
            continue;
        }
        *s = match r.next(100) {
            0..=2 => TORCH,
            3 => 6,
            4..=6 => 4,
            7..=9 => 2,
            10..=30 => AIR,
            31..=33 => 5,
            34 => LAVA,
            35 => 8,
            36 if r.next(20) == 0 => UNKNOWN,
            37 if r.next(4) == 0 => MUSHROOM,
            _ => STONE,
        };
    }
    v
}

/// The random cases come first in [`cases`], the hand-made ones after.
const RANDOM_CASES: usize = 200;

fn cases(height: usize) -> Vec<Case> {
    let mut r = Rng(0x0c2_3008);
    let mut out: Vec<Case> = Vec::new();
    // 200 sets of 9 random columns (varied ground heights, absent columns, no sky).
    for case in 0..RANDOM_CASES {
        let ground = r.next(height as u64 + 8) as usize;
        let cols = (0..9)
            .map(|i| {
                if case % 9 == 4 && i == (case / 9) % 9 {
                    return None;
                }
                let g = (ground as i64 + r.next(30) as i64 - 15).max(0) as usize;
                Some(terrain(&mut r, height, g))
            })
            .collect();
        out.push((cols, case % 5 != 3));
    }
    let flat = |g: usize| -> Vec<u32> {
        let mut v = vec![AIR; 256 * height];
        for s in &mut v[..256 * g] {
            *s = STONE;
        }
        v
    };
    // All empty, and absent centre with full neighbours.
    out.push((vec![Some(vec![AIR; 256 * height]); 9], true));
    out.push(((0..9).map(|i| (i != 4).then(|| flat(20))).collect(), true));
    // Emitter on the highest non-neutral cell (the light climbs 14 cells in the void), in the
    // centre then in a neighbour.
    for at in [4usize, 0, 5] {
        let mut cols: Vec<Option<Vec<u32>>> = (0..9).map(|_| Some(flat(20))).collect();
        cols[at].as_mut().unwrap()[idx(7, 31, 9)] = TORCH;
        cols[at].as_mut().unwrap()[idx(7, 40, 9)] = 6;
        out.push((cols.clone(), true));
        out.push((cols, false));
    }
    // Emitter at the build limit, and lava at the top.
    for s in [TORCH, LAVA] {
        let mut cols: Vec<Option<Vec<u32>>> = (0..9).map(|_| Some(flat(10))).collect();
        cols[4].as_mut().unwrap()[idx(3, height - 1, 12)] = s;
        out.push((cols.clone(), true));
        out.push((cols, false));
    }
    // Empty centre, tall neighbour (tower up to the top).
    let mut cols: Vec<Option<Vec<u32>>> =
        (0..9).map(|i| Some(if i == 4 { vec![AIR; 256 * height] } else { flat(8) })).collect();
    for y in 0..height {
        cols[1].as_mut().unwrap()[idx(5, y, 15)] = STONE;
    }
    cols[1].as_mut().unwrap()[idx(4, height / 2, 15)] = TORCH;
    out.push((cols, true));
    // Shaft down to the bottom, torch at the bottom.
    let mut cols: Vec<Option<Vec<u32>>> = (0..9).map(|_| Some(flat(height - 20))).collect();
    let c = cols[4].as_mut().unwrap();
    for y in 1..height {
        c[idx(8, y, 8)] = AIR;
    }
    c[idx(8, 1, 8)] = TORCH;
    out.push((cols, true));
    // Nether: stone floor and ceiling, no sky; End: floating island, no sky.
    let mut cols: Vec<Option<Vec<u32>>> = (0..9).map(|_| Some(terrain(&mut r, height, 30))).collect();
    for c in cols.iter_mut().flatten() {
        for k in 0..256 {
            for y in height - 4..height {
                c[(y << 8) | k] = STONE;
            }
        }
    }
    out.push((cols.clone(), false));
    out.push((cols, true));
    let mut cols: Vec<Option<Vec<u32>>> = (0..9).map(|_| Some(vec![AIR; 256 * height])).collect();
    for k in 0..256 {
        for y in 40..50 {
            cols[4].as_mut().unwrap()[(y << 8) | k] = if k == 77 && y == 49 { TORCH } else { STONE };
        }
    }
    out.push((cols, false));
    // A whole section of sea lanterns over a stone floor in every column: block sheet all 15,
    // sky sheet all 0 under the cover (both uniform kinds, with and without sky).
    let mut cols: Vec<Option<Vec<u32>>> = (0..9).map(|_| Some(flat(16))).collect();
    for c in cols.iter_mut().flatten() {
        c[256 * 32..256 * 48].fill(6);
    }
    out.push((cols.clone(), true));
    out.push((cols, false));
    // A single level-1 emitter in the whole zone (its block sheet is not empty), in the centre
    // then in a neighbour; and a zone without any emitter.
    for at in [4usize, 2] {
        let mut cols: Vec<Option<Vec<u32>>> = (0..9).map(|_| Some(flat(20))).collect();
        cols[at].as_mut().unwrap()[idx(1, 20, 14)] = MUSHROOM;
        out.push((cols.clone(), true));
        out.push((cols, false));
    }
    out.push(((0..9).map(|_| Some(flat(30))).collect(), true));
    // Overhang over a pit: sky seeds at the source height that only light the cell below.
    let mut cols: Vec<Option<Vec<u32>>> = (0..9).map(|_| Some(flat(30))).collect();
    let c = cols[4].as_mut().unwrap();
    for y in 26..30 {
        c[idx(6, y, 6)] = AIR;
    }
    c[idx(6, 40, 6)] = STONE;
    out.push((cols, true));
    // Deep ground (the depths no light reaches are skipped): sky down a shaft, along a tunnel and
    // into a pit below the lowest source height; a torch in a cave far under the surface.
    let mut cols: Vec<Option<Vec<u32>>> = (0..9).map(|_| Some(flat(70))).collect();
    let c = cols[4].as_mut().unwrap();
    for y in 45..70 {
        c[idx(4, y, 4)] = AIR;
    }
    for x in 4..9 {
        c[idx(x, 45, 4)] = AIR;
        c[idx(x, 46, 4)] = AIR;
    }
    for y in 25..45 {
        c[idx(8, y, 4)] = AIR;
    }
    for z in 9..14 {
        for x in 6..12 {
            for y in 30..45 {
                c[idx(x, y, z)] = AIR;
            }
        }
    }
    c[idx(9, 40, 11)] = TORCH;
    out.push((cols.clone(), true));
    out.push((cols, false));
    // Pond: a layer of leaves (attenuation 1) level with the stone around it, lit only from above
    // (the sky seed at the source height lights the cell under it).
    let mut cols: Vec<Option<Vec<u32>>> = (0..9).map(|_| Some(flat(21))).collect();
    let c = cols[4].as_mut().unwrap();
    for z in 5..9 {
        for x in 5..9 {
            c[idx(x, 20, z)] = 2;
        }
    }
    out.push((cols, true));
    out
}

/// The 8 resolution combinations of the old layout; interning (only applied when the sheets are
/// packed) on for half of them, the all-off one excluded and the all-on one included, so that each
/// test run stays short.
const COMBOS: [FullOpts; 8] = {
    let mut v = [FullOpts::OLD; 8];
    let mut i = 0;
    while i < 8 {
        let intern_uniform = (i as u32).count_ones() % 2 == 1;
        v[i] = FullOpts { skip: i & 1 != 0, noemit: i & 2 != 0, layout: i & 4 != 0, intern_uniform, ..FullOpts::OLD };
        i += 1;
    }
    v
};

/// The old resolution path, with and without interning, also gives the frozen copy.
#[test]
fn light_chunk_inner_matches_frozen_reference() {
    let t = tables();
    let height = 96;
    for (n, (cols, has_sky)) in cases(height as usize).iter().enumerate() {
        let refs: Columns = std::array::from_fn(|i| cols[i].as_deref());
        let want = reference::light_chunk(&t, refs, height, *has_sky);
        for intern in [false, true] {
            assert_eq!(light_chunk_inner(&t, refs, height, *has_sky, intern), want, "case {n}, interned {intern}");
        }
    }
}

/// With interning, every all-0 or all-0xff sheet of both paths is a shared one and no other
/// sheet is; without it, none is shared. The cases must hold both kinds (explicit failure).
#[test]
fn uniform_sheets_shared_only_when_interned() {
    let t = tables();
    let height = 96;
    let (zero, full) = (rc_world::zero_layer(), rc_world::full_layer());
    let (mut zeros, mut fulls) = (0usize, 0usize);
    for (n, (cols, has_sky)) in cases(height as usize).iter().enumerate() {
        let refs: Columns = std::array::from_fn(|i| cols[i].as_deref());
        for intern in [false, true] {
            let fast = FullOpts { skip: true, noemit: true, layout: true, intern_uniform: intern, ..FullOpts::OLD };
            let on_grid = FullOpts { intern_uniform: intern, ..FullOpts::ALL };
            let new = light_chunk_opt(&t, refs, [None; 9], height, *has_sky, fast);
            let grid = light_chunk_opt(&t, refs, [None; 9], height, *has_sky, on_grid);
            let old = light_chunk_inner(&t, refs, height, *has_sky, intern);
            for (sky, block) in [new, grid, old] {
                for sheet in sky.iter().chain(&block).flatten() {
                    let is_zero = Arc::ptr_eq(sheet, &zero);
                    let is_full = Arc::ptr_eq(sheet, &full);
                    let uniform = rc_world::shared_layer_equal_to(sheet).is_some();
                    assert_eq!(is_zero || is_full, intern && uniform, "case {n}, interned {intern}");
                    zeros += usize::from(is_zero);
                    fulls += usize::from(is_full);
                }
            }
        }
    }
    assert!(zeros > 0 && fulls > 0, "cases without both uniform kinds: {zeros} zero, {fulls} full");
}

#[test]
fn light_chunk_opt_matches_frozen_reference() {
    let t = tables();
    let height = 96;
    let all = cases(height as usize);
    for (n, (cols, has_sky)) in all.iter().enumerate() {
        let refs: Columns = std::array::from_fn(|i| cols[i].as_deref());
        let want = reference::light_chunk(&t, refs, height, *has_sky);
        let owned: Vec<Option<ColumnScan>> =
            cols.iter().map(|c| c.as_deref().map(|c| scan_column(&t, c, height))).collect();
        let scans: Scans = std::array::from_fn(|i| owned[i].as_ref());
        // The property grid all on, then each of its own switches off alone (every combination:
        // `grid_and_batch`).
        let grid = [
            FullOpts::ALL,
            FullOpts { occlusion_table: false, ..FullOpts::ALL },
            FullOpts { queue_directions: false, ..FullOpts::ALL },
            FullOpts { no_emitter_exit: false, ..FullOpts::ALL },
            FullOpts { sky_seed_filter: false, ..FullOpts::ALL },
            FullOpts { emitters_in_fill: false, ..FullOpts::ALL },
        ];
        for o in COMBOS.into_iter().chain(grid) {
            // Twice in a row: thread buffers reused (`layout`).
            for _ in 0..2 {
                assert_eq!(
                    light_chunk_opt(&t, refs, [None; 9], height, *has_sky, o),
                    want,
                    "case {n}, {o:?}, without scan"
                );
                assert_eq!(light_chunk_opt(&t, refs, scans, height, *has_sky, o), want, "case {n}, {o:?}, scans");
            }
        }
    }
}

/// Raw light (larger of sky and block light) of cell `[x, y, z]` of the central chunk in the sheets
/// of a resolution: an absent sky sheet is full (dropped at the top), an absent block sheet is zero.
fn raw_light(sheets: &LightSheets, has_sky: bool, [x, y, z]: [i32; 3]) -> u8 {
    let section = (y >> 4) as usize + 1;
    let i = (((y & 15) << 8) | (z << 4) | x) as usize;
    let nibble = |l: &LightLayer, absent: u8| l.as_ref().map_or(absent, |a| (a[i / 2] >> ((i & 1) * 4)) & 15);
    nibble(&sheets.0[section], if has_sky { 15 } else { 0 }).max(nibble(&sheets.1[section], 0))
}

/// Answers of [`raw_light_at_least`] (`search_nearby` or not) that the resolution contradicts, over a
/// sample of the cells of every case at `threshold`; then the answers of each kind (light reached,
/// not reached, resolve) and the "not reached" ones that were tight (light one level under).
fn light_bound_mismatches(threshold: u8, search_nearby: bool) -> (usize, [usize; 4]) {
    let t = tables();
    let height = 96;
    let (mut wrong, mut kinds) = (0usize, [0usize; 4]);
    for (cols, has_sky) in cases(height as usize) {
        let refs: Columns = std::array::from_fn(|i| cols[i].as_deref());
        if refs[4].is_none() {
            assert_eq!(raw_light_at_least(&t, refs, height, has_sky, [0, 0, 0], threshold, search_nearby), None);
            continue;
        }
        let sheets = reference::light_chunk(&t, refs, height, has_sky);
        for y in 0..height {
            for z in 0..16 {
                for x in (0..16).filter(|x| (x * 7 + y * 3 + z * 5) % 11 == 0) {
                    let light = raw_light(&sheets, has_sky, [x, y, z]);
                    match raw_light_at_least(&t, refs, height, has_sky, [x, y, z], threshold, search_nearby) {
                        Some(true) => {
                            kinds[0] += 1;
                            wrong += usize::from(light < threshold);
                        }
                        Some(false) => {
                            kinds[1] += 1;
                            kinds[3] += usize::from(light + 1 == threshold);
                            wrong += usize::from(light >= threshold);
                        }
                        None => kinds[2] += 1,
                    }
                }
            }
        }
    }
    (wrong, kinds)
}

/// The bound never contradicts the resolution, at the mushrooms' threshold (13), below and above;
/// the cases hold every kind of answer, tight "not reached" ones included (explicit failure). With
/// the nearby search, it always answers, and exactly.
#[test]
fn light_bound_agrees_with_the_resolution() {
    for threshold in [12, 13, 14, 15] {
        let (wrong, kinds) = light_bound_mismatches(threshold, false);
        assert_eq!(wrong, 0, "threshold {threshold}: {wrong} answers contradicted ({kinds:?})");
        assert!(kinds.iter().all(|&n| n > 0), "threshold {threshold}: a kind of answer never met: {kinds:?}");
        let (wrong, kinds) = light_bound_mismatches(threshold, true);
        assert_eq!(wrong, 0, "threshold {threshold}, nearby search: {wrong} answers contradicted ({kinds:?})");
        assert_eq!(kinds[2], 0, "threshold {threshold}: the nearby search left cells unanswered");
        assert!(kinds[0] > 0 && kinds[3] > 0, "threshold {threshold}, nearby search: {kinds:?}");
    }
    let t = tables();
    let empty: Columns = [None; 9];
    assert_eq!(raw_light_at_least(&t, empty, 96, true, [0, 0, 0], 0, true), Some(true));
    assert_eq!(raw_light_at_least(&t, empty, 96, true, [0, 0, 0], 16, true), Some(false));
}

/// Each deliberate error (sources looked for one step too close, emitters asked for one level more,
/// the nearby search through occluded faces or one round short) makes the answers contradict the
/// resolution somewhere.
#[test]
fn sabotages_break_the_light_bound() {
    for (k, search_nearby) in [
        (SABOTAGE_BOUND_REACH_ONE_SHORT, false),
        (SABOTAGE_BOUND_EMITTER_ONE_LEVEL_HIGH, false),
        (SABOTAGE_SEARCH_IGNORES_OCCLUSION, true),
        (SABOTAGE_SEARCH_ONE_ROUND_SHORT, true),
    ] {
        SABOTAGE.with(|s| s.set(k));
        let (wrong, kinds) = light_bound_mismatches(13, search_nearby);
        SABOTAGE.with(|s| s.set(0));
        assert!(wrong > 0, "sabotage {k} not caught ({kinds:?})");
    }
}

/// The scan of a column follows the rule of the frozen copy: source height per stack equal to
/// that of the reference resolution (sky 15 above, nothing non-neutral higher).
#[test]
fn scan_column_tops_and_sources() {
    let t = tables();
    let height = 96;
    let mut r = Rng(7);
    for _ in 0..40 {
        let g = r.next(100) as usize;
        let c = terrain(&mut r, height as usize, g);
        let s = scan_column(&t, &c, height);
        let top = (0..c.len()).filter(|&k| !t.neutral(c[k])).map(|k| (k >> 8) as i32 + 1).max().unwrap_or(0);
        assert_eq!(s.top, top);
        let grid = Grid { cols: [None, None, None, None, Some(&c[..]), None, None, None, None], height };
        for k in 0..256 {
            let (x, z) = ((k & 15) as i32, (k >> 4) as i32);
            assert_eq!(s.src[k], source_height(&grid, &t, x + 16, z + 16), "stack {k}");
        }
    }
}
