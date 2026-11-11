//! Spectator menu (`docs/specs/end-sleep-spectator.md` § 3): bar in place of the
//! hotbar, opened by a 1–9 key or the middle button; categories "teleport to a
//! player" and "to a team member", pages of 6 (7 on the first); a slot is
//! selected on the first press and used on the second; fade after 5 s without action.
//! Teleportation: `teleport_to_entity` packet (player UUID).

use hashbrown::HashMap;
use std::time::Instant;

use rc_text::{Component, StyledChar};
use winit::event::{ElementState, MouseButton, WindowEvent};

use crate::gui::Gui;
use crate::kept_text::{Kept, misc_screen_caches_on};
use crate::players::PlayerList;
use crate::scoreboard::Scoreboard;

const FADE_DELAY_MS: f32 = 5000.0;
const FADE_TIME_MS: f32 = 2000.0;
/// RGB colors of the 16 team colors.
const TEAM_RGB: [u32; 16] = [
    0x000000, 0x0000AA, 0x00AA00, 0x00AAAA, 0xAA0000, 0xAA00AA, 0xFFAA00, 0xAAAAAA, 0x555555, 0x5555FF, 0x55FF55,
    0x55FFFF, 0xFF5555, 0xFF55FF, 0xFFFF55, 0xFFFFFF,
];

#[derive(Clone, Debug, PartialEq)]
pub struct PlayerItem {
    pub uuid: u128,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TeamItem {
    pub name: Component,
    pub color: Option<u32>,
    pub players: Vec<PlayerItem>,
    /// Player whose face serves as the icon (drawn at random on creation).
    pub icon: u128,
}

#[derive(Clone, Debug, PartialEq)]
enum Category {
    Root { players: Vec<PlayerItem>, teams: Vec<TeamItem> },
    Players(Vec<PlayerItem>),
    Teams(Vec<TeamItem>),
}

/// Slot of a page.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Empty,
    Close,
    Scroll { dir: i32, enabled: bool },
    TeleportPlayers { enabled: bool },
    TeleportTeams { enabled: bool },
    Player(PlayerItem),
    Team(TeamItem),
}

impl Item {
    fn as_ref(&self) -> ItemRef<'_> {
        match self {
            Item::Empty => ItemRef::Empty,
            Item::Close => ItemRef::Close,
            Item::Scroll { dir, enabled } => ItemRef::Scroll { dir: *dir, enabled: *enabled },
            Item::TeleportPlayers { enabled } => ItemRef::TeleportPlayers { enabled: *enabled },
            Item::TeleportTeams { enabled } => ItemRef::TeleportTeams { enabled: *enabled },
            Item::Player(p) => ItemRef::Player(p),
            Item::Team(t) => ItemRef::Team(t),
        }
    }

    /// Usable slot ([`ItemRef::enabled`]).
    fn enabled(&self, spectators: &[u128]) -> bool {
        self.as_ref().enabled(spectators)
    }
}

/// Slot of a page read in place: an [`Item`] without the copy of the player (name) or of the
/// team (name, members).
#[derive(Clone, Copy, Debug, PartialEq)]
enum ItemRef<'a> {
    Empty,
    Close,
    Scroll { dir: i32, enabled: bool },
    TeleportPlayers { enabled: bool },
    TeleportTeams { enabled: bool },
    Player(&'a PlayerItem),
    Team(&'a TeamItem),
}

impl ItemRef<'_> {
    fn to_owned(self) -> Item {
        match self {
            ItemRef::Empty => Item::Empty,
            ItemRef::Close => Item::Close,
            ItemRef::Scroll { dir, enabled } => Item::Scroll { dir, enabled },
            ItemRef::TeleportPlayers { enabled } => Item::TeleportPlayers { enabled },
            ItemRef::TeleportTeams { enabled } => Item::TeleportTeams { enabled },
            ItemRef::Player(p) => Item::Player(p.clone()),
            ItemRef::Team(t) => Item::Team(t.clone()),
        }
    }

    /// Usable slot; a player is only while not in spectator mode now (`spectators`: players
    /// currently in that mode), even if they were not when the menu was built.
    fn enabled(self, spectators: &[u128]) -> bool {
        match self {
            ItemRef::Empty => false,
            ItemRef::Player(p) => !spectators.contains(&p.uuid),
            ItemRef::Close | ItemRef::Team(_) => true,
            ItemRef::Scroll { enabled, .. }
            | ItemRef::TeleportPlayers { enabled }
            | ItemRef::TeleportTeams { enabled } => enabled,
        }
    }

    fn name_key(self) -> Option<&'static str> {
        Some(match self {
            ItemRef::Close => "spectatorMenu.close",
            ItemRef::Scroll { dir, .. } if dir < 0 => "spectatorMenu.previous_page",
            ItemRef::Scroll { .. } => "spectatorMenu.next_page",
            ItemRef::TeleportPlayers { .. } => "spectatorMenu.teleport",
            ItemRef::TeleportTeams { .. } => "spectatorMenu.team_teleport",
            _ => return None,
        })
    }
}

