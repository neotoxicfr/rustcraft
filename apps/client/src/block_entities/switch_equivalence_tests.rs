//! Equivalence proofs of the block entity optimisation switches (`RUSTCRAFT_OPT_BLOCK_ENTITY_PROPS`,
//! `_TICKER_LIST`, `_CHANGE_EARLY_EXIT`, `RUSTCRAFT_OPT_CONDUIT_WATER_TABLE`) and their micro-benches
//! (ignored, `--ignored --nocapture`).
//!
//! One scripted world holds every drawn kind plus the entities whose tick reads the block state
//! (vaults, the six trial spawner states, campfires, potent sulfur, a working conduit) and moving
//! blocks; its states change through `on_block_change` while it runs. Each run records, tick by
//! tick, the particles, sounds and piston steps (bit for bit) and every entity with its animation.

use std::sync::Arc;

use super::*;

fn version() -> Arc<VersionData> {
    Arc::new(rc_testdata::version_data().expect("version data missing (extraction)"))
}

/// The defaults: every new path, ticking entities walked by column (hash order).
const DEFAULTS: Switches = Switches {
    props_in_tick: true,
    ticker_list: true,
    ticker_order: false,
    change_early_exit: true,
    conduit_water_table: true,
};

/// Every ticking entity walked in insertion order: an order that does not depend on the hash
/// maps, for comparisons in emission order.
const ORDERED: Switches = Switches { ticker_order: true, ..DEFAULTS };

fn block<'a>(d: &'a VersionData, name: &str) -> &'a rc_data::BlockInfo {
    d.blocks.iter().find(|b| b.name == name).unwrap_or_else(|| panic!("block {name}"))
}

/// State of `name` with the properties `props` (the others: first match).
fn state(d: &VersionData, name: &str, props: &[(&str, &str)]) -> u32 {
    let b = block(d, name);
    (b.first_state..b.first_state + b.state_count)
        .find(|&s| props.iter().all(|p| d.state_properties(s).any(|q| q == *p)))
        .unwrap_or_else(|| panic!("state {name} {props:?}"))
}

const PLAYER: [f64; 3] = [10.5, 64.0, 10.5];
const PLAYER_ID: u128 = 0x1234;
const TRIAL_STATES: [&str; 6] =
    ["inactive", "waiting_for_players", "active", "waiting_for_reward_ejection", "ejecting_reward", "cooldown"];
const FACINGS: [&str; 4] = ["north", "south", "west", "east"];

/// What one tick produced, as comparable text (floats as bits).
#[derive(PartialEq, Debug)]
struct TickRecord {
    particles: Vec<String>,
    sounds: Vec<String>,
    pistons: Vec<String>,
    moving: usize,
    version: u64,
    entities: Vec<String>,
}

struct Scene {
    bes: BlockEntities,
    world: HashMap<[i32; 3], u32>,
    air: u32,
    trial_spawners: Vec<([i32; 3], bool)>,
    campfires: Vec<([i32; 3], &'static str, bool, &'static str)>,
    sulfur: Vec<([i32; 3], &'static str)>,
    vault: [i32; 3],
    chest: [i32; 3],
    spawner: [i32; 3],
}

impl Scene {
    fn set(&mut self, pos: [i32; 3], s: u32) {
        let old = self.world.get(&pos).copied().unwrap_or(self.air);
        self.bes.on_block_change(pos, old, s);
        if s == self.air {
            self.world.remove(&pos);
        } else {
            self.world.insert(pos, s);
        }
    }

    fn entity(&mut self, pos: [i32; 3]) -> &mut BlockEntity {
        self.bes.get_mut(pos).unwrap_or_else(|| panic!("entity at {pos:?}"))
    }

    fn new(d: &Arc<VersionData>, sw: Switches) -> Scene {
        let mut bes = BlockEntities::new(d.clone());
        bes.switches = sw;
        let prismarine = d.blocks.iter().position(|b| b.name == "minecraft:prismarine").expect("prismarine") as i32;
        bes.set_block_tags(&[("minecraft:conduit_effect_block".into(), vec![prismarine])]);
        let mut scene = Scene {
            bes,
            world: HashMap::new(),
            air: state(d, "minecraft:air", &[]),
            trial_spawners: Vec::new(),
            campfires: Vec::new(),
            sulfur: Vec::new(),
            vault: [10, 64, 12],
            chest: [0, 0, 0],
            spawner: [12, 64, 9],
        };
        scene.place_every_kind(d);
        scene.place_state_readers(d);
        scene.place_animated(d);
        scene.place_conduit(d);
        scene
    }

