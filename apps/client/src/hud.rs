//! In-game interface outside the chat (`docs/specs/hud.md`): vignette, crosshair, hotbar,
//! armor, hearts, hunger, experience. The items in the slots, air, effects and
//! mounts are waiting for their data (inventory, entity data).

use std::time::Instant;

use crate::gui::{Blend, Gui};
use crate::sky_mesh::JavaRandom;

/// What is needed to draw the hotbar items.
pub struct HotbarItems<'a> {
    pub inv: &'a crate::inventory::PlayerInventory,
    pub ids: &'a crate::stack::ComponentIds,
    pub data: &'a rc_data::VersionData,
    pub infos: &'a [rc_data::ItemInfo],
    pub names: &'a [String],
    pub env: &'a crate::stack::HudEnv,
    /// Fraction of the current tick (pickup animation).
    pub pt: f32,
}

pub struct Hud {
    /// Selected slot (0..8) and last slot sent to the server (`hud.md` § 6.2).
    pub selected: usize,
    carried: usize,
    scroll_acc: f64,
    scroll_acc_x: f64,
    /// Detached camera (F5): no crosshair.
    pub detached: bool,
    /// HUD hidden (F1).
    pub hidden: bool,
    /// 3D crosshair of the debug screen in place of the 2D one (`debug-f3.md` § 8.3).
    pub crosshair_3d: bool,
    tick: i32,
    pub hardcore: bool,
    /// Absorption and score of the local player (entity data 17 and 18).
    pub absorption: f32,
    pub score: i32,
    health: f32,
    food: i32,
    saturation: f32,
    /// The first health packet does not trigger a blink.
    flash_on_set: bool,
    damage_cooldown: i32,
    last_health: i32,
    display_health: i32,
    last_health_time: Option<Instant>,
    blink_until: i64,
    xp_progress: f32,
    xp_level: i32,
    /// Smoothed brightness of the vignette (§ 5.3).
    vignette: f32,
    /// "Show vignette" option disabled.
    pub no_vignette: bool,
    /// Weight of the world border warning (red tint of the vignette, § 5.3).
    pub border_warn: f32,
    /// Experience level text, redone when the level changes.
    level_text: Option<(i32, Vec<rc_text::StyledChar>)>,
    /// Name of the held item (§ 5.1): item, name, timer.
    held: Option<(i32, rc_text::Component)>,
    held_timer: i32,
    /// Evaluation of the slots (hotbar then off hand), redone when the stack, the selection
    /// or the tick change.
    slot_cache: Vec<Option<SlotCache>>,
    /// Attack indicator of the frame: recharge strength (0..1), "full" (§ 2, § 3.3).
    pub attack: Option<(f32, bool)>,
    /// "Attack indicator" option: 0 none, 1 crosshair, 2 hotbar.
    pub attack_mode: i32,
    /// Left main hand (side of the indicator on the bar).
    pub main_left: bool,
    /// Air, mount, contextual bars, overlays (`hud_more.rs`).
    pub more: crate::hud_more::HudMore,
    /// Effects and freeze of the player (hearts, hunger), set every frame.
    pub status: Status,
    /// Text background outside the chat: `None` if the option reserves it for the chat, otherwise the opacity chosen
    /// (alpha only).
    pub text_background: Option<u32>,
    /// "Notification display time" option.
    pub notification_time: f64,
    /// Spectator: the aim touches an entity with a menu or an interactive block (crosshair shown).
    pub spectator_aim: bool,
}

struct SlotCache {
    stack: rc_proto_v777::item::ItemStack,
    hold: crate::stack::Hold,
    tick: i64,
    layers: Vec<rc_assets::item::Layer>,
    key: u64,
    bar: Option<(i32, u32)>,
}

/// Heart kind (`hud.md` § 4.2): container, then fills depending on the player's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Heart {
    Container,
    Normal,
    Poisoned,
    Withered,
    Absorbing,
    Frozen,
}

/// Sprites per kind, index `hardcore·4 + half·2 + blinking` (the container ignores "half").
const HEART_SPRITES: [[&str; 8]; 6] = [
    [
        "hud/heart/container",
        "hud/heart/container_blinking",
        "hud/heart/container",
        "hud/heart/container_blinking",
        "hud/heart/container_hardcore",
        "hud/heart/container_hardcore_blinking",
        "hud/heart/container_hardcore",
        "hud/heart/container_hardcore_blinking",
    ],
    [
        "hud/heart/full",
        "hud/heart/full_blinking",
        "hud/heart/half",
        "hud/heart/half_blinking",
        "hud/heart/hardcore_full",
        "hud/heart/hardcore_full_blinking",
        "hud/heart/hardcore_half",
        "hud/heart/hardcore_half_blinking",
    ],
    [
        "hud/heart/poisoned_full",
        "hud/heart/poisoned_full_blinking",
        "hud/heart/poisoned_half",
        "hud/heart/poisoned_half_blinking",
        "hud/heart/poisoned_hardcore_full",
        "hud/heart/poisoned_hardcore_full_blinking",
        "hud/heart/poisoned_hardcore_half",
        "hud/heart/poisoned_hardcore_half_blinking",
    ],
    [
        "hud/heart/withered_full",
        "hud/heart/withered_full_blinking",
        "hud/heart/withered_half",
        "hud/heart/withered_half_blinking",
        "hud/heart/withered_hardcore_full",
        "hud/heart/withered_hardcore_full_blinking",
        "hud/heart/withered_hardcore_half",
        "hud/heart/withered_hardcore_half_blinking",
    ],
    [
        "hud/heart/absorbing_full",
        "hud/heart/absorbing_full_blinking",
        "hud/heart/absorbing_half",
        "hud/heart/absorbing_half_blinking",
        "hud/heart/absorbing_hardcore_full",
        "hud/heart/absorbing_hardcore_full_blinking",
        "hud/heart/absorbing_hardcore_half",
        "hud/heart/absorbing_hardcore_half_blinking",
    ],
    [
        "hud/heart/frozen_full",
        "hud/heart/frozen_full_blinking",
        "hud/heart/frozen_half",
        "hud/heart/frozen_half_blinking",
        "hud/heart/frozen_hardcore_full",
        "hud/heart/frozen_hardcore_full_blinking",
        "hud/heart/frozen_hardcore_half",
        "hud/heart/frozen_hardcore_half_blinking",
    ],
];

