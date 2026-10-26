//! Special screens (`docs/specs/special-screens.md`): dispatch between the common container screen
//! (`screens.rs`) and the dedicated screens (trading, mounts, workstation blocks, books,
//! signs, operator screens), item drawing helpers and demo driver.

use rc_proto_v777::item::ItemStack;
use rc_text::StyledChar;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::keyboard::KeyCode;

use crate::gui::{Blend, Gui};
use crate::hud::HotbarItems;
use crate::inventory::PlayerInventory;
use crate::menu::Kind;
use crate::menu_special::Sp;

/// State specific to an open special container.
#[derive(Default)]
pub struct Extra {
    pub trade: crate::screens_trade::Trade,
    pub work: crate::screens_work::Work,
}

/// Screen outside a container (or lectern) opened over the game.
pub enum Overlay {
    Book(Box<crate::book::Book>),
    Sign(Box<crate::sign_edit::SignEdit>),
    Op(Box<crate::op_screens::OpScreen>),
}

/// Request for a 3D model in the interface, collected at draw time, pushed to the entity renderer.
#[derive(Debug, Clone)]
pub enum ModelReq {
    /// Known entity: interface box, size, vertical offset, mouse.
    Entity { id: i32, rect: [f32; 4], size: f32, offset: f32, mouse: (f32, f32) },
    /// Enchanting table book: box, opening, pages (`special-screens.md` § 3.7).
    Book { rect: [f32; 4], open: f32, flip: [f32; 2] },
    /// Smithing mannequin (§ 3.6): equipment (entity indices: 1 left hand, 2 feet,
    /// 3 legs, 4 chest, 5 head).
    Stand { rect: [f32; 4], equipment: Vec<(usize, ItemStack)> },
}

#[derive(Default)]
pub struct Special {
    pub overlay: Option<Overlay>,
    /// Known command blocks: command, output, always active, output tracking.
    pub op_known: hashbrown::HashMap<[i32; 3], (String, String, bool, bool)>,
    test: GuiTest,
}

/// Models requested by the drawing of the interface of the current frame.
static MODELS: std::sync::Mutex<Vec<ModelReq>> = std::sync::Mutex::new(Vec::new());

/// Request for a 3D model at this place of the interface (above what precedes).
pub fn request_model(gui: &mut Gui, req: ModelReq) {
    gui.models_here();
    let mut m = MODELS.lock().unwrap_or_else(|p| p.into_inner());
    // Without an entity renderer, the requests are never consumed: bound.
    if m.len() < 8 {
        m.push(req);
    }
}

/// Root matrix of an interface model (`inventories.md` § 5.1): interface frame
/// (x to the right, y upward = −row, z toward the screen) ← pose frame of the official screen.
fn gui_root(cx: f32, cy: f32, scale: f32, lift: f32, rotation: glam::Mat4) -> glam::Mat4 {
    use glam::{Mat4, Vec3};
    Mat4::from_scale(Vec3::new(1.0, -1.0, 1.0))
        * Mat4::from_translation(Vec3::new(cx, cy, 50.0))
        * Mat4::from_scale(Vec3::new(scale, scale, -scale))
        * Mat4::from_translation(Vec3::new(0.0, lift, 0.0))
        * rotation
}

/// Origin of the enchanting book in its frame: middle in x, 17 px below the top (not the
/// middle of the frame, as for entities).
fn book_origin(rect: [f32; 4]) -> (f32, f32) {
    ((rect[0] + rect[2]) / 2.0, rect[1] + 17.0)
}

#[cfg(test)]
#[test]
fn book_sits_17_below_the_frame_top() {
    // Enchanting screen frame: 38 × 31 at (x+14, y+14); old center y = 79.5.
    assert_eq!(book_origin([114.0, 64.0, 152.0, 95.0]), (133.0, 81.0));
}

/// Lights of the interface models (entities, enchanting book): the game's,
/// (0.2; −1; 1) and (−0.2; −1; 0) normalized, are given in its pose frame; our
/// root adds a mirror in y in front of this pose (see [`gui_root`]) and the normals follow
/// the inverse transpose, hence the same lights passed through this mirror.
pub(crate) fn entity_in_ui_lights() -> [[f32; 4]; 2] {
    let n = |x: f32, y: f32, z: f32| {
        let v = glam::Vec3::new(x, -y, z).normalize();
        [v.x, v.y, v.z, 0.0]
    };
    [n(0.2, -1.0, 1.0), n(-0.2, -1.0, 0.0)]
}

/// Id reserved for the smithing mannequin (never assigned by the server).
const STAND_ID: i32 = i32::MIN + 7;

