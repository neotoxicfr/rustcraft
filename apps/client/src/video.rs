//! Video settings applied outside the renderer: vertical sync, frame limit
//! (`maxFps`, `inactivityFpsLimit`), exclusive fullscreen and its resolution
//! (`docs/specs/option-menus.md` § 3).

use std::time::{Duration, Instant};

/// Forcing of the sync by `RUSTCRAFT_VSYNC=0|1` (measurements), otherwise the setting decides.
pub fn vsync_forced() -> Option<bool> {
    match rc_data::env_str!("RUSTCRAFT_VSYNC") {
        Some("1") => Some(true),
        Some("0") => Some(false),
        _ => None,
    }
}

/// Mode requested from the surface and mode actually retained by wgpu for `Auto*` (same order
/// of preference as wgpu), for the F3 screen.
pub fn present_modes(caps: &[wgpu::PresentMode], vsync: bool) -> (wgpu::PresentMode, wgpu::PresentMode) {
    use wgpu::PresentMode as P;
    let pick = |order: [P; 2]| order.into_iter().find(|m| caps.contains(m)).unwrap_or(P::Fifo);
    if vsync {
        // Game sync interval 1 = strict queue (Fifo, always available): AutoVsync's "relaxed"
        // mode lets late frames through without waiting and ran at
        // several thousand frames per second on NVIDIA.
        (P::Fifo, P::Fifo)
    } else {
        (P::AutoNoVsync, pick([P::Immediate, P::Mailbox]))
    }
}

/// "Unlimited" value of the frame limit slider.
pub const UNLIMITED: i32 = 260;

/// The game's frame limiter: effective limit according to activity, then wait for the end of the
/// frame duration (sleep corrected by the mean overshoot, end in a short busy wait).
pub struct FrameLimiter {
    last_input: Instant,
}

impl Default for FrameLimiter {
    fn default() -> Self {
        FrameLimiter { last_input: Instant::now() }
    }
}

/// State of the window and the game that sets the effective limit.
pub struct LimitIn {
    /// `maxFps` setting (10..=260).
    pub max_fps: i32,
    /// `inactivityFpsLimit` setting: "afk" (default) or "minimized".
    pub afk_limit: bool,
    pub minimized: bool,
    pub focused: bool,
    pub exclusive_fullscreen: bool,
    /// A world is loaded (otherwise out-of-world menus: 60 frames per second).
    pub in_world: bool,
}

impl FrameLimiter {
    /// Keyboard or mouse input: leaves inactivity.
    pub fn input(&mut self) {
        self.last_input = Instant::now();
    }

    /// Effective limit (frames per second); `UNLIMITED` = none.
    pub fn limit(&self, i: &LimitIn) -> i32 {
        // Minimised window, or exclusive fullscreen without focus: 10.
        if i.minimized || (!i.focused && i.exclusive_fullscreen) {
            return 10;
        }
        if i.afk_limit {
            let afk = self.last_input.elapsed();
            if afk > Duration::from_secs(600) {
                return 10;
            }
            if afk > Duration::from_secs(60) {
                return i.max_fps.min(30);
            }
        }
        if i.in_world { i.max_fps } else { 60 }
    }
}

/// Resources of the terrain camera binding (kept to redo it when the block texture
/// sampler changes).
pub struct TerrainRes {
    pub layout: wgpu::BindGroupLayout,
    pub quads: wgpu::Buffer,
    pub atlas: wgpu::TextureView,
    pub big: wgpu::TextureView,
    pub anim: wgpu::Buffer,
    pub fade: wgpu::Buffer,
    pub lightmap: wgpu::TextureView,
    pub lightmap_sampler: wgpu::Sampler,
    /// Model offsets of the compact faces (binding 9).
    pub offsets: wgpu::Buffer,
    /// Mip levels and anisotropy of the sampler in place.
    pub key: (u32, u16),
}

