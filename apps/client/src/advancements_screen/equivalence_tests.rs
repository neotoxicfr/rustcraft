//! Proofs and micro-bench of the advancements screen switches on a frozen tree: what is kept
//! between frames draws the same layers to the byte, item icons included; the tiled background
//! and the merged link outlines, which place other quads, render the same pixels on the GPU at
//! scales 1 to 6.

use rc_proto::nbt::Nbt;
use rc_proto_v777::advancements::{Added, Display, UpdateAdvancements};

use super::*;
use crate::gui::tests_opt;
use crate::render::upload::Uploader;

const OLD: AdvancementOpts = AdvancementOpts { tiled_background: false, merged_links: false, screen_caches: false };
const NEW: AdvancementOpts = AdvancementOpts { tiled_background: true, merged_links: true, screen_caches: true };

const STONE: &str = "minecraft:gui/advancements/backgrounds/stone";
const NETHER: &str = "minecraft:gui/advancements/backgrounds/nether";
const MISSING: &str = "minecraft:gui/advancements/backgrounds/not_in_the_game";

/// One advancement of the frozen tree. `shape`: frame (0 task, 1 challenge, 2 goal), then
/// hidden; `groups`: requirement groups, each of one criterion named `c<k>`.
fn advancement(id: &str, parent: Option<&str>, at: (f32, f32), shape: (u8, bool), groups: usize) -> Added {
    let title = match id.len() % 3 {
        0 => Nbt::Compound(vec![("translate".into(), Nbt::String("gui.done".into()))]),
        1 => Nbt::String(format!("Advancement {id}")),
        _ => Nbt::String(format!("A rather long advancement title for the node {id}")),
    };
    Added {
        id: id.to_owned(),
        parent: parent.map(str::to_owned),
        display: Some(Display {
            title: Some(title),
            description: Some(Nbt::String(format!("What the player must do to earn {id}, in a few words"))),
            icon: ItemStack { item: 1, count: 1, added: Vec::new(), removed: Vec::new() },
            frame: shape.0,
            background: None,
            toast: true,
            hidden: shape.1,
        }),
        requirements: (0..groups).map(|k| vec![format!("c{k}")]).collect(),
        x: at.0,
        y: at.1,
    }
}

fn root(id: &str, background: &str) -> Added {
    let mut a = advancement(id, None, (0.0, 0.0), (0, false), 1);
    if let Some(d) = &mut a.display {
        d.background = Some(background.to_owned());
    }
    a
}

/// The frozen tree: three tabs (the third with a background the game does not have). The first
/// holds 70 nodes laid out like the game does (a column per depth, rows half a slot apart) plus
/// links the game's layout never makes (child left of its parent, in the same column, far away):
/// every shape of link, node frames of the three kinds, hidden nodes earned or not, partial
/// progress.
fn frozen_tree() -> UpdateAdvancements {
    frozen_tree_with_third_background(MISSING)
}

/// [`frozen_tree`], the background of its third tab chosen.
fn frozen_tree_with_third_background(third: &str) -> UpdateAdvancements {
    let mut added = vec![root("story/root", STONE), root("nether/root", NETHER), root("lost/root", third)];
    let mut progress = vec![("story/root".to_owned(), vec![("c0".to_owned(), Some(1))])];
    let mut previous_column = vec!["story/root".to_owned()];
    for depth in 1..=14 {
        let mut column = Vec::new();
        for row in 0..5 {
            let k = depth * 5 + row;
            let id = format!("story/n{k}");
            let parent = previous_column[(k * 7) % previous_column.len()].clone();
            // Rows spread from −5 to +5 slots by halves, uneven on purpose.
            let y = ((k * 13) % 21) as f32 * 0.5 - 5.0;
            let groups = 1 + k % 3;
            added.push(advancement(&id, Some(&parent), (depth as f32, y), ((k % 3) as u8, k % 11 == 0), groups));
            let earned: Vec<(String, Option<i64>)> =
                (0..groups).filter(|g| (k + g) % 4 != 0).map(|g| (format!("c{g}"), Some(5))).collect();
            if k % 2 == 0 {
                progress.push((id.clone(), earned));
            }
            column.push(id);
        }
        previous_column = column;
    }
    for (id, parent, at) in [
        ("story/left_of_parent", "story/n40", (2.0, 1.5)),
        ("story/same_column", "story/n41", (8.0, -2.0)),
        ("story/far_right", "story/n12", (9.0, 6.0)),
        ("story/just_below", "story/n13", (3.0, 0.04)),
        ("nether/a", "nether/root", (1.0, -1.0)),
        ("nether/b", "nether/a", (2.0, 0.5)),
        ("lost/a", "lost/root", (1.0, 0.0)),
    ] {
        added.push(advancement(id, Some(parent), at, (2, false), 2));
        progress.push((id.to_owned(), vec![("c1".to_owned(), Some(9))]));
    }
    UpdateAdvancements { reset: true, added, removed: Vec::new(), progress, show: false }
}

