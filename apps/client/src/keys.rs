//! Rebindable keys and game inputs (spec `keys.md`): action → key table read from
//! `options.txt` (`key_key.*`, mouse included), "pressed" state per action with toggles
//! (sneak, sprint, attack, use), global keys (fullscreen, capture), discrete
//! wheel, mouse rotation and cinematic camera, frame rate outside the game.

use hashbrown::HashSet;
use std::time::{Duration, Instant};

use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::keyboard::{KeyCode, PhysicalKey};

use crate::App;
use crate::options::{KEYS, Options};

/// Tags of a registry: (registry, [(tag, ids)]).
type RegistryTags = (String, Vec<(String, Vec<i32>)>);

/// Physical key or mouse button (0 left, 1 right, 2 middle, 3+ side).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Input {
    Key(KeyCode),
    Mouse(u8),
}

// Indices of the assignments in `options::KEYS`.
pub const ATTACK: usize = 0;
pub const USE: usize = 1;
pub const FORWARD: usize = 2;
pub const LEFT: usize = 3;
pub const BACK: usize = 4;
pub const RIGHT: usize = 5;
pub const JUMP: usize = 6;
pub const SNEAK: usize = 7;
pub const SPRINT: usize = 8;
pub const DROP: usize = 9;
pub const INVENTORY: usize = 10;
pub const CHAT: usize = 11;
pub const PLAYERLIST: usize = 12;
pub const PICK: usize = 13;
pub const COMMAND: usize = 14;
pub const TOGGLE_GUI: usize = 17;
pub const SPECTATOR_SHADER: usize = 18;
pub const SCREENSHOT: usize = 19;
pub const PERSPECTIVE: usize = 20;
pub const SMOOTH_CAMERA: usize = 21;
pub const FULLSCREEN: usize = 22;
pub const SWAP: usize = 25;
pub const SAVE_TOOLBAR: usize = 26;
pub const LOAD_TOOLBAR: usize = 27;
pub const QUICK_ACTIONS: usize = 29;
pub const HOTBAR: usize = 32;
/// Game assignments tracked here (the F3 debug keys stay in `debug_keys.rs`).
const N: usize = 41;
const DEBUG_OVERLAY: usize = 30;
const DEBUG_MODIFIER: usize = 31;

/// Named key (`key.keyboard.w`, `key.mouse.left`, `key.mouse.4`…); `None` if free.
pub fn parse(name: &str) -> Option<Input> {
    if let Some(m) = name.strip_prefix("key.mouse.") {
        return match m {
            "left" => Some(Input::Mouse(0)),
            "right" => Some(Input::Mouse(1)),
            "middle" => Some(Input::Mouse(2)),
            n => n.parse::<u8>().ok().filter(|&n| n >= 1).map(|n| Input::Mouse(n - 1)),
        };
    }
    crate::keynames::key_code(name).map(Input::Key)
}

/// Button number (0 left, 1 right, 2 middle, 3 back, 4 forward, others as is).
pub fn mouse_index(b: MouseButton) -> Option<u8> {
    match b {
        MouseButton::Left => Some(0),
        MouseButton::Right => Some(1),
        MouseButton::Middle => Some(2),
        MouseButton::Back => Some(3),
        MouseButton::Forward => Some(4),
        MouseButton::Other(n) => u8::try_from(n).ok(),
    }
}

/// Smoothing of the cinematic camera (per axis): accumulated target, remaining to travel, inertia.
#[derive(Debug, Default, Clone, Copy)]
struct Smooth {
    target: f64,
    remaining: f64,
    last: f64,
}

impl Smooth {
    fn delta(&mut self, add: f64, time: f64) -> f64 {
        self.target += add;
        let mut d = self.target - self.remaining;
        let next = self.last + (d - self.last) * 0.5;
        let sign = d.signum();
        if sign * d > sign * self.last {
            d = next;
        }
        self.last = next;
        self.remaining += d * time;
        d * time
    }
}

/// State of the assignments: key of each action, pressed or not, toggles.
pub struct Binds {
    bound: [Option<Input>; N],
    /// Toggle mode (settings `toggleCrouch`, `toggleSprint`, `toggleAttack`, `toggleUse`).
    toggle: [bool; N],
    down: [bool; N],
    released_by_screen: [bool; N],
    /// Keys physically held down (all screens combined).
    phys: HashSet<KeyCode>,
    /// Cinematic camera (key `key.smoothCamera`, not saved).
    pub smooth_camera: bool,
    /// Aiming through the spyglass: rotation without the ×8 factor.
    turn: (f64, f64),
    smooth: (Smooth, Smooth),
    last_turn: Option<Instant>,
    focused: bool,
    last_frame: Instant,
    /// Dialogs of the `quick_actions` tag (indices of the `minecraft:dialog` registry).
    quick_actions: Vec<i32>,
    /// Ticks of the `RUSTCRAFT_KEYTEST` demo.
    test_tick: u32,
}

