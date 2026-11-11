//! Data reload (`/reload`): recipes, loot data and tags are read again from the pack stack while
//! the frozen registries stay those of the opening; every player in game (host and LAN guests)
//! then gets the tags, the item sets of the recipes and their recipe book again, in the game's
//! order; a guest who connects later gets the reloaded tags.
//! Fresh flat world: the 26.3 data cache and the client jar are required (absent: FAILURE).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rc_client::Event;
use rc_client::recipes::RecipeEvent;

use crate::blocks::probe::Probe;
use crate::newworld::{NewWorld, Preset, create_world};

const PACK_META: &str = r#"{"pack":{"description":"reload tests","min_format":121,"max_format":121}}"#;
const DIRT_TO_DIAMOND: &str =
    r#"{"type":"minecraft:crafting_shapeless","ingredients":["minecraft:dirt"],"result":{"id":"minecraft:diamond"}}"#;
const DIRT_TO_EMERALD: &str =
    r#"{"type":"minecraft:crafting_shapeless","ingredients":["minecraft:dirt"],"result":{"id":"minecraft:emerald"}}"#;
const DIRT_SLAB_CUTTING: &str =
    r#"{"type":"minecraft:stonecutting","ingredient":"minecraft:dirt","result":{"id":"minecraft:oak_slab","count":2}}"#;

fn write_pack_file(world: &Path, path: &str, text: &str) {
    let file = world.join("datapacks").join("p").join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, text).unwrap();
}

/// Fresh flat world with data pack `p` holding `pack_files`, opened.
fn world_with_pack(tag: &str, pack_files: &[(&str, &str)]) -> (Probe, PathBuf) {
    let data = Arc::new(rc_testdata::version_data().expect("data reload tests: 26.3 data cache missing"));
    assert!(crate::registries::client_jar(&data).is_some(), "data reload tests: 26.3 client jar missing");
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-data-reload-{tag}-{}", std::process::id())));
    let mut w = NewWorld::new(tag);
    w.seed = 1;
    w.preset = Preset::Flat;
    w.allow_commands = true;
    create_world(&dir, &data, &w).expect("world creation");
    write_pack_file(&dir, "pack.mcmeta", PACK_META);
    for (path, text) in pack_files {
        write_pack_file(&dir, path, text);
    }
    let mut p = Probe::open(&dir, data);
    p.s.loaded = true;
    p.s.teleport = None;
    let _ = p.take_events();
    (p, dir)
}

fn reload(p: &mut Probe) {
    let mut src = p.s.player_source();
    src.level = 4;
    assert!(p.s.perform(src, "reload").0, "reload refused");
}

/// Kinds of the reload events, in the order sent (other events left out).
fn reload_event_kinds(events: &[Event]) -> Vec<&'static str> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Tags(_) => Some("tags"),
            Event::UpdateRecipes(_) => Some("recipe item sets"),
            Event::Recipes(RecipeEvent::Settings(_)) => Some("book settings"),
            Event::Recipes(RecipeEvent::Add(_, true)) => Some("book entries replaced"),
            Event::Recipes(_) => Some("other book event"),
            _ => None,
        })
        .collect()
}

/// Opens the world to the local network and seats a guest without a real connection; returns
/// the reception of its packets.
fn add_guest(p: &mut Probe, name: &str) -> crossbeam_channel::Receiver<Vec<u8>> {
    crate::lan::announce::LOCAL_ONLY.store(true, std::sync::atomic::Ordering::Relaxed);
    if p.s.lan.is_none() {
        let req = crate::lan::Publish { open: true, port: 0, guest_commands: false, force_mode: Some(0) };
        p.s.lan_publish(&req).expect("opening to the local network");
    }
    let uuid = rc_client::offline_uuid(name);
    let mut ctx = p.s.new_player_ctx(name, uuid, 2, 0, Some(0));
    ctx.loaded = true;
    let (out, received) = crossbeam_channel::unbounded();
    let (keep_input_open, inp) = crossbeam_channel::unbounded();
    std::mem::forget(keep_input_open);
    p.s.lan.as_mut().unwrap().guests.push(crate::lan::Guest {
        name: name.into(),
        uuid,
        out,
        inp,
        ctx: Some(Box::new(ctx)),
        enc: Default::default(),
        gone: false,
        joined: true,
        net: Default::default(),
        req_view: 2,
        chunks: Default::default(),
        moves: Default::default(),
    });
    received
}