impl TerrainRes {
    /// `camera`: camera slot (whole buffer or fixed offset of a grouped buffer).
    pub fn bind(
        &self,
        device: &wgpu::Device,
        camera: wgpu::BindingResource<'_>,
        sampler: &wgpu::Sampler,
    ) -> wgpu::BindGroup {
        use wgpu::BindingResource as R;
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: camera },
                wgpu::BindGroupEntry { binding: 1, resource: self.quads.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: R::TextureView(&self.atlas) },
                wgpu::BindGroupEntry { binding: 3, resource: R::Sampler(sampler) },
                wgpu::BindGroupEntry { binding: 4, resource: self.anim.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 5, resource: R::TextureView(&self.big) },
                wgpu::BindGroupEntry { binding: 6, resource: self.fade.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 7, resource: R::TextureView(&self.lightmap) },
                wgpu::BindGroupEntry { binding: 8, resource: R::Sampler(&self.lightmap_sampler) },
                wgpu::BindGroupEntry { binding: 9, resource: self.offsets.as_entire_binding() },
            ],
        })
    }
}

/// Block texture sampler, like the game's: linear, clamped edges,
/// mip levels limited to `mips` (`mipmapLevels`), anisotropy `aniso` (1 = none).
pub fn atlas_sampler(device: &wgpu::Device, mips: u32, aniso: u16) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("block textures"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        lod_max_clamp: mips as f32,
        anisotropy_clamp: aniso.max(1),
        ..Default::default()
    })
}

/// Video mode of exclusive fullscreen (`fullscreenResolution`, the game's JSON): width,
/// height, red, green, blue bits, frequency (Hz).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoMode {
    pub width: u32,
    pub height: u32,
    pub bits: [u16; 3],
    pub refresh: f32,
}

impl VideoMode {
    /// Tolerant reading of the game's JSON (`{"width":…,"height":…,"red_bits":…,…}`).
    pub fn read(s: &str) -> Option<VideoMode> {
        let v: serde_json::Value = serde_json::from_str(s).ok()?;
        let int = |k: &str| v.get(k).and_then(serde_json::Value::as_u64);
        Some(VideoMode {
            width: u32::try_from(int("width")?).ok()?,
            height: u32::try_from(int("height")?).ok()?,
            bits: [int("red_bits")?, int("green_bits")?, int("blue_bits")?].map(|b| b as u16),
            refresh: v.get("refresh_rate")?.as_f64()? as f32,
        })
    }

    pub fn write(&self) -> String {
        format!(
            "{{\"width\":{},\"height\":{},\"red_bits\":{},\"green_bits\":{},\"blue_bits\":{},\"refresh_rate\":{:?}}}",
            self.width, self.height, self.bits[0], self.bits[1], self.bits[2], self.refresh
        )
    }

    fn of(m: &winit::monitor::VideoModeHandle) -> VideoMode {
        let size = m.size();
        let depth = m.bit_depth();
        // Depth 32 (Windows) or 24: 8 bits per component, like the game's XRGB8888 format.
        let c = if matches!(depth, 24 | 32) { 8 } else { (depth / 3).max(1) };
        VideoMode {
            width: size.width,
            height: size.height,
            bits: [c; 3],
            refresh: m.refresh_rate_millihertz() as f32 / 1000.0,
        }
    }
}

/// Video modes of the window's screen (8 bits per component at least), from largest to
/// smallest then from highest frequency to lowest, like the game's list.
pub fn monitor_modes(window: &winit::window::Window) -> Vec<(VideoMode, winit::monitor::VideoModeHandle)> {
    let Some(monitor) = window.current_monitor() else { return Vec::new() };
    let mut v: Vec<_> =
        monitor.video_modes().map(|m| (VideoMode::of(&m), m)).filter(|(m, _)| m.bits.iter().all(|&b| b >= 8)).collect();
    v.sort_by(|a, b| {
        (b.0.width, b.0.height, b.0.bits.iter().sum::<u16>())
            .cmp(&(a.0.width, a.0.height, a.0.bits.iter().sum::<u16>()))
            .then(b.0.refresh.total_cmp(&a.0.refresh))
    });
    v.dedup_by(|a, b| a.0 == b.0);
    v
}