impl Default for Binds {
    fn default() -> Self {
        let now = Instant::now();
        Binds {
            bound: [None; N],
            toggle: [false; N],
            down: [false; N],
            released_by_screen: [false; N],
            phys: HashSet::new(),
            smooth_camera: false,
            turn: (0.0, 0.0),
            smooth: Default::default(),
            last_turn: None,
            // Without focus until the window has received it (`Focused(true)` event): the
            // raw mouse does not rotate the camera of a window left in the background.
            focused: false,
            last_frame: now,
            quick_actions: Vec::new(),
            test_tick: 0,
        }
    }
}

/// Assignments that can toggle and whether their state comes back when a screen closes.
fn toggle_kind(i: usize) -> Option<(&'static str, bool)> {
    match i {
        SNEAK => Some(("toggleCrouch", true)),
        SPRINT => Some(("toggleSprint", true)),
        ATTACK => Some(("toggleAttack", true)),
        USE => Some(("toggleUse", false)),
        _ => None,
    }
}

impl Binds {
    /// Rereads the table from the settings (hot: the keys screen writes into `Options`).
    pub fn sync(&mut self, o: &Options) {
        for i in 0..N {
            let name = o.s(&KEY_OPTIONS[i]);
            let inp = if i == DEBUG_OVERLAY || i == DEBUG_MODIFIER { None } else { parse(name) };
            if self.bound[i] != inp {
                self.bound[i] = inp;
                self.down[i] = false;
            }
            self.toggle[i] = toggle_kind(i).is_some_and(|(opt, _)| o.b(opt));
        }
    }

    /// The input is that of action `i` (free key: never).
    pub fn is(&self, i: usize, inp: Input) -> bool {
        self.bound[i] == Some(inp)
    }

    /// The action is held down (or activated, in toggle mode).
    pub fn down(&self, i: usize) -> bool {
        self.down[i]
    }

    /// Held actions, bit `i` = `options::KEYS[i]` (debug keys included: never).
    pub fn down_mask(&self) -> u64 {
        self.down.iter().enumerate().fold(0, |m, (i, &d)| m | u64::from(d) << i)
    }

    /// Keyboard-less demos: forces the state of an action.
    pub fn force(&mut self, i: usize, on: bool) {
        self.down[i] = on;
    }

    /// Actions assigned to this input (several in case of a conflict: all act).
    pub fn matching(&self, inp: Input) -> impl Iterator<Item = usize> + '_ {
        (0..N).filter(move |&i| self.bound[i] == Some(inp))
    }

    fn set_down(&mut self, i: usize, on: bool) {
        if self.toggle[i] {
            if on {
                self.down[i] = !self.down[i];
            }
        } else {
            self.down[i] = on;
        }
    }

    /// Screen open, mouse released: nothing is held any more; an active toggle remembers it.
    pub fn release_all(&mut self) {
        for i in 0..N {
            if toggle_kind(i).is_some() && ((self.toggle[i] && self.down[i]) || self.released_by_screen[i]) {
                self.released_by_screen[i] = true;
            }
            self.down[i] = false;
        }
    }

    /// Back to the game: toggles restored, keyboard keys still held taken back.
    pub fn resume(&mut self) {
        for i in 0..N {
            if let Some((_, restore)) = toggle_kind(i) {
                let back = restore
                    && self.toggle[i]
                    && matches!(self.bound[i], Some(Input::Key(_)))
                    && self.released_by_screen[i];
                self.released_by_screen[i] = false;
                if back {
                    self.down[i] = true;
                }
            }
            if !self.toggle[i]
                && let Some(Input::Key(k)) = self.bound[i]
            {
                self.down[i] = self.phys.contains(&k);
            }
        }
    }

    /// Respawn: toggles reset to zero.
    pub fn reset_toggles(&mut self) {
        for i in 0..N {
            if toggle_kind(i).is_some() {
                self.down[i] = false;
            }
        }
    }

    /// Tags received: quick action dialogs.
    pub fn set_tags(&mut self, tags: &[RegistryTags]) {
        self.quick_actions.clear();
        if let Some((_, list)) = tags.iter().find(|(r, _)| r == "minecraft:dialog")
            && let Some((_, ids)) = list.iter().find(|(n, _)| n == "minecraft:quick_actions" || n == "quick_actions")
        {
            self.quick_actions = ids.clone();
        }
    }
}

