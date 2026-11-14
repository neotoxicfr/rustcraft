//! The client's placement prediction reads the server's own placement rules. On a flat world,
//! each click below is first predicted through the client world trait, then placed by the
//! server: the cell, the placed state and the second block must be the server's, and a few
//! states are pinned to their expected value. Before, the client placed only full single-state
//! blocks by itself and had no placement state for stairs, slabs, doors, wall torches, fences…

use std::sync::Arc;

use crate::blocks::Dir;
use crate::blocks::probe::{Probe, state_string};
use crate::blocks::rules::ShapeRules;
use crate::placement_prediction::{BlockClick, ClientWorld, PlacementPredictor, PlacingPlayer};
use crate::player::Stack;
use crate::server::Server;

/// The server's world, read like the client holds it.
struct ServerWorldSeenByClient<'a>(&'a Server);

impl ClientWorld for ServerWorldSeenByClient<'_> {
    fn block_state(&self, p: [i32; 3]) -> u32 {
        self.0.get(p)
    }
    fn block_tag(&self, name: &str, block: u16) -> bool {
        self.0.tag(name, block)
    }
    fn raw_brightness(&self, p: [i32; 3]) -> i32 {
        self.0.raw_brightness(p)
    }
    fn min_y(&self) -> i32 {
        self.0.dim.height.min_y
    }
    fn height(&self) -> i32 {
        self.0.dim.height.sections as i32 * 16
    }
    fn border_bounds(&self) -> [f64; 4] {
        self.0.border_bounds()
    }
    fn obstructed(&self, state: u32, p: [i32; 3]) -> bool {
        self.0.placement_obstructed(state, p)
    }
}

/// Flat world (grass top at y = -61), fixed seed, no structures; the player far from the cases.
fn open(tag: &str) -> Option<(Probe, std::path::PathBuf)> {
    let data = Arc::new(rc_testdata::version_data()?);
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-placement-prediction-{tag}-{}", std::process::id())));
    let mut w = crate::newworld::NewWorld::new("placement-prediction");
    w.preset = crate::newworld::Preset::Flat;
    w.seed = 0;
    w.generate_structures = false;
    crate::newworld::create_world(&dir, &data, &w).expect("world creation");
    let mut p = Probe::open(&dir, data);
    p.s.loaded = true;
    p.s.teleport = None;
    p.s.game_mode = 1;
    let around: Vec<(i32, i32)> = (-2..=8).flat_map(|x| (-2..=2).map(move |z| (x, z))).collect();
    p.s.load_now(&around);
    p.s.pos = [0.5, -60.0, -20.5];
    Some((p, dir))
}

/// One click: setup commands, item, clicked block and face, clicked point (offsets in the block),
/// player's yaw, pitch and sneak key; the expected placed state when pinned (`None` for "the
/// server's, whatever it is"); `"refused"` when nothing may be placed.
struct Case {
    setup: &'static str,
    item: &'static str,
    clicked: [i32; 3],
    face: Dir,
    offset: [f64; 3],
    yaw: f32,
    pitch: f32,
    sneaking: bool,
    expect: Option<&'static str>,
}

const GROUND: i32 = -61;
const AIR: i32 = -60;

fn on_ground(x: i32, item: &'static str, yaw: f32, expect: Option<&'static str>) -> Case {
    Case { setup: "", item, clicked: [x, GROUND, 0], face: Dir::Up, offset: [0.5, 1.0, 0.5], yaw, pitch: 30.0, sneaking: false, expect }
}

