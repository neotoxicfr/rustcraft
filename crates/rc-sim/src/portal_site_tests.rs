//! Equivalence proofs and micro-benches of the portal optimisations: touched-portal prefilter,
//! exit portal site search, gateway cooldown lookup.

use hashbrown::HashMap;
use rc_physics::Aabb;

use crate::blocks::{Dir, P, flag};
use crate::dims::{NETHER, Transition};
use crate::mobs::jrand::JRand;
use crate::portal::GatewayKeyRef;
use crate::portal_site::{SiteChoice, SitePaths};
use crate::portal_tests::open_new_world;
use crate::server::Server;

fn ready(s: &mut Server) {
    s.loaded = true;
    s.teleport = None;
}

fn state(s: &Server, name: &str) -> u32 {
    s.view().default_state(s.table().block_id(name))
}

/// (kind, position) of each touched portal, in order.
fn touched(s: &Server, b: &Aabb, prefilter: bool, table_ids: bool) -> Vec<(crate::dims::PortalKind, P)> {
    s.portals_touched_with(b, prefilter, table_ids).into_iter().map(|t| (t.kind, t.pos)).collect()
}

/// Random box of an entity around `center` (sizes from an item to a ghast).
fn random_box(r: &mut JRand, center: [f64; 3]) -> Aabb {
    let w = 0.25 + r.next_double() * 3.75;
    let h = 0.25 + r.next_double() * 3.75;
    let p = [
        center[0] + (r.next_double() - 0.5) * 8.0,
        center[1] + (r.next_double() - 0.5) * 8.0,
        center[2] + (r.next_double() - 0.5) * 8.0,
    ];
    Aabb { min: [p[0] - w / 2.0, p[1], p[2] - w / 2.0], max: [p[0] + w / 2.0, p[1] + h, p[2] + w / 2.0] }
}

/// Portal blocks of the three kinds set across a section boundary and beside a chunk taken out
/// of the map; returns the centers to draw boxes around.
fn set_portals(s: &mut Server, spawn: P) -> Vec<[f64; 3]> {
    let (np, ep, gw) = (state(s, "nether_portal"), state(s, "end_portal"), state(s, "end_gateway"));
    // A section boundary at least 3 sections above the bottom of the world.
    let boundary = s.dim.height.min_y + 16 * 8;
    // Column at the east edge of its chunk: the next chunk east is removed below.
    let x = (spawn[0] & !15) + 15;
    let z = spawn[2];
    s.load_now(&[(x >> 4, z >> 4), ((x >> 4) + 1, z >> 4), (x >> 4, (z >> 4) + 1)]);
    let mut centers = Vec::new();
    for (i, st) in [np, ep, gw].into_iter().enumerate() {
        let zi = z + 3 * i as i32;
        for y in boundary - 1..=boundary {
            s.set_block([x, y, zi], st, flag::CLIENTS | flag::KNOWN_SHAPE);
            s.set_block([x - 1, y, zi], st, flag::CLIENTS | flag::KNOWN_SHAPE);
        }
        centers.push([f64::from(x), f64::from(boundary), f64::from(zi)]);
    }
    s.chunks.remove(&((x >> 4) + 1, z >> 4));
    // Far from any portal, in open air and underground.
    centers.push([f64::from(x) - 40.0, f64::from(boundary), f64::from(z)]);
    centers.push([f64::from(x) - 40.0, f64::from(s.dim.height.min_y + 20), f64::from(z)]);
    centers
}

