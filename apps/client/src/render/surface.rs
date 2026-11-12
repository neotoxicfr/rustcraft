//! Presentation surface: its configuration at startup, resizing, vertical sync, and the depth target
//! that follows the window size.

use winit::window::Window;

use super::Renderer;
use crate::present_watch::Stage;

/// What the startup configuration of the surface decided.
pub(super) struct SurfaceSetup {
    pub(super) config: wgpu::SurfaceConfiguration,
    /// The surface allows copying the frame out (captures, menu blur and screen effects need it).
    pub(super) can_capture: bool,
    /// Presentation mode in force.
    pub(super) present_mode: wgpu::PresentMode,
    /// Presentation modes offered by the surface (choice of vertical sync).
    pub(super) present_modes_caps: Vec<wgpu::PresentMode>,
}

/// Configures `surface` at the window size: colour format, copy usage when allowed, presentation mode.
pub(super) fn configure_surface(
    surface: &wgpu::Surface<'static>,
    adapter: &wgpu::Adapter,
    device: &wgpu::Device,
    window: &Window,
) -> SurfaceSetup {
    let size = window.inner_size();
    let mut config = surface.get_default_config(adapter, size.width.max(1), size.height.max(1)).expect("format");
    // Like the game: colours computed directly on the gamma values (neither texture nor
    // sRGB surface), otherwise tints and shading come out washed out.
    let caps = surface.get_capabilities(adapter);
    if let Some(f) = caps
        .formats
        .iter()
        .copied()
        .find(|f| !f.is_srgb() && f.remove_srgb_suffix() == config.format.remove_srgb_suffix())
    {
        config.format = f;
    }
    let can_capture = surface.get_capabilities(adapter).usages.contains(wgpu::TextureUsages::COPY_SRC);
    if can_capture {
        config.usage |= wgpu::TextureUsages::COPY_SRC;
    }
    // Vertical sync: setting `enableVsync` (set by `set_vsync`); `RUSTCRAFT_VSYNC=0|1`
    // forces it for measurements. Without forcing, the window starts without sync until the setting.
    let (requested, present_mode) =
        crate::video::present_modes(&caps.present_modes, crate::video::vsync_forced().unwrap_or(false));
    config.present_mode = requested;
    // The settings are not applied yet: no frame limit known, like no vertical sync.
    config.desired_maximum_frame_latency =
        crate::video::frame_latency(requested, false, crate::video::LatencyRules::in_force());
    crate::present_watch::during(Stage::Configure, || surface.configure(device, &config));
    eprintln!("presentation: {present_mode:?} (surface: {:?})", caps.present_modes);
    let present_modes_caps = caps.present_modes.clone();
    SurfaceSetup { config, can_capture, present_mode, present_modes_caps }
}

/// Depth target of the frame; `sampled`: also read as a texture (the depth copies of the improved
/// transparency).
pub(super) fn depth_view(device: &wgpu::Device, w: u32, h: u32, sampled: bool) -> wgpu::TextureView {
    let sampling = if sampled { wgpu::TextureUsages::TEXTURE_BINDING } else { wgpu::TextureUsages::empty() };
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | sampling,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

/// What the frame does with its depth target once a pass is over (switches read once).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DepthPaths {
    /// `RUSTCRAFT_OPT_DEPTH_DISCARD`: the depth is not kept at the end of a pass no later pass reads
    /// (the world pass without improved transparency, the pass after the transparency: every pass
    /// that follows clears it).
    pub discard_unread: bool,
    /// `RUSTCRAFT_OPT_DEPTH_NO_SAMPLING`: the depth target is made without the texture usage while
    /// improved transparency is off (made again when it turns on), so that the driver may keep it
    /// in its fully compressed form.
    pub sampled_only_for_transparency: bool,
}

impl DepthPaths {
    pub(super) fn from_switches() -> Self {
        DepthPaths {
            discard_unread: rc_data::opt_switch!("RUSTCRAFT_OPT_DEPTH_DISCARD"),
            sampled_only_for_transparency: rc_data::opt_switch!("RUSTCRAFT_OPT_DEPTH_NO_SAMPLING"),
        }
    }

