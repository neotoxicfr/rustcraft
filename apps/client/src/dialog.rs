//! Server dialogs (`show_dialog`, `clear_dialog`, `protocol-supplements.md` § 12):
//! header (title and warning button), scrolling body (clickable messages, items,
//! editable inputs, button grid of the lists), footer (actions, or exit of the lists),
//! response-waiting screen and warning disconnect screen.
//!
//! Known: the components of a body item are used only for its tooltip (name, description),
//! like the chat's `show_item` hover; tab order skips the multi-line inputs
//! (mouse focus).

use std::sync::Arc;
use std::time::Instant;

use rc_proto::nbt::Nbt;
use rc_proto_v777::item::ItemStack;
use rc_text::{Arg, ClickEvent, Component, HoverEvent, StyledChar};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::keyboard::{KeyCode, ModifiersState};

use crate::App;
use crate::book::Area;
use crate::gui::Gui;
use crate::hud::HotbarItems;
use crate::widgets::{Event, Kind, List, Panel, TextField, Widget};

fn get<'a>(n: &'a Nbt, key: &str) -> Option<&'a Nbt> {
    match n {
        Nbt::Compound(v) => v.iter().find(|(k, _)| k == key).map(|(_, v)| v),
        _ => None,
    }
}

fn string(n: Option<&Nbt>) -> Option<String> {
    match n? {
        Nbt::String(s) => Some(s.clone()),
        _ => None,
    }
}

fn number(n: Option<&Nbt>) -> Option<f64> {
    Some(match n? {
        Nbt::Byte(v) => f64::from(*v),
        Nbt::Short(v) => f64::from(*v),
        Nbt::Int(v) => f64::from(*v),
        Nbt::Long(v) => *v as f64,
        Nbt::Float(v) => f64::from(*v),
        Nbt::Double(v) => *v,
        _ => return None,
    })
}

fn boolean(n: Option<&Nbt>, default: bool) -> bool {
    number(n).map_or(default, |v| v != 0.0)
}

fn text(n: Option<&Nbt>) -> Option<Component> {
    n.and_then(rc_text::from_nbt)
}

/// Bounded optional integer (`default` if missing); out of bounds: dialog rejected, like the game.
fn int_in(n: &Nbt, key: &str, default: i32, min: i32, max: i32) -> Option<i32> {
    match number(get(n, key)) {
        None => Some(default),
        Some(v) => Some(v as i32).filter(|v| (min..=max).contains(v)),
    }
}

/// Width of a button, message or input: 1..1024.
fn width(n: &Nbt, key: &str, default: i32) -> Option<i32> {
    int_in(n, key, default, 1, 1024)
}

/// Id without namespace by default.
fn short(id: &str) -> &str {
    id.strip_prefix("minecraft:").unwrap_or(id)
}

fn chars(gui: &Gui, c: &Component) -> Vec<StyledChar> {
    rc_text::styled_chars(&rc_text::flatten(c, &gui.lang))
}

/// JSON (files `data/*/dialog/*.json`) to NBT, for a single parser.
pub fn json_to_nbt(v: &serde_json::Value) -> Nbt {
    use serde_json::Value as V;
    match v {
        V::Null => Nbt::Compound(Vec::new()),
        V::Bool(b) => Nbt::Byte(i8::from(*b)),
        V::Number(n) => match n.as_i64() {
            Some(i) if i32::try_from(i).is_ok() => Nbt::Int(i as i32),
            _ => Nbt::Double(n.as_f64().unwrap_or(0.0)),
        },
        V::String(s) => Nbt::String(s.clone()),
        V::Array(a) => Nbt::List(a.iter().map(json_to_nbt).collect()),
        V::Object(o) => Nbt::Compound(o.iter().map(|(k, v)| (k.clone(), json_to_nbt(v))).collect()),
    }
}

/// Template variable name: letters, decimal digits and `_` (empty allowed). Like the game,
/// the test works on one UTF-16 unit at a time: any character outside the basic plane is
/// rejected, as are non-decimal numbers (½, Ⅻ). Known difference: combining marks
/// classed as alphabetic by Unicode (Indic vowels, circled letters) remain allowed.
fn variable_name_ok(s: &str) -> bool {
    s.chars().all(|c| c == '_' || (c.len_utf16() == 1 && ((c.is_alphabetic() && !c.is_numeric()) || decimal_digit(c))))
}

/// Unicode decimal digit (category Nd) of the basic plane: runs of ten starting at these values.
fn decimal_digit(c: char) -> bool {
    const STARTS: [u32; 37] = [
        0x30, 0x660, 0x6F0, 0x7C0, 0x966, 0x9E6, 0xA66, 0xAE6, 0xB66, 0xBE6, 0xC66, 0xCE6, 0xD66, 0xDE6, 0xE50, 0xED0,
        0xF20, 0x1040, 0x1090, 0x17E0, 0x1810, 0x1946, 0x19D0, 0x1A80, 0x1A90, 0x1B50, 0x1BB0, 0x1C40, 0x1C50, 0xA620,
        0xA8D0, 0xA900, 0xA9D0, 0xA9F0, 0xAA50, 0xABF0, 0xFF10,
    ];
    let c = c as u32;
    STARTS.iter().any(|&s| (s..s + 10).contains(&c))
}

/// Command template `$(key)`: alternating pieces of text and variables.
#[derive(Debug, Clone, PartialEq)]
pub struct Template {
    segments: Vec<String>,
    vars: Vec<String>,
}

impl Template {
    /// Read like the game: `$(` opens a variable closed by the next `)`; invalid name,
    /// unclosed variable or template without a variable: rejected.
    fn parse(s: &str) -> Option<Template> {
        let (mut segments, mut vars) = (Vec::new(), Vec::new());
        let (mut start, mut from) = (0, 0);
        while let Some(i) = s[from..].find('$').map(|i| i + from) {
            if s[i + 1..].starts_with('(') {
                segments.push(s[start..i].to_owned());
                let end = s[i + 1..].find(')')? + i + 1;
                let var = &s[i + 2..end];
                if !variable_name_ok(var) {
                    return None;
                }
                vars.push(var.to_owned());
                start = end + 1;
                from = start;
            } else {
                from = i + 1;
            }
        }
        if start == 0 {
            return None;
        }
        if start != s.len() {
            segments.push(s[start..].to_owned());
        }
        Some(Template { segments, vars })
    }

    /// Filled template; variable without input: empty text; duplicate key: the last input
    /// wins (the game stores the inputs by key).
    fn fill(&self, values: &[Value]) -> String {
        let mut out = String::new();
        for (seg, var) in self.segments.iter().zip(&self.vars) {
            out.push_str(seg);
            if let Some(v) = values.iter().rev().find(|v| v.key == *var) {
                out.push_str(&v.text);
            }
        }
        if self.segments.len() > self.vars.len()
            && let Some(last) = self.segments.last()
        {
            out.push_str(last);
        }
        out
    }
}