/// The touched-portal check with the palette prefilter returns the same portals in the same
/// order as the cell-by-cell walk, on 10,000 random boxes near portals of the three kinds, across
/// a section boundary and beside an absent chunk; ids from the table equal ids by name.
#[test]
fn portals_touched_prefilter_matches_cell_by_cell() {
    let Some((mut s, dir)) = open_new_world("portal-touch-prefilter", 20_261_107) else { return };
    ready(&mut s);
    let spawn = s.level.spawn_pos;
    let centers = set_portals(&mut s, spawn);
    let mut r = JRand::new(7);
    let mut seen = [0usize; 3];
    for i in 0..10_000 {
        let b = random_box(&mut r, centers[i % centers.len()]);
        let want = touched(&s, &b, false, false);
        assert_eq!(touched(&s, &b, true, true), want, "box {b:?}");
        assert_eq!(touched(&s, &b, false, true), want, "box {b:?} (table ids)");
        for (kind, _) in &want {
            seen[match kind {
                crate::dims::PortalKind::Nether => 0,
                crate::dims::PortalKind::End => 1,
                crate::dims::PortalKind::Gateway => 2,
            }] += 1;
        }
    }
    eprintln!("touches seen (nether, end, gateway): {seen:?}");
    assert!(seen.iter().all(|&n| n > 100), "too few touches to prove anything: {seen:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn gateway_key_lookup_without_copy_finds_the_owned_key() {
    let mut map: HashMap<(String, P), i32> = HashMap::new();
    map.insert(("minecraft:the_end".to_owned(), [96, 75, 0]), 40);
    map.insert(("minecraft:overworld".to_owned(), [96, 75, 0]), 7);
    assert_eq!(map.get(&GatewayKeyRef("minecraft:the_end", [96, 75, 0])), Some(&40));
    assert_eq!(map.get(&GatewayKeyRef("minecraft:overworld", [96, 75, 0])), Some(&7));
    assert_eq!(map.get(&GatewayKeyRef("minecraft:the_end", [96, 75, 1])), None);
    assert_eq!(map.get(&GatewayKeyRef("minecraft:the_nether", [96, 75, 0])), None);
}

/// At equal distance the first spot offered stays, a full spot beats any nearer partial one, and
/// chaining the choices of consecutive shares gives the choice of one walk.
#[test]
fn site_choice_keeps_the_first_of_equal_spots() {
    let spots: [(i64, P, bool); 7] = [
        (9, [1, 0, 0], false),
        (4, [2, 0, 0], false),
        (4, [3, 0, 0], false),
        (25, [4, 0, 0], true),
        (16, [5, 0, 0], true),
        (16, [6, 0, 0], true),
        (1, [7, 0, 0], false),
    ];
    let mut one = SiteChoice::default();
    for &(d, p, full) in &spots {
        one.offer(d, p, full);
    }
    assert_eq!(one.best(), Some([5, 0, 0]));
    let mut partial_only = SiteChoice::default();
    for &(d, p, _) in &spots[..3] {
        partial_only.offer(d, p, false);
    }
    assert_eq!(partial_only.best(), Some([2, 0, 0]));
    for cut in 0..=spots.len() {
        let (mut a, mut b) = (SiteChoice::default(), SiteChoice::default());
        spots[..cut].iter().for_each(|&(d, p, f)| a.offer(d, p, f));
        spots[cut..].iter().for_each(|&(d, p, f)| b.offer(d, p, f));
        assert_eq!(a.then(b).best(), one.best(), "cut at {cut}");
    }
}

/// Every combination of the site search leads, threads included.
fn all_paths() -> Vec<SitePaths> {
    let mut out = Vec::new();
    for bits in 0..16u8 {
        let bit = |i: u8| bits & (1 << i) != 0;
        out.push(SitePaths { heightmap: bit(0), column_reads: bit(1), distance_cutoff: bit(2), parallel: bit(3) });
    }
    out
}

/// Pools of 1, 2, 3 and 8 threads for the shared search.
fn test_pools() -> Vec<rayon::ThreadPool> {
    [1, 2, 3, 8].into_iter().map(|t| rayon::ThreadPoolBuilder::new().num_threads(t).build().expect("pool")).collect()
}

/// Same rule as the game's motion-blocking heightmap, scanned down the column.
fn tag_scan_height(s: &Server, x: i32, z: i32) -> i32 {
    for y in (s.dim.height.min_y..s.top_y()).rev() {
        let p = [x, y, z];
        if s.btag(p, "minecraft:blocks_motion_in_heightmap") || !s.fluid_of(s.get(p)).is_empty() {
            return y + 1;
        }
    }
    s.dim.height.min_y
}

/// Area of `radius` blocks around `center` loaded with the 20-block margin of the search.
fn load_area(s: &mut Server, center: P, radius: i32) {
    let r = radius + 16 + 4;
    let list: Vec<(i32, i32)> = ((center[0] - r) >> 4..=(center[0] + r) >> 4)
        .flat_map(|x| ((center[2] - r) >> 4..=(center[2] + r) >> 4).map(move |z| (x, z)))
        .collect();
    s.load_now(&list);
}

/// Checks of one dimension: every path against the reference on `origins` random arrival points
/// within `radius` of `center`; returns the number of points where a spot was found.
fn check_site_paths(s: &mut Server, center: P, radius: i32, origins: usize, seed: i64) -> usize {
    load_area(s, center, radius);
    let mut r = JRand::new(seed);
    let (min_y, max_y) = (s.dim.height.min_y, s.max_placeable_y());
    let mut found = 0;
    let pools = test_pools();
    // Every failing combination is listed, so that one run tells which lead broke.
    let mut failures: Vec<String> = Vec::new();
    for n in 0..origins {
        let origin = [
            center[0] + r.int_between(-radius, radius),
            r.int_between(min_y, max_y),
            center[2] + r.int_between(-radius, radius),
        ];
        let dir = if n % 2 == 0 { Dir::East } else { Dir::South };
        // The area holds no column where the former height rule and the game's differ (checked by
        // the caller): every path, heightmap included, must find the reference spot.
        let want = s.portal_site_cell_by_cell(origin, dir);
        found += usize::from(want.is_some());
        for paths in all_paths() {
            let mut at = |what: &str, got: Option<P>| {
                let failure = format!("{paths:?} {what}");
                if got != want && !failures.contains(&failure) {
                    eprintln!("{} origin {origin:?} {dir:?}: {got:?} instead of {want:?}, {failure}", s.dim.name);
                    failures.push(failure);
                }
            };
            if paths.parallel {
                let search = s.portal_site_search(origin, dir, paths);
                for pool in &pools {
                    at(&format!("{} threads", pool.current_num_threads()), pool.install(|| search.run()));
                }
            } else {
                at("one thread", s.portal_site(origin, dir, paths));
            }
        }
    }
    assert!(failures.is_empty(), "{}: {} failing combinations:\n{}", s.dim.name, failures.len(), failures.join("\n"));
    found
}

/// Columns of the area where the live heightmap, a tag scan and the former local rule disagree:
/// (live map against tag scan, former rule against tag scan).
fn height_differences(s: &Server, center: P, radius: i32) -> (usize, usize) {
    let cobweb = s.table().block_id("cobweb");
    let sapling = s.table().block_id("bamboo_sapling");
    let former = |x: i32, z: i32| {
        for y in (s.dim.height.min_y..s.top_y()).rev() {
            let st = s.get([x, y, z]);
            let b = s.view().block(st);
            if (s.view().legacy_solid(st) && b != cobweb && b != sapling) || !s.fluid_of(st).is_empty() {
                return y + 1;
            }
        }
        s.dim.height.min_y
    };
    let (mut live, mut rule) = (0, 0);
    for x in center[0] - radius..=center[0] + radius {
        for z in center[2] - radius..=center[2] + radius {
            let scan = tag_scan_height(s, x, z);
            live += usize::from(s.hm_motion_blocking(x, z) != scan);
            rule += usize::from(former(x, z) != scan);
        }
    }
    (live, rule)
}

/// Built spots in open air: a pool of water, cobwebs and bamboo saplings near the center, so the
/// fluid and the two exceptions of the former height rule are walked.
fn build_column_cases(s: &mut Server, center: P) {
    let top = s.hm_motion_blocking(center[0], center[2]);
    let (water, web, sapling) = (state(s, "water"), state(s, "cobweb"), state(s, "bamboo_sapling"));
    for i in 0..4 {
        s.set_block([center[0] + i, top, center[2] + 2], water, flag::ALL);
        s.set_block([center[0] - 2, top + 3 + i, center[2] - i], web, flag::ALL);
        s.set_block([center[0] + 3, top, center[2] - 3 - i], sapling, flag::ALL);
    }
}

/// Exit portal site: every combination of the leads (heightmap, column reads, distance cutoff,
/// shares on 1 to 8 threads) chooses the reference spot, Overworld then Nether, on random
/// arrival points; the live heightmap equals a scan with the game's rule.
#[test]
fn portal_site_paths_choose_the_reference_spot() {
    let Some((mut s, dir)) = open_new_world("portal-site-paths", 1_234_567_890_123_456_789) else { return };
    ready(&mut s);
    let spawn = s.level.spawn_pos;
    let center = [spawn[0], 0, spawn[2]];
    load_area(&mut s, center, 24);
    build_column_cases(&mut s, center);
    // Columns reached by the spirals of arrival points up to 24 blocks away.
    assert_eq!(height_differences(&s, center, 24 + 16), (0, 0), "heights off the game's rule (live map, former rule)");
    let found = check_site_paths(&mut s, center, 24, 60, 11);
    let found_built = check_site_paths(&mut s, center, 4, 20, 12);
    eprintln!("overworld: {found}+{found_built} spots found");
    assert!(found > 30, "too few spots found to prove anything: {found}");
    s.change_dimension(Transition {
        dim: NETHER.into(),
        pos: [f64::from(center[0] / 8), 70.0, f64::from(center[2] / 8)],
        yaw: 0.0,
        pitch: 0.0,
        relative: 0,
        sound: false,
        ticket: false,
        missing_respawn: false,
    });
    ready(&mut s);
    assert_eq!(s.dim.name, NETHER);
    let nether_center = [center[0] / 8, 0, center[2] / 8];
    load_area(&mut s, nether_center, 24);
    assert_eq!(height_differences(&s, nether_center, 24 + 16), (0, 0), "Nether heights off the game's rule");
    let found = check_site_paths(&mut s, nether_center, 24, 60, 13);
    eprintln!("nether: {found} spots found");
    assert!(found > 10, "too few spots found in the Nether: {found}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Same proof on 200 seeds, ten arrival points each (long: `--ignored`).
#[test]
#[ignore]
fn portal_site_paths_choose_the_reference_spot_on_200_seeds() {
    let mut skipped = 0;
    for seed in 1..=200i64 {
        let Some((mut s, dir)) = open_new_world(&format!("portal-site-seed-{seed}"), seed * 7_919) else { return };
        ready(&mut s);
        let spawn = s.level.spawn_pos;
        let center = [spawn[0], 0, spawn[2]];
        load_area(&mut s, center, 8);
        let (live, rule) = height_differences(&s, center, 8 + 16);
        assert_eq!(live, 0, "seed {seed}: live heightmap off the game's rule");
        // A sign on top of a column: the heightmap path follows the game there, not the reference.
        if rule == 0 {
            check_site_paths(&mut s, center, 8, 10, seed);
        } else {
            skipped += 1;
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
    eprintln!("{skipped} seeds skipped (sign on top of a column)");
}

/// Blocks whose states the former height rule (legacy solid, cobweb and bamboo sapling excepted)
/// and the game's `blocks_motion_in_heightmap` tag classify differently: the 26 standing and wall
/// signs, solid for the former rule but outside the tag. A sign on top of a column raised its
/// height under the former rule; the heightmap path, like the game, ignores it.
#[test]
fn former_height_rule_against_the_game_tag() {
    let Some((s, dir)) = open_new_world("portal-height-rule", 5) else { return };
    let cobweb = s.table().block_id("cobweb");
    let sapling = s.table().block_id("bamboo_sapling");
    let mut differ: Vec<String> = Vec::new();
    for st in 0..s.data.states.len() as u32 {
        let b = s.view().block(st);
        let former = s.view().legacy_solid(st) && b != cobweb && b != sapling;
        let tag = s.tag_u32("minecraft:blocks_motion_in_heightmap", u32::from(b), crate::tag_bits::mode());
        if former != tag {
            assert!(former && !tag, "state {st}: in the tag but not solid for the former rule");
            let name = s.data.blocks[b as usize].name.clone();
            if !differ.contains(&name) {
                differ.push(name);
            }
        }
    }
    eprintln!("{} blocks differ: {differ:?}", differ.len());
    assert_eq!(differ.len(), 26, "{differ:?}");
    assert!(differ.iter().all(|n| n.ends_with("_sign")), "{differ:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

// --- Micro-benches (ignored): `cargo test --release -p rc-sim -- --ignored bench_portal`. ---

fn time_per_call(n: u32, mut f: impl FnMut() -> usize) -> (f64, usize) {
    let mut found = 0;
    let t = std::time::Instant::now();
    for _ in 0..n {
        found += f();
    }
    (t.elapsed().as_nanos() as f64 / f64::from(n), found)
}

/// Touched-portal check of a mob box (0.6 × 1.8): far from any portal in open air and
/// underground, in a section holding a portal, in the portal; prefilter and ids on and off.
#[test]
#[ignore]
fn bench_portals_touched() {
    let Some((mut s, dir)) = open_new_world("portal-touch-bench", 81) else { return };
    ready(&mut s);
    let spawn = s.level.spawn_pos;
    let centers = set_portals(&mut s, spawn);
    let mob = |c: [f64; 3]| Aabb { min: [c[0] - 0.3, c[1], c[2] - 0.3], max: [c[0] + 0.3, c[1] + 1.8, c[2] + 0.3] };
    let cases = [
        ("far, open air", centers[3]),
        ("far, underground", centers[4]),
        ("portal section, beside", [centers[0][0] - 6.0, centers[0][1], centers[0][2]]),
        ("in the Nether portal", [centers[0][0] + 0.5, centers[0][1] - 1.0, centers[0][2] + 0.5]),
    ];
    for (label, c) in cases {
        for (prefilter, table_ids) in [(false, false), (false, true), (true, false), (true, true)] {
            let b = mob(c);
            let (ns, found) = time_per_call(200_000, || s.portals_touched_with(&b, prefilter, table_ids).len());
            eprintln!("{label:24} prefilter {prefilter:5} table ids {table_ids:5}: {ns:6.1} ns per call ({found})");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Site search of the exit portal (ms per search), Overworld then Nether, each lead alone and
/// combined, then the shares on 1 thread and powers of 2 up to every logical core.
#[test]
#[ignore]
fn bench_portal_site() {
    let Some((mut s, dir)) = open_new_world("portal-site-bench", 1_234_567_890_123_456_789) else { return };
    ready(&mut s);
    let spawn = s.level.spawn_pos;
    let center = [spawn[0], 0, spawn[2]];
    bench_site_dimension(&mut s, center, "overworld");
    s.change_dimension(Transition {
        dim: NETHER.into(),
        pos: [f64::from(center[0] / 8), 70.0, f64::from(center[2] / 8)],
        yaw: 0.0,
        pitch: 0.0,
        relative: 0,
        sound: false,
        ticket: false,
        missing_respawn: false,
    });
    ready(&mut s);
    bench_site_dimension(&mut s, [center[0] / 8, 0, center[2] / 8], "nether");
    let _ = std::fs::remove_dir_all(&dir);
}

fn bench_site_dimension(s: &mut Server, center: P, label: &str) {
    load_area(s, center, 8);
    let mut r = JRand::new(3);
    let origins: Vec<P> = (0..16)
        .map(|_| [center[0] + r.int_between(-8, 8), r.int_between(32, 120), center[2] + r.int_between(-8, 8)])
        .collect();
    let off = SitePaths::REFERENCE;
    let all = SitePaths { heightmap: true, column_reads: true, distance_cutoff: true, parallel: false };
    let variants = [
        ("reference", off),
        ("heightmap", SitePaths { heightmap: true, ..off }),
        ("column reads", SitePaths { column_reads: true, ..off }),
        ("distance cutoff", SitePaths { distance_cutoff: true, ..off }),
        ("heightmap + column reads", SitePaths { heightmap: true, column_reads: true, ..off }),
        ("all three", all),
        ("shares, no cutoff", SitePaths { distance_cutoff: false, parallel: true, ..all }),
        ("shares, all", SitePaths { parallel: true, ..all }),
    ];
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    for (name, paths) in variants {
        let mut threads = vec![1];
        while paths.parallel && threads.last().is_some_and(|&t| t < cores) {
            threads.push((threads.last().copied().unwrap_or(1) * 2).min(cores));
        }
        for t in threads {
            let pool = rayon::ThreadPoolBuilder::new().num_threads(t).build().expect("pool");
            let site = |o: P| {
                if paths.parallel {
                    // The search is prepared on the calling thread, like on the server thread.
                    let search = s.portal_site_search(o, Dir::East, paths);
                    pool.install(|| search.run())
                } else {
                    s.portal_site(o, Dir::East, paths)
                }
            };
            let (ns, found) = time_per_call(3, || origins.iter().filter(|&&o| site(o).is_some()).count());
            let ms = ns / 1.0e6 / origins.len() as f64;
            eprintln!("{label:9} {name:26} {t:2} threads: {ms:7.3} ms per search ({found} found)");
        }
    }
}

/// Cooldown lookup of 20 End gateways (two per gateway and per tick), with and without the copy
/// of the dimension name.
#[test]
#[ignore]
fn bench_gateway_lookup() {
    let Some((mut s, dir)) = open_new_world("portal-gateway-bench", 82) else { return };
    let gateways: Vec<P> = (0..20).map(|i| [i * 3, 80, 0]).collect();
    for (i, p) in gateways.iter().enumerate() {
        if i % 2 == 0 {
            s.dims.gateways.insert((s.dim.name.clone(), *p), 40);
        }
    }
    for without_copy in [false, true, false, true] {
        let (ns, cooling) =
            time_per_call(100_000, || gateways.iter().filter(|&&p| s.gateway_cooling_with(p, without_copy)).count());
        eprintln!(
            "gateway lookup without copy {without_copy:5}: {:6.1} ns per tick of 20 gateways ({cooling})",
            ns * 2.0
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