    /// Paths before these switches.
    #[cfg(test)]
    pub(super) const OLD: DepthPaths = DepthPaths { discard_unread: false, sampled_only_for_transparency: false };

    /// End of the world pass: improved transparency reads the depth after it (copies, culling,
    /// composition); without it, the next pass that has a depth clears it.
    pub(super) fn world_pass_store(self, improved_transparency: bool) -> wgpu::StoreOp {
        self.store_unless_unread(improved_transparency)
    }

    /// End of the pass after the transparency: the passes that follow (glowing outlines, screen
    /// effects, hand and interface) clear the depth or have none.
    pub(super) fn after_transparency_store(self) -> wgpu::StoreOp {
        self.store_unless_unread(false)
    }

    fn store_unless_unread(self, read_later: bool) -> wgpu::StoreOp {
        if read_later || !self.discard_unread { wgpu::StoreOp::Store } else { wgpu::StoreOp::Discard }
    }

    /// The depth target needs the texture usage this frame.
    pub(super) fn depth_sampled(self, improved_transparency: bool) -> bool {
        improved_transparency || !self.sampled_only_for_transparency
    }
}

/// Surface image acquired ahead, for the next frame (`RUSTCRAFT_OPT_ACQUIRE_BEFORE_INPUT`).
///
/// Acquiring waits for a free image of the swap chain (with vertical sync: for the screen). That
/// wait used to come at the start of the drawing, after the event loop had read the mouse and the
/// keys for the frame: the frame reached the screen with input older by the wait, up to a whole
/// refresh. The next image is now acquired right after a presentation: the wait is over before
/// the loop reads the input of the frame that will draw into it. The frame itself is unchanged
/// (same reads, same order), only the place of the wait moves.
///
/// Generic over the image so that the order of the uses is tested without a surface.
pub(super) struct AcquiredAhead<Image> {
    held: Option<Image>,
}

impl<Image> Default for AcquiredAhead<Image> {
    fn default() -> Self {
        AcquiredAhead { held: None }
    }
}

impl<Image> AcquiredAhead<Image> {
    /// The image held for the frame being drawn, if any: it leaves the slot.
    pub(super) fn take(&mut self) -> Option<Image> {
        self.held.take()
    }

    /// Right after a presentation: keeps the image just acquired for the next frame.
    pub(super) fn hold(&mut self, image: Image) {
        debug_assert!(self.held.is_none(), "one image acquired at a time");
        self.held = Some(image);
    }

    /// Before the surface is configured again: the configuration fails while an image is out, and
    /// an image of the old swap chain is of no use to the new one.
    pub(super) fn discard(&mut self) {
        self.held = None;
    }
}

/// The presentation surface with the image acquired ahead from it. The surface is private to this
/// type, so the renderer cannot configure it, nor hand it to the next renderer, while the held
/// image is out: the driver refuses to configure a surface whose image is out, and the client
/// would stop at the first resize.
pub(super) struct PresentationSurface {
    /// Declared before `surface`: fields are dropped in this order, and an image still held has
    /// to go back to its surface before the surface goes.
    acquired_ahead: AcquiredAhead<wgpu::CurrentSurfaceTexture>,
    surface: wgpu::Surface<'static>,
}

impl PresentationSurface {
    pub(super) fn new(surface: wgpu::Surface<'static>) -> Self {
        PresentationSurface { acquired_ahead: AcquiredAhead::default(), surface }
    }

    /// The image the frame draws into: the one acquired after the last presentation when there
    /// is one, otherwise acquired now. A surface that had no image to give then is answered now,
    /// like a failed acquisition here.
    pub(super) fn image_for_this_frame(&mut self) -> wgpu::CurrentSurfaceTexture {
        match self.acquired_ahead.take() {
            Some(acquired_ahead) => acquired_ahead,
            None => self.acquire(),
        }
    }

    /// Acquires the next image of the surface (may wait for a free one).
    fn acquire(&self) -> wgpu::CurrentSurfaceTexture {
        crate::present_watch::during(Stage::Acquire, || self.surface.get_current_texture())
    }

