//! Choice of the graphics API: Vulkan first, DX12 only as a fallback.
//!
//! A common Vulkan + DX12 instance lets wgpu sort the adapters by TYPE only
//! (discrete, integrated, other, virtual, software): the API order only breaks ties. A
//! card seen as "other" by Vulkan and as "discrete" by DX12 would therefore go to DX12, and both
//! APIs would be initialized at every launch. Here each API is tried alone, in
//! order: the first one that gives a hardware adapter compatible with the window wins;
//! a software adapter (WARP, lavapipe) is only a very last resort.
//!
//! The "Graphics API" setting (`preferredGraphicsBackend` of options.txt) chooses the order at
//! startup, like the game which tries one API then the other: "default" and "vulkan" try
//! Vulkan then DX12; "opengl" (the non-Vulkan API in the game) tries DX12 then Vulkan.
//! The value read at startup is retained: the video screen compares it with the current value to
//! announce that a restart is needed.
//!
//! `RUSTCRAFT_BACKEND=vulkan|dx12` forces a single API (comparisons, faulty drivers).

use std::sync::{Arc, OnceLock};
use winit::window::Window;

/// Value of the `preferredGraphicsBackend` setting read at startup.
static STARTUP_PREFERENCE: OnceLock<String> = OnceLock::new();

/// Retains the setting's value at startup (first call only, like the game which freezes it
/// when reading options.txt).
pub fn set_startup_preference(value: &str) {
    let _ = STARTUP_PREFERENCE.set(value.to_owned());
}

/// Setting value retained at startup ("default" if nothing was read).
pub fn startup_preference() -> &'static str {
    STARTUP_PREFERENCE.get().map_or("default", String::as_str)
}

/// Window surface and adapter retained (the surface keeps its instance alive).
pub struct Picked {
    pub surface: wgpu::Surface<'static>,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

/// APIs to try, in order, according to `RUSTCRAFT_BACKEND` then the `preferred` setting.
fn order(forced: Option<&str>, preferred: &str) -> &'static [wgpu::Backends] {
    match (forced, preferred) {
        (Some("dx12"), _) => &[wgpu::Backends::DX12],
        (Some("vulkan"), _) => &[wgpu::Backends::VULKAN],
        (_, "opengl") => &[wgpu::Backends::DX12, wgpu::Backends::VULKAN],
        _ => &[wgpu::Backends::VULKAN, wgpu::Backends::DX12],
    }
}

fn instance(backends: wgpu::Backends) -> wgpu::Instance {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = backends;
    // Indirect draw arguments built on the CPU with known bounds: wgpu's
    // validation (compute pass + per-draw cost) is superfluous.
    // `WGPU_VALIDATION_INDIRECT_CALL=1` restores it.
    // DX12: without this validation, wgpu does not rewrite the start instance of the indirect
    // commands into the shader constants; so no shader drawn indirectly must
    // read `@builtin(instance_index)` (the terrain reads its faces through an instance buffer,
    // which DX12 does offset itself).
    desc.flags.remove(wgpu::InstanceFlags::VALIDATION_INDIRECT_CALL);
    desc.flags = desc.flags.with_env();
    // DX12: shader compiler pinned (FXC, shipped with Windows) instead of wgpu's automatic choice,
    // which takes `dxcompiler.dll` if it finds one in the PATH: the result no longer depends
    // on the machine, and that DLL crashed on accented labels (e.g. "error reading
    // '<label with accents>'", seen with 1.8.2502). `WGPU_DX12_COMPILER=dxc` stays possible for
    // tests: the labels are then no longer passed to the driver.
    let compiler = wgpu::Dx12Compiler::Fxc.with_env();
    if !matches!(compiler, wgpu::Dx12Compiler::Fxc) {
        desc.flags |= wgpu::InstanceFlags::DISCARD_HAL_LABELS;
    }
    desc.backend_options.dx12.shader_compiler = compiler;
    wgpu::Instance::new(desc)
}

/// wgpu log on the error output (`RUSTCRAFT_GPULOG=warn|info|debug`): with
/// `WGPU_VALIDATION=1`, the messages of the Vulkan and DX12 validation layers go through it.
struct GpuLog;

impl log::Log for GpuLog {
    fn enabled(&self, m: &log::Metadata<'_>) -> bool {
        m.level() <= log::max_level()
    }
    fn log(&self, r: &log::Record<'_>) {
        if self.enabled(r.metadata()) {
            eprintln!("gpu {} {}: {}", r.level(), r.target(), r.args());
        }
    }
    fn flush(&self) {}
}

fn install_log() {
    let level = match rc_data::env_str!("RUSTCRAFT_GPULOG") {
        Some("debug") => log::LevelFilter::Debug,
        Some("info") => log::LevelFilter::Info,
        Some(_) => log::LevelFilter::Warn,
        None => return,
    };
    if log::set_logger(&GpuLog).is_ok() {
        log::set_max_level(level);
    }
}