fn cases() -> Vec<Case> {
    let mut c = vec![
        on_ground(2, "minecraft:oak_stairs", 0.0, Some("minecraft:oak_stairs[facing=south,half=bottom,shape=straight,waterlogged=false]")),
        on_ground(4, "minecraft:oak_slab", 0.0, Some("minecraft:oak_slab[type=bottom,waterlogged=false]")),
        on_ground(6, "minecraft:oak_door", 90.0, Some("minecraft:oak_door[facing=west,half=lower,hinge=left,open=false,powered=false]")),
        on_ground(8, "minecraft:torch", 0.0, Some("minecraft:torch")),
        on_ground(10, "minecraft:red_bed", 0.0, Some("minecraft:red_bed[facing=south,occupied=false,part=foot]")),
        on_ground(14, "minecraft:carrot", 0.0, Some("refused")),
        on_ground(16, "minecraft:redstone", 0.0, None),
        on_ground(18, "minecraft:tall_grass", 0.0, None),
        on_ground(20, "minecraft:scaffolding", 0.0, None),
        on_ground(22, "minecraft:powder_snow_bucket", 0.0, Some("minecraft:powder_snow")),
        on_ground(24, "minecraft:repeater", 45.0, None),
        on_ground(26, "minecraft:rail", 100.0, None),
        on_ground(28, "minecraft:anvil", 200.0, None),
        on_ground(30, "minecraft:oak_sign", 33.0, None),
        on_ground(32, "minecraft:white_banner", 300.0, None),
        on_ground(34, "minecraft:skeleton_skull", 12.0, None),
        on_ground(36, "minecraft:oak_leaves", 0.0, None),
        on_ground(38, "minecraft:candle", 0.0, None),
        on_ground(40, "minecraft:stone_button", 0.0, None),
        on_ground(42, "minecraft:lever", 270.0, None),
        on_ground(44, "minecraft:observer", 0.0, None),
        on_ground(46, "minecraft:piston", 0.0, None),
        on_ground(48, "minecraft:end_rod", 0.0, None),
        on_ground(50, "minecraft:lantern", 0.0, None),
        on_ground(52, "minecraft:oak_trapdoor", 0.0, None),
        on_ground(54, "minecraft:oak_fence_gate", 90.0, None),
        on_ground(56, "minecraft:cobblestone_wall", 0.0, None),
        on_ground(58, "minecraft:glass_pane", 0.0, None),
        on_ground(60, "minecraft:snow", 0.0, None),
        on_ground(62, "minecraft:sea_pickle", 0.0, None),
    ];
    let side = |x: i32, setup: &'static str, item: &'static str, face: Dir, offset: [f64; 3], yaw: f32, expect: Option<&'static str>| {
        Case { setup, item, clicked: [x, AIR, 0], face, offset, yaw, pitch: 0.0, sneaking: false, expect }
    };
    c.extend([
        // Upper half of a block's side: a slab goes on top, a trapdoor hangs at the top.
        side(64, "setblock 64 -60 0 minecraft:stone", "minecraft:oak_slab", Dir::East, [1.0, 0.75, 0.5], 90.0, Some("minecraft:oak_slab[type=top,waterlogged=false]")),
        side(66, "setblock 66 -60 0 minecraft:stone", "minecraft:oak_trapdoor", Dir::East, [1.0, 0.75, 0.5], 90.0, None),
        side(68, "setblock 68 -60 0 minecraft:stone", "minecraft:torch", Dir::East, [1.0, 0.5, 0.5], 90.0, Some("minecraft:wall_torch[facing=east]")),
        side(70, "setblock 70 -60 0 minecraft:stone", "minecraft:oak_sign", Dir::East, [1.0, 0.5, 0.5], 90.0, None),
        side(72, "setblock 72 -60 0 minecraft:stone", "minecraft:lever", Dir::East, [1.0, 0.5, 0.5], 90.0, None),
        side(74, "setblock 74 -60 0 minecraft:stone", "minecraft:white_banner", Dir::East, [1.0, 0.5, 0.5], 90.0, None),
        side(76, "setblock 76 -60 0 minecraft:stone", "minecraft:glow_lichen", Dir::East, [1.0, 0.5, 0.5], 90.0, None),
        side(78, "setblock 78 -60 0 minecraft:stone", "minecraft:ladder", Dir::East, [1.0, 0.5, 0.5], 90.0, None),
        side(80, "setblock 80 -60 0 minecraft:stone", "minecraft:bell", Dir::East, [1.0, 0.5, 0.5], 90.0, None),
        side(82, "setblock 82 -60 0 minecraft:jungle_log", "minecraft:cocoa_beans", Dir::East, [1.0, 0.5, 0.5], 90.0, None),
        // A bottom slab clicked on its top: double.
        side(96, "setblock 96 -60 0 minecraft:oak_slab", "minecraft:oak_slab", Dir::Up, [0.5, 0.5, 0.5], 0.0, Some("minecraft:oak_slab[type=double,waterlogged=false]")),
    ]);
    let next_to = |x: i32, setup: &'static str, item: &'static str, yaw: f32, expect: Option<&'static str>| Case { setup, ..on_ground(x, item, yaw, expect) };
    c.extend([
        // Against blocks of its kind: a fence connects, stairs turn a corner, chests join,
        // sea pickles stack (not while sneaking).
        next_to(84, "setblock 85 -60 0 minecraft:oak_fence", "minecraft:oak_fence", 0.0, Some("minecraft:oak_fence[east=true,north=false,south=false,waterlogged=false,west=false]")),
        next_to(88, "setblock 88 -60 1 minecraft:oak_stairs[facing=east]", "minecraft:oak_stairs", 0.0, None),
        next_to(92, "setblock 93 -60 0 minecraft:chest[facing=south]", "minecraft:chest", 180.0, None),
        Case { setup: "setblock 98 -60 0 minecraft:sea_pickle", item: "minecraft:sea_pickle", clicked: [98, AIR, 0], face: Dir::Up, offset: [0.5, 0.375, 0.5], yaw: 0.0, pitch: 30.0, sneaking: false, expect: Some("minecraft:sea_pickle[pickles=2,waterlogged=true]") },
    ]);
    c.extend([
        // A lantern and a hanging sign under a block (looking up), pointed dripstone below it.
        Case { setup: "setblock 100 -58 0 minecraft:stone", item: "minecraft:lantern", clicked: [100, -58, 0], face: Dir::Down, offset: [0.5, 0.0, 0.5], yaw: 0.0, pitch: -60.0, sneaking: false, expect: None },
        Case { setup: "setblock 102 -58 0 minecraft:stone", item: "minecraft:oak_hanging_sign", clicked: [102, -58, 0], face: Dir::Down, offset: [0.5, 0.0, 0.5], yaw: 10.0, pitch: -60.0, sneaking: false, expect: None },
        Case { setup: "setblock 104 -58 0 minecraft:stone", item: "minecraft:pointed_dripstone", clicked: [104, -58, 0], face: Dir::Down, offset: [0.5, 0.0, 0.5], yaw: 0.0, pitch: -60.0, sneaking: false, expect: None },
        // Seeds on farmland: crops; stems read their own fruit's support tag.
        Case { setup: "setblock 106 -61 0 minecraft:farmland", item: "minecraft:carrot", clicked: [106, GROUND, 0], face: Dir::Up, offset: [0.5, 1.0, 0.5], yaw: 0.0, pitch: 30.0, sneaking: false, expect: Some("minecraft:carrots[age=0]") },
        Case { setup: "setblock 108 -61 0 minecraft:farmland", item: "minecraft:pumpkin_seeds", clicked: [108, GROUND, 0], face: Dir::Up, offset: [0.5, 1.0, 0.5], yaw: 0.0, pitch: 30.0, sneaking: false, expect: Some("minecraft:pumpkin_stem[age=0]") },
        Case { setup: "setblock 110 -61 0 minecraft:farmland", item: "minecraft:melon_seeds", clicked: [110, GROUND, 0], face: Dir::Up, offset: [0.5, 1.0, 0.5], yaw: 0.0, pitch: 30.0, sneaking: false, expect: Some("minecraft:melon_stem[age=0]") },
        // Sneaking against a sea pickle: no second pickle, a new one above it.
        Case { setup: "setblock 112 -60 0 minecraft:sea_pickle", item: "minecraft:sea_pickle", clicked: [112, AIR, 0], face: Dir::Up, offset: [0.5, 0.375, 0.5], yaw: 0.0, pitch: 30.0, sneaking: true, expect: None },
        // Above the build height: nothing.
        Case { setup: "setblock 114 319 0 minecraft:stone", item: "minecraft:stone", clicked: [114, 319, 0], face: Dir::Up, offset: [0.5, 1.0, 0.5], yaw: 0.0, pitch: 30.0, sneaking: false, expect: Some("refused") },
    ]);
    c
}