/// Pushes to the entity renderer the models requested by the frame (after the hand).
pub fn push_models(
    er: &mut crate::entity_render::EntityRenderer,
    all: &mut crate::entities::Entities,
    items: Option<&rc_assets::item::ItemModels>,
    pt: f32,
) {
    use glam::Mat4;
    let reqs = std::mem::take(&mut *MODELS.lock().unwrap_or_else(|p| p.into_inner()));
    if reqs.is_empty() {
        return;
    }
    er.begin_gui();
    for r in reqs {
        match r {
            ModelReq::Entity { id, rect, size, offset, mouse } => {
                // Local player: outside the table, in `me` (inventory screen).
                let e = if id == all.local { all.me.as_mut() } else { all.map.get_mut(&id) };
                let Some(e) = e else { continue };
                let (cx, cy) = ((rect[0] + rect[2]) / 2.0, (rect[1] + rect[3]) / 2.0);
                let xa = ((cx - mouse.0) / 40.0).atan();
                let ya = ((cy - mouse.1) / 40.0).atan();
                let saved = (e.body, e.body_o, e.head, e.head_o, e.yaw, e.yaw_o, e.pitch, e.pitch_o);
                let body = 180.0 + xa * 20.0;
                let head = 180.0 + xa * 40.0;
                (e.body, e.body_o) = (body, body);
                (e.head, e.head_o, e.yaw, e.yaw_o) = (head, head, head, head);
                (e.pitch, e.pitch_o) = (-ya * 20.0, -ya * 20.0);
                let scale_e = if e.baby() { 0.5 } else { 1.0 };
                // Height of the box (not that of the eyes): center of the model in the frame.
                let h = e.dims(&er.dims)[0];
                let rot = Mat4::from_rotation_z(std::f32::consts::PI) * Mat4::from_rotation_x((ya * 20.0).to_radians());
                let root = gui_root(cx, cy, size / scale_e, h / 2.0 + offset * scale_e, rot);
                er.push_gui_entity(e, root, items);
                (e.body, e.body_o, e.head, e.head_o, e.yaw, e.yaw_o, e.pitch, e.pitch_o) = saved;
                let _ = pt;
            }
            ModelReq::Book { rect, open, flip } => {
                let (cx, cy) = book_origin(rect);
                let o = 1.0 - open;
                let rot = Mat4::from_rotation_y(std::f32::consts::PI)
                    * Mat4::from_rotation_x(25f32.to_radians())
                    * Mat4::from_translation(glam::Vec3::new(o * 0.2, o * 0.1, o * 0.25))
                    * Mat4::from_rotation_y((-o * 90.0 - 90.0).to_radians())
                    * Mat4::from_rotation_x(std::f32::consts::PI);
                let root = gui_root(cx, cy, 40.0, 0.0, rot);
                er.push_gui_book(root, open, flip[0], flip[1]);
            }
            ModelReq::Stand { rect, equipment } => {
                // Temporary armor stand (no base, with arms), body oriented at 210°.
                let kind = all
                    .type_names
                    .iter()
                    .position(|n| n.strip_prefix("minecraft:").unwrap_or(n) == "armor_stand")
                    .map_or(-1, |i| i as i32);
                let add = rc_proto_v777::entity::AddEntity {
                    id: STAND_ID,
                    uuid: 0,
                    kind,
                    pos: [0.0; 3],
                    velocity: [0.0; 3],
                    pitch: 0.0,
                    yaw: 210.0,
                    head_yaw: 210.0,
                    data: 0,
                };
                all.apply(rc_client::EntityEvent::Add(Box::new(add)));
                let Some(e) = all.map.get_mut(&STAND_ID) else { continue };
                // Stand flags (index 15): 4 arms, 8 no base.
                e.meta.insert(15, rc_proto_v777::entity::Meta::Byte(4 | 8));
                for (slot, st) in equipment {
                    if let Some(s) = e.equipment.get_mut(slot) {
                        *s = Some(st);
                    }
                }
                (e.body, e.body_o, e.head, e.head_o, e.yaw, e.yaw_o) = (210.0, 210.0, 210.0, 210.0, 210.0, 210.0);
                let (cx, cy) = ((rect[0] + rect[2]) / 2.0, (rect[1] + rect[3]) / 2.0);
                let rot = Mat4::from_rotation_x(0.436_332_3) * Mat4::from_rotation_z(std::f32::consts::PI);
                let root = gui_root(cx, cy, 25.0, 1.0, rot);
                er.push_gui_entity(e, root, items);
                all.map.remove(&STAND_ID);
            }
        }
    }
}

