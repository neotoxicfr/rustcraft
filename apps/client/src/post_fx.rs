//! Full-screen image effects (`docs/specs/miscellaneous.md` § 5.3 and § 6): effects imposed
//! by the server (`post_effects`) and effect of the entity watched in spectator (creeper, spider,
//! enderman). Applied after the world and the hand, before the interface. Hard-wired: `invert`,
//! `creeper`, `spider` and `blur` (menu blur); any other id is ignored.
//!
//! Switches, read once when the effects are created (`=0`: the path before):
//! - `RUSTCRAFT_OPT_POST_FX_SKIP_UNCHANGED_CONFIG` (on): the configurations of every pass cross
//!   to the GPU in one block when the frame size changes, instead of one write per pass on every
//!   frame;
//! - `RUSTCRAFT_OPT_POST_FX_BIND_GROUP_CACHE` (on): the bindings of a pass are made once per pair
//!   of work targets it reads, instead of once per pass on every frame;
//! - `RUSTCRAFT_OPT_RELEASE_IDLE_TARGETS` (on, shared with the menu blur): the work targets are
//!   freed a few seconds after the last frame with an effect instead of living as long as the
//!   window;
//! - `RUSTCRAFT_OPT_POST_FX_TARGET_ALIASING` (on): an effect allocates only the work targets it
//!   uses and writes over those whose image is no longer read: 1 for invert, 2 for creeper, 3 for
//!   spider, instead of 5 whatever the effect;
//! - `RUSTCRAFT_OPT_CREEPER_PASS_FUSION` (off, `=1` turns it on): the two creeper passes in one,
//!   which rounds the green image to 8 bits itself where the first pass stored it into an 8-bit
//!   target. Off because a GPU does not have to round that write to the nearest: on the one
//!   measured, 0.2 % of the colours come out one of the 16 levels away (`docs/perf.md`, C3-M14).
//!
//! The block of configurations is written through [`FrameBuffer`]
//! (`RUSTCRAFT_OPT_FRAME_WRITES_RING`).

use std::time::Instant;

use crate::blur::{idle_long_enough, work_texture};
use crate::render::upload::Uploader;
use crate::render::upload::per_frame::FrameBuffer;

/// Known effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    Invert,
    Creeper,
    Spider,
    Blur,
}

impl Effect {
    /// Effect of an id `ns:path` (`minecraft:` by default), if it is wired.
    pub fn from_id(id: &str) -> Option<Effect> {
        let path = id.strip_prefix("minecraft:").unwrap_or(id);
        Some(match path {
            "invert" => Effect::Invert,
            "creeper" => Effect::Creeper,
            "spider" => Effect::Spider,
            "blur" => Effect::Blur,
            _ => return None,
        })
    }

    /// Effect of a watched entity (§ 6.1).
    pub fn for_camera(kind: &str) -> Option<Effect> {
        match kind {
            "creeper" => Some(Effect::Creeper),
            "spider" | "cave_spider" => Some(Effect::Spider),
            "enderman" => Some(Effect::Invert),
            _ => None,
        }
    }

    /// Place of the effect's passes in [`Plan::chains`]; `None` for the menu blur, which has its
    /// own passes (`blur.rs`).
    fn chain_index(self) -> Option<usize> {
        match self {
            Effect::Invert => Some(0),
            Effect::Creeper => Some(1),
            Effect::Spider => Some(2),
            Effect::Blur => None,
        }
    }
}

/// Switches of the image effects; the tests build every combination in one run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// Pass configurations written in one block per frame size
    /// (`RUSTCRAFT_OPT_POST_FX_SKIP_UNCHANGED_CONFIG`); off: one write per pass on every frame.
    pub skip_unchanged_config: bool,
    /// Bindings kept per pair of work targets read (`RUSTCRAFT_OPT_POST_FX_BIND_GROUP_CACHE`);
    /// off: made for every pass of every frame.
    pub bind_group_cache: bool,
    /// Work targets freed once idle (`RUSTCRAFT_OPT_RELEASE_IDLE_TARGETS`); off: kept for life.
    pub release_idle_targets: bool,
    /// Only the work targets an effect uses, reused once their image is no longer read
    /// (`RUSTCRAFT_OPT_POST_FX_TARGET_ALIASING`); off: five targets, one per image.
    pub target_aliasing: bool,
    /// Creeper in one pass (`RUSTCRAFT_OPT_CREEPER_PASS_FUSION`, off by default); off: two passes.
    pub creeper_pass_fusion: bool,
}