    /// One block of every drawn kind (moving pistons apart), in a row across four columns.
    fn place_every_kind(&mut self, d: &VersionData) {
        let mut seen = Vec::new();
        for (i, b) in d.blocks.iter().enumerate() {
            let Some(c) = self.bes.tables.carriers[i] else { continue };
            if c.kind == Kind::Piston || seen.contains(&c.kind) {
                continue;
            }
            let pos = [2 * seen.len() as i32, 70, 40];
            seen.push(c.kind);
            self.set(pos, b.default_state);
            if c.kind == Kind::Chest {
                self.chest = pos;
                self.bes.on_event(pos, 1, 1, 0);
            }
        }
        assert_eq!(seen.len(), Kind::ALL.len() - 1, "{seen:?}");
    }

    /// Vaults, the six trial spawner states (ominous or not), campfires, potent sulfur.
    fn place_state_readers(&mut self, d: &VersionData) {
        let item = Some(Stack { item: 1, count: 1 });
        for (pos, facing, ominous, with_item) in [
            (self.vault, "north", false, true),
            ([11, 64, 12], "east", true, true),
            ([12, 64, 12], "south", false, false),
        ] {
            let ominous_value = if ominous { "true" } else { "false" };
            self.set(pos, state(d, "minecraft:vault", &[("facing", facing), ("ominous", ominous_value)]));
            let e = self.entity(pos);
            e.data = Data::Vault(if with_item { item } else { None });
            e.anim.vault_players = vec![PLAYER_ID];
            e.anim.vault_range = 4.5;
        }
        for (i, st) in TRIAL_STATES.iter().enumerate() {
            for ominous in [false, true] {
                let pos = [20 + i as i32, 64, if ominous { 4 } else { 2 }];
                let value = if ominous { "true" } else { "false" };
                self.set(pos, state(d, "minecraft:trial_spawner", &[("trial_spawner_state", st), ("ominous", value)]));
                self.entity(pos).data = Data::Trial(Some("minecraft:zombie".into()), 1100);
                self.trial_spawners.push((pos, ominous));
            }
        }
        for i in 0..8 {
            let name = if i < 4 { "minecraft:campfire" } else { "minecraft:soul_campfire" };
            let (lit, signal, facing) = (i % 2 == 0, (i / 2) % 2 == 0, FACINGS[i % 4]);
            let pos = [30 + 2 * i as i32, 64, 20];
            self.set(pos, campfire_state(d, name, lit, signal, facing));
            self.entity(pos).data = Data::Items(vec![item, None, item, item], false);
            self.campfires.push((pos, name, signal, facing));
        }
        let water = state(d, "minecraft:water", &[("level", "0")]);
        for (i, st) in ["wet", "dry", "dormant"].into_iter().enumerate() {
            let pos = [50 + 2 * i as i32, 64, 5];
            self.set(pos, state(d, "minecraft:potent_sulfur", &[("potent_sulfur_state", st)]));
            for y in 65..=66 {
                self.world.insert([pos[0], y, pos[2]], water);
            }
            self.sulfur.push((pos, st));
        }
    }

    /// Entities whose tick moves on its own: spawner, enchanting table, bell, powered head,
    /// gateway, beacon.
    fn place_animated(&mut self, d: &VersionData) {
        self.set(self.spawner, state(d, "minecraft:spawner", &[]));
        self.entity(self.spawner).data = Data::Spawner(Some("minecraft:pig".into()), 16, 20, 200);
        self.set([11, 64, 11], state(d, "minecraft:enchanting_table", &[]));
        let bell = [9, 64, 9];
        self.set(bell, state(d, "minecraft:bell", &[]));
        self.bes.on_event(bell, 1, 2, 0);
        self.set([8, 64, 12], state(d, "minecraft:player_head", &[("powered", "true")]));
        self.set([14, 64, 14], state(d, "minecraft:end_gateway", &[]));
        self.set([16, 64, 16], state(d, "minecraft:beacon", &[]));
    }