impl Special {
    /// Drawing of the screens outside a container (after the container screen).
    pub fn draw(
        &mut self,
        gui: &mut Gui,
        queue: &wgpu::Queue,
        it: &HotbarItems<'_>,
        mouse: Option<(f32, f32)>,
        time: f64,
    ) {
        match &mut self.overlay {
            Some(Overlay::Book(b)) => b.draw(gui, queue, it, mouse, time),
            Some(Overlay::Sign(s)) => s.draw(gui, queue, time),
            Some(Overlay::Op(o)) => o.draw(gui, queue, mouse, time),
            None => {}
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Drawing helpers.

/// Color of the screen titles (no shadow).
pub const TITLE_COLOR: u32 = 0xff40_4040;

pub fn chars(gui: &Gui, c: &rc_text::Component) -> Vec<StyledChar> {
    rc_text::styled_chars(&rc_text::flatten(c, &gui.lang))
}

pub fn tr(gui: &Gui, key: &str) -> Vec<StyledChar> {
    chars(gui, &rc_text::Component::translate(key, vec![]))
}

/// Item of a stack at `(x, y)` (item atlas render).
pub fn item(gui: &mut Gui, it: &HotbarItems<'_>, st: &ItemStack, x: f32, y: f32) {
    use std::hash::{Hash, Hasher};
    let Some(models) = gui.items.as_ref().map(|a| &a.models) else { return };
    let info = usize::try_from(st.item).ok().and_then(|i| it.infos.get(i));
    let name = usize::try_from(st.item).ok().and_then(|i| it.names.get(i)).map_or("", String::as_str);
    let view = crate::stack::StackRef { stack: st, info, ids: it.ids, data: it.data, hold: crate::stack::Hold::NONE };
    let (mut layers, mut key) = (Vec::new(), Vec::new());
    if let Some(model) = view.model() {
        models.layers(name, &model, &view, it.env, (x as i32) ^ ((y as i32) << 10), &mut layers, &mut key);
    }
    if layers.is_empty() {
        return;
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    gui.stratum(Blend::Premultiplied);
    gui.item(h.finish(), &layers, x, y, 0.0);
    gui.stratum(Blend::Normal);
}

/// Decorations of a stack: wear bar and quantity (`force`: quantity even if 1).
pub fn decorations(
    gui: &mut Gui,
    queue: &wgpu::Queue,
    it: &HotbarItems<'_>,
    st: &ItemStack,
    x: f32,
    y: f32,
    force: bool,
) {
    let info = usize::try_from(st.item).ok().and_then(|i| it.infos.get(i));
    let view = crate::stack::StackRef { stack: st, info, ids: it.ids, data: it.data, hold: crate::stack::Hold::NONE };
    if let Some((w, color)) = view.durability_bar() {
        gui.fill(x + 2.0, y + 13.0, x + 15.0, y + 15.0, 0xff00_0000);
        gui.fill(x + 2.0, y + 13.0, x + 2.0 + w as f32, y + 14.0, 0xff00_0000 | color);
    }
    crate::hud::cooldown_veil(gui, st, x, y);
    if st.count != 1 || force {
        let chars = crate::gui::plain(&st.count.to_string());
        let tw = rc_text::width(gui, &chars);
        gui.text(queue, &chars, x + 17.0 - tw as f32, y + 9.0, 0xffff_ffff, true, 0);
    }
}

/// Item and decorations.
pub fn stack(gui: &mut Gui, queue: &wgpu::Queue, it: &HotbarItems<'_>, st: &ItemStack, x: i32, y: i32) {
    item(gui, it, st, x as f32, y as f32);
    decorations(gui, queue, it, st, x as f32, y as f32, false);
}

/// Tooltip of a stack near the mouse.
pub fn item_tooltip(
    gui: &mut Gui,
    queue: &wgpu::Queue,
    it: &HotbarItems<'_>,
    enchantments: &[String],
    st: &ItemStack,
    m: (f32, f32),
) {
    let lines = crate::screens::tooltip_lines(st, it, enchantments, gui, crate::screens::advanced());
    let w = lines.iter().map(|l| rc_text::width(gui, l)).max().unwrap_or(0);
    let info = usize::try_from(st.item).ok().and_then(|i| it.infos.get(i));
    let view = crate::stack::StackRef { stack: st, info, ids: it.ids, data: it.data, hold: crate::stack::Hold::NONE };
    let style = crate::screens::tooltip_style(&view);
    let m = (m.0.floor() as i32, m.1.floor() as i32);
    crate::screens::draw_tooltip_ex(gui, queue, &lines, w, m, true, style.as_deref());
}

/// Plain-text tooltip near the mouse (lines at a step of 10, no gap under the 1st).
pub fn text_tooltip(gui: &mut Gui, queue: &wgpu::Queue, lines: &[Vec<StyledChar>], m: (f32, f32)) {
    let w = lines.iter().map(|l| rc_text::width(gui, l)).max().unwrap_or(0);
    crate::screens::draw_text_tooltip(gui, queue, lines, w, m.0.floor() as i32, m.1.floor() as i32);
}

pub fn inside(m: Option<(f32, f32)>, x: i32, y: i32, w: i32, h: i32) -> bool {
    m.is_some_and(|(mx, my)| mx >= x as f32 && mx < (x + w) as f32 && my >= y as f32 && my < (y + h) as f32)
}

// ---------------------------------------------------------------------------------------------
// Hooks of `screens.rs`.

/// Drawing context of a special container.
pub struct Ctx<'a, 'b> {
    pub gui: &'a mut Gui,
    pub queue: &'a wgpu::Queue,
    pub inv: &'a PlayerInventory,
    pub it: &'a HotbarItems<'b>,
    pub gx: i32,
    pub gy: i32,
    pub mouse: Option<(f32, f32)>,
    pub enchantments: &'a [String],
}

/// The lectern has no container screen: the reading screen replaces it.
pub fn hides_container(kind: Kind) -> bool {
    matches!(kind, Kind::Special(Sp::Lectern))
}

/// Background and elements under the slots (titles included) of a special container.
#[allow(clippy::too_many_arguments)]
pub fn draw_back(
    gui: &mut Gui,
    queue: &wgpu::Queue,
    inv: &PlayerInventory,
    it: &HotbarItems<'_>,
    (gx, gy): (i32, i32),
    mouse: Option<(f32, f32)>,
    enchantments: &[String],
) {
    let Some(c) = &inv.container else { return };
    let Kind::Special(s) = c.kind else { return };
    let mut cx = Ctx { gui, queue, inv, it, gx, gy, mouse, enchantments };
    match s {
        Sp::Merchant => crate::screens_trade::draw_back(&mut cx, c),
        Sp::Mount(m) => crate::screens_mount::draw_back(&mut cx, c, m),
        _ => crate::screens_work::draw_back(&mut cx, c, s),
    }
}

/// Tooltips and elements above the slots.
#[allow(clippy::too_many_arguments)]
pub fn draw_front(
    gui: &mut Gui,
    queue: &wgpu::Queue,
    inv: &PlayerInventory,
    it: &HotbarItems<'_>,
    (gx, gy): (i32, i32),
    mouse: Option<(f32, f32)>,
    enchantments: &[String],
) {
    let Some(c) = &inv.container else { return };
    let Kind::Special(s) = c.kind else { return };
    // Item tooltip already displayed (non-empty hovered slot): nothing more.
    let mut cx = Ctx { gui, queue, inv, it, gx, gy, mouse, enchantments };
    match s {
        Sp::Merchant => crate::screens_trade::draw_front(&mut cx, c),
        Sp::Mount(_) => {}
        _ => crate::screens_work::draw_front(&mut cx, c, s),
    }
}

/// Title and "Inventory" of a special container (common color, no shadow).
pub fn titles(
    cx: &mut Ctx<'_, '_>,
    c: &crate::inventory::Container,
    title_at: Option<(i32, i32)>,
    label_at: Option<(i32, i32)>,
) {
    let (gx, gy) = (cx.gx, cx.gy);
    if let Some((x, y)) = title_at {
        let t = c.title.clone().unwrap_or_else(|| rc_text::Component::text(""));
        let t = chars(cx.gui, &t);
        // x < 0: centered on −x.
        let x = if x < 0 { -x - rc_text::width(cx.gui, &t) / 2 } else { x };
        cx.gui.text(cx.queue, &t, (gx + x) as f32, (gy + y) as f32, TITLE_COLOR, false, 0);
    }
    if let Some((x, y)) = label_at {
        let l = tr(cx.gui, "container.inventory");
        cx.gui.text(cx.queue, &l, (gx + x) as f32, (gy + y) as f32, TITLE_COLOR, false, 0);
    }
}

// ---------------------------------------------------------------------------------------------
// Demo driver (`RUSTCRAFT_GUITEST`).

#[derive(Default)]
struct GuiTest {
    step: usize,
    ticks: u32,
    release_use: u32,
}

fn key_code(name: &str) -> Option<KeyCode> {
    Some(match name {
        "Escape" => KeyCode::Escape,
        "Enter" => KeyCode::Enter,
        "Backspace" => KeyCode::Backspace,
        "Up" => KeyCode::ArrowUp,
        "Down" => KeyCode::ArrowDown,
        "Left" => KeyCode::ArrowLeft,
        "Right" => KeyCode::ArrowRight,
        "PageUp" => KeyCode::PageUp,
        "PageDown" => KeyCode::PageDown,
        "Tab" => KeyCode::Tab,
        "E" => KeyCode::KeyE,
        "Home" => KeyCode::Home,
        "End" => KeyCode::End,
        _ => return None,
    })
}

// ---------------------------------------------------------------------------------------------
// Application side.

impl crate::App {
    pub(crate) fn play_ui(&mut self, sound: &str, pitch: f32) {
        if let Some(a) = self.audio.get() {
            a.play(sound, rc_audio::Category::Ui, None, 0.25, pitch, None);
        }
    }

    /// Packets received specific to the special screens.
    pub(crate) fn special_apply(&mut self, ev: rc_client::Event) {
        use rc_client::Event;
        match ev {
            Event::MerchantOffers(o) => {
                if let Some(c) = &mut self.inventory.container
                    && c.window == o.window
                    && c.kind == Kind::Special(Sp::Merchant)
                {
                    c.extra.trade.set_offers(*o);
                    self.trade_predict();
                }
            }
            Event::UpdateRecipes(r) => crate::menu_special::set_recipes(*r),
            Event::MountScreen { window, columns, entity } => self.open_mount(window, columns, entity),
            Event::OpenBook(hand) => self.open_book_read(hand),
            Event::OpenSignEditor { pos, front } => self.open_sign_editor(pos, front),
            _ => {}
        }
    }

    /// Closing a screen outside a container (Esc, Done, forced closing).
    pub(crate) fn close_overlay(&mut self) {
        match self.special.overlay.take() {
            Some(Overlay::Sign(s)) => self.sign_close(*s),
            Some(Overlay::Book(b)) if b.is_lectern() => {
                self.special.overlay = Some(Overlay::Book(b));
                self.close_screen();
                self.special.overlay = None;
            }
            Some(_) => {}
            None => return,
        }
        self.grab(true);
    }

    /// Opens a screen outside a container.
    pub(crate) fn open_overlay(&mut self, o: Overlay) {
        self.special.overlay = Some(o);
        self.keys.clear();
        self.grab(false);
    }

    /// Window input: true if a special screen consumed it.
    pub(crate) fn special_window_event(&mut self, event: &WindowEvent) -> bool {
        if self.special.overlay.is_some() {
            return self.overlay_event(event);
        }
        let Some(Kind::Special(s)) = self.inventory.container.as_ref().map(|c| c.kind) else { return false };
        if !self.screens.open {
            return false;
        }
        let Some(m) = self.gui_mouse() else { return false };
        match event {
            WindowEvent::MouseInput { state, button, .. } => {
                let b = match button {
                    MouseButton::Left => 0,
                    MouseButton::Right => 1,
                    _ => return false,
                };
                if *state == ElementState::Pressed {
                    self.special_press(s, m, b)
                } else {
                    self.special_release(s, m, b)
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = Some((position.x, position.y));
                if let Some(m) = self.gui_mouse() {
                    self.special_motion(s, m);
                }
                false
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let y = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => f64::from(*y),
                    winit::event::MouseScrollDelta::PixelDelta(p) => p.y / 20.0,
                };
                self.special_wheel(s, m, y)
            }
            WindowEvent::KeyboardInput { event, .. } if self.chat.input.is_none() => {
                if event.state != ElementState::Pressed {
                    return false;
                }
                let Some(k) = crate::widgets::edit_key(event, self.mods) else { return false };
                let text = crate::widgets::typed_text(event, self.mods).map(str::to_owned);
                self.special_key(s, k, text.as_deref())
            }
            // Text committed by the IME: like a keystroke (anvil name field).
            e @ WindowEvent::Ime(_) if self.chat.input.is_none() => {
                crate::ime::commit(e).is_some_and(|t| self.special_key(s, crate::ime::TEXT_ONLY, Some(t)))
            }
            _ => false,
        }
    }

    fn special_press(&mut self, s: Sp, m: (f32, f32), b: u8) -> bool {
        match s {
            Sp::Merchant => self.trade_press(m, b),
            Sp::Mount(_) => false,
            _ => self.work_press(s, m, b),
        }
    }

    fn special_release(&mut self, s: Sp, m: (f32, f32), b: u8) -> bool {
        match s {
            Sp::Merchant => self.trade_release(m, b),
            Sp::Mount(_) => false,
            _ => self.work_release(s, m, b),
        }
    }

    fn special_motion(&mut self, s: Sp, m: (f32, f32)) {
        match s {
            Sp::Merchant => self.trade_motion(m),
            Sp::Mount(_) => {}
            _ => self.work_motion(s, m),
        }
    }

    fn special_wheel(&mut self, s: Sp, m: (f32, f32), dy: f64) -> bool {
        match s {
            Sp::Merchant => self.trade_wheel(m, dy),
            Sp::Mount(_) => false,
            _ => self.work_wheel(s, m, dy),
        }
    }

    fn special_key(&mut self, s: Sp, k: KeyCode, text: Option<&str>) -> bool {
        match s {
            Sp::Anvil => self.anvil_key(k, text),
            _ => false,
        }
    }

    /// Input of a screen outside a container: all the input goes to it.
    fn overlay_event(&mut self, event: &WindowEvent) -> bool {
        match event {
            WindowEvent::CloseRequested | WindowEvent::Resized(_) | WindowEvent::ModifiersChanged(_) => false,
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = Some((position.x, position.y));
                true
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button, .. } => {
                let b = match button {
                    MouseButton::Left => 0,
                    MouseButton::Right => 1,
                    _ => return true,
                };
                if let Some(m) = self.gui_mouse() {
                    self.overlay_press(m, b);
                }
                true
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let y = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => f64::from(*y),
                    winit::event::MouseScrollDelta::PixelDelta(p) => p.y / 20.0,
                };
                if let Some(m) = self.gui_mouse()
                    && matches!(self.special.overlay, Some(Overlay::Op(_)))
                    && let Some(Overlay::Op(mut op)) = self.special.overlay.take()
                {
                    let a = self.op_wheel(&mut op, m, y);
                    self.special.overlay = Some(Overlay::Op(op));
                    self.op_action(a);
                }
                true
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return true;
                }
                let Some(k) = crate::widgets::edit_key(event, self.mods) else { return true };
                let text = crate::widgets::typed_text(event, self.mods).map(str::to_owned);
                self.overlay_key(k, text.as_deref());
                true
            }
            // Text committed by the IME: like a keystroke (book, sign, command blocks…).
            e @ WindowEvent::Ime(_) => {
                if let Some(t) = crate::ime::commit(e) {
                    self.overlay_key(crate::ime::TEXT_ONLY, Some(t));
                }
                true
            }
            _ => false,
        }
    }

    fn overlay_press(&mut self, m: (f32, f32), b: u8) {
        let Some(o) = self.special.overlay.take() else { return };
        let Some(gui) = self.renderer.as_ref().map(|r| &r.gui) else {
            self.special.overlay = Some(o);
            return;
        };
        match o {
            Overlay::Book(mut bk) => {
                let a = bk.press(gui, m, b, self.mods.shift_key());
                self.special.overlay = Some(Overlay::Book(bk));
                self.book_action(a);
            }
            Overlay::Sign(mut sg) => {
                let done = sg.press(gui, m, b);
                self.special.overlay = Some(Overlay::Sign(sg));
                if done {
                    self.close_overlay();
                }
            }
            Overlay::Op(mut op) => {
                let a = self.op_press(&mut op, m, b);
                self.special.overlay = Some(Overlay::Op(op));
                self.op_action(a);
            }
        }
    }

    fn overlay_key(&mut self, k: KeyCode, text: Option<&str>) {
        let mods = self.mods;
        let Some(o) = self.special.overlay.take() else { return };
        match o {
            Overlay::Book(mut bk) => {
                let act = bk.key(self.renderer.as_ref().map(|r| &r.gui), k, mods, text);
                self.special.overlay = Some(Overlay::Book(bk));
                self.book_action(act);
            }
            Overlay::Sign(mut sg) => {
                let gui = self.renderer.as_ref().map(|r| &r.gui);
                let done = sg.key(gui, k, mods, text);
                let edited = sg.take_edit();
                let (pos, front, lines) = (sg.pos, sg.front, sg.lines());
                self.special.overlay = Some(Overlay::Sign(sg));
                if edited {
                    self.sign_live(pos, front, &lines);
                }
                if done {
                    self.close_overlay();
                }
            }
            Overlay::Op(mut op) => {
                let act = self.op_key(&mut op, k, text);
                self.special.overlay = Some(Overlay::Op(op));
                self.op_action(act);
            }
        }
    }

    /// Special screens tick (animations, predictions, distances, demo).
    pub(crate) fn special_tick(&mut self) {
        self.special_test_tick();
        // Lectern closed by the server: its reading screen too.
        let lectern = self.screens.open
            && matches!(self.inventory.container.as_ref().map(|c| c.kind), Some(Kind::Special(Sp::Lectern)));
        if !lectern && matches!(&self.special.overlay, Some(Overlay::Book(b)) if b.is_lectern()) {
            self.special.overlay = None;
            self.grab(true);
        }
        if let Some(c) = &self.inventory.container
            && let Kind::Special(s) = c.kind
        {
            match s {
                Sp::Merchant => self.trade_tick(),
                Sp::Lectern => self.lectern_tick(),
                _ => self.work_tick(s),
            }
        }
        if let Some(Overlay::Sign(_)) = &self.special.overlay {
            self.sign_tick();
        }
        if let Some(Overlay::Op(_)) = &self.special.overlay {
            self.op_tick();
        }
    }

    /// Keyboard-less demo: `RUSTCRAFT_GUITEST` = steps separated by `;`, one every
    /// `RUSTCRAFT_GUITEST_EVERY` ticks (10) after tick 200. Steps: `use` (right click
    /// on the target), `useitem`, `mouse:x,y`, `click:x,y[,button]`, `sclick:x,y` (shift),
    /// `wheel:x,y,d`, `key:Name`, `type:text`, `wait`.
    fn special_test_tick(&mut self) {
        let Some(test) = rc_data::env_str!("RUSTCRAFT_GUITEST") else { return };
        if !self.positioned {
            return;
        }
        if self.special.test.release_use > 0 {
            self.special.test.release_use -= 1;
            if self.special.test.release_use == 0 {
                self.act.release(1);
                // S8: end of the held use (`hold`) sent to the server.
                let _ = self.session.link.send_player_action(6, [0; 3], 0, 0);
            }
        }
        let every: u32 = rc_data::env_str!("RUSTCRAFT_GUITEST_EVERY").and_then(|v| v.parse().ok()).unwrap_or(10);
        self.special.test.ticks += 1;
        let t = self.special.test.ticks;
        if t < 200 || !(t - 200).is_multiple_of(every.max(1)) {
            return;
        }
        let Some(step) = test.split(';').map(str::trim).nth(self.special.test.step) else { return };
        self.special.test.step += 1;
        eprintln!("screen demo: {step}");
        let (verb, arg) = step.split_once(':').unwrap_or((step, ""));
        let nums: Vec<f32> = arg.split(',').filter_map(|v| v.trim().parse().ok()).collect();
        match verb {
            "use" => match self.pick() {
                crate::pick::Hit::Entity { id, at } => {
                    let Some(e) = self.entities.map.get(&id) else { return };
                    let rel = [at[0] - e.pos[0], at[1] - e.pos[1], at[2] - e.pos[2]];
                    let kind = e.kind.clone();
                    let _ = self.session.link.send_interact(id, 0, rel, false);
                    if kind == "command_block_minecart" {
                        self.op_use_minecart(id);
                    }
                    eprintln!("demo: entity {id} ({kind})");
                }
                crate::pick::Hit::Block { pos, face, at, inside } => {
                    let view = crate::environ::WorldView {
                        grid: &self.world.grid,
                        min_y: self.world.min_y,
                        height: self.world.height,
                    };
                    let state = view.state(pos[0], pos[1], pos[2]);
                    self.op_use_block(pos, state);
                    let seq = self.act.next_seq();
                    let rel = [0, 1, 2].map(|a| (at[a] - f64::from(pos[a])) as f32);
                    let _ = self.session.link.send_use_item_on(pos, i32::from(face), rel, inside, seq);
                    eprintln!("demo: block {pos:?}");
                }
                crate::pick::Hit::Miss => eprintln!("demo: nothing targeted"),
            },
            // Capture now (instead of the `RUSTCRAFT_CAPTURE` delay).
            "shot" => {
                let now = self.started.elapsed().as_secs_f64();
                if let Some((_, t)) = &mut self.capture {
                    *t = now;
                }
            }
            // F3+I (Shift: client data).
            "f3i" => self.debug_copy_recreate(arg == "shift"),
            "invkey" => {
                if !self.ride_inventory() {
                    let v = self.vehicle().and_then(|v| self.entities.map.get(&v)).map(|e| e.kind.clone());
                    eprintln!("demo: no mount with an inventory ({v:?})");
                }
            }
            "useitem"
                if self.inventory.hotbar(self.hud.selected).is_some_and(|s| {
                    self.item_names.get(s.item as usize).is_some_and(|n| n.ends_with("writable_book"))
                }) =>
            {
                self.open_book_edit(0);
            }
            "useitem" => {
                let _ = self.session.link.send_use_item(0, 0, self.yaw, self.pitch);
            }
            // S8: use held `N` ticks then released (bow, crossbow, trident).
            "hold" => {
                let _ = self.session.link.send_use_item(0, 0, self.yaw, self.pitch);
                self.special.test.release_use = nums.first().map_or(20, |&n| n.max(1.0) as u32);
            }
            "mouse" if nums.len() >= 2 => self.menutest.2 = Some((nums[0], nums[1])),
            "click" | "sclick" if nums.len() >= 2 => {
                let m = (nums[0], nums[1]);
                self.menutest.2 = Some(m);
                let b = nums.get(2).map_or(0, |&b| b as u8);
                let shift = verb == "sclick";
                self.test_click(m, b, shift);
            }
            "wheel" if nums.len() >= 3 => {
                let m = (nums[0], nums[1]);
                self.menutest.2 = Some(m);
                if self.special.overlay.is_none()
                    && let Some(Kind::Special(s)) = self.inventory.container.as_ref().map(|c| c.kind)
                {
                    self.special_wheel(s, m, f64::from(nums[2]));
                }
            }
            "key" => {
                if let Some(k) = key_code(arg) {
                    self.test_key(k, None);
                }
            }
            "type" => {
                for ch in arg.chars() {
                    let s = ch.to_string();
                    self.test_key(KeyCode::KeyZ, Some(&s));
                }
            }
            _ => {}
        }
    }

    fn test_click(&mut self, m: (f32, f32), b: u8, shift: bool) {
        if self.special.overlay.is_some() {
            self.overlay_press(m, b);
            return;
        }
        if !self.screens.open {
            return;
        }
        if let Some(Kind::Special(s)) = self.inventory.container.as_ref().map(|c| c.kind)
            && self.special_press(s, m, b)
        {
            self.special_release(s, m, b);
            return;
        }
        let Some(r) = &self.renderer else { return };
        let creative = self.env.game_mode == 1;
        let mut clicks = self.screens.press(&r.gui, &self.inventory, b, m, shift, creative);
        clicks.extend(self.screens.release(&r.gui, &self.inventory, b, m, shift));
        self.screen_clicks(clicks);
    }

    fn test_key(&mut self, k: KeyCode, text: Option<&str>) {
        if self.special.overlay.is_some() {
            self.overlay_key(k, text);
            return;
        }
        if let Some(Kind::Special(s)) = self.inventory.container.as_ref().map(|c| c.kind)
            && self.screens.open
            && self.special_key(s, k, text)
        {
            return;
        }
        if k == KeyCode::Escape {
            self.close_screen();
        }
    }
}

#[cfg(test)]
mod ui_light_tests {
    use glam::{Mat3, Mat4, Vec3};

