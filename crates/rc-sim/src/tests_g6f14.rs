//! G6-F14: game master blocks in singleplayer (structure block save, load and size detection,
//! jigsaw and test block settings, test block signal), live template placement (`place
//! template`: world folder, integrity on the game's generator, entities), copied command block,
//! border defaults, `forceload` and `worldborder add`. Fresh flat world: the 26.3 cache and the
//! client jar are required (their absence makes the tests FAIL).

use std::sync::Arc;

use rc_client::Event;
use rc_proto::nbt::Nbt;
use rc_proto::{BlockPos, Reader};
use rc_proto_v777::special::{StructureBlock, write_set_jigsaw_block, write_set_structure_block, write_set_test_block};

use crate::blocks::rules::*;
use crate::blocks::probe::{Probe, state_string};
use crate::commands::ents::E;
use crate::mobs::jrand::JRand;

const Y: i32 = -60;

struct W {
    p: Probe,
}

fn world(tag: &str) -> W {
    let data = Arc::new(rc_testdata::version_data().expect("G6-F14: 26.3 data cache missing"));
    assert!(crate::registries::client_jar(&data).is_some(), "G6-F14: 26.3 client jar missing");
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-g6f14-{tag}-{}", std::process::id())));
    let mut nw = crate::newworld::NewWorld::new(tag);
    nw.seed = 7;
    nw.preset = crate::newworld::Preset::Flat;
    nw.allow_commands = true;
    nw.generate_structures = false;
    crate::newworld::create_world(&dir, &data, &nw).expect("world creation");
    let mut p = Probe::open(&dir, data);
    let want: Vec<(i32, i32)> = (-2..=3).flat_map(|x| (-2..=3).map(move |z| (x, z))).collect();
    p.s.ensure_loaded(&want);
    let t = std::time::Instant::now();
    while !want.iter().all(|c| p.s.chunks.get(c).is_some_and(Option::is_some)) {
        assert!(t.elapsed().as_secs() < 300, "G6-F14: anti-hang guard: flat world generation stuck for 300 s");
        p.s.poll_gen();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    p.s.pos = [8.5, f64::from(Y), 8.5];
    p.s.game_mode = 1;
    let _ = p.take_events();
    W { p }
}

impl W {
    fn cmd(&mut self, text: &str) {
        let src = self.p.s.player_source();
        let (ok, _) = self.p.s.perform(src, text);
        assert!(ok, "command refused: {text}");
    }

    fn try_cmd(&mut self, text: &str) -> bool {
        let src = self.p.s.player_source();
        self.p.s.perform(src, text).0
    }

    fn name(&self, at: [i32; 3]) -> String {
        state_string(&self.p.s, self.p.s.get(at))
    }

    fn be(&self, at: [i32; 3]) -> Nbt {
        self.p.s.be_get(at).cloned().expect("block entity")
    }

    fn structure(&mut self, at: [i32; 3], s: &StructureBlock) -> Vec<String> {
        let mut w = Vec::new();
        write_set_structure_block(&mut w, BlockPos::new(at[0], at[1], at[2]), s);
        let id = self.p.s.peer.ids().set_structure_block;
        assert!(self.p.s.tech_packet(id, &mut Reader::new(&w)), "packet not consumed");
        self.keys()
    }

    /// Translation keys of the chat messages since the last read.
    fn keys(&mut self) -> Vec<String> {
        fn key(c: &rc_text::Component) -> Option<String> {
            match &c.content {
                rc_text::Content::Translate { key, .. } => Some(key.clone()),
                _ => c.extra.first().and_then(key),
            }
        }
        self.p
            .take_events()
            .iter()
            .filter_map(|e| match e {
                Event::Chat { component, .. } => key(component),
                _ => None,
            })
            .collect()
    }

    fn chat_text(&mut self) -> String {
        self.p
            .take_events()
            .iter()
            .filter_map(|e| match e {
                Event::Chat { component, .. } => Some(format!("{component:?}")),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn settings(action: i32, mode: i32, name: &str, offset: [i8; 3], size: [i8; 3], flags: u8) -> StructureBlock {
    StructureBlock {
        action,
        mode,
        name: name.into(),
        offset,
        size,
        mirror: 0,
        rotation: 0,
        data: String::new(),
        integrity: 1.0,
        seed: 0,
        flags,
    }
}

/// Saving an area writes `generated/<ns>/structure/<path>.nbt` (blocks in the game's order,
/// palette, block entities without position), and loading it by a structure block of the same
/// size rebuilds it elsewhere; another size only takes the template's size.
#[test]
fn structure_block_saves_and_loads_a_template() {
    let mut w = world("save-load");
    let (sb, load) = ([0, Y, 0], [0, Y, 8]);
    w.cmd(&format!("setblock {} {} {} minecraft:structure_block[mode=save]", sb[0], sb[1], sb[2]));
    assert_eq!(int_of(&w.be(sb), "posY"), 1, "default offset of a new structure block");
    w.cmd(&format!("fill 2 {Y} 0 4 {} 2 minecraft:stone", Y + 1));
    w.cmd(&format!("setblock 3 {} 1 minecraft:oak_stairs[facing=east]", Y + 1));
    w.cmd(&format!("setblock 2 {Y} 2 minecraft:chest{{Items:[{{Slot:0b,id:\"minecraft:apple\",count:3}}]}}"));
    let keys = w.structure(sb, &settings(1, 0, "g6f14:house", [2, 0, 0], [3, 2, 3], 1 | 4));
    assert!(keys.iter().any(|k| k == "structure_block.save_success"), "keys {keys:?}");
    let file = w.p.s.world.join("generated").join("g6f14").join("structure").join("house.nbt");
    let root = crate::nbtio::read_dat(&file).expect("template file in generated/<ns>/structure/");
    let blocks = crate::nbtio::list(root.get("blocks"));
    assert_eq!(blocks.len(), 18, "every cell of the 3×2×3 area, air included");
    // Full blocks first (by y, x, z), block entities last, without position.
    let last = blocks.last().expect("blocks");
    let nbt = last.get("nbt").expect("chest data");
    assert_eq!(nbt.get("id").and_then(Nbt::as_str), Some("minecraft:chest"));
    assert!(nbt.get("x").is_none(), "position removed from the block entity");
    let first_pos = crate::nbtio::list(blocks[0].get("pos")).to_vec();
    assert_eq!(first_pos, vec![Nbt::Int(0), Nbt::Int(0), Nbt::Int(0)], "first full block at the origin");
    assert_eq!(crate::nbtio::list(root.get("size")), &[Nbt::Int(3), Nbt::Int(2), Nbt::Int(3)]);
    assert_eq!(assert_current_palette_keys(&root), 2, "chest and stairs entries in the palette");
    // Loading at another structure block of the same size.
    w.cmd(&format!("setblock {} {} {} minecraft:structure_block[mode=load]", load[0], load[1], load[2]));
    let keys = w.structure(load, &settings(2, 1, "g6f14:house", [2, 0, 0], [3, 2, 3], 1 | 4));
    assert!(keys.iter().any(|k| k == "structure_block.load_success"), "keys {keys:?}");
    for (x, y, z) in [(0, 0, 0), (1, 1, 1), (2, 0, 2)] {
        let a = w.name([2 + x, Y + y, z]);
        let b = w.name([2 + x, Y + y, 8 + z]);
        assert_eq!(a, b, "cell {x} {y} {z} rebuilt");
    }
    let chest = w.be([2, Y, 10]);
    assert!(format!("{chest:?}").contains("minecraft:apple"), "chest contents loaded: {chest:?}");
    // Another size: only the template's size is taken (loading prepared).
    let keys = w.structure(load, &settings(2, 1, "g6f14:house", [2, 0, 0], [1, 1, 1], 1 | 4));
    assert!(keys.iter().any(|k| k == "structure_block.load_prepare"), "keys {keys:?}");
    assert_eq!((int_of(&w.be(load), "sizeX"), int_of(&w.be(load), "sizeY")), (3, 2), "size taken from the template");
    // Unknown template.
    let keys = w.structure(load, &settings(2, 1, "g6f14:nothing", [2, 0, 0], [3, 2, 3], 1 | 4));
    assert!(keys.iter().any(|k| k == "structure_block.load_not_found"), "keys {keys:?}");
}

/// A save reaching above the build height records the cells there as void air (the state the
/// world reads outside its height).
#[test]
fn structure_block_saves_void_air_above_the_world() {
    let mut w = world("save-void");
    let sb = [0, 318, 0];
    w.cmd("setblock 0 318 0 minecraft:structure_block[mode=save]");
    let keys = w.structure(sb, &settings(1, 0, "g6f14:top", [0, 1, 0], [1, 4, 1], 1 | 4));
    assert!(keys.iter().any(|k| k == "structure_block.save_success"), "keys {keys:?}");
    let file = w.p.s.world.join("generated").join("g6f14").join("structure").join("top.nbt");
    let root = crate::nbtio::read_dat(&file).expect("template file");
    let names: Vec<&str> =
        crate::nbtio::list(root.get("palette")).iter().filter_map(|e| e.get("id").and_then(Nbt::as_str)).collect();
    assert_eq!(names, ["minecraft:air", "minecraft:void_air"], "y 319 inside, 320..322 outside");
    assert_eq!(assert_current_palette_keys(&root), 0);
}

/// Palette of a saved template in the game's current keys (its reader ignores the old ones at
/// this data version and loads air): a compound per entry with `id`, plus `properties` sorted by
/// name for any block that has properties, default state included; none for the others.
/// Returns how many of the chest (default state) and east stairs entries were checked.
fn assert_current_palette_keys(root: &Nbt) -> usize {
    let palette = crate::nbtio::list(root.get("palette"));
    assert!(!palette.is_empty(), "empty palette");
    for entry in palette {
        let Nbt::Compound(fields) = entry else { panic!("palette entry not a compound: {entry:?}") };
        let keys: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys == ["id"] || keys == ["id", "properties"], "palette entry keys {keys:?}");
    }
    let properties = |id: &str| -> Option<Vec<(String, String)>> {
        let entry = palette.iter().find(|e| e.get("id").and_then(Nbt::as_str) == Some(id))?;
        let Some(Nbt::Compound(p)) = entry.get("properties") else { return Some(Vec::new()) };
        Some(p.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("?").to_owned())).collect())
    };
    let pairs = |v: &[(&str, &str)]| v.iter().map(|(k, x)| ((*k).to_owned(), (*x).to_owned())).collect::<Vec<_>>();
    for id in ["minecraft:air", "minecraft:void_air", "minecraft:stone"] {
        assert!(properties(id).is_none_or(|p| p.is_empty()), "{id} has no property");
    }
    let mut checked = 0;
    if let Some(chest) = properties("minecraft:chest") {
        let default = [("facing", "north"), ("type", "single"), ("waterlogged", "false")];
        assert_eq!(chest, pairs(&default), "default state written with its properties");
        checked += 1;
    }
    if let Some(stairs) = properties("minecraft:oak_stairs") {
        let east = [("facing", "east"), ("half", "bottom"), ("shape", "straight"), ("waterlogged", "false")];
        assert_eq!(stairs, pairs(&east), "properties sorted by name");
        checked += 1;
    }
    checked
}

fn int_of(n: &Nbt, k: &str) -> i64 {
    crate::nbtio::int(n, k).unwrap_or(i64::MIN)
}

/// Size detection: the corner blocks of the same name around the save block give the area
/// (interior of their box), the structure block's mode drives its block state.
#[test]
fn structure_block_detects_its_size_from_corners() {
    let mut w = world("detect");
    let sb = [0, Y, 0];
    w.cmd(&format!("setblock {} {} {} minecraft:structure_block[mode=data]", sb[0], sb[1], sb[2]));
    for (x, y, z) in [(3, Y + 1, 3), (7, Y + 4, 9)] {
        w.cmd(&format!(
            "setblock {x} {y} {z} minecraft:structure_block[mode=corner]{{mode:\"CORNER\",name:\"g6f14:c\"}}"
        ));
    }
    // A corner of another name is ignored.
    w.cmd(&format!(
        "setblock 20 {Y} 20 minecraft:structure_block[mode=corner]{{mode:\"CORNER\",name:\"g6f14:other\"}}"
    ));
    let keys = w.structure(sb, &settings(3, 0, "g6f14:c", [0, 1, 0], [0, 0, 0], 4));
    assert!(keys.iter().any(|k| k == "structure_block.size_success"), "keys {keys:?}");
    let be = w.be(sb);
    let got = ["posX", "posY", "posZ", "sizeX", "sizeY", "sizeZ"].map(|k| int_of(&be, k));
    assert_eq!(got, [4, 2, 4, 3, 2, 5], "interior of the corners' box");
    assert_eq!(be.get("mode").and_then(Nbt::as_str), Some("SAVE"));
    assert!(w.name(sb).contains("mode=save"), "block state follows the mode: {}", w.name(sb));
    // Not in save mode: failure.
    let keys = w.structure(sb, &settings(3, 3, "g6f14:c", [0, 1, 0], [0, 0, 0], 4));
    assert!(keys.iter().any(|k| k == "structure_block.size_failure"), "keys {keys:?}");
}

/// Redstone on a save block captures into memory only; `place template` then finds the
/// capture; a load block powered by redstone places it.
#[test]
fn structure_block_redstone_save_and_load() {
    let mut w = world("redstone");
    let sb = [0, Y, 0];
    w.cmd(&format!(
        "setblock 0 {Y} 0 minecraft:structure_block[mode=save]{{mode:\"SAVE\",name:\"g6f14:mem\",posX:2,posY:0,posZ:0,sizeX:2,sizeY:1,sizeZ:1}}"
    ));
    w.cmd(&format!("setblock 2 {Y} 0 minecraft:gold_block"));
    w.cmd(&format!("setblock 3 {Y} 0 minecraft:diamond_block"));
    w.cmd(&format!("setblock 0 {} 0 minecraft:redstone_block", Y + 1));
    assert_eq!(int_of(&w.be(sb), "powered"), 1, "rising edge recorded");
    let file = w.p.s.world.join("generated").join("g6f14").join("structure").join("mem.nbt");
    assert!(!file.exists(), "redstone save stays in memory");
    w.cmd(&format!("place template g6f14:mem 5 {Y} 5"));
    assert_eq!(w.name([5, Y, 5]), "minecraft:gold_block");
    assert_eq!(w.name([6, Y, 5]), "minecraft:diamond_block");
    w.cmd(&format!(
        "setblock 0 {Y} 12 minecraft:structure_block[mode=load]{{mode:\"LOAD\",name:\"g6f14:mem\",posX:2,posY:0,posZ:0}}"
    ));
    w.cmd(&format!("setblock 0 {} 12 minecraft:redstone_block", Y + 1));
    assert_eq!(w.name([2, Y, 12]), "minecraft:gold_block", "powered load block places the template");
}

/// `place template` with an integrity below 1 keeps each block on the game's own generator seeded
/// by the command (the palette draw comes first), reads the world's `structure` folder.
#[test]
fn place_template_integrity_follows_the_game_generator() {
    let mut w = world("integrity");
    w.cmd(&format!(
        "setblock 0 {Y} 0 minecraft:structure_block[mode=save]{{mode:\"SAVE\",name:\"g6f14:slab\",posX:2,posY:0,posZ:0,sizeX:4,sizeY:1,sizeZ:4}}"
    ));
    w.cmd(&format!("fill 2 {Y} 0 5 {Y} 3 minecraft:stone"));
    let keys = w.structure([0, Y, 0], &settings(1, 0, "g6f14:slab", [2, 0, 0], [4, 1, 4], 1 | 4));
    assert!(keys.iter().any(|k| k == "structure_block.save_success"), "keys {keys:?}");
    w.cmd(&format!("place template g6f14:slab 20 {Y} 20 none none 0.5 7"));
    let mut r = JRand::new(7);
    let _palette = r.next_int_bound(1);
    let mut want = Vec::new();
    for x in 0..4 {
        for z in 0..4 {
            if r.next_float() <= 0.5 {
                want.push([20 + x, Y, 20 + z]);
            }
        }
    }
    let got: Vec<[i32; 3]> = (0..4)
        .flat_map(|x| (0..4).map(move |z| [20 + x, Y, 20 + z]))
        .filter(|&p| w.name(p) == "minecraft:stone")
        .collect();
    assert!(!want.is_empty() && want.len() < 16, "the draw keeps part of the blocks");
    assert_eq!(got, want, "kept blocks of the game's generator");
}

/// `place template` places the template's entities (identifier removed, look turned by the
/// mirror), unlike before.
#[test]
fn place_template_places_entities() {
    let mut w = world("entities");
    w.cmd(&format!(
        "setblock 0 {Y} 0 minecraft:structure_block[mode=save]{{mode:\"SAVE\",name:\"g6f14:stand\",posX:2,posY:0,posZ:0,sizeX:3,sizeY:3,sizeZ:3,ignoreEntities:0b}}"
    ));
    w.cmd(&format!("summon minecraft:armor_stand 3.5 {Y} 1.5 {{Rotation:[30f,0f],NoGravity:1b}}"));
    // Ignore entities off (flags without bit 1).
    let keys = w.structure([0, Y, 0], &settings(1, 0, "g6f14:stand", [2, 0, 0], [3, 3, 3], 4));
    assert!(keys.iter().any(|k| k == "structure_block.save_success"), "keys {keys:?}");
    let stands = |w: &W| {
        w.p.s
            .all_ents()
            .into_iter()
            .filter(|&e| e != E::Player && w.p.s.ent_type(e) == "minecraft:armor_stand")
            .collect::<Vec<_>>()
    };
    assert_eq!(stands(&w).len(), 1);
    w.cmd(&format!("place template g6f14:stand 10 {Y} 20 none front_back"));
    let all = stands(&w);
    assert_eq!(all.len(), 2, "the template's armor stand is placed");
    let placed = *all.iter().find(|&&e| w.p.s.e_pos(e)[2] > 10.0).expect("placed stand");
    let pos = w.p.s.e_pos(placed);
    // Front-back flips x around the origin: 1.5 → 1 − 1.5 = −0.5.
    assert!((pos[0] - 9.5).abs() < 1e-9 && (pos[2] - 21.5).abs() < 1e-9, "position {pos:?}");
    let yaw = w.p.s.ent_rot(placed)[0];
    assert!((yaw + 30.0).abs() < 1e-4, "front-back turns 30° into −30°, got {yaw}");
}

/// A command block copied with its data (`auto:1b`) schedules its first tick when placed, as a
/// loaded block entity that becomes automatic.
#[test]
fn copied_automatic_command_block_schedules_its_tick() {
    let mut w = world("cmdblock");
    let data = Nbt::Compound(vec![
        ("id".into(), Nbt::String("minecraft:command_block".into())),
        ("Command".into(), Nbt::String("say g6f14".into())),
        ("auto".into(), Nbt::Byte(1)),
    ]);
    let comps = Nbt::Compound(vec![("minecraft:block_entity_data".into(), data)]);
    let slot = 36 + w.p.s.held as usize;
    w.p.s.slots[slot] =
        Some(crate::player::Stack { id: "minecraft:command_block".into(), count: 1, components: Some(comps) });
    w.cmd(&format!("setblock 5 {Y} 5 minecraft:stone"));
    w.p.s.use_item_on_block([5, Y, 5], crate::blocks::Dir::Up, [0.5, 1.0, 0.5]);
    let at = [5, Y + 1, 5];
    assert!(w.name(at).starts_with("minecraft:command_block"), "placed: {}", w.name(at));
    assert_eq!(int_of(&w.be(at), "auto"), 1, "copied setting kept");
    let b = w.p.s.view().block(w.p.s.get(at));
    assert!(w.p.s.has_scheduled_tick(at, b), "first tick scheduled");
}

/// Border of a world without border file: warning time of 300 (game's default settings).
#[test]
fn default_border_warning_time_is_300() {
    let w = world("border");
    assert_eq!(w.p.s.cmd.border.warn_time, 300);
    w.p.s.send_border();
    let init = w.p.take_events().into_iter().find_map(|e| match e {
        Event::Border(rc_proto_v777::border::BorderPacket::Init { warn_ticks, .. }) => Some(warn_ticks),
        _ => None,
    });
    assert_eq!(init, Some(300));
}

/// `worldborder add <distance> <time>` extends the transition in progress (its remaining ticks
/// are added).
#[test]
fn worldborder_add_extends_the_transition() {
    let mut w = world("wb-add");
    w.cmd("worldborder set 100");
    w.cmd("worldborder set 200 10");
    w.cmd("worldborder add 50 20");
    let l = w.p.s.cmd.border.lerp.expect("transition");
    assert_eq!((l.2, l.3), (30, 30), "10 remaining + 20");
}

/// `forceload query` lists the chunks in the order of their packed position (z, then x as an
/// unsigned value); a column beyond 30 million blocks is out of the world.
#[test]
fn forceload_list_order_and_world_bound() {
    let mut w = world("forceload");
    w.cmd("forceload add -20 0");
    w.cmd("forceload add 16 0");
    w.cmd("forceload add 0 -16");
    let _ = w.chat_text();
    w.cmd("forceload query");
    let text = w.chat_text();
    assert!(text.contains("[0, -1], [1, 0], [-2, 0]"), "order: {text}");
    assert!(!w.try_cmd("forceload add 30000000 0"));
    assert!(w.keys().iter().any(|k| k == "argument.pos.outofworld"));
}

/// Jigsaw block settings from the screen; test block mode and message, redstone edges, signal of
/// a powered start block.
#[test]
fn jigsaw_and_test_block_settings() {
    let mut w = world("jigsaw-test");
    let j = [0, Y, 0];
    // New block entity: rollable whatever the front; data loaded without a joint: from the front.
    w.cmd(&format!("setblock 2 {Y} 0 minecraft:jigsaw[orientation=north_up]{{name:\"minecraft:x\"}}"));
    assert_eq!(w.be([2, Y, 0]).get("joint").and_then(Nbt::as_str), Some("aligned"), "loaded, horizontal front");
    assert_eq!(w.be([2, Y, 0]).get("pool").and_then(Nbt::as_str), Some("minecraft:empty"), "loading defaults");
    w.cmd(&format!("setblock 0 {Y} 0 minecraft:jigsaw[orientation=north_up]"));
    assert_eq!(w.be(j).get("joint").and_then(Nbt::as_str), Some("rollable"), "new block entity");
    // Test instance block: the game's empty data.
    w.cmd(&format!("setblock 0 {Y} 4 minecraft:test_instance_block"));
    let data = w.be([0, Y, 4]).get("data").cloned().expect("default data");
    assert_eq!(
        data,
        Nbt::Compound(vec![
            ("size".to_owned(), Nbt::IntArray(vec![0, 0, 0])),
            ("rotation".to_owned(), Nbt::String("none".to_owned())),
            ("ignore_entities".to_owned(), Nbt::Byte(0)),
            ("status".to_owned(), Nbt::String("cleared".to_owned())),
        ])
    );
    let mut buf = Vec::new();
    write_set_jigsaw_block(
        &mut buf,
        BlockPos::new(0, Y, 0),
        "a",
        "g6f14:b",
        "g6f14:pool",
        "minecraft:stone",
        "rollable",
        3,
        4,
    );
    let id = w.p.s.peer.ids().set_jigsaw_block;
    w.p.s.tech_packet(id, &mut Reader::new(&buf));
    let be = w.be(j);
    let s = |k: &str| be.get(k).and_then(Nbt::as_str).map(str::to_owned);
    assert_eq!(s("name").as_deref(), Some("minecraft:a"));
    assert_eq!(s("pool").as_deref(), Some("g6f14:pool"));
    assert_eq!(s("joint").as_deref(), Some("rollable"));
    assert_eq!((int_of(&be, "selection_priority"), int_of(&be, "placement_priority")), (3, 4));
    // Test block: mode and message.
    let t = [4, Y, 0];
    w.cmd(&format!("setblock 4 {Y} 0 minecraft:test_block"));
    let mut buf = Vec::new();
    write_set_test_block(&mut buf, BlockPos::new(4, Y, 0), 1, "hello");
    let id = w.p.s.peer.ids().set_test_block;
    w.p.s.tech_packet(id, &mut Reader::new(&buf));
    assert!(w.name(t).contains("mode=log"), "state: {}", w.name(t));
    assert_eq!(w.be(t).get("message").and_then(Nbt::as_str), Some("hello"));
    w.cmd(&format!("setblock 5 {Y} 0 minecraft:redstone_block"));
    assert_eq!(int_of(&w.be(t), "powered"), 1, "rising edge recorded");
    w.cmd(&format!("setblock 5 {Y} 0 minecraft:air"));
    assert_eq!(int_of(&w.be(t), "powered"), 0, "falling edge recorded");
    // Start block powered (by the test runner): signal 15 to its neighbours.
    w.cmd(&format!("setblock 8 {Y} 0 minecraft:test_block[mode=start]{{mode:\"start\",powered:1b}}"));
    assert_eq!(w.p.s.signal([8, Y, 0], crate::blocks::Dir::East), 15);
    // Not an operator in creative: refused.
    w.p.s.game_mode = 0;
    let mut buf = Vec::new();
    write_set_test_block(&mut buf, BlockPos::new(4, Y, 0), 2, "no");
    w.p.s.tech_packet(id, &mut Reader::new(&buf));
    assert_eq!(w.be(t).get("message").and_then(Nbt::as_str), Some("hello"), "survival player refused");
}

/// `clone` copies the scheduled block ticks of the source box (same due time and priority),
/// and reads the source from the dimension named after `from`.
#[test]
fn clone_copies_scheduled_ticks_and_reads_the_source_dimension() {
    let mut w = world("clone");
    let (src, dst) = ([0, Y, 30], [6, Y, 30]);
    w.cmd(&format!("setblock {} {} {} minecraft:stone", src[0], src[1], src[2]));
    let b = w.p.s.view().block(w.p.s.get(src));
    w.p.s.schedule_tick_priority(src, b, 7, -1);
    let before = w.p.s.bs.block_ticks.in_box(src, src);
    assert_eq!(before.len(), 1);
    w.cmd(&format!(
        "clone {} {} {} {} {} {} {} {} {}",
        src[0], src[1], src[2], src[0], src[1], src[2], dst[0], dst[1], dst[2]
    ));
    let copied = w.p.s.bs.block_ticks.in_box(dst, dst);
    assert_eq!(copied.len(), 1, "tick copied");
    assert_eq!((copied[0].at, copied[0].priority, copied[0].kind), (before[0].at, -1, before[0].kind));
    assert!(copied[0].sub > before[0].sub, "ordered after the copied ticks");
    // Source in another dimension (not loaded there): refused, not cloned from here.
    assert!(!w.try_cmd(&format!(
        "clone from minecraft:the_nether {} {} {} {} {} {} 12 {Y} 30",
        src[0], src[1], src[2], src[0], src[1], src[2]
    )));
    assert!(w.keys().iter().any(|k| k == "argument.pos.unloaded"));
    assert_eq!(w.name([12, Y, 30]), "minecraft:air");
}

/// `clone … move` over an overlapping box reads the destination's previous states before the
/// source is cleared: the powered lever replaced at the destination is passed as the removed
/// state of the final neighbour pass, so the lamp behind its support turns off.
#[test]
fn clone_move_keeps_the_previous_destination_state() {
    let mut w = world("clone-move");
    let z = 40;
    // Support placed last: its neighbour updates light the lamp behind it.
    w.cmd(&format!("setblock 1 {Y} {} minecraft:redstone_lamp", z - 2));
    w.cmd(&format!("setblock 1 {Y} {z} minecraft:lever[face=wall,facing=south,powered=true]"));
    w.cmd(&format!("setblock 1 {Y} {} minecraft:stone", z - 1));
    assert!(w.name([1, Y, z - 2]).contains("lit=true"), "lamp lit by the lever: {}", w.name([1, Y, z - 2]));
    w.cmd(&format!("clone 0 {Y} {z} 3 {Y} {z} 1 {Y} {z} replace move"));
    for _ in 0..6 {
        w.p.tick();
    }
    assert!(w.name([1, Y, z - 2]).contains("lit=false"), "lever removal propagated: {}", w.name([1, Y, z - 2]));
}

/// `locate poi` finds the nearest point of interest (here a bell: meeting point).
#[test]
fn locate_poi_finds_the_nearest_point() {
    let mut w = world("poi");
    w.cmd(&format!("setblock 3 {Y} 3 minecraft:bell"));
    let src = w.p.s.player_source();
    let (ok, dist) = w.p.s.perform(src, "locate poi minecraft:meeting");
    assert!(ok, "located: {:?}", w.keys());
    assert_eq!(dist, 7, "horizontal distance from (8, 8) to (3, 3)");
    let src = w.p.s.player_source();
    assert!(!w.p.s.perform(src, "locate poi minecraft:armorer").0, "no armorer around");
    assert!(w.keys().iter().any(|k| k == "commands.locate.poi.not_found"));
}

/// The server's entities collide with the border: a box moving out of a border of 4 blocks
/// centred on the origin stops at its edge.
#[test]
fn border_blocks_the_server_entities() {
    let mut w = world("border-collide");
    w.cmd("worldborder center 0 0");
    w.cmd("worldborder set 4");
    let world = w.p.s.world();
    let b = rc_physics::Aabb { min: [1.0, f64::from(Y) + 1.0, 0.2], max: [1.6, f64::from(Y) + 2.8, 0.8] };
    let m = rc_physics::collide::resolve(&world, &b, [2.0, 0.0, 0.0], false, 0.0);
    // Integer centre corrected to the block centre (0.5): edge at 2.5, collision box at its ceiling 3.
    assert!((m[0] - 1.4).abs() < 1e-9, "stopped at x = 3, moved {m:?}");
}

/// Bench (loaded machine): cost added to an entity movement by the border (view built at first
/// need, then the edge test of an entity far from it), best of 5 over 1,000,000 calls.
#[test]
#[ignore = "bench"]
fn bench_border_in_entity_collision() {
    use rc_physics::World as _;
    let w = world("bench-border");
    let b = rc_physics::Aabb { min: [1.0, f64::from(Y) + 1.0, 0.2], max: [1.6, f64::from(Y) + 2.8, 0.8] };
    let n = 1_000_000;
    let best = |f: &dyn Fn() -> usize| {
        (0..5)
            .map(|_| {
                let t = std::time::Instant::now();
                let mut acc = 0usize;
                for _ in 0..n {
                    acc = acc.wrapping_add(f());
                }
                std::hint::black_box(acc);
                t.elapsed().as_secs_f64() * 1e9 / f64::from(n)
            })
            .fold(f64::MAX, f64::min)
    };
    let view = best(&|| std::hint::black_box(w.p.s.world()).min_y as usize);
    let with_border = best(&|| {
        let v = w.p.s.world();
        let mut out = Vec::new();
        if let Some(br) = v.border() {
            br.collision([1.3, 0.5], &b, &mut out);
        }
        out.len()
    });
    eprintln!("view {view:.1} ns, view + border + edge test {with_border:.1} ns (loaded machine)");
}

fn count_non_air(w: &W, lo: [i32; 3], hi: [i32; 3]) -> usize {
    let mut n = 0;
    for x in lo[0]..=hi[0] {
        for y in lo[1]..=hi[1] {
            for z in lo[2]..=hi[2] {
                if !w.p.s.view().is_air(w.p.s.get([x, y, z])) {
                    n += 1;
                }
            }
        }
    }
    n
}

/// Name of a jigsaw of the pillager outpost base plate (target of the assemblies below).
fn base_plate_jigsaw(w: &mut W) -> String {
    let (wd, _) = w.p.s.place_data().expect("generation data");
    let t = wd.templates.get("minecraft:pillager_outpost/base_plate", &wd.blocks).expect("base plate template");
    t.jigsaws[0].first().expect("a jigsaw").name.to_string()
}

/// `place structure` generates a start at the position's chunk whatever the biome and places it
/// (here an igloo in a flat plains world); `place jigsaw` assembles a pool at a position.
#[test]
fn place_structure_and_jigsaw_build_into_the_world() {
    let mut w = world("place-structure");
    let area = ([-24, Y, -24], [40, Y + 10, 40]);
    let before = count_non_air(&w, area.0, area.1);
    let ok = w.try_cmd(&format!("place structure minecraft:igloo 8 {Y} 8"));
    assert!(ok, "igloo refused: {}", w.chat_text());
    let after = count_non_air(&w, area.0, area.1);
    assert!(after > before + 50, "igloo placed: {before} -> {after}");
    let target = base_plate_jigsaw(&mut w);
    let lo = [-24, Y - 4, 24];
    let hi = [24, Y + 6, 40];
    let before = count_non_air(&w, lo, hi);
    w.cmd(&format!("place jigsaw minecraft:pillager_outpost/base_plates {target} 1 0 {Y} 32"));
    let after = count_non_air(&w, lo, hi);
    assert!(after != before, "base plate placed: {before} -> {after}");
    assert!(!w.try_cmd(&format!("place structure minecraft:nothing 0 {Y} 0")));
}

/// A jigsaw block generates its pool in front of it on request (screen's button).
#[test]
fn jigsaw_block_generates_its_pool() {
    let mut w = world("jigsaw-generate");
    let target = base_plate_jigsaw(&mut w);
    let j = [0, Y, 32];
    w.cmd(&format!("setblock 0 {Y} 32 minecraft:jigsaw[orientation=up_north]"));
    let mut buf = Vec::new();
    let at = BlockPos::new(j[0], j[1], j[2]);
    write_set_jigsaw_block(
        &mut buf,
        at,
        "minecraft:empty",
        &target,
        "minecraft:pillager_outpost/base_plates",
        "minecraft:air",
        "rollable",
        0,
        0,
    );
    let id = w.p.s.peer.ids().set_jigsaw_block;
    w.p.s.tech_packet(id, &mut Reader::new(&buf));
    let (lo, hi) = ([-24, Y - 4, 24], [24, Y + 8, 40]);
    let before = count_non_air(&w, lo, hi);
    let mut buf = Vec::new();
    rc_proto_v777::special::write_jigsaw_generate(&mut buf, at, 1, false);
    let id = w.p.s.peer.ids().jigsaw_generate;
    w.p.s.tech_packet(id, &mut Reader::new(&buf));
    let after = count_non_air(&w, lo, hi);
    assert!(after != before, "pool generated: {before} -> {after}");
}

/// Bench (loaded machine): `place jigsaw` of the outpost base plate (journal of the placements
/// replayed into the world), best of 3 at three chunks.
#[test]
#[ignore = "bench"]
fn bench_place_jigsaw() {
    let mut w = world("bench-jigsaw");
    let target = base_plate_jigsaw(&mut w);
    let best = [0, 16, 32]
        .iter()
        .map(|x| {
            let t = std::time::Instant::now();
            w.cmd(&format!("place jigsaw minecraft:pillager_outpost/base_plates {target} 1 {x} {Y} 32"));
            t.elapsed().as_secs_f64() * 1e3
        })
        .fold(f64::MAX, f64::min);
    eprintln!("place jigsaw base plate: {best:.2} ms (loaded machine)");
}

/// `place jigsaw` writes its pool pieces with a known shape: a fence already in the world next to
/// a placed block does not connect to it (game quirk), whereas it connects when placed after.
#[test]
fn place_jigsaw_leaves_the_world_shapes_alone() {
    let cmd = |target: &str| format!("place jigsaw minecraft:pillager_outpost/base_plates {target} 1 0 {Y} 32");
    let (lo, hi) = ([-24, Y, 16], [24, Y + 4, 48]);
    // First world: cells that the placement fills, with an empty neighbour to the east.
    let mut w = world("jigsaw-shape-a");
    let target = base_plate_jigsaw(&mut w);
    let cells = |w: &W| {
        let mut v = std::collections::BTreeMap::new();
        for x in lo[0]..=hi[0] {
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    v.insert([x, y, z], w.p.s.view().is_air(w.p.s.get([x, y, z])));
                }
            }
        }
        v
    };
    let before = cells(&w);
    w.cmd(&cmd(&target));
    let after = cells(&w);
    let east = |p: [i32; 3]| [p[0] + 1, p[1], p[2]];
    let candidates: Vec<[i32; 3]> = before
        .iter()
        .filter(|(p, was_air)| **was_air && !after[*p])
        .map(|(p, _)| *p)
        .filter(|p| before.get(&east(*p)) == Some(&true) && after.get(&east(*p)) == Some(&true))
        .collect();
    let mut found = None;
    for c in candidates {
        let n = east(c);
        w.cmd(&format!("setblock {} {} {} minecraft:oak_fence", n[0], n[1], n[2]));
        if w.name(n).contains("west=true") {
            found = Some(n);
            break;
        }
    }
    let n = found.expect("a placed block a fence connects to");
    drop(w);
    // Second world: the fence is there first; the placement leaves it unconnected.
    let mut w = world("jigsaw-shape-b");
    w.cmd(&format!("setblock {} {} {} minecraft:oak_fence", n[0], n[1], n[2]));
    assert!(w.name(n).contains("west=false"));
    w.cmd(&cmd(&target));
    assert!(!w.p.s.view().is_air(w.p.s.get([n[0] - 1, n[1], n[2]])), "same placement");
    assert!(w.name(n).contains("west=false"), "no shape update from a pool piece: {}", w.name(n));
}