impl Heart {
    pub fn sprite(self, hardcore: bool, half: bool, blinking: bool) -> &'static str {
        HEART_SPRITES[self as usize][usize::from(hardcore) * 4 + usize::from(half) * 2 + usize::from(blinking)]
    }

    /// Fill of the health hearts: poison, then wither, then full freeze.
    pub fn of(s: &Status) -> Heart {
        if s.poison {
            Heart::Poisoned
        } else if s.wither {
            Heart::Withered
        } else if s.frozen {
            Heart::Frozen
        } else {
            Heart::Normal
        }
    }
}

/// Player state read by the hearts and hunger, set every frame by the caller.
#[derive(Clone, Copy, Debug, Default)]
pub struct Status {
    pub poison: bool,
    pub wither: bool,
    /// Full freeze (freeze ticks ≥ 140).
    pub frozen: bool,
    pub regeneration: bool,
    pub hunger: bool,
}

/// Freeze ticks beyond which the player is completely frozen.
pub const TICKS_TO_FREEZE: i32 = 140;

/// Heart offset by 2 px upwards under regeneration (−1: none).
pub fn regen_index(tick: i32, max_health: f32, regeneration: bool) -> i32 {
    if regeneration { tick % (max_health + 5.0).ceil() as i32 } else { -1 }
}

/// Hunger sprites (empty, half, full), "hunger" variant under the effect of the same name.
pub fn food_sprites(hunger: bool) -> [&'static str; 3] {
    if hunger {
        ["hud/food_empty_hunger", "hud/food_half_hunger", "hud/food_full_hunger"]
    } else {
        ["hud/food_empty", "hud/food_half", "hud/food_full"]
    }
}

/// Background behind a HUD text (action bar, title, held item name): rectangle 2 px
/// around the text, background colour multiplied by that of the text; nothing if the background is reserved
/// for the chat (`background` with zero alpha).
#[allow(clippy::too_many_arguments)]
pub fn text_with_backdrop(
    gui: &mut Gui,
    queue: &wgpu::Queue,
    chars: &[rc_text::StyledChar],
    x: f32,
    y: f32,
    width: i32,
    color: u32,
    background: u32,
) {
    if background != 0 {
        let c = multiply(background, color);
        gui.fill(x - 2.0, y - 2.0, x + width as f32 + 2.0, y + 9.0 + 2.0, c);
    }
    gui.text(queue, chars, x, y, color, true, 0);
}

/// Component-by-component product of two ARGB colours (integer division by 255).
pub fn multiply(a: u32, b: u32) -> u32 {
    (0..4).fold(0, |acc, k| {
        let s = k * 8;
        acc | ((a >> s & 0xff) * (b >> s & 0xff) / 255) << s
    })
}

/// Text background according to the options: default colour if the background is reserved for the chat, otherwise
/// the text background opacity (`text_background`, alpha only).
pub fn background_color(text_background: Option<u32>, default: u32) -> u32 {
    text_background.unwrap_or(default)
}

/// Off hand: abscissa of the item, of the frame and its sprite (on the left, or on the right when
/// the main hand is the left one).
pub fn offhand_layout(main_left: bool, cx: i32) -> (i32, i32, &'static str) {
    if main_left {
        (cx + 91 + 10, cx + 91, "hud/hotbar_offhand_right")
    } else {
        (cx - 91 - 26, cx - 91 - 29, "hud/hotbar_offhand_left")
    }
}

/// Rows of the cooldown veil of a slot at line `y`: `[top, bottom)`.
pub fn veil_rows(y: i32, fraction: f32) -> (i32, i32) {
    let top = y + (16.0 * (1.0 - fraction)).floor() as i32;
    (top, top + (16.0 * fraction).ceil() as i32)
}

/// Item cooldowns of the frame (copy of the local player's tracking and tick fraction), names of the
/// items and `use_cooldown` component: read by all the slot decorations.
struct CooldownFrame {
    cooldowns: rc_client::cooldowns::Cooldowns,
    partial: f32,
    names: Vec<String>,
    use_cooldown: Option<u16>,
}

thread_local! {
    static COOLDOWNS: std::cell::RefCell<Option<CooldownFrame>> = const { std::cell::RefCell::new(None) };
}

/// Cooldowns visible for this frame (called once per frame, before the slot drawings).
pub fn set_item_cooldowns(
    cooldowns: &rc_client::cooldowns::Cooldowns,
    partial: f32,
    names: &[String],
    use_cooldown: Option<u16>,
) {
    COOLDOWNS.with(|c| {
        let mut c = c.borrow_mut();
        match c.as_mut() {
            Some(f) => {
                f.cooldowns.clone_from(cooldowns);
                f.partial = partial;
                f.use_cooldown = use_cooldown;
                if f.names.len() != names.len() {
                    f.names = names.to_vec();
                }
            }
            None => {
                *c = Some(CooldownFrame { cooldowns: cooldowns.clone(), partial, names: names.to_vec(), use_cooldown })
            }
        }
    });
}

/// Cooldown group of a stack: that of its `use_cooldown` component if it names one,
/// otherwise the item's id.
pub fn cooldown_group<'a>(
    stack: &'a rc_proto_v777::item::ItemStack,
    names: &'a [String],
    use_cooldown: Option<u16>,
) -> Option<std::borrow::Cow<'a, str>> {
    if let Some(b) = use_cooldown.and_then(|id| stack.component(id))
        && b.len() > 5
        && b[4] == 1
    {
        let mut r = rc_proto::Reader::new(&b[5..]);
        if let Ok(g) = rc_proto::read_str(&mut r, 32767) {
            return Some(normalize_id(g));
        }
    }
    let name = names.get(usize::try_from(stack.item).ok()?)?;
    Some(normalize_id(name))
}

