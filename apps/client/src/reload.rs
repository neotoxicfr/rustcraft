//! Resource reload in game (`docs/specs/resource-packs.md` § 6): new stack read
//! on a background thread (cache of the baked resources per stack), loading screen, then everything is
//! replaced and sent back to the GPU: renderer recreated on the same window, sections re-meshed.

use std::sync::Arc;
use std::time::Instant;

use winit::event::WindowEvent;

use crate::gui::Gui;
use crate::resources::Loaded;
use crate::{App, audio_slot, pack_stack, particles, resources, skins};

/// Reload in progress or in exit fade.
pub(crate) struct Reload {
    /// Resources and packs removed, and audio engine built by the same thread
    /// (`RUSTCRAFT_OPT_RELOADAUDIO`).
    rx: crossbeam_channel::Receiver<((Loaded, resources::Dropped), Option<rc_audio::AudioEngine>)>,
    start: Instant,
    /// Resources applied (start of the exit fade).
    done: Option<Instant>,
    /// Displayed progress (smoothed).
    shown: f32,
}

/// State of the resource packs in `App`.
#[derive(Default)]
pub(crate) struct Packs {
    pub stack: pack_stack::PackStack,
    pub reload: Option<Reload>,
    /// A reload was requested during the previous one: restarted at its end.
    again: bool,
    /// `RUSTCRAFT_RELOADTEST` demo done.
    test_done: bool,
    /// Registries received from the server (entity variants), handed back to the new renderer.
    pub registries: Vec<(String, rc_client::RegistryEntries)>,
    /// Reloads applied (the server packs wait for theirs).
    pub generation: u64,
    /// Packs imposed by the server (`server_packs.rs`).
    pub server: crate::server_packs::ServerPacks,
    /// "Resource Packs" screen open (`pack_screen.rs`) and step of its demo.
    pub screen: Option<Box<crate::pack_screen::PackScreen>>,
    pub screen_test: u8,
    /// Loading failed with the chosen packs (they were removed): notification to show.
    pub load_failed: bool,
}

/// Typical duration of a reload, for the bar (the real progress is not measured).
const EXPECTED_SECS: f32 = 3.0;
const FADE_IN_MS: f32 = 500.0;
const FADE_OUT_MS: f32 = 1000.0;

impl Packs {
    pub fn new(stack: pack_stack::PackStack, load_failed: bool) -> Self {
        Packs { stack, load_failed, ..Default::default() }
    }

    /// Registry received: kept for the next renderer (the last of each name).
    pub fn remember_registry(&mut self, name: &str, entries: &rc_client::RegistryEntries) {
        self.registries.retain(|(n, _)| n != name);
        self.registries.push((name.to_owned(), entries.clone()));
    }

    /// A reload is in progress or requested (not yet applied).
    pub fn reloading_pending(&self) -> bool {
        self.again || self.reload.as_ref().is_some_and(|r| r.done.is_none())
    }

    /// Drawing over everything else: server packs prompt, loading screen.
    pub fn draw(&mut self, gui: &mut Gui, q: &wgpu::Queue, o: &crate::options::Options, mouse: Option<(f32, f32)>) {
        if let Some(s) = &mut self.screen {
            crate::pack_screen::draw(s, gui, q, o, mouse);
        }
        self.server.draw(gui, q, o, mouse);
        if let Some(r) = &mut self.reload {
            r.draw(gui, o.b("darkMojangStudiosBackground"));
        }
    }
}