impl Options {
    /// The switches' defaults: every lead that draws the bytes of before (the tests name the forms).
    #[cfg(test)]
    pub const NEW: Options = Options {
        skip_unchanged_config: true,
        bind_group_cache: true,
        release_idle_targets: true,
        target_aliasing: true,
        creeper_pass_fusion: false,
    };
    /// The code before the leads.
    #[cfg(test)]
    pub const OLD: Options = Options {
        skip_unchanged_config: false,
        bind_group_cache: false,
        release_idle_targets: false,
        target_aliasing: false,
        creeper_pass_fusion: false,
    };

    pub fn from_switches() -> Self {
        Options {
            skip_unchanged_config: rc_data::opt_switch!("RUSTCRAFT_OPT_POST_FX_SKIP_UNCHANGED_CONFIG"),
            bind_group_cache: rc_data::opt_switch!("RUSTCRAFT_OPT_POST_FX_BIND_GROUP_CACHE"),
            release_idle_targets: rc_data::opt_switch!("RUSTCRAFT_OPT_RELEASE_IDLE_TARGETS"),
            target_aliasing: rc_data::opt_switch!("RUSTCRAFT_OPT_POST_FX_TARGET_ALIASING"),
            creeper_pass_fusion: rc_data::opt_switch!("RUSTCRAFT_OPT_CREEPER_PASS_FUSION", default_off),
        }
    }
}

/// Pass configuration (96 bytes, placed every [`SLOT_BYTES`]).
#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct PassConfig {
    size: [f32; 2],
    dir: [f32; 2],
    radius: f32,
    rotation: f32,
    pad: [f32; 2],
    scale: [f32; 2],
    offset: [f32; 2],
    scissor: [f32; 4],
    vignette: [f32; 4],
    modulate: [f32; 4],
}

const PASS_CONFIG_BYTES: u64 = std::mem::size_of::<PassConfig>() as u64;
/// Distance between two configurations in their buffer (the alignment of a dynamic offset).
const SLOT_BYTES: u64 = 256;
// The size of the shader's `Cfg`, which fits in a slot.
const _: () = assert!(PASS_CONFIG_BYTES == 96 && PASS_CONFIG_BYTES <= SLOT_BYTES);
/// Passes an effect list runs in one frame at most: the slots of the configuration buffer. An
/// effect whose passes no longer fit is not applied, nor those after it.
const PASSES_PER_FRAME: u64 = 32;

/// Most work targets an effect uses (spider with one target per image).
const MAX_WORK_TARGETS: usize = 5;
/// Work target that receives the copy of the frame.
const FRAME_COPY: usize = 0;

/// Fragment program of a pass; its number is its place in [`PostFx::pipelines`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Program {
    Blur,
    Invert,
    Color,
    Bits,
    Spider,
    Blit,
    Creeper,
}

impl Program {
    const ALL: [Program; 7] = [
        Program::Blur,
        Program::Invert,
        Program::Color,
        Program::Bits,
        Program::Spider,
        Program::Blit,
        Program::Creeper,
    ];

    fn entry_point(self) -> &'static str {
        match self {
            Program::Blur => "fs_blur",
            Program::Invert => "fs_invert",
            Program::Color => "fs_color",
            Program::Bits => "fs_bits",
            Program::Spider => "fs_spider",
            Program::Blit => "fs_blit",
            Program::Creeper => "fs_creeper",
        }
    }

    /// False for the programs no chain uses under `options`: their pipeline is not made.
    fn used(self, options: Options) -> bool {
        match self {
            Program::Color | Program::Bits => !options.creeper_pass_fusion,
            Program::Creeper => options.creeper_pass_fusion,
            _ => true,
        }
    }
}

/// Output of a pass: a work target or the frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Output {
    Target(usize),
    Frame,
}

/// One full-screen pass: `program` reads the work targets `source` and `back` (the same one for
/// the programs with a single input).
#[derive(Clone, Copy)]
struct Pass {
    program: Program,
    source: usize,
    back: usize,
    output: Output,
    config: PassConfig,
}

