//! "Create New World" (`docs/specs/world-screens.md` § 2): Game / World / More tabs,
//! name and folder, mode, difficulty, commands, world type (flat and single-biome worlds
//! customisable), seed, structures, bonus chest, game rules, experiments and data
//! packs; "Create" starts the generation (`rc_sim::newworld`) as a background task then opens the
//! world. Also "Re-Create" (settings re-read from an existing world).

use std::path::Path;
use std::sync::{Arc, Mutex};

use rc_data::VersionData;
use rc_proto::nbt::Nbt;
use rc_sim::newworld::{NewWorld, Preset};
use rc_sim::storage::CreationData;
use rc_text::{Component, StyledChar};
use winit::keyboard::KeyCode;

use crate::gui::Gui;
use crate::options::Options;
use crate::server_list::Column;
use crate::widgets::{self, Entry, Event, Kind, List, Panel, TextField, Widget, chars, tr};
use crate::world_list::{In, Job, Msg, background, colored, feed, italic, saves_dir, t};
use crate::world_options::{RulesEditor, RulesOut};

// ---------- Shared data ----------

static SHARED: Mutex<Option<Arc<VersionData>>> = Mutex::new(None);
/// Biomes and flat presets (read once, in the background).
static CREATION: Mutex<Option<Arc<CreationData>>> = Mutex::new(None);
static LOADING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_shared_data(data: Arc<VersionData>) {
    *SHARED.lock().unwrap_or_else(|p| p.into_inner()) = Some(data);
}

pub fn shared_data() -> Option<Arc<VersionData>> {
    SHARED.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

/// Biomes and flat presets, read in the background on first request.
fn creation_data(data: &Arc<VersionData>) -> Option<Arc<CreationData>> {
    if let Some(c) = CREATION.lock().unwrap_or_else(|p| p.into_inner()).clone() {
        return Some(c);
    }
    if !LOADING.swap(true, std::sync::atomic::Ordering::AcqRel) {
        let data = data.clone();
        let _ = std::thread::Builder::new().name("creation-data".into()).spawn(move || {
            let c = rc_sim::storage::creation_data(&data).unwrap_or_else(|e| {
                eprintln!("creation data: {e}");
                CreationData::default()
            });
            *CREATION.lock().unwrap_or_else(|p| p.into_inner()) = Some(Arc::new(c));
        });
    }
    None
}

// ---------- Settings ----------

/// Data packs of the world to create.
#[derive(Debug, Clone, PartialEq)]
pub struct Packs {
    pub enabled: Vec<String>,
    pub disabled: Vec<String>,
    /// Features enabled (`minecraft:vanilla` + those of the experimental packs).
    pub features: Vec<String>,
}

/// Packs provided by the game: default, then experimental feature packs.
const BUILTIN: [&str; 4] = ["vanilla", "minecart_improvements", "redstone_experiments", "trade_rebalance"];
const FEATURE_PACKS: [&str; 3] = ["minecart_improvements", "redstone_experiments", "trade_rebalance"];

fn packs_of(selected: &[String]) -> Packs {
    let enabled = selected.to_vec();
    let disabled = BUILTIN.iter().filter(|p| !selected.iter().any(|s| s == *p)).map(|s| (*s).to_owned()).collect();
    let mut features = vec!["minecraft:vanilla".to_owned()];
    features.extend(selected.iter().filter(|s| FEATURE_PACKS.contains(&s.as_str())).map(|s| format!("minecraft:{s}")));
    Packs { enabled, disabled, features }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    Survival,
    Hardcore,
    Creative,
}

impl Mode {
    fn key(self) -> &'static str {
        match self {
            Mode::Survival => "survival",
            Mode::Hardcore => "hardcore",
            Mode::Creative => "creative",
        }
    }
}

const DIFFICULTIES: [&str; 4] = ["peaceful", "easy", "normal", "hard"];
/// World types of the `extended` tag (`worldgen/world_preset`): those of the `normal` tag (the
/// first `NORMAL_PRESETS`), then the debug world, offered only with Alt held.
const PRESETS: [&str; 6] = ["normal", "flat", "large_biomes", "amplified", "single_biome_surface", "debug_all_block_states"];
const NORMAL_PRESETS: usize = 5;
const DEBUG_PRESET: usize = 5;

/// Alt key held (extended list of buttons with an alternate list), tracked by the window.
static ALT_DOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn set_alt_down(down: bool) {
    ALT_DOWN.store(down, std::sync::atomic::Ordering::Relaxed);
}

/// Next world type: the list walked depends on Alt at the time of the click, the index is kept
/// (from debug without Alt, index 5 + 1 falls back into the short list).
fn cycle_preset(p: usize, d: i32, alt: bool) -> usize {
    let n = if alt { PRESETS.len() } else { NORMAL_PRESETS };
    (p as i32 + d).rem_euclid(n as i32) as usize
}

/// Default flat world settings (preset `minecraft:flat`).
fn default_flat() -> Nbt {
    let layer =
        |b: &str, h: i32| Nbt::Compound(vec![("block".into(), Nbt::String(b.into())), ("height".into(), Nbt::Int(h))]);
    Nbt::Compound(vec![
        ("biome".into(), Nbt::String("minecraft:plains".into())),
        ("features".into(), Nbt::Byte(0)),
        ("lakes".into(), Nbt::Byte(0)),
        (
            "layers".into(),
            Nbt::List(vec![
                layer("minecraft:bedrock", 1),
                layer("minecraft:dirt", 2),
                layer("minecraft:grass_block", 1),
            ]),
        ),
        (
            "structure_overrides".into(),
            Nbt::List(vec![Nbt::String("minecraft:strongholds".into()), Nbt::String("minecraft:villages".into())]),
        ),
    ])
}

/// Layers (block, height) of a flat setting, bottom to top.
fn layers(flat: &Nbt) -> Vec<(String, i32)> {
    rc_sim::nbtio::list(flat.get("layers"))
        .iter()
        .filter_map(|l| {
            let b = l.get("block")?.as_str()?.to_owned();
            let h = rc_sim::nbtio::int(l, "height").unwrap_or(1) as i32;
            Some((b, h))
        })
        .collect()
}

fn set_layers(flat: &mut Nbt, v: &[(String, i32)]) {
    let list = v
        .iter()
        .map(|(b, h)| Nbt::Compound(vec![("block".into(), Nbt::String(b.clone())), ("height".into(), Nbt::Int(*h))]))
        .collect();
    rc_sim::nbtio::set(flat, "layers", Nbt::List(list));
}

/// Sharing text of a flat setting: `2*minecraft:dirt,minecraft:grass_block;minecraft:plains`.
fn flat_text(flat: &Nbt) -> String {
    let l: Vec<String> =
        layers(flat).iter().map(|(b, h)| if *h > 1 { format!("{h}*{b}") } else { b.clone() }).collect();
    let biome = flat.get("biome").and_then(Nbt::as_str).unwrap_or("minecraft:plains");
    format!("{};{biome}", l.join(","))
}

/// Parsing the sharing text (layers limited to a total height of 4064); `None`: invalid.
fn parse_flat(text: &str, base: &Nbt, biomes: &[String]) -> Option<Nbt> {
    let mut parts = text.split(';');
    let first = parts.next()?;
    let mut out = Vec::new();
    let mut used = 0i32;
    for d in first.split(',') {
        let (h, b) = match d.split_once('*') {
            Some((n, b)) => (n.trim().parse::<i32>().ok()?.max(0), b),
            None => (1, d),
        };
        let b = b.trim();
        if b.is_empty() {
            return None;
        }
        let id = if b.contains(':') { b.to_owned() } else { format!("minecraft:{b}") };
        let h = h.min(4064 - used);
        if h <= 0 && used >= 4064 {
            break;
        }
        used += h;
        out.push((id, h));
    }
    if out.is_empty() {
        return None;
    }
    let mut n = base.clone();
    set_layers(&mut n, &out);
    let biome = parts
        .next()
        .map(|b| if b.contains(':') { b.to_owned() } else { format!("minecraft:{b}") })
        .filter(|b| biomes.is_empty() || biomes.contains(b))
        .unwrap_or_else(|| "minecraft:plains".into());
    rc_sim::nbtio::set(&mut n, "biome", Nbt::String(biome));
    Some(n)
}

/// Displayed name of a layer block (the block's item; water and lava: their bucket).
fn block_name(gui: &Gui, id: &str) -> String {
    let short = id.strip_prefix("minecraft:").unwrap_or(id);
    match short {
        "water" => t(gui, "item.minecraft.water_bucket"),
        "lava" => t(gui, "item.minecraft.lava_bucket"),
        _ => {
            let k = format!("block.minecraft.{short}");
            if gui.lang.map.contains_key(&k) { t(gui, &k) } else { t(gui, &format!("item.minecraft.{short}")) }
        }
    }
}

fn gv(gui: &Gui, name: &str, value: &str) -> String {
    tr(&gui.lang, "options.generic_value", &[t(gui, name), value.to_owned()])
}

fn on_off(gui: &Gui, v: bool) -> String {
    t(gui, if v { "options.on" } else { "options.off" })
}

// ---------- Main screen ----------

/// Colour of the World tab labels ("Structures", "Bonus Chest"): white in every
/// mode.
const LABEL_COLOR: u32 = 0xffff_ffff;

/// Fixed labels of a tab, the only ones drawn by the screen: key, offset from the origin,
/// colour. All white in every mode, like the game: in hardcore, only the bonus chest switch
/// greys out, not its label.
fn tab_labels(tab: usize) -> &'static [(&'static str, i32, i32, u32)] {
    match tab {
        0 => &[("selectWorld.enterName", 1, 0, LABEL_COLOR)],
        1 => &[
            ("selectWorld.enterSeed", 0, 28, LABEL_COLOR),
            ("selectWorld.mapFeatures", 0, 78, LABEL_COLOR),
            ("selectWorld.bonusItems", 0, 102, LABEL_COLOR),
        ],
        _ => &[],
    }
}

/// Outcome of a key given to the main tab.
#[derive(Debug, PartialEq, Eq)]
enum MainKey {
    None,
    Escape,
    /// Enter not used by a component: world creation.
    Create,
}

const TAB_H: i32 = 24;
const FOOT_H: i32 = 33;

const NAME: u32 = 1;
const MODE: u32 = 2;
const DIFFICULTY: u32 = 3;
const COMMANDS: u32 = 4;
const TYPE: u32 = 5;
const CUSTOMIZE: u32 = 6;
const SEED: u32 = 7;
const STRUCTURES: u32 = 8;
const BONUS: u32 = 9;
const RULES: u32 = 10;
const EXPERIMENTS: u32 = 11;
const DATA_PACKS: u32 = 12;
const CREATE: u32 = 13;
const CANCEL: u32 = 14;

pub struct CreateWorld {
    data: Arc<VersionData>,
    pub panel: Panel,
    size: (i32, i32),
    tab: usize,
    sub: Vec<Sub>,
    name: String,
    mode: Mode,
    difficulty: usize,
    /// Commands chosen (otherwise: depending on the mode, enabled in creative).
    commands: Option<bool>,
    seed: String,
    /// World type (index into `PRESETS`); `None`: settings taken from a world ("Customized").
    preset: Option<usize>,
    flat: Nbt,
    biome: String,
    structures: bool,
    bonus: bool,
    rules: Vec<(String, String)>,
    packs: Vec<String>,
    recreated: bool,
    /// Re-creation: type and settings taken as they are.
    kept: Option<(Preset, Option<Nbt>)>,
    /// Target folder (free name in `saves`).
    target: String,
    /// Origin of the tab grid layers (for the labels).
    origin: (i32, i32),
}