/// Id with its namespace (`minecraft:` by default).
fn normalize_id(s: &str) -> std::borrow::Cow<'_, str> {
    if s.contains(':') { std::borrow::Cow::Borrowed(s) } else { std::borrow::Cow::Owned(format!("minecraft:{s}")) }
}

/// Translucent white veil of a slot on cooldown (pearl, shield, chorus…), between the durability
/// bar and the count.
pub fn cooldown_veil(gui: &mut Gui, stack: &rc_proto_v777::item::ItemStack, x: f32, y: f32) {
    let fraction = COOLDOWNS.with(|c| {
        let c = c.borrow();
        let f = c.as_ref()?;
        let group = cooldown_group(stack, &f.names, f.use_cooldown)?;
        Some(f.cooldowns.percent(&group, f.partial))
    });
    if let Some(fr) = fraction
        && fr > 0.0
    {
        let (top, bottom) = veil_rows(y as i32, fr);
        gui.fill(x, top as f32, x + 16.0, bottom as f32, 0x7fff_ffff);
    }
}

impl Default for Hud {
    fn default() -> Self {
        Hud {
            detached: false,
            attack: None,
            attack_mode: 1,
            main_left: false,
            selected: 0,
            carried: 0,
            scroll_acc: 0.0,
            scroll_acc_x: 0.0,
            // `RUSTCRAFT_NOHUD`: start with the HUD hidden (measuring its cost).
            hidden: rc_data::env_flag!("RUSTCRAFT_NOHUD"),
            crosshair_3d: false,
            tick: 0,
            hardcore: false,
            absorption: 0.0,
            score: 0,
            health: 20.0,
            food: 20,
            saturation: 5.0,
            flash_on_set: false,
            damage_cooldown: 0,
            last_health: 0,
            display_health: 0,
            last_health_time: None,
            blink_until: 0,
            xp_progress: 0.0,
            xp_level: 0,
            vignette: 1.0,
            no_vignette: false,
            border_warn: 0.0,
            level_text: None,
            held: None,
            held_timer: 0,
            slot_cache: (0..10).map(|_| None).collect(),
            more: crate::hud_more::HudMore::new(),
            status: Status::default(),
            text_background: None,
            notification_time: 1.0,
            spectator_aim: false,
        }
    }
}

/// Survival or adventure: health and experience bars shown.
fn survival(game_mode: i32) -> bool {
    game_mode == 0 || game_mode == 2
}

impl Hud {
    /// Health packet (of the local player).
    pub fn set_health(&mut self, health: f32, food: i32, saturation: f32) {
        if self.flash_on_set {
            let dmg = self.health - health;
            if dmg < 0.0 {
                self.damage_cooldown = 10;
            } else if dmg > 0.0 {
                self.damage_cooldown = 20;
            }
        } else {
            self.flash_on_set = true;
        }
        self.health = health;
        self.food = food;
        self.saturation = saturation;
    }

    /// HUD ticks (blinking of the player list).
    pub fn gui_ticks(&self) -> i32 {
        self.tick
    }

    /// Player dead or dying (health received ≤ 0).
    pub fn dead(&self) -> bool {
        self.health <= 0.0
    }

    pub fn set_experience(&mut self, progress: f32, level: i32) {
        if progress != self.xp_progress {
            self.more.xp_changed();
        }
        self.xp_progress = progress;
        self.xp_level = level;
    }

    /// Respawn: new local player, first health packet silent again.
    /// Respawn: new player (new hunger; full health) while waiting for the health packet that
    /// the server only sends on the next tick; without it, the in-between tick sees a health of 0
    /// and reopens the death screen. Health, absorption and score are entity data: reset to
    /// their default value unless the server asks to keep them (`keep_data`, flag 2).
    pub fn respawn(&mut self, keep_data: bool) {
        self.flash_on_set = false;
        self.damage_cooldown = 0;
        self.food = 20;
        self.saturation = 5.0;
        if !keep_data {
            self.health = 20.0;
            self.absorption = 0.0;
            self.score = 0;
        }
    }

    /// Food level received (20 before the first health packet).
    /// Experience level (enchantment and anvil costs).
    pub fn experience_level(&self) -> i32 {
        self.xp_level
    }

    pub fn food_level(&self) -> i32 {
        self.food
    }

    /// Slot imposed by the server: the "last sent" one does not move, hence an acknowledgement on the tick.
    pub fn held_slot(&mut self, slot: i32) {
        if (0..9).contains(&slot) {
            self.selected = slot as usize;
        }
    }

    /// Wheel (§ 3.6): accumulation per axis, reset on sign change, integer part
    /// consumed; returns the notches (horizontal, vertical) of this event.
    pub fn wheel(&mut self, x: f64, y: f64) -> (i32, i32) {
        let step = |acc: &mut f64, v: f64| {
            if *acc != 0.0 && v.signum() != acc.signum() {
                *acc = 0.0;
            }
            *acc += v;
            let n = *acc as i32;
            *acc -= f64::from(n);
            n
        };
        (step(&mut self.scroll_acc_x, x), step(&mut self.scroll_acc, y))
    }

    /// Next or previous slot according to the sign of the notch (upwards: previous slot).
    pub fn scroll_select(&mut self, wheel: i32) {
        if wheel != 0 {
            self.selected = (self.selected as i32 - wheel.signum()).rem_euclid(9) as usize;
        }
    }

    /// Vertical wheel alone: accumulation then selection.
    #[cfg(test)]
    pub fn scroll(&mut self, y: f64) {
        let (_, n) = self.wheel(0.0, y);
        self.scroll_select(n);
    }

    /// Slot to send this tick (`set_carried_item`), if it changed.
    pub fn take_carried(&mut self) -> Option<i16> {
        (self.selected != self.carried).then(|| {
            self.carried = self.selected;
            self.selected as i16
        })
    }

