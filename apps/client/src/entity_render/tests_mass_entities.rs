//! Entities in bulk (primed TNT, ground items): equivalence proofs of their fast paths and
//! micro-benches of `prepare` on scenes shaped like the `tnt-heap` and `dropped-items` bench
//! scenes. 26.3 data and game and a hardware GPU are needed (test skipped otherwise, failure
//! under `RUSTCRAFT_OPT_REQUIRE=1`).

use super::tests_c3m11::{digest, prep};
use super::tests_g2f03::{Kit, kit};
use super::*;
use rc_proto_v777::entity::Meta;
use rc_proto_v777::item::ItemStack;

/// The eight items of the `dropped-items` bench scene.
const GROUND_ITEMS: [&str; 8] = [
    "minecraft:diamond",
    "minecraft:apple",
    "minecraft:iron_sword",
    "minecraft:oak_log",
    "minecraft:redstone",
    "minecraft:compass",
    "minecraft:golden_apple",
    "minecraft:stone",
];

/// A new entity of registry kind `kind` (real type id), network id `id`, at `pos`.
fn spawn(k: &Kit, kind: &str, id: i32, pos: [f64; 3]) -> Entity {
    let names = k.r.source.registry("minecraft:entity_type").expect("type registry").entries.clone();
    let type_id = names
        .iter()
        .position(|n| n.strip_prefix("minecraft:") == Some(kind))
        .unwrap_or_else(|| panic!("kind {kind} missing from the 26.3 registry"));
    let mut all = crate::entities::Entities { type_names: names, ..Default::default() };
    let add = rc_proto_v777::entity::AddEntity {
        id,
        uuid: 0x7000_0000 + id as u128 * 7919,
        kind: type_id as i32,
        pos,
        velocity: [0.0; 3],
        pitch: 0.0,
        yaw: (id as f32 * 37.0) % 360.0,
        head_yaw: 0.0,
        data: 0,
    };
    all.apply(rc_client::EntityEvent::Add(Box::new(add)));
    all.map.remove(&id).expect("entity added")
}

/// `count` primed TNT in a heap of 12 × 12 columns one block apart, in front of the camera of
/// `prep`: fuses spread over 0 to 79 ticks (some swell, half blink), every third one moving.
pub(super) fn tnt_heap(k: &Kit, count: usize, first_id: i32) -> Vec<Entity> {
    (0..count)
        .map(|i| {
            let (x, y, z) = ((i % 12) as f64 - 6.0, (i / 144) as f64, ((i / 12) % 12) as f64 + 4.0);
            let mut e = spawn(k, "tnt", first_id + i as i32, [x + 0.5, 64.0 + y, z + 0.5]);
            e.meta.insert(8, Meta::Int((i * 7 % 80) as i32));
            e.age = 40 + (i % 50) as u32;
            if i % 3 == 0 {
                e.pos_o = [e.pos[0] - 0.12, e.pos[1] - 0.3, e.pos[2] + 0.05];
            }
            e
        })
        .collect()
}

/// `count` ground items of the eight kinds of `dropped-items` on a 25-wide grid 1.5 blocks apart;
/// every fifth stack holds 20 or 40 (several copies drawn), one in seven moves.
pub(super) fn ground_items(k: &Kit, count: usize, first_id: i32) -> Vec<Entity> {
    (0..count)
        .map(|i| {
            let (x, z) = ((i % 25) as f64 * 1.5 - 18.0, (i / 25) as f64 * 1.5 + 3.0);
            let mut e = spawn(k, "item", first_id + i as i32, [x, 64.0, z]);
            let n = match i % 5 {
                0 => 20,
                1 => 40,
                _ => 1,
            };
            let item = k.item(GROUND_ITEMS[i % GROUND_ITEMS.len()]);
            e.meta.insert(8, Meta::Item(Some(ItemStack { item, count: n, added: vec![], removed: vec![] })));
            e.age = 10 + (i * 13 % 400) as u32;
            if i % 7 == 0 {
                e.pos_o = [e.pos[0] + 0.02, e.pos[1] + 0.1, e.pos[2]];
            }
            e
        })
        .collect()
}

/// Median of the samples, in place.
fn median(v: &mut [f64]) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