/// A later packet: nodes earned, one removed, one added.
fn later_packet() -> UpdateAdvancements {
    UpdateAdvancements {
        reset: false,
        added: vec![advancement("story/late", Some("story/n20"), (5.0, 7.0), (1, false), 1)],
        removed: vec!["story/far_right".to_owned()],
        progress: vec![
            ("story/n11".to_owned(), vec![("c0".to_owned(), Some(3)), ("c1".to_owned(), Some(3)), ("c2".to_owned(), Some(3))]),
            ("story/n22".to_owned(), vec![("c0".to_owned(), Some(3))]),
        ],
        show: false,
    }
}

/// Icons given to the nodes of the frozen tree, in turn: flat items, blocks, and the three whose
/// needle follows the surroundings of the player.
const ICON_ITEMS: [&str; 8] =
    ["compass", "stone", "clock", "diamond_sword", "oak_planks", "recovery_compass", "golden_apple", "chest"];

/// Item data of the installed game, as the screens receive it, around a player whose
/// surroundings move from frame to frame: the icons drawn are the real ones, and those of a
/// compass or a clock change.
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

    /// Models of every item of the game, for the atlas of a test interface.
    fn models(&self, e: &tests_opt::Env) -> rc_assets::item::ItemModels {
        rc_assets::item::ItemModels::load(&e.assets, self.data.items.iter().map(|i| i.model.as_str()))
    }

    fn item(&self, name: &str) -> i32 {
        self.names.iter().position(|n| n == &format!("minecraft:{name}")).unwrap_or_else(|| panic!("item {name}")) as i32
    }

    /// Registry ids of [`ICON_ITEMS`].
    fn icon_ids(&self) -> Vec<i32> {
        ICON_ITEMS.iter().map(|name| self.item(name)).collect()
    }

    /// Frame `frame`: the interface clock and the world time move on, the sun turns and the
    /// player walks around the world spawn (the compass points to it).
    fn at_frame(&mut self, frame: u32) {
        let angle = f64::from(frame) * 0.37;
        self.env.tick = i64::from(frame);
        self.env.game_time = i64::from(frame);
        self.env.sun_angle = (frame * 23 % 360) as f32;
        self.env.pos = [300.0 * angle.cos(), 64.0, 300.0 * angle.sin()];
    }
}

/// `packet` with the icons of its nodes taken from `icons` in turn.
fn with_icons(mut packet: UpdateAdvancements, icons: &[i32]) -> UpdateAdvancements {
    for (k, added) in packet.added.iter_mut().enumerate() {
        if let Some(display) = &mut added.display {
            display.icon.item = icons[k % icons.len()];
        }
    }
    packet
}

/// Item data for the proofs that draw no icon (their interface has no item models).
struct NoItems {
    data: rc_data::VersionData,
    ids: crate::stack::ComponentIds,
    inventory: crate::inventory::PlayerInventory,
    env: crate::stack::HudEnv,
}

impl NoItems {
    fn new() -> NoItems {
        NoItems {
            data: rc_data::VersionData::default(),
            ids: Default::default(),
            inventory: Default::default(),
            env: crate::stack::HudEnv::empty(),
        }
    }

