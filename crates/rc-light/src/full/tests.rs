//! New path of the full light = frozen copy (`reference`), byte for byte, for
//! all combinations of switches, with and without column scans.

use super::*;
use crate::reference;

/// Air, stone (full), leaves (attenuation 1), torch (14), slab (bottom face), glass
/// (neutral), sea lantern (15, full), lava (15, attenuation 1), cave air (neutral).
fn tables() -> LightTables {
    const F: [u64; 4] = [u64::MAX; 4];
    LightTables::from_parts(
        vec![0, 0, 0, 14, 0, 0, 15, 15, 0],
        vec![0, 15, 1, 0, 0, 0, 15, 1, 0],
        vec![0, 1, 0, 0, 2, 0, 1, 0, 0],
        vec![[[0; 4]; 6], [F; 6], [F, [0; 4], [0; 4], [0; 4], [0; 4], [0; 4]]],
    )
}

const AIR: u32 = 0;
const STONE: u32 = 1;
const TORCH: u32 = 3;
const LAVA: u32 = 7;
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
            _ => STONE,
        };
    }
    v
}

fn cases(height: usize) -> Vec<Case> {
    let mut r = Rng(0x0c2_3008);
    let mut out: Vec<Case> = Vec::new();
    // 200 sets of 9 random columns (varied ground heights, absent columns, no sky).
    for case in 0..200 {
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
    out
}

const COMBOS: [FullOpts; 8] = {
    let mut v = [FullOpts::OLD; 8];
    let mut i = 0;
    while i < 8 {
        v[i] = FullOpts { skip: i & 1 != 0, noemit: i & 2 != 0, layout: i & 4 != 0 };
        i += 1;
    }
    v
};

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
        for o in COMBOS {
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
