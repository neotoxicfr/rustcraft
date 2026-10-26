//! State machine of the application: screens outside a world (title screen…) or world joined.
//! With no arguments, the client starts on the title screen; with `host port username`, it connects
//! right away as before. The rest of the application sees only a session (offline:
//! [`rc_client::Session::offline`], sends have no effect).

use std::time::Instant;

use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::keyboard::KeyCode;

use rc_client::{Config, Event, Session};
use rc_physics::Player;
use rc_text::Component;
use rc_world::ChunkGrid;

use crate::servers::ServerEntry;
use crate::{App, TICK, environ, front, widgets, world_sounds};

/// Command-line arguments.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Args {
    /// `host port username`: direct connection (test scripts).
    pub direct: Option<(String, u16, String)>,
    /// `--username <username>` (offline).
    pub username: Option<String>,
    /// Launcher options, as for the official game (`launch.rs`).
    pub launch: crate::launch::Launch,
}

impl Args {
    pub fn parse(args: impl Iterator<Item = String>) -> Args {
        let (launch, pos) = crate::launch::Launch::parse(args);
        let mut out = Args { username: launch.username.clone(), launch, ..Args::default() };
        if let Some(addr) = &out.launch.quick_play_multiplayer {
            // Direct connection requested by the launcher (`--quickPlayMultiplayer host[:port]`).
            let a = rc_client::status::ServerAddress::parse(addr);
            let name = out.username.clone().unwrap_or_else(|| "Rustcraft".into());
            out.direct = Some((a.host, a.port, name));
        } else if !pos.is_empty() {
            let host = pos[0].clone();
            let port = pos.get(1).and_then(|p| p.parse().ok()).unwrap_or(25565);
            let name = pos.get(2).cloned().or_else(|| out.username.clone()).unwrap_or_else(|| "Rustcraft".into());
            out.direct = Some((host, port, name));
        }
        out
    }
}

impl App {
    /// Frame outside a world: menu ticks (music, clicks), top screen over the panorama.
    pub(crate) fn menu_frame(&mut self) {
        // Singleplayer quick play (`--quickPlaySingleplayer`): at the first frame outside a world.
        if self.renderer.is_some()
            && let Some(id) = self.quick_play.take()
        {
            self.quick_play_solo(&id);
        }
        self.poll_connect();
        self.reload_tick();
        let now = Instant::now();
        let elapsed = now.duration_since(self.last).as_secs_f64();
        let dt = elapsed.min(0.1);
        self.last = now;
        for _ in 0..crate::ticking::due_ticks(&mut self.tick_acc, elapsed, TICK) {
            self.menu_tick();
        }
        self.menus.outside = true;
        let mouse = self.gui_mouse();
        let aspect = self.renderer.as_ref().map_or(16.0 / 9.0, |r| r.aspect());
        let view = environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
        let frame_env = self.env.frame(
            &view,
            &self.phys,
            &self.data,
            self.eye,
            0.0,
            0.0,
            aspect,
            0.0,
            dt,
            self.options.i("renderDistance"),
            false,
        );
        let Some(r) = &mut self.renderer else { return };
        if let Some((path, at)) = &self.capture
            && self.started.elapsed().as_secs_f64() >= *at
        {
            r.request_capture(path.clone());
            self.capture = None;
            self.quit_after_capture = true;
        } else if self.quit_after_capture {
            rc_assets::cache::wait_writer();
            std::process::exit(0);
        }
        let (ww, wh) = r.size();
        r.gui.begin(ww, wh);
        let q = r.queue_handle();
        let time = self.started.elapsed().as_secs_f64();
        if self.packs.screen.is_some() {
            // Packs screen (`pack_screen.rs`): it replaces the screen below.
        } else if self.menus.is_open() {
            self.menus.draw(&mut r.gui, &q, &self.options, mouse, time);
        } else {
            self.front.draw(&mut r.gui, &q, &self.options, mouse, time);
        }
        // Notifications outside a world (world backup, access failures).
        {
            let env = crate::stack::HudEnv::empty();

            let items = crate::hud::HotbarItems {
                inv: &self.inventory,
                ids: &self.component_ids,
                data: &self.data,
                infos: &self.data.items,
                names: &self.item_names,
                env: &env,
                pt: 0.0,
            };
            let (now, mult) = (time * 1000.0, self.options.f("notificationDisplayTime"));
            self.toasts.periodic_tick(&r.gui, now);
            self.toasts.update(now, mult);
            self.toasts.draw(&mut r.gui, &q, &items, now, mult);
            // Dialog received during configuration (`dialog.rs`): objects of the built-in registry.
            if let Some(d) = &mut self.exact.dialog {
                d.draw(&mut r.gui, &q, mouse, Some(&items), &self.screens.hasher.enchantments);
            }
        }
        // Server packs prompt and resource loading screen (`reload.rs`).
        self.packs.draw(&mut r.gui, &q, &self.options, mouse);
        r.show_panorama = true;
        // Partly released (24 MB of VRAM): reloaded from the resource cache on return.
        if r.panorama.is_none()
            && let Some(p) = crate::panorama::PanoramaAssets::load(&self.assets)
        {
            r.enable_panorama(&p);
        }
        if let Some(p) = &mut r.panorama {
            p.speed = self.options.f("panoramaScrollSpeed") as f32;
        }
        r.view.hand = false;
        let _ = r.render(self.eye, 0.0, 0.0, self.world.min_y, self.world.height, &frame_env);
        crate::first_frame(self.started);
        if let Some(r) = &self.renderer
            && self.fps_since.elapsed().as_secs_f64() >= 1.0
        {
            r.window.set_title(&format!("Rustcraft {}", crate::front::VERSION_NAME));
            self.fps_since = Instant::now();
        }
    }