/// A switch of this file: its name as in its environment variable, and its field.
type Switch = (&'static str, fn(&mut EntOpt) -> &mut bool);

/// The switches of this file.
fn bulk_switches() -> [Switch; 8] {
    [
        ("DISTANCE_SORT_KEYS", |o| &mut o.distance_sort_keys),
        ("PARALLEL_ENTITY_LIST", |o| &mut o.parallel_entity_list),
        ("PRIMED_TNT_SHARED_POSE", |o| &mut o.primed_tnt_shared_pose),
        ("PRIMED_TNT_BY_KIND_FLAG", |o| &mut o.primed_tnt_by_kind_flag),
        ("STILL_TNT_CACHE", |o| &mut o.still_tnt_cache),
        ("GROUND_ITEM_LAYOUT_CACHE", |o| &mut o.ground_item_layout_cache),
        ("ITEM_GROUPS_IN_PLACE", |o| &mut o.item_groups_in_place),
        ("INSTANCE_BUCKETS", |o| &mut o.instance_buckets),
    ]
}

/// Every switch of this file on (`EntOpt::NEW`) or off ([`bulk_off`]), switch `which` flipped.
fn with_switch(base_on: bool, which: usize) -> EntOpt {
    let mut o = if base_on { EntOpt::NEW } else { bulk_off() };
    let flag = (bulk_switches()[which].1)(&mut o);
    *flag = !*flag;
    o
}

/// The mass scene: a TNT heap (a third moving, some swelling), ground items of the eight kinds
/// (copies, a compass that reads its surroundings) and, for the parallel list, enough of them
/// to cross [`PARALLEL_LIST_MIN_ENTITIES`] (the rows beyond the first 500 lie past their render
/// distance: listed, never drawn).
fn mass_scene(k: &Kit) -> Vec<Entity> {
    let mut ents = tnt_heap(k, 1728, 1);
    ents.extend(ground_items(k, 2400, 10_000));
    assert!(ents.len() >= PARALLEL_LIST_MIN_ENTITIES, "the scene does not reach the parallel list");
    ents
}

/// Fingerprint of two consecutive frames of `ents` (the second one replays what is kept) and
/// of the batches built from the last one.
fn two_frames(k: &mut Kit, ents: &[Entity], opt: EntOpt) -> (u64, u64, u64) {
    // Each setting builds what it keeps itself (the first frame), then reuses it (the second).
    k.r.forget_primed_tnt();
    k.r.forget_ground_layouts();
    prep(k, ents, opt);
    let first = digest(&k.r);
    prep(k, ents, opt);
    let second = digest(&k.r);
    let flat = k.r.build_batches();
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut eat = |bytes: &[u8]| {
        for &b in bytes {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x100_0000_01b3);
        }
    };
    eat(bytemuck::cast_slice(&flat));
    for (pass, src, verts, first, count) in &k.r.batches {
        eat(&(*pass as u32).to_le_bytes());
        eat(&[*src]);
        for v in [verts.start, verts.end, *first, *count] {
            eat(&v.to_le_bytes());
        }
    }
    (first, second, h)
}

/// Expected fingerprints of the mass scene (26.3 data and game) with the switches of this file
/// off and every other one on: any change to what `prepare` places or to the batch order makes
/// the test fail.
const GOLDEN_MASS: (u64, u64, u64) = (0xe34e_bcd5_a0c8_6b96, 0xe34e_bcd5_a0c8_6b96, 0x67a0_e1d3_d106_2756);

/// Every switch of this file, alone on and alone off, places exactly what its old path places
/// (matrices compared up to the sign of zeros, as everywhere in these proofs), over a first
/// frame and a second one that reuses what was kept, and builds the same batches. The other
/// switches stay on (the sine table changes the bobbing of the items: a fidelity switch with its
/// own proof).
#[test]
fn bulk_entities_same_under_every_switch() {
    let Some(mut k) = kit() else { return };
    let ents = mass_scene(&k);
    // Caches (layers, meshes, atlas) filled, then the reference.
    two_frames(&mut k, &ents, EntOpt::NEW);
    let reference = two_frames(&mut k, &ents, bulk_off());
    // The far ground items are beyond their render distance (16 blocks at this view scale).
    assert!(k.r.drawn > 1800, "scene almost empty: {} entities drawn", k.r.drawn);
    let mut differ = Vec::new();
    if two_frames(&mut k, &ents, EntOpt::NEW) != reference {
        differ.push("every switch on".to_owned());
    }
    for (i, (name, _)) in bulk_switches().iter().enumerate() {
        if two_frames(&mut k, &ents, with_switch(true, i)) != reference {
            differ.push(format!("{name}=0, the others on"));
        }
        if two_frames(&mut k, &ents, with_switch(false, i)) != reference {
            differ.push(format!("{name} alone"));
        }
    }
    assert!(differ.is_empty(), "not the old placement: {differ:?}");
    // The fast paths were taken: shared pose built, TNT placements kept, layouts kept (the
    // compass reads its surroundings, the others are drawn from their layout).
    two_frames(&mut k, &ents, EntOpt::NEW);
    let (shared_pose, kept_tnt) = k.r.primed_tnt_counts();
    assert!(shared_pose, "no shared TNT pose");
    assert!(kept_tnt > 1000, "TNT placements kept: {kept_tnt}");
    let (drawn, surroundings) = k.r.ground_layouts_counts();
    assert!(drawn >= 7 && surroundings >= 1, "ground layouts: {drawn} kept, {surroundings} reading their surroundings");
    assert_eq!(reference, GOLDEN_MASS, "mass scene fingerprint changed: {reference:#x?}");
}

