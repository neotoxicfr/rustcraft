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
//!
//! The first API of the order is opened on a background thread while the resources load
//! (`early_open.rs`, `RUSTCRAFT_OPT_EARLY_GPU_OPEN`); the window is attached when it exists.

mod early_open;

pub use early_open::EarlyGpuOpen;

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

/// GPU context of the window: surface, adapter, device and queue (the surface keeps its instance
/// alive). The renderer owns it and hands it back on a resource reload
/// (`RUSTCRAFT_OPT_KEEP_GPU_ON_RELOAD`).
pub struct GpuContext {
    pub surface: wgpu::Surface<'static>,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub text: GpuText,
}

/// Adapter retained on one API and its device, before any surface is attached.
struct OpenedDevice {
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    text: GpuText,
}

impl OpenedDevice {
    /// Software adapter (WARP, lavapipe): only a last resort.
    fn is_software(&self) -> bool {
        self.adapter.get_info().device_type == wgpu::DeviceType::Cpu
    }

    fn with_surface(self, surface: wgpu::Surface<'static>) -> GpuContext {
        let OpenedDevice { adapter, device, queue, text } = self;
        GpuContext { surface, adapter, device, queue, text }
    }
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

/// Name of an API in the "Tried" list of the error box.
fn api_name(backends: wgpu::Backends) -> &'static str {
    if backends == wgpu::Backends::DX12 { "DirectX 12" } else { "Vulkan" }
}

/// Instance of one API, with the startup milestones around the driver's loading.
fn instance_with_marks(backends: wgpu::Backends) -> wgpu::Instance {
    // Separates the window work before it (background, icon) from the loading of the driver.
    crate::boot::mark("before GPU instance");
    let inst = instance(backends);
    crate::boot::mark(&format!("instance {}", api_name(backends)));
    inst
}

/// Adapter of `inst` (compatible with `surface` when one is given) and its device. The error
/// carries the stage that failed, as printed, and the reason for the "Tried" list.
fn open_adapter_and_device(
    inst: &wgpu::Instance,
    surface: Option<&wgpu::Surface<'_>>,
) -> Result<OpenedDevice, (&'static str, String)> {
    let found = pollster::block_on(inst.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: surface,
        force_fallback_adapter: false,
        ..Default::default()
    }));
    let adapter = found.map_err(|e| ("no compatible adapter", e.to_string()))?;
    crate::boot::mark("adapter");
    // Like the game: the device is created in the same attempt; a failure notes the "Tried"
    // line and moves on to the next API.
    let (device, queue) = open_device(&adapter).map_err(|e| ("cannot create a device", e))?;
    crate::boot::mark("GPU device");
    let text = GpuText::new(&adapter.get_info(), vulkan_api_version(&adapter));
    Ok(OpenedDevice { adapter, device, queue, text })
}

/// Hardware adapter (with its device) of the first API that offers one, otherwise the first software one found.
pub fn pick(window: &Arc<Window>) -> GpuContext {
    install_log();
    let forced = rc_data::env_str!("RUSTCRAFT_BACKEND");
    let mut software: Option<GpuContext> = None;
    let mut tried: Vec<(&str, String)> = Vec::new();
    for &backends in order(forced, startup_preference()) {
        let inst = instance_with_marks(backends);
        let name = api_name(backends);
        let surface = match inst.create_surface(window.clone()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("API {backends:?}: cannot create a surface");
                tried.push((name, e.to_string()));
                continue;
            }
        };
        let opened = match open_adapter_and_device(&inst, Some(&surface)) {
            Ok(o) => o,
            Err((stage, e)) => {
                eprintln!("API {backends:?}: {stage}");
                tried.push((name, e));
                continue;
            }
        };
        if !opened.is_software() {
            return opened.with_surface(surface);
        }
        if software.is_none() {
            software = Some(opened.with_surface(surface));
        }
    }
    software.unwrap_or_else(|| {
        // Like the game: native error box then stop.
        crate::video::error_box(&crate::video::no_backend_message(&tried));
        std::process::exit(1)
    })
}

/// Surface on `window` and its device through one API, opened like [`pick`] opens them, for the
/// proofs that need a real surface. `None`: this API has no hardware adapter for the window.
#[cfg(test)]
pub(crate) fn open_on_one_api(window: &Arc<Window>, backends: wgpu::Backends) -> Option<GpuContext> {
    let inst = instance(backends);
    let surface = inst.create_surface(window.clone()).ok()?;
    let opened = open_adapter_and_device(&inst, Some(&surface)).ok()?;
    (!opened.is_software()).then(|| opened.with_surface(surface))
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

    /// wgpu is built without its OpenGL and WebGPU backends (workspace `Cargo.toml`): `order` never
    /// asks for them and they weighed 450 KiB of code. Fails if a dependency turns them back on.
    #[test]
    fn only_the_tried_backends_are_compiled() {
        let compiled = wgpu::Instance::enabled_backend_features();
        let unused = wgpu::Backends::GL | wgpu::Backends::BROWSER_WEBGPU;
        assert!(!compiled.intersects(unused), "unused wgpu backends compiled in: {compiled:?}");
        if cfg!(windows) {
            assert_eq!(compiled, wgpu::Backends::VULKAN | wgpu::Backends::DX12);
        }
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