    /// Entity shader diffuse: two lights, ambient 0.4.
    fn diffuse(l: [[f32; 4]; 2], n: Vec3) -> f32 {
        let d = |v: [f32; 4]| Vec3::new(v[0], v[1], v[2]).dot(n).max(0.0);
        (0.6 * (d(l[0]) + d(l[1])) + 0.4).min(1.0)
    }

    /// Transformed normal (inverse transpose), normalized.
    fn normal(m: Mat4, n: Vec3) -> Vec3 {
        (Mat3::from_mat4(m).inverse().transpose() * n).normalize()
    }

    /// The game's lights in its pose frame.
    fn game_lights() -> [[f32; 4]; 2] {
        let n = |v: Vec3| {
            let v = v.normalize();
            [v.x, v.y, v.z, 0.0]
        };
        [n(Vec3::new(0.2, -1.0, 1.0)), n(Vec3::new(-0.2, -1.0, 0.0))]
    }

    /// Same lighting as the game for any face of an inventory model: the game's pose
    /// (translation, scale (s, s, −s), rotation) with its lights, against our root with
    /// ours.
    #[test]
    fn ui_models_lit_like_the_game() {
        let rot = Mat4::from_rotation_z(std::f32::consts::PI) * Mat4::from_rotation_x(0.3);
        let ours = super::gui_root(120.0, 80.0, 30.0, 0.9, rot);
        let game = Mat4::from_translation(Vec3::new(120.0, 80.0, 50.0))
            * Mat4::from_scale(Vec3::new(30.0, 30.0, -30.0))
            * Mat4::from_translation(Vec3::new(0.0, 0.9, 0.0))
            * rot;
        let faces =
            [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z, Vec3::new(0.3, -0.5, 0.8).normalize()];
        for n in faces {
            let want = diffuse(game_lights(), normal(game, n));
            let got = diffuse(super::entity_in_ui_lights(), normal(ours, n));
            assert!((want - got).abs() < 1e-5, "face {n}: game {want}, ours {got}");
        }
    }

    /// Front face of an inventory player (toward the screen): 0.82 like the game (0.74 with the
    /// world lights).
    #[test]
    fn inventory_player_front_face() {
        let front = Vec3::Z;
        assert!((diffuse(super::entity_in_ui_lights(), front) - 0.82).abs() < 1e-3);
        let world = |v: Vec3| {
            let v = v.normalize();
            [v.x, v.y, v.z, 0.0]
        };
        let lw = [world(Vec3::new(0.2, 1.0, -0.7)), world(Vec3::new(-0.2, 1.0, 0.7))];
        assert!((diffuse(lw, front) - 0.739).abs() < 1e-3);
    }
}
