//! Proofs and micro-benches of the recipe book on the whole book our integrated server sends after
//! `/recipe give @s *` on a new flat world (the `recipe-book` bench scene): a scenario of 300 frames
//! (mouse over the widgets, filter, tabs, pages, search, variants menu, ghost recipe, inventory,
//! tags, recipe book packets, language and screens changing) drawn on a test interface. Without the
//! version data, the installed game or a GPU, the tests skip themselves; `RUSTCRAFT_OPT_REQUIRE=1`
//! makes that a failure.

use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use rc_client::Event;
use winit::keyboard::{KeyCode, ModifiersState};

use super::*;
use crate::gui::tests_opt;
use crate::inventory::Container;

/// The book and the tags our integrated server sends to a player of a new flat world who is given
/// every recipe.
struct ServerBook {
    data: Arc<rc_data::VersionData>,
    /// Every book entry of `/recipe give @s *`, with its flags (notification and highlight).
    entries: Vec<(RecipeEntry, u8)>,
    /// The tag packets of the join, in order.
    tags: Vec<rc_proto_v777::play::Tags>,
}

fn server_book() -> Option<&'static ServerBook> {
    static BOOK: OnceLock<Option<ServerBook>> = OnceLock::new();
    BOOK.get_or_init(fetch_server_book).as_ref()
}

fn fetch_server_book() -> Option<ServerBook> {
    let data = Arc::new(rc_testdata::version_data()?);
    rc_testdata::need(rc_sim::registries::client_jar(&data), "26.3 client jar")?;
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-app-recipe-book-{}", std::process::id())));
    let mut world = rc_sim::newworld::NewWorld::new("Recipe book");
    world.preset = rc_sim::newworld::Preset::Flat;
    world.allow_commands = true;
    world.generate_structures = false;
    rc_sim::newworld::create_world(&dir, &data, &world).expect("new flat world");
    let name = "BookReader";
    let opts = rc_sim::Options {
        world: dir.clone(),
        name: name.into(),
        uuid: rc_client::offline_uuid(name),
        info: rc_proto_v777::play::ClientInformation { view_distance: 2, ..Default::default() },
        sim_distance: 2,
    };
    let (session, server) = rc_sim::start(opts, data.clone()).expect("integrated server");
    let (mut tags, mut asked) = (Vec::new(), false);
    // Every book entry received, by id: the join book and the advancements of the first ticks may
    // already hold a few recipes, which the command then leaves out (how many depends on the speed
    // of the machine).
    let mut by_id = std::collections::BTreeMap::new();
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        assert!(Instant::now() < deadline, "no whole recipe book from the integrated server");
        let Ok(event) = session.events.recv_timeout(Duration::from_millis(20)) else { continue };
        match event {
            Event::Tags(t) => tags.push(t),
            Event::Teleport(p) if !asked => {
                session.link.send_accept_teleport(p.id, p.pos, p.yaw, p.pitch).expect("teleport accepted");
                session.link.send_player_loaded().expect("player loaded");
                session.link.send_command("recipe give @s *").expect("command sent");
                asked = true;
            }
            Event::Recipes(RecipeEvent::Add(entries, _)) => {
                let whole = entries.len() > 500;
                by_id.extend(entries.into_iter().map(|(e, _)| (e.id, e)));
                if whole {
                    break;
                }
            }
            left @ (Event::Disconnected(_) | Event::Kicked(_)) => panic!("integrated server left: {left:?}"),
            _ => {}
        }
    }
    session.link.disconnect();
    server.wait();
    // In id order, every recipe highlighted (the notification flag only makes toasts): the same
    // book on every run.
    let entries = by_id.into_values().map(|e| (e, 2)).collect();
    Some(ServerBook { data, entries, tags })
}

/// What the screens hand the book: version data, item names and components, the received tags,
/// the player's inventory and the open screen.
struct Scene {
    data: Arc<rc_data::VersionData>,
    names: Vec<String>,
    ids: crate::stack::ComponentIds,
    env: crate::stack::HudEnv,
    received: Received,
    inventory: PlayerInventory,
}

