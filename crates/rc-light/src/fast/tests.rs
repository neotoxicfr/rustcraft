//! Equivalence proof of the new path: sheets and touched sections identical, bit for bit,
//! to the old `relight_zones`, on random suites of changes (all combinations
//! of switches), and deliberate sabotages detected.

use super::*;
use crate::{FULL, LightLayer, Section};
use rc_world::LIGHT_BYTES;

const HALF_A: [u64; 4] = [u64::MAX, u64::MAX, 0, 0];
const HALF_B: [u64; 4] = [0, 0, u64::MAX, u64::MAX];
/// Low half-face (direction 0) and complementary high half-face (direction 1).
const HA0: u32 = 22;
const HB1: u32 = 29;
const LAMP_OFF: u32 = 6;
const LAMP_ON: u32 = 7;
const TORCH: u32 = 3;
const GLASS: u32 = 5;
/// Bit of `LIGHTDEDUP` in [`Opts::from_bits`].
const DEDUP: u16 = 512;

/// Test states: air, stone, leaves, torch, low slab, glass, lamp off and on
/// (differ only by emission), high slab, attenuations 2 to 14, half-faces in the six
/// directions (two complementary halves), attenuating emitter with half-face, emission 1.
pub(super) fn tables() -> LightTables {
    let mut emission = vec![0u8; 36];
    let mut dampening = vec![0u8; 36];
    let mut face = vec![0u32; 36];
    let mut masks = vec![[[0u64; 4]; 6], [FULL; 6]];
    let mut only = |dir: usize, m: [u64; 4]| {
        let mut f = [[0u64; 4]; 6];
        f[dir] = m;
        masks.push(f);
        (masks.len() - 1) as u32
    };
    let (down, up) = (only(0, FULL), only(1, FULL));
    let ha: Vec<u32> = (0..6).map(|d| only(d, HALF_A)).collect();
    let hb: Vec<u32> = (0..6).map(|d| only(d, HALF_B)).collect();
    dampening[1] = 15;
    face[1] = 1;
    dampening[2] = 1;
    emission[3] = 14;
    face[4] = down;
    dampening[6] = 15;
    face[6] = 1;
    emission[7] = 15;
    dampening[7] = 15;
    face[7] = 1;
    face[8] = up;
    for (k, s) in (9..=21).enumerate() {
        dampening[s] = 2 + k as u8;
    }
    face[22..28].copy_from_slice(&ha);
    face[28..34].copy_from_slice(&hb);
    emission[34] = 7;
    dampening[34] = 3;
    face[34] = ha[0];
    emission[35] = 1;
    LightTables::from_parts(emission, dampening, face, masks)
}

pub(super) struct Rng(u64);

impl Rng {
    pub(super) fn below(&mut self, n: u64) -> i32 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) % n) as i32
    }
}

type Col = (Vec<Section>, Vec<LightLayer>, Vec<LightLayer>);

#[derive(Clone, PartialEq)]
pub(super) struct World(pub(super) hashbrown::HashMap<(i32, i32), Col>);

impl LightGrid for World {
    fn column(&self, cx: i32, cz: i32) -> Option<(&[Section], &[LightLayer], &[LightLayer])> {
        self.0.get(&(cx, cz)).map(|(s, k, b)| (&s[..], &k[..], &b[..]))
    }
    fn layers_mut(&mut self, cx: i32, cz: i32, sky: bool) -> Option<&mut [LightLayer]> {
        self.0.get_mut(&(cx, cz)).map(|(_, k, b)| if sky { &mut k[..] } else { &mut b[..] })
    }
}

