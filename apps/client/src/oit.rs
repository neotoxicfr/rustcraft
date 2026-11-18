//! Improved transparency (option "improvedTransparency", Fabulous preset): order-independent
//! transparency (docs/specs/oit.md).
//!
//! After the solids pass, each translucent family is drawn three times:
//! 1. per-pixel depth bounds (RG32F + R32F targets, or one RGBA32F target, MAX blend);
//! 2. wavelet transmittance (two additive RGBA16F targets, 8 coefficients);
//! 3. accumulation of the premultiplied color attenuated by what is in front (additive RGBA16F).
//!
//! Between 1 and 2, the bounds are "culled": a nearly opaque translucent fragment becomes
//! the far bound and enters the main depth. Finally the composition blends
//! the accumulation into the image (premultiplied alpha) and writes the nearest translucent
//! depth into it. The targets exist only while the option is active; with the option off, nothing here is used.
//!
//! Switches ([`Options`], read once): `RUSTCRAFT_OPT_OIT_COMPACT_BOUNDS` (bounds in 12 bytes per
//! pixel, culled copy in 8, instead of 16 and 16), `RUSTCRAFT_OPT_OIT_UNROLLED_WAVELET` (wavelet
//! without a local array indexed per fragment), `RUSTCRAFT_OPT_OIT_PROJECTION_ON_CHANGE` (projection
//! written when it changes, through the transit ring under `RUSTCRAFT_OPT_FRAME_WRITES_RING`).

use crate::render::upload::Uploader;
use crate::render::upload::per_frame::FrameBuffer;

/// Common functions appended to the shader of each family, before the bounds layout and the
/// wavelet form chosen by [`Options`].
const COMMON_WGSL: &str = include_str!("oit.wgsl");

/// Switches of improved transparency, read once when its passes are first prepared; the tests
/// build every combination in one run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// Bounds as (depth, −depth) in RG32F plus the opaque depth in R32F, the distances computed
    /// when read (`RUSTCRAFT_OPT_OIT_COMPACT_BOUNDS`); off: (−distance, distance, depth, opaque
    /// depth) in one RGBA32F target.
    pub compact_bounds: bool,
    /// Wavelet levels side by side in vectors (`RUSTCRAFT_OPT_OIT_UNROLLED_WAVELET`); off: a loop
    /// over the levels and a local array of the coefficients.
    pub unrolled_wavelet: bool,
    /// Projection coefficients written only when they change
    /// (`RUSTCRAFT_OPT_OIT_PROJECTION_ON_CHANGE`); off: on every frame.
    pub projection_on_change: bool,
}

impl Options {
    /// Every lead on: the default of the switches (the tests name the forms).
    #[cfg(test)]
    pub const NEW: Options = Options { compact_bounds: true, unrolled_wavelet: true, projection_on_change: true };
    /// The code before the leads.
    #[cfg(test)]
    pub const OLD: Options = Options { compact_bounds: false, unrolled_wavelet: false, projection_on_change: false };

    pub fn from_switches() -> Self {
        Options {
            compact_bounds: rc_data::opt_switch!("RUSTCRAFT_OPT_OIT_COMPACT_BOUNDS"),
            unrolled_wavelet: rc_data::opt_switch!("RUSTCRAFT_OPT_OIT_UNROLLED_WAVELET"),
            projection_on_change: rc_data::opt_switch!("RUSTCRAFT_OPT_OIT_PROJECTION_ON_CHANGE"),
        }
    }

    /// Common functions followed by the chosen bounds layout and wavelet form.
    pub fn common_source(self) -> String {
        let bounds = if self.compact_bounds {
            include_str!("oit_bounds_compact.wgsl")
        } else {
            include_str!("oit_bounds_rgba.wgsl")
        };
        let wavelet = if self.unrolled_wavelet {
            include_str!("oit_wavelet_unrolled.wgsl")
        } else {
            include_str!("oit_wavelet_loops.wgsl")
        };
        format!("{COMMON_WGSL}\n{bounds}\n{wavelet}")
    }

