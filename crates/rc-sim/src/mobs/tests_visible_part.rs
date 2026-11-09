//! Equivalence proofs of the visible-part paths (`visible_part.rs`) and of the explosion's
//! object pass: visible parts of a few hundred boxes of every size around walls of every shape,
//! twenty neighbouring TNT among items, orbs, falling blocks and mobs, and the 1,728-TNT heap.
//! Every switch combination must give, to the bit, what the former paths give in the same
//! process; the visible parts are also frozen (no platform-dependent maths), the explosion scenes
//! on Windows only (the TNT are primed through the C library's sin/cos, whose last bits differ on
//! Linux). Each scene run gets a thread of its own, so the test random seeds start afresh.

use rc_physics::Aabb;

use crate::bench_kit::cmd;
use crate::blocks::entities::EntKind;
use crate::blocks::probe::Probe;
use crate::mobs::visible_part::{FORCED_VISIBLE_PART_PATHS, VisiblePartPaths};

/// Floats in [0, 1) of a small fixed generator.
struct Draws(u64);

impl Draws {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }

    fn pick<'a, T>(&mut self, of: &'a [T]) -> &'a T {
        &of[(self.next() * of.len() as f64) as usize]
    }
}

fn digest(bytes: &[u8]) -> String {
    crate::rng::md5(bytes)[..8].iter().map(|x| format!("{x:02x}")).collect()
}

/// Number of on/off switches of `VisiblePartPaths`.
const SWITCHES: usize = 6;

/// `paths` with switch `i` set to `on`.
fn with_switch(mut paths: VisiblePartPaths, i: usize, on: bool) -> VisiblePartPaths {
    let switch = match i {
        0 => &mut paths.cell_cube,
        1 => &mut paths.air_shortcut,
        2 => &mut paths.cube_per_call,
        3 => &mut paths.same_box_once,
        4 => &mut paths.parallel,
        _ => &mut paths.objects_one_pass,
    };
    *switch = on;
    paths
}

/// Every former path, the defaults, each new path alone, every new path, each former path
/// alone, the threads from the first target on 2, 4 and 8 threads, and the cube filled always
/// and never.
fn every_path_mix() -> Vec<(String, VisiblePartPaths)> {
    let old = VisiblePartPaths::OLD;
    // Every new path with the threads from the first target, then the defaults as they are.
    let new = VisiblePartPaths { walked_cells_per_task: 1, threads: 4, ..VisiblePartPaths::NEW };
    let mut mixes = vec![
        ("old".to_owned(), old),
        ("defaults".to_owned(), VisiblePartPaths::NEW),
        ("new".to_owned(), new),
        ("cube always".to_owned(), VisiblePartPaths { cube_cost_limit: usize::MAX, ..new }),
        ("cube never".to_owned(), VisiblePartPaths { cube_cost_limit: 0, ..new }),
        ("lone cube always".to_owned(), VisiblePartPaths { lone_box_cube_cost_limit: usize::MAX, ..new }),
        ("lone cube never".to_owned(), VisiblePartPaths { lone_box_cube_cost_limit: 0, ..new }),
    ];
    mixes.extend((0..SWITCHES).map(|i| (format!("only{i}"), with_switch(VisiblePartPaths { threads: 4, ..old }, i, true))));
    mixes.extend((0..SWITCHES).map(|i| (format!("but{i}"), with_switch(new, i, false))));
    mixes.extend([2, 8].map(|threads| (format!("threads{threads}"), VisiblePartPaths { threads, ..new })));
    mixes
}

/// Blocks of many collision shapes around the boxes of `visible_part_boxes`, across the chunk
/// border at x = 48.
fn build_shapes(p: &mut Probe) {
    crate::bench_visible_part::build_mixed_walls(p);
    let shapes = [
        "oak_carpet",
        "snow[layers=3]",
        "stone_slab[type=top]",
        "chain",
        "lantern",
        "iron_bars",
        "oak_trapdoor[half=top]",
        "oak_stairs[facing=east,half=top]",
        "scaffolding",
        "end_rod",
        "anvil",
        "brewing_stand",
        "cauldron",
        "hopper",
        "lily_pad",
        "torch",
        "water",
        "stone",
    ];
    let mut r = Draws(21);
    for _ in 0..400 {
        let (x, y, z) = ((r.next() * 30.0) as i32 + 38, (r.next() * 6.0) as i32 - 60, (r.next() * 22.0) as i32 + 6);
        let block = r.pick(&shapes);
        // Refused when the cell already holds that block: nothing to place.
        let src = p.s.server_source();
        let _ = p.s.perform(src, &format!("setblock {x} {y} {z} minecraft:{block}"));
    }
    cmd(p, "fill 44 -61 8 46 -61 10 minecraft:air");
}