/// Effect of choosing a slot.
#[derive(Debug, PartialEq)]
pub enum Action {
    None,
    Teleport(u128),
}

impl Category {
    fn items_len(&self) -> usize {
        match self {
            Category::Root { .. } => 2,
            Category::Players(p) => p.len(),
            Category::Teams(t) => t.len(),
        }
    }

    fn entry(&self, i: usize) -> ItemRef<'_> {
        match self {
            Category::Root { players, teams } => match i {
                0 => ItemRef::TeleportPlayers { enabled: !players.is_empty() },
                1 => ItemRef::TeleportTeams { enabled: !teams.is_empty() },
                _ => ItemRef::Empty,
            },
            Category::Players(p) => p.get(i).map_or(ItemRef::Empty, ItemRef::Player),
            Category::Teams(t) => t.get(i).map_or(ItemRef::Empty, ItemRef::Team),
        }
    }

    /// Slot `slot` (0..8) of page `page`: 0 = previous page (following pages), 7 = next page,
    /// 8 = close.
    fn slot(&self, page: i32, slot: i32) -> ItemRef<'_> {
        let index = slot + page * 6;
        if page > 0 && slot == 0 {
            ItemRef::Scroll { dir: -1, enabled: true }
        } else if slot == 7 {
            ItemRef::Scroll { dir: 1, enabled: (index as usize) < self.items_len() }
        } else if slot == 8 {
            ItemRef::Close
        } else if index >= 0 {
            self.entry(index as usize)
        } else {
            ItemRef::Empty
        }
    }

    fn prompt_key(&self) -> &'static str {
        match self {
            Category::Root { .. } => "spectatorMenu.root.prompt",
            Category::Players(_) => "spectatorMenu.teleport.prompt",
            Category::Teams(_) => "spectatorMenu.team_teleport.prompt",
        }
    }

    /// Text shown above the bar: name of the selected slot, otherwise the category's prompt.
    fn action_component(&self, page: i32, selected: i32) -> Component {
        let item = if selected >= 0 { self.slot(page, selected) } else { ItemRef::Empty };
        match item {
            ItemRef::Empty => Component::translate(self.prompt_key(), vec![]),
            ItemRef::Player(p) => Component::text(p.name.clone()),
            ItemRef::Team(t) => t.name.clone(),
            other => Component::translate(other.name_key().unwrap_or(""), vec![]),
        }
    }
}

struct Menu {
    category: Category,
    selected: i32,
    page: i32,
    /// Action text and its width, and the (selected slot, page) they were made for.
    action_text: Kept<(Vec<StyledChar>, i32)>,
    action_text_of: (i32, i32),
}

impl Menu {
    /// Menu of a category, first page, nothing selected.
    fn new(category: Category) -> Menu {
        Menu { category, selected: -1, page: 0, action_text: Kept::default(), action_text_of: (-1, 0) }
    }

    /// Slot `slot` of the current page, read in place.
    fn item_ref(&self, slot: i32) -> ItemRef<'_> {
        self.category.slot(self.page, slot)
    }

    /// Copy of the slot `slot` of the current page.
    fn item(&self, slot: i32) -> Item {
        self.item_ref(slot).to_owned()
    }

    /// Action text and its width; `keep`: flattened and measured once per selected slot and page.
    fn action_text(&mut self, gui: &Gui, keep: bool) -> &(Vec<StyledChar>, i32) {
        let shown = (self.selected, self.page);
        if self.action_text_of != shown {
            self.action_text_of = shown;
            self.action_text.forget();
        }
        let category = &self.category;
        self.action_text.get(gui, keep, |gui| {
            let component = category.action_component(shown.1, shown.0);
            let chars = rc_text::styled_chars(&rc_text::flatten(&component, &gui.lang));
            let width = rc_text::width(gui, &chars);
            (chars, width)
        })
    }
}

/// Atlas sprites reserved for the faces of the nine slots.
const FACE_SPRITES: [&str; rc_assets::gui::FACE_SLOTS] = [
    "dyn/face/0",
    "dyn/face/1",
    "dyn/face/2",
    "dyn/face/3",
    "dyn/face/4",
    "dyn/face/5",
    "dyn/face/6",
    "dyn/face/7",
    "dyn/face/8",
];