/// Columns −1..=2 × −1..=2 except two absent; `garbage`: random sheets (some absent)
/// and a section in direct identifiers, otherwise all air, full sky and zero block (consistent).
pub(super) fn world(height: i32, garbage: bool, rng: &mut Rng) -> World {
    let sections = (height / 16) as usize;
    let mut m = hashbrown::HashMap::new();
    for cx in -1..=2 {
        for cz in -1..=2 {
            if (cx, cz) == (2, 2) || (cx, cz) == (-1, 2) {
                continue;
            }
            let mut layer = |full: u8| -> LightLayer {
                if !garbage {
                    return Some(Arc::new([full; LIGHT_BYTES]));
                }
                match rng.below(5) {
                    0 => None,
                    1 => Some(Arc::new([full; LIGHT_BYTES])),
                    _ => Some(Arc::new(std::array::from_fn(|_| rng.below(256) as u8))),
                }
            };
            let sky: Vec<LightLayer> = (0..sections + 2).map(|_| layer(0xff)).collect();
            let block: Vec<LightLayer> = (0..sections + 2).map(|_| layer(0)).collect();
            let mut secs = vec![Section::empty(0, 0); sections];
            if garbage && (cx, cz) == (0, 0) {
                // Section in direct identifiers (overflowed palette).
                let values: Vec<u32> = (0..4096).map(|_| [0, 0, 0, 1, GLASS, 2][rng.below(6) as usize]).collect();
                secs[1].blocks = rc_world::Container::pack(&values, 1, 1, 15);
                assert!(matches!(secs[1].blocks, rc_world::Container::Packed { palette: None, .. }));
            }
            m.insert((cx, cz), (secs, sky, block));
        }
    }
    World(m)
}

pub(super) fn set(w: &mut World, p: [i32; 3], s: u32) -> Option<u32> {
    let (sections, _, _) = w.0.get_mut(&(p[0] >> 4, p[2] >> 4))?;
    let i = rc_world::block_index((p[0] & 15) as usize, (p[1] & 15) as usize, (p[2] & 15) as usize);
    let sec = sections.get_mut((p[1] >> 4) as usize)?;
    let old = sec.blocks.get(i);
    sec.set_blocks(&[(i as u16, s)], 15, |s| s == 0);
    Some(old)
}

/// Changes of one step: 1 to 4 (grouped or scattered), sometimes 20 to 50 (flowing water), sometimes
/// with duplicates; heights biased towards the top and bottom of the world.
fn changes(rng: &mut Rng, height: i32) -> Vec<[i32; 3]> {
    let count = match rng.below(20) {
        0 => 20 + rng.below(31),
        _ => 1 + rng.below(4),
    };
    let y = |rng: &mut Rng| match rng.below(10) {
        0 => height - 1 - rng.below(3),
        1 => rng.below(3),
        _ => rng.below(height as u64),
    };
    let center = [rng.below(60) - 14, y(rng), rng.below(60) - 14];
    let mut ps = Vec::new();
    for _ in 0..count {
        let spread: i32 = if rng.below(2) == 0 { 4 } else { 30 };
        ps.push([
            (center[0] + rng.below(spread as u64) - spread / 2).clamp(-16, 47),
            (center[1] + rng.below(spread as u64) - spread / 2).clamp(0, height - 1),
            (center[2] + rng.below(spread as u64) - spread / 2).clamp(-16, 47),
        ]);
        if rng.below(8) == 0 {
            ps.push(*ps.last().expect("a change"));
        }
    }
    ps
}

fn state(rng: &mut Rng) -> u32 {
    match rng.below(40) {
        0..=11 => 0,
        12..=17 => 1,
        18 => 999,
        _ => rng.below(36) as u32,
    }
}

/// Stacks "low half-face above a clear section, complementary high half-face
/// below": the edge only closes if the shape above the skipped section is forgotten.
fn script(height: i32) -> Vec<([i32; 3], u32)> {
    let mut out = Vec::new();
    if height < 64 {
        return out;
    }
    for (x, z) in [(3, 4), (20, 9), (-5, 30), (33, 33), (8, 40)] {
        out.push(([x, 48, z], HA0));
        out.push(([x, 31, z], HB1));
    }
    out
}

pub(super) struct Case<'a> {
    pub t: &'a LightTables,
    pub height: i32,
    pub has_sky: bool,
    pub garbage: bool,
    pub steps: usize,
    pub seed: u64,
    pub state: &'a dyn Fn(&mut Rng) -> u32,
}

