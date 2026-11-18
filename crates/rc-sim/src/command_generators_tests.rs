//! Proofs of the generators shared by the commands: one per dimension and structure
//! setting, the generation thread's one used by the commands, the spawn search's one handed to the
//! server of a world just created; same searches, same placements, same files as private ones.

use std::sync::Arc;

use rc_proto::nbt::Nbt;
use rc_worldgen::chunkgen::DimGen;

use crate::server::Server;

use super::force;

const SEED: i64 = 20_261_103;
const OVERWORLD: &str = "minecraft:overworld";
const NETHER: &str = "minecraft:the_nether";

/// Every switch of the generators shared by the commands.
const NEW_PATHS: [&str; 6] = [
    "COMMAND_WORLD_DATA_ONLY",
    "ONE_COMMAND_GENERATOR",
    "SHARE_WORKER_GENERATOR",
    "KEEP_NEW_WORLD_DATA",
    "REUSE_SPAWN_GENERATOR",
    "EARLY_GENERATOR_START",
];

/// World created with these structure setting and seed, then opened (`None`: no game data); every
/// switch of this file on for the calling thread (the tests turn some off afterwards), whatever
/// the environment says.
fn world(tag: &str, seed: i64, structures: bool) -> Option<(Server, std::path::PathBuf)> {
    force(&NEW_PATHS, Some(true));
    let data = Arc::new(rc_testdata::version_data()?);
    crate::registries::client_jar(&data)?;
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-command-generators-{tag}-{}", std::process::id())));
    let mut w = crate::newworld::NewWorld::new(tag);
    w.seed = seed;
    w.generate_structures = structures;
    crate::newworld::create_world(&dir, &data, &w).expect("world creation");
    Some((crate::portal_tests::open_world(&dir, data), dir))
}

fn thread_generator(s: &Server, structures: bool) -> Option<Arc<DimGen>> {
    s.generator.as_ref()?.thread_generator(&s.dim.name, structures)
}

/// World with structures: the searches, `/place` and `/place structure` use one generator, the
/// generation thread's; another dimension has its own; coming back finds the first one again
/// (nothing rebuilt).
#[test]
fn one_generator_per_dimension_with_structures() {
    let Some((mut s, _dir)) = world("one-gen", SEED, true) else { return };
    let search = s.locate_gen().expect("search generator");
    let place = s.place_data().expect("place generator").1;
    let structured = s.structured_gen().expect("structure generator").1;
    assert!(Arc::ptr_eq(&search, &place) && Arc::ptr_eq(&search, &structured), "one generator");
    assert!(Arc::ptr_eq(&search, &thread_generator(&s, true).expect("thread generator")), "the thread's");
    assert!(search.structures.is_some());
    s.enter_world(NETHER);
    let nether = s.locate_gen().expect("Nether generator");
    assert_eq!(nether.dimension, NETHER);
    s.enter_world(OVERWORLD);
    assert!(Arc::ptr_eq(&s.locate_gen().unwrap(), &search), "kept across a dimension trip");
    assert!(s.command_gens.built.is_empty(), "nothing built by the server: {}", s.command_gens.built.len());
    // Without the thread's: one private generator per dimension, built once.
    force(&["SHARE_WORKER_GENERATOR"], Some(false));
    let private = s.locate_gen().expect("private generator");
    assert!(!Arc::ptr_eq(&private, &search));
    assert!(Arc::ptr_eq(&private, &s.place_data().unwrap().1) && Arc::ptr_eq(&private, &s.structured_gen().unwrap().1));
    assert_eq!(s.command_gens.built.len(), 1);
    force(&["SHARE_WORKER_GENERATOR"], None);
}

