//! Recipe book (`docs/specs/creative-inventory.md` § 9–12): model (known recipes,
//! collections, craftable), display on the left of inventory, crafting table and furnace screens,
//! ghost recipe, placement (`place_recipe`).

use hashbrown::{HashMap, HashSet};

use rc_client::recipes::{HolderSet, RecipeDisplay, RecipeEntry, RecipeEvent, SlotDisplay};
use rc_proto::varint::write_varint;
use rc_proto_v777::item::ItemStack;
use rc_text::{Component, StyledChar};

use crate::creative::Received;
use crate::gui::{Blend, Gui};
use crate::hud::HotbarItems;
use crate::inventory::PlayerInventory;
use crate::menu::{self, Kind};
use crate::screens::{Cached, decorate, refresh, tooltip_lines};
use crate::widgets::TextField;

/// Book types: 0 crafting table (and inventory), 1 furnace, 2 blast furnace, 3 smoker.
pub fn book_type(kind: Kind) -> Option<usize> {
    match kind {
        Kind::Player | Kind::Crafting => Some(0),
        Kind::Furnace(v) => Some(1 + usize::from(v.min(2))),
        _ => None,
    }
}

/// Book tab: categories (names of `recipe_book_category`, empty = search) and icons.
struct TabDef {
    categories: &'static [&'static str],
    icons: &'static [&'static str],
}

const CRAFT_SEARCH: &[&str] = &["crafting_equipment", "crafting_building_blocks", "crafting_misc", "crafting_redstone"];

fn tabs(book: usize) -> &'static [TabDef] {
    const C: &[TabDef] = &[
        TabDef { categories: &[], icons: &["compass"] },
        TabDef { categories: &["crafting_equipment"], icons: &["iron_axe", "golden_sword"] },
        TabDef { categories: &["crafting_building_blocks"], icons: &["bricks"] },
        TabDef { categories: &["crafting_misc"], icons: &["lava_bucket", "apple"] },
        TabDef { categories: &["crafting_redstone"], icons: &["redstone"] },
    ];
    const F: &[TabDef] = &[
        TabDef { categories: &[], icons: &["compass"] },
        TabDef { categories: &["furnace_food"], icons: &["porkchop"] },
        TabDef { categories: &["furnace_blocks"], icons: &["stone"] },
        TabDef { categories: &["furnace_misc"], icons: &["lava_bucket", "emerald"] },
    ];
    const B: &[TabDef] = &[
        TabDef { categories: &[], icons: &["compass"] },
        TabDef { categories: &["blast_furnace_blocks"], icons: &["redstone_ore"] },
        TabDef { categories: &["blast_furnace_misc"], icons: &["iron_shovel", "golden_leggings"] },
    ];
    const S: &[TabDef] = &[
        TabDef { categories: &[], icons: &["compass"] },
        TabDef { categories: &["smoker_food"], icons: &["porkchop"] },
    ];
    [C, F, B, S][book.min(3)]
}

fn search_categories(book: usize) -> &'static [&'static str] {
    [
        CRAFT_SEARCH,
        &["furnace_food", "furnace_blocks", "furnace_misc"][..],
        &["blast_furnace_blocks", "blast_furnace_misc"][..],
        &["smoker_food"][..],
    ][book.min(3)]
}

/// Ghost recipe: slots of the current menu and stacks to show (§ 11.4).
pub struct Ghost {
    pub window: i32,
    pub slots: Vec<(usize, Vec<ItemStack>, bool)>,
}

/// Variants menu (§ 11.5).
struct Overlay {
    x: i32,
    y: i32,
    cols: i32,
    recipes: Vec<(i32, bool)>,
}

/// Collection: recipes of the same (category, group), in order of encounter.
struct Collection {
    category: i32,
    recipes: Vec<i32>,
}

/// Resolution context for slot displays.
pub struct Ctx<'a> {
    pub data: &'a rc_data::VersionData,
    pub rec: &'a Received,
    pub item_names: &'a [String],
}

impl Ctx<'_> {
    fn stack(&self, item: i32) -> ItemStack {
        ItemStack { item, count: 1, added: Vec::new(), removed: Vec::new() }
    }

    fn holder_set(&self, h: &HolderSet) -> Vec<i32> {
        match h {
            HolderSet::Ids(v) => v.clone(),
            HolderSet::Tag(t) => {
                let t = t.strip_prefix('#').unwrap_or(t);
                let t = if t.contains(':') { t.to_owned() } else { format!("minecraft:{t}") };
                self.rec.item_tags().iter().find(|(n, _)| *n == t).map(|(_, v)| v.clone()).unwrap_or_default()
            }
        }
    }

    fn component(&self, name: &str) -> Option<u16> {
        self.data.registry("minecraft:data_component_type")?.entries.iter().position(|n| n == name).map(|i| i as u16)
    }

    /// Stacks of a slot display, in order (§ 9.4).
    pub fn resolve(&self, d: &SlotDisplay) -> Vec<ItemStack> {
        match d {
            SlotDisplay::Empty => Vec::new(),
            SlotDisplay::AnyFuel => {
                let rules = FuelNames(self.item_names);
                (0..self.item_names.len() as i32).filter(|&i| rules.is_fuel(i)).map(|i| self.stack(i)).collect()
            }
            SlotDisplay::WithAnyPotion(x) => {
                let base = self.resolve(x);
                let Some(pc) = self.component("minecraft:potion_contents") else { return base };
                let n = self.data.registry("minecraft:potion").map_or(0, |r| r.entries.len());
                let mut out = Vec::new();
                for p in 0..n {
                    let mut b = vec![1];
                    write_varint(&mut b, p as i32);
                    b.extend_from_slice(&[0, 0, 0]);
                    for s in &base {
                        let mut s = s.clone();
                        s.added.retain(|c| c.0 != pc);
                        s.added.push((pc, b.clone().into_boxed_slice()));
                        out.push(s);
                    }
                }
                out
            }
            SlotDisplay::OnlyWithComponent(x, c) => {
                self.resolve(x).into_iter().filter(|s| s.added.iter().any(|a| i32::from(a.0) == *c)).collect()
            }
            SlotDisplay::Item(i) => vec![self.stack(*i)],
            SlotDisplay::Stack(s) => vec![s.clone()],
            SlotDisplay::Tag(h) => self.holder_set(h).into_iter().map(|i| self.stack(i)).collect(),
            SlotDisplay::Dyed(_, target) => self.resolve(target).into_iter().take(16).collect(),
            SlotDisplay::SmithingTrim(base, _) => self.resolve(base).into_iter().take(16).collect(),
            SlotDisplay::WithRemainder(x) => self.resolve(x),
            SlotDisplay::Composite(l) => l.iter().flat_map(|x| self.resolve(x)).collect(),
        }
    }
}

