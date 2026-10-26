//! Workstations (`docs/specs/special-screens.md` § 3, `inventories.md` § 5.8–5.9): grindstone,
//! stonecutter, loom, cartography table, smithing table, enchanting table,
//! anvil, beacon, crafter, Shulker box.

use rc_proto_v777::item::ItemStack;
use rc_text::StyledChar;
use winit::keyboard::{KeyCode, ModifiersState};

use crate::inventory::Container;

use crate::menu_special::Sp;
use crate::special::{Ctx, ModelReq, inside, tr};

/// Cyclic empty-slot icon (smithing table, § 3.6).
#[derive(Default, Clone)]
pub struct Cycle {
    list: Vec<&'static str>,
    ticks: u32,
    index: usize,
}

impl Cycle {
    fn set(&mut self, list: Vec<&'static str>) {
        // The tick counter continues across the list changes (only the index
        // goes back to the start): the next switch keeps the 30-tick cadence already begun.
        if list != self.list {
            self.list = list;
            self.index = 0;
        }
    }

    fn tick(&mut self) {
        if self.list.is_empty() {
            return;
        }
        self.ticks += 1;
        if self.ticks.is_multiple_of(30) {
            self.index = (self.index + 1) % self.list.len();
        }
    }
}

/// Animated book of the enchanting table (§ 3.7).
#[derive(Default, Clone, Copy)]
pub struct BookAnim {
    open: f32,
    open_o: f32,
    flip: f32,
    flip_o: f32,
    target: f32,
    speed: f32,
    seed: u32,
}

impl BookAnim {
    fn rand4(&mut self) -> f32 {
        self.seed = self.seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        ((self.seed >> 16) % 4) as f32
    }

    fn tick(&mut self, changed: bool, offers: bool) {
        self.open_o = self.open;
        self.flip_o = self.flip;
        if changed {
            loop {
                self.target += self.rand4() - self.rand4();
                if (self.flip - self.target).abs() > 1.0 {
                    break;
                }
            }
        }
        self.open = (self.open + if offers { 0.2 } else { -0.2 }).clamp(0.0, 1.0);
        let d = ((self.target - self.flip) * 0.4).clamp(-0.2, 0.2);
        self.speed += (d - self.speed) * 0.9;
        self.flip += self.speed;
    }
}

#[derive(Default)]
pub struct Work {
    /// Mount whose inventory is open.
    pub mount_entity: Option<i32>,
    /// Inputs observed at the last tick (detection of changes).
    seen: Vec<Option<ItemStack>>,
    /// Stonecutter: recipes of the input item, scrolling, drag.
    recipes: Vec<Option<ItemStack>>,
    scroll: f32,
    dragging: bool,
    /// Loom: patterns offered (indices of the `banner_pattern` registry).
    patterns: Vec<usize>,
    show_patterns: bool,
    /// Smithing table: icons of the empty slots.
    icons: [Cycle; 3],
    book: BookAnim,
    /// Anvil: name field and last name sent.
    name: Option<crate::widgets::TextField>,
    sent_name: String,
    /// Experience level of the player and creative mode (costs).
    pub level: i32,
    pub creative: bool,
    /// Beacon: effects chosen locally (`mob_effect` ids) and properties seen.
    primary: Option<i32>,
    secondary: Option<i32>,
    beacon_props: [i32; 3],
    effects: Vec<String>,
}

/// Data of the banner pattern registry (id, resource, translation key).
pub static PATTERNS: std::sync::RwLock<Vec<(String, String, String)>> = std::sync::RwLock::new(Vec::new());

/// Synchronised `banner_pattern` registry received.
pub fn set_patterns(entries: &[(String, Option<rc_proto::nbt::Nbt>)]) {
    let list = entries
        .iter()
        .map(|(name, nbt)| {
            let field = |k: &str| nbt.as_ref().and_then(|n| n.get(k)).and_then(|v| v.as_str()).map(str::to_owned);
            let path = name.strip_prefix("minecraft:").unwrap_or(name);
            let asset = field("asset_id").unwrap_or_else(|| name.clone());
            let key = field("translation_key").unwrap_or_else(|| format!("block.minecraft.banner.{path}"));
            (name.clone(), asset, key)
        })
        .collect();
    *PATTERNS.write().unwrap_or_else(|p| p.into_inner()) = list;
}

const DYES: [&str; 16] = [
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "light_gray",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
];

/// Texture colours of the dyes (ARGB), `DYES` order.
const DYE_RGB: [u32; 16] = [
    0xf9fffe, 0xf9801d, 0xc74ebd, 0x3ab3da, 0xfed83d, 0x80c71f, 0xf38baa, 0x474f52, 0x9d9d97, 0x169c9c, 0x8932b8,
    0x3c44aa, 0x835432, 0x5e7c16, 0xb02e26, 0x1d1d21,
];

fn dye_index(name: &str) -> Option<usize> {
    DYES.iter().position(|d| name.strip_suffix("_dye").or_else(|| name.strip_suffix("_banner")) == Some(*d))
}

fn name_of<'a>(it: &'a crate::hud::HotbarItems<'_>, s: &ItemStack) -> &'a str {
    let n = usize::try_from(s.item).ok().and_then(|i| it.names.get(i)).map_or("", String::as_str);
    n.strip_prefix("minecraft:").unwrap_or(n)
}

fn item_id(it: &crate::hud::HotbarItems<'_>, name: &str) -> Option<i32> {
    it.names.iter().position(|n| n.strip_prefix("minecraft:").unwrap_or(n) == name).map(|i| i as i32)
}

fn simple(item: i32) -> ItemStack {
    ItemStack { item, count: 1, added: Vec::new(), removed: Vec::new() }
}

/// Position of a text in the image (`None`: absent; x < 0: centred on −x).
type Place = Option<(i32, i32)>;

/// Background, titles (`None`: no title) and position of "Inventory".
fn layout(s: Sp) -> (&'static str, Place, Place) {
    match s {
        Sp::Grindstone => ("grindstone", Some((8, 6)), Some((8, 72))),
        Sp::Stonecutter => ("stonecutter", Some((8, 5)), Some((8, 72))),
        Sp::Loom => ("loom", Some((8, 4)), Some((8, 72))),
        Sp::Cartography => ("cartography_table", Some((8, 4)), Some((8, 72))),
        Sp::Smithing => ("smithing", Some((44, 15)), Some((8, 72))),
        Sp::Enchant => ("enchanting_table", Some((8, 6)), Some((8, 72))),
        Sp::Anvil => ("anvil", Some((60, 6)), Some((8, 72))),
        Sp::Beacon => ("beacon", None, None),
        Sp::Crafter => ("crafter", Some((-88, 6)), Some((8, 72))),
        _ => ("shulker_box", Some((8, 6)), Some((8, 72))),
    }
}

pub fn draw_back(cx: &mut Ctx<'_, '_>, c: &Container, s: Sp) {
    let (gx, gy) = (cx.gx, cx.gy);
    let f = |v: i32| v as f32;
    let (bg, title, label) = layout(s);
    let (w, h) = crate::menu_special::image_size(s);
    cx.gui.sprite_region(&format!("container_bg/{bg}"), 256.0, 256.0, 0.0, 0.0, f(gx), f(gy), f(w), f(h));
    let slot = |i: usize| c.slots.get(i).and_then(Option::as_ref);
    match s {
        Sp::Grindstone | Sp::Anvil => {
            let (x, y) = if s == Sp::Grindstone { (92, 31) } else { (99, 45) };
            if (slot(0).is_some() || slot(1).is_some()) && slot(2).is_none() {
                let e = if s == Sp::Grindstone { "container/grindstone/error" } else { "container/anvil/error" };
                cx.gui.sprite(e, f(gx + x), f(gy + y), 28.0, 21.0, 0xffff_ffff);
            }
            if s == Sp::Anvil {
                draw_anvil(cx, c);
            }
        }
        Sp::Stonecutter => draw_stonecutter(cx, c),
        Sp::Loom => draw_loom(cx, c),
        Sp::Cartography => draw_cartography(cx, c),
        Sp::Smithing => draw_smithing(cx, c),
        Sp::Enchant => draw_enchant(cx, c),
        Sp::Beacon => draw_beacon(cx, c),
        Sp::Crafter => {
            for i in 0..9 {
                if c.props[i] == 1 {
                    let d = crate::menu_special::slot_def(s, i, c.slots.len());
                    cx.gui.sprite(
                        "container/crafter/disabled_slot",
                        f(gx + d.x - 1),
                        f(gy + d.y - 1),
                        18.0,
                        18.0,
                        0xffff_ffff,
                    );
                }
            }
            let (sw, sh) = (cx.gui.width as i32, cx.gui.height as i32);
            let r = if c.props[9] == 1 {
                "container/crafter/powered_redstone"
            } else {
                "container/crafter/unpowered_redstone"
            };
            cx.gui.sprite(r, f(sw / 2 + 9), f(sh / 2 - 48), 16.0, 16.0, 0xffff_ffff);
        }
        _ => {}
    }
    crate::special::titles(cx, c, title, label);
}