impl Reload {
    /// Loading screen: brand background, logo, bar (the game's, in our terms).
    pub fn draw(&mut self, gui: &mut Gui, dark: bool) {
        let (w, h) = (gui.width, gui.height);
        let ms = |t: Instant| t.elapsed().as_secs_f32() * 1000.0;
        let fade_in = ms(self.start) / FADE_IN_MS;
        let fade_out = self.done.map_or(-1.0, |d| ms(d) / FADE_OUT_MS);
        let (bg_alpha, logo_alpha) = if fade_out >= 0.0 {
            let a = 1.0 - (fade_out - 1.0).clamp(0.0, 1.0);
            (a, a)
        } else {
            (fade_in.clamp(0.15, 1.0), fade_in.clamp(0.0, 1.0))
        };
        let brand: u32 = if dark { 0x000000 } else { 0xEF323D };
        let alpha = |a: f32| ((a * 255.0).ceil() as u32).min(255) << 24;
        gui.next_layer();
        gui.fill(0.0, 0.0, w, h, alpha(bg_alpha) | brand);
        let (cx, cy) = ((w * 0.5).floor(), (h * 0.5).floor());
        let logo_h = (w * 0.75).min(h) * 0.25;
        let half_h = (logo_h * 0.5).floor();
        let half_w = (logo_h * 4.0 * 0.5).floor();
        // Logo as a weighted add by its alpha (white adds to the background), alpha truncated; each
        // half shifted by a sixteenth of a texel over 120 (to the left then the right), like the game.
        let white = (((logo_alpha * 255.0).floor() as u32).min(255) << 24) | 0xFF_FFFF;
        let du = 0.0625 / 120.0;
        gui.stratum(crate::gui::Blend::AdditiveAlpha);
        gui.sprite_smooth(
            [cx - half_w, cy - half_h, cx, cy - half_h + logo_h.floor()],
            [-du, 0.0, 1.0 - du, 0.5],
            white,
        );
        gui.sprite_smooth(
            [cx, cy - half_h, cx + half_w, cy - half_h + logo_h.floor()],
            [du, 0.5, 1.0 + du, 1.0],
            white,
        );
        gui.stratum(crate::gui::Blend::Normal);
        // Progress bar (smoothed like the game), faded out during the first second of the exit.
        let actual =
            if self.done.is_some() { 1.0 } else { (self.start.elapsed().as_secs_f32() / EXPECTED_SECS).min(0.9) };
        self.shown = (self.shown * 0.95 + actual * 0.05).clamp(0.0, 1.0);
        if fade_out < 1.0 {
            let fade = 1.0 - fade_out.clamp(0.0, 1.0);
            let c = ((fade * 255.0).round() as u32) << 24 | 0xFF_FFFF;
            let bar_y = (h * 0.8325).floor();
            let (x0, y0, x1, y1) = (cx - half_w, bar_y - 5.0, cx + half_w, bar_y + 5.0);
            let filled = ((x1 - x0 - 2.0) * self.shown).ceil();
            gui.fill(x0 + 2.0, y0 + 2.0, x0 + filled, y1 - 2.0, c);
            gui.fill(x0 + 1.0, y0, x1 - 1.0, y0 + 1.0, c);
            gui.fill(x0 + 1.0, y1 - 1.0, x1 - 1.0, y1, c);
            gui.fill(x0, y0, x0 + 1.0, y1, c);
            gui.fill(x1 - 1.0, y0, x1, y1, c);
        }
    }

    /// Exit fade finished.
    fn finished(&self) -> bool {
        self.done.is_some_and(|d| d.elapsed().as_secs_f32() * 1000.0 >= 2.0 * FADE_OUT_MS)
    }
}

impl App {
    /// Reloads all the resources with the current stack (server packs, player's choice).
    pub(crate) fn reload_resources(&mut self) {
        if self.packs.reload.as_ref().is_some_and(|r| r.done.is_none()) {
            self.packs.again = true;
            return;
        }
        let (data, (server, player), lang) =
            (self.data.clone(), self.packs.stack.split_paths(), self.options.s("lang").to_owned());
        eprintln!("resource reload: {} pack(s)", server.len() + player.len());
        // Audio engine of the new stack built during baking, on the chosen device
        // (`RUSTCRAFT_OPT_RELOADAUDIO`); the swap stays at the same instant (`apply_loaded`).
        let aopt = audio_slot::opt();
        let audio = (aopt.reload && self.audio.is_on())
            .then(|| (self.options.s("soundDevice").to_owned(), audio_slot::volumes(&self.options), aopt.device1));
        let (tx, rx) = crossbeam_channel::bounded(1);
        let spawned = std::thread::Builder::new().name("resources".into()).spawn(move || {
            // Engine started as soon as each tried stack is indexed (`AUDIOEARLY`), replaced for the
            // next stack if this one is abandoned; on the retained stack only with `=0`.
            let make = rc_audio::AudioEngine::with_device;
            let _ = tx.send(audio_slot::load_with_engine(audio, make, |hook| {
                resources::load_recovering(&data, &server, &player, &lang, hook)
            }));
        });
        if let Err(e) = spawned {
            eprintln!("resource reload: {e}");
            return;
        }
        self.packs.reload = Some(Reload { rx, start: Instant::now(), done: None, shown: 0.0 });
    }

