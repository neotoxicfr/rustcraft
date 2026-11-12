//! Creative inventory screen (`docs/specs/creative-inventory.md` § 1–6): tabs, grid,
//! scrollbar, search, inventory tab with trash, tooltips.

use rc_data::CreativeKind;
use rc_proto_v777::item::ItemStack;
use rc_text::{Component, StyledChar};

use crate::creative::{CreativeLists, Params, Received, SearchIndex};
use crate::gui::{Blend, Gui, Pre};
use crate::hud::HotbarItems;
use crate::inventory::PlayerInventory;
use crate::kept_text::Kept;
use crate::menu::{self, Kind};
use crate::screens::{Cached, SearchIndexKey, decorate, draw_tooltip, refresh};
use crate::tooltips::shared_cache::{self, ExtendedTooltip};
use crate::widgets::TextField;

#[cfg(test)]
mod equivalence_tests;

/// Tab `container/creative_inventory/tab_<top|bottom>_<unselected|selected>_<column + 1>`
/// (26 × 32): sprite resolved once for the 7 columns, looked up by name beyond.
fn tab_sprite(gui: &mut Gui, top: bool, selected: bool, column: usize, x: f32, y: f32) {
    if column < 7 {
        return gui.sprite_pre(Pre::creative_tab(top, selected, column), x, y, 26.0, 32.0, 0xffff_ffff);
    }
    let (row, sel) = (if top { "top" } else { "bottom" }, if selected { "selected" } else { "unselected" });
    gui.sprite(&format!("container/creative_inventory/tab_{row}_{sel}_{}", column + 1), x, y, 26.0, 32.0, 0xffff_ffff);
}

/// Size of the image (§ 6.1).
pub const W: i32 = 195;
pub const H: i32 = 136;

/// Hovered or targeted slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// Slot of the creative grid (0..45).
    Grid(usize),
    /// Slot of the player's menu 0.
    Menu(usize),
    /// Trash of the inventory tab.
    Trash,
}

/// Switches of the creative screen, read once from `RUSTCRAFT_OPT_*`; the proofs set them per
/// screen to compare both paths.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CreativeOpts {
    /// `RUSTCRAFT_OPT_CREATIVE_DRAW_NO_CLONE`: the drawing reads the stacks in place (no list of
    /// slots or of stack copies per frame), keeps its tab titles and trash tooltip per language
    /// and fonts, and decodes the hovered bundle's contents only when the stack changes.
    pub draw_without_copies: bool,
    /// `RUSTCRAFT_OPT_CREATIVE_HASHED_DEDUP`: repeats of the lists found through a hash table.
    pub hashed_dedup: bool,
    /// `RUSTCRAFT_OPT_CREATIVE_GRID_BY_INDEX`: the grid of a category or search tab holds places in
    /// the lists, not copies of their stacks (no copy at each keystroke or tab change).
    pub grid_by_index: bool,
    /// `RUSTCRAFT_OPT_CREATIVE_TAGS_MATCHED_ONCE`: a `#` search matches each tag once.
    pub tags_matched_once: bool,
    /// `RUSTCRAFT_OPT_INDEXED_NAME_LOOKUPS`: tab icons found through the item registry's index.
    pub indexed_names: bool,
}

impl CreativeOpts {
    /// Every switch off: the old paths.
    #[cfg(test)]
    pub(crate) const OLD: CreativeOpts = CreativeOpts {
        draw_without_copies: false,
        hashed_dedup: false,
        grid_by_index: false,
        tags_matched_once: false,
        indexed_names: false,
    };

    /// Every switch on.
    #[cfg(test)]
    pub(crate) const NEW: CreativeOpts = CreativeOpts {
        draw_without_copies: true,
        hashed_dedup: true,
        grid_by_index: true,
        tags_matched_once: true,
        indexed_names: true,
    };

    fn from_switches() -> CreativeOpts {
        CreativeOpts {
            draw_without_copies: rc_data::opt_switch!("RUSTCRAFT_OPT_CREATIVE_DRAW_NO_CLONE"),
            hashed_dedup: rc_data::opt_switch!("RUSTCRAFT_OPT_CREATIVE_HASHED_DEDUP"),
            grid_by_index: rc_data::opt_switch!("RUSTCRAFT_OPT_CREATIVE_GRID_BY_INDEX"),
            tags_matched_once: rc_data::opt_switch!("RUSTCRAFT_OPT_CREATIVE_TAGS_MATCHED_ONCE"),
            indexed_names: crate::item_ids::indexed_name_lookups(),
        }
    }
}

/// What the grid shows, place by place (9 places per row).
#[derive(Debug, PartialEq)]
enum Grid {
    /// Copies of the stacks, empty slots included: the saved hotbars, and every tab when
    /// [`CreativeOpts::grid_by_index`] is off.
    Copies(Vec<Option<ItemStack>>),
    /// The list of category tab `k`, read in place.
    TabList(usize),
    /// Places in the search list of the stacks the query kept.
    SearchHits(Vec<usize>),
    /// The whole search list (empty query).
    WholeSearch,
}

impl Grid {
    fn len(&self, lists: &CreativeLists) -> usize {
        match self {
            Grid::Copies(stacks) => stacks.len(),
            Grid::TabList(k) => lists.tab.get(*k).map_or(0, Vec::len),
            Grid::SearchHits(places) => places.len(),
            Grid::WholeSearch => lists.search.len(),
        }
    }

    fn get<'a>(&'a self, lists: &'a CreativeLists, place: usize) -> Option<&'a ItemStack> {
        match self {
            Grid::Copies(stacks) => stacks.get(place)?.as_ref(),
            Grid::TabList(k) => lists.tab.get(*k)?.get(place),
            Grid::SearchHits(places) => lists.search.get(*places.get(place)?),
            Grid::WholeSearch => lists.search.get(place),
        }
    }
}

/// What the lines this screen adds to an item tooltip depend on, besides the tooltip itself.
#[derive(Clone, Copy, PartialEq, Eq)]
struct OwnTooltipInputs {
    /// Selected tab: its kind decides which lines are added.
    tab: usize,
    /// The hovered slot is in the grid: a category grid adds nothing, the hotbar row under it does.
    in_grid: bool,
    /// `CreativeScreen::tooltip_lists_revision` when the lines were added.
    lists_and_tags: u64,
}

/// A text line of the screen and its width.
struct MeasuredLine {
    chars: Vec<StyledChar>,
    width: i32,
}