pub fn draw_front(cx: &mut Ctx<'_, '_>, c: &Container, s: Sp) {
    let Some(m) = cx.mouse else { return };
    // Tooltips specific to the screen: shown even with an item in hand, except the crafter's.
    let (gx, gy) = (cx.gx, cx.gy);
    match s {
        Sp::Stonecutter => {
            let w = &c.extra.work;
            if c.slots.first().is_some_and(Option::is_some)
                && let Some(i) = recipe_at(w, gx, gy, m)
                && let Some(Some(st)) = w.recipes.get(i)
            {
                crate::special::item_tooltip(cx.gui, cx.queue, cx.it, cx.enchantments, st, m);
            }
        }
        Sp::Loom => {
            let w = &c.extra.work;
            if w.show_patterns
                && let Some(i) = pattern_at(w, gx, gy, m)
                && i as i32 != c.props[0]
                && let Some(&p) = w.patterns.get(i)
            {
                let dye =
                    c.slots.get(1).and_then(Option::as_ref).and_then(|d| dye_index(name_of(cx.it, d))).unwrap_or(0);
                let key =
                    PATTERNS.read().unwrap_or_else(|p| p.into_inner()).get(p).map(|e| e.2.clone()).unwrap_or_default();
                let line = tr(cx.gui, &format!("{key}.{}", DYES[dye]));
                crate::special::text_tooltip(cx.gui, cx.queue, &[line], m);
            }
        }
        Sp::Smithing => {
            let err = c.props[0] == 1 && inside(Some(m), gx + 65, gy + 46, 28, 21);
            let hover = |i: usize| {
                let d = crate::menu_special::slot_def(s, i, c.slots.len());
                inside(Some(m), gx + d.x - 1, gy + d.y - 1, 18, 18) && c.slots.get(i).is_some_and(Option::is_none)
            };
            let template = c.slots.first().and_then(Option::as_ref).map(|t| name_of(cx.it, t).to_owned());
            let kind = template.as_deref().and_then(|t| {
                if t == "netherite_upgrade_smithing_template" {
                    Some("netherite_upgrade")
                } else if t.ends_with("_armor_trim_smithing_template") {
                    Some("armor_trim")
                } else {
                    None
                }
            });
            let key = if err {
                Some("container.upgrade.error_tooltip".to_owned())
            } else if hover(0) {
                Some("container.upgrade.missing_template_tooltip".to_owned())
            } else if let Some(k) = kind {
                if hover(1) {
                    Some(format!("item.minecraft.smithing_template.{k}.base_slot_description"))
                } else if hover(2) {
                    Some(format!("item.minecraft.smithing_template.{k}.additions_slot_description"))
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(k) = key {
                let lines = rc_text::wrap(cx.gui, &tr(cx.gui, &k), 115.0);
                crate::special::text_tooltip(cx.gui, cx.queue, &lines, m);
            }
        }
        Sp::Enchant => enchant_tooltip(cx, c, m),
        Sp::Beacon => beacon_tooltip(cx, c, m),
        Sp::Crafter if cx.inv.carried.is_none() => {
            for i in 0..9 {
                let d = crate::menu_special::slot_def(s, i, c.slots.len());
                if c.props[i] == 0
                    && c.slots.get(i).is_some_and(Option::is_none)
                    && inside(Some(m), gx + d.x - 1, gy + d.y - 1, 18, 18)
                {
                    let line = tr(cx.gui, "gui.togglable_slot");
                    crate::special::text_tooltip(cx.gui, cx.queue, &[line], m);
                }
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------------------------
// Stonecutter (§ 3.3).

fn sc_rows(w: &Work) -> i32 {
    (w.recipes.len() as i32 + 3) / 4 - 3
}

fn sc_start(w: &Work) -> usize {
    let rows = sc_rows(w).max(0);
    ((w.scroll * rows as f32 + 0.5).floor() as usize) * 4
}

fn recipe_at(w: &Work, gx: i32, gy: i32, m: (f32, f32)) -> Option<usize> {
    let start = sc_start(w);
    (0..12).find_map(|j| {
        let (x, y) = (gx + 52 + 16 * (j % 4) as i32, gy + 14 + 18 * (j / 4) as i32);
        let i = start + j;
        (i < w.recipes.len() && inside(Some(m), x, y, 16, 18)).then_some(i)
    })
}

fn draw_stonecutter(cx: &mut Ctx<'_, '_>, c: &Container) {
    let (gx, gy) = (cx.gx, cx.gy);
    let f = |v: i32| v as f32;
    let w = &c.extra.work;
    let n = w.recipes.len();
    let sprite = if n > 12 { "container/stonecutter/scroller" } else { "container/stonecutter/scroller_disabled" };
    cx.gui.sprite(sprite, f(gx + 119), f(gy + 15 + (41.0 * w.scroll) as i32), 12.0, 15.0, 0xffff_ffff);
    if c.slots.first().is_none_or(Option::is_none) {
        return;
    }
    let start = sc_start(w);
    for j in 0..12usize {
        let i = start + j;
        if i >= n {
            break;
        }
        let (x, y) = (gx + 52 + 16 * (j % 4) as i32, gy + 16 + 18 * (j / 4) as i32);
        let bg = if i as i32 == c.props[0] {
            "container/stonecutter/recipe_selected"
        } else if inside(cx.mouse, x, y, 16, 18) {
            "container/stonecutter/recipe_highlighted"
        } else {
            "container/stonecutter/recipe"
        };
        cx.gui.sprite(bg, f(x), f(y - 1), 16.0, 18.0, 0xffff_ffff);
        if let Some(st) = &w.recipes[i] {
            crate::special::item(cx.gui, cx.it, st, f(x), f(y));
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Loom (§ 3.4).

fn loom_rows(w: &Work) -> i32 {
    (w.patterns.len() as i32 + 3) / 4 - 4
}

fn loom_first_row(w: &Work) -> usize {
    (w.scroll * loom_rows(w).max(0) as f32 + 0.5).floor() as usize
}

fn pattern_at(w: &Work, gx: i32, gy: i32, m: (f32, f32)) -> Option<usize> {
    let first = loom_first_row(w) * 4;
    (0..16).find_map(|k| {
        let (x, y) = (gx + 60 + 14 * (k % 4) as i32, gy + 13 + 14 * (k / 4) as i32);
        let i = first + k;
        (i < w.patterns.len() && inside(Some(m), x, y, 14, 14)).then_some(i)
    })
}

/// Layers of a banner (`banner_patterns`): (pattern index, dye).
fn banner_layers(it: &crate::hud::HotbarItems<'_>, st: &ItemStack) -> Vec<(Option<usize>, usize)> {
    use rc_proto::Reader;
    use rc_proto::varint::read_varint;
    let info = usize::try_from(st.item).ok().and_then(|i| it.infos.get(i));
    let view = crate::stack::StackRef { stack: st, info, ids: it.ids, data: it.data, hold: crate::stack::Hold::NONE };
    let Some(b) = view.patch("minecraft:banner_patterns") else { return Vec::new() };
    let mut r = Reader::new(b);
    let n = read_varint(&mut r).unwrap_or(0).clamp(0, 64);
    let mut out = Vec::new();
    for _ in 0..n {
        let Ok(h) = read_varint(&mut r) else { break };
        let pattern = if h == 0 {
            let Ok(asset) = rc_proto::read_str(&mut r, 32767).map(str::to_owned) else { break };
            if rc_proto::read_str(&mut r, 32767).is_err() {
                break;
            }
            PATTERNS.read().unwrap_or_else(|p| p.into_inner()).iter().position(|e| e.1 == asset)
        } else {
            Some((h - 1) as usize)
        };
        let Ok(color) = read_varint(&mut r) else { break };
        out.push((pattern, color.clamp(0, 15) as usize));
    }
    out
}

/// Sprite of a pattern in the interface atlas (`banner/<resource>`).
fn pattern_sprite(p: usize) -> Option<String> {
    let g = PATTERNS.read().unwrap_or_else(|p| p.into_inner());
    let asset = &g.get(p)?.1;
    Some(format!("banner/{}", asset.strip_prefix("minecraft:").unwrap_or(asset)))
}

fn tint(rgb: u32) -> u32 {
    0xff00_0000 | rgb
}

fn draw_loom(cx: &mut Ctx<'_, '_>, c: &Container) {
    let (gx, gy) = (cx.gx, cx.gy);
    let f = |v: i32| v as f32;
    let w = &c.extra.work;
    let n = w.patterns.len();
    let sprite = if w.show_patterns && n > 16 { "container/loom/scroller" } else { "container/loom/scroller_disabled" };
    cx.gui.sprite(sprite, f(gx + 119), f(gy + 13 + (41.0 * w.scroll) as i32), 12.0, 15.0, 0xffff_ffff);
    let banner = c.slots.first().and_then(Option::as_ref);
    let layers = banner.map_or(0, |b| banner_layers(cx.it, b).len());
    // Preview (flattened: flag seen from the front) or error with 6 layers.
    if layers >= 6 {
        cx.gui.sprite("container/loom/error", f(gx + 138), f(gy + 52), 26.0, 26.0, 0xffff_ffff);
    } else if let Some(res) = c.slots.get(3).and_then(Option::as_ref) {
        let base = dye_index(name_of(cx.it, res)).unwrap_or(0);
        let (x, y) = (gx + 141, gy + 8);
        cx.gui.sprite_stretch(
            "banner/base",
            (64.0, 64.0),
            [1.0, 1.0, 20.0, 40.0],
            [f(x), f(y), 20.0, 40.0],
            tint(DYE_RGB[base]),
        );
        for (p, color) in banner_layers(cx.it, res) {
            if let Some(sp) = p.and_then(pattern_sprite) {
                cx.gui.sprite_stretch(
                    &sp,
                    (64.0, 64.0),
                    [1.0, 1.0, 20.0, 40.0],
                    [f(x), f(y), 20.0, 40.0],
                    tint(DYE_RGB[color]),
                );
            }
        }
    }
    if !w.show_patterns {
        return;
    }
    let first = loom_first_row(w) * 4;
    for k in 0..16usize {
        let i = first + k;
        let Some(&p) = w.patterns.get(i) else { break };
        let (x, y) = (gx + 60 + 14 * (k % 4) as i32, gy + 13 + 14 * (k / 4) as i32);
        let bg = if i as i32 == c.props[0] {
            "container/loom/pattern_selected"
        } else if inside(cx.mouse, x, y, 14, 14) {
            "container/loom/pattern_highlighted"
        } else {
            "container/loom/pattern"
        };
        cx.gui.sprite(bg, f(x), f(y), 14.0, 14.0, 0xffff_ffff);
        // Thumbnail: grey flat (grey dye) then white pattern, untinted.
        cx.gui.fill(f(x + 4), f(y + 2), f(x + 9), f(y + 12), tint(DYE_RGB[7]));
        if let Some(sp) = pattern_sprite(p) {
            cx.gui.sprite_stretch(
                &sp,
                (64.0, 64.0),
                [0.0, 1.0, 21.0, 40.0],
                [f(x + 4), f(y + 2), 5.0, 10.0],
                0xffff_ffff,
            );
        }
    }
}

/// Patterns offered (§ 3.4): without a pattern item, those of the `no_item_required` tag;
/// otherwise those of the tag carried by `provides_banner_patterns`.
fn loom_patterns(it: &crate::hud::HotbarItems<'_>, pattern_item: Option<&ItemStack>) -> Vec<usize> {
    let tag = match pattern_item {
        None => "minecraft:no_item_required".to_owned(),
        Some(st) => {
            let info = usize::try_from(st.item).ok().and_then(|i| it.infos.get(i));
            let view = crate::stack::StackRef { stack: st, info, ids: it.ids, data: it.data, hold: crate::stack::Hold::NONE };
            match view.patch("minecraft:provides_banner_patterns") {
                Some(b) => {
                    let mut r = rc_proto::Reader::new(b);
                    match rc_proto::read_str(&mut r, 32767) {
                        Ok(t) => t.to_owned(),
                        Err(_) => return Vec::new(),
                    }
                }
                // Default component of the item: tag of the same name as the item.
                None => format!("minecraft:pattern_item/{}", name_of(it, st).trim_end_matches("_banner_pattern")),
            }
        }
    };
    let tag = if tag.contains(':') { tag } else { format!("minecraft:{tag}") };
    crate::menu_special::pattern_tag(&tag)
}

// ---------------------------------------------------------------------------------------------
// Cartography table (§ 3.5): frames, error icon and map preview.

/// 2D affine `[a, b, c, d, e, f]` (x' = a·x + c·y + e, y' = b·x + d·y + f): product `p · q`.
fn affine_mul(p: [f32; 6], q: [f32; 6]) -> [f32; 6] {
    [
        p[0] * q[0] + p[2] * q[1],
        p[1] * q[0] + p[3] * q[1],
        p[0] * q[2] + p[2] * q[3],
        p[1] * q[2] + p[3] * q[3],
        p[0] * q[4] + p[2] * q[5] + p[4],
        p[1] * q[4] + p[3] * q[5] + p[5],
    ]
}

fn affine_translate(x: f32, y: f32) -> [f32; 6] {
    [1.0, 0.0, 0.0, 1.0, x, y]
}

fn affine_scale(s: f32) -> [f32; 6] {
    [s, 0.0, 0.0, s, 0.0, 0.0]
}

/// Last content sent in the slot `dyn/map/0`: map, generation, fingerprint.
static MAP_SLOT: std::sync::Mutex<Option<(i32, u32, u64)>> = std::sync::Mutex::new(None);

/// Map `id` drawn at `(x, y)` at scale `s`: 128 × 128 pixels then the decorations
/// visible in a frame (8 × 8 icon rotated in sixteenths of a turn, name reduced on a dark background).
fn draw_map_preview(cx: &mut Ctx<'_, '_>, id: i32, x: i32, y: i32, s: f32) {
    use std::hash::{Hash, Hasher};
    let store = crate::maps::store();
    let Some(d) = store.get(id) else { return };
    let mut h = std::collections::hash_map::DefaultHasher::new();
    d.colors.hash(&mut h);
    let key = (id, store.generation, h.finish());
    let mut last = MAP_SLOT.lock().unwrap_or_else(|p| p.into_inner());
    if *last != Some(key) {
        let pal = crate::maps::palette();
        let rgba: Vec<u8> = d.colors.iter().flat_map(|&c| pal[c as usize]).collect();
        if cx.gui.write_sprite(cx.queue, "dyn/map/0", (128, 128), &rgba) {
            *last = Some(key);
        }
    }
    drop(last);
    let (x, y) = (x as f32, y as f32);
    cx.gui.sprite("dyn/map/0", x, y, 128.0 * s, 128.0 * s, 0xffff_ffff);
    let base = affine_mul(affine_translate(x, y), affine_scale(s));
    for dec in &d.decorations {
        let Some(&(sprite, on_frame)) = usize::try_from(dec.kind).ok().and_then(|k| crate::maps::DECORATIONS.get(k))
        else {
            continue;
        };
        if !on_frame {
            continue;
        }
        let (px, py) = (f32::from(dec.x) / 2.0 + 64.0, f32::from(dec.y) / 2.0 + 64.0);
        let a = (f32::from(dec.rot) * 360.0 / 16.0).to_radians();
        let rot = [a.cos(), a.sin(), -a.sin(), a.cos(), 0.0, 0.0];
        let m = [base, affine_translate(px, py), rot, affine_scale(4.0), affine_translate(-0.125, 0.125)]
            .into_iter()
            .reduce(affine_mul)
            .unwrap_or(base);
        // Quad −1..1, texture flipped vertically (top of the quad = bottom of the sprite).
        let mark = cx.gui.mark();
        cx.gui.sprite_stretch(
            &format!("map_deco/{sprite}"),
            (1.0, 1.0),
            [0.0, 1.0, 1.0, -1.0],
            [-1.0, -1.0, 2.0, 2.0],
            0xffff_ffff,
        );
        cx.gui.transform_since(mark, m);
        if let Some(name) = dec.name.as_ref().and_then(rc_text::from_nbt) {
            let chars = crate::special::chars(cx.gui, &name);
            let w = rc_text::width(cx.gui, &chars) as f32;
            let sc = (25.0 / w).clamp(0.0, 6.0 / 9.0);
            let m = affine_mul(base, affine_mul(affine_translate(px - w * sc / 2.0, py + 4.0), affine_scale(sc)));
            let mark = cx.gui.mark();
            cx.gui.text(cx.queue, &chars, 0.0, 0.0, 0xffff_ffff, false, 0x8000_0000);
            cx.gui.transform_since(mark, m);
        }
    }
}

fn draw_cartography(cx: &mut Ctx<'_, '_>, c: &Container) {
    let (gx, gy) = (cx.gx, cx.gy);
    let f = |v: i32| v as f32;
    let add = c.slots.get(1).and_then(Option::as_ref).map(|s| name_of(cx.it, s).to_owned());
    let (dup, scaling, locking) =
        (add.as_deref() == Some("map"), add.as_deref() == Some("paper"), add.as_deref() == Some("glass_pane"));
    // Map known to the client: locked, or paper impossible (cannot be expanded, scale ≥ 4)
    // → error icon and "normal" preview instead of the expanded one.
    let map = c.slots.first().and_then(Option::as_ref);
    let mut locked = false;
    let mut id = None;
    if let Some(m) = map
        && let Some(map_id) = crate::maps::map_id(cx.it.ids, m)
        && let Some((d_locked, d_scale)) = crate::maps::store().get(map_id).map(|d| (d.locked, d.scale))
    {
        id = Some(map_id);
        let error = |g: &mut crate::gui::Gui| {
            g.sprite("container/cartography_table/error", f(gx + 35), f(gy + 31), 28.0, 21.0, 0xffff_ffff);
        };
        if d_locked {
            locked = true;
            if scaling || locking {
                error(cx.gui);
            }
        }
        let extendable = crate::menu_special::in_tag("minecraft:extendable_maps", m.item)
            .unwrap_or_else(|| name_of(cx.it, m) == "filled_map");
        if scaling && (!extendable || d_scale >= 4) {
            locked = true;
            error(cx.gui);
        }
    }
    let frame = |cx: &mut Ctx<'_, '_>, name: &str, x: i32, y: i32, w: f32| {
        cx.gui.sprite(&format!("container/cartography_table/{name}"), f(x), f(y), w, 66.0, 0xffff_ffff);
    };
    if scaling && !locked {
        frame(cx, "scaled_map", gx + 67, gy + 13, 66.0);
        if let Some(id) = id {
            draw_map_preview(cx, id, gx + 85, gy + 31, 0.226);
        }
    } else if dup {
        frame(cx, "duplicated_map", gx + 83, gy + 13, 50.0);
        if let Some(id) = id {
            draw_map_preview(cx, id, gx + 86, gy + 16, 0.34);
        }
        cx.gui.next_layer();
        frame(cx, "duplicated_map", gx + 67, gy + 29, 50.0);
        if let Some(id) = id {
            draw_map_preview(cx, id, gx + 70, gy + 32, 0.34);
        }
    } else {
        frame(cx, "map", gx + 67, gy + 13, 66.0);
        if let Some(id) = id {
            draw_map_preview(cx, id, gx + 71, gy + 17, 0.45);
        }
        if locking {
            cx.gui.sprite("container/cartography_table/locked", f(gx + 118), f(gy + 60), 10.0, 14.0, 0xffff_ffff);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Smithing table (§ 3.6).

fn smithing_icons(template: Option<&str>) -> [Vec<&'static str>; 3] {
    const TRIM_BASE: [&str; 4] =
        ["container/slot/helmet", "container/slot/chestplate", "container/slot/leggings", "container/slot/boots"];
    const TRIM_ADD: [&str; 7] = [
        "container/slot/ingot",
        "container/slot/redstone_dust",
        "container/slot/lapis_lazuli",
        "container/slot/quartz",
        "container/slot/diamond",
        "container/slot/emerald",
        "container/slot/amethyst_shard",
    ];
    const UP_BASE: [&str; 11] = [
        "container/slot/helmet",
        "container/slot/sword",
        "container/slot/chestplate",
        "container/slot/pickaxe",
        "container/slot/leggings",
        "container/slot/axe",
        "container/slot/boots",
        "container/slot/hoe",
        "container/slot/shovel",
        "container/slot/nautilus_armor",
        "container/slot/spear",
    ];
    let first =
        vec!["container/slot/smithing_template_armor_trim", "container/slot/smithing_template_netherite_upgrade"];
    match template {
        None => [first, Vec::new(), Vec::new()],
        Some("netherite_upgrade_smithing_template") => [first, UP_BASE.to_vec(), vec!["container/slot/ingot"]],
        Some(t) if t.ends_with("_armor_trim_smithing_template") => [first, TRIM_BASE.to_vec(), TRIM_ADD.to_vec()],
        Some(_) => [first, Vec::new(), Vec::new()],
    }
}

fn draw_smithing(cx: &mut Ctx<'_, '_>, c: &Container) {
    let (gx, gy) = (cx.gx, cx.gy);
    let f = |v: i32| v as f32;
    let w = &c.extra.work;
    for (i, cyc) in w.icons.iter().enumerate() {
        if c.slots.get(i).is_some_and(Option::is_some) || cyc.list.is_empty() {
            continue;
        }
        let d = crate::menu_special::slot_def(Sp::Smithing, i, c.slots.len());
        let cur = cyc.list[cyc.index % cyc.list.len()];
        if cyc.list.len() > 1 && cyc.ticks >= 30 {
            let a = ((cyc.ticks % 30) as f32 + cx.it.pt).min(4.0) / 4.0;
            let old = cyc.list[(cyc.index + cyc.list.len() - 1) % cyc.list.len()];
            let alpha = |a: f32| ((a.clamp(0.0, 1.0) * 255.0) as u32) << 24 | 0x00ff_ffff;
            cx.gui.sprite(old, f(gx + d.x), f(gy + d.y), 16.0, 16.0, alpha(1.0 - a));
            cx.gui.sprite(cur, f(gx + d.x), f(gy + d.y), 16.0, 16.0, alpha(a));
        } else {
            cx.gui.sprite(cur, f(gx + d.x), f(gy + d.y), 16.0, 16.0, 0xffff_ffff);
        }
    }
    if c.props[0] == 1 {
        cx.gui.sprite("container/smithing/error", f(gx + 65), f(gy + 46), 28.0, 21.0, 0xffff_ffff);
    }
    // Mannequin: equipment from the result (§ 3.6).
    let mut equipment = Vec::new();
    if let Some(r) = c.slots.get(3).and_then(Option::as_ref) {
        let e = usize::try_from(r.item).ok().and_then(|i| cx.it.infos.get(i)).map_or(0, |i| i.equip_slot);
        let slot = match e {
            2..=5 => usize::from(e),
            _ => 1,
        };
        equipment.push((slot, r.clone()));
    }
    let rect = [f(gx + 121), f(gy + 20), f(gx + 161), f(gy + 80)];
    crate::special::request_model(cx.gui, ModelReq::Stand { rect, equipment });
}

// ---------------------------------------------------------------------------------------------
// Enchanting table (`inventories.md` § 5.8, § 3.7).

/// Fixed list of "galactic" words (`minecraft:alt` font); the order matters for the draws.
const GALACTIC_WORDS: [&str; 62] = [
    "the",
    "elder",
    "scrolls",
    "klaatu",
    "berata",
    "niktu",
    "xyzzy",
    "bless",
    "curse",
    "light",
    "darkness",
    "fire",
    "air",
    "earth",
    "water",
    "hot",
    "dry",
    "cold",
    "wet",
    "ignite",
    "snuff",
    "embiggen",
    "twist",
    "shorten",
    "stretch",
    "fiddle",
    "destroy",
    "imbue",
    "galvanize",
    "enchant",
    "free",
    "limited",
    "range",
    "of",
    "towards",
    "inside",
    "sphere",
    "cube",
    "self",
    "other",
    "ball",
    "mental",
    "physical",
    "grow",
    "shrink",
    "demon",
    "elemental",
    "spirit",
    "animal",
    "creature",
    "beast",
    "humanoid",
    "undead",
    "fresh",
    "stale",
    "phnglui",
    "mglwnafh",
    "cthulhu",
    "rlyeh",
    "wgahnagl",
    "fhtagn",
    "baguette",
];

/// Name of an option: 3 or 4 words drawn from the Java generator reseeded on every frame with the
/// enchantment seed (data 3); successive options consume the same generator.
fn galactic(rng: &mut rc_audio::JavaRandom) -> String {
    let words = rng.next_int_bounded(2) + 3;
    (0..words)
        .map(|_| GALACTIC_WORDS[rng.next_int_bounded(GALACTIC_WORDS.len() as i32) as usize])
        .collect::<Vec<_>>()
        .join(" ")
}

/// Longest prefix whose cumulated width fits in `max` (cuts at the character, one line).
fn head_by_width(m: &dyn rc_text::Metrics, chars: &[StyledChar], max: i32) -> Vec<StyledChar> {
    let mut left = max as f32;
    let mut n = 0;
    for s in chars {
        let (a, bold) = m.advance(&s.style.font, s.c);
        left -= if s.style.bold { a + bold } else { a };
        if left < 0.0 {
            break;
        }
        n += 1;
    }
    chars[..n].to_vec()
}

fn enchant_available(c: &Container, i: usize) -> bool {
    let w = &c.extra.work;
    let cost = c.props[i];
    let lapis = c.slots.get(1).and_then(Option::as_ref).map_or(0, |s| s.count);
    let item = c.slots.first().is_some_and(Option::is_some);
    if w.creative {
        return cost > 0 && item;
    }
    cost > 0 && item && lapis > i as i32 && w.level > i as i32 && w.level >= cost
}

fn draw_enchant(cx: &mut Ctx<'_, '_>, c: &Container) {
    let (gx, gy) = (cx.gx, cx.gy);
    let f = |v: i32| v as f32;
    let w = &c.extra.work;
    // Animated book (interpolated values).
    let pt = cx.it.pt;
    let open = w.book.open_o + (w.book.open - w.book.open_o) * pt;
    let flip = w.book.flip_o + (w.book.flip - w.book.flip_o) * pt;
    let frac = |v: f32| v - v.floor();
    let p1 = (frac(flip + 0.25) * 1.6 - 0.3).clamp(0.0, 1.0);
    let p2 = (frac(flip + 0.75) * 1.6 - 0.3).clamp(0.0, 1.0);
    let rect = [f(gx + 14), f(gy + 14), f(gx + 52), f(gy + 45)];
    crate::special::request_model(cx.gui, ModelReq::Book { rect, open, flip: [p1, p2] });
    let mut rng = rc_audio::JavaRandom::new(i64::from(c.props[3]));
    for i in 0..3usize {
        let (x, y) = (gx + 60, gy + 14 + 19 * i as i32);
        let cost = c.props[i];
        if cost == 0 {
            cx.gui.sprite("container/enchanting_table/enchantment_slot_disabled", f(x), f(y), 108.0, 19.0, 0xffff_ffff);
            continue;
        }
        let cost_s = crate::gui::plain(&cost.to_string());
        let cw = rc_text::width(cx.gui, &cost_s);
        let mut name = rc_text::Component::text(galactic(&mut rng));
        name.style.font = Some("minecraft:alt".into());
        let name = crate::special::chars(cx.gui, &name);
        let name = head_by_width(cx.gui, &name, 86 - cw);
        let lines = rc_text::wrap(cx.gui, &name, (86 - cw) as f32);
        let (bg, icon, name_color, cost_color) = if !enchant_available(c, i) {
            ("enchantment_slot_disabled", format!("level_{}_disabled", i + 1), 0xff34_2f25u32, 0xff40_7f10u32)
        } else if inside(cx.mouse, x, y, 108, 19) {
            ("enchantment_slot_highlighted", format!("level_{}", i + 1), 0xffff_ff80, 0xff80_ff20)
        } else {
            ("enchantment_slot", format!("level_{}", i + 1), 0xff68_5e4a, 0xff80_ff20)
        };
        cx.gui.sprite(&format!("container/enchanting_table/{bg}"), f(x), f(y), 108.0, 19.0, 0xffff_ffff);
        cx.gui.sprite(&format!("container/enchanting_table/{icon}"), f(x + 1), f(y + 1), 16.0, 16.0, 0xffff_ffff);
        for (k, l) in lines.iter().enumerate() {
            cx.gui.text(cx.queue, l, f(gx + 80), f(gy + 16 + 19 * i as i32 + 9 * k as i32), name_color, false, 0);
        }
        cx.gui.text(cx.queue, &cost_s, f(gx + 80 + 86 - cw), f(gy + 23 + 19 * i as i32), cost_color, true, 0);
    }
}

fn enchant_tooltip(cx: &mut Ctx<'_, '_>, c: &Container, m: (f32, f32)) {
    let (gx, gy) = (cx.gx, cx.gy);
    let w = &c.extra.work;
    for i in 0..3usize {
        if !inside(Some(m), gx + 60, gy + 14 + 19 * i as i32, 108, 17) {
            continue;
        }
        let (cost, ench, lvl) = (c.props[i], c.props[4 + i], c.props[7 + i]);
        if cost <= 0 || ench < 0 {
            return;
        }
        // Full name: grey (red if curse), level omitted for a single-level enchantment.
        let Some(name) = crate::tooltips::enchantment_line(ench, lvl, cx.enchantments, &crate::screens::SINGLE_LEVEL)
        else {
            return;
        };
        let mut clue = rc_text::Component::translate("container.enchant.clue", vec![rc_text::Arg::Component(name)]);
        clue.style.color = Some(0xff_ffff);
        let mut lines = vec![crate::special::chars(cx.gui, &clue)];
        if !w.creative {
            lines.push(Vec::new());
            let need = i as i32 + 1;
            if w.level < cost {
                let mut l = rc_text::Component::translate(
                    "container.enchant.level.requirement",
                    vec![rc_text::Arg::Text(cost.to_string())],
                );
                l.style.color = Some(0xff_5555);
                lines.push(crate::special::chars(cx.gui, &l));
            } else {
                let lapis = c.slots.get(1).and_then(Option::as_ref).map_or(0, |s| s.count);
                let key = if need == 1 { "container.enchant.lapis.one" } else { "container.enchant.lapis.many" };
                let mut l = rc_text::Component::translate(key, vec![rc_text::Arg::Text(need.to_string())]);
                l.style.color = Some(if lapis >= need { 0xaa_aaaa } else { 0xff_5555 });
                lines.push(crate::special::chars(cx.gui, &l));
                let key = if need == 1 { "container.enchant.level.one" } else { "container.enchant.level.many" };
                let mut l = rc_text::Component::translate(key, vec![rc_text::Arg::Text(need.to_string())]);
                l.style.color = Some(0xaa_aaaa);
                lines.push(crate::special::chars(cx.gui, &l));
            }
        }
        crate::special::text_tooltip(cx.gui, cx.queue, &lines, m);
        return;
    }
}

// ---------------------------------------------------------------------------------------------
// Anvil (`inventories.md` § 5.9).

fn draw_anvil(cx: &mut Ctx<'_, '_>, c: &Container) {
    let (gx, gy) = (cx.gx, cx.gy);
    let f = |v: i32| v as f32;
    let w = &c.extra.work;
    let has = c.slots.first().is_some_and(Option::is_some);
    let bg = if has { "container/anvil/text_field" } else { "container/anvil/text_field_disabled" };
    cx.gui.sprite(bg, f(gx + 59), f(gy + 20), 110.0, 16.0, 0xffff_ffff);
    if let Some(t) = &w.name {
        let blink = has && (cx.it.env.tick / 6) % 2 == 0;
        crate::widgets::draw_field_blink(cx.gui, cx.queue, t, [gx + 62, gy + 24, 103, 12], has, blink);
    }
    // Cost.
    let cost = c.props[0];
    if cost > 0 {
        let result = c.slots.get(2).is_some_and(Option::is_some);
        let takeable = w.creative || w.level >= cost;
        let line: Option<(Vec<StyledChar>, u32)> = if cost >= 40 && !w.creative {
            Some((tr(cx.gui, "container.repair.expensive"), 0xffff_6060))
        } else if result {
            let comp =
                rc_text::Component::translate("container.repair.cost", vec![rc_text::Arg::Text(cost.to_string())]);
            Some((crate::special::chars(cx.gui, &comp), if takeable { 0xff80_ff20 } else { 0xffff_6060 }))
        } else {
            None
        };
        if let Some((l, color)) = line {
            let tw = rc_text::width(cx.gui, &l);
            let tx = 176 - 8 - tw - 2;
            cx.gui.fill(f(gx + tx - 2), f(gy + 67), f(gx + 168), f(gy + 79), 0x4f00_0000);
            cx.gui.text(cx.queue, &l, f(gx + tx), f(gy + 69), color, true, 0);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Beacon (§ 3.8).

const BEACON_RANKS: [&[&str]; 4] =
    [&["speed", "haste"], &["resistance", "jump_boost"], &["strength"], &["regeneration"]];

/// Beacon button: position, effect, primary (true) or secondary, "level II".
struct BeaconButton {
    x: i32,
    y: i32,
    effect: Option<i32>,
    name: &'static str,
    primary: bool,
    upgrade: bool,
}

fn effect_id(w: &Work, name: &str) -> Option<i32> {
    w.effects.iter().position(|e| e.strip_prefix("minecraft:").unwrap_or(e) == name).map(|i| i as i32)
}

fn effect_name(w: &Work, id: i32) -> &str {
    w.effects.get(id as usize).map_or("", |e| e.strip_prefix("minecraft:").unwrap_or(e))
}

fn beacon_buttons(w: &Work, gx: i32, gy: i32) -> Vec<BeaconButton> {
    let mut out = Vec::new();
    for (t, rank) in BEACON_RANKS.iter().take(3).enumerate() {
        let k = rank.len() as i32;
        let l = 22 * k + 2 * (k - 1);
        for (c, name) in rank.iter().enumerate() {
            out.push(BeaconButton {
                x: gx + 76 + 24 * c as i32 - l / 2,
                y: gy + 22 + 25 * t as i32,
                effect: effect_id(w, name),
                name,
                primary: true,
                upgrade: false,
            });
        }
    }
    out.push(BeaconButton {
        x: gx + 144,
        y: gy + 47,
        effect: effect_id(w, "regeneration"),
        name: "regeneration",
        primary: false,
        upgrade: false,
    });
    if let Some(p) = w.primary {
        out.push(BeaconButton { x: gx + 168, y: gy + 47, effect: Some(p), name: "", primary: false, upgrade: true });
    }
    out
}

fn beacon_state(w: &Work, levels: i32, b: &BeaconButton, rank: usize) -> (bool, bool) {
    if b.primary {
        (rank < levels as usize, b.effect.is_some() && w.primary == b.effect)
    } else {
        (levels > 3, b.effect.is_some() && w.secondary == b.effect)
    }
}

fn beacon_rank(i: usize) -> usize {
    [0, 0, 1, 1, 2, 3, 3].get(i).copied().unwrap_or(3)
}

fn draw_beacon(cx: &mut Ctx<'_, '_>, c: &Container) {
    let (gx, gy) = (cx.gx, cx.gy);
    let f = |v: i32| v as f32;
    let w = &c.extra.work;
    let levels = c.props[0];
    for (i, name) in ["netherite_ingot", "emerald", "diamond", "gold_ingot", "iron_ingot"].iter().enumerate() {
        if let Some(id) = item_id(cx.it, name) {
            let x = [20, 41, 63, 86, 108][i];
            crate::special::item(cx.gui, cx.it, &simple(id), f(gx + x), f(gy + 109));
        }
    }
    for l in [("block.minecraft.beacon.primary", 62), ("block.minecraft.beacon.secondary", 169)] {
        let t = tr(cx.gui, l.0);
        let tw = rc_text::width(cx.gui, &t);
        cx.gui.text(cx.queue, &t, f(gx + l.1 - tw / 2), f(gy + 10), 0xffe0_e0e0, true, 0);
    }
    let frame = |g: &mut crate::gui::Gui, x: i32, y: i32, active: bool, selected: bool, hover: bool| {
        let s = if !active {
            "container/beacon/button_disabled"
        } else if selected {
            "container/beacon/button_selected"
        } else if hover {
            "container/beacon/button_highlighted"
        } else {
            "container/beacon/button"
        };
        g.sprite(s, f(x), f(y), 22.0, 22.0, 0xffff_ffff);
    };
    for (i, b) in beacon_buttons(w, gx, gy).iter().enumerate() {
        let (active, selected) = beacon_state(w, levels, b, beacon_rank(i));
        frame(cx.gui, b.x, b.y, active, selected, inside(cx.mouse, b.x, b.y, 22, 22));
        let name = if b.upgrade { b.effect.map_or("", |e| effect_name(w, e)) } else { b.name };
        cx.gui.sprite(&format!("mob_effect/{name}"), f(b.x + 2), f(b.y + 2), 18.0, 18.0, 0xffff_ffff);
    }
    let pay = c.slots.first().is_some_and(Option::is_some);
    let confirm = pay && w.primary.is_some();
    let (x, y) = (gx + 164, gy + 107);
    frame(cx.gui, x, y, confirm, false, inside(cx.mouse, x, y, 22, 22));
    cx.gui.sprite("container/beacon/confirm", f(x + 2), f(y + 2), 18.0, 18.0, 0xffff_ffff);
    let (x, y) = (gx + 190, gy + 107);
    frame(cx.gui, x, y, true, false, inside(cx.mouse, x, y, 22, 22));
    cx.gui.sprite("container/beacon/cancel", f(x + 2), f(y + 2), 18.0, 18.0, 0xffff_ffff);
}

fn beacon_tooltip(cx: &mut Ctx<'_, '_>, c: &Container, m: (f32, f32)) {
    let (gx, gy) = (cx.gx, cx.gy);
    let w = &c.extra.work;
    // Beacon buttons: widget tooltips (menu positioner, cut at 170 px).
    let widget_tip = |cx: &mut Ctx<'_, '_>, l: Vec<StyledChar>, x: i32, y: i32| {
        let lines = rc_text::wrap(cx.gui, &l, 170.0);
        crate::widgets::draw_tooltip(cx.gui, cx.queue, &lines, (x, y, 22, 22), Some(m));
    };
    if inside(Some(m), gx + 164, gy + 107, 22, 22) {
        let l = crate::special::tr(cx.gui, "gui.done");
        widget_tip(cx, l, gx + 164, gy + 107);
        return;
    }
    if inside(Some(m), gx + 190, gy + 107, 22, 22) {
        let l = crate::special::tr(cx.gui, "gui.cancel");
        widget_tip(cx, l, gx + 190, gy + 107);
        return;
    }
    for b in beacon_buttons(w, gx, gy) {
        if !inside(Some(m), b.x, b.y, 22, 22) {
            continue;
        }
        let name = if b.upgrade { b.effect.map_or("", |e| effect_name(w, e)) } else { b.name };
        let mut c = rc_text::Component::translate(format!("effect.minecraft.{name}"), vec![]);
        if b.upgrade {
            c.extra.push(rc_text::Component::text(" II"));
        }
        let l = crate::special::chars(cx.gui, &c);
        widget_tip(cx, l, b.x, b.y);
        return;
    }
}

// ---------------------------------------------------------------------------------------------
// Application side: inputs and tick.

impl crate::App {
    fn work_origin(&self, s: Sp) -> Option<(i32, i32)> {
        let r = self.renderer.as_ref()?;
        let (w, h) = crate::menu_special::image_size(s);
        Some(((r.gui.width as i32 - w) / 2, (r.gui.height as i32 - h) / 2))
    }

    fn send_button(&self, button: i32) {
        let Some(c) = &self.inventory.container else { return };
        let link = &self.session.link;
        let w = c.window;
        let _ = link
            .send_raw(link.ids.container_button_click, |b| rc_proto_v777::special::write_button_click(b, w, button));
    }

    pub(crate) fn work_press(&mut self, s: Sp, m: (f32, f32), b: u8) -> bool {
        let Some((gx, gy)) = self.work_origin(s) else { return false };
        let Some(c) = &mut self.inventory.container else { return false };
        if b != 0 && s != Sp::Crafter {
            return false;
        }
        match s {
            Sp::Stonecutter => {
                if c.slots.first().is_none_or(Option::is_none) {
                    return false;
                }
                let w = &mut c.extra.work;
                // The 12 cells respond even when empty (index out of the list): sound and packet
                // still leave, as in the game; only the already chosen recipe stays silent.
                let start = sc_start(w);
                let cell = (0..12).find(|&j| {
                    let (x, y) = (gx + 52 + 16 * (j % 4) as i32, gy + 14 + 18 * (j / 4) as i32);
                    inside(Some(m), x, y, 16, 18)
                });
                if let Some(j) = cell
                    && (start + j) as i32 != c.props[0]
                {
                    self.play_ui("minecraft:ui.stonecutter.select_recipe", 1.0);
                    self.send_button((start + j) as i32);
                    return true;
                }
                if inside(Some(m), gx + 119, gy + 9, 12, 54) {
                    w.dragging = true;
                    return true;
                }
                false
            }
            Sp::Loom => {
                let w = &mut c.extra.work;
                if !w.show_patterns {
                    return false;
                }
                if let Some(i) = pattern_at(w, gx, gy, m) {
                    self.play_ui("minecraft:ui.loom.select_pattern", 1.0);
                    self.send_button(i as i32);
                    return true;
                }
                // Grab of the bar: the click then continues to the slots (nothing underneath).
                if inside(Some(m), gx + 119, gy + 9, 12, 56) {
                    w.dragging = true;
                    return true;
                }
                false
            }
            Sp::Enchant => {
                for i in 0..3usize {
                    if inside(Some(m), gx + 60, gy + 14 + 19 * i as i32, 108, 19) {
                        if enchant_available(c, i) {
                            self.send_button(i as i32);
                        }
                        return true;
                    }
                }
                false
            }
            Sp::Beacon => self.beacon_press(gx, gy, m),
            Sp::Crafter => self.crafter_press(gx, gy, m, b),
            _ => false,
        }
    }

    fn beacon_press(&mut self, gx: i32, gy: i32, m: (f32, f32)) -> bool {
        let Some(c) = &mut self.inventory.container else { return false };
        let levels = c.props[0];
        let pay = c.slots.first().is_some_and(Option::is_some);
        let w = &mut c.extra.work;
        if inside(Some(m), gx + 164, gy + 107, 22, 22) {
            if pay && w.primary.is_some() {
                let (p, s) = (w.primary, w.secondary);
                self.play_ui(crate::widgets::CLICK_SOUND, 1.0);
                let link = &self.session.link;
                let _ = link.send_raw(link.ids.set_beacon, |b| rc_proto_v777::special::write_set_beacon(b, p, s));
                self.close_screen();
            }
            return true;
        }
        if inside(Some(m), gx + 190, gy + 107, 22, 22) {
            self.play_ui(crate::widgets::CLICK_SOUND, 1.0);
            self.close_screen();
            return true;
        }
        let buttons = beacon_buttons(w, gx, gy);
        for (i, b) in buttons.iter().enumerate() {
            if !inside(Some(m), b.x, b.y, 22, 22) {
                continue;
            }
            let (active, selected) = beacon_state(w, levels, b, beacon_rank(i));
            if active && !selected {
                if b.primary {
                    w.primary = b.effect;
                    if w.secondary != w.primary {
                        w.secondary = None;
                    }
                } else {
                    w.secondary = b.effect;
                }
            }
            if active {
                self.play_ui(crate::widgets::CLICK_SOUND, 1.0);
            }
            return true;
        }
        false
    }

    /// Crafter (§ 3.9): toggle of an empty grid slot, then the normal click follows.
    fn crafter_press(&mut self, gx: i32, gy: i32, m: (f32, f32), b: u8) -> bool {
        if self.env.game_mode == 3 || b != 0 {
            return false;
        }
        let carried_empty = self.inventory.carried.is_none();
        let Some(c) = &mut self.inventory.container else { return false };
        let n = c.slots.len();
        let Some(i) = (0..9).find(|&i| {
            let d = crate::menu_special::slot_def(Sp::Crafter, i, n);
            inside(Some(m), gx + d.x - 1, gy + d.y - 1, 18, 18)
        }) else {
            return false;
        };
        if c.slots[i].is_some() {
            return false;
        }
        let disabled = c.props[i] == 1;
        if disabled || carried_empty {
            self.crafter_toggle(i, disabled);
        }
        false
    }

    /// Toggle of slot `i` of the crafter grid (`disabled`: state before): local state,
    /// packet, button sound (1.0 when re-enabling, 0.75 when disabling).
    fn crafter_toggle(&mut self, i: usize, disabled: bool) {
        let Some(c) = &mut self.inventory.container else { return };
        c.props[i] = i32::from(!disabled);
        let window = c.window;
        let link = &self.session.link;
        let _ = link.send_raw(link.ids.container_slot_state_changed, |w| {
            rc_proto_v777::special::write_slot_state(w, i as i32, window, disabled)
        });
        let pos = self.player.pos;
        if let Some(a) = self.audio.get() {
            let pitch = if disabled { 1.0 } else { 0.75 };
            a.play(crate::widgets::CLICK_SOUND, rc_audio::Category::Player, Some(pos), 0.4, pitch, None);
        }
    }

    /// Crafter, swap key (`digit`: hotbar slot 0-8, or 40 for the off
    /// hand) on an empty, disabled slot of the grid: re-enabled if the inventory slot
    /// holds an item; the normal swap follows.
    pub(crate) fn crafter_swap_key(&mut self, m: (f32, f32), digit: i8) {
        if self.env.game_mode == 3 {
            return;
        }
        let Some((gx, gy)) = self.work_origin(Sp::Crafter) else { return };
        let Some(c) = &self.inventory.container else { return };
        if !matches!(c.kind, crate::menu::Kind::Special(Sp::Crafter)) {
            return;
        }
        let n = c.slots.len();
        let Some(i) = (0..9).find(|&i| {
            let d = crate::menu_special::slot_def(Sp::Crafter, i, n);
            inside(Some(m), gx + d.x - 1, gy + d.y - 1, 18, 18)
        }) else {
            return;
        };
        let held = if digit == 40 { self.inventory.offhand() } else { self.inventory.hotbar(digit as usize) };
        let carried = self.inventory.carried.is_some();
        if crafter_reenables(carried, c.slots[i].is_some(), c.props[i] == 1, held.is_some()) {
            self.crafter_toggle(i, true);
        }
    }

    pub(crate) fn work_release(&mut self, _s: Sp, _m: (f32, f32), _b: u8) -> bool {
        let Some(c) = &mut self.inventory.container else { return false };
        std::mem::take(&mut c.extra.work.dragging)
    }

    pub(crate) fn work_motion(&mut self, s: Sp, m: (f32, f32)) {
        let Some((_, gy)) = self.work_origin(s) else { return };
        let Some(c) = &mut self.inventory.container else { return };
        let w = &mut c.extra.work;
        if !w.dragging {
            return;
        }
        // The bar follows the mouse only if it is active (more elements than slots).
        w.scroll = match s {
            Sp::Stonecutter if w.recipes.len() > 12 => ((m.1 - gy as f32 - 14.0 - 7.5) / 39.0).clamp(0.0, 1.0),
            Sp::Loom if w.show_patterns && w.patterns.len() > 16 => {
                ((m.1 - gy as f32 - 13.0 - 7.5) / 41.0).clamp(0.0, 1.0)
            }
            _ => w.scroll,
        };
    }

    pub(crate) fn work_wheel(&mut self, s: Sp, _m: (f32, f32), dy: f64) -> bool {
        let Some(c) = &mut self.inventory.container else { return false };
        let w = &mut c.extra.work;
        let rows = match s {
            Sp::Stonecutter if w.recipes.len() > 12 => sc_rows(w),
            Sp::Loom if w.show_patterns && w.patterns.len() > 16 => loom_rows(w),
            _ => return false,
        };
        w.scroll = (w.scroll - dy as f32 / rows.max(1) as f32).clamp(0.0, 1.0);
        true
    }

    /// Screen tick: input changes, animations, anvil field, beacon.
    pub(crate) fn work_tick(&mut self, s: Sp) {
        let level = self.hud.experience_level();
        let creative = self.env.game_mode == 1;
        let effects = &self.hud_extra.effect_names;
        let it_names = &self.item_names;
        let anvil_name = (s == Sp::Anvil)
            .then(|| self.inventory.container.as_ref()?.slots.first()?.as_ref().map(|st| self.item_display_name(st)))
            .flatten();
        let Some(c) = &mut self.inventory.container else { return };
        let w = &mut c.extra.work;
        w.level = level;
        w.creative = creative;
        if w.effects.is_empty() {
            w.effects = effects.clone();
        }
        let inputs: Vec<Option<ItemStack>> = c.slots.iter().take(3).cloned().collect();
        let changed = inputs != w.seen;
        let first_changed = inputs.first() != w.seen.first();
        let same_item = inputs.first().and_then(Option::as_ref).map(|s| s.item)
            == w.seen.first().and_then(Option::as_ref).map(|s| s.item);
        w.seen = inputs;
        let name = |s: &ItemStack| {
            let n = usize::try_from(s.item).ok().and_then(|i| it_names.get(i)).map_or("", String::as_str);
            n.strip_prefix("minecraft:").unwrap_or(n).to_owned()
        };
        match s {
            Sp::Stonecutter if !same_item => {
                w.recipes = c.slots[0].as_ref().map_or_else(Vec::new, |s| crate::menu_special::stonecutter_for(s.item));
                w.scroll = 0.0;
            }
            Sp::Smithing => {
                let t = c.slots[0].as_ref().map(name);
                let [a, b, cc] = smithing_icons(t.as_deref());
                w.icons[0].set(a);
                w.icons[1].set(b);
                w.icons[2].set(cc);
                for i in &mut w.icons {
                    i.tick();
                }
            }
            Sp::Enchant => {
                let offers = c.props[..3].iter().any(|&v| v != 0);
                if w.book.seed == 0 {
                    w.book.seed = 0x9e37_79b9;
                }
                w.book.tick(first_changed, offers);
            }
            Sp::Anvil => {
                if first_changed || w.name.is_none() {
                    let mut t = crate::widgets::TextField::new(Vec::new());
                    t.max_len = 50;
                    t.bordered = false;
                    t.text_color = 0xffff_ffff;
                    t.uneditable_color = 0xffff_ffff;
                    t.invert_highlight = false;
                    let display = anvil_name.unwrap_or_default();
                    t.set_value(&display);
                    w.sent_name = display;
                    w.name = Some(t);
                }
            }
            Sp::Beacon => {
                let p = [c.props[0], c.props[1], c.props[2]];
                if p != w.beacon_props {
                    w.beacon_props = p;
                    w.primary = (p[1] > 0).then_some(p[1] - 1);
                    w.secondary = (p[2] > 0).then_some(p[2] - 1);
                }
            }
            _ => {}
        }
        let _ = changed;
        if s == Sp::Loom {
            self.loom_refresh(changed);
        }
    }

    /// Displayed name of a stack (plain text).
    fn item_display_name(&self, st: &ItemStack) -> String {
        let info = usize::try_from(st.item).ok().and_then(|i| self.data.items.get(i));
        let view =
            crate::stack::StackRef { stack: st, info, ids: &self.component_ids, data: &self.data, hold: crate::stack::Hold::NONE };
        let lang = self.renderer.as_ref().map(|r| &r.gui.lang);
        match lang {
            Some(l) => rc_text::plain_string(&view.display_name(), l),
            None => String::new(),
        }
    }

    fn loom_refresh(&mut self, changed: bool) {
        if !changed {
            return;
        }
        let Some(c) = &self.inventory.container else { return };
        let (banner, dye, pat) = (c.slots[0].clone(), c.slots[1].clone(), c.slots[2].clone());
        let env = crate::stack::HudEnv::empty();

        let it = crate::hud::HotbarItems {
            inv: &self.inventory,
            ids: &self.component_ids,
            data: &self.data,
            infos: &self.data.items,
            names: &self.item_names,
            env: &env,
            pt: 0.0,
        };
        let layers = banner.as_ref().map_or(0, |b| banner_layers(&it, b).len());
        let list = if banner.is_some() && dye.is_some() { loom_patterns(&it, pat.as_ref()) } else { Vec::new() };
        let show = banner.is_some() && dye.is_some() && layers < 6 && !list.is_empty();
        if let Some(c) = &mut self.inventory.container {
            let w = &mut c.extra.work;
            if w.patterns != list {
                w.scroll = 0.0;
            }
            w.patterns = list;
            w.show_patterns = show;
            // A single proposal: chosen by default.
            if show && w.patterns.len() == 1 && c.props[0] != 0 {
                c.props[0] = 0;
                self.send_button(0);
            }
        }
    }

    /// Anvil name field (permanent focus): true if the key was used.
    pub(crate) fn anvil_key(&mut self, k: KeyCode, text: Option<&str>) -> bool {
        let mods: ModifiersState = self.mods;
        let Some(c) = &mut self.inventory.container else { return false };
        if k == KeyCode::Escape || c.slots.first().is_none_or(Option::is_none) {
            return false;
        }
        let w = &mut c.extra.work;
        let Some(t) = &mut w.name else { return false };
        let (used, mut edited) = t.key(k, mods);
        if !used && let Some(s) = text.filter(|s| s.chars().all(|ch| !ch.is_control())) {
            let before = t.value();
            t.insert(s);
            edited |= t.value() != before;
        }
        if edited {
            let v = t.value();
            let st = c.slots[0].clone();
            let default = st.as_ref().filter(|s| !self.has_custom_name(s)).map(|s| self.item_display_name(s));
            let send = if default.as_deref() == Some(v.as_str()) { String::new() } else { v };
            if let Some(c) = &mut self.inventory.container
                && send != c.extra.work.sent_name
            {
                c.extra.work.sent_name = send.clone();
                let link = &self.session.link;
                let _ = link.send_raw(link.ids.rename_item, |w| rc_proto_v777::special::write_rename_item(w, &send));
            }
        }
        // As long as slot 0 is filled, the field keeps all the keys except Escape.
        true
    }

    fn has_custom_name(&self, st: &ItemStack) -> bool {
        self.component_ids.of("minecraft:custom_name").is_some_and(|id| st.component(id).is_some())
    }
}

/// Swap key on a slot of the crafter grid: re-enabled only if it is
/// empty, disabled, and the key's inventory slot holds an item; never with an item
/// on the cursor (the game then swaps nothing).
fn crafter_reenables(carried: bool, occupied: bool, disabled: bool, held: bool) -> bool {
    !carried && !occupied && disabled && held
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1-9 on a disabled slot with an item on the hotbar: re-enabled (the old code had
    /// no case); without an item, active or occupied slot: nothing.
    #[test]
    fn crafter_swap_reenables_disabled_slot() {
        assert!(crafter_reenables(false, false, true, true));
        assert!(!crafter_reenables(false, false, true, false));
        assert!(!crafter_reenables(false, false, false, true));
        assert!(!crafter_reenables(false, true, true, true));
        // Item on the cursor: no swap, hence nothing re-enabled.
        assert!(!crafter_reenables(true, false, true, true));
    }

    #[test]
    fn book_flip_moves_toward_target_and_opens() {
        let mut b = BookAnim { seed: 1, ..Default::default() };
        b.tick(true, true);
        assert!((b.flip - b.target).abs() > 0.0);
        for _ in 0..10 {
            b.tick(false, true);
        }
        assert_eq!(b.open, 1.0);
        for _ in 0..10 {
            b.tick(false, false);
        }
        assert_eq!(b.open, 0.0);
    }

    #[test]
    fn smithing_icons_follow_template() {
        let [_, b, a] = smithing_icons(Some("netherite_upgrade_smithing_template"));
        assert_eq!((b.len(), a.len()), (11, 1));
        let [_, b, a] = smithing_icons(Some("coast_armor_trim_smithing_template"));
        assert_eq!((b.len(), a.len()), (4, 7));
    }

    #[test]
    fn cycle_keeps_tick_count_across_list_changes() {
        let mut c = Cycle::default();
        c.set(vec!["a", "b"]);
        for _ in 0..17 {
            c.tick();
        }
        // New list: the index restarts at 0, the cadence does not (switch at tick 30, not 47).
        c.set(vec!["c", "d", "e"]);
        assert_eq!((c.index, c.ticks), (0, 17));
        for _ in 0..13 {
            c.tick();
        }
        assert_eq!(c.index, 1);
    }

    #[test]
    fn galactic_names_match_reference() {
        // Outputs of the reference Java generator (enchantment seed, 3 options in a row).
        let cases: [(i32, [&str; 3]); 3] = [
            (0, ["scrolls humanoid fire earth", "elder niktu bless", "cold spirit imbue curse"]),
            (12345, ["mglwnafh shorten undead", "snuff grow shrink fiddle", "cold physical darkness"]),
            (-987_654_321, ["air physical scrolls shrink", "range ball scrolls", "curse fhtagn enchant"]),
        ];
        for (seed, want) in cases {
            let mut rng = rc_audio::JavaRandom::new(i64::from(seed));
            let got: Vec<String> = (0..3).map(|_| galactic(&mut rng)).collect();
            assert_eq!(got, want, "seed {seed}");
        }
    }

    #[test]
    fn map_preview_affine_composes_like_a_pose_stack() {
        // Translation then scale: the point (1, 0) goes to (x + s, y).
        let m = affine_mul(affine_translate(10.0, 20.0), affine_scale(0.5));
        assert_eq!(m, [0.5, 0.0, 0.0, 0.5, 10.0, 20.0]);
        let m = affine_mul(m, affine_translate(4.0, 2.0));
        assert_eq!((m[4], m[5]), (12.0, 21.0));
    }
}