#[derive(Default)]
pub struct SpectatorUi {
    menu: Option<Menu>,
    last_selection: Option<Instant>,
    /// 8 × 8 faces (face then hat on top) of the skins received, by UUID.
    faces: HashMap<u128, Vec<u8>>,
    /// UUID written into each `dyn/face/<i>` slot.
    slots: Vec<Option<u128>>,
    /// Font identity of the interface whose atlas the faces were written in.
    slots_interface: Option<usize>,
    /// Label of each slot's key (assigned key, then ready text): recomputed when the
    /// assignment changes, emptied on every menu opening (language or layout changed).
    key_labels: Vec<(String, Vec<StyledChar>)>,
    /// Players in spectator mode, read again each tick and at each key while the menu is open.
    spectators: Vec<u128>,
}

/// Settings of the slot keys 1–9 (`options.txt`).
pub const HOTBAR_KEY_OPTIONS: [&str; 9] = [
    "key_key.hotbar.1",
    "key_key.hotbar.2",
    "key_key.hotbar.3",
    "key_key.hotbar.4",
    "key_key.hotbar.5",
    "key_key.hotbar.6",
    "key_key.hotbar.7",
    "key_key.hotbar.8",
    "key_key.hotbar.9",
];

/// The game's UUID order: high then low halves compared as signed integers.
fn uuid_key(u: u128) -> (i64, i64) {
    ((u >> 64) as u64 as i64, u as u64 as i64)
}

/// Players listed excluding spectators, sorted by UUID.
fn listed_players(players: &PlayerList, only: Option<&[String]>) -> Vec<PlayerItem> {
    let mut v: Vec<(u128, String)> = players
        .map
        .iter()
        .filter(|(_, p)| p.game_mode != 3)
        .filter(|(_, p)| match only {
            Some(names) => names.contains(&p.name),
            None => p.listed,
        })
        .map(|(u, p)| (*u, p.name.clone()))
        .collect();
    v.sort_by_key(|(u, _)| uuid_key(*u));
    v.into_iter().map(|(uuid, name)| PlayerItem { uuid, name }).collect()
}

/// Teams with at least one known member excluding spectators (order of the team names).
fn team_items(players: &PlayerList, scores: &Scoreboard) -> Vec<TeamItem> {
    let mut names: Vec<&String> = scores.teams.keys().collect();
    names.sort();
    let mut rng = rc_audio::JavaRandom::from_entropy();
    names
        .into_iter()
        .filter_map(|team| {
            let members: Vec<String> =
                scores.members.iter().filter(|(_, t)| *t == team).map(|(h, _)| h.clone()).collect();
            // Members in the team's order, each looked up by name in the player list.
            let found: Vec<PlayerItem> = members
                .iter()
                .filter_map(|n| {
                    players
                        .map
                        .iter()
                        .find(|(_, p)| p.name == *n && p.game_mode != 3)
                        .map(|(u, p)| PlayerItem { uuid: *u, name: p.name.clone() })
                })
                .collect();
            if found.is_empty() {
                return None;
            }
            let icon = found[rng.next_int_bounded(found.len() as i32) as usize].uuid;
            let t = scores.teams.get(team)?;
            let name = t.params.display.as_ref().and_then(rc_text::from_nbt).unwrap_or_else(|| Component::text(team));
            let color = t.params.color.and_then(|c| TEAM_RGB.get(c as usize).copied());
            Some(TeamItem { name, color, players: found, icon })
        })
        .collect()
}

impl SpectatorUi {
    pub fn active(&self) -> bool {
        self.menu.is_some()
    }

    pub fn close(&mut self) {
        self.menu = None;
        self.last_selection = None;
    }

    fn open(&mut self, players: &PlayerList, scores: &Scoreboard) {
        let category = Category::Root { players: listed_players(players, None), teams: team_items(players, scores) };
        self.menu = Some(Menu::new(category));
        self.key_labels.clear();
        self.game_modes(players);
    }

    /// Game modes of the players (menu open): who is in spectator mode now.
    pub fn game_modes(&mut self, players: &PlayerList) {
        if self.menu.is_none() {
            return;
        }
        self.spectators.clear();
        self.spectators.extend(players.map.iter().filter(|(_, p)| p.game_mode == 3).map(|(u, _)| *u));
    }

    /// Label of the key assigned to slot `slot` (`bound`: key name from the settings),
    /// like the controls screen (keyboard layout, otherwise translation).
    #[cfg(test)]
    fn key_label(&mut self, slot: usize, bound: &str, lang: &rc_text::Lang) -> &[StyledChar] {
        slot_key_label(&mut self.key_labels, slot, bound, lang)
    }