    /// Format of the bounds target and of its culled copy.
    fn bounds_format(self) -> wgpu::TextureFormat {
        if self.compact_bounds { DEPTHS_FORMAT } else { RGBA_BOUNDS_FORMAT }
    }

    /// Clear value of the bounds target.
    fn bounds_clear(self) -> wgpu::Color {
        if self.compact_bounds { DEPTHS_CLEAR } else { RGBA_BOUNDS_CLEAR }
    }

    /// Color targets of the bounds stage (the compact layout adds the opaque depth's).
    fn bounds_targets(
        self,
        blend: Option<wgpu::BlendState>,
        mask: wgpu::ColorWrites,
    ) -> Vec<Option<wgpu::ColorTargetState>> {
        let mut targets = vec![target(self.bounds_format(), blend, mask)];
        if self.compact_bounds {
            targets.push(target(OPAQUE_DEPTH_FORMAT, blend, mask));
        }
        targets
    }
}

fn target(
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
    write_mask: wgpu::ColorWrites,
) -> Option<wgpu::ColorTargetState> {
    Some(wgpu::ColorTargetState { format, blend, write_mask })
}

/// Stage of a translucent family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Bounds,
    Trans,
    Accum,
}

impl Stage {
    pub const ALL: [Stage; 3] = [Stage::Bounds, Stage::Trans, Stage::Accum];

    fn index(self) -> usize {
        self as usize
    }

    /// Names of the stage's passes (terrain, elements, clouds): labels and GPU time per pass.
    pub fn pass_names(self) -> [&'static str; 3] {
        match self {
            Stage::Bounds => {
                ["transparency: terrain, bounds", "transparency: elements, bounds", "transparency: clouds, bounds"]
            }
            Stage::Trans => [
                "transparency: terrain, coefficients",
                "transparency: elements, coefficients",
                "transparency: clouds, coefficients",
            ],
            Stage::Accum => [
                "transparency: terrain, accumulation",
                "transparency: elements, accumulation",
                "transparency: clouds, accumulation",
            ],
        }
    }
}

/// GPU timestamps of a pass (`gpu_time`), `None` without measurement.
pub type Stamp<'a> = Option<wgpu::RenderPassTimestampWrites<'a>>;

/// RGBA32F bounds (−distance, distance, depth, opaque depth), cleared to (−FLT_MAX, 0, 0, 0).
const RGBA_BOUNDS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;
const RGBA_BOUNDS_CLEAR: wgpu::Color = wgpu::Color { r: -f32::MAX as f64, g: 0.0, b: 0.0, a: 0.0 };
/// Compact bounds (nearest depth, minus the farthest depth), cleared to (0, −FLT_MAX): no fragment
/// reads as nearer than the far plane and farther than the eye, like the RGBA32F clear value.
const DEPTHS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg32Float;
const DEPTHS_CLEAR: wgpu::Color = wgpu::Color { r: 0.0, g: -f32::MAX as f64, b: 0.0, a: 0.0 };
/// Compact bounds: depth of the nearest nearly opaque fragment, cleared to 0.
const OPAQUE_DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Float;
pub const COEFF_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Projection coefficients: (P[2][2], P[3][2], 0, 0) in f32.
const PROJECTION_BYTES: u64 = 16;

/// The three pipelines of a family.
pub struct Set([wgpu::RenderPipeline; 3]);

impl Set {
    pub fn get(&self, s: Stage) -> &wgpu::RenderPipeline {
        &self.0[s.index()]
    }
}

/// Current stage and its binding (group 3), passed to the families.
#[derive(Clone, Copy)]
pub struct Frame<'a> {
    pub stage: Stage,
    pub bind: &'a wgpu::BindGroup,
}

impl Frame<'_> {
    /// Pipeline of the stage, then binding of group 3 (kept from one pipeline to the next).
    pub fn set(&self, pass: &mut wgpu::RenderPass<'_>, set: &Set) {
        pass.set_pipeline(set.get(self.stage));
        pass.set_bind_group(3, self.bind, &[]);
    }
}