/// Option names `key_<assignment>` (prepared once: no allocation per tick).
static KEY_OPTIONS: std::sync::LazyLock<Vec<String>> =
    std::sync::LazyLock::new(|| KEYS.iter().map(|k| format!("key_{}", k.0)).collect());

impl App {
    /// On every tick: table reread (hot reassignment), finished captures announced.
    pub(crate) fn keys_tick(&mut self) {
        self.binds.sync(&self.options);
        self.player.auto_jump = self.options.b("autoJump");
        self.player.sprint_window = self.options.i("sprintWindow").clamp(0, 10) as u32;
        self.screenshot_poll();
        self.keytest_tick();
    }

    /// Keyboard-less demo: `RUSTCRAFT_KEYTEST=<tick>:<+|->:<key>,…` (key in the
    /// options.txt format, `+` press, `-` release), ticks counted in game; presses
    /// go through the global keys then through the game, like the keyboard.
    fn keytest_tick(&mut self) {
        let Some(spec) = rc_data::env_os!("RUSTCRAFT_KEYTEST") else { return };
        if !self.positioned {
            return;
        }
        self.binds.test_tick += 1;
        let t = self.binds.test_tick;
        for step in spec.to_string_lossy().split(',') {
            let p: Vec<&str> = step.splitn(3, ':').collect();
            let [at, dir, name] = p[..] else { continue };
            if at.parse::<u32>().ok() != Some(t) {
                continue;
            }
            let Some(inp) = parse(name) else { continue };
            eprintln!("keys demo: {step}");
            let press = dir == "+";
            if let Input::Key(k) = inp {
                if press {
                    self.binds.phys.insert(k);
                } else {
                    self.binds.phys.remove(&k);
                }
            }
            if press && self.global_input(inp) {
                continue;
            }
            if press && !self.grabbed {
                self.grabbed = true;
            }
            self.game_input(inp, press);
        }
    }