impl Pass {
    fn new(program: Program, source: usize, output: Output, config: PassConfig) -> Pass {
        Pass { program, source, back: source, output, config }
    }

    /// Eye of the spider: `source` laid over the image `back`.
    fn eye(source: usize, back: usize, output: usize, config: PassConfig) -> Pass {
        Pass { program: Program::Spider, source, back, output: Output::Target(output), config }
    }
}

/// Passes of one effect at a frame size.
struct Chain {
    passes: Vec<Pass>,
    /// Work targets the passes use (the first ones).
    work_targets: usize,
    /// Passes counted against [`PASSES_PER_FRAME`]: those of the effect before any fusion, so
    /// that a long list stops at the same effect whatever the switches.
    counted_passes: u64,
    /// Slot of the first pass in the block of configurations written once per frame size.
    first_slot: u64,
}

impl Chain {
    fn new(passes: Vec<Pass>, counted_passes: u64, first_slot: u64) -> Chain {
        let highest_target = passes
            .iter()
            .flat_map(|p| [p.source, p.back, if let Output::Target(t) = p.output { t } else { FRAME_COPY }])
            .max()
            .unwrap_or(FRAME_COPY);
        Chain { passes, work_targets: highest_target + 1, counted_passes, first_slot }
    }
}

/// Passes of `invert`: the copy of the frame, inverted, into the frame.
fn invert_chain(base: PassConfig) -> Vec<Pass> {
    vec![Pass::new(Program::Invert, FRAME_COPY, Output::Frame, base)]
}

/// Passes of `creeper`: green image then mosaic, or both at once.
fn creeper_chain(base: PassConfig, one_pass: bool) -> Vec<Pass> {
    if one_pass {
        return vec![Pass::new(Program::Creeper, FRAME_COPY, Output::Frame, base)];
    }
    const GREEN: usize = 1;
    vec![
        Pass::new(Program::Color, FRAME_COPY, Output::Target(GREEN), base),
        Pass::new(Program::Bits, GREEN, Output::Frame, base),
    ]
}

/// Passes of `spider` (§ 6.3): a large and a small blur of the frame, five eyes laid one over the
/// other, a reddish copy into the frame.
///
/// `reuse_targets`: three work targets instead of five. The first eye (sharp frame over the
/// large blur) is drawn before the small blur, which changes no image and frees two targets for
/// what follows: the large blur's target then takes the first half of the small blur, and the
/// copy of the frame its second half.
fn spider_chain(base: PassConfig, reuse_targets: bool) -> Vec<Pass> {
    const ACROSS: [f32; 2] = [1.0, 0.0];
    const DOWN: [f32; 2] = [0.0, 1.0];
    const WHOLE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
    const NARROW: [f32; 4] = [0.21, 0.0, 0.79, 1.0];
    const SMALL_EYE_FADE: [f32; 4] = [0.31, 0.1, 0.69, 0.9];
    let blur = |source, output, radius, dir| {
        Pass::new(Program::Blur, source, Output::Target(output), PassConfig { radius, dir, ..base })
    };
    let eye =
        |scale, offset, rotation, scissor, vignette| PassConfig { scale, offset, rotation, scissor, vignette, ..base };
    let eyes = [
        eye([1.25, 2.0], [-0.125, -0.1], 0.0, WHOLE, [0.1, 0.1, 0.9, 0.9]),
        eye([2.35, 4.2], [-1.1, -1.5], -45.0, NARROW, SMALL_EYE_FADE),
        eye([2.35, 4.2], [0.45, -4.45], 45.0, NARROW, SMALL_EYE_FADE),
        eye([2.35, 2.35], [-0.385, -1.29], 0.0, WHOLE, SMALL_EYE_FADE),
        eye([2.35, 2.35], [-0.965, -1.29], 0.0, WHOLE, SMALL_EYE_FADE),
    ];
    let reddish =
        |source| Pass::new(Program::Blit, source, Output::Frame, PassConfig { modulate: [1.0, 0.8, 0.8, 1.0], ..base });
    if reuse_targets {
        const FIRST: usize = 1;
        const SECOND: usize = 2;
        return vec![
            blur(FRAME_COPY, FIRST, 15.0, ACROSS),
            blur(FIRST, SECOND, 15.0, DOWN),
            Pass::eye(FRAME_COPY, SECOND, FIRST, eyes[0]),
            // The large blur (SECOND) is no longer read: first half of the small blur.
            blur(FRAME_COPY, SECOND, 7.0, ACROSS),
            // The copy of the frame is no longer read: it becomes the small blur.
            blur(SECOND, FRAME_COPY, 7.0, DOWN),
            Pass::eye(FRAME_COPY, FIRST, SECOND, eyes[1]),
            Pass::eye(FRAME_COPY, SECOND, FIRST, eyes[2]),
            Pass::eye(FRAME_COPY, FIRST, SECOND, eyes[3]),
            Pass::eye(FRAME_COPY, SECOND, FIRST, eyes[4]),
            reddish(FIRST),
        ];
    }
    const TEMP: usize = 1;
    const LARGE_BLUR: usize = 2;
    const SMALL_BLUR: usize = 3;
    const SWAP: usize = 4;
    vec![
        blur(FRAME_COPY, TEMP, 15.0, ACROSS),
        blur(TEMP, LARGE_BLUR, 15.0, DOWN),
        blur(FRAME_COPY, TEMP, 7.0, ACROSS),
        blur(TEMP, SMALL_BLUR, 7.0, DOWN),
        Pass::eye(FRAME_COPY, LARGE_BLUR, TEMP, eyes[0]),
        Pass::eye(SMALL_BLUR, TEMP, SWAP, eyes[1]),
        Pass::eye(SMALL_BLUR, SWAP, TEMP, eyes[2]),
        Pass::eye(SMALL_BLUR, TEMP, SWAP, eyes[3]),
        Pass::eye(SMALL_BLUR, SWAP, TEMP, eyes[4]),
        reddish(TEMP),
    ]
}