    /// Tick outside a world: menu music, sounds, menus and demo.
    fn menu_tick(&mut self) {
        if let Some(a) = self.audio.get() {
            let view =
                environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
            let ctx = world_sounds::TickCtx {
                view: &view,
                env: &mut self.env,
                player: None,
                camera: self.eye,
                dimension: "",
                boss_flags: 0,
                loading: false,
                paused: false,
            };
            self.world_sounds.tick(a, ctx);
        }
        self.tick_audio();
        self.menus_tick();
        self.dialog_tick();
        // Singleplayer screens: creation, copies, world closing as a background task.
        if let Some(r) = &self.renderer {
            let a = self.front.tick(&r.gui);
            self.front_actions(a);
        }
        let clicks = std::mem::take(&mut self.front.clicks);
        if let Some(a) = self.audio.get() {
            for _ in 0..clicks {
                a.play(widgets::CLICK_SOUND, rc_audio::Category::Ui, None, 0.25, 1.0, None);
            }
        }
    }

    /// Input meant for the screens outside a world (menus closed); true if the event is consumed.
    pub(crate) fn front_window_event(&mut self, event: &WindowEvent) -> bool {
        if !self.front.is_open() || self.menus.is_open() {
            return false;
        }
        let actions = match event {
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return true;
                }
                let Some(r) = &self.renderer else { return true };
                let mut a = Vec::new();
                if let Some(k) = crate::widgets::edit_key(event, self.mods) {
                    a = self.front.key(&r.gui, &mut self.options, k, self.mods);
                }
                if let Some(t) = crate::widgets::typed_text(event, self.mods) {
                    a.extend(self.front.typed(&r.gui, &mut self.options, t));
                }
                a
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let b = match button {
                    MouseButton::Left => 0,
                    MouseButton::Right => 1,
                    MouseButton::Middle => 2,
                    _ => return true,
                };
                let (Some(r), Some(m)) = (&self.renderer, self.gui_mouse()) else { return true };
                if *state == ElementState::Pressed {
                    self.front.press(&r.gui, &mut self.options, m, b, self.mods.shift_key())
                } else {
                    self.front.release(&r.gui, &mut self.options)
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let y = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => f64::from(*y),
                    winit::event::MouseScrollDelta::PixelDelta(p) => p.y / 20.0,
                };
                let (Some(r), Some(m)) = (&self.renderer, self.gui_mouse()) else { return true };
                self.front.wheel(&r.gui, &mut self.options, m, y)
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = Some((position.x, position.y));
                let (Some(r), Some(m)) = (&self.renderer, self.gui_mouse()) else { return true };
                self.front.motion(&r.gui, &mut self.options, m)
            }
            // Text validated by the IME: like a keystroke.
            e @ WindowEvent::Ime(_) => {
                let (Some(r), Some(t)) = (&self.renderer, crate::ime::commit(e)) else { return true };
                self.front.typed(&r.gui, &mut self.options, t)
            }
            _ => return false,
        };
        self.front_actions(actions);
        true
    }

    pub(crate) fn front_actions(&mut self, actions: Vec<front::Action>) {
        for a in actions {
            match a {
                front::Action::Quit => self.exit_requested = true,
                front::Action::Menu(ids) => {
                    self.menus.outside = true;
                    for id in ids {
                        self.menus.open(id, false);
                    }
                }
                front::Action::SaveOptions => self.save_options(),
                front::Action::Connect(e) => self.start_connect(e),
                front::Action::CancelConnect => self.cancel_connect(),
                front::Action::OpenWorld(dir, new) => self.open_world(&dir, new),
                front::Action::WorldPack(p) => self.world_pack_push(p),
                front::Action::DropWorldPack => self.world_pack_pop(),
                front::Action::Singleplayer => self.front.open_solo(self.data.clone()),
                front::Action::Toast(b) => {
                    let (t, m) = *b;
                    if let Some(r) = &self.renderer {
                        self.toasts.system(&r.gui, &t, m.as_ref(), 5000.0);
                    }
                }
                other => self.world_front_action(other),
            }
        }
    }

    /// Keyboard-less demo outside a world (`RUSTCRAFT_MENUTEST=title`): `RUSTCRAFT_MENUCLICKS`
    /// = actions separated by commas, one every 10 ticks (`~N:` at the head: N ticks
    /// before the next) starting at 3 s, addressed
    /// to the open menus otherwise to the screen outside a world: `x:y` left click (GUI units), `m:x:y`
    /// mouse, `dc:x:y` double click, `w:x:y:notches` wheel, `k:<key>`, `t:<text>` typing,
    /// `wait:<ticks>` wait, `pause` pause menu (in a world), `cap:<name>` capture without quitting,
    /// `quit` close.
    pub(crate) fn titletest_tick(&mut self) {
        if self.renderer.is_none() {
            return;
        }
        self.menutest.1 += 1;
        if self.menutest.1 < self.front.test_next {
            return;
        }
        let spec = rc_data::env_str!("RUSTCRAFT_MENUCLICKS").unwrap_or_default();
        let Some(step) = spec.split(',').filter(|s| !s.is_empty()).nth(self.menutest.0) else { return };
        // `~N:` at the head: N ticks before the next step (10 by default).
        let (delay, step) = match step.strip_prefix('~').and_then(|s| s.split_once(':')) {
            Some((n, rest)) => (n.parse().unwrap_or(10), rest),
            None => (10, step),
        };
        self.front.test_next = self.menutest.1 + delay;
        // Typing: everything after `t:` (colon included).
        let p: Vec<&str> = match step.strip_prefix("t:") {
            Some(text) => vec!["t", text],
            None => step.split(':').collect(),
        };
        self.menutest.0 += 1;
        eprintln!("demo outside a world: {step}");
        // Capture without quitting (`cap:<name>`, next to the `RUSTCRAFT_CAPTURE` file), end (`quit`).
        if let ["cap", name] = p[..] {
            let dir = rc_data::env_str!("RUSTCRAFT_CAPTURE").and_then(|v| {
                let path = std::path::PathBuf::from(v.split_once(';').map_or(v, |x| x.0));
                path.parent().map(std::path::Path::to_path_buf)
            });
            if let (Some(d), Some(r)) = (dir, &mut self.renderer) {
                r.request_capture(d.join(format!("{name}.bmp")));
            }
            return;
        }
        if p[..] == ["quit"] {
            self.exit_requested = true;
            return;
        }
        let Some(r) = &self.renderer else { return };
        let f = |s: &str| s.trim().parse::<f32>().unwrap_or(0.0);
        let gui = &r.gui;
        let to_menus = self.menus.is_open();
        let mut menu_actions = Vec::new();
        let mut front_actions = Vec::new();
        let key = |k: &str| match k {
            "Tab" => KeyCode::Tab,
            "Down" => KeyCode::ArrowDown,
            "Up" => KeyCode::ArrowUp,
            "Left" => KeyCode::ArrowLeft,
            "Right" => KeyCode::ArrowRight,
            "Enter" => KeyCode::Enter,
            "F5" => KeyCode::F5,
            "Delete" => KeyCode::Delete,
            "Backspace" => KeyCode::Backspace,
            _ => KeyCode::Escape,
        };
        match p[..] {
            // Wait of n ticks before the next step.
            ["wait", n] => self.front.test_next = self.menutest.1 + n.trim().parse::<u32>().unwrap_or(0),
            ["pause"] => {
                self.open_pause(false);
            }
            [x, y] if x != "k" && x != "t" => {
                let m = (f(x), f(y));
                self.menutest.2 = Some(m);
                if self.packs.screen.is_some() {
                    // Resource packs screen (`pack_screen.rs`) above the menus.
                    self.pack_screen_click();
                } else if to_menus {
                    menu_actions = self.menus.press(gui, &mut self.options, m, 0, false);
                    menu_actions.extend(self.menus.release(gui, &mut self.options));
                } else {
                    front_actions = self.front.press(gui, &mut self.options, m, 0, false);
                    front_actions.extend(self.front.release(gui, &mut self.options));
                }
            }
            ["dc", x, y] => {
                let m = (f(x), f(y));
                self.menutest.2 = Some(m);
                for _ in 0..2 {
                    front_actions.extend(self.front.press(gui, &mut self.options, m, 0, false));
                    front_actions.extend(self.front.release(gui, &mut self.options));
                }
            }
            ["m", x, y] => {
                let m = (f(x), f(y));
                self.menutest.2 = Some(m);
                if to_menus {
                    menu_actions = self.menus.motion(gui, &mut self.options, m);
                } else {
                    front_actions = self.front.motion(gui, &mut self.options, m);
                }
            }
            ["w", x, y, n] => {
                let m = (f(x), f(y));
                self.menutest.2 = Some(m);
                if to_menus {
                    menu_actions = self.menus.wheel(gui, &mut self.options, m, f64::from(f(n)));
                } else {
                    front_actions = self.front.wheel(gui, &mut self.options, m, f64::from(f(n)));
                }
            }
            ["k", k] => {
                if to_menus {
                    menu_actions = self.menus.key(gui, &mut self.options, key(k), self.mods);
                } else if self.front.is_open() {
                    front_actions = self.front.key(gui, &mut self.options, key(k), self.mods);
                }
            }
            ["t", text] => {
                if to_menus {
                    menu_actions = self.menus.typed(gui, &mut self.options, text);
                } else {
                    front_actions = self.front.typed(gui, &mut self.options, text);
                }
            }
            _ => {}
        }
        self.menu_actions(menu_actions);
        self.front_actions(front_actions);
    }
}