/// Sub-screens of the creation.
enum Sub {
    Rules(Box<Rules>),
    Experiments(Box<Experiments>),
    Packs(Box<PackSelect>),
    Flat(Box<FlatEdit>),
    Presets(Box<FlatPresets>),
    Buffet(Box<Buffet>),
    /// Confirmation: creation despite the warning.
    Confirm(Box<Column>, ConfirmKind),
    /// Packs chosen with experimental features: apply if "Continue".
    ExpConfirm(Box<ExpConfirm>),
}

enum ConfirmKind {
    /// Experimental re-created world: create if yes.
    Create,
}

impl CreateWorld {
    fn base(data: Arc<VersionData>) -> CreateWorld {
        let _ = creation_data(&data);
        CreateWorld {
            data,
            panel: Panel::default(),
            size: (-1, -1),
            tab: 0,
            sub: Vec::new(),
            name: String::new(),
            mode: Mode::Survival,
            difficulty: 2,
            commands: None,
            seed: String::new(),
            preset: Some(0),
            flat: default_flat(),
            biome: "minecraft:plains".into(),
            structures: true,
            bonus: false,
            rules: Vec::new(),
            packs: vec!["vanilla".into()],
            recreated: false,
            kept: None,
            target: String::new(),
            origin: (0, 0),
        }
    }

    /// New world (default settings, name "New World" translated on first draw).
    pub fn fresh(data: Arc<VersionData>) -> CreateWorld {
        CreateWorld::base(data)
    }

    /// "Re-Create": settings, rules, seed and generator taken from the world.
    pub fn recreate(data: Arc<VersionData>, dir: &Path) -> Result<CreateWorld, String> {
        let level = rc_sim::level::Level::read(dir).map_err(|e| e.to_string())?;
        let mut c = CreateWorld::base(data);
        c.recreated = true;
        c.name = level.name.clone();
        c.commands = Some(level.allow_commands);
        c.difficulty = DIFFICULTIES.iter().position(|d| *d == level.difficulty).unwrap_or(2);
        c.mode = if level.hardcore {
            Mode::Hardcore
        } else if level.game_type == 1 {
            Mode::Creative
        } else {
            Mode::Survival
        };
        let wgs = rc_sim::nbtio::read_dat(&dir.join("data").join("minecraft").join("world_gen_settings.dat"))
            .map_err(|e| e.to_string())?;
        let d = wgs.get("data").ok_or("world_gen_settings without data")?;
        if let Some(seed) = rc_sim::nbtio::int(d, "seed") {
            c.seed = seed.to_string();
        }
        c.structures = rc_sim::nbtio::int(d, "generate_structures") != Some(0);
        c.bonus = rc_sim::nbtio::int(d, "bonus_chest") == Some(1);
        let generator = d.get("dimensions").and_then(|x| x.get("minecraft:overworld")).and_then(|o| o.get("generator"));
        c.set_recreated_type(generator);
        if let Ok(rules) = rc_sim::nbtio::read_dat(&dir.join("data").join("minecraft").join("game_rules.dat"))
            && let Some(Nbt::Compound(m)) = rules.get("data")
        {
            c.rules = m
                .iter()
                .map(|(k, v)| {
                    let text = match v {
                        Nbt::Byte(b) => (*b != 0).to_string(),
                        other => other.as_i64().unwrap_or(0).to_string(),
                    };
                    (k.clone(), text)
                })
                .collect();
        }
        let enabled: Vec<String> = level
            .root
            .get("Data")
            .and_then(|d| d.get("DataPacks"))
            .map(|p| rc_sim::nbtio::list(p.get("Enabled")).iter().filter_map(Nbt::as_str).map(str::to_owned).collect())
            .unwrap_or_default();
        let builtin: Vec<String> = enabled.into_iter().filter(|p| BUILTIN.contains(&p.as_str())).collect::<Vec<_>>();
        if !builtin.is_empty() {
            c.packs = builtin;
        }
        Ok(c)
    }

    /// Type of a re-created world, deduced from its generator like the game: flat → "Superflat" (setting
    /// kept), debug → debug, noise → "Default". Quirk reproduced: since the "Default" type
    /// has no editor, the game reapplies it and resets the generator to new;
    /// an amplified, large-biomes or single-biome world is therefore re-created as a normal world.
    /// Other generator: "Customized" (inactive), taken as it is.
    fn set_recreated_type(&mut self, generator: Option<&Nbt>) {
        self.kept = None;
        match generator.and_then(|g| g.get("type")).and_then(Nbt::as_str) {
            Some("minecraft:flat") => {
                self.preset = Some(1);
                if let Some(s) = generator.and_then(|g| g.get("settings")) {
                    self.flat = s.clone();
                }
            }
            Some("minecraft:debug") => self.preset = Some(DEBUG_PRESET),
            Some("minecraft:noise") | None => self.preset = Some(0),
            Some(_) => {
                self.preset = None;
                self.kept = Some((Preset::Normal, None));
            }
        }
    }

    /// Chosen type: like the game, its settings restart from those of the type (flat layers and default
    /// single biome); a customised setting is lost when cycling through the types.
    fn set_preset(&mut self, p: usize) {
        self.preset = Some(p);
        self.flat = default_flat();
        self.biome = "minecraft:plains".into();
    }

    pub fn panel_mut(&mut self) -> &mut Panel {
        match self.sub.last_mut() {
            None => &mut self.panel,
            Some(Sub::Rules(r)) => &mut r.panel,
            Some(Sub::Experiments(e)) => &mut e.panel,
            Some(Sub::Packs(p)) => &mut p.panel,
            Some(Sub::Flat(f)) => &mut f.panel,
            Some(Sub::Presets(p)) => &mut p.panel,
            Some(Sub::Buffet(b)) => &mut b.panel,
            Some(Sub::Confirm(c, _)) => &mut c.panel,
            Some(Sub::ExpConfirm(e)) => e.panel_mut(),
        }
    }

    /// Debug world: spectator mode forced (neither hardcore nor creative).
    fn debug(&self) -> bool {
        self.preset == Some(DEBUG_PRESET)
    }

    fn hardcore(&self) -> bool {
        self.mode == Mode::Hardcore && !self.debug()
    }