impl Scene {
    fn new(book: &ServerBook) -> Scene {
        let data = book.data.clone();
        let names = data.registry("minecraft:item").expect("item registry").entries.clone();
        let mut received = Received::default();
        for t in &book.tags {
            received.tags(t);
        }
        let ids = crate::stack::ComponentIds::new(&data);
        let mut scene =
            Scene { data, names, ids, env: crate::stack::HudEnv::empty(), received, inventory: PlayerInventory::default() };
        for (slot, item, count) in [
            (9, "oak_log", 10),
            (10, "oak_planks", 20),
            (11, "stick", 8),
            (12, "cobblestone", 64),
            (13, "iron_ingot", 5),
            (14, "coal", 3),
            (15, "string", 4),
            (16, "redstone", 9),
            (36, "diamond", 2),
        ] {
            scene.put(slot, item, count);
        }
        scene
    }

    fn item(&self, name: &str) -> i32 {
        self.names.iter().position(|n| n == &format!("minecraft:{name}")).unwrap_or_else(|| panic!("item {name}")) as i32
    }

    fn put(&mut self, slot: usize, item: &str, count: i32) {
        let item = self.item(item);
        self.inventory.menu[slot] = Some(ItemStack { item, count, added: Vec::new(), removed: Vec::new() });
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

    fn ctx(&self) -> Ctx<'_> {
        Ctx { data: &self.data, rec: &self.received, item_names: &self.names }
    }

    /// Opens a container screen of `kind` (window 3), or goes back to the inventory.
    fn open(&mut self, kind: Kind) {
        self.inventory.container = (kind != Kind::Player).then(|| Container {
            window: 3,
            kind,
            title: None,
            slots: vec![None; if kind == Kind::Crafting { 46 } else { 39 }],
            state: 0,
            props: [0; 10],
            extra: Default::default(),
        });
    }

    /// Tags as received, with the item tag `name` holding `item` alone: the recipes that need the
    /// tag need that item now. (An emptied tag would make an ingredient that any content fills.)
    fn narrow_item_tag(&mut self, book: &ServerBook, name: &str, item: &str) {
        let item = self.item(item);
        for t in &book.tags {
            let mut t = t.clone();
            for (registry, list) in &mut t {
                if registry == "minecraft:item" {
                    list.iter_mut().filter(|(n, _)| n == name).for_each(|(_, ids)| *ids = vec![item]);
                }
            }
            self.received.tags(&t);
        }
    }

    /// What the scenario changes around the book at `frame`: inventory, open screen, tags.
    fn change(&mut self, book: &ServerBook, frame: u32) {
        match frame {
            110 => self.put(17, "gold_ingot", 3),
            // Same counts, other slots: nothing to recompute in the game's terms.
            215 => {
                let planks = self.inventory.menu[10].take();
                self.inventory.menu[20] = planks;
            }
            // A renamed stack does not count.
            230 => {
                let name = crate::stack::component_type_id(&self.data, "minecraft:custom_name").expect("custom_name");
                if let Some(s) = &mut self.inventory.menu[36] {
                    // Network text component: a string tag holding "x".
                    s.added.push((name, vec![8, 0, 1, b'x'].into_boxed_slice()));
                }
            }
            232 => self.put(1, "oak_planks", 1),
            240 => self.narrow_item_tag(book, "minecraft:planks", "bamboo_planks"),
            // The craftable recipes are computed again after the tags changed.
            245 => self.put(18, "stick", 1),
            270 => self.open(Kind::Crafting),
            285 => self.open(Kind::Furnace(0)),
            _ => {}
        }
    }
}

/// Corner of the wide book (§ 11.1) and the centres of its widgets, in interface units.
struct Spots {
    bx: f32,
    by: f32,
}

impl Spots {
    fn of(gui: &Gui) -> Spots {
        Spots { bx: ((gui.width as i32 - 147) / 2 - 86) as f32, by: ((gui.height as i32 - 166) / 2) as f32 }
    }

    fn page_button(&self, k: u32) -> (f32, f32) {
        (self.bx + 11.0 + 25.0 * (k % 5) as f32 + 12.0, self.by + 31.0 + 25.0 * (k / 5) as f32 + 12.0)
    }

    /// The `shown`-th tab of the column (hidden tabs take no place).
    fn tab(&self, shown: u32) -> (f32, f32) {
        (self.bx - 30.0 + 17.0, self.by + 3.0 + 27.0 * shown as f32 + 13.0)
    }

    fn filter(&self) -> (f32, f32) {
        (self.bx + 110.0 + 13.0, self.by + 12.0 + 8.0)
    }

    fn search(&self) -> (f32, f32) {
        (self.bx + 25.0 + 40.0, self.by + 13.0 + 7.0)
    }

    fn next_page(&self) -> (f32, f32) {
        (self.bx + 93.0 + 6.0, self.by + 137.0 + 8.0)
    }