/// Connection being opened: result of the connection thread, username and client information.
/// Abandoned (receiver dropped), the session possibly opened is closed by the thread.
pub(crate) struct Pending {
    rx: crossbeam_channel::Receiver<Result<Session, String>>,
    name: String,
    info: rc_proto_v777::play::ClientInformation,
}

/// Local reason for the end of a session (no reason from the server) as a displayable component.
fn local_reason(reason: &str) -> Component {
    if reason.starts_with("unknown host") {
        return Component::translate(
            "disconnect.genericReason",
            vec![rc_text::Arg::Component(Component::translate("disconnect.unknownHost", vec![]))],
        );
    }
    let generic = |text: String| {
        Component::translate("disconnect.genericReason", vec![rc_text::Arg::Component(Component::text(text))])
    };
    match rc_client::reason::parse(reason) {
        rc_client::reason::Reason::Key(k) => Component::translate(k, vec![]),
        // Session error: "Internal Exception:" and the exception, like the game.
        rc_client::reason::Reason::Internal(e) => generic(format!("Internal Exception: {e}")),
        // Connection opening failure: the exception's message alone.
        rc_client::reason::Reason::Connect(m) => generic(m.to_owned()),
        rc_client::reason::Reason::Plain(p) => Component::text(p.strip_prefix("network: ").unwrap_or(p).to_owned()),
    }
}