    fn items(&self) -> HotbarItems<'_> {
        HotbarItems {
            inv: &self.inventory,
            ids: &self.ids,
            data: &self.data,
            infos: &[],
            names: &[],
            env: &self.env,
            pt: 0.0,
        }
    }
}

/// A screen with its own copy of the advancements (the screen writes the selected tab in them).
struct Opened {
    adv: Advancements,
    screen: AdvancementsScreen,
}

impl Opened {
    fn new(opts: AdvancementOpts) -> Opened {
        Opened::of(frozen_tree(), opts)
    }

    fn of(tree: UpdateAdvancements, opts: AdvancementOpts) -> Opened {
        let mut adv = Advancements::default();
        adv.apply(tree);
        let screen = AdvancementsScreen::with_opts(&mut adv, true, false, opts);
        Opened { adv, screen }
    }

    /// Scroll of the selected tab set directly (the first centring leaves positive scrolls that
    /// a drag cannot reach).
    fn scroll_to(&mut self, scroll: (f64, f64)) {
        if let Some(t) = self.screen.selected.and_then(|i| self.screen.tabs.get_mut(i)) {
            t.centered = true;
            t.scroll = scroll;
        }
    }

    fn draw(&mut self, e: &tests_opt::Env, g: &mut Gui, it: &HotbarItems<'_>, mouse: Option<(f32, f32)>) {
        self.screen.draw(g, &e.queue, it, &mut self.adv, &e.assets, 0, mouse, 0.0);
    }
}

/// The frozen tree is the one described: three tabs, the story tab with every kind of node.
#[test]
fn frozen_tree_has_every_kind_of_node() {
    let o = Opened::new(OLD);
    assert_eq!(o.screen.tabs.len(), 3);
    let story = &o.screen.tabs[0];
    assert_eq!(story.nodes.len(), 75);
    assert_eq!(story.order.len(), 75, "every node hangs from the root");
    assert_eq!(story.order, tree_order(&story.nodes));
    let count = |f: &dyn Fn(&TabNode) -> bool| story.nodes.iter().filter(|n| f(n)).count();
    assert!(count(&|n| n.hidden && !n.visible()) >= 2, "hidden nodes not earned");
    assert!(count(&|n| n.hidden && n.visible()) >= 1, "hidden nodes earned");
    assert!(count(&|n| n.percent() >= 1.0) >= 10 && count(&|n| n.percent() > 0.0 && n.percent() < 1.0) >= 10);
    for n in &story.nodes {
        assert_eq!((n.shown(true), n.obtained(true)), (n.shown(false), n.obtained(false)), "{}", n.id);
    }
    let links_left = count(&|n| n.parent.is_some_and(|p| story.nodes[p].x > n.x));
    let links_same_column = count(&|n| n.parent.is_some_and(|p| story.nodes[p].x == n.x));
    assert!(links_left >= 1 && links_same_column >= 1, "{links_left} leftward, {links_same_column} in a column");
}

/// The window test of a link is its bounding box: every rectangle of the link (white line and
/// both outlines) lies inside it, for links in every direction.
#[test]
fn link_rectangles_stay_in_the_tested_box() {
    for (child, parent) in [((40, 30), (0, 0)), ((-60, -45), (10, 10)), ((0, 80), (0, 0)), ((28, 1), (0, 0))] {
        let l = LinkPath::between(child, parent);
        let [left, top, right, bottom] = link_bounding_box(child, parent);
        // Rows and columns of the 8 lines of the outline (the white line is inside them).
        let xs = [l.start_x, l.turn_x + 1, l.child_x, l.turn_x - 1];
        let ys = [l.parent_y - 1, l.parent_y + 1, l.child_y - 1, l.child_y + 1];
        assert!(xs.iter().all(|x| (left..right).contains(x)), "{child:?} {parent:?}");
        assert!(ys.iter().all(|y| (top..bottom).contains(y)), "{child:?} {parent:?}");
    }
    // Just outside each edge of the window: skipped; one pixel in: drawn.
    assert!(!link_reaches_window((IW, 0), (IW + 50, 0)) && link_reaches_window((IW - 14, 0), (IW + 50, 0)));
    assert!(!link_reaches_window((-100, 0), (-32, 0)) && link_reaches_window((-100, 0), (-31, 0)));
    assert!(!link_reaches_window((0, IH - 12), (-30, IH + 30)) && link_reaches_window((0, IH - 13), (-30, IH + 30)));
    assert!(!link_reaches_window((0, -60), (-30, -15)) && link_reaches_window((0, -60), (-30, -14)));
}

