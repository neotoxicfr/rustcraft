//! Proofs of the incremental relight (`LIGHTINC`): unchecked (`LIGHTINCVERIFY=0`, `fast/inc.rs`),
//! sheets and touched sections identical, bit for bit, to the old path on consistent worlds (the
//! fixed point held at every step), sabotages seen, and difference reported on inconsistent sheets;
//! checked (`fast/checked_inc.rs`, shipped), identical everywhere, kept on every resolution of the
//! consistent worlds and handed back to the zone on inconsistent sheets.

use super::*;
use crate::fast::checked_inc::{self, CHECKS};
use crate::fast::inc::{
    CHECKED_GROUP_MAX, SABOTAGE_EMISSION_NOT_RESTORED, SABOTAGE_OVERLAPPING_GROUPS_TAKEN_AS_ISOLATED,
    SABOTAGE_REST_RESOLVED_IN_SERIES, SABOTAGE_SKY_CANDIDATES_NOT_SORTED_BY_BLOCK_ZONE, SABOTAGE_SKY_COLUMN_NOT_WALKED,
    SABOTAGE_STACK_NOT_FROM_ITS_HIGHEST_CHANGE, SOLVES,
};

/// Switches of the zone at random (without `LIGHTDEDUP`), unchecked incremental on, group size
/// threshold sometimes low (the large groups then go through the zone among the small ones).
fn inc_opts(rng: &mut Rng) -> Opts {
    Opts { inc_verify: false, ..checked_opts(rng) }
}

/// [`inc_opts`] with the check on.
fn checked_opts(rng: &mut Rng) -> Opts {
    let zone = random_opts(rng);
    let inc_max = if rng.below(4) == 0 { 3 } else { usize::MAX };
    Opts { inc: true, inc_verify: true, inc_max, ..zone }
}

/// The unchecked incremental with the other switches of `o`.
fn unchecked(o: Opts) -> Opts {
    Opts { inc: true, inc_verify: false, ..o }
}

fn solves() -> usize {
    SOLVES.with(|s| s.get())
}

/// Checked resolutions of this thread so far: (kept, handed back to the zone).
fn checks() -> (usize, usize) {
    CHECKS.with(|n| n.get())
}

/// Checked resolutions since `before`.
fn checks_since(before: (usize, usize)) -> (usize, usize) {
    let now = checks();
    (now.0 - before.0, now.1 - before.1)
}

/// 10,000 random steps (placing and removing sources, opaques, slabs, half-faces, roofs,
/// sky wells, flowing water in groups of 20 to 50) on consistent worlds.
const COHERENT_SUITES: [(u64, bool, usize, i32); 4] =
    [(101, true, 3000, 64), (102, true, 3000, 64), (103, false, 2000, 64), (104, true, 2000, 48)];

#[test]
fn unchecked_incremental_matches_old_on_coherent_worlds() {
    let t = tables();
    let n0 = solves();
    for (seed, has_sky, steps, height) in COHERENT_SUITES {
        let case = Case { t: &t, height, has_sky, garbage: false, steps, seed, state: &state };
        compare(&case, inc_opts).unwrap_or_else(|e| panic!("seed {seed} (sky {has_sky}): {e}"));
    }
    let n = solves() - n0;
    // The incremental must really be used (otherwise the proof proves nothing).
    assert!(n > 5000, "only {n} incremental resolutions");
}

/// The same suites checked: identical to the old path, and every checked resolution kept (the
/// incremental gives the zone's fixed point on these worlds, so a fallback is a defect of the
/// incremental or of the check).
#[test]
fn incremental_matches_old_on_coherent_worlds() {
    let t = tables();
    let before = checks();
    for (seed, has_sky, steps, height) in COHERENT_SUITES {
        let case = Case { t: &t, height, has_sky, garbage: false, steps, seed, state: &state };
        compare(&case, checked_opts).unwrap_or_else(|e| panic!("seed {seed} (sky {has_sky}): {e}"));
    }
    let (kept, fallbacks) = checks_since(before);
    assert!(kept > 5000, "only {kept} checked resolutions kept");
    assert_eq!(fallbacks, 0, "checked resolutions handed back to the zone on consistent worlds");
}

/// Written scene: emitters next to a brighter source that is removed (the torch, emptied by the
/// decrease, must get its emission back), then putting back and removing a roof above them.
fn emitters_scene(o: Opts) -> Result<(), String> {
    let t = tables();
    let height = 64;
    let mut rng = Rng(7);
    let mut a = world(height, false, &mut rng);
    let mut b = a.clone();
    let steps: [&[([i32; 3], u32)]; 6] = [
        &[([5, 20, 5], LAMP_ON), ([6, 20, 5], TORCH), ([5, 20, 6], 34), ([4, 20, 5], 35)],
        &[([5, 20, 5], 0)],
        &[([5, 20, 5], LAMP_ON)],
        &[([5, 23, 5], 1), ([6, 23, 5], 1), ([5, 23, 6], 1)],
        &[([5, 20, 5], LAMP_OFF)],
        &[([5, 23, 5], 0), ([6, 20, 5], 0)],
    ];
    for (k, s) in steps.iter().enumerate() {
        for &(p, st) in *s {
            set(&mut a, p, st);
            set(&mut b, p, st);
        }
        let ps: Vec<[i32; 3]> = s.iter().map(|&(p, _)| p).collect();
        let ta = crate::relight_zones(&mut a, height, &t, true, &ps, true);
        let mut out = [Touched::new()];
        relight_zones(&mut b, height, &t, true, &ps, None, o, &mut out);
        if a != b || ta != out[0] {
            return Err(format!("emitters scene, step {k}"));
        }
    }
    Ok(())
}