    /// Waterlogged conduit in a water cube, with a full prismarine frame: it turns active.
    fn place_conduit(&mut self, d: &VersionData) {
        let c = [40, 80, 40];
        let water = state(d, "minecraft:water", &[("level", "0")]);
        let prismarine = state(d, "minecraft:prismarine", &[]);
        for ox in -2i32..=2 {
            for oy in -2i32..=2 {
                for oz in -2i32..=2 {
                    let (ax, ay, az) = (ox.abs(), oy.abs(), oz.abs());
                    let p = [c[0] + ox, c[1] + oy, c[2] + oz];
                    if ax <= 1 && ay <= 1 && az <= 1 {
                        self.world.insert(p, water);
                    } else if (ox == 0 && (ay == 2 || az == 2))
                        || (oy == 0 && (ax == 2 || az == 2))
                        || (oz == 0 && (ax == 2 || ay == 2))
                    {
                        self.world.insert(p, prismarine);
                    }
                }
            }
        }
        self.set(c, state(d, "minecraft:conduit", &[("waterlogged", "true")]));
    }

    /// State changes of tick `t`, all through `on_block_change`.
    fn script(&mut self, d: &VersionData, t: i64) {
        if t % 50 == 25 {
            for (n, &(pos, ominous)) in self.trial_spawners.clone().iter().enumerate() {
                let st = TRIAL_STATES[(n / 2 + (t / 50) as usize + 1) % 6];
                let value = if ominous { "true" } else { "false" };
                self.set(pos, state(d, "minecraft:trial_spawner", &[("trial_spawner_state", st), ("ominous", value)]));
            }
        }
        if t == 100 || t == 250 {
            for (i, &(pos, name, signal, facing)) in self.campfires.clone().iter().enumerate() {
                let lit = (i % 2 == 0) != (t == 100);
                self.set(pos, campfire_state(d, name, lit, signal, facing));
            }
        }
        if t == 150 {
            self.set(self.vault, state(d, "minecraft:vault", &[("facing", "west"), ("ominous", "true")]));
        }
        if t == 200 {
            for &(pos, st) in &self.sulfur.clone() {
                let next = if st == "dry" { "wet" } else { "dry" };
                self.set(pos, state(d, "minecraft:potent_sulfur", &[("potent_sulfur_state", next)]));
            }
        }
        if t == 60 {
            self.set(self.chest, self.air);
        }
        if t == 80 {
            // Replaced at once: the new spawner must tick once, not twice.
            let spawner = self.world[&self.spawner];
            self.set(self.spawner, self.air);
            self.set(self.spawner, spawner);
            self.entity(self.spawner).data = Data::Spawner(Some("minecraft:cow".into()), 16, 5, 100);
        }
        if t == 120 {
            self.bes.forget_chunk(1, 2);
        }
        if t == 130 {
            // A ticking entity in a column that had none: the walk must find it.
            let pos = [200, 64, 200];
            self.set(pos, state(d, "minecraft:ender_chest", &[]));
            self.bes.on_event(pos, 1, 1, 1130);
        }
        if t == 10 || t == 300 {
            self.start_movers(d);
        }
    }

    /// Four moving stones, created through the block change then the mover data.
    fn start_movers(&mut self, d: &VersionData) {
        let moving = state(d, "minecraft:moving_piston", &[]);
        let stone = state(d, "minecraft:stone", &[]);
        for pos in [[3, 90, 3], [40, 90, 3], [3, 90, 40], [20, 90, 20]] {
            self.set(pos, moving);
            self.bes.set_mover(pos, rc_sim::MovingBlock { moved: stone, dir: 5, extending: true, source: false });
        }
    }

