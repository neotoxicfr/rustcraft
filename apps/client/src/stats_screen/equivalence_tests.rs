//! Proofs and micro-benches of the statistics screen switches: lists built through the name
//! indexes against the scans, rows kept between frames against rows redone on every frame.

use super::*;
use crate::gui::tests_opt;

const OLD: StatsOpts = StatsOpts { indexed_sync: false, row_cache: false };
const NEW: StatsOpts = StatsOpts { indexed_sync: true, row_cache: true };

/// Item data of the installed game, as the screens receive it, around a player in the overworld
/// (the compass points to the world spawn).
struct GameItems {
    data: rc_data::VersionData,
    names: Vec<String>,
    ids: crate::stack::ComponentIds,
    inventory: crate::inventory::PlayerInventory,
    env: crate::stack::HudEnv,
}

impl GameItems {
    fn load() -> Option<GameItems> {
        let data = rc_testdata::version_data()?;
        let names = data.registry("minecraft:item").expect("item registry").entries.clone();
        let ids = crate::stack::ComponentIds::new(&data);
        let overworld = || "minecraft:overworld".to_owned();
        let env = crate::stack::HudEnv {
            dimension: overworld(),
            spawn: Some((overworld(), [0, 64, 0])),
            ..crate::stack::HudEnv::empty()
        };
        Some(GameItems { data, names, ids, inventory: Default::default(), env })
    }

    /// Models of every item of the game, for the atlas of a test interface.
    fn models(&self, e: &tests_opt::Env) -> rc_assets::item::ItemModels {
        rc_assets::item::ItemModels::load(&e.assets, self.data.items.iter().map(|i| i.model.as_str()))
    }

    /// Frame `frame`: the interface clock moves on and the player walks around the world spawn,
    /// so the compass icon changes from frame to frame. The world time stands still: a needle
    /// swings once per world tick in which its icon is made, and the old path makes the icons of
    /// rows that are off screen (the one difference between the paths, see `docs/perf.md`).
    fn at_frame(&mut self, frame: u32) {
        let angle = f64::from(frame) * 0.37;
        self.env.tick = i64::from(frame);
        self.env.pos = [300.0 * angle.cos(), 64.0, 300.0 * angle.sin()];
    }

    fn items(&self) -> HotbarItems<'_> {
        HotbarItems {
            inv: &self.inventory,
            ids: &self.ids,
            data: &self.data,
            infos: &self.data.items,
            names: &self.names,
            env: &self.env,
            pt: 0.0,
        }
    }

    fn item(&self, name: &str) -> i32 {
        self.names.iter().position(|n| n == &format!("minecraft:{name}")).unwrap_or_else(|| panic!("item {name}")) as i32
    }

    fn block(&self, name: &str) -> i32 {
        self.data.block_id(&format!("minecraft:{name}")).unwrap_or_else(|| panic!("block {name}")) as i32
    }

    fn entity(&self, name: &str) -> i32 {
        self.data.registry_id("minecraft:entity_type", &format!("minecraft:{name}")).expect("entity type") as i32
    }

    /// A frozen statistics packet: mined blocks whose item bears another name, simple items,
    /// air (never listed), kills and deaths, a few custom statistics.
    fn frozen_packet(&self) -> Vec<(i32, i32, i32)> {
        let mut values = Vec::new();
        for (n, block) in [
            "redstone_wire",
            "wall_torch",
            "wheat",
            "stone",
            "air",
            "oak_wall_sign",
            "carrots",
            "attached_melon_stem",
            "powder_snow",
        ]
        .iter()
        .enumerate()
        {
            values.push((stats::MINED, self.block(block), 10 + n as i32));
        }
        for (kind, item, n) in [
            (stats::USED, "wheat", 3),
            (stats::USED, "diamond_pickaxe", 1_234_567),
            (stats::USED, "air", 9),
            (stats::CRAFTED, "oak_planks", 64),
            (stats::BROKEN, "iron_pickaxe", 2),
            (stats::PICKED_UP, "cobblestone", 500),
            (stats::DROPPED, "dirt", 7),
        ] {
            values.push((kind, self.item(item), n));
        }
        values.push((stats::KILLED, self.entity("zombie"), 12));
        values.push((stats::KILLED_BY, self.entity("creeper"), 3));
        values.push((stats::KILLED, self.entity("creeper"), 1));
        for (id, n) in [(1, 72_000), (6, 123_456), (23, 42), (25, 125)] {
            values.push((stats::CUSTOM, id, n));
        }
        values
    }

    /// Every item used and dropped, every block mined, every entity type killed: the largest lists.
    fn everything_packet(&self) -> Vec<(i32, i32, i32)> {
        let mut values = Vec::new();
        for item in 0..self.names.len() as i32 {
            values.push((stats::USED, item, 1 + item % 7));
            values.push((stats::DROPPED, item, 1 + item % 5));
        }
        for block in 0..self.data.blocks.len() as i32 {
            values.push((stats::MINED, block, 1 + block % 9));
        }
        let entities = self.data.registry("minecraft:entity_type").map_or(0, |r| r.entries.len());
        for entity in 0..entities as i32 {
            values.push((stats::KILLED, entity, 1 + entity % 3));
        }
        values
    }
}