/// Fuels approximated by name (like the furnace prediction, `inventories.md` § 6.2).
struct FuelNames<'a>(&'a [String]);

impl FuelNames<'_> {
    fn is_fuel(&self, i: i32) -> bool {
        let n = self.0.get(i as usize).map_or("", |n| n.strip_prefix("minecraft:").unwrap_or(n));
        matches!(n, "coal" | "charcoal" | "coal_block" | "lava_bucket" | "blaze_rod" | "stick" | "bamboo")
            || ["_log", "_planks", "_wood", "_stem", "_sapling", "_slab", "_fence"].iter().any(|e| n.ends_with(e))
    }
}

/// Corner of the variants menu (`total` recipes over `cols` columns) opened from the button
/// `(x, y)`, book center `(cx, cy)`: shifted by a whole number of buttons (25 px) so as not to
/// exceed cx + 50 on the right, cy + 50 at the bottom (⌈⌉), cy − 100 at the top.
fn overlay_origin((x, y): (i32, i32), total: i32, cols: i32, (cx, cy): (i32, i32)) -> (i32, i32) {
    let rows = (total + cols - 1) / cols;
    let (mut ox, mut oy) = (x, y);
    let right = ox + total.min(cols) * 25;
    if right > cx + 50 {
        ox -= 25 * ((right - (cx + 50)) / 25);
    }
    let bottom = oy + rows * 25;
    if bottom > cy + 50 {
        oy -= 25 * ((bottom - (cy + 50) + 24) / 25);
    }
    if oy < cy - 100 {
        // Negative overflow: truncated division equals ⌈⌉.
        oy -= 25 * ((oy - (cy - 100)) / 25);
    }
    (ox, oy)
}

/// Assignment: each ingredient receives one unit of an item of its set (§ 10.2).
fn can_assign(ingredients: &[Vec<i32>], counts: &HashMap<i32, i32>) -> bool {
    fn augment(
        i: usize,
        ingredients: &[Vec<i32>],
        counts: &HashMap<i32, i32>,
        owner: &mut HashMap<i32, Vec<usize>>,
        seen: &mut HashSet<i32>,
    ) -> bool {
        for &t in &ingredients[i] {
            let cap = counts.get(&t).copied().unwrap_or(0);
            if cap <= 0 || !seen.insert(t) {
                continue;
            }
            let used = owner.get(&t).map_or(0, Vec::len) as i32;
            if used < cap {
                owner.entry(t).or_default().push(i);
                return true;
            }
            let holders = owner.get(&t).cloned().unwrap_or_default();
            for j in holders {
                if augment(j, ingredients, counts, owner, seen) {
                    let v = owner.get_mut(&t).expect("owner");
                    if let Some(p) = v.iter().position(|&x| x == j) {
                        v[p] = i;
                    }
                    return true;
                }
            }
        }
        false
    }
    let mut owner: HashMap<i32, Vec<usize>> = HashMap::new();
    (0..ingredients.len())
        .all(|i| ingredients[i].is_empty() || augment(i, ingredients, counts, &mut owner, &mut HashSet::new()))
}

#[derive(Default)]
pub struct RecipeBook {
    /// Known recipes and insertion rank.
    known: HashMap<i32, (RecipeEntry, u64)>,
    inserted: u64,
    pub highlight: HashSet<i32>,
    /// (open, filter) per book type.
    pub settings: [(bool, bool); 4],
    collections: Vec<Collection>,
    /// Book visible for the open screen.
    pub visible: bool,
    book: usize,
    tab: usize,
    page: usize,
    pub search: TextField,
    pub focused: bool,
    pub ghost: Option<Ghost>,
    last_placed: Option<i32>,
    /// Last recipe placed by a click since the screen opened (Enter / Space).
    last_recipe: Option<i32>,
    /// Click sounds to play (book buttons).
    pub clicks: u32,
    /// End of the bounce of each slot of the page, in `clock` ticks (newly shown recipe).
    anim: [f32; 20],
    /// Elapsed ticks, Ctrl included (the bounce does not stop with Ctrl, scrolling does).
    pub clock: u64,
    overlay: Option<Overlay>,
    craftable: HashSet<i32>,
    /// Fingerprint of the available content (recompute of the "craftable").
    stamp: u64,
    /// Time in ticks (does not advance with Ctrl).
    pub time: u64,
    cache: Vec<Option<Cached>>,
    cursor: usize,
    index: Option<(u64, HashMap<i32, Vec<String>>)>,
    generation: u64,
    tooltip: Option<(ItemStack, Vec<Vec<StyledChar>>, i32)>,
    /// Notifications of unlocked recipes to create: (station stack, result stack).
    pub unlocked: Vec<(Option<ItemStack>, Option<ItemStack>)>,
    /// Tracked screen (window, type): a change reapplies the settings.
    pub screen: Option<(i32, Kind)>,
    /// Release to swallow after a click consumed by the book.
    pub swallow_release: bool,
    /// Demo `RUSTCRAFT_BOOKTEST`: elapsed ticks.
    pub test: u32,
}

/// Geometry (§ 11.1–11.2).
struct Layout {
    bx: i32,
    by: i32,
    tabs: Vec<(i32, i32, usize)>,
    buttons: Vec<(i32, i32, usize)>,
    pages: usize,
}

impl RecipeBook {
    pub fn new() -> RecipeBook {
        let mut search = TextField::new(Vec::new());
        search.max_len = 50;
        search.text_color = 0xffff_ffff;
        RecipeBook { search, ..Default::default() }
    }

