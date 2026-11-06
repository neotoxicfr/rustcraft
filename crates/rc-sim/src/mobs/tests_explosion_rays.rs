//! Equivalence proofs of the explosion and wind burst engine. A fixed set of explosions and
//! bursts in a flat world is fingerprinted: cells hit by each explosion (in order), blocks
//! left, items on the ground, primed TNT tick by tick, mobs, decorations and the player. The
//! digests were recorded on the code before the ray walker was shared; every switch
//! combination must give them again. Each run gets a thread of its own, so the test random
//! seeds and the recorded cells start afresh.

use crate::blocks::entities::EntKind;
use crate::blocks::probe::Probe;
use crate::mobs::blast_rays::{BlastPaths, FORCED_BLAST_PATHS};
use crate::mobs::explode::Blast;
use crate::tests_g3f13::open;

fn cmd(p: &mut Probe, c: &str) {
    let src = p.s.server_source();
    let (ok, _) = p.s.perform(src, c);
    assert!(ok, "command refused: {c}");
}

fn game_ticks(p: &mut Probe, n: i32) {
    for _ in 0..n {
        p.s.game_tick();
        p.s.tick += 1;
    }
}

/// Bytes of each fingerprinted part, digested at the end.
#[derive(Default)]
struct Trace {
    parts: Vec<(&'static str, Vec<u8>)>,
}

impl Trace {
    fn part(&mut self, name: &'static str) -> &mut Vec<u8> {
        if let Some(i) = self.parts.iter().position(|(n, _)| *n == name) {
            return &mut self.parts[i].1;
        }
        self.parts.push((name, Vec::new()));
        &mut self.parts.last_mut().expect("part").1
    }