    fn select(&mut self, slot: i32) -> Action {
        let Some(m) = &mut self.menu else { return Action::None };
        let item = m.item(slot);
        if item == Item::Empty {
            return Action::None;
        }
        if m.selected != slot || !item.enabled(&self.spectators) {
            m.selected = slot;
            return Action::None;
        }
        match item {
            Item::Close => self.close(),
            Item::Scroll { dir, .. } => m.page += dir,
            Item::TeleportPlayers { .. } | Item::TeleportTeams { .. } => {
                let Category::Root { players, teams } = &m.category else { return Action::None };
                let category = if matches!(item, Item::TeleportPlayers { .. }) {
                    Category::Players(players.clone())
                } else {
                    Category::Teams(teams.clone())
                };
                *m = Menu::new(category);
            }
            // Members of the team in UUID order, like the player category.
            Item::Team(t) => {
                let mut players = t.players;
                players.sort_by_key(|p| uuid_key(p.uuid));
                *m = Menu::new(Category::Players(players));
            }
            Item::Player(p) => return Action::Teleport(p.uuid),
            Item::Empty => {}
        }
        Action::None
    }

    /// Slot key 1–9 (`slot` 0..8).
    pub fn hotbar_key(&mut self, slot: usize, players: &PlayerList, scores: &Scoreboard) -> Action {
        self.last_selection = Some(Instant::now());
        if self.menu.is_some() {
            self.game_modes(players);
            self.select(slot as i32)
        } else {
            self.open(players, scores);
            Action::None
        }
    }

    /// Middle button: uses the selected slot, or opens the menu.
    pub fn action_key(&mut self, players: &PlayerList, scores: &Scoreboard) -> Action {
        self.last_selection = Some(Instant::now());
        self.game_modes(players);
        match &self.menu {
            Some(m) if m.selected != -1 => self.select(m.selected),
            Some(_) => Action::None,
            None => {
                self.open(players, scores);
                Action::None
            }
        }
    }

    /// Wheel (menu open): next active slot in the direction `wheel` (±1).
    pub fn scroll(&mut self, wheel: i32) {
        let Some(m) = &self.menu else { return };
        let mut s = m.selected + wheel;
        while (0..=8).contains(&s) && !m.item_ref(s).enabled(&self.spectators) {
            s += wheel;
        }
        if (0..=8).contains(&s) {
            let _ = self.select(s);
            self.last_selection = Some(Instant::now());
        }
    }

    fn alpha(&self) -> f32 {
        let since = self.last_selection.map_or(f32::MAX, |t| t.elapsed().as_secs_f32() * 1000.0);
        ((FADE_DELAY_MS - since) / FADE_TIME_MS).clamp(0.0, 1.0)
    }

    /// Skin received: face (8, 8) then hat (40, 8) blended on top.
    pub fn skin(&mut self, uuid: u128, image: &rc_assets::texture::Image) {
        if let Some(f) = face_of(image) {
            self.faces.insert(uuid, f);
            self.slots.clear();
        }
    }