fn stats_of(values: &[(i32, i32, i32)]) -> Stats {
    let mut s = Stats::default();
    s.award(values);
    s
}

/// Screen opened before the statistics arrived, then synchronised with `s`.
fn synced(opts: StatsOpts, lang: &rc_text::Lang, s: &Stats, it: &HotbarItems<'_>) -> StatsScreen {
    let mut screen = StatsScreen::with_opts(&Stats::default(), true, false, opts);
    screen.sync(lang, s, it);
    screen
}

type ListsBuilt = (Vec<(i32, Option<i32>)>, Vec<(usize, String)>, Vec<(i32, String)>);

fn lists(screen: &StatsScreen) -> ListsBuilt {
    let text = |c: &[StyledChar]| c.iter().map(|s| s.c).collect::<String>();
    (
        screen.items.iter().map(|r| (r.item, r.block)).collect(),
        screen.general.iter().map(|(id, name)| (*id, text(name))).collect(),
        screen.mobs.clone(),
    )
}

/// Lists built through the name indexes equal the scanned ones, on the frozen packet (rows
/// checked by name: redstone wire → redstone, wall torch → torch, wheat crop → seeds, wheat item
/// without block, air never listed) and on the largest lists the game can send.
#[test]
fn indexed_sync_builds_the_scanned_lists() {
    let Some(game) = GameItems::load() else { return };
    let it = game.items();
    let lang = rc_text::Lang::default();
    let s = stats_of(&game.frozen_packet());
    let (old, new) = (synced(OLD, &lang, &s, &it), synced(NEW, &lang, &s, &it));
    assert_eq!(lists(&new), lists(&old), "frozen packet");
    let block_name = |b: i32| short_name(&game.data.blocks[b as usize].name).to_owned();
    let mut rows: Vec<(String, Option<String>)> =
        new.items.iter().map(|r| (short_name(&game.names[r.item as usize]).to_owned(), r.block.map(block_name))).collect();
    rows.sort();
    let mut want: Vec<(String, Option<String>)> = [
        ("carrot", Some("carrots")),
        ("cobblestone", Some("cobblestone")),
        ("diamond_pickaxe", None),
        ("dirt", Some("dirt")),
        ("iron_pickaxe", None),
        ("oak_planks", Some("oak_planks")),
        ("oak_sign", Some("oak_sign")),
        ("powder_snow_bucket", Some("powder_snow")),
        ("redstone", Some("redstone_wire")),
        ("stone", Some("stone")),
        ("torch", Some("torch")),
        ("wheat", None),
        ("wheat_seeds", Some("wheat")),
    ]
    .map(|(item, block)| (item.to_owned(), block.map(str::to_owned)))
    .into();
    want.sort();
    assert_eq!(rows, want);
    assert_eq!(new.mobs.iter().map(|m| m.0).collect::<Vec<_>>(), {
        let mut ids = vec![game.entity("zombie"), game.entity("creeper")];
        ids.sort_unstable();
        ids
    });
    assert_eq!(new.general.len(), CUSTOM_STATS.len());
    assert_eq!(new.active, [true, true, true]);

    let s = stats_of(&game.everything_packet());
    let (old, new) = (synced(OLD, &lang, &s, &it), synced(NEW, &lang, &s, &it));
    assert!(new.items.len() > 1_000 && new.mobs.len() > 100, "{} items, {} mobs", new.items.len(), new.mobs.len());
    assert!(new.items.iter().filter(|r| r.block.is_some()).count() > 500, "blocks found for the block items");
    assert_eq!(lists(&new), lists(&old), "largest lists");
}

/// The old path of the row cache: rows redone on every frame.
const REDONE: StatsOpts = StatsOpts { row_cache: false, ..NEW };