    fn digests(&self) -> Vec<(&'static str, String)> {
        self.parts
            .iter()
            .map(|(n, b)| (*n, crate::rng::md5(b)[..8].iter().map(|x| format!("{x:02x}")).collect()))
            .collect()
    }
}

fn put_f64s(out: &mut Vec<u8>, v: &[f64]) {
    for x in v {
        out.extend_from_slice(&x.to_bits().to_le_bytes());
    }
}

fn put_i32s(out: &mut Vec<u8>, v: &[i32]) {
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
}

/// Cells hit by every explosion and burst since the last call, in order.
fn record_cells(t: &mut Trace) {
    let blasts = crate::mobs::explode::BLAST_CELLS.with_borrow_mut(std::mem::take);
    let out = t.part("cells");
    for (center, cells) in blasts {
        put_f64s(out, &center);
        put_i32s(out, &[cells.len() as i32]);
        for c in cells {
            put_i32s(out, &c);
        }
    }
}

fn record_primed_tnt(p: &Probe, t: &mut Trace) {
    let out = t.part("primed tnt");
    put_i32s(out, &[p.s.tick as i32]);
    for e in p.s.bs.entities.list.iter().filter(|e| !e.removed) {
        if let EntKind::Tnt(fuse) = e.kind {
            put_i32s(out, &[e.id, fuse]);
            put_f64s(out, &e.pos);
        }
    }
}

/// Blocks of every area the scenes touch.
fn record_blocks(p: &Probe, t: &mut Trace) {
    let out = t.part("blocks");
    for x in -32..160 {
        for z in -32..48 {
            for y in (-64..-48).chain(312..320) {
                put_i32s(out, &[p.s.st([x, y, z]) as i32]);
            }
        }
    }
}

fn record_entities(p: &Probe, t: &mut Trace) {
    let out = t.part("items");
    for d in &p.s.life.drops {
        out.extend_from_slice(d.stack.id.as_bytes());
        put_i32s(out, &[d.id, d.stack.count, i32::from(d.is_removed())]);
        put_f64s(out, &d.pos);
        put_f64s(out, &d.vel);
    }
    let out = t.part("mobs");
    for m in p.s.mobs.list.iter() {
        put_i32s(out, &[m.id, m.kind as i32]);
        put_f64s(out, &m.pos());
        put_f64s(out, &m.vel);
        out.extend_from_slice(&m.health.to_bits().to_le_bytes());
    }
    let out = t.part("decorations");
    let mut decos: Vec<&(i32, rc_proto::nbt::Nbt)> = p.s.entities.values().flat_map(|l| l.iter()).collect();
    decos.sort_by_key(|(id, _)| *id);
    for (id, e) in decos {
        put_i32s(out, &[*id]);
        put_f64s(out, &crate::nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default());
        put_f64s(out, &crate::nbtio::f64s::<3>(e.get("Motion")).unwrap_or_default());
    }
    let out = t.part("player");
    put_f64s(out, &p.s.pos);
    put_f64s(out, &p.s.mobs.player_vel);
    out.extend_from_slice(&p.s.life.vit.health.to_bits().to_le_bytes());
}

/// 8 × 8 TNT blocks every 3 blocks on the grass, mixed blocks between them, mobs and armor
/// stands behind a partial wall, a creative player in reach; the corner TNT is primed, then
/// the chain runs until no primed TNT is left.
fn tnt_grid(p: &mut Probe, t: &mut Trace) {
    for (x, z, block) in [
        (44, 4, "oak_log"),
        (47, 7, "glass"),
        (50, 10, "obsidian"),
        (53, 4, "oak_leaves"),
        (41, 16, "beehive"),
        (59, 19, "stone"),
        (45, 13, "cobblestone"),
    ] {
        cmd(p, &format!("setblock {x} -60 {z} minecraft:{block}"));
    }
    cmd(p, "setblock 56 -60 13 minecraft:chest{Items:[{Slot:0b,id:\"minecraft:diamond\",count:3}]}");
    for z in 0..22 {
        let wall = ["oak_fence", "glass_pane", "stone_slab", "cobblestone_wall"][z as usize % 4];
        cmd(p, &format!("setblock 37 -60 {z} minecraft:{wall}"));
    }
    for z in [2.5, 8.5, 14.5] {
        cmd(p, &format!("summon minecraft:zombie 35.5 -60 {z} {{NoAI:1b,PersistenceRequired:1b}}"));
    }
    cmd(p, "summon minecraft:zombie 52.5 -60 11.5 {NoAI:1b,PersistenceRequired:1b}");
    cmd(p, "summon minecraft:iron_golem 63.5 -60 10.5 {NoAI:1b,PersistenceRequired:1b}");
    for z in [5.5, 11.5, 17.5] {
        cmd(p, &format!("summon minecraft:armor_stand 35.5 -60 {z}"));
    }
    for i in 0..8 {
        for j in 0..8 {
            cmd(p, &format!("setblock {} -60 {} minecraft:tnt", 40 + 3 * i, 3 * j));
        }
    }
    p.s.game_mode = 1;
    p.s.flying = false;
    p.s.pos = [36.5, -60.0, 10.5];
    cmd(p, "setblock 40 -60 0 minecraft:air");
    p.s.tnt_prime([40, -60, 0]);
    for tick in 0..600 {
        game_ticks(p, 1);
        record_primed_tnt(p, t);
        let primed = p.s.bs.entities.list.iter().any(|e| !e.removed && matches!(e.kind, EntKind::Tnt(_)));
        if !primed && tick > 10 {
            break;
        }
    }
    assert!(crate::mobs::explode::BLAST_CELLS.with_borrow(Vec::len) >= 32, "TNT chain did not run");
    record_cells(t);
    record_entities(p, t);
}

/// The special explosions, called directly: TNT in water, TNT that went through a portal, blue
/// wither skull, a radius-7 explosion, a primed TNT minecart on rails, a respawn anchor in water
/// with fire, the world's floor and ceiling, a radius-20 explosion above the ground.
fn special_blasts(p: &mut Probe, t: &mut Trace) {
    p.s.pos = [0.5, -60.0, 0.5];
    // Primed TNT in a water pool.
    cmd(p, "fill 86 -61 -4 94 -57 4 minecraft:water");
    cmd(p, "summon minecraft:tnt 90.5 -60 0.5 {fuse:1}");
    game_ticks(p, 3);
    // TNT that went through a portal: the portal neither resists nor breaks.
    cmd(p, "setblock 104 -60 1 minecraft:nether_portal[axis=z]");
    cmd(p, "setblock 104 -59 1 minecraft:nether_portal[axis=z]");
    cmd(p, "fill 105 -60 -1 105 -58 3 minecraft:glass");
    p.s.tnt_explode(-1, [102.5, -59.5, 1.5], 4.0, None, false, true);
    // Blue skull: destructible blocks resist 0.8 at most, bedrock stays.
    cmd(p, "fill 113 -60 -1 117 -58 3 minecraft:obsidian");
    cmd(p, "setblock 115 -57 1 minecraft:bedrock");
    p.s.mobs.wither.dangerous_blast = true;
    p.s.explode_core(None, None, None, [115.5, -56.5, 1.5], 1.0, false, "minecraft:wither_skull");
    p.s.mobs.wither.dangerous_blast = false;
    // Radius 7 into stone with an obsidian line.
    cmd(p, "fill 124 -63 24 136 -58 36 minecraft:stone");
    cmd(p, "fill 124 -60 30 136 -60 30 minecraft:obsidian");
    p.s.explode_core(None, None, None, [130.5, -60.5, 30.5], 7.0, false, "minecraft:explosion");
    // Primed TNT minecart: rails and the blocks under them spared.
    cmd(p, "fill 141 -60 0 149 -60 0 minecraft:rail");
    cmd(p, "fill 141 -60 2 149 -60 2 minecraft:powered_rail");
    let cart = Blast::TntCart { primed: true };
    p.s.explode_with(None, None, None, [145.5, -59.5, 1.0], 4.0, false, "minecraft:explosion", cart);
    // Respawn anchor in water, with fire: its own cell resists like water.
    cmd(p, "fill 96 -61 26 104 -59 34 minecraft:water");
    cmd(p, "setblock 100 -60 30 minecraft:air");
    let anchor = Blast::Respawn(Some([100, -60, 30]));
    p.s.explode_with(None, None, None, [100.5, -59.5, 30.5], 5.0, true, "minecraft:bad_respawn_point", anchor);
    // Floor: rays below the bottom of the world (bedrock removed).
    cmd(p, "fill -14 -64 -14 -6 -62 -6 minecraft:air");
    p.s.explode_with(None, None, None, [-9.5, -63.5, -9.5], 4.0, false, "minecraft:explosion", Blast::Tnt(false));
    // Ceiling: rays above the top of the world.
    cmd(p, "fill -14 316 16 -6 319 24 minecraft:stone");
    p.s.explode_with(None, None, None, [-9.5, 318.5, 19.5], 4.0, false, "minecraft:explosion", Blast::Tnt(false));
    // Radius 20, above the ground.
    p.s.explode_core(None, None, None, [20.5, -52.5, 20.5], 20.0, false, "minecraft:explosion");
    record_cells(t);
    record_entities(p, t);
    game_ticks(p, 20);
    record_entities(p, t);
}

/// Wind bursts (breeze radius 3, player charge radius 1.2) among the blocks they trigger and
/// the blocks that stop them.
fn wind_bursts(p: &mut Probe, t: &mut Trace) {
    for (x, block) in [
        (61, "lever"),
        (62, "oak_door"),
        (63, "oak_trapdoor"),
        (64, "oak_fence_gate"),
        (66, "candle[lit=true]"),
        (67, "bell"),
        (68, "barrier"),
        (69, "bedrock"),
    ] {
        cmd(p, &format!("setblock {x} -60 43 minecraft:{block}"));
    }
    cmd(p, "fill 60 -60 45 70 -58 45 minecraft:barrier");
    cmd(p, "summon minecraft:zombie 65.5 -60 41.5 {NoAI:1b,PersistenceRequired:1b}");
    cmd(p, "setblock 64 -60 41 minecraft:oak_fence");
    for x in [63.5, 67.5] {
        cmd(p, &format!("summon minecraft:armor_stand {x} -60 41.5"));
    }
    let breeze = "minecraft:entity.breeze.wind_burst";
    p.s.wind_burst_with(None, None, [65.5, -59.5, 43.5], 3.0, 1.0, breeze, &mut []);
    let charge = "minecraft:entity.wind_charge.wind_burst";
    p.s.wind_burst_with(None, None, [62.5, -59.5, 42.5], 1.2, 1.22, charge, &mut []);
    p.s.wind_burst_with(None, None, [68.5, -59.5, 44.5], 3.0, 1.0, breeze, &mut []);
    record_cells(t);
    record_entities(p, t);
}

/// Every scene in a fresh world, on a thread of its own; digests of each part.
fn run_scenes(tag: &str, before: impl FnOnce() + Send + 'static) -> Vec<(&'static str, String)> {
    let tag = tag.to_owned();
    std::thread::Builder::new()
        .stack_size(32 << 20)
        .spawn(move || {
            before();
            let (mut p, _dir) = open(&format!("blastrays-{tag}"));
            let mut t = Trace::default();
            tnt_grid(&mut p, &mut t);
            special_blasts(&mut p, &mut t);
            wind_bursts(&mut p, &mut t);
            record_blocks(&p, &mut t);
            t.digests()
        })
        .expect("scene thread")
        .join()
        .expect("scene run")
}

/// Digests of the code before the shared walker (recorded on `main` 2e768c2f).
const FROZEN: &[(&str, &str)] = &[
    ("primed tnt", "14b14d27de93c87f"),
    ("cells", "f8f3aca12fd38e89"),
    ("items", "d4a59b02c01ac18f"),
    ("mobs", "efe03ced8e874517"),
    ("decorations", "eb3c7dd8ef42a80d"),
    ("player", "c57c813a84dd981a"),
    ("blocks", "8034538fd1394bb7"),
];

/// Median of 3 timings of `n` calls of `f`, in microseconds per call.
fn micros_per_call(p: &mut Probe, n: u32, f: &dyn Fn(&mut Probe)) -> f64 {
    let mut times: Vec<f64> = (0..3)
        .map(|_| {
            let start = std::time::Instant::now();
            for _ in 0..n {
                f(p);
            }
            start.elapsed().as_secs_f64() * 1e6 / f64::from(n)
        })
        .collect();
    times.sort_by(f64::total_cmp);
    times[1]
}

/// Explosion that leaves the world as it is (no block interaction): the rays, the entity
/// passes and the packet.
fn harmless_blast(p: &mut Probe, center: [f64; 3], radius: f32) {
    p.s.explode_with(None, None, None, center, radius, false, "minecraft:explosion", Blast::None(false));
}

const BENCH_GROUND: [f64; 3] = [60.5, -59.94, 10.5];
const BENCH_MIX: [&str; 8] = ["stone", "dirt", "oak_log", "oak_leaves", "glass", "cobblestone", "oak_planks", "sand"];

/// Walls of mixed shapes (fences, panes, slabs, stairs, walls) between `BENCH_GROUND` and the
/// boxes of `bench_boxes`.
fn build_bench_walls(p: &mut Probe) {
    for (z, block) in ["oak_fence", "glass_pane", "stone_slab", "oak_stairs", "cobblestone_wall"].iter().enumerate() {
        let z = 30 + 2 * z as i32;
        cmd(p, &format!("fill 55 -60 {z} 66 -59 {z} minecraft:{block}"));
    }
}

/// 24 mob-sized boxes behind the walls of `build_bench_walls`.
fn bench_boxes() -> Vec<rc_physics::Aabb> {
    (0..24)
        .map(|i| {
            let (x, z) = (55.2 + f64::from(i % 12), 41.2 + f64::from(i / 12) * 1.5);
            rc_physics::Aabb { min: [x, -60.0, z], max: [x + 0.6, -58.05, z + 0.6] }
        })
        .collect()
}

/// One round of the bench with the paths in force, microseconds per call of each measure.
fn bench_round(p: &mut Probe, slow_old_walk: bool) -> Vec<(&'static str, f64)> {
    let mut out = Vec::new();
    // The former hit list search makes radius 20 take a third of a second: fewer calls.
    let (n7, n20) = if slow_old_walk { (3, 1) } else { (20, 5) };
    for (name, center, radius, n) in [
        ("tnt on the ground", BENCH_GROUND, 4.0, 30),
        ("tnt in the air", [60.5, -30.5, 10.5], 4.0, 30),
        ("tnt in water", [90.5, -50.5, 10.5], 4.0, 30),
        ("radius 7 on the ground", BENCH_GROUND, 7.0, n7),
        ("radius 20 on the ground", BENCH_GROUND, 20.0, n20),
    ] {
        out.push((name, micros_per_call(p, n, &|p| harmless_blast(p, center, radius))));
    }
    for (name, radius) in [("wind 1.2", 1.2), ("wind 3", 3.0)] {
        let sound = "minecraft:entity.wind_charge.wind_burst";
        out.push((
            name,
            micros_per_call(p, 30, &|p| p.s.wind_burst_with(None, None, BENCH_GROUND, radius, 1.0, sound, &mut [])),
        ));
    }
    let boxes = bench_boxes();
    let center = [60.5, -59.5, 27.5];
    let seen = micros_per_call(p, 20, &|p| {
        for b in &boxes {
            std::hint::black_box(p.s.seen_percent(center, b));
        }
    });
    out.push(("visible part of 24 boxes", seen));
    out.push(("destroying 512 blocks", destroy_mix_micros(p)));
    out
}

/// The 512-block mix filled again and the items on the ground removed.
fn refill_mix(p: &mut Probe) {
    for (i, block) in BENCH_MIX.iter().enumerate() {
        let x = 120 + i as i32;
        // Refused when the column is still whole: nothing to refill.
        let src = p.s.server_source();
        let _ = p.s.perform(src, &format!("fill {x} -61 6 {x} -54 13 minecraft:{block}"));
    }
    p.s.life.drops.clear();
}

/// A TNT in the middle of the block mix.
fn blow_mix(p: &mut Probe) {
    p.s.explode_with(None, None, None, [123.5, -57.5, 9.5], 4.0, false, "minecraft:explosion", Blast::Tnt(false));
}

/// Median time of a TNT destroying a fresh 512-block mix (refill and item removal not timed).
fn destroy_mix_micros(p: &mut Probe) -> f64 {
    let mut times = Vec::new();
    for _ in 0..9 {
        refill_mix(p);
        let start = std::time::Instant::now();
        blow_mix(p);
        times.push(start.elapsed().as_secs_f64() * 1e6);
    }
    times.sort_by(f64::total_cmp);
    times[times.len() / 2]
}

/// 300 drops of 8 kinds, as a TNT in the block mix gives.
fn mix_drops() -> Vec<crate::player::Stack> {
    let kinds = BENCH_MIX.map(|b| format!("minecraft:{b}"));
    (0..300)
        .map(|i| crate::player::Stack { id: kinds[i % 8].as_str().into(), count: 1 + (i % 3) as i32, components: None })
        .collect()
}

/// Microseconds of the loot grouping alone of `mix_drops` (the copies of the drops included).
fn loot_grouping_micros(open_stacks: bool) -> f64 {
    let drops = mix_drops();
    let start = std::time::Instant::now();
    for _ in 0..200 {
        let mut loot = crate::mobs::explode::BlastLoot::new(open_stacks);
        for d in &drops {
            loot.add([0; 3], d.clone());
        }
        std::hint::black_box(loot.stacks);
    }
    start.elapsed().as_secs_f64() * 1e6 / 200.0
}

/// The open stacks group the loot exactly like the walk over every stack, drop sizes and kinds
/// mixed (full stacks, a drop bigger than a stack, an empty drop).
#[test]
fn open_stacks_group_the_loot_like_every_stack() {
    let mut drops = mix_drops();
    drops.push(crate::player::Stack { id: "minecraft:stone".into(), count: 40, components: None });
    drops.push(crate::player::Stack { id: "minecraft:dirt".into(), count: 0, components: None });
    drops.extend(mix_drops().into_iter().rev());
    let group = |open_stacks: bool| {
        let mut loot = crate::mobs::explode::BlastLoot::new(open_stacks);
        for (i, d) in drops.iter().enumerate() {
            loot.add([i as i32, 0, 0], d.clone());
        }
        loot.stacks.into_iter().map(|(p, s)| (p, s.id, s.count)).collect::<Vec<_>>()
    };
    let every = group(false);
    assert!(every.len() > 40, "too few stacks to prove anything: {}", every.len());
    assert_eq!(group(true), every);
}

/// The paths the bench compares: every former path, each new path alone, every new path, each
/// former path alone, then the parallel rays (at every radius) on 1 thread and powers of 2 up
/// to `RUSTCRAFT_THREADS` or every logical core.
fn bench_configurations() -> Vec<(String, BlastPaths)> {
    const NAMES: [&str; SWITCHES] = [
        "last cell",
        "cell cache",
        "hit bits",
        "one state read",
        "shared walker",
        "clip shape ref",
        "lookups once",
        "mobs in box",
        "open stacks",
    ];
    let mut configs = vec![("all former".to_owned(), BlastPaths::OLD)];
    configs.extend(NAMES.iter().enumerate().map(|(i, n)| (format!("only {n}"), with_switch(BlastPaths::OLD, i, true))));
    configs.push(("all new".to_owned(), BlastPaths::NEW));
    configs.extend(
        NAMES.iter().enumerate().map(|(i, n)| (format!("all but {n}"), with_switch(BlastPaths::NEW, i, false))),
    );
    let cores = rc_data::env_str!("RUSTCRAFT_THREADS")
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, std::num::NonZero::get));
    let mut threads = 1;
    while threads <= cores {
        let paths = BlastPaths { parallel_rays: true, parallel_min_radius: 0.0, threads, ..BlastPaths::NEW };
        configs.push((format!("parallel rays {threads} threads"), paths));
        threads *= 2;
    }
    configs
}

