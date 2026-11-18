//! Equivalence proofs of the registry's search paths on random registries (seeds from
//! `rng::test_seed`): every new path gives the old path's answer, and a deliberately broken
//! variant is caught by the same comparison (the proof would see a real mistake).

use super::*;
use crate::rng::Rng;

/// Block area of the random registries: 24 × 24 chunk columns, sections −4 to 11.
const AREA_COLUMNS: i32 = 24;
const MIN_SECTION: i32 = -4;
const SECTION_LEVELS: i32 = 16;

fn random_block(rng: &mut Rng) -> [i32; 3] {
    let span = AREA_COLUMNS * 16;
    [
        rng.next_int_bound(span) - span / 2,
        rng.next_int_bound(SECTION_LEVELS * 16) + MIN_SECTION * 16,
        rng.next_int_bound(span) - span / 2,
    ]
}

/// A block near one of a few cluster centres: villages of several sections, with gaps.
fn clustered_block(rng: &mut Rng, centres: &[[i32; 3]]) -> [i32; 3] {
    let c = centres[rng.next_int_bound(centres.len() as i32) as usize];
    let spread = 8 + rng.next_int_bound(40);
    let mut d = || rng.next_int_bound(2 * spread + 1) - spread;
    [c[0] + d(), (c[1] + d() / 2).clamp(MIN_SECTION * 16, (MIN_SECTION + SECTION_LEVELS) * 16 - 1), c[2] + d()]
}

/// A registry built by the operations the server uses: additions, type changes on a known
/// position, removals, tickets taken and given back.
fn random_registry(rng: &mut Rng) -> Pois {
    let mut p = Pois::default();
    let centres: Vec<[i32; 3]> = (0..1 + rng.next_int_bound(4)).map(|_| random_block(rng)).collect();
    let mut known: Vec<[i32; 3]> = Vec::new();
    let ops = 40 + rng.next_int_bound(400);
    for _ in 0..ops {
        let pick = |rng: &mut Rng, known: &[[i32; 3]]| known[rng.next_int_bound(known.len() as i32) as usize];
        match rng.next_int_bound(10) {
            0..=4 => {
                let pos = if rng.next_int_bound(4) == 0 { random_block(rng) } else { clustered_block(rng, &centres) };
                let ty = ALL[rng.next_int_bound(ALL.len() as i32) as usize];
                p.add_record(pos, ty, ty.max_tickets());
                known.push(pos);
            }
            5 | 6 if !known.is_empty() => {
                let pos = pick(rng, &known);
                p.acquire(pos);
            }
            7 if !known.is_empty() => {
                let pos = pick(rng, &known);
                p.release(pos);
            }
            8 if !known.is_empty() => {
                let pos = pick(rng, &known);
                p.remove_record(pos);
            }
            9 if !known.is_empty() => {
                let pos = pick(rng, &known);
                let ty = ALL[rng.next_int_bound(ALL.len() as i32) as usize];
                p.add_record(pos, ty, ty.max_tickets());
            }
            _ => {}
        }
    }
    p
}

fn random_section(rng: &mut Rng) -> (i32, i32, i32) {
    let b = random_block(rng);
    // A little beyond the area too: far from every village.
    (b[0] >> 4, (b[1] >> 4) + rng.next_int_bound(5) - 2, b[2] >> 4)
}

fn with_paths<R>(p: &mut Pois, paths: PoiPaths, f: impl FnOnce(&Pois) -> R) -> R {
    let kept = p.paths;
    p.paths = paths;
    let r = f(p);
    p.paths = kept;
    r
}

const SHELLS: PoiPaths = PoiPaths { village_shells: true, ..PoiPaths::OLD };
const COLUMNS: PoiPaths = PoiPaths { village_columns: true, ..PoiPaths::OLD };
const SORTED: PoiPaths = PoiPaths { sorted_records: true, ..PoiPaths::OLD };