impl MeasuredLine {
    fn translated(gui: &Gui, key: &str) -> MeasuredLine {
        let chars = rc_text::styled_chars(&rc_text::flatten(&Component::translate(key, vec![]), &gui.lang));
        MeasuredLine { width: rc_text::width(gui, &chars), chars }
    }
}

/// Texts of the screen that change only with the language or the fonts.
struct ScreenTexts {
    /// Title of each tab (drawn on the image and as the tab's tooltip).
    tab_titles: Vec<MeasuredLine>,
    /// Tooltip of the trash.
    trash: MeasuredLine,
}

impl ScreenTexts {
    fn build(gui: &Gui, tabs: &[rc_data::CreativeTab]) -> ScreenTexts {
        ScreenTexts {
            tab_titles: tabs.iter().map(|t| MeasuredLine::translated(gui, &t.title)).collect(),
            trash: MeasuredLine::translated(gui, "inventory.binSlot"),
        }
    }
}

pub struct CreativeScreen {
    pub open: bool,
    /// Selected tab, remembered for the session (§ 1).
    pub tab: usize,
    /// Scroll `s ∈ [0, 1]` (§ 6.3).
    pub scroll: f32,
    pub bar_drag: bool,
    pub lists: CreativeLists,
    /// Stacks of the current tab's grid.
    grid: Grid,
    pub search: TextField,
    index: Option<(SearchIndexKey, SearchIndex)>,
    /// Visible tags of the last `#` query (§ 4.2).
    pub visible_tags: Vec<String>,
    /// Item tags received (name, items), copied with the index.
    index_tags: Vec<(String, Vec<i32>)>,
    /// Counter of list rebuilds (search index to redo).
    rebuilt: u64,
    /// Left press consumed by a tab (selection on release, § 6.2).
    pub tab_pressed: bool,
    cache: Vec<Option<Cached>>,
    icons: Vec<Option<Cached>>,
    carried_cache: Option<Cached>,
    /// Tooltip of the hovered stack with the lines this screen adds (tab names, visible tags).
    tooltip: Option<ExtendedTooltip<OwnTooltipInputs>>,
    /// Counts the changes of the lists and tags those added lines are made from.
    tooltip_lists_revision: u64,
    /// Tab titles and trash tooltip ([`CreativeOpts::draw_without_copies`]).
    texts: Kept<ScreenTexts>,
    /// Last hovered stack and its container contents, decoded once per stack
    /// ([`CreativeOpts::draw_without_copies`]).
    hovered_contents: Option<(ItemStack, Option<Vec<ItemStack>>)>,
    opts: CreativeOpts,
    /// Demos: ticks waited, opening done, hotbar ticks, click ticks.
    pub test: (u32, bool, u32, u32),
    /// Drag in progress: button, menu 0 slots retained.
    pub drag: Option<(u8, Vec<usize>)>,
    /// Local limiter of drops (§ 7.1).
    pub drops: i32,
    /// Entity of the local player, drawn in the inventory tab.
    pub player_model: Option<i32>,
}

impl Default for CreativeScreen {
    fn default() -> Self {
        CreativeScreen::with_opts(CreativeOpts::from_switches())
    }
}

/// Corner of the image (§ 6.1).
pub fn origin(gui: &Gui) -> (i32, i32) {
    ((gui.width as i32 - W) / 2, (gui.height as i32 - H) / 2)
}

/// Relative x of a tab (§ 6.2).
fn tab_x(t: &rc_data::CreativeTab) -> i32 {
    let c = i32::from(t.column);
    if t.right { W - 27 * (7 - c) + 1 } else { 27 * c }
}

/// Position of a menu 0 slot in the inventory tab (§ 5), `None` if off screen.
pub fn inventory_slot_pos(i: usize) -> Option<(i32, i32)> {
    let i32_ = |v: usize| v as i32;
    Some(match i {
        5..=8 => {
            let p = i32_(i - 5);
            (54 + 54 * (p / 2), 6 + 27 * (p % 2))
        }
        45 => (35, 20),
        9..=35 => (9 + 18 * (i32_(i - 9) % 9), 54 + 18 * (i32_(i - 9) / 9)),
        36..=44 => (9 + 18 * i32_(i - 36), 112),
        _ => return None,
    })
}

/// Key of a slot for the bundles' hovered-slot rule (`bundle.rs`): menu slots keep their index,
/// grid slots come after every menu, the trash has none (`usize::MAX`). The chosen items go by
/// stack: [`CreativeScreen::stack_key`].
pub fn hit_key(h: Hit) -> usize {
    match h {
        Hit::Grid(i) => crate::bundle::CREATIVE_GRID + i,
        Hit::Menu(i) => i,
        Hit::Trash => usize::MAX,
    }
}

/// [`CreativeScreen::stack_key`] with the grid's first row given.
fn stack_key_at(h: Hit, first_row: usize) -> usize {
    match h {
        Hit::Grid(i) => crate::bundle::CREATIVE_GRID + i + 9 * first_row,
        _ => hit_key(h),
    }
}

/// Relative position of a slot (§ 5, § 6.1).
pub fn hit_pos(h: Hit) -> Option<(i32, i32)> {
    match h {
        Hit::Grid(i) => Some((9 + 18 * (i as i32 % 9), 18 + 18 * (i as i32 / 9))),
        Hit::Menu(i) => inventory_slot_pos(i),
        Hit::Trash => Some((173, 112)),
    }
}

/// Displayed slots of a tab, in drawing order: the menu and the trash on the inventory tab, the
/// grid and the hotbar row elsewhere.
fn displayed_slots(inventory_tab: bool) -> impl Iterator<Item = Hit> {
    let tab = (5..46).map(Hit::Menu).chain(std::iter::once(Hit::Trash)).filter(move |_| inventory_tab);
    let grid = (0..45).map(Hit::Grid).chain((36..45).map(Hit::Menu)).filter(move |_| !inventory_tab);
    tab.chain(grid)
}

/// Contents of the container `st` (bundle), decoded again only when the hovered stack changes.
fn kept_contents<'a>(
    kept: &'a mut Option<(ItemStack, Option<Vec<ItemStack>>)>,
    st: &ItemStack,
    ctx: &crate::bundle::Ctx<'_>,
) -> Option<&'a [ItemStack]> {
    if kept.as_ref().is_none_or(|(hovered, _)| hovered != st) {
        *kept = Some((st.clone(), ctx.contents(st)));
    }
    kept.as_ref().and_then(|(_, contents)| contents.as_deref())
}

