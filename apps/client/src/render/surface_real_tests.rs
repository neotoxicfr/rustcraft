//! The presentation surface on real surfaces: one hidden window per API (Vulkan, DirectX 12),
//! opened and configured like the renderer does, then driven through what a session does to it
//! with an image acquired ahead: resize, vertical sync switched, frames held ahead changed,
//! handed to the next renderer, dropped. A driver refuses to configure a surface whose image is
//! out; the model of `surface.rs` only imitates that.
//!
//! Not covered here, to see in a launched client: minimise and restore, exclusive fullscreen.

use std::sync::Arc;
use std::time::Duration;

use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::platform::pump_events::EventLoopExtPumpEvents;
use winit::platform::windows::EventLoopBuilderExtWindows;
use winit::window::Window;

use super::{PresentationSurface, configure_surface};
use crate::gpu_backend::GpuContext;

const APIS: [(&str, wgpu::Backends); 2] = [("Vulkan", wgpu::Backends::VULKAN), ("DirectX 12", wgpu::Backends::DX12)];

/// Hidden windows created at the first turn of the event loop, one per API: a window that held
/// the swap chain of one API is not always accepted by the other.
#[derive(Default)]
struct HiddenWindows(Vec<Arc<Window>>);

impl ApplicationHandler for HiddenWindows {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        while self.0.len() < APIS.len() {
            let hidden = Window::default_attributes().with_visible(false).with_inner_size(PhysicalSize::new(320, 240));
            let Ok(window) = el.create_window(hidden) else { return };
            self.0.push(Arc::new(window));
        }
    }

    fn window_event(&mut self, _: &ActiveEventLoop, _: winit::window::WindowId, _: winit::event::WindowEvent) {}
}

/// A real surface with what the renderer keeps beside it.
struct RealSurface {
    surface: PresentationSurface,
    config: wgpu::SurfaceConfiguration,
    present_modes: Vec<wgpu::PresentMode>,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    window: Arc<Window>,
    presented: u32,
}

impl RealSurface {
    /// Configured like a renderer just built on `gpu` (`Renderer::new`).
    fn of_a_new_renderer(gpu: GpuContext, window: Arc<Window>) -> RealSurface {
        let GpuContext { surface, adapter, device, queue, .. } = gpu;
        let setup = configure_surface(&surface, &adapter, &device, &window);
        RealSurface {
            surface: PresentationSurface::new(surface),
            config: setup.config,
            present_modes: setup.present_modes_caps,
            adapter,
            device,
            queue,
            window,
            presented: 0,
        }
    }

    /// One frame like the renderer's (`Renderer::render`), its drawing reduced to a clear.
    fn frame(&mut self, acquire_next: bool) {
        let frame = match self.surface.image_for_this_frame() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
            _ => return,
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        self.queue.submit([encoder.finish()]);
        self.surface.present(&self.queue, frame, acquire_next);
        self.presented += 1;
    }

    /// `count` frames, every one of them presented.
    fn frames(&mut self, count: u32, acquire_next: bool, when: &str) {
        let before = self.presented;
        for _ in 0..count {
            self.frame(acquire_next);
        }
        assert_eq!(self.presented - before, count, "frames presented {when}");
    }

    /// An image of the surface is out, held for the next frame.
    fn holds_an_image(&self) -> bool {
        use wgpu::CurrentSurfaceTexture::{Suboptimal, Success};
        matches!(self.surface.acquired_ahead.held, Some(Success(_) | Suboptimal(_)))
    }

    fn configure(&mut self) {
        self.surface.configure(&self.device, &self.config);
    }

    /// The window resized, then the surface like `Renderer::resize`.
    fn resize(&mut self, event_loop: &mut EventLoop<()>, windows: &mut HiddenWindows, width: u32, height: u32) {
        let _ = self.window.request_inner_size(PhysicalSize::new(width, height));
        let _ = event_loop.pump_app_events(Some(Duration::ZERO), windows);
        let size = self.window.inner_size();
        assert_eq!((size.width, size.height), (width, height), "the hidden window took its new size");
        (self.config.width, self.config.height) = (width, height);
        self.configure();
    }