// ---- L9-STRUCT: features and structures replayed into the live world ----

fn chunk_loaded(w: &W, c: (i32, i32)) -> bool {
    matches!(w.p.s.chunks.get(&c), Some(Some(_)))
}

/// A feature placed into the live world keeps its own flags: the oak's trunk is written with
/// neighbour updates (flags 19), so a lit lamp without power beside it schedules its switching
/// off. A copy written back with the "clients" flag alone left it lit and unscheduled.
#[test]
fn live_feature_writes_update_their_neighbours() {
    let mut w = world("feature-flags");
    let (at, lamp) = ([8, Y, 8], [9, Y, 8]);
    w.cmd(&format!("setblock {} {} {} minecraft:redstone_lamp[lit=true]", lamp[0], lamp[1], lamp[2]));
    assert!(w.name(lamp).contains("lit=true"), "{}", w.name(lamp));
    let id = u32::from(w.p.s.bs.t.block_id("minecraft:redstone_lamp"));
    let scheduled =
        |w: &W| w.p.s.bs.block_ticks.chunk_ticks((0, 0)).iter().any(|t| t.pos == lamp && t.kind == id);
    assert!(!scheduled(&w), "lamp tick before the tree");
    assert!(w.p.s.place_world_feature("minecraft:oak", at), "oak placed");
    assert!(w.name(at).starts_with("minecraft:oak_log"), "trunk: {}", w.name(at));
    assert!(scheduled(&w), "the trunk's neighbour update reaches the lamp");
}

