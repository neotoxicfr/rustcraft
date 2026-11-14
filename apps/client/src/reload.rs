//! Resource reload in game (`docs/specs/resource-packs.md` § 6): new stack read
//! on a background thread (cache of the baked resources per stack), loading screen, then everything is
//! replaced and sent back to the GPU: renderer recreated on the same window, sections re-meshed.
//! The GPU context (surface, adapter, device) is kept from one renderer to the next
//! (`RUSTCRAFT_OPT_KEEP_GPU_ON_RELOAD`, on by default; `=0`: driver reopened as before).

use std::sync::Arc;
use std::time::Instant;

use winit::event::WindowEvent;

use crate::gpu_backend::GpuContext;
use crate::gui::Gui;
use crate::resources::Loaded;
use crate::{App, audio_slot, pack_stack, particles, resources, skins};

/// Why a renderer is built, and the GPU context it starts from.
pub(crate) enum RendererBuild {
    /// First renderer: device opened early in the background or picked now; the session events
    /// received meanwhile may be applied between its phases.
    Startup,
    /// Resource reload: the old renderer's context when it is kept, otherwise picked again.
    Reload { kept_gpu: Option<GpuContext> },
}

/// The renderer reuses the old one's GPU context on a reload (`RUSTCRAFT_OPT_KEEP_GPU_ON_RELOAD`).
fn keep_gpu_on_reload() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_KEEP_GPU_ON_RELOAD")
}

/// The exit fade starts once the new resources are applied, like the game, instead of when they
/// arrive (`RUSTCRAFT_OPT_RELOAD_FADE_AFTER_APPLY`; `=0`: the application ate the start of the fade).
fn fade_after_apply() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_RELOAD_FADE_AFTER_APPLY")
}

/// Instant the exit fade starts from: the end of the application, or the arrival of the resources.
fn fade_start(received: Instant, applied: Instant, after_apply: bool) -> Instant {
    if after_apply { applied } else { received }
}

/// Resources and packs removed, and audio engine built by the same thread
/// (`RUSTCRAFT_OPT_RELOADAUDIO`).
type Reloaded = ((Loaded, resources::Dropped), Option<rc_audio::AudioEngine>);

/// Reload in progress or in exit fade; also the startup screen, whose loading is already over.
pub(crate) struct Reload {
    /// `None`: startup screen, nothing left to receive.
    rx: Option<crossbeam_channel::Receiver<Reloaded>>,
    /// Steps done by the loading thread (the bar).
    progress: Arc<resources::LoadProgress>,
    start: Instant,
    /// Resources applied (start of the exit fade).
    done: Option<Instant>,
    /// Displayed progress (smoothed).
    shown: f32,
    /// Fades in over what was on screen (reload); the startup screen is opaque from the start.
    fade_in: bool,
}

impl Reload {
    /// The game's screen at launch: Mojang Studios logo on the brand background, no fade-in. The
    /// resources are loaded before the window opens, so its bar is full and its exit (1 s for the
    /// bar, then 1 s for the background and logo over the title screen) starts at once.
    pub fn startup() -> Reload {
        let progress = Arc::new(resources::LoadProgress::default());
        Reload { rx: None, progress, start: Instant::now(), done: None, shown: 1.0, fade_in: false }
    }

    /// Opaque over the screens below: from the end of the fade-in (reload; the whole time at
    /// startup) to the first second of the exit. The game draws no screen then, so a screen's own
    /// opening fade (title screen) only starts once the loading screen thins out.
    pub fn hides_screens(&self) -> bool {
        let opaque_since_start = !self.fade_in || self.start.elapsed().as_secs_f32() * 1000.0 >= FADE_IN_MS;
        let exit_ms = self.done.map_or(0.0, |d| d.elapsed().as_secs_f32() * 1000.0);
        opaque_since_start && exit_ms < FADE_OUT_MS
    }
}

/// Screen drawn under the overlays outside a world ([`Packs::screen_outside_world`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScreenOutsideWorld {
    /// Covered by the opaque loading screen: nothing drawn, no opening fade started.
    Hidden,
    PacksScreen,
    Menus,
    /// Title screen and the other screens outside a world (`front.rs`).
    Front,
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

