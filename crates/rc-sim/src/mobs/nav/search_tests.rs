//! Equivalence proofs of the path search on the village of the bench model world
//! (`RUSTCRAFT_OPT_WORLD`, otherwise `target/perf/model` of the repository): 2,000 searches
//! drawn with a fixed seed (ground walkers of every size, flyers, amphibians, swimmers, the
//! warden, a linked creaking, penalties set along the way, one to four targets) and 2,000 static
//! cell types, reduced to one SHA-256 fingerprint compared with the value recorded on the code
//! before the optimisations. Without the model world the tests skip (explicit failure under
//! `RUSTCRAFT_OPT_REQUIRE=1`).

use std::cell::Cell;
use std::path::PathBuf;
use std::sync::Arc;

use rc_physics::Aabb;
use rc_testdata::need;

use super::{
    MemoKind, MobView, NodeEvaluator, Path, PathType, SearchOptions, TypeRules, TypeSet, find_path, find_path_multi,
    find_path_with, static_path_type,
};
use crate::mobs::env::Fluid;
use crate::mobs::jrand::JRand;
use crate::mobs::species::Kind;
use crate::server::Server;

/// Village of the bench model world (`scenes.json`, `found.village`).
const VILLAGE: [i32; 2] = [-1408, 368];
/// The only region file the searches read (chunks -96..-65 × 0..31).
const VILLAGE_REGION: &str = "r.-3.0.mca";

/// Fingerprint of the 2,000 searches and 2,000 static types, recorded on the code before the
/// optimisations of the path search (integ/optim3-v1 ce5eb8b7): 2,000 paths, 572 reaching their
/// target. An optimisation never changes it.
const EXPECTED_SEARCHES: &str = "a68d85057b651eca9b6f4de8c4719c85a8c7ce3766df78d725bea091eb1aeabe";

/// Bench model world folder: `RUSTCRAFT_OPT_WORLD`, otherwise `target/perf/model`.
fn model_dir() -> Option<PathBuf> {
    if let Some(p) = rc_data::env_os!("RUSTCRAFT_OPT_WORLD") {
        return Some(PathBuf::from(p));
    }
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/perf/model");
    p.join("level.dat").is_file().then_some(p)
}

fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).expect("folder");
    for e in std::fs::read_dir(from).expect("model folder").flatten() {
        let target = to.join(e.file_name());
        if e.path().is_dir() {
            copy_tree(&e.path(), &target);
        } else {
            std::fs::copy(e.path(), target).expect("model file");
        }
    }
}

/// Server on a copy of the village part of the model world (level, data, packs and the one region
/// file), chunks within 7 of the village loaded from that file (none generated).
pub(crate) fn village_server(tag: &str) -> Option<Server> {
    let data = Arc::new(need(rc_testdata::version_data(), "26.3 data cache")?);
    need(crate::registries::client_jar(&data), "jar client 26.3")?;
    let model = need(model_dir(), "bench model world (RUSTCRAFT_OPT_WORLD or target/perf/model)")?;
    let region = model.join("dimensions/minecraft/overworld/region").join(VILLAGE_REGION);
    let region = need(region.is_file().then_some(region), "village region file of the model world")?;
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-sim-{tag}-{}", std::process::id())));
    std::fs::create_dir_all(&dir).expect("world folder");
    std::fs::copy(model.join("level.dat"), dir.join("level.dat")).expect("level.dat");
    for sub in ["data", "datapacks"] {
        copy_tree(&model.join(sub), &dir.join(sub));
    }
    let to = dir.join("dimensions/minecraft/overworld/region");
    std::fs::create_dir_all(&to).expect("region folder");
    std::fs::copy(&region, to.join(VILLAGE_REGION)).expect("region file");
    let mut s = crate::portal_tests::open_world(&dir, data);
    s.enter_world(crate::dims::OVERWORLD);
    s.pos = [f64::from(VILLAGE[0]) + 0.5, 90.0, f64::from(VILLAGE[1]) + 0.5];
    let (cx, cz) = (VILLAGE[0] >> 4, VILLAGE[1] >> 4);
    let list: Vec<(i32, i32)> = (cx - 7..=cx + 7).flat_map(|x| (cz - 7..=cz + 7).map(move |z| (x, z))).collect();
    s.load_now(&list);
    let missing = list.iter().filter(|c| !s.chunks.get(*c).is_some_and(Option::is_some)).count();
    assert_eq!(missing, 0, "village chunks missing from the model world");
    s.stop_gen();
    Some(s)
}