/// Box sizes (width, height): primed TNT, item, orb, zombie, player, armor stand, ghast, a point,
/// a thin pole.
const SIZES: [(f64, f64); 9] =
    [(0.98, 0.98), (0.25, 0.25), (0.5, 0.5), (0.6, 1.95), (0.6, 1.8), (0.5, 1.975), (4.0, 4.0), (0.0, 0.0), (0.1, 3.0)];

/// Boxes of every size around `center`, within 9 blocks: on the ground one time in three, some
/// on whole or half coordinates, then copies of earlier boxes and boxes equal but for their top.
fn boxes_around(center: [f64; 3], seed: u64) -> Vec<Aabb> {
    let mut r = Draws(seed);
    let mut boxes = Vec::new();
    for i in 0..48 {
        let (w, h) = *r.pick(&SIZES);
        let mut at = [center[0] + r.next() * 14.0 - 7.0, center[1] + r.next() * 6.0 - 1.5, center[2] + r.next() * 14.0 - 7.0];
        if i % 3 == 0 {
            at[1] = -60.0;
        }
        if i % 5 == 1 {
            at = at.map(|v| (v * 2.0).round() / 2.0);
        }
        boxes.push(Aabb { min: [at[0] - w / 2.0, at[1], at[2] - w / 2.0], max: [at[0] + w / 2.0, at[1] + h, at[2] + w / 2.0] });
    }
    for i in [0, 3, 3, 7, 12] {
        boxes.push(boxes[i]);
    }
    let taller = Aabb { max: [boxes[5].max[0], boxes[5].max[1] + 0.5, boxes[5].max[2]], ..boxes[5] };
    boxes.push(taller);
    boxes
}

/// Explosion centres: a TNT on the grass among the walls, in the air, inside a pane, at the chunk
/// border, inside the ground, at the top of the world and below its floor.
const CENTERS: [[f64; 3]; 7] = [
    crate::bench_visible_part::BENCH_CENTER,
    [58.0, -57.3, 17.0],
    [54.5, -59.5, 16.5],
    [47.95, -59.7, 10.2],
    [40.5, -63.5, 4.5],
    [60.5, 319.6, 20.5],
    [60.5, -64.5, 20.5],
];

/// Visible parts (as bits) of every box around every centre with `paths`, one call per box or
/// together.
fn visible_parts(p: &Probe, paths: VisiblePartPaths, together: bool) -> Vec<u32> {
    FORCED_VISIBLE_PART_PATHS.set(Some(paths));
    let mut out = Vec::new();
    for (i, center) in CENTERS.iter().enumerate() {
        let boxes = boxes_around(*center, i as u64 + 1);
        if together {
            out.extend(p.s.seen_percents(*center, &boxes).iter().map(|v| v.to_bits()));
        } else {
            out.extend(boxes.iter().map(|b| p.s.seen_percent(*center, b).to_bits()));
        }
    }
    FORCED_VISIBLE_PART_PATHS.set(None);
    out
}

/// Digest of the visible parts, recorded on the code before the visible-part paths
/// (`integ/optim3-v4` 1e0a7de4, Windows); the visible part uses no platform-dependent maths.
const FROZEN_VISIBLE_PARTS: &str = "b264faa2604d2315";