/// Passes of every effect at a frame size.
struct Plan {
    size: (u32, u32),
    /// By [`Effect::chain_index`].
    chains: [Chain; 3],
}

impl Plan {
    fn new(size: (u32, u32), options: Options) -> Plan {
        let base = PassConfig { size: [size.0 as f32, size.1 as f32], modulate: [1.0; 4], ..Default::default() };
        let invert = Chain::new(invert_chain(base), 1, 0);
        let creeper = Chain::new(creeper_chain(base, options.creeper_pass_fusion), 2, invert.passes.len() as u64);
        let spider_first_slot = creeper.first_slot + creeper.passes.len() as u64;
        let spider = Chain::new(spider_chain(base, options.target_aliasing), 10, spider_first_slot);
        Plan { size, chains: [invert, creeper, spider] }
    }

    /// The configurations of every pass, each at its slot.
    fn config_block(&self) -> Vec<u8> {
        let passes = self.chains.iter().flat_map(|chain| &chain.passes);
        let mut block = Vec::new();
        for pass in passes {
            let slot_start = block.len();
            block.extend_from_slice(bytemuck::bytes_of(&pass.config));
            block.resize(slot_start + SLOT_BYTES as usize, 0);
        }
        block
    }
}

/// Work targets at the frame size, made as the effects ask for them, and the bindings that read
/// them.
struct Targets {
    size: (u32, u32),
    textures: Vec<wgpu::Texture>,
    views: Vec<wgpu::TextureView>,
    /// Bindings by (`source`, `back`), made at their first use.
    bind_groups: [[Option<wgpu::BindGroup>; MAX_WORK_TARGETS]; MAX_WORK_TARGETS],
}

impl Targets {
    fn new(size: (u32, u32)) -> Targets {
        Targets {
            size,
            textures: Vec::new(),
            views: Vec::new(),
            bind_groups: [const { [const { None }; MAX_WORK_TARGETS] }; MAX_WORK_TARGETS],
        }
    }

    /// At least `count` targets.
    fn grow(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat, count: usize) {
        while self.textures.len() < count {
            let usage = wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_DST;
            let texture = work_texture(device, "image effects", self.size, format, usage);
            self.views.push(texture.create_view(&Default::default()));
            self.textures.push(texture);
        }
    }

    /// Bindings of `pass`. `kept_under`: the pair of targets they are kept under for the next
    /// passes and frames; `None`: made for this pass alone.
    fn bindings(
        &mut self,
        device: &wgpu::Device,
        shared: &SharedBindings,
        pass: &Pass,
        kept_under: Option<(usize, usize)>,
    ) -> wgpu::BindGroup {
        let make = || shared.bind(device, &self.views[pass.source], &self.views[pass.back]);
        match kept_under {
            Some((source, back)) => self.bind_groups[source][back].get_or_insert_with(make).clone(),
            None => make(),
        }
    }
}