/// Old and new path on two copies of the same world: sheets and touched sections
/// identical at every step. `pick`: switches of the step.
pub(super) fn compare(case: &Case, mut pick: impl FnMut(&mut Rng) -> Opts) -> Result<(), String> {
    let mut rng = Rng(case.seed);
    let mut a = world(case.height, case.garbage, &mut rng);
    let mut b = a.clone();
    for step in 0..case.steps {
        let ps: Vec<[i32; 3]> = if step == 0 {
            let s = script(case.height);
            for &(p, st) in &s {
                set(&mut a, p, st);
                set(&mut b, p, st);
            }
            s.iter().map(|&(p, _)| p).collect()
        } else {
            let ps = changes(&mut rng, case.height);
            for &p in &ps {
                let s = (case.state)(&mut rng);
                set(&mut a, p, s);
                set(&mut b, p, s);
            }
            ps
        };
        if ps.is_empty() {
            continue;
        }
        let o = pick(&mut rng);
        let ta = crate::relight_zones(&mut a, case.height, case.t, case.has_sky, &ps, true);
        let mut out = [Touched::new()];
        relight_zones(&mut b, case.height, case.t, case.has_sky, &ps, None, o, &mut out);
        if a != b {
            return Err(format!("sheets differ at step {step} ({} changes, {o:?})", ps.len()));
        }
        if ta != out[0] {
            return Err(format!("touched sections differ at step {step} ({o:?})"));
        }
    }
    Ok(())
}

/// Random switches, except `LIGHTDEDUP` (the grouping changes, hence the repair of
/// inconsistent data too); one step in four, all of them.
fn random_opts(rng: &mut Rng) -> Opts {
    if rng.below(4) == 0 {
        Opts::from_bits(!DEDUP)
    } else {
        Opts::from_bits(rng.below(1 << 11) as u16 & !DEDUP)
    }
}

#[test]
fn new_path_matches_old_bit_for_bit() {
    let t = tables();
    for (seed, has_sky, garbage, steps, height) in [
        (1u64, true, false, 2000, 64),
        (2, true, true, 2000, 64),
        (3, false, false, 600, 64),
        (4, false, true, 600, 64),
        (5, true, true, 600, 48),
    ] {
        let case = Case { t: &t, height, has_sky, garbage, steps, seed, state: &state };
        if let Err(e) = compare(&case, random_opts) {
            panic!("seed {seed} (sky {has_sky}, random data {garbage}): {e}");
        }
    }
}

/// Shipped configuration (switches absent from the environment): bit for bit the old path,
/// sheets and touched sections, on consistent and random data.
fn shipped() -> Opts {
    Opts::from_bits(!super::DEFAULT_OFF & ((1 << NAMES.len()) - 1))
}

#[test]
fn shipped_defaults_match_old() {
    let t = tables();
    for (seed, has_sky, garbage) in [(21u64, true, false), (22, true, true), (23, false, true)] {
        let case = Case { t: &t, height: 64, has_sky, garbage, steps: 1500, seed, state: &state };
        compare(&case, |_| shipped()).unwrap_or_else(|e| panic!("seed {seed} (random {garbage}): {e}"));
    }
}

/// Buffers kept per thread: purged beyond `RETAIN_BYTES`, buckets included.
#[test]
fn retained_buffers_are_bounded() {
    super::with_work(true, |w| w.lane.buckets[15].reserve(super::RETAIN_BYTES / 8 + 1));
    super::WORK.with(|cell| {
        let b = cell.borrow().bytes();
        assert!(b <= super::RETAIN_BYTES, "{b} bytes kept");
    });
}

