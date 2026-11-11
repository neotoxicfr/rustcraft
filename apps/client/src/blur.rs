//! Blur of the menu background (`docs/specs/option-menus.md` § 1.12, `title-screen-servers.md` § 1.1):
//! the already drawn frame (world, HUD, or panorama outside a world) is copied then blurred by three
//! pairs of box passes (horizontal then vertical) of radius `menuBackgroundBlurriness`, in
//! real pixels; the last pass writes into the frame. The menu interface is drawn afterwards.
//!
//! Switches, read once when the blur is created (`=0`: the path before):
//! - `RUSTCRAFT_OPT_BLUR_SKIP_UNCHANGED_CONFIG` (on): the two pass configurations cross to the
//!   GPU only when the radius or the frame size changes, instead of on every blurred frame;
//! - `RUSTCRAFT_OPT_RELEASE_IDLE_TARGETS` (on, shared with the image effects): the work targets
//!   are freed a few seconds after the last blurred frame instead of living as long as the window;
//! - `RUSTCRAFT_OPT_COMPUTE_BLUR` (off, `=1` turns it on): the first five passes are compute
//!   passes that slide a sum along each line (`blur_compute.wgsl`), a texel costing two reads
//!   whatever the radius; the sixth stays a fragment pass, the only way to write into the frame.
//!   Off because it loses at the menu's radii (0 to 10) on the GPU measured and needs a third
//!   work target (`docs/perf.md`, C3-M14); its bytes are the exact box averages, one step away
//!   from the fragment passes wherever the GPU does not round their 8-bit writes to the nearest.
//!
//! The configurations are written through [`FrameBuffer`] (`RUSTCRAFT_OPT_FRAME_WRITES_RING`).

use std::time::{Duration, Instant};

use crate::render::upload::Uploader;
use crate::render::upload::per_frame::FrameBuffer;

/// Work targets unused for this long are freed (`RUSTCRAFT_OPT_RELEASE_IDLE_TARGETS`): long enough
/// for an inventory closed and opened again to find them, short enough to give the memory back
/// to the game right after a menu or a spectator effect.
pub(crate) const IDLE_RELEASE_AFTER: Duration = Duration::from_secs(5);

/// True when work targets last used at `last_used` have been idle long enough at `now`.
pub(crate) fn idle_long_enough(last_used: Option<Instant>, now: Instant) -> bool {
    last_used.is_some_and(|used| now.saturating_duration_since(used) >= IDLE_RELEASE_AFTER)
}

/// Full-screen work texture of an image pass.
pub(crate) fn work_texture(
    device: &wgpu::Device,
    label: &str,
    size: (u32, u32),
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}

/// Switches of the menu blur; the tests build every combination in one run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// Pass configurations written only when the radius or the frame size changes
    /// (`RUSTCRAFT_OPT_BLUR_SKIP_UNCHANGED_CONFIG`); off: on every blurred frame.
    pub skip_unchanged_config: bool,
    /// Work targets freed once idle (`RUSTCRAFT_OPT_RELEASE_IDLE_TARGETS`); off: kept for life.
    pub release_idle_targets: bool,
    /// First five passes as compute sliding sums (`RUSTCRAFT_OPT_COMPUTE_BLUR`, off by default);
    /// off: six fragment passes.
    pub compute_passes: bool,
}

impl Options {
    /// The switches' defaults: every lead that draws the bytes of before (the tests name the forms).
    #[cfg(test)]
    pub const NEW: Options = Options { skip_unchanged_config: true, release_idle_targets: true, compute_passes: false };
    /// The code before the leads.
    #[cfg(test)]
    pub const OLD: Options =
        Options { skip_unchanged_config: false, release_idle_targets: false, compute_passes: false };

    pub fn from_switches() -> Self {
        Options {
            skip_unchanged_config: rc_data::opt_switch!("RUSTCRAFT_OPT_BLUR_SKIP_UNCHANGED_CONFIG"),
            release_idle_targets: rc_data::opt_switch!("RUSTCRAFT_OPT_RELEASE_IDLE_TARGETS"),
            compute_passes: rc_data::opt_switch!("RUSTCRAFT_OPT_COMPUTE_BLUR", default_off),
        }
    }
}

/// Configuration of a pass (aligned to 16 bytes), read by the fragment and the compute shaders.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct PassConfig {
    inv_size: [f32; 2],
    dir: [f32; 2],
    radius: f32,
    pad: [f32; 3],
}