/// World without structures: searches and `/place` share the thread's generator, `/place
/// structure` gets its own with the structures enabled.
#[test]
fn one_generator_per_setting_without_structures() {
    let Some((mut s, _dir)) = world("no-struct", SEED, false) else { return };
    let search = s.locate_gen().expect("search generator");
    let place = s.place_data().expect("place generator").1;
    assert!(Arc::ptr_eq(&search, &place));
    assert!(Arc::ptr_eq(&search, &thread_generator(&s, false).expect("thread generator")));
    assert!(search.structures.is_none(), "searches without structures");
    let structured = s.structured_gen().expect("structure generator").1;
    assert!(structured.structures.is_some() && !Arc::ptr_eq(&structured, &search));
    assert!(Arc::ptr_eq(&structured, &s.structured_gen().unwrap().1), "built once");
}

/// The template blocks read the generation data without building a generator.
#[test]
fn template_data_builds_no_generator() {
    let Some((mut s, _dir)) = world("tpl-data", SEED, true) else { return };
    force(&["SHARE_WORKER_GENERATOR"], Some(false));
    let wd = s.template_data().expect("generation data");
    assert!(s.command_gens.built.is_empty(), "no generator for the template data");
    assert!(Arc::ptr_eq(&wd, &s.world_data().unwrap()));
    force(&["SHARE_WORKER_GENERATOR"], None);
}

/// Every structure tag searched from several origins: (origin, result).
type Searches = Vec<([i32; 3], Option<([i32; 3], String)>)>;

fn searches(s: &mut Server) -> Searches {
    // Villages, strongholds (rings), ocean monuments, buried treasures, jungle temples.
    let tags = [
        "#minecraft:village",
        "#minecraft:eye_of_ender_located",
        "minecraft:monument",
        "#minecraft:on_treasure_maps",
        "minecraft:jungle_pyramid",
    ];
    let origins = [[0, 64, 0], [1500, 64, -700], [-2600, 64, 1900], [400, 64, 4100]];
    let mut out = Vec::new();
    for tag in tags {
        for origin in origins {
            let wanted = s.structure_tag_members(tag);
            assert!(!wanted.is_empty(), "{tag}");
            out.push((origin, s.structure_search(&wanted, origin, 100)));
        }
    }
    out
}

/// Searches with the generation thread's generator = with a private one, the thread idle and the
/// thread busy with a batch during the searches.
#[test]
fn shared_generator_matches_private_searches() {
    let Some((mut s, _dir)) = world("search", SEED, true) else { return };
    let shared = searches(&mut s);
    assert_eq!(shared.len(), 20);
    assert!(shared.iter().filter(|(_, r)| r.is_some()).count() >= 15, "searches found too little: {shared:?}");
    if let Some(g) = &mut s.generator {
        g.want((20..26).flat_map(|x| (-3..3).map(move |z| (x, z))).collect());
    }
    let busy = searches(&mut s);
    force(&["SHARE_WORKER_GENERATOR"], Some(false));
    let private = searches(&mut s);
    force(&["SHARE_WORKER_GENERATOR"], None);
    assert_eq!(shared, private, "shared and private generators");
    assert_eq!(busy, private, "searches during a generation batch");
}

/// `/place structure` and the jigsaw block take their pieces from the generator: the generation
/// thread's one, busy with a batch, assembles the same starts as a private one. (Two live worlds
/// cannot be compared block for block: the world randomness of a test follows its thread.)
#[test]
fn shared_generator_assembles_the_same_structure_starts() {
    let Some((mut s, _dir)) = world("place", SEED, true) else { return };
    let shared = s.structured_gen().expect("structure generator").1;
    assert!(Arc::ptr_eq(&shared, &thread_generator(&s, true).unwrap()));
    let private = s.build_command_generator(true).expect("private generator");
    if let Some(g) = &mut s.generator {
        g.want((-30..-24).flat_map(|x| (-3..3).map(move |z| (x, z))).collect());
    }
    let ids = [
        "minecraft:igloo",
        "minecraft:pillager_outpost",
        "minecraft:village_plains",
        "minecraft:village_desert",
        "minecraft:desert_pyramid",
        "minecraft:swamp_hut",
        "minecraft:trail_ruins",
        "minecraft:ancient_city",
        "minecraft:shipwreck",
        "minecraft:ruined_portal",
    ];
    let mut placed = 0;
    for id in ids {
        for (cx, cz) in [(0, 0), (5, -3), (-40, 17), (120, 64)] {
            let a = shared.structure_start_anywhere(id, cx, cz);
            let b = private.structure_start_anywhere(id, cx, cz);
            placed += usize::from(a.is_some());
            assert_eq!(format!("{a:?}"), format!("{b:?}"), "{id} at ({cx}, {cz})");
        }
    }
    assert!(placed >= 30, "too few starts compared: {placed}");
}