/// Bench (ignored): microseconds per explosion that leaves the world as it is (TNT on the ground,
/// in the air, in water, radius 7 and 20 on the ground), per wind burst (radius 1.2 and 3), per
/// visible part of 24 boxes behind mixed walls, per TNT destroying a fresh 512-block mix, for each
/// configuration of `bench_configurations`. The configurations alternate over 3 rounds and the
/// best round is kept (loaded machine: the least disturbed).
/// `cargo test --release -p rc-sim --lib bench_explosion_rays -- --ignored --nocapture`.
#[test]
#[ignore = "measurement bench"]
fn bench_explosion_rays() {
    let (mut p, _dir) = open("blastrays-bench");
    cmd(&mut p, "fill 80 -61 0 100 -40 20 minecraft:water");
    build_bench_walls(&mut p);
    let configs = bench_configurations();
    let mut best: Vec<Vec<(&str, f64)>> = Vec::new();
    for round in 0..3 {
        for (i, (_, paths)) in configs.iter().enumerate() {
            FORCED_BLAST_PATHS.set(Some(*paths));
            let times = bench_round(&mut p, !paths.hit_bits);
            match best.get_mut(i) {
                Some(kept) => kept.iter_mut().zip(&times).for_each(|(k, t)| k.1 = k.1.min(t.1)),
                None => best.push(times),
            }
        }
        eprintln!("bench_explosion_rays round {round} done");
    }
    for ((label, _), times) in configs.iter().zip(&best) {
        let line: Vec<String> = times.iter().map(|(n, t)| format!("{n} {t:.1}")).collect();
        eprintln!("bench_explosion_rays {label}: {} (us)", line.join(", "));
    }
    for open_stacks in [false, true, false, true] {
        eprintln!(
            "bench_explosion_rays loot grouping of 300 drops, open stacks {open_stacks}: {:.1} us",
            loot_grouping_micros(open_stacks)
        );
    }
    // 500 mobs out of reach: what gathering them costs each explosion.
    for i in 0..500 {
        let (x, z) = (f64::from(i % 25) + 0.5, f64::from(i / 25) * 2.0 - 20.5);
        cmd(&mut p, &format!("summon minecraft:zombie {x} -60 {z} {{NoAI:1b,PersistenceRequired:1b}}"));
    }
    let mut by_box = [f64::MAX; 2];
    for _ in 0..3 {
        for (i, in_box) in [false, true].into_iter().enumerate() {
            FORCED_BLAST_PATHS.set(Some(BlastPaths { mobs_in_box: in_box, ..BlastPaths::NEW }));
            by_box[i] = by_box[i].min(micros_per_call(&mut p, 30, &|p| harmless_blast(p, BENCH_GROUND, 4.0)));
        }
    }
    eprintln!(
        "bench_explosion_rays tnt on the ground, 500 mobs out of reach: every mob {:.1} us, in the box {:.1} us",
        by_box[0], by_box[1]
    );
}