    /// Presents `frame`. `acquire_next` (`RUSTCRAFT_OPT_ACQUIRE_BEFORE_INPUT`): the wait for the
    /// next image ends here, before the event loop reads the input of the frame that will draw
    /// into it.
    pub(super) fn present(&mut self, queue: &wgpu::Queue, frame: wgpu::SurfaceTexture, acquire_next: bool) {
        crate::present_watch::during(Stage::Present, || queue.present(frame));
        if acquire_next {
            let next = self.acquire();
            self.acquired_ahead.hold(next);
        }
    }

    /// Configures the surface; the image held goes back to it first.
    pub(super) fn configure(&mut self, device: &wgpu::Device, config: &wgpu::SurfaceConfiguration) {
        self.acquired_ahead.discard();
        crate::present_watch::during(Stage::Configure, || self.surface.configure(device, config));
    }

    /// The bare surface, for the renderer built next on it (resource reload): it configures the
    /// surface at once, so the image held goes back first.
    pub(super) fn into_surface(self) -> wgpu::Surface<'static> {
        let PresentationSurface { acquired_ahead, surface } = self;
        drop(acquired_ahead);
        surface
    }
}

impl Renderer {
    /// Configures the surface again with the current configuration.
    pub(super) fn reconfigure_surface(&mut self) {
        self.surface.configure(&self.device, &self.config);
    }

    pub fn resize(&mut self, w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        self.config.width = w;
        self.config.height = h;
        self.reconfigure_surface();
        self.remake_depth(self.depth_sampled);
    }

    /// Depth target of the frame with the usage `improved_transparency` needs: made again when that
    /// changes (`RUSTCRAFT_OPT_DEPTH_NO_SAMPLING`).
    pub(super) fn keep_depth_usage(&mut self, improved_transparency: bool) {
        let sampled = self.depth_paths.depth_sampled(improved_transparency);
        if sampled != self.depth_sampled {
            self.remake_depth(sampled);
        }
    }

    fn remake_depth(&mut self, sampled: bool) {
        self.depth = depth_view(&self.device, self.config.width, self.config.height, sampled);
        self.depth_sampled = sampled;
        // Improved transparency targets bound to the old depth: redone as needed.
        if let Some(o) = &mut self.oit {
            o.release();
        }
    }

    /// Vertical sync (`enableVsync`): surface reconfigured on the fly if the mode changes,
    /// like the game; `RUSTCRAFT_VSYNC=0|1` overrides the setting. `fps_limited`: the `maxFps`
    /// setting sets a limit (one of the inputs of the frames held ahead, `video.rs` `frame_latency`).
    pub fn set_vsync(&mut self, on: bool, fps_limited: bool) {
        let (requested, effective) =
            crate::video::present_modes(&self.present_modes_caps, crate::video::vsync_forced().unwrap_or(on));
        let latency = crate::video::frame_latency(requested, fps_limited, crate::video::LatencyRules::in_force());
        let mode_changed = requested != self.config.present_mode;
        if mode_changed || latency != self.config.desired_maximum_frame_latency {
            self.config.present_mode = requested;
            self.config.desired_maximum_frame_latency = latency;
            self.present_mode = effective;
            self.reconfigure_surface();
            if latency == crate::video::USUAL_FRAME_LATENCY {
                eprintln!("presentation: {effective:?}");
            } else {
                eprintln!("presentation: {effective:?}, {latency} frame(s) held ahead");
            }
        }
    }
}

#[cfg(all(test, windows))]
#[path = "surface_real_tests.rs"]
mod real_surface_tests;

#[cfg(test)]
#[path = "depth_target_test.rs"]
mod depth_target_tests;

#[cfg(test)]
mod tests {
    use super::{AcquiredAhead, DepthPaths};