/// `place feature` of an ore writes straight into the sections, like the game's ore veins: the
/// cells change on the server, but no client update is sent and the chunk is not marked for
/// saving (game quirk).
#[test]
fn place_feature_ore_writes_silently() {
    let mut w = world("ore-silent");
    w.cmd(&format!("fill 0 {Y} 0 15 {} 15 minecraft:stone", Y + 15));
    w.p.s.flush_block_changes();
    let _ = w.p.take_events();
    w.p.s.chunks.get_mut(&(0, 0)).and_then(Option::as_mut).expect("chunk").dirty = false;
    let ore = |w: &W| {
        let mut v = Vec::new();
        for x in 0..16 {
            for y in Y..Y + 16 {
                for z in 0..16 {
                    if w.name([x, y, z]) == "minecraft:iron_ore" {
                        v.push([x, y, z]);
                    }
                }
            }
        }
        v
    };
    assert!(ore(&w).is_empty());
    w.cmd(&format!("place feature minecraft:ore_iron 8 {} 8", Y + 8));
    let cells = ore(&w);
    assert!(!cells.is_empty(), "vein placed");
    assert!(cells.iter().all(|p| !w.p.s.changed.contains(p)), "an ore cell was sent to the client");
    assert!(!w.p.s.chunks[&(0, 0)].as_ref().expect("chunk").dirty, "chunk marked for saving");
}