    /// Tick of the held item name: empty slot → 0; other item or other name → 40 × notification
    /// duration; otherwise −1.
    pub fn tick_held(&mut self, held: Option<(i32, rc_text::Component)>) {
        match held {
            None => {
                self.held_timer = 0;
                self.held = None;
            }
            Some(h) if self.held.as_ref() != Some(&h) => {
                self.held_timer = (40.0 * self.notification_time) as i32;
                self.held = Some(h);
            }
            Some(_) => self.held_timer = (self.held_timer - 1).max(0),
        }
    }

    /// Tick: counters, and vignette towards `1 − brightness(raw light at the eyes)`.
    pub fn tick(&mut self, brightness: f32) {
        self.tick = self.tick.wrapping_add(1);
        if self.damage_cooldown > 0 {
            self.damage_cooldown -= 1;
        }
        let target = (1.0 - brightness).clamp(0.0, 1.0);
        self.vignette += (target - self.vignette) * 0.01;
    }

    /// Drawing in the game's order (§ 1.3); `max_health` and `armor` come from the attributes.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        gui: &mut Gui,
        queue: &wgpu::Queue,
        game_mode: i32,
        max_health: f64,
        armor: f64,
        items: Option<&HotbarItems<'_>>,
    ) {
        if self.hidden {
            return;
        }
        // Vignette: `dst·(1 − tex·b)`; below b < 1/510 no byte changes (difference < ½ level),
        // the full-screen quad is skipped (full-daylight case once the brightness is smoothed).
        let b = self.vignette.clamp(0.0, 1.0);
        let w = self.border_warn;
        if (b >= 1.0 / 510.0 || w > 0.0) && !self.no_vignette {
            let gb = b + (1.0 - b) * w;
            gui.vignette_rgb([b * (1.0 - w), gb, gb]);
        }
        // Camera overlays (pumpkin, powder snow, portal, nausea: `overlays.rs`).
        gui.overlays_here();
        let (w, h) = (gui.width as i32, gui.height as i32);
        let f = |v: i32| v as f32;
        // Crosshair (first person; in spectator, only on an entity with a menu or an interactive
        // block), inversion blend.
        if (game_mode != 3 || self.spectator_aim) && !self.detached && !self.crosshair_3d {
            gui.stratum(Blend::Invert);
            gui.sprite("hud/crosshair", f((w - 15) / 2), f((h - 15) / 2), 15.0, 15.0, 0xffff_ffff);
            // Attack indicator under the crosshair (§ 2).
            if let Some((force, full)) = self.attack
                && self.attack_mode == 1
            {
                let (x, y) = (f(w / 2 - 8), f(h / 2 - 7 + 16));
                if full {
                    gui.sprite("hud/crosshair_attack_indicator_full", x, y, 16.0, 16.0, 0xffff_ffff);
                } else if force < 1.0 {
                    let p = (force * 17.0) as i32;
                    gui.sprite("hud/crosshair_attack_indicator_background", x, y, 16.0, 4.0, 0xffff_ffff);
                    if p > 0 {
                        gui.sprite_region(
                            "hud/crosshair_attack_indicator_progress",
                            16.0,
                            4.0,
                            0.0,
                            0.0,
                            x,
                            y,
                            f(p),
                            4.0,
                        );
                    }
                }
            }
        }
        gui.stratum(Blend::Normal);
        let parts = bottom_parts(game_mode);
        let cx = w / 2;
        if parts.item_hotbar {
            gui.sprite("hud/hotbar", f(cx - 91), f(h - 22), 182.0, 22.0, 0xffff_ffff);
            gui.sprite(
                "hud/hotbar_selection",
                f(cx - 92 + self.selected as i32 * 20),
                f(h - 23),
                24.0,
                23.0,
                0xffff_ffff,
            );
            if let Some(it) = items {
                self.draw_items(gui, queue, it, cx, h);
            }
        }
        // Attack indicator on the bar (§ 3.3), filling from bottom to top.
        if let Some((force, _)) = self.attack
            && parts.item_hotbar
            && self.attack_mode == 2
            && force < 1.0
        {
            let (x, y) = (if self.main_left { cx - 113 } else { cx + 97 }, h - 20);
            gui.sprite("hud/hotbar_attack_indicator_background", f(x), f(y), 18.0, 18.0, 0xffff_ffff);
            let p = (force * 19.0) as i32;
            if p > 0 {
                let src = f(18 - p);
                gui.sprite_region(
                    "hud/hotbar_attack_indicator_progress",
                    18.0,
                    18.0,
                    0.0,
                    src,
                    f(x),
                    f(y) + src,
                    18.0,
                    f(p),
                );
            }
        }
        if survival(game_mode) {
            self.draw_health(gui, w, h, max_health, armor);
        }
        self.more.draw_vehicle(gui, w, h);
        // Contextual bar (§ 4.6): background, level, then foreground (marker points).
        let (left, top) = ((w - 182) / 2, h - 29);
        let bar = if parts.contextual_bar { self.more.bar(survival(game_mode)) } else { crate::hud_more::Bar::Empty };
        match bar {
            crate::hud_more::Bar::Experience => {
                gui.sprite("hud/experience_bar_background", f(left), f(top), 182.0, 5.0, 0xffff_ffff);
                let p = (self.xp_progress * 183.0) as i32;
                if p > 0 {
                    gui.sprite_region("hud/experience_bar_progress", 182.0, 5.0, 0.0, 0.0, f(left), f(top), f(p), 5.0);
                }
            }
            crate::hud_more::Bar::Jump => self.more.draw_jump(gui, left, top),
            crate::hud_more::Bar::Locator => self.more.draw_locator_background(gui, left, top),
            crate::hud_more::Bar::Empty => {}
        }
        if survival(game_mode) && self.xp_level > 0 {
            let level = self.xp_level;
            if self.level_text.as_ref().is_none_or(|t| t.0 != level) {
                let c =
                    rc_text::Component::translate("gui.experience.level", vec![rc_text::Arg::Text(level.to_string())]);
                self.level_text = Some((level, rc_text::styled_chars(&rc_text::flatten(&c, &gui.lang))));
            }
            let chars = &self.level_text.as_ref().expect("set above").1;
            let x = (w - rc_text::width(gui, chars)) / 2;
            let y = h - 24 - 9 - 2;
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                gui.text(queue, chars, f(x + dx), f(y + dy), 0xff00_0000, false, 0);
            }
            gui.text(queue, chars, f(x), f(y), 0xff80_ff20, false, 0);
        }
        if bar == crate::hud_more::Bar::Locator {
            self.more.draw_locator(gui, w, top);
        }
        // Held item name, faded over the last 10 ticks (in spectator, the menu's action text
        // replaces it: `spectator.rs`).
        if parts.held_name
            && self.held_timer > 0
            && let Some((_, name)) = &self.held
        {
            let a = (self.held_timer as f32 * 256.0 / 10.0).min(255.0) as u32;
            if a > 0 {
                let chars = rc_text::styled_chars(&rc_text::flatten(name, &gui.lang));
                let tw = rc_text::width(gui, &chars);
                let x = (w - tw) / 2;
                let y = h - 59 + if survival(game_mode) { 0 } else { 14 };
                let bg = background_color(self.text_background, 0);
                text_with_backdrop(gui, queue, &chars, f(x), f(y), tw, a << 24 | 0xff_ffff, bg);
            }
        }
    }

    /// Off hand slot, slot items (seeds 1..9, then 10) and their decorations.
    fn draw_items(&mut self, gui: &mut Gui, queue: &wgpu::Queue, it: &HotbarItems<'_>, cx: i32, h: i32) {
        use std::hash::{Hash, Hasher};
        let f = |v: i32| v as f32;
        let mut plan = Vec::new();
        // Off hand on the side opposite the main hand.
        let (offhand_x, frame_x, frame) = offhand_layout(self.main_left, cx);
        let slots = (0..9).map(|i| (it.inv.hotbar(i), cx - 90 + i as i32 * 20 + 2, i)).chain(std::iter::once((
            it.inv.offhand(),
            offhand_x,
            9,
        )));
        if it.inv.offhand().is_some() {
            gui.sprite(frame, f(frame_x), f(h - 23), 29.0, 24.0, 0xffff_ffff);
        }
        let Some(models) = gui.items.as_ref().map(|a| &a.models) else { return };
        for (stack, x, i) in slots {
            let Some(stack) = stack else {
                self.slot_cache[i] = None;
                continue;
            };
            // Selected slot = main hand, slot 9 = off hand.
            let hold = if i == self.selected || i == 9 { crate::stack::Hold::local_hand(i != 9) } else { crate::stack::Hold::NONE };
            let fresh = self.slot_cache[i]
                .as_ref()
                .is_some_and(|c| c.tick == it.env.tick && c.hold == hold && c.stack == *stack);
            if !fresh {
                let info = usize::try_from(stack.item).ok().and_then(|i| it.infos.get(i));
                let name = usize::try_from(stack.item).ok().and_then(|i| it.names.get(i)).map_or("", String::as_str);
                let view = crate::stack::StackRef { stack, info, ids: it.ids, data: it.data, hold };
                let mut layers = Vec::new();
                let mut key = Vec::new();
                if let Some(model) = view.model() {
                    models.layers(name, &model, &view, it.env, i as i32 + 1, &mut layers, &mut key);
                }
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                key.hash(&mut hasher);
                self.slot_cache[i] = Some(SlotCache {
                    stack: stack.clone(),
                    hold,
                    tick: it.env.tick,
                    layers,
                    key: hasher.finish(),
                    bar: view.durability_bar(),
                });
            }
            let c = self.slot_cache[i].as_ref().expect("filled above");
            let pop = if i < 9 { it.inv.pop[i] as f32 - it.pt } else { 0.0 };
            plan.push((x, i, pop, c.bar, stack.count));
        }
        gui.stratum(Blend::Premultiplied);
        let y = h - 16 - 3;
        for &(x, i, pop, _, _) in &plan {
            if let Some(c) = &self.slot_cache[i]
                && !c.layers.is_empty()
            {
                gui.item(c.key, &c.layers, f(x), f(y), pop);
            }
        }
        gui.stratum(Blend::Normal);
        for (x, i, _, bar, count) in &plan {
            if let Some((w, color)) = bar {
                gui.fill(f(x + 2), f(y + 13), f(x + 15), f(y + 15), 0xff00_0000);
                gui.fill(f(x + 2), f(y + 13), f(x + 2 + w), f(y + 14), 0xff00_0000 | color);
            }
            if let Some(c) = &self.slot_cache[*i] {
                cooldown_veil(gui, &c.stack, f(*x), f(y));
            }
            if *count != 1 {
                let chars = crate::gui::plain(&count.to_string());
                let tw = rc_text::width(gui, &chars);
                gui.text(queue, &chars, f(x + 17 - tw), f(y + 9), 0xffff_ffff, true, 0);
            }
        }
    }

    /// Armor, hearts and hunger (§ 4.1–4.4); the air waits for the entity data.
    fn draw_health(&mut self, gui: &mut Gui, w: i32, h: i32, max_health: f64, armor: f64) {
        let f = |v: i32| v as f32;
        let t = i64::from(self.tick);
        let current = self.health.ceil() as i32;
        let blink = self.blink_until > t && (self.blink_until - t) / 3 % 2 == 1;
        let now = Instant::now();
        if current < self.last_health && self.damage_cooldown > 0 {
            self.last_health_time = Some(now);
            self.blink_until = t + 20;
        } else if current > self.last_health && self.damage_cooldown > 0 {
            self.last_health_time = Some(now);
            self.blink_until = t + 10;
        }
        if self.last_health_time.is_none_or(|l| now.duration_since(l).as_millis() > 1000) {
            self.display_health = current;
            self.last_health_time = Some(now);
        }
        self.last_health = current;
        let old = self.display_health;
        let mut random = JavaRandom::new(i64::from(self.tick.wrapping_mul(312_871)));
        let (x_left, x_right, y_base) = (w / 2 - 91, w / 2 + 91, h - 39);
        let max_hp = (max_health as f32).max(old.max(current) as f32);
        let absorption = self.absorption.ceil() as i32;
        let rows = ((max_hp + absorption as f32) / 2.0 / 10.0).ceil() as i32;
        let row_h = (10 - (rows - 2)).max(3);
        let regen_index = regen_index(self.tick, max_hp, self.status.regeneration);
        // Armor.
        let armor = armor.floor() as i32;
        if armor > 0 {
            let y = y_base - (rows - 1) * row_h - 10;
            for i in 0..10 {
                let s = match (i * 2 + 1).cmp(&armor) {
                    std::cmp::Ordering::Less => "hud/armor_full",
                    std::cmp::Ordering::Equal => "hud/armor_half",
                    std::cmp::Ordering::Greater => "hud/armor_empty",
                };
                gui.sprite(s, f(x_left + i * 8), f(y), 9.0, 9.0, 0xffff_ffff);
            }
        }
        // Hearts, from last to first.
        let hc = self.hardcore;
        let kind = Heart::of(&self.status);
        // Absorption: golden hearts, except under wither (withered hearts).
        let absorbing = if kind == Heart::Withered { kind } else { Heart::Absorbing };
        let container = Heart::Container.sprite(hc, false, blink);
        let containers = (max_hp / 2.0).ceil() as i32;
        let abs_containers = (absorption as f32 / 2.0).ceil() as i32;
        for c in (0..containers + abs_containers).rev() {
            let x = x_left + (c % 10) * 8;
            let mut y = y_base - (c / 10) * row_h;
            if current + absorption <= 4 {
                y += random.next_int(2);
            }
            if c < containers && c == regen_index {
                y -= 2;
            }
            let (x, y) = (f(x), f(y));
            gui.sprite(container, x, y, 9.0, 9.0, 0xffff_ffff);
            let halves = c * 2;
            if c >= containers {
                let a = halves - containers * 2;
                if a < absorption {
                    gui.sprite(absorbing.sprite(hc, a + 1 == absorption, false), x, y, 9.0, 9.0, 0xffff_ffff);
                }
            }
            if blink && halves < old {
                gui.sprite(kind.sprite(hc, halves + 1 == old, true), x, y, 9.0, 9.0, 0xffff_ffff);
            }
            if halves < current {
                gui.sprite(kind.sprite(hc, halves + 1 == current, false), x, y, 9.0, 9.0, 0xffff_ffff);
            }
        }
        // Hunger (except heart mount), then the air with the same random draw.
        let mut y_air = y_base - 10;
        if self.more.vehicle_hearts() == 0 {
            self.draw_food(gui, &mut random, x_right, y_base);
            y_air -= 10;
        }
        self.more.draw_air(gui, &mut random, self.tick, x_right, y_air);
    }

    fn draw_food(&self, gui: &mut Gui, random: &mut JavaRandom, x_right: i32, y_base: i32) {
        let f = |v: i32| v as f32;
        let [empty, half, full] = food_sprites(self.status.hunger);
        for i in 0..10 {
            let mut y = y_base;
            if self.saturation <= 0.0 && self.tick % (self.food * 3 + 1) == 0 {
                y += random.next_int(3) - 1;
            }
            let x = f(x_right - i * 8 - 9);
            gui.sprite(empty, x, f(y), 9.0, 9.0, 0xffff_ffff);
            if i * 2 + 1 < self.food {
                gui.sprite(full, x, f(y), 9.0, 9.0, 0xffff_ffff);
            }
            if i * 2 + 1 == self.food {
                gui.sprite(half, x, f(y), 9.0, 9.0, 0xffff_ffff);
            }
        }
    }
}