#[test]
fn visible_parts_match_the_former_path_on_every_path() {
    let (mut p, _dir) = crate::tests_g3f13::open("visible-part-paths");
    build_shapes(&mut p);
    let former = visible_parts(&p, VisiblePartPaths::OLD, false);
    let distinct: hashbrown::HashSet<u32> = former.iter().copied().collect();
    assert!(distinct.len() > 40, "too few different parts to prove anything: {}", distinct.len());
    assert!(former.contains(&0) && former.contains(&1.0f32.to_bits()), "no hidden or no clear box");
    let bytes: Vec<u8> = former.iter().flat_map(|v| v.to_le_bytes()).collect();
    assert_eq!(digest(&bytes), FROZEN_VISIBLE_PARTS, "the former path no longer gives the frozen parts");
    let mut differences = Vec::new();
    for (tag, paths) in every_path_mix() {
        for together in [false, true] {
            let got = visible_parts(&p, paths, together);
            let wrong = got.iter().zip(&former).filter(|(a, b)| a != b).count();
            if wrong > 0 {
                differences.push(format!("{tag}, together {together}: {wrong} parts differ"));
            }
        }
    }
    assert!(differences.is_empty(), "visible parts differ: {differences:#?}");
}

/// The frozen parts of the shapes used by the explosion proofs (`tests_explosion_rays`) on every
/// path: boxes half hidden behind a slab, a fence, a pane, stairs and a full block.
#[test]
fn half_hidden_boxes_keep_their_frozen_parts_on_every_path() {
    let (mut p, _dir) = crate::tests_g3f13::open("visible-part-half-hidden");
    for (x, block) in [(10, "stone_slab"), (14, "oak_fence"), (18, "glass_pane"), (22, "oak_stairs"), (26, "stone")] {
        cmd(&mut p, &format!("setblock {x} -60 0 minecraft:{block}"));
    }
    let boxes: Vec<Aabb> = [10, 14, 18, 22, 26]
        .iter()
        .flat_map(|&x| {
            let x = f64::from(x) + 1.2;
            [0.0, 0.3].map(|dz| Aabb { min: [x, -60.0, 0.2 + dz], max: [x + 0.6, -58.2, 0.8 + dz] })
        })
        .collect();
    let frozen: [f32; 10] =
        [0.6, 0.6, 0.688_888_9, 0.688_888_9, 0.733_333_35, 0.733_333_35, 0.422_222_23, 0.6, 0.066_666_67, 0.066_666_67];
    for (tag, paths) in every_path_mix() {
        FORCED_VISIBLE_PART_PATHS.set(Some(paths));
        let one_by_one: Vec<f32> = boxes.iter().map(|b| p.s.seen_percent([b.min[0] - 2.5, -59.5, 0.5], b)).collect();
        assert_eq!(one_by_one, frozen, "{tag}");
    }
    FORCED_VISIBLE_PART_PATHS.set(None);
}

/// Bytes of everything an explosion moves or hurts: primed TNT and falling blocks, items, orbs,
/// mobs, the player.
fn record_objects(p: &Probe, out: &mut Vec<u8>) {
    let f64s = |out: &mut Vec<u8>, v: &[f64]| v.iter().for_each(|x| out.extend_from_slice(&x.to_bits().to_le_bytes()));
    let i32s = |out: &mut Vec<u8>, v: &[i32]| v.iter().for_each(|x| out.extend_from_slice(&x.to_le_bytes()));
    i32s(out, &[p.s.tick as i32]);
    for e in &p.s.bs.entities.list {
        let fuse = if let EntKind::Tnt(fuse) = e.kind { fuse } else { -1 };
        i32s(out, &[e.id, fuse, i32::from(e.removed)]);
        f64s(out, &e.pos);
        f64s(out, &e.vel);
    }
    for d in &p.s.life.drops {
        out.extend_from_slice(d.stack.id.as_bytes());
        i32s(out, &[d.id, d.stack.count, d.health, i32::from(d.is_removed())]);
        f64s(out, &d.pos);
        f64s(out, &d.vel);
    }
    for o in &p.s.life.orbs {
        i32s(out, &[o.id, o.health, i32::from(o.is_removed())]);
        f64s(out, &o.pos);
        f64s(out, &o.vel);
    }
    for m in p.s.mobs.list.iter() {
        i32s(out, &[m.id, m.kind as i32]);
        f64s(out, &m.pos());
        f64s(out, &m.vel);
        out.extend_from_slice(&m.health.to_bits().to_le_bytes());
    }
    f64s(out, &p.s.pos);
    f64s(out, &p.s.mobs.player_vel);
    out.extend_from_slice(&p.s.life.vit.health.to_bits().to_le_bytes());
}