    /// Client-bound packet (§ 9.5).
    pub fn on_event(&mut self, e: RecipeEvent, ctx: &Ctx<'_>) {
        match e {
            RecipeEvent::Add(entries, replace) => {
                if replace {
                    self.known.clear();
                    self.highlight.clear();
                }
                for (entry, flags) in entries {
                    let id = entry.id;
                    if flags & 1 != 0 {
                        let station = ctx.resolve(entry.display.station()).into_iter().next();
                        let result = ctx.resolve(entry.display.result()).into_iter().next();
                        self.unlocked.push((station, result));
                    }
                    if flags & 2 != 0 {
                        self.highlight.insert(id);
                    }
                    let rank = self.known.get(&id).map_or_else(
                        || {
                            self.inserted += 1;
                            self.inserted
                        },
                        |e| e.1,
                    );
                    self.known.insert(id, (entry, rank));
                }
            }
            RecipeEvent::Remove(ids) => {
                for id in ids {
                    self.known.remove(&id);
                    self.highlight.remove(&id);
                }
            }
            RecipeEvent::Settings(s) => self.settings = s,
            RecipeEvent::Ghost { .. } => return,
        }
        self.regroup();
        self.generation += 1;
        self.stamp = 0;
    }

    /// Iteration order of the recipe table (§ 14, point 1): id modulo the
    /// capacity, then insertion order.
    fn ordered(&self) -> Vec<i32> {
        let mut cap = 16usize;
        while self.known.len() * 4 > cap * 3 {
            cap *= 2;
        }
        let mut ids: Vec<(usize, u64, i32)> =
            self.known.iter().map(|(&id, (_, rank))| (id as usize % cap, *rank, id)).collect();
        ids.sort_unstable();
        ids.into_iter().map(|x| x.2).collect()
    }

    /// Grouping into collections (§ 10.1).
    fn regroup(&mut self) {
        let mut out: Vec<Collection> = Vec::new();
        let mut by_group: HashMap<(i32, i32), usize> = HashMap::new();
        for id in self.ordered() {
            let e = &self.known[&id].0;
            match e.group {
                Some(g) => match by_group.get(&(e.category, g)) {
                    Some(&k) => out[k].recipes.push(id),
                    None => {
                        by_group.insert((e.category, g), out.len());
                        out.push(Collection { category: e.category, recipes: vec![id] });
                    }
                },
                None => out.push(Collection { category: e.category, recipes: vec![id] }),
            }
        }
        self.collections = out;
    }

    /// Recipe displayable in this screen (§ 10.2).
    fn selected(&self, id: i32, kind: Kind) -> bool {
        let Some((e, _)) = self.known.get(&id) else { return false };
        let (gw, gh) = match kind {
            Kind::Player => (2, 2),
            Kind::Crafting => (3, 3),
            _ => return matches!(e.display, RecipeDisplay::Furnace { .. }),
        };
        match &e.display {
            RecipeDisplay::Shaped { width, height, .. } => *width <= gw && *height <= gh,
            RecipeDisplay::Shapeless { ingredients, .. } => ingredients.len() as i32 <= gw * gh,
            _ => false,
        }
    }

    /// Available content (§ 10.2): main inventory and crafting grid.
    fn available(inv: &PlayerInventory, kind: Kind, ctx: &Ctx<'_>) -> HashMap<i32, i32> {
        let comp = |n: &str| ctx.component(n);
        let (damage, ench, name) =
            (comp("minecraft:damage"), comp("minecraft:enchantments"), comp("minecraft:custom_name"));
        let plain = |s: &ItemStack| {
            !s.added.iter().any(|(t, b)| {
                Some(*t) == name
                    || (Some(*t) == ench && b.first().is_some_and(|&n| n > 0))
                    || (Some(*t) == damage && b.first().is_some_and(|&d| d > 0))
            })
        };
        let mut out: HashMap<i32, i32> = HashMap::new();
        let mut add = |s: &ItemStack| {
            if plain(s) {
                let max = ctx.data.items.get(s.item as usize).map_or(64, |i| i32::from(i.max_stack_size)).max(1);
                *out.entry(s.item).or_default() += s.count.min(max);
            }
        };
        for s in inv.menu[9..45].iter().flatten() {
            add(s);
        }
        let (_, slots, _, _) = crate::screens::current(inv);
        let grid = match kind {
            Kind::Player => 1..5,
            Kind::Crafting => 1..10,
            _ => 0..0,
        };
        for s in slots.get(grid).into_iter().flatten().flatten() {
            add(s);
        }
        out
    }

    /// Recompute of the "craftable" if the content changed (§ 10.2).
    pub fn refresh_craftable(&mut self, inv: &PlayerInventory, kind: Kind, ctx: &Ctx<'_>) {
        use std::hash::{Hash, Hasher};
        let counts = Self::available(inv, kind, ctx);
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let mut v: Vec<_> = counts.iter().collect();
        v.sort();
        v.hash(&mut h);
        format!("{kind:?}").hash(&mut h);
        self.generation.hash(&mut h);
        let stamp = h.finish() | 1;
        if stamp == self.stamp {
            return;
        }
        self.stamp = stamp;
        self.craftable.clear();
        for (&id, (e, _)) in &self.known {
            if !self.selected(id, kind) {
                continue;
            }
            let Some(req) = &e.requirements else { continue };
            let sets: Vec<Vec<i32>> = req.iter().map(|h| ctx.holder_set(h)).collect();
            if can_assign(&sets, &counts) {
                self.craftable.insert(id);
            }
        }
    }

    fn category_ids(&self, ctx: &Ctx<'_>, names: &[&str]) -> Vec<i32> {
        let reg = ctx.data.registry("minecraft:recipe_book_category").map(|r| &r.entries[..]).unwrap_or(&[]);
        names
            .iter()
            .filter_map(|n| reg.iter().position(|e| e.strip_prefix("minecraft:").unwrap_or(e) == *n).map(|i| i as i32))
            .collect()
    }

    /// Collections of a tab (§ 10.1), in category order for the search.
    fn tab_collections(&self, tab: usize, ctx: &Ctx<'_>) -> Vec<usize> {
        let def = &tabs(self.book)[tab.min(tabs(self.book).len() - 1)];
        let cats = if def.categories.is_empty() {
            self.category_ids(ctx, search_categories(self.book))
        } else {
            self.category_ids(ctx, def.categories)
        };
        cats.iter()
            .flat_map(|&c| self.collections.iter().enumerate().filter(move |(_, x)| x.category == c).map(|(i, _)| i))
            .collect()
    }

    fn shown_recipes(&self, c: usize, kind: Kind) -> Vec<i32> {
        let filter = self.settings[self.book].1;
        self.collections[c]
            .recipes
            .iter()
            .copied()
            .filter(|&id| if filter { self.craftable.contains(&id) } else { self.selected(id, kind) })
            .collect()
    }