/// A click predicted through the client world, then placed by the server: same blocks.
#[test]
fn predicted_placements_are_the_servers() {
    let Some((mut p, _dir)) = open("server") else { return };
    let predictor = PlacementPredictor::new(&p.s.data);
    let data = p.s.data.clone();
    for case in cases() {
        p.run(case.setup);
        p.s.yaw = case.yaw;
        p.s.pitch = case.pitch;
        p.s.sneaking = case.sneaking;
        p.s.hit_inside = false;
        p.s.slots[36 + p.s.held as usize] = Some(Stack { id: case.item.to_owned(), count: 64, components: None });
        let at = [0, 1, 2].map(|i| f64::from(case.clicked[i]) + case.offset[i]);
        let player = PlacingPlayer { yaw: case.yaw, pitch: case.pitch, sneaking: case.sneaking, hit_inside: false, game_master: false };
        let click = BlockClick { item: case.item, clicked: case.clicked, face: case.face as u8, at };
        let world = ServerWorldSeenByClient(&p.s);
        let predicted = predictor.predict(&data, &world, &player, &click);
        let second = predicted.and_then(|placed| predictor.second_block(&data, &world, placed.pos, placed.state));
        let placed_by_server = p.s.place_item(case.item, case.clicked, case.face, at);
        let what = format!("{} on {:?} {:?}", case.item, case.clicked, case.face);
        assert_eq!(predicted.is_some(), placed_by_server, "{what}: placed");
        if let Some(expect) = case.expect {
            let got = predicted.map_or_else(|| "refused".to_owned(), |placed| state_string(&p.s, placed.state));
            assert_eq!(got, expect, "{what}: pinned state");
        }
        let Some(placed) = predicted else { continue };
        assert_eq!(state_string(&p.s, placed.state), state_string(&p.s, p.s.get(placed.pos)), "{what}: state");
        if let Some((pos, state)) = second {
            assert_eq!(state_string(&p.s, state), state_string(&p.s, p.s.get(pos)), "{what}: second block");
        }
    }
}