/// Entities with a menu (chests and hoppers on rails, chest boats and rafts).
pub fn menu_entity(kind: &str) -> bool {
    let k = kind.strip_prefix("minecraft:").unwrap_or(kind);
    k.ends_with("chest_boat") || k.ends_with("chest_raft") || k == "chest_minecart" || k == "hopper_minecart"
}

/// Blocks with an always-openable menu and portals (except chests and lecterns, handled apart).
fn menu_block(name: &str) -> bool {
    matches!(
        name,
        "barrel"
            | "dispenser"
            | "dropper"
            | "furnace"
            | "blast_furnace"
            | "smoker"
            | "hopper"
            | "brewing_stand"
            | "crafter"
            | "beacon"
            | "anvil"
            | "chipped_anvil"
            | "damaged_anvil"
            | "cartography_table"
            | "crafting_table"
            | "enchanting_table"
            | "grindstone"
            | "loom"
            | "smithing_table"
            | "stonecutter"
            | "nether_portal"
            | "end_portal"
            | "end_gateway"
    ) || name.ends_with("shulker_box")
}

/// Chests (single, trapped, copper): menu only if neither the cell nor its linked half are
/// blocked (conductor above or cat sitting on it).
fn chest_block(name: &str) -> bool {
    name == "chest" || name == "trapped_chest" || name.ends_with("copper_chest")
}