    fn previous_page(&self) -> (f32, f32) {
        (self.bx + 38.0 + 6.0, self.by + 137.0 + 8.0)
    }

    /// Mouse of `frame`: over a page button, a tab, the filter, a page arrow, nothing, or away.
    fn mouse(&self, frame: u32) -> Option<(f32, f32)> {
        match frame % 6 {
            0 | 1 => Some(self.page_button(frame / 6 % 20)),
            2 => Some(self.tab(frame / 6 % 5)),
            3 => Some(if frame % 12 == 3 { self.filter() } else { self.next_page() }),
            4 => Some((self.bx + 200.0, self.by + 10.0)),
            _ => None,
        }
    }
}

/// One book driven by the scenario, with its own link: the packets it sends are compared too.
struct BookUnderTest {
    book: RecipeBook,
    session: rc_client::Session,
    peer: rc_client::memory::Peer,
}

impl BookUnderTest {
    fn new(scene: &Scene, book: RecipeBook) -> BookUnderTest {
        let (session, peer) = rc_client::memory::session(scene.data.clone(), 1).expect("in-memory link");
        BookUnderTest { book, session, peer }
    }

    fn sent(&self) -> Vec<Vec<u8>> {
        self.peer.packets.try_iter().collect()
    }

    fn click(&mut self, scene: &Scene, gui: &Gui, at: (f32, f32), button: u8) {
        let (it, ctx) = (scene.items(), scene.ctx());
        self.book.press(gui, &scene.inventory, &it, &[], &ctx, at, button, false, &self.session.link);
    }

    fn type_text(&mut self, window: i32, text: &str) {
        let none = ModifiersState::empty();
        self.book.key(KeyCode::KeyA, none, Some(text), false, false, window, &self.session.link);
    }

    fn erase(&mut self, window: i32, n: usize) {
        let none = ModifiersState::empty();
        for _ in 0..n {
            self.book.key(KeyCode::Backspace, none, None, false, false, window, &self.session.link);
        }
    }

    /// What the player and the server do to the book at `frame` (the same for every book compared).
    fn act(&mut self, server: &ServerBook, scene: &Scene, gui: &Gui, frame: u32) {
        let spots = Spots::of(gui);
        let (kind, _, window, _) = crate::screens::current(&scene.inventory);
        let ctx = scene.ctx();
        match frame {
            0 => {
                self.book.on_event(RecipeEvent::Add(server.entries.clone(), false), &ctx);
                self.book.settings = [(true, false); 4];
                self.book.on_screen(kind);
            }
            20 | 40 | 225 => self.click(scene, gui, spots.filter(), 0),
            50 => self.click(scene, gui, spots.tab(1), 0),
            70 => self.click(scene, gui, spots.tab(3), 0),
            90 => self.click(scene, gui, spots.tab(0), 0),
            100 => {
                self.click(scene, gui, spots.search(), 0);
                self.type_text(window, "plank");
            }
            120 => self.type_text(window, "s"),
            130 => {
                self.erase(window, 6);
                self.type_text(window, "minecraft:stone");
            }
            // The filter setting changes alone, nothing else (as a setting can).
            142 | 147 => self.book.settings[self.book.book].1 ^= true,
            150 => {
                self.erase(window, 15);
                self.type_text(window, "zzzz");
            }
            160 => self.erase(window, 4),
            165 | 170 | 177 => self.click(scene, gui, spots.next_page(), 0),
            180 => self.click(scene, gui, spots.previous_page(), 0),
            // Variants menu of a button of several recipes, then a click outside closes it.
            190 => {
                for k in 0..20 {
                    self.click(scene, gui, spots.page_button(k), 1);
                    if self.book.overlay.is_some() {
                        break;
                    }
                }
            }
            196 => self.click(scene, gui, (spots.bx - 60.0, spots.by), 0),
            // Ghost recipe of the first shaped recipe that fits the 2×2 grid, cleared by a click.
            198 => {
                let shaped = server.entries.iter().find(|(e, _)| {
                    matches!(e.display, RecipeDisplay::Shaped { width, height, .. } if width <= 2 && height <= 2)
                });
                if let Some((e, _)) = shaped {
                    self.book.on_ghost(0, &e.display, &scene.inventory, &ctx);
                }
            }
            208 => self.book.crafting_click(kind, 1),
            // Book packets: recipes forgotten, one learnt again (notification and highlight).
            250 => {
                let ids: Vec<i32> = server.entries.iter().take(40).map(|(e, _)| e.id).collect();
                self.book.on_event(RecipeEvent::Remove(ids), &ctx);
                self.book.on_event(RecipeEvent::Add(vec![(server.entries[3].0.clone(), 3)], false), &ctx);
            }
            262 => {
                self.click(scene, gui, spots.search(), 0);
                self.type_text(window, "planches");
            }
            266 => self.erase(window, 8),
            // The screen changed at the frame before: the tick that follows tells the book.
            271 | 286 => self.book.on_screen(kind),
            _ => {}
        }
        // Notifications of unlocked recipes go to the toasts, not drawn here.
        self.book.unlocked.clear();
    }