/// What a scene records: the world and its objects, and the entity and explosion events sent to
/// the client (what the network sees: velocities sent at once, removals…).
#[derive(Default)]
struct SceneRecord {
    state: Vec<u8>,
    events: Vec<u8>,
}

/// `scene` in a fresh world on a thread of its own with `paths`; the digests of what it
/// recorded, (state, events).
fn scene_digest(tag: &str, paths: VisiblePartPaths, scene: fn(&mut Probe, &mut SceneRecord)) -> (String, String) {
    let tag = tag.to_owned();
    std::thread::Builder::new()
        .stack_size(32 << 20)
        .spawn(move || {
            FORCED_VISIBLE_PART_PATHS.set(Some(paths));
            let (mut p, _dir) = crate::tests_g3f13::open(&tag);
            let mut record = SceneRecord::default();
            scene(&mut p, &mut record);
            (digest(&record.state), digest(&record.events))
        })
        .expect("scene thread")
        .join()
        .expect("scene")
}

/// Runs a tick; the entity and explosion events sent to the client are recorded, the others
/// thrown away.
fn tick(p: &mut Probe, events: &mut Vec<u8>) {
    p.s.game_tick();
    p.s.tick += 1;
    for e in p.take_events() {
        if matches!(e, rc_client::Event::Entity(_) | rc_client::Event::Explode(_)) {
            events.extend_from_slice(format!("{e:?}\n").as_bytes());
        }
    }
}

/// Twenty neighbouring TNT blocks behind a partial wall, among items (one a shulker box whose
/// contents spill when the blast destroys it), orbs, two falling blocks, two primed TNT stacked
/// on one spot, zombies, armor stands and a creative player; the corner TNT primed, every tick
/// recorded until no primed TNT is left.
fn twenty_tnt(p: &mut Probe, record: &mut SceneRecord) {
    for i in 0..20 {
        cmd(p, &format!("setblock {} -60 {} minecraft:tnt", 80 + 2 * (i % 5), 2 + 2 * (i / 5)));
    }
    for z in 0..11 {
        let wall = ["oak_fence", "glass_pane", "stone_slab", "cobblestone_wall"][z as usize % 4];
        cmd(p, &format!("setblock 78 -60 {z} minecraft:{wall}"));
    }
    let mut r = Draws(3);
    for i in 0..30 {
        let (x, z) = (77.0 + r.next() * 14.0, r.next() * 12.0 - 1.0);
        let item = if i == 7 {
            "{id:\"minecraft:shulker_box\",count:1,components:{\"minecraft:container\":[{slot:0,item:{id:\"minecraft:diamond\",count:2}}]}}"
        } else {
            "{id:\"minecraft:cobblestone\",count:3}"
        };
        cmd(p, &format!("summon minecraft:item {x:.3} -60 {z:.3} {{Item:{item},PickupDelay:32767s}}"));
    }
    for _ in 0..10 {
        let (x, z) = (78.0 + r.next() * 12.0, r.next() * 10.0);
        cmd(p, &format!("summon minecraft:experience_orb {x:.3} -60 {z:.3} {{Value:3}}"));
    }
    for x in [85.5, 91.5] {
        cmd(p, &format!("summon minecraft:falling_block {x} -57 5.5 {{BlockState:{{Name:\"minecraft:sand\"}},Time:1,NoGravity:1b}}"));
    }
    for _ in 0..2 {
        cmd(p, "summon minecraft:tnt 87.5 -60 9.5 {fuse:30s}");
    }
    for z in [2.5, 6.5] {
        cmd(p, &format!("summon minecraft:zombie 76.5 -60 {z} {{NoAI:1b,PersistenceRequired:1b}}"));
        cmd(p, &format!("summon minecraft:armor_stand 92.5 -60 {z}"));
    }
    p.s.game_mode = 1;
    p.s.flying = false;
    p.s.pos = [79.5, -60.0, 12.5];
    cmd(p, "setblock 80 -60 2 minecraft:air");
    p.s.tnt_prime([80, -60, 2]);
    let summoned: Vec<i32> = p.s.life.drops.iter().map(|d| d.id).collect();
    let mut blasts = 0;
    for t in 0..400 {
        tick(p, &mut record.events);
        blasts += crate::mobs::explode::TNT_BLASTS.with_borrow_mut(std::mem::take).len();
        record_objects(p, &mut record.state);
        if t > 10 && !crate::bench_tnt_heap::primed_tnt_left(p) {
            break;
        }
    }
    assert!(blasts >= 20, "the chain did not run: {blasts} explosions");
    // The blast destroys items, not only pushes them (none can be picked up).
    let destroyed = summoned.iter().filter(|&&id| !p.s.life.drops.iter().any(|d| d.id == id)).count();
    assert!(destroyed >= 10, "the blast destroyed {destroyed} of the {} items", summoned.len());
}