/// Fullscreen requested for the window: exclusive in the preferred mode (or the screen's
/// current mode) if `exclusive`, otherwise borderless windowed. Shared with the fullscreen key.
pub fn fullscreen_for(window: &winit::window::Window, exclusive: bool, preferred: &str) -> winit::window::Fullscreen {
    use winit::window::Fullscreen;
    if !exclusive {
        return Fullscreen::Borderless(None);
    }
    let modes = monitor_modes(window);
    let wanted = VideoMode::read(preferred);
    let pick = wanted.and_then(|w| modes.iter().find(|(m, _)| *m == w)).or_else(|| {
        // Current mode of the screen (approximated: its size, the highest frequency offered).
        let size = window.current_monitor()?.size();
        modes.iter().find(|(m, _)| m.width == size.width && m.height == size.height)
    });
    match pick {
        Some((_, h)) => Fullscreen::Exclusive(h.clone()),
        None => Fullscreen::Borderless(None),
    }
}

/// Modes of the window's screen for the "Fullscreen Resolution" slider (`menus.rs`),
/// sampled each time the settings are applied. Empty: setting unavailable.
static MODES: std::sync::RwLock<Vec<VideoMode>> = std::sync::RwLock::new(Vec::new());

pub fn mode_count() -> i32 {
    MODES.read().map_or(0, |m| m.len() as i32)
}

/// Index of the mode `json` (setting `fullscreenResolution`) in the list, −1 = current mode.
pub fn mode_index(json: &str) -> i32 {
    let Some(w) = VideoMode::read(json) else { return -1 };
    MODES.read().ok().and_then(|m| m.iter().position(|x| *x == w)).map_or(-1, |i| i as i32)
}

/// Value of the setting for index `i` (−1: empty string = current mode).
pub fn mode_json(i: i32) -> String {
    MODES
        .read()
        .ok()
        .and_then(|m| usize::try_from(i).ok().and_then(|i| m.get(i).map(VideoMode::write)))
        .unwrap_or_default()
}

/// Arguments of `options.fullscreen.entry`: width, height, frequency, bits.
pub fn mode_label_args(json: &str) -> Option<[String; 4]> {
    let m = VideoMode::read(json)?;
    let rate =
        if m.refresh == m.refresh.round() { format!("{}", m.refresh as i32) } else { format!("{:.2}", m.refresh) };
    Some([m.width.to_string(), m.height.to_string(), rate, m.bits.iter().sum::<u16>().to_string()])
}

/// Darkness effect of the local player: fade in and out of 22 ticks (fade out started
/// 22 ticks before the end), tick counter for the pulsation.
#[derive(Default)]
pub struct Darkness {
    /// (remaining duration, −1 = infinite; previous factor; current factor).
    effect: Option<(i32, f32, f32)>,
    ticks: i32,
    /// Blindness: remaining duration in ticks (−1 = infinite), for the fog.
    blind: Option<i32>,
}

const DARKNESS_BLEND: i32 = 22;

impl Darkness {
    /// Effect added or updated: an effect already present keeps its fade; otherwise fade from
    /// zero, or factor set at once without the fade flag (0x08).
    pub fn effect(&mut self, name: Option<&str>, duration: i32, flags: i8) {
        if name == Some("minecraft:blindness") {
            self.blind = Some(duration);
        }
        if name != Some("minecraft:darkness") {
            return;
        }
        let on = duration == -1 || duration > DARKNESS_BLEND;
        self.effect = Some(match self.effect {
            Some((_, o, f)) => (duration, o, f),
            None if flags & 8 == 0 => {
                let f = f32::from(u8::from(on));
                (duration, f, f)
            }
            None => (duration, 0.0, 0.0),
        });
    }

    pub fn remove(&mut self, name: Option<&str>) {
        match name {
            Some("minecraft:darkness") => self.effect = None,
            Some("minecraft:blindness") => self.blind = None,
            _ => {}
        }
    }

    /// Camera effects for the fog: blindness (remaining ticks) and darkness fade.
    pub fn fog(&self, pt: f32) -> (Option<i32>, Option<f32>) {
        (self.blind, self.effect.map(|(_, o, f)| o + (f - o) * pt))
    }

    pub fn tick(&mut self) {
        self.ticks = self.ticks.wrapping_add(1);
        if let Some(d) = &mut self.blind
            && *d > 0
        {
            *d -= 1;
        }
        if let Some((d, o, f)) = &mut self.effect {
            if *d > 0 {
                *d -= 1;
            }
            *o = *f;
            let target = if *d == -1 || *d > DARKNESS_BLEND { 1.0 } else { 0.0 };
            let step = 1.0 / DARKNESS_BLEND as f32;
            *f += (target - *f).clamp(-step, step);
        }
    }