    /// Key of the displayed mode ("spectator" for debug).
    fn mode_key(&self) -> &'static str {
        if self.debug() { "spectator" } else { self.mode.key() }
    }

    fn commands(&self) -> bool {
        if self.debug() {
            return true;
        }
        if self.hardcore() {
            return false;
        }
        self.commands.unwrap_or(self.mode == Mode::Creative)
    }

    /// Structures and bonus chest: never in the debug world (chest: nor in hardcore).
    fn structures(&self) -> bool {
        self.structures && !self.debug()
    }

    fn bonus(&self) -> bool {
        self.bonus && !self.hardcore() && !self.debug()
    }

    fn difficulty(&self) -> usize {
        if self.hardcore() { 3 } else { self.difficulty }
    }

    fn update_target(&mut self, gui: &Gui) {
        let trimmed = self.name.trim();
        let base = if trimmed.is_empty() { t(gui, "selectWorld.newWorld") } else { trimmed.to_owned() };
        let saves = saves_dir();
        let _ = std::fs::create_dir_all(&saves);
        self.target = rc_sim::storage::find_available_name(&saves, &base, "");
    }

    fn preset_key(&self) -> String {
        match self.preset {
            Some(i) => format!("generator.minecraft.{}", PRESETS[i]),
            None => "generator.custom".into(),
        }
    }

    fn customizable(&self) -> bool {
        matches!(self.preset, Some(1 | 4))
    }

    /// Components of the open tab, then "Create" and "Cancel".
    fn layout(&mut self, gui: &Gui) {
        let size = (gui.width as i32, gui.height as i32);
        if self.size == size {
            return;
        }
        let first = self.size.0 == -1;
        if first {
            if self.name.is_empty() {
                self.name = t(gui, "selectWorld.newWorld");
            }
            self.update_target(gui);
        }
        self.size = size;
        let (w, h) = size;
        let area = h - FOOT_H - TAB_H;
        let mut v = Vec::new();
        match self.tab {
            0 => {
                let (x0, y0) = ((w - 210) / 2, TAB_H + (area - 117).max(0) / 6);
                self.origin = (x0, y0);
                let mut f = TextField::new(Vec::new());
                f.text = self.name.chars().collect();
                f.cursor = f.text.len();
                f.anchor = f.cursor;
                let mut name = Widget::new(NAME, Kind::Field(Box::new(f)), x0 + 1, y0 + 13, 208, 20, Vec::new());
                self.name_tooltip(gui, &mut name);
                v.push(name);
                let mut mode = Widget::new(
                    MODE,
                    Kind::Cycle,
                    x0,
                    y0 + 41,
                    210,
                    20,
                    chars(&gv(
                        gui,
                        "selectWorld.gameMode",
                        &t(gui, &format!("selectWorld.gameMode.{}", self.mode_key())),
                    )),
                )
                .with_tooltip(gui, &t(gui, &format!("selectWorld.gameMode.{}.info", self.mode_key())));
                mode.active = !self.debug();
                v.push(mode);
                let dk = DIFFICULTIES[self.difficulty()];
                let mut diff = Widget::new(
                    DIFFICULTY,
                    Kind::Cycle,
                    x0,
                    y0 + 69,
                    210,
                    20,
                    chars(&gv(gui, "options.difficulty", &t(gui, &format!("options.difficulty.{dk}")))),
                )
                .with_tooltip(gui, &t(gui, &format!("options.difficulty.{dk}.info")));
                diff.active = !self.hardcore();
                v.push(diff);
                let mut cmd = Widget::new(
                    COMMANDS,
                    Kind::Cycle,
                    x0,
                    y0 + 97,
                    210,
                    20,
                    chars(&gv(gui, "selectWorld.allowCommands", &on_off(gui, self.commands()))),
                )
                .with_tooltip(gui, &t(gui, "selectWorld.allowCommands.info"));
                cmd.active = !self.hardcore() && !self.debug();
                v.push(cmd);
            }
            1 => {
                let (x0, y0) = ((w - 310) / 2, TAB_H + (area - 117).max(0) / 6);
                self.origin = (x0, y0);
                let mut ty = Widget::new(
                    TYPE,
                    Kind::Cycle,
                    x0,
                    y0,
                    150,
                    20,
                    chars(&gv(gui, "selectWorld.mapType", &t(gui, &self.preset_key()))),
                );
                if self.preset == Some(3) {
                    ty.set_tooltip(gui, &t(gui, "generator.minecraft.amplified.info"));
                }
                ty.active = self.preset.is_some();
                v.push(ty);
                let mut cu = Widget::button(CUSTOMIZE, x0 + 160, y0, 150, &t(gui, "selectWorld.customizeType"));
                cu.active = self.customizable();
                v.push(cu);
                let mut f = TextField::new(chars(&t(gui, "selectWorld.seedInfo")));
                f.text = self.seed.chars().collect();
                f.cursor = f.text.len();
                f.anchor = f.cursor;
                v.push(Widget::new(SEED, Kind::Field(Box::new(f)), x0, y0 + 41, 308, 20, Vec::new()));
                let mut st = Widget::new(
                    STRUCTURES,
                    Kind::Cycle,
                    x0 + 266,
                    y0 + 73,
                    44,
                    20,
                    chars(&on_off(gui, self.structures())),
                )
                .with_tooltip(gui, &t(gui, "selectWorld.mapFeatures.info"));
                st.active = !self.debug();
                v.push(st);
                let mut bo = Widget::new(
                    BONUS,
                    Kind::Cycle,
                    x0 + 266,
                    y0 + 97,
                    44,
                    20,
                    chars(&on_off(gui, self.bonus())),
                );
                bo.active = !self.hardcore() && !self.debug();
                v.push(bo);
            }
            _ => {
                let (x0, y0) = ((w - 210) / 2, TAB_H + (area - 76).max(0) / 6);
                self.origin = (x0, y0);
                v.push(Widget::button(RULES, x0, y0, 210, &t(gui, "selectWorld.gameRules")));
                v.push(Widget::button(EXPERIMENTS, x0, y0 + 28, 210, &t(gui, "selectWorld.experiments")));
                v.push(Widget::button(DATA_PACKS, x0, y0 + 56, 210, &t(gui, "selectWorld.dataPacks")));
            }
        }
        v.push(Widget::button(CREATE, w / 2 - 154, h - 26, 150, &t(gui, "selectWorld.create")));
        v.push(Widget::button(CANCEL, w / 2 + 4, h - 26, 150, &t(gui, "gui.cancel")));
        let keyboard = self.panel.keyboard;
        let focus = self.panel.focus.and_then(|i| self.panel.widgets.get(i)).map(|wd| wd.id);
        self.panel = Panel::new(v);
        self.panel.keyboard = keyboard;
        if first && self.tab == 0 {
            self.panel.focus_widget(NAME);
        } else if let Some(id) = focus {
            self.panel.focus = self.panel.widgets.iter().position(|wd| wd.id == id);
        }
    }

    fn name_tooltip(&self, gui: &Gui, wd: &mut Widget) {
        let full = tr(&gui.lang, "selectWorld.targetFolder", &["\u{1}".into()]);
        let (a, b) = full.split_once('\u{1}').unwrap_or((full.as_str(), ""));
        let mut line = chars(a);
        line.extend(italic(chars(&self.target)));
        line.extend(chars(b));
        wd.tooltip = rc_text::wrap(gui, &line, 170.0);
    }

    fn relayout(&mut self) {
        self.size = (-2, -2);
    }

    /// Tab under the mouse (bar 24 high).
    fn tab_at(&self, w: i32, m: (f32, f32)) -> Option<usize> {
        let (x0, tw) = tab_bar(w);
        if m.1 < 0.0 || m.1 >= TAB_H as f32 || m.0 < x0 as f32 {
            return None;
        }
        let i = ((m.0 - x0 as f32) / tw as f32) as usize;
        (i < 3).then_some(i)
    }

    /// Tab change (click, Ctrl+Tab, Ctrl+digit): the click sound comes from the change
    /// itself, never from the tab button (re-clicking the current tab is silent).
    fn select_tab(&mut self, i: usize) {
        if i != self.tab {
            self.panel.clicks += 1;
            self.tab = i;
            self.panel.focus = None;
            self.relayout();
        }
    }

    fn new_world(&self) -> NewWorld {
        let name = self.name.trim();
        let mut w = NewWorld::new(if name.is_empty() { &self.target } else { name });
        w.seed = rc_sim::newworld::seed_from_text(&self.seed);
        match (&self.kept, self.preset) {
            (Some((p, flat)), None) => {
                w.preset = p.clone();
                w.flat_settings = flat.clone();
            }
            (_, i) => {
                w.preset = match i.unwrap_or(0) {
                    1 => Preset::Flat,
                    2 => Preset::LargeBiomes,
                    3 => Preset::Amplified,
                    4 => Preset::SingleBiome(self.biome.clone()),
                    DEBUG_PRESET => Preset::Debug,
                    _ => Preset::Normal,
                };
                if i == Some(1) {
                    w.flat_settings = Some(self.flat.clone());
                }
            }
        }
        w.game_mode = if self.mode == Mode::Creative { 1 } else { 0 };
        w.hardcore = self.hardcore();
        w.difficulty = DIFFICULTIES[self.difficulty()].into();
        w.allow_commands = self.commands();
        w.generate_structures = self.structures();
        w.bonus_chest = self.bonus();
        if self.debug() {
            // Debug world: spectator, peaceful, commands; default rules except for stopped
            // time (chosen rules ignored).
            w.game_mode = 3;
            w.difficulty = DIFFICULTIES[0].into();
            w.game_rules = vec![("minecraft:advance_time".into(), Nbt::Byte(0))];
            return w;
        }
        w.game_rules = self
            .rules
            .iter()
            .map(|(k, v)| {
                let n = match v.as_str() {
                    "true" => Nbt::Byte(1),
                    "false" => Nbt::Byte(0),
                    s => Nbt::Int(s.parse().unwrap_or(0)),
                };
                (k.clone(), n)
            })
            .collect();
        w
    }

    fn create(&mut self, gui: &Gui) -> Vec<Msg> {
        self.update_target(gui);
        let dir = saves_dir().join(&self.target);
        let w = self.new_world();
        // Debug world: default data configuration (chosen packs ignored).
        let packs = if self.debug() { packs_of(&["vanilla".into()]) } else { packs_of(&self.packs) };
        vec![Msg::Job(Job::Create(dir, Box::new(w), Box::new(packs)))]
    }

    fn experimental(&self) -> bool {
        self.packs.iter().any(|p| FEATURE_PACKS.contains(&p.as_str()))
    }

    /// End of the sub-screens' background jobs (data read).
    pub(crate) fn tick(&mut self) -> Vec<Msg> {
        Vec::new()
    }

    pub(crate) fn input(&mut self, gui: &Gui, i: &In<'_>) -> Vec<Msg> {
        if !self.sub.is_empty() {
            return self.sub_input(gui, i);
        }
        self.layout(gui);
        let w = gui.width as i32;
        match i {
            // Tabs: click on the bar, Ctrl + Tab (Shift: previous), Ctrl + 1…3.
            In::Press(m, 0, _) => {
                if let Some(k) = self.tab_at(w, *m) {
                    self.select_tab(k);
                    return Vec::new();
                }
            }
            In::Key(KeyCode::Tab, mods) if mods.control_key() => {
                let n = if mods.shift_key() { (self.tab + 2) % 3 } else { (self.tab + 1) % 3 };
                self.select_tab(n);
                return Vec::new();
            }
            // Ctrl + digit: 1 → first tab, 0 → tenth, capped at the last.
            In::Key(k, mods) if mods.control_key() && digit(*k).is_some() => {
                let d = digit(*k).unwrap_or(1);
                self.select_tab(((d + 9) % 10).min(2));
                return Vec::new();
            }
            _ => {}
        }
        let (ev, key) = self.feed_main(i);
        let mut out = Vec::new();
        match key {
            MainKey::Escape => {
                out.push(Msg::Pop);
                return out;
            }
            MainKey::Create => return self.on_create(gui),
            MainKey::None => {}
        }
        for e in ev {
            match e {
                Event::Edit(NAME) => {
                    if let Some(Widget { kind: Kind::Field(f), .. }) = self.panel.get(NAME) {
                        self.name = f.value();
                    }
                    self.update_target(gui);
                    let mut wd = self.panel.get(NAME).cloned();
                    if let Some(wd) = &mut wd {
                        self.name_tooltip(gui, wd);
                        if let Some(n) = self.panel.get_mut(NAME) {
                            n.tooltip = wd.tooltip.clone();
                        }
                    }
                }
                Event::Edit(SEED) => {
                    if let Some(Widget { kind: Kind::Field(f), .. }) = self.panel.get(SEED) {
                        self.seed = f.value();
                    }
                }
                Event::Cycle(MODE, d) => {
                    let modes = [Mode::Survival, Mode::Hardcore, Mode::Creative];
                    let k = modes.iter().position(|m| *m == self.mode).unwrap_or(0) as i32;
                    self.mode = modes[(k + d).rem_euclid(3) as usize];
                    self.relayout();
                }
                Event::Cycle(DIFFICULTY, d) => {
                    self.difficulty = (self.difficulty as i32 + d).rem_euclid(4) as usize;
                    self.relayout();
                }
                Event::Cycle(COMMANDS, _) => {
                    self.commands = Some(!self.commands());
                    self.relayout();
                }
                Event::Cycle(TYPE, d) => {
                    if let Some(p) = self.preset {
                        let alt = ALT_DOWN.load(std::sync::atomic::Ordering::Relaxed);
                        self.set_preset(cycle_preset(p, d, alt));
                        self.relayout();
                    }
                }
                Event::Cycle(STRUCTURES, _) => {
                    self.structures = !self.structures;
                    self.relayout();
                }
                Event::Cycle(BONUS, _) => {
                    self.bonus = !self.bonus;
                    self.relayout();
                }
                Event::Press(CUSTOMIZE) => match self.preset {
                    Some(1) => self.sub.push(Sub::Flat(Box::new(FlatEdit::new(self.flat.clone())))),
                    Some(4) => self.sub.push(Sub::Buffet(Box::new(Buffet::new(self.biome.clone())))),
                    _ => {}
                },
                Event::Press(RULES) => self.sub.push(Sub::Rules(Box::new(Rules::new(&self.rules)))),
                Event::Press(EXPERIMENTS) => self.sub.push(Sub::Experiments(Box::new(Experiments::new(&self.packs)))),
                Event::Press(DATA_PACKS) => self.sub.push(Sub::Packs(Box::new(PackSelect::new(&self.packs)))),
                Event::Press(CREATE) => out.extend(self.on_create(gui)),
                Event::Press(CANCEL) => out.push(Msg::Pop),
                _ => {}
            }
        }
        out
    }

    /// Input of the main tab given to the panel: events, then Escape (back) or Enter
    /// not used by a component (create). A focused field (the name is by default) does not use
    /// Enter, like the game; a focused button does (it returns its event).
    fn feed_main(&mut self, i: &In<'_>) -> (Vec<Event>, MainKey) {
        let enter = matches!(i, In::Key(KeyCode::Enter | KeyCode::NumpadEnter, _));
        let (ev, escape) = feed(&mut self.panel, i);
        let key = if escape {
            MainKey::Escape
        } else if enter && ev.is_empty() {
            MainKey::Create
        } else {
            MainKey::None
        };
        (ev, key)
    }

    /// "Create": warning if the re-created world uses experimental features.
    fn on_create(&mut self, gui: &Gui) -> Vec<Msg> {
        if self.recreated && self.experimental() {
            let col = Column::new(
                Component::translate("selectWorld.warning.experimental.title", vec![]),
                Component::translate("selectWorld.warning.experimental.question", vec![]),
                vec![(1, t(gui, "gui.yes"), 150), (2, t(gui, "gui.no"), 150)],
                20,
                20,
            );
            self.sub.push(Sub::Confirm(Box::new(col), ConfirmKind::Create));
            return Vec::new();
        }
        self.create(gui)
    }

    fn sub_input(&mut self, gui: &Gui, i: &In<'_>) -> Vec<Msg> {
        let data = self.data.clone();
        let biomes = creation_data(&data).map(|c| c.biomes.clone()).unwrap_or_default();
        let presets = creation_data(&data).map(|c| c.flat_presets.clone()).unwrap_or_default();
        let Some(top) = self.sub.last_mut() else { return Vec::new() };
        let step = match top {
            Sub::Rules(r) => r.input(gui, i),
            Sub::Experiments(e) => e.input(gui, i),
            Sub::Packs(p) => p.input(gui, i),
            Sub::Flat(f) => f.input(gui, i),
            Sub::Presets(p) => p.input(gui, i, &biomes, &presets),
            Sub::Buffet(b) => b.input(gui, i, &biomes),
            Sub::Confirm(c, _) => {
                let (ev, escape) = feed(&mut c.panel, i);
                if ev.contains(&Event::Press(1)) {
                    SubOut::Yes
                } else if escape || ev.contains(&Event::Press(2)) {
                    SubOut::Close
                } else {
                    SubOut::None
                }
            }
            Sub::ExpConfirm(e) => e.input(gui, i),
        };
        match step {
            SubOut::None => {}
            SubOut::Close => {
                self.sub.pop();
            }
            SubOut::Rules(r) => {
                self.sub.pop();
                self.rules = r;
            }
            SubOut::Packs(p) => {
                self.sub.pop();
                // Experimental packs added: warning before keeping them.
                let new_exp = p.iter().any(|x| FEATURE_PACKS.contains(&x.as_str()) && !self.packs.contains(x));
                if new_exp {
                    self.sub.push(Sub::ExpConfirm(Box::new(ExpConfirm::new(p))));
                } else {
                    self.packs = p;
                }
            }
            SubOut::PacksDirect(p) => {
                self.sub.pop();
                self.packs = p;
            }
            SubOut::Flat(f) => {
                self.sub.pop();
                self.flat = f;
            }
            SubOut::OpenPresets(current) => {
                self.sub.push(Sub::Presets(Box::new(FlatPresets::new(&current))));
            }
            SubOut::Preset(f) => {
                self.sub.pop();
                if let Some(Sub::Flat(e)) = self.sub.last_mut() {
                    e.set(f);
                }
            }
            SubOut::Biome(b) => {
                self.sub.pop();
                self.biome = b;
            }
            SubOut::Yes => match self.sub.pop() {
                Some(Sub::Confirm(_, ConfirmKind::Create)) => return self.create(gui),
                Some(Sub::ExpConfirm(e)) => self.packs = e.packs,
                _ => {}
            },
        }
        Vec::new()
    }

    pub fn draw(&mut self, gui: &mut Gui, q: &wgpu::Queue, o: &Options, mouse: Option<(f32, f32)>, time: f64) {
        let data = self.data.clone();
        match self.sub.last_mut() {
            Some(Sub::Rules(r)) => return r.draw(gui, q, o, mouse, time),
            Some(Sub::Experiments(e)) => return e.draw(gui, q, o, mouse, time),
            Some(Sub::Packs(p)) => return p.draw(gui, q, o, mouse, time),
            Some(Sub::Flat(f)) => return f.draw(gui, q, o, mouse, time),
            Some(Sub::Presets(p)) => {
                let presets = creation_data(&data).map(|c| c.flat_presets.clone()).unwrap_or_default();
                return p.draw(gui, q, o, mouse, time, &presets);
            }
            Some(Sub::Buffet(b)) => {
                let biomes = creation_data(&data).map(|c| c.biomes.clone()).unwrap_or_default();
                return b.draw(gui, q, o, mouse, time, &biomes);
            }
            Some(Sub::Confirm(c, _)) => return c.draw(gui, q, o, mouse, time),
            Some(Sub::ExpConfirm(e)) => return e.draw(gui, q, o, mouse, time),
            None => {}
        }
        self.layout(gui);
        let f = |v: i32| v as f32;
        let (w, h) = (gui.width as i32, gui.height as i32);
        gui.next_layer();
        gui.blur(o.i("menuBackgroundBlurriness"));
        // Tab header (own background), content on the menu background, footer separator.
        gui.sprite_tiled("bg/tab_header_background", 0.0, 0.0, f(w), f(TAB_H), 16.0, 16.0, 0.0, 0.0);
        gui.sprite_tiled("bg/menu_background", 0.0, f(TAB_H), f(w), f(h), 32.0, 32.0, 0.0, 0.0);
        gui.sprite_tiled("bg/footer_separator", 0.0, f(h - FOOT_H - 2), f(w), f(h - FOOT_H), 32.0, 2.0, 0.0, 0.0);
        gui.next_layer();
        self.draw_tabs(gui, q, mouse, time);
        // Tab labels.
        let (x0, y0) = self.origin;
        for &(key, dx, dy, color) in tab_labels(self.tab) {
            gui.text(q, &chars(&t(gui, key)), f(x0 + dx), f(y0 + dy), color, true, 0);
        }
        self.panel.draw(gui, q, mouse, time);
    }

    fn draw_tabs(&mut self, gui: &mut Gui, q: &wgpu::Queue, mouse: Option<(f32, f32)>, time: f64) {
        let f = |v: i32| v as f32;
        let w = gui.width as i32;
        let (x0, tw) = tab_bar(w);
        gui.sprite_tiled("bg/header_separator", 0.0, f(TAB_H - 2), f(x0), f(TAB_H), 32.0, 2.0, 0.0, 0.0);
        gui.sprite_tiled("bg/header_separator", f(x0 + 3 * tw), f(TAB_H - 2), f(w), f(TAB_H), 32.0, 2.0, 0.0, 0.0);
        let keys = ["createWorld.tab.game.title", "createWorld.tab.world.title", "createWorld.tab.more.title"];
        let hovered = mouse.and_then(|m| self.tab_at(w, m));
        for (i, key) in keys.iter().enumerate() {
            let x = x0 + tw * i as i32;
            let selected = i == self.tab;
            let hl = hovered == Some(i);
            let s = match (selected, hl) {
                (true, true) => "widget/tab_selected_highlighted",
                (true, false) => "widget/tab_selected",
                (false, true) => "widget/tab_highlighted",
                (false, false) => "widget/tab",
            };
            gui.sprite(s, f(x), 0.0, f(tw), f(TAB_H), 0xffff_ffff);
            let label = chars(&t(gui, key));
            if selected {
                gui.sprite_tiled("bg/menu_background", f(x + 2), 2.0, f(x + tw - 2), f(TAB_H), 32.0, 32.0, 0.0, 0.0);
                let lw = rc_text::width(gui, &label).min(tw - 4);
                let ux = x + (tw - lw) / 2;
                gui.fill(f(ux), f(TAB_H - 2), f(ux + lw), f(TAB_H - 1), 0xffff_ffff);
            }
            let top = if selected { 0 } else { 3 };
            widgets::scrolling_text(gui, q, &label, x + 1, top, x + tw - 1, TAB_H, 0xffff_ffff, time);
            if hl {
                widgets::request_cursor(widgets::Cursor::Hand);
            }
        }
    }
}