/// Description of a family: its groups 0 to 2, its shader (with [`WGSL`] and its entries
/// `<fs>_bounds`, `<fs>_trans`, `<fs>_accum`), its vertices and its face culling.
pub struct Spec<'a> {
    pub label: &'a str,
    pub groups: &'a [Option<&'a wgpu::BindGroupLayout>],
    pub module: &'a wgpu::ShaderModule,
    pub vs: &'a str,
    pub buffers: &'a [Option<wgpu::VertexBufferLayout<'a>>],
    pub fs: &'a str,
    pub cull: bool,
    pub topology: wgpu::PrimitiveTopology,
    /// Bounds stage with depth write (detailed clouds, in their own depth).
    pub bounds_write: bool,
}

impl<'a> Spec<'a> {
    pub fn new(
        label: &'a str,
        groups: &'a [Option<&'a wgpu::BindGroupLayout>],
        module: &'a wgpu::ShaderModule,
        vs: &'a str,
        buffers: &'a [Option<wgpu::VertexBufferLayout<'a>>],
        fs: &'a str,
    ) -> Self {
        Spec {
            label,
            groups,
            module,
            vs,
            buffers,
            fs,
            cull: false,
            topology: wgpu::PrimitiveTopology::TriangleList,
            bounds_write: false,
        }
    }

    pub fn cull(mut self, cull: bool) -> Self {
        self.cull = cull;
        self
    }

    pub fn bounds_write(mut self) -> Self {
        self.bounds_write = true;
        self
    }
}

/// Blends of the stages: MAX (bounds), addition (transmittance, accumulation).
fn max_blend() -> wgpu::BlendState {
    let c = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Max,
    };
    wgpu::BlendState { color: c, alpha: c }
}

fn add_blend() -> wgpu::BlendState {
    let c = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    };
    wgpu::BlendState { color: c, alpha: c }
}

/// Layouts of group 3 per stage and of the fullscreen passes, for the shaders of `options`.
pub struct Layouts {
    options: Options,
    stage: [wgpu::BindGroupLayout; 3],
    /// Culling: the bounds (and the opaque depths of the compact layout).
    cull: wgpu::BindGroupLayout,
    composite: wgpu::BindGroupLayout,
    depth: wgpu::BindGroupLayout,
}

fn texture_entry(binding: u32, depth: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: if depth {
                wgpu::TextureSampleType::Depth
            } else {
                wgpu::TextureSampleType::Float { filterable: false }
            },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

impl Layouts {
    fn new(device: &wgpu::Device, options: Options) -> Self {
        let params = wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = |label, entries: &[wgpu::BindGroupLayoutEntry]| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some(label), entries })
        };
        let mut cull = vec![params, texture_entry(1, false)];
        if options.compact_bounds {
            cull.push(texture_entry(6, false));
        }
        Layouts {
            options,
            stage: [
                layout("transparency: bounds", &[params]),
                layout("transparency: transmittance", &[params, texture_entry(1, false)]),
                layout(
                    "transparency: accumulation",
                    &[params, texture_entry(1, false), texture_entry(2, false), texture_entry(3, false)],
                ),
            ],
            cull: layout("transparency: culling", &cull),
            composite: layout(
                "transparency: composition",
                &[texture_entry(1, false), texture_entry(2, false), texture_entry(3, false), texture_entry(4, false)],
            ),
            depth: layout("transparency: depth copy", &[texture_entry(5, true)]),
        }
    }

    /// Shader of a family for improved transparency: its source, the common functions of these
    /// layouts' options, then its own entries (the classic shader stays unchanged).
    pub fn module(&self, device: &wgpu::Device, label: &str, family: &str, entries: &str) -> wgpu::ShaderModule {
        let source = format!("{family}\n{}\n{entries}", self.options.common_source());
        device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(label),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        })
    }

    /// The three pipelines of a family.
    pub fn set(&self, device: &wgpu::Device, spec: &Spec<'_>) -> Set {
        let mut groups: [Option<&wgpu::BindGroupLayout>; 4] = [None; 4];
        for (g, l) in groups.iter_mut().zip(spec.groups) {
            *g = *l;
        }
        let make = |stage: Stage| {
            let mut groups = groups;
            groups[3] = Some(&self.stage[stage.index()]);
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(spec.label),
                bind_group_layouts: &groups,
                immediate_size: 0,
            });
            let all = wgpu::ColorWrites::ALL;
            let coefficients = || target(COEFF_FORMAT, Some(add_blend()), all);
            let (suffix, targets) = match stage {
                Stage::Bounds => ("bounds", self.options.bounds_targets(Some(max_blend()), all)),
                Stage::Trans => ("trans", vec![coefficients(), coefficients()]),
                Stage::Accum => ("accum", vec![coefficients()]),
            };
            let fs = format!("{}_{suffix}", spec.fs);
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(spec.label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: spec.module,
                    entry_point: Some(spec.vs),
                    compilation_options: Default::default(),
                    buffers: spec.buffers,
                },
                fragment: Some(wgpu::FragmentState {
                    module: spec.module,
                    entry_point: Some(&fs),
                    compilation_options: Default::default(),
                    targets: &targets,
                }),
                primitive: wgpu::PrimitiveState {
                    topology: spec.topology,
                    cull_mode: spec.cull.then_some(wgpu::Face::Back),
                    ..Default::default()
                },
                // All the stages: "≥" test with no write or offset (except cloud bounds).
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(stage == Stage::Bounds && spec.bounds_write),
                    depth_compare: Some(crate::render::DEPTH_NEARER),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        Set([make(Stage::Bounds), make(Stage::Trans), make(Stage::Accum)])
    }
}