    /// Collections listed on the pages (§ 11.2).
    fn listed(&mut self, kind: Kind, ctx: &Ctx<'_>, gui: &Gui, it: &HotbarItems<'_>, ench: &[String]) -> Vec<usize> {
        let q = self.search.value().to_lowercase();
        if !q.is_empty() {
            self.ensure_index(ctx, gui, it, ench);
        }
        let filter = self.settings[self.book].1;
        let idx = self.index.as_ref().map(|x| &x.1);
        self.tab_collections(self.tab, ctx)
            .into_iter()
            .filter(|&c| self.collections[c].recipes.iter().any(|&id| self.selected(id, kind)))
            .filter(|&c| {
                q.is_empty()
                    || self.collections[c].recipes.iter().any(|id| {
                        idx.and_then(|m| m.get(id)).is_some_and(|lines| match q.split_once(':') {
                            None => lines.iter().any(|l| l.contains(&q)),
                            Some((a, b)) => lines.iter().any(|l| {
                                l.split_once(':').is_some_and(|(ns, p)| ns.contains(a.trim()) && p.contains(b.trim()))
                            }),
                        })
                    })
            })
            .filter(|&c| !filter || self.collections[c].recipes.iter().any(|id| self.craftable.contains(id)))
            .collect()
    }

    /// Search index (§ 10.3): tooltip lines of the results and ids.
    fn ensure_index(&mut self, ctx: &Ctx<'_>, gui: &Gui, it: &HotbarItems<'_>, ench: &[String]) {
        let key = self.generation ^ (gui.lang.map.len() as u64).rotate_left(40);
        if self.index.as_ref().is_some_and(|x| x.0 == key) {
            return;
        }
        let mut m = HashMap::new();
        for (&id, (e, _)) in &self.known {
            let mut lines = Vec::new();
            for s in ctx.resolve(e.display.result()) {
                lines.extend(crate::screens::tooltip_search_lines(&s, it, ench, gui, crate::screens::advanced()));
                if let Some(n) = ctx.item_names.get(s.item as usize) {
                    lines.push(n.clone());
                }
            }
            m.insert(id, lines);
        }
        self.index = Some((key, m));
    }

    /// Recipe shown by the `n`-th listed collection (demos).
    pub fn nth_recipe(&self, kind: Kind, ctx: &Ctx<'_>, n: usize) -> Option<i32> {
        let cats: Vec<i32> = self.collections.iter().map(|c| c.category).collect();
        let sel = self.known.keys().filter(|&&id| self.selected(id, kind)).count();
        eprintln!(
            "book: {} recipes, {} collections, {} selected, tab {} → {} collections, categories {:?}, settings {:?}",
            self.known.len(),
            self.collections.len(),
            sel,
            self.tab,
            self.tab_collections(self.tab, ctx).len(),
            {
                let mut c = cats.clone();
                c.sort_unstable();
                c.dedup();
                c
            },
            self.settings
        );
        let cols: Vec<usize> = self
            .tab_collections(self.tab, ctx)
            .into_iter()
            .filter(|&c| self.collections[c].recipes.iter().any(|&id| self.selected(id, kind)))
            .collect();
        self.shown_recipes(*cols.get(n)?, kind).first().copied()
    }

    /// Opening a screen: book visible according to the settings (§ 11.1).
    pub fn on_screen(&mut self, kind: Kind) {
        match book_type(kind) {
            Some(b) => {
                if b != self.book {
                    self.tab = 0;
                    self.page = 0;
                }
                self.book = b;
                self.visible = self.settings[b].0;
            }
            None => self.visible = false,
        }
        self.overlay = None;
        self.focused = false;
        self.stamp = 0;
        self.last_recipe = None;
    }

    fn narrow(gui: &Gui) -> bool {
        (gui.width as i32) < 379
    }

    fn layout(&self, gui: &Gui, kind: Kind, listed: usize, ctx: &Ctx<'_>) -> Layout {
        let d = if Self::narrow(gui) { 0 } else { 86 };
        let bx = (gui.width as i32 - 147) / 2 - d;
        let by = (gui.height as i32 - 166) / 2;
        let mut tabs_out = Vec::new();
        for (t, def) in tabs(self.book).iter().enumerate() {
            let show = def.categories.is_empty() || {
                let cols = self.tab_collections(t, ctx);
                cols.iter().any(|&c| self.collections[c].recipes.iter().any(|&id| self.selected(id, kind)))
            };
            if show {
                let i = tabs_out.len() as i32;
                tabs_out.push((bx - 30, by + 3 + 27 * i, t));
            }
        }
        let pages = listed.div_ceil(20).max(1);
        let page = self.page.min(pages - 1);
        let buttons = (0..20)
            .filter(|i| page * 20 + i < listed)
            .map(|i| (bx + 11 + 25 * (i as i32 % 5), by + 31 + 25 * (i as i32 / 5), page * 20 + i))
            .collect();
        Layout { bx, by, tabs: tabs_out, buttons, pages }
    }

    /// Book button of the screen (`inventories.md` § 5.1, 5.5, 5.6).
    pub fn button_pos(gui: &Gui, kind: Kind, gx: i32) -> Option<(i32, i32)> {
        let h2 = gui.height as i32 / 2;
        match kind {
            Kind::Player => Some((gx + 104, h2 - 22)),
            Kind::Crafting => Some((gx + 5, h2 - 49)),
            Kind::Furnace(_) => Some((gx + 20, h2 - 49)),
            _ => None,
        }
    }

    /// Book toggle (button): setting sent to the server.
    pub fn toggle(&mut self, link: &rc_client::link::Link) {
        self.visible = !self.visible;
        self.settings[self.book].0 = self.visible;
        self.overlay = None;
        let (o, f) = self.settings[self.book];
        let _ = link.send_recipe_book_settings(self.book as i32, o, f);
    }