/// Work of the rays counted rather than timed (a loaded machine does not change it): cells
/// worked out by a TNT on the ground for each walk, then the allocations of an explosion, of
/// the visible part of 24 boxes behind walls and of a TNT destroying the block mix, former
/// paths against new ones. The visible part with borrowed shapes allocates nothing.
#[test]
fn explosion_work_and_allocations_are_counted() {
    use crate::mobs::blast_rays::{ExplosionRules, hit_cells};
    let (mut p, _dir) = open("blastrays-count");
    build_bench_walls(&mut p);
    let draws = p.s.ray_draws();
    let heights = p.s.world_heights();
    let rules = ExplosionRules {
        water_cell: None,
        portal_immune: false,
        nether_portal: u16::MAX,
        rail_safe: false,
        dangerous_wither: false,
    };
    let mut reads = Vec::new();
    for (last_cell, cell_cache) in [(false, false), (true, false), (false, true), (true, true)] {
        let worked_out = std::cell::Cell::new(0u32);
        let world = p.s.blast_world();
        let paths = BlastPaths { last_cell, cell_cache, ..BlastPaths::NEW };
        let hit = hit_cells(BENCH_GROUND, 4.0, &draws, heights, paths, |q| {
            worked_out.set(worked_out.get() + 1);
            world.explosion_effect(q, &rules)
        });
        reads.push(worked_out.get());
        eprintln!(
            "explosion work: last cell {last_cell}, cell cache {cell_cache}: {} cells worked out, {} hit",
            worked_out.get(),
            hit.len()
        );
    }
    assert!(reads[1] < reads[0] && reads[2] < reads[1] && reads[3] == reads[2], "cells worked out: {reads:?}");
    use crate::test_alloc::allocations;
    let boxes = bench_boxes();
    let mut counted = Vec::new();
    // A first round warms what is built on first use (loot tables, name indexes), not counted.
    for (label, paths) in [("warm-up", BlastPaths::NEW), ("former", BlastPaths::OLD), ("new", BlastPaths::NEW)] {
        FORCED_BLAST_PATHS.set(Some(paths));
        let before = allocations();
        harmless_blast(&mut p, BENCH_GROUND, 4.0);
        let blast = allocations() - before;
        let before = allocations();
        for b in &boxes {
            std::hint::black_box(p.s.seen_percent([60.5, -59.5, 27.5], b));
        }
        let seen = allocations() - before;
        refill_mix(&mut p);
        let before = allocations();
        blow_mix(&mut p);
        let destroy = allocations() - before;
        eprintln!(
            "explosion allocations, {label} paths: explosion {blast}, visible part of 24 boxes {seen}, destroying the mix {destroy}"
        );
        counted.push((blast, seen));
    }
    assert!(counted[1].1 > 0 && counted[2].1 == 0, "allocations of the visible parts: {counted:?}");
}

