//! GPU opened in the background while the resources load (`RUSTCRAFT_OPT_EARLY_GPU_OPEN`, on by
//! default; `=0`: everything is opened once the window exists, as before).
//!
//! Loading the driver, choosing the adapter and opening the device take about 150 ms and need no
//! window: a "gpu-init" thread does them from the entry into `main`, on the first API of the order
//! known at that point (forced API, default preference). When the window exists, its surface is
//! created on the main thread with the instance of that thread. The device is kept only if it is
//! the one [`pick`](super::pick) would have chosen: same order of APIs once the settings are read,
//! a hardware adapter, and an adapter that can present to the window. wgpu sorts the adapters by
//! type, filtering first those that cannot present (Vulkan and DX12 ignore the surface otherwise):
//! the first adapter without a surface is therefore the one chosen with it whenever it can present.
//! In every other case the early device is dropped and `pick` runs exactly as before (software
//! fallback, "Tried" list and error box included).

use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Instant;

use winit::window::Window;

use super::{GpuContext, OpenedDevice};

/// Opening of the first API in progress on the "gpu-init" thread.
pub struct EarlyGpuOpen {
    /// Order of the APIs the thread started from.
    order: &'static [wgpu::Backends],
    thread: JoinHandle<Option<OpenedOnThread>>,
}

/// What the thread hands over: its instance (the window's surface is created on it) and the device.
struct OpenedOnThread {
    instance: wgpu::Instance,
    opened: OpenedDevice,
}

/// Outcome of the early opening once the window exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EarlyVerdict {
    /// The early device is the one `pick` would choose: kept.
    Kept,
    /// The settings changed the order of the APIs (other preference, crash safeguard).
    OrderChanged,
    /// Instance, adapter or device could not be opened on the first API.
    NotOpened,
    /// No surface on the window with that instance, or the adapter cannot present to it.
    CannotPresent,
    /// Software adapter: `pick` tries the other APIs first.
    Software,
}

/// Verdict from the facts known at the join, in the order they are checked.
fn verdict(order_unchanged: bool, opened: bool, can_present: bool, software: bool) -> EarlyVerdict {
    if !order_unchanged {
        EarlyVerdict::OrderChanged
    } else if !opened {
        EarlyVerdict::NotOpened
    } else if !can_present {
        EarlyVerdict::CannotPresent
    } else if software {
        EarlyVerdict::Software
    } else {
        EarlyVerdict::Kept
    }
}

/// Opens instance, adapter and device of `backends` without a window.
fn open_without_window(backends: wgpu::Backends) -> Option<OpenedOnThread> {
    let instance = super::instance_with_marks(backends);
    match super::open_adapter_and_device(&instance, None) {
        Ok(opened) => Some(OpenedOnThread { instance, opened }),
        Err((stage, e)) => {
            eprintln!("early GPU opening, API {backends:?}: {stage} ({e})");
            None
        }
    }
}

impl EarlyGpuOpen {
    /// Starts the "gpu-init" thread (`None` with the switch off or if the thread cannot start).
    pub fn start() -> Option<EarlyGpuOpen> {
        if !rc_data::opt_switch!("RUSTCRAFT_OPT_EARLY_GPU_OPEN") {
            return None;
        }
        super::install_log();
        // The settings are not read yet: default preference, the join checks it did not change.
        let order = super::order(rc_data::env_str!("RUSTCRAFT_BACKEND"), "default");
        let backends = *order.first()?;
        let thread = std::thread::Builder::new()
            .name("gpu-init".into())
            .spawn(move || open_without_window(backends))
            .map_err(|e| eprintln!("early GPU opening: {e}"))
            .ok()?;
        Some(EarlyGpuOpen { order, thread })
    }