    /// Ghost recipe received (§ 11.4).
    pub fn on_ghost(&mut self, window: i32, display: &RecipeDisplay, inv: &PlayerInventory, ctx: &Ctx<'_>) {
        let (kind, slots, cur, _) = crate::screens::current(inv);
        if window != cur || book_type(kind).is_none() {
            return;
        }
        let mut out = Vec::new();
        out.push((if matches!(kind, Kind::Furnace(_)) { 2 } else { 0 }, ctx.resolve(display.result()), true));
        let (gw, gh) = if kind == Kind::Player { (2, 2) } else { (3, 3) };
        match display {
            RecipeDisplay::Shaped { width, height, ingredients, .. } => {
                let dy = if *height < gh / 2 + gh % 2 && *height * 2 < gh { gh / 2 - height / 2 } else { 0 };
                let dx = if *width * 2 < gw { gw / 2 - width / 2 } else { 0 };
                for (k, d) in ingredients.iter().enumerate() {
                    let (c, r) = (k as i32 % width, k as i32 / width);
                    let cell = (r + dy) * gw + c + dx;
                    out.push((1 + cell as usize, ctx.resolve(d), false));
                }
            }
            RecipeDisplay::Shapeless { ingredients, .. } => {
                for (k, d) in ingredients.iter().enumerate() {
                    out.push((1 + k, ctx.resolve(d), false));
                }
            }
            RecipeDisplay::Furnace { ingredient, fuel, .. } => {
                out.push((0, ctx.resolve(ingredient), false));
                if slots.get(1).is_some_and(Option::is_none) {
                    out.push((1, ctx.resolve(fuel), false));
                }
            }
            _ => {}
        }
        out.retain(|x| !x.1.is_empty());
        self.ghost = Some(Ghost { window, slots: out });
    }

    /// Click on a crafting slot: ghost and last recipe cleared (§ 12).
    pub fn crafting_click(&mut self, kind: Kind, slot: i16) {
        let craft = match kind {
            Kind::Player => (0..5).contains(&slot),
            Kind::Crafting => (0..10).contains(&slot),
            Kind::Furnace(_) => (0..3).contains(&slot),
            _ => false,
        };
        if craft {
            self.ghost = None;
            self.last_placed = None;
            self.stamp = 0;
        }
    }

    /// Placement of a recipe; false if refused (non-craftable recipe already placed). Retained for
    /// Enter / Space.
    fn place(&mut self, id: i32, window: i32, shift: bool, narrow: bool, link: &rc_client::link::Link) -> bool {
        if !self.craftable.contains(&id) && self.last_placed == Some(id) {
            return false;
        }
        self.last_placed = Some(id);
        self.last_recipe = Some(id);
        self.ghost = None;
        let _ = link.send_place_recipe(window, id, shift);
        if narrow {
            self.visible = false;
            self.overlay = None;
        }
        true
    }

    /// Mouse press (0 left, 1 right); true if the book consumed it.
    #[allow(clippy::too_many_arguments)]
    pub fn press(
        &mut self,
        gui: &Gui,
        inv: &PlayerInventory,
        it: &HotbarItems<'_>,
        ench: &[String],
        ctx: &Ctx<'_>,
        m: (f32, f32),
        button: u8,
        shift: bool,
        link: &rc_client::link::Link,
    ) -> bool {
        let (kind, slots, window, _) = crate::screens::current(inv);
        let n = slots.len();
        let (gx, gy, _, h) = crate::screens::origin(gui, kind, n, (self.visible, Self::narrow(gui)));
        let (mx, my) = (m.0.floor() as i32, m.1.floor() as i32);
        let inside = |x: i32, y: i32, w: i32, hh: i32| mx >= x && mx < x + w && my >= y && my < y + hh;
        if let Some((bx, by)) = Self::button_pos(gui, kind, gx)
            && button == 0
            && inside(bx, by, 20, 18)
        {
            self.clicks += 1;
            self.toggle(link);
            return true;
        }
        if !self.visible {
            return false;
        }
        // Variants menu open: every click goes to it (§ 11.5). A left click on a
        // variant places it (click sound) and the menu stays open; any other click closes it.
        if let Some(o) = self.overlay.take() {
            let hit = (button == 0)
                .then(|| {
                    o.recipes.iter().enumerate().find_map(|(i, &(id, _))| {
                        let (x, y) = (o.x + 4 + 25 * (i as i32 % o.cols), o.y + 5 + 25 * (i as i32 / o.cols));
                        inside(x, y, 24, 24).then_some(id)
                    })
                })
                .flatten();
            if let Some(id) = hit {
                self.clicks += 1;
                self.overlay = Some(o);
                let narrow = Self::narrow(gui);
                let _ = self.place(id, window, shift, narrow, link);
            }
            return true;
        }
        let listed = self.listed(kind, ctx, gui, it, ench);
        let l = self.layout(gui, kind, listed.len(), ctx);
        self.focused = false;
        if inside(l.bx + 25, l.by + 13, 81, 14) || inside(l.bx + 8, l.by + 13, 17, 14) {
            self.focused = true;
            return true;
        }
        if button == 0 && inside(l.bx + 110, l.by + 12, 26, 16) {
            self.clicks += 1;
            self.settings[self.book].1 = !self.settings[self.book].1;
            let (o, f) = self.settings[self.book];
            let _ = link.send_recipe_book_settings(self.book as i32, o, f);
            self.stamp = 0;
            return true;
        }
        for &(x, y, t) in &l.tabs {
            let x = if t == self.tab { x - 2 } else { x };
            if button == 0 && inside(x, y, 35, 27) {
                // Tab button: click sound even on the already chosen tab.
                self.clicks += 1;
                if self.tab != t {
                    self.tab = t;
                    self.page = 0;
                }
                return true;
            }
        }
        let page = self.page.min(l.pages - 1);
        if button == 0 && l.pages > 1 {
            if page + 1 < l.pages && inside(l.bx + 93, l.by + 137, 12, 17) {
                self.clicks += 1;
                self.page = page + 1;
                return true;
            }
            if page > 0 && inside(l.bx + 38, l.by + 137, 12, 17) {
                self.clicks += 1;
                self.page = page - 1;
                return true;
            }
        }
        for &(x, y, k) in &l.buttons {
            if !inside(x, y, 25, 25) {
                continue;
            }
            let c = listed[k];
            let shown = self.shown_recipes(c, kind);
            if shown.is_empty() {
                return true;
            }
            // Recipe button (left or right): click sound, even without a variants menu.
            if button <= 1 {
                self.clicks += 1;
            }
            if button == 1 && shown.len() > 1 {
                let mut recipes: Vec<(i32, bool)> =
                    shown.iter().filter(|id| self.craftable.contains(*id)).map(|&id| (id, true)).collect();
                if !self.settings[self.book].1 {
                    recipes.extend(shown.iter().filter(|id| !self.craftable.contains(*id)).map(|&id| (id, false)));
                }
                let t = recipes.len() as i32;
                let cols = if t <= 16 { 4 } else { 5 };
                let (ox, oy) = overlay_origin((x, y), t, cols, (l.bx + 73, l.by + 13 + 83));
                self.overlay = Some(Overlay { x: ox, y: oy, cols, recipes });
                return true;
            }
            if button == 0 {
                let m = shown.len();
                let id = shown[(self.time / 30) as usize % m];
                let _ = self.place(id, window, shift, Self::narrow(gui), link);
            }
            return true;
        }
        // Narrow book: every click outside the book is swallowed; otherwise clicks in the book too.
        let in_book = inside(l.bx, l.by, 147, 166) || inside(l.bx - 30, l.by, 30, 166);
        let _ = (gy, h);
        in_book || Self::narrow(gui)
    }