impl CreativeScreen {
    pub(crate) fn with_opts(opts: CreativeOpts) -> CreativeScreen {
        let mut search = TextField::new(Vec::new());
        search.max_len = 50;
        search.bordered = false;
        search.text_color = 0xffff_ffff;
        search.invert_highlight = false;
        CreativeScreen {
            open: false,
            tab: 0,
            scroll: 0.0,
            bar_drag: false,
            lists: CreativeLists::default(),
            grid: Grid::Copies(Vec::new()),
            search,
            index: None,
            visible_tags: Vec::new(),
            index_tags: Vec::new(),
            rebuilt: 0,
            tab_pressed: false,
            cache: Vec::new(),
            icons: Vec::new(),
            carried_cache: None,
            tooltip: None,
            tooltip_lists_revision: 0,
            texts: Kept::default(),
            hovered_contents: None,
            opts,
            test: (0, false, 0, 0),
            drag: None,
            drops: 0,
            player_model: None,
        }
    }

    pub fn kind(&self, data: &rc_data::VersionData) -> CreativeKind {
        data.creative.tabs.get(self.tab).map_or(CreativeKind::Category, |t| t.kind)
    }

    /// Displayed tab (§ 2.3).
    pub fn visible(&self, data: &rc_data::VersionData, k: usize) -> bool {
        data.creative
            .tabs
            .get(k)
            .is_some_and(|t| t.kind != CreativeKind::Category || self.lists.tab.get(k).is_some_and(|l| !l.is_empty()))
    }

    fn default_tab(data: &rc_data::VersionData) -> usize {
        data.creative.tabs.iter().position(|t| t.name == "minecraft:building_blocks").unwrap_or(0)
    }

    /// Displayed slots of the current tab.
    pub fn hits(&self, data: &rc_data::VersionData) -> Vec<Hit> {
        self.hit_iter(data).collect()
    }

    /// Displayed slots of the current tab, without building a list.
    fn hit_iter(&self, data: &rc_data::VersionData) -> impl Iterator<Item = Hit> + use<> {
        displayed_slots(self.kind(data) == CreativeKind::Inventory)
    }

    /// Hovered slot, if the cursor is over a slot.
    pub fn hovered(&self, gui: &Gui, data: &rc_data::VersionData, mouse: (f32, f32)) -> Option<Hit> {
        let (gx, gy) = origin(gui);
        self.hit_at(data, (mouse.0.floor() as i32 - gx, mouse.1.floor() as i32 - gy))
    }

    /// Slot under a point relative to the image's corner (checked every frame: no list built).
    fn hit_at(&self, data: &rc_data::VersionData, (mx, my): (i32, i32)) -> Option<Hit> {
        self.hit_iter(data).find(|&h| {
            let Some((x, y)) = hit_pos(h) else { return false };
            mx >= x - 1 && mx < x + 17 && my >= y - 1 && my < y + 17
        })
    }

    /// Tab under the cursor for a click (§ 6.2).
    pub fn tab_at(&self, gui: &Gui, data: &rc_data::VersionData, mouse: (f32, f32)) -> Option<usize> {
        let (gx, gy) = origin(gui);
        let (mx, my) = (mouse.0 - gx as f32, mouse.1 - gy as f32);
        data.creative.tabs.iter().enumerate().find_map(|(k, t)| {
            let x = tab_x(t) as f32;
            let y = if t.row == 0 { -32.0 } else { H as f32 };
            (self.visible(data, k) && x <= mx && mx <= x + 26.0 && y <= my && my <= y + 32.0).then_some(k)
        })
    }

    /// Tab hovered for the tooltip (§ 6.2).
    fn tab_hover(&self, gui: &Gui, data: &rc_data::VersionData, mouse: (f32, f32)) -> Option<usize> {
        let (gx, gy) = origin(gui);
        let (mx, my) = (mouse.0 - gx as f32, mouse.1 - gy as f32);
        data.creative.tabs.iter().enumerate().find_map(|(k, t)| {
            let x = tab_x(t) as f32 + 3.0;
            let y = if t.row == 0 { -32.0 } else { H as f32 } + 3.0;
            (self.visible(data, k) && x <= mx && mx < x + 21.0 && y <= my && my < y + 27.0).then_some(k)
        })
    }

    /// Outside the image and not on the current tab (§ 6.2).
    pub fn outside(&self, gui: &Gui, data: &rc_data::VersionData, mouse: (f32, f32)) -> bool {
        let (gx, gy) = origin(gui);
        let (mx, my) = (mouse.0.floor() as i32 - gx, mouse.1.floor() as i32 - gy);
        let out = mx < 0 || my < 0 || mx >= W || my >= H;
        out && self.tab_at(gui, data, mouse) != Some(self.tab)
    }

    /// Number of places of the grid's list.
    fn grid_len(&self) -> usize {
        self.grid.len(&self.lists)
    }

    fn rows_over(&self) -> i32 {
        (self.grid_len() as i32 + 8) / 9 - 5
    }

    pub fn can_scroll(&self, data: &rc_data::VersionData) -> bool {
        data.creative.tabs.get(self.tab).is_some_and(|t| t.scroll) && self.grid_len() > 45
    }

    fn first_row(&self) -> i32 {
        ((self.scroll * self.rows_over() as f32 + 0.5).floor() as i32).max(0)
    }

    /// Stack displayed in grid slot `i`.
    pub fn grid_stack(&self, i: usize) -> Option<&ItemStack> {
        let k = self.first_row() as usize;
        self.grid.get(&self.lists, i + 9 * k)
    }

    /// The grid shows copies of `stacks` (the saved hotbars), unless it already shows them.
    pub fn show_stacks(&mut self, stacks: &[Option<ItemStack>]) {
        if !matches!(&self.grid, Grid::Copies(shown) if shown == stacks) {
            self.grid = Grid::Copies(stacks.to_vec());
        }
    }

    /// Key of the stack a slot shows in the bundles' chosen-item table: a menu slot keeps its
    /// index; a grid slot gives its stack's place in the grid's list (`CREATIVE_GRID` + place), as
    /// the game keeps the choice on the stack, which the grid may scroll away from under the mouse.
    pub fn stack_key(&self, h: Hit) -> usize {
        stack_key_at(h, self.first_row() as usize)
    }

    /// Stack of a displayed slot.
    pub fn stack_at(&self, inv: &PlayerInventory, h: Hit) -> Option<ItemStack> {
        self.slot_stack(inv, h).cloned()
    }