    /// Bar and action text; `default_face` gives the default face of a UUID, `bound` the
    /// key assigned to each slot (0..8).
    #[allow(clippy::too_many_arguments)]
    pub fn draw<'o>(
        &mut self,
        gui: &mut Gui,
        queue: &wgpu::Queue,
        backdrop: u32,
        default_face: &dyn Fn(u128) -> Option<Vec<u8>>,
        bound: &dyn Fn(usize) -> &'o str,
    ) {
        if self.menu.is_none() {
            return;
        }
        let alpha = self.alpha();
        if alpha <= 0.0 {
            self.close();
            return;
        }
        self.draw_faded(gui, queue, (backdrop, alpha), default_face, bound, misc_screen_caches_on());
    }

    /// [`SpectatorUi::draw`] at fade `alpha` (0..1]. `keep`: slots read in place and action
    /// text kept between frames; otherwise copied and flattened on every frame (old path).
    fn draw_faded<'o>(
        &mut self,
        gui: &mut Gui,
        queue: &wgpu::Queue,
        (backdrop, alpha): (u32, f32),
        default_face: &dyn Fn(u128) -> Option<Vec<u8>>,
        bound: &dyn Fn(usize) -> &'o str,
        keep: bool,
    ) {
        // An interface rebuilt by a resource reload has a new atlas: no face is written in it yet.
        if self.slots_interface != Some(gui.font_generation()) {
            self.slots_interface = Some(gui.font_generation());
            self.slots.clear();
        }
        self.slots.resize(rc_assets::gui::FACE_SLOTS, None);
        let SpectatorUi { menu, faces, slots, key_labels, spectators, .. } = self;
        let Some(m) = menu else { return };
        let (w, h) = (gui.width as i32, gui.height as i32);
        let a = ((alpha * 255.0) as u32) << 24;
        let white = a | 0xff_ffff;
        let cx = w / 2;
        let y = (h as f32 - 22.0 * alpha).floor() as i32;
        let f = |v: i32| v as f32;
        gui.sprite("hud/hotbar", f(cx - 91), f(y), 182.0, 22.0, white);
        if m.selected >= 0 {
            gui.sprite("hud/hotbar_selection", f(cx - 92 + m.selected * 20), f(y - 1), 24.0, 23.0, white);
        }
        let copies: Vec<Item>;
        let items: [ItemRef<'_>; 9] = if keep {
            std::array::from_fn(|slot| m.item_ref(slot as i32))
        } else {
            copies = (0..9).map(|slot| m.item(slot)).collect();
            std::array::from_fn(|slot| copies[slot].as_ref())
        };
        let mut faces = FaceSlots { faces, written: slots, default_face };
        for (slot, item) in items.into_iter().enumerate() {
            if matches!(item, ItemRef::Empty) {
                continue;
            }
            let x = cx - 90 + slot as i32 * 20 + 2;
            let iy = y + 3;
            let on = item.enabled(spectators);
            let b = if on { 1.0 } else { 0.25 };
            let gray = a | (((b * 255.0) as u32) * 0x01_0101);
            let sprite = |gui: &mut Gui, name: &str| gui.sprite(name, f(x), f(iy), 16.0, 16.0, gray);
            match item {
                ItemRef::Close => sprite(gui, "spectator/close"),
                ItemRef::Scroll { dir, .. } => {
                    sprite(gui, if dir < 0 { "spectator/scroll_left" } else { "spectator/scroll_right" })
                }
                ItemRef::TeleportPlayers { .. } => sprite(gui, "spectator/teleport_to_player"),
                ItemRef::TeleportTeams { .. } => sprite(gui, "spectator/teleport_to_team"),
                ItemRef::Player(p) => faces.draw(gui, queue, slot, p.uuid, (f(x + 2), f(iy + 2)), white),
                ItemRef::Team(t) => {
                    if let Some(rgb) = t.color {
                        let c = |s: u32| (((rgb >> s) & 0xff) as f32 * b) as u32;
                        gui.fill(
                            f(x + 1),
                            f(iy + 1),
                            f(x + 15),
                            f(iy + 15),
                            0xff00_0000 | c(16) << 16 | c(8) << 8 | c(0),
                        );
                    }
                    faces.draw(gui, queue, slot, t.icon, (f(x + 2), f(iy + 2)), gray);
                }
                ItemRef::Empty => {}
            }
            if on {
                let key = slot_key_label(key_labels, slot, bound(slot), &gui.lang);
                let kw = rc_text::width(gui, key);
                gui.text(queue, key, f(x + 19 - 2 - kw), f(iy + 6 + 3), white, true, 0);
            }
        }
        // Action text: name of the selected slot, otherwise the category's prompt.
        let (chars, tw) = m.action_text(gui, keep);
        let tw = *tw;
        let (tx, ty) = ((w - tw) / 2, h - 35);
        if backdrop >> 24 != 0 {
            // Background multiplied by the text color (fade alpha).
            let ba = ((backdrop >> 24) as f32 * alpha) as u32;
            gui.fill(f(tx - 2), f(ty - 2), f(tx + tw + 2), f(ty + 9 + 2), ba << 24 | (backdrop & 0xff_ffff));
        }
        gui.text(queue, chars, f(tx), f(ty), white, true, 0);
    }
}

/// Label of the key assigned to slot `slot` (`bound`: key name from the settings), like the
/// controls screen (keyboard layout, otherwise translation), kept in `labels` while the
/// assignment stays the same.
fn slot_key_label<'l>(
    labels: &'l mut Vec<(String, Vec<StyledChar>)>,
    slot: usize,
    bound: &str,
    lang: &rc_text::Lang,
) -> &'l [StyledChar] {
    if labels.len() <= slot {
        labels.resize_with(9, Default::default);
    }
    let e = &mut labels[slot];
    if e.0 != bound || e.1.is_empty() {
        e.0.clear();
        e.0.push_str(bound);
        e.1 = crate::gui::plain(&crate::options::key_label(lang, bound));
    }
    &e.1
}

/// Faces of the bar: skins received, player written in each atlas slot, default faces.
struct FaceSlots<'a> {
    faces: &'a HashMap<u128, Vec<u8>>,
    written: &'a mut [Option<u128>],
    default_face: &'a dyn Fn(u128) -> Option<Vec<u8>>,
}

impl FaceSlots<'_> {
    /// 12 × 12 face of player `uuid` in the bar slot `slot` (its atlas sprite rewritten if it
    /// holds another face).
    fn draw(&mut self, gui: &mut Gui, queue: &wgpu::Queue, slot: usize, uuid: u128, (x, y): (f32, f32), color: u32) {
        let sprite = FACE_SPRITES[slot];
        if self.written[slot] != Some(uuid) {
            let default;
            let rgba: &[u8] = match self.faces.get(&uuid) {
                Some(face) => face,
                None => {
                    let Some(face) = (self.default_face)(uuid) else { return };
                    default = face;
                    &default
                }
            };
            if !gui.write_sprite(queue, sprite, (8, 8), rgba) {
                return;
            }
            self.written[slot] = Some(uuid);
        }
        gui.sprite(sprite, x, y, 12.0, 12.0, color);
    }
}