/// Singleplayer: the client's prediction reads the very block table of the integrated server
/// (one `Arc` of version data for both), instead of a copy (`RUSTCRAFT_OPT_SHARED_BLOCK_TABLE`).
#[test]
fn the_prediction_shares_the_servers_block_table() {
    let Some((p, _dir)) = open("shared-table") else { return };
    let predictor = PlacementPredictor::new(&p.s.data);
    assert_eq!(
        Arc::ptr_eq(predictor.table(), &p.s.bs.t),
        rc_data::opt_switch!("RUSTCRAFT_OPT_SHARED_BLOCK_TABLE"),
        "one table for the server and the client"
    );
}

/// Cost of the client's copy of the block tables (built once at the client's start): time and
/// the size of its biggest arrays. A figure to read (`--ignored --nocapture`), not a check.
#[test]
#[ignore]
fn bench_placement_tables() {
    let Some(data) = rc_testdata::version_data() else { return };
    let mut times: Vec<f64> = (0..15)
        .map(|_| {
            let start = std::time::Instant::now();
            std::hint::black_box(crate::blocks::Table::new(&data));
            start.elapsed().as_secs_f64() * 1e3
        })
        .collect();
    times.sort_by(f64::total_cmp);
    let data = Arc::new(data);
    let start = std::time::Instant::now();
    let (first, second) = (PlacementPredictor::new(&data), PlacementPredictor::new(&data));
    let shared = start.elapsed().as_secs_f64() * 1e3;
    std::hint::black_box((first, second));
    let table = crate::blocks::Table::new(&data);
    let masks = table.face_masks.len() * std::mem::size_of::<[[u64; 4]; 6]>();
    let kinds = table.kind.len() * std::mem::size_of::<crate::blocks::Kind>();
    println!(
        "placement tables: one build median {:.2} ms (min {:.2}); two predictors on one shared table {shared:.2} ms; face masks {} KiB, kinds {} KiB",
        times[7],
        times[0],
        masks / 1024,
        kinds / 1024
    );
}

/// Cost of one predicted click (stairs beside stairs, a fence beside a fence, a door, a wall
/// torch) on the flat world. A figure to read (`--ignored --nocapture`), not a check.
#[test]
#[ignore]
fn bench_predicted_click() {
    let Some((mut p, _dir)) = open("bench-click") else { return };
    let predictor = PlacementPredictor::new(&p.s.data);
    p.run("setblock 3 -60 1 minecraft:oak_stairs[facing=east]\nsetblock 7 -60 0 minecraft:oak_fence\nsetblock 10 -60 0 minecraft:stone");
    let player = PlacingPlayer { yaw: 0.0, pitch: 30.0, sneaking: false, hit_inside: false, game_master: false };
    let clicks = [
        BlockClick { item: "minecraft:oak_stairs", clicked: [3, GROUND, 0], face: 1, at: [3.5, -60.0, 0.5] },
        BlockClick { item: "minecraft:oak_fence", clicked: [6, GROUND, 0], face: 1, at: [6.5, -60.0, 0.5] },
        BlockClick { item: "minecraft:oak_door", clicked: [8, GROUND, 0], face: 1, at: [8.5, -60.0, 0.5] },
        BlockClick { item: "minecraft:torch", clicked: [10, AIR, 0], face: 5, at: [11.0, -59.5, 0.5] },
    ];
    let world = ServerWorldSeenByClient(&p.s);
    for click in &clicks {
        assert!(predictor.predict(&p.s.data, &world, &player, click).is_some(), "{}", click.item);
        const ROUNDS: u32 = 20_000;
        let start = std::time::Instant::now();
        for _ in 0..ROUNDS {
            std::hint::black_box(predictor.predict(&p.s.data, &world, &player, std::hint::black_box(click)));
        }
        println!("predicted click {}: {:.0} ns", click.item, start.elapsed().as_secs_f64() * 1e9 / f64::from(ROUNDS));
    }
}