    /// (drop in brightness, darkening of the lightmap) for the frame, setting
    /// `darknessEffectScale` applied like the game.
    pub fn light(&self, pt: f32, scale: f32) -> (f32, f32) {
        let Some((_, o, f)) = self.effect else { return (0.0, 0.0) };
        let modifier = (o + (f - o) * pt) * scale;
        let pulse = ((self.ticks as f32 - pt) * std::f32::consts::PI * 0.025).cos() * 0.45 * modifier;
        (modifier, pulse.max(0.0) * scale)
    }
}

/// State of the video settings outside the renderer.
#[derive(Default)]
pub struct VideoState {
    pub limiter: FrameLimiter,
    /// Window in the foreground.
    focused: bool,
    /// Mesh options in force (compared each time the settings are applied).
    pub mesh: rc_mesh::wire::MeshOptions,
    /// First pass done (music frequency set without a draw, as at startup).
    applied: bool,
    /// Fullscreen applied: (exclusive, preferred mode), to change only when needed.
    fullscreen: Option<(bool, String)>,
    /// Columns modified by a player action since their last mesh.
    player_changed: hashbrown::HashSet<(i32, i32)>,
    /// Darkness effect (lightmap, setting `darknessEffectScale`).
    pub darkness: Darkness,
    /// Mip levels in force (changed outside the video screen).
    mips: Option<u32>,
    /// Font options in force: (Unicode forced, Japanese variants).
    font_opts: (bool, bool),
    /// Tracking of the audio devices and list seen at the last check (created at the first tick).
    audio_devices: Option<(rc_audio::devices::DeviceTracker, Option<rc_audio::devices::DeviceList>)>,
}

impl VideoState {
    /// Block changed by a player action (prediction): column and neighbours marked.
    pub fn mark_player_change(&mut self, cx: i32, cz: i32) {
        for dx in -1..=1 {
            for dz in -1..=1 {
                self.player_changed.insert((cx + dx, cz + dz));
            }
        }
    }

    /// Synchronous meshing (setting `prioritizeChunkUpdates`): 1 = columns changed by the
    /// player, 2 = also those whose centre is within √768 blocks of the eye (the game
    /// measures at the centre of each section; here at eye height). Consumes the mark.
    pub fn mesh_sync(&mut self, x: i32, z: i32, eye: [f64; 3], mode: i32) -> bool {
        let player = self.player_changed.remove(&(x, z));
        let (dx, dz) = (f64::from(x * 16 + 8) - eye[0], f64::from(z * 16 + 8) - eye[2]);
        match mode {
            1 => player,
            2 => player || dx * dx + dz * dz < 768.0,
            _ => false,
        }
    }

    /// Inputs and focus of the window (activity for the inactivity limit).
    pub fn on_window_event(&mut self, e: &winit::event::WindowEvent) {
        use winit::event::WindowEvent as W;
        match e {
            W::KeyboardInput { .. } | W::MouseInput { .. } | W::MouseWheel { .. } | W::CursorMoved { .. } => {
                self.limiter.input();
            }
            W::Focused(f) => self.focused = *f,
            _ => {}
        }
    }
}

impl crate::App {
    /// End of the loop's turn: video values depending on the frame (the frame pacing is
    /// then set by `keys.rs` `pace_frames`, with [`App::fps_limit`]).
    pub(crate) fn video_tick(&mut self) {
        self.fullscreen_tick();
        self.sampler_tick();
        // Entity distance: effective display distance (bounded by the server).
        let opt = self.options.i("renderDistance");
        let chunks = if self.env.server_view > 0 { opt.min(self.env.server_view) } else { opt };
        let scale = (f64::from(chunks) / 8.0).clamp(1.0, 2.5) * self.options.f("entityDistanceScaling");
        let Some(r) = &mut self.renderer else { return };
        if let Some(er) = &mut r.entities {
            er.view_scale = scale;
        }
    }