/// Body and navigation of a searching mob.
#[derive(Clone, Copy)]
struct Profile {
    kind: Kind,
    width: f32,
    height: f32,
    step: f64,
    fly: bool,
    amphibious: bool,
    swim: bool,
    can_open_doors: bool,
    follow: f32,
}

const fn walker(kind: Kind, width: f32, height: f32, step: f64, follow: f32) -> Profile {
    Profile { kind, width, height, step, fly: false, amphibious: false, swim: false, can_open_doors: false, follow }
}

/// Every evaluator and footprint: ground (1×1, 1×2, 1×3, 2×1, 2×3, 3×3 cells), doors, flyers,
/// amphibians, swimmers, flat distances (warden), heart link (creaking).
const PROFILES: [Profile; 20] = [
    Profile { can_open_doors: true, ..walker(Kind::Villager, 0.6, 1.95, 0.6, 48.0) },
    walker(Kind::Villager, 0.6, 1.95, 0.6, 48.0),
    walker(Kind::Zombie, 0.6, 1.95, 0.6, 35.0),
    walker(Kind::IronGolem, 1.4, 2.7, 1.0, 16.0),
    walker(Kind::Spider, 1.4, 0.9, 0.6, 16.0),
    walker(Kind::Cat, 0.6, 0.7, 0.6, 16.0),
    walker(Kind::Chicken, 0.4, 0.7, 0.6, 16.0),
    walker(Kind::Ravager, 1.95, 2.2, 1.0, 32.0),
    walker(Kind::Warden, 0.9, 2.9, 1.0, 24.0),
    walker(Kind::Creaking, 0.9, 2.7, 1.0, 32.0),
    walker(Kind::Fox, 0.6, 0.7, 0.6, 16.0),
    walker(Kind::Enderman, 0.6, 2.9, 1.0, 64.0),
    walker(Kind::Camel, 1.7, 2.375, 1.5, 16.0),
    walker(Kind::Sniffer, 1.9, 1.75, 0.6, 16.0),
    walker(Kind::CopperGolem, 0.49, 0.98, 0.6, 16.0),
    Profile { fly: true, ..walker(Kind::Bee, 0.7, 0.6, 0.6, 48.0) },
    Profile { fly: true, ..walker(Kind::Parrot, 0.5, 0.9, 0.6, 16.0) },
    Profile { fly: true, ..walker(Kind::Allay, 0.35, 0.6, 0.6, 48.0) },
    Profile { amphibious: true, ..walker(Kind::Turtle, 1.2, 0.4, 1.0, 16.0) },
    Profile { swim: true, ..walker(Kind::Dolphin, 0.9, 0.6, 0.6, 16.0) },
];

/// One drawn search.
pub(crate) struct Search {
    pub(crate) view: MobView,
    pub(crate) targets: Vec<[i32; 3]>,
    pub(crate) max_len: f32,
    pub(crate) reach: i32,
    pub(crate) max_visited: i32,
}

fn surface(s: &Server, x: i32, z: i32) -> i32 {
    s.surface_y(x, z, 4)
}

fn view_at(s: &Server, r: &mut JRand, p: &Profile, block: [i32; 3]) -> MobView {
    let pos = [f64::from(block[0]) + 0.5, f64::from(block[1]), f64::from(block[2]) + 0.5];
    let hw = f64::from(p.width) / 2.0;
    let bbox =
        Aabb { min: [pos[0] - hw, pos[1], pos[2] - hw], max: [pos[0] + hw, pos[1] + f64::from(p.height), pos[2] + hw] };
    let in_water = s.mob_fluid(block) == Fluid::Water;
    // Penalties set along the way on one search in eight (owner following, panic…).
    let malus_override = match r.next_int_bound(8) {
        0 => vec![(super::PathType::Water, 0.0)],
        1 => vec![(super::PathType::DoorWoodClosed, 0.0), (super::PathType::Fence, -1.0)],
        _ => Vec::new(),
    };
    let home = (p.kind == Kind::Creaking).then(|| [block[0] + r.next_int_bound(9) - 4, block[1], block[2] + 6]);
    MobView {
        kind: p.kind,
        pos,
        block,
        bbox,
        width: p.width,
        height: p.height,
        step: p.step,
        on_ground: !p.fly && !p.swim,
        in_water,
        max_fall: 3 + r.next_int_bound(4),
        can_float: !p.swim && r.next_bool(),
        can_open_doors: p.can_open_doors,
        fly: p.fly,
        amphibious: p.amphibious,
        rng_seed: r.next_long(),
        swim: p.swim,
        breach: p.swim,
        home,
        malus_override,
    }
}