/// Touched sections: all those where a cell changed, with the edges of those cells.
fn covers(before: &World, after: &World, touched: &Touched) -> Result<(), String> {
    for (&(cx, cz), (_, sky, block)) in &after.0 {
        let (_, osky, oblock) = &before.0[&(cx, cz)];
        for (new, old) in [(sky, osky), (block, oblock)] {
            for (li, (n, o)) in new.iter().zip(old).enumerate() {
                let (Some(n), Some(o)) = (n, o) else { continue };
                for i in 0..4096 {
                    let (x, y, z) = (i & 15, i >> 8, (i >> 4) & 15);
                    if nibble(n, x, y, z) != nibble(o, x, y, z) {
                        let bits = rc_world::edge::of_index(i);
                        let got = touched.get(&(cx, li as i32 - 1, cz)).copied().unwrap_or(0);
                        if got & bits != bits {
                            return Err(format!("section {:?} wrongly reported", (cx, li as i32 - 1, cz)));
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// Consistent world (fixed point held at every step): changes sorted, deduplicated and in
/// any order give the same sheets as the old path on the original order.
#[test]
fn dedup_and_order_keep_the_fixed_point() {
    let t = tables();
    let mut rng = Rng(77);
    let height = 64;
    let mut a = world(height, false, &mut rng);
    let mut b = a.clone();
    for step in 0..800 {
        let mut ps = changes(&mut rng, height);
        for &p in &ps {
            let s = state(&mut rng);
            set(&mut a, p, s);
            set(&mut b, p, s);
        }
        let ta = crate::relight_zones(&mut a, height, &t, true, &ps, true);
        // Shuffled order for the new path.
        for i in (1..ps.len()).rev() {
            ps.swap(i, rng.below(i as u64 + 1) as usize);
        }
        let before = b.clone();
        let mut out = [Touched::new()];
        relight_zones(&mut b, height, &t, true, &ps, None, Opts::ALL, &mut out);
        assert!(a == b, "sheets differ at step {step}");
        covers(&before, &b, &out[0]).unwrap_or_else(|e| panic!("step {step}: {e}"));
        covers(&before, &a, &ta).unwrap_or_else(|e| panic!("step {step} (old): {e}"));
    }
}

/// Consistent world: sky resolved around only the changes that touch it (lamps, torches
/// replaced by glass: emission only) = old path that resolves the sky everywhere.
#[test]
fn sky_skipped_when_only_emission_changes() {
    let t = tables();
    let mut rng = Rng(91);
    let height = 64;
    let mut a = world(height, false, &mut rng);
    let mut b = a.clone();
    let mut skipped = 0;
    for step in 0..800 {
        let ps = changes(&mut rng, height);
        let mut sky_ps = Vec::new();
        for &p in &ps {
            let s = match rng.below(3) {
                0 => [LAMP_OFF, LAMP_ON, TORCH, GLASS][rng.below(4) as usize],
                _ => state(&mut rng),
            };
            let old = set(&mut a, p, s).unwrap_or(0);
            set(&mut b, p, s);
            if t.matters_sky(old, s) {
                sky_ps.push(p);
            }
        }
        skipped += ps.len() - sky_ps.len();
        crate::relight_zones(&mut a, height, &t, true, &ps, true);
        let before = b.clone();
        let o = if step % 2 == 0 { Opts::ALL } else { Opts::from_bits(4) };
        let mut out = [Touched::new(), Touched::new()];
        relight_zones(&mut b, height, &t, true, &ps, Some(&sky_ps), o, &mut out);
        assert!(a == b, "sheets differ at step {step} ({o:?})");
        let mut all = out[0].clone();
        for (k, v) in &out[1] {
            *all.entry(*k).or_insert(0) |= v;
        }
        covers(&before, &b, &all).unwrap_or_else(|e| panic!("step {step}: {e}"));
    }
    assert!(skipped > 300, "too few changes without effect on the sky: {skipped}");
    // Properties: the lamp does not count for the sky, the stone does.
    assert!(!t.matters_sky(LAMP_OFF, LAMP_ON) && t.matters(LAMP_OFF, LAMP_ON));
    assert!(!t.matters_sky(TORCH, GLASS) && t.matters_sky(0, 1) && t.matters_sky(0, 4) && t.matters_sky(0, 9));
    // Unknown state: opacity 15 for propagation, zero attenuation for the columns.
    assert!(t.matters_sky(0, 999) && t.matters_sky(1, 999));
}

/// Touched sections split sky / block: their union = the common list, on the old and
/// new path.
#[test]
fn split_union_matches_combined() {
    let t = tables();
    let mut rng = Rng(5);
    let height = 64;
    let w0 = world(height, true, &mut rng);
    let (mut a, mut b, mut c, mut d) = (w0.clone(), w0.clone(), w0.clone(), w0);
    for step in 0..300 {
        let ps = changes(&mut rng, height);
        for &p in &ps {
            let s = state(&mut rng);
            for w in [&mut a, &mut b, &mut c, &mut d] {
                set(w, p, s);
            }
        }
        let one = crate::relight_zones(&mut a, height, &t, true, &ps, true);
        let mut old = [Touched::new(), Touched::new()];
        crate::relight_zones_split(&mut b, height, &t, true, &ps, &mut old);
        let mut new = [Touched::new(), Touched::new()];
        relight_zones(&mut c, height, &t, true, &ps, None, Opts::from_bits(!DEDUP), &mut new);
        let mut new1 = [Touched::new()];
        relight_zones(&mut d, height, &t, true, &ps, None, Opts::from_bits(!DEDUP), &mut new1);
        assert!(a == b && b == c && c == d, "sheets at step {step}");
        for split in [&old, &new] {
            let mut u = split[0].clone();
            for (k, v) in &split[1] {
                *u.entry(*k).or_insert(0) |= v;
            }
            assert_eq!(u, one, "union at step {step}");
        }
        assert_eq!(new1[0], one, "step {step}");
        assert_eq!(old, new, "step {step}");
    }
}

/// Each sabotage of the new path is seen by the comparison (difference or panic).
#[test]
fn sabotages_are_caught() {
    let t = tables();
    let names = [
        "",
        "section skip of 17",
        "shape above kept after the skip",
        "no sentinel ring",
        "block slice shifted by one row",
        "queue: arrival direction removed instead of the return one",
        "memo: first blocked edge shifted by one cell",
        "section in direct identifiers taken for clear",
    ];
    for (k, name) in names.iter().enumerate().skip(1) {
        let caught = (0..6u64).any(|seed| {
            SABOTAGE.with(|s| s.set(k as u8));
            let garbage = seed % 2 == 1;
            let case = Case { t: &t, height: 64, has_sky: true, garbage, steps: 400, seed, state: &state };
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                // Without the multi-core paths: the sabotage is only seen from the thread that sets it.
                compare(&case, |_| Opts::from_bits(!DEDUP & !PAR_BITS))
            }));
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

/// Real 26.3 data (if present): game tables, states drawn from the whole registry.
#[test]
fn real_tables_match_old() {
    let Some(d) = rc_testdata::version_data() else {
        eprintln!("26.3 data absent: test ignored");
        return;
    };
    let t = LightTables::new(&d);
    eprintln!("face shapes: {}, pair table: {} B", t.masks.len(), t.occ.len());
    let n = d.states.len() as u64;
    let pick = move |rng: &mut Rng| if rng.below(3) == 0 { 0 } else { rng.below(n) as u32 };
    for (seed, garbage) in [(11u64, false), (12, true)] {
        let case = Case { t: &t, height: 64, has_sky: true, garbage, steps: 800, seed, state: &pick };
        compare(&case, random_opts).unwrap_or_else(|e| panic!("seed {seed}: {e}"));
        let case = Case { seed: seed + 10, ..case };
        compare(&case, |_| shipped()).unwrap_or_else(|e| panic!("seed {} (shipped): {e}", seed + 10));
    }
}

/// Table of occluded edges = computation on the masks, for all pairs and directions.
#[test]
fn occlusion_table_matches_masks() {
    let t = tables();
    let n = t.masks.len() as u32;
    for a in 0..n {
        for b in 0..n {
            for dir in 0..6 {
                assert_eq!(t.occluded_faces(a, b, dir, true), t.occluded_faces(a, b, dir, false), "{a} {b} {dir}");
            }
        }
    }
}

/// Bits of the multi-core switches (`LIGHTJOIN`, `LIGHTPARSOLVE`, `LIGHTGROUPS`).
const PAR_BITS: u16 = 2048 | 4096 | 8192;

/// Random switches (without `LIGHTDEDUP`), multi-core on one step in two, thresholds at 0
/// so that the small zones of the tests go through `par.rs`.
fn par_opts(rng: &mut Rng) -> Opts {
    let mut b = rng.below(1 << 14) as u16 & !DEDUP;
    if rng.below(2) == 0 {
        b |= PAR_BITS;
    }
    Opts { join_min: 0, par_min: 0, chunk: 64, ..Opts::from_bits(b) }
}

pub(super) fn pool(threads: usize) -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new().num_threads(threads).build().expect("thread pool")
}

/// Multi-core paths in a pool of 1 then of 32 threads: sheets and touched sections
/// identical, bit for bit, to the old path.
#[test]
fn parallel_paths_match_old_with_1_and_32_threads() {
    let t = tables();
    for threads in [1, 32] {
        pool(threads).install(|| {
            for (seed, has_sky, garbage, steps, height) in
                [(31u64, true, false, 700, 64), (32, true, true, 700, 64), (33, false, true, 300, 64), (34, true, true, 300, 48)]
            {
                let case = Case { t: &t, height, has_sky, garbage, steps, seed, state: &state };
                compare(&case, par_opts).unwrap_or_else(|e| panic!("{threads} threads, seed {seed} (random {garbage}): {e}"));
            }
        });
    }
}

/// Sky limited to the changes that touch it (block and sky in separate groups): multi-core =
/// same path without multi-core, sheets and two lists of sections.
#[test]
fn parallel_masked_matches_sequential() {
    let t = tables();
    let mut rng = Rng(41);
    let height = 64;
    let mut a = world(height, true, &mut rng);
    let mut b = a.clone();
    let base = Opts::from_bits(!DEDUP & !PAR_BITS & ((1 << NAMES.len()) - 1));
    let par = Opts { join_min: 0, par_min: 0, chunk: 64, ..Opts::from_bits(!DEDUP) };
    pool(8).install(|| {
        for step in 0..500 {
            let ps = changes(&mut rng, height);
            let mut sky_ps = Vec::new();
            for &p in &ps {
                let s = if rng.below(2) == 0 { [LAMP_OFF, LAMP_ON, TORCH][rng.below(3) as usize] } else { state(&mut rng) };
                let old = set(&mut a, p, s).unwrap_or(0);
                set(&mut b, p, s);
                if t.matters_sky(old, s) {
                    sky_ps.push(p);
                }
            }
            let mut oa = [Touched::new(), Touched::new()];
            relight_zones(&mut a, height, &t, true, &ps, Some(&sky_ps), base, &mut oa);
            let mut ob = [Touched::new(), Touched::new()];
            relight_zones(&mut b, height, &t, true, &ps, Some(&sky_ps), par, &mut ob);
            assert!(a == b, "sheets at step {step}");
            assert_eq!(oa, ob, "sections at step {step}");
        }
    });
}

fn ctx(t: &LightTables) -> Ctx<'_> {
    Ctx { height: 64, t, o: Opts { join_min: 0, par_min: 0, chunk: 64, ..Opts::from_bits(!DEDUP) } }
}

/// Scheduling: groups 32 blocks apart or more at the same level, overlapping groups
/// (9 to 31 blocks apart) one after the other, for the sky as for the block; then sheets
/// identical to the old path on these scenes, in a pool of 32 threads.
#[test]
fn group_schedule_far_and_overlapping() {
    let t = tables();
    let c = ctx(&t);
    let unit = |ps: &[[i32; 3]], block, sky| (ps.to_vec(), block, sky);
    // Far apart: x = -12 and x = 40 (zones -28..4 and 24..56 disjoint), and a 3rd above the 1st.
    let far = [unit(&[[-12, 30, 0]], true, true), unit(&[[40, 30, 30]], true, true)];
    assert_eq!(super::par::unit_levels(&c, &far), [0, 0]);
    // Zones 33 wide: the writing (without margin) of one crosses the reading of the other up to
    // 31 blocks apart.
    for d in [32, 33, 40] {
        let apart = [unit(&[[0, 30, 0]], true, true), unit(&[[d, 30, 0]], true, true)];
        assert_eq!(super::par::unit_levels(&c, &apart), [0, 0], "gap {d}");
    }
    for d in [9, 20, 31] {
        let near = [unit(&[[0, 30, 0]], true, true), unit(&[[d, 30, 0]], true, true)];
        assert_eq!(super::par::unit_levels(&c, &near), [0, 1], "gap {d}");
        // Block alone at distant heights: block zones disjoint in y.
        let apart_y = [unit(&[[0, 2, 0]], true, false), unit(&[[d, 60, 0]], true, false)];
        assert_eq!(super::par::unit_levels(&c, &apart_y), [0, 0], "block, gap {d}");
        // Sky: zone taken down to the bottom of the world, hence in conflict.
        let sky_y = [unit(&[[0, 2, 0]], false, true), unit(&[[d, 60, 0]], false, true)];
        assert_eq!(super::par::unit_levels(&c, &sky_y), [0, 1], "sky, gap {d}");
        // Block against sky: different sheets, never in conflict.
        let mixed = [unit(&[[0, 30, 0]], true, false), unit(&[[d, 30, 0]], false, true)];
        assert_eq!(super::par::unit_levels(&c, &mixed), [0, 0], "block against sky, gap {d}");
    }
    // Chain: 0 and 2 far apart, 1 overlaps both (2 therefore waits for 1).
    let chain = [unit(&[[-12, 30, 0]], true, true), unit(&[[14, 30, 0]], true, true), unit(&[[40, 30, 0]], true, true)];
    assert_eq!(super::par::unit_levels(&c, &chain), [0, 1, 2]);
    let fan = [unit(&[[-12, 30, 0]], true, true), unit(&[[40, 30, 0]], true, true), unit(&[[14, 30, 0]], true, true)];
    assert_eq!(super::par::unit_levels(&c, &fan), [0, 0, 1]);
    // Result on the scenes: torches and stones placed at the positions of the groups.
    let scenes: [&[[i32; 3]]; 3] =
        [&[[-12, 30, 0], [40, 30, 30], [-12, 50, 40]], &[[0, 30, 0], [9, 30, 0], [20, 31, 2], [33, 30, 0]], &[[-12, 30, 0], [14, 30, 0], [40, 30, 0]]];
    let mut rng = Rng(5);
    let base = world(64, true, &mut rng);
    pool(32).install(|| {
        for (n, ps) in scenes.iter().enumerate() {
            for s in [TORCH, 1, LAMP_ON, 0] {
                let (mut a, mut b) = (base.clone(), base.clone());
                for &p in *ps {
                    set(&mut a, p, s);
                    set(&mut b, p, s);
                }
                let ta = crate::relight_zones(&mut a, 64, &t, true, ps, true);
                let mut out = [Touched::new()];
                relight_zones(&mut b, 64, &t, true, ps, None, c.o, &mut out);
                assert!(a == b, "scene {n}, state {s}: sheets");
                assert_eq!(ta, out[0], "scene {n}, state {s}: sections");
            }
        }
    });
}

/// Sabotages of the multi-core paths seen by the comparison: overlapping groups launched in
/// parallel (8), block written into the sky sheet after `LIGHTJOIN` (9), a share of the parallel
/// queue lost (10), last slice of the parallel read forgotten (11). In a pool of 4 threads, the sabotage set on the thread that runs it.
#[test]
fn parallel_sabotages_are_caught() {
    let t = tables();
    let names = [
        (8u8, "overlapping groups in parallel"),
        (9, "block written into the sky"),
        (10, "parallel queue truncated"),
        (11, "last slice of the parallel read forgotten"),
    ];
    let all = Opts { join_min: 0, par_min: 0, chunk: 64, ..Opts::from_bits(!DEDUP) };
    for (k, name) in names {
        let caught = pool(4).install(|| {
            (0..6u64).any(|seed| {
                SABOTAGE.with(|s| s.set(k));
                let case = Case { t: &t, height: 64, has_sky: true, garbage: seed % 2 == 1, steps: 300, seed, state: &state };
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| compare(&case, |_| all)));
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
            })
        });
        assert!(caught, "sabotage {k} ({name}) not detected");
    }
}

/// Shipped values seen by the tests and benches: parallel propagation in tasks of 2048 entries
/// (the benches measure what runs in game), `LIGHTJOIN` off by default (no measured gain).
#[test]
fn shipped_chunk_and_join_default() {
    assert_eq!(Opts::from_bits(u16::MAX).chunk, 2048);
    let k = NAMES.iter().position(|n| *n == "LIGHTJOIN").expect("name");
    assert_ne!(super::DEFAULT_OFF & 1 << k, 0, "LIGHTJOIN off by default");
}

/// `ZONEONCE=0` keeps two zone reads per group (block then sky) even with `LIGHTJOIN` or
/// `LIGHTGROUPS` on; `ZONEONCE=1` a single one. One-thread pool: everything on the same counter.
#[test]
fn zone_once_off_reads_twice_with_join_and_groups() {
    let t = tables();
    let mut rng = Rng(9);
    let base = world(64, true, &mut rng);
    let zo = 1u16 << NAMES.iter().position(|n| *n == "ZONEONCE").expect("name");
    let one = [[0, 30, 0]];
    let far = [[-12, 30, 0], [40, 30, 30]];
    pool(1).install(|| {
        for extra in [2048u16, 8192, 2048 | 8192] {
            for once in [false, true] {
                let bits = (!DEDUP & !PAR_BITS & !zo & ((1 << NAMES.len()) - 1)) | extra | if once { zo } else { 0 };
                let o = Opts { join_min: 0, par_min: usize::MAX, ..Opts::from_bits(bits) };
                for ps in [&one[..], &far[..]] {
                    let mut w = base.clone();
                    for &p in ps {
                        set(&mut w, p, 1);
                    }
                    super::READS.with(|r| r.set(0));
                    let mut out = [Touched::new()];
                    relight_zones(&mut w, 64, &t, true, ps, None, o, &mut out);
                    let n = super::READS.with(|r| r.get());
                    let want = ps.len() * if once { 1 } else { 2 };
                    assert_eq!(n, want, "bits {extra}, ZONEONCE {once}, {} groups", ps.len());
                }
            }
        }
    });
}

mod bench;
mod inc;