#[test]
fn emitters_next_to_a_removed_source() {
    let before = checks();
    emitters_scene(unchecked(shipped())).unwrap();
    emitters_scene(shipped()).unwrap();
    assert_eq!(checks_since(before).1, 0, "the emitters scene was handed back to the zone");
}

/// Shipped configuration (`LIGHTINC` checked, no size limit) on a consistent world, and the
/// unchecked incremental with the same switches.
#[test]
fn shipped_incremental_matches_old() {
    let t = tables();
    let o = Opts { inc_max: usize::MAX, ..shipped() };
    let before = checks();
    for (seed, has_sky) in [(111u64, true), (112, false)] {
        let case = Case { t: &t, height: 64, has_sky, garbage: false, steps: 1500, seed, state: &state };
        compare(&case, |_| o).unwrap_or_else(|e| panic!("seed {seed}: {e}"));
        compare(&case, |_| unchecked(o)).unwrap_or_else(|e| panic!("seed {seed} (unchecked): {e}"));
    }
    let (kept, fallbacks) = checks_since(before);
    assert!(kept > 1000 && fallbacks == 0, "{kept} kept, {fallbacks} handed back to the zone");
}

/// Sky limited to the changes that touch it (`relight_masked`): incremental = old path.
#[test]
fn incremental_masked_matches_old() {
    let t = tables();
    let mut rng = Rng(121);
    let height = 64;
    let mut a = world(height, false, &mut rng);
    let mut worlds = [a.clone(), a.clone()];
    for step in 0..1500 {
        let ps = changes(&mut rng, height);
        let mut sky_ps = Vec::new();
        for &p in &ps {
            let s = match rng.below(3) {
                0 => [LAMP_OFF, LAMP_ON, TORCH, GLASS][rng.below(4) as usize],
                _ => state(&mut rng),
            };
            let old = set(&mut a, p, s).unwrap_or(0);
            for b in &mut worlds {
                set(b, p, s);
            }
            if t.matters_sky(old, s) {
                sky_ps.push(p);
            }
        }
        let mut oa = [Touched::new(), Touched::new()];
        crate::relight_zones_split(&mut a, height, &t, true, &ps, &mut oa);
        for (b, o) in worlds.iter_mut().zip([shipped(), unchecked(shipped())]) {
            let mut ob = [Touched::new(), Touched::new()];
            relight_zones(b, height, &t, true, &ps, Some(&sky_ps), o, &mut ob);
            assert!(a == *b, "sheets differ at step {step} (checked {})", o.inc_verify);
            assert_eq!(oa, ob, "sections at step {step} (checked {})", o.inc_verify);
        }
    }
}

