//! Proofs of the incremental relight (`LIGHTINC`, `fast/inc.rs`): sheets and touched sections
//! identical, bit for bit, to the old path on consistent worlds (the fixed point held at
//! every step), sabotages seen, and difference reported on inconsistent sheets.

use super::*;
use crate::fast::inc::SOLVES;

/// Switches of the zone at random (without `LIGHTDEDUP`), incremental on, group size
/// threshold sometimes low (the large groups then go through the zone among the small ones).
fn inc_opts(rng: &mut Rng) -> Opts {
    let zone = random_opts(rng);
    let inc_max = if rng.below(4) == 0 { 3 } else { usize::MAX };
    Opts { inc: true, inc_max, ..zone }
}

fn solves() -> usize {
    SOLVES.with(|s| s.get())
}

/// 10,000 random steps (placing and removing sources, opaques, slabs, half-faces, roofs,
/// sky wells, flowing water in groups of 20 to 50) on consistent worlds.
#[test]
fn incremental_matches_old_on_coherent_worlds() {
    let t = tables();
    let n0 = solves();
    for (seed, has_sky, steps, height) in
        [(101u64, true, 3000, 64), (102, true, 3000, 64), (103, false, 2000, 64), (104, true, 2000, 48)]
    {
        let case = Case { t: &t, height, has_sky, garbage: false, steps, seed, state: &state };
        compare(&case, inc_opts).unwrap_or_else(|e| panic!("seed {seed} (sky {has_sky}): {e}"));
    }
    let n = solves() - n0;
    // The incremental must really be used (otherwise the proof proves nothing).
    assert!(n > 5000, "only {n} incremental resolutions");
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
    emitters_scene(Opts { inc: true, ..shipped() }).unwrap();
}

/// Shipped configuration (`LIGHTINC` on, no size limit) on a consistent world.
#[test]
fn shipped_incremental_matches_old() {
    let t = tables();
    let o = Opts { inc: true, inc_max: usize::MAX, ..shipped() };
    for (seed, has_sky) in [(111u64, true), (112, false)] {
        let case = Case { t: &t, height: 64, has_sky, garbage: false, steps: 1500, seed, state: &state };
        compare(&case, |_| o).unwrap_or_else(|e| panic!("seed {seed}: {e}"));
    }
}

/// Sky limited to the changes that touch it (`relight_masked`): incremental = old path.
#[test]
fn incremental_masked_matches_old() {
    let t = tables();
    let mut rng = Rng(121);
    let height = 64;
    let mut a = world(height, false, &mut rng);
    let mut b = a.clone();
    for step in 0..1500 {
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
        let mut oa = [Touched::new(), Touched::new()];
        crate::relight_zones_split(&mut a, height, &t, true, &ps, &mut oa);
        let o = Opts { inc: true, ..shipped() };
        let mut ob = [Touched::new(), Touched::new()];
        relight_zones(&mut b, height, &t, true, &ps, Some(&sky_ps), o, &mut ob);
        assert!(a == b, "sheets differ at step {step}");
        assert_eq!(oa, ob, "sections at step {step}");
    }
}