/// Targets at the frame size and their bindings.
struct Targets {
    size: (u32, u32),
    bounds: wgpu::TextureView,
    /// Compact layout: the nearest nearly opaque depths, beside `bounds`.
    bounds_opaque: Option<wgpu::TextureView>,
    culled: wgpu::TextureView,
    coeffs: [wgpu::TextureView; 2],
    accum: wgpu::TextureView,
    /// Cloud depth and terrain depth with the boats' water mask.
    cloud_depth: wgpu::TextureView,
    terrain_depth: wgpu::TextureView,
    /// Group 3 of the stages (transmittance and accumulation: culled bounds).
    stage: [wgpu::BindGroup; 3],
    composite: wgpu::BindGroup,
    /// Unculled bounds (copy and culling).
    cull: wgpu::BindGroup,
    /// Main depth (copies).
    main_depth: wgpu::BindGroup,
}

/// Fullscreen passes and targets of improved transparency.
pub struct Oit {
    pub layouts: Layouts,
    /// Projection coefficients (16 bytes, created with the passes).
    params: FrameBuffer,
    /// Bits of the coefficients last written, `None` before the first write.
    written_projection: Option<[u32; 4]>,
    composite: wgpu::RenderPipeline,
    copy_bounds: wgpu::RenderPipeline,
    cull: wgpu::RenderPipeline,
    copy_depth: wgpu::RenderPipeline,
    copy_depth_in_bounds: wgpu::RenderPipeline,
    targets: Option<Targets>,
}

impl Oit {
    pub fn new(device: &wgpu::Device, surface: wgpu::TextureFormat) -> Self {
        Self::with_options(device, surface, Options::from_switches())
    }