    fn draw(&mut self, scene: &Scene, gui: &mut Gui, queue: &wgpu::Queue, mouse: Option<(f32, f32)>) {
        let (it, ctx) = (scene.items(), scene.ctx());
        self.book.draw(gui, queue, &scene.inventory, &it, &[], &ctx, mouse, &self.session.link);
    }

    /// The tick of the screen: time stands still while Ctrl is held (frames 60 to 69).
    fn tick(&mut self, frame: u32) {
        if !(60..70).contains(&frame) {
            self.book.time += 1;
        }
        self.book.clock += 1;
    }
}

/// Another language with the same keys as the game's: book texts and an item name translated.
fn other_language(e: &tests_opt::Env) -> rc_text::Lang {
    let base = e.assets.read("assets/minecraft/lang/en_us.json").unwrap_or_default();
    let other = r#"{"gui.recipebook.page": "Page %s sur %s", "gui.recipebook.search_hint": "Chercher...",
        "block.minecraft.oak_planks": "Planches de chêne", "gui.recipebook.moreRecipes": "Clic droit : plus"}"#;
    rc_text::Lang::load(&base, Some(other.as_bytes()), None)
}

/// FNV-1a: a fixed digest of the drawn layers, the same from one run to the next.
fn digest(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3))
}

/// Digest of a frame's layers without the atlas coordinates of the item icons (premultiplied
/// layers): their cells follow the addresses of the item models, different from one run to the
/// next. Places, sizes and colours of the icons stay in it, and everything else.
fn frame_digest(layers: &[u8]) -> u64 {
    const UV: std::ops::Range<usize> = 8..16;
    let vertex = std::mem::size_of::<crate::gui::Vertex>();
    let mut kept = Vec::with_capacity(layers.len());
    let mut p = 0;
    while p < layers.len() {
        let blend = layers[p];
        kept.push(blend);
        p += 1;
        // Shapes, then text: a vertex count and the vertices.
        for _ in 0..2 {
            let count = u64::from_le_bytes(layers[p..p + 8].try_into().expect("count")) as usize;
            kept.extend_from_slice(&layers[p..p + 8]);
            p += 8;
            for v in layers[p..p + count * vertex].chunks(vertex) {
                let at = kept.len();
                kept.extend_from_slice(v);
                if blend == Blend::Premultiplied as u8 {
                    kept[at + UV.start..at + UV.end].fill(0);
                }
            }
            p += count * vertex;
        }
    }
    digest(&kept)
}

/// Digest of the stacks the book drew at its last frame, in drawing order: which icon is where.
fn drawn_stacks_digest(book: &RecipeBook) -> u64 {
    let mut bytes = Vec::new();
    for c in &book.cache[..book.cursor] {
        let Some(c) = c else {
            bytes.push(0);
            continue;
        };
        let s = &c.stack;
        bytes.extend(s.item.to_le_bytes().into_iter().chain(s.count.to_le_bytes()));
        for (id, value) in &s.added {
            bytes.extend(id.to_le_bytes().into_iter().chain(value.iter().copied()));
        }
        bytes.extend(s.removed.iter().flat_map(|id| id.to_le_bytes()));
    }
    digest(&bytes)
}

/// The scenario's frames: the layers of every frame of every book, the digest of the stacks it
/// drew, and the packets each sent.
struct Played {
    frames: Vec<Vec<Vec<u8>>>,
    stacks: Vec<Vec<u64>>,
    packets: Vec<Vec<Vec<u8>>>,
}