const FADE_IN_MS: f32 = 500.0;
/// Share of the displayed bar kept at each frame (the rest moves toward the real progress).
const BAR_SMOOTHING: f32 = 0.95;
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

    /// The loading screen covers every screen below it (see [`Reload::hides_screens`]).
    pub fn screens_hidden(&self) -> bool {
        self.reload.as_ref().is_some_and(Reload::hides_screens)
    }

    /// Screen the frame outside a world draws under the overlays: none while the loading screen
    /// covers them (the title screen's opening fade then starts once it thins out, like the game),
    /// otherwise the packs screen (drawn by [`Packs::draw`]), an options menu or the front screens.
    pub fn screen_outside_world(&self, menus_open: bool) -> ScreenOutsideWorld {
        if self.screens_hidden() {
            ScreenOutsideWorld::Hidden
        } else if self.screen.is_some() {
            ScreenOutsideWorld::PacksScreen
        } else if menus_open {
            ScreenOutsideWorld::Menus
        } else {
            ScreenOutsideWorld::Front
        }
    }

    /// Drawing over everything else: packs screen, server packs prompt (both hidden under an
    /// opaque loading screen, like any screen in the game), loading screen.
    pub fn draw(&mut self, gui: &mut Gui, q: &wgpu::Queue, o: &crate::options::Options, mouse: Option<(f32, f32)>) {
        if !self.screens_hidden() {
            if let Some(s) = &mut self.screen {
                crate::pack_screen::draw(s, gui, q, o, mouse);
            }
            self.server.draw(gui, q, o, mouse);
        }
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
        let fade_out = self.done.map_or(-1.0, |d| ms(d) / FADE_OUT_MS);
        let (bg_alpha, logo_alpha) = screen_alphas(ms(self.start), self.done.map(ms), self.fade_in);
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
        // Progress bar, faded out during the first second of the exit.
        self.advance_bar();
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

    /// Bar of this frame, smoothed like the game toward the share of loading steps done (all of
    /// them once the resources are applied).
    fn advance_bar(&mut self) -> f32 {
        let actual = if self.done.is_some() { 1.0 } else { self.progress.fraction() };
        self.shown = (self.shown * BAR_SMOOTHING + actual * (1.0 - BAR_SMOOTHING)).clamp(0.0, 1.0);
        self.shown
    }

    /// Exit fade finished.
    fn finished(&self) -> bool {
        self.done.is_some_and(|d| d.elapsed().as_secs_f32() * 1000.0 >= 2.0 * FADE_OUT_MS)
    }
}