/// The 2,000 searches of the fingerprint (fixed seed).
pub(crate) fn draw_searches(s: &Server, n: usize) -> Vec<Search> {
    let mut r = JRand::new(0xC6_0002);
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let p = PROFILES[i % PROFILES.len()];
        let x = VILLAGE[0] + r.next_int_bound(113) - 56;
        let z = VILLAGE[1] + r.next_int_bound(113) - 56;
        let lift = if p.fly { r.next_int_bound(5) } else { 0 };
        let start = [x, surface(s, x, z) + lift, z];
        let view = view_at(s, &mut r, &p, start);
        let count = if r.next_int_bound(4) == 0 { 1 + r.next_int_bound(4) } else { 1 };
        let targets = (0..count)
            .map(|_| {
                let tx = x + r.next_int_bound(65) - 32;
                let tz = z + r.next_int_bound(65) - 32;
                let ty = if p.fly || p.swim { start[1] + r.next_int_bound(9) - 4 } else { surface(s, tx, tz) };
                [tx, ty, tz]
            })
            .collect();
        let follow = p.follow;
        out.push(Search {
            view,
            targets,
            max_len: if r.next_bool() { follow.max(16.0) } else { 16.0 },
            reach: r.next_int_bound(3),
            max_visited: (follow * 16.0).floor() as i32,
        });
    }
    out
}

/// Text form of a path: everything a caller can read.
pub(crate) fn path_line(p: &Option<Path>) -> String {
    let Some(p) = p else { return "none".into() };
    let mut line =
        format!("{:?} {} {:08x} {} {}:", p.target, p.reached, p.dist_to_target.to_bits(), p.next, p.nodes.len());
    for (n, t) in &p.nodes {
        line.push_str(&format!(" {},{},{},{t:?}", n[0], n[1], n[2]));
    }
    line
}

/// Result of a search through the public entry points (one target: single-target search).
fn run_search(s: &Server, q: &Search) -> Option<Path> {
    if q.targets.len() == 1 {
        find_path(s, &q.view, q.targets[0], q.max_len, q.reach, q.max_visited)
    } else {
        find_path_multi(s, &q.view, &q.targets, q.max_len, q.reach, q.max_visited)
    }
}

/// Result of a search with explicit switches.
pub(crate) fn run_search_with(s: &Server, q: &Search, opts: SearchOptions) -> Option<Path> {
    find_path_with(s, &q.view, &q.targets, q.max_len, q.reach, q.max_visited, opts)
}

