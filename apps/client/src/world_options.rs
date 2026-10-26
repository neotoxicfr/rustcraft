//! Screens opened from the pause menu and the Options screen (`docs/specs/option-menus.md` § 3.12–3.15):
//! world options (difficulty, lock, game mode, rules, local network), in-game game rules,
//! chat restrictions, credits and attributions, social interactions, confirmations.
//!
//! The screens live in the stack of [`crate::menus::Menus`]; this module builds their
//! components, handles their events and draws their texts. The world state seen by these
//! screens ([`Ctx`]) is provided by the application.

use rc_text::StyledChar;

use crate::gui::Gui;
use crate::menus::{Action, Confirm, Id};
use crate::options::{Options, Val};
use crate::widgets::{Entry, Event, Kind, List, Panel, TextField, Widget, chars, tr};

/// Game state read by the screens (updated by the application at each tick).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ctx {
    /// Difficulty (0 peaceful … 3 hard) and lock (`change_difficulty`).
    pub difficulty: i32,
    pub locked: bool,
    pub hardcore: bool,
    /// Permission level (entity event 24–28).
    pub op_level: i32,
    /// Game mode of the player (0 survival … 3 spectator).
    pub game_mode: i32,
    /// Singleplayer world open (`rc-sim`): the player is its host.
    pub solo: bool,
}

/// Known player (server player list).
#[derive(Debug, Clone, PartialEq)]
pub struct Player {
    pub uuid: u128,
    pub name: String,
}

/// State of the screens of this module.
#[derive(Debug, Default)]
pub struct World {
    pub ctx: Ctx,
    /// Rule values received for the open screen (`game_rule_values`), `None`: pending.
    pub rules: Option<Vec<(String, String)>>,
    /// Button added by the server to the pause menu: label (external title of the dialog).
    pub pause_extra: Option<Vec<StyledChar>>,
    /// Online players, local player, server name (social interactions).
    pub players: Vec<Player>,
    pub local: u128,
    pub server_name: String,
    /// `servers.dat` entry of the current connection (name, address): code of conduct.
    pub server: Option<(String, String)>,
    /// Address of the link to confirm.
    pub url: String,
    /// Tag `pause_screen_additions` (ids of the dialog registry) and dialog of the
    /// retained server button.
    pause_tag: Vec<i32>,
    pause_dialog: Option<i32>,
    /// Cursor shape applied to the window.
    cursor: crate::widgets::Cursor,
    /// Code of conduct demonstration done.
    coc_test: bool,
    // Editing of the world options.
    wanted_difficulty: i32,
    wanted_locked: bool,
    wanted_mode: i32,
    wanted_default_mode: i32,
    wanted_commands: bool,
    wanted_lan: bool,
    port: String,
    wanted_guest: bool,
    wanted_force: bool,
    /// Values held by the integrated server at opening (guests' commands, forced mode):
    /// restored when a constraint that forced them is lifted.
    init_guest: bool,
    init_force: bool,
    /// World open to the local network: port, guests' commands, forced mode.
    pub published: Option<(u16, bool, bool)>,
    /// Retained port (entered and valid, otherwise suggested free port, shown as the field's hint);
    /// port of the already open world (0 otherwise: any entered port is then tested).
    port_num: u16,
    init_port: u16,
    /// Error of the port field (translation key, bounds 1024–65535); `None`: valid port.
    port_err: Option<&'static str>,
    // Modified rules: full id, entered text.
    edits: Vec<(String, String)>,
    // Social interactions: tab (0 all, 1 hidden, 2 blocked), players shown.
    social_page: u8,
    shown: Vec<u128>,
    /// Players seen during the session (stay shown "offline" in the hidden tab).
    seen: Vec<Player>,
}

impl World {
    /// World options opened: wanted values = current values.
    pub fn start_world_options(&mut self) {
        self.wanted_difficulty = self.ctx.difficulty;
        self.wanted_locked = self.ctx.locked || self.ctx.hardcore;
        self.wanted_mode = self.ctx.game_mode;
        self.wanted_default_mode = self.ctx.game_mode;
        self.wanted_commands = self.ctx.op_level >= 2;
        self.wanted_lan = self.published.is_some();
        self.port = self.published.map(|p| p.0.to_string()).unwrap_or_default();
        self.init_guest = self.published.is_some_and(|p| p.1);
        self.init_force = self.published.is_none_or(|p| p.2);
        self.wanted_guest = self.init_guest;
        self.wanted_force = self.init_force;
        self.init_port = self.published.map_or(0, |p| p.0);
        self.port_num = if self.init_port != 0 { self.init_port } else { available_port() };
        self.port_err = None;
        self.update_guest();
    }

    /// Guests' commands depending on the local network and the commands wanted: forced to "No"
    /// (greyed button) if either is off, otherwise the server's value; then forced mode.
    fn update_guest(&mut self) {
        self.wanted_guest = self.wanted_lan && self.wanted_commands && self.init_guest;
        self.update_force();
    }

    /// Forced mode: "Yes" in hardcore or outside the local network, "No" if the guests have
    /// commands, otherwise the server's value.
    fn update_force(&mut self) {
        self.wanted_force = if self.ctx.hardcore || !self.wanted_lan {
            true
        } else if self.wanted_guest {
            false
        } else {
            self.init_force
        };
    }

    /// Commands off: default and personal modes brought back to the initial default mode.
    fn commands_changed(&mut self) {
        if !self.wanted_commands {
            self.wanted_default_mode = self.ctx.game_mode;
            self.wanted_mode = self.wanted_default_mode;
        }
        self.update_guest();
    }

    /// Local network toggled: port field reset to the open port (or emptied), error cleared.
    fn lan_toggled(&mut self) {
        self.update_guest();
        let desired = if self.wanted_lan && self.init_port != 0 { self.init_port.to_string() } else { String::new() };
        if self.port != desired {
            self.port = desired;
            let p = self.port.clone();
            self.port_err = self.parse_port(&p);
        }
        if !self.wanted_lan {
            self.port_err = None;
        }
    }

    /// Port entry: empty = free port; outside 1024–65535 or unreadable = invalid; a port other
    /// than the already open one and taken by another program = unavailable.
    fn parse_port(&mut self, value: &str) -> Option<&'static str> {
        if value.trim().is_empty() {
            self.port_num = available_port();
            return None;
        }
        match value.parse::<i32>() {
            Ok(p) if !(1024..=65535).contains(&p) => Some("lanServer.port.invalid"),
            Ok(p) if p != i32::from(self.init_port) && !port_free(p as u16) => Some("lanServer.port.unavailable"),
            Ok(p) => {
                self.port_num = p as u16;
                None
            }
            Err(_) => {
                self.port_num = available_port();
                Some("lanServer.port.invalid")
            }
        }
    }

    fn lan_changed(&self) -> bool {
        match self.published {
            None => self.wanted_lan,
            Some((port, guest, force)) => {
                !self.wanted_lan
                    || self.port_num != port
                    || self.wanted_guest != guest
                    || self.wanted_force != force
            }
        }
    }

    /// Game rules opened: values to ask the server again.
    pub fn start_game_rules(&mut self) {
        self.rules = None;
        self.edits.clear();
    }

    /// Online players (also remembered for the hidden tab).
    pub fn set_players(&mut self, players: Vec<Player>) {
        for p in &players {
            if let Some(s) = self.seen.iter_mut().find(|s| s.uuid == p.uuid) {
                s.name.clone_from(&p.name);
            } else {
                self.seen.push(p.clone());
            }
        }
        self.players = players;
    }

    fn difficulty_allowed(&self) -> bool {
        self.ctx.op_level >= 2 || self.ctx.solo
    }

    fn rules_allowed(&self) -> bool {
        let commands = if self.ctx.solo { self.wanted_commands } else { self.ctx.op_level >= 2 };
        !self.ctx.hardcore && commands
    }

    fn has_changes(&self) -> bool {
        let lock_initial = self.ctx.locked || self.ctx.hardcore;
        // Local network wanted: an invalid or taken port blocks "Apply".
        if self.ctx.solo && self.wanted_lan && self.port_err.is_some() {
            return false;
        }
        self.wanted_difficulty != self.ctx.difficulty
            || self.wanted_locked != lock_initial
            || (self.ctx.solo
                && (self.wanted_mode != self.ctx.game_mode
                    || self.wanted_default_mode != self.ctx.game_mode
                    || self.wanted_commands != (self.ctx.op_level >= 2)
                    || self.lan_changed()))
    }

    /// Current value of a rule (modified, otherwise received).
    fn rule_value(&self, key: &str) -> Option<&str> {
        if let Some((_, v)) = self.edits.iter().find(|(k, _)| k == key) {
            return Some(v);
        }
        self.rules.as_ref()?.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    fn set_edit(&mut self, key: &str, v: String) {
        let initial = self.rules.as_ref().and_then(|r| r.iter().find(|(k, _)| k == key)).map(|(_, v)| v.clone());
        self.edits.retain(|(k, _)| k != key);
        if initial.as_deref() != Some(v.as_str()) {
            self.edits.push((key.to_owned(), v));
        }
    }

    /// Modified and valid rules, in the packet format.
    fn changed_rules(&self) -> Vec<(String, String)> {
        self.edits.iter().filter(|(k, v)| rule_info(k).is_some_and(|r| r.parse(v).is_some())).cloned().collect()
    }

    fn rules_invalid(&self) -> bool {
        self.edits.iter().any(|(k, v)| rule_info(k).is_some_and(|r| r.parse(v).is_none()))
    }
}