    /// Frame limit per second, `None` = none: `maxFps`, 60 outside a world, 10 minimised (or
    /// exclusive fullscreen without focus), 30 then 10 after 1 and 10 min without input. Automatic
    /// captures are not limited (measurements), except `RUSTCRAFT_FPSLIMIT=1`.
    pub(crate) fn fps_limit(&self) -> Option<u32> {
        let r = self.renderer.as_ref()?;
        let input = LimitIn {
            max_fps: self.options.i("maxFps"),
            afk_limit: self.options.s("inactivityFpsLimit") == "afk",
            minimized: r.window.is_minimized().unwrap_or(false),
            focused: self.video.focused,
            exclusive_fullscreen: matches!(r.window.fullscreen(), Some(winit::window::Fullscreen::Exclusive(_))),
            in_world: self.front.in_world,
        };
        static FORCED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let forced = *FORCED.get_or_init(|| rc_data::env_os!("RUSTCRAFT_FPSLIMIT").is_some_and(|v| v == "1"));
        let mut limit = if self.capture.is_some() && !forced { UNLIMITED } else { self.video.limiter.limit(&input) };
        // Vertical sync: the pacing follows the screen even if the driver lets the presentation
        // return without waiting (observed: several thousand frames per second in Fifo).
        if vsync_forced().unwrap_or_else(|| self.options.b("enableVsync"))
            && let Some(mhz) = r.window.current_monitor().and_then(|m| m.refresh_rate_millihertz())
        {
            limit = limit.min(((mhz + 500) / 1000) as i32);
        }
        (limit > 0 && limit < UNLIMITED).then_some(limit as u32)
    }

    /// Video and sound settings with immediate effect (called by `apply_options`).
    pub(crate) fn apply_video_options(&mut self) {
        let o = &self.options;
        // Meshing: a change remeshes all the columns, like the game.
        let (offset_faces, water_double) = crate::remesh::formats();
        let mesh = rc_mesh::wire::MeshOptions {
            blend: o.i("biomeBlendRadius").clamp(0, 7),
            ao: o.b("ao"),
            cutout_leaves: o.b("cutoutLeaves"),
            offset_faces,
            water_double,
        };
        if mesh != self.video.mesh {
            self.video.mesh = mesh;
            let cols: Vec<(i32, i32)> = self.world.grid.iter().map(|c| (c.x, c.z)).collect();
            for (x, z) in cols {
                self.world.touch(x, z);
            }
        }
        // Sound: music frequency, device (engine redone, like the game's reload).
        let first = !self.video.applied;
        self.video.applied = true;
        self.world_sounds.set_music_frequency(o.s("musicFrequency"), !first);
        let volumes = crate::audio_slot::volumes(o);
        if crate::audio_slot::apply_device(&mut self.audio, &self.assets, &volumes, o.s("soundDevice"), first) {
            self.entity_sounds.clear();
        }
        let Some(r) = &mut self.renderer else { return };
        // Fonts: Unicode forced (`default` font replaced by the unihex table) and Japanese
        // variants, reloaded on change only (startup: fonts baked by default).
        let font_opts = (o.b("forceUnicodeFont"), o.b("japaneseGlyphVariants"));
        if font_opts != self.video.font_opts {
            self.video.font_opts = font_opts;
            let opts = rc_assets::font::FontOptions { uniform: font_opts.0, jp: font_opts.1 };
            // Same providers as at loading (only the filter changes): same success, and same
            // fallback tolerated without a pack (`resources.rs`).
            let lenient = self.packs.stack.paths().is_empty();
            let fonts = crate::resources::FONT_IDS
                .iter()
                .filter_map(|id| {
                    let f = rc_assets::font::Font::try_load(&self.assets, id, opts, lenient)?;
                    Some(((*id).to_owned(), f))
                })
                .collect();
            r.gui.set_fonts(fonts);
        }
        r.set_vsync(o.b("enableVsync"));
        r.fade_secs = o.f("chunkSectionFadeInTime").clamp(0.0, 2.0);
        // Block texture filtering: RGSS in the shader, anisotropy in the sampler
        // (immediate); mip levels on leaving the video screen (`sampler_tick`).
        r.rgss = o.i("textureFiltering") == 1;
        r.improved_transparency = o.b("improvedTransparency");
        self.sampler_tick();
        let Some(r) = &mut self.renderer else { return };
        let o = &self.options;
        r.outline.high_contrast = o.b("highContrastBlockOutline");
        if let Some(er) = &mut r.entities {
            er.shadows = o.b("entityShadows");
            er.glint = (o.f("glintSpeed") as f32, o.f("glintStrength") as f32);
        }
        if let Some(items) = &mut r.gui.items {
            items.glint = (o.f("glintSpeed") as f32, o.f("glintStrength") as f32);
        }
        // Exclusive fullscreen and its resolution (plain fullscreen stays in `apply_options`).
        if let Ok(mut m) = MODES.write() {
            *m = monitor_modes(&r.window).into_iter().map(|(v, _)| v).collect();
        }
        self.fullscreen_tick();
    }