/// Digest of the twenty TNT's state, recorded on the code before the visible-part paths
/// (1e0a7de4, Windows); its events are compared with the former paths run here.
const FROZEN_TWENTY_TNT: &str = "ba951c19508429c9";

#[test]
fn twenty_tnt_push_and_hurt_like_the_former_paths() {
    let former = scene_digest("twenty-tnt-old", VisiblePartPaths::OLD, twenty_tnt);
    if cfg!(windows) {
        assert_eq!(former.0, FROZEN_TWENTY_TNT, "the former paths no longer give the frozen digest");
    }
    let differences: Vec<String> = every_path_mix()
        .into_iter()
        .filter_map(|(tag, paths)| {
            let got = scene_digest(&format!("twenty-tnt-{tag}"), paths, twenty_tnt);
            (got != former).then(|| format!("{tag}: {got:?}"))
        })
        .collect();
    assert!(differences.is_empty(), "twenty TNT digests differ from {former:?}: {differences:#?}");
}

/// The 1,728-TNT heap with zombies around it, run to its end; then every block of the area and
/// every object recorded (what a save of the world would hold).
fn tnt_heap(p: &mut Probe, record: &mut SceneRecord) {
    let out = &mut record.state;
    for x in [36.5, 56.5] {
        cmd(p, &format!("summon minecraft:zombie {x} -60 9.5 {{NoAI:1b,PersistenceRequired:1b}}"));
    }
    p.s.game_mode = 1;
    crate::bench_tnt_heap::build_and_set_off_heap(p);
    let run = crate::bench_tnt_heap::run_heap_chain(p, 1200);
    assert!(run.explosions > 1500, "the heap did not go off: {} explosions", run.explosions);
    for x in 20..72 {
        for z in -12..32 {
            for y in -64..-30 {
                out.extend_from_slice(&p.s.st([x, y, z]).to_le_bytes());
            }
        }
    }
    record_objects(p, out);
}

/// Digest of the heap, recorded on the code before the visible-part paths (1e0a7de4, Windows).
const FROZEN_TNT_HEAP: &str = "151af9d5fa45dd3d";

#[test]
fn tnt_heap_leaves_the_world_of_the_former_paths() {
    let former = scene_digest("heap-old", VisiblePartPaths::OLD, tnt_heap).0;
    if cfg!(windows) {
        assert_eq!(former, FROZEN_TNT_HEAP, "the former paths no longer give the frozen digest");
    }
    let threads = VisiblePartPaths { walked_cells_per_task: 1, threads: 8, ..VisiblePartPaths::NEW };
    let one_thread = VisiblePartPaths { parallel: false, ..VisiblePartPaths::NEW };
    let runs = [("defaults", VisiblePartPaths::NEW), ("all new paths on one thread", one_thread), ("threads", threads)];
    let differences: Vec<String> = runs
        .into_iter()
        .filter_map(|(tag, paths)| {
            let got = scene_digest(&format!("heap-{}", tag.replace(' ', "-")), paths, tnt_heap).0;
            (got != former).then(|| format!("{tag}: {got}"))
        })
        .collect();
    assert!(differences.is_empty(), "heap digests differ from {former}: {differences:#?}");
}