/// `place jigsaw` only needs the chunk of its position loaded; the pieces that reach other chunks
/// load them (generated when absent) and land there, like the game, instead of being dropped.
#[test]
fn place_jigsaw_loads_the_chunks_its_pieces_reach() {
    let mut w = world("jigsaw-unloaded");
    let target = base_plate_jigsaw(&mut w);
    let home = (10, 10);
    w.p.s.load_now(&[home]);
    assert!(chunk_loaded(&w, home), "home chunk");
    let around: Vec<(i32, i32)> = (-1..=1)
        .flat_map(|dz| (-1..=1).map(move |dx| (home.0 + dx, home.1 + dz)))
        .filter(|&c| c != home)
        .collect();
    assert!(around.iter().all(|&c| !chunk_loaded(&w, c)), "neighbours unloaded before");
    w.cmd(&format!("place jigsaw minecraft:pillager_outpost/base_plates {target} 2 168 {Y} 168"));
    let reached: Vec<(i32, i32)> = around.iter().copied().filter(|&c| chunk_loaded(&w, c)).collect();
    assert!(!reached.is_empty(), "no chunk around the position loaded by the pieces");
    // Flat terrain: bedrock, two dirt, grass, then air from Y up.
    let flat = |y: i32| match y - (Y - 4) {
        0 => "minecraft:bedrock",
        1 | 2 => "minecraft:dirt",
        3 => "minecraft:grass_block",
        _ => "minecraft:air",
    };
    let mut placed = 0;
    for &(cx, cz) in &reached {
        for x in cx * 16..cx * 16 + 16 {
            for z in cz * 16..cz * 16 + 16 {
                for y in Y - 4..Y + 30 {
                    let n = w.name([x, y, z]);
                    placed += usize::from(n.split('[').next() != Some(flat(y)));
                }
            }
        }
    }
    assert!(placed > 0, "no piece block in the chunks reached: {reached:?}");
}