// ---------- Game rules (`game_rule` registry of 26.3) ----------

#[derive(Debug, Clone, Copy)]
enum Def {
    Bool(bool),
    /// Default, minimum, maximum.
    Int(i32, i32, i32),
}

#[derive(Debug, Clone, Copy)]
struct Rule {
    id: &'static str,
    category: &'static str,
    def: Def,
}

impl Rule {
    /// Normalised value if the entry is valid for the rule.
    fn parse(&self, v: &str) -> Option<String> {
        match self.def {
            Def::Bool(_) => matches!(v, "true" | "false").then(|| v.to_owned()),
            Def::Int(_, lo, hi) => v.parse::<i32>().ok().filter(|n| (lo..=hi).contains(n)).map(|n| n.to_string()),
        }
    }

    fn default_text(&self) -> String {
        match self.def {
            Def::Bool(b) => b.to_string(),
            Def::Int(d, ..) => d.to_string(),
        }
    }
}

const MAX: i32 = i32::MAX;

macro_rules! rules {
    ($($id:literal $cat:literal $def:expr;)*) => { &[$(Rule { id: $id, category: $cat, def: $def }),*] };
}

/// Rules known to the client: short id, category, type and bounds.
const RULES: &[Rule] = rules! {
    "advance_time" "updates" Def::Bool(true);
    "advance_weather" "updates" Def::Bool(true);
    "allow_entering_nether_using_portals" "misc" Def::Bool(true);
    "block_drops" "drops" Def::Bool(true);
    "block_explosion_drop_decay" "drops" Def::Bool(true);
    "command_blocks_work" "misc" Def::Bool(true);
    "command_block_output" "chat" Def::Bool(true);
    "drowning_damage" "player" Def::Bool(true);
    "elytra_movement_check" "player" Def::Bool(true);
    "ender_pearls_vanish_on_death" "player" Def::Bool(true);
    "entity_drops" "drops" Def::Bool(true);
    "fall_damage" "player" Def::Bool(true);
    "fire_damage" "player" Def::Bool(true);
    "fire_spread_radius_around_player" "updates" Def::Int(128, -1, MAX);
    "forgive_dead_players" "mobs" Def::Bool(true);
    "freeze_damage" "player" Def::Bool(true);
    "global_sound_events" "misc" Def::Bool(true);
    "immediate_respawn" "player" Def::Bool(false);
    "keep_inventory" "player" Def::Bool(false);
    "lava_source_conversion" "updates" Def::Bool(false);
    "limited_crafting" "player" Def::Bool(false);
    "locator_bar" "player" Def::Bool(true);
    "log_admin_commands" "chat" Def::Bool(true);
    "max_block_modifications" "misc" Def::Int(32768, 1, MAX);
    "max_command_forks" "misc" Def::Int(65536, 0, MAX);
    "max_command_sequence_length" "misc" Def::Int(65536, 0, MAX);
    "max_entity_cramming" "mobs" Def::Int(24, 0, MAX);
    "max_minecart_speed" "misc" Def::Int(8, 1, 1000);
    "max_snow_accumulation_height" "updates" Def::Int(1, 0, 8);
    "mob_drops" "drops" Def::Bool(true);
    "mob_explosion_drop_decay" "drops" Def::Bool(true);
    "mob_griefing" "mobs" Def::Bool(true);
    "natural_health_regeneration" "player" Def::Bool(true);
    "player_movement_check" "player" Def::Bool(true);
    "players_nether_portal_creative_delay" "player" Def::Int(0, 0, MAX);
    "players_nether_portal_default_delay" "player" Def::Int(80, 0, MAX);
    "players_sleeping_percentage" "player" Def::Int(100, 0, MAX);
    "projectiles_can_break_blocks" "drops" Def::Bool(true);
    "pvp" "player" Def::Bool(true);
    "raids" "mobs" Def::Bool(true);
    "random_tick_speed" "updates" Def::Int(3, 0, MAX);
    "reduced_debug_info" "misc" Def::Bool(false);
    "respawn_radius" "player" Def::Int(10, 0, MAX);
    "send_command_feedback" "chat" Def::Bool(true);
    "show_advancement_messages" "chat" Def::Bool(true);
    "show_death_messages" "chat" Def::Bool(true);
    "spawner_blocks_work" "misc" Def::Bool(true);
    "spawn_mobs" "spawning" Def::Bool(true);
    "spawn_monsters" "spawning" Def::Bool(true);
    "spawn_patrols" "spawning" Def::Bool(true);
    "spawn_phantoms" "spawning" Def::Bool(true);
    "spawn_wandering_traders" "spawning" Def::Bool(true);
    "spawn_wardens" "spawning" Def::Bool(true);
    "spectators_generate_chunks" "player" Def::Bool(true);
    "spread_vines" "updates" Def::Bool(true);
    "tnt_explodes" "misc" Def::Bool(true);
    "tnt_explosion_drop_decay" "drops" Def::Bool(false);
    "universal_anger" "mobs" Def::Bool(false);
    "water_source_conversion" "updates" Def::Bool(true);
};

/// Rule of a full id (`minecraft:keep_inventory`) or short.
fn rule_info(key: &str) -> Option<&'static Rule> {
    let short = key.strip_prefix("minecraft:").unwrap_or(key);
    RULES.iter().find(|r| r.id == short)
}

/// Rules displayed: (category, [(received key, rule)]) in the game's order — categories by
/// id, rules by translation key — filtered by the search.
fn rule_groups<'a>(
    lang: &rc_text::Lang,
    keys: &'a [(String, String)],
    search: &str,
) -> Vec<(&'static str, Vec<(&'a str, &'static Rule)>)> {
    let needle = search.to_lowercase();
    let mut groups: Vec<(&'static str, Vec<(&'a str, &'static Rule)>)> = Vec::new();
    for (k, _) in keys {
        let Some(r) = rule_info(k) else { continue };
        let t = |key: &str| tr(lang, key, &[]).to_lowercase();
        let desc_key = format!("gamerule.minecraft.{}.description", r.id);
        let matches = needle.is_empty()
            || r.id.contains(&needle)
            || t(&format!("gamerule.minecraft.{}", r.id)).contains(&needle)
            || t(&format!("gamerule.category.minecraft.{}", r.category)).contains(&needle)
            || (lang.map.contains_key(&desc_key) && t(&desc_key).contains(&needle));
        if !matches {
            continue;
        }
        match groups.iter_mut().find(|g| g.0 == r.category) {
            Some(g) => g.1.push((k.as_str(), r)),
            None => groups.push((r.category, vec![(k.as_str(), r)])),
        }
    }
    groups.sort_by_key(|g| g.0);
    for g in &mut groups {
        g.1.sort_by_key(|(_, r)| r.id);
    }
    groups
}

// ---------- Component identifiers ----------

const DONE: u32 = 50;
const SEARCH: u32 = 52;
const W_HEADER: u32 = 199;
const W_DIFF: u32 = 200;
const W_LOCK: u32 = 201;
const W_RULES: u32 = 202;
const W_RESTRICT: u32 = 203;
const W_DEFAULT_MODE: u32 = 204;
const W_MODE: u32 = 205;
const W_COMMANDS: u32 = 206;
const W_LAN: u32 = 207;
const W_PORT: u32 = 208;
const W_GUEST: u32 = 209;
const W_FORCE: u32 = 210;
const W_APPLY: u32 = 211;
const CANCEL: u32 = 212;
const YES: u32 = 220;
const NO: u32 = 221;
const COPY: u32 = 222;
const R_ACTION: u32 = 223;
const C_CREDITS: u32 = 224;
const C_ATTRIBUTION: u32 = 225;
const C_LICENSES: u32 = 226;
/// Restrictions: "account settings" action (chat refused by the account).
const R_ACCOUNT: u32 = 227;
const S_TAB: u32 = 230;
const S_BLOCKING: u32 = 233;
/// Rule `i` displayed: button or field.
const RULE: u32 = 3000;
/// Player `i` displayed: hide / show, report.
const S_HIDE: u32 = 4000;
const S_REPORT: u32 = 5000;

const ATTRIBUTION_URL: &str = "https://aka.ms/MinecraftJavaAttribution";
const LICENSES_URL: &str = "https://aka.ms/MinecraftJavaLicenses";
const BLOCKING_URL: &str = "https://aka.ms/javablocking";
const ACCOUNT_SETTINGS_URL: &str = "https://aka.ms/JavaAccountSettings";

/// Stack change requested of `Menus`.
pub(crate) enum Step {
    None,
    /// Top screen closed (`n` screens).
    Pop(usize),
    Open(Id),
    /// Components to rebuild (values changed).
    Rebuild,
}

/// Screens of this module.
pub(crate) fn owns(id: Id) -> bool {
    matches!(id, Id::WorldOptions | Id::GameRules | Id::Restrictions | Id::Credits | Id::Social | Id::Confirm(_))
}

/// Title of a screen (`None`: drawn by the screen itself).
pub(crate) fn title(id: Id) -> Option<&'static str> {
    Some(match id {
        Id::WorldOptions => "options.worldOptions.title",
        Id::GameRules => "editGamerule.title",
        Id::Restrictions => "restrictions_screen.title",
        Id::Credits => "credits_and_attribution.screen.title",
        Id::Social => "gui.socialInteractions.title",
        _ => return None,
    })
}