const PASS_CONFIG_BYTES: u64 = std::mem::size_of::<PassConfig>() as u64;
// The size of the `Cfg` of both shaders.
const _: () = assert!(PASS_CONFIG_BYTES == 32);

/// Texels one compute invocation walks along the pass direction (`RUN_TEXELS` of the shader), and
/// the invocations of a workgroup: runs side by side, lines one under the other.
const RUN_TEXELS: u32 = 64;
const WORKGROUP_RUNS: u32 = 4;
const WORKGROUP_LINES: u32 = 16;

/// The compute passes write the 8-bit values themselves: a format every device can store into.
const COMPUTE_TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// One of the six passes, with the bindings it reads.
enum Pass {
    /// Fragment pass into a work target, or into the frame (`None`) for the last one.
    Draw { reads: wgpu::BindGroup, into: Option<wgpu::TextureView> },
    /// Compute pass: source and destination are both in the bindings.
    Compute { reads_and_writes: wgpu::BindGroup, workgroups: [u32; 2] },
}

/// Work textures at the frame size, and the six passes that ping-pong between them.
struct Targets {
    size: (u32, u32),
    /// Receives the copy of the frame, read by the first pass.
    frame_copy: wgpu::Texture,
    passes: [Pass; 6],
}

/// Pipeline of the compute passes and layout of their bindings.
struct ComputePasses {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
}

/// Fault injected by a test, which must make an equivalence proof fail.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sabotage {
    /// The configurations written for the first blurred frame are never written again.
    ConfigWrittenOnce,
}

pub struct Blur {
    options: Options,
    /// Fragment passes and layout of their bindings.
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    /// `None` without the compute passes.
    compute: Option<ComputePasses>,
    sampler: wgpu::Sampler,
    horizontal_config: FrameBuffer,
    vertical_config: FrameBuffer,
    /// Frame size and radius of the configurations last written, `None` before the first write.
    written_config: Option<((u32, u32), i32)>,
    format: wgpu::TextureFormat,
    targets: Option<Targets>,
    targets_last_used: Option<Instant>,
    #[cfg(test)]
    pub(crate) sabotage: Option<Sabotage>,
}

impl Blur {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Blur {
        Self::with_options(device, format, Options::from_switches())
    }

    pub fn with_options(device: &wgpu::Device, format: wgpu::TextureFormat, options: Options) -> Blur {
        Self::with_compute_source(device, format, options, include_str!("blur_compute.wgsl"))
    }

