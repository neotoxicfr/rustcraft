//! Operator screens (`docs/specs/special-screens.md` § 6): command block and command block
//! minecart (with command suggestions), structure block, jigsaw block, test block and
//! test instance block, identical to the game. Opened locally by a creative player of
//! permission level ≥ 2; the data comes from the known block entities (`tech_blocks.rs`).

use rc_proto::BlockPos;
use rc_proto::nbt::Nbt;
use rc_text::{Component, StyledChar};
use winit::keyboard::{KeyCode, ModifiersState};

use crate::gui::Gui;
use crate::tech_blocks::{Jigsaw, Structure, Tech, TestInstance};
use crate::widgets::{Event, Kind as W, Panel, TextField, Widget};

/// What the screen asks of the application.
#[derive(Debug, Clone, PartialEq)]
pub enum OpAction {
    Close,
    Send(OpPacket),
    Click,
    /// Structure settings applied right away to the client's block entity (in-game box).
    Live([i32; 3], Box<Structure>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum OpPacket {
    CommandBlock { pos: BlockPos, command: String, mode: i32, flags: u8 },
    Minecart { entity: i32, command: String, track: bool },
    Structure { pos: BlockPos, s: Box<rc_proto_v777::special::StructureBlock> },
    Jigsaw { pos: BlockPos, fields: [String; 5], selection: i32, placement: i32 },
    JigsawGenerate { pos: BlockPos, levels: i32, keep: bool },
    TestBlock { pos: BlockPos, mode: i32, message: String },
    TestInstance { pos: BlockPos, action: i32, test: Option<String>, size: [i32; 3], rotation: i32, ignore: bool },
}

/// State specific to each editor.
#[derive(Debug, Clone)]
enum Ed {
    Command {
        /// Minecart: id of the entity.
        minecart: Option<i32>,
        mode: i32,
        conditional: bool,
        auto: bool,
        track: bool,
        /// Block data received (active commands).
        ready: bool,
        output: String,
    },
    Structure {
        s: Structure,
        initial: Structure,
    },
    Jigsaw {
        vertical: bool,
        aligned: bool,
        levels: i32,
        slider: f64,
        keep: bool,
    },
    Test {
        mode: i32,
    },
    TestInstance {
        rotation: i32,
        entities: bool,
        /// Full text of the information area (status received or invalid id).
        info: Vec<StyledChar>,
        /// Information area: scrolling (pixels) and focus.
        scroll: f64,
        info_focus: bool,
    },
}

/// Information area of the test instance: position (y), size, inner margin,
/// scroll per notch, scroll bar width (on the right, outside the frame).
const INFO_Y: i32 = 70;
const INFO_W: i32 = 316;
const INFO_H: i32 = 72;
const INFO_PAD: i32 = 4;
const INFO_STEP: f64 = 9.0;
const INFO_BAR: i32 = 6;

/// Lines of the information area (inner width 308); one empty line per repeated
/// line break.
fn info_lines(gui: &Gui, info: &[StyledChar]) -> Vec<Vec<StyledChar>> {
    let mut out = Vec::new();
    if info.is_empty() {
        return out;
    }
    for para in info.split(|ch| ch.c == '\n') {
        let wrapped = rc_text::wrap(gui, para, (INFO_W - 2 * INFO_PAD) as f32);
        if wrapped.is_empty() {
            out.push(Vec::new());
        } else {
            out.extend(wrapped);
        }
    }
    out
}

/// Maximum scroll: content (lines × 9 + margins) minus the height of the area.
fn info_max_scroll(lines: usize) -> f64 {
    f64::from((lines as i32 * 9 + 2 * INFO_PAD - INFO_H).max(0))
}

pub struct OpScreen {
    pub pos: [i32; 3],
    ed: Ed,
    panel: Panel,
    size: (i32, i32),
    /// Text of the fields (id − 1), kept when the panel is redone.
    values: Vec<String>,
    /// Command input for the suggestions (command block, minecart).
    pub chat: Option<Box<crate::chat::Chat>>,
    /// Initial request (test instance) to send at the first draw.
    init_sent: bool,
}

// Fields: 1 to 10. Buttons: 20 and up.
const DONE: u32 = 100;
const CANCEL: u32 = 101;
const MODE: u32 = 20;
const COND: u32 = 21;
const AUTO: u32 = 22;
const TRACK: u32 = 23;
const SAVE: u32 = 30;
const LOAD: u32 = 31;
const DETECT: u32 = 32;
const ENTITIES: u32 = 33;
const STRICT: u32 = 34;
const MIRROR: u32 = 35;
const AIR: u32 = 36;
const BBOX: u32 = 37;
const ROT: u32 = 40; // 40..=43
const JOINT: u32 = 50;
const LEVELS: u32 = 51;
const KEEP: u32 = 52;
const GENERATE: u32 = 53;
const RESET: u32 = 60;
const TSAVE: u32 = 61;
const RUN: u32 = 62;
const TROT: u32 = 63;
const TENT: u32 = 64;

const GRAY: u32 = 0xffa0_a0a0;

fn field(id: u32, x: i32, y: i32, w: i32, text: &str, max: usize) -> Widget {
    let mut t = TextField::new(Vec::new());
    t.max_len = max;
    t.set_value(text);
    Widget::new(id, W::Field(Box::new(t)), x, y, w, 20, Vec::new())
}

fn tr(gui: &Gui, key: &str) -> String {
    crate::widgets::tr(&gui.lang, key, &[])
}

fn on_off(gui: &Gui, v: bool) -> String {
    tr(gui, if v { "options.on" } else { "options.off" })
}

/// Valid id: rendered normalised with the namespace.
pub fn identifier(s: &str) -> Option<String> {
    let path_ok = |p: &str| p.chars().all(|c| matches!(c, '_' | '-' | 'a'..='z' | '0'..='9' | '/' | '.'));
    let ns_ok = |n: &str| n != ".." && n.chars().all(|c| matches!(c, '_' | '-' | 'a'..='z' | '0'..='9' | '.'));
    match s.split_once(':') {
        Some((ns, p)) if path_ok(p) && (ns.is_empty() || ns_ok(ns)) => {
            Some(format!("{}:{p}", if ns.is_empty() { "minecraft" } else { ns }))
        }
        Some(_) => None,
        None if path_ok(s) => Some(format!("minecraft:{s}")),
        None => None,
    }
}

/// Character allowed in a structure name, depending on its place (`:` once before `/`).
fn name_char_ok(cur: &[char], c: char, at: usize) -> bool {
    let colon = cur.iter().position(|&x| x == ':');
    let slash = cur.iter().position(|&x| x == '/');
    match c {
        ':' => slash.is_none_or(|s| at <= s) && colon.is_none(),
        '/' => colon.is_none_or(|p| at > p),
        _ => matches!(c, '_' | '-' | 'a'..='z' | '0'..='9' | '.'),
    }
}

/// Decimal format `0.0###`: one to four decimals.
fn integrity_text(v: f32) -> String {
    let s = format!("{:.4}", f64::from(v));
    let t = s.trim_end_matches('0');
    if t.ends_with('.') { format!("{t}0") } else { t.to_owned() }
}

fn parse_int(s: &str) -> i32 {
    s.parse().unwrap_or(0)
}

/// Reading of a float the Java way: blanks around and f/d suffix allowed, 1 otherwise.
fn parse_integrity(s: &str) -> f32 {
    let t = s.trim();
    let t = t.strip_suffix(['f', 'F', 'd', 'D']).unwrap_or(t);
    t.parse().unwrap_or(1.0)
}

impl OpScreen {
    fn new(pos: [i32; 3], ed: Ed, values: Vec<String>) -> OpScreen {
        let chat = matches!(ed, Ed::Command { .. }).then(|| {
            let mut c = Box::<crate::chat::Chat>::default();
            c.open("");
            c
        });
        OpScreen { pos, ed, panel: Panel::default(), size: (0, 0), values, chat, init_sent: false }
    }

    pub fn is_command_block(&self) -> bool {
        matches!(self.ed, Ed::Command { minecart: None, .. })
    }

    pub fn minecart(&self) -> Option<i32> {
        match self.ed {
            Ed::Command { minecart, .. } => minecart,
            _ => None,
        }
    }

    pub fn is_test_instance(&self) -> bool {
        matches!(self.ed, Ed::TestInstance { .. })
    }

    /// Block data received (command block): command, output, modes; active commands.
    pub fn refresh(&mut self, command: String, output: String, track: bool, auto: bool, mode: i32, cond: bool) {
        self.sync_values();
        if let Ed::Command { mode: m, conditional, auto: a, track: t, ready, output: o, .. } = &mut self.ed {
            *m = mode;
            *conditional = cond;
            *a = auto;
            *t = track;
            *ready = true;
            *o = output;
        }
        self.values[0] = command;
        // Components redone with these values at the next draw.
        self.panel = Panel::default();
    }

    fn focus_id(&self) -> Option<u32> {
        self.panel.focus.and_then(|i| self.panel.widgets.get(i)).map(|w| w.id)
    }

    fn text_field(&mut self, id: u32) -> Option<&mut TextField> {
        match self.panel.get_mut(id).map(|w| &mut w.kind) {
            Some(W::Field(t)) => Some(t),
            _ => None,
        }
    }

    fn value(&self, id: u32) -> String {
        match self.panel.get(id).map(|w| &w.kind) {
            Some(W::Field(t)) => t.value(),
            _ => self.values.get(id as usize - 1).cloned().unwrap_or_default(),
        }
    }

    fn sync_values(&mut self) {
        self.values.resize(10, String::new());
        for w in &self.panel.widgets {
            if let W::Field(t) = &w.kind
                && (1..=10).contains(&w.id)
            {
                self.values[w.id as usize - 1] = t.value();
            }
        }
    }

    fn set_value(&mut self, id: u32, s: &str) {
        if let Some(t) = self.text_field(id) {
            t.text.clear();
            t.cursor = 0;
            t.anchor = 0;
            t.offset = 0;
            t.set_value(s);
        }
        self.values.resize(10, String::new());
        self.values[id as usize - 1] = s.to_owned();
    }

    fn label(&self, gui: &Gui, id: u32) -> String {
        match (&self.ed, id) {
            (Ed::Command { mode, .. }, MODE) => tr(
                gui,
                ["advMode.mode.sequence", "advMode.mode.auto", "advMode.mode.redstone"][(*mode).clamp(0, 2) as usize],
            ),
            (Ed::Command { conditional, .. }, COND) => {
                tr(gui, if *conditional { "advMode.mode.conditional" } else { "advMode.mode.unconditional" })
            }
            (Ed::Command { auto, .. }, AUTO) => {
                tr(gui, if *auto { "advMode.mode.autoexec.bat" } else { "advMode.mode.redstoneTriggered" })
            }
            (Ed::Command { track, .. }, TRACK) => (if *track { "O" } else { "X" }).to_owned(),
            (Ed::Structure { s, .. }, MODE) => {
                let k = ["save", "load", "corner", "data"][s.mode.clamp(0, 3) as usize];
                tr(gui, &format!("structure_block.mode.{k}"))
            }
            (Ed::Structure { s, .. }, ENTITIES) => on_off(gui, !s.ignore_entities),
            (Ed::Structure { s, .. }, STRICT) => on_off(gui, s.strict),
            (Ed::Structure { s, .. }, AIR) => on_off(gui, s.show_air),
            (Ed::Structure { s, .. }, BBOX) => on_off(gui, s.show_bounding_box),
            (Ed::Structure { s, .. }, MIRROR) => {
                tr(gui, ["mirror.none", "mirror.left_right", "mirror.front_back"][s.mirror.clamp(0, 2) as usize])
            }
            (Ed::Jigsaw { aligned, .. }, JOINT) => {
                tr(gui, if *aligned { "jigsaw_block.joint.aligned" } else { "jigsaw_block.joint.rollable" })
            }
            (Ed::Jigsaw { levels, .. }, LEVELS) => {
                crate::widgets::tr(&gui.lang, "jigsaw_block.levels", &[levels.to_string()])
            }
            (Ed::Jigsaw { keep, .. }, KEEP) => crate::widgets::tr(
                &gui.lang,
                "options.generic_value",
                &[tr(gui, "jigsaw_block.keep_jigsaws"), on_off(gui, *keep)],
            ),
            (Ed::Test { mode }, MODE) => tr(
                gui,
                &format!("test_block.mode.{}", ["start", "log", "fail", "accept"][(*mode).clamp(0, 3) as usize]),
            ),
            (Ed::TestInstance { rotation, .. }, TROT) => {
                ["0", "90", "180", "270"][(*rotation).clamp(0, 3) as usize].into()
            }
            (Ed::TestInstance { entities, .. }, TENT) => on_off(gui, *entities),
            _ => String::new(),
        }
    }

    fn relabel(&mut self, gui: &Gui, id: u32) {
        let label = crate::widgets::chars(&self.label(gui, id));
        if let Some(w) = self.panel.get_mut(id) {
            w.msg = label;
        }
    }

    /// Components redone at the screen size.
    fn layout(&mut self, gui: &Gui) {
        let (w, h) = (gui.width as i32, gui.height as i32);
        if self.size == (w, h) && !self.panel.widgets.is_empty() {
            return;
        }
        let first = self.panel.widgets.is_empty() && self.size == (0, 0);
        if !self.panel.widgets.is_empty() {
            self.sync_values();
        }
        self.values.resize(10, String::new());
        self.size = (w, h);
        let v = |i: u32| self.values[i as usize - 1].clone();
        let c = w / 2;
        let mut ws = Vec::new();
        let cycle = |id: u32, x: i32, y: i32, width: i32| Widget::new(id, W::Cycle, x, y, width, 20, Vec::new());
        let button = |id: u32, x: i32, y: i32, width: i32, key: &str| Widget::button(id, x, y, width, &tr(gui, key));
        let mut focus = 1;
        match &self.ed {
            Ed::Command { minecart, track, ready, output, .. } => {
                let prev_y = if minecart.is_some() { 150 } else { 135 };
                ws.push(field(1, c - 150, 50, 300, &v(1), 32500));
                let mut out = field(2, c - 150, prev_y, 276, if *track { output } else { "-" }, 32500);
                if let W::Field(t) = &mut out.kind {
                    t.editable = false;
                }
                ws.push(out);
                ws.push(cycle(TRACK, c + 150 - 20, prev_y, 20));
                if minecart.is_none() {
                    ws.push(cycle(MODE, c - 154, 165, 100));
                    ws.push(cycle(COND, c - 50, 165, 100));
                    ws.push(cycle(AUTO, c + 54, 165, 100));
                }
                ws.push(button(DONE, c - 154, h / 4 + 132, 150, "gui.done"));
                ws.push(button(CANCEL, c + 4, h / 4 + 132, 150, "gui.cancel"));
                if minecart.is_none() && !*ready {
                    for w in &mut ws {
                        if matches!(w.id, DONE | TRACK | MODE | COND | AUTO) {
                            w.active = false;
                        }
                    }
                }
            }
            Ed::Structure { s, .. } => {
                if first {
                    let vals = [
                        s.name.clone(),
                        s.pos[0].to_string(),
                        s.pos[1].to_string(),
                        s.pos[2].to_string(),
                        s.size[0].to_string(),
                        s.size[1].to_string(),
                        s.size[2].to_string(),
                        integrity_text(s.integrity),
                        s.seed.to_string(),
                        s.metadata.clone(),
                    ];
                    self.values = vals.to_vec();
                }
                let v = |i: u32| self.values[i as usize - 1].clone();
                ws.push(button(DONE, c - 154, 210, 150, "gui.done"));
                ws.push(button(CANCEL, c + 4, 210, 150, "gui.cancel"));
                ws.push(button(SAVE, c + 104, 185, 50, "structure_block.button.save"));
                ws.push(button(LOAD, c + 104, 185, 50, "structure_block.button.load"));
                ws.push(cycle(MODE, c - 154, 185, 50));
                ws.push(button(DETECT, c + 104, 120, 50, "structure_block.button.detect_size"));
                ws.push(cycle(ENTITIES, c + 104, 160, 50));
                ws.push(cycle(STRICT, c + 104, 120, 50));
                ws.push(cycle(MIRROR, c - 20, 185, 40));
                ws.push(cycle(AIR, c + 104, 80, 50));
                ws.push(cycle(BBOX, c + 104, 80, 50));
                for (i, (x, t)) in
                    [(c - 102, "0"), (c - 61, "90"), (c + 21, "180"), (c + 62, "270")].into_iter().enumerate()
                {
                    ws.push(Widget::button(ROT + i as u32, x, 185, 40, t));
                }
                ws.push(field(1, c - 152, 40, 300, &v(1), 128));
                for (i, x) in [c - 152, c - 72, c + 8].into_iter().enumerate() {
                    ws.push(field(2 + i as u32, x, 80, 80, &v(2 + i as u32), 15));
                }
                for (i, x) in [c - 152, c - 72, c + 8].into_iter().enumerate() {
                    ws.push(field(5 + i as u32, x, 120, 80, &v(5 + i as u32), 15));
                }
                ws.push(field(8, c - 152, 120, 80, &v(8), 15));
                ws.push(field(9, c - 72, 120, 80, &v(9), 31));
                ws.push(field(10, c - 152, 120, 240, &v(10), 128));
            }
            Ed::Jigsaw { vertical, slider, .. } => {
                focus = 1;
                ws.push(field(1, c - 153, 20, 300, &v(1), 128));
                ws.push(field(2, c - 153, 55, 300, &v(2), 128));
                ws.push(field(3, c - 153, 90, 300, &v(3), 128));
                ws.push(field(4, c - 153, 125, 300, &v(4), 256));
                let tip = |key: &str| rc_text::wrap(gui, &crate::widgets::chars(&tr(gui, key)), 170.0);
                let mut sel = field(5, c - 153, 160, 98, &v(5), 3);
                sel.tooltip = tip("jigsaw_block.selection_priority.tooltip");
                ws.push(sel);
                let mut place = field(6, c - 50, 160, 98, &v(6), 3);
                place.tooltip = tip("jigsaw_block.placement_priority.tooltip");
                ws.push(place);
                let mut joint = cycle(JOINT, c + 54, 160, 100);
                joint.visible = *vertical;
                joint.active = *vertical;
                ws.push(joint);
                ws.push(Widget::new(
                    LEVELS,
                    W::Slider { value: *slider, steps: 0, adjusting: false },
                    c - 154,
                    185,
                    100,
                    20,
                    Vec::new(),
                ));
                ws.push(cycle(KEEP, c - 50, 185, 100));
                ws.push(button(GENERATE, c + 54, 185, 100, "jigsaw_block.generate"));
                ws.push(button(DONE, c - 154, 210, 150, "gui.done"));
                ws.push(button(CANCEL, c + 4, 210, 150, "gui.cancel"));
            }
            Ed::Test { .. } => {
                ws.push(field(1, c - 152, 80, 240, &v(1), 128));
                ws.push(cycle(MODE, c - 154, 185, 50));
                ws.push(button(DONE, c - 154, 210, 150, "gui.done"));
                ws.push(button(CANCEL, c + 4, 210, 150, "gui.cancel"));
            }
            Ed::TestInstance { .. } => {
                let left = c - 158;
                let wsize = |n: i32| ((316 - (n - 1) * 8) as f32 / n as f32) as i32;
                let wx =
                    |i: i32, n: i32| (left as f32 + i as f32 * (8.0 + (316 - (n - 1) * 8) as f32 / n as f32)) as i32;
                ws.push(field(1, left, 40, 316, &v(1), 128));
                for i in 0..3 {
                    ws.push(field(2 + i as u32, wx(i, 5), 160, wsize(5), &v(2 + i as u32), 15));
                }
                ws.push(cycle(TROT, wx(3, 5), 160, wsize(5)));
                ws.push(cycle(TENT, wx(4, 5), 160, wsize(5)));
                ws.push(button(RESET, wx(0, 2), 185, wsize(2), "test_instance.action.reset"));
                ws.push(button(TSAVE, wx(1, 2), 185, wsize(2), "test_instance.action.save"));
                ws.push(button(RUN, wx(0, 3), 210, wsize(3), "test_instance.action.run"));
                ws.push(button(DONE, wx(1, 3), 210, wsize(3), "gui.done"));
                ws.push(button(CANCEL, wx(2, 3), 210, wsize(3), "gui.cancel"));
            }
        }
        self.panel = Panel::new(ws);
        for id in [MODE, COND, AUTO, TRACK, ENTITIES, STRICT, MIRROR, AIR, BBOX, JOINT, LEVELS, KEEP, TROT, TENT] {
            self.relabel(gui, id);
        }
        self.update_state();
        self.panel.focus_widget(focus);
    }

    /// Visibility and activity of the components according to the state (mode, rotation, validity).
    fn update_state(&mut self) {
        match self.ed.clone() {
            Ed::Structure { s, .. } => {
                let show: &[u32] = match s.mode {
                    0 => &[1, 2, 3, 4, 5, 6, 7, SAVE, DETECT, ENTITIES, AIR],
                    1 => &[1, 2, 3, 4, 8, 9, LOAD, ENTITIES, STRICT, MIRROR, ROT, ROT + 1, ROT + 2, ROT + 3, BBOX],
                    2 => &[1],
                    _ => &[10],
                };
                for w in &mut self.panel.widgets {
                    let toggled = (1..=10).contains(&w.id)
                        || matches!(w.id, SAVE | LOAD | DETECT | ENTITIES | STRICT | MIRROR | AIR | BBOX)
                        || (ROT..ROT + 4).contains(&w.id);
                    if toggled {
                        w.visible = show.contains(&w.id);
                    }
                    if (ROT..ROT + 4).contains(&w.id) {
                        w.active = w.id - ROT != s.rotation as u32;
                    }
                }
            }
            Ed::Jigsaw { .. } => {
                let ok = [1, 2, 3].iter().all(|&i| identifier(&self.value(i)).is_some());
                for id in [DONE, GENERATE] {
                    if let Some(w) = self.panel.get_mut(id) {
                        w.active = ok;
                    }
                }
            }
            Ed::Test { mode } => {
                if let Some(w) = self.panel.get_mut(1) {
                    w.visible = mode != 0;
                }
            }
            Ed::TestInstance { rotation, .. } => {
                let ok = rotation == 0 && identifier(&self.value(1)).is_some();
                if let Some(w) = self.panel.get_mut(TSAVE) {
                    w.active = ok;
                }
            }
            Ed::Command { .. } => {}
        }
    }

    fn handle(&mut self, gui: Option<&Gui>, events: Vec<Event>, alt: bool) -> (Vec<OpAction>, bool) {
        let mut out = Vec::new();
        let mut edited = false;
        if std::mem::take(&mut self.panel.clicks) > 0 {
            out.push(OpAction::Click);
        }
        for e in events {
            match e {
                Event::Press(id) => self.pressed(id, &mut out),
                Event::Cycle(id, d) => {
                    self.cycled(id, d, alt, &mut out);
                    if let Some(g) = gui {
                        self.relabel(g, id);
                    }
                    self.update_state();
                }
                Event::Slide(LEVELS, v) => {
                    if let Ed::Jigsaw { levels, slider, .. } = &mut self.ed {
                        *slider = v;
                        *levels = (v * 20.0).clamp(0.0, 20.0).floor() as i32;
                    }
                    if let Some(g) = gui {
                        self.relabel(g, LEVELS);
                    }
                }
                Event::Edit(id) => {
                    edited |= id == 1;
                    self.update_state();
                    if id == 1 && self.is_test_instance() {
                        out.push(self.test_instance(1));
                        if identifier(&self.value(1)).is_none()
                            && let Ed::TestInstance { info, .. } = &mut self.ed
                        {
                            *info = invalid_id();
                        }
                    }
                }
                _ => {}
            }
        }
        (out, edited)
    }

    fn structure_live(&self, out: &mut Vec<OpAction>) {
        if let Ed::Structure { s, .. } = &self.ed {
            out.push(OpAction::Live(self.pos, Box::new(s.clone())));
        }
    }

    fn cycled(&mut self, id: u32, d: i32, alt: bool, out: &mut Vec<OpAction>) {
        let mut live = false;
        match (&mut self.ed, id) {
            (Ed::Command { mode, .. }, MODE) => *mode = (*mode + d).rem_euclid(3),
            (Ed::Command { conditional, .. }, COND) => *conditional = !*conditional,
            (Ed::Command { auto, .. }, AUTO) => *auto = !*auto,
            (Ed::Command { track, output, .. }, TRACK) => {
                *track = !*track;
                let shown = if *track { output.clone() } else { "-".into() };
                self.set_value(2, &shown);
                return;
            }
            (Ed::Structure { s, .. }, MODE) => {
                // "Data" mode only with Alt (the game's alternate list).
                let list: &[i32] = if alt { &[0, 1, 2, 3] } else { &[0, 1, 2] };
                let i = list.iter().position(|&m| m == s.mode).map_or(-1, |i| i as i32);
                s.mode = list[(i + d).rem_euclid(list.len() as i32) as usize];
                live = true;
            }
            (Ed::Structure { s, .. }, ENTITIES) => {
                s.ignore_entities = !s.ignore_entities;
                live = true;
            }
            (Ed::Structure { s, .. }, STRICT) => {
                s.strict = !s.strict;
                live = true;
            }
            (Ed::Structure { s, .. }, AIR) => {
                s.show_air = !s.show_air;
                live = true;
            }
            (Ed::Structure { s, .. }, BBOX) => {
                s.show_bounding_box = !s.show_bounding_box;
                live = true;
            }
            (Ed::Structure { s, .. }, MIRROR) => {
                s.mirror = (s.mirror + d).rem_euclid(3);
                live = true;
            }
            (Ed::Jigsaw { aligned, .. }, JOINT) => *aligned = !*aligned,
            (Ed::Jigsaw { keep, .. }, KEEP) => *keep = !*keep,
            (Ed::Test { mode }, MODE) => *mode = (*mode + d).rem_euclid(4),
            (Ed::TestInstance { rotation, .. }, TROT) => *rotation = (*rotation + d).rem_euclid(4),
            (Ed::TestInstance { entities, .. }, TENT) => *entities = !*entities,
            _ => {}
        }
        if live {
            self.structure_live(out);
        }
    }

    fn pressed(&mut self, id: u32, out: &mut Vec<OpAction>) {
        if let Ed::Structure { s, .. } = &mut self.ed
            && (ROT..ROT + 4).contains(&id)
        {
            s.rotation = (id - ROT) as i32;
            self.update_state();
            self.structure_live(out);
            return;
        }
        match id {
            DONE => self.done(out),
            CANCEL => self.cancel(out),
            SAVE | LOAD | DETECT => {
                let (mode, action) = structure_button(id);
                if matches!(&self.ed, Ed::Structure { s, .. } if s.mode == mode) {
                    out.push(OpAction::Send(self.structure_packet(action)));
                    out.push(OpAction::Close);
                }
            }
            GENERATE => {
                self.done(out);
                if let Ed::Jigsaw { levels, keep, .. } = self.ed {
                    out.push(OpAction::Send(OpPacket::JigsawGenerate { pos: self.block_pos(), levels, keep }));
                }
            }
            RESET | TSAVE | RUN => {
                out.push(self.test_instance(match id {
                    RESET => 3,
                    TSAVE => 4,
                    _ => 6,
                }));
                out.push(OpAction::Close);
            }
            _ => {}
        }
    }

    fn block_pos(&self) -> BlockPos {
        BlockPos::new(self.pos[0], self.pos[1], self.pos[2])
    }

    fn structure_packet(&self, action: i32) -> OpPacket {
        let Ed::Structure { s, .. } = &self.ed else { unreachable!("structure screen") };
        let byte = |id: u32| parse_int(&self.value(id)) as i8;
        let seed = self.value(9).parse::<i64>().unwrap_or(0);
        let flags = u8::from(s.ignore_entities)
            | u8::from(s.show_air) << 1
            | u8::from(s.show_bounding_box) << 2
            | u8::from(s.strict) << 3;
        OpPacket::Structure {
            pos: self.block_pos(),
            s: Box::new(rc_proto_v777::special::StructureBlock {
                action,
                mode: s.mode,
                name: self.value(1),
                offset: [byte(2), byte(3), byte(4)],
                size: [byte(5), byte(6), byte(7)],
                mirror: s.mirror,
                rotation: s.rotation,
                data: self.value(10),
                integrity: parse_integrity(&self.value(8)),
                seed,
                flags,
            }),
        }
    }

    fn test_instance(&self, action: i32) -> OpAction {
        let Ed::TestInstance { rotation, entities, .. } = self.ed else { unreachable!("test instance") };
        let size = |id: u32| self.value(id).parse::<i32>().map_or(1, |v| v.clamp(1, 48));
        OpAction::Send(OpPacket::TestInstance {
            pos: self.block_pos(),
            action,
            test: identifier(&self.value(1)),
            size: [size(2), size(3), size(4)],
            rotation,
            ignore: !entities,
        })
    }

    fn done(&mut self, out: &mut Vec<OpAction>) {
        let pos = self.block_pos();
        match &self.ed {
            Ed::Command { minecart: Some(entity), track, .. } => {
                out.push(OpAction::Send(OpPacket::Minecart { entity: *entity, command: self.value(1), track: *track }));
            }
            Ed::Command { mode, conditional, auto, track, .. } => {
                out.push(OpAction::Send(OpPacket::CommandBlock {
                    pos,
                    command: self.value(1),
                    mode: *mode,
                    flags: u8::from(*track) | u8::from(*conditional) << 1 | u8::from(*auto) << 2,
                }));
            }
            Ed::Structure { .. } => out.push(OpAction::Send(self.structure_packet(0))),
            Ed::Jigsaw { aligned, .. } => {
                let id = |i: u32| identifier(&self.value(i)).unwrap_or_default();
                out.push(OpAction::Send(OpPacket::Jigsaw {
                    pos,
                    fields: [
                        id(2),
                        id(3),
                        id(1),
                        self.value(4),
                        (if *aligned { "aligned" } else { "rollable" }).into(),
                    ],
                    selection: parse_int(&self.value(5)),
                    placement: parse_int(&self.value(6)),
                }));
            }
            Ed::Test { mode } => {
                out.push(OpAction::Send(OpPacket::TestBlock { pos, mode: *mode, message: self.value(1) }));
            }
            Ed::TestInstance { .. } => out.push(self.test_instance(2)),
        }
        out.push(OpAction::Close);
    }

    /// Cancel / Escape: the structure goes back to its original settings.
    fn cancel(&mut self, out: &mut Vec<OpAction>) {
        if let Ed::Structure { s, initial } = &mut self.ed {
            *s = initial.clone();
            self.structure_live(out);
        }
        out.push(OpAction::Close);
    }

    pub fn press(&mut self, gui: &Gui, m: (f32, f32), b: u8, mods: ModifiersState) -> (Vec<OpAction>, bool) {
        self.layout(gui);
        // Information area: a left click on it (bar included) gives it focus, without sound.
        let over = self.over_info(gui, m);
        if let Ed::TestInstance { info_focus, .. } = &mut self.ed {
            if b == 0 && over {
                *info_focus = true;
                self.panel.focus = None;
                return (Vec::new(), false);
            }
            if b == 0 {
                *info_focus = false;
            }
        }
        let mut ev = Vec::new();
        self.panel.press(m, b, mods.shift_key(), &mut ev);
        self.handle(Some(gui), ev, mods.alt_key())
    }

    /// Mouse on the test instance's information area (frame and scroll bar).
    fn over_info(&self, gui: &Gui, m: (f32, f32)) -> bool {
        if !self.is_test_instance() {
            return false;
        }
        let left = (gui.width as i32 / 2 - 158) as f32;
        m.0 >= left && m.0 < left + (INFO_W + INFO_BAR) as f32 && m.1 >= INFO_Y as f32 && m.1 < (INFO_Y + INFO_H) as f32
    }

    /// Scrolling of the information area by `d` pixels (bounded); true if it changed.
    fn scroll_info(&mut self, gui: &Gui, d: f64) -> bool {
        let Ed::TestInstance { info, scroll, .. } = &mut self.ed else { return false };
        let max = info_max_scroll(info_lines(gui, info).len());
        let before = *scroll;
        *scroll = (*scroll + d).clamp(0.0, max);
        *scroll != before
    }

    pub fn wheel(&mut self, gui: &Gui, m: (f32, f32), dy: f64, mods: ModifiersState) -> Vec<OpAction> {
        self.layout(gui);
        if self.over_info(gui, m) {
            self.scroll_info(gui, -dy * INFO_STEP);
            return Vec::new();
        }
        let mut ev = Vec::new();
        self.panel.wheel(m, dy, &mut ev);
        self.handle(Some(gui), ev, mods.alt_key()).0
    }

    /// Key: (actions, command field modified).
    pub fn key(
        &mut self,
        gui: Option<&Gui>,
        k: KeyCode,
        mods: ModifiersState,
        text: Option<&str>,
    ) -> (Vec<OpAction>, bool) {
        if k == KeyCode::Escape {
            let mut out = Vec::new();
            self.cancel(&mut out);
            return (out, false);
        }
        if let Some(g) = gui
            && self.info_key(g, k, mods)
        {
            return (Vec::new(), false);
        }
        let focus = self.focus_id();
        let on_field = focus.is_some_and(|f| (1..=10).contains(&f));
        let confirm = matches!(k, KeyCode::Enter | KeyCode::NumpadEnter) && (focus.is_none() || on_field);
        let mut ev = Vec::new();
        if !confirm {
            self.panel.key(k, mods, &mut ev);
        }
        // Ctrl + letter types nothing (winit nevertheless returns the letter without Ctrl); AltGr (Ctrl + Alt)
        // types its character, as in the game (`widgets::typed_text`).
        if on_field
            && !(mods.control_key() && !mods.alt_key())
            && let Some(s) = text
        {
            if focus == Some(1) && matches!(self.ed, Ed::Structure { .. }) {
                // Structure name: id characters only.
                for ch in s.chars() {
                    let ok = self.text_field(1).is_some_and(|t| name_char_ok(&t.text, ch, t.cursor));
                    if ok {
                        self.panel.typed(&ch.to_string(), &mut ev);
                    }
                }
            } else {
                self.panel.typed(s, &mut ev);
            }
        }
        let (mut out, edited) = self.handle(gui, ev, mods.alt_key());
        if confirm {
            match &self.ed {
                Ed::Command { .. } | Ed::Structure { .. } => self.done(&mut out),
                Ed::Jigsaw { .. } if self.panel.get(DONE).is_some_and(|w| w.active) => self.done(&mut out),
                _ => {}
            }
        }
        (out, edited)
    }

    /// Keyboard of the information area (test instance): place in the tab order
    /// between the id (1) and the X size (2); up and down arrows: scroll by 9 pixels
    /// when it has focus (otherwise, as elsewhere, the arrow moves the focus).
    fn info_key(&mut self, gui: &Gui, k: KeyCode, mods: ModifiersState) -> bool {
        let Ed::TestInstance { info_focus, .. } = self.ed else { return false };
        let back = mods.shift_key();
        match k {
            KeyCode::Tab if info_focus => {
                self.set_info_focus(false);
                self.panel.focus_widget(if back { 1 } else { 2 });
                true
            }
            KeyCode::Tab if self.focus_id() == Some(if back { 2 } else { 1 }) => {
                self.set_info_focus(true);
                self.panel.focus = None;
                true
            }
            KeyCode::ArrowUp | KeyCode::ArrowDown if info_focus => {
                let d = if k == KeyCode::ArrowUp { -INFO_STEP } else { INFO_STEP };
                if self.scroll_info(gui, d) {
                    return true;
                }
                self.set_info_focus(false);
                false
            }
            _ => false,
        }
    }

    fn set_info_focus(&mut self, on: bool) {
        if let Ed::TestInstance { info_focus, .. } = &mut self.ed {
            *info_focus = on;
        }
    }

    /// Command field → suggestion input (same text, same cursor).
    fn push_chat(&mut self) {
        let t = match self.panel.get(1).map(|w| &w.kind) {
            Some(W::Field(t)) => (t.text.clone(), t.cursor),
            // Panel not yet redone: text known, cursor at the end.
            _ => {
                let v: Vec<char> = self.values.first().map(|s| s.chars().collect()).unwrap_or_default();
                let n = v.len();
                (v, n)
            }
        };
        if let Some(c) = &mut self.chat
            && let Some(i) = &mut c.input
        {
            i.set(t.0, t.1);
        }
    }

    /// Suggestion input → field: text and cursor (suggestion applied), then style.
    fn pull_text(&mut self) {
        let Some((text, cursor)) =
            self.chat.as_ref().and_then(|c| c.input.as_ref()).map(|i| (i.text.clone(), i.cursor))
        else {
            return;
        };
        if let Some(t) = self.text_field(1) {
            t.text = text;
            t.cursor = cursor.min(t.text.len());
            t.anchor = t.cursor;
        }
        self.pull_chat();
    }

    /// Colours of the parse and ghost text of the chosen suggestion.
    fn pull_chat(&mut self) {
        let Some(c) = &self.chat else { return };
        let colors = c.format.clone();
        let ghost = c.ghost.clone();
        if let Some(t) = self.text_field(1) {
            t.colors = colors;
            t.ghost = ghost;
        }
    }

    pub fn draw(&mut self, gui: &mut Gui, q: &wgpu::Queue, mouse: Option<(f32, f32)>, time: f64) {
        self.layout(gui);
        gui.next_layer();
        crate::widgets::ingame_ui_background(gui);
        let c = gui.width as i32 / 2;
        let text = |gui: &mut Gui, key: &str, x: i32, y: i32| {
            let s = crate::widgets::chars(&tr(gui, key));
            gui.text(q, &s, x as f32, y as f32, GRAY, true, 0);
        };
        let right = |gui: &mut Gui, key: &str, y: i32| {
            let s = crate::widgets::chars(&tr(gui, key));
            let w = rc_text::width(gui, &s);
            gui.text(q, &s, (c + 154 - w) as f32, y as f32, GRAY, true, 0);
        };
        let title = |gui: &mut Gui, key: &str, y: i32| {
            let t = crate::widgets::chars(&tr(gui, key));
            crate::widgets::centered(gui, q, &t, c, y, 0xffff_ffff);
        };
        match &self.ed {
            Ed::Command { minecart, .. } => {
                title(gui, "advMode.setCommand", 20);
                text(gui, "advMode.command", c - 149, 40);
                let prev_y = if minecart.is_some() { 150 } else { 135 };
                if !self.value(2).is_empty() {
                    text(gui, "advMode.previousOutput", c - 149, prev_y - 10);
                }
            }
            Ed::Structure { s, .. } => {
                title(gui, "block.minecraft.structure_block", 10);
                if s.mode != 3 {
                    text(gui, "structure_block.structure_name", c - 153, 30);
                }
                if s.mode <= 1 {
                    text(gui, "structure_block.position", c - 153, 70);
                    right(gui, "structure_block.include_entities", 150);
                }
                if s.mode == 0 {
                    text(gui, "structure_block.size", c - 153, 110);
                    right(gui, "structure_block.detect_size", 110);
                    right(gui, "structure_block.show_air", 70);
                }
                if s.mode == 1 {
                    text(gui, "structure_block.integrity", c - 153, 110);
                    right(gui, "structure_block.strict", 110);
                    right(gui, "structure_block.show_boundingbox", 70);
                }
                if s.mode == 3 {
                    text(gui, "structure_block.custom_data", c - 153, 110);
                }
                let k = ["save", "load", "corner", "data"][s.mode.clamp(0, 3) as usize];
                text(gui, &format!("structure_block.mode_info.{k}"), c - 153, 174);
            }
            Ed::Jigsaw { vertical, .. } => {
                text(gui, "jigsaw_block.pool", c - 153, 10);
                text(gui, "jigsaw_block.name", c - 153, 45);
                text(gui, "jigsaw_block.target", c - 153, 80);
                text(gui, "jigsaw_block.final_state", c - 153, 115);
                text(gui, "jigsaw_block.selection_priority", c - 153, 150);
                text(gui, "jigsaw_block.placement_priority", c - 50, 150);
                if *vertical {
                    text(gui, "jigsaw_block.joint_label", c + 53, 150);
                }
            }
            Ed::Test { mode } => {
                title(gui, "block.minecraft.test_block", 10);
                if *mode != 0 {
                    text(gui, "test_block.message", c - 153, 70);
                }
                let k = ["start", "log", "fail", "accept"][(*mode).clamp(0, 3) as usize];
                text(gui, &format!("test_block.mode_info.{k}"), c - 153, 174);
            }
            Ed::TestInstance { info, scroll, info_focus, .. } => {
                let left = c - 158;
                title(gui, "block.minecraft.test_instance_block", 10);
                text(gui, "test_instance_block.test_id", left, 30);
                text(gui, "test_instance_block.size", left, 150);
                let (rx, ex) = (self.panel.get(TROT).map_or(0, |w| w.x), self.panel.get(TENT).map_or(0, |w| w.x));
                text(gui, "test_instance_block.rotation", rx, 150);
                text(gui, "test_instance_block.entities", ex, 150);
                // Information area: frame (highlighted with focus), wrapped text cut to the
                // frame minus one pixel, shifted by the scroll, bar on the right if the text overflows.
                let (x, y) = (left as f32, INFO_Y as f32);
                let (w, h) = (INFO_W as f32, INFO_H as f32);
                let frame = if *info_focus { "widget/text_field_highlighted" } else { "widget/text_field" };
                gui.sprite(frame, x, y, w, h, 0xffff_ffff);
                let lines = info_lines(gui, info);
                let old = gui.clip();
                gui.set_clip(Some([x + 1.0, y + 1.0, x + w - 1.0, y + h - 1.0]));
                let top = (INFO_Y + INFO_PAD) as f32 - *scroll as f32;
                for (i, line) in lines.iter().enumerate() {
                    let ly = top + 9.0 * i as f32;
                    if ly + 9.0 > y && ly < y + h {
                        gui.text(q, line, (left + INFO_PAD) as f32, ly, 0xffff_ffff, true, 0);
                    }
                }
                gui.set_clip(old);
                let max = info_max_scroll(lines.len());
                if max > 0.0 {
                    let content = lines.len() as i32 * 9 + 2 * INFO_PAD;
                    let bar = ((INFO_H * INFO_H) as f32 / content as f32) as i32;
                    let bar = bar.clamp(32, INFO_H - 8);
                    let by = ((*scroll as i32) * (INFO_H - bar) / max as i32 + INFO_Y).max(INFO_Y);
                    let bx = x + w;
                    gui.sprite("widget/scroller_background", bx, y, INFO_BAR as f32, h, 0xffff_ffff);
                    gui.sprite("widget/scroller", bx, by as f32, INFO_BAR as f32, bar as f32, 0xffff_ffff);
                }
            }
        }
        self.panel.draw(gui, q, mouse, time);
    }

    /// Screen abscissas of the command field positions (field scroll applied).
    fn command_anchor(&self, gui: &Gui) -> Option<crate::commands::FieldAnchor> {
        let w = self.panel.get(1)?;
        let W::Field(t) = &w.kind else { return None };
        let all = crate::widgets::chars(&t.value());
        let inner = w.w - 8;
        let width_of = |a: usize, b: usize| rc_text::width(gui, &all[a.min(all.len())..b.min(all.len())]);
        let mut offset = t.offset.min(t.cursor);
        while offset < t.cursor && width_of(offset, t.cursor) > inner {
            offset += 1;
        }
        let tx = (w.x + 4) as f32;
        let xs = (0..=all.len()).map(|i| if i <= offset { tx } else { tx + width_of(offset, i) as f32 }).collect();
        Some(crate::commands::FieldAnchor { xs, inner: inner as f32, top: 72.0, lines: 7, fill: 0x8000_0000 })
    }
}

/// Message of the information area on receiving a status: "failed" followed by the error
/// (red by default, its own colours kept) and an empty line if the block carries one,
/// then the status.
fn status_message(error: Option<Component>, status: Option<Component>) -> Component {
    let mut msg = Component::text("");
    if let Some(e) = error {
        let mut red = Component::text("");
        red.style.color = Some(0xFF5555);
        red.extra.push(e);
        msg.extra.push(Component::translate("test_instance.description.failed", vec![rc_text::Arg::Component(red)]));
        msg.extra.push(Component::text("\n\n"));
    }
    msg.extra.extend(status);
    msg
}

fn invalid_id() -> Vec<StyledChar> {
    let mut c = Component::translate("test_instance.description.invalid_id", Vec::new());
    c.style.color = Some(0xFF5555);
    rc_text::styled_chars(&rc_text::flatten(&c, &rc_text::Lang::default()))
}

/// Command suggestions on top of the editor (after its drawing).
pub fn draw_suggestions(
    special: &mut crate::special::Special,
    cmd: &mut crate::commands::CommandUi,
    gui: &mut Gui,
    q: &wgpu::Queue,
    mouse: Option<(f32, f32)>,
) {
    let Some(crate::special::Overlay::Op(op)) = &mut special.overlay else { return };
    let Some(anchor) = op.command_anchor(gui) else { return };
    cmd.field = Some(anchor);
    if let Some(chat) = &mut op.chat {
        cmd.draw(gui, q, chat, mouse);
    }
    op.pull_chat();
}

/// Plain text of a JSON/NBT output component (`LastOutput`).
fn plain(n: Option<&Nbt>, lang: &rc_text::Lang) -> String {
    let Some(n) = n else { return String::new() };
    let c = match n {
        Nbt::String(s) if s.starts_with('{') || s.starts_with('"') => serde_json::from_str::<serde_json::Value>(s)
            .ok()
            .and_then(|v| v.get("text").and_then(|t| t.as_str()).map(rc_text::Component::text)),
        other => rc_text::from_nbt(other),
    };
    c.map(|c| rc_text::plain_string(&c, lang)).unwrap_or_default()
}

impl crate::App {
    fn op_allowed(&self) -> bool {
        self.env.game_mode == 1 && self.debug.op_level >= 2
    }

    fn op_open(&mut self, s: OpScreen) {
        let command = s.chat.is_some();
        self.open_overlay(crate::special::Overlay::Op(Box::new(s)));
        if command {
            self.commands_refresh();
            self.cmd.allowed = (true, true);
            self.cmd.field =
                Some(crate::commands::FieldAnchor { lines: 7, top: 72.0, fill: 0x8000_0000, ..Default::default() });
            if let Some(crate::special::Overlay::Op(op)) = &mut self.special.overlay
                && let Some(chat) = &mut op.chat
            {
                self.cmd.open(chat);
            }
            self.op_flush();
        }
    }

    /// Operator block used (creative, permission ≥ 2): screen opened locally; true if it
    /// was one.
    pub(crate) fn op_use_block(&mut self, pos: [i32; 3], state: u32) -> bool {
        if !self.op_allowed() {
            return false;
        }
        let Some(b) = self.data.block_of(state) else { return false };
        let name = b.name.strip_prefix("minecraft:").unwrap_or(&b.name).to_owned();
        let prop = |k: &str| self.data.state_properties(state).find(|(p, _)| *p == k).map(|(_, v)| v.to_owned());
        let (ed, values) = match name.as_str() {
            "command_block" | "chain_command_block" | "repeating_command_block" => {
                let known = self.special.op_known.get(&pos).cloned();
                let track = known.as_ref().is_none_or(|k| k.3);
                let output = known.map(|k| k.1).unwrap_or_default();
                (
                    Ed::Command {
                        minecart: None,
                        mode: 2,
                        conditional: false,
                        auto: false,
                        track,
                        ready: false,
                        output,
                    },
                    Vec::new(),
                )
            }
            "structure_block" => {
                let mode = match prop("mode").as_deref() {
                    Some("save") => 0,
                    Some("load") => 1,
                    Some("corner") => 2,
                    _ => 3,
                };
                let s = self.tech.structure(pos, mode);
                (Ed::Structure { s: s.clone(), initial: s }, Vec::new())
            }
            "jigsaw" => {
                let orientation = prop("orientation").unwrap_or_default();
                let vertical = orientation.starts_with("up") || orientation.starts_with("down");
                let j = match self.tech.map.get(&pos) {
                    Some(Tech::Jigsaw(j)) => j.clone(),
                    _ => Jigsaw::default(),
                };
                let values = vec![
                    j.pool.clone(),
                    j.name.clone(),
                    j.target.clone(),
                    j.final_state.clone(),
                    j.selection_priority.to_string(),
                    j.placement_priority.to_string(),
                ];
                (
                    Ed::Jigsaw {
                        vertical,
                        aligned: j.aligned.unwrap_or(!vertical),
                        levels: 0,
                        slider: 0.0,
                        keep: true,
                    },
                    values,
                )
            }
            "test_block" => {
                let (mode, message) = match self.tech.map.get(&pos) {
                    Some(Tech::Test(m, s)) => (*m, s.clone()),
                    _ => {
                        let m = match prop("mode").as_deref() {
                            Some("start") => 0,
                            Some("log") => 1,
                            Some("accept") => 3,
                            _ => 2,
                        };
                        (m, String::new())
                    }
                };
                (Ed::Test { mode }, vec![message])
            }
            "test_instance_block" => {
                let t = match self.tech.map.get(&pos) {
                    Some(Tech::TestInstance(t)) => t.clone(),
                    _ => TestInstance::default(),
                };
                let values = vec![
                    t.test.clone().unwrap_or_default(),
                    t.size[0].to_string(),
                    t.size[1].to_string(),
                    t.size[2].to_string(),
                ];
                (
                    Ed::TestInstance {
                        rotation: t.combined_rotation(self.tech.tests.get(t.test.as_deref())),
                        entities: !t.ignore_entities,
                        info: Vec::new(),
                        scroll: 0.0,
                        info_focus: false,
                    },
                    values,
                )
            }
            _ => return false,
        };
        self.op_open(OpScreen::new(pos, ed, values));
        true
    }

    /// Command block minecart used: screen (command and output of the entity data).
    pub(crate) fn op_use_minecart(&mut self, id: i32) -> bool {
        if !self.op_allowed() {
            return false;
        }
        let Some(e) = self.entities.map.get(&id) else { return false };
        use rc_proto_v777::entity::Meta;
        let command = match e.meta.get(&13) {
            Some(Meta::String(s)) => s.clone(),
            _ => String::new(),
        };
        let lang = self.renderer.as_ref().map(|r| r.gui.lang.clone()).unwrap_or_default();
        let output = match e.meta.get(&14) {
            Some(Meta::Text(n)) => plain(n.as_ref(), &lang),
            _ => String::new(),
        };
        let ed = Ed::Command {
            minecart: Some(id),
            mode: 2,
            conditional: false,
            auto: false,
            track: true,
            ready: true,
            output,
        };
        self.op_open(OpScreen::new([0, 0, 0], ed, vec![command]));
        true
    }

    /// `block_entity_data` of a command block: remembered, and the open screen is refreshed.
    pub(crate) fn op_block_data(&mut self, pos: [i32; 3], nbt: Option<&Nbt>) {
        let Some(n) = nbt else { return };
        if rc_data::env_flag!("RUSTCRAFT_OPLOG") {
            eprintln!("block entity {pos:?}: {n:?}");
        }
        let Some(cmd) = n.get("Command").and_then(Nbt::as_str) else { return };
        let lang = self.renderer.as_ref().map(|r| r.gui.lang.clone()).unwrap_or_default();
        let output = plain(n.get("LastOutput"), &lang);
        let flag = |k: &str| n.get(k).and_then(Nbt::as_i64).is_some_and(|v| v != 0);
        let entry = (cmd.to_owned(), output.clone(), flag("auto"), flag("TrackOutput"));
        self.special.op_known.insert(pos, entry.clone());
        // Mode and condition: from the block placed.
        let view =
            crate::environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
        let state = view.state(pos[0], pos[1], pos[2]);
        let name = self.data.block_of(state).map(|b| b.name.clone()).unwrap_or_default();
        let mode = match name.as_str() {
            "minecraft:chain_command_block" => 0,
            "minecraft:repeating_command_block" => 1,
            _ => 2,
        };
        let cond = self.data.state_properties(state).any(|(k, v)| k == "conditional" && v == "true");
        let mut refreshed = false;
        if let Some(crate::special::Overlay::Op(o)) = &mut self.special.overlay
            && o.pos == pos
            && o.is_command_block()
        {
            o.refresh(entry.0, entry.1, entry.3, entry.2, mode, cond);
            refreshed = true;
        }
        if refreshed {
            self.op_edited();
        }
    }

    pub(crate) fn op_action(&mut self, actions: Vec<OpAction>) {
        for a in actions {
            match a {
                OpAction::Close => {
                    if let Some(crate::special::Overlay::Op(op)) = &mut self.special.overlay
                        && let Some(chat) = &mut op.chat
                    {
                        self.cmd.close(chat);
                        self.cmd.field = None;
                    }
                    self.close_overlay();
                }
                OpAction::Click => self.play_ui(crate::widgets::CLICK_SOUND, 1.0),
                OpAction::Live(pos, s) => {
                    self.tech.map.insert(pos, Tech::Structure(*s));
                }
                OpAction::Send(p) => self.op_send(p),
            }
        }
    }

    fn op_send(&mut self, p: OpPacket) {
        let link = &self.session.link;
        use rc_proto_v777::special as sp;
        let _ = match p {
            OpPacket::CommandBlock { pos, command, mode, flags } => link
                .send_raw(link.ids.set_command_block, |w| sp::write_set_command_block(w, pos, &command, mode, flags)),
            OpPacket::Minecart { entity, command, track } => link.send_raw(link.ids.set_command_minecart, |w| {
                sp::write_set_command_minecart(w, entity, &command, track)
            }),
            OpPacket::Structure { pos, s } => {
                link.send_raw(link.ids.set_structure_block, |w| sp::write_set_structure_block(w, pos, &s))
            }
            OpPacket::Jigsaw { pos, fields: f, selection, placement } => link
                .send_raw(link.ids.set_jigsaw_block, |w| {
                    sp::write_set_jigsaw_block(w, pos, &f[0], &f[1], &f[2], &f[3], &f[4], selection, placement)
                }),
            OpPacket::JigsawGenerate { pos, levels, keep } => {
                link.send_raw(link.ids.jigsaw_generate, |w| sp::write_jigsaw_generate(w, pos, levels, keep))
            }
            OpPacket::TestBlock { pos, mode, message } => {
                link.send_raw(link.ids.set_test_block, |w| sp::write_set_test_block(w, pos, mode, &message))
            }
            OpPacket::TestInstance { pos, action, test, size, rotation, ignore } => link
                .send_raw(link.ids.test_instance_block_action, |w| {
                    sp::write_test_instance_action(w, pos, action, test.as_deref(), size, rotation, ignore)
                }),
        };
    }

    /// Validity of the open editor: command block still there, minecart still present;
    /// first state request of the test instance.
    pub(crate) fn op_tick(&mut self) {
        let Some(crate::special::Overlay::Op(op)) = &mut self.special.overlay else { return };
        if op.is_test_instance() && !op.init_sent {
            op.init_sent = true;
            let a = op.test_instance(0);
            let invalid = identifier(&op.value(1)).is_none();
            if invalid && let Ed::TestInstance { info, .. } = &mut op.ed {
                *info = invalid_id();
            }
            self.op_action(vec![a]);
            return;
        }
        let valid = match (op.minecart(), op.is_command_block()) {
            (Some(id), _) => self.entities.map.contains_key(&id),
            (None, true) => {
                let p = op.pos;
                let view = crate::environ::WorldView {
                    grid: &self.world.grid,
                    min_y: self.world.min_y,
                    height: self.world.height,
                };
                self.data.block_of(view.state(p[0], p[1], p[2])).is_some_and(|b| b.name.ends_with("command_block"))
            }
            _ => true,
        };
        if !valid {
            self.op_action(vec![OpAction::Close]);
        }
    }

    /// Keyboard of the open editor: suggestion list first (command block).
    pub(crate) fn op_key(&mut self, op: &mut OpScreen, k: KeyCode, text: Option<&str>) -> Vec<OpAction> {
        let mods = self.mods;
        if op.focus_id() == Some(1)
            && self.cmd.has_list()
            && let Some(chat) = &mut op.chat
        {
            use crate::commands::Key;
            let key = match k {
                KeyCode::ArrowUp => Some(Key::Up),
                KeyCode::ArrowDown => Some(Key::Down),
                KeyCode::Tab => Some(Key::Tab { shift: mods.shift_key() }),
                KeyCode::Escape => Some(Key::Escape),
                _ => None,
            };
            if let Some(key) = key {
                self.cmd.key(chat, key);
                op.pull_text();
                self.op_flush();
                return Vec::new();
            }
        }
        let gui = self.renderer.as_ref().map(|r| &r.gui);
        let (acts, edited) = op.key(gui, k, mods, text);
        if op.chat.is_some() {
            op.push_chat();
            if edited && let Some(chat) = &mut op.chat {
                self.cmd.text_changed(chat);
            }
            op.pull_chat();
            self.op_flush();
        }
        acts
    }

    pub(crate) fn op_press(&mut self, op: &mut OpScreen, m: (f32, f32), b: u8) -> Vec<OpAction> {
        if b == 0
            && let Some(chat) = &mut op.chat
            && self.cmd.click(chat, m)
        {
            op.pull_text();
            self.op_flush();
            return Vec::new();
        }
        let Some(gui) = self.renderer.as_ref().map(|r| &r.gui) else { return Vec::new() };
        let (acts, edited) = op.press(gui, m, b, self.mods);
        if op.chat.is_some() {
            op.push_chat();
            if edited && let Some(chat) = &mut op.chat {
                self.cmd.text_changed(chat);
            }
            op.pull_chat();
        }
        acts
    }

    pub(crate) fn op_wheel(&mut self, op: &mut OpScreen, m: (f32, f32), dy: f64) -> Vec<OpAction> {
        if op.chat.is_some() && self.cmd.wheel(m, dy as f32) {
            return Vec::new();
        }
        let Some(gui) = self.renderer.as_ref().map(|r| &r.gui) else { return Vec::new() };
        op.wheel(gui, m, dy, self.mods)
    }

    /// Text of the command field replaced (data received): parse restarted.
    fn op_edited(&mut self) {
        if let Some(crate::special::Overlay::Op(op)) = &mut self.special.overlay
            && op.chat.is_some()
        {
            op.push_chat();
            if let Some(chat) = &mut op.chat {
                self.cmd.update(chat);
            }
            op.pull_chat();
        }
        self.op_flush();
    }

    /// Request for suggestions to the server (field text up to the cursor).
    fn op_flush(&mut self) {
        if let Some((id, text)) = self.cmd.outgoing.take() {
            let _ = self.session.link.send_command_suggestion(id, &text);
        }
    }

    /// Suggestion reply: to the command editor if it is open, otherwise to the chat.
    pub(crate) fn op_suggestions(&mut self, s: rc_proto_v777::commands::ServerSuggestions) {
        if let Some(crate::special::Overlay::Op(op)) = &mut self.special.overlay
            && let Some(chat) = &mut op.chat
        {
            self.cmd.server_suggestions(chat, s);
            op.pull_chat();
            return;
        }
        self.cmd.server_suggestions(&mut self.chat, s);
    }

    /// `test_instance_block_status`: description text and size of the open screen.
    pub(crate) fn op_test_status(&mut self, status: Option<Component>, size: Option<[i32; 3]>) {
        let Some(crate::special::Overlay::Op(op)) = &mut self.special.overlay else { return };
        if !op.is_test_instance() {
            return;
        }
        let lang = self.renderer.as_ref().map(|r| r.gui.lang.clone()).unwrap_or_default();
        // Error of the block at the time of the status (not at opening), then the status.
        let error = self.tech.map.get(&op.pos).and_then(|t| match t {
            Tech::TestInstance(i) => i.error.clone(),
            _ => None,
        });
        if let Ed::TestInstance { info, .. } = &mut op.ed {
            *info = rc_text::styled_chars(&rc_text::flatten(&status_message(error, status), &lang));
        }
        if let Some(s) = size {
            for (i, v) in s.iter().enumerate() {
                op.set_value(2 + i as u32, &v.to_string());
            }
        }
    }
}

/// Action button of the structure block: (mode required, action sent). Packet actions:
/// 0 update, 1 save the area, 2 load the area, 3 detect the size; modes: 0 save,
/// 1 load.
const fn structure_button(id: u32) -> (i32, i32) {
    match id {
        SAVE => (0, 1),
        LOAD => (1, 2),
        _ => (0, 3),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structure_buttons_send_their_own_action() {
        assert_eq!(structure_button(SAVE), (0, 1));
        assert_eq!(structure_button(LOAD), (1, 2));
        assert_eq!(structure_button(DETECT), (0, 3));
    }

    /// Ctrl+A, Ctrl+C, Ctrl+V in the command field: shortcuts only, no letter typed
    /// (winit returns "a", "c", "v" without Ctrl); AltGr (Ctrl + Alt) types its character.
    #[test]
    fn ctrl_shortcuts_leave_the_command_intact() {
        let ed = Ed::Command {
            minecart: Some(1),
            mode: 2,
            conditional: false,
            auto: false,
            track: true,
            ready: true,
            output: String::new(),
        };
        let mut s = OpScreen::new([0, 0, 0], ed, vec!["say hi".into()]);
        s.panel = Panel::new(vec![field(1, 0, 0, 300, "say hi", 32500)]);
        s.panel.focus_widget(1);
        let (ctrl, alt) = (ModifiersState::CONTROL, ModifiersState::ALT);
        s.key(None, KeyCode::KeyA, ctrl, Some("a"));
        s.key(None, KeyCode::KeyC, ctrl, Some("c"));
        assert_eq!(s.value(1), "say hi");
        s.key(None, KeyCode::End, ModifiersState::empty(), None);
        crate::widgets::set_clipboard("!".into());
        s.key(None, KeyCode::KeyV, ctrl, Some("v"));
        assert_eq!(s.value(1), "say hi!");
        s.key(None, KeyCode::Digit0, ctrl | alt, Some("@"));
        assert_eq!(s.value(1), "say hi!@");
    }

    #[test]
    fn identifiers() {
        assert_eq!(identifier("empty").as_deref(), Some("minecraft:empty"));
        assert_eq!(identifier(":x").as_deref(), Some("minecraft:x"));
        assert_eq!(identifier("a:b/c").as_deref(), Some("a:b/c"));
        assert_eq!(identifier("A"), None);
        assert_eq!(identifier("a:b:c"), None);
    }

    #[test]
    fn structure_name_chars() {
        let t: Vec<char> = "ab".chars().collect();
        assert!(name_char_ok(&t, ':', 2));
        assert!(!name_char_ok(&t, 'A', 2));
        let t: Vec<char> = "a:b".chars().collect();
        assert!(!name_char_ok(&t, ':', 3));
        assert!(name_char_ok(&t, '/', 3));
        assert!(!name_char_ok(&t, '/', 1));
    }

    #[test]
    fn status_message_puts_current_error_before_status() {
        let mut lang = rc_text::Lang::default();
        lang.map.insert("test_instance.description.failed".into(), "Failed: %s".into());
        let flat = |c: &Component| rc_text::flatten(c, &lang);
        let status = Some(Component::text("Ready"));
        assert_eq!(rc_text::plain_text(&flat(&status_message(None, status.clone()))), "Ready");
        let mut err = Component::text("Boom");
        err.style.color = Some(0x55FF55);
        let segs = flat(&status_message(Some(err), status));
        let text = rc_text::plain_text(&segs);
        assert!(text.ends_with("\n\nReady"), "{text:?}");
        // The error's own colour wins over the red that wraps it.
        let boom = segs.iter().find(|s| s.text.contains("Boom")).expect("error present");
        assert_eq!(boom.style.color, Some(0x55FF55));
        let mut plain = Component::text("Plain");
        plain.style.color = None;
        let segs = flat(&status_message(Some(plain), None));
        let p = segs.iter().find(|s| s.text.contains("Plain")).expect("error present");
        assert_eq!(p.style.color, Some(0xFF5555));
    }

    #[test]
    fn info_scroll_bounds() {
        // 7 lines fit (7 × 9 + 8 = 71 ≤ 72); the eighth scrolls by 8 pixels.
        assert_eq!(info_max_scroll(7), 0.0);
        assert_eq!(info_max_scroll(8), 8.0);
    }

    #[test]
    fn integrity_format() {
        assert_eq!(integrity_text(1.0), "1.0");
        assert_eq!(integrity_text(0.5), "0.5");
        assert_eq!(integrity_text(0.123_45), "0.1235");
        assert_eq!(parse_integrity(" 0.5f "), 0.5);
        assert_eq!(parse_integrity("x"), 1.0);
    }
}