/// Screen drawn over the previous one (pop-up window).
pub(crate) fn is_popup(id: Id) -> bool {
    id == Id::Confirm(Confirm::LockDifficulty)
}

fn t(gui: &Gui, k: &str) -> String {
    tr(&gui.lang, k, &[])
}

fn gv(gui: &Gui, name: &str, value: &str) -> String {
    tr(&gui.lang, "options.generic_value", &[t(gui, name), value.to_owned()])
}

fn on_off(gui: &Gui, v: bool) -> String {
    t(gui, if v { "options.on" } else { "options.off" })
}

const DIFFICULTIES: [&str; 4] = ["peaceful", "easy", "normal", "hard"];
const MODES: [&str; 4] = ["survival", "creative", "adventure", "spectator"];

fn label(id: u32, x: i32, y: i32, w: i32, text: Vec<StyledChar>, color: u32, centered: bool) -> Widget {
    Widget::new(id, Kind::Label { color, centered }, x, y, w, 9, text)
}

/// Components of a screen for a `W × H` interface.
pub(crate) fn build(id: Id, gui: &Gui, o: &Options, w: &mut World, search: &str) -> (Vec<Widget>, Option<List>) {
    let (sw, sh) = (gui.width as i32, gui.height as i32);
    match id {
        Id::WorldOptions => build_world_options(gui, w, sw, sh),
        Id::GameRules => build_rules(gui, w, search, sw, sh),
        Id::Restrictions => build_restrictions(gui, o, w.ctx.solo, sw, sh),
        Id::Credits => {
            let y = (33 + 30).min(sh - 33 - 76);
            let x = (sw - 210) / 2;
            let v = vec![
                Widget::button(C_CREDITS, x, y, 210, &t(gui, "credits_and_attribution.button.credits")),
                Widget::button(C_ATTRIBUTION, x, y + 28, 210, &t(gui, "credits_and_attribution.button.attribution")),
                Widget::button(C_LICENSES, x, y + 56, 210, &t(gui, "credits_and_attribution.button.licenses")),
                Widget::button(DONE, (sw - 200) / 2, sh - 26, 200, &t(gui, "gui.done")),
            ];
            (v, None)
        }
        Id::Social => build_social(gui, w, search, sw, sh),
        Id::Confirm(c) => (build_confirm(c, gui, w, sw, sh), None),
        _ => (Vec::new(), None),
    }
}

/// World options: general section (and multiplayer in singleplayer), "Apply" / "Cancel" footer.
fn build_world_options(gui: &Gui, w: &World, sw: i32, sh: i32) -> (Vec<Widget>, Option<List>) {
    let x0 = sw / 2 - 154;
    let x1 = x0 + 158;
    let mut v = Vec::new();
    let mut y = 33 + 8;
    v.push(Widget::new(W_HEADER, Kind::Title, x0, y, 308, 17, chars(&t(gui, "options.worldOptions.general.title"))));
    y += 17 + 4;
    let ctx = &w.ctx;
    let gm = |k: &str, m: i32| gv(gui, k, &t(gui, &format!("selectWorld.gameMode.{}", MODES[m.clamp(0, 3) as usize])));
    let mode_tip = |w: &mut Widget, tip: &str| {
        let key = if ctx.hardcore {
            "options.worldOptions.game_mode.disabled.tooltip"
        } else if !w.active {
            "options.worldOptions.game_mode.disabled.operator.tooltip"
        } else {
            tip
        };
        w.set_tooltip(gui, &t(gui, key));
    };
    // Next column of the 2 × n grid of the general section.
    let mut col = 0;
    let place = |v: &mut Vec<Widget>, mut wd: Widget, y: &mut i32, col: &mut i32| {
        wd.x = if *col == 0 { x0 } else { x1 };
        wd.y = *y;
        v.push(wd);
        if *col == 1 {
            *y += 24;
        }
        *col = 1 - *col;
    };
    if ctx.solo {
        let active = !ctx.hardcore && w.wanted_commands;
        let mut d = Widget::new(
            W_DEFAULT_MODE,
            Kind::Cycle,
            x0,
            y,
            308,
            20,
            chars(&gm("options.worldOptions.game_mode", w.wanted_default_mode)),
        );
        d.active = active;
        mode_tip(&mut d, "options.worldOptions.game_mode.tooltip");
        v.push(d);
        y += 24;
        let mut p = Widget::new(
            W_MODE,
            Kind::Cycle,
            x0,
            y,
            308,
            20,
            chars(&gm("options.worldOptions.personal_game_mode", w.wanted_mode)),
        );
        p.active = active;
        mode_tip(&mut p, "options.worldOptions.personal_game_mode.tooltip");
        v.push(p);
        y += 24;
        let mut c = Widget::new(
            W_COMMANDS,
            Kind::Cycle,
            0,
            0,
            150,
            20,
            chars(&gv(gui, "selectWorld.allowCommands", &on_off(gui, w.wanted_commands))),
        );
        c.active = !ctx.hardcore;
        let tip = if ctx.hardcore {
            "options.worldOptions.allow_commands.disabled.tooltip"
        } else {
            "options.worldOptions.allow_commands.tooltip"
        };
        c.set_tooltip(gui, &t(gui, tip));
        place(&mut v, c, &mut y, &mut col);
    }
    // Difficulty (150 − 20) and lock.
    let allowed = w.difficulty_allowed();
    let blocked = ctx.locked || ctx.hardcore;
    let dname = t(gui, &format!("options.difficulty.{}", DIFFICULTIES[w.wanted_difficulty.clamp(0, 3) as usize]));
    let mut diff = Widget::new(W_DIFF, Kind::Cycle, 0, 0, 130, 20, chars(&gv(gui, "options.difficulty", &dname)));
    diff.active = allowed && !blocked;
    let tip = if !allowed {
        "options.worldOptions.difficulty.disabled.operator.tooltip"
    } else if ctx.locked {
        "options.worldOptions.difficulty.disabled.locked.tooltip"
    } else if ctx.hardcore {
        "options.worldOptions.difficulty.disabled.hardcore.tooltip"
    } else {
        ""
    };
    if !tip.is_empty() {
        diff.set_tooltip(gui, &t(gui, tip));
    }
    let (dx, dy) = if col == 0 { (x0, y) } else { (x1, y) };
    place(&mut v, diff, &mut y, &mut col);
    let lock_sprite = if w.wanted_locked { "widget/locked_button" } else { "widget/unlocked_button" };
    let mut lock =
        Widget::new(W_LOCK, Kind::Sprite(lock_sprite), dx + 130, dy, 20, 20, chars(&t(gui, "difficulty.lock.title")));
    lock.active = allowed && !blocked;
    v.push(lock);
    let mut rules = Widget::button(W_RULES, 0, 0, 150, &t(gui, "editGamerule.inGame.button"));
    rules.active = w.rules_allowed();
    if ctx.hardcore {
        rules.set_tooltip(gui, &t(gui, "editGamerule.inGame.disabled.hardcore.tooltip"));
    } else if !rules.active {
        rules.set_tooltip(gui, &t(gui, "editGamerule.inGame.disabled.tooltip"));
    }
    place(&mut v, rules, &mut y, &mut col);
    let restrict = Widget::button(W_RESTRICT, 0, 0, 150, &t(gui, "restrictions_screen.button"));
    place(&mut v, restrict, &mut y, &mut col);
    if col == 1 {
        y += 24;
    }
    if ctx.solo {
        // "Multiplayer" section: opening to the local network (behaviour to come with singleplayer).
        y += 8 + 8 + 8 - 4;
        v.push(Widget::new(
            W_HEADER + 1000,
            Kind::Title,
            x0,
            y,
            308,
            17,
            chars(&t(gui, "options.worldOptions.multiplayer.title")),
        ));
        y += 17 + 4;
        let lan_tip = if w.wanted_lan {
            "menu.multiplayerOptions.network.lan.tooltip"
        } else {
            "menu.multiplayerOptions.network.off.tooltip"
        };
        let lan = Widget::new(
            W_LAN,
            Kind::Cycle,
            x0,
            y,
            150,
            20,
            chars(&gv(gui, "menu.multiplayerOptions.lan", &on_off(gui, w.wanted_lan))),
        )
        .with_tooltip(gui, &t(gui, lan_tip));
        v.push(lan);
        let hint = if w.wanted_lan { w.port_num.to_string() } else { t(gui, "lanServer.port") };
        let mut f = TextField::new(chars(&hint));
        f.text = w.port.chars().collect();
        f.cursor = f.text.len();
        f.anchor = f.cursor;
        f.editable = w.wanted_lan;
        f.text_color = port_color(w.port_err);
        let mut pf = Widget::new(W_PORT, Kind::Field(Box::new(f)), x1, y, 150, 20, Vec::new());
        pf.set_tooltip(gui, &port_tip(gui, w.port_err));
        pf.active = w.wanted_lan;
        v.push(pf);
        y += 24;
        let mut g = Widget::new(
            W_GUEST,
            Kind::Cycle,
            x0,
            y,
            150,
            20,
            chars(&gv(gui, "options.worldOptions.guest.command_access", &on_off(gui, w.wanted_guest))),
        );
        g.active = w.wanted_lan && w.wanted_commands;
        let gt = if !w.wanted_lan {
            "options.worldOptions.guest.command_access.disabled.scope.tooltip"
        } else if !w.wanted_commands {
            "options.worldOptions.guest.command_access.disabled.commands.tooltip"
        } else {
            "options.worldOptions.guest.command_access.tooltip"
        };
        g.set_tooltip(gui, &t(gui, gt));
        v.push(g);
        let mut fg = Widget::new(
            W_FORCE,
            Kind::Cycle,
            x1,
            y,
            150,
            20,
            chars(&gv(gui, "options.worldOptions.guest.force_game_mode", &on_off(gui, w.wanted_force))),
        );
        fg.active = w.wanted_lan && !w.wanted_guest && !ctx.hardcore;
        let ft = if ctx.hardcore {
            "options.worldOptions.guest.force_game_mode.off.hardcore.tooltip"
        } else if !w.wanted_lan {
            "options.worldOptions.guest.force_game_mode.off.scope.tooltip"
        } else if w.wanted_guest {
            "options.worldOptions.guest.force_game_mode.off.commands.tooltip"
        } else if w.wanted_force {
            "options.worldOptions.guest.force_game_mode.on.tooltip"
        } else {
            "options.worldOptions.guest.force_game_mode.off.tooltip"
        };
        fg.set_tooltip(gui, &t(gui, ft));
        v.push(fg);
    }
    let fx = (sw - 308) / 2;
    let mut apply = Widget::button(W_APPLY, fx, sh - 26, 150, &t(gui, "menu.multiplayerOptions.applyChanges"));
    apply.active = w.has_changes();
    v.push(apply);
    v.push(Widget::button(CANCEL, fx + 158, sh - 26, 150, &t(gui, "gui.cancel")));
    // Scrolling area (list background and separators) without entries.
    (v, Some(List::new(0, 33, sw, sh - 66, 308, 24)))
}