/// Each sabotage of the unchecked incremental is seen: sky column not walked, emission of the
/// emptied cells forgotten, overlapping groups resolved incrementally, stack walked from the first
/// change of the list instead of the highest.
#[test]
fn unchecked_incremental_sabotages_are_caught() {
    let t = tables();
    let names = [
        (SABOTAGE_SKY_COLUMN_NOT_WALKED, "sky column not walked"),
        (SABOTAGE_EMISSION_NOT_RESTORED, "emission not restored"),
        (SABOTAGE_OVERLAPPING_GROUPS_TAKEN_AS_ISOLATED, "overlapping groups"),
        (SABOTAGE_STACK_NOT_FROM_ITS_HIGHEST_CHANGE, "stack walked from the first change"),
    ];
    for (k, name) in names {
        let caught = (0..6u64).any(|seed| {
            SABOTAGE.with(|s| s.set(k));
            let case = Case { t: &t, height: 64, has_sky: true, garbage: false, steps: 600, seed: 200 + seed, state: &state };
            let o = unchecked(Opts::from_bits(!DEDUP & !PAR_BITS));
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                emitters_scene(o)?;
                compare(&case, |_| o)
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

/// First cell that does not hold the fixed point (diagnostic): stored value against the value
/// drawn from its source and its neighbours.
fn incoherence(w: &World, t: &LightTables, h: i32, sky: bool) -> Option<String> {
    let st = |x: i32, y: i32, z: i32| -> Option<u32> {
        let (s, _, _) = w.0.get(&(x >> 4, z >> 4))?;
        if !(0..h).contains(&y) {
            return Some(0);
        }
        Some(s[(y >> 4) as usize].blocks.get(rc_world::block_index((x & 15) as usize, (y & 15) as usize, (z & 15) as usize)))
    };
    let val = |x: i32, y: i32, z: i32| -> Option<u8> {
        let (_, k, b) = w.0.get(&(x >> 4, z >> 4))?;
        if y >= h {
            return sky.then_some(15);
        }
        if y < 0 {
            return None;
        }
        let l = if sky { k } else { b }[((y >> 4) + 1) as usize].as_ref()?;
        Some(nibble(l, (x & 15) as usize, (y & 15) as usize, (z & 15) as usize))
    };
    for &(cx, cz) in w.0.keys() {
        for lz in 0..16 {
            for lx in 0..16 {
                let (x, z) = (cx * 16 + lx, cz * 16 + lz);
                let mut clear = true;
                for y in (0..h).rev() {
                    let s = st(x, y, z).expect("column");
                    let above = st(x, y + 1, z).expect("column");
                    clear = clear && !t.damps(s) && !t.occluded(above, s, 0);
                    let Some(v) = val(x, y, z) else { continue };
                    let mut best = if sky { if clear { 15 } else { 0 } } else { t.emission.get(s as usize).copied().unwrap_or(0) } as i32;
                    for (d, step) in STEP.iter().enumerate() {
                        let (qx, qy, qz) = (x - step[0], y - step[1], z - step[2]);
                        let Some(vq) = val(qx, qy, qz) else { continue };
                        let sq = st(qx, qy, qz).unwrap_or(0);
                        if !t.occluded(sq, s, d) {
                            best = best.max(i32::from(vq) - i32::from(t.opacity(s)));
                        }
                    }
                    if i32::from(v) != best.max(0) {
                        return Some(format!("({x}, {y}, {z}): {v} stored, {} expected", best.max(0)));
                    }
                }
            }
        }
    }
    None
}

/// Diagnostic (ignored): does the old path keep the world consistent step by step?
#[test]
#[ignore]
fn old_path_coherence_trace() {
    let t = tables();
    let height = 64;
    let mut rng = Rng(111);
    let mut a = world(height, false, &mut rng);
    for step in 0..1400 {
        let ps: Vec<[i32; 3]> = if step == 0 {
            let s = script(height);
            for &(p, st) in &s {
                set(&mut a, p, st);
            }
            s.iter().map(|&(p, _)| p).collect()
        } else {
            let ps = changes(&mut rng, height);
            for &p in &ps {
                let s = state(&mut rng);
                set(&mut a, p, s);
            }
            ps
        };
        if ps.is_empty() {
            continue;
        }
        crate::relight_zones(&mut a, height, &t, true, &ps, true);
        for sky in [false, true] {
            if let Some(e) = incoherence(&a, &t, height, sky) {
                panic!("step {step}, {} groups, sky {sky}: {e}; changes {ps:?}", groups(&ps).len());
            }
        }
    }
}

/// Diagnostic (ignored): first cells that differ between the old path and the incremental.
#[test]
#[ignore]
fn incremental_first_gap() {
    let t = tables();
    let height = 64;
    let mut rng = Rng(111);
    let mut a = world(height, false, &mut rng);
    let mut b = a.clone();
    let o = unchecked(shipped());
    for step in 0..1400 {
        let ps: Vec<[i32; 3]> = if step == 0 {
            let s = script(height);
            for &(p, st) in &s {
                set(&mut a, p, st);
                set(&mut b, p, st);
            }
            s.iter().map(|&(p, _)| p).collect()
        } else {
            let ps = changes(&mut rng, height);
            for &p in &ps {
                let s = state(&mut rng);
                set(&mut a, p, s);
                set(&mut b, p, s);
            }
            ps
        };
        if ps.is_empty() {
            continue;
        }
        let before = b.clone();
        crate::relight_zones(&mut a, height, &t, true, &ps, true);
        let mut out = [Touched::new()];
        relight_zones(&mut b, height, &t, true, &ps, None, o, &mut out);
        if a != b {
            let mut gaps = Vec::new();
            for (k, (_, sa, ba)) in &a.0 {
                let (_, sb, bb) = &b.0[k];
                let (_, s0, b0) = &before.0[k];
                for (sky, la, lb, l0) in [(true, sa, sb, s0), (false, ba, bb, b0)] {
                    for li in 0..la.len() {
                        let (Some(x), Some(y), Some(z0)) = (&la[li], &lb[li], &l0[li]) else { continue };
                        for i in 0..4096 {
                            let (lx, ly, lz) = (i & 15, i >> 8, (i >> 4) & 15);
                            let (va, vb) = (nibble(x, lx, ly, lz), nibble(y, lx, ly, lz));
                            if va != vb {
                                let p = [k.0 * 16 + lx as i32, (li as i32 - 1) * 16 + ly as i32, k.1 * 16 + lz as i32];
                                gaps.push(format!("{p:?} sky {sky}: zone {va}, inc {vb}, before {}", nibble(z0, lx, ly, lz)));
                            }
                        }
                    }
                }
            }
            let states: Vec<_> = ps.iter().map(|p| state_in(b.column(p[0] >> 4, p[2] >> 4).map(|c| c.0), (p[0] & 15) as usize, (p[2] & 15) as usize, p[1], height)).collect();
            panic!("step {step}: changes {ps:?} states {states:?}, groups {:?}\n{}", groups(&ps), gaps[..gaps.len().min(30)].join("\n"));
        }
    }
}

/// Inconsistent sheets (random values, sections without sheet): the zone repairs its whole box,
/// the incremental (like the game's engine) only touches what depends on the changes. Expected and
/// reported difference (protocol A3): this test measures it and checks that it does exist, so
/// that the proof on consistent worlds is not read as valid everywhere.
#[test]
fn incoherent_layers_are_kept_like_the_game() {
    let t = tables();
    let case = Case { t: &t, height: 64, has_sky: true, garbage: true, steps: 300, seed: 131, state: &state };
    let r = compare(&case, |_| unchecked(shipped()));
    eprintln!("inconsistent sheets: {r:?}");
    assert!(r.is_err(), "no difference on inconsistent sheets: the test no longer measures anything");
}

/// Its inverse twin: checked, the same inconsistent sheets give the zone's result, bit for bit, at
/// every step (the shipped switches, then random ones, sky and block only), and the check really
/// hands resolutions back to the zone (otherwise the proof would only cover consistent zones).
#[test]
fn incoherent_layers_are_repaired_like_the_zone_when_checked() {
    let t = tables();
    let before = checks();
    let case = Case { t: &t, height: 64, has_sky: true, garbage: true, steps: 300, seed: 131, state: &state };
    compare(&case, |_| shipped()).unwrap_or_else(|e| panic!("shipped: {e}"));
    for (seed, has_sky) in [(132u64, true), (133, false), (134, true)] {
        let case = Case { t: &t, height: 64, has_sky, garbage: true, steps: 600, seed, state: &state };
        compare(&case, checked_opts).unwrap_or_else(|e| panic!("seed {seed} (sky {has_sky}): {e}"));
    }
    let (kept, fallbacks) = checks_since(before);
    eprintln!("inconsistent sheets, checked: {kept} kept, {fallbacks} handed back to the zone");
    assert!(fallbacks > 50, "only {fallbacks} resolutions handed back: the check sees no inconsistency");
    assert!(kept > 300, "only {kept} resolutions kept");
}

/// Incremental and parallel groups together (thresholds at 0), pool of 1 then of 32 threads, consistent
/// worlds: sheets and touched sections identical to the old path, unchecked and checked (the check
/// and the source heights then run by planes and rows on the pool, any zone being large enough).
#[test]
fn incremental_with_parallel_groups_matches_old() {
    let t = tables();
    for threads in [1, 32] {
        pool(threads).install(|| {
            for (seed, has_sky) in [(141u64, true), (142, false)] {
                let case = Case { t: &t, height: 64, has_sky, garbage: false, steps: 800, seed, state: &state };
                compare(&case, |rng| unchecked(par_opts(rng)))
                    .unwrap_or_else(|e| panic!("{threads} threads, seed {seed}: {e}"));
                compare(&case, |rng| Opts { inc: true, ..par_opts(rng) })
                    .unwrap_or_else(|e| panic!("{threads} threads, seed {seed} (checked): {e}"));
            }
            // Inconsistent sheets, checked: the zone's result too.
            let case = Case { t: &t, height: 64, has_sky: true, garbage: true, steps: 400, seed: 143, state: &state };
            compare(&case, |rng| Opts { inc: true, ..par_opts(rng) })
                .unwrap_or_else(|e| panic!("{threads} threads, inconsistent sheets (checked): {e}"));
        });
    }
}

/// One call, a new consistent world: sheets compared with the old path; returns the passes through
/// the parallel groups, the incremental resolutions and the sky zones computed.
fn one_call(sab: u8, has_sky: bool, ps: &[[i32; 3]], o: Opts) -> (usize, usize, usize) {
    let t = tables();
    let height = 64;
    let mut a = world(height, false, &mut Rng(1));
    let mut b = a.clone();
    for &p in ps {
        set(&mut a, p, LAMP_ON);
        set(&mut b, p, LAMP_ON);
    }
    crate::relight_zones(&mut a, height, &t, has_sky, ps, true);
    let incremental = || solves() + checks().0 + checks().1;
    let count = || (RUN_GROUPS.with(|n| n.get()), incremental(), SKY_ZONES.with(|n| n.get()));
    let n0 = count();
    SABOTAGE.with(|s| s.set(sab));
    let mut out = [Touched::new()];
    relight_zones(&mut b, height, &t, has_sky, ps, None, o, &mut out);
    SABOTAGE.with(|s| s.set(0));
    assert!(a == b, "sheets differ (sabotage {sab})");
    let n = count();
    (n.0 - n0.0, n.1 - n0.1, n.2 - n0.2)
}

/// Unchecked, an isolated group does not take the others away from the parallel groups: it is resolved
/// incrementally, the two groups whose zones cross go through `LIGHTGROUPS`. The old scheduling
/// (everything in series as soon as a group is isolated) must be seen. Checked, the isolated group goes
/// to the parallel groups with the others (it would otherwise hold them back).
#[test]
fn isolated_group_leaves_the_rest_to_parallel_groups() {
    let ps = [[-10, 5, -10], [30, 50, 30], [40, 50, 30]];
    assert_eq!(groups(&ps).len(), 3, "three groups expected");
    let checked = Opts { join_min: 0, ..shipped() };
    assert!(checked.groups, "LIGHTGROUPS on as shipped");
    let o = unchecked(checked);
    let (par, inc, _) = one_call(0, false, &ps, o);
    assert_eq!((par, inc), (1, 1), "parallel groups for the rest, one incremental resolution");
    let (par, _, _) = one_call(SABOTAGE_REST_RESOLVED_IN_SERIES, false, &ps, o);
    assert_eq!(par, 0, "sabotage: rest resolved in series, not seen");
    assert_eq!(one_call(0, false, &ps, checked), (1, 0, 0), "checked: every group to the parallel groups");
    let (par, inc, _) = one_call(crate::fast::inc::SABOTAGE_CHECKED_GROUPS_IN_SERIES, false, &ps, checked);
    assert_eq!((par, inc), (1, 1), "sabotage: checked group resolved before the parallel groups, not seen");
}

/// Checked, several isolated groups are left to the parallel groups (resolved at the same time there,
/// one after the other here); unchecked, they stay incremental. Sabotage 67 (checked groups in series)
/// must be seen.
#[test]
fn several_isolated_groups_go_to_the_parallel_groups_when_checked() {
    let ps = [[-10, 5, -10], [30, 50, 30], [-10, 50, 30]];
    assert_eq!(groups(&ps).len(), 3, "three groups expected");
    let checked = Opts { join_min: 0, ..shipped() };
    assert_eq!(one_call(0, false, &ps, checked), (1, 0, 0), "checked: every group to the parallel groups");
    let (par, inc, _) = one_call(0, false, &ps, unchecked(checked));
    assert_eq!((par, inc), (0, 3), "unchecked: every group incremental");
    let (par, inc, _) = one_call(crate::fast::inc::SABOTAGE_CHECKED_GROUPS_IN_SERIES, false, &ps, checked);
    assert_eq!((par, inc), (0, 3), "sabotage 67 not seen");
}

/// No isolated group (block zones that cross): the incremental computes no sky zone
/// more than the zone path. Without the sort by the block zone (sabotage), two more.
#[test]
fn overlapping_groups_cost_no_extra_sky_zone() {
    let ps = [[20, 30, 20], [30, 30, 20]];
    assert_eq!(groups(&ps).len(), 2, "two groups expected");
    let zone = Opts { groups: false, ..shipped_zone() };
    for inc in [Opts { inc: true, ..zone }, unchecked(zone)] {
        let (_, _, z0) = one_call(0, true, &ps, zone);
        let (_, n, z1) = one_call(0, true, &ps, inc);
        assert_eq!((n, z1), (0, z0), "extra sky zones or unexpected incremental resolution");
        let (_, _, z2) = one_call(SABOTAGE_SKY_CANDIDATES_NOT_SORTED_BY_BLOCK_ZONE, true, &ps, inc);
        assert_eq!(z2, z0 + 2, "sabotage: sky candidates not sorted by block zone, not seen");
    }
}

/// An isolated sky group, checked: its sky zone is computed once, like the zone path (the checked
/// resolution takes the one computed to find the isolated groups).
#[test]
fn isolated_sky_group_computes_its_sky_zone_once() {
    let ps = [[5, 30, 5]];
    let (_, _, by_zone) = one_call(0, true, &ps, shipped_zone());
    let (_, checked, by_checked) = one_call(0, true, &ps, shipped());
    assert!(checked > 0 && by_zone == 1, "{checked} checked resolutions, {by_zone} sky zones for the zone path");
    assert_eq!(by_checked, by_zone, "sky zone computed again by the checked resolution");
}

/// `LIGHTINC` on unless `=0`; without `LIGHTINCMAX`, checked groups of at most
/// [`CHECKED_GROUP_MAX`] changes and no limit unchecked; `LIGHTINCMAX` without the incremental:
/// ignored, everything at 0 stays the old code (sabotage: threshold kept).
#[test]
fn incmax_alone_keeps_old_options() {
    for checked in [true, false] {
        let default_max = if checked { CHECKED_GROUP_MAX } else { usize::MAX };
        assert_eq!(Opts::inc_env(None, Some("16"), checked), (true, 16));
        assert_eq!(Opts::inc_env(None, None, checked), (true, default_max));
        assert_eq!(Opts::inc_env(Some("0"), Some("16"), checked), (false, usize::MAX));
        assert_eq!(Opts::inc_env(Some(" 0 "), None, checked), (false, usize::MAX));
        assert_eq!(Opts::inc_env(Some("1"), Some("16"), checked), (true, 16));
        assert_eq!(Opts::inc_env(Some("1"), None, checked), (true, default_max));
        assert_eq!(Opts::inc_env(Some("1"), Some("many"), checked), (true, default_max));
        let (inc, inc_max) = Opts::inc_env(Some("0"), Some("16"), checked);
        assert!(Opts { inc, inc_max, ..Opts::OLD } == Opts::OLD);
    }
    const { assert!(CHECKED_GROUP_MAX >= 64, "the random suites' groups (up to 56 changes) stay checked") };
    SABOTAGE.with(|s| s.set(crate::fast::SABOTAGE_INCMAX_KEPT_WITHOUT_INCREMENTAL));
    let r = Opts::inc_env(Some("0"), Some("16"), true);
    SABOTAGE.with(|s| s.set(0));
    assert_ne!(r, (false, usize::MAX), "sabotage: threshold kept without the incremental, not seen");
}

/// Real 26.3 data (if present; required under `RUSTCRAFT_OPT_REQUIRE=1`): game tables,
/// states drawn from the whole registry, consistent worlds. The incremental must return the old path
/// bit for bit: a difference specific to the real shapes, opacities or emissions thus cannot
/// hide in the differences of the real traces (inconsistent sheets, protocol A3).
#[test]
fn incremental_real_tables_match_old() {
    let Some(d) = rc_testdata::version_data() else {
        return;
    };
    let t = LightTables::new(&d);
    let n = d.states.len() as u64;
    // One draw in four among the real emitters: drawn from the whole registry, two neighbouring
    // emitters are too rare to exercise the decrease that empties one of them.
    let lit: Vec<u32> = (0..n as u32).filter(|&s| t.emission.get(s as usize).is_some_and(|&e| e > 0)).collect();
    assert!(!lit.is_empty(), "no emitter in the real tables");
    let drawn = lit.clone();
    let pick = move |rng: &mut Rng| match rng.below(4) {
        0 => 0,
        1 => drawn[rng.below(drawn.len() as u64) as usize],
        _ => rng.below(n) as u32,
    };
    let (n0, checked_before) = (solves(), checks());
    for (seed, has_sky, steps) in [(151u64, true, 2500), (152, false, 1500), (153, true, 1500)] {
        let case = Case { t: &t, height: 64, has_sky, garbage: false, steps, seed, state: &pick };
        compare(&case, |_| unchecked(shipped())).unwrap_or_else(|e| panic!("seed {seed} (sky {has_sky}): {e}"));
        compare(&case, |_| shipped()).unwrap_or_else(|e| panic!("seed {seed} (sky {has_sky}, checked): {e}"));
    }
    let case = Case { t: &t, height: 64, has_sky: true, garbage: false, steps: 1000, seed: 154, state: &pick };
    compare(&case, inc_opts).unwrap_or_else(|e| panic!("seed 154 (random): {e}"));
    compare(&case, checked_opts).unwrap_or_else(|e| panic!("seed 154 (random, checked): {e}"));
    let s = solves() - n0;
    assert!(s > 4000, "only {s} incremental resolutions");
    // Real shapes, opacities and emissions: the check keeps every consistent resolution too.
    let (kept, fallbacks) = checks_since(checked_before);
    assert!(kept > 4000 && fallbacks == 0, "real tables: {kept} kept, {fallbacks} handed back to the zone");
    // Inconsistent sheets with the real tables: checked, the zone's result.
    let case = Case { t: &t, height: 64, has_sky: true, garbage: true, steps: 600, seed: 155, state: &pick };
    compare(&case, |_| shipped()).unwrap_or_else(|e| panic!("seed 155 (inconsistent sheets, checked): {e}"));
    // Scene of real emitters: a weak one next to a strong one that is placed then removed (the weak one,
    // emptied by the decrease, must get its emission back).
    let emit = |s: u32| t.emission.get(s as usize).copied().unwrap_or(0);
    let strong = *lit.iter().find(|&&s| emit(s) == 15).expect("emitter of 15");
    let weak = *lit.iter().find(|&&s| (1..14).contains(&emit(s))).expect("weak emitter");
    let scene = |o: Opts| -> Result<(), String> {
        let mut a = world(64, false, &mut Rng(9));
        let mut b = a.clone();
        let steps: [&[([i32; 3], u32)]; 3] = [&[([5, 20, 5], strong), ([6, 20, 5], weak)], &[([5, 20, 5], 0)], &[([5, 20, 5], strong)]];
        for (k, s) in steps.iter().enumerate() {
            for &(p, st) in *s {
                set(&mut a, p, st);
                set(&mut b, p, st);
            }
            let ps: Vec<[i32; 3]> = s.iter().map(|&(p, _)| p).collect();
            crate::relight_zones(&mut a, 64, &t, true, &ps, true);
            let mut out = [Touched::new()];
            relight_zones(&mut b, 64, &t, true, &ps, None, o, &mut out);
            if a != b {
                return Err(format!("scene of real emitters, step {k}"));
            }
        }
        Ok(())
    };
    let o = unchecked(Opts::from_bits(!DEDUP & !PAR_BITS));
    scene(o).unwrap();
    scene(Opts { inc_verify: true, ..o }).unwrap();
    // The proof on real tables sees the defects of the incremental (sky column, emission).
    let incremental_defects =
        [SABOTAGE_SKY_COLUMN_NOT_WALKED, SABOTAGE_EMISSION_NOT_RESTORED, SABOTAGE_STACK_NOT_FROM_ITS_HIGHEST_CHANGE];
    for k in incremental_defects {
        let caught = (0..4u64).any(|seed| {
            let case = Case { t: &t, height: 64, has_sky: true, garbage: false, steps: 400, seed: 160 + seed, state: &pick };
            SABOTAGE.with(|s| s.set(k));
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                scene(o)?;
                compare(&case, |_| o)
            }));
            SABOTAGE.with(|s| s.set(0));
            !matches!(r, Ok(Ok(())))
        });
        assert!(caught, "sabotage {k} not seen on the real tables");
    }
}

/// The emitters scene then `compare` under sabotage `k`: their result and the checked resolutions
/// they handed back to the zone.
fn under_sabotage(k: u8, case: &Case, o: Opts) -> (Result<(), String>, usize) {
    let before = checks();
    SABOTAGE.with(|s| s.set(k));
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        emitters_scene(o)?;
        compare(case, |_| o)
    }));
    SABOTAGE.with(|s| s.set(0));
    let r = r.unwrap_or_else(|_| Err("panic".into()));
    (r, checks_since(before).1)
}