/// A screen opened before the statistics arrived.
fn open(opts: StatsOpts) -> StatsScreen {
    StatsScreen::with_opts(&Stats::default(), true, false, opts)
}

/// A test interface that holds the item models of the game: the layers carry the icons of the
/// Items rows.
fn interface_with_icons(e: &tests_opt::Env, game: &GameItems) -> Gui {
    tests_opt::gui_with_item_models(e, tests_opt::ALL, Some(game.models(e)))
}

/// Two screens over the same statistics, one redoing its rows and one keeping them, drawn in
/// turn on one interface. One, not two: the atlas cell of an icon is named after the address of
/// its item model and freed cells are handed out again in hash order, so two interfaces would
/// place the same icons in different cells.
struct Pair {
    screens: [StatsScreen; 2],
    gui: Gui,
}

impl Pair {
    fn new(e: &tests_opt::Env, game: &GameItems) -> Pair {
        Pair { screens: [open(REDONE), open(NEW)], gui: interface_with_icons(e, game) }
    }

    /// Draws one frame of each screen and returns their layers, redone rows first.
    fn frame(
        &mut self,
        e: &tests_opt::Env,
        s: &Stats,
        it: &HotbarItems<'_>,
        mouse: Option<(f32, f32)>,
        time: f64,
    ) -> [Vec<u8>; 2] {
        self.screens.each_mut().map(|screen| {
            self.gui.begin(1600, 900);
            screen.draw(&mut self.gui, &e.queue, it, s, 0, mouse, time);
            tests_opt::layer_bytes(&self.gui)
        })
    }
}

/// Rows kept between frames draw the layers of rows redone on every frame, to the byte, over 160
/// frames on the three tabs: scrolling, column sorts, new statistics (frame 40), another language
/// with the same number of translations (frame 70), other fonts (frame 100), header and tab
/// hovering. The layers carry the item icons; the compass row, whose icon changes on every frame,
/// leaves the screen and comes back. The kept rows really are kept (few texts built, icons made
/// for the rows met only). Without an installed game or a GPU: skipped
/// (`RUSTCRAFT_OPT_REQUIRE=1`: failure).
#[test]
fn kept_rows_draw_the_layers_of_redone_rows() {
    let Some(mut game) = GameItems::load() else { return };
    let Some(e) = tests_opt::env() else { return };
    let compass = game.item("compass");
    let mut s = stats_of(&game.everything_packet());
    s.award(&game.frozen_packet());
    let mut pair = Pair::new(&e, &game);
    let base = e.assets.read("assets/minecraft/lang/en_us.json").unwrap_or_default();
    let other_language = r#"{"stats.none": "aucun", "stat.itemsButton": "Objets",
        "stat_type.minecraft.killed": "Vous avez tué %s %s", "stat_type.minecraft.killed_by.none": "Jamais tué par %s"}"#;
    let (mut differing_frames, mut previous) = (0, Vec::new());
    // Renders of the compass icon met on the kept path (one per direction of the needle).
    let mut compass_renders = hashbrown::HashSet::new();
    for frame in 0..160u32 {
        game.at_frame(frame);
        let it = game.items();
        match frame {
            40 => s.award(&[(stats::USED, game.item("stone"), 7_654_321), (stats::CUSTOM, 23, 99)]),
            70 => {
                let language = rc_text::Lang::load(&base, Some(other_language.as_bytes()), None);
                pair.gui.lang = std::sync::Arc::new(language);
            }
            100 => {
                let opts = rc_assets::font::FontOptions { uniform: true, jp: false };
                let load = |id: &&str| ((*id).to_owned(), rc_assets::font::Font::load(&e.assets, id, opts));
                pair.gui.set_fonts(crate::resources::FONT_IDS.iter().map(load).collect());
            }
            _ => {}
        }
        // Tab of the frame: General, Items, Mobs in turn, 10 frames each.
        let tab = (frame / 10 % 3) as usize;
        for screen in &mut pair.screens {
            if screen.loaded {
                screen.select_tab(tab);
            }
            if frame % 20 == 15 {
                screen.sort = Some(Sort { col: (frame / 20 % 6) as usize, desc: frame % 40 == 15 });
                screen.resort = true;
            }
        }
        // Mouse over the tabs, then over the list (header buttons and item slots).
        let mouse = match frame % 4 {
            0 => None,
            1 => Some((260.0 + (frame % 7) as f32 * 4.0, 10.0)),
            _ => Some((150.0 + (frame % 9) as f32 * 30.0, 40.0 + (frame % 5) as f32 * 20.0)),
        };
        let [redone, kept] = pair.frame(&e, &s, &it, mouse, f64::from(frame) / 20.0);
        let first = redone.iter().zip(&kept).position(|(a, b)| a != b);
        assert!(redone.len() == kept.len() && first.is_none(), "frame {frame} (tab {tab}): layers differ (byte {first:?})");
        differing_frames += usize::from(previous != redone);
        previous = redone;
        let kept_compass = pair.screens[1].items.iter().find(|r| r.item == compass).and_then(|r| r.cache.as_ref());
        compass_renders.extend(kept_compass.filter(|c| !c.layers.is_empty()).map(|c| c.key));
        // Scroll after the frame (the list exists once the first frame laid it out): down and up
        // by steps, and every third frame of the Items tab to the compass row, 40 pixels under
        // the top of the list, so that this row leaves the screen and comes back again and again.
        let compass_row = pair.screens[1].items.iter().position(|r| r.item == compass);
        for screen in &mut pair.screens {
            let Some(l) = &mut screen.panel.list else { continue };
            let to_compass = compass_row.filter(|_| tab == 1 && frame % 3 == 0).and_then(|row| l.entry_rect(row + 1));
            let step = match to_compass {
                Some((_, y, _, _)) => y - l.y - 40,
                None if frame % 10 < 6 => 37,
                None => -11,
            };
            l.set_scroll(l.scroll + f64::from(step));
        }
    }
    assert!(differing_frames > 100, "the scene barely moves: {differing_frames} frames differ from the previous one");
    let [redone, kept] = [pair.screens[0].texts.builds, pair.screens[1].texts.builds];
    assert!(redone > 1_500, "rows redone on every frame: {redone} texts built");
    assert!(kept > 50 && kept * 2 < redone, "kept rows: {kept} texts built against {redone}");
    assert!(compass_renders.len() > 5, "the compass icon barely changes: {} renders", compass_renders.len());
    let icons_made = |screen: &StatsScreen| {
        screen.items.iter().filter(|r| r.cache.as_ref().is_some_and(|c| !c.layers.is_empty())).count()
    };
    let [redone, kept] = [icons_made(&pair.screens[0]), icons_made(&pair.screens[1])];
    assert!(redone > 1_000, "rows redone on every frame: {redone} icons made, the layers hold no icons");
    assert!(kept > 20 && kept * 2 < redone, "kept rows: {kept} icons made against {redone}");
}