/// `EntOpt::NEW` with every switch of this file off: the reference of the benches (the other
/// switches are not what is measured here).
fn bulk_off() -> EntOpt {
    let mut o = EntOpt::NEW;
    for (_, flag) in bulk_switches() {
        *flag(&mut o) = false;
    }
    o
}

/// `prepare` then the batches of the mass scenes, 1 core (the parallel list off unless named),
/// each configuration in turn, median of the rounds in µs.
/// `cargo test --release -p rc-app -- --ignored bench_mass_entities --nocapture`.
#[test]
#[ignore]
fn bench_mass_entities() {
    let Some(mut k) = kit() else { return };
    let only = |which: &[&str]| {
        let mut o = bulk_off();
        for (name, flag) in bulk_switches() {
            if which.contains(&name) {
                *flag(&mut o) = true;
            }
        }
        o
    };
    let mut configs: Vec<(String, EntOpt)> = vec![("old".into(), bulk_off())];
    let serial_all = EntOpt { parallel_entity_list: false, ..EntOpt::NEW };
    configs.push(("all (1 core)".into(), serial_all));
    configs.push(("all but STILL_TNT_CACHE".into(), EntOpt { still_tnt_cache: false, ..serial_all }));
    for (name, _) in bulk_switches().into_iter().filter(|(n, _)| *n != "PARALLEL_ENTITY_LIST") {
        configs.push((format!("{name} alone"), only(&[name])));
    }
    configs.push(("SHARED_POSE + STILL_TNT_CACHE".into(), only(&["PRIMED_TNT_SHARED_POSE", "STILL_TNT_CACHE"])));
    configs.push(("SHARED_POSE + BY_KIND_FLAG".into(), only(&["PRIMED_TNT_SHARED_POSE", "PRIMED_TNT_BY_KIND_FLAG"])));
    configs.push(("LAYOUT_CACHE + GROUPS_IN_PLACE".into(), only(&["GROUND_ITEM_LAYOUT_CACHE", "ITEM_GROUPS_IN_PLACE"])));
    let scenes = [
        ("1,728 TNT", tnt_heap(&k, 1728, 1)),
        ("500 ground items", ground_items(&k, 500, 1)),
        ("TNT + items", mass_scene(&k)),
    ];
    for (label, ents) in &scenes {
        let rounds = 61;
        let mut times = vec![Vec::with_capacity(rounds); configs.len()];
        for round in 0..rounds + 3 {
            for (i, (_, o)) in configs.iter().enumerate() {
                let clock = std::time::Instant::now();
                prep(&mut k, ents, *o);
                std::hint::black_box(k.r.build_batches());
                if round >= 3 {
                    times[i].push(clock.elapsed().as_secs_f64() * 1e6);
                }
            }
        }
        let old = median(&mut times[0]);
        println!("{label} ({} drawn), prepare + batches:", k.r.drawn);
        for (i, (name, _)) in configs.iter().enumerate() {
            let m = median(&mut times[i]);
            println!("  {name:>32}: {m:>8.1} µs ({:+.1}%)", (m / old - 1.0) * 100.0);
        }
    }
}

/// The frame list alone (positions, sizes, kinds, distances, sort) on 1 thread and on pools of
/// 2 to all logical cores, by entity count: where spreading it starts to pay
/// ([`PARALLEL_LIST_MIN_ENTITIES`]). Median µs.
/// `cargo test --release -p rc-app -- --ignored bench_parallel_entity_list --nocapture`.
#[test]
#[ignore]
fn bench_parallel_entity_list() {
    let Some(mut k) = kit() else { return };
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    let mut threads = vec![1usize];
    while threads.last().is_some_and(|&t| t * 2 <= cores) {
        threads.push(threads.last().unwrap() * 2);
    }
    if threads.last() != Some(&cores) {
        threads.push(cores);
    }
    for n in [500usize, 1000, 2000, 4000, 8000, 16000] {
        let mut ents = tnt_heap(&k, n.min(1728), 1);
        ents.extend(ground_items(&k, n.saturating_sub(1728), 100_000));
        let serial = EntOpt { parallel_entity_list: false, ..EntOpt::NEW };
        let parallel = EntOpt { parallel_entity_list: true, ..EntOpt::NEW };
        let mut line = format!("{n:>6} entities: 1 thread (serial path)");
        let mut time = |o: EntOpt, pool: Option<&rayon::ThreadPool>| {
            k.r.opt = o;
            let mut v = Vec::new();
            for round in 0..83 {
                let clock = std::time::Instant::now();
                // The parallel path is taken from any count to find the threshold.
                let mut run = || std::hint::black_box(k.r.frame_list_from(ents.iter(), 0.37, [0.5, 66.0, -2.5], 0).len());
                match pool {
                    Some(p) => p.install(run),
                    None => run(),
                };
                if round >= 3 {
                    v.push(clock.elapsed().as_secs_f64() * 1e6);
                }
            }
            median(&mut v)
        };
        line += &format!(" {:.1}", time(serial, None));
        for &t in &threads {
            let pool = rayon::ThreadPoolBuilder::new().num_threads(t).build().expect("thread pool");
            line += &format!(", {t} threads {:.1}", time(parallel, Some(&pool)));
        }
        println!("{line} µs");
    }
}