    pub fn with_options(device: &wgpu::Device, surface: wgpu::TextureFormat, options: Options) -> Self {
        let layouts = Layouts::new(device, options);
        let mut params = FrameBuffer::new("transparency: projection", wgpu::BufferUsages::UNIFORM, PROJECTION_BYTES);
        // Made now: the bindings of `ensure` read it before the first write.
        params.reserve(device, PROJECTION_BYTES);
        let module = layouts.module(device, "transparency: passes", "", include_str!("oit_passes.wgsl"));
        let pipeline = |label: &str,
                        group: &wgpu::BindGroupLayout,
                        fs: &str,
                        targets: &[Option<wgpu::ColorTargetState>],
                        compare: wgpu::CompareFunction,
                        write: bool| {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[None, None, None, Some(group)],
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_screen"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets,
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(write),
                    depth_compare: Some(compare),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let always = wgpu::CompareFunction::Always;
        // Composition: premultiplied alpha (One, 1 − αs) on the color and the alpha.
        let premultiplied = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let composite = pipeline(
            "transparency: composition",
            &layouts.composite,
            "fs_composite",
            &[target(
                surface,
                Some(wgpu::BlendState { color: premultiplied, alpha: premultiplied }),
                wgpu::ColorWrites::ALL,
            )],
            always,
            true,
        );
        let culled_target = [target(options.bounds_format(), None, wgpu::ColorWrites::ALL)];
        let copy_bounds =
            pipeline("transparency: bounds copy", &layouts.cull, "fs_copy_bounds", &culled_target, always, false);
        let cull = pipeline(
            "transparency: bounds culling",
            &layouts.cull,
            "fs_cull_bounds",
            &culled_target,
            crate::render::DEPTH_NEARER,
            true,
        );
        let copy_depth = pipeline("transparency: depth copy", &layouts.depth, "fs_copy_depth", &[], always, true);
        let copy_depth_in_bounds = pipeline(
            "transparency: depth copy (bounds)",
            &layouts.depth,
            "fs_copy_depth",
            &options.bounds_targets(None, wgpu::ColorWrites::empty()),
            always,
            true,
        );
        Oit {
            layouts,
            params,
            written_projection: None,
            composite,
            copy_bounds,
            cull,
            copy_depth,
            copy_depth_in_bounds,
            targets: None,
        }
    }

    /// Targets at the frame size (redone on resize: `main_depth` changes).
    pub fn ensure(&mut self, device: &wgpu::Device, size: (u32, u32), main_depth: &wgpu::TextureView) {
        if self.targets.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        let tex = |label: &str, format: wgpu::TextureFormat| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width: size.0.max(1), height: size.1.max(1), depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let options = self.layouts.options;
        let bounds = tex("transparency: bounds", options.bounds_format());
        let bounds_opaque =
            options.compact_bounds.then(|| tex("transparency: opaque depth bounds", OPAQUE_DEPTH_FORMAT));
        let culled = tex("transparency: culled bounds", options.bounds_format());
        let coeffs = [
            tex("transparency: coefficients 0-3", COEFF_FORMAT),
            tex("transparency: coefficients 4-7", COEFF_FORMAT),
        ];
        let accum = tex("transparency: accumulation", COEFF_FORMAT);
        let cloud_depth = tex("transparency: cloud depth", wgpu::TextureFormat::Depth32Float);
        let terrain_depth = tex("transparency: terrain depth", wgpu::TextureFormat::Depth32Float);
        let view = wgpu::BindingResource::TextureView;
        let bind = |label, layout: &wgpu::BindGroupLayout, entries: &[wgpu::BindGroupEntry<'_>]| {
            device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some(label), layout, entries })
        };
        let params_buffer = self.params.buffer().expect("projection buffer made with the passes");
        let params = wgpu::BindGroupEntry { binding: 0, resource: params_buffer.as_entire_binding() };
        let l = &self.layouts;
        let stage = [
            bind("transparency: bounds", &l.stage[0], std::slice::from_ref(&params)),
            bind(
                "transparency: transmittance",
                &l.stage[1],
                &[params.clone(), wgpu::BindGroupEntry { binding: 1, resource: view(&culled) }],
            ),
            bind(
                "transparency: accumulation",
                &l.stage[2],
                &[
                    params.clone(),
                    wgpu::BindGroupEntry { binding: 1, resource: view(&culled) },
                    wgpu::BindGroupEntry { binding: 2, resource: view(&coeffs[0]) },
                    wgpu::BindGroupEntry { binding: 3, resource: view(&coeffs[1]) },
                ],
            ),
        ];
        let composite = bind(
            "transparency: composition",
            &l.composite,
            &[
                wgpu::BindGroupEntry { binding: 1, resource: view(&bounds) },
                wgpu::BindGroupEntry { binding: 2, resource: view(&coeffs[0]) },
                wgpu::BindGroupEntry { binding: 3, resource: view(&coeffs[1]) },
                wgpu::BindGroupEntry { binding: 4, resource: view(&accum) },
            ],
        );
        let mut cull_entries = vec![params.clone(), wgpu::BindGroupEntry { binding: 1, resource: view(&bounds) }];
        if let Some(opaque) = &bounds_opaque {
            cull_entries.push(wgpu::BindGroupEntry { binding: 6, resource: view(opaque) });
        }
        let cull = bind("transparency: culling", &l.cull, &cull_entries);
        let main_depth = bind(
            "transparency: depth",
            &l.depth,
            &[wgpu::BindGroupEntry { binding: 5, resource: view(main_depth) }],
        );
        self.targets = Some(Targets {
            size,
            bounds,
            bounds_opaque,
            culled,
            coeffs,
            accum,
            cloud_depth,
            terrain_depth,
            stage,
            composite,
            cull,
            main_depth,
        });
    }

    /// Frees the targets (option off or frame resized).
    pub fn release(&mut self) {
        self.targets = None;
    }

    /// Depth coefficients of the frame's world projection, written through the uploader (its
    /// transit ring under `RUSTCRAFT_OPT_FRAME_WRITES_RING`, instead of a staging buffer made and
    /// dropped by the queue on every frame).
    pub fn set_projection(&mut self, up: &mut Uploader, queue: &wgpu::Queue, proj: glam::Mat4) {
        let coefficients = [proj.z_axis.z, proj.w_axis.z, 0.0, 0.0];
        let bits = coefficients.map(f32::to_bits);
        // The projection only moves with the field of view and the frame size: an unchanged one
        // is not sent again (compared bit for bit).
        if self.layouts.options.projection_on_change && self.written_projection == Some(bits) {
            return;
        }
        self.params.write(up, queue, bytemuck::cast_slice(&coefficients));
        self.written_projection = Some(bits);
    }

    fn t(&self) -> &Targets {
        self.targets.as_ref().expect("transparency targets prepared by `ensure`")
    }

    pub fn terrain_depth(&self) -> &wgpu::TextureView {
        &self.t().terrain_depth
    }

    /// Stage and binding to pass to the families.
    pub fn frame(&self, stage: Stage) -> Frame<'_> {
        Frame { stage, bind: &self.t().stage[stage.index()] }
    }

    /// Pass of a stage toward its targets, with the depth `depth` (read, never written except
    /// by the cloud bounds); `clear`: first draw of the stage in the frame.
    pub fn begin<'e>(
        &self,
        enc: &'e mut wgpu::CommandEncoder,
        stage: Stage,
        depth: &wgpu::TextureView,
        clear: bool,
        label: &str,
        timestamp_writes: Stamp<'_>,
    ) -> wgpu::RenderPass<'e> {
        let t = self.t();
        let attach = |view, clear_value: wgpu::Color| {
            Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: if clear { wgpu::LoadOp::Clear(clear_value) } else { wgpu::LoadOp::Load },
                    store: wgpu::StoreOp::Store,
                },
            })
        };
        let zero = wgpu::Color::TRANSPARENT;
        let bounds_clear = self.layouts.options.bounds_clear();
        let (colors, n) = match stage {
            Stage::Bounds => match &t.bounds_opaque {
                Some(opaque) => ([attach(&t.bounds, bounds_clear), attach(opaque, zero)], 2),
                None => ([attach(&t.bounds, bounds_clear), None], 1),
            },
            Stage::Trans => ([attach(&t.coeffs[0], zero), attach(&t.coeffs[1], zero)], 2),
            Stage::Accum => ([attach(&t.accum, zero), None], 1),
        };
        enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
            color_attachments: &colors[..n],
            depth_stencil_attachment: Some(depth_load(depth)),
            timestamp_writes,
            ..Default::default()
        })
    }

    /// Cloud pass: own depth; at the bounds stage, toward the culled bounds,
    /// after the copy of the main depth.
    pub fn begin_clouds<'e>(
        &self,
        enc: &'e mut wgpu::CommandEncoder,
        stage: Stage,
        timestamp_writes: Stamp<'_>,
    ) -> wgpu::RenderPass<'e> {
        let t = self.t();
        let label = stage.pass_names()[2];
        if stage != Stage::Bounds {
            return self.begin(enc, stage, &t.cloud_depth, false, label, timestamp_writes);
        }
        let culled = Some(wgpu::RenderPassColorAttachment {
            view: &t.culled,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
        });
        // Compact layout: the bounds pipelines also write the opaque depths, which nothing reads
        // after the culling (the clouds are culled by their own depth): cleared, never stored.
        let opaque = t.bounds_opaque.as_ref().map(|view| wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Discard,
            },
        });
        let colors = [culled, opaque];
        let n = if colors[1].is_some() { 2 } else { 1 };
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
            color_attachments: &colors[..n],
            depth_stencil_attachment: Some(depth_load(&t.cloud_depth)),
            timestamp_writes,
            ..Default::default()
        });
        pass.set_pipeline(&self.copy_depth_in_bounds);
        pass.set_bind_group(3, &t.main_depth, &[]);
        pass.draw(0..3, 0..1);
        pass
    }

    /// Terrain depth = copy of the main depth; the water mask is drawn into it
    /// afterwards in the rendered pass.
    pub fn begin_water_mask<'e>(
        &self,
        enc: &'e mut wgpu::CommandEncoder,
        timestamp_writes: Stamp<'_>,
    ) -> wgpu::RenderPass<'e> {
        let t = self.t();
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("transparency: water mask"),
            color_attachments: &[],
            depth_stencil_attachment: Some(depth_load(&t.terrain_depth)),
            timestamp_writes,
            ..Default::default()
        });
        pass.set_pipeline(&self.copy_depth);
        pass.set_bind_group(3, &t.main_depth, &[]);
        pass.draw(0..3, 0..1);
        pass
    }

    /// Bounds culling: copy, then opaque translucent fragments written into the main
    /// depth.
    pub fn cull(&self, enc: &mut wgpu::CommandEncoder, main_depth: &wgpu::TextureView, timestamp_writes: Stamp<'_>) {
        let t = self.t();
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("transparency: bounds culling"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &t.culled,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: Some(depth_load(main_depth)),
            timestamp_writes,
            ..Default::default()
        });
        pass.set_bind_group(3, &t.cull, &[]);
        pass.set_pipeline(&self.copy_bounds);
        pass.draw(0..3, 0..1);
        pass.set_pipeline(&self.cull);
        pass.draw(0..3, 0..1);
    }

    /// Composition onto the image and its depth.
    pub fn composite(
        &self,
        enc: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        main_depth: &wgpu::TextureView,
        timestamp_writes: Stamp<'_>,
    ) {
        let t = self.t();
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("transparency: composition"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: Some(depth_load(main_depth)),
            timestamp_writes,
            ..Default::default()
        });
        pass.set_pipeline(&self.composite);
        pass.set_bind_group(3, &t.composite, &[]);
        pass.draw(0..3, 0..1);
    }
}

