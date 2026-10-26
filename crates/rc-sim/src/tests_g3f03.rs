//! G3-F03: integrated server loop (autosave, pause), item slot held during a
//! use, data pack drop-in (selection, stack, experiments), nested function tags,
//! `execute if|unless stopwatch|slots`, validation of `store` and `facing` gaze.
//! Fresh world created by the generator: only the version data and the jar are required
//! (absent: explicit failure).

use std::io::Write as _;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use rc_client::Event;
use rc_proto::nbt::Nbt;

use crate::input::Input;
use crate::player::Stack;
use crate::server::Server;

struct T {
    s: Server,
    session: rc_client::Session,
    dir: std::path::PathBuf,
}

impl Drop for T {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Fresh world; `prepare` writes into its folder before opening (packs, level.dat).
fn open_with(tag: &str, prepare: impl FnOnce(&std::path::Path)) -> T {
    let data = Arc::new(rc_testdata::version_data().expect("version data missing: G3-F03 tests impossible"));
    crate::registries::client_jar(&data).expect("client jar missing: G3-F03 tests impossible");
    let dir = std::env::temp_dir().join(format!("rc-g3f03-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut w = crate::newworld::NewWorld::new(tag);
    w.seed = 7;
    crate::newworld::create_world(&dir, &data, &w).expect("world creation");
    prepare(&dir);
    crate::tests::solo_slot();
    let uuid = rc_client::offline_uuid("G3F03");
    let (session, peer) = rc_client::memory::session(data.clone(), uuid).expect("session");
    let opts = crate::Options {
        world: dir.clone(),
        name: "G3F03".into(),
        uuid,
        info: rc_proto_v777::play::ClientInformation { view_distance: 2, ..Default::default() },
        sim_distance: 12,
    };
    let mut s = match Server::open(opts, uuid, data, peer, Arc::new(AtomicBool::new(false))) {
        Ok(s) => s,
        Err((_, e)) => panic!("opening: {e}"),
    };
    s.loaded = true;
    s.teleport = None;
    s.game_mode = 0;
    s.load_now(&[(-1, -1), (-1, 0), (0, -1), (0, 0)]);
    T { s, session, dir }
}

fn open(tag: &str) -> T {
    open_with(tag, |_| {})
}

fn write(path: &std::path::Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn item(id: &str, count: i32) -> Option<Stack> {
    Some(Stack { id: id.into(), count, components: None })
}

fn run(s: &mut Server, c: &str) -> (bool, i32) {
    let src = s.server_source();
    s.perform(src, c)
}

fn run_as_player(s: &mut Server, c: &str) -> (bool, i32) {
    let mut src = s.player_source();
    src.level = 4;
    s.perform(src, c)
}

fn strings(v: Option<&Nbt>) -> Vec<String> {
    crate::nbtio::list(v).iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()
}

/// Changing slot while eating stops the use: the item of the new slot is not
/// consumed with the old slot's timer. The off-hand use continues.
#[test]
fn changing_the_held_slot_stops_eating() {
    let mut t = open("held");
    let s = &mut t.s;
    s.held = 0;
    s.slots[36] = item("minecraft:bread", 4);
    s.slots[37] = item("minecraft:golden_apple", 1);
    s.life.vit.food = 10;
    s.on_use_item(0);
    assert!(s.life.vit.using.is_some_and(|u| u.0 == 0), "the bread is being eaten");
    s.handle(Input::HeldSlot(1));
    assert_eq!(s.held, 1);
    assert!(s.life.vit.using.is_none(), "use stopped on slot change");
    for _ in 0..40 {
        s.game_tick();
    }
    assert_eq!(s.slots[37].as_ref().map(|x| x.count), Some(1), "golden apple intact");
    assert_eq!(s.slots[36].as_ref().map(|x| x.count), Some(4), "bread intact");
    // Same slot: nothing changes; off hand: the use continues.
    s.on_use_item(0);
    s.handle(Input::HeldSlot(1));
    assert!(s.life.vit.using.is_some(), "same slot: use kept");
    s.life.vit.using = Some((1, 20));
    s.handle(Input::HeldSlot(2));
    assert_eq!(s.life.vit.using, Some((1, 20)), "off hand: use kept");
}

/// While paused, the time spent in the world still counts (and nothing else).
#[test]
fn paused_ticks_count_total_world_time() {
    let mut t = open("pause");
    let s = &mut t.s;
    let w0 = s.stat_value("minecraft:custom", "minecraft:total_world_time");
    let p0 = s.stat_value("minecraft:custom", "minecraft:play_time");
    let tick = s.tick;
    s.paused_tick();
    s.paused_tick();
    assert_eq!(s.stat_value("minecraft:custom", "minecraft:total_world_time"), w0 + 2);
    assert_eq!(s.stat_value("minecraft:custom", "minecraft:play_time"), p0);
    assert_eq!(s.tick, tick, "the world does not tick");
}

/// Autosave deadline: 6000 ticks first, then 300 s at the rate;
/// `tick rate` only brings it closer.
#[test]
fn autosave_follows_the_tick_rate() {
    let mut t = open("autosave");
    let s = &mut t.s;
    assert_eq!(s.autosave, 6000);
    assert!(run_as_player(s, "tick rate 1").0);
    assert_eq!(s.autosave, 300, "rate 1: 300 ticks");
    assert!(run_as_player(s, "tick rate 100").0);
    assert_eq!(s.autosave, 300, "never pushed back");
    s.autosave = 1000;
    assert!(run_as_player(s, "tick rate 2").0);
    assert_eq!(s.autosave, 600);
}

/// Function tags: nested ones flattened in order, duplicates removed; cycle,
/// required function absent: tag refused; optional entry absent: skipped.
#[test]
fn nested_function_tags_are_resolved() {
    let t = open_with("ftags", |dir| {
        let p = dir.join("datapacks").join("t");
        write(&p.join("pack.mcmeta"), r#"{"pack":{"description":"t","min_format":121,"max_format":121}}"#);
        write(&p.join("data/t/function/a.mcfunction"), "say a\n");
        write(&p.join("data/t/function/b.mcfunction"), "say b\n");
        let tags = p.join("data/t/tags/function");
        write(&tags.join("inner.json"), r#"{"values":["t:a"]}"#);
        write(&tags.join("outer.json"), r##"{"values":["#t:inner","t:b","t:a"]}"##);
        write(&tags.join("cyc1.json"), r##"{"values":["#t:cyc2"]}"##);
        write(&tags.join("cyc2.json"), r##"{"values":["#t:cyc1"]}"##);
        write(&tags.join("opt.json"), r##"{"values":[{"id":"t:missing","required":false},"t:b"]}"##);
        write(&tags.join("req.json"), r#"{"values":["t:missing","t:a"]}"#);
    });
    let ft = &t.s.cmd.function_tags;
    assert_eq!(ft.get("t:outer").map(Vec::as_slice), Some(&["t:a".to_owned(), "t:b".to_owned()][..]));
    assert_eq!(ft.get("t:opt").map(Vec::as_slice), Some(&["t:b".to_owned()][..]));
    for k in ["t:cyc1", "t:cyc2", "t:req"] {
        assert!(!ft.contains_key(k), "{k} should have been refused");
    }
}

/// Pack drop-in: world packs (folder, archive) added automatically and stacked on the
/// jar (file replaced, tag completed), disabled pack or without `pack.mcmeta`
/// ignored, selection rewritten in level.dat.
#[test]
fn world_packs_are_selected_and_stacked() {
    let t = open_with("stack", |dir| {
        let dp = dir.join("datapacks");
        write(&dp.join("a/pack.mcmeta"), r#"{"pack":{"description":"a","min_format":121,"max_format":121}}"#);
        write(&dp.join("a/data/minecraft/tags/block/mineable/pickaxe.json"), r#"{"values":["minecraft:dirt"]}"#);
        write(&dp.join("a/data/minecraft/loot_table/blocks/dirt.json"), r#"{"type":"minecraft:block","pools":[],"random_sequence":"g3f03:a"}"#);
        // Archive: replaces the loot table of pack "a" (above it).
        let f = std::fs::File::create(dp.join("b.zip")).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let o = zip::write::SimpleFileOptions::default();
        z.start_file("pack.mcmeta", o).unwrap();
        z.write_all(br#"{"pack":{"description":"b","min_format":121,"max_format":121}}"#).unwrap();
        z.start_file("data/minecraft/loot_table/blocks/dirt.json", o).unwrap();
        z.write_all(br#"{"type":"minecraft:block","pools":[],"random_sequence":"g3f03:b"}"#).unwrap();
        z.finish().unwrap();
        // Disabled in level.dat, and folder without header: ignored.
        write(&dp.join("off/pack.mcmeta"), r#"{"pack":{"description":"off"}}"#);
        write(&dp.join("off/data/minecraft/loot_table/blocks/dirt.json"), r#"{"random_sequence":"g3f03:off"}"#);
        write(&dp.join("bare/data/minecraft/loot_table/blocks/dirt.json"), r#"{"random_sequence":"g3f03:bare"}"#);
        crate::storage::set_data_config(dir, &["vanilla".to_owned()], &["file/off".to_owned()], &["minecraft:vanilla".to_owned()])
            .unwrap();
    });
    let s = &t.s;
    let dirt = s.data.state(s.default_state("minecraft:dirt")).expect("dirt").block as u32;
    let stone = s.data.state(s.default_state("minecraft:stone")).expect("stone").block as u32;
    let tag = s.block_tags.get("minecraft:mineable/pickaxe").expect("tag");
    assert!(tag.binary_search(&dirt).is_ok(), "dirt added by the pack");
    assert!(tag.binary_search(&stone).is_ok(), "jar stone kept");
    let loot = s.life.pack.get("loot_table", "blocks/dirt").expect("loot");
    assert_eq!(loot["random_sequence"], "g3f03:b", "archive above the folder");
    let d = s.level.root.get("Data");
    let dp = d.and_then(|d| d.get("DataPacks"));
    assert_eq!(strings(dp.and_then(|x| x.get("Enabled"))), ["vanilla", "file/a", "file/b.zip"]);
    let disabled = strings(dp.and_then(|x| x.get("Disabled")));
    assert!(disabled.contains(&"file/off".to_owned()) && disabled.contains(&"trade_rebalance".to_owned()));
    assert!(!disabled.contains(&"file/bare".to_owned()), "folder without header: not a pack");
    assert_eq!(s.features, ["minecraft:vanilla"]);
}

/// Experiments: the world flags make the jar's function pack load (its
/// trades replace those of the base game) and are announced to the client.
#[test]
fn experiments_enable_their_feature_pack() {
    let vanilla = open("exp-off");
    let base = vanilla.s.life.pack.get("tags/villager_trade", "armorer/level_1").expect("base trades");
    assert!(base["values"].to_string().contains("emerald_iron_boots"));
    drop(vanilla);
    let mut t = open_with("exp-on", |dir| {
        let feats = ["minecraft:vanilla".to_owned(), "minecraft:trade_rebalance".to_owned()];
        crate::storage::set_data_config(dir, &["vanilla".to_owned(), "trade_rebalance".to_owned()], &[], &feats).unwrap();
    });
    assert_eq!(t.s.features, ["minecraft:vanilla", "minecraft:trade_rebalance"]);
    let v = t.s.life.pack.get("tags/villager_trade", "armorer/level_1").expect("rebalanced trades");
    assert_eq!(v["values"], serde_json::json!(["minecraft:smith/1/coal_emerald", "minecraft:armorer/1/iron_ingot_emerald"]));
    while t.session.events.try_recv().is_ok() {}
    t.s.join();
    let mut seen = None;
    while let Ok(e) = t.session.events.try_recv() {
        if let Event::EnabledFeatures(f) = e {
            seen = Some(f);
        }
    }
    assert_eq!(seen.as_deref(), Some(&["minecraft:vanilla".to_owned(), "minecraft:trade_rebalance".to_owned()][..]));
}

/// `execute if|unless stopwatch`: range in seconds, unknown stopwatch is an error.
#[test]
fn execute_if_stopwatch() {
    let mut t = open("stopwatch");
    let s = &mut t.s;
    assert!(run(s, "stopwatch create t:w").0);
    assert!(run(s, "execute if stopwatch t:w 0.. run say ok").0);
    assert!(!run(s, "execute if stopwatch t:w 1000.. run say ok").0);
    assert!(run(s, "execute unless stopwatch t:w 1000.. run say ok").0);
    assert!(!run(s, "execute if stopwatch t:none 0.. run say ok").0, "unknown: error");
}

/// `execute if|unless slots`: number of slots of the range (empty ones included); source without
/// container: error.
#[test]
fn execute_if_slots() {
    let mut t = open("slots");
    let s = &mut t.s;
    s.slots.fill(None);
    assert_eq!(run_as_player(s, "execute if slots entity @s container.*"), (true, 36));
    assert_eq!(run_as_player(s, "execute if slots entity @s weapon.mainhand"), (true, 1));
    assert!(!run_as_player(s, "execute unless slots entity @s armor.*").0, "four armor slots");
    // Same source for `if items`: the player (and not a block) is read.
    s.slots[36 + s.held as usize] = item("minecraft:bread", 3);
    assert_eq!(run_as_player(s, "execute if items entity @s weapon.mainhand minecraft:bread"), (true, 3));
    let p = [2, 100, 2];
    let cmd = format!("setblock {} {} {} minecraft:hopper", p[0], p[1], p[2]);
    assert!(run(s, &cmd).0);
    assert!(s.be_get(p).is_some(), "hopper block entity: {:?}", crate::blocks::probe::state_string(s, s.get(p)));
    assert_eq!(run(s, &format!("execute if slots block {} {} {} container.*", p[0], p[1], p[2])), (true, 5));
    let cmd = format!("setblock {} {} {} minecraft:stone", p[0], p[1], p[2]);
    assert!(run(s, &cmd).0);
    assert!(!run(s, &format!("execute if slots block {} {} {} container.*", p[0], p[1], p[2])).0);
}

/// `execute store`: unknown bar and block without a block entity stop the command from the
/// redirect (the final command does not run).
#[test]
fn execute_store_targets_are_validated() {
    let mut t = open("store");
    let s = &mut t.s;
    assert!(run(s, "scoreboard objectives add g dummy").0);
    assert!(!run(s, "execute store result bossbar t:none value run scoreboard players set #x g 1").0);
    assert_eq!(s.cmd.scores.get("#x", "g"), None, "final command not run");
    let p = [2, 100, 2];
    assert!(run(s, &format!("setblock {} {} {} minecraft:stone", p[0], p[1], p[2])).0);
    let c = format!("execute store result block {} {} {} Foo int 1 run scoreboard players set #y g 1", p[0], p[1], p[2]);
    assert!(!run(s, &c).0);
    assert_eq!(s.cmd.scores.get("#y", "g"), None);
    assert!(run(s, "bossbar add t:bar \"b\"").0);
    assert!(run(s, "execute store result bossbar t:bar value run scoreboard players set #z g 7").0);
}

/// `execute facing`: the game's arctangent (table) and π as a 32-bit float, like the game.
#[test]
fn execute_facing_uses_the_game_math() {
    let t = open("facing");
    let s = &t.s;
    let mut src = s.server_source();
    src.pos = [0.0, 0.0, 0.0];
    src.entity = None;
    src.eyes = false;
    let pi = f64::from(std::f32::consts::PI);
    let mut differs = false;
    for (i, target) in [[3.0, 1.7, 5.3], [-2.25, 0.4, 7.9], [11.1, -6.0, -0.3], [0.7, 9.0, -4.4]].into_iter().enumerate() {
        let r = s.facing(&src, target);
        let (dx, dy, dz) = (target[0], target[1], target[2]);
        let sd = (dx * dx + dz * dz).sqrt();
        let pitch = crate::mobs::mth::wrap((-(crate::mobs::mth::atan2(dy, sd) * 180.0 / pi)) as f32);
        let yaw = crate::mobs::mth::wrap((crate::mobs::mth::atan2(dz, dx) * 180.0 / pi) as f32 - 90.0);
        assert_eq!((r.rot[0].to_bits(), r.rot[1].to_bits()), (pitch.to_bits(), yaw.to_bits()), "target {i}");
        let exact = crate::mobs::mth::wrap((dz.atan2(dx).to_degrees() - 90.0) as f32);
        differs |= exact.to_bits() != yaw.to_bits();
    }
    assert!(differs, "the targets must tell the exact computation from the game's");
}

/// Registry tag resolved (numbers), `None` if absent or refused.
fn reg_tag(s: &Server, reg: &str, tag: &str) -> Option<Vec<i32>> {
    let (_, t) = s.regs.tags.iter().find(|(r, _)| r == reg)?;
    t.iter().find(|(n, _)| n == tag).map(|(_, v)| v.clone())
}

/// Reload: the registries stay those of the opening. A pack enabled by `/datapack`
/// that adds an enchantment sorted first does not shift the tag numbers, and a
/// tag that requires the new enchantment is refused (unknown to the frozen registry).
#[test]
fn reload_keeps_the_frozen_registries() {
    let mut t = open("frozen");
    let before = reg_tag(&t.s, "minecraft:enchantment", "minecraft:in_enchanting_table").expect("jar tag");
    let synced = t.s.regs.synced.clone();
    let p = t.dir.join("datapacks").join("x");
    write(&p.join("pack.mcmeta"), r#"{"pack":{"description":"x","min_format":121,"max_format":121}}"#);
    write(&p.join("data/a/enchantment/aaa.json"), "{}");
    write(&p.join("data/a/tags/enchantment/uses_new.json"), r#"{"values":["a:aaa"]}"#);
    write(&p.join("data/a/tags/enchantment/old_only.json"), r#"{"values":["minecraft:sharpness"]}"#);
    assert!(run(&mut t.s, "datapack enable \"file/x\"").0, "activation");
    assert_eq!(strings(t.s.level.root.get("Data").and_then(|d| d.get("DataPacks")).and_then(|x| x.get("Enabled"))), ["vanilla", "file/x"]);
    assert_eq!(t.s.regs.synced, synced, "frozen registries");
    assert_eq!(reg_tag(&t.s, "minecraft:enchantment", "minecraft:in_enchanting_table").as_ref(), Some(&before), "numbers unchanged");
    assert_eq!(reg_tag(&t.s, "minecraft:enchantment", "a:uses_new"), None, "entry unknown to the frozen registry");
    let sharp = synced.iter().find(|(n, _)| n == "minecraft:enchantment").unwrap().1.iter().position(|e| e == "minecraft:sharpness");
    assert_eq!(reg_tag(&t.s, "minecraft:enchantment", "a:old_only"), Some(vec![sharp.unwrap() as i32]), "new tag loaded");
}

/// `filter` section of a pack: the designated files of the packs below disappear
/// (a loot table, a whole tag), not those of the pack itself.
#[test]
fn pack_filter_hides_the_packs_below() {
    let t = open_with("filter", |dir| {
        let p = dir.join("datapacks").join("f");
        write(
            &p.join("pack.mcmeta"),
            r#"{"pack":{"description":"f","min_format":121,"max_format":121},
                "filter":{"block":[{"namespace":"minecraft","path":"^loot_table/blocks/dirt\\.json$|tags/block/mineable/pickaxe"}]}}"#,
        );
        write(&p.join("data/minecraft/tags/block/mineable/pickaxe.json"), r#"{"values":["minecraft:dirt"]}"#);
    });
    let s = &t.s;
    assert!(s.life.pack.get("loot_table", "blocks/dirt").is_none(), "jar loot filtered");
    assert!(s.life.pack.get("loot_table", "blocks/stone").is_some(), "other loot kept");
    let dirt = s.data.state(s.default_state("minecraft:dirt")).expect("dirt").block as u32;
    let stone = s.data.state(s.default_state("minecraft:stone")).expect("stone").block as u32;
    let tag = s.block_tags.get("minecraft:mineable/pickaxe").expect("pack tag");
    assert!(tag.binary_search(&dirt).is_ok(), "pack tag kept");
    assert!(tag.binary_search(&stone).is_err(), "jar tag filtered");
}

/// Tag with valid JSON but invalid content: it exists, empty (the game creates it before
/// reading its content); the one that cites it is loaded. Invalid JSON syntax: absent.
#[test]
fn tag_with_invalid_content_exists_empty() {
    let t = open_with("badtag", |dir| {
        let p = dir.join("datapacks").join("p");
        write(&p.join("pack.mcmeta"), r#"{"pack":{"description":"p","min_format":121,"max_format":121}}"#);
        let tags = p.join("data/p/tags/block");
        write(&tags.join("bad.json"), r#"{"vals":[]}"#);
        write(&tags.join("use.json"), r##"{"values":["#p:bad","minecraft:dirt"]}"##);
        write(&tags.join("broken.json"), r##"{"values":["#p:bad""##);
        write(&tags.join("use_broken.json"), r##"{"values":["#p:broken"]}"##);
    });
    let s = &t.s;
    let dirt = s.data.state(s.default_state("minecraft:dirt")).expect("dirt").block as i32;
    assert_eq!(reg_tag(s, "minecraft:block", "p:bad"), Some(Vec::new()));
    assert_eq!(reg_tag(s, "minecraft:block", "p:use"), Some(vec![dirt]));
    assert_eq!(reg_tag(s, "minecraft:block", "p:broken"), None);
    assert_eq!(reg_tag(s, "minecraft:block", "p:use_broken"), None);
}

/// Tags of the `worldgen/…` data registries outside the synchronized registries: filed
/// under their full name for the parser (like `/locate structure #…`), never under
/// `minecraft:worldgen`.
#[test]
fn worldgen_data_tags_keep_their_registry() {
    let t = open("wgtags");
    let d = &t.s.cmd.data;
    assert!(d.has_tag("minecraft:worldgen/feature", "minecraft:can_spawn_from_bone_meal"));
    assert!(d.has_tag("minecraft:worldgen/structure", "minecraft:village"));
    assert!(d.tag_names("minecraft:worldgen").is_empty(), "'worldgen' is not a registry");
}