/// A recipe, a loot table and an item tag added to a pack after the opening exist after
/// `/reload`; an enchantment changed by the pack does not (registry frozen at the opening).
#[test]
fn reload_reads_recipes_loot_and_tags_again_but_not_frozen_registries() {
    let (mut p, dir) = world_with_pack("data", &[("data/p/recipe/diamond.json", DIRT_TO_DIAMOND)]);
    assert!(p.s.life.recipes.index("p:diamond").is_some(), "pack recipe read at the opening");
    let sharpness_at_opening = p.s.life.pack.get("enchantment", "sharpness").expect("sharpness");
    write_pack_file(&dir, "data/p/recipe/emerald.json", DIRT_TO_EMERALD);
    write_pack_file(&dir, "data/p/recipe/slab_cutting.json", DIRT_SLAB_CUTTING);
    write_pack_file(&dir, "data/minecraft/loot_table/blocks/dirt.json", r#"{"type":"minecraft:block","pools":[]}"#);
    write_pack_file(&dir, "data/p/tags/item/shiny.json", r#"{"values":["minecraft:diamond"]}"#);
    write_pack_file(&dir, "data/minecraft/enchantment/sharpness.json", r#"{"max_level":99}"#);
    let stonecutter_recipes = p.s.life.stations.stonecut.len();
    reload(&mut p);
    let s = &p.s;
    assert!(s.life.recipes.index("p:emerald").is_some(), "recipe added to the pack");
    assert!(s.life.recipes.index("p:diamond").is_some(), "recipe kept");
    assert_eq!(s.life.stations.stonecut.len(), stonecutter_recipes + 1, "stonecutter recipe added to the pack");
    let dirt_loot = s.life.pack.get("loot_table", "blocks/dirt").expect("dirt loot table");
    assert_eq!(dirt_loot["pools"].as_array().map(Vec::len), Some(0), "loot table replaced by the pack");
    let diamond = s.life.items.id("minecraft:diamond").expect("diamond");
    assert_eq!(s.life.items.tags.get("p:shiny"), Some(&vec![diamond]), "item tag added to the pack");
    assert_eq!(s.life.pack.get("enchantment", "sharpness"), Some(sharpness_at_opening), "enchantment frozen");
}

/// After `/reload` the player gets, in the game's order: the tags, the item sets of the recipes
/// (with a stonecutter recipe the pack just added), the settings of their recipe book, then its
/// entries, which replace those the client held.
#[test]
fn reload_sends_tags_recipes_and_recipe_book_to_the_player() {
    let (mut p, dir) = world_with_pack("resend", &[("data/p/recipe/diamond.json", DIRT_TO_DIAMOND)]);
    p.s.award_recipes(&["p:diamond".to_owned()]);
    write_pack_file(&dir, "data/p/recipe/slab_cutting.json", DIRT_SLAB_CUTTING);
    write_pack_file(&dir, "data/p/tags/block/marked.json", r#"{"values":["minecraft:dirt"]}"#);
    let stonecutter_recipes = p.s.life.stations.stonecut.len();
    let _ = p.take_events();
    reload(&mut p);
    let events = p.take_events();
    assert_eq!(
        reload_event_kinds(&events),
        ["tags", "recipe item sets", "book settings", "book entries replaced"],
        "what the player gets after a reload, in order"
    );
    for e in &events {
        match e {
            Event::Tags(tags) => {
                let block_tags = &tags.iter().find(|(registry, _)| registry == "minecraft:block").expect("block tags").1;
                assert!(block_tags.iter().any(|(name, _)| name == "p:marked"), "the tag the pack just added is sent");
            }
            Event::UpdateRecipes(sets) => {
                assert_eq!(sets.stonecutter.len(), stonecutter_recipes + 1, "the recipe the pack just added is sent");
            }
            Event::Recipes(RecipeEvent::Add(entries, _)) => {
                assert_eq!(entries.len(), 1, "the one recipe the player knows");
            }
            _ => {}
        }
    }
}

/// A reload run from another dimension (`execute in`, sends cut off while the command runs
/// there) still reaches the player.
#[test]
fn reload_run_from_another_dimension_reaches_the_player() {
    let (mut p, _dir) = world_with_pack("other-dimension", &[]);
    let mut src = p.s.player_source();
    src.level = 4;
    assert!(p.s.perform(src, "execute in minecraft:the_nether run reload").0, "reload refused");
    assert_eq!(
        reload_event_kinds(&p.take_events()),
        ["tags", "recipe item sets", "book settings", "book entries replaced"],
        "what the player gets after a reload run from the Nether"
    );
}

/// With the world open to the local network, a guest gets the same four packets in the same
/// order as the host.
#[test]
fn reload_reaches_lan_guests() {
    let (mut p, dir) = world_with_pack("lan", &[]);
    let guest_packets = add_guest(&mut p, "Guest");
    write_pack_file(&dir, "data/p/tags/block/marked.json", r#"{"values":["minecraft:dirt"]}"#);
    while guest_packets.try_recv().is_ok() {}
    reload(&mut p);
    let ids = *p.s.peer.ids();
    let reload_packets = [
        (ids.update_tags, "tags"),
        (ids.update_recipes, "recipe item sets"),
        (ids.recipe_book_settings, "book settings"),
        (ids.recipe_book_add, "book entries"),
    ];
    let received: Vec<&str> = guest_packets
        .try_iter()
        .filter_map(|packet| {
            let id = rc_proto::varint::read_varint(&mut rc_proto::Reader::new(&packet)).ok()?;
            reload_packets.iter().find(|(packet_id, _)| *packet_id == id).map(|(_, kind)| *kind)
        })
        .collect();
    assert_eq!(received, ["tags", "recipe item sets", "book settings", "book entries"], "packets of the guest");
    let host_events = p.take_events();
    assert_eq!(
        reload_event_kinds(&host_events),
        ["tags", "recipe item sets", "book settings", "book entries replaced"],
        "events of the host"
    );
}

/// An advancement added to a pack exists, after `/reload`, in the progress of every player in
/// game: the host's and the guest's.
#[test]
fn reload_reads_the_advancements_again_for_every_player() {
    let (mut p, dir) = world_with_pack("advancements", &[]);
    let _guest_packets = add_guest(&mut p, "Guest");
    write_pack_file(&dir, "data/p/advancement/added.json", r#"{"criteria":{"never":{"trigger":"minecraft:impossible"}}}"#);
    reload(&mut p);
    assert!(p.s.prog.tree.index.contains_key("p:added"), "progress of the host");
    let guest = p.s.lan.as_ref().and_then(|lan| lan.guests[0].ctx.as_ref()).expect("guest seated");
    assert!(guest.prog.tree.index.contains_key("p:added"), "progress of the guest");
}

/// The tags kept for the guests who connect after a reload are the reloaded ones.
#[test]
fn guests_connecting_after_a_reload_get_the_reloaded_tags() {
    let (mut p, dir) = world_with_pack("lan-later", &[]);
    let _guest_packets = add_guest(&mut p, "Guest");
    write_pack_file(&dir, "data/p/tags/block/marked.json", r#"{"values":["minecraft:dirt"]}"#);
    reload(&mut p);
    let lan = p.s.lan.as_ref().expect("world open to the local network");
    let tags_for_later_guests = lan.shared.tags.lock().unwrap();
    let block_tags = &tags_for_later_guests.iter().find(|(registry, _)| registry == "minecraft:block").expect("block tags").1;
    assert!(block_tags.iter().any(|(name, _)| name == "p:marked"), "later guests get the reloaded tags");
}

/// Bench (loaded machine allowed): `cargo test --release -p rc-sim --lib bench_reload_cost --
/// --nocapture`. Whole `/reload` of a world with the jar's data alone, and what the data read
/// again and the resend to the player cost within it.
#[test]
fn bench_reload_cost() {
    let (mut p, _dir) = world_with_pack("bench", &[]);
    let median_ms = |run: &mut dyn FnMut()| {
        let mut samples: Vec<f64> = (0..7)
            .map(|_| {
                let start = std::time::Instant::now();
                run();
                start.elapsed().as_secs_f64() * 1e3
            })
            .collect();
        samples.sort_by(f64::total_cmp);
        samples[samples.len() / 2]
    };
    let whole = median_ms(&mut || p.s.s9_reload());
    let jar = crate::registries::Jar::read(&crate::registries::client_jar(&p.s.data).expect("jar")).expect("jar read");
    let pack = median_ms(&mut || {
        std::hint::black_box(p.s.life.pack.reloaded(&jar));
    });
    let recipes = median_ms(&mut || {
        let recipes = crate::recipes::Recipes::load(&p.s.life.pack, &p.s.life.items);
        let enchantments = p.s.regs.entries("minecraft:enchantment");
        std::hint::black_box(crate::stations::Data::load(&p.s.life.pack, &p.s.life.items, &recipes, enchantments));
    });
    let resend = median_ms(&mut || {
        let _ = p.s.send(Event::Tags(p.s.regs.tags.clone()));
        p.s.send_recipe_sets();
        p.s.send_recipe_book();
    });
    eprintln!(
        "[bench] /reload: whole {whole:.2} ms; within it, pack files {pack:.2} ms, recipes and stations {recipes:.2} ms, \
         resend to one player {resend:.2} ms (medians of 7)"
    );
}

/// The recipe ranks a player holds (last recipe crafted, recipes crafted this tick) follow the
/// recipes when a reload shifts the list.
#[test]
fn held_recipe_ranks_follow_the_reloaded_list() {
    let (mut p, dir) = world_with_pack("ranks", &[]);
    let stick = p.s.life.recipes.index("minecraft:stick").expect("stick recipe");
    let torch = p.s.life.recipes.index("minecraft:torch").expect("torch recipe");
    p.s.life.last_recipe = Some(stick);
    p.s.life.pending.crafted = vec![torch, stick];
    // Namespace `a` sorts before `minecraft`: every rank of the jar's recipes moves by one.
    write_pack_file(&dir, "data/a/recipe/first.json", DIRT_TO_DIAMOND);
    reload(&mut p);
    let life = &p.s.life;
    assert_ne!(life.recipes.index("minecraft:stick"), Some(stick), "the reload shifted the list");
    let id_at = |rank: usize| life.recipes.list[rank].id.as_str();
    assert_eq!(life.last_recipe.map(id_at), Some("minecraft:stick"), "last recipe crafted");
    let crafted: Vec<&str> = life.pending.crafted.iter().map(|&rank| id_at(rank)).collect();
    assert_eq!(crafted, ["minecraft:torch", "minecraft:stick"], "recipes crafted this tick");
}