/// Each sabotage of the incremental is seen: sky column not walked (20), emission of the
/// emptied cells forgotten (21), overlapping groups resolved incrementally (22), stack walked
/// from the first change of the list instead of the highest (23).
#[test]
fn incremental_sabotages_are_caught() {
    let t = tables();
    let names = [
        (20u8, "sky column not walked"),
        (21, "emission not restored"),
        (22, "overlapping groups"),
        (23, "stack walked from the first change"),
    ];
    for (k, name) in names {
        let caught = (0..6u64).any(|seed| {
            SABOTAGE.with(|s| s.set(k));
            let case = Case { t: &t, height: 64, has_sky: true, garbage: false, steps: 600, seed: 200 + seed, state: &state };
            let o = Opts { inc: true, ..Opts::from_bits(!DEDUP & !PAR_BITS) };
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
    let o = Opts { inc: true, ..shipped() };
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
    let o = Opts { inc: true, ..shipped() };
    let r = compare(&case, |_| o);
    eprintln!("inconsistent sheets: {r:?}");
    assert!(r.is_err(), "no difference on inconsistent sheets: the test no longer measures anything");
}

/// Incremental and parallel groups together (thresholds at 0), pool of 1 then of 32 threads, consistent
/// worlds: sheets and touched sections identical to the old path.
#[test]
fn incremental_with_parallel_groups_matches_old() {
    let t = tables();
    for threads in [1, 32] {
        pool(threads).install(|| {
            for (seed, has_sky) in [(141u64, true), (142, false)] {
                let case = Case { t: &t, height: 64, has_sky, garbage: false, steps: 800, seed, state: &state };
                compare(&case, |rng| Opts { inc: true, ..par_opts(rng) })
                    .unwrap_or_else(|e| panic!("{threads} threads, seed {seed}: {e}"));
            }
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
    let count = || (RUN_GROUPS.with(|n| n.get()), solves(), SKY_ZONES.with(|n| n.get()));
    let n0 = count();
    SABOTAGE.with(|s| s.set(sab));
    let mut out = [Touched::new()];
    relight_zones(&mut b, height, &t, has_sky, ps, None, o, &mut out);
    SABOTAGE.with(|s| s.set(0));
    assert!(a == b, "sheets differ (sabotage {sab})");
    let n = count();
    (n.0 - n0.0, n.1 - n0.1, n.2 - n0.2)
}

/// An isolated group does not take the others away from the parallel groups: it is resolved incrementally,
/// the two groups whose zones cross go through `LIGHTGROUPS`. Sabotage 24 (old
/// scheduling: everything in series as soon as a group is isolated) must be seen.
#[test]
fn isolated_group_leaves_the_rest_to_parallel_groups() {
    let ps = [[-10, 5, -10], [30, 50, 30], [40, 50, 30]];
    assert_eq!(groups(&ps).len(), 3, "three groups expected");
    let o = Opts { inc: true, join_min: 0, ..shipped() };
    assert!(o.groups, "LIGHTGROUPS on as shipped");
    let (par, inc, _) = one_call(0, false, &ps, o);
    assert_eq!((par, inc), (1, 1), "parallel groups for the rest, one incremental resolution");
    let (par, _, _) = one_call(24, false, &ps, o);
    assert_eq!(par, 0, "sabotage 24 not seen");
}

/// No isolated group (block zones that cross): the incremental computes no sky zone
/// more than the zone path. Sabotage 26 (sort by the block zone removed) must
/// cost two more.
#[test]
fn overlapping_groups_cost_no_extra_sky_zone() {
    let ps = [[20, 30, 20], [30, 30, 20]];
    assert_eq!(groups(&ps).len(), 2, "two groups expected");
    let zone = Opts { groups: false, ..shipped() };
    let inc = Opts { inc: true, ..zone };
    let (_, _, z0) = one_call(0, true, &ps, zone);
    let (_, n, z1) = one_call(0, true, &ps, inc);
    assert_eq!((n, z1), (0, z0), "extra sky zones or unexpected incremental resolution");
    let (_, _, z2) = one_call(26, true, &ps, inc);
    assert_eq!(z2, z0 + 2, "sabotage 26 not seen");
}

/// `LIGHTINCMAX` without `LIGHTINC`: ignored, everything at 0 stays the old code (sabotage 25: threshold kept).
#[test]
fn incmax_alone_keeps_old_options() {
    assert_eq!(Opts::inc_env(None, Some("16")), (false, usize::MAX));
    assert_eq!(Opts::inc_env(Some("0"), Some("16")), (false, usize::MAX));
    assert_eq!(Opts::inc_env(Some("1"), Some("16")), (true, 16));
    assert_eq!(Opts::inc_env(Some("1"), None), (true, usize::MAX));
    let (inc, inc_max) = Opts::inc_env(None, Some("16"));
    assert!(Opts { inc, inc_max, ..Opts::OLD } == Opts::OLD);
    SABOTAGE.with(|s| s.set(25));
    let r = Opts::inc_env(None, Some("16"));
    SABOTAGE.with(|s| s.set(0));
    assert_ne!(r, (false, usize::MAX), "sabotage 25 not seen");
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
    let n0 = solves();
    for (seed, has_sky, steps) in [(151u64, true, 2500), (152, false, 1500), (153, true, 1500)] {
        let case = Case { t: &t, height: 64, has_sky, garbage: false, steps, seed, state: &pick };
        compare(&case, |_| Opts { inc: true, ..shipped() }).unwrap_or_else(|e| panic!("seed {seed} (sky {has_sky}): {e}"));
    }
    let case = Case { t: &t, height: 64, has_sky: true, garbage: false, steps: 1000, seed: 154, state: &pick };
    compare(&case, inc_opts).unwrap_or_else(|e| panic!("seed 154 (random): {e}"));
    let s = solves() - n0;
    assert!(s > 4000, "only {s} incremental resolutions");
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
    let o = Opts { inc: true, ..Opts::from_bits(!DEDUP & !PAR_BITS) };
    scene(o).unwrap();
    // The proof on real tables sees the defects of the incremental (sky column, emission).
    for k in [20u8, 21, 23] {
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