    /// The depth is kept only for improved transparency, the one reader after the world pass, and
    /// made with the texture usage only for it; the old path keeps and samples it always.
    #[test]
    fn depth_is_kept_and_sampled_only_for_improved_transparency() {
        let (store, discard) = (wgpu::StoreOp::Store, wgpu::StoreOp::Discard);
        let new = DepthPaths { discard_unread: true, sampled_only_for_transparency: true };
        let rows = [(new, false, discard, discard, false), (new, true, store, discard, true)];
        let old = DepthPaths::OLD;
        let old_rows = [(old, false, store, store, true), (old, true, store, store, true)];
        for (paths, oit, world, after_transparency, sampled) in rows.into_iter().chain(old_rows) {
            assert_eq!(paths.world_pass_store(oit), world, "{paths:?}, improved transparency {oit}");
            assert_eq!(paths.after_transparency_store(), after_transparency, "{paths:?}");
            assert_eq!(paths.depth_sampled(oit), sampled, "{paths:?}, improved transparency {oit}");
        }
    }

    /// Stand-in for the surface: hands out numbered images and checks that a single one is out.
    #[derive(Default)]
    struct FakeSurface {
        acquired: std::cell::Cell<u32>,
        out: std::cell::Cell<bool>,
    }

    impl FakeSurface {
        fn acquire(&self) -> u32 {
            assert!(!self.out.replace(true), "an image acquired while another one is out");
            self.acquired.set(self.acquired.get() + 1);
            self.acquired.get()
        }

        fn present(&self, _image: u32) {
            assert!(self.out.replace(false), "nothing to present");
        }

        /// Configuring refuses an image still out, like the real surface.
        fn configure(&self) {
            assert!(!self.out.get(), "configured with an image out");
        }
    }

    /// Frames drawn one after the other, a reconfiguration (resize, vertical sync) before the
    /// frames listed in `reconfigured_before`: the images drawn into, in order.
    fn drawn_images(ahead: bool, frames: u32, reconfigured_before: &[u32]) -> (Vec<u32>, u32) {
        let surface = FakeSurface::default();
        let mut slot = AcquiredAhead::default();
        let mut drawn = Vec::new();
        for frame in 0..frames {
            if reconfigured_before.contains(&frame) {
                // An image discarded goes back to the surface.
                let given_back = slot.held.is_some();
                slot.discard();
                surface.out.set(surface.out.get() && !given_back);
                surface.configure();
            }
            let image = slot.take().unwrap_or_else(|| surface.acquire());
            drawn.push(image);
            surface.present(image);
            if ahead {
                slot.hold(surface.acquire());
            }
        }
        (drawn, surface.acquired.get())
    }

    /// Acquiring ahead changes neither the images drawn into nor their order: every frame draws
    /// into the next image, once, and exactly one more image is acquired than drawn (the one held
    /// for the frame to come). A reconfiguration drops the held image and the frame after it
    /// acquires its own. Sabotage checked: a held image kept across a reconfiguration fails.
    #[test]
    fn acquiring_ahead_draws_into_the_same_images_in_the_same_order() {
        let (at_frame_start, acquired) = drawn_images(false, 6, &[]);
        assert_eq!((at_frame_start.as_slice(), acquired), ([1, 2, 3, 4, 5, 6].as_slice(), 6));
        let (ahead, acquired) = drawn_images(true, 6, &[]);
        assert_eq!((ahead.as_slice(), acquired), ([1, 2, 3, 4, 5, 6].as_slice(), 7));
        // Reconfigured before frames 2 and 5: the image held then (3, then 7) is dropped.
        let (ahead, acquired) = drawn_images(true, 6, &[2, 5]);
        assert_eq!((ahead.as_slice(), acquired), ([1, 2, 4, 5, 6, 8].as_slice(), 9));
        let (at_frame_start, _) = drawn_images(false, 6, &[2, 5]);
        assert_eq!(at_frame_start, [1, 2, 3, 4, 5, 6]);
    }

    /// `discard` empties the slot: the next frame acquires.
    #[test]
    fn a_discarded_image_is_not_drawn_into() {
        let mut slot = AcquiredAhead::default();
        slot.hold(1);
        slot.discard();
        assert_eq!(slot.take(), None);
        slot.hold(3);
        assert_eq!(slot.take(), Some(3));
        assert_eq!(slot.take(), None, "taken once only");
    }
}