/// Every `HPos` field of a compound, in order.
fn heights(n: &Nbt, out: &mut Vec<i64>) {
    match n {
        Nbt::Compound(f) => {
            for (k, v) in f {
                if k == "HPos" {
                    out.extend(v.as_i64());
                } else {
                    heights(v, out);
                }
            }
        }
        Nbt::List(l) => l.iter().for_each(|v| heights(v, out)),
        _ => {}
    }
}

/// The one difference of the shared generator, the game's behavior: a scattered structure whose
/// start chunk was decorated in memory (proto-chunk not written, `RUSTCRAFT_OPT_PROTO_LAZY`) is
/// written by an exploration map's reference with the height its placement measured, where a
/// private generator writes −1 (not measured). The game holds one structure start object per chunk,
/// read by the map search and modified by the placement.
#[test]
fn shared_generator_writes_the_measured_height_of_a_scattered_piece() {
    let Some((mut s, _dir)) = world("hpos", SEED, true) else { return };
    // Pieces that keep their measured height (not the igloo: template pieces).
    let wanted: Vec<String> =
        ["minecraft:swamp_hut", "minecraft:desert_pyramid", "minecraft:jungle_pyramid"].map(str::to_owned).to_vec();
    let (pos, id) = s.structure_search(&wanted, [0, 64, 0], 100).expect("scattered structure");
    let start = (pos[0] >> 4, pos[2] >> 4);
    // The start chunk decorated as the neighbour of a full chunk, not written.
    s.load_now(&[(start.0 + 1, start.1 + 1)]);
    let shared = s.locate_gen().expect("shared generator");
    let private = s.build_command_generator(true).expect("private generator");
    let (mut a, mut b) = (Vec::new(), Vec::new());
    heights(&shared.structures_nbt(start.0, start.1), &mut a);
    heights(&private.structures_nbt(start.0, start.1), &mut b);
    eprintln!("{id} at {start:?}: shared {a:?}, private {b:?}");
    assert!(!a.is_empty() && a.len() == b.len(), "{id}: {a:?} {b:?}");
    assert!(a.iter().all(|h| *h >= 0), "shared: height measured by the placement {a:?}");
    assert!(b.iter().all(|h| *h == -1), "private: height not measured {b:?}");
}

/// Time the server thread spends on its first `/locate`, `/place feature` and `/place structure`
/// generators (the tick they freeze), old path against the shared ones, in a world with
/// structures; the generation thread's generator awaited first (steady state). Run once as is and
/// once with `RUSTCRAFT_OPT_SHARED_CLIMATE_LISTS=0 RUSTCRAFT_OPT_SHARED_GENERATOR_DEFS=0
/// RUSTCRAFT_OPT_SHARED_PROC_LISTS=0` (generator setup of before):
/// `cargo test --release -p rc-sim --lib bench_first_command_generators -- --ignored --nocapture`.
#[test]
#[ignore = "benchmark"]
fn bench_first_command_generators() {
    let ms = |t: std::time::Instant| t.elapsed().as_secs_f64() * 1e3;
    for (label, one, share) in [("old path", false, false), ("one private", true, false), ("thread's", true, true)] {
        let mut times = Vec::new();
        for round in 0..3 {
            let Some((mut s, _dir)) = world(&format!("bench-{one}-{share}-{round}"), SEED + round, true) else { return };
            let _ = thread_generator(&s, true);
            force(&["ONE_COMMAND_GENERATOR"], Some(one));
            force(&["SHARE_WORKER_GENERATOR"], Some(share));
            let t = std::time::Instant::now();
            assert!(s.locate_gen().is_some());
            let locate = ms(t);
            let t = std::time::Instant::now();
            assert!(s.place_data().is_some());
            let place = ms(t);
            let t = std::time::Instant::now();
            assert!(s.structured_gen().is_some());
            let structured = ms(t);
            force(&["ONE_COMMAND_GENERATOR", "SHARE_WORKER_GENERATOR"], None);
            times.push((locate, place, structured));
        }
        times.sort_by(|a, b| (a.0 + a.1 + a.2).total_cmp(&(b.0 + b.1 + b.2)));
        let (l, p, st) = times[1];
        eprintln!("{label:<12} first /locate {l:>7.1} ms, /place feature {p:>7.1} ms, /place structure {st:>7.1} ms (median of 3)");
    }
}