/// Digit of a key on the top row.
fn digit(k: KeyCode) -> Option<usize> {
    [
        KeyCode::Digit0,
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
        KeyCode::Digit6,
        KeyCode::Digit7,
        KeyCode::Digit8,
        KeyCode::Digit9,
    ]
    .iter()
    .position(|&d| d == k)
}

/// Menu tab bar (3 tabs, also the statistics one): origin and width of a
/// tab (rounded up to even); the origin centres the reserved width, not the tabs.
pub(crate) fn tab_bar(w: i32) -> (i32, i32) {
    let even = |v: i32| (v + 1).div_euclid(2) * 2;
    let tabs = w.min(400) - 28;
    (even((w - tabs) / 2), even(tabs / 3))
}

/// Outcome of a sub-screen.
enum SubOut {
    None,
    Close,
    Rules(Vec<(String, String)>),
    /// Packs chosen in the packs screen (warning if experimental ones added).
    Packs(Vec<String>),
    /// Packs chosen in the experiments screen (no warning).
    PacksDirect(Vec<String>),
    Flat(Nbt),
    OpenPresets(Nbt),
    Preset(Nbt),
    Biome(String),
    Yes,
}

// ---------- Game rules ----------

struct Rules {
    panel: Panel,
    size: (i32, i32),
    editor: RulesEditor,
    search: String,
}

impl Rules {
    fn new(values: &[(String, String)]) -> Rules {
        Rules { panel: Panel::default(), size: (-1, -1), editor: RulesEditor::new(values), search: String::new() }
    }

    fn layout(&mut self, gui: &Gui) {
        let size = (gui.width as i32, gui.height as i32);
        if self.size == size {
            return;
        }
        let first = self.size.0 == -1;
        self.size = size;
        let (v, list) = self.editor.build(gui, &self.search);
        let scroll = self.panel.list.as_ref().map(|l| l.scroll);
        let focus = self.panel.focus.and_then(|i| self.panel.widgets.get(i)).map(|wd| wd.id);
        let keyboard = self.panel.keyboard;
        self.panel = Panel::new(v);
        self.panel.list = list.map(|mut l| {
            l.inworld = false;
            if let Some(s) = scroll {
                l.set_scroll(s);
            }
            l
        });
        self.panel.keyboard = keyboard;
        if first {
            // Initial focus: the search (first component).
            self.panel.focus = Some(0);
        } else if let Some(id) = focus {
            self.panel.focus = self.panel.widgets.iter().position(|wd| wd.id == id);
        }
    }

    fn input(&mut self, gui: &Gui, i: &In<'_>) -> SubOut {
        self.layout(gui);
        let (ev, escape) = feed(&mut self.panel, i);
        if escape {
            return SubOut::Close;
        }
        for e in ev {
            let search = RulesEditor::search(&self.panel);
            match self.editor.handle(gui, &self.search, &mut self.panel, &e) {
                RulesOut::None => {}
                RulesOut::Rebuild => {
                    self.search = search;
                    self.size = (-2, -2);
                    self.layout(gui);
                }
                RulesOut::Done(changes) => return SubOut::Rules(changes),
                RulesOut::Cancel => return SubOut::Close,
            }
        }
        SubOut::None
    }

    fn draw(&mut self, gui: &mut Gui, q: &wgpu::Queue, o: &Options, mouse: Option<(f32, f32)>, time: f64) {
        self.layout(gui);
        background(gui, o);
        let w = gui.width as i32;
        widgets::centered(gui, q, &chars(&t(gui, "editGamerule.title")), w / 2, 4, 0xffff_ffff);
        self.panel.draw(gui, q, mouse, time);
    }
}

// ---------- Experiments ----------

const E_DONE: u32 = 1;
const E_CANCEL: u32 = 2;
const E_SWITCH: u32 = 10;

struct Experiments {
    panel: Panel,
    size: (i32, i32),
    on: [bool; 3],
    base: Vec<String>,
    info: Vec<Vec<StyledChar>>,
    top: i32,
}

impl Experiments {
    fn new(packs: &[String]) -> Experiments {
        let on = FEATURE_PACKS.map(|p| packs.iter().any(|x| x == p));
        Experiments { panel: Panel::default(), size: (-1, -1), on, base: packs.to_vec(), info: Vec::new(), top: 0 }
    }

    fn layout(&mut self, gui: &Gui) {
        let size = (gui.width as i32, gui.height as i32);
        if self.size == size {
            return;
        }
        self.size = size;
        let (w, h) = size;
        self.info = rc_text::wrap(gui, &colored(&t(gui, "selectWorld.experiments.info"), 0xff_5555), 310.0);
        // Content: warning, 15, grid (3 rows of 42 + 4 at the top).
        let content = 9 * self.info.len() as i32 + 15 + 4 + 3 * 42 - 4;
        self.top = (33 + 30).min(h - 33 - content);
        let x0 = (w - 310) / 2;
        let gy = self.top + 9 * self.info.len() as i32 + 15 + 4;
        let mut v = Vec::new();
        for (k, on) in self.on.iter().enumerate() {
            let desc = t(gui, &format!("dataPack.{}.description", FEATURE_PACKS[k]));
            let _ = desc;
            v.push(Widget::new(
                E_SWITCH + k as u32,
                Kind::Cycle,
                x0 + 299 - 44,
                gy + 42 * k as i32,
                44,
                20,
                chars(&on_off(gui, *on)),
            ));
        }
        v.push(Widget::button(E_DONE, w / 2 - 154, h - 26, 150, &t(gui, "gui.done")));
        v.push(Widget::button(E_CANCEL, w / 2 + 4, h - 26, 150, &t(gui, "gui.cancel")));
        let keyboard = self.panel.keyboard;
        let focus = self.panel.focus;
        self.panel = Panel::new(v);
        self.panel.keyboard = keyboard;
        self.panel.focus = focus;
    }