    /// Key (book visible, outside spectator): (consumed, click sound). Game order:
    /// Esc in narrow mode hides the book; search with focus: everything goes to it except
    /// Esc; chat key: focus of the search (its character is not typed);
    /// Enter / Space: the last clicked recipe is placed again (click sound).
    #[allow(clippy::too_many_arguments)]
    pub fn key(
        &mut self,
        k: winit::keyboard::KeyCode,
        mods: winit::keyboard::ModifiersState,
        text: Option<&str>,
        chat_key: bool,
        narrow: bool,
        window: i32,
        link: &rc_client::link::Link,
    ) -> (bool, bool) {
        use winit::keyboard::KeyCode as K;
        if !self.visible {
            return (false, false);
        }
        if k == K::Escape && narrow {
            self.toggle(link);
            return (true, false);
        }
        if self.focused {
            if k == K::Escape {
                return (false, false);
            }
            let (_, mut changed) = self.search.key(k, mods);
            if let Some(t) = text.filter(|t| t.chars().all(|c| !c.is_control())) {
                let before = self.search.text.clone();
                self.search.insert(t);
                changed |= before != self.search.text;
            }
            if changed {
                self.page = 0;
            }
            return (true, false);
        }
        if chat_key {
            self.focused = true;
            return (true, false);
        }
        if matches!(k, K::Enter | K::Space | K::NumpadEnter)
            && let Some(id) = self.last_recipe
        {
            return (self.place(id, window, mods.shift_key(), narrow, link), true);
        }
        (false, false)
    }