/// The parts whose digests differ from the frozen ones when the scenes run with `paths`
/// (`None`: the switches), named after `tag`.
fn differing_parts(tag: &str, paths: Option<BlastPaths>) -> Vec<String> {
    assert!(!FROZEN.is_empty(), "no frozen digests");
    let got = run_scenes(tag, move || FORCED_BLAST_PATHS.set(paths));
    assert_eq!(got.len(), FROZEN.len(), "{tag}: parts {got:?}");
    got.iter()
        .zip(FROZEN)
        .filter(|((name, digest), (frozen_name, frozen))| name != frozen_name || digest.as_str() != *frozen)
        .map(|((name, digest), _)| format!("{tag}: {name} {digest}"))
        .collect()
}

/// Asserts every run of `runs` gives the frozen digests (all runs made, then every difference
/// listed).
fn check_runs(runs: Vec<(String, Option<BlastPaths>)>) {
    let differences: Vec<String> = runs.iter().flat_map(|(tag, paths)| differing_parts(tag, *paths)).collect();
    assert!(differences.is_empty(), "explosion digests differ: {differences:#?}");
}

fn check_paths(tag: &str, paths: Option<BlastPaths>) {
    check_runs(vec![(tag.to_owned(), paths)]);
}

/// Number of on/off switches of `BlastPaths`.
const SWITCHES: usize = 9;

