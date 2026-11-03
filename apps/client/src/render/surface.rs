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

impl Renderer {
    pub fn resize(&mut self, w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        self.config.width = w;
        self.config.height = h;
        crate::present_watch::during(Stage::Configure, || self.surface.configure(&self.device, &self.config));
        self.depth = depth_view(&self.device, w, h);
        // Improved transparency targets bound to the old depth: redone as needed.
        if let Some(o) = &mut self.oit {
            o.release();
        }
    }

    /// Vertical sync (`enableVsync`): surface reconfigured on the fly if the mode changes,
    /// like the game; `RUSTCRAFT_VSYNC=0|1` overrides the setting.
    pub fn set_vsync(&mut self, on: bool) {
        let (requested, effective) =
            crate::video::present_modes(&self.present_modes_caps, crate::video::vsync_forced().unwrap_or(on));
        if requested != self.config.present_mode {
            self.config.present_mode = requested;
            self.present_mode = effective;
            crate::present_watch::during(Stage::Configure, || self.surface.configure(&self.device, &self.config));
            eprintln!("presentation: {effective:?}");
        }
    }
}