/// Region copy of a live chunk: height maps from one walk per column (from the highest section
/// that is not all air) equal to one walk per map from the top of the world, for built, flat and
/// unloaded chunks.
#[test]
fn deco_chunk_one_walk_matches_per_map() {
    let mut w = world("deco-prime");
    w.cmd(&format!("fill 0 {Y} 0 15 {} 15 minecraft:stone", Y + 20));
    w.cmd(&format!("fill 2 {} 2 6 {} 6 minecraft:water", Y + 21, Y + 40));
    w.cmd(&format!("fill 8 {} 8 12 {} 12 minecraft:oak_leaves", Y + 21, Y + 60));
    w.cmd("fill 16 100 0 31 100 15 minecraft:glass");
    w.cmd("setblock 20 300 4 minecraft:cobweb");
    let (wd, dg) = w.p.s.place_data().expect("generation data");
    for c in [(0, 0), (1, 0), (-2, -2), (40, 40)] {
        let a = w.p.s.deco_chunk_with(c.0, c.1, &dg, &wd, true);
        let b = w.p.s.deco_chunk_with(c.0, c.1, &dg, &wd, false);
        assert_eq!(a.heights, b.heights, "chunk {c:?}");
        assert_eq!(a.blocks, b.blocks, "chunk {c:?}");
    }
}

