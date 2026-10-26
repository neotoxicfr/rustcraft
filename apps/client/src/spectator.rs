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
    fn enabled(&self) -> bool {
        match self {
            Item::Empty => false,
            Item::Close | Item::Player(_) | Item::Team(_) => true,
            Item::Scroll { enabled, .. } | Item::TeleportPlayers { enabled } | Item::TeleportTeams { enabled } => {
                *enabled
            }
        }
    }

    fn name_key(&self) -> Option<&'static str> {
        Some(match self {
            Item::Close => "spectatorMenu.close",
            Item::Scroll { dir, .. } if *dir < 0 => "spectatorMenu.previous_page",
            Item::Scroll { .. } => "spectatorMenu.next_page",
            Item::TeleportPlayers { .. } => "spectatorMenu.teleport",
            Item::TeleportTeams { .. } => "spectatorMenu.team_teleport",
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

struct Menu {
    category: Category,
    selected: i32,
    page: i32,
}

impl Menu {
    fn items_len(&self) -> usize {
        match &self.category {
            Category::Root { .. } => 2,
            Category::Players(p) => p.len(),
            Category::Teams(t) => t.len(),
        }
    }

    fn entry(&self, i: usize) -> Item {
        match &self.category {
            Category::Root { players, teams } => match i {
                0 => Item::TeleportPlayers { enabled: !players.is_empty() },
                1 => Item::TeleportTeams { enabled: !teams.is_empty() },
                _ => Item::Empty,
            },
            Category::Players(p) => p.get(i).cloned().map_or(Item::Empty, Item::Player),
            Category::Teams(t) => t.get(i).cloned().map_or(Item::Empty, Item::Team),
        }
    }

    /// Slot `slot` (0..8) of the current page: 0 = previous page (following pages), 7 = next
    /// page, 8 = close.
    fn item(&self, slot: i32) -> Item {
        let index = slot + self.page * 6;
        if self.page > 0 && slot == 0 {
            Item::Scroll { dir: -1, enabled: true }
        } else if slot == 7 {
            Item::Scroll { dir: 1, enabled: (index as usize) < self.items_len() }
        } else if slot == 8 {
            Item::Close
        } else if index >= 0 {
            self.entry(index as usize)
        } else {
            Item::Empty
        }
    }

    fn prompt_key(&self) -> &'static str {
        match self.category {
            Category::Root { .. } => "spectatorMenu.root.prompt",
            Category::Players(_) => "spectatorMenu.teleport.prompt",
            Category::Teams(_) => "spectatorMenu.team_teleport.prompt",
        }
    }
}

#[derive(Default)]
pub struct SpectatorUi {
    menu: Option<Menu>,
    last_selection: Option<Instant>,
    /// 8 × 8 faces (face then hat on top) of the skins received, by UUID.
    faces: HashMap<u128, Vec<u8>>,
    /// UUID written into each `dyn/face/<i>` slot.
    slots: Vec<Option<u128>>,
    /// Label of each slot's key (assigned key, then ready text): recomputed when the
    /// assignment changes, emptied on every menu opening (language or layout changed).
    key_labels: Vec<(String, Vec<StyledChar>)>,
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
        self.menu = Some(Menu { category, selected: -1, page: 0 });
        self.key_labels.clear();
    }

    /// Label of the key assigned to slot `slot` (`bound`: key name from the settings),
    /// like the controls screen (keyboard layout, otherwise translation).
    fn key_label(&mut self, slot: usize, bound: &str, lang: &rc_text::Lang) -> &[StyledChar] {
        if self.key_labels.len() <= slot {
            self.key_labels.resize_with(9, Default::default);
        }
        let e = &mut self.key_labels[slot];
        if e.0 != bound || e.1.is_empty() {
            e.0.clear();
            e.0.push_str(bound);
            e.1 = crate::gui::plain(&crate::options::key_label(lang, bound));
        }
        &e.1
    }

    fn select(&mut self, slot: i32) -> Action {
        let Some(m) = &mut self.menu else { return Action::None };
        let item = m.item(slot);
        if item == Item::Empty {
            return Action::None;
        }
        if m.selected != slot || !item.enabled() {
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
                *m = Menu { category, selected: -1, page: 0 };
            }
            Item::Team(t) => *m = Menu { category: Category::Players(t.players), selected: -1, page: 0 },
            Item::Player(p) => return Action::Teleport(p.uuid),
            Item::Empty => {}
        }
        Action::None
    }

    /// Slot key 1–9 (`slot` 0..8).
    pub fn hotbar_key(&mut self, slot: usize, players: &PlayerList, scores: &Scoreboard) -> Action {
        self.last_selection = Some(Instant::now());
        if self.menu.is_some() {
            self.select(slot as i32)
        } else {
            self.open(players, scores);
            Action::None
        }
    }

    /// Middle button: uses the selected slot, or opens the menu.
    pub fn action_key(&mut self, players: &PlayerList, scores: &Scoreboard) -> Action {
        self.last_selection = Some(Instant::now());
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
        while (0..=8).contains(&s) && !m.item(s).enabled() {
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
        let Some(m) = &self.menu else { return };
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
        let items: Vec<Item> = (0..9).map(|s| m.item(s)).collect();
        self.slots.resize(rc_assets::gui::FACE_SLOTS, None);
        for (slot, item) in items.iter().enumerate() {
            if *item == Item::Empty {
                continue;
            }
            let x = cx - 90 + slot as i32 * 20 + 2;
            let iy = y + 3;
            let b = if item.enabled() { 1.0 } else { 0.25 };
            let gray = a | (((b * 255.0) as u32) * 0x01_0101);
            let sprite = |gui: &mut Gui, name: &str| gui.sprite(name, f(x), f(iy), 16.0, 16.0, gray);
            match item {
                Item::Close => sprite(gui, "spectator/close"),
                Item::Scroll { dir, .. } => {
                    sprite(gui, if *dir < 0 { "spectator/scroll_left" } else { "spectator/scroll_right" })
                }
                Item::TeleportPlayers { .. } => sprite(gui, "spectator/teleport_to_player"),
                Item::TeleportTeams { .. } => sprite(gui, "spectator/teleport_to_team"),
                Item::Player(p) => self.face(gui, queue, slot, p.uuid, default_face, f(x + 2), f(iy + 2), white),
                Item::Team(t) => {
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
                    self.face(gui, queue, slot, t.icon, default_face, f(x + 2), f(iy + 2), gray);
                }
                Item::Empty => {}
            }
            if item.enabled() {
                let key = self.key_label(slot, bound(slot), &gui.lang);
                let kw = rc_text::width(gui, key);
                gui.text(queue, key, f(x + 19 - 2 - kw), f(iy + 6 + 3), white, true, 0);
            }
        }
        // Action text: name of the selected slot, otherwise the category's prompt.
        let Some(m) = &self.menu else { return };
        let selected = if m.selected >= 0 { m.item(m.selected) } else { Item::Empty };
        let comp = match &selected {
            Item::Empty => Component::translate(m.prompt_key(), vec![]),
            Item::Player(p) => Component::text(p.name.clone()),
            Item::Team(t) => t.name.clone(),
            other => Component::translate(other.name_key().unwrap_or(""), vec![]),
        };
        let chars: Vec<StyledChar> = rc_text::styled_chars(&rc_text::flatten(&comp, &gui.lang));
        let tw = rc_text::width(gui, &chars);
        let (tx, ty) = ((w - tw) / 2, h - 35);
        if backdrop >> 24 != 0 {
            // Background multiplied by the text color (fade alpha).
            let ba = ((backdrop >> 24) as f32 * alpha) as u32;
            gui.fill(f(tx - 2), f(ty - 2), f(tx + tw + 2), f(ty + 9 + 2), ba << 24 | (backdrop & 0xff_ffff));
        }
        gui.text(queue, &chars, f(tx), f(ty), white, true, 0);
    }

    /// 12 × 12 face of a player (`dyn/face/<slot>` slot rewritten if needed).
    #[allow(clippy::too_many_arguments)]
    fn face(
        &mut self,
        gui: &mut Gui,
        queue: &wgpu::Queue,
        slot: usize,
        uuid: u128,
        default_face: &dyn Fn(u128) -> Option<Vec<u8>>,
        x: f32,
        y: f32,
        color: u32,
    ) {
        let name = format!("dyn/face/{slot}");
        if self.slots[slot] != Some(uuid) {
            let rgba = self.faces.get(&uuid).cloned().or_else(|| default_face(uuid));
            let Some(rgba) = rgba else { return };
            if !gui.write_sprite(queue, &name, (8, 8), &rgba) {
                return;
            }
            self.slots[slot] = Some(uuid);
        }
        gui.sprite(&name, x, y, 12.0, 12.0, color);
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
        Menu { category: Category::Players(players), selected: -1, page: 0 }
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

    #[test]
    fn uuid_order_is_signed() {
        let mut v = [1u128 << 127, 5u128];
        v.sort_by_key(|u| uuid_key(*u));
        assert_eq!(v, [1u128 << 127, 5]);
    }
}