    /// `compute_source`: the compute shader (the tests edit it for their sabotages).
    fn with_compute_source(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        options: Options,
        compute_source: &str,
    ) -> Blur {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blur"),
            entries: &[
                config_entry(wgpu::ShaderStages::FRAGMENT),
                source_entry(wgpu::ShaderStages::FRAGMENT),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blur"),
            source: wgpu::ShaderSource::Wgsl(include_str!("blur.wgsl").into()),
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("blur"),
            bind_group_layouts: &[Some(&layout)],
            ..Default::default()
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("blur"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                // After compute passes, the one fragment pass rounds its 8-bit values as they do.
                entry_point: Some(if options.compute_passes { "fs_rounded" } else { "fs" }),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        // Bilinear sampling, repeated edges (like the game's target).
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("blur"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        // Made now: the bindings of the targets read them before the first write.
        let config = |label| {
            let mut buffer = FrameBuffer::new(label, wgpu::BufferUsages::UNIFORM, PASS_CONFIG_BYTES);
            buffer.reserve(device, PASS_CONFIG_BYTES);
            buffer
        };
        Blur {
            options,
            pipeline,
            layout,
            compute: options.compute_passes.then(|| ComputePasses::new(device, compute_source)),
            sampler,
            horizontal_config: config("blur horizontal"),
            vertical_config: config("blur vertical"),
            written_config: None,
            format,
            targets: None,
            targets_last_used: None,
            #[cfg(test)]
            sabotage: None,
        }
    }

    /// Frees the work targets once they have been idle long enough at `now`; the next blurred
    /// frame makes them again, as the first one did.
    pub fn release_idle_targets(&mut self, now: Instant) {
        if self.options.release_idle_targets && idle_long_enough(self.targets_last_used, now) {
            self.targets = None;
            self.targets_last_used = None;
        }
    }

    /// Bytes of the work textures now allocated: two for the fragment passes, a third one with
    /// the compute passes.
    #[cfg(test)]
    fn work_target_bytes(&self) -> u64 {
        let textures = if self.compute.is_some() { 3 } else { 2 };
        self.targets.as_ref().map_or(0, |t| textures * texture_bytes(t.size))
    }

    fn prepare_targets(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        if self.targets.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        self.targets = Some(match &self.compute {
            Some(compute) => self.compute_targets(device, size, compute),
            None => self.fragment_targets(device, size),
        });
    }

    /// Bindings of a fragment pass reading `source` with the configuration `config`.
    fn sampled(&self, device: &wgpu::Device, source: &wgpu::TextureView, config: &FrameBuffer) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("blur"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: config_binding(config) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(source) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        })
    }

    /// Six fragment passes: A → B (horizontal), B → A (vertical) twice, then A → B, B → frame.
    fn fragment_targets(&self, device: &wgpu::Device, size: (u32, u32)) -> Targets {
        let usage = wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_DST;
        let a = work_texture(device, "blur A", size, self.format, usage);
        let b = work_texture(device, "blur B", size, self.format, usage);
        let (a_view, b_view) = (a.create_view(&Default::default()), b.create_view(&Default::default()));
        let read_a = self.sampled(device, &a_view, &self.horizontal_config);
        let read_b = self.sampled(device, &b_view, &self.vertical_config);
        let horizontal = || Pass::Draw { reads: read_a.clone(), into: Some(b_view.clone()) };
        let vertical = || Pass::Draw { reads: read_b.clone(), into: Some(a_view.clone()) };
        let onto_frame = Pass::Draw { reads: read_b.clone(), into: None };
        Targets {
            size,
            frame_copy: a,
            passes: [horizontal(), vertical(), horizontal(), vertical(), horizontal(), onto_frame],
        }
    }

    /// Five compute passes then a fragment one: copy → C (horizontal), C → D (vertical),
    /// D → C, C → D, D → C, and C → frame (vertical). The frame's own format cannot always be
    /// stored into by a compute pass, hence C and D beside the copy.
    fn compute_targets(&self, device: &wgpu::Device, size: (u32, u32), compute: &ComputePasses) -> Targets {
        let sampled = wgpu::TextureUsages::TEXTURE_BINDING;
        let stored = sampled | wgpu::TextureUsages::STORAGE_BINDING;
        let frame_copy =
            work_texture(device, "blur frame copy", size, self.format, sampled | wgpu::TextureUsages::COPY_DST);
        let c = work_texture(device, "blur C", size, COMPUTE_TARGET_FORMAT, stored);
        let d = work_texture(device, "blur D", size, COMPUTE_TARGET_FORMAT, stored);
        let view = |texture: &wgpu::Texture| texture.create_view(&Default::default());
        let (copy_view, c_view, d_view) = (view(&frame_copy), view(&c), view(&d));
        let slide = |config: &FrameBuffer, source: &wgpu::TextureView, destination: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("blur (compute)"),
                layout: &compute.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: config_binding(config) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(source) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(destination) },
                ],
            })
        };
        let copy_to_c = slide(&self.horizontal_config, &copy_view, &c_view);
        let c_to_d = slide(&self.vertical_config, &c_view, &d_view);
        let d_to_c = slide(&self.horizontal_config, &d_view, &c_view);
        // Runs along the pass direction, lines across it.
        let (along_x, along_y) = (workgroups(size.0, size.1), workgroups(size.1, size.0));
        let compute_pass =
            |bindings: &wgpu::BindGroup, workgroups| Pass::Compute { reads_and_writes: bindings.clone(), workgroups };
        let onto_frame = Pass::Draw { reads: self.sampled(device, &c_view, &self.vertical_config), into: None };
        Targets {
            size,
            frame_copy,
            passes: [
                compute_pass(&copy_to_c, along_x),
                compute_pass(&c_to_d, along_y),
                compute_pass(&d_to_c, along_x),
                compute_pass(&c_to_d, along_y),
                compute_pass(&d_to_c, along_x),
                onto_frame,
            ],
        }
    }

    /// Writes the two pass configurations when the radius or the frame size changed.
    fn write_configs(&mut self, up: &mut Uploader, queue: &wgpu::Queue, size: (u32, u32), radius: i32) {
        let unchanged = self.written_config == Some((size, radius));
        #[cfg(test)]
        let unchanged =
            unchanged || (self.sabotage == Some(Sabotage::ConfigWrittenOnce) && self.written_config.is_some());
        if self.options.skip_unchanged_config && unchanged {
            return;
        }
        let horizontal = PassConfig {
            inv_size: [1.0 / size.0 as f32, 1.0 / size.1 as f32],
            dir: [1.0, 0.0],
            radius: radius as f32,
            pad: [0.0; 3],
        };
        let vertical = PassConfig { dir: [0.0, 1.0], ..horizontal };
        self.horizontal_config.write(up, queue, bytemuck::bytes_of(&horizontal));
        self.vertical_config.write(up, queue, bytemuck::bytes_of(&vertical));
        self.written_config = Some((size, radius));
    }

    /// Blurs `frame` (already drawn, usable as a copy source) in place: a copy of it, then the six
    /// passes, the last one into the frame. `timer`: GPU time of the six passes (name "blur").
    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        up: &mut Uploader,
        enc: &mut wgpu::CommandEncoder,
        frame: &wgpu::Texture,
        frame_view: &wgpu::TextureView,
        radius: i32,
        timer: Option<&crate::gpu_time::GpuTimer>,
    ) {
        let size = (frame.width(), frame.height());
        self.write_configs(up, queue, size, radius);
        self.prepare_targets(device, size);
        self.targets_last_used = Some(Instant::now());
        let Some(targets) = &self.targets else { return };
        enc.copy_texture_to_texture(
            frame.as_image_copy(),
            targets.frame_copy.as_image_copy(),
            wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
        );
        for pass in &targets.passes {
            let timestamp_writes = timer.and_then(|t| t.pass("blur", crate::gpu_time::Role::Inner));
            match pass {
                Pass::Draw { reads, into } => {
                    let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("blur"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: into.as_ref().unwrap_or(frame_view),
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                        })],
                        timestamp_writes,
                        ..Default::default()
                    });
                    pass.set_pipeline(&self.pipeline);
                    pass.set_bind_group(0, reads, &[]);
                    pass.draw(0..3, 0..1);
                }
                Pass::Compute { reads_and_writes, workgroups } => {
                    let Some(compute) = &self.compute else { continue };
                    let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("blur"),
                        timestamp_writes: timestamp_writes.map(|w| wgpu::ComputePassTimestampWrites {
                            query_set: w.query_set,
                            beginning_of_pass_write_index: w.beginning_of_pass_write_index,
                            end_of_pass_write_index: w.end_of_pass_write_index,
                        }),
                    });
                    pass.set_pipeline(&compute.pipeline);
                    pass.set_bind_group(0, reads_and_writes, &[]);
                    pass.dispatch_workgroups(workgroups[0], workgroups[1], 1);
                }
            }
        }
    }
}