impl App {
    /// "Join" (§ 4.1): connection screen, resolution and connection on a separate thread.
    pub(crate) fn start_connect(&mut self, entry: ServerEntry) {
        // `servers.dat` entry for the code of conduct (`notices.rs`).
        self.menus.world.server = Some((entry.name.clone(), entry.ip.clone()));
        self.menus.world.server_name = entry.name.clone();
        self.connect_to(entry, None);
    }

    /// Connection; `transfer`: opened by a transfer, with the cookies of the previous
    /// connection (`protocol-supplements.md` § 10).
    pub(crate) fn connect_to(&mut self, entry: ServerEntry, transfer: Option<rc_client::Cookies>) {
        if self.front.is_connecting() {
            return;
        }
        // Server packs: preference of the entry (kept as is on transfer).
        if transfer.is_none() {
            self.server_packs_connect(&entry);
        }
        let addr = rc_client::status::ServerAddress::parse(&entry.ip);
        self.debug.logs.place = entry.name.clone(); // F3+L archive name
        self.front.close_delay = std::time::Duration::ZERO;
        self.quick_log.set_world(crate::quickplay::Kind::Multiplayer, &entry.ip, Some(&entry.name));
        let name = self.front.username.clone();
        let info = self.options.client_info();
        let (uuid, token) = crate::launch::profile(&name);
        let cfg = Config {
            host: addr.host,
            port: addr.port,
            name: name.clone(),
            info: info.clone(),
            transfer: transfer.is_some(),
            cookies: transfer.unwrap_or_default(),
            uuid: Some(uuid),
            token,
        };
        let data = self.data.clone();
        let (tx, rx) = crossbeam_channel::bounded(1);
        eprintln!("connecting to {} ({})", entry.name, entry.ip);
        // Server list entry: updated by `server_data` (`misc.rs`).
        self.misc.server = Some((entry.name.clone(), entry.ip.clone()));
        self.misc.unsecure_seen = false;
        let spawned = std::thread::Builder::new().name("connection".into()).spawn(move || {
            let r = rc_client::connect(cfg, data).map_err(|e| e.to_string());
            // Attempt abandoned in the meantime: the opened session is closed again.
            if let Err(crossbeam_channel::SendError(Ok(s))) = tx.send(r) {
                s.link.disconnect();
            }
        });
        self.front.open_connecting();
        match spawned {
            Ok(_) => self.front.pending = Some(Pending { rx, name, info }),
            Err(e) => self.session_end(&e.to_string()),
        }
    }