impl crate::App {
    pub(crate) fn spectator_act(&mut self, a: Action) {
        if let Action::Teleport(uuid) = a {
            let _ = self.session.link.send_teleport_to_entity(uuid);
        }
    }

    /// Keys 1–9 in spectator (`slot` 0..8).
    pub(crate) fn spectator_hotbar(&mut self, slot: usize) {
        let a = self.spectator.hotbar_key(slot, &self.players, &self.scores);
        self.spectator_act(a);
    }

    /// Spectator in-game input: middle button (selected slot).
    pub(crate) fn spectator_event(&mut self, e: &WindowEvent) -> bool {
        if self.env.game_mode != 3 || !self.grabbed || self.chat.input.is_some() {
            return false;
        }
        match e {
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Middle, .. } => {
                let a = self.spectator.action_key(&self.players, &self.scores);
                self.spectator_act(a);
                true
            }
            // The wheel goes through the common wheel (`game_scroll`): sensitivity, discrete mode
            // and accumulation, then menu or flight speed.
            _ => false,
        }
    }
}

/// Default face of a player without a downloaded skin (default skin according to the UUID).
pub fn default_face(assets: &rc_assets::Assets, uuid: u128) -> Option<Vec<u8>> {
    let skin = crate::entity_render::default_skin(uuid);
    let kind = if skin < 9 { "slim" } else { "wide" };
    let path = format!(
        "assets/minecraft/textures/entity/player/{kind}/{}.png",
        crate::entity_render::SKINS[skin as usize % 9]
    );
    face_of(&assets.image(&path)?)
}