/// Game rules: search, list by category (24 per row, 220 wide), "Done" and
/// "Cancel". Before the server's reply: waiting text only.
fn build_rules(gui: &Gui, w: &World, search: &str, sw: i32, sh: i32) -> (Vec<Widget>, Option<List>) {
    let mut v = Vec::new();
    let mut field = TextField::new(chars(&format!("§7§o{}", t(gui, "gui.game_rule.search"))));
    field.text = search.chars().collect();
    field.cursor = field.text.len();
    field.anchor = field.cursor;
    v.push(Widget::new(SEARCH, Kind::Field(Box::new(field)), sw / 2 - 100, 17, 200, 15, Vec::new()));
    let fx = (sw - 308) / 2;
    let mut done = Widget::button(DONE, fx, sh - 26, 150, &t(gui, "gui.done"));
    done.active = !w.rules_invalid();
    v.push(done);
    v.push(Widget::button(CANCEL, fx + 158, sh - 26, 150, &t(gui, "gui.cancel")));
    let Some(keys) = &w.rules else { return (v, None) };
    let mut l = List::new(0, 36, sw, sh - 36 - 33, 220, 24);
    let left = l.row_left();
    let right = left + 220;
    let mut top = 0;
    let mut i = 0u32;
    for (cat, rules) in rule_groups(&gui.lang, keys, search) {
        let mut h = label(
            0,
            0,
            2 + top + 8,
            sw,
            chars(&format!("§e§l§n{}", t(gui, &format!("gamerule.category.minecraft.{cat}")))),
            0xffff_ffff,
            true,
        );
        h.in_list = true;
        v.push(h);
        top += 24;
        for (key, r) in rules {
            let name_key = format!("gamerule.minecraft.{}", r.id);
            let desc_key = format!("{name_key}.description");
            let mut tip = vec![chars(&format!("§e{}", r.id))];
            if gui.lang.map.contains_key(&desc_key) {
                tip.extend(rc_text::wrap(gui, &chars(&t(gui, &desc_key)), 150.0));
            }
            tip.push(chars(&format!("§7{}", tr(&gui.lang, "editGamerule.default", &[r.default_text()]))));
            let lines = rc_text::wrap(gui, &chars(&t(gui, &name_key)), 170.0);
            let y = 2 + top;
            match lines.len() {
                0 => {}
                1 => {
                    let mut a = label(0, left, y + 5, 170, lines[0].clone(), 0xffff_ffff, false);
                    a.in_list = true;
                    v.push(a);
                }
                _ => {
                    for (k, line) in lines.iter().take(2).enumerate() {
                        let mut a = label(0, left, y + 10 * k as i32, 170, line.clone(), 0xffff_ffff, false);
                        a.in_list = true;
                        v.push(a);
                    }
                }
            }
            // Whole row (above the texts): tooltip of the rule on hover.
            let mut lab = label(0, left, y, 220, Vec::new(), 0xffff_ffff, false);
            lab.h = 24;
            lab.tooltip = tip.clone();
            lab.in_list = true;
            v.push(lab);
            let value = w.rule_value(key).unwrap_or("").to_owned();
            let mut wd = match r.def {
                Def::Bool(_) => {
                    Widget::new(RULE + i, Kind::Cycle, right - 45, y, 44, 20, chars(&on_off(gui, value == "true")))
                }
                Def::Int(..) => {
                    let mut f = TextField::new(Vec::new());
                    f.text = value.chars().collect();
                    f.cursor = f.text.len();
                    f.anchor = f.cursor;
                    if r.parse(&value).is_none() {
                        f.text_color = 0xffff_0000;
                    }
                    Widget::new(RULE + i, Kind::Field(Box::new(f)), right - 45, y, 44, 20, Vec::new())
                }
            };
            wd.tooltip = tip;
            wd.in_list = true;
            v.push(wd);
            top += 24;
            i += 1;
        }
    }
    l.content = top + 4;
    (v, Some(l))
}

/// Received key of the rule displayed at rank `i` (same order as the construction).
fn rule_at(gui: &Gui, w: &World, search: &str, i: u32) -> Option<String> {
    let keys = w.rules.as_ref()?;
    rule_groups(&gui.lang, keys, search).into_iter().flat_map(|g| g.1).nth(i as usize).map(|(k, _)| k.to_owned())
}

/// Chat restrictions (`chatVisibility`) and the permissions that follow from them.
fn build_restrictions(gui: &Gui, o: &Options, solo: bool, sw: i32, sh: i32) -> (Vec<Widget>, Option<List>) {
    let mut v = Vec::new();
    let vis = o.i("chatVisibility");
    let mut y = 33;
    let wrap = |s: &str| rc_text::wrap(gui, &chars(s), 250.0);
    // Restrictions (key, action): chat setting, then on a multiplayer server those of the
    // launcher (`--disableChat`) and the account, like the game.
    let mut restrictions: Vec<(&str, Option<(u32, &str)>)> = Vec::new();
    match vis {
        2 => restrictions.push((
            "chat_restriction.chat_and_commands_disabled_by_options",
            Some((R_ACTION, "chat_restriction.chat_disabled_by_options.action")),
        )),
        1 => restrictions.push((
            "chat_restriction.chat_disabled_by_options",
            Some((R_ACTION, "chat_restriction.chat_disabled_by_options.action")),
        )),
        _ => {}
    }
    let launcher = !solo && rc_client::session::chat_disabled_by_launcher();
    let profile = !solo && rc_client::session::chat_disabled_by_profile();
    if launcher {
        restrictions.push(("chat_restriction.disabled_by_launcher", None));
    }
    if profile {
        restrictions.push((
            "chat_restriction.disabled_by_profile",
            Some((R_ACCOUNT, "chat_restriction.disabled_by_profile.action")),
        ));
    }
    for (k, action) in &restrictions {
        let lines = wrap(&format!("§c§l{}", t(gui, k)));
        y += 4;
        for l in lines {
            v.push(label(0, 0, y, sw, l, 0xffff_ffff, true));
            y += 9;
        }
        y += 4;
        if let Some((id, label_key)) = action {
            v.push(Widget::button(*id, sw / 2 - 100, y, 200, &t(gui, label_key)));
            y += 20;
        }
    }
    if !restrictions.is_empty() {
        y += 10;
    }
    let (mut send, cmds, sys, mut recv) = match vis {
        2 => (false, false, false, false),
        1 => (false, true, true, false),
        _ => (true, true, true, true),
    };
    if launcher || profile {
        (send, recv) = (false, false);
    }
    let line = |id: &str, ok: bool| {
        let k = format!("restrictions_screen.permission.{id}.{}", if ok { "allowed" } else { "denied" });
        format!("{}{}", if ok { "§a" } else { "§c" }, t(gui, &k))
    };
    y += 4;
    for (id, ok) in [
        ("send_messages", send),
        ("send_commands", cmds),
        ("receive_system_messages", sys),
        ("receive_player_messages", recv),
    ] {
        for l in wrap(&line(id, ok)) {
            v.push(label(0, 0, y, sw, l, 0xffff_ffff, true));
            y += 9;
        }
    }
    v.push(Widget::button(DONE, (sw - 200) / 2, sh - 26, 200, &t(gui, "gui.done")));
    (v, Some(List::new(0, 33, sw, sh - 66, 250, 9)))
}