    /// Vertical sync and frames held ahead, like `Renderer::set_vsync`.
    fn set_vsync(&mut self, vsync: bool, frames_held_ahead: u32) {
        self.config.present_mode = crate::video::present_modes(&self.present_modes, vsync).0;
        self.config.desired_maximum_frame_latency = frames_held_ahead;
        self.configure();
    }

    /// The resource reload on a kept device (`Renderer::into_gpu_context`, then `Renderer::new`).
    fn rebuilt_on_the_same_device(self) -> RealSurface {
        let RealSurface { surface, adapter, device, queue, window, presented, .. } = self;
        let text = crate::gpu_backend::GpuText::new(&adapter.get_info(), None);
        let gpu = GpuContext { surface: surface.into_surface(), adapter, device, queue, text };
        RealSurface { presented, ..RealSurface::of_a_new_renderer(gpu, window) }
    }
}

/// What a session does to the surface, with the next image acquired after every presentation
/// (`acquire_next`) or at the start of the frame. Every reconfiguration below happens with an
/// image out when `acquire_next` is on.
fn session(
    surface: RealSurface,
    event_loop: &mut EventLoop<()>,
    windows: &mut HiddenWindows,
    acquire_next: bool,
) -> u32 {
    let mut surface = surface;
    surface.set_vsync(true, crate::video::USUAL_FRAME_LATENCY);
    surface.frames(3, acquire_next, "with vertical sync");
    assert_eq!(surface.holds_an_image(), acquire_next);

    surface.resize(event_loop, windows, 400, 300);
    surface.frames(2, acquire_next, "after a resize");
    assert_eq!(surface.holds_an_image(), acquire_next);

    surface.set_vsync(false, crate::video::USUAL_FRAME_LATENCY);
    surface.frames(2, acquire_next, "without vertical sync");
    surface.set_vsync(true, 1);
    surface.frames(2, acquire_next, "with one frame held ahead");
    assert_eq!(surface.holds_an_image(), acquire_next);

    let mut surface = surface.rebuilt_on_the_same_device();
    surface.frames(2, acquire_next, "on the renderer built next");
    assert_eq!(surface.holds_an_image(), acquire_next);
    // Dropped with its image out: the image goes back before the surface goes.
    surface.presented
}

/// Acquiring ahead on real Vulkan and DirectX 12 surfaces: the surface is configured again
/// (resize, vertical sync, frames held ahead), handed to the next renderer and dropped while the
/// image of the next frame is out, and every frame is presented, as when acquiring at the start
/// of the frame. Sabotage checked: without the discard in `PresentationSurface::configure` the
/// driver refuses the first reconfiguration, on both APIs. Without a desktop session or a
/// hardware adapter the test skips itself; under `RUSTCRAFT_OPT_REQUIRE=1` that is a failure.
#[test]
fn a_real_surface_is_reconfigured_and_handed_over_with_an_image_acquired_ahead() {
    crate::render::upload::tests::gpu_serial();
    let event_loop = EventLoop::builder().with_any_thread(true).build().ok();
    let Some(mut event_loop) = rc_testdata::need(event_loop, "event loop (desktop session)") else { return };
    let mut windows = HiddenWindows::default();
    let _ = event_loop.pump_app_events(Some(Duration::ZERO), &mut windows);
    let all_windows = (windows.0.len() == APIS.len()).then_some(());
    let Some(()) = rc_testdata::need(all_windows, "hidden windows (desktop session)") else { return };

    for (index, (name, backends)) in APIS.into_iter().enumerate() {
        for acquire_next in [false, true] {
            let window = windows.0[index].clone();
            let _ = window.request_inner_size(PhysicalSize::new(320, 240));
            let _ = event_loop.pump_app_events(Some(Duration::ZERO), &mut windows);
            let gpu = crate::gpu_backend::open_on_one_api(&window, backends);
            let Some(gpu) = rc_testdata::need(gpu, &format!("hardware {name} adapter")) else { break };
            let surface = RealSurface::of_a_new_renderer(gpu, window);
            let presented = session(surface, &mut event_loop, &mut windows, acquire_next);
            assert_eq!(presented, 11, "{name}, next image acquired ahead: {acquire_next}");
            eprintln!("{name}: 11 frames presented, next image acquired ahead: {acquire_next}");
        }
    }
}