/// Plays the scenario on one interface, the books drawn in turn. With `item_icons`, the interface
/// holds the item models of the game and the layers carry the icons; their atlas cells are named
/// after the addresses of the models, so these layers can only be compared within one run.
fn play(server: &ServerBook, e: &tests_opt::Env, books: Vec<RecipeBook>, item_icons: bool) -> Played {
    let mut scene = Scene::new(server);
    let models = item_icons
        .then(|| rc_assets::item::ItemModels::load(&e.assets, scene.data.items.iter().map(|i| i.model.as_str())));
    let mut gui = tests_opt::gui_with_item_models(e, tests_opt::ALL, models);
    gui.begin(tests_opt::W, tests_opt::H);
    let mut books: Vec<BookUnderTest> = books.into_iter().map(|b| BookUnderTest::new(&scene, b)).collect();
    let n = books.len();
    let mut played = Played { frames: vec![Vec::new(); n], stacks: vec![Vec::new(); n], packets: vec![Vec::new(); n] };
    for frame in 0..300 {
        scene.change(server, frame);
        if frame == 260 {
            gui.lang = Arc::new(other_language(e));
        }
        let mouse = Spots::of(&gui).mouse(frame);
        for (k, b) in books.iter_mut().enumerate() {
            b.act(server, &scene, &gui, frame);
            gui.begin(tests_opt::W, tests_opt::H);
            b.draw(&scene, &mut gui, &e.queue, mouse);
            played.frames[k].push(tests_opt::layer_bytes(&gui));
            played.stacks[k].push(drawn_stacks_digest(&b.book));
            b.tick(frame);
        }
    }
    for (k, b) in books.iter().enumerate() {
        played.packets[k] = b.sent();
    }
    played
}

/// The paths compared: the old one, each switch alone, all of them (last).
const PATHS: [(&str, BookOpts); 6] = [
    ("old", BookOpts::OLD),
    ("page cache", BookOpts { page_cache: true, ..BookOpts::OLD }),
    ("content sequence", BookOpts { content_sequence: true, ..BookOpts::OLD }),
    ("ingredient sets", BookOpts { ingredient_sets: true, ..BookOpts::OLD }),
    ("parallel index", BookOpts { parallel_index: true, ..BookOpts::OLD }),
    ("new", BookOpts::NEW),
];

/// All the switches on.
const NEW_PATH: (&str, BookOpts) = PATHS[PATHS.len() - 1];

/// The scenario on a book of `opts`: frame digests and packets, printed so that the frozen values
/// of [`book_draws_its_frozen_frames`] can be renewed after an intended change.
fn frozen_run(server: &ServerBook, e: &tests_opt::Env, opts: BookOpts) -> (Vec<u64>, u64, usize) {
    let played = play(server, e, vec![RecipeBook::with_opts(opts)], true);
    let frames: Vec<u64> =
        played.frames[0].iter().zip(&played.stacks[0]).map(|(f, s)| frame_digest(f) ^ s.rotate_left(17)).collect();
    let packets = digest(&played.packets[0].concat());
    (frames, packets, played.packets[0].len())
}

/// Every path draws the frames of the old path to the byte, item icons included (one interface,
/// the books drawn in turn), and sends the same packets; the kept page view really is kept.
#[test]
fn paths_draw_the_same_frames() {
    let Some(server) = rc_testdata::need(server_book(), "whole recipe book from the integrated server") else { return };
    let Some(e) = tests_opt::env() else { return };
    let books = PATHS.iter().map(|(_, opts)| RecipeBook::with_opts(*opts)).collect();
    let played = play(server, &e, books, true);
    let (old_frames, old_packets) = (&played.frames[0], &played.packets[0]);
    for (k, (name, _)) in PATHS.iter().enumerate().skip(1) {
        for (frame, (old, new)) in old_frames.iter().zip(&played.frames[k]).enumerate() {
            let first = old.iter().zip(new).position(|(a, b)| a != b);
            assert!(old.len() == new.len() && first.is_none(), "{name}, frame {frame}: layers differ (byte {first:?})");
        }
        assert_eq!(&played.packets[k], old_packets, "{name}: packets sent");
    }
    let icons_changing = played.stacks[0].windows(2).filter(|w| w[0] != w[1]).count();
    // Results cycle every 30 ticks, and pages, tabs and searches change: 27 frames in all.
    assert!(icons_changing > 20, "the icons barely change: {icons_changing} frames");
}