/// Distance and bounded predicates (`== 0`, `<= 1`, `<= d`, `> 1`) on 10⁵ sections of 20
/// registries: shells and columns against the whole cube.
#[test]
fn village_distance_paths_agree_with_the_whole_cube() {
    let mut rng = Rng::new(crate::rng::test_seed());
    let mut seen = [0u32; 8];
    let mut off_by_one_caught = 0;
    for _ in 0..20 {
        let mut p = random_registry(&mut rng);
        for _ in 0..5000 {
            let s = random_section(&mut rng);
            let full = with_paths(&mut p, PoiPaths::OLD, |p| p.sections_to_village(s));
            seen[full as usize] += 1;
            for paths in [SHELLS, COLUMNS, PoiPaths { village_shells: true, village_columns: true, ..PoiPaths::OLD }] {
                with_paths(&mut p, paths, |p| {
                    assert_eq!(p.sections_to_village(s), full, "distance of {s:?} with {paths:?}");
                    for d in -1..=8 {
                        assert_eq!(p.within_sections_of_village(s, d), full <= d, "within {d} of {s:?} with {paths:?}");
                    }
                });
            }
            assert_eq!(p.section_has_occupied_village(s), full == 0);
            // Sabotage: each shell read one step too far out.
            let off_by_one = (0..=6).find(|&d| p.shell_has_village(s, d + 1)).unwrap_or(7);
            off_by_one_caught += i32::from(off_by_one != full);
        }
    }
    eprintln!("village distances seen (0..=7): {seen:?}");
    assert!(seen.iter().all(|&n| n > 0), "every distance 0..=7 must be covered: {seen:?}");
    assert!(off_by_one_caught > 0, "a shell shifted by one must be caught");
}

/// The rings of columns 0..=6 cover the 13 × 13 square once each.
#[test]
fn rings_of_columns_cover_the_square_once() {
    let mut seen: Vec<(i32, i32)> = (0..=6).flat_map(|r| ring_columns((10, -3), r)).collect();
    assert!(ring_columns((0, 0), 2).all(|(x, z)| x.abs().max(z.abs()) == 2));
    seen.sort_unstable();
    let square: Vec<(i32, i32)> = (4..=16).flat_map(|x| (-9..=3).map(move |z| (x, z))).collect();
    assert_eq!(seen, square);
}

/// Collections of village sections (`village_sections_in_cube`, cube read or columns) serve the
/// three batched callers: the golem's closest section (radius 2, collection ±8), the raid
/// centre move ("within one" over ±2, collection ±3), the golem's village sections (`== 0`).
#[test]
fn batched_village_tests_agree_with_one_search_per_section() {
    use crate::mobs::golems::iron::closest_section;
    let mut rng = Rng::new(crate::rng::test_seed());
    let mut moved = 0;
    for _ in 0..20 {
        let mut p = random_registry(&mut rng);
        for _ in 0..300 {
            let c = random_section(&mut rng);
            let center = [c.0, c.1, c.2];
            let old = with_paths(&mut p, PoiPaths::OLD, |p| {
                closest_section(center, 2, |s| p.sections_to_village((s[0], s[1], s[2])))
            });
            moved += i32::from(old != center);
            for paths in [PoiPaths::OLD, COLUMNS] {
                with_paths(&mut p, paths, |p| {
                    let villages = p.village_sections_in_cube(c, 8);
                    let new = closest_section(center, 2, |s| village_distance_among(&villages, (s[0], s[1], s[2])));
                    assert_eq!(new, old, "closest section around {c:?} with {paths:?}");
                    let near = p.village_sections_in_cube(c, 3);
                    for dx in -2..=2 {
                        for dy in -2..=2 {
                            for dz in -2..=2 {
                                let s = (c.0 + dx, c.1 + dy, c.2 + dz);
                                let full = p.village_distance_full_cube(s);
                                assert_eq!(village_distance_among(&near, s) <= 1, full <= 1, "within one of {s:?}");
                                assert_eq!(p.section_has_occupied_village(s), full == 0);
                            }
                        }
                    }
                });
            }
        }
    }
    assert!(moved > 0, "some centres must have a closer village section");
}