    fn input(&mut self, gui: &Gui, i: &In<'_>) -> SubOut {
        self.layout(gui);
        let (ev, escape) = feed(&mut self.panel, i);
        if escape {
            return SubOut::Close;
        }
        for e in ev {
            match e {
                Event::Cycle(id, _) if (E_SWITCH..E_SWITCH + 3).contains(&id) => {
                    let k = (id - E_SWITCH) as usize;
                    self.on[k] = !self.on[k];
                    self.size = (-2, -2);
                }
                Event::Press(E_DONE) => {
                    // Non-experimental packs kept, then the chosen experiments (reverse order).
                    let mut p: Vec<String> =
                        self.base.iter().filter(|x| !FEATURE_PACKS.contains(&x.as_str())).cloned().collect();
                    for k in (0..3).rev() {
                        if self.on[k] {
                            p.push(FEATURE_PACKS[k].to_owned());
                        }
                    }
                    return SubOut::PacksDirect(p);
                }
                Event::Press(E_CANCEL) => return SubOut::Close,
                _ => {}
            }
        }
        SubOut::None
    }

    fn draw(&mut self, gui: &mut Gui, q: &wgpu::Queue, o: &Options, mouse: Option<(f32, f32)>, time: f64) {
        self.layout(gui);
        background(gui, o);
        let (w, h) = (gui.width as i32, gui.height as i32);
        let f = |v: i32| v as f32;
        widgets::centered(gui, q, &chars(&t(gui, "selectWorld.experiments")), w / 2, 12, 0xffff_ffff);
        gui.sprite_tiled("bg/header_separator", 0.0, 31.0, f(w), 33.0, 32.0, 2.0, 0.0, 0.0);
        gui.sprite_tiled("bg/footer_separator", 0.0, f(h - 33), f(w), f(h - 31), 32.0, 2.0, 0.0, 0.0);
        let x0 = (w - 310) / 2;
        let mut y = self.top;
        for l in &self.info {
            gui.text(q, l, f(x0), f(y), 0xffff_ffff, true, 0);
            y += 9;
        }
        let gy = y + 15 + 4;
        for (k, p) in FEATURE_PACKS.iter().enumerate() {
            let ry = gy + 42 * k as i32;
            let title = chars(&t(gui, &format!("dataPack.{p}.name")));
            gui.text(q, &title, f(x0), f(ry + 5), 0xffff_ffff, true, 0);
            let desc = colored(&t(gui, &format!("dataPack.{p}.description")), 0xaa_aaaa);
            for (j, l) in rc_text::wrap(gui, &desc, 255.0).iter().take(2).enumerate() {
                gui.text(q, l, f(x0), f(ry + 20 + 9 * j as i32), 0xffff_ffff, true, 0);
            }
        }
        self.panel.draw(gui, q, mouse, time);
    }
}

// ---------- Experimental features warning ----------

const X_DETAILS: u32 = 1;
const X_PROCEED: u32 = 2;
const X_BACK: u32 = 3;
/// Width of the grid (two columns of 150 spaced by 10), of the message, of the "Details" button.
const X_GRID_W: i32 = 310;
const X_DETAILS_W: i32 = 100;
/// Height of the lines of the details list ((9 + 2) × 3).
const X_ROW_H: i32 = 33;

/// Positions of the warning: top of the grid, corners of "Details", "Continue", "Back".
#[derive(Debug, PartialEq, Eq)]
struct ExpGrid {
    top: i32,
    details: (i32, i32),
    proceed: (i32, i32),
    back: (i32, i32),
}

/// Centred grid: title (9), message (`lines` × 9), "Details" (20), buttons (20), rows
/// spaced by 20; "Details" centred, buttons of 150 spaced by 10.
fn exp_grid(w: i32, h: i32, lines: i32) -> ExpGrid {
    let msg_h = 9 * lines;
    let grid_h = 9 + 20 + msg_h + 20 + 20 + 20 + 20;
    let top = (h - grid_h) / 2;
    let x0 = (w - X_GRID_W) / 2;
    let details_y = top + 9 + 20 + msg_h + 20;
    let buttons_y = details_y + 20 + 20;
    ExpGrid {
        top,
        details: (x0 + (X_GRID_W - X_DETAILS_W) / 2, details_y),
        proceed: (x0, buttons_y),
        back: (x0 + 160, buttons_y),
    }
}

/// Non-vanilla features requested by a pack (`minecraft:<id>` for a feature pack).
fn pack_features(id: &str) -> Option<String> {
    FEATURE_PACKS.contains(&id).then(|| format!("minecraft:{id}"))
}

/// "Experimental features": title, message (310 wide), "Details" (100), then
/// "Continue" / "Back", grid centred on screen (spacing 20 between rows, 10 between
/// columns). "Details" opens the list of chosen packs that request non-vanilla
/// features (title in bold, "Features: …"), "Back" returns to it.
struct ExpConfirm {
    panel: Panel,
    size: (i32, i32),
    packs: Vec<String>,
    message: Vec<Vec<StyledChar>>,
    /// Top of the grid.
    top: i32,
    /// Details list open: (title, message lines) per pack.
    details: Option<Vec<DetailRow>>,
}

/// Details line: pack title in bold, lines of the features message.
type DetailRow = (Vec<StyledChar>, Vec<Vec<StyledChar>>);

impl ExpConfirm {
    fn new(packs: Vec<String>) -> ExpConfirm {
        ExpConfirm { panel: Panel::default(), size: (-1, -1), packs, message: Vec::new(), top: 0, details: None }
    }

    fn panel_mut(&mut self) -> &mut Panel {
        &mut self.panel
    }

    fn layout(&mut self, gui: &Gui) {
        let size = (gui.width as i32, gui.height as i32);
        if self.size == size {
            return;
        }
        self.size = size;
        let (w, h) = size;
        let keyboard = self.panel.keyboard;
        if self.details.is_some() {
            let row_w = w * 3 / 4;
            let rows: Vec<_> = self
                .packs
                .iter()
                .filter_map(|id| {
                    let f = pack_features(id)?;
                    let mut title = chars(&t(gui, &format!("dataPack.{id}.name")));
                    for c in &mut title {
                        c.style.bold = true;
                    }
                    let msg = chars(&tr(&gui.lang, "selectWorld.experimental.details.entry", &[f]));
                    Some((title, rc_text::wrap(gui, &msg, row_w as f32)))
                })
                .collect();
            let n = rows.len() as i32;
            let mut l = List::new(0, 33, w, h - 66, row_w, X_ROW_H);
            l.inworld = false;
            l.custom_rows = true;
            l.entries = (0..n).map(|i| Entry { top: i * X_ROW_H, h: X_ROW_H, label: Vec::new() }).collect();
            l.content = n * X_ROW_H + 4;
            self.details = Some(rows);
            self.panel = Panel::new(vec![Widget::button(X_BACK, w / 2 - 75, h - 27, 150, &t(gui, "gui.back"))]);
            self.panel.list = Some(l);
        } else {
            let msg = chars(&t(gui, "selectWorld.experimental.message"));
            self.message = rc_text::wrap(gui, &msg, X_GRID_W as f32);
            let g = exp_grid(w, h, self.message.len() as i32);
            self.top = g.top;
            self.panel = Panel::new(vec![
                Widget::button(X_DETAILS, g.details.0, g.details.1, X_DETAILS_W, &t(gui, "selectWorld.experimental.details")),
                Widget::button(X_PROCEED, g.proceed.0, g.proceed.1, 150, &t(gui, "gui.proceed")),
                Widget::button(X_BACK, g.back.0, g.back.1, 150, &t(gui, "gui.back")),
            ]);
        }
        self.panel.keyboard = keyboard;
    }

    fn input(&mut self, gui: &Gui, i: &In<'_>) -> SubOut {
        self.layout(gui);
        let (ev, escape) = feed(&mut self.panel, i);
        if self.details.is_some() {
            // Back (or Escape): warning reopened.
            if escape || ev.contains(&Event::Press(X_BACK)) {
                self.details = None;
                self.size = (-2, -2);
            }
            return SubOut::None;
        }
        if escape || ev.contains(&Event::Press(X_BACK)) {
            return SubOut::Close;
        }
        if ev.contains(&Event::Press(X_PROCEED)) {
            return SubOut::Yes;
        }
        if ev.contains(&Event::Press(X_DETAILS)) {
            self.details = Some(Vec::new());
            self.size = (-2, -2);
        }
        SubOut::None
    }

    fn draw(&mut self, gui: &mut Gui, q: &wgpu::Queue, o: &Options, mouse: Option<(f32, f32)>, time: f64) {
        self.layout(gui);
        background(gui, o);
        let (w, h) = (gui.width as i32, gui.height as i32);
        let f = |v: i32| v as f32;
        if let Some(rows) = &self.details {
            widgets::centered(gui, q, &chars(&t(gui, "selectWorld.experimental.details.title")), w / 2, 12, 0xffff_ffff);
            gui.sprite_tiled("bg/header_separator", 0.0, 31.0, f(w), 33.0, 32.0, 2.0, 0.0, 0.0);
            gui.sprite_tiled("bg/footer_separator", 0.0, f(h - 33), f(w), f(h - 31), 32.0, 2.0, 0.0, 0.0);
            self.panel.draw_with(gui, q, mouse, time, &mut |gui, q, l| {
                for (k, (title, lines)) in rows.iter().enumerate() {
                    let Some((rl, top, _, rh)) = l.entry_rect(k) else { continue };
                    if top + rh < l.y || top > l.y + l.h {
                        continue;
                    }
                    let (cx, cy) = (rl + 2, top + 2);
                    gui.text(q, title, f(cx), f(cy), 0xffff_ffff, true, 0);
                    for (j, line) in lines.iter().enumerate() {
                        gui.text(q, line, f(cx), f(cy + 12 + 9 * j as i32), 0xffff_ffff, true, 0);
                    }
                }
            });
            return;
        }
        let x0 = (w - X_GRID_W) / 2;
        let title = chars(&t(gui, "selectWorld.experimental.title"));
        let tw = rc_text::width(gui, &title);
        gui.text(q, &title, f(x0 + (X_GRID_W - tw) / 2), f(self.top), 0xffff_ffff, true, 0);
        // Centred message: block as wide as its longest line, centred in the grid, each
        // line centred in the block.
        let mw = self.message.iter().map(|l| rc_text::width(gui, l)).max().unwrap_or(0);
        let mx = x0 + (X_GRID_W - mw) / 2;
        for (k, line) in self.message.iter().enumerate() {
            let lw = rc_text::width(gui, line);
            gui.text(q, line, f(mx + (mw - lw) / 2), f(self.top + 29 + 9 * k as i32), 0xffff_ffff, true, 0);
        }
        self.panel.draw(gui, q, mouse, time);
    }
}

// ---------- Data packs ----------

const P_FOLDER: u32 = 1;
const P_DONE: u32 = 2;
const P_HEADER: i32 = 49;
const P_FOOTER: i32 = 33;
const P_W: i32 = 200;
const P_ENTRY: i32 = 36;

struct PackSelect {
    panel: Panel,
    size: (i32, i32),
    selected: Vec<String>,
}

impl PackSelect {
    fn new(packs: &[String]) -> PackSelect {
        PackSelect { panel: Panel::default(), size: (-1, -1), selected: packs.to_vec() }
    }

    fn available(&self) -> Vec<String> {
        BUILTIN.iter().filter(|p| !self.selected.iter().any(|s| s == *p)).map(|s| (*s).to_owned()).collect()
    }

    fn list_x(&self, right: bool) -> i32 {
        if right { self.size.0 / 2 + 15 } else { self.size.0 / 2 - 15 - P_W }
    }