    /// Block sampler: anisotropy right away, mip levels retained while the video
    /// screen is open (the game reloads the textures on leaving it).
    pub(crate) fn sampler_tick(&mut self) {
        let Some(r) = &mut self.renderer else { return };
        let o = &self.options;
        if self.menus.top_id() != Some(crate::menus::Id::Video) || self.video.mips.is_none() {
            self.video.mips = Some(o.i("mipmapLevels").clamp(0, 4) as u32);
        }
        let aniso = if o.i("textureFiltering") == 2 { 1u16 << o.i("maxAnisotropyBit").clamp(1, 3) } else { 1 };
        r.set_atlas_sampler(self.video.mips.unwrap_or(4), aniso);
    }

    /// Exclusive fullscreen: immediate toggle like the game; new mode applied on closing
    /// the video screen (the game does it on leaving the screen).
    pub(crate) fn fullscreen_tick(&mut self) {
        let Some(r) = &self.renderer else { return };
        let o = &self.options;
        // Test clients (`RUSTCRAFT_BACKGROUND=1`): windowed, as in `apply_options`.
        if !o.b("fullscreen") || crate::launch::background() {
            self.video.fullscreen = None;
            return;
        }
        let (exclusive, mode) = (o.b("exclusiveFullscreen"), o.s("fullscreenResolution"));
        let in_video = self.menus.top_id() == Some(crate::menus::Id::Video);
        let apply = match &self.video.fullscreen {
            None => true,
            Some((e, m)) => *e != exclusive || (exclusive && m != mode && !in_video),
        };
        if apply {
            r.window.set_fullscreen(Some(fullscreen_for(&r.window, exclusive, mode)));
            self.video.fullscreen = Some((exclusive, mode.to_owned()));
        }
    }
}

impl crate::App {
    /// Check of the devices on every tick: device lost, system default changed
    /// ("default" setting) or chosen device back ⇒ engine redone, like the game.
    pub(crate) fn watch_audio_devices(&mut self) {
        let Some(a) = self.audio.get() else { return };
        let (tracker, seen) = self.video.audio_devices.get_or_insert_with(|| {
            let t = rc_audio::devices::DeviceTracker::new();
            // Old tracking: list already read; persistent thread: first sample to come.
            let l = t.with_current(|l| l.cloned());
            (t, l)
        });
        let change = if a.is_device_lost() {
            eprintln!("[audio] device lost");
            tracker.force_refresh();
            true
        } else if a.is_opening() {
            // Output being opened: no open name to compare.
            false
        } else {
            tracker.tick();
            let (name, selected) = (a.current_device_name(), self.options.s("soundDevice"));
            tracker.with_current(|l| l.is_some_and(|l| rc_audio::devices::should_change_seen(name, selected, l, seen)))
        };
        if change {
            self.entity_sounds.clear();
            let v = crate::audio_slot::volumes(&self.options);
            crate::audio_slot::reload_audio(a, &self.assets, &v, self.options.s("soundDevice"));
        }
    }
}

/// Native error box (title "Minecraft", error icon, OK button), like the game when
/// no graphics API starts; message also written to the console.
pub fn error_box(message: &str) {
    eprintln!("{message}");
    #[cfg(windows)]
    #[allow(unsafe_code)]
    {
        #[link(name = "user32")]
        unsafe extern "system" {
            fn MessageBoxW(hwnd: isize, text: *const u16, caption: *const u16, kind: u32) -> i32;
        }
        let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
        let (text, caption) = (wide(message), wide("Minecraft"));
        // SAFETY: zero-terminated UTF-16 strings, alive during the call; 0x10 = error.
        unsafe {
            MessageBoxW(0, text.as_ptr(), caption.as_ptr(), 0x10);
        }
    }
}

