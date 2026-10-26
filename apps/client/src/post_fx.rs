//! Full-screen image effects (`docs/specs/miscellaneous.md` § 5.3 and § 6): effects imposed
//! by the server (`post_effects`) and effect of the entity watched in spectator (creeper, spider,
//! enderman). Applied after the world and the hand, before the interface. Hard-wired: `invert`,
//! `creeper`, `spider` and `blur` (menu blur); any other id is ignored.

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
}

/// Pass configuration (96 bytes, placed every 256 bytes).
#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct Cfg {
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

const SLOT: u64 = 256;
const SLOTS: u64 = 32;

/// Work targets: copy of the image (M), temp (T), large blur (L), small blur (S), swap (W).
const M: usize = 0;
const T: usize = 1;
const L: usize = 2;
const S: usize = 3;
const W: usize = 4;

struct Targets {
    size: (u32, u32),
    tex: Vec<wgpu::Texture>,
    views: Vec<wgpu::TextureView>,
}

#[derive(Clone, Copy)]
enum Prog {
    Blur,
    Invert,
    Color,
    Bits,
    Spider,
    Blit,
}

/// Output of a pass: a target or the image.
#[derive(Clone, Copy)]
enum Dst {
    Target(usize),
    Frame,
}

pub struct PostFx {
    layout: wgpu::BindGroupLayout,
    pipelines: [wgpu::RenderPipeline; 6],
    linear: wgpu::Sampler,
    nearest: wgpu::Sampler,
    uniforms: wgpu::Buffer,
    format: wgpu::TextureFormat,
    targets: Option<Targets>,
}

impl PostFx {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> PostFx {
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
                        min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<Cfg>() as u64),
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
            source: wgpu::ShaderSource::Wgsl(include_str!("post_fx.wgsl").into()),
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("image effects"),
            bind_group_layouts: &[Some(&layout)],
            ..Default::default()
        });
        let pipeline = |fs: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(fs),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
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
        let pipelines = ["fs_blur", "fs_invert", "fs_color", "fs_bits", "fs_spider", "fs_blit"].map(pipeline);
        let linear = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("image effects (bilinear)"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let nearest =
            device.create_sampler(&wgpu::SamplerDescriptor { label: Some("image effects"), ..Default::default() });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("image effects"),
            size: SLOT * SLOTS,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        PostFx { layout, pipelines, linear, nearest, uniforms, format, targets: None }
    }

    fn targets(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        if self.targets.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        let tex: Vec<wgpu::Texture> = (0..5)
            .map(|_| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("image effects"),
                    size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: self.format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                })
            })
            .collect();
        let views = tex.iter().map(|t| t.create_view(&Default::default())).collect();
        self.targets = Some(Targets { size, tex, views });
    }

    /// Applies `effects` in order on `frame` (usable as a copy source). `blur`: menu blur
    /// (radius of the option), for the `blur` effect.
    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
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
        self.targets(device, size);
        let base = Cfg { size: [size.0 as f32, size.1 as f32], modulate: [1.0; 4], ..Default::default() };
        let mut slot = 0u64;
        for &fx in effects {
            if fx == Effect::Blur {
                blur.run(device, queue, enc, frame, frame_view, blur_radius, timer);
                continue;
            }
            let Some(t) = &self.targets else { return };
            enc.copy_texture_to_texture(
                frame.as_image_copy(),
                t.tex[M].as_image_copy(),
                wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            );
            let blur_cfg = |r: f32, dir: [f32; 2]| Cfg { radius: r, dir, ..base };
            let eye = |scale: [f32; 2], offset: [f32; 2], rotation: f32, scissor: [f32; 4], vignette: [f32; 4]| Cfg {
                scale,
                offset,
                rotation,
                scissor,
                vignette,
                ..base
            };
            const V: [f32; 4] = [0.31, 0.1, 0.69, 0.9];
            const FULL: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
            const NARROW: [f32; 4] = [0.21, 0.0, 0.79, 1.0];
            let passes: Vec<(Prog, usize, usize, Dst, Cfg)> = match fx {
                Effect::Invert => vec![(Prog::Invert, M, M, Dst::Frame, base)],
                Effect::Creeper => {
                    vec![(Prog::Color, M, M, Dst::Target(T), base), (Prog::Bits, T, T, Dst::Frame, base)]
                }
                Effect::Spider => vec![
                    (Prog::Blur, M, M, Dst::Target(T), blur_cfg(15.0, [1.0, 0.0])),
                    (Prog::Blur, T, T, Dst::Target(L), blur_cfg(15.0, [0.0, 1.0])),
                    (Prog::Blur, M, M, Dst::Target(T), blur_cfg(7.0, [1.0, 0.0])),
                    (Prog::Blur, T, T, Dst::Target(S), blur_cfg(7.0, [0.0, 1.0])),
                    (
                        Prog::Spider,
                        M,
                        L,
                        Dst::Target(T),
                        eye([1.25, 2.0], [-0.125, -0.1], 0.0, FULL, [0.1, 0.1, 0.9, 0.9]),
                    ),
                    (Prog::Spider, S, T, Dst::Target(W), eye([2.35, 4.2], [-1.1, -1.5], -45.0, NARROW, V)),
                    (Prog::Spider, S, W, Dst::Target(T), eye([2.35, 4.2], [0.45, -4.45], 45.0, NARROW, V)),
                    (Prog::Spider, S, T, Dst::Target(W), eye([2.35, 2.35], [-0.385, -1.29], 0.0, FULL, V)),
                    (Prog::Spider, S, W, Dst::Target(T), eye([2.35, 2.35], [-0.965, -1.29], 0.0, FULL, V)),
                    (Prog::Blit, T, T, Dst::Frame, Cfg { modulate: [1.0, 0.8, 0.8, 1.0], ..base }),
                ],
                Effect::Blur => unreachable!("handled above"),
            };
            for (prog, src, back, dst, cfg) in passes {
                if slot >= SLOTS {
                    return;
                }
                queue.write_buffer(&self.uniforms, slot * SLOT, bytemuck::bytes_of(&cfg));
                let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("image effects"),
                    layout: &self.layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                buffer: &self.uniforms,
                                offset: 0,
                                size: wgpu::BufferSize::new(std::mem::size_of::<Cfg>() as u64),
                            }),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&t.views[src]),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(&t.views[back]),
                        },
                        wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.linear) },
                        wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&self.nearest) },
                    ],
                });
                let out = match dst {
                    Dst::Target(i) => &t.views[i],
                    Dst::Frame => frame_view,
                };
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("image effect"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: out,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                    })],
                    timestamp_writes: timer.and_then(|t| t.pass("image effect", crate::gpu_time::Role::Inner)),
                    ..Default::default()
                });
                pass.set_pipeline(&self.pipelines[prog as usize]);
                pass.set_bind_group(0, &bind, &[(slot * SLOT) as u32]);
                pass.draw(0..3, 0..1);
                slot += 1;
            }
        }
    }
}
