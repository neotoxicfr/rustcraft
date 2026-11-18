//! Proofs and micro-benches of the creative screen switches: lists with and without the hash
//! table, the grid by places against copies, `#` tags matched once against once per stack, and
//! the drawing that reads the stacks in place against the copies made at every frame.

use super::*;
use crate::gui::tests_opt;
use crate::tooltips::test_fixture::TooltipWorld;

/// One switch on, the others off: each lead alone.
fn each_switch_alone() -> [(&'static str, CreativeOpts); 5] {
    let o = CreativeOpts::OLD;
    [
        ("draw without copies", CreativeOpts { draw_without_copies: true, ..o }),
        ("hashed dedup", CreativeOpts { hashed_dedup: true, ..o }),
        ("grid by index", CreativeOpts { grid_by_index: true, ..o }),
        ("tags matched once", CreativeOpts { tags_matched_once: true, ..o }),
        ("indexed names", CreativeOpts { indexed_names: true, ..o }),
    ]
}

/// Words of item names that make the item tags of the proofs.
const TAG_WORDS: [&str; 40] = [
    "sword",
    "log",
    "planks",
    "wool",
    "stone",
    "ingot",
    "dye",
    "glass",
    "boat",
    "door",
    "slab",
    "stairs",
    "wall",
    "fence",
    "banner",
    "bed",
    "candle",
    "carpet",
    "concrete",
    "terracotta",
    "coral",
    "copper",
    "ore",
    "sapling",
    "leaves",
    "seeds",
    "pickaxe",
    "axe",
    "shovel",
    "hoe",
    "helmet",
    "boots",
    "bucket",
    "egg",
    "music_disc",
    "sherd",
    "trim",
    "bundle",
    "shulker_box",
    "rail",
];

/// Registries and tags as a server sends them: the game's base paintings and enchantments, the
/// goat horns, the banner patterns the tabs name, and item tags made from words of the item names
/// (with, without and with another namespace).
fn received(world: &TooltipWorld) -> Received {
    let mut rec = Received::default();
    let entries = |names: Vec<String>| names.into_iter().map(|n| (n, None)).collect::<Vec<_>>();
    let creative = &world.data.creative;
    rec.registry("minecraft:painting_variant", &entries(creative.paintings.iter().map(|p| p.0.clone()).collect()));
    rec.registry("minecraft:enchantment", &entries(creative.enchantments.iter().map(|e| e.0.clone()).collect()));
    let horns = ["ponder", "sing", "seek", "feel", "admire", "call", "yearn", "dream"];
    rec.registry("minecraft:instrument", &entries(horns.map(|h| format!("minecraft:{h}_goat_horn")).to_vec()));
    let mut patterns: Vec<String> = Vec::new();
    for tab in &creative.tabs {
        for entry in &tab.entries {
            if let rc_data::CreativeEntry::Stack(s) = entry {
                for (p, _) in &s.banner {
                    if !patterns.contains(p) {
                        patterns.push(p.clone());
                    }
                }
            }
        }
    }
    rec.registry("minecraft:banner_pattern", &entries(patterns));
    let ids_with = |word: &str| -> Vec<i32> {
        let names = world.item_names.iter().enumerate();
        names.filter(|(_, n)| n.contains(word)).map(|(i, _)| i as i32).collect()
    };
    let item_tags = TAG_WORDS
        .iter()
        .enumerate()
        .map(|(k, word)| {
            let name = match k % 3 {
                0 => format!("minecraft:{word}s"),
                1 => format!("{word}s"),
                _ => format!("c:{word}s"),
            };
            (name, ids_with(word))
        })
        .collect();
    let paintings = (0..creative.paintings.len() as i32).filter(|i| i % 4 != 0).collect();
    rec.tags(&vec![
        ("minecraft:item".to_owned(), item_tags),
        ("minecraft:painting_variant".to_owned(), vec![("minecraft:placeable".to_owned(), paintings)]),
        ("minecraft:instrument".to_owned(), vec![("minecraft:goat_horns".to_owned(), (0..8).collect())]),
    ]);
    rec
}

fn params(permission: bool) -> Params {
    Params { features: vec!["minecraft:vanilla".to_owned()], permission, generation: 0 }
}

/// Queries of the proofs: words, namespaces, tags, edge cases.
const QUERIES: [&str; 50] = [
    "",
    "a",
    "st",
    "stone",
    "STONE",
    " stone ",
    "diamond",
    "sword",
    "red",
    "light",
    "potion",
    "enchant",
    "book",
    "sharp",
    "horn",
    "painting",
    "banner",
    "zzz",
    "e",
    "of",
    "minecraft:",
    "minecraft:sto",
    "mine:dia",
    ":",
    "c:",
    "a:b",
    "minecraft:diamond",
    "x:",
    "#",
    "#sw",
    "#minecraft:",
    "#c:",
    "#:log",
    "#a:b",
    "#wool",
    "#minecraft:wool",
    "#c:ingot",
    "#bed",
    "#minecraft:sword",
    "# ",
    "#zz",
    "spawn",
    "egg",
    "music",
    "copper",
    "oxidized",
    "glass",
    "épée",
    "lingering",
    "trim",
];

/// A screen on the game's lists, the search tab selected and its index built.
fn search_screen(world: &TooltipWorld, rec: &Received, opts: CreativeOpts) -> CreativeScreen {
    let mut screen = CreativeScreen::with_opts(opts);
    screen.update_lists(&world.data, rec, params(true), &[]);
    let search = world.data.creative.tabs.iter().position(|t| t.kind == CreativeKind::Search).expect("search tab");
    screen.select(&world.data, search, &[]);
    screen.ensure_index(&world.english, &world.items(), &world.enchantments, rec);
    screen
}

/// Every stack of the grid, place by place.
fn grid_stacks(screen: &CreativeScreen) -> Vec<Option<ItemStack>> {
    (0..screen.grid_len()).map(|place| screen.grid.get(&screen.lists, place).cloned()).collect()
}

/// The lists built with the hash table are the lists built with the walk, in the same order, with
/// and without the operator tab.
#[test]
fn lists_are_the_same_with_and_without_the_hash_table() {
    let world = TooltipWorld::new();
    let rec = received(&world);
    for permission in [false, true] {
        let [mut walk, mut hashed] = [CreativeLists::default(), CreativeLists::default()];
        assert!(walk.update(&world.data, &rec, params(permission), false));
        assert!(hashed.update(&world.data, &rec, params(permission), true));
        assert!(walk.search.len() > 1_500, "search list of {} stacks", walk.search.len());
        assert!(walk.tab.iter().filter(|l| !l.is_empty()).count() > 8);
        assert_eq!(walk.tab, hashed.tab, "tab lists, permission {permission}");
        assert_eq!(walk.for_search, hashed.for_search, "search lists of the tabs, permission {permission}");
        assert_eq!(walk.search, hashed.search, "search list, permission {permission}");
    }
}

/// The former query, over the tags as names (the index held copies of the names per stack).
fn former_query(
    idx: &SearchIndex,
    tag_names: &[Vec<String>],
    query: &str,
    all_tags: &[(String, Vec<i32>)],
) -> (Vec<usize>, Vec<String>) {
    let ns_path = |id: &str| -> (String, String) {
        let (a, b) = id.split_once(':').unwrap_or(("minecraft", id));
        (a.to_owned(), b.to_owned())
    };
    let split = |q: &str| q.split_once(':').map(|(a, b)| (a.trim().to_owned(), b.trim().to_owned()));
    let q = query.to_lowercase();
    let n = idx.ids.len();
    if q.is_empty() {
        return ((0..n).collect(), Vec::new());
    }
    if let Some(t) = q.strip_prefix('#') {
        let matches = |tag: &str| {
            let (ns, path) = ns_path(tag);
            match split(t) {
                None => path.contains(t),
                Some((a, b)) => ns.contains(&a) && path.contains(&b),
            }
        };
        let hits = (0..n).filter(|&i| tag_names[i].iter().any(|g| matches(g))).collect();
        let visible = all_tags.iter().map(|(g, _)| g.clone()).filter(|g| matches(g)).collect();
        return (hits, visible);
    }
    let in_lines = |i: usize, s: &str| idx.lines[i].iter().any(|l| l.contains(s));
    let hits = (0..n)
        .filter(|&i| match split(&q) {
            None => in_lines(i, &q),
            Some((a, b)) => {
                let (ns, path) = ns_path(&idx.ids[i]);
                ns.contains(&a) && (path.contains(&b) || in_lines(i, &b))
            }
        })
        .collect();
    (hits, Vec::new())
}

/// The 50 queries find the same places with the tags matched once or per stack, and the same as
/// the former query over tag names; the grid shows the same stacks by places or by copies, with
/// every switch alone and all together; a category tab shows the same stacks too.
#[test]
fn search_finds_the_same_stacks_on_every_path() {
    let world = TooltipWorld::new();
    let rec = received(&world);
    let mut old = search_screen(&world, &rec, CreativeOpts::OLD);
    let (_, idx) = old.index.as_ref().expect("index built");
    // Tags of each stack by name, as the former index kept them (bare names given the namespace).
    let full = |g: &str| if g.contains(':') { g.to_owned() } else { format!("minecraft:{g}") };
    let tag_names: Vec<Vec<String>> =
        idx.tags.iter().map(|places| places.iter().map(|&p| full(&old.index_tags[p as usize].0)).collect()).collect();
    let (mut hit_total, mut tag_total) = (0, 0);
    for q in QUERIES {
        let want = former_query(idx, &tag_names, q, &old.index_tags);
        for once in [false, true] {
            assert_eq!(idx.query(q, &old.index_tags, once), want, "query {q:?}, tags matched once {once}");
        }
        (hit_total, tag_total) = (hit_total + want.0.len(), tag_total + want.1.len());
    }
    assert!(hit_total > 5_000 && tag_total > 20, "queries barely find anything: {hit_total} stacks, {tag_total} tags");
    let mut others: Vec<(&str, CreativeScreen)> = each_switch_alone()
        .into_iter()
        .chain([("all", CreativeOpts::NEW)])
        .map(|(label, opts)| (label, search_screen(&world, &rec, opts)))
        .collect();
    for q in QUERIES {
        old.search.text = q.chars().collect();
        old.search_changed(&world.data);
        let want = (grid_stacks(&old), old.visible_tags.clone());
        for (label, screen) in &mut others {
            screen.search.text = q.chars().collect();
            screen.search_changed(&world.data);
            assert_eq!((grid_stacks(screen), screen.visible_tags.clone()), want, "{label}: query {q:?}");
        }
    }
    for (k, tab) in world.data.creative.tabs.iter().enumerate().filter(|(_, t)| t.kind == CreativeKind::Category) {
        old.select(&world.data, k, &[]);
        for (label, screen) in &mut others {
            screen.select(&world.data, k, &[]);
            assert_eq!(grid_stacks(screen), grid_stacks(&old), "{label}: tab {}", tab.name);
        }
    }
}

/// The game's English with every tab title and the trash tooltip translated otherwise: the same
/// number of keys, another table.
fn other_language(e: &tests_opt::Env, data: &rc_data::VersionData) -> rc_text::Lang {
    let base = e.assets.read("assets/minecraft/lang/en_us.json").unwrap_or_default();
    let keys = data.creative.tabs.iter().map(|t| t.title.as_str()).chain(["inventory.binSlot"]);
    let entries: Vec<String> = keys.map(|key| format!("\"{key}\": \"Onglet {key}\"")).collect();
    let other = format!("{{{}}}", entries.join(", "));
    rc_text::Lang::load(&base, Some(other.as_bytes()), None)
}

/// Mouse positions of the drawing proof, relative to the image's corner: none, over the grid,
/// over the hotbar row, over the trash, over a tab of each row, outside.
const MOUSE_SPOTS: [Option<(i32, i32)>; 9] = [
    None,
    Some((12, 22)),
    Some((84, 58)),
    Some((30, 115)),
    Some((175, 114)),
    Some((10, -14)),
    Some((40, 150)),
    Some((-30, 60)),
    Some((170, 40)),
];

/// The drawing that reads the stacks in place places what the old drawing places, byte for byte,
/// on every tab, with the mouse over each kind of spot, a stack carried or not, a bundle hovered;
/// with every switch alone and all together. Without an installed game or a GPU: skipped
/// (`RUSTCRAFT_OPT_REQUIRE=1`: failure).
#[test]
fn drawing_in_place_places_what_the_copies_place() {
    let Some(e) = tests_opt::env() else { return };
    let world = TooltipWorld::new();
    let rec = received(&world);
    let models = rc_assets::item::ItemModels::load(&e.assets, world.data.items.iter().map(|i| i.model.as_str()));
    let mut gui = tests_opt::gui_with_item_models(&e, tests_opt::ALL, Some(models));
    gui.begin(1600, 900);
    let stack = |name: &str| world.stack(&format!("minecraft:{name}"), Vec::new());
    let ctx = crate::bundle::Ctx { ids: &world.ids, data: &world.data };
    let bundle = ctx.with_contents(&stack("bundle"), &[stack("stone"), stack("diamond_sword")]);
    // A sword, a bundle and 12 stones in the hotbar, a map in the inventory; the second one carries
    // an emerald.
    let inventories = [false, true].map(|carrying| {
        let mut inventory = crate::inventory::PlayerInventory::default();
        inventory.menu[36] = Some(stack("diamond_sword"));
        inventory.menu[37] = Some(bundle.clone());
        inventory.menu[38] = Some(ItemStack { count: 12, ..stack("stone") });
        inventory.menu[20] = Some(stack("filled_map"));
        inventory.carried = carrying.then(|| stack("emerald"));
        inventory
    });
    let inventory = &inventories[0];
    let env = crate::stack::HudEnv::empty();
    let it = HotbarItems {
        inv: inventory,
        ids: &world.ids,
        data: &world.data,
        infos: &world.data.items,
        names: &world.item_names,
        env: &env,
        pt: 0.0,
    };
    let screens = |opts| {
        let mut screen = CreativeScreen::with_opts(opts);
        screen.open = true;
        screen.update_lists(&world.data, &rec, params(true), &[]);
        screen
    };
    let (gx, gy) = origin(&gui);
    // A first pass grows the item atlas: a frame that doubles it moves the cells of the icons
    // placed before in the same frame, which only the first of the screens drawn would show.
    let english = gui.lang.clone();
    let fonts = |opts: rc_assets::font::FontOptions| {
        let load = |id: &&str| ((*id).to_owned(), rc_assets::font::Font::load(&e.assets, id, opts));
        crate::resources::FONT_IDS.iter().map(load).collect()
    };
    for check in [false, true] {
        gui.lang = english.clone();
        gui.set_fonts(fonts(Default::default()));
        let mut pair = [screens(CreativeOpts::OLD), screens(CreativeOpts::NEW)];
        let mut alone: Vec<(&str, CreativeScreen)> =
            each_switch_alone().into_iter().map(|(label, opts)| (label, screens(opts))).collect();
        let (mut frames, mut differing, mut previous) = (0, 0, Vec::new());
        for (k, tab) in world.data.creative.tabs.iter().enumerate() {
            // Another language with the same keys at the fourth tab, other fonts at the eighth:
            // the kept titles and tooltips must follow.
            match k {
                3 => gui.lang = std::sync::Arc::new(other_language(&e, &world.data)),
                7 => gui.set_fonts(fonts(rc_assets::font::FontOptions { uniform: true, jp: false })),
                _ => {}
            }
            for (spot, mouse) in MOUSE_SPOTS.iter().enumerate() {
                let mouse = mouse.map(|(x, y)| ((gx + x) as f32 + 0.5, (gy + y) as f32 + 0.5));
                let mut frame = |screen: &mut CreativeScreen, inventory: &crate::inventory::PlayerInventory| {
                    if screen.tab != k || spot == 0 {
                        screen.select(&world.data, k, &[]);
                        screen.search.text = if spot % 2 == 0 { "sword" } else { "#c:" }.chars().collect();
                        screen.ensure_index(&gui.lang, &it, &world.enchantments, &rec);
                        screen.search_changed(&world.data);
                        screen.scroll = if spot > 4 { 0.4 } else { 0.0 };
                    }
                    gui.begin(1600, 900);
                    screen.draw(&mut gui, &e.queue, inventory, &it, mouse, &world.enchantments, 1.25);
                    tests_opt::layer_bytes(&gui)
                };
                let shown = &inventories[usize::from(spot == 3)];
                let [old, new] = pair.each_mut().map(|screen| frame(screen, shown));
                let alone_frames: Vec<(&str, Vec<u8>)> =
                    alone.iter_mut().map(|(label, screen)| (*label, frame(screen, shown))).collect();
                if !check {
                    continue;
                }
                let first = old.iter().zip(&new).position(|(a, b)| a != b);
                let around =
                    |bytes: &[u8]| first.map(|i| bytes[i.saturating_sub(24)..(i + 24).min(bytes.len())].to_vec());
                assert!(
                    old.len() == new.len() && first.is_none(),
                    "tab {} spot {spot}: layers differ (byte {first:?} of {} / {}): {:?} against {:?}",
                    tab.name,
                    old.len(),
                    new.len(),
                    around(&old),
                    around(&new)
                );
                for (label, bytes) in alone_frames {
                    assert!(bytes == old, "{label}: tab {} spot {spot}", tab.name);
                }
                differing += usize::from(previous != old);
                frames += 1;
                previous = old;
            }
        }
        if check {
            assert!(differing * 10 > frames * 8, "the scene barely moves: {differing} of {frames} frames differ");
        }
    }
}

/// Micro-bench (ignored): `cargo test --release -p rc-app bench_creative_lists_update -- --ignored
/// --nocapture`. Building the lists with the walk and with the hash table.
#[test]
#[ignore = "micro-bench"]
fn bench_creative_lists_update() {
    let world = TooltipWorld::new();
    let rec = received(&world);
    for pass in 0..3 {
        for (label, hashed) in [("walk (former)", false), ("hash table", true)] {
            let build = (0..5)
                .map(|_| {
                    let mut lists = CreativeLists::default();
                    let start = std::time::Instant::now();
                    lists.update(&world.data, &rec, params(true), hashed);
                    start.elapsed().as_secs_f64() * 1e3
                })
                .fold(f64::MAX, f64::min);
            println!("pass {pass}: {label:<14} {build:>7.3} ms");
        }
    }
}

/// Micro-bench (ignored): `cargo test --release -p rc-app bench_creative_search_index -- --ignored
/// --nocapture`. The whole creative search index (tooltip search lines, ids and tags of every stack
/// of the search list) on each way of finding the component ids.
#[test]
#[ignore = "micro-bench"]
fn bench_creative_search_index() {
    use crate::stack::ComponentLookup;
    let lookups = [
        ("walk over the names (former)", ComponentLookup::FORMER),
        ("full name, hashed", ComponentLookup { by_full_name: true, hashed: true, resolved_table: false }),
        ("hashed + table of resolved ids", ComponentLookup { by_full_name: true, hashed: true, resolved_table: true }),
    ];
    for (label, lookup) in lookups {
        let world = TooltipWorld::with_lookup(lookup);
        let (it, received) = (world.items(), received(&world));
        let mut screen = CreativeScreen::default();
        screen.update_lists(&world.data, &received, params(true), &[]);
        let build = (0..5)
            .map(|_| {
                screen.index = None;
                let start = std::time::Instant::now();
                screen.ensure_index(&world.english, &it, &world.enchantments, &received);
                start.elapsed().as_secs_f64() * 1e3
            })
            .fold(f64::MAX, f64::min);
        println!("{label:<31} {build:>7.2} ms for the index of {} stacks", screen.lists.search.len());
    }
}

/// Micro-bench (ignored): `cargo test --release -p rc-app bench_creative_keystroke -- --ignored
/// --nocapture`. A keystroke in the search tab (query and new grid) for the 50 queries, by copies or
/// by places, `#` tags matched per stack or once.
#[test]
#[ignore = "micro-bench"]
fn bench_creative_keystroke() {
    let world = TooltipWorld::new();
    let rec = received(&world);
    let o = CreativeOpts::OLD;
    let ways = [
        ("copies, tags per stack (former)", o),
        ("places", CreativeOpts { grid_by_index: true, ..o }),
        ("tags matched once", CreativeOpts { tags_matched_once: true, ..o }),
        ("places + tags once", CreativeOpts { grid_by_index: true, tags_matched_once: true, ..o }),
    ];
    for pass in 0..3 {
        for (label, opts) in ways {
            let mut screen = search_screen(&world, &rec, opts);
            // Best of 7 runs of 5 rounds: the machine is shared with other builds.
            let per_query = |screen: &mut CreativeScreen, queries: &[&str]| {
                (0..7)
                    .map(|_| {
                        let start = std::time::Instant::now();
                        for _ in 0..5 {
                            for q in queries {
                                screen.search.text = q.chars().collect();
                                screen.search_changed(&world.data);
                            }
                        }
                        start.elapsed().as_secs_f64() * 1e6 / (5 * queries.len()) as f64
                    })
                    .fold(f64::MAX, f64::min)
            };
            let words: Vec<&str> = QUERIES.iter().copied().filter(|q| !q.starts_with('#')).collect();
            let tags: Vec<&str> = QUERIES.iter().copied().filter(|q| q.starts_with('#')).collect();
            let (w, t) = (per_query(&mut screen, &words), per_query(&mut screen, &tags));
            println!("pass {pass}: {label:<32} words {w:>7.1} µs, tags {t:>7.1} µs per keystroke");
        }
    }
}

/// Micro-bench (ignored): `cargo test --release -p rc-app bench_creative_draw -- --ignored
/// --nocapture`. 10,000 frames of the screen with the mouse over a grid stack, old drawing against
/// the drawing in place, on a category tab, the search tab and the inventory tab.
#[test]
#[ignore = "micro-bench"]
fn bench_creative_draw() {
    let Some(e) = tests_opt::env() else { return };
    let world = TooltipWorld::new();
    let rec = received(&world);
    let models = rc_assets::item::ItemModels::load(&e.assets, world.data.items.iter().map(|i| i.model.as_str()));
    let mut gui = tests_opt::gui_with_item_models(&e, tests_opt::ALL, Some(models));
    gui.begin(1600, 900);
    let inventory = crate::inventory::PlayerInventory::default();
    let env = crate::stack::HudEnv::empty();
    let it = HotbarItems {
        inv: &inventory,
        ids: &world.ids,
        data: &world.data,
        infos: &world.data.items,
        names: &world.item_names,
        env: &env,
        pt: 0.0,
    };
    let (gx, gy) = origin(&gui);
    let mouse = Some(((gx + 30) as f32, (gy + 40) as f32));
    let tabs = &world.data.creative.tabs;
    let pick = |kind| tabs.iter().position(|t| t.kind == kind).expect("tab");
    for (tab_label, k) in [
        ("category", pick(CreativeKind::Category)),
        ("search", pick(CreativeKind::Search)),
        ("inventory", pick(CreativeKind::Inventory)),
    ] {
        for pass in 0..2 {
            let o = CreativeOpts::OLD;
            let ways = [
                ("copies (former)", o),
                ("in place alone", CreativeOpts { draw_without_copies: true, ..o }),
                ("icon index alone", CreativeOpts { indexed_names: true, ..o }),
                ("all switches", CreativeOpts::NEW),
            ];
            for (label, opts) in ways {
                let mut screen = CreativeScreen::with_opts(opts);
                screen.open = true;
                screen.update_lists(&world.data, &rec, params(true), &[]);
                screen.select(&world.data, k, &[]);
                let n = 10_000;
                let start = std::time::Instant::now();
                for _ in 0..n {
                    gui.begin(1600, 900);
                    screen.draw(&mut gui, &e.queue, &inventory, &it, mouse, &world.enchantments, 1.0);
                }
                let per_frame = start.elapsed().as_secs_f64() * 1e6 / f64::from(n);
                println!("pass {pass}: {tab_label:<9} {label:<16} {per_frame:>6.2} µs per frame");
            }
        }
    }
}
