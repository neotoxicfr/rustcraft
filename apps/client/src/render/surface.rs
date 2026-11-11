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

pub(super) fn depth_view(device: &wgpu::Device, w: u32, h: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            // Read by the depth copies of the improved transparency.
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
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

impl Renderer {
    /// Acquires the next image of the surface (may wait for a free one).
    pub(super) fn acquire_surface_image(&self) -> wgpu::CurrentSurfaceTexture {
        crate::present_watch::during(Stage::Acquire, || self.surface.get_current_texture())
    }

    /// Configures the surface again with the current configuration.
    pub(super) fn reconfigure_surface(&mut self) {
        self.acquired_ahead.discard();
        crate::present_watch::during(Stage::Configure, || self.surface.configure(&self.device, &self.config));
    }

    pub fn resize(&mut self, w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        self.config.width = w;
        self.config.height = h;
        self.reconfigure_surface();
        self.depth = depth_view(&self.device, w, h);
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

#[cfg(test)]
mod tests {
    use super::AcquiredAhead;

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