/// Parts of the bottom of the screen drawn according to the game mode.
#[derive(Debug, PartialEq, Eq)]
struct BottomParts {
    /// Hotbar, slot items and the bar's attack indicator.
    item_hotbar: bool,
    /// Contextual bar (experience, jump, locator).
    contextual_bar: bool,
    /// Held item name.
    held_name: bool,
}

/// In spectator, the spectator menu replaces the hotbar and the held item name;
/// hearts, vehicle and contextual bar (the locator, for lack of experience) remain.
fn bottom_parts(game_mode: i32) -> BottomParts {
    let spectator = game_mode == 3;
    BottomParts { item_hotbar: !spectator, contextual_bar: true, held_name: !spectator }
}

/// Spectator crosshair: aim at an entity with a menu, or at a block that would open a menu
/// (or a portal). `state_at` gives the state of a cell, `conductor` says whether a state conducts
/// redstone, `cat_sitting` whether there is a sitting cat in the cell above a position.
pub fn spectator_aim(
    hit: &crate::pick::Hit,
    data: &rc_data::VersionData,
    state_at: &dyn Fn([i32; 3]) -> u32,
    menu_entity_hit: &dyn Fn(i32) -> bool,
    cat_sitting: &dyn Fn([i32; 3]) -> bool,
) -> bool {
    use crate::pick::Hit;
    let pos = match *hit {
        Hit::Miss => return false,
        Hit::Entity { id, .. } => return menu_entity_hit(id),
        Hit::Block { pos, .. } => pos,
    };
    let state = state_at(pos);
    let Some(block) = data.block_of(state) else { return false };
    let name = block.name.strip_prefix("minecraft:").unwrap_or(&block.name);
    let props = |s: u32, key: &str| data.state_properties(s).find(|(k, _)| *k == key).map(|(_, v)| v.to_owned());
    if menu_block(name) {
        return true;
    }
    if name == "lectern" {
        return props(state, "has_book").as_deref() == Some("true");
    }
    if !chest_block(name) {
        return false;
    }
    let conductor = |s: u32| data.sim.get(s as usize).is_some_and(|b| b & rc_data::model::sim_bits::CONDUCTOR != 0);
    let blocked = |p: [i32; 3]| conductor(state_at([p[0], p[1] + 1, p[2]])) || cat_sitting(p);
    if blocked(pos) {
        return false;
    }
    let (Some(kind), Some(facing)) = (props(state, "type"), props(state, "facing")) else { return true };
    if kind == "single" {
        return true;
    }
    // Linked half: to the right of the gaze for "left", to the left for "right".
    let (dx, dz) = match (facing.as_str(), kind == "left") {
        ("north", true) | ("south", false) => (1, 0),
        ("north", false) | ("south", true) => (-1, 0),
        ("east", true) | ("west", false) => (0, 1),
        ("east", false) | ("west", true) => (0, -1),
        _ => return true,
    };
    let other = [pos[0] + dx, pos[1], pos[2] + dz];
    let os = state_at(other);
    let same_block = data.state(os).is_some_and(|i| data.state(state).is_some_and(|s| s.block == i.block));
    let linked = same_block
        && props(os, "type").is_some_and(|t| t != "single" && t != kind)
        && props(os, "facing").as_deref() == Some(facing.as_str());
    !(linked && blocked(other))
}