    /// Stack of a displayed slot, read in place.
    fn slot_stack<'a>(&'a self, inv: &'a PlayerInventory, h: Hit) -> Option<&'a ItemStack> {
        match h {
            Hit::Grid(i) => self.grid_stack(i),
            Hit::Menu(i) => inv.menu.get(i)?.as_ref(),
            Hit::Trash => None,
        }
    }

    /// Wheel (scaled offset): the offset divided by the rows over the grid in double, then cut to
    /// a float, like the game.
    pub fn wheel(&mut self, data: &rc_data::VersionData, delta: f64) {
        if self.can_scroll(data) {
            self.scroll = (self.scroll - (delta / f64::from(self.rows_over())) as f32).clamp(0.0, 1.0);
        }
    }

    /// Scrollbar: press (true if the drag starts), movement. The bar only follows the mouse once
    /// it moves, not at the press.
    pub fn bar_press(&mut self, gui: &Gui, data: &rc_data::VersionData, mouse: (f32, f32)) -> bool {
        let (gx, gy) = origin(gui);
        let (mx, my) = (mouse.0 - gx as f32, mouse.1 - gy as f32);
        let has_bar = data.creative.tabs.get(self.tab).is_some_and(|t| t.scroll);
        let inside = (175.0..189.0).contains(&mx) && (18.0..130.0).contains(&my);
        if has_bar && inside && self.can_scroll(data) {
            self.bar_drag = true;
            return true;
        }
        false
    }

    pub fn bar_motion(&mut self, gui: &Gui, mouse: (f32, f32)) {
        if self.bar_drag {
            let gy = origin(gui).1 as f32;
            self.scroll = ((mouse.1 - (gy + 18.0) - 7.5) / (112.0 - 15.0)).clamp(0.0, 1.0);
        }
    }

    /// Grid contents for the current tab; `keep_row` keeps the starting row. The grid's stacks are
    /// new (saved hotbars read again), without chosen bundle items.
    pub fn reload(&mut self, data: &rc_data::VersionData, hotbars: &[Option<ItemStack>], keep_row: bool) {
        crate::bundle::forget_grid();
        let row = self.first_row();
        self.grid = match self.kind(data) {
            CreativeKind::Category if self.opts.grid_by_index => Grid::TabList(self.tab),
            CreativeKind::Category => Grid::Copies(
                self.lists.tab.get(self.tab).map(|l| l.iter().cloned().map(Some).collect()).unwrap_or_default(),
            ),
            CreativeKind::Search => self.filtered(),
            CreativeKind::Hotbar => Grid::Copies(hotbars.to_vec()),
            CreativeKind::Inventory => Grid::Copies(Vec::new()),
        };
        self.scroll =
            if keep_row && self.rows_over() > 0 { (row as f32 / self.rows_over() as f32).clamp(0.0, 1.0) } else { 0.0 };
    }

    fn filtered(&mut self) -> Grid {
        // The visible tags are replaced below: the tag lines of a kept tooltip may be stale.
        self.tooltip_lists_revision += 1;
        let q = self.search.value();
        let by_index = self.opts.grid_by_index;
        let search = &self.lists.search;
        match &self.index {
            Some((_, idx)) if !q.is_empty() => {
                let (hits, tags) = idx.query(&q, &self.index_tags, self.opts.tags_matched_once);
                self.visible_tags = tags;
                if by_index {
                    Grid::SearchHits(hits.into_iter().filter(|&i| i < search.len()).collect())
                } else {
                    Grid::Copies(hits.into_iter().filter_map(|i| search.get(i).cloned().map(Some)).collect())
                }
            }
            _ => {
                self.visible_tags.clear();
                if by_index { Grid::WholeSearch } else { Grid::Copies(search.iter().cloned().map(Some).collect()) }
            }
        }
    }

    /// Choice of a tab (§ 6.2, § 4.3).
    pub fn select(&mut self, data: &rc_data::VersionData, k: usize, hotbars: &[Option<ItemStack>]) {
        let was = self.tab;
        self.tab = k;
        if self.kind(data) == CreativeKind::Search && was != k {
            self.search.text.clear();
            self.search.cursor = 0;
            self.search.anchor = 0;
        }
        self.bar_drag = false;
        self.reload(data, hotbars, false);
    }

    /// Rebuilding the lists if the parameters changed (§ 3.1); on opening and on
    /// every tick of the open screen.
    pub fn update_lists(
        &mut self,
        data: &rc_data::VersionData,
        rec: &Received,
        params: Params,
        hotbars: &[Option<ItemStack>],
    ) {
        if !self.lists.update(data, rec, params, self.opts.hashed_dedup) {
            return;
        }
        self.rebuilt += 1;
        self.tooltip_lists_revision += 1;
        self.index = None;
        if self.kind(data) == CreativeKind::Category && !self.visible(data, self.tab) {
            self.select(data, Self::default_tab(data), hotbars);
        } else {
            self.reload(data, hotbars, true);
        }
    }

    /// Search index (§ 4.1), redone after a rebuild or a language change.
    pub fn ensure_index(
        &mut self,
        lang: &rc_text::Lang,
        it: &HotbarItems<'_>,
        enchantments: &[String],
        rec: &Received,
    ) {
        let key =
            SearchIndexKey { language: lang.generation, listed_content: self.rebuilt, server_data: rec.generation };
        if self.index.as_ref().is_some_and(|(k, _)| *k == key) {
            return;
        }
        let by_item = crate::creative::tags_by_item(rec);
        let names = it.names;
        let mut idx = SearchIndex::default();
        crate::tooltips_more::set_creative(true);
        for s in &self.lists.search {
            let lines = crate::screens::tooltip_search_lines(s, it, enchantments, lang);
            idx.lines.push(lines);
            idx.ids.push(usize::try_from(s.item).ok().and_then(|i| names.get(i)).cloned().unwrap_or_default());
            idx.tags.push(by_item.get(&s.item).cloned().unwrap_or_default());
        }
        crate::tooltips_more::set_creative(false);
        self.index_tags = rec.item_tags().to_vec();
        self.tooltip_lists_revision += 1;
        self.index = Some((key, idx));
    }

    /// Search text modified: filter and back to the top (§ 4.2), other stacks without choices.
    pub fn search_changed(&mut self, data: &rc_data::VersionData) {
        if self.kind(data) == CreativeKind::Search {
            crate::bundle::forget_grid();
            self.grid = self.filtered();
            self.scroll = 0.0;
        }
    }

