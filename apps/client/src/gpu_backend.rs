//! Choice of the graphics API: Vulkan first, DX12 only as a fallback.
//!
//! A common Vulkan + DX12 instance lets wgpu sort the adapters by TYPE only
//! (discrete, integrated, other, virtual, software): the API order only breaks ties. A
//! card seen as "other" by Vulkan and as "discrete" by DX12 would therefore go to DX12, and both
//! APIs would be initialized at every launch. Here each API is tried alone, in
//! order: the first one that gives a hardware adapter compatible with the window wins;
//! a software adapter (WARP, lavapipe) is only a very last resort. The game's list of refused
//! Vulkan devices only concerns old Intel GPUs under MoltenVK (Vulkan over Metal, macOS), which
//! our Windows and Linux builds never use: nothing is refused here.
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
    pub text: GpuText,
}

/// Adapter described in the game's words, computed once at startup (F3 `system_specs`, system
/// report of F3+L).
#[derive(Clone, Debug, PartialEq)]
pub struct GpuText {
    /// Vendor: short name of the known vendors, otherwise the identifier in hexadecimal.
    pub vendor: String,
    pub backend: &'static str,
    /// Vulkan: API version of the device, driver name and driver information.
    pub driver: String,
}

/// Vendor name of a PCI vendor identifier, as the game names them (`0x…` otherwise).
pub fn vendor_name(id: u32) -> String {
    match id {
        0x1002 | 0x1022 => "AMD",
        0x1010 => "IMGTEC",
        0x106B => "APPLE",
        0x10DE | 0x12D2 => "NVIDIA",
        0x13B5 => "ARM",
        0x1414 => "MICROSOFT",
        0x14E4 => "BROADCOM",
        0x168C | 0x17CB | 0x1969 | 0x5143 => "QUALCOMM",
        0x8086 => "INTEL",
        _ => return format!("0x{id:x}"),
    }
    .to_owned()
}

fn backend_name(b: wgpu::Backend) -> &'static str {
    match b {
        wgpu::Backend::Vulkan => "Vulkan",
        wgpu::Backend::Dx12 => "DX12",
        wgpu::Backend::Metal => "Metal",
        wgpu::Backend::Gl => "OpenGL",
        _ => "Unknown",
    }
}

impl GpuText {
    /// `api`: Vulkan API version of the device (packed major 7 bits, minor 10, patch 12).
    pub fn new(info: &wgpu::AdapterInfo, api: Option<u32>) -> GpuText {
        let driver = match api {
            Some(v) => {
                format!("{}.{}.{} {} {}", v >> 22 & 127, v >> 12 & 1023, v & 4095, info.driver, info.driver_info)
            }
            // No game equivalent (DX12): name and information of the driver, trimmed.
            None => format!("{} {}", info.driver, info.driver_info).trim().to_owned(),
        };
        GpuText { vendor: vendor_name(info.vendor), backend: backend_name(info.backend), driver }
    }
}

/// Vulkan API version of the device (beyond `AdapterInfo`); `None` on another API.
#[cfg(any(windows, target_os = "linux"))]
#[allow(unsafe_code)]
fn vulkan_api_version(a: &wgpu::Adapter) -> Option<u32> {
    // SAFETY: only reads the properties the Vulkan backend cached when it listed the device; no
    // raw handle is used or kept.
    let hal = unsafe { a.as_hal::<wgpu::hal::api::Vulkan>() }?;
    Some(hal.physical_device_capabilities().properties().api_version)
}

#[cfg(not(any(windows, target_os = "linux")))]
fn vulkan_api_version(_: &wgpu::Adapter) -> Option<u32> {
    None
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
        let info = adapter.get_info();
        let text = GpuText::new(&info, vulkan_api_version(&adapter));
        if info.device_type != wgpu::DeviceType::Cpu {
            return Picked { surface, adapter, device, queue, text };
        }
        if software.is_none() {
            software = Some(Picked { surface, adapter, device, queue, text });
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

    /// Vendor names of the game, hexadecimal otherwise; Vulkan driver line with the API version.
    #[test]
    fn adapter_text_like_the_game() {
        assert_eq!(vendor_name(0x10DE), "NVIDIA");
        assert_eq!(vendor_name(0x1002), "AMD");
        assert_eq!(vendor_name(0x8086), "INTEL");
        assert_eq!(vendor_name(0x5143), "QUALCOMM");
        assert_eq!(vendor_name(0xABC), "0xabc");
        let mut info = wgpu::AdapterInfo::new(wgpu::DeviceType::DiscreteGpu, wgpu::Backend::Vulkan);
        info.vendor = 0x10DE;
        info.driver = "NVIDIA".into();
        info.driver_info = "566.36".into();
        // 1.3.289: (1 << 22) | (3 << 12) | 289.
        let t = GpuText::new(&info, Some((1 << 22) | (3 << 12) | 289));
        assert_eq!((t.vendor.as_str(), t.backend, t.driver.as_str()), ("NVIDIA", "Vulkan", "1.3.289 NVIDIA 566.36"));
        info.backend = wgpu::Backend::Dx12;
        info.driver.clear();
        let dx = GpuText::new(&info, None);
        assert_eq!((dx.backend, dx.driver.as_str()), ("DX12", "566.36"));
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