/// Confirmations: difficulty lock (pop-up window), abandoning the rules, link.
fn build_confirm(c: Confirm, gui: &Gui, w: &World, sw: i32, sh: i32) -> Vec<Widget> {
    let mut v = Vec::new();
    match c {
        Confirm::LockDifficulty => {
            // Window of 250 (content 214): bold title, question, Yes / No; margins of 12.
            let dname =
                t(gui, &format!("options.difficulty.{}", DIFFICULTIES[w.wanted_difficulty.clamp(0, 3) as usize]));
            let title = rc_text::wrap(gui, &chars(&format!("§l{}", t(gui, "difficulty.lock.title"))), 214.0);
            let msg = rc_text::wrap(gui, &chars(&tr(&gui.lang, "difficulty.lock.question", &[dname])), 214.0);
            let h = 9 * title.len() as i32 + 12 + 9 * msg.len() as i32 + 12 + 20;
            let mut y = (sh - h) / 2;
            for l in title {
                v.push(label(0, 0, y, sw, l, 0xffff_ffff, true));
                y += 9;
            }
            y += 12;
            for l in msg {
                v.push(label(0, 0, y, sw, l, 0xffff_ffff, true));
                y += 9;
            }
            y += 12;
            let x = sw / 2 - 107;
            v.push(Widget::button(YES, x, y, 104, &t(gui, "gui.yes")));
            v.push(Widget::button(NO, x + 110, y, 104, &t(gui, "gui.no")));
        }
        Confirm::DiscardRules | Confirm::Link => {
            let (title, msg) = match c {
                Confirm::DiscardRules => (
                    t(gui, "editGamerule.inGame.discardChanges.title"),
                    t(gui, "editGamerule.inGame.discardChanges.message"),
                ),
                _ => (t(gui, "chat.link.confirmTrusted"), w.url.clone()),
            };
            let lines: Vec<_> = rc_text::wrap(gui, &chars(&msg), (sw - 50) as f32).into_iter().take(15).collect();
            let h = 9 + 8 + 9 * lines.len() as i32 + 8 + 16 + 20;
            let mut y = (sh - h) / 2;
            v.push(label(0, 0, y, sw, chars(&title), 0xffff_ffff, true));
            y += 9 + 8;
            for l in lines {
                v.push(label(0, 0, y, sw, l, 0xffff_ffff, true));
                y += 9;
            }
            y += 8 + 16;
            if c == Confirm::Link {
                let x = sw / 2 - 154;
                v.push(Widget::button(YES, x, y, 100, &t(gui, "chat.link.open")));
                v.push(Widget::button(COPY, x + 104, y, 100, &t(gui, "chat.copy")));
                v.push(Widget::button(NO, x + 208, y, 100, &t(gui, "gui.cancel")));
            } else {
                let x = sw / 2 - 152;
                v.push(Widget::button(YES, x, y, 150, &t(gui, "gui.yes")));
                v.push(Widget::button(NO, x + 154, y, 150, &t(gui, "gui.no")));
            }
        }
    }
    v
}

/// Social interactions: tabs, search, players (36 per row), blocking link, "Done".
fn build_social(gui: &Gui, w: &mut World, search: &str, sw: i32, sh: i32) -> (Vec<Widget>, Option<List>) {
    let window_h = (sh - 128 - 16).max(52);
    let list_end = 80 + window_h - 8;
    let margin_x = (sw - 238) / 2;
    let mut l = List::new(0, 88, sw, list_end - 88, 220, 36);
    l.background = false;
    l.separators = false;
    l.custom_rows = true;
    let (left, right) = (l.row_left(), l.row_left() + 220);
    let bw = 220 / 3;
    let tabs = [
        (left, "gui.socialInteractions.tab_all"),
        ((left + right - bw) / 2 + 1, "gui.socialInteractions.tab_hidden"),
        (right - bw + 1, "gui.socialInteractions.tab_blocked"),
    ];
    let mut v = Vec::new();
    for (k, (x, key)) in tabs.iter().enumerate() {
        let text = if k as u8 == w.social_page { format!("§n{}", t(gui, key)) } else { t(gui, key) };
        v.push(Widget::button(S_TAB + k as u32, *x, 45, bw, &text));
    }
    let mut field = TextField::new(chars(&format!("§7§o{}", t(gui, "gui.socialInteractions.search_hint"))));
    field.text = search.chars().collect();
    field.cursor = field.text.len();
    field.anchor = field.cursor;
    field.max_len = 16;
    v.push(Widget::new(SEARCH, Kind::Field(Box::new(field)), margin_x + 28, 74, 200, 15, Vec::new()));
    if w.social_page == 2 {
        v.push(Widget::button(
            S_BLOCKING,
            sw / 2 - 100,
            64 + window_h,
            200,
            &t(gui, "gui.socialInteractions.blocking_hint"),
        ));
    }
    v.push(Widget::button(DONE, (sw - 200) / 2, sh - 26, 200, &t(gui, "gui.done")));
    // Players of the tab, filtered and sorted by name (case-insensitive).
    let hidden = rc_client::chat::hidden_players();
    let needle = search.to_lowercase();
    let mut list: Vec<Player> = match w.social_page {
        0 => w.players.clone(),
        1 => hidden.iter().filter_map(|u| w.seen.iter().find(|p| p.uuid == *u).cloned()).collect(),
        // Blocked: online players from the account's block list (`rc_client::session`).
        _ => w.players.iter().filter(|p| rc_client::session::is_blocked(p.uuid)).cloned().collect(),
    };
    list.retain(|p| needle.is_empty() || p.name.to_lowercase().contains(&needle));
    list.sort_by_key(|p| p.name.to_lowercase());
    w.shown = list.iter().map(|p| p.uuid).collect();
    let mut top = 0;
    for (i, p) in list.iter().enumerate() {
        l.entries.push(Entry { top, h: 36, label: chars(&p.name) });
        if p.uuid != w.local {
            let by = 2 + top + 8;
            let is_hidden = hidden.contains(&p.uuid);
            let (sprite, key, tip) = if is_hidden {
                (
                    "social_interactions/unmute_button",
                    "gui.socialInteractions.show",
                    "gui.socialInteractions.tooltip.show",
                )
            } else {
                (
                    "social_interactions/mute_button",
                    "gui.socialInteractions.hide",
                    "gui.socialInteractions.tooltip.hide",
                )
            };
            let mut hb =
                Widget::new(S_HIDE + i as u32, Kind::Sprite(sprite), right - 24 - 24, by, 20, 20, chars(&t(gui, key)))
                    .with_tooltip(gui, &t(gui, tip));
            hb.in_list = true;
            v.push(hb);
            // Report: service unavailable (offline).
            let mut rb = Widget::new(
                S_REPORT + i as u32,
                Kind::Sprite("social_interactions/report_button"),
                right - 24,
                by,
                20,
                20,
                chars(&t(gui, "gui.socialInteractions.report")),
            )
            .with_tooltip(gui, &t(gui, "gui.socialInteractions.tooltip.report.disabled"));
            rb.active = false;
            rb.in_list = true;
            v.push(rb);
        }
        top += 36;
    }
    l.content = top + 4;
    (v, Some(l))
}