fn depth_load(view: &wgpu::TextureView) -> wgpu::RenderPassDepthStencilAttachment<'_> {
    wgpu::RenderPassDepthStencilAttachment {
        view,
        depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
        stencil_ops: None,
    }
}

#[cfg(test)]
#[path = "oit_gpu_test.rs"]
mod gpu_tests;

#[cfg(test)]
mod tests {
    use super::Options;

    /// Shaders of the families (source, own entries, prefixes of the three stages).
    const FAMILIES: &[(&str, &str, &str, &[&str])] = &[
        ("terrain", include_str!("terrain.wgsl"), include_str!("terrain_oit.wgsl"), &["fs_translucent"]),
        ("clouds", include_str!("sky.wgsl"), include_str!("sky_oit.wgsl"), &["fs_cloud"]),
        ("particles", include_str!("particles.wgsl"), include_str!("particles_oit.wgsl"), &["fs"]),
        ("weather", include_str!("weather.wgsl"), include_str!("weather_oit.wgsl"), &["fs"]),
        ("border", include_str!("border.wgsl"), include_str!("border_oit.wgsl"), &["fs"]),
        (
            "entities",
            include_str!("entity.wgsl"),
            include_str!("entity_oit.wgsl"),
            &["fs_translucent", "fs_eyes", "fs_bolt", "fs_swirl", "fs_rays", "fs_shadow"],
        ),
        ("cracks", include_str!("crack.wgsl"), include_str!("crack_oit.wgsl"), &["fs"]),
        ("name tags", include_str!("nametag.wgsl"), include_str!("nametag_oit.wgsl"), &["fs"]),
        ("text displays", include_str!("text_display.wgsl"), include_str!("text_display_oit.wgsl"), &["fs"]),
        ("halos", include_str!("block_fx.wgsl"), include_str!("block_fx_oit.wgsl"), &["fs_halo"]),
        ("outline", include_str!("outline.wgsl"), include_str!("outline_oit.wgsl"), &["fs"]),
        ("debug", include_str!("debug_lines.wgsl"), include_str!("debug_lines_oit.wgsl"), &["fs", "fs_face"]),
        ("debug text", include_str!("debug_text.wgsl"), include_str!("debug_text_oit.wgsl"), &["fs"]),
        ("passes", "", include_str!("oit_passes.wgsl"), &[]),
    ];