    /// Drawing (§ 11) after the container screen.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        gui: &mut Gui,
        queue: &wgpu::Queue,
        inv: &PlayerInventory,
        it: &HotbarItems<'_>,
        ench: &[String],
        ctx: &Ctx<'_>,
        mouse: Option<(f32, f32)>,
        link: &rc_client::link::Link,
    ) {
        let (kind, slots, window, _) = crate::screens::current(inv);
        if book_type(kind).is_none() {
            return;
        }
        self.cursor = 0;
        let n = slots.len();
        let narrow = Self::narrow(gui);
        let (gx, gy, _, _) = crate::screens::origin(gui, kind, n, (self.visible, narrow));
        let f = |v: i32| v as f32;
        let (mx, my) = mouse.map_or((-1000, -1000), |(x, y)| (x.floor() as i32, y.floor() as i32));
        let inside = |x: i32, y: i32, w: i32, h: i32| mx >= x && mx < x + w && my >= y && my < y + h;
        let mut items: Vec<(ItemStack, f32, f32, f32, bool)> = Vec::new();
        let mut tip: Option<Vec<Vec<StyledChar>>> = None;
        let mut tip_stack: Option<ItemStack> = None;
        // Book button.
        gui.next_layer();
        if let Some((bx, by)) = Self::button_pos(gui, kind, gx) {
            let name = if inside(bx, by, 20, 18) { "recipe_book/button_highlighted" } else { "recipe_book/button" };
            gui.sprite(name, f(bx), f(by), 20.0, 18.0, 0xffff_ffff);
        }
        // Ghost recipe on the screen's slots.
        if let Some(g) = &self.ghost
            && g.window == window
            && !(self.visible && narrow)
        {
            for (slot, stacks, result) in &g.slots {
                let d = menu::slot_def(kind, *slot, n);
                let (x, y) = (gx + d.x, gy + d.y);
                if *result && kind != Kind::Player {
                    gui.fill(f(x - 4), f(y - 4), f(x + 20), f(y + 20), 0x30ff_0000);
                } else {
                    gui.fill(f(x), f(y), f(x + 16), f(y + 16), 0x30ff_0000);
                }
                let s = stacks[(self.time / 30) as usize % stacks.len()].clone();
                if inside(x, y, 16, 16) {
                    tip_stack = Some(s.clone());
                }
                items.push((s, f(x), f(y), 1.0, *result));
            }
        }
        if !self.visible {
            self.flush_items(gui, queue, it, &mut items);
            if let Some(g) = &self.ghost
                && g.window == window
            {
                for (slot, _, _) in &g.slots {
                    let d = menu::slot_def(kind, *slot, n);
                    gui.fill(f(gx + d.x), f(gy + d.y), f(gx + d.x + 16), f(gy + d.y + 16), 0x30ff_ffff);
                }
            }
            if let Some(s) = tip_stack {
                self.draw_stack_tooltip(gui, queue, it, ench, &s, mx, my);
            }
            return;
        }
        let _ = gy;
        self.refresh_craftable(inv, kind, ctx);
        let listed = self.listed(kind, ctx, gui, it, ench);
        let l = self.layout(gui, kind, listed.len(), ctx);
        if self.page >= l.pages {
            self.page = l.pages - 1;
        }
        if !l.tabs.iter().any(|t| t.2 == self.tab) {
            self.tab = 0;
        }
        gui.next_layer();
        gui.sprite_region("bg/recipe_book", 256.0, 256.0, 1.0, 1.0, f(l.bx), f(l.by), 147.0, 166.0);
        // Search field.
        let (sx, sy) = (l.bx + 25, l.by + 13);
        if self.search.text.is_empty() && !self.focused {
            gui.sprite("widget/text_field", f(sx), f(sy), 81.0, 14.0, 0xffff_ffff);
            let hint = rc_text::styled_chars(&rc_text::flatten(
                &Component::translate("gui.recipebook.search_hint", vec![]),
                &gui.lang,
            ));
            gui.text(queue, &hint, f(sx + 4), f(sy + 3), 0xff80_8080, false, 0);
        } else {
            let blink = self.focused && (self.time / 6).is_multiple_of(2);
            crate::widgets::draw_field_blink(gui, queue, &self.search, [sx, sy, 81, 14], self.focused, blink);
        }
        // Filter.
        let filter = self.settings[self.book].1;
        let pre = if self.book == 0 { "recipe_book/filter" } else { "recipe_book/furnace_filter" };
        let hl = if inside(l.bx + 110, l.by + 12, 26, 16) { "_highlighted" } else { "" };
        let st = if filter { "enabled" } else { "disabled" };
        gui.sprite(&format!("{pre}_{st}{hl}"), f(l.bx + 110), f(l.by + 12), 26.0, 16.0, 0xffff_ffff);
        if !hl.is_empty() {
            let key = if filter { ["craftable", "smeltable", "blastable", "smokable"][self.book] } else { "all" };
            tip = Some(vec![rc_text::styled_chars(&rc_text::flatten(
                &Component::translate(format!("gui.recipebook.toggleRecipes.{key}"), vec![]),
                &gui.lang,
            ))]);
        }
        // Tabs.
        for &(x, y, t) in &l.tabs {
            let sel = t == self.tab;
            let x = if sel { x - 2 } else { x };
            gui.sprite(
                if sel { "recipe_book/tab_selected" } else { "recipe_book/tab" },
                f(x),
                f(y),
                35.0,
                27.0,
                0xffff_ffff,
            );
            let icons = tabs(self.book)[t].icons;
            let xs: &[i32] = if icons.len() == 1 { &[9] } else { &[3, 14] };
            for (icon, dx) in icons.iter().zip(xs) {
                let at = rc_data::ids::with_key(&["minecraft:", icon], |k| ctx.item_names.iter().position(|n| *n == k));
                if let Some(id) = at {
                    items.push((ctx.stack(id as i32), f(x + dx), f(y + 5), 1.0, false));
                }
            }
        }
        // Page.
        let page = self.page;
        for &(x, y, k) in &l.buttons {
            let c = listed[k];
            let shown = self.shown_recipes(c, kind);
            if shown.is_empty() {
                continue;
            }
            // Highlighted recipes that became visible (§ 11.3): seen, and the slot bounces
            // for 15 ticks (animation attached to the slot; `anim` = end tick).
            let slot = k % 20;
            let now = self.clock as f32 + it.pt;
            for r in &shown {
                if self.highlight.remove(r) {
                    let _ = link.send_recipe_seen(*r);
                    self.anim[slot] = now + 15.0;
                }
            }
            let left = self.anim[slot] - now;
            let squeeze = if left > 0.0 { 1.0 + 0.1 * (left / 15.0 * std::f32::consts::PI).sin() } else { 1.0 };
            // Scale around (x + 8, y + 12).
            let (px, py) = (f(x + 8), f(y + 12));
            let at = |v: i32, p: f32| p + (v as f32 - p) * squeeze;
            let any = self.collections[c].recipes.iter().any(|id| self.craftable.contains(id));
            let many = if shown.len() > 1 { "slot_many_" } else { "slot_" };
            let bg = format!("recipe_book/{many}{}", if any { "craftable" } else { "uncraftable" });
            gui.sprite(&bg, at(x, px), at(y, py), 25.0 * squeeze, 25.0 * squeeze, 0xffff_ffff);
            let cyc = (self.time / 30) as usize;
            let m = shown.len();
            let id = shown[cyc % m];
            let results = self.known.get(&id).map(|e| ctx.resolve(e.0.display.result())).unwrap_or_default();
            if results.is_empty() {
                continue;
            }
            let s = results[(cyc / m) % results.len()].clone();
            // Same display for all the recipes: all the displayed items of all the
            // recipes identical (item and components) to the first.
            let same = m > 1 && {
                let all: Vec<ItemStack> = shown
                    .iter()
                    .flat_map(|r| self.known.get(r).map(|e| ctx.resolve(e.0.display.result())).unwrap_or_default())
                    .collect();
                all.iter().all(|o| o.item == all[0].item && o.added == all[0].added && o.removed == all[0].removed)
            };
            if same {
                items.push((s.clone(), at(x + 5, px), at(y + 5, py), squeeze, false));
                items.push((s.clone(), at(x + 3, px), at(y + 3, py), squeeze, false));
            } else {
                items.push((s.clone(), at(x + 4, px), at(y + 4, py), squeeze, false));
            }
            if self.overlay.is_none() && inside(x, y, 25, 25) {
                let mut lines = tooltip_lines(&s, it, ench, gui, crate::screens::advanced());
                if m > 1 {
                    lines.push(rc_text::styled_chars(&rc_text::flatten(
                        &Component::translate("gui.recipebook.moreRecipes", vec![]),
                        &gui.lang,
                    )));
                }
                tip = Some(lines);
            }
        }
        if l.pages > 1 {
            if page + 1 < l.pages {
                let h = if inside(l.bx + 93, l.by + 137, 12, 17) { "_highlighted" } else { "" };
                gui.sprite(
                    &format!("recipe_book/page_forward{h}"),
                    f(l.bx + 93),
                    f(l.by + 137),
                    12.0,
                    17.0,
                    0xffff_ffff,
                );
            }
            if page > 0 {
                let h = if inside(l.bx + 38, l.by + 137, 12, 17) { "_highlighted" } else { "" };
                gui.sprite(
                    &format!("recipe_book/page_backward{h}"),
                    f(l.bx + 38),
                    f(l.by + 137),
                    12.0,
                    17.0,
                    0xffff_ffff,
                );
            }
            let t = rc_text::styled_chars(&rc_text::flatten(
                &Component::translate(
                    "gui.recipebook.page",
                    vec![rc_text::Arg::Text((page + 1).to_string()), rc_text::Arg::Text(l.pages.to_string())],
                ),
                &gui.lang,
            ));
            let w = rc_text::width(gui, &t);
            gui.text(queue, &t, f(l.bx + 73 - w / 2), f(l.by + 141), 0xffff_ffff, true, 0);
        }
        self.flush_items(gui, queue, it, &mut items);
        // Veil of the ghost slots.
        if let Some(g) = &self.ghost
            && g.window == window
            && !narrow
        {
            for (slot, _, _) in &g.slots {
                let d = menu::slot_def(kind, *slot, n);
                gui.fill(f(gx + d.x), f(gy + d.y), f(gx + d.x + 16), f(gy + d.y + 16), 0x30ff_ffff);
            }
        }
        // Variants menu.
        if let Some(o) = &self.overlay {
            gui.next_layer();
            let t = o.recipes.len() as i32;
            let rows = (t + o.cols - 1) / o.cols;
            gui.sprite(
                "recipe_book/overlay_recipe",
                f(o.x),
                f(o.y),
                f(t.min(o.cols) * 25 + 8),
                f(rows * 25 + 8),
                0xffff_ffff,
            );
            let furnace = self.book != 0;
            let mut mini = Vec::new();
            for (i, &(id, ok)) in o.recipes.iter().enumerate() {
                let (x, y) = (o.x + 4 + 25 * (i as i32 % o.cols), o.y + 5 + 25 * (i as i32 / o.cols));
                let base = if furnace { "recipe_book/furnace_overlay" } else { "recipe_book/crafting_overlay" };
                let name = format!(
                    "{base}{}{}",
                    if ok { "" } else { "_disabled" },
                    if inside(x, y, 24, 24) { "_highlighted" } else { "" }
                );
                gui.sprite(&name, f(x), f(y), 24.0, 24.0, 0xffff_ffff);
                let Some((e, _)) = self.known.get(&id) else { continue };
                let cells: Vec<(i32, i32, &SlotDisplay)> = match &e.display {
                    RecipeDisplay::Shaped { width, ingredients, .. } => {
                        ingredients.iter().enumerate().map(|(k, d)| (k as i32 % width, k as i32 / width, d)).collect()
                    }
                    RecipeDisplay::Shapeless { ingredients, .. } => {
                        ingredients.iter().enumerate().map(|(k, d)| (k as i32 % 3, k as i32 / 3, d)).collect()
                    }
                    RecipeDisplay::Furnace { ingredient, .. } => vec![(1, 1, ingredient)],
                    _ => Vec::new(),
                };
                for (c, r, d) in cells {
                    let st = ctx.resolve(d);
                    if st.is_empty() {
                        continue;
                    }
                    let s = st[(self.time / 30) as usize % st.len()].clone();
                    let (cx, cy) = (x + 2 + 3 + 7 * c, y + 2 + 3 + 7 * r);
                    mini.push((s, f(cx) - 3.0, f(cy) - 3.0, 0.375, false));
                }
            }
            self.flush_items(gui, queue, it, &mut mini);
        }
        if let Some(s) = tip_stack {
            self.draw_stack_tooltip(gui, queue, it, ench, &s, mx, my);
        } else if let Some(lines) = tip {
            let w = lines.iter().map(|l| rc_text::width(gui, l)).max().unwrap_or(0);
            // Recipe button: component tooltip (no gap under the name).
            crate::screens::draw_text_tooltip(gui, queue, &lines, w, mx, my);
        } else if let Some((bx, by)) = Self::button_pos(gui, kind, gx)
            && inside(bx, by, 20, 18)
        {
            let _ = by;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_stack_tooltip(
        &mut self,
        gui: &mut Gui,
        queue: &wgpu::Queue,
        it: &HotbarItems<'_>,
        ench: &[String],
        s: &ItemStack,
        mx: i32,
        my: i32,
    ) {
        if self.tooltip.as_ref().is_none_or(|t| t.0 != *s) {
            let lines = tooltip_lines(s, it, ench, gui, crate::screens::advanced());
            let w = lines.iter().map(|l| rc_text::width(gui, l)).max().unwrap_or(0);
            self.tooltip = Some((s.clone(), lines, w));
        }
        if let Some((_, lines, w)) = &self.tooltip {
            // Ghost slot: component tooltip, with the stack's style.
            let info = usize::try_from(s.item).ok().and_then(|i| it.infos.get(i));
            let view = crate::stack::StackRef { stack: s, info, ids: it.ids, data: it.data, hold: crate::stack::Hold::NONE };
            let style = crate::screens::tooltip_style(&view);
            crate::screens::draw_tooltip_ex(gui, queue, lines, *w, (mx, my), false, style.as_deref());
        }
    }

    /// Up-to-date layers then drawing of the pending items (x, y, scale, decorations).
    fn flush_items(
        &mut self,
        gui: &mut Gui,
        queue: &wgpu::Queue,
        it: &HotbarItems<'_>,
        items: &mut Vec<(ItemStack, f32, f32, f32, bool)>,
    ) {
        if items.is_empty() {
            return;
        }
        // Same indices from one frame to the next: the remembered render of an unchanged stack is used.
        let base = self.cursor;
        if self.cache.len() < base + items.len() {
            self.cache.resize_with(base + items.len(), || None);
        }
        if let Some(models) = gui.items.as_ref().map(|a| &a.models) {
            for (k, (s, ..)) in items.iter().enumerate() {
                refresh(&mut self.cache[base + k], s, it, models, 2000 + k as i32);
            }
        }
        gui.stratum(Blend::Premultiplied);
        for (k, (_, x, y, sc, _)) in items.iter().enumerate() {
            if let Some(c) = &self.cache[base + k]
                && !c.layers.is_empty()
            {
                if *sc == 1.0 {
                    gui.item(c.key, &c.layers, *x, *y, 0.0);
                } else {
                    gui.item_scaled(c.key, &c.layers, *x, *y, *sc);
                }
            }
        }
        gui.stratum(Blend::Normal);
        for (k, (_, x, y, _, deco)) in items.iter().enumerate() {
            if *deco && let Some(c) = &self.cache[base + k] {
                decorate(gui, queue, c, *x, *y, (c.stack.count, false));
            }
        }
        self.cursor += items.len();
        items.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_rises_by_whole_buttons() {
        // Center (100, 100): maximum bottom 150. Button at y = 140, 2 rows → bottom 190,
        // overflow 40 → raise by 2 buttons (50), not by 40 + 24 = 64.
        assert_eq!(overlay_origin((0, 140), 8, 4, (100, 100)), (0, 90));
        // Right: 4 columns from x = 120 → 220 > 150, overflow 70 → 2 buttons.
        assert_eq!(overlay_origin((120, 0), 4, 4, (100, 100)), (70, 0));
    }

    #[test]
    fn assignment_uses_each_unit_once() {
        let counts: HashMap<i32, i32> = [(1, 1), (2, 1)].into_iter().collect();
        // The first ingredient accepts 1 or 2, the second only 1: it must be reassigned.
        assert!(can_assign(&[vec![1, 2], vec![1]], &counts));
        assert!(!can_assign(&[vec![1], vec![1]], &counts));
        assert!(can_assign(&[vec![2], vec![1], vec![]], &counts));
        let many: HashMap<i32, i32> = [(5, 4)].into_iter().collect();
        assert!(can_assign(&vec![vec![5]; 4], &many));
        assert!(!can_assign(&vec![vec![5]; 5], &many));
    }
}