/// Event of a component of these screens.
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle(
    id: Id,
    e: &Event,
    gui: &Gui,
    o: &mut Options,
    w: &mut World,
    panel: &mut Panel,
    search: &str,
    out: &mut Vec<Action>,
) -> Step {
    match (id, e) {
        (_, Event::Press(DONE)) if id != Id::GameRules => Step::Pop(1),
        (Id::WorldOptions, Event::Press(CANCEL)) => Step::Pop(1),
        (Id::WorldOptions, Event::Cycle(W_DIFF, dir)) => {
            w.wanted_difficulty = (w.wanted_difficulty + dir).rem_euclid(4);
            Step::Rebuild
        }
        (Id::WorldOptions, Event::Press(W_LOCK)) => {
            w.wanted_locked = !w.wanted_locked;
            Step::Rebuild
        }
        (Id::WorldOptions, Event::Cycle(W_DEFAULT_MODE, dir)) => {
            w.wanted_default_mode = (w.wanted_default_mode + dir).rem_euclid(4);
            Step::Rebuild
        }
        (Id::WorldOptions, Event::Cycle(W_MODE, dir)) => {
            w.wanted_mode = (w.wanted_mode + dir).rem_euclid(4);
            Step::Rebuild
        }
        (Id::WorldOptions, Event::Cycle(W_COMMANDS, _)) => {
            w.wanted_commands = !w.wanted_commands;
            w.commands_changed();
            Step::Rebuild
        }
        (Id::WorldOptions, Event::Cycle(W_LAN, _)) => {
            w.wanted_lan = !w.wanted_lan;
            w.lan_toggled();
            Step::Rebuild
        }
        (Id::WorldOptions, Event::Cycle(W_GUEST, _)) => {
            w.wanted_guest = !w.wanted_guest;
            w.update_force();
            Step::Rebuild
        }
        (Id::WorldOptions, Event::Cycle(W_FORCE, _)) => {
            w.wanted_force = !w.wanted_force;
            Step::Rebuild
        }
        (Id::WorldOptions, Event::Edit(W_PORT)) => {
            let Some(Kind::Field(f)) = panel.get(W_PORT).map(|x| &x.kind) else { return Step::None };
            w.port = f.value();
            let p = w.port.clone();
            w.port_err = w.parse_port(&p);
            // Colour, tooltip and hint of the field, "Apply": without rebuilding (focus kept).
            if let Some(wd) = panel.get_mut(W_PORT) {
                if let Kind::Field(f) = &mut wd.kind {
                    f.text_color = port_color(w.port_err);
                    f.hint = chars(&w.port_num.to_string());
                }
                wd.set_tooltip(gui, &port_tip(gui, w.port_err));
            }
            if let Some(a) = panel.get_mut(W_APPLY) {
                a.active = w.has_changes();
            }
            Step::None
        }
        (Id::WorldOptions, Event::Press(W_RULES)) => {
            w.start_game_rules();
            out.push(Action::RequestGameRules);
            Step::Open(Id::GameRules)
        }
        (Id::WorldOptions, Event::Press(W_RESTRICT)) => Step::Open(Id::Restrictions),
        (Id::WorldOptions, Event::Press(W_APPLY)) => {
            if w.wanted_locked != (w.ctx.locked || w.ctx.hardcore) {
                Step::Open(Id::Confirm(Confirm::LockDifficulty))
            } else {
                apply_world_options(w, out);
                Step::Pop(1)
            }
        }
        (Id::Confirm(Confirm::LockDifficulty), Event::Press(YES)) => {
            apply_world_options(w, out);
            Step::Pop(2)
        }
        (Id::Confirm(_), Event::Press(NO)) => Step::Pop(1),
        (Id::Confirm(Confirm::DiscardRules), Event::Press(YES)) => {
            w.edits.clear();
            Step::Pop(2)
        }
        (Id::Confirm(Confirm::Link), Event::Press(YES)) => {
            out.push(Action::OpenUrl(w.url.clone()));
            Step::Pop(1)
        }
        (Id::Confirm(Confirm::Link), Event::Press(COPY)) => {
            crate::widgets::copy_to_clipboard(w.url.clone());
            Step::Pop(1)
        }
        (Id::GameRules, Event::Edit(SEARCH)) => Step::Rebuild,
        (Id::GameRules, Event::Press(DONE)) => {
            let changed = w.changed_rules();
            if !changed.is_empty() {
                out.push(Action::SetGameRules(changed));
            }
            w.edits.clear();
            Step::Pop(1)
        }
        (Id::GameRules, Event::Press(CANCEL)) => escape(id, w),
        (Id::GameRules, Event::Cycle(b, _)) if *b >= RULE => {
            let Some(key) = rule_at(gui, w, search, b - RULE) else { return Step::None };
            let cur = w.rule_value(&key) == Some("true");
            w.set_edit(&key, (!cur).to_string());
            Step::Rebuild
        }
        (Id::GameRules, Event::Edit(b)) if *b >= RULE => {
            let Some(key) = rule_at(gui, w, search, b - RULE) else { return Step::None };
            let Some(wd) = panel.get_mut(*b) else { return Step::None };
            let Kind::Field(f) = &mut wd.kind else { return Step::None };
            let text = f.value();
            let valid = rule_info(&key).is_some_and(|r| r.parse(&text).is_some());
            f.text_color = if valid { 0xffe0_e0e0 } else { 0xffff_0000 };
            w.set_edit(&key, text);
            let invalid = w.rules_invalid();
            if let Some(d) = panel.get_mut(DONE) {
                d.active = !invalid;
            }
            Step::None
        }
        (Id::Restrictions, Event::Press(R_ACTION)) => Step::Open(Id::Chat),
        (Id::Restrictions, Event::Press(R_ACCOUNT)) => {
            w.url = ACCOUNT_SETTINGS_URL.to_owned();
            Step::Open(Id::Confirm(Confirm::Link))
        }
        (Id::Credits, Event::Press(C_CREDITS)) => {
            out.push(Action::Credits);
            Step::None
        }
        (Id::Credits, Event::Press(b)) if *b == C_ATTRIBUTION || *b == C_LICENSES => {
            w.url = if *b == C_ATTRIBUTION { ATTRIBUTION_URL } else { LICENSES_URL }.to_owned();
            Step::Open(Id::Confirm(Confirm::Link))
        }
        (Id::Social, Event::Press(b)) if (S_TAB..S_TAB + 3).contains(b) => {
            w.social_page = (b - S_TAB) as u8;
            Step::Rebuild
        }
        (Id::Social, Event::Press(S_BLOCKING)) => {
            w.url = BLOCKING_URL.to_owned();
            Step::Open(Id::Confirm(Confirm::Link))
        }
        (Id::Social, Event::Edit(SEARCH)) => Step::Rebuild,
        (Id::Social, Event::Press(b)) if (S_HIDE..S_REPORT).contains(b) => {
            let Some(&uuid) = w.shown.get((b - S_HIDE) as usize) else { return Step::None };
            let hide = !rc_client::chat::is_hidden(uuid);
            rc_client::chat::set_hidden(uuid, hide);
            let name = w.seen.iter().find(|p| p.uuid == uuid).map(|p| p.name.clone()).unwrap_or_default();
            out.push(Action::HidePlayer(hide, name));
            Step::Rebuild
        }
        _ => {
            let _ = o;
            Step::None
        }
    }
}

/// Packets of the world options (difficulty, lock; personal mode in singleplayer).
fn apply_world_options(w: &World, out: &mut Vec<Action>) {
    if w.wanted_difficulty != w.ctx.difficulty {
        out.push(Action::ChangeDifficulty(w.wanted_difficulty));
    }
    if w.wanted_locked && !(w.ctx.locked || w.ctx.hardcore) {
        out.push(Action::LockDifficulty);
    }
    if w.ctx.solo && w.wanted_mode != w.ctx.game_mode {
        out.push(Action::ChangeGameMode(w.wanted_mode));
    }
    if w.ctx.solo && w.lan_changed() {
        let port = w.port_num;
        let force = w.wanted_force.then_some(w.wanted_default_mode);
        out.push(Action::Publish { open: w.wanted_lan, port, guest: w.wanted_guest, force });
    }
}

/// Free TCP port (the system picks one), like the game's screen.
fn available_port() -> u16 {
    std::net::TcpListener::bind((rc_sim::lan::announce::bind_addr(), 0))
        .and_then(|l| l.local_addr())
        .map_or(25564, |a| a.port())
}

/// Free port for opening to the local network (listen attempt, closed right away).
fn port_free(p: u16) -> bool {
    std::net::TcpListener::bind((rc_sim::lan::announce::bind_addr(), p)).is_ok_and(|l| l.local_addr().is_ok_and(|a| a.port() == p))
}

/// Colour of the port field text: light grey, red if invalid.
fn port_color(err: Option<&str>) -> u32 {
    if err.is_some() { 0xffdf_5050 } else { 0xffe0_e0e0 }
}

/// Tooltip of the port field: its role, or the error with the bounds.
fn port_tip(gui: &Gui, err: Option<&str>) -> String {
    match err {
        Some(k) => tr(&gui.lang, k, &["1024".to_owned(), "65535".to_owned()]),
        None => t(gui, "lanServer.port"),
    }
}

/// Escape not consumed by the components.
pub(crate) fn escape(id: Id, w: &mut World) -> Step {
    match id {
        Id::GameRules if !w.edits.is_empty() => Step::Open(Id::Confirm(Confirm::DiscardRules)),
        _ => Step::Pop(1),
    }
}

/// Texts specific to the screens (drawn before the components).
pub(crate) fn draw(id: Id, gui: &mut Gui, q: &wgpu::Queue, w: &World, panel: &Panel, time: f64) {
    let (sw, sh) = (gui.width as i32, gui.height as i32);
    match id {
        Id::GameRules if w.rules.is_none() => {
            let y = (36 + 30).min(sh - 33 - 22);
            let text = chars(&t(gui, "editGamerule.inGame.downloadingGamerules"));
            crate::widgets::centered(gui, q, &text, sw / 2, y, 0xffff_ffff);
            const DOTS: [&str; 4] = ["O o o", "o O o", "o o O", "o O o"];
            let dots = chars(DOTS[((time * 1000.0) as u64 / 300 % 4) as usize]);
            crate::widgets::centered(gui, q, &dots, sw / 2, y + 13, 0xff80_8080);
        }
        Id::Confirm(Confirm::LockDifficulty) => {
            // Window background around the content (margin 18).
            let ys: Vec<i32> = panel.widgets.iter().map(|x| x.y).collect();
            let (top, bottom) = (ys.iter().min().copied().unwrap_or(0), ys.iter().max().copied().unwrap_or(0) + 20);
            let x = sw / 2 - 107;
            gui.sprite(
                "popup/background",
                (x - 18) as f32,
                (top - 18) as f32,
                250.0,
                (bottom - top + 36) as f32,
                0xffff_ffff,
            );
        }
        Id::Social => {
            let window_h = (sh - 128 - 16).max(52);
            let margin_x = (sw - 238) / 2;
            gui.sprite(
                "social_interactions/background",
                (margin_x + 3) as f32,
                64.0,
                236.0,
                (window_h + 16) as f32,
                0xffff_ffff,
            );
            gui.sprite("icon/search", (margin_x + 13) as f32, 76.0, 12.0, 12.0, 0xffff_ffff);
            let n = w.players.len();
            let key = if n > 1 {
                "gui.socialInteractions.server_label.multiple"
            } else {
                "gui.socialInteractions.server_label.single"
            };
            let server = chars(&tr(&gui.lang, key, &[w.server_name.clone(), n.to_string()]));
            gui.text(q, &server, (margin_x + 8) as f32, 35.0, 0xffff_ffff, true, 0);
            let list_end = 80 + window_h - 8;
            if w.shown.is_empty() {
                let searching =
                    panel.get(SEARCH).is_some_and(|x| matches!(&x.kind, Kind::Field(f) if !f.text.is_empty()));
                let key = if searching {
                    Some("gui.socialInteractions.search_empty")
                } else if w.social_page == 1 {
                    Some("gui.socialInteractions.empty_hidden")
                } else if w.social_page == 2 {
                    Some("gui.socialInteractions.empty_blocked")
                } else {
                    None
                };
                if let Some(k) = key {
                    let text = chars(&format!("§7{}", t(gui, k)));
                    crate::widgets::centered(gui, q, &text, sw / 2, (72 + list_end) / 2, 0xffff_ffff);
                }
            }
        }
        _ => {}
    }
}