    /// Any event, before the screens: physical keys, focus, activity, then global
    /// keys (fullscreen, capture); true if the event is consumed.
    pub(crate) fn keys_observe(&mut self, event: &WindowEvent) -> bool {
        match event {
            WindowEvent::KeyboardInput { event, .. } => {
                self.video.limiter.input();
                if let PhysicalKey::Code(k) = event.physical_key {
                    if event.state == ElementState::Pressed {
                        self.binds.phys.insert(k);
                        if !event.repeat {
                            return self.global_input(Input::Key(k));
                        }
                    } else {
                        self.binds.phys.remove(&k);
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                self.video.limiter.input();
                if *state == ElementState::Pressed
                    && let Some(b) = mouse_index(*button)
                {
                    return self.global_input(Input::Mouse(b));
                }
            }
            WindowEvent::MouseWheel { .. } | WindowEvent::CursorMoved { .. } => {
                self.video.limiter.input();
            }
            WindowEvent::Focused(f) => {
                self.binds.focused = *f;
                if !*f {
                    self.binds.phys.clear();
                }
            }
            _ => {}
        }
        false
    }

    /// Global keys (all screens, except F3 held): fullscreen and screenshot.
    fn global_input(&mut self, inp: Input) -> bool {
        if self.debug.modifier_down {
            return false;
        }
        if self.binds.is(FULLSCREEN, inp) {
            let full = !self.options.b("fullscreen");
            self.options.set("fullscreen", crate::options::Val::B(full));
            self.save_options();
            self.apply_options();
            return true;
        }
        if self.binds.is(SCREENSHOT, inp) {
            self.take_screenshot();
            return true;
        }
        false
    }

    /// Window closing: refused if Alt+F4 is held and `quitShortcuts` disabled.
    pub(crate) fn quit_allowed(&self) -> bool {
        self.options.b("quitShortcuts") || !(self.mods.alt_key() && self.binds.phys.contains(&KeyCode::F4))
    }

    /// Mouse captured or released (screen open): state of the actions (§ 1.3).
    pub(crate) fn keys_grab(&mut self, on: bool) {
        if on {
            self.binds.resume();
        } else {
            self.binds.release_all();
        }
        self.sync_act_buttons();
    }

    /// Attack, use and pick-block buttons according to the state of the actions.
    fn sync_act_buttons(&mut self) {
        for (b, i) in [ATTACK, USE, PICK].into_iter().enumerate() {
            if !self.binds.down(i) {
                self.act.release(b);
            }
        }
    }

    /// Keyboard key in game (no screen, input closed): Esc, then the actions.
    pub(crate) fn game_keyboard(&mut self, event: &KeyEvent) {
        let PhysicalKey::Code(k) = event.physical_key else { return };
        let pressed = event.state == ElementState::Pressed;
        if pressed && k == KeyCode::Escape && !event.repeat {
            // Esc with no screen: pause menu (§ 2.1), no packet sent.
            self.open_pause(true);
            return;
        }
        // Free debug camera (specific to Rustcraft): only with RUSTCRAFT_FREECAM=1.
        if pressed && k == KeyCode::F7 && !event.repeat && rc_data::env_flag!("RUSTCRAFT_FREECAM") {
            self.free_cam = !self.free_cam;
        }
        if pressed {
            self.keys.insert(k);
        } else {
            self.keys.remove(&k);
        }
        self.game_input(Input::Key(k), pressed);
    }

    /// Mouse button in game (mouse captured).
    pub(crate) fn game_mouse(&mut self, state: ElementState, button: MouseButton) {
        if let Some(b) = mouse_index(button) {
            self.game_input(Input::Mouse(b), state == ElementState::Pressed);
        }
    }

    /// Press (repeats included) or release of an input in game: all the assigned actions
    /// change state; a press triggers each one.
    fn game_input(&mut self, inp: Input, pressed: bool) {
        let mut hit = [false; N];
        for i in self.binds.matching(inp) {
            hit[i] = true;
        }
        for (i, &h) in hit.iter().enumerate() {
            if !h {
                continue;
            }
            self.binds.set_down(i, pressed);
            if pressed {
                self.game_action(i);
            }
        }
        self.sync_act_buttons();
    }

    /// An action triggered by its key (§ 2).
    fn game_action(&mut self, i: usize) {
        match i {
            ATTACK | USE | PICK => {
                let b = [ATTACK, USE, PICK].iter().position(|&x| x == i).unwrap_or(0);
                if self.grabbed {
                    self.act.press(b);
                }
            }
            DROP if self.grabbed => self.act.drop_key(self.mods.control_key()),
            SWAP if self.grabbed => self.act.swap_key(),
            TOGGLE_GUI => self.hud.hidden = !self.hud.hidden,
            INVENTORY if self.env.game_mode != 3 => {
                // Vehicle with inventory: screen requested from the server (`mounts.md` § 1.6).
                if self.vehicle_inventory() || self.ride_inventory() {
                    return;
                }
                // Inventory: local opening, no packet (§ 3).
                self.open_inventory();
                self.keys.clear();
                self.grab(false);
            }
            CHAT | COMMAND => {
                self.chat.open(if i == COMMAND { "/" } else { "" });
                self.commands_refresh();
                self.cmd.open(&mut self.chat);
                self.commands_send();
                self.keys.clear();
                self.grab(false);
            }
            SPECTATOR_SHADER if !self.debug.modifier_down => self.misc.toggle_effect(),
            PERSPECTIVE => self.camera.view = self.camera.view.next(),
            SMOOTH_CAMERA => self.binds.smooth_camera = !self.binds.smooth_camera,
            QUICK_ACTIONS => self.quick_actions(),
            h if (HOTBAR..HOTBAR + 9).contains(&h) => {
                let slot = h - HOTBAR;
                // Spectator: menu keys (`spectator.rs`), otherwise hotbar slot.
                if self.env.game_mode == 3 {
                    self.spectator_hotbar(slot);
                } else if !self.hotbar_activator(slot) {
                    self.hud.selected = slot;
                }
            }
            _ => {}
        }
    }

    /// Quick actions (G): the tag's dialog if it is alone, otherwise the list of actions.
    fn quick_actions(&mut self) {
        let id = match self.binds.quick_actions.as_slice() {
            [] => return,
            [one] => *one,
            _ => match self.exact.dialogs.iter().position(|(n, _)| n == "minecraft:quick_actions") {
                Some(i) => i as i32,
                None => return,
            },
        };
        self.show_dialog(rc_client::extra::DialogRef::Registry(id), false);
    }

    /// Key in a container screen: close, swap with the hotbar, drop.
    /// Returns (hotbar slot or 40 for the offhand, drop with Ctrl, close).
    pub(crate) fn screen_key_actions(&self, k: KeyCode) -> (Option<i8>, Option<bool>, bool) {
        let inp = Input::Key(k);
        let close = k == KeyCode::Escape || self.binds.is(INVENTORY, inp);
        let digit = if self.binds.is(SWAP, inp) {
            Some(40)
        } else {
            (0..9).find(|&i| self.binds.is(HOTBAR + i, inp)).map(|i| i as i8)
        };
        let drop = self.binds.is(DROP, inp).then_some(self.mods.control_key());
        (digit, drop, close)
    }

    /// Wheel in game (§ 3.6): discrete mode, sensitivity, accumulation; hotbar or flight speed.
    pub(crate) fn game_scroll(&mut self, delta: MouseScrollDelta) {
        let (x, y) = match delta {
            MouseScrollDelta::LineDelta(x, y) => (f64::from(x), f64::from(y)),
            MouseScrollDelta::PixelDelta(p) => (p.x / 20.0, p.y / 20.0),
        };
        let k = self.options.f("mouseWheelSensitivity");
        let discrete = self.options.b("discrete_mouse_scroll");
        let (x, y) = if discrete { (x.signum() * k, y.signum() * k) } else { (x * k, y * k) };
        // Zero sign: zero (the sign of 0.0 in Rust is 1).
        let (x, y) = (if x == 0.0 { 0.0 } else { x }, if y == 0.0 { 0.0 } else { y });
        let (wx, wy) = self.hud.wheel(x, y);
        if wx == 0 && wy == 0 {
            return;
        }
        let wheel = if wy == 0 { -wx } else { wy };
        if self.env.game_mode == 3 {
            // Spectator: menu open, next slot; otherwise flight speed (vertical axis only).
            if self.spectator.active() {
                self.spectator.scroll(-wheel);
            } else {
                self.player.fly_speed = Self::spectator_fly_speed(self.player.fly_speed, wy);
            }
            return;
        }
        self.hud.scroll_select(wheel);
    }

    /// Spectator flight speed after `wy` notches: ± 0.005 per notch, between 0 and 0.2.
    pub(crate) fn spectator_fly_speed(speed: f32, wy: i32) -> f32 {
        (speed + wy as f32 * 0.005).clamp(0.0, 0.2)
    }

    /// Raw mouse movement: accumulated, applied at the frame (`mouse_frame`).
    pub(crate) fn mouse_motion(&mut self, dx: f64, dy: f64) {
        if self.grabbed {
            self.binds.turn.0 += dx;
            self.binds.turn.1 += dy;
            self.video.limiter.input();
        }
    }

    /// Mouse rotation (§ 3.9), once per frame: sensitivity, cinematic camera,
    /// spyglass, inversion; 0.15° per unit.
    pub(crate) fn mouse_frame(&mut self) {
        let now = Instant::now();
        let time = self.binds.last_turn.map_or(0.0, |t| now.duration_since(t).as_secs_f64());
        self.binds.last_turn = Some(now);
        let (ax, ay) = std::mem::take(&mut self.binds.turn);
        if !self.grabbed || !self.binds.focused {
            return;
        }
        let sens = self.options.mouse_factor();
        let cube = sens / 8.0;
        let (xo, yo) = if self.binds.smooth_camera {
            let (sx, sy) = &mut self.binds.smooth;
            (sx.delta(ax * sens, time * sens), sy.delta(ay * sens, time * sens))
        } else {
            self.binds.smooth = Default::default();
            let k = if self.scoping_first_person() { cube } else { sens };
            (ax * k, ay * k)
        };
        if xo == 0.0 && yo == 0.0 {
            return;
        }
        let xo = if self.options.b("invertXMouse") { -xo } else { xo };
        let yo = if self.options.b("invertYMouse") { -yo } else { yo };
        self.yaw += (xo * 0.15) as f32;
        self.pitch = (self.pitch + (yo * 0.15) as f32).clamp(-90.0, 90.0);
        self.clamp_boat_view();
    }

    /// After the events: next frame right away, or at the limit's deadline.
    pub(crate) fn pace_frames(&mut self, el: &ActiveEventLoop) {
        let Some(r) = &self.renderer else { return };
        let Some(fps) = self.fps_limit() else {
            el.set_control_flow(ControlFlow::Poll);
            r.window.request_redraw();
            crate::present_watch::enter(crate::present_watch::Stage::AwaitRedraw);
            return;
        };
        let next = self.binds.last_frame + Duration::from_secs_f64(1.0 / f64::from(fps));
        let now = Instant::now();
        if now >= next {
            self.binds.last_frame = now;
            el.set_control_flow(ControlFlow::Poll);
            r.window.request_redraw();
            crate::present_watch::enter(crate::present_watch::Stage::AwaitRedraw);
        } else {
            el.set_control_flow(ControlFlow::WaitUntil(next));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indices_match_option_table() {
        for (i, name) in [
            (ATTACK, "key.attack"),
            (USE, "key.use"),
            (FORWARD, "key.forward"),
            (LEFT, "key.left"),
            (BACK, "key.back"),
            (RIGHT, "key.right"),
            (JUMP, "key.jump"),
            (SNEAK, "key.sneak"),
            (SPRINT, "key.sprint"),
            (DROP, "key.drop"),
            (INVENTORY, "key.inventory"),
            (CHAT, "key.chat"),
            (PLAYERLIST, "key.playerlist"),
            (PICK, "key.pickItem"),
            (COMMAND, "key.command"),
            (TOGGLE_GUI, "key.toggleGui"),
            (SPECTATOR_SHADER, "key.toggleSpectatorShaderEffects"),
            (SCREENSHOT, "key.screenshot"),
            (PERSPECTIVE, "key.togglePerspective"),
            (SMOOTH_CAMERA, "key.smoothCamera"),
            (FULLSCREEN, "key.fullscreen"),
            (SWAP, "key.swapOffhand"),
            (SAVE_TOOLBAR, "key.saveToolbarActivator"),
            (LOAD_TOOLBAR, "key.loadToolbarActivator"),
            (QUICK_ACTIONS, "key.quickActions"),
            (DEBUG_OVERLAY, "key.debug.overlay"),
            (DEBUG_MODIFIER, "key.debug.modifier"),
            (HOTBAR, "key.hotbar.1"),
            (N - 1, "key.hotbar.9"),
        ] {
            assert_eq!(KEYS[i].0, name);
        }
    }

    #[test]
    fn names_round_trip() {
        for &k in crate::keynames::CODES {
            assert_eq!(parse(crate::keynames::key_name(k)), Some(Input::Key(k)), "{k:?}");
        }
        assert_eq!(parse("key.mouse.left"), Some(Input::Mouse(0)));
        assert_eq!(parse("key.mouse.4"), Some(Input::Mouse(3)));
        assert_eq!(parse(&crate::options::mouse_name(5)), Some(Input::Mouse(5)));
        assert_eq!(parse("key.keyboard.unknown"), None);
    }

    fn binds(extra: &str) -> Binds {
        let mut b = Binds::default();
        b.sync(&Options::parse(extra));
        b
    }

    #[test]
    fn rebinding_conflicts_and_toggles() {
        let b = binds("key_key.forward:key.keyboard.up\nkey_key.jump:key.keyboard.w");
        assert!(b.is(FORWARD, Input::Key(KeyCode::ArrowUp)));
        assert!(!b.is(FORWARD, Input::Key(KeyCode::KeyW)));
        // W: jump alone; two actions on the same key both act.
        let b = binds("key_key.drop:key.keyboard.f");
        let both: Vec<_> = b.matching(Input::Key(KeyCode::KeyF)).collect();
        assert_eq!(both, vec![DROP, SWAP]);
        let mut b = binds("toggleCrouch:true\nkey_key.attack:key.mouse.5");
        assert!(b.is(ATTACK, Input::Mouse(4)));
        b.set_down(SNEAK, true);
        b.set_down(SNEAK, false);
        assert!(b.down(SNEAK), "toggle: the release changes nothing");
        // Screen opened then closed: the toggle comes back, a simple key follows the keyboard.
        b.release_all();
        assert!(!b.down(SNEAK));
        b.phys.insert(KeyCode::KeyW);
        b.resume();
        assert!(b.down(SNEAK) && b.down(FORWARD) && !b.down(JUMP));
        b.set_down(SNEAK, true);
        assert!(!b.down(SNEAK));
    }

    #[test]
    fn smooth_camera_eases_and_settles() {
        let mut s = Smooth::default();
        let first = s.delta(10.0, 0.016);
        let mut total = first;
        for _ in 0..400 {
            total += s.delta(0.0, 0.016);
        }
        assert!(first < 10.0 * 0.016, "damped start {first}");
        assert!((total - 10.0).abs() < 0.5, "almost reaches the target {total}");
    }
}