impl crate::App {
    /// HUD state of the frame: effects and freeze (hearts, hunger), text background, notification
    /// duration, spectator crosshair, item cooldowns of the slots.
    pub(crate) fn hud_frame(&mut self, hit: crate::pick::Hit) {
        let fx = &self.hud_extra;
        let frozen = self.entities.me.as_ref().and_then(|e| e.meta_int(7)).unwrap_or(0) >= TICKS_TO_FREEZE;
        self.hud.status = Status {
            poison: fx.has_effect("minecraft:poison"),
            wither: fx.has_effect("minecraft:wither"),
            frozen,
            regeneration: fx.has_effect("minecraft:regeneration"),
            hunger: fx.has_effect("minecraft:hunger"),
        };
        self.hud.text_background = (!self.options.b("backgroundForChatOnly"))
            .then(|| ((self.options.f("textBackgroundOpacity") * 255.0) as u32) << 24);
        self.hud.notification_time = self.options.f("notificationDisplayTime");
        self.hud.spectator_aim = self.env.game_mode == 3 && {
            let view = crate::environ::WorldView {
                grid: &self.world.grid,
                min_y: self.world.min_y,
                height: self.world.height,
            };
            let entities = &self.entities.map;
            let cat_sitting = |p: [i32; 3]| {
                let (x0, y0, z0) = (f64::from(p[0]), f64::from(p[1] + 1), f64::from(p[2]));
                entities.values().any(|e| {
                    let s = if e.baby() { 0.5 } else { 1.0 };
                    let (hw, h) = (0.3 * s, 0.7 * s);
                    e.kind == "cat"
                        && e.meta_byte(17) & 1 != 0
                        && e.pos[0] + hw > x0
                        && e.pos[0] - hw < x0 + 1.0
                        && e.pos[1] + h > y0
                        && e.pos[1] < y0 + 1.0
                        && e.pos[2] + hw > z0
                        && e.pos[2] - hw < z0 + 1.0
                })
            };
            spectator_aim(
                &hit,
                &self.data,
                &|p| view.state(p[0], p[1], p[2]),
                &|id| entities.get(&id).is_some_and(|e| menu_entity(&e.kind)),
                &cat_sitting,
            )
        };
        let pt = self.partial() as f32;
        set_item_cooldowns(&self.cooldowns, pt, &self.item_names, self.component_ids.of("use_cooldown"));
    }
}

#[cfg(test)]
mod tests {
    use super::Hud;

    #[test]
    fn respawn_is_alive_before_the_health_packet() {
        let mut h = Hud::default();
        h.set_health(0.0, 3, 0.0);
        (h.absorption, h.score) = (4.0, 17);
        h.respawn(false);
        assert!(!h.dead(), "death screen reopened between respawn and health");
        assert_eq!((h.food_level(), h.absorption, h.score), (20, 0.0, 0));
        // Data kept (dimension change): health, absorption and score preserved.
        h.set_health(7.0, 3, 0.0);
        (h.absorption, h.score) = (4.0, 17);
        h.respawn(true);
        assert_eq!((h.health, h.absorption, h.score), (7.0, 4.0, 17));
    }

    #[test]
    fn wheel_wraps_and_accumulates() {
        let mut h = Hud::default();
        h.scroll(1.0);
        assert_eq!(h.selected, 8, "wheel up: previous slot, wrap-around");
        h.scroll(-0.5);
        assert_eq!(h.selected, 8);
        h.scroll(-0.5);
        assert_eq!(h.selected, 0);
        // Sign change: the accumulation restarts from zero.
        h.scroll(-0.6);
        h.scroll(0.6);
        assert_eq!(h.selected, 0);
    }