/// Each sabotage of the checked path is seen. A defect that makes the incremental or the check
/// stricter only costs fallbacks, the result staying the zone's: on consistent worlds (no fallback
/// otherwise) it must hand resolutions back, sheets still identical — sky column not walked,
/// emission not restored, stack walked from its lowest change, check without the sky sources,
/// margin seeds ignored by the check; the same for the forced give-up at the first emptied cell (the
/// zone taking over from a half-done decrease). Faces looked up the wrong way, or only where the cell
/// itself has a shape: seen either way. A defect that makes it looser changes the result on
/// inconsistent sheets: check skipped, fallback on the incremental's values.
#[test]
fn checked_sabotages_are_caught() {
    let t = tables();
    let o = Opts { inc: true, ..Opts::from_bits(!DEDUP & !PAR_BITS) };
    assert!(o.inc_verify, "checked by default");
    let coherent = |seed: u64| Case { t: &t, height: 64, has_sky: true, garbage: false, steps: 400, seed, state: &state };
    for (k, name) in [
        (SABOTAGE_SKY_COLUMN_NOT_WALKED, "sky column not walked"),
        (SABOTAGE_EMISSION_NOT_RESTORED, "emission not restored"),
        (SABOTAGE_STACK_NOT_FROM_ITS_HIGHEST_CHANGE, "stack walked from its lowest change"),
        (checked_inc::SABOTAGE_CHECK_WITHOUT_SKY_SOURCE, "check without the sky sources"),
        (checked_inc::SABOTAGE_CHECK_IGNORES_MARGIN_SEEDS, "margin seeds ignored"),
        (checked_inc::SABOTAGE_GIVE_UP_AT_ONCE, "give-up forced at the first emptied cell"),
    ] {
        let (r, fallbacks) = under_sabotage(k, &coherent(300 + u64::from(k)), o);
        r.unwrap_or_else(|e| panic!("sabotage {k} ({name}) changed the result: {e}"));
        assert!(fallbacks > 0, "sabotage {k} ({name}) not seen");
    }
    for (k, name) in [
        (checked_inc::SABOTAGE_CHECK_FACES_THE_WRONG_WAY, "faces the wrong way"),
        (checked_inc::SABOTAGE_SHAPES_LOOKED_UP_ON_THE_CELL_ONLY, "shapes looked up on the cell only"),
    ] {
        let (r, fallbacks) = under_sabotage(k, &coherent(362), o);
        assert!(r.is_err() || fallbacks > 0, "sabotage {k} ({name}) not seen");
    }
    // The forced give-up on inconsistent sheets too: the zone's result from the half-done decrease.
    let k = checked_inc::SABOTAGE_GIVE_UP_AT_ONCE;
    let (r, fallbacks) = under_sabotage(k, &Case { garbage: true, ..coherent(131) }, o);
    r.unwrap_or_else(|e| panic!("forced give-up changed the result on inconsistent sheets: {e}"));
    assert!(fallbacks > 0, "forced give-up never reached on inconsistent sheets");
    for (k, name) in [(checked_inc::SABOTAGE_CHECK_SKIPPED, "check skipped"), (checked_inc::SABOTAGE_UNDO_FORGOTTEN, "undo forgotten")] {
        let case = Case { t: &t, height: 64, has_sky: true, garbage: true, steps: 300, seed: 131, state: &state };
        let (r, _) = under_sabotage(k, &case, o);
        assert!(r.is_err(), "sabotage {k} ({name}) not seen");
    }
}

