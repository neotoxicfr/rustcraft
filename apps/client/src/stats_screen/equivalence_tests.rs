//! Proofs and micro-benches of the statistics screen switches: lists built through the name
//! indexes against the scans, rows kept between frames against rows redone on every frame.

use super::*;
use crate::gui::tests_opt;

const OLD: StatsOpts = StatsOpts { indexed_sync: false, row_cache: false };
const NEW: StatsOpts = StatsOpts { indexed_sync: true, row_cache: true };

/// Item data of the installed game, as the screens receive it.
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
        Some(GameItems { data, names, ids, inventory: Default::default(), env: crate::stack::HudEnv::empty() })
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

/// Two screens over the same statistics, one keeping its rows and one redoing them.
struct Pair {
    screens: [StatsScreen; 2],
    guis: [Gui; 2],
}

impl Pair {
    /// Both screens opened before the statistics arrived.
    fn new(e: &tests_opt::Env) -> Pair {
        let guis = [tests_opt::gui(e, tests_opt::ALL), tests_opt::gui(e, tests_opt::ALL)];
        let kept = StatsOpts { row_cache: true, ..NEW };
        let redone = StatsOpts { row_cache: false, ..NEW };
        let open = |opts| StatsScreen::with_opts(&Stats::default(), true, false, opts);
        Pair { screens: [open(redone), open(kept)], guis }
    }

    /// Draws one frame on both and returns their layers.
    fn frame(
        &mut self,
        e: &tests_opt::Env,
        s: &Stats,
        it: &HotbarItems<'_>,
        mouse: Option<(f32, f32)>,
        time: f64,
    ) -> [Vec<u8>; 2] {
        let mut out = [Vec::new(), Vec::new()];
        for ((screen, g), layers) in self.screens.iter_mut().zip(&mut self.guis).zip(&mut out) {
            g.begin(1600, 900);
            screen.draw(g, &e.queue, it, s, 0, mouse, time);
            *layers = tests_opt::layer_bytes(g);
        }
        out
    }

    fn each(&mut self, mut f: impl FnMut(&mut StatsScreen, &mut Gui)) {
        for (screen, gui) in self.screens.iter_mut().zip(&mut self.guis) {
            f(screen, gui);
        }
    }
}

/// Rows kept between frames draw the layers of rows redone on every frame, to the byte, over 160
/// frames on the three tabs: scrolling, column sorts, new statistics (frame 40), another language
/// with the same number of translations (frame 70), other fonts (frame 100), header and tab
/// hovering. The kept rows really are kept (few texts built). Without an installed game or a GPU:
/// skipped (`RUSTCRAFT_OPT_REQUIRE=1`: failure).
#[test]
fn kept_rows_draw_the_layers_of_redone_rows() {
    let Some(game) = GameItems::load() else { return };
    let Some(e) = tests_opt::env() else { return };
    let it = game.items();
    let mut s = stats_of(&game.everything_packet());
    s.award(&game.frozen_packet());
    let mut pair = Pair::new(&e);
    let base = e.assets.read("assets/minecraft/lang/en_us.json").unwrap_or_default();
    let other_language = r#"{"stats.none": "aucun", "stat.itemsButton": "Objets",
        "stat_type.minecraft.killed": "Vous avez tué %s %s", "stat_type.minecraft.killed_by.none": "Jamais tué par %s"}"#;
    let (mut differing_frames, mut previous) = (0, Vec::new());
    for frame in 0..160u32 {
        match frame {
            40 => s.award(&[(stats::USED, game.item("stone"), 7_654_321), (stats::CUSTOM, 23, 99)]),
            70 => pair.each(|_, g| {
                g.lang = std::sync::Arc::new(rc_text::Lang::load(&base, Some(other_language.as_bytes()), None));
            }),
            100 => pair.each(|_, g| {
                let opts = rc_assets::font::FontOptions { uniform: true, jp: false };
                let load = |id: &&str| ((*id).to_owned(), rc_assets::font::Font::load(&e.assets, id, opts));
                g.set_fonts(crate::resources::FONT_IDS.iter().map(load).collect());
            }),
            _ => {}
        }
        // Tab of the frame: General, Items, Mobs in turn, 10 frames each.
        let tab = (frame / 10 % 3) as usize;
        pair.each(|screen, _| {
            if screen.loaded {
                screen.select_tab(tab);
            }
            if frame % 20 == 15 {
                screen.sort = Some(Sort { col: (frame / 20 % 6) as usize, desc: frame % 40 == 15 });
                screen.resort = true;
            }
        });
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
        // Scroll after the frame (the list exists once the first frame laid it out).
        pair.each(|screen, _| {
            if let Some(l) = &mut screen.panel.list {
                let step = if frame % 10 < 6 { 37.0 } else { -11.0 };
                l.set_scroll(l.scroll + step);
            }
        });
    }
    assert!(differing_frames > 100, "the scene barely moves: {differing_frames} frames differ from the previous one");
    let [redone, kept] = [pair.screens[0].texts.builds, pair.screens[1].texts.builds];
    assert!(redone > 1_500, "rows redone on every frame: {redone} texts built");
    assert!(kept > 50 && kept * 2 < redone, "kept rows: {kept} texts built against {redone}");
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
/// against kept rows, time and bytes allocated per frame. Without a GPU: skipped.
/// `cargo test --release -p rc-app bench_stats_draw -- --ignored --nocapture`
#[test]
#[ignore = "micro-bench"]
fn bench_stats_draw() {
    let Some(game) = GameItems::load() else { return };
    let Some(e) = tests_opt::env() else { return };
    let it = game.items();
    let s = stats_of(&game.everything_packet());
    for scale in [3, 1] {
        for tab in 0..3 {
            for (name, row_cache) in [("redone", false), ("kept", true)] {
                let mut g = tests_opt::gui(&e, tests_opt::ALL);
                g.scale_option = scale;
                let mut screen = StatsScreen::with_opts(&Stats::default(), true, false, StatsOpts { row_cache, ..NEW });
                let once = |g: &mut Gui, screen: &mut StatsScreen| {
                    g.begin(1600, 900);
                    screen.draw(g, &e.queue, &it, &s, 0, None, 0.0);
                };
                once(&mut g, &mut screen);
                screen.select_tab(tab);
                for _ in 0..50 {
                    once(&mut g, &mut screen);
                }
                let frames = 2_000;
                let t = std::time::Instant::now();
                for _ in 0..frames {
                    once(&mut g, &mut screen);
                }
                let per_frame = t.elapsed().as_secs_f64() * 1e6 / f64::from(frames);
                let bytes = crate::debug_sys::allocated_bytes(|| (0..100).for_each(|_| once(&mut g, &mut screen))) / 100;
                println!(
                    "stats draw, scale {scale}, tab {tab}, rows {name:<6}: {per_frame:>7.2} µs, {bytes:>7} bytes allocated per frame"
                );
            }
        }
    }
}