    fn tick(&mut self, t: i64) -> TickRecord {
        let world = &self.world;
        let air = self.air;
        let state = |x: i32, y: i32, z: i32| world.get(&[x, y, z]).copied().unwrap_or(air);
        let surface = |_: i32, _: i32| 72;
        let w = TickWorld {
            state: &state,
            surface: &surface,
            players: &[PLAYER],
            named: &[(PLAYER_ID, PLAYER)],
            game_time: 1000 + t,
            min_y: -64,
        };
        let mut out = Vec::new();
        self.bes.tick(&w, &mut out);
        let mut steps = Vec::new();
        self.bes.tick_pistons(&mut steps);
        let sounds = std::mem::take(&mut self.bes.sounds);
        for step in &steps {
            if let PistonTick::Finish(pos, s) = *step {
                self.set(pos, s);
            }
        }
        let mut entities: Vec<(&[i32; 3], &BlockEntity)> = self.bes.chunks.values().flat_map(|m| m.iter()).collect();
        entities.sort_by_key(|(p, _)| **p);
        TickRecord {
            particles: out
                .iter()
                .map(|(k, p, v)| format!("{k:?} {:?} {:?}", p.map(f64::to_bits), v.map(f64::to_bits)))
                .collect(),
            sounds: sounds
                .iter()
                .map(|(n, p, v, h)| format!("{n} {:?} {} {}", p.map(f64::to_bits), v.to_bits(), h.to_bits()))
                .collect(),
            pistons: steps.iter().map(|s| format!("{s:?}")).collect(),
            moving: self.bes.moving,
            version: self.bes.version,
            entities: entities.iter().map(|(p, e)| entity_text(p, e)).collect(),
        }
    }
}

fn campfire_state(d: &VersionData, name: &str, lit: bool, signal: bool, facing: &str) -> u32 {
    let flag = |b: bool| if b { "true" } else { "false" };
    state(d, name, &[("lit", flag(lit)), ("signal_fire", flag(signal)), ("facing", facing), ("waterlogged", "false")])
}

fn entity_text(pos: &[i32; 3], e: &BlockEntity) -> String {
    let mut anim = e.anim.clone();
    if !e.kind.ticks_on_client() {
        // The full walk also seeded the entities without a tick (signs, banners…); nothing reads
        // that seed outside the tick, the ticker list leaves it at zero.
        anim.seed = 0;
    }
    format!("{pos:?} {:?} {} {} {} {:?} {:?} {} {anim:?}", e.kind, e.sub, e.wall, e.state, e.props, e.data, e.rev)
}

/// The scripted world run for 400 ticks; `ordered`: keep the emission order (otherwise each tick's
/// lists are sorted, for a comparison with the walk of the hash maps, whose order is random).
fn run(d: &Arc<VersionData>, sw: Switches, ordered: bool) -> Vec<TickRecord> {
    let mut scene = Scene::new(d, sw);
    (0..400)
        .map(|t| {
            scene.script(d, t);
            let mut r = scene.tick(t);
            if !ordered {
                r.particles.sort();
                r.sounds.sort();
                r.pistons.sort();
            }
            r
        })
        .collect()
}

fn assert_same_runs(a: Switches, b: Switches, ordered: bool) {
    let d = version();
    let (ra, rb) = (run(&d, a, ordered), run(&d, b, ordered));
    let particles: usize = ra.iter().map(|r| r.particles.len()).sum();
    let sounds: usize = ra.iter().map(|r| r.sounds.len()).sum();
    let pistons: usize = ra.iter().map(|r| r.pistons.len()).sum();
    // The scene must exercise what it compares.
    assert!(particles > 2000 && sounds > 5 && pistons > 20, "{particles} particles, {sounds} sounds, {pistons} steps");
    for (t, (x, y)) in ra.iter().zip(&rb).enumerate() {
        assert_eq!(x, y, "tick {t}: {a:?} against {b:?}");
    }
}

/// Props read from the entity give the same particles, sounds and animations, in the same order,
/// as the state re-read from the world, through every state change of the script.
#[test]
fn tick_props_match_world_reread() {
    assert_same_runs(ORDERED, Switches { props_in_tick: false, ..ORDERED }, true);
}

/// The walk by ticking columns (with the moving pistons' list) gives, tick by tick, the same
/// particles, sounds, piston steps, moving count and entity states as walking every entity
/// (orders compared sorted: both follow the hash maps).
#[test]
fn ticking_columns_match_full_walk() {
    assert_same_runs(DEFAULTS, Switches { ticker_list: false, ..DEFAULTS }, false);
}

/// Same for the walk of every ticking entity in insertion order.
#[test]
fn ordered_ticker_list_matches_full_walk() {
    assert_same_runs(ORDERED, Switches { ticker_list: false, ..DEFAULTS }, false);
}

/// The early exit of block changes keeps every run identical (eager path for the whole script).
#[test]
fn early_exit_run_matches_eager_run() {
    assert_same_runs(ORDERED, Switches { change_early_exit: false, ..ORDERED }, true);
}

/// Conduit water by table: same activity as by fluid name.
#[test]
fn conduit_water_table_matches_names() {
    let d = version();
    let conduit = run(&d, ORDERED, true);
    let active = conduit.last().unwrap().entities.iter().any(|e| e.contains("Conduit") && e.contains("active: true"));
    assert!(active, "the scripted conduit must turn active");
    assert_same_runs(ORDERED, Switches { conduit_water_table: false, ..ORDERED }, true);
}

/// Fidelity (not equivalence): moving blocks push in the order they were created, like the game's
/// ticker list; the walk of the hash maps gave a random order.
#[test]
fn pistons_push_in_creation_order() {
    let d = version();
    let mut bes = BlockEntities::new(d.clone());
    bes.switches = DEFAULTS;
    let moving = state(&d, "minecraft:moving_piston", &[]);
    let stone = state(&d, "minecraft:stone", &[]);
    let air = state(&d, "minecraft:air", &[]);
    let order =
        [[50, 64, 3], [-7, 64, 90], [3, 64, 3], [200, 64, -40], [17, 64, 17], [-90, 64, -90], [4, 64, 3], [33, 64, 1]];
    for pos in order {
        bes.on_block_change(pos, air, moving);
        bes.set_mover(pos, rc_sim::MovingBlock { moved: stone, dir: 1, extending: true, source: false });
    }
    let mut steps = Vec::new();
    bes.tick_pistons(&mut steps);
    let pushed: Vec<[i32; 3]> = steps
        .iter()
        .map(|s| match s {
            PistonTick::Push(p, ..) | PistonTick::Finish(p, _) => *p,
        })
        .collect();
    assert_eq!(pushed, order);
}

/// Small deterministic generator for the random block changes.
struct Xorshift(u64);

impl Xorshift {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

/// States drawn by the random changes: a quarter carriers, some copper chests and statues
/// (entity kept across blocks), moving pistons, the rest anything in the registry.
struct StatePool {
    carriers: Vec<u32>,
    copper: Vec<u32>,
    moving: u32,
    count: usize,
}

impl StatePool {
    fn new(d: &VersionData, bes: &BlockEntities) -> Self {
        let mut carriers = Vec::new();
        let mut copper = Vec::new();
        for (s, info) in d.states.iter().enumerate() {
            let Some(c) = bes.tables.carriers[usize::from(info.block)] else { continue };
            carriers.push(s as u32);
            if keeps_entity_across_blocks(c) {
                copper.push(s as u32);
            }
        }
        StatePool { carriers, copper, moving: state(d, "minecraft:moving_piston", &[]), count: d.states.len() }
    }