    /// Result of the connection thread: session in place (the connection continues with its
    /// events), or failure.
    pub(crate) fn poll_connect(&mut self) {
        let Some(p) = &self.front.pending else { return };
        let r = match p.rx.try_recv() {
            Ok(r) => r,
            Err(crossbeam_channel::TryRecvError::Empty) => return,
            Err(crossbeam_channel::TryRecvError::Disconnected) => Err("?".into()),
        };
        let Some(p) = self.front.pending.take() else { return };
        match r {
            Ok(session) => {
                self.local_uuid = crate::launch::profile(&p.name).0;
                self.local_name = p.name;
                self.last_info = p.info;
                self.session = session;
            }
            Err(e) => {
                eprintln!("connection: {e}");
                self.session_end(&e);
            }
        }
    }

    /// "Cancel": attempt abandoned, connection closed (`connect.aborted`).
    fn cancel_connect(&mut self) {
        self.front.pending = None;
        self.session.link.disconnect();
        if let Ok(s) = Session::offline(self.data.clone()) {
            self.session = s;
        }
    }

    /// Session events seen by the state machine before the game; `None`: consumed.
    pub(crate) fn session_event(&mut self, ev: Event) -> Option<Event> {
        match ev {
            Event::LoginFinished => {
                self.front.set_status("connect.joining");
                None
            }
            Event::LoginStatus(key) => {
                self.front.set_status(key);
                None
            }
            Event::Kicked(c) => {
                self.front.kicked = Some(c);
                None
            }
            Event::Joined { .. } => {
                // World loading screen until the player's chunk (§ 4.3).
                self.front.enter_world();
                self.cooldowns.clear();
                self.menus.close_all();
                self.menus.outside = false;
                Some(ev)
            }
            Event::GameEvent { kind: 13, .. } => {
                self.front.chunks_started();
                Some(ev)
            }
            Event::Respawn { ref dimension, .. } => {
                // Respawn: loading screen until the player's chunk, then a new
                // "player loaded" to the server; the position remains to be received.
                let died = self.hud.dead() || self.death.is_some();
                let reason = crate::front::LoadReason::for_respawn(died, &self.dimension, dimension);
                self.dimension = dimension.clone();
                self.front.respawn_loading(reason);
                self.cooldowns.clear();
                self.menus.close_all();
                self.session.link.expect_load();
                self.positioned = false;
                self.placed = false;
                Some(ev)
            }
            Event::Disconnected(reason) => {
                eprintln!("disconnected: {reason}");
                self.session_end(&reason);
                None
            }
            ev => Some(ev),
        }
    }