    #[test]
    fn carried_item_sent_once_and_server_slot_acknowledged() {
        let mut h = Hud::default();
        assert_eq!(h.take_carried(), None);
        h.selected = 3;
        assert_eq!(h.take_carried(), Some(3));
        assert_eq!(h.take_carried(), None);
        h.held_slot(5);
        assert_eq!(h.take_carried(), Some(5));
        h.held_slot(9);
        assert_eq!(h.selected, 5);
    }

    #[test]
    fn heart_kind_follows_poison_wither_then_freeze() {
        use super::{Heart, Status};
        let all = Status { poison: true, wither: true, frozen: true, ..Default::default() };
        assert_eq!(Heart::of(&all), Heart::Poisoned);
        assert_eq!(Heart::of(&Status { poison: false, ..all }), Heart::Withered);
        assert_eq!(Heart::of(&Status { frozen: true, ..Default::default() }), Heart::Frozen);
        assert_eq!(Heart::of(&Status::default()), Heart::Normal);
        assert_eq!(Heart::Withered.sprite(true, true, false), "hud/heart/withered_hardcore_half");
        assert_eq!(Heart::Frozen.sprite(false, false, true), "hud/heart/frozen_full_blinking");
        assert_eq!(Heart::Container.sprite(true, true, true), "hud/heart/container_hardcore_blinking");
        assert_eq!(Heart::Poisoned.sprite(false, true, true), "hud/heart/poisoned_half_blinking");
    }

    #[test]
    fn regeneration_bob_and_hunger_food() {
        use super::{food_sprites, regen_index};
        assert_eq!(regen_index(57, 20.0, false), -1);
        // ceil(20 + 5) = 25: heart 57 % 25 = 7 rises.
        assert_eq!(regen_index(57, 20.0, true), 7);
        assert_eq!(regen_index(57, 20.5, true), 57 % 26);
        assert_eq!(food_sprites(true)[2], "hud/food_full_hunger");
        assert_eq!(food_sprites(false)[0], "hud/food_empty");
    }

    #[test]
    fn offhand_goes_right_for_left_handed_players() {
        use super::offhand_layout;
        assert_eq!(offhand_layout(false, 200), (200 - 117, 200 - 120, "hud/hotbar_offhand_left"));
        assert_eq!(offhand_layout(true, 200), (301, 291, "hud/hotbar_offhand_right"));
    }

    #[test]
    fn held_name_time_scales_with_notification_option() {
        let mut h = Hud { notification_time: 2.5, ..Hud::default() };
        h.tick_held(Some((1, rc_text::Component::text("a"))));
        assert_eq!(h.held_timer, 100);
    }

    #[test]
    fn cooldown_veil_rows_and_backdrop_color() {
        use super::{multiply, veil_rows};
        assert_eq!(veil_rows(100, 1.0), (100, 116));
        assert_eq!(veil_rows(100, 0.475), (108, 116));
        assert_eq!(veil_rows(100, 0.01), (115, 116));
        // Background 50% × text at alpha 128: alpha 64 (integer division), black.
        assert_eq!(multiply(0x8000_0000, 0x80ff_ffff), 0x4000_0000);
    }

    #[test]
    fn cooldown_group_reads_use_cooldown_component() {
        use super::cooldown_group;
        let names = vec!["minecraft:air".to_owned(), "minecraft:ender_pearl".to_owned()];
        let mut st = rc_proto_v777::item::ItemStack { item: 1, count: 1, added: Vec::new(), removed: Vec::new() };
        assert_eq!(cooldown_group(&st, &names, Some(7)).as_deref(), Some("minecraft:ender_pearl"));
        // `use_cooldown`: 1.0 s, group "foo:bar".
        let mut b = 1.0f32.to_be_bytes().to_vec();
        b.push(1);
        b.push(7);
        b.extend_from_slice(b"foo:bar");
        st.added.push((7, b.into_boxed_slice()));
        assert_eq!(cooldown_group(&st, &names, Some(7)).as_deref(), Some("foo:bar"));
        // Without group: the item's.
        let mut b = 1.0f32.to_be_bytes().to_vec();
        b.push(0);
        st.added[0].1 = b.into_boxed_slice();
        assert_eq!(cooldown_group(&st, &names, Some(7)).as_deref(), Some("minecraft:ender_pearl"));
    }

    /// In spectator, only the hotbar (and the held name) disappears: the contextual bar,
    /// hence the locator bar, stays drawn.
    #[test]
    fn spectator_keeps_contextual_bar() {
        use super::{BottomParts, bottom_parts};
        assert_eq!(bottom_parts(3), BottomParts { item_hotbar: false, contextual_bar: true, held_name: false });
        for mode in 0..=2 {
            assert_eq!(bottom_parts(mode), BottomParts { item_hotbar: true, contextual_bar: true, held_name: true });
        }
    }

    #[test]
    fn spectator_crosshair_only_on_menus() {
        use super::{menu_entity, spectator_aim};
        use crate::pick::Hit;
        assert!(menu_entity("minecraft:chest_minecart"));
        assert!(menu_entity("bamboo_chest_raft"));
        assert!(menu_entity("oak_chest_boat"));
        assert!(!menu_entity("minecart"));
        assert!(!menu_entity("villager"));
        let data = rc_data::VersionData::default();
        let miss = spectator_aim(&Hit::Miss, &data, &|_| 0, &|_| true, &|_| false);
        assert!(!miss);
        let ent = Hit::Entity { id: 3, at: [0.0; 3] };
        assert!(spectator_aim(&ent, &data, &|_| 0, &|id| id == 3, &|_| false));
        assert!(!spectator_aim(&ent, &data, &|_| 0, &|id| id == 4, &|_| false));
    }

    #[test]
    fn first_health_packet_is_silent() {
        let mut h = Hud::default();
        h.set_health(10.0, 20, 5.0);
        assert_eq!(h.damage_cooldown, 0);
        h.set_health(6.0, 20, 5.0);
        assert_eq!(h.damage_cooldown, 20);
        h.set_health(7.0, 20, 5.0);
        assert_eq!(h.damage_cooldown, 10);
    }
}