/// Player rows (background, name, state) in the clip of the list.
pub(crate) fn draw_social_rows(gui: &mut Gui, q: &wgpu::Queue, w: &World, l: &List) {
    let hidden = rc_client::chat::hidden_players();
    for (i, uuid) in w.shown.iter().enumerate() {
        let Some((x, y, rw, rh)) = l.entry_rect(i) else { continue };
        if y + rh < l.y || y > l.y + l.h {
            continue;
        }
        let online = w.players.iter().any(|p| p.uuid == *uuid);
        let is_hidden = hidden.contains(uuid);
        let is_blocked = rc_client::session::is_blocked(*uuid);
        let status = match (is_blocked, is_hidden, online) {
            (true, _, false) => Some("gui.socialInteractions.status_blocked_offline"),
            (_, true, false) => Some("gui.socialInteractions.status_hidden_offline"),
            (true, _, true) => Some("gui.socialInteractions.status_blocked"),
            (_, true, true) => Some("gui.socialInteractions.status_hidden"),
            (false, false, false) => Some("gui.socialInteractions.status_offline"),
            _ => None,
        };
        let bg = if status.is_some() { 0xff30_3030 } else { 0xff4a_4a4a };
        gui.fill(x as f32, y as f32, (x + rw) as f32, (y + rh) as f32, bg);
        let tx = x + 4 + 24 + 4;
        let name = w.seen.iter().find(|p| p.uuid == *uuid).map(|p| p.name.as_str()).unwrap_or("");
        let ty = match status {
            Some(k) => {
                let ty = y + (rh - 18) / 2;
                let s = chars(&format!("§o{}", t(gui, k)));
                gui.text(q, &s, tx as f32, (ty + 12) as f32, 0x8cff_ffff, false, 0);
                ty
            }
            None => y + (rh - 9) / 2,
        };
        gui.text(q, &chars(name), tx as f32, ty as f32, 0xffff_ffff, false, 0);
        if !online {
            gui.fill((x + 4) as f32, (y + 6) as f32, (x + 28) as f32, (y + 30) as f32, 0xbe00_0000);
        }
    }
}

// ---------- Application ----------

impl World {
    /// Tags of the dialog registry: those of the pause menu (`pause_screen_additions`).
    pub fn set_tags(&mut self, tags: &rc_proto_v777::play::Tags) {
        if let Some((_, list)) = tags.iter().find(|(r, _)| r == "minecraft:dialog" || r == "dialog") {
            self.pause_tag = list
                .iter()
                .find(|(t, _)| t == "minecraft:pause_screen_additions" || t == "pause_screen_additions")
                .map(|(_, ids)| ids.clone())
                .unwrap_or_default();
        }
    }
}

/// Pause menu button added by the server: single dialog of the tag, otherwise "custom
/// options" (several), otherwise "server links" if there are links.
fn pause_dialog(tag: &[i32], dialogs: &rc_client::RegistryEntries, links: bool) -> Option<i32> {
    let by_name =
        |n: &str| dialogs.iter().position(|(k, _)| k == n || k.strip_prefix("minecraft:") == Some(n)).map(|i| i as i32);
    match tag.len() {
        1 => Some(tag[0]),
        0 if links => by_name("server_links"),
        0 => None,
        _ => by_name("custom_options"),
    }
}

impl crate::App {
    /// Menu tick: game state for the world options, players, server button,
    /// reconfiguration, end of the welcome, cursor shape.
    pub(crate) fn world_menus_tick(&mut self) {
        let ctx = Ctx {
            difficulty: self.exact.difficulty.0,
            locked: self.exact.difficulty.1,
            hardcore: self.hud.hardcore,
            op_level: self.debug.op_level,
            game_mode: self.env.game_mode,
            solo: crate::world_list::solo_open(),
        };
        if !ctx.solo {
            self.menus.world.published = None;
        }
        self.menus.set_world(ctx);
        let top = self.menus.top();
        let w = &mut self.menus.world;
        w.local = self.local_uuid;
        if top == Some(Id::Social) || top == Some(Id::Pause) {
            let mut players: Vec<Player> =
                self.players.map.iter().map(|(u, p)| Player { uuid: *u, name: p.name.clone() }).collect();
            players.sort_by_key(|p| p.uuid);
            if players != w.players {
                w.set_players(players);
            }
        }
        // Server button in the pause menu (pause dialog or server links).
        let id = pause_dialog(&w.pause_tag, &self.exact.dialogs, !self.exact.links.is_empty());
        if id != w.pause_dialog {
            let label = id.and_then(|i| self.dialog_external_title(i));
            let w = &mut self.menus.world;
            w.pause_dialog = id;
            w.pause_extra = label;
        }
        // Reconfiguration: the server sends the session back to configuration.
        if self.front.in_world
            && !self.front.is_reconfiguring()
            && self.session.link.names.phase() == rc_data::Phase::Configuration
        {
            self.menus.close_all();
            self.front.open_reconfig();
            self.grab(false);
        }
        // Demonstration: `RUSTCRAFT_COCTEST=<text>` simulates a received code of conduct (once).
        if !self.menus.world.coc_test
            && let Some(text) = rc_data::env_str!("RUSTCRAFT_COCTEST")
            && self.started.elapsed().as_secs() >= 3
        {
            self.menus.world.coc_test = true;
            // Demonstration entry in `servers.dat` (memory of the acceptance).
            let (name, ip) = ("Code of conduct".to_owned(), "coc.test".to_owned());
            let dir = crate::servers::game_dir();
            let mut list = crate::servers::ServerList::load(&dir);
            if list.find(&ip).is_none() {
                list.add(crate::servers::ServerEntry::new(name.clone(), ip.clone()));
                let _ = list.save(&dir);
            }
            self.menus.world.server = Some((name, ip));
            self.on_code_of_conduct(text.to_owned());
        }
        if std::mem::take(&mut self.front.onboarding_done) {
            self.options.set("onboardAccessibility", Val::B(false));
            self.save_options();
        }
        self.apply_cursor();
    }

    /// External title of a dialog of the registry (otherwise its title).
    fn dialog_external_title(&self, i: i32) -> Option<Vec<StyledChar>> {
        let (name, content) = self.exact.dialogs.get(usize::try_from(i).ok()?)?;
        let nbt = match content {
            Some(c) => c.clone(),
            None => {
                let (ns, path) = name.split_once(':').unwrap_or(("minecraft", name));
                let bytes = self.assets.read(&format!("data/{ns}/dialog/{path}.json"))?;
                crate::dialog::json_to_nbt(&serde_json::from_slice(&bytes).ok()?)
            }
        };
        let field = |k: &str| match &nbt {
            rc_proto::nbt::Nbt::Compound(v) => v.iter().find(|(n, _)| n == k).map(|(_, v)| v),
            _ => None,
        };
        let c = field("external_title").or_else(|| field("title")).and_then(rc_text::from_nbt)?;
        let lang = &self.renderer.as_ref()?.gui.lang;
        Some(rc_text::styled_chars(&rc_text::flatten(&c, lang)))
    }

    /// Cursor shape requested by the interface (`allowCursorChanges`).
    fn apply_cursor(&mut self) {
        use crate::widgets::Cursor;
        use winit::window::CursorIcon;
        let want = crate::widgets::take_cursor();
        let want = if self.options.b("allowCursorChanges") { want } else { Cursor::Arrow };
        if want == self.menus.world.cursor {
            return;
        }
        self.menus.world.cursor = want;
        let icon = match want {
            Cursor::Arrow => CursorIcon::Default,
            Cursor::Hand => CursorIcon::Pointer,
            Cursor::IBeam => CursorIcon::Text,
            Cursor::NotAllowed => CursorIcon::NotAllowed,
            Cursor::ResizeEw => CursorIcon::EwResize,
            Cursor::ResizeNs => CursorIcon::NsResize,
            Cursor::ResizeAll => CursorIcon::AllScroll,
        };
        if let Some(r) = &self.renderer {
            r.window.set_cursor(icon);
        }
    }