/// `paths` with switch `i` set to `on`.
fn with_switch(mut paths: BlastPaths, i: usize, on: bool) -> BlastPaths {
    let switch = match i {
        0 => &mut paths.last_cell,
        1 => &mut paths.cell_cache,
        2 => &mut paths.hit_bits,
        3 => &mut paths.one_state_read,
        4 => &mut paths.shared_walker,
        5 => &mut paths.clip_shape_ref,
        6 => &mut paths.lookups_once,
        7 => &mut paths.mobs_in_box,
        _ => &mut paths.open_stacks,
    };
    *switch = on;
    paths
}

#[test]
fn explosion_scenes_match_the_frozen_digests() {
    check_paths("current", None);
}

/// Every former path, then each new path alone.
#[test]
fn each_new_path_alone_gives_the_frozen_digests() {
    let mut runs = vec![("old".to_owned(), Some(BlastPaths::OLD))];
    runs.extend((0..SWITCHES).map(|i| (format!("only{i}"), Some(with_switch(BlastPaths::OLD, i, true)))));
    check_runs(runs);
}

/// Every new path, then each former path alone.
#[test]
fn each_former_path_alone_gives_the_frozen_digests() {
    let mut runs = vec![("new".to_owned(), Some(BlastPaths::NEW))];
    runs.extend((0..SWITCHES).map(|i| (format!("but{i}"), Some(with_switch(BlastPaths::NEW, i, false)))));
    check_runs(runs);
}