/// What the screen keeps between frames (draw order, node states, titles, nodes and links
/// outside the window skipped, missing background not read again) draws the layers of the old
/// path to the byte over 150 frames: scrolling in every direction, tab changes, hovering of
/// nodes and tabs, a later packet (frame 50), another language (frame 80), other fonts (frame
/// 110). The interface holds the item models of the game: the layers carry the icons of the
/// nodes and of the tabs, those of the compasses and clocks changing with the frame. Without an
/// installed game or a GPU: skipped (`RUSTCRAFT_OPT_REQUIRE=1`: failure).
#[test]
fn kept_screen_state_draws_the_old_layers() {
    let Some(mut game) = GameItems::load() else { return };
    let Some(e) = tests_opt::env() else { return };
    let icons = game.icon_ids();
    let compass = game.item("compass");
    let kept_only = AdvancementOpts { screen_caches: true, ..OLD };
    let mut paths = [OLD, kept_only].map(|opts| Opened::of(with_icons(frozen_tree(), &icons), opts));
    // Both screens drawn in turn on one interface. One, not two: the atlas cell of an icon is
    // named after the address of its item model and cells are handed out in the order the icons
    // are first placed (the old path also places those the clipping drops), so two interfaces
    // would put the same icons in different cells.
    let mut gui = tests_opt::gui_with_item_models(&e, tests_opt::ALL, Some(game.models(&e)));
    let g = &mut gui;
    let base = e.assets.read("assets/minecraft/lang/en_us.json").unwrap_or_default();
    let (mut differing_frames, mut previous) = (0, Vec::new());
    // Renders of the compass icons met on the kept path (one per direction of the needle).
    let mut compass_renders = hashbrown::HashSet::new();
    for frame in 0..150u32 {
        game.at_frame(frame);
        let it = game.items();
        let mouse = match frame % 5 {
            0 => None,
            // Over the tabs above the window.
            1 => Some((150.0 + (frame % 3) as f32 * 32.0, 68.0)),
            // Over the nodes inside the window.
            _ => Some((160.0 + (frame * 7 % 200) as f32, 110.0 + (frame * 5 % 90) as f32)),
        };
        match frame {
            80 => {
                let other_language = r#"{"gui.done": "Terminé", "gui.advancements": "Progrès"}"#;
                g.lang = std::sync::Arc::new(rc_text::Lang::load(&base, Some(other_language.as_bytes()), None));
            }
            110 => {
                let opts = rc_assets::font::FontOptions { uniform: true, jp: false };
                let load = |id: &&str| ((*id).to_owned(), rc_assets::font::Font::load(&e.assets, id, opts));
                g.set_fonts(crate::resources::FONT_IDS.iter().map(load).collect());
            }
            _ => {}
        }
        let mut layers = Vec::new();
        for opened in &mut paths {
            if frame == 50 {
                opened.adv.apply(with_icons(later_packet(), &icons));
            }
            if frame % 30 == 20 {
                let tab = (frame / 30 % 3) as usize;
                opened.screen.select(tab, &mut opened.adv);
            }
            g.begin(1600, 900);
            opened.screen.tick(g, &mut opened.adv, mouse);
            opened.draw(&e, g, &it, mouse);
            layers.push(tests_opt::layer_bytes(g));
            // Dragged after the frame: left and up, then back, by amounts that are not whole tiles.
            if let Some(t) = opened.screen.selected.and_then(|i| opened.screen.tabs.get_mut(i)) {
                let step = if frame % 40 < 25 { (-13.7, -3.3) } else { (21.9, 5.1) };
                t.scroll_by(step.0, step.1);
            }
        }
        let first = layers[0].iter().zip(&layers[1]).position(|(a, b)| a != b);
        assert!(layers[0].len() == layers[1].len() && first.is_none(), "frame {frame}: layers differ (byte {first:?})");
        differing_frames += usize::from(previous != layers[0]);
        previous = layers.swap_remove(0);
        let kept = &paths[1].screen;
        let drawn_nodes = kept.selected.and_then(|i| kept.tabs.get(i)).map_or(&[][..], |t| &t.nodes[..]);
        let compasses = drawn_nodes.iter().filter(|n| n.icon.item == compass);
        compass_renders.extend(compasses.filter_map(|n| n.cache.as_ref()).filter(|c| !c.layers.is_empty()).map(|c| c.key));
    }
    assert!(differing_frames > 100, "the scene barely moves: {differing_frames} frames differ from the previous one");
    assert!(compass_renders.len() > 8, "icons that follow the player barely change: {} compass renders", compass_renders.len());
    let (old, kept) = (&paths[0].screen, &paths[1].screen);
    for (name, screen) in [("old", old), ("kept", kept)] {
        let with_icon = |t: &Tab| t.nodes.iter().filter(|n| n.cache.as_ref().is_some_and(|c| !c.layers.is_empty())).count();
        let icons_made: usize = screen.tabs.iter().map(with_icon).sum();
        assert!(icons_made > 60, "{name} path: {icons_made} node icons made, the layers hold no icons");
    }
    assert_eq!(old.slots, kept.slots, "same backgrounds written or missing");
    assert!(matches!(&kept.slots[2], BackgroundSlot::Missing(b) if b.contains("not_in_the_game")), "{:?}", kept.slots[2]);
    assert!(matches!(&kept.slots[0], BackgroundSlot::Written(_)));
}