/// Region copies of a live structure's grid shared among threads: the same copies, in the same
/// order, as on one thread (loaded and unloaded chunks mixed).
#[test]
fn region_copies_on_threads_match_one_thread() {
    let mut w = world("deco-par");
    w.cmd(&format!("fill 0 {Y} 0 15 {} 15 minecraft:stone", Y + 20));
    let (wd, dg) = w.p.s.place_data().expect("generation data");
    let cells: Vec<(i32, i32)> = (-3..=3).flat_map(|z| (-3..=3).map(move |x| (x, z))).collect();
    let one = w.p.s.copy_chunks_on(&cells, &dg, &wd, 1);
    for n in [2, 4, 7] {
        let many = w.p.s.copy_chunks_on(&cells, &dg, &wd, n);
        assert_eq!(many.len(), one.len(), "{n} threads");
        for (a, b) in one.iter().zip(&many) {
            assert_eq!((a.cx, a.cz), (b.cx, b.cz), "{n} threads: order");
            assert!(a.blocks == b.blocks && a.heights == b.heights && a.biomes == b.biomes, "{n} threads: {:?}", (a.cx, a.cz));
        }
    }
}

/// A live structure writes a barrier without side effects before each block that carries data,
/// like the game: a chest standing where the igloo puts one of its block entities is replaced
/// without spilling its contents.
#[test]
fn live_structure_replaces_block_entities_through_a_barrier() {
    let cmd = format!("place structure minecraft:igloo 8 {Y} 8");
    let (lo, hi) = ([-24, Y - 4, -24], [40, Y + 10, 40]);
    // First world: a cell of the igloo that holds a block entity other than a chest (a chest
    // would keep the old one's entity even without the barrier).
    let mut w = world("igloo-be-a");
    w.cmd(&cmd);
    let mut cell = None;
    'find: for x in lo[0]..=hi[0] {
        for y in lo[1]..=hi[1] {
            for z in lo[2]..=hi[2] {
                if w.p.s.be_get([x, y, z]).is_some() && !w.name([x, y, z]).starts_with("minecraft:chest") {
                    cell = Some([x, y, z]);
                    break 'find;
                }
            }
        }
    }
    let c = cell.expect("a block entity in the igloo");
    let block = w.name(c);
    drop(w);
    // Second world: a chest with a diamond there first.
    let mut w = world("igloo-be-b");
    w.cmd(&format!(
        "setblock {} {} {} minecraft:chest{{Items:[{{Slot:0b,id:\"minecraft:diamond\",count:1}}]}}",
        c[0], c[1], c[2]
    ));
    w.cmd(&cmd);
    assert_eq!(w.name(c), block, "same igloo block");
    let diamonds = w.p.s.life.drops.iter().filter(|d| !d.is_removed() && d.stack.id == "minecraft:diamond").count();
    assert_eq!(diamonds, 0, "the chest's contents were spilled");
}