/// Radius searches, radii 8 to 64, every occupancy and several type filters: the kept search
/// order gives the exact sequence of records (insertion ranks included) of the sorting path.
#[test]
fn range_search_in_kept_order_gives_the_same_records() {
    let preds: [fn(PoiType) -> bool; 4] =
        [|_| true, PoiType::is_village, |t| t == PoiType::Home, |t| matches!(t, PoiType::Beehive | PoiType::BeeNest)];
    let mut rng = Rng::new(crate::rng::test_seed());
    let (mut found, mut sabotage_caught) = (0usize, 0);
    for _ in 0..20 {
        let mut p = random_registry(&mut rng);
        for _ in 0..500 {
            let q = RangeQuery {
                center: random_block(&mut rng),
                radius: 8 + rng.next_int_bound(57),
                occ: [Occupancy::Any, Occupancy::HasSpace, Occupancy::IsOccupied][rng.next_int_bound(3) as usize],
                heights: MIN_SECTION..=MIN_SECTION + SECTION_LEVELS - 1,
            };
            let pred = preds[rng.next_int_bound(4) as usize];
            let old = with_paths(&mut p, PoiPaths::OLD, |p| p.records_in_range(pred, &q));
            let new = with_paths(&mut p, SORTED, |p| p.records_in_range(pred, &q));
            assert_eq!(new, old, "radius {} around {:?}", q.radius, q.center);
            found += old.len();
            sabotage_caught += i32::from(reversed_section_changes(&mut p, &q, pred, &old));
        }
    }
    assert!(found > 1000, "the searches must find records: {found}");
    assert!(sabotage_caught > 0, "a section read in reverse order must be caught");
}

/// Sabotage: one section holding two records of a found type read in reverse order; true if the
/// comparison sees it. The section is put back afterwards.
fn reversed_section_changes(p: &mut Pois, q: &RangeQuery, pred: fn(PoiType) -> bool, old: &[Record]) -> bool {
    let Some(key) = old
        .iter()
        .map(|r| section_key(r.pos))
        .find(|k| old.iter().filter(|r| section_key(r.pos) == *k).count() >= 2)
    else {
        return false;
    };
    let sec = p.sections.get_mut(&key).expect("section");
    sec.search_order.reverse();
    let broken = with_paths(p, SORTED, |p| p.records_in_range(pred, q));
    p.sections.get_mut(&key).expect("section").search_order.reverse();
    broken != old
}

/// The column summaries and the kept search orders stay what a fresh computation gives, after
/// every kind of change.
#[test]
fn column_summaries_and_search_orders_stay_in_step() {
    let mut rng = Rng::new(crate::rng::test_seed());
    for _ in 0..30 {
        let p = random_registry(&mut rng);
        let mut fresh: HashMap<(i32, i32), ColumnSummary> = HashMap::new();
        for (k, s) in &p.sections {
            let sum = s.summary();
            if sum.filled {
                let c = fresh.entry((k.0, k.2)).or_default();
                c.filled_sections.push(k.1);
                c.occupied_village += sum.occupied_village;
            }
            let mut again = s.clone();
            again.rebuild_search_order();
            assert_eq!(again.search_order, s.search_order, "search order of {k:?}");
        }
        for c in fresh.values_mut() {
            c.filled_sections.sort_unstable();
        }
        assert_eq!(p.columns, fresh, "column summaries");
    }
}