    /// End of connection or session: world left, disconnect screen (§ 4.2).
    pub(crate) fn session_end(&mut self, reason: &str) {
        let title = if self.front.in_world { "disconnect.lost" } else { "connect.failed" };
        let kicked = self.front.kicked.take();
        // Internal session error: report written and server report link (before
        // `leave_world` forgets the links).
        let mut info = crate::front::DisconnectInfo::default();
        if kicked.is_none()
            && let rc_client::reason::Reason::Internal(e) = rc_client::reason::parse(reason)
        {
            info.bug_link = crate::disconnect_report::bug_link(&self.exact.links);
            info.report = crate::disconnect_report::store(
                &crate::servers::game_dir(),
                crate::screenshot::local_now(),
                e,
                info.bug_link.as_deref(),
                &self.exact.report,
            );
        }
        let reason = kicked.unwrap_or_else(|| local_reason(reason));
        let solo = self.solo.is_some();
        self.leave_world();
        self.server_packs_disconnect();
        let Some(r) = &self.renderer else { return };
        self.front.disconnected(&self.options, &r.gui.lang, title, reason, solo, info);
        // Background test client: no disconnect screen left open.
        if crate::launch::background() {
            eprintln!("background: disconnected, closing the client");
            self.exit_requested = true;
        }
    }

    /// Singleplayer quick play: requested world (the last played if empty), otherwise error screen.
    fn quick_play_solo(&mut self, id: &str) {
        match crate::quickplay::resolve_world(id, &crate::servers::game_dir()) {
            // Same opening steps as from the list (world pack included).
            Some(dir) => {
                let Some(r) = &self.renderer else { return self.open_world(&dir, false) };
                let actions = self.front.open_solo_world(&r.gui, self.data.clone(), dir);
                self.front_actions(actions);
            }
            None => {
                let Some(r) = &self.renderer else { return };
                self.front.quick_play_error(&self.options, &r.gui.lang, self.data.clone());
            }
        }
    }

    /// Singleplayer: world opened by the integrated server, in-memory session (`rc-sim`); `new`: world
    /// just created (loading screen kept 500 ms longer, like the game).
    pub(crate) fn open_world(&mut self, dir: &std::path::Path, new: bool) {
        if self.front.is_connecting() || self.solo.is_some() {
            // Opening refused: the pack of this world (if it was loaded) does not stay active; that
            // of an already open world is kept.
            if self.solo.is_none() {
                self.world_pack_pop();
            }
            return;
        }
        // Fallback: the singleplayer screens already refuse a world with a disallowed link before
        // any read; check redone here (the folder may have changed in the meantime), world pack
        // removed. A new world is not checked.
        if !new && let Some(r) = &self.renderer {
            match crate::servers::allowed_symlinks().check_world(dir) {
                Ok(refused) if !refused.is_empty() => {
                    for f in &refused {
                        eprintln!("singleplayer: symbolic link refused {} -> {}", f.link.display(), f.target.display());
                    }
                    self.front.symlink_world(&r.gui.lang);
                    self.world_pack_pop();
                    return;
                }
                Ok(_) => {}
                Err(e) => eprintln!("singleplayer: symbolic links of {}: {e}", dir.display()),
            }
        }
        self.front.close_delay = std::time::Duration::from_millis(if new { 500 } else { 0 });
        let name = self.front.username.clone();
        let info = self.options.client_info();
        let (uuid, _) = crate::launch::profile(&name);
        let sim_distance = self.options.i("simulationDistance");
        let opts =
            rc_sim::Options { world: dir.to_owned(), name: name.clone(), uuid, info: info.clone(), sim_distance };
        eprintln!("singleplayer: opening {}", dir.display());
        // World name (F3+L archive, quick play log: folder and level name).
        self.debug.logs.place = rc_sim::level::Level::read(dir).map(|l| l.name).unwrap_or_default();
        let id = dir.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        self.quick_log.set_world(crate::quickplay::Kind::Singleplayer, &id, Some(&self.debug.logs.place));
        match rc_sim::start(opts, self.data.clone()) {
            Ok((session, handle)) => {
                // Loading screen right away, map and bar of the preparation (the game
                // does not open a connection screen in singleplayer).
                self.front.open_solo_loading(handle.prep());
                self.local_uuid = uuid;
                self.local_name = name;
                self.last_info = info;
                self.session = session;
                self.solo = Some(handle);
                crate::world_list::set_solo_open(true);
            }
            Err(e) => self.session_end(&e.to_string()),
        }
    }