impl ComputePasses {
    fn new(device: &wgpu::Device, source: &str) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blur (compute)"),
            entries: &[
                config_entry(wgpu::ShaderStages::COMPUTE),
                source_entry(wgpu::ShaderStages::COMPUTE),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: COMPUTE_TARGET_FORMAT,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blur (compute)"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("blur (compute)"),
            bind_group_layouts: &[Some(&layout)],
            ..Default::default()
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("blur (compute)"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("blur_runs"),
            compilation_options: Default::default(),
            cache: None,
        });
        ComputePasses { pipeline, layout }
    }
}

/// Binding 0 of both forms: the pass configuration.
fn config_entry(visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding: 0,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

/// Binding 1 of both forms: the texture the pass reads.
fn source_entry(visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding: 1,
        visibility,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn config_binding(config: &FrameBuffer) -> wgpu::BindingResource<'_> {
    config.buffer().expect("configuration buffer made with the blur").as_entire_binding()
}

/// Workgroups of a compute pass over lines of `along` texels, `lines` of them.
fn workgroups(along: u32, lines: u32) -> [u32; 2] {
    [along.div_ceil(RUN_TEXELS).div_ceil(WORKGROUP_RUNS), lines.div_ceil(WORKGROUP_LINES)]
}

/// Bytes of one work texture (4 per texel in every format used here).
#[cfg(test)]
pub(crate) fn texture_bytes(size: (u32, u32)) -> u64 {
    u64::from(size.0) * u64::from(size.1) * 4
}

#[cfg(test)]
#[path = "blur_test.rs"]
pub(crate) mod tests;