/// What the bindings of every pass share: their layout, the samplers and the configurations.
struct SharedBindings {
    layout: wgpu::BindGroupLayout,
    linear: wgpu::Sampler,
    nearest: wgpu::Sampler,
    /// Configurations of the passes, one per slot; a pass picks its own by a dynamic offset.
    configs: FrameBuffer,
}

impl SharedBindings {
    fn config_buffer(&self) -> &wgpu::Buffer {
        self.configs.buffer().expect("configuration buffer made with the effects")
    }

    /// Bindings of a pass reading the targets `source` and `back`.
    fn bind(&self, device: &wgpu::Device, source: &wgpu::TextureView, back: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("image effects"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: self.config_buffer(),
                        offset: 0,
                        size: wgpu::BufferSize::new(PASS_CONFIG_BYTES),
                    }),
                },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(source) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(back) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.linear) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&self.nearest) },
            ],
        })
    }
}

/// One full-screen triangle drawn by `pipeline` into `output`, its configuration `config_offset`
/// bytes into the configuration buffer.
fn draw_pass(
    enc: &mut wgpu::CommandEncoder,
    pipeline: &wgpu::RenderPipeline,
    bindings: &wgpu::BindGroup,
    config_offset: u64,
    output: &wgpu::TextureView,
    timer: Option<&crate::gpu_time::GpuTimer>,
) {
    let mut render_pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("image effect"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: output,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
        })],
        timestamp_writes: timer.and_then(|t| t.pass("image effect", crate::gpu_time::Role::Inner)),
        ..Default::default()
    });
    render_pass.set_pipeline(pipeline);
    render_pass.set_bind_group(0, bindings, &[config_offset as u32]);
    render_pass.draw(0..3, 0..1);
}

/// Fault injected by a test, which must make an equivalence proof fail.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sabotage {
    /// The block of configurations written for the first frame size is never written again.
    ConfigsWrittenOnce,
    /// Bindings kept by the `source` target alone, whatever the `back` one.
    BindingsKeptBySourceOnly,
}

pub struct PostFx {
    options: Options,
    /// By [`Program`]; `None` for a program the chains do not use.
    pipelines: [Option<wgpu::RenderPipeline>; 7],
    shared: SharedBindings,
    format: wgpu::TextureFormat,
    plan: Option<Plan>,
    targets: Option<Targets>,
    targets_last_used: Option<Instant>,
    #[cfg(test)]
    pub(crate) sabotage: Option<Sabotage>,
}

impl PostFx {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> PostFx {
        Self::with_options(device, format, Options::from_switches())
    }

    pub fn with_options(device: &wgpu::Device, format: wgpu::TextureFormat, options: Options) -> PostFx {
        Self::with_shader_source(device, format, options, include_str!("post_fx.wgsl"))
    }