/// World creation, then opening until the main world generator is ready, with the hand-over
/// switches off and on, the generation thread started after or before the registries (median of 3):
/// `cargo test --release -p rc-sim --lib bench_new_world_opening -- --ignored --nocapture`.
#[test]
#[ignore = "benchmark"]
fn bench_new_world_opening() {
    let Some(data) = rc_testdata::version_data().map(Arc::new) else { return };
    let ms = |t: std::time::Instant| t.elapsed().as_secs_f64() * 1e3;
    let cases = [
        ("nothing kept, late start", false, false, false),
        ("nothing kept, early start", false, false, true),
        ("data kept", true, false, true),
        ("data and generator kept", true, true, true),
    ];
    for (label, keep, reuse, early) in cases {
        let mut times = Vec::new();
        for round in 0..3 {
            force(&["KEEP_NEW_WORLD_DATA"], Some(keep));
            force(&["REUSE_SPAWN_GENERATOR"], Some(reuse));
            force(&["EARLY_GENERATOR_START"], Some(early));
            let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-new-world-opening-{keep}{reuse}{early}{round}")));
            let mut w = crate::newworld::NewWorld::new("bench");
            w.seed = SEED + i64::from(round);
            let t = std::time::Instant::now();
            crate::newworld::create_world(&dir, &data, &w).expect("world creation");
            let created = ms(t);
            let t = std::time::Instant::now();
            let s = crate::portal_tests::open_world(&dir, data.clone());
            let opened = ms(t);
            assert!(thread_generator(&s, true).is_some());
            let ready = ms(t);
            force(&["KEEP_NEW_WORLD_DATA", "REUSE_SPAWN_GENERATOR", "EARLY_GENERATOR_START"], None);
            times.push((created, opened, ready));
        }
        times.sort_by(|a, b| (a.0 + a.2).total_cmp(&(b.0 + b.2)));
        let (c, o, r) = times[1];
        eprintln!("{label:<28} creation {c:>7.1} ms, opening {o:>6.1} ms, generator ready {r:>6.1} ms after opening began");
    }
}

/// A world just created leaves its spawn search's generator to its server, which takes it (and
/// releases it); switch off: only the data are kept and the server builds its own.
#[test]
fn new_world_hands_its_generator_to_the_server() {
    let Some(data) = rc_testdata::version_data().map(Arc::new) else { return };
    if crate::registries::client_jar(&data).is_none() {
        return;
    }
    for reuse in [true, false] {
        force(&NEW_PATHS, Some(true));
        force(&["REUSE_SPAWN_GENERATOR"], Some(reuse));
        let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-spawn-generator-handover-{reuse}-{}", std::process::id())));
        let mut w = crate::newworld::NewWorld::new("hand");
        w.seed = SEED;
        crate::newworld::create_world(&dir, &data, &w).expect("world creation");
        let kept = super::kept_new_world_generator(&dir);
        assert_eq!(super::new_world_kept(&dir), Some(reuse), "kept for the server");
        let s = crate::portal_tests::open_world(&dir, data.clone());
        assert_eq!(super::new_world_kept(&dir), None, "taken by the server");
        let used = thread_generator(&s, true).expect("thread generator");
        assert_eq!(kept.is_some_and(|k| Arc::ptr_eq(&k, &used)), reuse, "spawn generator reused: {reuse}");
        force(&["REUSE_SPAWN_GENERATOR"], None);
    }
}