/// The one difference between the two paths, and it goes towards the game: the hand of a clock
/// swings once per world tick in which its icon is made. The game makes the icon of a row only
/// while it draws the row, and so do the kept rows; the old path made every icon on every frame,
/// whatever the tab, so its hands went on turning unseen. While the row is on screen both paths
/// make the same icon; after 40 world ticks on another tab during which the sun went down, the
/// kept row starts again from the hand it last showed, the redone row shows another one.
#[test]
fn clock_hand_of_a_row_off_screen_stands_still() {
    let Some(mut game) = GameItems::load() else { return };
    let Some(e) = tests_opt::env() else { return };
    let clock = game.item("clock");
    let s = stats_of(&[(stats::USED, clock, 1)]);
    // An interface each: the hands (state of the item models) must not be shared.
    let mut paths = [REDONE, NEW].map(|opts| (open(opts), interface_with_icons(&e, &game)));
    // Interface tick at which the clock icon of the screen was last made, and the layers made.
    let clock_icon = |screen: &StatsScreen| {
        let row = screen.items.iter().find(|r| r.item == clock);
        row.and_then(|r| r.cache.as_ref()).filter(|c| !c.layers.is_empty()).map(|c| (c.tick, c.layers.clone()))
    };
    for frame in 0..52u32 {
        (game.env.tick, game.env.game_time) = (i64::from(frame), i64::from(frame));
        // Items tab at midday, then 40 frames on the General tab while the sun goes down, then
        // the Items tab again.
        let on_items = !(10..50).contains(&frame);
        game.env.sun_angle = if frame < 10 { 0.0 } else { 180.0 };
        let [redone, kept] = paths.each_mut().map(|(screen, gui)| {
            if screen.loaded {
                screen.select_tab(usize::from(on_items));
            }
            gui.begin(1600, 900);
            screen.draw(gui, &e.queue, &game.items(), &s, 0, None, 0.0);
            clock_icon(screen)
        });
        let made_at = [&redone, &kept].map(|icon| icon.as_ref().map(|(tick, _)| *tick));
        match frame {
            1..10 => assert!(redone.is_some() && redone == kept, "frame {frame}: row on screen, {made_at:?}"),
            49 => assert_eq!(made_at, [Some(49), Some(9)], "only the old path makes the icon of a row it does not draw"),
            50 => {
                assert_eq!(made_at, [Some(50), Some(50)], "both rows drawn again");
                assert_ne!(redone, kept, "the hand turned unseen on the old path only");
            }
            _ => {}
        }
    }
}

