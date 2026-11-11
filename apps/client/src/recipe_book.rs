//! Recipe book (`docs/specs/creative-inventory.md` § 9–12): model (known recipes,
//! collections, craftable), display on the left of inventory, crafting table and furnace screens,
//! ghost recipe, placement (`place_recipe`).

use std::rc::Rc;

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
use crate::screens::{Cached, SearchIndexKey, decorate, refresh};
use crate::tooltips::shared_cache::{self, ExtendedTooltip, ItemTooltip};
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

/// Item to draw: stack, corner, scale across and down, count and bar drawn.
type Drawn = (ItemStack, f32, f32, (f32, f32), bool);

/// Tooltip lines of a button and the button's rectangle.
type WidgetTip = (Vec<Vec<StyledChar>>, (i32, i32, i32, i32));

/// Offset of a shaped recipe of `size` cells on a grid side of `grid` cells: centred only
/// when it takes less than half of the side (a 1-wide recipe on 3 columns goes to the
/// middle one, a 2-wide stays on the left).
fn centre_offset(grid: i32, size: i32) -> i32 {
    if (size as f32) < grid as f32 / 2.0 { (grid as f32 / 2.0 - size as f32 / 2.0).floor() as i32 } else { 0 }
}

/// Thumbnail cells (column, row, ingredient) of a variant on the 3×3 pattern of the
/// variants menu: shaped centred like the placement on the grid, shapeless in reading
/// order, furnace in the middle.
fn overlay_cells(display: &RecipeDisplay) -> Vec<(i32, i32, &SlotDisplay)> {
    match display {
        RecipeDisplay::Shaped { width, height, ingredients, .. } => {
            let (dx, dy) = (centre_offset(3, *width), centre_offset(3, *height));
            ingredients.iter().enumerate().map(|(k, d)| (k as i32 % width + dx, k as i32 / width + dy, d)).collect()
        }
        RecipeDisplay::Shapeless { ingredients, .. } => {
            ingredients.iter().enumerate().map(|(k, d)| (k as i32 % 3, k as i32 / 3, d)).collect()
        }
        RecipeDisplay::Furnace { ingredient, .. } => vec![(1, 1, ingredient)],
        _ => Vec::new(),
    }
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
        crate::stack::component_type_id(self.data, name)
    }

    /// The stack carries the component: in its patch, or by default without having removed it.
    fn has(&self, s: &ItemStack, c: u16) -> bool {
        s.component(c).is_some()
            || (!s.removed.contains(&c)
                && usize::try_from(s.item).ok().and_then(|i| self.data.items.get(i)).is_some_and(|i| i.components.contains(&c)))
    }

    /// Colour (0..16) of a dye: the stack's `dye` component, else the default, else white.
    fn dye_of(&self, s: &ItemStack, dye: Option<u16>) -> usize {
        if let Some(c) = dye {
            if let Some(b) = s.component(c) {
                let v = rc_proto::varint::read_varint(&mut rc_proto::Reader::new(b));
                return v.map_or(0, |v| v.clamp(0, 15) as usize);
            }
            if s.removed.contains(&c) {
                return 0;
            }
        }
        let info = usize::try_from(s.item).ok().and_then(|i| self.data.items.get(i));
        info.map_or(0, |i| usize::from(i.dye.saturating_sub(1)))
    }

    /// Dyeing demo: targets × dyes (the target varies fastest), 16 at most, each target
    /// receiving the colour mixed with its own.
    fn dyed_demo(&self, dye: &SlotDisplay, target: &SlotDisplay) -> Vec<ItemStack> {
        let targets = self.resolve(target);
        let dyes = if targets.is_empty() { Vec::new() } else { self.resolve(dye) };
        let Some(dc) = self.component("minecraft:dyed_color") else { return Vec::new() };
        let dye_c = self.component("minecraft:dye");
        let n = targets.len();
        (0..n * dyes.len())
            .take(16)
            .map(|k| {
                let (t, d) = (&targets[k % n], &dyes[k / n]);
                let current = t.component(dc).and_then(|b| Some(i32::from_be_bytes(b.get(..4)?.try_into().ok()?)));
                let color = crate::entity_render::DYE_COLORS[self.dye_of(d, dye_c)] as i32;
                let rgb = rc_sim::mobs::apply_dyes(current, &[color]);
                let mut s = t.clone();
                s.count = 1;
                s.added.retain(|a| a.0 != dc);
                s.removed.retain(|r| *r != dc);
                s.added.push((dc, rgb.to_be_bytes().into()));
                s
            })
            .collect()
    }

    /// Stacks of a slot display, in order (§ 9.4).
    pub fn resolve(&self, d: &SlotDisplay) -> Vec<ItemStack> {
        match d {
            SlotDisplay::Empty => Vec::new(),
            // Items with a default `cooking_fuel`, grouped by value (groups in the order of their
            // first item), in id order within a group.
            SlotDisplay::AnyFuel => {
                let mut fuels: Vec<(u16, i32)> = (self.data.items.iter().enumerate())
                    .filter(|(_, i)| i.fuel != 0)
                    .map(|(k, i)| (i.fuel, k as i32))
                    .collect();
                fuels.sort_unstable();
                fuels.into_iter().map(|(_, i)| self.stack(i)).collect()
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
                let Ok(c) = u16::try_from(*c) else { return Vec::new() };
                self.resolve(x).into_iter().filter(|s| self.has(s, c)).collect()
            }
            SlotDisplay::Item(i) => vec![self.stack(*i)],
            SlotDisplay::Stack(s) => vec![s.clone()],
            SlotDisplay::Tag(h) => self.holder_set(h).into_iter().map(|i| self.stack(i)).collect(),
            SlotDisplay::Dyed(dye, target) => self.dyed_demo(dye, target),
            SlotDisplay::SmithingTrim(base, _) => self.resolve(base).into_iter().take(16).collect(),
            SlotDisplay::WithRemainder(x) => self.resolve(x),
            SlotDisplay::Composite(l) => l.iter().flat_map(|x| self.resolve(x)).collect(),
        }
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
    /// End of the vertical bounce of each tab, in `clock` ticks (tab holding a newly
    /// unlocked recipe); `retab` asks for the check at the next drawing (book shown, recipes changed).
    tab_anim: [f32; 5],
    retab: bool,
    /// The search reads the easter egg word: the client switches to the pirate language.
    pub pirate: bool,
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
    index: Option<(SearchIndexKey, HashMap<i32, Vec<String>>)>,
    generation: u64,
    /// Tooltip of the hovered recipe button; its own input: the "more recipes" line is shown.
    button_tooltip: Option<ExtendedTooltip<bool>>,
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
        // Any book packet refreshes the tabs of the open screen, bounce included.
        self.retab = true;
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
        let counts = Self::available(inv, kind, ctx);
        let mut v: Vec<_> = counts.iter().collect();
        v.sort();
        // `| 1`: never the "to recompute" stamp (0).
        let stamp = crate::cache_key::cache_key(&(v, format!("{kind:?}"), self.generation)) | 1;
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
            self.ensure_index(ctx, &gui.lang, it, ench);
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
    fn ensure_index(&mut self, ctx: &Ctx<'_>, lang: &rc_text::Lang, it: &HotbarItems<'_>, ench: &[String]) {
        let key = SearchIndexKey { language: lang.generation, listed_content: self.generation, server_data: 0 };
        if self.index.as_ref().is_some_and(|x| x.0 == key) {
            return;
        }
        let mut m = HashMap::new();
        for (&id, (e, _)) in &self.known {
            let mut lines = Vec::new();
            for s in ctx.resolve(e.display.result()) {
                lines.extend(crate::screens::tooltip_search_lines(&s, it, ench, lang));
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
        // New screen: fresh tab buttons, checked at the first drawing.
        self.tab_anim = [0.0; 5];
        self.retab = true;
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

    /// Book toggle (button, Esc in narrow mode).
    pub fn toggle(&mut self, link: &rc_client::link::Link) {
        self.set_visible(!self.visible, link);
    }

    /// Shows or hides the book: the open setting follows and the settings go to the
    /// server each time. Shown: fresh search field (no focus) and tab buttons; hidden:
    /// variants menu closed.
    pub fn set_visible(&mut self, visible: bool, link: &rc_client::link::Link) {
        if visible {
            self.focused = false;
            self.tab_anim = [0.0; 5];
            self.retab = true;
        } else {
            self.overlay = None;
        }
        self.visible = visible;
        self.settings[self.book].0 = visible;
        let (o, f) = self.settings[self.book];
        let _ = link.send_recipe_book_settings(self.book as i32, o, f);
    }

    /// Screen tick: the book follows the open setting (the server can change it), and
    /// every change of visibility echoes the settings back like a toggle.
    pub fn follow_settings(&mut self, link: &rc_client::link::Link) {
        let open = self.settings[self.book].0;
        if self.visible != open {
            self.set_visible(open, link);
        }
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
                let (dx, dy) = (centre_offset(gw, *width), centre_offset(gh, *height));
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
    /// Enter / Space, which place again without hiding the book.
    fn place(&mut self, id: i32, window: i32, shift: bool, link: &rc_client::link::Link) -> bool {
        if !self.craftable.contains(&id) && self.last_placed == Some(id) {
            return false;
        }
        self.last_placed = Some(id);
        self.last_recipe = Some(id);
        self.ghost = None;
        let _ = link.send_place_recipe(window, id, shift);
        true
    }

    /// Placement by a click (recipe button or variant): the narrow book then closes, which
    /// writes the open setting and sends it (otherwise the next tick would show it again).
    fn place_clicked(&mut self, id: i32, window: i32, shift: bool, narrow: bool, link: &rc_client::link::Link) {
        if self.place(id, window, shift, link) && narrow {
            self.set_visible(false, link);
        }
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
                self.place_clicked(id, window, shift, Self::narrow(gui), link);
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
                self.place_clicked(id, window, shift, Self::narrow(gui), link);
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
            let (mut used, mut changed) = self.search.key(k, mods);
            if let Some(t) = text.and_then(crate::widgets::chat_typed) {
                let before = self.search.text.clone();
                self.search.insert(&t);
                changed |= before != self.search.text;
                used = true;
            }
            if changed {
                self.page = 0;
            }
            // Every input taken by the field rereads the easter egg word.
            if used && self.search.value().to_lowercase() == "excitedze" {
                self.pirate = true;
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
            return (self.place(id, window, mods.shift_key(), link), true);
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
        let mut items: Vec<Drawn> = Vec::new();
        // Tooltip of the hovered recipe button: the result's tooltip, and whether the button
        // stands for several recipes.
        let mut button_tip: Option<(Rc<ItemTooltip>, bool)> = None;
        // Tooltip of a button (filter, page arrows): placed against the button, not the mouse.
        let mut widget_tip: Option<WidgetTip> = None;
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
                items.push((s, f(x), f(y), (1.0, 1.0), *result));
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
                Self::draw_stack_tooltip(gui, queue, it, ench, &s, mx, my);
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
            let line = Component::translate(format!("gui.recipebook.toggleRecipes.{key}"), vec![]);
            widget_tip = Some((vec![rc_text::styled_chars(&rc_text::flatten(&line, &gui.lang))], (l.bx + 110, l.by + 12, 26, 16)));
        }
        // Tabs. A category tab holding a newly unlocked recipe (among those its filter shows)
        // bounces vertically for 15 ticks; checked when the book shows and when recipes change,
        // before the page marks its recipes as seen.
        let now = self.clock as f32 + it.pt;
        self.start_tab_bounce(&l.tabs, kind, ctx, now);
        for &(x, y, t) in &l.tabs {
            let sel = t == self.tab;
            let x = if sel { x - 2 } else { x };
            let left = self.tab_anim[t] - now;
            let squeeze = if left > 0.0 { 1.0 + 0.1 * (left / 15.0 * std::f32::consts::PI).sin() } else { 1.0 };
            // Vertical scale around y + 12.
            let at = |v: i32| f(y + 12) + f(v - y - 12) * squeeze;
            gui.sprite(
                if sel { "recipe_book/tab_selected" } else { "recipe_book/tab" },
                f(x),
                at(y),
                35.0,
                27.0 * squeeze,
                0xffff_ffff,
            );
            let icons = tabs(self.book)[t].icons;
            let xs: &[i32] = if icons.len() == 1 { &[9] } else { &[3, 14] };
            for (icon, dx) in icons.iter().zip(xs) {
                let id = rc_data::ids::with_key(&["minecraft:", icon], |k| ctx.item_names.iter().position(|n| *n == k));
                if let Some(id) = id {
                    items.push((ctx.stack(id as i32), f(x + dx), at(y + 5), (1.0, squeeze), false));
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
                items.push((s.clone(), at(x + 5, px), at(y + 5, py), (squeeze, squeeze), false));
                items.push((s.clone(), at(x + 3, px), at(y + 3, py), (squeeze, squeeze), false));
            } else {
                items.push((s.clone(), at(x + 4, px), at(y + 4, py), (squeeze, squeeze), false));
            }
            if self.overlay.is_none() && inside(x, y, 25, 25) {
                let result = shared_cache::item_tooltip(&s, it, ench, gui, crate::screens::advanced(), false);
                button_tip = Some((result, m > 1));
            }
        }
        if l.pages > 1 {
            // Page arrows: highlighted sprite and tooltip while hovered.
            let arrows = [(page + 1 < l.pages, 93, "forward", "next_page"), (page > 0, 38, "backward", "previous_page")];
            for (shown, dx, sprite, key) in arrows {
                if !shown {
                    continue;
                }
                let r = (l.bx + dx, l.by + 137, 12, 17);
                let hover = inside(r.0, r.1, r.2, r.3);
                let h = if hover { "_highlighted" } else { "" };
                gui.sprite(&format!("recipe_book/page_{sprite}{h}"), f(r.0), f(r.1), 12.0, 17.0, 0xffff_ffff);
                if hover {
                    let line = Component::translate(format!("gui.recipebook.{key}"), vec![]);
                    widget_tip = Some((vec![rc_text::styled_chars(&rc_text::flatten(&line, &gui.lang))], r));
                }
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
                for (c, r, d) in overlay_cells(&e.display) {
                    let st = ctx.resolve(d);
                    if st.is_empty() {
                        continue;
                    }
                    let s = st[(self.time / 30) as usize % st.len()].clone();
                    let (cx, cy) = (x + 2 + 3 + 7 * c, y + 2 + 3 + 7 * r);
                    mini.push((s, f(cx) - 3.0, f(cy) - 3.0, (0.375, 0.375), false));
                }
            }
            self.flush_items(gui, queue, it, &mut mini);
        }
        if let Some(s) = tip_stack {
            Self::draw_stack_tooltip(gui, queue, it, ench, &s, mx, my);
        } else if let Some((result, more_recipes)) = button_tip {
            let tooltip = ExtendedTooltip::refresh(&mut self.button_tooltip, result, more_recipes, gui, |lines| {
                if more_recipes {
                    let line = Component::translate("gui.recipebook.moreRecipes", vec![]);
                    lines.push(rc_text::styled_chars(&rc_text::flatten(&line, &gui.lang)));
                }
            });
            // Recipe button: component tooltip (no gap under the name).
            crate::screens::draw_text_tooltip(gui, queue, &tooltip.lines, tooltip.width, mx, my);
        } else if let Some((lines, r)) = widget_tip {
            crate::widgets::draw_tooltip(gui, queue, &lines, r, mouse);
        }
    }

    /// Pending tab check (book shown, recipes changed): every shown category tab holding a
    /// highlighted recipe among those its filter shows starts its 15-tick bounce at `now`.
    fn start_tab_bounce(&mut self, shown: &[(i32, i32, usize)], kind: Kind, ctx: &Ctx<'_>, now: f32) {
        if !std::mem::take(&mut self.retab) {
            return;
        }
        for &(_, _, t) in shown {
            let hot = !tabs(self.book)[t].categories.is_empty()
                && self
                    .tab_collections(t, ctx)
                    .into_iter()
                    .any(|c| self.shown_recipes(c, kind).iter().any(|id| self.highlight.contains(id)));
            if hot {
                self.tab_anim[t] = now + 15.0;
            }
        }
    }

    /// Ghost slot: component tooltip (no gap under the name), with the stack's style.
    fn draw_stack_tooltip(
        gui: &mut Gui,
        queue: &wgpu::Queue,
        it: &HotbarItems<'_>,
        ench: &[String],
        s: &ItemStack,
        mx: i32,
        my: i32,
    ) {
        let tooltip = shared_cache::item_tooltip(s, it, ench, gui, crate::screens::advanced(), false);
        let style = tooltip.style.as_deref();
        crate::screens::draw_tooltip_ex(gui, queue, &tooltip.lines, tooltip.width, (mx, my), false, style);
    }

    /// Up-to-date layers then drawing of the pending items (x, y, scale, decorations).
    fn flush_items(
        &mut self,
        gui: &mut Gui,
        queue: &wgpu::Queue,
        it: &HotbarItems<'_>,
        items: &mut Vec<Drawn>,
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
                if *sc == (1.0, 1.0) {
                    gui.item(c.key, &c.layers, *x, *y, 0.0);
                } else {
                    gui.item_stretched(c.key, &c.layers, *x, *y, sc.0, sc.1);
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

    /// Slot displays resolved like the game: fuels from `cooking_fuel` (grouped by value),
    /// default components, coloured dyeing demo.
    #[test]
    fn slot_displays_follow_item_components() {
        let data = rc_testdata::version_data().expect("version data missing (extraction)");
        let names = data.registry("minecraft:item").expect("item registry").entries.clone();
        let rec = Received::default();
        let ctx = Ctx { data: &data, rec: &rec, item_names: &names };
        let id = |n: &str| names.iter().position(|x| x == &format!("minecraft:{n}")).expect(n) as i32;
        let items = |v: &[ItemStack]| v.iter().map(|s| s.item).collect::<Vec<_>>();
        // Fuels: 347 items from the report, each value in one block.
        let fuels = items(&ctx.resolve(&SlotDisplay::AnyFuel));
        assert_eq!(fuels.len(), 347);
        for n in ["coal", "oak_planks", "lava_bucket", "white_wool", "dried_kelp_block"] {
            assert!(fuels.contains(&id(n)), "{n} fuel");
        }
        for n in ["stone_slab", "crimson_stem", "bucket"] {
            assert!(!fuels.contains(&id(n)), "{n} not a fuel");
        }
        let groups: Vec<u16> = fuels.iter().map(|&i| data.items[i as usize].fuel).collect();
        assert!(groups.windows(2).all(|w| w[0] <= w[1]), "values grouped");
        // Default component: the red dye carries `dye` without a patch.
        let comp = |n: &str| ctx.component(n).expect(n);
        let only = SlotDisplay::OnlyWithComponent(
            Box::new(SlotDisplay::Composite(vec![SlotDisplay::Item(id("red_dye")), SlotDisplay::Item(id("stone"))])),
            i32::from(comp("minecraft:dye")),
        );
        assert_eq!(items(&ctx.resolve(&only)), vec![id("red_dye")]);
        // Dyeing: fastest target, colour of the dye (red 0xB02E26, blue 0x3C44AA).
        let both = |a: &str, b: &str| {
            Box::new(SlotDisplay::Composite(vec![SlotDisplay::Item(id(a)), SlotDisplay::Item(id(b))]))
        };
        let dyed = ctx.resolve(&SlotDisplay::Dyed(both("red_dye", "blue_dye"), both("leather_helmet", "leather_boots")));
        let dc = comp("minecraft:dyed_color");
        let shown: Vec<(i32, i32)> = dyed
            .iter()
            .map(|s| (s.item, i32::from_be_bytes(s.component(dc).expect("colour").try_into().unwrap())))
            .collect();
        let (h, b) = (id("leather_helmet"), id("leather_boots"));
        assert_eq!(shown, vec![(h, 0xB02E26), (b, 0xB02E26), (h, 0x3C44AA), (b, 0x3C44AA)]);
        // Target already dyed: mixture (black 0x1D1D21 + white 0xF9FFFE).
        let black = ItemStack { item: h, count: 3, added: vec![(dc, 0x1D1D21i32.to_be_bytes().into())], removed: Vec::new() };
        let mixed = ctx.resolve(&SlotDisplay::Dyed(
            Box::new(SlotDisplay::Item(id("white_dye"))),
            Box::new(SlotDisplay::Stack(black)),
        ));
        assert_eq!(mixed.len(), 1);
        assert_eq!(mixed[0].count, 1);
        let rgb = i32::from_be_bytes(mixed[0].component(dc).unwrap().try_into().unwrap());
        assert_eq!(rgb, rc_sim::mobs::apply_dyes(Some(0x1D1D21), &[0xF9FFFE]));
        assert_eq!(mixed[0].added.iter().filter(|a| a.0 == dc).count(), 1);
    }

    /// In-memory link and the packet ids it sends.
    fn link() -> Option<(rc_client::Session, rc_client::memory::Peer)> {
        let data = rc_testdata::version_data()?;
        Some(rc_client::memory::session(std::sync::Arc::new(data), 1).expect("session"))
    }

    fn sent(peer: &rc_client::memory::Peer) -> Vec<Vec<u8>> {
        peer.packets.try_iter().collect()
    }

    fn settings_packet(s: &rc_client::Session, book: u8, open: bool, filter: bool) -> Vec<u8> {
        let mut v = Vec::new();
        write_varint(&mut v, s.link.ids.recipe_book_change_settings);
        v.extend([book, u8::from(open), u8::from(filter)]);
        v
    }

    /// Narrow book: a placement by click hides the book AND writes the open setting sent to
    /// the server, so the tick that follows the setting keeps it hidden.
    #[test]
    fn narrow_click_placement_keeps_the_book_closed() {
        let Some((s, peer)) = link() else { return };
        let mut b = RecipeBook::new();
        b.settings[0] = (true, false);
        b.on_screen(Kind::Crafting);
        assert!(b.visible);
        b.place_clicked(7, 3, false, true, &s.link);
        assert!(!b.visible);
        assert_eq!(b.settings[0], (false, false));
        b.follow_settings(&s.link);
        assert!(!b.visible, "the next tick must not reopen the book");
        let p = sent(&peer);
        assert_eq!(p.len(), 2, "placement then settings");
        let mut place = Vec::new();
        write_varint(&mut place, s.link.ids.place_recipe);
        place.extend([3, 7, 0]);
        assert_eq!(p[0], place);
        assert_eq!(p[1], settings_packet(&s, 0, false, false));
        // Wide book: the placement leaves it open, no settings sent.
        b.settings[0].0 = true;
        b.on_screen(Kind::Player);
        b.place_clicked(8, 0, true, false, &s.link);
        assert!(b.visible);
        assert_eq!(sent(&peer).len(), 1);
    }

    /// Enter / Space place the last recipe again without hiding the narrow book.
    #[test]
    fn enter_in_narrow_mode_keeps_the_book_shown() {
        let Some((s, peer)) = link() else { return };
        let mut b = RecipeBook::new();
        b.settings[0] = (true, false);
        b.on_screen(Kind::Crafting);
        b.last_recipe = Some(5);
        let none = winit::keyboard::ModifiersState::empty();
        let (used, click) = b.key(winit::keyboard::KeyCode::Enter, none, None, false, true, 2, &s.link);
        assert!(used && click);
        assert!(b.visible && b.settings[0].0);
        assert_eq!(sent(&peer).len(), 1, "placement only");
    }

    /// The server opens the book: the screen follows at the tick and echoes the settings;
    /// Esc in narrow mode closes it the same way.
    #[test]
    fn open_setting_is_followed_and_echoed() {
        let Some((s, peer)) = link() else { return };
        let mut b = RecipeBook::new();
        b.on_screen(Kind::Furnace(0));
        assert!(!b.visible);
        b.focused = true;
        b.settings[1] = (true, true);
        b.follow_settings(&s.link);
        assert!(b.visible);
        assert!(!b.focused, "fresh search field");
        assert_eq!(sent(&peer), vec![settings_packet(&s, 1, true, true)]);
        b.follow_settings(&s.link);
        assert!(sent(&peer).is_empty(), "no change, nothing sent");
        let none = winit::keyboard::ModifiersState::empty();
        let _ = b.key(winit::keyboard::KeyCode::Escape, none, None, false, true, 1, &s.link);
        assert!(!b.visible);
        assert_eq!(sent(&peer), vec![settings_packet(&s, 1, false, true)]);
    }

    /// Typing the easter egg word in the search asks for the pirate language.
    #[test]
    fn search_word_asks_for_the_pirate_language() {
        let Some((s, _peer)) = link() else { return };
        let mut b = RecipeBook::new();
        b.settings[0] = (true, false);
        b.on_screen(Kind::Player);
        b.focused = true;
        let none = winit::keyboard::ModifiersState::empty();
        let k = winit::keyboard::KeyCode::KeyE;
        let _ = b.key(k, none, Some("Excited"), false, false, 0, &s.link);
        assert!(!b.pirate);
        let _ = b.key(k, none, Some("ze"), false, false, 0, &s.link);
        assert!(b.pirate);
    }

    /// Reference walk of the official grid placement, in our words: rows from the top, a short
    /// recipe (less than half of the side) starts at the middle row / column.
    fn grid_walk(grid: i32, w: i32, h: i32, n: usize) -> Vec<(i32, i32)> {
        let mut out = Vec::new();
        let mut y = 0;
        while y < grid {
            let start_y = (grid as f32 / 2.0 - h as f32 / 2.0).floor() as i32;
            if (h as f32) < grid as f32 / 2.0 && start_y > y {
                y += 1;
            }
            for x in 0..grid {
                if out.len() == n {
                    return out;
                }
                let start_x = (grid as f32 / 2.0 - w as f32 / 2.0).floor() as i32;
                let centred = (w as f32) < grid as f32 / 2.0;
                let (end, take) = if centred { (start_x + w, start_x <= x && x < start_x + w) } else { (w, x < w) };
                if take {
                    out.push((x, y));
                } else if x == end {
                    break;
                }
            }
            y += 1;
        }
        out
    }

    #[test]
    fn shaped_cells_follow_the_grid_placement() {
        for grid in [2, 3] {
            for w in 1..=grid {
                for h in 1..=grid {
                    let n = (w * h) as usize;
                    let ours: Vec<(i32, i32)> = (0..n as i32)
                        .map(|k| (k % w + centre_offset(grid, w), k / w + centre_offset(grid, h)))
                        .collect();
                    assert_eq!(ours, grid_walk(grid, w, h, n), "grid {grid}, recipe {w}×{h}");
                }
            }
        }
        // Thumbnails of the variants menu: stick (1×2) in the middle column, slab (3×1) on the
        // middle row.
        let d = |w: i32, h: i32| RecipeDisplay::Shaped {
            width: w,
            height: h,
            ingredients: vec![SlotDisplay::Empty; (w * h) as usize],
            result: SlotDisplay::Empty,
            station: SlotDisplay::Empty,
        };
        let cells = |r: &RecipeDisplay| overlay_cells(r).into_iter().map(|(c, r, _)| (c, r)).collect::<Vec<_>>();
        assert_eq!(cells(&d(1, 2)), vec![(1, 0), (1, 1)]);
        assert_eq!(cells(&d(3, 1)), vec![(0, 1), (1, 1), (2, 1)]);
        assert_eq!(cells(&d(2, 2)), vec![(0, 0), (1, 0), (0, 1), (1, 1)]);
    }

    /// Index built in English, language switched to French (same number of keys): the next search
    /// indexes the French name, like the game that rebuilds its search trees.
    #[test]
    fn search_index_follows_a_language_change() {
        use crate::screens::SwordInTwoLanguages;
        let f = SwordInTwoLanguages::new(rc_data::VersionData::default());
        let rec = Received::default();
        let ctx = Ctx { data: &f.data, rec: &rec, item_names: &f.names };
        let display = RecipeDisplay::Shapeless {
            ingredients: vec![SlotDisplay::Item(0)],
            result: SlotDisplay::Item(0),
            station: SlotDisplay::Empty,
        };
        let entry = RecipeEntry { id: 1, display, group: None, category: 0, requirements: None };
        let mut b = RecipeBook::new();
        b.known.insert(1, (entry, 0));
        let mut indexed = |lang: &rc_text::Lang| {
            b.ensure_index(&ctx, lang, &f.items(), &[]);
            b.index.as_ref().expect("index built").1[&1].clone()
        };
        assert!(indexed(&f.english).contains(&"diamond sword".to_owned()));
        let french = indexed(&f.french);
        assert!(french.contains(&"épée en diamant".to_owned()), "index still in English: {french:?}");
    }

    /// A tab whose category holds a newly unlocked recipe bounces (search tab never); filtered
    /// book: only for a craftable one.
    #[test]
    fn tab_with_new_recipe_bounces() {
        let Some(data) = rc_testdata::version_data() else { return };
        let names = data.registry("minecraft:item").expect("item registry").entries.clone();
        let cats = data.registry("minecraft:recipe_book_category").expect("categories").entries.clone();
        let cat = |n: &str| cats.iter().position(|c| c == &format!("minecraft:{n}")).expect(n) as i32;
        let rec = Received::default();
        let ctx = Ctx { data: &data, rec: &rec, item_names: &names };
        let entry = |id: i32, category: i32| RecipeEntry {
            id,
            display: RecipeDisplay::Shapeless {
                ingredients: vec![SlotDisplay::Item(1)],
                result: SlotDisplay::Item(1),
                station: SlotDisplay::Empty,
            },
            group: None,
            category,
            requirements: None,
        };
        let mut b = RecipeBook::new();
        b.on_event(
            RecipeEvent::Add(vec![(entry(1, cat("crafting_building_blocks")), 2), (entry(2, cat("crafting_misc")), 0)], true),
            &ctx,
        );
        b.settings[0] = (true, false);
        b.on_screen(Kind::Crafting);
        let shown = [(0, 0, 0), (0, 0, 2), (0, 0, 3)];
        b.start_tab_bounce(&shown, Kind::Crafting, &ctx, 100.0);
        assert_eq!(b.tab_anim[..4], [0.0, 0.0, 115.0, 0.0], "building blocks only");
        // Checked once: a later drawing does not restart it.
        b.start_tab_bounce(&shown, Kind::Crafting, &ctx, 105.0);
        assert_eq!(b.tab_anim[2], 115.0);
        // Filtered book: the highlighted recipe is not craftable, no bounce.
        b.settings[0].1 = true;
        b.on_screen(Kind::Crafting);
        b.start_tab_bounce(&shown, Kind::Crafting, &ctx, 200.0);
        assert_eq!(b.tab_anim[2], 0.0);
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