/// Stores block light `v` at `p` (a shared sheet is copied first).
fn set_block_light(w: &mut World, p: [i32; 3], v: u8) {
    let (_, _, block) = w.0.get_mut(&(p[0] >> 4, p[2] >> 4)).expect("column");
    let sheet = block[((p[1] >> 4) + 1) as usize].as_mut().expect("sheet");
    let (x, y, z) = ((p[0] & 15) as usize, (p[1] & 15) as usize, (p[2] & 15) as usize);
    crate::set_nibble(std::sync::Arc::make_mut(sheet), x, y, z, v);
}

/// A dark interior next to a lit margin cell (an inconsistent sheet: 12 stored with nothing lighting
/// it). The zone lights the interior from that margin seed, so the zone may not be taken as dark. The
/// random sheets cannot draw this case (a zone of theirs is dark only if its 20 to 30 interior
/// sections all came out absent or zero, about 0.4^27), hence the written scene. Sabotage: the dark
/// test leaves the margin out, the interior stays dark.
#[test]
fn lit_margin_keeps_a_zone_from_being_dark() {
    let t = tables();
    let height = 64;
    let change = [5, 20, 5];
    let scene = |sab: u8| -> bool {
        let mut a = world(height, false, &mut Rng(1));
        // The block zone of the change spans x = 5 - 16 ..= 5 + 16: x = -11 is its margin.
        set_block_light(&mut a, [change[0] - 16, change[1], change[2]], 12);
        set(&mut a, change, 1);
        let mut b = a.clone();
        crate::relight_zones(&mut a, height, &t, false, &[change], true);
        let before = checks();
        SABOTAGE.with(|s| s.set(sab));
        let mut out = [Touched::new()];
        relight_zones(&mut b, height, &t, false, &[change], None, shipped(), &mut out);
        SABOTAGE.with(|s| s.set(0));
        assert_eq!(checks_since(before).0 + checks_since(before).1, 1, "one checked resolution expected");
        a == b
    };
    assert!(scene(0), "lit margin: the zone's result expected");
    assert!(!scene(checked_inc::SABOTAGE_DARK_ZONE_IGNORES_MARGIN), "sabotage: dark test without the margin, not seen");
}