    fn draw(&self, rng: &mut Xorshift) -> u32 {
        match rng.below(20) {
            0..=4 => self.carriers[rng.below(self.carriers.len())],
            5..=7 => self.copper[rng.below(self.copper.len())],
            8 => self.moving,
            _ => rng.below(self.count) as u32,
        }
    }
}

/// Everything a block change may touch: columns, entities, version, head requests.
fn snapshot(bes: &BlockEntities) -> (Vec<String>, u64, usize) {
    let mut columns: Vec<_> = bes.chunks.iter().collect();
    columns.sort_by_key(|(k, _)| **k);
    let mut out = Vec::new();
    for (k, map) in columns {
        let mut list: Vec<_> = map.iter().collect();
        list.sort_by_key(|(p, _)| **p);
        out.push(format!("column {k:?}"));
        out.extend(
            list.iter().map(|(p, e)| format!("{p:?} {:?} {} {} {} {:?}", e.kind, e.sub, e.wall, e.state, e.props)),
        );
    }
    (out, bes.version, bes.heads.len())
}

/// Random block changes (positions in four columns, old state usually the world's) applied to the
/// early-exit path and to the eager one: same columns, entities, version, and ticker lists.
#[test]
fn early_exit_matches_eager_on_random_changes() {
    let d = version();
    let mut lazy = BlockEntities::new(d.clone());
    lazy.switches = ORDERED;
    let mut eager = BlockEntities::new(d.clone());
    eager.switches = Switches { change_early_exit: false, ..ORDERED };
    let pool = StatePool::new(&d, &lazy);
    let air = state(&d, "minecraft:air", &[]);
    let mut world: HashMap<[i32; 3], u32> = HashMap::new();
    let mut rng = Xorshift(0x9E37_79B9_7F4A_7C15);
    let (mut removed, mut kept) = (0, 0);
    for step in 0..20_000 {
        let pos = [rng.below(4) as i32 * 9 - 9, 64, rng.below(4) as i32 * 9 - 9];
        let world_old = world.get(&pos).copied().unwrap_or(air);
        let old = if rng.below(10) == 0 { pool.draw(&mut rng) } else { world_old };
        let new = if rng.below(10) == 0 { old } else { pool.draw(&mut rng) };
        let before = lazy.get(pos).map(|e| e.ticket);
        lazy.on_block_change(pos, old, new);
        eager.on_block_change(pos, old, new);
        match (before, lazy.get(pos).map(|e| e.ticket)) {
            (Some(a), Some(b)) if a == b => kept += 1,
            (Some(_), None) => removed += 1,
            _ => {}
        }
        world.insert(pos, new);
        assert_eq!(snapshot(&lazy), snapshot(&eager), "step {step}: {pos:?} {old} -> {new}");
    }
    assert!(removed > 500 && kept > 500, "{removed} removals, {kept} kept entities");
    let w = TickWorld { state: &|_, _, _| air, surface: &|_, _| 0, players: &[], named: &[], game_time: 0, min_y: -64 };
    lazy.tick(&w, &mut Vec::new());
    eager.tick(&w, &mut Vec::new());
    assert_eq!((&lazy.lists.tickers, &lazy.lists.movers), (&eager.lists.tickers, &eager.lists.movers));
    assert_eq!(lazy.moving, eager.moving);
}

/// One entity of state `s` created through a block change at `pos`, with `data`; particles of
/// `ticks` ticks (game time from 0), the player at `PLAYER`.
fn particles_of(d: &Arc<VersionData>, pos: [i32; 3], s: u32, data: Data, ticks: i64) -> Vec<Emit> {
    let mut bes = BlockEntities::new(d.clone());
    bes.switches = DEFAULTS;
    bes.on_block_change(pos, state(d, "minecraft:air", &[]), s);
    let e = bes.get_mut(pos).expect("entity");
    e.data = data;
    e.anim.vault_players = vec![PLAYER_ID];
    e.anim.vault_range = 4.5;
    let lookup = move |x: i32, y: i32, z: i32| if [x, y, z] == pos { s } else { 0 };
    let mut out = Vec::new();
    for t in 0..ticks {
        let w = TickWorld {
            state: &lookup,
            surface: &|_, _| 0,
            players: &[PLAYER],
            named: &[(PLAYER_ID, PLAYER)],
            game_time: t,
            min_y: -64,
        };
        bes.tick(&w, &mut out);
    }
    out
}

/// The vault's connection particles start at the keyhole, on the side it faces (offsets of the
/// version that matched the facing by name).
#[test]
fn vault_keyhole_follows_facing() {
    let d = version();
    let item = Data::Vault(Some(Stack { item: 1, count: 1 }));
    for (facing, keyhole) in [
        ("north", [10.5, 65.75, 12.0]),
        ("south", [10.5, 65.75, 13.0]),
        ("west", [10.0, 65.75, 12.5]),
        ("east", [11.0, 65.75, 12.5]),
    ] {
        let s = state(&d, "minecraft:vault", &[("facing", facing), ("ominous", "false")]);
        let out = particles_of(&d, [10, 64, 12], s, item.clone(), 1);
        let from: Vec<[f64; 3]> =
            out.iter().filter(|p| p.0 == crate::particles::Kind::VaultConnection).map(|p| p.1).collect();
        assert!(!from.is_empty() && from.iter().all(|p| *p == keyhole), "{facing}: {from:?}");
    }
}

/// The smoke of the food in slot 0 rises over the corner the campfire's facing turns it to
/// (corners of the version that matched the facing by name).
#[test]
fn campfire_food_smoke_follows_facing() {
    let d = version();
    let food = Data::Items(vec![Some(Stack { item: 1, count: 1 }), None, None, None], false);
    for (facing, corner) in [
        ("south", [0.1875, 0.1875]),
        ("west", [0.8125, 0.1875]),
        ("north", [0.8125, 0.8125]),
        ("east", [0.1875, 0.8125]),
    ] {
        let s = campfire_state(&d, "minecraft:campfire", true, false, facing);
        let out = particles_of(&d, [0, 64, 0], s, food.clone(), 200);
        let corners: Vec<[f64; 2]> =
            out.iter().filter(|p| p.2 == [0.0, 5.0e-4, 0.0]).map(|p| [p.1[0], p.1[2]]).collect();
        assert!(!corners.is_empty() && corners.iter().all(|c| *c == corner), "{facing}: {corners:?}");
    }
}

/// Campfire smoke: none unlit, signal smoke over a hay bale, cosy smoke otherwise.
#[test]
fn campfire_smoke_follows_lit_and_signal() {
    use crate::particles::Kind as P;
    let d = version();
    let kinds = |lit: bool, signal: bool| {
        let s = campfire_state(&d, "minecraft:campfire", lit, signal, "north");
        let mut k: Vec<P> = particles_of(&d, [0, 64, 0], s, Data::None, 200).iter().map(|p| p.0).collect();
        k.dedup();
        k
    };
    assert_eq!(kinds(false, true), []);
    assert_eq!(kinds(true, true), [P::CampfireSignalSmoke]);
    assert_eq!(kinds(true, false), [P::CampfireCosySmoke]);
}

/// Particle kinds of each trial spawner state (as when the state was matched by name).
#[test]
fn trial_spawner_particles_follow_state() {
    use crate::particles::Kind as P;
    let d = version();
    let expected: [(&str, bool, &[P]); 7] = [
        ("inactive", false, &[]),
        ("waiting_for_players", false, &[P::SmallFlame]),
        ("waiting_for_reward_ejection", false, &[P::SmallFlame]),
        ("ejecting_reward", false, &[P::SmallFlame]),
        ("active", false, &[P::Flame, P::Smoke]),
        ("active", true, &[P::Smoke, P::SoulFireFlame]),
        ("cooldown", false, &[P::Smoke]),
    ];
    for (st, ominous, kinds) in expected {
        let value = if ominous { "true" } else { "false" };
        let s = state(&d, "minecraft:trial_spawner", &[("trial_spawner_state", st), ("ominous", value)]);
        let mut got: Vec<String> =
            particles_of(&d, [0, 64, 0], s, Data::Trial(None, 0), 100).iter().map(|p| format!("{:?}", p.0)).collect();
        got.sort();
        got.dedup();
        let want: Vec<String> = kinds.iter().map(|k| format!("{k:?}")).collect();
        assert_eq!(got, want, "{st} ominous {ominous}");
    }
}

/// The extended properties fit in the size the structure already had.
#[test]
fn props_stay_compact() {
    assert_eq!(std::mem::size_of::<Props>(), 24);
}

/// The dead entries left by reloaded columns are dropped while the tick is frozen.
#[test]
fn ticker_lists_stay_bounded_without_tick() {
    let d = version();
    let mut bes = BlockEntities::new(d.clone());
    bes.switches = ORDERED;
    let chest = state(&d, "minecraft:chest", &[]);
    let list: Vec<rc_proto_v777::chunk::BlockEntity> =
        (0..16).map(|x| rc_proto_v777::chunk::BlockEntity { x: x as u8, y: 64, z: 0, kind: -1, data: None }).collect();
    for _ in 0..1000 {
        bes.load_chunk(0, 0, &list, |_, _, _| chest);
    }
    let listed = bes.lists.tickers.len();
    assert!((16..=2 * 16 + 1024 + 16).contains(&listed), "{listed}");
}

/// Micro-bench (ignored): 256 signs + 64 lit campfires + 16 active trial spawners, 2,000 ticks, for
/// each walk (every entity, ticking columns, ordered list) with and without the props; then 512
/// chests, where every entity ticks (worst case of the ordered list: one lookup more per entity).
/// One core: the tick is serial.
#[test]
#[ignore]
fn bench_block_entity_tick() {
    let d = version();
    let mut world = HashMap::new();
    let sign = state(&d, "minecraft:oak_sign", &[]);
    let campfire = campfire_state(&d, "minecraft:campfire", true, false, "north");
    let spawner = state(&d, "minecraft:trial_spawner", &[("trial_spawner_state", "active"), ("ominous", "false")]);
    for i in 0..256 {
        world.insert([i % 64, 64, i / 64 * 4], sign);
    }
    for i in 0..64 {
        world.insert([i * 2, 66, 40], campfire);
    }
    for i in 0..16 {
        world.insert([i * 3, 68, 70], spawner);
    }
    bench_tick_of(&d, &world, "signs, campfires, trial spawners");
    let chest = state(&d, "minecraft:chest", &[]);
    let chests: HashMap<[i32; 3], u32> = (0..512).map(|i| ([i % 64 * 2, 64, i / 64 * 2], chest)).collect();
    bench_tick_of(&d, &chests, "chests");
}

fn bench_tick_of(d: &Arc<VersionData>, world: &HashMap<[i32; 3], u32>, what: &str) {
    let air = state(d, "minecraft:air", &[]);
    let lookup = |x: i32, y: i32, z: i32| world.get(&[x, y, z]).copied().unwrap_or(air);
    for pass in 0..2 {
        let walks = [("every entity", false, false), ("ticking columns", true, false), ("ordered list", true, true)];
        for (walk, props) in walks.into_iter().flat_map(|w| [(w, false), (w, true)]) {
            let (walk, ticker_list, ticker_order) = walk;
            let mut bes = BlockEntities::new(d.clone());
            bes.switches = Switches { props_in_tick: props, ticker_list, ticker_order, ..DEFAULTS };
            for (&pos, &s) in world {
                bes.on_block_change(pos, air, s);
                match bes.get_mut(pos).map(|e| e.kind) {
                    Some(Kind::Campfire) => bes.get_mut(pos).unwrap().data = Data::Items(vec![None; 4], false),
                    Some(Kind::TrialSpawner) => bes.get_mut(pos).unwrap().data = Data::Trial(None, 0),
                    _ => {}
                }
            }
            let mut out = Vec::new();
            let t0 = std::time::Instant::now();
            for t in 0..2000 {
                let w = TickWorld {
                    state: &lookup,
                    surface: &|_, _| 0,
                    players: &[],
                    named: &[],
                    game_time: t,
                    min_y: -64,
                };
                out.clear();
                bes.tick(&w, &mut out);
                bes.sounds.clear();
            }
            let us = t0.elapsed().as_secs_f64() * 1.0e6 / 2000.0;
            eprintln!(
                "{what}, pass {pass}: walk {walk}, props in tick {props}: {us:.2} us per tick ({} entities)",
                world.len()
            );
        }
    }
}

/// Micro-bench (ignored): 10^6 mixed block changes (nine in ten without an entity: wire power,
/// stone and air; the rest carriers), early exit against the eager path.
#[test]
#[ignore]
fn bench_on_block_change() {
    let d = version();
    let wires: Vec<u32> = (0..16).map(|p| state(&d, "minecraft:redstone_wire", &[("power", &p.to_string())])).collect();
    let stone = state(&d, "minecraft:stone", &[]);
    let air = state(&d, "minecraft:air", &[]);
    let chests: Vec<u32> =
        FACINGS.iter().map(|f| state(&d, "minecraft:chest", &[("facing", f), ("type", "single")])).collect();
    let sign = state(&d, "minecraft:oak_sign", &[]);
    let mut rng = Xorshift(0x2545_F491_4F6C_DD1D);
    let mut world: HashMap<[i32; 3], u32> = HashMap::new();
    let changes: Vec<([i32; 3], u32, u32)> = (0..1_000_000)
        .map(|_| {
            let pos = [rng.below(256) as i32 - 128, 64, rng.below(256) as i32 - 128];
            let new = match rng.below(20) {
                0..=13 => wires[rng.below(16)],
                14..=17 => [stone, air][rng.below(2)],
                18 => chests[rng.below(4)],
                _ => sign,
            };
            let old = world.insert(pos, new).unwrap_or(air);
            (pos, old, new)
        })
        .collect();
    for pass in 0..2 {
        for early in [false, true] {
            let mut bes = BlockEntities::new(d.clone());
            bes.switches = Switches { change_early_exit: early, ..DEFAULTS };
            let t0 = std::time::Instant::now();
            for &(pos, old, new) in &changes {
                bes.on_block_change(pos, old, new);
            }
            let ns = t0.elapsed().as_secs_f64() * 1.0e9 / changes.len() as f64;
            eprintln!("pass {pass}: early exit {early}: {ns:.1} ns per change ({} entities left)", bes.len());
        }
    }
}