/// The page view of the new path is built again only when one of its inputs changes: a few dozen
/// times over the 300 frames, not on every frame.
#[test]
fn kept_page_view_is_built_again_on_changes_only() {
    let Some(server) = rc_testdata::need(server_book(), "whole recipe book from the integrated server") else { return };
    let Some(e) = tests_opt::env() else { return };
    let mut scene = Scene::new(server);
    let mut gui = tests_opt::gui(&e, tests_opt::ALL);
    gui.begin(tests_opt::W, tests_opt::H);
    let mut b = BookUnderTest::new(&scene, RecipeBook::with_opts(BookOpts::NEW));
    let (mut builds, mut previous) = (0, std::ptr::null());
    for frame in 0..300 {
        scene.change(server, frame);
        b.act(server, &scene, &gui, frame);
        let mouse = Spots::of(&gui).mouse(frame);
        gui.begin(tests_opt::W, tests_opt::H);
        b.draw(&scene, &mut gui, &e.queue, mouse);
        b.tick(frame);
        // A new view is made while the previous one is still held: a new address.
        let now = b.book.page_view.as_ref().map_or(std::ptr::null(), Rc::as_ptr);
        builds += usize::from(now != previous);
        previous = now;
    }
    assert!((10..80).contains(&builds), "page view built {builds} times over 300 frames");
}

/// Without a GPU: the craftable recipes, the listed collections, the tabs and the recipes of each
/// listed collection are those of the old path on every path, for the three books, every tab,
/// filtered or not, searches, inventories (renamed stack, crafting grid) and an item tag emptied.
#[test]
fn paths_list_the_same_recipes() {
    let Some(server) = rc_testdata::need(server_book(), "whole recipe book from the integrated server") else { return };
    let mut scene = Scene::new(server);
    let lang = rc_text::Lang::load(b"{}", None, None);
    let mut books: Vec<RecipeBook> = PATHS.iter().map(|(_, opts)| RecipeBook::with_opts(*opts)).collect();
    for b in &mut books {
        b.on_event(RecipeEvent::Add(server.entries.clone(), false), &scene.ctx());
        b.settings = [(true, false); 4];
    }
    let mut checked = 0;
    // The crafting table after the item tags changed: its 2×2 recipes were resolved for the
    // inventory screen before the change.
    let screens = [Kind::Player, Kind::Furnace(0), Kind::Crafting, Kind::Furnace(1), Kind::Furnace(2)];
    for (step, kind) in screens.into_iter().enumerate() {
        scene.change(server, [230, 232, 240, 245, 110][step]);
        scene.open(kind);
        for b in &mut books {
            b.on_screen(kind);
        }
        for tab in 0..tabs(books[0].book).len() {
            for search in ["", "plank", "minecraft:stone", "zzzz"] {
                // The filter innermost: from one listing to the next, it changes alone.
                for filter in [false, true] {
                    let listings: Vec<_> = books
                        .iter_mut()
                        .map(|b| {
                            (b.tab, b.settings[b.book].1) = (tab, filter);
                            b.search.text = search.chars().collect();
                            let (it, ctx) = (scene.items(), scene.ctx());
                            b.refresh_craftable(&scene.inventory, kind, &ctx);
                            let view = b.page_view(kind, &ctx, &lang, &it, &[]);
                            let shown: Vec<Vec<i32>> =
                                (0..view.listed.len()).map(|k| b.shown_of(&view, k, kind).into_owned()).collect();
                            let mut craftable: Vec<i32> = b.craftable.iter().copied().collect();
                            craftable.sort_unstable();
                            (craftable, view.listed.clone(), view.tabs.clone(), shown)
                        })
                        .collect();
                    for (k, listing) in listings.iter().enumerate().skip(1) {
                        let what = format!("{}: {kind:?}, tab {tab}, filter {filter}, search {search:?}", PATHS[k].0);
                        assert!(listing == &listings[0], "{what}: not the old path's");
                    }
                    checked += usize::from(!listings[0].1.is_empty());
                }
            }
        }
    }
    assert!(checked > 40, "only {checked} listings with recipes");
}

/// The books the micro-bench compares, by name.
fn bench_paths() -> Vec<(&'static str, RecipeBook)> {
    PATHS.iter().map(|&(name, opts)| (name, RecipeBook::with_opts(opts))).collect()
}

/// A steady state of the open book the micro-bench measures, and what changes on every frame.
#[derive(Clone, Copy, Debug)]
enum Situation {
    /// Search tab, no search, mouse over a page button: nothing changes.
    Still,
    /// The query "a" (most recipes match), nothing else changes.
    Query,
    /// Second tab, craftable filter on.
    FilteredTab,
    /// One stack of the inventory changes on every frame: the craftable recipes are computed again.
    InventoryChanges,
    /// A book packet on every frame (a recipe learnt again): everything built from the book again.
    BookPacket,
}