    /// Game closing with a singleplayer world open: closing save awaited.
    pub(crate) fn close_solo(&mut self) {
        if let Some(h) = self.solo.take() {
            self.session.link.disconnect();
            h.wait();
            crate::world_list::set_solo_open(false);
        }
        self.front.finish_save();
    }

    /// Singleplayer simulation distance; the pause is decided per frame (`pause.rs`).
    pub(crate) fn solo_pause(&self) {
        if let Some(h) = &self.solo {
            h.set_simulation_distance(self.options.i("simulationDistance")); // W9
        }
    }

    /// "Disconnect" of the pause menu, "Title Screen" of the death screen: back to the list
    /// (singleplayer: "Save and Quit", back to the title screen).
    pub(crate) fn disconnect_to_menu(&mut self) {
        let solo = self.solo.is_some();
        self.leave_world();
        self.server_packs_disconnect();
        if solo {
            self.front.open_title(&self.options, false);
        } else {
            self.front.back_to_list(&self.options);
        }
    }

    /// Closing without a disconnect screen (transfer, `exact.rs`).
    pub(crate) fn disconnect_quietly(&mut self) {
        self.leave_world();
    }

    /// Session closed and world reset (entities, sounds, screens): another server can
    /// be joined without restarting the client.
    fn leave_world(&mut self) {
        self.session.link.disconnect();
        self.profiling_cancel();
        // Singleplayer: the integrated server saves and releases the lock; "Saving world…"
        // shown over the screens outside a world until the end.
        if let Some(h) = self.solo.take() {
            self.front.begin_save(self.data.clone(), h);
        }
        if let Ok(s) = Session::offline(self.data.clone()) {
            self.session = s;
        }
        self.front.in_world = false;
        self.front.pending = None;
        self.menus.close_all();
        self.reset_world();
        self.grab(false);
    }

    fn reset_world(&mut self) {
        self.world.grid = ChunkGrid::new(16);
        self.world.versions.clear();
        self.world.dirty.clear();
        self.world.meshes.clear();
        if let Some(r) = &mut self.renderer {
            r.clear();
        }
        self.entities.clear();
        self.entities.me = None;
        self.entities.removed_vehicle = None;
        self.particles.clear();
        self.players = Default::default();
        self.tab_list = Default::default();
        self.skins.forget_asked();
        self.scores = Default::default();
        if let Some(a) = self.audio.get() {
            a.stop_all();
        }
        self.entity_sounds.clear();
        self.world_sounds.join(self.audio.get());
        self.hud_extra.reset();
        // F3 graph logs (ticks, ping, bandwidth) and subscription: nothing carries over
        // from one server to the next.
        self.debug.logs.reset();
        self.chat.disconnected();
        self.hud = Default::default();
        self.inventory = Default::default();
        self.screens.hide();
        self.death = None;
        self.dimension.clear();
        self.spawn = None;
        self.last_death = None;
        self.biome_names.clear();
        self.pending_registries.clear();
        self.relight.clear();
        self.relight_sky.clear();
        self.light_recv.clear();
        if let Some(s) = &mut self.light_shadow {
            **s = Default::default();
        }
        self.sync = Default::default();
        self.player = Player::new([0.0; 3], 0.0, 0.0);
        self.positioned = false;
        self.prev_pos = [0.0; 3];
        self.player_id = 0;
        self.last_input = None;
        self.free_cam = false;
        self.act = crate::interact::Interact::new(&self.data);
        self.keys.clear();
        self.online_mode = false;
        self.eye = [0.0, 100.0, 0.0];
        (self.yaw, self.pitch) = (0.0, 0.0);
        self.placed = false;
        // Weather and border of a new world: nothing until the server's packets.
        self.env.env.weather = Default::default();
        self.weather.border = Default::default();
        self.weather.built = None;
        self.weather.sky_flash = 0;
        self.exact = Default::default();
        self.apply_options();
    }