    /// Context on `window` from the early device, or `None` when [`pick`](super::pick) must run
    /// (the reason is printed).
    pub fn join(self, window: &Arc<Window>) -> Option<GpuContext> {
        let now = super::order(rc_data::env_str!("RUSTCRAFT_BACKEND"), super::startup_preference());
        if now != self.order {
            // Not waited for: the thread ends on its own and drops its device.
            eprintln!("early GPU opening dropped: {:?}", EarlyVerdict::OrderChanged);
            return None;
        }
        let waited = Instant::now();
        let early = self.thread.join().ok().flatten();
        crate::boot::mark("GPU joined");
        eprintln!("early GPU opening: joined after waiting {:.0} ms", waited.elapsed().as_secs_f64() * 1e3);
        let Some(OpenedOnThread { instance, opened }) = early else {
            eprintln!("early GPU opening dropped: {:?}", EarlyVerdict::NotOpened);
            return None;
        };
        let surface = instance.create_surface(window.clone()).ok();
        let can_present = surface.as_ref().is_some_and(|s| opened.adapter.is_surface_supported(s));
        match (verdict(true, true, can_present, opened.is_software()), surface) {
            (EarlyVerdict::Kept, Some(surface)) => Some(opened.with_surface(surface)),
            (v, _) => {
                eprintln!("early GPU opening dropped: {v:?}");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every case where `pick` could choose another device drops the early one; only the case where
    /// it is the same device keeps it. Sabotage checked: dropping any condition fails a row.
    #[test]
    fn early_device_kept_only_when_pick_would_choose_it() {
        use EarlyVerdict as V;
        // (order unchanged, opened, can present, software) → verdict.
        let rows = [
            ((true, true, true, false), V::Kept),
            ((false, true, true, false), V::OrderChanged),
            ((false, false, false, true), V::OrderChanged),
            ((true, false, true, false), V::NotOpened),
            ((true, false, false, true), V::NotOpened),
            ((true, true, false, false), V::CannotPresent),
            ((true, true, false, true), V::CannotPresent),
            ((true, true, true, true), V::Software),
        ];
        for ((order, opened, present, software), expected) in rows {
            assert_eq!(verdict(order, opened, present, software), expected, "{order} {opened} {present} {software}");
        }
    }

    /// The order checked at the join is the one `pick` follows: a preference that differs from the
    /// default changes it (the early device is then dropped), a forced API keeps it.
    #[test]
    fn order_known_before_the_settings() {
        let early = super::super::order(None, "default");
        assert_eq!(super::super::order(None, "vulkan"), early);
        assert_ne!(super::super::order(None, "opengl"), early);
        assert_eq!(super::super::order(Some("dx12"), "opengl"), super::super::order(Some("dx12"), "default"));
    }

    /// The old path's request on the first API, written out here on purpose rather than through
    /// the shared helper: high-performance adapter, no fallback, device opened as `pick` opens it.
    /// The window's surface is left out: no window is created in tests.
    fn old_path_request_without_surface(backends: wgpu::Backends) -> Option<(wgpu::AdapterInfo, wgpu::Device)> {
        let inst = super::super::instance(backends);
        let adapter = pollster::block_on(inst.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
            ..Default::default()
        }))
        .ok()?;
        let (device, _queue) = super::super::open_device(&adapter).ok()?;
        Some((adapter.get_info(), device))
    }

    /// The early opening, run on a "gpu-init" thread, asks for what the old path asks for: same
    /// adapter, same device features and limits. What this cannot prove without a window: that the
    /// adapter chosen without a surface is the one chosen with it. That rests on reading wgpu-core
    /// 30.0.1 (adapters filtered by surface, then a stable sort by type; the Vulkan and DX12
    /// backends list adapters without the surface) and is to be read again at every wgpu update.
    /// Times printed (loaded machine). Sabotage checked: the early opening asking for a low-power
    /// adapter and a device without features fails on the features.
    #[test]
    fn early_opening_asks_for_the_old_path_adapter_and_device() {
        crate::render::upload::tests::gpu_serial();
        let backends = super::super::order(rc_data::env_str!("RUSTCRAFT_BACKEND"), "default")[0];
        let t = Instant::now();
        let Some((info, device)) =
            rc_testdata::need(old_path_request_without_surface(backends), "GPU device on the first API")
        else {
            return;
        };
        let old_ms = t.elapsed().as_secs_f64() * 1e3;
        let t = Instant::now();
        let thread = std::thread::Builder::new().name("gpu-init".into()).spawn(move || open_without_window(backends));
        let early = thread.expect("thread").join().expect("join").expect("opened on the thread").opened;
        let early_ms = t.elapsed().as_secs_f64() * 1e3;
        eprintln!("opening {backends:?}: old path request {old_ms:.1} ms, early opening on its thread {early_ms:.1} ms");
        assert_eq!(early.adapter.get_info(), info, "adapter");
        assert_eq!(early.device.features(), device.features(), "device features");
        assert_eq!(early.device.limits(), device.limits(), "device limits");
    }
}