/// Rays shared between 1 to 32 threads at every radius, and the cells kept in hash maps only.
#[test]
fn parallel_rays_and_hash_maps_give_the_frozen_digests() {
    let parallel = BlastPaths { parallel_rays: true, parallel_min_radius: 0.0, ..BlastPaths::NEW };
    let mut runs: Vec<(String, Option<BlastPaths>)> = [1, 2, 4, 8, 32]
        .iter()
        .map(|&threads| (format!("threads{threads}"), Some(BlastPaths { threads, ..parallel })))
        .collect();
    runs.push(("maps".to_owned(), Some(BlastPaths { max_cube_cells: 0, ..BlastPaths::NEW })));
    runs.push(("maps-threads4".to_owned(), Some(BlastPaths { max_cube_cells: 0, threads: 4, ..parallel })));
    check_runs(runs);
}

/// Visible part of boxes half hidden behind a slab, a fence, a pane, stairs and a full block,
/// frozen on the former code; the same with the shape borrowed or copied.
#[test]
fn seen_percent_of_half_hidden_boxes_is_frozen() {
    let (mut p, _dir) = open("blastrays-seen");
    for (x, block) in [(10, "stone_slab"), (14, "oak_fence"), (18, "glass_pane"), (22, "oak_stairs"), (26, "stone")] {
        cmd(&mut p, &format!("setblock {x} -60 0 minecraft:{block}"));
    }
    let boxes: Vec<rc_physics::Aabb> = [10, 14, 18, 22, 26]
        .iter()
        .flat_map(|&x| {
            let x = f64::from(x) + 1.2;
            [0.0, 0.3].map(|dz| rc_physics::Aabb { min: [x, -60.0, 0.2 + dz], max: [x + 0.6, -58.2, 0.8 + dz] })
        })
        .collect();
    let seen = |p: &Probe, shape_ref: bool| -> Vec<f32> {
        FORCED_BLAST_PATHS.set(Some(BlastPaths { clip_shape_ref: shape_ref, ..BlastPaths::NEW }));
        boxes.iter().map(|b| p.s.seen_percent([b.min[0] - 2.5, -59.5, 0.5], b)).collect()
    };
    let frozen: [f32; 10] =
        [0.6, 0.6, 0.688_888_9, 0.688_888_9, 0.733_333_35, 0.733_333_35, 0.422_222_23, 0.6, 0.066_666_67, 0.066_666_67];
    assert_eq!(seen(&p, false), frozen, "copied shapes");
    assert_eq!(seen(&p, true), frozen, "borrowed shapes");
}