    /// World of the player loaded (for `player_loaded`): end of the loading screen (§ 4.3),
    /// mouse captured when it closes (outside test scripts).
    pub(crate) fn world_loaded(&mut self) -> bool {
        // Eye block of the player (the camera at loading); its column meshed and appeared enough
        // (fade ≥ 0.3). Without a renderer (no window yet), the received column is enough.
        let p = self.player.pos;
        let cam = [p[0], p[1] + self.player.eye_height(), p[2]].map(|v| v.floor() as i32);
        let (cx, cz) = (cam[0] >> 4, cam[2] >> 4);
        let shown = match &self.renderer {
            Some(r) => r.column_visibility(cx, cz),
            None => self.world.grid.get(cx, cz).map(|_| 1.0),
        };
        let height = (self.world.min_y, self.world.height);
        let ready = level_ready(cam[1], height, self.env.game_mode == 3, self.hud.dead(), shown);
        let was_open = self.front.is_open();
        let done = self.front.loading_done(ready);
        if done && was_open && !self.front.is_open() && !self.front.direct {
            self.grab(true);
        }
        done
    }
}

/// World ready for the loading screen: camera outside the world height (`min_y`, height),
/// spectator, dead, or camera column meshed and visible at least at 0.3 of its fade
/// (`shown`).
fn level_ready(cam_y: i32, (min_y, height): (i32, i32), spectator: bool, dead: bool, shown: Option<f32>) -> bool {
    let outside = cam_y < min_y || cam_y >= min_y + height;
    outside || spectator || dead || shown.is_some_and(|v| v >= 0.3)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Column received but not yet meshed: not ready (the old criterion said ready);
    /// meshed but barely appeared: not yet; at 0.3 of the fade, or camera outside the world: ready.
    #[test]
    fn loading_waits_for_the_camera_section() {
        let h = (-64, 384);
        assert!(!level_ready(70, h, false, false, None));
        assert!(!level_ready(70, h, false, false, Some(0.29)));
        assert!(level_ready(70, h, false, false, Some(0.3)));
        assert!(level_ready(400, h, false, false, None));
        assert!(level_ready(-65, h, false, false, None));
        assert!(level_ready(70, h, true, false, None) && level_ready(70, h, false, true, None));
    }

    fn parse(v: &[&str]) -> Args {
        Args::parse(v.iter().map(|s| (*s).to_owned()))
    }

    #[test]
    fn arguments() {
        assert_eq!(parse(&[]), Args::default());
        assert_eq!(
            parse(&["--username", "Z", "--quickPlayMultiplayer", "mc.example.net:25570"]).direct,
            Some(("mc.example.net".into(), 25570, "Z".into()))
        );
        assert_eq!(parse(&["--username", "Bob"]).username.as_deref(), Some("Bob"));
        assert_eq!(parse(&["--username", "Bob"]).direct, None);
        assert_eq!(parse(&["127.0.0.1", "25570", "X"]).direct, Some(("127.0.0.1".into(), 25570, "X".into())));
        assert_eq!(parse(&["h"]).direct, Some(("h".into(), 25565, "Rustcraft".into())));
        assert_eq!(parse(&["h", "1", "--username", "Y"]).direct, Some(("h".into(), 1, "Y".into())));
    }
}