    fn validate(name: &str, source: &str) -> naga::Module {
        let module =
            naga::front::wgsl::parse_str(source).unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(source)));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
            .validate(&module)
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        module
    }

    /// The four shader forms: bounds layout × wavelet form.
    fn shader_forms() -> Vec<Options> {
        let mut forms = Vec::new();
        for compact_bounds in [false, true] {
            for unrolled_wavelet in [false, true] {
                forms.push(Options { compact_bounds, unrolled_wavelet, ..Options::NEW });
            }
        }
        forms
    }

    /// Each family shader, completed with the common functions (every bounds layout and wavelet
    /// form) and its entries, is valid WGSL that exposes the three stages of each of its render
    /// types.
    #[test]
    fn family_shaders_are_valid_and_complete() {
        for options in shader_forms() {
            for (name, family, entries, prefixes) in FAMILIES {
                // Terrain and clouds: source reduced to the default variant (`terrain_shader` and
                // `sky`, all the variants validated there). Small layers: likewise (`small_layers`).
                let family = match *name {
                    "terrain" => crate::terrain_shader::terrain(crate::terrain_shader::ShaderOpts::NEW),
                    "clouds" => crate::sky::SkyOpts::from_env().shader_source(),
                    "name tags" | "text displays" | "outline" | "debug" | "debug text" => {
                        crate::small_layers::SmallLayerOptions::NEW.shader_source(family)
                    }
                    _ => family.to_string(),
                };
                let name = format!("{name} ({options:?})");
                let module = validate(&name, &format!("{family}\n{}\n{entries}", options.common_source()));
                for p in *prefixes {
                    for stage in ["bounds", "trans", "accum"] {
                        let e = format!("{p}_{stage}");
                        assert!(module.entry_points.iter().any(|ep| ep.name == e), "{name}: entry {e} missing");
                    }
                }
            }
        }
    }

    /// Local arrays and accesses by a computed index into an array held by value, in the wavelet
    /// functions run by every translucent fragment.
    fn wavelet_array_indexing(options: Options) -> usize {
        let source = format!("{}\n{}", options.common_source(), include_str!("oit_passes.wgsl"));
        let (module, info) = crate::terrain_shader::tests::validate(&source);
        let mut found = 0;
        for name in ["oit_add_transmittance", "oit_sample_absorbance", "oit_sample_transmittance"] {
            let (handle, function) =
                module.functions.iter().find(|(_, f)| f.name.as_deref() == Some(name)).expect("wavelet function");
            found += crate::terrain_shader::tests::value_array_indexing(&module, function, &info[handle]);
        }
        found
    }

    /// The unrolled wavelet keeps no local array (which GPUs place in local memory when it is
    /// indexed by the fragment's bin); the loop form has some, which shows the check sees them.
    #[test]
    fn unrolled_wavelet_has_no_local_array() {
        let unrolled = wavelet_array_indexing(Options::NEW);
        let loops = wavelet_array_indexing(Options { unrolled_wavelet: false, ..Options::NEW });
        assert_eq!(unrolled, 0, "unrolled wavelet: {unrolled} local arrays or indexed accesses");
        assert!(loops >= 2, "loop form: {loops} local arrays found, the check sees nothing");
    }
}