/// Where `prepare` spends its time on the mass scenes: the list (positions, sizes, kinds,
/// distances, sort), the shadows, the object path of TNT and the ground items, each timed alone
/// over the same entities (1 core, median of 101 rounds, µs).
/// `cargo test --release -p rc-app -- --ignored bench_mass_entities_breakdown --nocapture`.
#[test]
#[ignore]
fn bench_mass_entities_breakdown() {
    let Some(mut k) = kit() else { return };
    let state = |x: i32, y: i32, z: i32| if y < 64 && (x + z) & 3 == 0 { 1u32 } else { 0 };
    let bright = |x: i32, y: i32, z: i32| (x * 3 + y + z * 5).rem_euclid(16);
    let tints = |_: i32, _: i32, _: i32| [0xff_ffff; 4];
    let world = ShadowWorld { state: &state, brightness: &bright, ambient: 0.1, biome_tints: &tints };
    let light =
        |x: i32, y: i32, z: i32| ((x * 7 + y * 3 + z).rem_euclid(16) as u8, (x + y * 5 + z * 3).rem_euclid(16) as u8);
    let (pt, cam, origin) = (0.37f32, [0.5, 66.0, -2.5], [0, 0]);
    for (label, ents) in [("1,728 TNT", tnt_heap(&k, 1728, 1)), ("500 ground items", ground_items(&k, 500, 1))] {
        let rounds = 101;
        let mut t = [(); 6].map(|()| Vec::with_capacity(rounds));
        for round in 0..rounds + 3 {
            let clock = std::time::Instant::now();
            prep(&mut k, &ents, EntOpt::NEW);
            let whole = clock.elapsed().as_secs_f64() * 1e6;
            let r = &mut k.r;
            let clock = std::time::Instant::now();
            let list = r.frame_list(ents.iter(), pt, cam);
            let listing = clock.elapsed().as_secs_f64() * 1e6;
            r.shadow_verts.clear();
            let clock = std::time::Instant::now();
            for &(e, ..) in &list {
                r.push_shadow(e, pt, origin, cam, &world);
            }
            let shadows = clock.elapsed().as_secs_f64() * 1e6;
            r.mats.clear();
            for v in &mut r.instances {
                v.clear();
            }
            let clock = std::time::Instant::now();
            for &(e, _, sizes, _) in &list {
                r.set_current(Some(e), sizes);
                let p = e.render_pos(pt);
                let rel = [p[0] as f32, p[1] as f32, p[2] as f32];
                if e.kind == "item" {
                    r.push_item(e, &k.items, pt, rel, p, &light);
                } else {
                    r.push_object(e, pt, rel, p, cam, &light, Some(&k.items), &world);
                }
            }
            r.set_current(None, None);
            let objects = clock.elapsed().as_secs_f64() * 1e6;
            let clock = std::time::Instant::now();
            for &(e, ..) in &list {
                let p = e.render_pos(pt);
                std::hint::black_box(r.eye_light(e, p, &light));
            }
            let probes = clock.elapsed().as_secs_f64() * 1e6;
            // The TNT path alone, light already probed (nothing kept: a placement every time).
            r.opt.still_tnt_cache = false;
            r.mats.clear();
            for v in &mut r.instances {
                v.clear();
            }
            let clock = std::time::Instant::now();
            for &(e, ..) in list.iter().filter(|r| r.0.kind == "tnt") {
                let p = e.render_pos(pt);
                r.push_primed_tnt(e, pt, [p[0] as f32, p[1] as f32, p[2] as f32], 0xf0f0);
            }
            let tnt_alone = clock.elapsed().as_secs_f64() * 1e6;
            if round >= 3 {
                for (v, x) in t.iter_mut().zip([whole, listing, shadows, objects, probes, tnt_alone]) {
                    v.push(x);
                }
            }
        }
        let [whole, listing, shadows, objects, probes, tnt_alone] = t.map(|mut v| median(&mut v));
        println!(
            "{label}: prepare {whole:.1} µs; list and sort {listing:.1}, shadows {shadows:.1}, \
             object or item path {objects:.1} (light probes alone {probes:.1}, TNT placement alone {tnt_alone:.1})"
        );
    }
}