    /// Drawing (§ 6).
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        gui: &mut Gui,
        queue: &wgpu::Queue,
        inv: &PlayerInventory,
        it: &HotbarItems<'_>,
        mouse: Option<(f32, f32)>,
        enchantments: &[String],
        time: f32,
    ) {
        if !self.open {
            return;
        }
        let data = it.data;
        let tabs = &data.creative.tabs;
        let Some(cur) = tabs.get(self.tab) else { return };
        let (gx, gy) = origin(gui);
        let f = |v: i32| v as f32;
        let hover = mouse.and_then(|m| self.hovered(gui, data, m));
        let kind = cur.kind;
        gui.next_layer();
        crate::widgets::ingame_ui_background(gui);
        // Unselected tabs, then background of the current tab.
        let tab_rect = |t: &rc_data::CreativeTab| {
            let x = gx + tab_x(t);
            let y = if t.row == 0 { gy - 28 } else { gy + H - 4 };
            (x, y)
        };
        for (k, t) in tabs.iter().enumerate() {
            if k == self.tab || !self.visible(data, k) {
                continue;
            }
            let (x, y) = tab_rect(t);
            tab_sprite(gui, t.row == 0, false, usize::from(t.column), f(x), f(y));
        }
        let bg = cur.background.strip_prefix("minecraft:textures/gui/container/").unwrap_or(&cur.background);
        let bg = format!("container_bg/{}", bg.strip_suffix(".png").unwrap_or(bg));
        gui.sprite_region(&bg, 256.0, 256.0, 0.0, 0.0, f(gx), f(gy), f(W), f(H));
        // Inventory tab: local player following the mouse (size 20, offset 1/16).
        if kind == CreativeKind::Inventory
            && let Some(id) = self.player_model
        {
            let rect = [f(gx + 73), f(gy + 6), f(gx + 105), f(gy + 49)];
            let m = mouse.unwrap_or(((rect[0] + rect[2]) / 2.0, (rect[1] + rect[3]) / 2.0));
            let req = crate::special::ModelReq::Entity { id, rect, size: 20.0, offset: 0.0625, mouse: m };
            crate::special::request_model(gui, req);
        }
        if kind == CreativeKind::Search {
            let blink = (time * 1000.0) as i64 / 300 % 2 == 0;
            crate::widgets::draw_field_blink(gui, queue, &self.search, [gx + 82, gy + 6, 80, 9], true, blink);
        }
        if cur.scroll {
            let name = if self.can_scroll(data) { Pre::CREATIVE_SCROLLER } else { Pre::CREATIVE_SCROLLER_DISABLED };
            let y = gy + 18 + ((112 - 17) as f32 * self.scroll).floor() as i32;
            gui.sprite_pre(name, f(gx + 175), f(y), 12.0, 15.0, 0xffff_ffff);
        }
        {
            let (x, y) = tab_rect(cur);
            tab_sprite(gui, cur.row == 0, true, usize::from(cur.column), f(x), f(y));
        }
        // Title (§ 6.1).
        if cur.show_title && kind != CreativeKind::Inventory {
            self.draw_title(gui, queue, tabs, (f(gx + 8), f(gy + 6)));
        }
        // Empty slots (inventory tab), back highlight.
        let inventory_tab = kind == CreativeKind::Inventory;
        if inventory_tab {
            for h in displayed_slots(true) {
                if let Hit::Menu(i) = h
                    && inv.menu.get(i).is_some_and(Option::is_none)
                    && let Some(sp) = menu::slot_def(Kind::Player, i, 46).empty_sprite
                    && let Some((x, y)) = hit_pos(h)
                {
                    gui.sprite(sp, f(gx + x), f(gy + y), 16.0, 16.0, 0xffff_ffff);
                }
            }
        }
        if let Some(h) = hover
            && let Some((x, y)) = hit_pos(h)
        {
            gui.sprite("container/slot_highlight_back", f(gx + x - 4), f(gy + y - 4), 24.0, 24.0, 0xffff_ffff);
        }
        // Items (up-to-date layers first), tab icons.
        if self.opts.draw_without_copies {
            self.refresh_renders_in_place(gui, inv, it);
        } else {
            self.refresh_renders_from_copies(gui, inv, it);
        }
        gui.stratum(Blend::Premultiplied);
        for (i, h) in displayed_slots(inventory_tab).enumerate() {
            if let (Some(c), Some((x, y))) = (&self.cache[i], hit_pos(h))
                && !c.layers.is_empty()
            {
                gui.item(c.key, &c.layers, f(gx + x), f(gy + y), 0.0);
            }
        }
        for (k, t) in tabs.iter().enumerate() {
            if !self.visible(data, k) {
                continue;
            }
            if let Some(c) = &self.icons[k]
                && !c.layers.is_empty()
            {
                let (x, y) = tab_rect(t);
                let dy = if t.row == 0 { 9 } else { 7 };
                gui.item(c.key, &c.layers, f(x + 5), f(y + dy), 0.0);
            }
        }
        gui.stratum(Blend::Normal);
        for (i, h) in displayed_slots(inventory_tab).enumerate() {
            if let (Some(c), Some((x, y))) = (&self.cache[i], hit_pos(h)) {
                decorate(gui, queue, c, f(gx + x), f(gy + y), (c.stack.count, false));
            }
        }
        gui.next_layer();
        if let Some(h) = hover
            && let Some((x, y)) = hit_pos(h)
        {
            gui.sprite("container/slot_highlight_front", f(gx + x - 4), f(gy + y - 4), 24.0, 24.0, 0xffff_ffff);
        }
        // Carried item.
        if let (Some((mx, my)), Some(c)) = (mouse, &self.carried_cache) {
            let (x, y) = (mx.floor() - 8.0, my.floor() - 8.0);
            gui.stratum(Blend::Premultiplied);
            if !c.layers.is_empty() {
                gui.item(c.key, &c.layers, x, y, 0.0);
            }
            gui.stratum(Blend::Normal);
            decorate(gui, queue, c, x, y, (c.stack.count, false));
        }
        let Some((mx, my)) = mouse else { return };
        // Tooltips: hovered tab, trash (both even with an item carried), item (§ 5, § 6.4).
        let m = (mx.floor() as i32, my.floor() as i32);
        if let Some(k) = self.tab_hover(gui, data, (mx, my)) {
            return self.draw_text_tooltip(gui, queue, tabs, Some(k), m);
        }
        if hover == Some(Hit::Trash) {
            return self.draw_text_tooltip(gui, queue, tabs, None, m);
        }
        if let Some(h) = hover {
            self.draw_item_tooltip(gui, queue, inv, it, enchantments, h, m);
        }
    }

    /// Tab titles and trash tooltip, built again after a change of language, fonts or tabs.
    fn screen_texts(&mut self, gui: &Gui, tabs: &[rc_data::CreativeTab]) -> &ScreenTexts {
        if self.texts.get(gui, true, |gui| ScreenTexts::build(gui, tabs)).tab_titles.len() != tabs.len() {
            self.texts.forget();
        }
        self.texts.get(gui, true, |gui| ScreenTexts::build(gui, tabs))
    }

    /// Title of the current tab.
    fn draw_title(&mut self, gui: &mut Gui, queue: &wgpu::Queue, tabs: &[rc_data::CreativeTab], (x, y): (f32, f32)) {
        const COLOR: u32 = 0xff40_4040;
        if !self.opts.draw_without_copies {
            let key = tabs[self.tab].title.clone();
            let title = rc_text::styled_chars(&rc_text::flatten(&Component::translate(key, vec![]), &gui.lang));
            gui.text(queue, &title, x, y, COLOR, false, 0);
            return;
        }
        let tab = self.tab;
        let title = &self.screen_texts(gui, tabs).tab_titles[tab];
        gui.text(queue, &title.chars, x, y, COLOR, false, 0);
    }

    /// Tooltip of tab `tab`, or of the trash (`None`).
    fn draw_text_tooltip(
        &mut self,
        gui: &mut Gui,
        queue: &wgpu::Queue,
        tabs: &[rc_data::CreativeTab],
        tab: Option<usize>,
        (mx, my): (i32, i32),
    ) {
        if !self.opts.draw_without_copies {
            let key = tab.map_or_else(|| "inventory.binSlot".to_owned(), |k| tabs[k].title.clone());
            let t = rc_text::styled_chars(&rc_text::flatten(&Component::translate(key, vec![]), &gui.lang));
            let w = rc_text::width(gui, &t);
            return draw_tooltip(gui, queue, &[t], w, mx, my);
        }
        let texts = self.screen_texts(gui, tabs);
        let line = tab.map_or(&texts.trash, |k| &texts.tab_titles[k]);
        draw_tooltip(gui, queue, std::slice::from_ref(&line.chars), line.width, mx, my);
    }

    /// Item renders of the displayed slots, the tab icons and the carried stack, made again where
    /// they changed. Old path: copies of every displayed stack first, at every frame.
    fn refresh_renders_from_copies(&mut self, gui: &Gui, inv: &PlayerInventory, it: &HotbarItems<'_>) {
        let tabs = &it.data.creative.tabs;
        let hits = self.hits(it.data);
        let stacks: Vec<Option<ItemStack>> = hits.iter().map(|&h| self.stack_at(inv, h)).collect();
        let icon_stacks: Vec<Option<ItemStack>> =
            tabs.iter().map(|t| icon_stack(t, it, self.opts.indexed_names)).collect();
        self.cache.resize_with(hits.len(), || None);
        self.icons.resize_with(tabs.len(), || None);
        let Some(models) = gui.items.as_ref().map(|a| &a.models) else { return };
        for (i, s) in stacks.iter().enumerate() {
            // Stack key: the item chosen in a bundle with the wheel shows in its model.
            let hold = crate::stack::Hold {
                menu_slot: u16::try_from(self.stack_key(hits[i])).ok(),
                ..crate::stack::Hold::NONE
            };
            match s {
                Some(st) => crate::screens::refresh_held(&mut self.cache[i], st, it, models, i as i32, hold),
                None => self.cache[i] = None,
            }
        }
        for (i, s) in icon_stacks.iter().enumerate() {
            match s {
                Some(st) => refresh(&mut self.icons[i], st, it, models, 1000 + i as i32),
                None => self.icons[i] = None,
            }
        }
        self.refresh_carried_render(inv, it, models);
    }

    /// [`Self::refresh_renders_from_copies`] reading every stack in place.
    fn refresh_renders_in_place(&mut self, gui: &Gui, inv: &PlayerInventory, it: &HotbarItems<'_>) {
        let tabs = &it.data.creative.tabs;
        let inventory_tab = self.kind(it.data) == CreativeKind::Inventory;
        let first_row = self.first_row() as usize;
        self.cache.resize_with(if inventory_tab { 42 } else { 54 }, || None);
        self.icons.resize_with(tabs.len(), || None);
        let Some(models) = gui.items.as_ref().map(|a| &a.models) else { return };
        let (grid, lists, cache) = (&self.grid, &self.lists, &mut self.cache);
        for (i, h) in displayed_slots(inventory_tab).enumerate() {
            let stack = match h {
                Hit::Grid(g) => grid.get(lists, g + 9 * first_row),
                Hit::Menu(m) => inv.menu.get(m).and_then(Option::as_ref),
                Hit::Trash => None,
            };
            // Stack key: the item chosen in a bundle with the wheel shows in its model.
            let hold = crate::stack::Hold {
                menu_slot: u16::try_from(stack_key_at(h, first_row)).ok(),
                ..crate::stack::Hold::NONE
            };
            match stack {
                Some(st) => crate::screens::refresh_held(&mut cache[i], st, it, models, i as i32, hold),
                None => cache[i] = None,
            }
        }
        for (k, t) in tabs.iter().enumerate() {
            match icon_stack(t, it, self.opts.indexed_names) {
                Some(st) => refresh(&mut self.icons[k], &st, it, models, 1000 + k as i32),
                None => self.icons[k] = None,
            }
        }
        self.refresh_carried_render(inv, it, models);
    }

    fn refresh_carried_render(
        &mut self,
        inv: &PlayerInventory,
        it: &HotbarItems<'_>,
        models: &rc_assets::item::ItemModels,
    ) {
        match &inv.carried {
            Some(st) => {
                crate::screens::refresh_held(&mut self.carried_cache, st, it, models, 0, crate::stack::Hold::CARRIED)
            }
            None => self.carried_cache = None,
        }
    }

    /// Tooltip of the stack in slot `h`, with the lines this screen adds; with an item carried,
    /// only a container's tooltip (bundle) stays, like the container screens.
    #[allow(clippy::too_many_arguments)]
    fn draw_item_tooltip(
        &mut self,
        gui: &mut Gui,
        queue: &wgpu::Queue,
        inv: &PlayerInventory,
        it: &HotbarItems<'_>,
        enchantments: &[String],
        h: Hit,
        m: (i32, i32),
    ) {
        let ctx = crate::bundle::Ctx { ids: it.ids, data: it.data };
        // Taken out for the time of the drawing: the added lines read the rest of the screen.
        let mut kept = self.tooltip.take();
        let mut hovered_contents = self.hovered_contents.take();
        let copy = if self.opts.draw_without_copies { None } else { self.stack_at(inv, h) };
        let copied_contents = copy.as_ref().map(|st| ctx.contents(st));
        let Some(st) = copy.as_ref().or_else(|| self.slot_stack(inv, h)) else {
            (self.tooltip, self.hovered_contents) = (kept, hovered_contents);
            return;
        };
        let contents = match &copied_contents {
            Some(contents) => contents.as_deref(),
            None => kept_contents(&mut hovered_contents, st, &ctx),
        };
        if inv.carried.is_none() || contents.is_some() {
            let item_tooltip = shared_cache::item_tooltip(st, it, enchantments, gui, crate::screens::advanced(), true);
            let tooltip = ExtendedTooltip::refresh(&mut kept, item_tooltip, self.own_tooltip_inputs(h), gui, |lines| {
                self.add_tab_and_tag_lines(lines, st, h, it.data, &gui.lang);
            });
            let (lines, style) = (&tooltip.lines, tooltip.style.as_deref());
            match contents {
                // Container (bundle): grid of the items under the name, chosen item marked.
                Some(items) if !lines.is_empty() => {
                    let sel = crate::bundle::selected(self.stack_key(h));
                    crate::bundle::draw_tooltip(gui, queue, it, &ctx, lines, items, sel, m, style);
                }
                _ => crate::screens::draw_tooltip_ex(gui, queue, lines, tooltip.width, m, true, style),
            }
        }
        (self.tooltip, self.hovered_contents) = (kept, hovered_contents);
    }

    /// What [`Self::add_tab_and_tag_lines`] reads for the slot `h`, besides the tooltip and the
    /// language it is given.
    fn own_tooltip_inputs(&self, h: Hit) -> OwnTooltipInputs {
        OwnTooltipInputs {
            tab: self.tab,
            in_grid: matches!(h, Hit::Grid(_)),
            lists_and_tags: self.tooltip_lists_revision,
        }
    }

    /// Lines the creative screen adds to the tooltip of a stack (§ 6.4): visible tags in purple
    /// and tab names in blue, nothing in the grid of a category tab.
    fn add_tab_and_tag_lines(
        &self,
        lines: &mut Vec<Vec<StyledChar>>,
        st: &ItemStack,
        h: Hit,
        data: &rc_data::VersionData,
        lang: &rc_text::Lang,
    ) {
        let category_grid = self.kind(data) == CreativeKind::Category && matches!(h, Hit::Grid(_));
        if category_grid || lines.is_empty() {
            return;
        }
        let line = |c: Component| rc_text::styled_chars(&rc_text::flatten(&c, lang));
        if self.kind(data) == CreativeKind::Search && matches!(h, Hit::Grid(_)) {
            let mine: Vec<&String> =
                self.index_tags.iter().filter(|(_, ids)| ids.contains(&st.item)).map(|t| &t.0).collect();
            for t in &self.visible_tags {
                if mine.contains(&t) {
                    let mut c = Component::text(format!("#{t}"));
                    c.style.color = Some(0xaa_00aa);
                    lines.insert(1, line(c));
                }
            }
        }
        let mut at = 1;
        for (k, t) in data.creative.tabs.iter().enumerate() {
            if t.kind == CreativeKind::Search || !self.visible(data, k) {
                continue;
            }
            if self.lists.for_search.get(k).is_some_and(|l| l.iter().any(|x| crate::creative::same(x, st))) {
                let mut c = Component::translate(t.title.clone(), vec![]);
                c.style.color = Some(0x55_55ff);
                lines.insert(at, line(c));
                at += 1;
            }
        }
    }
}