/// Micro-bench (machine noted with the figures): radius searches r48 / r64 and village tests,
/// old against new paths, one core. `cargo test --release -p rc-sim poi_search_bench -- --ignored --nocapture`.
#[test]
#[ignore = "measurement bench"]
fn poi_search_bench() {
    use std::hint::black_box;
    use std::time::Instant;
    let mut rng = Rng::new(crate::rng::test_seed());
    // A village: 60 occupied homes and job sites around the origin; plus scattered hives.
    let mut p = Pois::default();
    for i in 0..60 {
        let pos = [rng.next_int_bound(64) - 32, 64 + rng.next_int_bound(8), rng.next_int_bound(64) - 32];
        let ty = if i % 2 == 0 { PoiType::Home } else { ALL[(i / 2 % 13) as usize] };
        p.add_record(pos, ty, ty.max_tickets());
        p.acquire(pos);
    }
    for _ in 0..40 {
        let pos = random_block(&mut rng);
        p.add_record(pos, PoiType::Beehive, 0);
    }
    let time = |p: &mut Pois, paths: PoiPaths, n: u32, f: &dyn Fn(&Pois, u32) -> usize| {
        p.paths = paths;
        let t = Instant::now();
        let mut acc = 0;
        for i in 0..n {
            acc += f(p, i);
        }
        black_box(acc);
        t.elapsed().as_secs_f64() * 1e9 / f64::from(n)
    };
    let heights = MIN_SECTION..=MIN_SECTION + SECTION_LEVELS - 1;
    for radius in [48, 64] {
        let q = RangeQuery { center: [5, 66, -3], radius, occ: Occupancy::Any, heights: heights.clone() };
        let search = |p: &Pois, _: u32| p.records_in_range(|t| t == PoiType::Home, &q).len();
        let old = time(&mut p, PoiPaths::OLD, 20_000, &search);
        let new = time(&mut p, SORTED, 20_000, &search);
        eprintln!("poi_in_range r{radius}: old {old:.0} ns, kept order {new:.0} ns");
    }
    let sites = [("in the village", (0, 4, 0)), ("at its edge", (5, 4, 3)), ("outside", (40, 4, 40))];
    for (name, s) in sites {
        let distance = |p: &Pois, i: u32| p.sections_to_village((s.0 + (i & 1) as i32, s.1, s.2)) as usize;
        let within = |p: &Pois, i: u32| usize::from(p.within_sections_of_village((s.0 + (i & 1) as i32, s.1, s.2), 1));
        let row = |p: &mut Pois, f: &dyn Fn(&Pois, u32) -> usize| {
            [PoiPaths::OLD, SHELLS, COLUMNS].map(|paths| time(p, paths, 100_000, f))
        };
        let [d_old, d_shells, d_columns] = row(&mut p, &distance);
        let [w_old, w_shells, w_columns] = row(&mut p, &within);
        eprintln!(
            "village {name}: distance old {d_old:.0} ns, shells {d_shells:.0} ns, columns {d_columns:.0} ns; \
             within one: old {w_old:.0} ns, cube {w_shells:.0} ns, columns {w_columns:.0} ns"
        );
    }
    // What a village test spends before reading the registry: the ±7 square of chunks checked
    // one by one (225 lookups) or the square found once in the memo of indexed squares.
    for x in -20..=20 {
        for z in -20..=20 {
            p.indexed.insert((x, z));
        }
    }
    p.indexed_squares.insert((0, 0, 7));
    let square = |p: &Pois, i: u32| {
        let c = (i & 1) as i32;
        (c - 7..=c + 7).flat_map(|z| (-7..=7).map(move |x| (x, z))).filter(|k| p.indexed.contains(k)).count()
    };
    let memo = |p: &Pois, _: u32| usize::from(p.indexed_squares.contains(&black_box((0, 0, 7))));
    let checked = time(&mut p, PoiPaths::OLD, 100_000, &square);
    let remembered = time(&mut p, PoiPaths::OLD, 100_000, &memo);
    eprintln!("indexing check of a village test: square checked {checked:.0} ns, square remembered {remembered:.0} ns");
    for (name, s) in sites {
        let center = [s.0, s.1, s.2];
        let per_section = |p: &Pois, _: u32| {
            let c = crate::mobs::golems::iron::closest_section(center, 2, |s| p.sections_to_village((s[0], s[1], s[2])));
            c[0] as usize
        };
        let batched = |p: &Pois, _: u32| {
            let villages = p.village_sections_in_cube(s, 8);
            let c = crate::mobs::golems::iron::closest_section(center, 2, |q| {
                village_distance_among(&villages, (q[0], q[1], q[2]))
            });
            c[0] as usize
        };
        let old = time(&mut p, PoiPaths::OLD, 2_000, &per_section);
        let shells = time(&mut p, SHELLS, 2_000, &per_section);
        let batch = time(&mut p, PoiPaths::OLD, 2_000, &batched);
        let batch_columns = time(&mut p, COLUMNS, 2_000, &batched);
        eprintln!(
            "closest section {name}: one search per section old {old:.0} ns, shells {shells:.0} ns; \
             one collection {batch:.0} ns, through the columns {batch_columns:.0} ns"
        );
    }
}