fn set_up(b: &mut BookUnderTest, server: &ServerBook, scene: &Scene, gui: &Gui, situation: Situation) {
    let (kind, _, window, _) = crate::screens::current(&scene.inventory);
    b.book.on_event(RecipeEvent::Add(server.entries.clone(), false), &scene.ctx());
    b.book.settings = [(true, false); 4];
    b.book.on_screen(kind);
    b.book.highlight.clear();
    let spots = Spots::of(gui);
    match situation {
        Situation::Query => {
            b.click(scene, gui, spots.search(), 0);
            b.type_text(window, "a");
        }
        Situation::FilteredTab => {
            b.click(scene, gui, spots.filter(), 0);
            b.click(scene, gui, spots.tab(1), 0);
        }
        _ => {}
    }
}

/// One path of the micro-bench: its book, scene and interface, kept from one round to the next.
struct BenchRun {
    name: &'static str,
    book: BookUnderTest,
    scene: Scene,
    gui: Gui,
    frame: u32,
    times: Vec<f64>,
}

impl BenchRun {
    fn new(name: &'static str, book: RecipeBook, server: &ServerBook, e: &tests_opt::Env, situation: Situation) -> BenchRun {
        let scene = Scene::new(server);
        let models = rc_assets::item::ItemModels::load(&e.assets, scene.data.items.iter().map(|i| i.model.as_str()));
        let mut gui = tests_opt::gui_with_item_models(e, tests_opt::ALL, Some(models));
        gui.begin(tests_opt::W, tests_opt::H);
        let mut book = BookUnderTest::new(&scene, book);
        set_up(&mut book, server, &scene, &gui, situation);
        BenchRun { name, book, scene, gui, frame: 0, times: Vec::new() }
    }

    /// One frame of `situation`: its change, then the drawing.
    fn frame(&mut self, server: &ServerBook, e: &tests_opt::Env, situation: Situation) {
        let frame = self.frame;
        self.frame += 1;
        match situation {
            Situation::InventoryChanges => self.scene.put(17, "gold_ingot", 1 + (frame % 2) as i32),
            Situation::BookPacket => {
                let again = RecipeEvent::Add(vec![(server.entries[3].0.clone(), 0)], false);
                self.book.book.on_event(again, &self.scene.ctx());
            }
            _ => {}
        }
        let mouse = Some(Spots::of(&self.gui).page_button(7));
        self.gui.begin(tests_opt::W, tests_opt::H);
        self.book.draw(&self.scene, &mut self.gui, &e.queue, mouse);
    }

    fn timed_frames(&mut self, server: &ServerBook, e: &tests_opt::Env, situation: Situation, frames: u32) {
        for _ in 0..frames {
            let t = Instant::now();
            self.frame(server, e, situation);
            self.times.push(t.elapsed().as_secs_f64() * 1e6);
        }
    }
}