/// Size of the offscreen target of the pixel proof: the smallest that admits GUI scale 6 (the
/// interface keeps at least 320 × 240 of its own pixels; 1600 × 900 stops at scale 3).
const SCALE_6_TARGET: (u32, u32) = (1920, 1440);

/// Pixels of one frame of the screen, tab scrolled to `scroll`, at GUI scale `scale`.
fn rendered(
    e: &tests_opt::Env,
    g: &mut Gui,
    up: &mut Uploader,
    opened: &mut Opened,
    (scale, scroll): (u32, (f64, f64)),
) -> Vec<u8> {
    let no_items = NoItems::new();
    g.scale_option = scale;
    g.begin(SCALE_6_TARGET.0, SCALE_6_TARGET.1);
    assert_eq!(g.scale, scale, "the target is too small for this GUI scale: another scale was drawn");
    // The offscreen render of the interface tests expects the places of the camera overlays and
    // of the models marked once each.
    g.overlays_here();
    g.models_here();
    opened.scroll_to(scroll);
    opened.draw(e, g, &no_items.items(), None);
    tests_opt::render_sized(e, g, up, SCALE_6_TARGET)
}

/// The tiled background (one repeated quad for 170 sprites) and the merged link outlines (4
/// rectangles for 8 lines), alone and with everything on, render the pixels of the old path:
/// not one pixel differs at scales 1 to 6, for scrolls that are not whole tiles (negative
/// included) and with the whole tree out of the window. Without an installed game or a GPU:
/// skipped.
#[test]
fn tiled_background_and_merged_links_render_the_old_pixels() {
    let Some(e) = tests_opt::env() else { return };
    let scrolls = [(0.0, 0.0), (-37.4, -5.2), (-141.9, 61.5), (-9.0, 127.0), (-301.3, 99.9), (-1000.0, -1000.0)];
    let mut reference = (Opened::new(OLD), tests_opt::gui(&e, tests_opt::ALL), Uploader::new(&e.device, true));
    let cases = [
        ("tiled background", AdvancementOpts { tiled_background: true, ..OLD }),
        ("merged links", AdvancementOpts { merged_links: true, ..OLD }),
        ("all", NEW),
    ];
    let mut paths: Vec<_> = cases
        .iter()
        .map(|(name, opts)| (*name, Opened::new(*opts), tests_opt::gui(&e, tests_opt::ALL), Uploader::new(&e.device, true)))
        .collect();
    let mut failures = Vec::new();
    // Bytes of quads placed over the whole test, old path then each case.
    let mut placed = [0usize; 4];
    for scale in 1..=6 {
        let mut images = Vec::new();
        for scroll in scrolls {
            let (opened, g, up) = &mut reference;
            let want = rendered(&e, g, up, opened, (scale, scroll));
            placed[0] += tests_opt::layer_bytes(g).len();
            for (k, (name, opened, g, up)) in paths.iter_mut().enumerate() {
                let got = rendered(&e, g, up, opened, (scale, scroll));
                placed[k + 1] += tests_opt::layer_bytes(g).len();
                let differing = tests_opt::diff_at_width(&want, &got, SCALE_6_TARGET.0);
                if !differing.is_empty() {
                    failures.push(format!(
                        "{name}, scale {scale}, scroll {scroll:?}: {} pixels differ ({})",
                        differing.len(),
                        tests_opt::where_diff(&differing)
                    ));
                }
            }
            images.push(want);
        }
        assert!(images[0] != images[1] && images[1] != images[2], "scale {scale}: the scroll does not show");
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    eprintln!("bytes of quads placed (old, tiled background, merged links, all): {placed:?}");
    assert!(placed[1..].iter().all(|&p| p < placed[0]), "a case places no fewer quads than the old path: {placed:?}");
    assert!(placed[3] < placed[1] && placed[3] < placed[2], "all the switches place the fewest quads: {placed:?}");
}

/// Micro-bench of a frame of the screen on the frozen tree (loaded machine possible): time on
/// the main thread, bytes allocated and bytes of quads placed per frame, old path against each
/// switch and all of them. The quads are what goes to the GPU. Two trees: every background in
/// the game, then one tab whose background is missing (looked up again on every frame by the old
/// path; the test resources have no baked cache, so that lookup opens the game archive: slower
/// than in the game). The interface holds the item models of the game: the icons are refreshed
/// and placed (the interface clock stands still: the 20 evaluations a second of each icon are not
/// in the figure). Without a GPU: skipped.
/// `cargo test --release -p rc-app bench_advancements_draw -- --ignored --nocapture`
#[test]
#[ignore = "micro-bench"]
fn bench_advancements_draw() {
    let Some(game) = GameItems::load() else { return };
    let Some(e) = tests_opt::env() else { return };
    let it = game.items();
    let icons = game.icon_ids();
    let cases = [
        ("old", OLD),
        ("tiled background", AdvancementOpts { tiled_background: true, ..OLD }),
        ("merged links", AdvancementOpts { merged_links: true, ..OLD }),
        ("kept screen state", AdvancementOpts { screen_caches: true, ..OLD }),
        ("all", NEW),
    ];
    for (tree, third_background) in [("backgrounds present", NETHER), ("one background missing", MISSING)] {
        for pass in 0..2 {
            for (name, opts) in cases {
                let mut g = tests_opt::gui_with_item_models(&e, tests_opt::ALL, Some(game.models(&e)));
                let packet = with_icons(frozen_tree_with_third_background(third_background), &icons);
                let mut opened = Opened::of(packet, opts);
                let once = |g: &mut Gui, opened: &mut Opened| {
                    g.begin(1600, 900);
                    opened.draw(&e, g, &it, None);
                };
                for _ in 0..100 {
                    once(&mut g, &mut opened);
                }
                let frames = 5_000;
                let t = std::time::Instant::now();
                for _ in 0..frames {
                    once(&mut g, &mut opened);
                }
                let per_frame = t.elapsed().as_secs_f64() * 1e6 / f64::from(frames);
                let allocated =
                    crate::debug_sys::allocated_bytes(|| (0..100).for_each(|_| once(&mut g, &mut opened))) / 100;
                let placed = tests_opt::layer_bytes(&g).len();
                println!(
                    "advancements draw, {tree:<22} pass {pass}, {name:<18}: {per_frame:>7.2} µs, {allocated:>6} bytes allocated, {placed:>6} bytes of quads per frame"
                );
            }
        }
    }
}