/// State written `minecraft:name[p=v,…]`.
fn state_of(s: &Server, text: &str) -> u32 {
    let (name, props) = text.split_once('[').map_or((text, ""), |(n, rest)| (n, rest.trim_end_matches(']')));
    let mut state = s.default_state(name);
    assert!(s.data.block_of(state).is_some_and(|b| b.name == name), "unknown block {name}");
    for (k, v) in props.split(',').filter_map(|kv| kv.split_once('=')) {
        state = s.view().set_named(state, k, v);
    }
    state
}

/// The second block placed with a door, a bed and a tall plant: the server's, and the small
/// dripleaf's top only on the server (the game's client does not place it).
#[test]
fn second_blocks_of_doors_beds_and_tall_plants() {
    let Some((p, _dir)) = open("second") else { return };
    let predictor = PlacementPredictor::new(&p.s.data);
    let world = ServerWorldSeenByClient(&p.s);
    let state = |text: &str| state_of(&p.s, text);
    let at = [2, AIR, 0];
    let door = state("minecraft:oak_door[facing=west,half=lower,hinge=left]");
    assert_eq!(
        predictor.second_block(&p.s.data, &world, at, door).map(|(q, s)| (q, state_string(&p.s, s))),
        Some(([2, AIR + 1, 0], "minecraft:oak_door[facing=west,half=upper,hinge=left,open=false,powered=false]".to_owned()))
    );
    let bed = state("minecraft:red_bed[facing=south]");
    assert_eq!(
        predictor.second_block(&p.s.data, &world, at, bed).map(|(q, s)| (q, state_string(&p.s, s))),
        Some(([2, AIR, 1], "minecraft:red_bed[facing=south,occupied=false,part=head]".to_owned()))
    );
    let dripleaf = state("minecraft:small_dripleaf[facing=north]");
    assert_eq!(predictor.second_block(&p.s.data, &world, at, dripleaf), None, "small dripleaf: server only");
    assert_eq!(predictor.second_block(&p.s.data, &world, at, state("minecraft:stone")), None);
}

/// Stems read the support tag of their own fruit: with a data pack that takes farmland out of
/// `supports_melon_stem` only, a melon stem no longer holds on farmland, a pumpkin stem still
/// does. Before, both read the crops' tags (filled alike in the vanilla data).
#[test]
fn stems_need_their_fruits_support() {
    let Some((mut p, _dir)) = open("stems") else { return };
    p.run("setblock 2 -61 0 minecraft:farmland\nsetblock 4 -61 0 minecraft:stone");
    let (melon, pumpkin) = (state_of(&p.s, "minecraft:melon_stem"), state_of(&p.s, "minecraft:pumpkin_stem"));
    assert!(p.s.can_survive(melon, [2, AIR, 0]), "melon stem on farmland");
    assert!(!p.s.can_survive(melon, [4, AIR, 0]), "melon stem on stone");
    let farmland = u32::from(p.s.table().block_id("minecraft:farmland"));
    p.s.block_tags.get_mut("minecraft:supports_melon_stem").expect("melon stem tag").retain(|&b| b != farmland);
    p.s.tag_bits = crate::tag_bits::TagBits::new(&p.s.block_tags, &p.s.data);
    assert!(!p.s.can_survive(melon, [2, AIR, 0]), "melon stem: farmland out of its tag");
    assert!(p.s.can_survive(pumpkin, [2, AIR, 0]), "pumpkin stem: its own tag untouched");
}