/// `LIGHTINCOVERLAP`: every group checked in order, the overlapping ones included; consistent and
/// inconsistent sheets, random switches: the old path's sheets and touched sections.
#[test]
fn overlapping_groups_in_order_match_old() {
    let t = tables();
    let before = checks();
    let in_order = |rng: &mut Rng| Opts { inc_overlap: true, ..checked_opts(rng) };
    for (seed, has_sky, garbage) in [(171u64, true, false), (172, false, false), (173, true, true), (174, false, true)] {
        let case = Case { t: &t, height: 64, has_sky, garbage, steps: 1200, seed, state: &state };
        compare(&case, in_order).unwrap_or_else(|e| panic!("seed {seed} (sky {has_sky}, random {garbage}): {e}"));
    }
    let (kept, fallbacks) = checks_since(before);
    eprintln!("groups in order: {kept} kept, {fallbacks} handed back to the zone");
    assert!(kept > 2000, "only {kept} kept");
}

/// Two groups whose boxes cross: a torch removed next to a pane of glass placed 10 cells away. The
/// first group's zone holds the torch, the second's the light the torch cast beyond the first box.
/// In order, the first group puts the torch out too (a change of the call inside its zone) and the
/// second puts out again the cells the first rewrote: both kept, the old path's sheets. Sabotages:
/// own changes only (66), the first group handed back; rewritten cells kept (65), the second.
#[test]
fn overlapping_groups_put_out_each_others_light() {
    let t = tables();
    let (height, torch, pane) = (64, [30, 30, 20], [20, 30, 20]);
    let mut start = world(height, false, &mut Rng(1));
    set(&mut start, torch, TORCH);
    crate::relight_zones(&mut start, height, &t, false, &[torch], true);
    let ps = [pane, torch];
    assert_eq!(groups(&ps).len(), 2, "two groups expected");
    let o = Opts { inc_overlap: true, ..shipped() };
    let run = |sab: u8| {
        let (mut a, mut b) = (start.clone(), start.clone());
        for w in [&mut a, &mut b] {
            set(w, pane, GLASS);
            set(w, torch, 0);
        }
        let ta = crate::relight_zones(&mut a, height, &t, false, &ps, true);
        let before = checks();
        SABOTAGE.with(|s| s.set(sab));
        let mut out = [Touched::new()];
        relight_zones(&mut b, height, &t, false, &ps, None, o, &mut out);
        SABOTAGE.with(|s| s.set(0));
        assert!(a == b && ta == out[0], "sheets or sections differ (sabotage {sab})");
        checks_since(before)
    };
    assert_eq!(run(0), (2, 0), "both groups kept");
    assert_eq!(run(checked_inc::SABOTAGE_OWN_CHANGES_ONLY).1, 1, "sabotage 66 not seen");
    assert_eq!(run(checked_inc::SABOTAGE_REWRITTEN_CELLS_KEPT).1, 1, "sabotage 65 not seen");
}