    /// `shader_source`: the passes' shader (the tests edit it for their sabotages).
    fn with_shader_source(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        options: Options,
        shader_source: &str,
    ) -> PostFx {
        let tex_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let sampler_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("image effects"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(PASS_CONFIG_BYTES),
                    },
                    count: None,
                },
                tex_entry(1),
                tex_entry(2),
                sampler_entry(3),
                sampler_entry(4),
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("image effects"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("image effects"),
            bind_group_layouts: &[Some(&layout)],
            ..Default::default()
        });
        let pipeline = |program: Program| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(program.entry_point()),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(program.entry_point()),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipelines = Program::ALL.map(|program| program.used(options).then(|| pipeline(program)));
        let linear = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("image effects (bilinear)"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let nearest =
            device.create_sampler(&wgpu::SamplerDescriptor { label: Some("image effects"), ..Default::default() });
        let config_bytes = SLOT_BYTES * PASSES_PER_FRAME;
        let mut configs = FrameBuffer::new("image effects", wgpu::BufferUsages::UNIFORM, config_bytes);
        // Made now: the bindings read it before the first write.
        configs.reserve(device, config_bytes);
        PostFx {
            options,
            pipelines,
            shared: SharedBindings { layout, linear, nearest, configs },
            format,
            plan: None,
            targets: None,
            targets_last_used: None,
            #[cfg(test)]
            sabotage: None,
        }
    }

    /// Frees the work targets once they have been idle long enough at `now`; the next frame with
    /// an effect makes them again, as the first one did.
    pub fn release_idle_targets(&mut self, now: Instant) {
        if self.options.release_idle_targets && idle_long_enough(self.targets_last_used, now) {
            self.targets = None;
            self.targets_last_used = None;
        }
    }

    /// Bytes of the work textures now allocated.
    #[cfg(test)]
    fn work_target_bytes(&self) -> u64 {
        self.targets.as_ref().map_or(0, |t| t.textures.len() as u64 * crate::blur::texture_bytes(t.size))
    }

    /// Passes of every effect at `size`; their configurations cross once, here, unless each pass
    /// writes its own on every frame.
    fn prepare_plan(&mut self, up: &mut Uploader, queue: &wgpu::Queue, size: (u32, u32)) {
        if self.plan.as_ref().is_some_and(|p| p.size == size) {
            return;
        }
        let plan = Plan::new(size, self.options);
        let write_block = self.options.skip_unchanged_config;
        #[cfg(test)]
        let write_block = write_block && !(self.sabotage == Some(Sabotage::ConfigsWrittenOnce) && self.plan.is_some());
        if write_block {
            self.shared.configs.write(up, queue, &plan.config_block());
        }
        self.plan = Some(plan);
    }

    /// Applies `effects` in order on `frame` (usable as a copy source). `blur`: menu blur
    /// (radius of the option), for the `blur` effect.
    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        up: &mut Uploader,
        enc: &mut wgpu::CommandEncoder,
        frame: &wgpu::Texture,
        frame_view: &wgpu::TextureView,
        effects: &[Effect],
        blur: &mut crate::blur::Blur,
        blur_radius: i32,
        timer: Option<&crate::gpu_time::GpuTimer>,
    ) {
        if effects.is_empty() {
            return;
        }
        let size = (frame.width(), frame.height());
        self.prepare_plan(up, queue, size);
        let Some(plan) = &self.plan else { return };
        let mut passes_left = PASSES_PER_FRAME;
        // Slot of the next pass when each pass writes its configuration on every frame.
        let mut next_frame_slot = 0;
        for &effect in effects {
            let Some(chain_index) = effect.chain_index() else {
                blur.run(device, queue, up, enc, frame, frame_view, blur_radius, timer);
                continue;
            };
            let chain = &plan.chains[chain_index];
            if chain.counted_passes > passes_left {
                return;
            }
            passes_left -= chain.counted_passes;
            if self.targets.as_ref().is_none_or(|t| t.size != size) {
                self.targets = Some(Targets::new(size));
            }
            let Some(targets) = &mut self.targets else { return };
            let wanted_targets = if self.options.target_aliasing { chain.work_targets } else { MAX_WORK_TARGETS };
            targets.grow(device, self.format, wanted_targets);
            self.targets_last_used = Some(Instant::now());
            enc.copy_texture_to_texture(
                frame.as_image_copy(),
                targets.textures[FRAME_COPY].as_image_copy(),
                wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            );
            for (index, pass) in chain.passes.iter().enumerate() {
                let slot = if self.options.skip_unchanged_config {
                    chain.first_slot + index as u64
                } else {
                    let slot = next_frame_slot;
                    let config = bytemuck::bytes_of(&pass.config);
                    queue.write_buffer(self.shared.config_buffer(), slot * SLOT_BYTES, config);
                    next_frame_slot += 1;
                    slot
                };
                let kept_under = self.options.bind_group_cache.then_some((pass.source, pass.back));
                #[cfg(test)]
                let kept_under = match self.sabotage {
                    Some(Sabotage::BindingsKeptBySourceOnly) => kept_under.map(|(source, _)| (source, source)),
                    _ => kept_under,
                };
                let bindings = targets.bindings(device, &self.shared, pass, kept_under);
                let Some(pipeline) = &self.pipelines[pass.program as usize] else { continue };
                let output = match pass.output {
                    Output::Target(target) => &targets.views[target],
                    Output::Frame => frame_view,
                };
                draw_pass(enc, pipeline, &bindings, slot * SLOT_BYTES, output, timer);
            }
        }
    }
}

#[cfg(test)]
#[path = "post_fx_test.rs"]
mod tests;