    fn entry_y(i: usize) -> i32 {
        P_HEADER + 4 + 13 + i as i32 * P_ENTRY + 2
    }

    fn layout(&mut self, gui: &Gui) {
        let size = (gui.width as i32, gui.height as i32);
        if self.size == size {
            return;
        }
        self.size = size;
        let (w, h) = size;
        let x0 = (w - 308) / 2;
        // World packs folder: created with the world (no folder here yet).
        let folder = Widget::button(P_FOLDER, x0, h - 26, 150, &t(gui, "pack.openFolder"))
            .with_tooltip(gui, &t(gui, "pack.folderInfo"))
            .inactive();
        let keyboard = self.panel.keyboard;
        self.panel = Panel::new(vec![folder, Widget::button(P_DONE, x0 + 158, h - 26, 150, &t(gui, "gui.done"))]);
        self.panel.keyboard = keyboard;
    }

    /// Pack under the mouse: (selected?, rank).
    fn hit(&self, m: (f32, f32)) -> Option<(bool, usize)> {
        for right in [false, true] {
            let x = self.list_x(right) as f32;
            if m.0 < x || m.0 >= x + P_W as f32 {
                continue;
            }
            let n = if right { self.selected.len() } else { self.available().len() };
            for i in 0..n {
                let y = PackSelect::entry_y(i) as f32;
                if m.1 >= y - 2.0 && m.1 < y + 34.0 {
                    return Some((right, i));
                }
            }
        }
        None
    }

    fn input(&mut self, gui: &Gui, i: &In<'_>) -> SubOut {
        self.layout(gui);
        if let In::Press(m, 0, _) = i
            && let Some((right, k)) = self.hit(*m)
        {
            // Click on the icon: choose / remove (the default pack stays).
            let x = self.list_x(right) + 2;
            if m.0 < (x + 32) as f32 {
                if right {
                    if self.selected[k] != "vanilla" {
                        self.panel.clicks += 1;
                        self.selected.remove(k);
                    }
                } else {
                    let id = self.available()[k].clone();
                    self.panel.clicks += 1;
                    self.selected.insert(0, id);
                }
                return SubOut::None;
            }
        }
        let (ev, escape) = feed(&mut self.panel, i);
        if escape || ev.contains(&Event::Press(P_DONE)) {
            return SubOut::Packs(self.selected.clone());
        }
        SubOut::None
    }

    fn draw(&mut self, gui: &mut Gui, q: &wgpu::Queue, o: &Options, mouse: Option<(f32, f32)>, time: f64) {
        self.layout(gui);
        background(gui, o);
        let (w, h) = self.size;
        let f = |v: i32| v as f32;
        widgets::centered(gui, q, &chars(&t(gui, "dataPack.title")), w / 2, 4, 0xffff_ffff);
        widgets::centered(gui, q, &chars(&t(gui, "pack.dropInfo")), w / 2, 17, 0xffaa_aaaa);
        let hover = mouse.and_then(|m| self.hit(m));
        for right in [false, true] {
            let x = self.list_x(right);
            gui.sprite_tiled(
                "bg/menu_list_background",
                f(x),
                f(P_HEADER),
                f(x + P_W),
                f(h - P_FOOTER),
                32.0,
                32.0,
                0.0,
                0.0,
            );
            let key = if right { "pack.selected.title" } else { "pack.available.title" };
            let mut tc = chars(&t(gui, key));
            for c in &mut tc {
                c.style.bold = true;
                c.style.underlined = true;
            }
            widgets::centered(gui, q, &tc, x + P_W / 2, P_HEADER + 4 + 2 + 1, 0xffff_ffff);
            let ids = if right { self.selected.clone() } else { self.available() };
            for (i, id) in ids.iter().enumerate() {
                let (cx, cy) = (x + 2, PackSelect::entry_y(i));
                gui.sprite("tex/misc/unknown_pack", f(cx), f(cy), 32.0, 32.0, 0xffff_ffff);
                let hovered = hover == Some((right, i));
                if hovered && !(right && id == "vanilla") {
                    gui.fill(f(cx), f(cy), f(cx + 32), f(cy + 32), 0xa090_9090);
                    let over = mouse.is_some_and(|m| m.0 < (cx + 32) as f32);
                    let base = if right { "unselect" } else { "select" };
                    let name = if over {
                        format!("transferable_list/{base}_highlighted")
                    } else {
                        format!("transferable_list/{base}")
                    };
                    gui.sprite(&name, f(cx), f(cy), 32.0, 32.0, 0xffff_ffff);
                }
                let title = crate::world_list::clamp_text(gui, &chars(&t(gui, &format!("dataPack.{id}.name"))), 157);
                gui.text(q, &title, f(cx + 34), f(cy + 1), 0xffff_ffff, true, 0);
                let mut desc = chars(&t(gui, &format!("dataPack.{id}.description")));
                if FEATURE_PACKS.contains(&id.as_str()) {
                    desc = colored(&t(gui, &format!("dataPack.{id}.description")), 0x80_8080);
                }
                for (j, l) in rc_text::wrap(gui, &desc, 157.0).iter().take(2).enumerate() {
                    gui.text(q, l, f(cx + 34), f(cy + 12 + 9 * j as i32), 0xff80_8080, true, 0);
                }
            }
        }
        gui.sprite_tiled("bg/header_separator", 0.0, f(P_HEADER - 2), f(w), f(P_HEADER), 32.0, 2.0, 0.0, 0.0);
        gui.sprite_tiled("bg/footer_separator", 0.0, f(h - P_FOOTER), f(w), f(h - P_FOOTER + 2), 32.0, 2.0, 0.0, 0.0);
        self.panel.draw(gui, q, mouse, time);
    }
}

// ---------- Flat world ----------

const F_REMOVE: u32 = 1;
const F_PRESETS: u32 = 2;
/// Heights of the header and footer of the flat world layers screen.
const FLAT_HEAD: i32 = 33;
const FLAT_FOOT: i32 = 64;

/// Top and height of the layer list for a screen of height `h`.
fn flat_list_bounds(h: i32) -> (i32, i32) {
    (FLAT_HEAD, h - FLAT_HEAD - FLAT_FOOT)
}
const F_DONE: u32 = 3;
const F_CANCEL: u32 = 4;

struct FlatEdit {
    panel: Panel,
    size: (i32, i32),
    flat: Nbt,
}

impl FlatEdit {
    fn new(flat: Nbt) -> FlatEdit {
        FlatEdit { panel: Panel::default(), size: (-1, -1), flat }
    }

    fn set(&mut self, flat: Nbt) {
        self.flat = flat;
        self.size = (-2, -2);
    }

    fn layout(&mut self, gui: &Gui) {
        let size = (gui.width as i32, gui.height as i32);
        if self.size == size {
            return;
        }
        self.size = size;
        let (w, h) = size;
        // List between the header (33) and the footer (64) of the screen: header of 13 then a row of
        // 24 per layer (top to bottom).
        let (ly, lh) = flat_list_bounds(h);
        let mut l = List::new(0, ly, w, lh, 220, 24);
        l.inworld = false;
        l.custom_rows = true;
        let n = layers(&self.flat).len() as i32;
        l.entries = (0..n).map(|i| Entry { top: 13 + i * 24, h: 24, label: Vec::new() }).collect();
        l.content = 13 + n * 24 + 4;
        if let Some(o) = &self.panel.list {
            l.selected = o.selected.filter(|&s| (s as i32) < n);
            l.set_scroll(o.scroll);
        }
        let x0 = (w - 308) / 2;
        let mut remove = Widget::button(F_REMOVE, x0, h - 54, 150, &t(gui, "createWorld.customize.flat.removeLayer"));
        remove.active = l.selected.is_some();
        let keyboard = self.panel.keyboard;
        self.panel = Panel::new(vec![
            remove,
            Widget::button(F_PRESETS, x0 + 158, h - 54, 150, &t(gui, "createWorld.customize.presets")),
            Widget::button(F_DONE, x0, h - 30, 150, &t(gui, "gui.done")),
            Widget::button(F_CANCEL, x0 + 158, h - 30, 150, &t(gui, "gui.cancel")),
        ]);
        self.panel.list = Some(l);
        self.panel.keyboard = keyboard;
    }

    fn input(&mut self, gui: &Gui, i: &In<'_>) -> SubOut {
        self.layout(gui);
        let (ev, escape) = feed(&mut self.panel, i);
        if escape {
            return SubOut::Close;
        }
        for e in ev {
            match e {
                Event::Select(_) => {
                    if let Some(b) = self.panel.get_mut(F_REMOVE) {
                        b.active = true;
                    }
                }
                Event::Press(F_REMOVE) => {
                    let sel = self.panel.list.as_ref().and_then(|l| l.selected);
                    if let Some(k) = sel {
                        let mut v = layers(&self.flat);
                        let n = v.len();
                        if k < n {
                            v.remove(n - 1 - k);
                            set_layers(&mut self.flat, &v);
                        }
                        if let Some(l) = &mut self.panel.list {
                            l.selected = if v.is_empty() { None } else { Some(k.min(v.len() - 1)) };
                        }
                        self.size = (-2, -2);
                    }
                }
                Event::Press(F_PRESETS) => return SubOut::OpenPresets(self.flat.clone()),
                Event::Press(F_DONE) => return SubOut::Flat(self.flat.clone()),
                Event::Press(F_CANCEL) => return SubOut::Close,
                _ => {}
            }
        }
        SubOut::None
    }

    fn draw(&mut self, gui: &mut Gui, q: &wgpu::Queue, o: &Options, mouse: Option<(f32, f32)>, time: f64) {
        self.layout(gui);
        background(gui, o);
        let w = gui.width as i32;
        widgets::centered(gui, q, &chars(&t(gui, "createWorld.customize.flat.title")), w / 2, 12, 0xffff_ffff);
        let ls = layers(&self.flat);
        let names: Vec<String> = ls.iter().rev().map(|(b, _)| block_name(gui, b)).collect();
        let heights: Vec<i32> = ls.iter().rev().map(|(_, h)| *h).collect();
        let tile = {
            let mut c = chars(&t(gui, "createWorld.customize.flat.tile"));
            c.iter_mut().for_each(|x| x.style.underlined = true);
            c
        };
        let height = {
            let mut c = chars(&t(gui, "createWorld.customize.flat.height"));
            c.iter_mut().for_each(|x| x.style.underlined = true);
            c
        };
        let selected = self.panel.list.as_ref().and_then(|l| l.selected);
        let focused = self.panel.focus.is_none();
        self.panel.draw_with(gui, q, mouse, time, &mut |gui, q, l| {
            let f = |v: i32| v as f32;
            let left = l.row_left() + 2;
            let right = l.row_left() + l.row_width - 2;
            let hy = l.y - l.scroll.floor() as i32 + 2 + 2;
            gui.text(q, &tile, f(left), f(hy), 0xffff_ffff, true, 0);
            let hw = rc_text::width(gui, &height);
            gui.text(q, &height, f(right - hw), f(hy), 0xffff_ffff, true, 0);
            let n = names.len();
            for k in 0..n {
                let Some((rl, top, rw, rh)) = l.entry_rect(k) else { continue };
                if selected == Some(k) {
                    let c = if focused { 0xffff_ffff } else { 0xff80_8080 };
                    gui.fill(f(rl), f(top), f(rl + rw), f(top + rh), c);
                    gui.fill(f(rl + 1), f(top + 1), f(rl + rw - 1), f(top + rh - 1), 0xff00_0000);
                }
                let (cx, cy) = (rl + 2, top + 2);
                gui.sprite("container/slot", f(cx + 1), f(cy + 1), 18.0, 18.0, 0xffff_ffff);
                let ty = cy + 10 - 4;
                gui.text(q, &chars(&names[k]), f(cx + 23), f(ty), 0xffff_ffff, true, 0);
                let key = if k == 0 {
                    "createWorld.customize.flat.layer.top"
                } else if k == n - 1 {
                    "createWorld.customize.flat.layer.bottom"
                } else {
                    "createWorld.customize.flat.layer"
                };
                let ht = chars(&tr(&gui.lang, key, &[heights[k].to_string()]));
                let hw = rc_text::width(gui, &ht);
                gui.text(q, &ht, f(rl + rw - 2 - hw), f(ty), 0xffff_ffff, true, 0);
            }
        });
    }
}