    /// On every frame, before drawing: resources ready → applied; end of the fade.
    pub(crate) fn reload_tick(&mut self) {
        // Demo: `RUSTCRAFT_RELOADTEST=<s>` chooses all the packs of the folder after
        // `s` seconds and reloads.
        if !self.packs.test_done
            && let Some(s) = rc_data::env_str!("RUSTCRAFT_RELOADTEST").and_then(|v| v.parse::<f64>().ok())
            && self.started.elapsed().as_secs_f64() >= s
        {
            self.packs.test_done = true;
            let st = &mut self.packs.stack;
            st.rescan();
            let files: Vec<String> = st
                .known
                .iter()
                .filter(|p| p.id.starts_with("file/") && !st.selected.contains(&p.id))
                .map(|p| p.id.clone())
                .collect();
            st.selected.extend(files);
            self.reload_resources();
        }
        if self.packs.load_failed
            && let Some(r) = &self.renderer
        {
            self.packs.load_failed = false;
            let title = rc_text::Component::translate("resourcePack.load_fail", vec![]);
            self.toasts.system(&r.gui, &title, None, 5000.0);
        }
        self.server_packs_tick();
        self.pack_screen_test();
        self.pack_screen_tick();
        let Some(r) = &mut self.packs.reload else { return };
        if r.done.is_none() {
            // The game shows the logo for at least a second before leaving.
            if r.start.elapsed().as_secs_f32() * 1000.0 < 2.0 * FADE_IN_MS {
                return;
            }
            let Ok(((l, dropped), audio)) = r.rx.try_recv() else { return };
            r.done = Some(Instant::now());
            if dropped != resources::Dropped::Nothing {
                // Like the game: packs removed from the settings, notification after the reload.
                let all = dropped == resources::Dropped::All;
                self.packs.stack.drop_after_failure(all, &mut self.options);
                if all {
                    self.server_packs_dropped();
                }
                self.save_options();
                self.packs.load_failed = true;
            }
            let t = Instant::now();
            self.apply_loaded(l, audio);
            eprintln!("resource reload: applied in {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
            self.packs.generation += 1;
            if std::mem::take(&mut self.packs.again) {
                self.packs.reload = None;
                self.reload_resources();
            }
        } else if r.finished() {
            self.packs.reload = None;
        }
    }

    /// Input taken by the loading screen (keyboard, mouse); window and focus pass through.
    pub(crate) fn reload_blocks_input(&self, e: &WindowEvent) -> bool {
        self.packs.reload.as_ref().is_some_and(|r| !r.finished())
            && matches!(
                e,
                WindowEvent::KeyboardInput { .. }
                    | WindowEvent::MouseInput { .. }
                    | WindowEvent::MouseWheel { .. }
                    | WindowEvent::DroppedFile(_)
            )
    }

    /// Replaces the resources and recreates the renderer on the same window.
    fn apply_loaded(&mut self, l: Loaded, audio: Option<rc_audio::AudioEngine>) {
        let window = self.renderer.as_ref().map(|r| r.window.clone());
        self.models = l.models;
        self.textures = l.textures;
        self.tables = l.tables;
        self.colormaps = l.colormaps;
        self.sky_assets = l.sky_assets;
        self.panorama_assets = l.panorama_assets;
        self.gui_assets = Some(l.gui_assets);
        self.entity_textures = Some(l.entity_textures);
        let level = self.particles.level;
        self.particles = particles::Particles::new(l.particle_assets, &self.data);
        self.particles.level = level;
        self.weather.images = Some(l.weather_images);
        self.weather.built = None;
        self.menus.languages = l.languages;
        self.clocks = vec![rc_assets::AnimationClock::default(); self.models.animations.len()];
        self.assets = l.assets;
        self.palette = Arc::new(rc_mesh::blocks::biome_palette(&self.data, &self.colormaps, &self.biome_names));
        // Sounds: new engine on the new stack (running sounds stop). Built by
        // the "resources" thread (`RELOADAUDIO`), otherwise here; the old one released in the background or here.
        if let Some(a) = self.audio.get() {
            a.stop_all();
            self.entity_sounds.clear();
            let aopt = audio_slot::opt();
            let (device, volumes) = (self.options.s("soundDevice"), audio_slot::volumes(&self.options));
            let make = rc_audio::AudioEngine::with_device;
            let new = audio_slot::reload_engine(audio, &self.assets, device, &volumes, aopt.device1, make);
            audio_slot::retire(std::mem::replace(a, new), aopt.reload);
        }
        // Skins requested again (disk cache) for the new entity atlas.
        self.skins = skins::Skins::new(rc_data::cache_root());
        self.skins.update(&self.players, self.local_uuid);
        self.pending_registries = self.packs.registries.clone();
        if let Some(w) = window {
            // The old engine (device, surface) is released before creating another.
            self.renderer = None;
            self.build_renderer(w);
            if let Some(r) = &mut self.renderer {
                r.shade = self.shade;
            }
        }
        // All the columns re-meshed with the new faces (old meshes in flight ignored).
        let cols: Vec<(i32, i32)> = self.world.grid.iter().map(|c| (c.x, c.z)).collect();
        for (x, z) in cols {
            self.world.touch(x, z);
        }
        self.apply_options();
    }

    /// Renderer on `window` with the pending resources (startup, reload).
    pub(crate) fn build_renderer(&mut self, window: Arc<winit::window::Window>) {
        let Some(gui_assets) = self.gui_assets.take() else { return };
        let entity_assets =
            self.entity_textures.take().filter(|_| !self.data.entity_models.is_empty()).map(|t| (self.data.clone(), t));
        let mut r =
            crate::Renderer::new(
                window,
                &self.models,
                &self.textures,
                &self.sky_assets,
                gui_assets,
                entity_assets,
                &self.tables,
            );
        self.sky_assets = crate::sky::SkyAssets::default();
        self.textures = Vec::new();
        if let Some(p) = self.panorama_assets.take() {
            r.enable_panorama(&p);
        }
        if let Some(i) = self.weather.images.take() {
            r.weather.set_images(i);
        }
        r.enable_overlays(self.assets.clone(), self.models.clone(), self.data.clone());
        r.load_block_fx(&self.assets);
        crate::memstats::step("renderer creation");
        if !r.wireframe {
            eprintln!("wireframe mode not supported by the adapter: solid rendering");
        }
        eprintln!("adapter: {}; pool: {} threads", r.adapter_name, rayon::current_num_threads());
        // Items in text: sprites of the atlases and skins read on demand.
        r.gui.objects.assets = Some(self.assets.clone());
        self.renderer = Some(r);
        self.apply_options();
        // System details for a crash report written by the panic hook.
        crate::crash_report::set_system(self.system_lines());
        // Periodic reminders for the user's country, timer started now (`toasts.rs`).
        let json = self.assets.read("assets/minecraft/regional_compliancies.json").unwrap_or_default();
        let now = self.started.elapsed().as_secs_f64() * 1000.0;
        self.toasts.periodic = crate::toasts::Periodic::new(&json, crate::debug_sys::country_iso3().as_deref(), now);
    }
}