/// Opens the device with the useful features the adapter offers.
fn open_device(adapter: &wgpu::Adapter) -> Result<(wgpu::Device, wgpu::Queue), String> {
    let available = adapter.features();
    let mut features = wgpu::Features::empty();
    features |= available & wgpu::Features::POLYGON_MODE_LINE;
    features |= available & wgpu::Features::TIMESTAMP_QUERY;
    // GPU time of the whole frame (`gpu_time.rs`): timestamps in the encoder; per-pass
    // diagnostic: pipeline statistics (fragments of the world pass).
    let timing = crate::gpu_time::settings();
    if timing.whole_frame && available.contains(wgpu::Features::TIMESTAMP_QUERY) {
        features |= available & wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
    }
    if timing.passes {
        features |= available & wgpu::Features::PIPELINE_STATISTICS_QUERY;
    }
    // MAX blend of RGBA32F depth bounds (improved transparency, `oit.rs`).
    features |= available & wgpu::Features::FLOAT32_BLENDABLE;
    // Grouped indirect draws: instances that do not start at 0 and indirect execution;
    // RUSTCRAFT_INDIRECT=0 goes back to one draw per series (comparison, future video setting).
    let indirect_ok = available.contains(wgpu::Features::INDIRECT_FIRST_INSTANCE)
        && adapter.get_downlevel_capabilities().flags.contains(wgpu::DownlevelFlags::INDIRECT_EXECUTION)
        && rc_data::env_str!("RUSTCRAFT_INDIRECT") != Some("0");
    if indirect_ok {
        features |= wgpu::Features::INDIRECT_FIRST_INSTANCE;
    }
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("rustcraft"),
        required_features: features,
        required_limits: adapter.limits(),
        // Memory blocks of 8 to 64 MB instead of 128 to 256: rare allocations (regions),
        // reservation much closer to the usage. `RUSTCRAFT_MEMHINT=perf`: wgpu's setting.
        memory_hints: if rc_data::env_str!("RUSTCRAFT_MEMHINT") == Some("perf") {
            wgpu::MemoryHints::Performance
        } else {
            wgpu::MemoryHints::MemoryUsage
        },
        ..Default::default()
    }))
    .map_err(|e| e.to_string())
}

/// Hardware adapter (with its device) of the first API that offers one, otherwise the first software one found.
pub fn pick(window: &Arc<Window>) -> Picked {
    install_log();
    let forced = rc_data::env_str!("RUSTCRAFT_BACKEND");
    let mut software: Option<Picked> = None;
    let mut tried: Vec<(&str, String)> = Vec::new();
    for &backends in order(forced, startup_preference()) {
        let inst = instance(backends);
        let name = if backends == wgpu::Backends::DX12 { "DirectX 12" } else { "Vulkan" };
        crate::boot::mark(&format!("instance {name}"));
        let surface = match inst.create_surface(window.clone()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("API {backends:?}: cannot create a surface");
                tried.push((name, e.to_string()));
                continue;
            }
        };
        let found = pollster::block_on(inst.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            ..Default::default()
        }));
        let adapter = match found {
            Ok(a) => a,
            Err(e) => {
                eprintln!("API {backends:?}: no compatible adapter");
                tried.push((name, e.to_string()));
                continue;
            }
        };
        crate::boot::mark("adapter");
        // Like the game: the device is created in the same attempt; a failure notes the "Tried"
        // line and moves on to the next API.
        let (device, queue) = match open_device(&adapter) {
            Ok(dq) => dq,
            Err(e) => {
                eprintln!("API {backends:?}: cannot create a device");
                tried.push((name, e));
                continue;
            }
        };
        crate::boot::mark("GPU device");
        if adapter.get_info().device_type != wgpu::DeviceType::Cpu {
            return Picked { surface, adapter, device, queue };
        }
        if software.is_none() {
            software = Some(Picked { surface, adapter, device, queue });
        }
    }
    software.unwrap_or_else(|| {
        // Like the game: native error box then stop.
        crate::video::error_box(&crate::video::no_backend_message(&tried));
        std::process::exit(1)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vulkan_first() {
        assert_eq!(order(None, "default"), &[wgpu::Backends::VULKAN, wgpu::Backends::DX12]);
        assert_eq!(order(Some("other"), "default"), &[wgpu::Backends::VULKAN, wgpu::Backends::DX12]);
        assert_eq!(order(Some("dx12"), "default"), &[wgpu::Backends::DX12]);
        assert_eq!(order(Some("vulkan"), "opengl"), &[wgpu::Backends::VULKAN]);
    }

    /// The "Graphics API" setting chooses the order (the environment variable takes priority).
    #[test]
    fn preference_sets_order() {
        assert_eq!(order(None, "vulkan"), &[wgpu::Backends::VULKAN, wgpu::Backends::DX12]);
        assert_eq!(order(None, "opengl"), &[wgpu::Backends::DX12, wgpu::Backends::VULKAN]);
        assert_eq!(order(None, "unknown"), &[wgpu::Backends::VULKAN, wgpu::Backends::DX12]);
        assert_eq!(order(Some("dx12"), "vulkan"), &[wgpu::Backends::DX12]);
    }
}