// ---------- Flat presets ----------

const PR_EXPORT: u32 = 1;
const PR_SELECT: u32 = 2;
const PR_CANCEL: u32 = 3;

struct FlatPresets {
    panel: Panel,
    size: (i32, i32),
    base: Nbt,
    text: String,
}

impl FlatPresets {
    fn new(current: &Nbt) -> FlatPresets {
        FlatPresets { panel: Panel::default(), size: (-1, -1), base: current.clone(), text: flat_text(current) }
    }

    fn layout(&mut self, gui: &Gui, n: usize) {
        let size = (gui.width as i32, gui.height as i32);
        if self.size == size && self.panel.list.as_ref().is_some_and(|l| l.entries.len() == n) {
            return;
        }
        self.size = size;
        let (w, h) = size;
        let mut f = TextField::new(Vec::new());
        f.text = self.text.chars().collect();
        f.max_len = 1230;
        let export = Widget::new(PR_EXPORT, Kind::Field(Box::new(f)), 50, 40, w - 100, 20, Vec::new());
        let mut sel =
            Widget::button(PR_SELECT, w / 2 - 155, h - 28, 150, &t(gui, "createWorld.customize.presets.select"));
        let old = self.panel.list.take();
        let mut l = List::new(0, 80, w, h - 117, 220, 24);
        l.inworld = false;
        l.custom_rows = true;
        l.entries = (0..n as i32).map(|i| Entry { top: i * 24, h: 24, label: Vec::new() }).collect();
        l.content = n as i32 * 24 + 4;
        if let Some(o) = old {
            l.selected = o.selected.filter(|&s| s < n);
            l.set_scroll(o.scroll);
        }
        sel.active = l.selected.is_some() || self.text.chars().count() > 1;
        let keyboard = self.panel.keyboard;
        let focus = self.panel.focus;
        self.panel =
            Panel::new(vec![export, sel, Widget::button(PR_CANCEL, w / 2 + 5, h - 28, 150, &t(gui, "gui.cancel"))]);
        self.panel.list = Some(l);
        self.panel.keyboard = keyboard;
        self.panel.focus = focus;
    }

    fn input(&mut self, gui: &Gui, i: &In<'_>, biomes: &[String], presets: &[rc_sim::storage::FlatPreset]) -> SubOut {
        self.layout(gui, presets.len());
        let (ev, escape) = feed(&mut self.panel, i);
        if escape {
            return SubOut::Close;
        }
        for e in ev {
            let out = self.on_event(e, biomes, presets);
            if self.size == (-2, -2) {
                self.layout(gui, presets.len());
            }
            if !matches!(out, SubOut::None) {
                return out;
            }
        }
        SubOut::None
    }

    /// Screen event; a selection requests the layout again (`size` invalidated).
    fn on_event(&mut self, e: Event, biomes: &[String], presets: &[rc_sim::storage::FlatPreset]) -> SubOut {
        match e {
            // Click, double click or Enter on a preset: selection only (the game applies
            // only with "Use Preset").
            Event::Select(k) | Event::Activate(k) => {
                if let Some(p) = presets.get(k) {
                    self.base = p.settings.clone();
                    self.text = flat_text(&p.settings);
                    self.size = (-2, -2);
                }
            }
            Event::Edit(PR_EXPORT) => {
                if let Some(Widget { kind: Kind::Field(f), .. }) = self.panel.get(PR_EXPORT) {
                    self.text = f.value();
                }
                let ok = self.text.chars().count() > 1 || self.panel.list.as_ref().is_some_and(|l| l.selected.is_some());
                if let Some(b) = self.panel.get_mut(PR_SELECT) {
                    b.active = ok;
                }
            }
            Event::Press(PR_SELECT) => return SubOut::Preset(self.chosen(biomes)),
            Event::Press(PR_CANCEL) => return SubOut::Close,
            _ => {}
        }
        SubOut::None
    }

    /// Text read; invalid: default flat setting (like the game).
    fn chosen(&self, biomes: &[String]) -> Nbt {
        parse_flat(&self.text, &self.base, biomes).unwrap_or_else(default_flat)
    }

    #[allow(clippy::too_many_arguments)]
    fn draw(
        &mut self,
        gui: &mut Gui,
        q: &wgpu::Queue,
        o: &Options,
        mouse: Option<(f32, f32)>,
        time: f64,
        presets: &[rc_sim::storage::FlatPreset],
    ) {
        self.layout(gui, presets.len());
        background(gui, o);
        let w = gui.width as i32;
        widgets::centered(gui, q, &chars(&t(gui, "createWorld.customize.presets.title")), w / 2, 8, 0xffff_ffff);
        gui.text(q, &chars(&t(gui, "createWorld.customize.presets.share")), 51.0, 30.0, 0xffa0_a0a0, true, 0);
        gui.text(q, &chars(&t(gui, "createWorld.customize.presets.list")), 51.0, 68.0, 0xffa0_a0a0, true, 0);
        let names: Vec<String> = presets
            .iter()
            .map(|p| {
                let k = format!("flat_world_preset.{}", p.id.replace(':', "."));
                if gui.lang.map.contains_key(&k) { t(gui, &k) } else { t(gui, "flat_world_preset.unknown") }
            })
            .collect();
        let selected = self.panel.list.as_ref().and_then(|l| l.selected);
        let focused = self.panel.focus.is_none();
        self.panel.draw_with(gui, q, mouse, time, &mut |gui, q, l| {
            let f = |v: i32| v as f32;
            for (k, name) in names.iter().enumerate() {
                let Some((rl, top, rw, rh)) = l.entry_rect(k) else { continue };
                if selected == Some(k) {
                    let c = if focused { 0xffff_ffff } else { 0xff80_8080 };
                    gui.fill(f(rl), f(top), f(rl + rw), f(top + rh), c);
                    gui.fill(f(rl + 1), f(top + 1), f(rl + rw - 1), f(top + rh - 1), 0xff00_0000);
                }
                let (cx, cy) = (rl + 2, top + 2);
                gui.sprite("container/slot", f(cx + 1), f(cy + 1), 18.0, 18.0, 0xffff_ffff);
                gui.text(q, &chars(name), f(cx + 23), f(cy + 6), 0xffff_ffff, true, 0);
            }
        });
    }
}

// ---------- Single-biome world ----------

/// Base letter of an accented Latin letter (lowercase), or the text of a ligature.
fn base_letter(c: char) -> Option<&'static str> {
    Some(match c {
        'à'..='å' | 'ā' | 'ă' | 'ą' => "a",
        'æ' => "ae",
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => "c",
        'ď' | 'đ' => "d",
        'è'..='ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => "g",
        'ĥ' | 'ħ' => "h",
        'ì'..='ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => "i",
        'ĵ' => "j",
        'ķ' => "k",
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => "l",
        'ñ' | 'ń' | 'ņ' | 'ň' => "n",
        'ò'..='ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => "o",
        'œ' => "oe",
        'ŕ' | 'ŗ' | 'ř' => "r",
        'ś' | 'ŝ' | 'ş' | 'š' | 'ș' => "s",
        'ß' => "ss",
        'ţ' | 'ť' | 'ŧ' | 'ț' => "t",
        'ù'..='ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
        'ŵ' => "w",
        'ý' | 'ÿ' | 'ŷ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        _ => return None,
    })
}

/// Sort key of biome names, close to the game's per-language ordering: letters without accent
/// or case first, then the accents (unaccented first), then the case (lowercase
/// first). Covers the Latin alphabet; other scripts keep code point order.
fn collation_key(name: &str) -> (String, String, Vec<bool>) {
    let lower = name.to_lowercase();
    let mut primary = String::with_capacity(lower.len());
    for c in lower.chars() {
        match base_letter(c) {
            Some(b) => primary.push_str(b),
            None => primary.push(c),
        }
    }
    let case = name.chars().map(char::is_uppercase).collect();
    (primary, lower, case)
}

const B_SEARCH: u32 = 1;
const B_DONE: u32 = 2;
const B_CANCEL: u32 = 3;
/// Top of the single-biome screen title (the search follows 12 px lower).
const BUFFET_TITLE_Y: i32 = 7;

struct Buffet {
    panel: Panel,
    size: (i32, i32),
    biome: String,
    filter: String,
    /// Biomes shown (id, name), sorted by name.
    shown: Vec<(String, String)>,
    count: usize,
}

impl Buffet {
    fn new(biome: String) -> Buffet {
        Buffet { panel: Panel::default(), size: (-1, -1), biome, filter: String::new(), shown: Vec::new(), count: 0 }
    }

    fn refill(&mut self, gui: &Gui, biomes: &[String]) {
        let f = self.filter.to_lowercase();
        let mut v: Vec<(String, String)> = biomes
            .iter()
            .map(|id| {
                let k = format!("biome.{}", id.replace(':', "."));
                let name = if gui.lang.map.contains_key(&k) { t(gui, &k) } else { id.clone() };
                (id.clone(), name)
            })
            .filter(|(_, n)| f.is_empty() || n.to_lowercase().contains(&f))
            .collect();
        // Key computed once per biome (≈ 65), not on every comparison.
        v.sort_by_cached_key(|(_, n)| collation_key(n));
        self.shown = v;
        self.count = biomes.len();
    }

    fn layout(&mut self, gui: &Gui, biomes: &[String]) {
        let size = (gui.width as i32, gui.height as i32);
        if self.size == size && self.count == biomes.len() {
            return;
        }
        let first = self.size.0 == -1;
        self.size = size;
        self.refill(gui, biomes);
        let (w, h) = size;
        let mut field = TextField::new(chars(&format!("§7§o{}", t(gui, "createWorld.customize.buffet.search"))));
        field.text = self.filter.chars().collect();
        field.cursor = field.text.len();
        field.anchor = field.cursor;
        // Header of 40: title (9) + 3 + search (15) centred, rounded to nearest (y 7, 19).
        let search =
            Widget::new(B_SEARCH, Kind::Field(Box::new(field)), w / 2 - 100, BUFFET_TITLE_Y + 12, 200, 15, Vec::new());
        let old = self.panel.list.take();
        let mut l = List::new(0, 40, w, h - 40 - 33, 220, 15);
        l.inworld = false;
        l.custom_rows = true;
        let n = self.shown.len() as i32;
        l.entries = (0..n).map(|i| Entry { top: i * 15, h: 15, label: Vec::new() }).collect();
        l.content = n * 15 + 4;
        l.selected = self.shown.iter().position(|(id, _)| *id == self.biome);
        if let Some(o) = old {
            l.set_scroll(o.scroll);
        }
        let mut done = Widget::button(B_DONE, w / 2 - 154, h - 26, 150, &t(gui, "gui.done"));
        done.active = l.selected.is_some();
        let keyboard = self.panel.keyboard;
        let focus = self.panel.focus;
        self.panel =
            Panel::new(vec![search, done, Widget::button(B_CANCEL, w / 2 + 4, h - 26, 150, &t(gui, "gui.cancel"))]);
        self.panel.list = Some(l);
        self.panel.keyboard = keyboard;
        if first {
            self.panel.focus_widget(B_SEARCH);
        } else {
            self.panel.focus = focus;
        }
    }