/// Game message when no graphics API started: one line per API tried.
pub fn no_backend_message(tried: &[(&str, String)]) -> String {
    let mut m = String::from("No supported graphics backend was found.");
    for (name, why) in tried {
        m += &format!("\n\n- Tried {name}: \n  {why}");
    }
    m + "\n\nPlease make sure you have up-to-date drivers (see aka.ms/mcdriver for instructions)."
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_backend_text() {
        let m = no_backend_message(&[("Vulkan", "a".into()), ("DirectX 12", "b".into())]);
        assert_eq!(
            m,
            "No supported graphics backend was found.\n\n- Tried Vulkan: \n  a\n\n- Tried DirectX 12: \n  b\n\n\
             Please make sure you have up-to-date drivers (see aka.ms/mcdriver for instructions)."
        );
    }

    fn base() -> LimitIn {
        LimitIn {
            max_fps: 120,
            afk_limit: true,
            minimized: false,
            focused: true,
            exclusive_fullscreen: false,
            in_world: true,
        }
    }

    #[test]
    fn limit_follows_window_and_world() {
        let f = FrameLimiter::default();
        assert_eq!(f.limit(&base()), 120);
        assert_eq!(f.limit(&LimitIn { in_world: false, ..base() }), 60);
        assert_eq!(f.limit(&LimitIn { minimized: true, ..base() }), 10);
        assert_eq!(f.limit(&LimitIn { focused: false, ..base() }), 120);
        assert_eq!(f.limit(&LimitIn { focused: false, exclusive_fullscreen: true, ..base() }), 10);
    }

    #[test]
    fn afk_lowers_limit() {
        let mut f = FrameLimiter { last_input: Instant::now() - Duration::from_secs(61) };
        assert_eq!(f.limit(&base()), 30);
        assert_eq!(f.limit(&LimitIn { max_fps: 20, ..base() }), 20);
        assert_eq!(f.limit(&LimitIn { afk_limit: false, ..base() }), 120);
        f.last_input = Instant::now() - Duration::from_secs(601);
        assert_eq!(f.limit(&base()), 10);
    }

    #[test]
    fn video_mode_json_round_trip() {
        let m = VideoMode { width: 1920, height: 1080, bits: [8; 3], refresh: 59.94 };
        assert_eq!(VideoMode::read(&m.write()), Some(m));
        let official = r#"{"width":2560,"height":1440,"red_bits":8,"green_bits":8,"blue_bits":8,"refresh_rate":144.0}"#;
        assert_eq!(VideoMode::read(official).map(|m| (m.width, m.refresh)), Some((2560, 144.0)));
        assert_eq!(VideoMode::read(""), None);
    }

    #[test]
    fn darkness_fades_and_scales() {
        let mut d = Darkness::default();
        d.effect(Some("minecraft:darkness"), 200, 8);
        assert_eq!(d.light(1.0, 1.0), (0.0, 0.0));
        for _ in 0..22 {
            d.tick();
        }
        let (m, _) = d.light(1.0, 1.0);
        assert!((m - 1.0).abs() < 1e-5);
        assert_eq!(d.light(1.0, 0.0), (0.0, 0.0));
        let (half, _) = d.light(1.0, 0.5);
        assert!((half - 0.5).abs() < 1e-5);
        d.remove(Some("minecraft:darkness"));
        assert_eq!(d.light(0.5, 1.0), (0.0, 0.0));
        // Without fade: full from the start.
        d.effect(Some("minecraft:darkness"), -1, 0);
        assert!((d.light(0.0, 1.0).0 - 1.0).abs() < 1e-5);
    }

    #[test]
    fn vsync_modes() {
        use wgpu::PresentMode as P;
        assert_eq!(present_modes(&[P::Fifo, P::Mailbox], false), (P::AutoNoVsync, P::Mailbox));
        assert_eq!(present_modes(&[P::Fifo, P::Immediate], true), (P::Fifo, P::Fifo));
    }
}