/// 8 × 8 face of a 64 × 64 (or 64 × 32) skin: face (8, 8), hat (40, 8) on top.
pub fn face_of(img: &rc_assets::texture::Image) -> Option<Vec<u8>> {
    if img.width < 48 || img.height < 16 {
        return None;
    }
    let px = |x: u32, y: u32| {
        let i = ((y * img.width + x) * 4) as usize;
        [img.rgba[i], img.rgba[i + 1], img.rgba[i + 2], img.rgba[i + 3]]
    };
    let mut out = Vec::with_capacity(8 * 8 * 4);
    for y in 0..8 {
        for x in 0..8 {
            let f = px(8 + x, 8 + y);
            let h = px(40 + x, 8 + y);
            let ha = u32::from(h[3]);
            let mix = |a: u8, b: u8| ((u32::from(a) * (255 - ha) + u32::from(b) * ha) / 255) as u8;
            out.extend_from_slice(&[mix(f[0], h[0]), mix(f[1], h[1]), mix(f[2], h[2]), 255]);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn menu(n: usize) -> Menu {
        let players = (0..n).map(|i| PlayerItem { uuid: i as u128, name: format!("p{i}") }).collect();
        Menu::new(Category::Players(players))
    }

    #[test]
    fn pages_and_scroll_slots() {
        let mut m = menu(10);
        assert_eq!(m.item(0), Item::Player(PlayerItem { uuid: 0, name: "p0".into() }));
        assert_eq!(m.item(6), Item::Player(PlayerItem { uuid: 6, name: "p6".into() }));
        assert_eq!(m.item(7), Item::Scroll { dir: 1, enabled: true });
        assert_eq!(m.item(8), Item::Close);
        m.page = 1;
        assert_eq!(m.item(0), Item::Scroll { dir: -1, enabled: true });
        assert_eq!(m.item(1), Item::Player(PlayerItem { uuid: 7, name: "p7".into() }));
        assert_eq!(m.item(4), Item::Empty);
        assert_eq!(m.item(7), Item::Scroll { dir: 1, enabled: false });
    }

    #[test]
    fn select_twice_to_use() {
        let mut ui = SpectatorUi { menu: Some(menu(3)), ..Default::default() };
        assert_eq!(ui.select(1), Action::None);
        assert_eq!(ui.select(1), Action::Teleport(1));
        ui.scroll(1);
        assert_eq!(ui.menu.as_ref().map(|m| m.selected), Some(2));
        // Slots 3 to 6 empty, 7 next page inactive: the wheel skips to "close".
        ui.scroll(1);
        assert_eq!(ui.menu.as_ref().map(|m| m.selected), Some(8));
        assert_eq!(ui.select(8), Action::None);
        assert!(!ui.active());
    }

    /// The slot's digit follows the assigned key (slot 1 reassigned to F1: "F1", not "1"),
    /// and follows a reassignment made with the menu open.
    #[test]
    fn slot_label_follows_binding() {
        let text = |c: &[StyledChar]| c.iter().map(|s| s.c).collect::<String>();
        let mut lang = rc_text::Lang::default();
        lang.map.insert("key.keyboard.f1".into(), "F1".into());
        lang.map.insert("key.keyboard.unknown".into(), "Not Bound".into());
        let mut ui = SpectatorUi::default();
        assert_eq!(text(ui.key_label(0, "key.keyboard.f1", &lang)), "F1");
        assert_eq!(text(ui.key_label(0, "key.keyboard.unknown", &lang)), "Not Bound");
        assert_eq!(text(ui.key_label(8, "key.keyboard.f1", &lang)), "F1");
    }

    /// A player who switched to spectator mode after the menu opened can be selected but not
    /// used; the members of a team are listed in UUID order.
    #[test]
    fn live_spectators_and_team_order() {
        let mut ui = SpectatorUi { menu: Some(menu(3)), spectators: vec![1], ..Default::default() };
        assert_eq!(ui.select(1), Action::None);
        assert_eq!(ui.select(1), Action::None, "now spectating: no teleport");
        ui.spectators.clear();
        assert_eq!(ui.select(1), Action::Teleport(1));
        let p = |u: u128| PlayerItem { uuid: u, name: format!("p{u}") };
        let team = TeamItem { name: Component::text("t"), color: None, players: vec![p(9), p(1u128 << 127), p(4)], icon: 9 };
        ui.menu = Some(Menu { selected: 0, ..Menu::new(Category::Teams(vec![team])) });
        assert_eq!(ui.select(0), Action::None);
        let m = ui.menu.as_ref().expect("menu");
        assert_eq!((m.item(0), m.item(1), m.item(2)), (Item::Player(p(1u128 << 127)), Item::Player(p(4)), Item::Player(p(9))));
    }

    /// What happens before a frame of the bar test.
    enum Step {
        /// Slot key pressed.
        Press(i32),
        /// Back to the root menu.
        Root,
        /// These players are now spectating.
        Spectating(Vec<u128>),
        Fade(f32),
        OtherLanguage,
    }

    /// Bar of ten players and two teams drawn with its slots read in place and its action text
    /// kept, against slots copied and text flattened on every frame: layers equal to the byte
    /// through the root, both pages of the players, the teams and the members of a team, with
    /// slots selected, players gone spectating, a fading bar and another language. Each step is
    /// drawn twice (the second frame reuses what the first kept). Without an installed game or a
    /// GPU: skipped (`RUSTCRAFT_OPT_REQUIRE=1`: failure).
    #[test]
    fn kept_bar_draws_the_layers_of_copied_slots() {
        use crate::gui::tests_opt;
        let Some(e) = tests_opt::env() else { return };
        let player = |u: u128| PlayerItem { uuid: u, name: format!("Player{u}") };
        let players: Vec<PlayerItem> = (1..=10).map(player).collect();
        let team = |name: &str, color: Option<u32>, members: &[u128]| TeamItem {
            name: Component::text(name),
            color,
            players: members.iter().map(|&u| player(u)).collect(),
            icon: members[0],
        };
        let teams = vec![team("Red team", Some(0xAA_0000), &[3, 1, 2]), team("No colour", None, &[9])];
        let root = || Menu::new(Category::Root { players: players.clone(), teams: teams.clone() });
        let mut uis = [false, true].map(|_| SpectatorUi { menu: Some(root()), ..Default::default() });
        let mut guis = [false, true].map(|_| tests_opt::gui(&e, tests_opt::ALL));
        let base = e.assets.read("assets/minecraft/lang/en_us.json").unwrap_or_default();
        let french = r#"{"spectatorMenu.teleport.prompt": "Choisissez un joueur", "spectatorMenu.next_page": "Page suivante"}"#;
        use Step::{Fade, OtherLanguage, Press, Root, Spectating};
        let steps = [
            Fade(1.0),
            // Root: both entries selected in turn, then the players opened.
            Press(1),
            Press(0),
            Press(0),
            Press(3),
            Spectating(vec![4, 2]),
            // Second page and back: slot 0 is "previous page" there, the first player here.
            Press(7),
            Press(7),
            Press(2),
            Press(0),
            Fade(0.5),
            Press(0),
            OtherLanguage,
            Press(7),
            Fade(0.02),
            // Teams, then the members of the first team (in UUID order).
            Root,
            Fade(1.0),
            Press(1),
            Press(1),
            Press(0),
            Spectating(Vec::new()),
            Press(0),
            Press(1),
            Press(5),
        ];
        let keys: [String; 9] = std::array::from_fn(|slot| format!("key.keyboard.{}", slot + 1));
        let default_face = |uuid: u128| Some(vec![(uuid as u8).wrapping_mul(37); 8 * 8 * 4]);
        let mut alpha = 1.0;
        let mut seen = hashbrown::HashSet::new();
        for (n, step) in steps.iter().enumerate() {
            for (ui, gui) in uis.iter_mut().zip(&mut guis) {
                match step {
                    Press(slot) => drop(ui.select(*slot)),
                    Root => ui.menu = Some(root()),
                    Spectating(who) => ui.spectators = who.clone(),
                    Fade(a) => alpha = *a,
                    OtherLanguage => {
                        gui.lang = std::sync::Arc::new(rc_text::Lang::load(&base, Some(french.as_bytes()), None));
                    }
                }
            }
            for repeat in 0..2 {
                let mut layers = Vec::new();
                for (keep, (ui, gui)) in uis.iter_mut().zip(&mut guis).enumerate() {
                    gui.begin(1600, 900);
                    let bound = |slot: usize| keys[slot].as_str();
                    ui.draw_faded(gui, &e.queue, (0x8000_0000, alpha), &default_face, &bound, keep == 1);
                    layers.push(tests_opt::layer_bytes(gui));
                }
                let first = layers[0].iter().zip(&layers[1]).position(|(a, b)| a != b);
                assert!(
                    layers[0].len() == layers[1].len() && first.is_none(),
                    "step {n}, frame {repeat}: layers differ (byte {first:?})"
                );
                seen.insert(layers.swap_remove(0));
            }
        }
        assert!(seen.len() >= 15, "only {} distinct frames in {} steps", seen.len(), steps.len());
        let menus = uis.each_ref().map(|ui| ui.menu.as_ref().map(|m| (m.selected, m.page, m.category.clone())));
        assert!(menus[0].is_some() && menus[0] == menus[1], "both bars end in the same menu");
    }

    /// Micro-bench of a frame of the bar (loaded machine possible): first page of ten players
    /// with one selected, then two teams of five with one selected; slots copied and action text
    /// flattened on every frame against slots read in place and text kept. Time and bytes
    /// allocated per frame. Main thread by nature. Without a GPU: skipped.
    /// `cargo test --release -p rc-app bench_spectator_bar -- --ignored --nocapture`
    #[test]
    #[ignore = "micro-bench"]
    fn bench_spectator_bar() {
        use crate::gui::tests_opt;
        let Some(e) = tests_opt::env() else { return };
        let player = |u: u128| PlayerItem { uuid: u, name: format!("Player{u}") };
        let players: Vec<PlayerItem> = (1..=10).map(player).collect();
        let team = |k: u128| TeamItem {
            name: Component::text(format!("Team {k}")),
            color: Some(0x55_FF55),
            players: (0..5).map(|m| player(k * 5 + m)).collect(),
            icon: k * 5,
        };
        let categories = [("players", Category::Players(players)), ("teams", Category::Teams(vec![team(1), team(2)]))];
        let keys: [String; 9] = std::array::from_fn(|slot| format!("key.keyboard.{}", slot + 1));
        let default_face = |uuid: u128| Some(vec![(uuid as u8).wrapping_mul(37); 8 * 8 * 4]);
        let bound = |slot: usize| keys[slot].as_str();
        let mut gui = tests_opt::gui(&e, tests_opt::ALL);
        for pass in 0..2 {
            for (label, category) in &categories {
                for (name, keep) in [("copied", false), ("in place", true)] {
                    let mut ui = SpectatorUi { menu: Some(Menu { selected: 1, ..Menu::new(category.clone()) }), ..Default::default() };
                    let mut once = |gui: &mut Gui| {
                        gui.begin(1600, 900);
                        ui.draw_faded(gui, &e.queue, (0x8000_0000, 1.0), &default_face, &bound, keep);
                    };
                    (0..200).for_each(|_| once(&mut gui));
                    let frames = 10_000;
                    let t = Instant::now();
                    (0..frames).for_each(|_| once(&mut gui));
                    let per_frame = t.elapsed().as_secs_f64() * 1e6 / f64::from(frames);
                    let bytes = crate::debug_sys::allocated_bytes(|| (0..100).for_each(|_| once(&mut gui))) / 100;
                    println!(
                        "spectator bar, pass {pass}, {label:<7} slots {name:<8}: {per_frame:>6.2} µs, {bytes:>5} bytes allocated per frame"
                    );
                }
            }
        }
    }

    #[test]
    fn uuid_order_is_signed() {
        let mut v = [1u128 << 127, 5u128];
        v.sort_by_key(|u| uuid_key(*u));
        assert_eq!(v, [1u128 << 127, 5]);
    }
}