/// Tab icon: simple stack of the extracted item; `indexed`: found through the item registry's
/// index, otherwise by a walk over the ~1,500 item names.
fn icon_stack(t: &rc_data::CreativeTab, it: &HotbarItems<'_>, indexed: bool) -> Option<ItemStack> {
    let id = crate::item_ids::item_id(it.data, it.names, &t.icon.item, indexed)?;
    Some(ItemStack { item: id, count: 1, added: Vec::new(), removed: Vec::new() })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Data with one tab of kind `kind` (selected tab 0).
    fn data(kind: CreativeKind) -> rc_data::VersionData {
        let mut d = rc_data::VersionData::default();
        d.creative.tabs.push(rc_data::CreativeTab { kind, ..Default::default() });
        d
    }

    /// Hit test of the former code: list of the displayed slots built at each call.
    fn listed(c: &CreativeScreen, data: &rc_data::VersionData, (mx, my): (i32, i32)) -> Option<Hit> {
        let hits: Vec<Hit> = if c.kind(data) == CreativeKind::Inventory {
            let mut v: Vec<Hit> = (5..46).map(Hit::Menu).collect();
            v.push(Hit::Trash);
            v
        } else {
            (0..45).map(Hit::Grid).chain((36..45).map(Hit::Menu)).collect()
        };
        hits.into_iter().find(|&h| {
            let Some((x, y)) = hit_pos(h) else { return false };
            mx >= x - 1 && mx < x + 17 && my >= y - 1 && my < y + 17
        })
    }

    /// Grid of 90 stacks scrolled one row down (slot `i` shows the stack at place `i + 9`).
    fn scrolled() -> CreativeScreen {
        let st = |item| Some(ItemStack { item, count: 1, added: Vec::new(), removed: Vec::new() });
        let mut c = CreativeScreen::default();
        c.show_stacks(&(0..90).map(st).collect::<Vec<_>>());
        c.scroll = 0.2;
        c
    }

    /// A grid slot's chosen-item key is its stack's place in the list (it follows a scroll); menu
    /// slots keep theirs.
    #[test]
    fn grid_stack_key_follows_the_list() {
        use crate::bundle::CREATIVE_GRID as G;
        let mut c = scrolled();
        assert_eq!(c.grid_stack(3).map(|s| s.item), Some(12));
        assert_eq!(c.stack_key(Hit::Grid(3)), G + 12);
        assert_eq!((c.stack_key(Hit::Menu(36)), c.stack_key(Hit::Trash)), (36, usize::MAX));
        c.scroll = 0.0;
        assert_eq!(c.stack_key(Hit::Grid(3)), G + 3);
    }

    /// The grid shows other stacks (tab chosen, lists rebuilt, search changed): the choices of
    /// its old stacks go with them; those of the menu slots stay.
    #[test]
    fn grid_rebuild_forgets_its_stacks_choices() {
        use crate::bundle::{CREATIVE_GRID as G, selected, set_selected};
        let mut c = scrolled();
        set_selected(G + 12, 1);
        set_selected(36, 2);
        c.reload(&data(CreativeKind::Category), &[], true);
        assert_eq!((selected(G + 12), selected(36)), (-1, 2));
        let mut c = scrolled();
        set_selected(G + 12, 1);
        c.search_changed(&data(CreativeKind::Search));
        assert_eq!((selected(G + 12), selected(36)), (-1, 2));
    }

    /// The hovered slot is checked every frame now (bundle stop-hover rule): same slot as the
    /// former list at every point of the image and around it, both kinds of tab, no list built.
    #[test]
    fn hit_test_without_a_list_finds_the_same_slot() {
        let c = CreativeScreen::default();
        for kind in [CreativeKind::Category, CreativeKind::Inventory] {
            let d = data(kind);
            let mut found = 0;
            for my in -4..H + 4 {
                for mx in -4..W + 4 {
                    let h = c.hit_at(&d, (mx, my));
                    assert_eq!(h, listed(&c, &d, (mx, my)), "{kind:?} at ({mx}, {my})");
                    found += usize::from(h.is_some());
                }
            }
            assert!(found > 0, "{kind:?}: no slot found");
            assert_eq!(c.hits(&d).len(), if kind == CreativeKind::Inventory { 42 } else { 54 });
        }
    }

    /// Index built in English, language switched to French (same number of keys), search tab
    /// reopened: "épée" finds the sword, like the game that rebuilds its search trees.
    #[test]
    fn search_index_follows_a_language_change() {
        use crate::screens::SwordInTwoLanguages;
        let f = SwordInTwoLanguages::new(data(CreativeKind::Search));
        let (it, rec) = (f.items(), Received::default());
        for opts in [CreativeOpts::OLD, CreativeOpts::NEW] {
            let mut c = CreativeScreen::with_opts(opts);
            c.lists.search = vec![SwordInTwoLanguages::sword()];
            let mut found = |query: &str, lang: &rc_text::Lang| {
                c.search.text = query.chars().collect();
                c.ensure_index(lang, &it, &[], &rec);
                c.search_changed(&f.data);
                c.grid_len()
            };
            assert_eq!(found("diamond", &f.english), 1);
            assert_eq!(found("épée", &f.english), 0);
            assert_eq!(found("épée", &f.french), 1, "index still in English after the language change");
            assert_eq!(found("diamond", &f.french), 0);
        }
    }

    /// Lines added to a tooltip (§ 6.4) for one stack: none in the grid of a category tab, the tab
    /// name from the hotbar row under it, the visible tags in the search grid. The former kept
    /// tooltip compared the stack and the tab only: each case that changes the lines must change
    /// the inputs the kept tooltip is compared by.
    #[test]
    fn added_tooltip_lines_and_their_inputs_follow_the_slot_the_tab_and_the_tags() {
        let mut data = rc_data::VersionData::default();
        let tab = |kind, title: &str| rc_data::CreativeTab { kind, title: title.into(), ..Default::default() };
        data.creative.tabs = vec![tab(CreativeKind::Category, "Building Blocks"), tab(CreativeKind::Search, "Search")];
        let lang = rc_text::Lang::default();
        let stone = ItemStack { item: 7, count: 1, added: Vec::new(), removed: Vec::new() };
        let mut c = CreativeScreen::default();
        c.lists.tab = vec![vec![stone.clone()], Vec::new()];
        c.lists.for_search = c.lists.tab.clone();
        c.index_tags = vec![("minecraft:stone_tool_materials".to_owned(), vec![7])];
        let lines_at = |c: &CreativeScreen, h: Hit| {
            let mut lines = vec![crate::gui::plain("Stone")];
            c.add_tab_and_tag_lines(&mut lines, &stone, h, &data, &lang);
            let text: Vec<String> = lines.iter().map(|line| line.iter().map(|ch| ch.c).collect()).collect();
            (text, c.own_tooltip_inputs(h))
        };
        let (grid, grid_inputs) = lines_at(&c, Hit::Grid(0));
        let (hotbar, hotbar_inputs) = lines_at(&c, Hit::Menu(36));
        assert_eq!((grid, hotbar), (vec!["Stone".to_owned()], vec!["Stone".to_owned(), "Building Blocks".to_owned()]));
        assert!(grid_inputs != hotbar_inputs, "same stack, same tab, other lines");
        c.tab = 1;
        let (search, search_inputs) = lines_at(&c, Hit::Grid(0));
        assert_eq!(search, ["Stone", "Building Blocks"]);
        assert!(search_inputs != grid_inputs);
        // A `#` query makes a tag visible: the screen filters again, the tag line appears.
        c.search.text = "#stone".chars().collect();
        c.visible_tags = vec!["minecraft:stone_tool_materials".to_owned()];
        c.tooltip_lists_revision += 1;
        let (tagged, tagged_inputs) = lines_at(&c, Hit::Grid(0));
        assert_eq!(tagged, ["Stone", "Building Blocks", "#minecraft:stone_tool_materials"]);
        assert!(tagged_inputs != search_inputs);
        // The screen reports it when it replaces the visible tags.
        let revision = c.tooltip_lists_revision;
        c.search_changed(&data);
        assert_ne!(c.tooltip_lists_revision, revision, "search filtered again");
    }

    /// Micro-bench of the hit test (former list against no list), alternated passes.
    /// `cargo test --release -p rc-app --bin rustcraft bench_creative_hit_test -- --ignored --nocapture`
    #[test]
    #[ignore = "micro-bench"]
    fn bench_creative_hit_test() {
        let c = CreativeScreen::default();
        let d = data(CreativeKind::Category);
        let points: Vec<(i32, i32)> = (0..64).map(|i| ((i * 37) % (W + 8) - 4, (i * 53) % (H + 8) - 4)).collect();
        let n = 200_000;
        for pass in 0..3 {
            let t = std::time::Instant::now();
            let mut k = 0usize;
            for i in 0..n {
                k += usize::from(listed(&c, &d, points[i % 64]).is_some());
            }
            let a = t.elapsed().as_nanos() as f64 / n as f64;
            let t = std::time::Instant::now();
            for i in 0..n {
                k += usize::from(c.hit_at(&d, points[i % 64]).is_some());
            }
            let b = t.elapsed().as_nanos() as f64 / n as f64;
            println!("pass {pass}: list {a:.1} ns, no list {b:.1} ns ({k})");
        }
    }
}