/// Micro-bench of a frame of the open book on the whole book (loaded machine possible): time and
/// bytes allocated per frame, for each path and situation. The paths take turns in rounds of 100
/// frames (10 rounds), so that a change of the machine's load falls on all of them; the bytes
/// allocated do not depend on the load. The interface holds the item models of the game. Main
/// thread, a single core by nature.
/// `cargo test --release -p rc-app bench_recipe_book_draw -- --ignored --nocapture`
#[test]
#[ignore = "micro-bench"]
fn bench_recipe_book_draw() {
    let Some(server) = rc_testdata::need(server_book(), "whole recipe book from the integrated server") else { return };
    let Some(e) = tests_opt::env() else { return };
    let situations =
        [Situation::Still, Situation::Query, Situation::FilteredTab, Situation::InventoryChanges, Situation::BookPacket];
    println!("recipe book: {} entries", server.entries.len());
    for situation in situations {
        let mut runs: Vec<BenchRun> =
            bench_paths().into_iter().map(|(name, book)| BenchRun::new(name, book, server, &e, situation)).collect();
        for run in &mut runs {
            for _ in 0..50 {
                run.frame(server, &e, situation);
            }
        }
        for _round in 0..10 {
            for run in &mut runs {
                run.timed_frames(server, &e, situation, 100);
            }
        }
        for run in &mut runs {
            let bytes = crate::debug_sys::allocated_bytes(|| (0..100).for_each(|_| run.frame(server, &e, situation))) / 100;
            run.times.sort_by(f64::total_cmp);
            let at = |share: usize| run.times[run.times.len() * share / 100];
            let _ = run.book.sent();
            println!(
                "recipe book, {:<16} {:<17}: median {:>7.2} µs, p90 {:>7.2} µs, p99 {:>7.2} µs, {bytes:>7} bytes allocated per frame",
                format!("{situation:?}"),
                run.name,
                at(50),
                at(90),
                at(99)
            );
        }
    }
}
/// Micro-bench of what the book builds again on a change (loaded machine possible), without the
/// drawing: the search index (first search, language or book change), the craftable recipes
/// (inventory change) and the page view (any input change), per path, best of 20 runs. The
/// parallel index runs on rayon's pool: `RAYON_NUM_THREADS=n` sets its threads for the run.
/// `cargo test --release -p rc-app bench_recipe_book_rebuilds -- --ignored --nocapture`
#[test]
#[ignore = "micro-bench"]
fn bench_recipe_book_rebuilds() {
    let Some(server) = rc_testdata::need(server_book(), "whole recipe book from the integrated server") else { return };
    let mut scene = Scene::new(server);
    let lang = rc_text::Lang::load(b"{}", None, None);
    let best = |mut run: Box<dyn FnMut() + '_>| {
        (0..20)
            .map(|_| {
                let t = Instant::now();
                run();
                t.elapsed().as_secs_f64() * 1e6
            })
            .fold(f64::MAX, f64::min)
    };
    for (name, opts) in PATHS {
        let mut b = RecipeBook::with_opts(opts);
        b.on_event(RecipeEvent::Add(server.entries.clone(), false), &scene.ctx());
        b.settings = [(true, false); 4];
        b.on_screen(Kind::Player);
        let index = best(Box::new(|| {
            b.index = None;
            b.ensure_index(&scene.ctx(), &lang, &scene.items(), &[]);
        }));
        let mut count = 0;
        let craftable = best(Box::new(|| {
            count += 1;
            scene.put(17, "gold_ingot", 1 + count % 2);
            b.refresh_craftable(&scene.inventory, Kind::Player, &scene.ctx());
        }));
        let view = best(Box::new(|| {
            b.page_view = None;
            b.craftable_version += 1;
            let _ = b.page_view(Kind::Player, &scene.ctx(), &lang, &scene.items(), &[]);
        }));
        println!("recipe book rebuilds, {name:<17}: index {index:>8.1} µs, craftable {craftable:>7.1} µs, page view {view:>7.1} µs");
        // What the kept data hold, about (vector headers counted, map tables not).
        let sets: usize = b.ingredient_sets.by_recipe.values().flatten().map(|ids| 24 + 4 * ids.capacity()).sum();
        let stack = |s: &ItemStack| std::mem::size_of::<ItemStack>() + s.added.iter().map(|a| 24 + a.1.len()).sum::<usize>();
        let results: usize = b.result_stacks.by_recipe.values().flatten().map(stack).sum();
        println!(
            "recipe book kept data, {name:<17}: ingredient sets of {} recipes {} KB, result stacks of {} recipes {} KB",
            b.ingredient_sets.by_recipe.len(),
            sets / 1024,
            b.result_stacks.by_recipe.len(),
            results / 1024
        );
    }
}

/// The frames the book draws in the scenario (the icons without their atlas cells, see
/// [`frame_digest`], and the stacks drawn) and the packets it sends, frozen on the code before
/// the optimisations: the digests of all the frames and of the packets equal theirs. After an
/// intended change of the drawing, the printed values renew the frozen ones.
#[test]
fn book_draws_its_frozen_frames() {
    const FRAMES: u64 = 0x2e27_397e_3ed2_47ba;
    const PACKETS: u64 = 0x66c0_d6f0_dc58_15c2;
    let Some(server) = rc_testdata::need(server_book(), "whole recipe book from the integrated server") else { return };
    let Some(e) = tests_opt::env() else { return };
    for (name, opts) in [PATHS[0], NEW_PATH] {
        let (frames, packets, sent) = frozen_run(server, &e, opts);
        let all = digest(&frames.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>());
        let changing = frames.windows(2).filter(|w| w[0] != w[1]).count();
        eprintln!("recipe book scenario, {name} path: frames {all:#018x}, packets {packets:#018x} ({sent} sent), {changing} frames changed");
        for (k, f) in frames.iter().enumerate().step_by(10) {
            eprintln!("  frame {k}: {f:#018x}");
        }
        assert!(changing > 200, "the scenario barely moves: {changing} frames differ from the previous one");
        assert_eq!((all, packets, sent), (FRAMES, PACKETS, 333), "{name} path: frames or packets of the recipe book changed");
    }
}