/// Opacity of the background and of the logo, `since_start` ms after the reload began and
/// `since_done` ms after the exit fade started (`None` while loading); without `fade_in`
/// (startup screen), opaque until the second second of the exit.
fn screen_alphas(since_start: f32, since_done: Option<f32>, fade_in: bool) -> (f32, f32) {
    match since_done {
        Some(d) => {
            let a = 1.0 - (d / FADE_OUT_MS - 1.0).clamp(0.0, 1.0);
            (a, a)
        }
        None if !fade_in => (1.0, 1.0),
        None => {
            let fade_in = since_start / FADE_IN_MS;
            (fade_in.clamp(0.15, 1.0), fade_in.clamp(0.0, 1.0))
        }
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
        // Kept if the new translations fail, like the game.
        let lang_in_use = self.renderer.as_ref().map(|r| r.gui.lang.clone());
        eprintln!("resource reload: {} pack(s)", server.len() + player.len());
        // Audio engine of the new stack built during baking, on the chosen device
        // (`RUSTCRAFT_OPT_RELOADAUDIO`); the swap stays at the same instant (`apply_loaded`).
        let aopt = audio_slot::opt();
        let audio = (aopt.reload && self.audio.is_on())
            .then(|| (self.options.s("soundDevice").to_owned(), audio_slot::volumes(&self.options), aopt.device1));
        let (tx, rx) = crossbeam_channel::bounded(1);
        let progress = Arc::new(resources::LoadProgress::default());
        let steps = progress.clone();
        let spawned = std::thread::Builder::new().name("resources".into()).spawn(move || {
            // Engine started as soon as each tried stack is indexed (`AUDIOEARLY`), replaced for the
            // next stack if this one is abandoned; on the retained stack only with `=0`.
            let make = rc_audio::AudioEngine::with_device;
            let lang = resources::LanguageToLoad { code: &lang, in_use: lang_in_use.as_ref() };
            let _ = tx.send(audio_slot::load_with_engine(audio, make, |hook| {
                resources::load_recovering(&data, &server, &player, &lang, &steps, hook)
            }));
        });
        if let Err(e) = spawned {
            eprintln!("resource reload: {e}");
            return;
        }
        let start = Instant::now();
        self.packs.reload = Some(Reload { rx: Some(rx), progress, start, done: None, shown: 0.0, fade_in: true });
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
            // Startup screen: everything was loaded before the window opened, the exit starts.
            let Some(rx) = &r.rx else {
                r.done = Some(Instant::now());
                if std::mem::take(&mut self.packs.again) {
                    self.reload_resources();
                }
                return;
            };
            // The game shows the logo for at least a second before leaving.
            if r.start.elapsed().as_secs_f32() * 1000.0 < 2.0 * FADE_IN_MS {
                return;
            }
            let Ok(((l, dropped), audio)) = rx.try_recv() else { return };
            let received = Instant::now();
            r.done = Some(received);
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
            if let Some(r) = &mut self.packs.reload {
                r.done = Some(fade_start(received, Instant::now(), fade_after_apply()));
            }
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
            // The old renderer is released before the new one is built: one swap chain per window
            // (DXGI, and Vulkan refuses a second surface in use on the same window), and its video
            // memory given back first. A kept context reuses the same surface and device.
            let old = self.renderer.take();
            let kept_gpu = old.filter(|_| keep_gpu_on_reload()).map(crate::Renderer::into_gpu_context);
            if let Some(gpu) = &kept_gpu {
                crate::render::wait_for_released_gpu_memory(&gpu.device);
            }
            self.build_renderer(w, RendererBuild::Reload { kept_gpu });
        }
        // All the columns re-meshed with the new faces (old meshes in flight ignored).
        let cols: Vec<(i32, i32)> = self.world.grid.iter().map(|c| (c.x, c.z)).collect();
        for (x, z) in cols {
            self.world.touch(x, z);
        }
        self.apply_options();
    }

    /// GPU context of the first renderer: the device opened early in the background when it is the
    /// one the order of the APIs gives, otherwise the APIs tried now on the window.
    fn startup_gpu(&mut self, window: &Arc<winit::window::Window>) -> GpuContext {
        let early = self.early_gpu.take().and_then(|e| e.join(window));
        early.unwrap_or_else(|| crate::gpu_backend::pick(window))
    }

    /// Renderer on `window` with the pending resources (startup, reload).
    pub(crate) fn build_renderer(&mut self, window: Arc<winit::window::Window>, how: RendererBuild) {
        let Some(gui_assets) = self.gui_assets.take() else { return };
        let (gpu, startup) = match how {
            RendererBuild::Startup => (self.startup_gpu(&window), true),
            RendererBuild::Reload { kept_gpu } => {
                (kept_gpu.unwrap_or_else(|| crate::gpu_backend::pick(&window)), false)
            }
        };
        let entity_assets =
            self.entity_textures.take().filter(|_| !self.data.entity_models.is_empty()).map(|t| (self.data.clone(), t));
        // Owned here so that the events applied between the build phases may borrow the app.
        let (models, tables) = (self.models.clone(), self.tables.clone());
        let textures = std::mem::take(&mut self.textures);
        let sky = std::mem::take(&mut self.sky_assets);
        let assets = crate::render::RendererAssets {
            models: &models,
            textures: &textures,
            sky: &sky,
            gui: gui_assets,
            entities: entity_assets,
            tables: &tables,
        };
        let events_meanwhile = startup && crate::events_before_renderer::enabled();
        let mut r = crate::Renderer::new(window, gpu, assets, &mut || {
            if events_meanwhile {
                self.apply_events_before_renderer();
            }
        });
        drop((textures, sky));
        // State the events may have set before the renderer existed (always the case on a reload):
        // shading of the dimension, entity lights of the Nether.
        r.shade = self.shade;
        // Columns loaded before it, every one of them on a reload: walked until they are meshed.
        for col in self.world.grid.iter() {
            r.note_loaded_column(col.x, col.z, &col.sections);
        }
        if self.shade == rc_mesh::wire::SHADE_NETHER
            && let Some(e) = &r.entities
        {
            e.set_dimension(&r.queue_handle(), true);
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Opacity of the loading screen on the frames that follow the application of the resources
    /// (60 Hz, 2.3 s): with the fade started after the application it does not depend on how long
    /// the application lasted, like the game; started at their arrival, a 300 ms application ate
    /// the start of the fade. Sabotage checked: `fade_start` returning the arrival fails the first
    /// assertion.
    #[test]
    fn fade_curve_does_not_depend_on_the_apply_duration() {
        let curve = |apply_ms: u64, after_apply: bool| {
            let received = Instant::now();
            let applied = received + Duration::from_millis(apply_ms);
            let done = fade_start(received, applied, after_apply);
            let since_done = |k: u64| (applied + Duration::from_millis(16 * k) - done).as_secs_f32() * 1000.0;
            (0..140).map(|k| screen_alphas(10_000.0, Some(since_done(k)), true)).collect::<Vec<_>>()
        };
        assert_eq!(curve(300, true), curve(0, true), "fade after the application");
        assert_eq!(curve(0, false), curve(0, true), "instant application: both starts agree");
        assert_ne!(curve(300, false), curve(0, false), "the old start loses the application time");
        // While loading: the background rises from 15 % and the logo from 0 over half a second.
        assert_eq!(screen_alphas(0.0, None, true), (0.15, 0.0));
        assert_eq!(screen_alphas(250.0, None, true), (0.5, 0.5));
        assert_eq!(screen_alphas(900.0, None, true), (1.0, 1.0));
    }

    /// The launch screen exists (there was none: the title screen came at once) and is opaque from
    /// its first frame, without the reload's fade-in; it covers the screens until the first second
    /// of its exit is over, then thins out over the title screen during the next one.
    #[test]
    fn startup_screen_is_opaque_then_fades_over_the_title() {
        assert_eq!(screen_alphas(0.0, None, false), (1.0, 1.0), "no fade-in");
        assert_eq!(screen_alphas(0.0, Some(500.0), false), (1.0, 1.0), "bar fading, background still opaque");
        assert_eq!(screen_alphas(0.0, Some(1500.0), false), (0.5, 0.5));
        assert_eq!(screen_alphas(0.0, Some(2000.0), false), (0.0, 0.0));
        let mut s = Reload::startup();
        assert!(s.hides_screens(), "title screen hidden behind the opaque logo");
        assert_eq!(s.shown, 1.0, "the loading is over: full bar");
        // Only elapsed times past a bound are checked: a slow machine can only make them longer.
        s.done = Some(Instant::now() - Duration::from_millis(1200));
        assert!(!s.hides_screens(), "second second of the exit: the title screen shows through");
        s.done = Some(Instant::now() - Duration::from_millis(2100));
        assert!(s.finished());
    }

    /// Outside a world, the frame draws no screen under the opaque launch screen (it drew the title
    /// screen from the first frame, whose opening fade then ran behind the logo and ended 1 s early),
    /// and the title screen or a menu once the launch screen thins out; the same for the screens
    /// the overlays draw themselves (packs screen, server packs prompt).
    #[test]
    fn opaque_launch_screen_hides_the_screens_outside_a_world() {
        let mut packs = Packs::default();
        assert_eq!(packs.screen_outside_world(false), ScreenOutsideWorld::Front, "no loading screen");
        packs.reload = Some(Reload::startup());
        assert!(packs.screens_hidden());
        assert_eq!(packs.screen_outside_world(false), ScreenOutsideWorld::Hidden, "title screen under the logo");
        assert_eq!(packs.screen_outside_world(true), ScreenOutsideWorld::Hidden, "menu under the logo");
        // Only elapsed times past a bound are checked: a slow machine can only make them longer.
        packs.reload.as_mut().expect("launch screen").done = Some(Instant::now() - Duration::from_millis(1200));
        assert!(!packs.screens_hidden());
        assert_eq!(packs.screen_outside_world(false), ScreenOutsideWorld::Front, "title screen shows through");
        assert_eq!(packs.screen_outside_world(true), ScreenOutsideWorld::Menus);
    }

    /// The reload bar follows the steps of the load instead of an assumed duration of 3 s: with no
    /// step done after 10 s it stays empty (the old bar crept toward 90 %), and once applied it
    /// heads for the end.
    #[test]
    fn reload_bar_follows_the_steps_done() {
        let mut r = Reload::startup();
        (r.fade_in, r.shown, r.start) = (true, 0.0, Instant::now() - Duration::from_secs(10));
        for _ in 0..100 {
            r.advance_bar();
        }
        assert_eq!(r.shown, 0.0);
        r.done = Some(Instant::now());
        let first = r.advance_bar();
        assert!((first - 0.05).abs() < 1e-6, "smoothed step {first}");
    }
}