    fn input(&mut self, gui: &Gui, i: &In<'_>, biomes: &[String]) -> SubOut {
        self.layout(gui, biomes);
        let (ev, escape) = feed(&mut self.panel, i);
        if escape {
            return SubOut::Close;
        }
        for e in ev {
            match e {
                Event::Edit(B_SEARCH) => {
                    if let Some(Widget { kind: Kind::Field(f), .. }) = self.panel.get(B_SEARCH) {
                        self.filter = f.value();
                    }
                    self.size = (-2, -2);
                    self.layout(gui, biomes);
                    if let Some(l) = &mut self.panel.list {
                        l.set_scroll(0.0);
                    }
                }
                Event::Select(k) | Event::Activate(k) => {
                    if let Some((id, _)) = self.shown.get(k) {
                        self.biome = id.clone();
                    }
                    if let Some(b) = self.panel.get_mut(B_DONE) {
                        b.active = true;
                    }
                }
                Event::Press(B_DONE) => return SubOut::Biome(self.biome.clone()),
                Event::Press(B_CANCEL) => return SubOut::Close,
                _ => {}
            }
        }
        SubOut::None
    }

    fn draw(
        &mut self,
        gui: &mut Gui,
        q: &wgpu::Queue,
        o: &Options,
        mouse: Option<(f32, f32)>,
        time: f64,
        biomes: &[String],
    ) {
        self.layout(gui, biomes);
        background(gui, o);
        let w = gui.width as i32;
        widgets::centered(gui, q, &chars(&t(gui, "createWorld.customize.buffet.title")), w / 2, BUFFET_TITLE_Y, 0xffff_ffff);
        let selected = self.panel.list.as_ref().and_then(|l| l.selected);
        let focused = self.panel.focus.is_none();
        let shown = &self.shown;
        self.panel.draw_with(gui, q, mouse, time, &mut |gui, q, l| {
            let f = |v: i32| v as f32;
            for (k, (_, name)) in shown.iter().enumerate() {
                let Some((rl, top, rw, rh)) = l.entry_rect(k) else { continue };
                if top + rh < l.y || top > l.y + l.h {
                    continue;
                }
                if selected == Some(k) {
                    let c = if focused { 0xffff_ffff } else { 0xff80_8080 };
                    gui.fill(f(rl), f(top), f(rl + rw), f(top + rh), c);
                    gui.fill(f(rl + 1), f(top + 1), f(rl + rw - 1), f(top + rh - 1), 0xff00_0000);
                }
                gui.text(q, &chars(name), f(rl + 2 + 5), f(top + 2 + 2), 0xffff_ffff, true, 0);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(settings: &str, source: Nbt) -> Nbt {
        Nbt::Compound(vec![
            ("type".into(), Nbt::String("minecraft:noise".into())),
            ("settings".into(), Nbt::String(settings.into())),
            ("biome_source".into(), source),
        ])
    }

    /// Re-creation: every noise generator goes back to a fresh "Default" (quirk of the
    /// game), flat keeps its layers; the old code kept "Customized" and the generator.
    #[test]
    fn recreated_type_follows_the_game() {
        let mut c = CreateWorld::base(Arc::new(VersionData::default()));
        let fixed = Nbt::Compound(vec![
            ("type".into(), Nbt::String("minecraft:fixed".into())),
            ("biome".into(), Nbt::String("minecraft:desert".into())),
        ]);
        for g in [noise("minecraft:amplified", Nbt::Compound(vec![])), noise("minecraft:overworld", fixed)] {
            c.set_recreated_type(Some(&g));
            assert_eq!(c.preset, Some(0));
            assert!(c.kept.is_none());
            assert_eq!(c.new_world().preset, Preset::Normal);
        }
        let mut flat = default_flat();
        set_layers(&mut flat, &[("minecraft:stone".into(), 5)]);
        let g = Nbt::Compound(vec![("type".into(), Nbt::String("minecraft:flat".into())), ("settings".into(), flat.clone())]);
        c.set_recreated_type(Some(&g));
        assert_eq!(c.preset, Some(1));
        assert_eq!(c.new_world().flat_settings, Some(flat));
        let g = Nbt::Compound(vec![("type".into(), Nbt::String("minecraft:debug".into()))]);
        c.set_recreated_type(Some(&g));
        assert_eq!(c.preset, Some(DEBUG_PRESET));
        let g = Nbt::Compound(vec![("type".into(), Nbt::String("mod:other".into()))]);
        c.set_recreated_type(Some(&g));
        assert_eq!(c.preset, None, "unknown generator: 'Customized'");
    }

    /// Changing type resets the flat layers and the single biome to their default.
    #[test]
    fn changing_type_resets_its_settings() {
        let mut c = CreateWorld::base(Arc::new(VersionData::default()));
        set_layers(&mut c.flat, &[("minecraft:stone".into(), 5)]);
        c.biome = "minecraft:desert".into();
        c.set_preset(cycle_preset(1, 1, false));
        c.set_preset(cycle_preset(2, -1, false));
        assert_eq!(c.preset, Some(1));
        assert_eq!(c.flat, default_flat());
        assert_eq!(c.biome, "minecraft:plains");
    }

    /// Enter creates the world even with the name field focused (initial focus of the screen), through the
    /// real panel; a focused button takes it; another key creates nothing; Escape goes back.
    #[test]
    fn enter_in_the_name_field_creates() {
        use winit::keyboard::ModifiersState;
        let enter = In::Key(KeyCode::Enter, ModifiersState::empty());
        let mut c = CreateWorld::base(Arc::new(VersionData::default()));
        let field = Widget::new(NAME, Kind::Field(Box::new(TextField::new(Vec::new()))), 0, 0, 208, 20, Vec::new());
        c.panel = Panel::new(vec![field]);
        c.panel.focus_widget(NAME);
        assert!(c.panel.focus.is_some());
        assert_eq!(c.feed_main(&enter).1, MainKey::Create, "Enter in the name field");
        assert_eq!(c.feed_main(&In::Key(KeyCode::KeyA, ModifiersState::empty())).1, MainKey::None);
        assert_eq!(c.feed_main(&In::Key(KeyCode::Escape, ModifiersState::empty())).1, MainKey::Escape);
        let button = Widget::new(CREATE, Kind::Button, 0, 30, 150, 20, Vec::new());
        c.panel = Panel::new(vec![button]);
        c.panel.focus_widget(CREATE);
        let (ev, key) = c.feed_main(&enter);
        assert_eq!(key, MainKey::None, "Enter taken by a focused button");
        assert_eq!(ev, [Event::Press(CREATE)]);
    }

    /// World tab labels (the only ones drawn) white: the bonus chest too, even in
    /// hardcore; the old code greyed it.
    #[test]
    fn bonus_label_stays_white_in_hardcore() {
        let labels = tab_labels(1);
        let bonus = labels.iter().find(|l| l.0 == "selectWorld.bonusItems").expect("bonus chest label");
        assert_eq!(bonus.3, 0xffff_ffff);
        assert!(labels.iter().chain(tab_labels(0)).all(|l| l.3 == 0xffff_ffff));
    }

    /// Experimental features warning: "Details" of 100 centred under the message,
    /// "Continue" / "Back" below, grid centred (427 × 240, message of 3 lines).
    #[test]
    fn experimental_warning_has_details() {
        let g = exp_grid(427, 240, 3);
        assert_eq!(g, ExpGrid { top: 52, details: (163, 128), proceed: (58, 168), back: (218, 168) });
        assert_eq!(pack_features("trade_rebalance").as_deref(), Some("minecraft:trade_rebalance"));
        assert_eq!(pack_features("vanilla"), None);
    }

    #[test]
    fn flat_text_round_trip() {
        let d = default_flat();
        assert_eq!(flat_text(&d), "minecraft:bedrock,2*minecraft:dirt,minecraft:grass_block;minecraft:plains");
        let n = parse_flat("minecraft:bedrock,3*stone;minecraft:desert", &d, &[]).unwrap();
        assert_eq!(layers(&n), vec![("minecraft:bedrock".to_owned(), 1), ("minecraft:stone".to_owned(), 3)]);
        assert_eq!(n.get("biome").and_then(Nbt::as_str), Some("minecraft:desert"));
        assert!(parse_flat(";", &d, &[]).is_none());
    }

    #[test]
    fn packs_from_selection() {
        let p = packs_of(&["vanilla".into(), "trade_rebalance".into()]);
        assert_eq!(p.disabled, ["minecart_improvements", "redstone_experiments"]);
        assert_eq!(p.features, ["minecraft:vanilla", "minecraft:trade_rebalance"]);
    }

    #[test]
    fn alt_list_reaches_the_debug_world() {
        // Without Alt: 5 types, debug is never reached.
        assert_eq!(cycle_preset(4, 1, false), 0);
        assert_eq!(cycle_preset(0, -1, false), 4);
        // Alt: extended list (tag `extended`), debug last.
        assert_eq!(PRESETS[cycle_preset(4, 1, true)], "debug_all_block_states");
        assert_eq!(cycle_preset(0, -1, true), DEBUG_PRESET);
        // From debug without Alt: the kept index falls back into the short list.
        assert_eq!(cycle_preset(DEBUG_PRESET, 1, false), 1);
    }

    #[test]
    fn debug_world_is_spectator_peaceful_with_commands() {
        let mut c = CreateWorld::base(Arc::new(VersionData::default()));
        c.mode = Mode::Hardcore;
        c.bonus = true;
        c.rules = vec![("minecraft:pvp".into(), "false".into())];
        c.preset = Some(DEBUG_PRESET);
        let w = c.new_world();
        assert_eq!(w.preset, Preset::Debug);
        assert_eq!((w.game_mode, w.hardcore, w.allow_commands), (3, false, true));
        assert_eq!(w.difficulty, "peaceful");
        assert!(!w.generate_structures && !w.bonus_chest);
        assert_eq!(w.game_rules, vec![("minecraft:advance_time".to_owned(), Nbt::Byte(0))]);
        assert_eq!(c.mode_key(), "spectator");
    }

    #[test]
    fn flat_layers_list_spans_header_to_footer() {
        // Header 33, footer 64 (not y 43, height h − 103).
        assert_eq!(flat_list_bounds(480), (33, 383));
    }

    #[test]
    fn flat_preset_double_click_only_selects() {
        let mut p = FlatPresets::new(&default_flat());
        let mut settings = default_flat();
        set_layers(&mut settings, &[("minecraft:stone".into(), 3)]);
        let presets = [rc_sim::storage::FlatPreset { id: "x".into(), display: "minecraft:stone".into(), settings }];
        assert!(matches!(p.on_event(Event::Activate(0), &[], &presets), SubOut::None));
        assert!(p.text.starts_with("3*minecraft:stone"), "{}", p.text);
        assert!(matches!(p.on_event(Event::Press(PR_SELECT), &[], &presets), SubOut::Preset(_)));
    }

    #[test]
    fn biome_names_sort_without_accents_or_case() {
        // French sample names on purpose: the accents exercise the folding of the collation key.
        let mut v = vec!["Plaines", "Forêt sombre", "forêt", "Fleurs", "Étendues", "Eaux", "Forêt"];
        v.sort_by_cached_key(|n| collation_key(n));
        assert_eq!(v, ["Eaux", "Étendues", "Fleurs", "forêt", "Forêt", "Forêt sombre", "Plaines"]);
    }

    #[test]
    fn buffet_header_matches_rounded_layout() {
        assert_eq!(BUFFET_TITLE_Y, 7);
    }
}