    /// Requests from the screens of this module.
    pub(crate) fn world_menu_action(&mut self, a: Action) {
        let link = self.session.link.clone();
        match a {
            Action::ChangeDifficulty(d) => {
                let _ = link.send_change_difficulty(d);
            }
            Action::LockDifficulty => {
                let _ = link.send_lock_difficulty(true);
            }
            Action::ChangeGameMode(m) => {
                let _ = link.send_change_game_mode(m);
            }
            Action::RequestGameRules => {
                // Action 2 of `client_command`: game rule values.
                let _ = link.send_client_command(2);
            }
            Action::SetGameRules(rules) => {
                let _ = link.send_set_game_rules(&rules);
            }
            Action::PauseExtra => {
                if let Some(i) = self.menus.world.pause_dialog {
                    self.menus.close_all();
                    self.show_dialog(rc_client::extra::DialogRef::Registry(i), false);
                }
            }
            Action::Credits => self.open_credits(),
            Action::OpenUrl(url) => open_url(&url),
            Action::HidePlayer(hidden, name) => {
                let key = if hidden {
                    "gui.socialInteractions.hidden_in_chat"
                } else {
                    "gui.socialInteractions.shown_in_chat"
                };
                let c = rc_text::Component::translate(key, vec![rc_text::Arg::Text(name)]);
                self.chat.add_client(c);
            }
            Action::Publish { open, port, guest, force } => {
                // The integrated server writes the opening message into the chat.
                if let Some(h) = &self.solo {
                    h.publish(rc_sim::lan::Publish { open, port, guest_commands: guest, force_mode: force });
                    self.menus.world.published = open.then_some((port, guest, force.is_some()));
                }
            }
            _ => {}
        }
    }

    /// "Credits": scroll of the game's credits (`texts/credits.json`).
    fn open_credits(&mut self) {
        let bytes = self.assets.read("assets/minecraft/texts/credits.json").unwrap_or_default();
        self.menus.roll = Some(crate::credits_roll::CreditsRoll::new(&bytes));
    }

    /// Requests from the out-of-world screens of `notices.rs`.
    pub(crate) fn world_front_action(&mut self, a: crate::front::Action) {
        match a {
            crate::front::Action::Conduct { accepted, remember, text } => {
                self.conduct_answer(&text, accepted, remember)
            }
            crate::front::Action::LeaveServer => self.disconnect_to_menu(),
            _ => {}
        }
    }
}

/// Opening of an address in the system browser.
fn open_url(url: &str) {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return;
    }
    let r = if cfg!(windows) {
        std::process::Command::new("explorer").arg(url).spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(url).spawn()
    };
    if let Err(e) = r {
        eprintln!("link {url}: {e}");
    }
}

// ---------- Rules of a world to create (`create_world.rs`) ----------

/// Game rules of the creation screen: same components and controls as the in-game screen,
/// initial values = game defaults + rules already chosen.
pub(crate) struct RulesEditor {
    w: World,
}

/// Follow-up of an event of the creation rules screen.
pub(crate) enum RulesOut {
    None,
    Rebuild,
    /// "Done": rules different from the defaults (full key, value).
    Done(Vec<(String, String)>),
    Cancel,
}

impl RulesEditor {
    pub(crate) fn new(values: &[(String, String)]) -> RulesEditor {
        let mut w = World {
            rules: Some(RULES.iter().map(|r| (format!("minecraft:{}", r.id), r.default_text())).collect()),
            ..Default::default()
        };
        for (k, v) in values {
            w.set_edit(k, v.clone());
        }
        RulesEditor { w }
    }

    pub(crate) fn build(&self, gui: &Gui, search: &str) -> (Vec<Widget>, Option<List>) {
        build_rules(gui, &self.w, search, gui.width as i32, gui.height as i32)
    }

    /// Search text.
    pub(crate) fn search(panel: &Panel) -> String {
        match panel.get(SEARCH).map(|w| &w.kind) {
            Some(Kind::Field(f)) => f.value(),
            _ => String::new(),
        }
    }

    pub(crate) fn handle(&mut self, gui: &Gui, search: &str, panel: &mut Panel, e: &Event) -> RulesOut {
        match e {
            Event::Edit(SEARCH) => RulesOut::Rebuild,
            Event::Press(DONE) => RulesOut::Done(self.w.changed_rules()),
            Event::Press(CANCEL) => RulesOut::Cancel,
            Event::Cycle(b, _) if *b >= RULE => {
                let Some(key) = rule_at(gui, &self.w, search, b - RULE) else { return RulesOut::None };
                let cur = self.w.rule_value(&key) == Some("true");
                self.w.set_edit(&key, (!cur).to_string());
                RulesOut::Rebuild
            }
            Event::Edit(b) if *b >= RULE => {
                let Some(key) = rule_at(gui, &self.w, search, b - RULE) else { return RulesOut::None };
                let Some(wd) = panel.get_mut(*b) else { return RulesOut::None };
                let Kind::Field(f) = &mut wd.kind else { return RulesOut::None };
                let text = f.value();
                let valid = rule_info(&key).is_some_and(|r| r.parse(&text).is_some());
                f.text_color = if valid { 0xffe0_e0e0 } else { 0xffff_0000 };
                self.w.set_edit(&key, text);
                let invalid = self.w.rules_invalid();
                if let Some(d) = panel.get_mut(DONE) {
                    d.active = !invalid;
                }
                RulesOut::None
            }
            _ => RulesOut::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pause_button_follows_tag_then_links() {
        let d: rc_client::RegistryEntries =
            vec![("minecraft:server_links".into(), None), ("minecraft:custom_options".into(), None)];
        assert_eq!(pause_dialog(&[], &d, false), None);
        assert_eq!(pause_dialog(&[], &d, true), Some(0));
        assert_eq!(pause_dialog(&[7], &d, true), Some(7));
        assert_eq!(pause_dialog(&[3, 4], &d, false), Some(1));
    }

    #[test]
    fn rule_values_are_checked_against_bounds() {
        let r = rule_info("minecraft:max_snow_accumulation_height").unwrap();
        assert_eq!(r.parse("8"), Some("8".into()));
        assert_eq!(r.parse("9"), None);
        assert_eq!(r.parse("x"), None);
        let b = rule_info("keep_inventory").unwrap();
        assert_eq!(b.parse("true"), Some("true".into()));
        assert_eq!(b.parse("1"), None);
        assert_eq!(rule_info("fire_spread_radius_around_player").unwrap().parse("-1"), Some("-1".into()));
        assert!(rule_info("minecraft:unknown").is_none());
    }

    #[test]
    fn edits_back_to_initial_value_are_dropped() {
        let mut w = World { rules: Some(vec![("minecraft:pvp".into(), "true".into())]), ..Default::default() };
        w.set_edit("minecraft:pvp", "false".into());
        assert_eq!(w.changed_rules(), vec![("minecraft:pvp".to_owned(), "false".to_owned())]);
        w.set_edit("minecraft:pvp", "true".into());
        assert!(w.changed_rules().is_empty());
    }

    #[test]
    fn lock_change_counts_as_change() {
        let mut w = World { ctx: Ctx { difficulty: 2, op_level: 4, ..Default::default() }, ..Default::default() };
        w.start_world_options();
        assert!(!w.has_changes());
        w.wanted_locked = true;
        assert!(w.has_changes());
    }

    fn solo(op: i32, published: Option<(u16, bool, bool)>) -> World {
        let mut w = World {
            ctx: Ctx { difficulty: 2, op_level: op, game_mode: 1, solo: true, ..Default::default() },
            published,
            ..Default::default()
        };
        w.start_world_options();
        w
    }

    #[test]
    fn guest_commands_and_forced_mode_follow_lan_and_commands() {
        // World open, commands and operator guests, mode not forced.
        let mut w = solo(4, Some((25600, true, false)));
        assert!(w.wanted_guest && !w.wanted_force);
        // Commands off: guests forced to no, forced mode taken back from the server, modes brought back.
        w.wanted_default_mode = 3;
        w.wanted_mode = 2;
        w.wanted_commands = false;
        w.commands_changed();
        assert!(!w.wanted_guest);
        assert!(!w.wanted_force);
        assert_eq!((w.wanted_default_mode, w.wanted_mode), (1, 1));
        // Commands restored: server value; operator guests => forced mode off.
        w.wanted_commands = true;
        w.commands_changed();
        assert!(w.wanted_guest && !w.wanted_force);
        // Local network off: guests no, forced mode yes.
        w.wanted_lan = false;
        w.lan_toggled();
        assert!(!w.wanted_guest && w.wanted_force);
        // World closed at opening: guests no, forced mode yes; hardcore: always forced.
        let w = solo(0, None);
        assert!(!w.wanted_guest && w.wanted_force);
        let mut h = solo(4, Some((25600, false, false)));
        h.ctx.hardcore = true;
        h.update_force();
        assert!(h.wanted_force);
    }

    #[test]
    fn lan_port_outside_bounds_blocks_apply() {
        let mut w = solo(4, None);
        w.wanted_lan = true;
        w.lan_toggled();
        assert!(w.has_changes());
        assert_eq!(w.parse_port("80"), Some("lanServer.port.invalid"));
        assert_eq!(w.parse_port("70000"), Some("lanServer.port.invalid"));
        assert_eq!(w.parse_port("abc"), Some("lanServer.port.invalid"));
        w.port_err = w.parse_port("80");
        assert!(!w.has_changes(), "an invalid port greys out Apply");
        // Port taken by another program: unavailable.
        let l = std::net::TcpListener::bind((rc_sim::lan::announce::bind_addr(), 0)).unwrap();
        let taken = l.local_addr().unwrap().port();
        if taken >= 1024 {
            assert_eq!(w.parse_port(&taken.to_string()), Some("lanServer.port.unavailable"));
        }
        assert_eq!(w.parse_port(""), None);
        assert!(w.port_num >= 1);
        // The port of the already open world is not tested (it is taken by us).
        let mut o = solo(4, Some((taken, false, true)));
        assert_eq!(o.parse_port(&taken.to_string()), None);
        assert_eq!(o.port_num, taken);
        drop(l);
    }
}