fn sha(lines: &[String]) -> String {
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    for l in lines {
        ctx.update(l.as_bytes());
        ctx.update(b"\n");
    }
    ctx.finish().as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// Cells of the static type check: one drawn around each search's mob.
fn static_cells(searches: &[Search]) -> Vec<[i32; 3]> {
    let mut r = JRand::new(0xC6_0003);
    searches
        .iter()
        .map(|q| {
            let b = q.view.block;
            [b[0] + r.next_int_bound(21) - 10, b[1] + r.next_int_bound(9) - 4, b[2] + r.next_int_bound(21) - 10]
        })
        .collect()
}

/// Static types of the drawn cells (random-position tests, `has_malus`) with switches `opts`.
fn static_lines(s: &Server, searches: &[Search], opts: Option<SearchOptions>) -> Vec<String> {
    searches
        .iter()
        .zip(static_cells(searches))
        .map(|(q, p)| {
            let t = match opts {
                None => static_path_type(s, &q.view, p),
                Some(o) => NodeEvaluator::single_cell_with(s, &q.view, o).static_type(p[0], p[1], p[2]),
            };
            format!("{p:?} {t:?}")
        })
        .collect()
}

/// Bench helper: mean ns of one block path type over `cells` with `opts` (one evaluator, no memo).
pub(crate) fn block_type_ns(s: &Server, view: &MobView, cells: &[[i32; 3]], opts: SearchOptions) -> f64 {
    let mut e =
        NodeEvaluator::new(s, view, SearchOptions { memo: MemoKind::Off, ..opts }, super::SearchBuffers::default());
    let t0 = std::time::Instant::now();
    let mut sum = 0u32;
    for &p in cells {
        sum = sum.wrapping_add(e.block_type(p) as u32);
    }
    std::hint::black_box(sum);
    t0.elapsed().as_nanos() as f64 / cells.len() as f64
}

thread_local! {
    /// On: every value a memo serves is computed again and compared (proof tests only).
    static MEMO_CHECK: Cell<bool> = const { Cell::new(false) };
}

/// Memo hit of the evaluator: under `MEMO_CHECK`, the kept value must equal a fresh computation.
pub(super) fn check_memo_hit(kept: PathType, compute: impl FnOnce() -> PathType) {
    if MEMO_CHECK.get() {
        assert_eq!(kept, compute(), "a memo of the search served a stale type");
    }
}

#[test]
fn village_searches_match_the_recorded_fingerprint() {
    let Some(mut s) = village_server("path-golden") else { return };
    let searches = draw_searches(&s, 2000);
    let paths: Vec<Option<Path>> = searches.iter().map(|q| run_search(&s, q)).collect();
    let mut lines: Vec<String> = paths.iter().map(path_line).collect();
    lines.extend(static_lines(&s, &searches, None));
    let got = sha(&lines);
    s.stop_parked();
    let found = paths.iter().flatten().count();
    let reached = paths.iter().flatten().filter(|p| p.reached).count();
    let nodes: usize = paths.iter().flatten().map(|p| p.nodes.len()).sum();
    let kinds: std::collections::BTreeSet<String> =
        paths.iter().flatten().flat_map(|p| p.nodes.iter().map(|n| format!("{:?}", n.1))).collect();
    eprintln!("village searches: {found} paths, {reached} reached, {nodes} nodes, types {kinds:?}, fingerprint {got}");
    // Coverage: every search finds a path, a quarter reach their target, the paths cross many cell types.
    assert!(found == 2000 && reached > 500 && nodes > 20_000 && kinds.len() >= 6, "coverage");
    assert_eq!(got, EXPECTED_SEARCHES, "path search fingerprint changed");
}

/// Each lead alone, some pairs, all together: every search and every static type equal to the old
/// path, each value served by a memo checked against a fresh computation.
#[test]
fn every_lead_gives_the_old_searches() {
    let Some(mut s) = village_server("path-leads") else { return };
    let searches = draw_searches(&s, 2000);
    let old = SearchOptions::OLD;
    let want: Vec<String> = searches.iter().map(|q| path_line(&run_search_with(&s, q, old))).collect();
    let want_static = static_lines(&s, &searches, Some(old));
    let mut lines = want.clone();
    lines.extend(want_static.iter().cloned());
    assert_eq!(sha(&lines), EXPECTED_SEARCHES, "old path against the recorded fingerprint");
    let table = TypeRules::Table;
    let cases = [
        SearchOptions { type_rules: TypeRules::StateOnce, ..old },
        SearchOptions { type_rules: table, ..old },
        SearchOptions { memo: MemoKind::HashMap, ..old },
        SearchOptions { memo: MemoKind::DirectMapped, ..old },
        SearchOptions { scratch: true, ..old },
        SearchOptions { region: true, ..old },
        SearchOptions { palette: true, ..old },
        SearchOptions { type_rules: table, memo: MemoKind::HashMap, ..old },
        SearchOptions { type_rules: table, palette: true, region: true, ..old },
        SearchOptions { memo: MemoKind::DirectMapped, palette: true, scratch: true, ..old },
        SearchOptions::ALL_NEW,
    ];
    MEMO_CHECK.set(true);
    for opts in cases {
        // Twice: the second run reuses the buffers the first one kept.
        for _ in 0..2 {
            for (i, q) in searches.iter().enumerate() {
                assert_eq!(path_line(&run_search_with(&s, q, opts)), want[i], "search {i} with {opts:?}");
            }
        }
        assert_eq!(static_lines(&s, &searches, Some(opts)), want_static, "static types with {opts:?}");
    }
    MEMO_CHECK.set(false);
    s.stop_parked();
}

/// Block tags modified as by a reloaded data pack: each `tag` loses its first two blocks and gains `add`.
fn reload_tags(s: &mut Server, edits: &[(&str, &str)]) {
    let mut tags = s.regs.tags.clone();
    let blocks = &mut tags.iter_mut().find(|(r, _)| r == "minecraft:block").expect("block tags").1;
    for &(tag, add) in edits {
        let id = s.data.blocks.iter().position(|b| b.name == add).expect("added block") as i32;
        let e = blocks.iter_mut().find(|(n, _)| n == tag).expect("modified tag");
        e.1.drain(..2.min(e.1.len()));
        e.1.push(id);
    }
    s.set_registry_tags(tags);
}

/// Writes `st` at `p` straight into its section (no update, no neighbour: the proof only reads it).
fn put_state(s: &mut Server, p: [i32; 3], st: u32) {
    let data = s.data.clone();
    let ly = (p[1] - s.dim.height.min_y) as usize;
    let c = s.chunks.get_mut(&(p[0] >> 4, p[2] >> 4)).and_then(|c| c.as_deref_mut()).expect("loaded chunk");
    let idx = rc_world::block_index((p[0] & 15) as usize, ly & 15, (p[2] & 15) as usize) as u16;
    let air = |x: u32| data.state(x).is_some_and(|i| i.has(rc_data::flags::AIR));
    c.sections[ly >> 4].set_blocks(&[(idx, st)], data.state_bits() as u8, air);
}

/// Every state written in the world, old rules by position against the rules of the state and the
/// table, at the three tag levels; count of states per path type.
fn check_every_state(s: &mut Server, p: [i32; 3]) -> std::collections::BTreeMap<String, usize> {
    let view = draw_searches(s, 1).remove(0).view;
    let mut per_type = std::collections::BTreeMap::new();
    for st in 0..s.data.states.len() as u32 {
        put_state(s, p, st);
        let old = NodeEvaluator::single_cell_with(s, &view, SearchOptions::OLD).block_type(p);
        for rules in [TypeRules::StateOnce, TypeRules::Table] {
            for tag_mode in 0..3 {
                let mut e = NodeEvaluator::single_cell_with(
                    s,
                    &view,
                    SearchOptions { type_rules: rules, ..SearchOptions::OLD },
                );
                e.tag_mode = tag_mode;
                assert_eq!(e.type_of_state(st), old, "state {st} with {rules:?}, tag level {tag_mode}");
                assert_eq!(e.block_type(p), old, "state {st} read at {p:?} with {rules:?}, tag level {tag_mode}");
            }
        }
        *per_type.entry(format!("{old:?}")).or_insert(0) += 1;
    }
    put_state(s, p, 0);
    // Outside the table: the rules of an unknown state, like the old path's.
    let e =
        NodeEvaluator::single_cell_with(s, &view, SearchOptions { type_rules: TypeRules::Table, ..SearchOptions::OLD });
    for st in [s.data.states.len() as u32, u32::MAX] {
        assert_eq!(e.type_of_state(st), PathType::Open, "unknown state {st}");
    }
    per_type
}

#[test]
fn state_table_matches_the_rules_of_every_state_and_after_a_reload() {
    let Some(mut s) = village_server("path-states") else { return };
    // High above the village: no neighbour matters to a single block type.
    let p = [VILLAGE[0], 300, VILLAGE[1]];
    let before = check_every_state(&mut s, p);
    eprintln!("states per path type: {before:?}");
    for (t, at_least) in
        [("Blocked", 10_000), ("Open", 4_000), ("Fence", 10_000), ("DamageCautious", 20), ("Fire", 500)]
    {
        assert!(before.get(t).copied().unwrap_or(0) >= at_least, "coverage of {t}: {before:?}");
    }
    reload_tags(
        &mut s,
        &[
            ("minecraft:fences", "minecraft:stone"),
            ("minecraft:walls", "minecraft:dirt"),
            ("minecraft:speleothems", "minecraft:cobblestone"),
        ],
    );
    let after = check_every_state(&mut s, p);
    assert_ne!(before, after, "the reload changes some path types");
    s.stop_parked();
}

/// The section palette shortcut agrees with the neighbour scan for a block of every path type:
/// one block of each type alone in empty sections high above the village, the cell beside it gets
/// the same neighbour verdict with and without the shortcut (a dangerous type the palette test
/// forgot would leave the cell walkable).
#[test]
fn palette_shortcut_agrees_with_the_neighbour_scan_for_every_block_type() {
    let Some(mut s) = village_server("path-palette") else { return };
    let view = draw_searches(&s, 1).remove(0).view;
    let p = [VILLAGE[0], 300, VILLAGE[1]];
    let cell = [p[0] + 1, p[1], p[2]];
    let mut first_state = std::collections::BTreeMap::new();
    {
        let e = NodeEvaluator::single_cell_with(&s, &view, SearchOptions::ALL_NEW);
        for st in 0..s.data.states.len() as u32 {
            let t = e.type_of_state(st);
            first_state.entry(format!("{t:?}")).or_insert((st, t));
        }
    }
    for t in ["Damaging", "Fire", "Lava", "Water", "DamageCautious", "Open", "Blocked"] {
        assert!(first_state.contains_key(t), "no state of type {t}: {first_state:?}");
    }
    for &(st, t) in first_state.values() {
        put_state(&mut s, p, st);
        let verdict = |palette: bool| {
            let opts = SearchOptions { palette, ..SearchOptions::ALL_NEW };
            NodeEvaluator::new(&s, &view, opts, super::SearchBuffers::default())
                .neighbour_danger(cell[0], cell[1], cell[2])
        };
        let scanned = verdict(false);
        assert_eq!(verdict(true), scanned, "state {st} of type {t:?}: palette shortcut against the scan");
        assert_eq!(scanned, super::neighbour_verdict(t), "state {st} of type {t:?}: verdict of the scan");
    }
    put_state(&mut s, p, 0);
    s.stop_parked();
}

/// The kept buffers start each search empty: a block changed between two searches is seen by the
/// second one (a memo kept from the first would serve the old type).
#[test]
fn kept_buffers_see_a_block_changed_between_searches() {
    let Some(mut s) = village_server("path-fresh") else { return };
    let searches = draw_searches(&s, 2000);
    let stone = s.data.blocks.iter().position(|b| b.name == "minecraft:stone").expect("stone");
    let stone = s.data.states.iter().position(|i| usize::from(i.block) == stone).expect("stone state") as u32;
    let mut checked = 0;
    for q in searches.iter().filter(|q| !q.view.fly && !q.view.swim && !q.view.amphibious).take(200) {
        let Some(first) = run_search_with(&s, q, SearchOptions::ALL_NEW) else { continue };
        if first.nodes.len() < 6 {
            continue;
        }
        // A wall across the middle of the path, two blocks high.
        let mid = first.nodes[first.nodes.len() / 2].0;
        let saved = [s.st(mid), s.st([mid[0], mid[1] + 1, mid[2]])];
        put_state(&mut s, mid, stone);
        put_state(&mut s, [mid[0], mid[1] + 1, mid[2]], stone);
        let again = path_line(&run_search_with(&s, q, SearchOptions::ALL_NEW));
        assert_eq!(again, path_line(&run_search_with(&s, q, SearchOptions::OLD)), "search after a block change");
        checked += usize::from(again != path_line(&Some(first)));
        put_state(&mut s, mid, saved[0]);
        put_state(&mut s, [mid[0], mid[1] + 1, mid[2]], saved[1]);
    }
    s.stop_parked();
    assert!(checked > 50, "paths changed by the wall: {checked}");
}

/// A random position draw of the proofs: name, most mob views one call may build when the view is
/// shared (one per ten-try loop it runs), and the call (mob, residence centre, aimed point).
type PositionDraw = (&'static str, u64, fn(&Server, &mut crate::mobs::mob::Mob, [i32; 3], [f64; 3]) -> String);

/// Every random position draw that tests penalties, each through its own ten-try loop.
const POSITION_DRAWS: &[PositionDraw] = &[
    ("villager land", 1, |s, m, _, _| format!("{:?}", s.rpos_land(m, None, 10, 7, &|_, p| f64::from(p[1])))),
    ("villager land at home", 1, |s, m, home, _| {
        format!("{:?}", s.rpos_land(m, Some((home, 12)), 10, 7, &|_, p| f64::from(p[1])))
    }),
    ("villager default", 1, |s, m, home, _| format!("{:?}", s.rpos_default(m, Some((home, 16)), 10, 7))),
    ("villager towards", 1, |s, m, _, to| {
        format!("{:?}", s.rpos_default_towards(m, None, 10, 7, to, std::f64::consts::FRAC_PI_2))
    }),
    ("wild land", 1, |s, m, _, _| {
        format!("{:?}", crate::mobs::wild::rpos::land_pos(s, m, 10, 7, |_, _, p| f64::from(p[1])))
    }),
    ("wild air and water", 1, |s, m, _, _| {
        let pos = crate::mobs::wild::rpos::air_and_water_pos(s, m, 8, 7, 3, 1.0, 0.5, std::f64::consts::FRAC_PI_2);
        format!("{pos:?}")
    }),
    ("goal default", 1, |s, m, _, _| format!("{:?}", s.default_random_pos(m, 10, 7))),
    ("goal land", 1, |s, m, _, _| format!("{:?}", s.land_random_pos(m, 10, 7))),
    ("goal away", 1, |s, m, _, to| format!("{:?}", s.default_random_pos_away(m, 10, 7, to))),
    ("goal towards", 1, |s, m, _, to| format!("{:?}", s.pos_towards(m, 10, 7, to, std::f32::consts::FRAC_PI_2))),
    // Hovering first, then air or water: two loops.
    ("goal fly wander", 2, |s, m, _, _| format!("{:?}", s.fly_wander_pos(m))),
    ("guardian default", 1, |s, m, _, _| format!("{:?}", s.guardian_random_pos(m, 10, 7))),
    ("guardian towards", 1, |s, m, _, to| {
        format!("{:?}", s.guardian_pos_towards(m, 10, 7, to, std::f64::consts::FRAC_PI_2))
    }),
    ("piglin away", 1, |s, m, _, to| format!("{:?}", s.pig_land_pos_away(m, 4.0, 8.0, 7, to))),
    ("allay fly", 1, |s, m, home, _| {
        format!("{:?}", crate::mobs::brainy::allay::fly_target_pos_with(s, m, Some((home, 12)), |_, _| 0.0))
    }),
];

/// Every random position draw, with the draw view shared (`share`) or rebuilt at each test: drawn
/// position and the mob's random source afterwards. With `share`, also checks the mob views built.
fn draws_with(s: &Server, m: &crate::mobs::mob::Mob, share: bool, home_at: [i32; 3]) -> String {
    super::FORCE_SHARED_DRAW_VIEW.set(Some(share));
    let to = [m.pos[0] + 9.0, m.pos[1], m.pos[2] - 5.0];
    let mut m = m.clone();
    let mut out = Vec::new();
    for (name, most_views, draw) in POSITION_DRAWS {
        let views_before = super::mob_views_built();
        out.push(draw(s, &mut m, home_at, to));
        let views = super::mob_views_built() - views_before;
        assert!(!share || views <= *most_views, "draw {name} built {views} mob views");
    }
    out.push(format!("{}", m.rng.next_long()));
    super::FORCE_SHARED_DRAW_VIEW.set(None);
    out.join(" ")
}

/// 1,000 draws of each random position kind (villager, wild animal, goal, guardian, piglin and
/// allay rules) on the village: the view built once per draw gives the positions and the random
/// sequence of the view built at each test, and no draw builds more than one view per loop.
#[test]
fn random_position_draws_match_with_one_view_per_draw() {
    use crate::mobs::mob::Mob;
    let Some(mut s) = village_server("path-draws") else { return };
    let mut r = JRand::new(0xC6_0004);
    let kinds = [Kind::Villager, Kind::Fox, Kind::Bee, Kind::Zombie, Kind::Creaking];
    let mut found = 0;
    for i in 0..1000 {
        let x = VILLAGE[0] + r.next_int_bound(97) - 48;
        let z = VILLAGE[1] + r.next_int_bound(97) - 48;
        let pos = [f64::from(x) + 0.5, f64::from(surface(&s, x, z)), f64::from(z) + 0.5];
        let mut m = Mob::new(kinds[i % kinds.len()], 100 + i as i32, i as u128, pos, 0.0, JRand::new(r.next_long()));
        m.tick_count = r.next_int_bound(5000);
        m.on_ground = true;
        let home_at = [x + r.next_int_bound(9) - 4, pos[1] as i32, z + r.next_int_bound(9) - 4];
        let shared = draws_with(&s, &m, true, home_at);
        assert_eq!(shared, draws_with(&s, &m, false, home_at), "draws of mob {i}");
        found += shared.matches("Some").count();
    }
    s.stop_parked();
    eprintln!("positions found: {found}");
    assert!(found > 8000, "positions found: {found}");
}

/// The movement helpers keep the current path without copying it: same answers and same navigation
/// state as the old sequence (path copied by the path request, then compared and dropped), over
/// repeated requests towards a point and towards an entity, with the speed changing in between.
#[test]
fn kept_path_moves_like_a_copied_path() {
    use crate::mobs::mob::Mob;
    use crate::mobs::mth::block_of;
    let Some(mut s) = village_server("path-keep") else { return };
    let mut r = JRand::new(0xC6_0005);
    let mut kept = 0;
    for i in 0..300 {
        let x = VILLAGE[0] + r.next_int_bound(97) - 48;
        let z = VILLAGE[1] + r.next_int_bound(97) - 48;
        let pos = [f64::from(x) + 0.5, f64::from(surface(&s, x, z)), f64::from(z) + 0.5];
        let kind = if i % 3 == 0 { Kind::Spider } else { Kind::Villager };
        let mut a = Mob::new(kind, 200 + i, i as u128, pos, 0.0, JRand::new(r.next_long()));
        a.on_ground = true;
        let target =
            [pos[0] + f64::from(r.next_int_bound(25) - 12), pos[1], pos[2] + f64::from(r.next_int_bound(25) - 12)];
        let mut b = a.clone();
        for step in 0..4 {
            let speed = 0.5 + f64::from(step) * 0.1;
            let before = a.nav.path.as_ref().map(|p| p.nodes.clone());
            let (got, want) = if step % 2 == 0 {
                let path = s.create_path(&mut b, block_of(target), 1);
                (s.move_to_pos(&mut a, target, speed), s.move_to_path(&mut b, path, speed))
            } else {
                let reach = if b.nav.climber { 0 } else { 1 };
                let path = s.create_path(&mut b, block_of(target), reach);
                let want = if path.is_none() && b.nav.climber {
                    b.nav.path_to = Some(block_of(target));
                    b.nav.speed_mod = speed;
                    true
                } else {
                    path.is_some_and(|p| s.move_to_path(&mut b, Some(p), speed))
                };
                (s.move_to_entity(&mut a, target, speed), want)
            };
            assert_eq!(got, want, "mob {i}, request {step}");
            assert_eq!(format!("{:?}", a.nav), format!("{:?}", b.nav), "navigation of mob {i}, request {step}");
            kept += usize::from(before.is_some() && before == a.nav.path.as_ref().map(|p| p.nodes.clone()));
            s.nav_tick(&mut a);
            s.nav_tick(&mut b);
        }
    }
    s.stop_parked();
    assert!(kept > 300, "requests that kept the current path: {kept}");
}

#[test]
fn type_set_walks_its_types_in_sorted_order() {
    for (i, t) in PathType::ALL.iter().enumerate() {
        assert_eq!(*t as usize, i, "declaration order of {t:?}");
    }
    let mut r = JRand::new(7);
    for _ in 0..100_000 {
        let mut set = TypeSet::default();
        let mut list = Vec::new();
        for _ in 0..r.next_int_bound(9) {
            let t = PathType::ALL[r.next_int_bound(27) as usize];
            set.insert(t);
            if !list.contains(&t) {
                list.push(t);
            }
        }
        list.sort();
        let (types, n) = set.in_order();
        assert_eq!(&types[..n], &list[..]);
    }
}

#[test]
fn relative_keys_are_distinct_within_a_search_reach() {
    // Near the world border, offsets far beyond any search (follow range at most 2,048) and the
    // largest the 21-bit fields hold.
    let origin = [-29_999_000, 300, 29_999_000];
    let mut r = JRand::new(11);
    let far = (1 << 20) - 1;
    let mut offsets: Vec<[i32; 3]> = (0..200_000)
        .map(|_| [r.next_int_bound(8193) - 4096, r.next_int_bound(8193) - 4096, r.next_int_bound(8193) - 4096])
        .collect();
    offsets.extend([[far, far, far], [-far, -far, -far], [far, -far, 0], [0, 0, 0], [-1, -1, -1]]);
    let mut by_key = std::collections::HashMap::new();
    for d in offsets {
        let key = super::relative_key(origin, origin[0] + d[0], origin[1] + d[1], origin[2] + d[2]);
        let prev = by_key.insert(key, d);
        assert!(prev.is_none_or(|p| p == d), "cells {prev:?} and {d:?} share a key");
    }
}