/// Text of an input in a template: `"`, `'` and `\` preceded by `\`, control characters
/// escaped as in SNBT (`\n`, `\t`… or `\xHH`).
pub fn escape_without_quotes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' | '\'' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if c < ' ' => out.push_str(&format!("\\x{:02X}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Number written like the game: integer without decimal, otherwise short form of a Java `float`
/// (scientific notation outside [10⁻³, 10⁷[).
fn number_text(v: f32) -> String {
    let i = v as i32;
    if i as f32 == v {
        return i.to_string();
    }
    let a = v.abs();
    if (1e-3..1e7).contains(&a) {
        return v.to_string();
    }
    let s = format!("{v:e}");
    let (m, e) = s.split_once('e').unwrap_or((&s, "0"));
    let m = if m.contains('.') { m.to_owned() } else { format!("{m}.0") };
    format!("{m}E{e}")
}

/// Value of an input: key, text for the templates, NBT for `dynamic/custom`.
#[derive(Debug, Clone, PartialEq)]
pub struct Value {
    pub key: String,
    pub text: String,
    pub nbt: Nbt,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Fixed click (link, command, clipboard, suggestion, page, dialog, custom
    /// action): same fields as a text click.
    Static(Arc<ClickEvent>),
    /// `dynamic/run_command`: command of the template filled by the inputs.
    Command(Template),
    /// `dynamic/custom`: `additions` completed with one entry per input.
    Custom { id: String, additions: Option<Nbt> },
}

impl Action {
    fn create(&self, values: &[Value]) -> ClickEvent {
        match self {
            Action::Static(e) => (**e).clone(),
            Action::Command(t) => ClickEvent { action: "run_command".into(), value: t.fill(values), nbt: None },
            Action::Custom { id, additions } => {
                let mut c = match additions {
                    Some(Nbt::Compound(v)) => v.clone(),
                    _ => Vec::new(),
                };
                for v in values {
                    match c.iter_mut().find(|(k, _)| *k == v.key) {
                        Some(e) => e.1 = v.nbt.clone(),
                        None => c.push((v.key.clone(), v.nbt.clone())),
                    }
                }
                ClickEvent { action: "custom".into(), value: id.clone(), nbt: Some(Nbt::Compound(c)) }
            }
        }
    }
}

fn action(n: &Nbt) -> Option<Action> {
    let kind = string(get(n, "type"))?;
    Some(match short(&kind) {
        "dynamic/run_command" => Action::Command(Template::parse(&string(get(n, "template"))?)?),
        "dynamic/custom" => Action::Custom {
            id: string(get(n, "id"))?,
            additions: get(n, "additions").filter(|a| matches!(a, Nbt::Compound(_))).cloned(),
        },
        k => {
            let Nbt::Compound(v) = n else { return None };
            let mut c: Vec<(String, Nbt)> = v.iter().filter(|(key, _)| key != "type").cloned().collect();
            c.push(("action".into(), Nbt::String(k.to_owned())));
            Action::Static(rc_text::click_event(&Nbt::Compound(c))?)
        }
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct Button {
    pub label: Component,
    pub tooltip: Option<Component>,
    pub width: i32,
    pub action: Option<Action>,
}

/// Button; action present but unreadable: button (and dialog) rejected.
fn button(n: &Nbt) -> Option<Button> {
    let action = match get(n, "action") {
        Some(a) => Some(action(a)?),
        None => None,
    };
    Some(Button { label: text(get(n, "label"))?, tooltip: text(get(n, "tooltip")), width: width(n, "width", 150)?, action })
}

/// Body message: default width 200; a single component is allowed.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub contents: Component,
    pub width: i32,
}

fn message(n: &Nbt) -> Option<Message> {
    if get(n, "contents").is_some() {
        return Some(Message { contents: text(get(n, "contents"))?, width: width(n, "width", 200)? });
    }
    Some(Message { contents: rc_text::from_nbt(n)?, width: 200 })
}

#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    Text(Message),
    Item {
        id: String,
        count: i32,
        components: Option<Nbt>,
        description: Option<Message>,
        decorations: bool,
        tooltip: bool,
        width: i32,
        height: i32,
    },
}

fn body(b: &Nbt) -> Option<Body> {
    let kind = string(get(b, "type"))?;
    Some(match short(&kind) {
        "plain_message" => Body::Text(message(b)?),
        "item" => {
            let item = get(b, "item")?;
            let (id, count, components) = match item {
                Nbt::String(s) => (s.clone(), 1, None),
                c => (string(get(c, "id"))?, int_in(c, "count", 1, 1, 99)?, get(c, "components").cloned()),
            };
            let description = match get(b, "description") {
                Some(d) => Some(message(d)?),
                None => None,
            };
            Body::Item {
                id,
                count,
                components,
                description,
                decorations: boolean(get(b, "show_decorations"), true),
                tooltip: boolean(get(b, "show_tooltip"), true),
                width: int_in(b, "width", 16, 1, 256)?,
                height: int_in(b, "height", 16, 1, 256)?,
            }
        }
        _ => return None,
    })
}

/// Bounds, initial value and step of a numeric slider.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Range {
    pub start: f32,
    pub end: f32,
    pub initial: Option<f32>,
    pub step: Option<f32>,
}

impl Range {
    fn initial_value(&self) -> f32 {
        self.initial.unwrap_or((self.start + self.end) / 2.0)
    }

    fn slider_of(&self, v: f32) -> f32 {
        if self.start == self.end { 0.5 } else { (v - self.start) / (self.end - self.start) }
    }

    /// Initial position of the slider in [0, 1].
    pub fn initial_slider(&self) -> f32 {
        self.slider_of(self.initial_value())
    }

    /// Value for a slider position: interpolated, then brought to the step counted from the
    /// initial value (one step less if the rounding leaves the bounds).
    pub fn value(&self, slider: f32) -> f32 {
        let v = self.start + slider * (self.end - self.start);
        let Some(step) = self.step else { return v };
        let init = self.initial_value();
        let steps = ((v - init) / step + 0.5).floor() as i32;
        let r = init + steps as f32 * step;
        let p = self.slider_of(r);
        if (0.0..=1.0).contains(&p) { r } else { init + (steps - steps.signum()) as f32 * step }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Control {
    Text {
        width: i32,
        label: Component,
        label_visible: bool,
        initial: String,
        max_len: usize,
        /// Multi-line: lines at most, height.
        multiline: Option<(Option<usize>, Option<i32>)>,
    },
    Bool {
        label: Component,
        initial: bool,
        on_true: String,
        on_false: String,
    },
    Choice {
        width: i32,
        options: Vec<(String, Option<Component>)>,
        initial: usize,
        label: Component,
        label_visible: bool,
    },
    Number {
        width: i32,
        label: Component,
        format: String,
        range: Range,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Input {
    pub key: String,
    pub control: Control,
}

fn input(n: &Nbt) -> Option<Input> {
    let key = string(get(n, "key")).filter(|k| variable_name_ok(k))?;
    let kind = string(get(n, "type"))?;
    let control = match short(&kind) {
        "text" => {
            let initial = string(get(n, "initial")).unwrap_or_default();
            let max_len = usize::try_from(int_in(n, "max_length", 32, 1, i32::MAX)?).ok()?;
            if initial.encode_utf16().count() > max_len {
                return None;
            }
            let multiline = match get(n, "multiline") {
                Some(m) => {
                    let lines = match number(get(m, "max_lines")) {
                        Some(v) if v >= 1.0 => Some(v as usize),
                        Some(_) => return None,
                        None => None,
                    };
                    let height = match get(m, "height") {
                        Some(_) => Some(int_in(m, "height", 1, 1, 512)?),
                        None => None,
                    };
                    Some((lines, height))
                }
                None => None,
            };
            Control::Text {
                width: width(n, "width", 200)?,
                label: text(get(n, "label"))?,
                label_visible: boolean(get(n, "label_visible"), true),
                initial,
                max_len,
                multiline,
            }
        }
        "boolean" => Control::Bool {
            label: text(get(n, "label"))?,
            initial: boolean(get(n, "initial"), false),
            on_true: string(get(n, "on_true")).unwrap_or_else(|| "true".into()),
            on_false: string(get(n, "on_false")).unwrap_or_else(|| "false".into()),
        },
        "single_option" => {
            let Some(Nbt::List(l)) = get(n, "options") else { return None };
            let mut options = Vec::new();
            let mut initial = None;
            for (i, o) in l.iter().enumerate() {
                match o {
                    Nbt::String(id) => options.push((id.clone(), None)),
                    c => {
                        if boolean(get(c, "initial"), false) {
                            if initial.is_some() {
                                return None;
                            }
                            initial = Some(i);
                        }
                        options.push((string(get(c, "id"))?, text(get(c, "display"))));
                    }
                }
            }
            if options.is_empty() {
                return None;
            }
            Control::Choice {
                width: width(n, "width", 200)?,
                options,
                initial: initial.unwrap_or(0),
                label: text(get(n, "label"))?,
                label_visible: boolean(get(n, "label_visible"), true),
            }
        }
        "number_range" => {
            let f = |k: &str| number(get(n, k)).map(|v| v as f32);
            let range = Range { start: f("start")?, end: f("end")?, initial: f("initial"), step: f("step") };
            if range.step.is_some_and(|s| s <= 0.0) {
                return None;
            }
            if let Some(i) = range.initial {
                let (lo, hi) = (range.start.min(range.end), range.start.max(range.end));
                if i < lo || i > hi {
                    return None;
                }
            }
            Control::Number {
                width: width(n, "width", 200)?,
                label: text(get(n, "label"))?,
                format: string(get(n, "label_format")).unwrap_or_else(|| "options.generic_value".into()),
                range,
            }
        }
        _ => return None,
    };
    Some(Input { key, control })
}

impl Input {
    /// Value of the input in a given state (text, box, chosen option, slider).
    fn value(&self, state: &State) -> Value {
        let key = self.key.clone();
        match (&self.control, state) {
            (Control::Bool { on_true, on_false, .. }, State::Bool(b)) => {
                Value { key, text: if *b { on_true.clone() } else { on_false.clone() }, nbt: Nbt::Byte(i8::from(*b)) }
            }
            (Control::Choice { options, .. }, State::Choice(i)) => {
                let id = options.get(*i).map(|o| o.0.clone()).unwrap_or_default();
                Value { key, text: id.clone(), nbt: Nbt::String(id) }
            }
            (Control::Number { range, .. }, State::Number(s)) => {
                let v = range.value(*s);
                Value { key, text: number_text(v), nbt: Nbt::Float(v) }
            }
            (_, State::Text(s)) => Value { key, text: escape_without_quotes(s), nbt: Nbt::String(s.clone()) },
            _ => Value { key, text: String::new(), nbt: Nbt::String(String::new()) },
        }
    }

    fn initial(&self) -> State {
        match &self.control {
            Control::Text { initial, .. } => State::Text(initial.clone()),
            Control::Bool { initial, .. } => State::Bool(*initial),
            Control::Choice { initial, .. } => State::Choice(*initial),
            Control::Number { range, .. } => State::Number(range.initial_slider()),
        }
    }
}

/// Current state of an input.
#[derive(Debug, Clone, PartialEq)]
enum State {
    Text(String),
    Bool(bool),
    Choice(usize),
    Number(f32),
}

#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub enum After {
    Close,
    None,
    Wait,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Dialog {
    pub title: Component,
    pub external_title: Option<Component>,
    pub escape: bool,
    /// Singleplayer paused while the dialog is displayed.
    pub pause: bool,
    pub after: After,
    pub body: Vec<Body>,
    pub inputs: Vec<Input>,
    /// List dialog (multi_action, server_links, dialog_list): button grid in the
    /// body, in `columns` columns; the footer only carries the exit.
    pub list: bool,
    pub grid: Vec<Button>,
    pub columns: usize,
    /// Footer: main actions (notice, confirmation) or exit (lists).
    pub footer: Vec<Button>,
    /// Cancel action (Escape).
    pub cancel: Option<Action>,
}

impl Dialog {
    /// Dialog in the data file format; `links`: server links; `set`: dialogs
    /// of a set (`#tag`, id, content or list) of a `dialog_list`.
    pub fn parse(
        n: &Nbt,
        links: &[rc_client::extra::ServerLink],
        set: &dyn Fn(&Nbt) -> Vec<Nbt>,
    ) -> Option<Dialog> {
        let kind = string(get(n, "type"))?;
        let after = match string(get(n, "after_action")).as_deref().map(short) {
            None | Some("close") => After::Close,
            Some("none") => After::None,
            Some("wait_for_response") => After::Wait,
            Some(_) => return None,
        };
        let pause = boolean(get(n, "pause"), true);
        // A dialog that pauses must lift the pause after the action.
        if pause && after == After::None {
            return None;
        }
        let body = match get(n, "body") {
            Some(Nbt::List(l)) => l.iter().map(body).collect::<Option<Vec<_>>>()?,
            Some(single @ Nbt::Compound(_)) => vec![body(single)?],
            _ => Vec::new(),
        };
        let inputs = match get(n, "inputs") {
            Some(Nbt::List(l)) => l.iter().map(input).collect::<Option<Vec<_>>>()?,
            _ => Vec::new(),
        };
        let exit = match get(n, "exit_action") {
            Some(e) => Some(button(e)?),
            None => None,
        };
        let columns = usize::try_from(int_in(n, "columns", 2, 1, i32::MAX)?).ok()?;
        let (list, grid, footer, cancel) = match short(&kind) {
            "notice" => {
                let b = match get(n, "action") {
                    Some(a) => button(a)?,
                    None => {
                        Button { label: Component::translate("gui.ok", vec![]), tooltip: None, width: 150, action: None }
                    }
                };
                let cancel = b.action.clone();
                (false, Vec::new(), vec![b], cancel)
            }
            "confirmation" => {
                let (yes, no) = (button(get(n, "yes")?)?, button(get(n, "no")?)?);
                let cancel = no.action.clone();
                (false, Vec::new(), vec![yes, no], cancel)
            }
            "multi_action" => {
                let Some(Nbt::List(l)) = get(n, "actions") else { return None };
                let grid = l.iter().map(button).collect::<Option<Vec<_>>>()?;
                if grid.is_empty() {
                    return None;
                }
                (true, grid, Vec::new(), None)
            }
            "server_links" => {
                let width = width(n, "button_width", 150)?;
                let grid = links
                    .iter()
                    .map(|l| {
                        let label = match (&l.label, l.kind) {
                            (Some(c), _) => c.clone(),
                            (None, k) => Component::translate(
                                format!("known_server_link.{}", link_name(k.unwrap_or(0))),
                                vec![],
                            ),
                        };
                        let ev = ClickEvent { action: "open_url".into(), value: l.url.clone(), nbt: None };
                        Button { label, tooltip: None, width, action: Some(Action::Static(Arc::new(ev))) }
                    })
                    .collect();
                (true, grid, Vec::new(), None)
            }
            "dialog_list" => {
                let width = width(n, "button_width", 150)?;
                let grid = set(get(n, "dialogs")?)
                    .into_iter()
                    .filter_map(|d| {
                        let label = text(get(&d, "external_title")).or_else(|| text(get(&d, "title")))?;
                        let id = string(Some(&d)).unwrap_or_default();
                        let ev = ClickEvent { action: "show_dialog".into(), value: id, nbt: Some(d) };
                        Some(Button { label, tooltip: None, width, action: Some(Action::Static(Arc::new(ev))) })
                    })
                    .collect();
                (true, grid, Vec::new(), None)
            }
            _ => return None,
        };
        let (footer, cancel) = if list {
            let cancel = exit.as_ref().and_then(|b| b.action.clone());
            (exit.into_iter().collect(), cancel)
        } else {
            (footer, cancel)
        };
        Some(Dialog {
            title: text(get(n, "title"))?,
            external_title: text(get(n, "external_title")),
            escape: boolean(get(n, "can_close_with_escape"), true),
            pause,
            after,
            body,
            inputs,
            list,
            grid,
            columns,
            footer,
            cancel,
        })
    }

    /// Buttons in display order: grid then footer.
    fn buttons(&self) -> impl Iterator<Item = &Button> {
        self.grid.iter().chain(&self.footer)
    }

    /// Footer height: 33, 5 for a list without exit.
    fn footer_height(&self) -> i32 {
        if self.list && self.footer.is_empty() { 5 } else { 33 }
    }
}

/// Names of the known links (§ 11).
fn link_name(k: i32) -> &'static str {
    const NAMES: [&str; 10] = [
        "bug_report",
        "community_guidelines",
        "support",
        "status",
        "feedback",
        "community",
        "website",
        "forums",
        "news",
        "announcements",
    ];
    NAMES.get(k as usize).copied().unwrap_or("bug_report")
}

// ---------------------------------------------------------------------------------------------
// Layout (rules of the game's layouts: horizontal alignment truncated, vertical rounded).

/// Centred offset of an element of size `size` in `avail` (truncation towards zero).
fn center(avail: i32, size: i32) -> i32 {
    ((avail - size) as f32 * 0.5) as i32
}

/// Button grid (`columns` columns, spacing 2): the incomplete last row forms a
/// line centred over the whole width. Relative positions, width and height.
fn pack(widths: &[i32], columns: usize) -> (Vec<(i32, i32)>, i32, i32) {
    let n = widths.len();
    if n == 0 {
        return (Vec::new(), 0, 0);
    }
    let cols = columns.max(1);
    let full = n / cols * cols;
    let mut col_w = vec![0; cols];
    for (i, &w) in widths[..full].iter().enumerate() {
        col_w[i % cols] = col_w[i % cols].max(w);
    }
    let rest_w = (full < n).then(|| widths[full..].iter().sum::<i32>() + 2 * (n - full - 1) as i32);
    if let Some(rw) = rest_w {
        // Width of the line spread over the columns, remainder distributed along the columns.
        let (num, den) = (rw - 2 * (cols as i32 - 1), cols as i32);
        let (q, m) = (num / den, num % den);
        let mut acc = 0;
        for w in &mut col_w {
            acc += m;
            let mut part = q;
            if acc >= den {
                acc -= den;
                part += 1;
            }
            *w = (*w).max(part);
        }
    }
    let mut col_x = vec![0; cols];
    for c in 1..cols {
        col_x[c] = col_x[c - 1] + col_w[c - 1] + 2;
    }
    let total_w = col_x[cols - 1] + col_w[cols - 1];
    let rows = n.div_ceil(cols) as i32;
    let mut out = Vec::with_capacity(n);
    for (i, &w) in widths[..full].iter().enumerate() {
        let c = i % cols;
        out.push((col_x[c] + center(col_w[c], w), (i / cols) as i32 * 22));
    }
    if let Some(rw) = rest_w {
        let mut x = center(total_w, rw);
        let y = (full / cols) as i32 * 22;
        for &w in &widths[full..] {
            out.push((x, y));
            x += w + 2;
        }
    }
    (out, total_w, rows * 22 - 2)
}

/// Column of elements (spacing 10, centred): relative positions and size.
fn column(sizes: &[(i32, i32)]) -> (Vec<(i32, i32)>, i32, i32) {
    let w = sizes.iter().map(|s| s.0).max().unwrap_or(0);
    let mut y = 0;
    let mut out = Vec::with_capacity(sizes.len());
    for (i, &(ew, eh)) in sizes.iter().enumerate() {
        if i > 0 {
            y += 10;
        }
        out.push((center(w, ew), y));
        y += eh;
    }
    (out, w, y)
}

/// Scrolling area of the body: (x, y, width, height) for a `cw` × `ch` content; margin of
/// 10 on each side (bar and spacing), 30 under the header of 33 if room allows.
fn scroll_area(w: i32, h: i32, cw: i32, ch: i32, footer: i32) -> (i32, i32, i32, i32) {
    let aw = cw + 20;
    let ah = ch.min(h - 33 - footer).max(0);
    (center(w, aw), (33 + 30).min(h - footer - ah), aw, ah)
}

// ---------------------------------------------------------------------------------------------
// Screen.

/// Split body message, in content coordinates.
struct TextEl {
    lines: Vec<Vec<StyledChar>>,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

/// Body item: `w` × `h` box (item drawn at its corner).
struct ItemEl {
    id: String,
    count: i32,
    components: Option<Nbt>,
    decorations: bool,
    tooltip: bool,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    /// Stack resolved at first draw (item registry).
    stack: Option<Option<ItemStack>>,
}

/// Multi-line input: area with a field background, own scrolling, character counter below.
struct Multi {
    input: usize,
    area: Area,
    limit: usize,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    scroll: f64,
    focus_at: Instant,
    dragging: bool,
}

impl Multi {
    fn content(&self) -> i32 {
        9 * self.area.lines.len() as i32 + 8
    }

    fn set_scroll(&mut self, s: f64) {
        self.scroll = s.clamp(0.0, f64::from((self.content() - self.h).max(0)));
    }

    /// Brings the cursor back into the visible part.
    fn reveal(&mut self, m: &dyn rc_text::Metrics) {
        self.area.reflow(m);
        let n = self.area.lines.len() as i64;
        let line = |i: i64| self.area.lines[i.clamp(0, n - 1) as usize];
        let at = self.area.lines.iter().position(|&(a, b)| self.area.cursor >= a && self.area.cursor <= b);
        let at = at.unwrap_or_else(|| self.area.lines.len().saturating_sub(1)) as f64;
        let first = line((self.scroll / 9.0) as i64);
        if self.area.cursor <= first.0 {
            self.set_scroll(at * 9.0);
        } else {
            let last = line(((self.scroll + f64::from(self.h)) / 9.0) as i64 - 1);
            if self.area.cursor > last.1 {
                self.set_scroll(at * 9.0 - f64::from(self.h) + 9.0 + 8.0);
            }
        }
    }
}

/// Built screen: components, separately drawn elements, last screen size.
struct Ui {
    size: (i32, i32),
    panel: Panel,
    title: Vec<StyledChar>,
    title_w: i32,
    /// Body components: index in the panel and abscissa in the content.
    slots: Vec<(usize, i32)>,
    /// Footer buttons: index in the panel.
    footer: Vec<usize>,
    texts: Vec<TextEl>,
    items: Vec<ItemEl>,
    multis: Vec<Multi>,
    multi_focus: Option<usize>,
    /// Chosen option of each choice input (input index).
    choices: Vec<usize>,
    content: (i32, i32),
    /// Screen abscissa of the content.
    origin: i32,
    title_xy: (i32, i32),
    last_click: Option<(Instant, (f32, f32))>,
}

/// Waiting screen: countdown of the back button (visible after 1 s, active after 5 s).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Wait {
    ticks: u32,
    label: usize,
    visible: bool,
    active: bool,
}

impl Wait {
    fn tick(&mut self) {
        if !self.active {
            let s = (self.ticks / 20) as usize;
            self.ticks += 1;
            self.visible = s >= 1;
            self.label = s.min(5);
            self.active = s >= 5;
        }
    }

    fn label(&self) -> Component {
        match self.label {
            0 => Component::text(""),
            5 => Component::translate("gui.back", vec![]),
            s => Component::translate("gui.waitingForResponse.button.inactive", vec![Arg::Text((5 - s).to_string())]),
        }
    }
}

/// Warning screen (disconnect or back), above the dialog.
struct Warn {
    panel: Panel,
    width: i32,
    lines: Vec<Vec<StyledChar>>,
    title: Vec<StyledChar>,
}

/// Outcome of an input: nothing, button action, close, disconnect.
enum Outcome {
    Pass,
    Done,
    Run(Option<ClickEvent>, After),
    Close,
    Disconnect,
}

const WARN: u32 = 1;
const INPUT: u32 = 100;
const LABEL: u32 = 600;
const GRID: u32 = 1000;
const FOOT: u32 = 5000;

/// Open screen: dialog, or waiting for a response (after a `wait_for_response` action).
pub struct Screen {
    pub dialog: Dialog,
    /// Open during configuration (commands are ignored there).
    pub configuration: bool,
    waiting: Option<Wait>,
    warning: Option<Warn>,
    /// Dialog cleared by the server under the warning: "Back" leads to the world.
    back_closes: bool,
    ui: Option<Ui>,
    opened: Instant,
}

impl Screen {
    pub fn new(dialog: Dialog, configuration: bool) -> Screen {
        Screen { dialog, configuration, waiting: None, warning: None, back_closes: false, ui: None, opened: Instant::now() }
    }

    /// Server `clear_dialog`. True: the screen closes. Like the game, the wait for a response
    /// stays displayed; under the warning, only the back screen changes (the world).
    fn clear(&mut self) -> bool {
        if self.waiting.is_some() {
            return false;
        }
        if self.warning.is_some() {
            self.back_closes = true;
            return false;
        }
        true
    }

    /// New server dialog: under the warning, it becomes the back screen.
    fn hand_over(&mut self, next: &mut Screen) {
        next.warning = self.warning.take();
    }

    /// "Back" of the warning: dialog, or world if it was cleared.
    fn warning_back(&mut self) -> Outcome {
        if self.back_closes {
            return Outcome::Close;
        }
        self.warning = None;
        Outcome::Done
    }

    /// The displayed screen pauses singleplayer: `pause` dialog, warning; not the wait.
    pub fn pauses(&self) -> bool {
        self.warning.is_some() || (self.waiting.is_none() && self.dialog.pause)
    }

    fn start_wait(&mut self) {
        self.waiting = Some(Wait::default());
        self.warning = None;
    }

    pub fn tick(&mut self) {
        if let Some(w) = &mut self.waiting {
            w.tick();
        }
    }

    /// Input values (initial values before the first draw).
    fn values(&self) -> Vec<Value> {
        self.dialog
            .inputs
            .iter()
            .enumerate()
            .map(|(i, inp)| {
                let Some(ui) = &self.ui else { return inp.value(&inp.initial()) };
                let state = match &inp.control {
                    Control::Text { multiline: Some(_), .. } => {
                        ui.multis.iter().find(|m| m.input == i).map(|m| State::Text(m.area.value()))
                    }
                    Control::Choice { .. } => Some(State::Choice(ui.choices[i])),
                    _ => ui.panel.get(INPUT + i as u32).and_then(|w| match &w.kind {
                        Kind::Field(t) => Some(State::Text(t.value())),
                        Kind::Checkbox { checked } => Some(State::Bool(*checked)),
                        Kind::Slider { value, .. } => Some(State::Number(*value as f32)),
                        _ => None,
                    }),
                };
                inp.value(&state.unwrap_or_else(|| inp.initial()))
            })
            .collect()
    }

    /// Action of button `i` (grid then footer).
    fn press(&self, i: usize) -> Outcome {
        let action = self.dialog.buttons().nth(i).and_then(|b| b.action.as_ref());
        Outcome::Run(action.map(|a| a.create(&self.values())), self.dialog.after)
    }

    fn build(&mut self, gui: &Gui) {
        let d = &self.dialog;
        let mut widgets = Vec::new();
        let title = chars(gui, &d.title);
        let title_w = rc_text::width(gui, &title);
        let mut warn = Widget::new(WARN, Kind::Sprite("dialog/warning_button"), 0, 0, 20, 20, Vec::new());
        warn.set_tooltip(gui, &rc_text::plain_text(&rc_text::flatten(
            &Component::translate("menu.custom_screen_info.tooltip", vec![]),
            &gui.lang,
        )));
        widgets.push(warn);
        // Body elements: size, then parts to place (relative to the element's corner).
        enum Part {
            Widget(usize),
            Text(usize),
            Item(usize),
            Multi(usize),
        }
        type Elem = ((i32, i32), Vec<(Part, i32, i32)>);
        let mut elems: Vec<Elem> = Vec::new();
        let (mut texts, mut items, mut multis) = (Vec::new(), Vec::new(), Vec::new());
        let text_el = |gui: &Gui, m: &Message| {
            let lines = rc_text::wrap(gui, &chars(gui, &m.contents), (m.width - 8) as f32);
            let h = 9 * lines.len() as i32 + 8;
            TextEl { lines, x: 0, y: 0, w: m.width, h }
        };
        for b in &d.body {
            match b {
                Body::Text(m) => {
                    let t = text_el(gui, m);
                    elems.push(((t.w, t.h), vec![(Part::Text(texts.len()), 0, 0)]));
                    texts.push(t);
                }
                Body::Item { id, count, components, description, decorations, tooltip, width, height } => {
                    items.push(ItemEl {
                        id: id.clone(),
                        count: *count,
                        components: components.clone(),
                        decorations: *decorations,
                        tooltip: *tooltip,
                        x: 0,
                        y: 0,
                        w: *width,
                        h: *height,
                        stack: None,
                    });
                    let item = Part::Item(items.len() - 1);
                    match description {
                        Some(m) => {
                            let t = text_el(gui, m);
                            let h = (*height).max(t.h);
                            let round = |a: i32| ((h - a) as f32 * 0.5).round() as i32;
                            let parts = vec![(item, 0, round(*height)), (Part::Text(texts.len()), width + 2, round(t.h))];
                            elems.push(((width + 2 + t.w, h), parts));
                            texts.push(t);
                        }
                        None => elems.push(((*width, *height), vec![(item, 0, 0)])),
                    }
                }
            }
        }
        let mut choices = vec![0; d.inputs.len()];
        for (i, inp) in d.inputs.iter().enumerate() {
            let id = INPUT + i as u32;
            // Label above (spacing 4), left-aligned.
            let labeled = |widgets: &mut Vec<Widget>, label: &Component, w: i32, h: i32, part: Part| {
                let msg = chars(gui, label);
                let lw = rc_text::width(gui, &msg);
                widgets.push(Widget::new(LABEL + i as u32, Kind::Label { color: 0xffff_ffff, centered: false }, 0, 0, lw, 9, msg));
                ((lw.max(w), 13 + h), vec![(Part::Widget(widgets.len() - 1), 0, 0), (part, 0, 13)])
            };
            let elem = match &inp.control {
                Control::Text { width, label, label_visible, initial, max_len, multiline } => {
                    let (part, h) = match multiline {
                        Some((lines, height)) => {
                            let h = height.unwrap_or_else(|| (9 * lines.unwrap_or(4) as i32 + 8).min(512));
                            let mut area = Area::new((width - 8) as f32, *max_len, *lines);
                            area.set_value_within(gui, initial);
                            area.reflow(gui);
                            multis.push(Multi {
                                input: i,
                                area,
                                limit: *max_len,
                                x: 0,
                                y: 0,
                                w: *width,
                                h,
                                scroll: 0.0,
                                focus_at: Instant::now(),
                                dragging: false,
                            });
                            (Part::Multi(multis.len() - 1), h)
                        }
                        None => {
                            let mut t = TextField::new(Vec::new());
                            t.max_len = *max_len;
                            t.set_value(initial);
                            widgets.push(Widget::new(id, Kind::Field(Box::new(t)), 0, 0, *width, 20, chars(gui, label)));
                            (Part::Widget(widgets.len() - 1), 20)
                        }
                    };
                    if *label_visible { labeled(&mut widgets, label, *width, h, part) } else { ((*width, h), vec![(part, 0, 0)]) }
                }
                Control::Bool { label, initial, .. } => {
                    let msg = chars(gui, label);
                    let w = 17 + 4 + rc_text::width(gui, &msg);
                    widgets.push(Widget::new(id, Kind::Checkbox { checked: *initial }, 0, 0, w, 17, msg));
                    ((w, 17), vec![(Part::Widget(widgets.len() - 1), 0, 0)])
                }
                Control::Choice { width, initial, .. } => {
                    choices[i] = *initial;
                    let msg = choice_label(gui, &inp.control, *initial);
                    widgets.push(Widget::new(id, Kind::Cycle, 0, 0, *width, 20, msg));
                    ((*width, 20), vec![(Part::Widget(widgets.len() - 1), 0, 0)])
                }
                Control::Number { width, range, .. } => {
                    let v = range.initial_slider();
                    let msg = number_label(gui, &inp.control, v);
                    let kind = Kind::Slider { value: f64::from(v), steps: 0, adjusting: false };
                    widgets.push(Widget::new(id, kind, 0, 0, *width, 20, msg));
                    ((*width, 20), vec![(Part::Widget(widgets.len() - 1), 0, 0)])
                }
            };
            elems.push(elem);
        }
        let make_button = |gui: &Gui, id: u32, b: &Button| {
            let mut w = Widget::new(id, Kind::Button, 0, 0, b.width, 20, chars(gui, &b.label));
            if let Some(t) = &b.tooltip {
                w.tooltip = rc_text::wrap(gui, &chars(gui, t), 170.0);
            }
            w
        };
        if d.list {
            let widths: Vec<i32> = d.grid.iter().map(|b| b.width).collect();
            let (pos, gw, gh) = pack(&widths, d.columns);
            let mut parts = Vec::with_capacity(pos.len());
            for (k, (b, (x, y))) in d.grid.iter().zip(pos).enumerate() {
                widgets.push(make_button(gui, GRID + k as u32, b));
                parts.push((Part::Widget(widgets.len() - 1), x, y));
            }
            elems.push(((gw, gh), parts));
        }
        let sizes: Vec<(i32, i32)> = elems.iter().map(|e| e.0).collect();
        let (pos, cw, ch) = column(&sizes);
        let mut slots = Vec::new();
        for ((_, parts), (ex, ey)) in elems.into_iter().zip(pos) {
            for (part, dx, dy) in parts {
                let (x, y) = (ex + dx, ey + dy);
                match part {
                    Part::Widget(k) => {
                        widgets[k].in_list = true;
                        widgets[k].y = y;
                        slots.push((k, x));
                    }
                    Part::Text(k) => (texts[k].x, texts[k].y) = (x, y),
                    Part::Item(k) => (items[k].x, items[k].y) = (x, y),
                    Part::Multi(k) => (multis[k].x, multis[k].y) = (x, y),
                }
            }
        }
        let mut footer = Vec::new();
        for (k, b) in d.footer.iter().enumerate() {
            widgets.push(make_button(gui, FOOT + k as u32, b));
            footer.push(widgets.len() - 1);
        }
        let mut list = List::new(0, 0, 0, 0, 0, 20);
        list.background = false;
        list.separators = false;
        list.custom_rows = true;
        list.content = ch;
        let mut panel = Panel::new(widgets);
        panel.list = Some(list);
        self.ui = Some(Ui {
            size: (-1, -1),
            panel,
            title,
            title_w,
            slots,
            footer,
            texts,
            items,
            multis,
            multi_focus: None,
            choices,
            content: (cw, ch),
            origin: 0,
            title_xy: (0, 0),
            last_click: None,
        });
    }

    /// Positions for a screen size (values and scrolling kept).
    fn place(&mut self, w: i32, h: i32) {
        let footer_h = self.dialog.footer_height();
        let Some(ui) = &mut self.ui else { return };
        if ui.size == (w, h) {
            return;
        }
        ui.size = (w, h);
        let (cw, ch) = ui.content;
        let (ax, ay, aw, ah) = scroll_area(w, h, cw, ch, footer_h);
        ui.origin = ax + 10;
        if let Some(l) = &mut ui.panel.list {
            (l.x, l.y, l.w, l.h) = (ax, ay, aw, ah);
            // Bar at 6 from the right edge of the area (row width chosen for that).
            l.row_width = cw - 8;
            l.set_scroll(l.scroll);
        }
        for &(k, x) in &ui.slots {
            ui.panel.widgets[k].x = ui.origin + x;
        }
        // Header: title then warning button (spacing 10), centred on 33.
        let row = ui.title_w + 10 + 20;
        let rx = center(w, row);
        ui.title_xy = (rx, 13);
        let (mut bx, mut by) = (rx + ui.title_w + 10, 7);
        if bx < 0 || by < 0 || bx > w - 20 || by > h - 20 {
            (bx, by) = ((w - 40).max(0), 5.min(h));
        }
        (ui.panel.widgets[0].x, ui.panel.widgets[0].y) = (bx, by);
        // Footer: centred row (spacing 8) at 7 from the top of the 33 band.
        let widths: i32 = ui.footer.iter().map(|&k| ui.panel.widgets[k].w).sum();
        let total = widths + 8 * (ui.footer.len() as i32 - 1).max(0);
        let mut x = center(w, total);
        for &k in &ui.footer {
            (ui.panel.widgets[k].x, ui.panel.widgets[k].y) = (x, h - 26);
            x += ui.panel.widgets[k].w + 8;
        }
    }

    pub fn draw(
        &mut self,
        gui: &mut Gui,
        q: &wgpu::Queue,
        mouse: Option<(f32, f32)>,
        items: Option<&HotbarItems<'_>>,
        enchantments: &[String],
    ) {
        let (w, h) = (gui.width as i32, gui.height as i32);
        // Warning: separate screen that replaces the dialog (nothing of the dialog behind),
        // including when the server cleared it in the meantime.
        if let Some(warn) = &mut self.warning {
            draw_warning(gui, q, warn, mouse, self.opened.elapsed().as_secs_f64());
            return;
        }
        gui.next_layer();
        crate::widgets::ingame_background(gui);
        if let Some(wait) = self.waiting {
            draw_wait(gui, q, &wait, mouse);
            return;
        }
        if self.ui.is_none() {
            self.build(gui);
        }
        self.place(w, h);
        let time = self.opened.elapsed().as_secs_f64();
        let Some(ui) = &mut self.ui else { return };
        let (tx, ty) = ui.title_xy;
        gui.text(q, &ui.title, tx as f32, ty as f32, 0xffff_ffff, true, 0);
        let Ui { panel, texts, items: item_els, multis, multi_focus, origin, .. } = ui;
        let origin = *origin;
        let focus = *multi_focus;
        let mouse_in = mouse;
        let mut hovered_item = None;
        panel.draw_with(gui, q, mouse_in, time, &mut |gui, q, l| {
            let top = |y: i32| l.y - l.scroll.floor() as i32 + y;
            for t in texts.iter() {
                let mid = origin + t.x + t.w / 2;
                for (k, line) in t.lines.iter().enumerate() {
                    let lx = mid - gui.visual_width(line) / 2;
                    gui.text(q, line, lx as f32, (top(t.y) + 4 + 9 * k as i32) as f32, 0xffff_ffff, true, 0);
                }
            }
            for (k, it) in item_els.iter_mut().enumerate() {
                let (x, y) = (origin + it.x, top(it.y));
                if let Some(hb) = items {
                    let (id, count) = (&it.id, it.count);
                    if let Some(st) = it.stack.get_or_insert_with(|| resolve(hb, id, count)) {
                        crate::special::item(gui, hb, st, x as f32, y as f32);
                        if it.decorations {
                            crate::special::decorations(gui, q, hb, st, x as f32, y as f32, false);
                        }
                    }
                }
                if it.tooltip && crate::special::inside(mouse_in, x, y, it.w, it.h) && inside_list(l, mouse_in) {
                    hovered_item = Some(k);
                }
            }
            for (k, m) in multis.iter().enumerate() {
                draw_multi(gui, q, m, origin + m.x, top(m.y), focus == Some(k));
            }
        });
        if let (Some(k), Some(m), Some(hb)) = (hovered_item, mouse_in, items) {
            let it = &item_els[k];
            match (&it.components, &it.stack) {
                (None, Some(Some(st))) => crate::special::item_tooltip(gui, q, hb, enchantments, st, m),
                _ => {
                    let h = HoverEvent::Item { id: it.id.clone(), count: it.count, components: it.components.clone() };
                    crate::links::draw_hover(gui, q, &h, m, hb.data, crate::screens::advanced());
                }
            }
        }
    }

    /// Window input (GUI units).
    fn input(&mut self, event: &WindowEvent, m: Option<(f32, f32)>, gui: &Gui, mods: ModifiersState, solo: bool) -> Outcome {
        if let Some(wait) = self.waiting {
            return match event {
                WindowEvent::KeyboardInput { event, .. }
                    if event.state == ElementState::Pressed && crate::widgets::physical_key(event) == Some(KeyCode::Escape) =>
                {
                    if wait.active { Outcome::Close } else { Outcome::Done }
                }
                WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => {
                    let (x, y) = wait_button(gui.width as i32, gui.height as i32);
                    if wait.active && crate::special::inside(m, x, y, 200, 20) { Outcome::Close } else { Outcome::Done }
                }
                WindowEvent::KeyboardInput { .. } | WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. } => {
                    Outcome::Done
                }
                _ => Outcome::Pass,
            };
        }
        if let Some(warn) = &mut self.warning {
            let mut ev = Vec::new();
            match event {
                WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                    match crate::widgets::edit_key(event, mods) {
                        Some(KeyCode::Escape) => ev.push(Event::Press(1)),
                        Some(k) => {
                            warn.panel.key(k, mods, &mut ev);
                        }
                        None => {}
                    }
                }
                WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => {
                    if let Some(m) = m {
                        warn.panel.press(m, 0, mods.shift_key(), &mut ev);
                    }
                }
                WindowEvent::CursorMoved { .. } => {
                    if let Some(m) = m {
                        warn.panel.motion(m, &mut ev);
                    }
                    return Outcome::Pass;
                }
                WindowEvent::KeyboardInput { .. } | WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. } => {}
                _ => return Outcome::Pass,
            }
            return match ev.first() {
                Some(Event::Press(0)) => Outcome::Disconnect,
                Some(Event::Press(_)) => self.warning_back(),
                _ => Outcome::Done,
            };
        }
        let escape = self.dialog.escape;
        let cancel = self.dialog.cancel.clone();
        let Some(ui) = &mut self.ui else {
            return if matches!(event, WindowEvent::CursorMoved { .. }) { Outcome::Pass } else { Outcome::Done };
        };
        let mut ev = Vec::new();
        match event {
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let Some(k) = crate::widgets::edit_key(event, mods) else { return Outcome::Done };
                if k == KeyCode::Escape {
                    return if escape {
                        Outcome::Run(cancel.map(|a| a.create(&self.values())), After::Close)
                    } else {
                        Outcome::Done
                    };
                }
                if let Some(f) = ui.multi_focus
                    && k != KeyCode::Tab
                {
                    let mt = &mut ui.multis[f];
                    mt.area.key(gui, k, mods);
                    // Ctrl + letter types nothing, AltGr types its character (`widgets::typed_text`).
                    if let Some(t) = crate::widgets::typed_text(event, mods) {
                        mt.area.insert(gui, t);
                    }
                    mt.reveal(gui);
                    return Outcome::Done;
                }
                if k == KeyCode::Tab {
                    ui.multi_focus = None;
                }
                ui.panel.key(k, mods, &mut ev);
                if let Some(t) = crate::widgets::typed_text(event, mods) {
                    ui.panel.typed(t, &mut ev);
                }
            }
            // Text committed by the IME: like a keystroke in the focused field.
            e @ WindowEvent::Ime(_) => {
                let Some(t) = crate::ime::commit(e) else { return Outcome::Pass };
                if let Some(f) = ui.multi_focus {
                    let mt = &mut ui.multis[f];
                    mt.area.insert(gui, t);
                    mt.reveal(gui);
                    return Outcome::Done;
                }
                ui.panel.typed(t, &mut ev);
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                let Some(m) = m else { return Outcome::Done };
                if *state == ElementState::Released {
                    ui.panel.release(&mut ev);
                    for mt in &mut ui.multis {
                        mt.dragging = false;
                    }
                } else {
                    let double = ui.last_click.is_some_and(|(t, p)| t.elapsed().as_millis() < 250 && p == m);
                    ui.last_click = Some((Instant::now(), m));
                    ui.panel.press(m, 0, mods.shift_key(), &mut ev);
                    ui.multi_focus = None;
                    if ui.panel.focus.is_none() {
                        let in_list = ui.panel.list.as_ref().is_some_and(|l| inside_list(l, Some(m)));
                        let top = ui.panel.list.as_ref().map_or(0, |l| l.y - l.scroll.floor() as i32);
                        if in_list {
                            for (k, mt) in ui.multis.iter_mut().enumerate() {
                                let (x, y) = (ui.origin + mt.x, top + mt.y);
                                if crate::special::inside(Some(m), x, y, mt.w, mt.h) {
                                    ui.multi_focus = Some(k);
                                    mt.focus_at = Instant::now();
                                    if double {
                                        mt.area.select_word();
                                    } else {
                                        mt.area.selecting = mods.shift_key();
                                        mt.area.seek_point(gui, m.0 - (x + 4) as f32, m.1 - (y + 4) as f32 + mt.scroll as f32);
                                        mt.dragging = true;
                                    }
                                    return Outcome::Done;
                                }
                            }
                            // Click in a message: action of the style under the mouse.
                            for t in &ui.texts {
                                let (x, y) = (ui.origin + t.x, top + t.y);
                                if !crate::special::inside(Some(m), x, y, t.w, t.h) {
                                    continue;
                                }
                                let row = ((m.1 - (y + 4) as f32) / 9.0).floor();
                                let Some(line) = (row >= 0.0).then(|| t.lines.get(row as usize)).flatten() else { continue };
                                let lx = x + t.w / 2 - gui.visual_width(line) / 2;
                                if let Some(click) = crate::links::style_at(gui, line, m.0 - lx as f32).and_then(|s| s.click.clone()) {
                                    return Outcome::Run(Some((*click).clone()), self.dialog.after);
                                }
                            }
                        }
                    }
                }
            }
            WindowEvent::CursorMoved { .. } => {
                if let Some(m) = m {
                    ui.panel.motion(m, &mut ev);
                    let top = ui.panel.list.as_ref().map_or(0, |l| l.y - l.scroll.floor() as i32);
                    for mt in ui.multis.iter_mut().filter(|mt| mt.dragging) {
                        let (x, y) = (ui.origin + mt.x, top + mt.y);
                        let shift = mods.shift_key();
                        mt.area.selecting = true;
                        mt.area.seek_point(gui, m.0 - (x + 4) as f32, m.1 - (y + 4) as f32 + mt.scroll as f32);
                        mt.area.selecting = shift;
                    }
                }
                if ev.is_empty() {
                    return Outcome::Pass;
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => f64::from(*y),
                    MouseScrollDelta::PixelDelta(p) => p.y / 20.0,
                };
                if let Some(m) = m {
                    let top = ui.panel.list.as_ref().map_or(0, |l| l.y - l.scroll.floor() as i32);
                    let over = ui.multis.iter_mut().find(|mt| {
                        let (x, y) = (ui.origin + mt.x, top + mt.y);
                        crate::special::inside(Some(m), x, y, mt.w + 6, mt.h)
                    });
                    match over {
                        Some(mt) if mt.content() > mt.h => mt.set_scroll(mt.scroll - dy * 4.0),
                        _ => ui.panel.wheel(m, dy, &mut ev),
                    }
                }
            }
            WindowEvent::MouseInput { .. } | WindowEvent::KeyboardInput { .. } => {}
            _ => return Outcome::Pass,
        }
        if ui.panel.focus.is_some() {
            ui.multi_focus = None;
        }
        let mut out = Outcome::Done;
        for e in ev {
            match e {
                Event::Press(WARN) => {
                    self.warning = Some(warn_screen(gui, solo));
                }
                Event::Press(id) if id >= FOOT => out = self.press(self.dialog.grid.len() + (id - FOOT) as usize),
                Event::Press(id) if id >= GRID => out = self.press((id - GRID) as usize),
                Event::Cycle(id, d) => {
                    let i = (id - INPUT) as usize;
                    let Some(ui) = &mut self.ui else { break };
                    if let Some(Control::Choice { options, .. }) = self.dialog.inputs.get(i).map(|x| &x.control) {
                        let n = options.len() as i32;
                        ui.choices[i] = (ui.choices[i] as i32 + d).rem_euclid(n) as usize;
                        let msg = choice_label(gui, &self.dialog.inputs[i].control, ui.choices[i]);
                        if let Some(w) = ui.panel.get_mut(id) {
                            w.msg = msg;
                        }
                    }
                }
                Event::Slide(id, v) => {
                    let i = (id - INPUT) as usize;
                    let Some(ui) = &mut self.ui else { break };
                    if let Some(inp) = self.dialog.inputs.get(i) {
                        let msg = number_label(gui, &inp.control, v as f32);
                        if let Some(w) = ui.panel.get_mut(id) {
                            w.msg = msg;
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// Clicks to sound since the last input.
    fn take_clicks(&mut self) -> u32 {
        let a = self.ui.as_mut().map_or(0, |u| std::mem::take(&mut u.panel.clicks));
        let b = self.warning.as_mut().map_or(0, |w| std::mem::take(&mut w.panel.clicks));
        a + b
    }
}

fn inside_list(l: &List, m: Option<(f32, f32)>) -> bool {
    crate::special::inside(m, l.x, l.y, l.w, l.h)
}

/// Stack of a body item (without its components), `None` if the item is unknown.
fn resolve(it: &HotbarItems<'_>, id: &str, count: i32) -> Option<ItemStack> {
    let full = if id.contains(':') { id.to_owned() } else { format!("minecraft:{id}") };
    let index = it.data.registry("minecraft:item")?.entries.iter().position(|e| *e == full)?;
    Some(ItemStack { item: index as i32, count, added: Vec::new(), removed: Vec::new() })
}

/// Label of a choice input: value alone, or "label: value".
fn choice_label(gui: &Gui, c: &Control, i: usize) -> Vec<StyledChar> {
    let Control::Choice { options, label, label_visible, .. } = c else { return Vec::new() };
    let (id, display) = &options[i.min(options.len() - 1)];
    let value = display.clone().unwrap_or_else(|| Component::text(id.clone()));
    if *label_visible {
        chars(gui, &Component::translate("options.generic_value", vec![Arg::Component(label.clone()), Arg::Component(value)]))
    } else {
        chars(gui, &value)
    }
}

/// Label of a numeric slider: format of the input (label, value).
fn number_label(gui: &Gui, c: &Control, slider: f32) -> Vec<StyledChar> {
    let Control::Number { label, format, range, .. } = c else { return Vec::new() };
    let v = number_text(range.value(slider));
    chars(gui, &Component::translate(format.clone(), vec![Arg::Component(label.clone()), Arg::Text(v)]))
}

/// Waiting screen button: 200 wide, 30 under the header of 33.
fn wait_button(w: i32, h: i32) -> (i32, i32) {
    (center(w, 200), 63.min(h - 20))
}

fn draw_wait(gui: &mut Gui, q: &wgpu::Queue, wait: &Wait, mouse: Option<(f32, f32)>) {
    let (w, h) = (gui.width as i32, gui.height as i32);
    let title = chars(gui, &Component::translate("gui.waitingForResponse.title", vec![]));
    let tw = rc_text::width(gui, &title);
    gui.text(q, &title, center(w, tw) as f32, 12.0, 0xffff_ffff, true, 0);
    if !wait.visible {
        return;
    }
    let (x, y) = wait_button(w, h);
    let hovered = crate::special::inside(mouse, x, y, 200, 20);
    let sprite = match (wait.active, hovered) {
        (false, _) => "widget/button_disabled",
        (true, true) => "widget/button_highlighted",
        (true, false) => "widget/button",
    };
    gui.sprite(sprite, x as f32, y as f32, 200.0, 20.0, 0xffff_ffff);
    let label = chars(gui, &wait.label());
    let color = if wait.active { 0xffff_ffff } else { 0xffa0_a0a0 };
    crate::widgets::scrolling_text(gui, q, &label, x + 2, y, x + 198, y + 20, color, 0.0);
}

/// Warning: title, text (cut at L − 50, 15 lines), "Disconnect" / "Back".
fn warn_screen(gui: &Gui, solo: bool) -> Warn {
    let tr = |k: &str| chars(gui, &Component::translate(k, vec![]));
    let first = tr(if solo { "menu.returnToMenu" } else { "menu.disconnect" });
    let panel =
        Panel::new(vec![Widget::new(0, Kind::Button, 0, 0, 150, 20, first), Widget::new(1, Kind::Button, 0, 0, 150, 20, tr("gui.back"))]);
    Warn { panel, width: -1, lines: Vec::new(), title: tr("menu.custom_screen_info.title") }
}

fn draw_warning(gui: &mut Gui, q: &wgpu::Queue, warn: &mut Warn, mouse: Option<(f32, f32)>, time: f64) {
    let (w, h) = (gui.width as i32, gui.height as i32);
    if warn.width != w {
        let msg = chars(gui, &Component::translate("menu.custom_screen_info.contents", vec![]));
        warn.lines = rc_text::wrap(gui, &msg, (w - 50) as f32);
        warn.lines.truncate(15);
        warn.width = w;
    }
    gui.next_layer();
    crate::widgets::ingame_background(gui);
    // Centred column (spacing 8): title, text, buttons 16 lower (spaced by 4).
    let total = 9 + 8 + 9 * warn.lines.len() as i32 + 8 + 16 + 20;
    let mut y = center(h, total);
    crate::widgets::centered(gui, q, &warn.title, w / 2, y, 0xffff_ffff);
    y += 17;
    for l in &warn.lines {
        crate::widgets::centered(gui, q, l, w / 2, y, 0xffff_ffff);
        y += 9;
    }
    y += 8 + 16;
    let x0 = center(w, 304);
    for (k, wd) in warn.panel.widgets.iter_mut().enumerate() {
        (wd.x, wd.y) = (x0 + 154 * k as i32, y);
    }
    warn.panel.draw(gui, q, mouse, time);
}

/// Multi-line input at (x, y): field background, scrolled lines, cursor, selection, scroll
/// bar on the right, character counter below.
fn draw_multi(gui: &mut Gui, q: &wgpu::Queue, m: &Multi, x: i32, y: i32, focused: bool) {
    let sprite = if focused { "widget/text_field_highlighted" } else { "widget/text_field" };
    gui.sprite(sprite, x as f32, y as f32, m.w as f32, m.h as f32, 0xffff_ffff);
    let old = gui.clip();
    let r = [(x + 1) as f32, (y + 1) as f32, (x + m.w - 1) as f32, (y + m.h - 1) as f32];
    gui.set_clip(Some(match old {
        Some(c) => [c[0].max(r[0]), c[1].max(r[1]), c[2].min(r[2]), c[3].min(r[3])],
        None => r,
    }));
    let a = &m.area;
    let scroll = m.scroll.floor() as i32;
    let (left, top0) = (x + 4, y + 4 - scroll);
    let visible = |t: i32| t + 9 >= y && t <= y + m.h;
    let (color, cursor_color) = (0xffe0_e0e0, 0xffd0_d0d0);
    let blink = focused && (m.focus_at.elapsed().as_millis() / 300).is_multiple_of(2);
    let s = |r: &[char]| crate::gui::plain(&r.iter().collect::<String>());
    let insert = a.cursor < a.text.len();
    let (mut cx, mut cy, mut drawn) = (left, top0, false);
    let mut top = top0;
    for &(b, e) in &a.lines {
        if !drawn && insert && a.cursor >= b && a.cursor <= e {
            cx = left + crate::book::plain_width(gui, &a.text[b..a.cursor]);
            cy = top;
            if visible(top) {
                gui.text(q, &s(&a.text[b..a.cursor]), left as f32, top as f32, color, true, 0);
                gui.text(q, &s(&a.text[a.cursor..e]), cx as f32, top as f32, color, true, 0);
                if blink {
                    gui.fill(cx as f32, (top - 1) as f32, (cx + 1) as f32, (top + 10) as f32, cursor_color);
                }
            }
            drawn = true;
        } else {
            if visible(top) {
                gui.text(q, &s(&a.text[b..e]), left as f32, top as f32, color, true, 0);
            }
            if !insert {
                cx = left + crate::book::plain_width(gui, &a.text[b..e]);
                cy = top;
            }
        }
        top += 9;
    }
    if blink && !insert && visible(cy) {
        gui.text(q, &crate::gui::plain("_"), cx as f32, cy as f32, cursor_color, true, 0);
    }
    if focused {
        crate::ime::field_cursor(cx, cy, 10);
    }
    let (sb, se) = a.selection();
    if sb != se {
        let mut top = top0;
        for &(b, e) in &a.lines {
            if sb > e {
                top += 9;
                continue;
            }
            if b > se {
                break;
            }
            if visible(top) {
                let x0 = crate::book::plain_width(gui, &a.text[b..sb.max(b)]);
                let x1 = if se > e { m.w - 4 } else { crate::book::plain_width(gui, &a.text[b..se]) };
                gui.text_highlight((left + x0) as f32, top as f32, (left + x1) as f32, (top + 9) as f32, true);
            }
            top += 9;
        }
    }
    gui.set_clip(old);
    let content = m.content();
    if content > m.h {
        let bx = (x + m.w) as f32;
        gui.sprite("widget/scroller_background", bx, y as f32, 6.0, m.h as f32, 0xffff_ffff);
        let max = f64::from(content - m.h);
        let bh = ((m.h * m.h) / content).clamp(32, (m.h - 8).max(32));
        let by = ((m.scroll.floor() * f64::from(m.h - bh) / max) as i32 + y).max(y);
        gui.sprite("widget/scroller", bx, by as f32, 6.0, bh as f32, 0xffff_ffff);
    }
    let used = crate::book::u16_len(&a.text);
    let count = Component::translate(
        "gui.multiLineEditBox.character_limit",
        vec![Arg::Text(used.to_string()), Arg::Text(m.limit.to_string())],
    );
    let count = chars(gui, &count);
    let tw = rc_text::width(gui, &count);
    gui.text(q, &count, (x + m.w - tw) as f32, (y + m.h + 4) as f32, 0xffa0_a0a0, true, 0);
}

impl App {
    /// `show_dialog`: replaces the current dialog screen, if any (the previous screen stays the same).
    pub(crate) fn show_dialog(&mut self, d: rc_client::extra::DialogRef, configuration: bool) {
        let nbt = match d {
            rc_client::extra::DialogRef::Inline(n) => Some(n),
            rc_client::extra::DialogRef::Registry(i) => self.dialog_entry(i),
        };
        let Some(dialog) = nbt.as_ref().and_then(|n| self.parse_dialog(n)) else {
            eprintln!("unreadable dialog");
            return;
        };
        if rc_client::protolog::enabled() {
            eprintln!("dialog: {} button(s), after {:?}", dialog.buttons().count(), dialog.after);
        }
        let mut next = Screen::new(dialog, configuration);
        if let Some(s) = &mut self.exact.dialog {
            s.hand_over(&mut next);
        }
        self.exact.dialog = Some(next);
        self.exact.dialog_ticks = 0;
        self.keys.clear();
        self.grab(false);
    }

    fn parse_dialog(&self, n: &Nbt) -> Option<Dialog> {
        Dialog::parse(n, &self.exact.links, &|v| self.dialog_set(v))
    }

    /// Entry of the `minecraft:dialog` registry: received content, otherwise file of the base pack.
    fn dialog_entry(&self, i: i32) -> Option<Nbt> {
        let (name, content) = self.exact.dialogs.get(usize::try_from(i).ok()?)?;
        if let Some(c) = content {
            return Some(c.clone());
        }
        let (ns, path) = name.split_once(':').unwrap_or(("minecraft", name));
        let bytes = self.assets.read(&format!("data/{ns}/dialog/{path}.json"))?;
        serde_json::from_slice::<serde_json::Value>(&bytes).ok().map(|v| json_to_nbt(&v))
    }

    fn dialog_named(&self, id: &str) -> Option<Nbt> {
        let i = self.exact.dialogs.iter().position(|(k, _)| short(k) == short(id))?;
        self.dialog_entry(i as i32)
    }

    /// Dialogs of a set: `#tag`, id, direct content or list of both.
    fn dialog_set(&self, v: &Nbt) -> Vec<Nbt> {
        match v {
            Nbt::String(s) => match s.strip_prefix('#') {
                Some(tag) => self
                    .exact
                    .dialog_tags
                    .iter()
                    .find(|(t, _)| short(t) == short(tag))
                    .map(|(_, ids)| ids.iter().filter_map(|&i| self.dialog_entry(i)).collect())
                    .unwrap_or_default(),
                None => self.dialog_named(s).into_iter().collect(),
            },
            Nbt::List(l) => l
                .iter()
                .filter_map(|e| match e {
                    Nbt::String(s) => self.dialog_named(s),
                    other => Some(other.clone()),
                })
                .collect(),
            c @ Nbt::Compound(_) => vec![c.clone()],
            _ => Vec::new(),
        }
    }

    /// Server `clear_dialog`: closes the dialog, except the wait and the warning.
    pub(crate) fn clear_dialog(&mut self) {
        if self.exact.dialog.as_mut().is_some_and(Screen::clear) {
            self.close_dialog();
        }
    }

    /// Escape or "close" action: back to the previous screen.
    pub(crate) fn close_dialog(&mut self) {
        if self.exact.dialog.take().is_some() && self.front.in_world && !self.menus.is_open() && !self.screens.open {
            self.grab(true);
        }
    }

    /// Action of a button, a text or Escape (§ 12), then next screen according to `after`.
    fn dialog_run(&mut self, ev: Option<ClickEvent>, after: After) {
        let Some(s) = &self.exact.dialog else { return };
        let config = s.configuration;
        if rc_client::protolog::enabled() {
            eprintln!("dialog: action {:?}, after {after:?}", ev.as_ref().map(|e| e.action.as_str()));
        }
        match &ev {
            // Other dialog: it replaces this one (unreadable: nothing changes).
            Some(e) if e.action == "show_dialog" => {
                let next = match &e.nbt {
                    Some(Nbt::String(id)) => self.dialog_named(id),
                    other => other.clone(),
                };
                if let Some(d) = next.as_ref().and_then(|n| self.parse_dialog(n)) {
                    self.exact.dialog = Some(Screen::new(d, config));
                }
                return;
            }
            // Configuration: command refused, the screen does not change.
            Some(e) if e.action == "run_command" && config => {
                eprintln!("dialog: command ignored during configuration: {}", e.value);
                return;
            }
            _ => {}
        }
        match after {
            After::Close => self.close_dialog(),
            After::None => {}
            After::Wait => {
                if let Some(s) = &mut self.exact.dialog {
                    s.start_wait();
                }
            }
        }
        let Some(e) = ev else { return };
        match e.action.as_str() {
            "run_command" => {
                let c = e.value.strip_prefix('/').unwrap_or(&e.value).to_owned();
                self.run_unattended(&c, crate::links::Origin::Chat);
            }
            "custom" => {
                let _ = self.session.link.send_custom_click_action(config, &e.value, e.nbt.as_ref());
            }
            "open_url" | "copy_to_clipboard" => self.click_event(&e, crate::links::Origin::Chat),
            // Suggestion: inserted in the next screen, which has no input field here.
            "suggest_command" => {}
            other => eprintln!("dialog: action without effect ({other})"),
        }
    }

    /// Inputs during a dialog (mouse, keyboard, wheel). True: consumed.
    pub(crate) fn dialog_window_event(&mut self, event: &WindowEvent) -> bool {
        if self.exact.dialog.is_none() {
            return false;
        }
        if let WindowEvent::CursorMoved { position, .. } = event {
            self.cursor = Some((position.x, position.y));
        }
        let m = self.gui_mouse();
        let (Some(r), Some(s)) = (&self.renderer, &mut self.exact.dialog) else { return true };
        let out = s.input(event, m, &r.gui, self.mods, self.solo.is_some());
        let clicks = s.take_clicks();
        if let Some(a) = self.audio.get() {
            for _ in 0..clicks {
                a.play(crate::widgets::CLICK_SOUND, rc_audio::Category::Ui, None, 0.25, 1.0, None);
            }
        }
        match out {
            Outcome::Pass => false,
            Outcome::Done => true,
            Outcome::Run(ev, after) => {
                self.dialog_run(ev, after);
                true
            }
            Outcome::Close => {
                self.close_dialog();
                true
            }
            Outcome::Disconnect => {
                self.exact.dialog = None;
                self.front.kicked = Some(Component::translate("menu.custom_screen_info.disconnect", vec![]));
                self.session_end("dialog");
                true
            }
        }
    }

    /// Mouse-free demo (`RUSTCRAFT_DIALOGTEST=<n>`): button n (grid then footer)
    /// pressed 60 ticks after a dialog opens.
    pub(crate) fn dialog_tick(&mut self) {
        let Some(s) = &mut self.exact.dialog else { return };
        s.tick();
        self.exact.dialog_ticks += 1;
        if self.exact.dialog_ticks == 60
            && let Some(n) = rc_data::env_str!("RUSTCRAFT_DIALOGTEST").and_then(|v| v.parse().ok())
            && let Outcome::Run(ev, after) = s.press(n)
        {
            self.dialog_run(ev, after);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(v: Vec<(&str, Nbt)>) -> Nbt {
        Nbt::Compound(v.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
    }

    fn parse(json: &str) -> Option<Dialog> {
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        Dialog::parse(&json_to_nbt(&v), &[], &|_| Vec::new())
    }

    #[test]
    fn notice_and_dynamic() {
        let d = parse(
            r#"{"type":"minecraft:multi_action","title":"T","body":{"type":"plain_message","contents":"Body"},
            "inputs":[{"type":"text","key":"name","label":"N","initial":"B\"ob"},{"type":"boolean","key":"ok","label":"O","initial":true}],
            "actions":[{"label":"Go","action":{"type":"dynamic/run_command","template":"say $(name) $(ok) $(absent)!"}},
            {"label":"C","width":80,"tooltip":"Help","action":{"type":"dynamic/custom","id":"x:y"}}],
            "exit_action":{"label":"End"}}"#,
        )
        .unwrap();
        assert_eq!((d.grid.len(), d.footer.len(), d.columns, d.list), (2, 1, 2, true));
        assert_eq!(d.cancel, None);
        assert_eq!(d.body.len(), 1);
        assert_eq!(d.grid[1].width, 80);
        assert_eq!(d.grid[1].tooltip, Some(Component::text("Help")));
        let s = Screen::new(d, false);
        let values = s.values();
        // Escaped text in the template, absent variable: empty.
        let Outcome::Run(Some(ev), After::Close) = s.press(0) else { panic!() };
        assert_eq!((ev.action.as_str(), ev.value.as_str()), ("run_command", "say B\\\"ob true !"));
        let Outcome::Run(Some(ev), _) = s.press(1) else { panic!() };
        assert_eq!(ev.nbt, Some(c(vec![("name", Nbt::String("B\"ob".into())), ("ok", Nbt::Byte(1))])));
        assert_eq!(values.len(), 2);
        let n = parse(r#"{"type":"notice","title":"A"}"#).unwrap();
        assert_eq!((n.footer.len(), n.cancel, n.escape, n.after, n.pause), (1, None, true, After::Close, true));
    }

    #[test]
    fn static_actions_and_lists() {
        let d = parse(
            r#"{"type":"confirmation","title":"T","pause":false,"after_action":"none",
            "yes":{"label":"Y","action":{"type":"open_url","url":"https://example.com"}},
            "no":{"label":"N","action":{"type":"copy_to_clipboard","value":"abc"}}}"#,
        )
        .unwrap();
        let Some(Action::Static(e)) = &d.footer[0].action else { panic!() };
        assert_eq!((e.action.as_str(), e.value.as_str()), ("open_url", "https://example.com"));
        let Some(Action::Static(e)) = &d.cancel else { panic!() };
        assert_eq!(e.action, "copy_to_clipboard");
        // A pause without a resume after the action is rejected.
        assert!(parse(r#"{"type":"notice","title":"A","after_action":"none"}"#).is_none());
        // List of dialogs: one button per dialog, external title otherwise title.
        let v: serde_json::Value = serde_json::from_str(
            r##"{"type":"dialog_list","title":"L","dialogs":"#minecraft:quick_actions","button_width":310,"columns":1}"##,
        )
        .unwrap();
        let sub = |t: &str, e: Option<&str>| {
            let mut v = vec![("type", Nbt::String("notice".into())), ("title", Nbt::String(t.into()))];
            if let Some(e) = e {
                v.push(("external_title", Nbt::String(e.into())));
            }
            c(v)
        };
        let set = |n: &Nbt| {
            assert_eq!(n, &Nbt::String("#minecraft:quick_actions".into()));
            vec![sub("A", Some("Ext")), sub("B", None)]
        };
        let d = Dialog::parse(&json_to_nbt(&v), &[], &set).unwrap();
        assert_eq!(d.grid.iter().map(|b| (b.label.clone(), b.width)).collect::<Vec<_>>(), vec![
            (Component::text("Ext"), 310),
            (Component::text("B"), 310)
        ]);
        let Some(Action::Static(e)) = &d.grid[1].action else { panic!() };
        assert_eq!((e.action.as_str(), e.nbt.clone()), ("show_dialog", Some(sub("B", None))));
        assert_eq!(d.footer_height(), 5);
        // Server links: "open the link" action.
        let links =
            [rc_client::extra::ServerLink { kind: Some(6), label: None, url: "https://example.org".into() }];
        let v: serde_json::Value = serde_json::from_str(r#"{"type":"server_links","title":"S"}"#).unwrap();
        let d = Dialog::parse(&json_to_nbt(&v), &links, &|_| Vec::new()).unwrap();
        let Some(Action::Static(e)) = &d.grid[0].action else { panic!() };
        assert_eq!((e.action.as_str(), e.value.as_str()), ("open_url", "https://example.org"));
    }

    #[test]
    fn templates_and_escapes() {
        assert!(Template::parse("say hi").is_none());
        assert!(Template::parse("say $(a").is_none());
        assert!(Template::parse("say $(a-b)").is_none());
        let t = Template::parse("$x $(a)$(b) end").unwrap();
        let v = |k: &str, s: &str| Value { key: k.into(), text: s.into(), nbt: Nbt::Byte(0) };
        assert_eq!(t.fill(&[v("b", "2"), v("a", "$(b)")]), "$x $(b)2 end");
        // Duplicate key: the last input wins.
        let t = Template::parse("say $(a)").unwrap();
        assert_eq!(t.fill(&[v("a", "x"), v("a", "y")]), "say y");
        // Names: letters and decimal digits of the basic plane only.
        assert!(Template::parse("say $(é_٣9)").is_some());
        for bad in ["say $(a½)", "say $(Ⅻ)", "say $(a😀)", "say $(𝐀)"] {
            assert!(Template::parse(bad).is_none(), "{bad}");
        }
        assert_eq!(escape_without_quotes("a\"b'c\\d\ne\u{1}"), "a\\\"b\\'c\\\\d\\ne\\x01");
    }

    #[test]
    fn number_range_values() {
        let r = Range { start: 0.0, end: 10.0, initial: None, step: None };
        // Without an initial value: middle of the range.
        assert_eq!(r.initial_slider(), 0.5);
        assert_eq!(number_text(r.value(r.initial_slider())), "5");
        let r = Range { start: 0.0, end: 10.0, initial: Some(1.0), step: Some(3.0) };
        // Steps counted from the initial value: 5 → 1 + 1 × 3.
        assert_eq!(r.value(0.5), 4.0);
        assert_eq!(r.value(1.0), 10.0);
        // 2 + 3 × 3 = 11 leaves the bounds: one step less.
        let r = Range { start: 0.0, end: 10.0, initial: Some(2.0), step: Some(3.0) };
        assert_eq!(r.value(1.0), 8.0);
        assert_eq!(number_text(2.5), "2.5");
        assert_eq!(number_text(0.0001), "1.0E-4");
        assert_eq!(number_text(-3.0), "-3");
    }

    #[test]
    fn layout_grid_and_areas() {
        // Two columns, three buttons: full row then last button centred.
        let (pos, w, h) = pack(&[150, 150, 150], 2);
        assert_eq!((w, h), (302, 42));
        assert_eq!(pos, vec![(0, 0), (152, 0), (76, 22)]);
        // A column wider than the buttons: sharing of the incomplete line.
        let (pos, w, h) = pack(&[100], 3);
        assert_eq!((pos, w, h), (vec![(0, 0)], 100, 20));
        assert_eq!(pack(&[], 2), (Vec::new(), 0, 0));
        let (pos, w, h) = column(&[(200, 17), (150, 20)]);
        assert_eq!((pos, w, h), (vec![(0, 0), (25, 27)], 200, 47));
        // Short content: 30 under the header; long: limited to the room between header and footer.
        assert_eq!(scroll_area(427, 240, 200, 47, 33), (103, 63, 220, 47));
        assert_eq!(scroll_area(427, 240, 200, 400, 33), (103, 33, 220, 174));
        assert_eq!(scroll_area(427, 240, 200, 400, 5), (103, 33, 220, 202));
    }

    #[test]
    fn clear_keeps_waiting_and_warning_like_the_game() {
        let screen = || Screen::new(parse(r#"{"type":"notice","title":"T"}"#).unwrap(), false);
        let warn = || Warn { panel: Panel::default(), width: -1, lines: Vec::new(), title: Vec::new() };
        // Dialog alone: closed.
        assert!(screen().clear());
        // Waiting for a response: it stays.
        let mut s = screen();
        s.start_wait();
        assert!(!s.clear());
        assert!(s.waiting.is_some());
        // Warning: it stays, "Back" then leads to the world.
        let mut s = screen();
        s.warning = Some(warn());
        assert!(!s.clear());
        assert!(s.warning.is_some());
        assert!(matches!(s.warning_back(), Outcome::Close));
        // Without clearing, "Back" returns to the dialog.
        let mut s = screen();
        s.warning = Some(warn());
        assert!(matches!(s.warning_back(), Outcome::Done));
        assert!(s.warning.is_none());
        // New dialog under the warning (even cleared): the warning stays and
        // "Back" leads to the new dialog.
        let mut s = screen();
        s.warning = Some(warn());
        s.clear();
        let mut next = screen();
        s.hand_over(&mut next);
        assert!(next.warning.is_some());
        assert!(matches!(next.warning_back(), Outcome::Done));
    }

    #[test]
    fn waiting_countdown() {
        let mut w = Wait::default();
        let mut seen = Vec::new();
        for t in 0..110 {
            w.tick();
            if t % 20 == 0 {
                seen.push((w.label, w.visible, w.active));
            }
        }
        // Visible from the 21st tick with "4", active at the 101st.
        assert_eq!(seen, vec![
            (0, false, false),
            (1, true, false),
            (2, true, false),
            (3, true, false),
            (4, true, false),
            (5, true, true)
        ]);
        assert_eq!(
            Wait { label: 1, ..Wait::default() }.label(),
            Component::translate("gui.waitingForResponse.button.inactive", vec![Arg::Text("4".into())])
        );
    }

    #[test]
    fn inputs_parse_like_the_game() {
        let d = parse(
            r#"{"type":"notice","title":"T","inputs":[
            {"type":"number_range","key":"n","label":"L","start":0,"end":4},
            {"type":"single_option","key":"o","label":"O","options":["a",{"id":"b","initial":true}]},
            {"type":"text","key":"m","label":"M","max_length":5,"multiline":{"max_lines":2}}]}"#,
        )
        .unwrap();
        let s = Screen::new(d, false);
        let v = s.values();
        assert_eq!((v[0].text.as_str(), v[0].nbt.clone()), ("2", Nbt::Float(2.0)));
        assert_eq!((v[1].text.as_str(), v[1].nbt.clone()), ("b", Nbt::String("b".into())));
        assert!(matches!(&s.dialog.inputs[2].control, Control::Text { multiline: Some((Some(2), None)), .. }));
        // Initial value longer than the limit, two initial options: rejected.
        assert!(parse(r#"{"type":"notice","title":"T","inputs":[{"type":"text","key":"k","label":"L","max_length":1,"initial":"ab"}]}"#).is_none());
        assert!(parse(r#"{"type":"notice","title":"T","inputs":[{"type":"single_option","key":"k","label":"L","options":[{"id":"a","initial":true},{"id":"b","initial":true}]}]}"#).is_none());
    }
}