/// Micro-bench of the list building when the statistics arrive (loaded machine possible): scans
/// against name indexes, a usual packet and the largest one. Main thread, a single core by nature.
/// `cargo test --release -p rc-app bench_stats_sync -- --ignored --nocapture`
#[test]
#[ignore = "micro-bench"]
fn bench_stats_sync() {
    let Some(game) = GameItems::load() else { return };
    let it = game.items();
    let lang = rc_text::Lang::default();
    let usual: Vec<(i32, i32, i32)> = (0..60)
        .flat_map(|k| [(stats::USED, k * 23 % game.names.len() as i32, 5), (stats::MINED, k * 17 % 1000, 3)])
        .collect();
    for (label, packet, runs) in [("usual (60 items, 60 blocks)", usual, 20), ("largest", game.everything_packet(), 3)] {
        let s = stats_of(&packet);
        for (name, opts) in [("scans", OLD), ("name indexes", NEW)] {
            let mut best = f64::MAX;
            let mut rows = 0;
            for _ in 0..runs {
                let t = std::time::Instant::now();
                let screen = synced(opts, &lang, &s, &it);
                best = best.min(t.elapsed().as_secs_f64() * 1e3);
                rows = screen.items.len();
            }
            let bytes = crate::debug_sys::allocated_bytes(|| drop(synced(opts, &lang, &s, &it)));
            println!("stats sync, {label:<28} {name:<13}: {best:>9.3} ms, {bytes:>12} bytes allocated, {rows} item rows");
        }
    }
}

/// Micro-bench of a frame of each tab (loaded machine possible): rows redone on every frame
/// against kept rows, time and bytes allocated per frame. The interface holds the item models of
/// the game: the icons are refreshed and placed. The interface clock stands still, as between
/// two of its 20 ticks a second; `moving clock` adds the frames where it moved (every icon made
/// again: the worst frame, one in fifteen at 300 frames a second). Without a GPU: skipped.
/// `cargo test --release -p rc-app bench_stats_draw -- --ignored --nocapture`
#[test]
#[ignore = "micro-bench"]
fn bench_stats_draw() {
    let Some(mut game) = GameItems::load() else { return };
    let Some(e) = tests_opt::env() else { return };
    let s = stats_of(&game.everything_packet());
    for scale in [3, 1] {
        for tab in 0..3 {
            for (name, row_cache) in [("redone", false), ("kept", true)] {
                let mut g = interface_with_icons(&e, &game);
                g.scale_option = scale;
                game.env.tick = 0;
                let mut screen = StatsScreen::with_opts(&Stats::default(), true, false, StatsOpts { row_cache, ..NEW });
                let once = |g: &mut Gui, screen: &mut StatsScreen, game: &GameItems| {
                    g.begin(1600, 900);
                    screen.draw(g, &e.queue, &game.items(), &s, 0, None, 0.0);
                };
                once(&mut g, &mut screen, &game);
                screen.select_tab(tab);
                for _ in 0..50 {
                    once(&mut g, &mut screen, &game);
                }
                let frames = 2_000;
                let t = std::time::Instant::now();
                for _ in 0..frames {
                    once(&mut g, &mut screen, &game);
                }
                let per_frame = t.elapsed().as_secs_f64() * 1e6 / f64::from(frames);
                let bytes =
                    crate::debug_sys::allocated_bytes(|| (0..100).for_each(|_| once(&mut g, &mut screen, &game))) / 100;
                let moving_frames = 200;
                let t = std::time::Instant::now();
                for _ in 0..moving_frames {
                    game.env.tick += 1;
                    once(&mut g, &mut screen, &game);
                }
                let moving_clock = t.elapsed().as_secs_f64() * 1e6 / f64::from(moving_frames);
                println!(
                    "stats draw, scale {scale}, tab {tab}, rows {name:<6}: {per_frame:>7.2} µs, {bytes:>7} bytes allocated per frame, moving clock {moving_clock:>8.2} µs"
                );
            }
        }
    }
}
