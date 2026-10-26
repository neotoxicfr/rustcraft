//! Blur of the menu background (`docs/specs/option-menus.md` § 1.12, `title-screen-servers.md` § 1.1):
//! the already drawn frame (world, HUD, or panorama outside a world) is copied then blurred by three
//! pairs of box passes (horizontal then vertical) of radius `menuBackgroundBlurriness`, in
//! real pixels; the last pass writes into the frame. The menu interface is drawn afterwards.

/// Configuration of a pass (aligned to 16 bytes).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Cfg {
    inv_size: [f32; 2],
    dir: [f32; 2],
    radius: f32,
    pad: [f32; 3],
}

/// Working textures at the frame size and their bindings (A read horizontally, B vertically).
struct Targets {
    size: (u32, u32),
    a: wgpu::Texture,
    a_view: wgpu::TextureView,
    b_view: wgpu::TextureView,
    read_a: wgpu::BindGroup,
    read_b: wgpu::BindGroup,
}

pub struct Blur {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// Horizontal and vertical passes.
    cfg_h: wgpu::Buffer,
    cfg_v: wgpu::Buffer,
    format: wgpu::TextureFormat,
    targets: Option<Targets>,
}

impl Blur {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Blur {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blur"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
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
                entry_point: Some("fs"),
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
        let buf = |label| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: std::mem::size_of::<Cfg>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        Blur {
            pipeline,
            layout,
            sampler,
            cfg_h: buf("blur horizontal"),
            cfg_v: buf("blur vertical"),
            format,
            targets: None,
        }
    }

    fn targets(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        if self.targets.as_ref().is_none_or(|t| t.size != size) {
            let tex = |label| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
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
            };
            let (a, b) = (tex("blur A"), tex("blur B"));
            let (a_view, b_view) = (a.create_view(&Default::default()), b.create_view(&Default::default()));
            let bind = |view: &wgpu::TextureView, cfg: &wgpu::Buffer| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("blur"),
                    layout: &self.layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: cfg.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(view) },
                        wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                    ],
                })
            };
            let (read_a, read_b) = (bind(&a_view, &self.cfg_h), bind(&b_view, &self.cfg_v));
            self.targets = Some(Targets { size, a, a_view, b_view, read_a, read_b });
        }
    }

    /// Blurs `frame` (already drawn, usable as a copy source) in place: copy into A, then
    /// A → B (horizontal), B → A (vertical) twice, and A → B, B → frame for the last pair.
    /// `timer`: GPU time of the six passes (name "blur").
    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        enc: &mut wgpu::CommandEncoder,
        frame: &wgpu::Texture,
        frame_view: &wgpu::TextureView,
        radius: i32,
        timer: Option<&crate::gpu_time::GpuTimer>,
    ) {
        let size = (frame.width(), frame.height());
        let inv = [1.0 / size.0 as f32, 1.0 / size.1 as f32];
        let r = radius as f32;
        let h = Cfg { inv_size: inv, dir: [1.0, 0.0], radius: r, pad: [0.0; 3] };
        let v = Cfg { dir: [0.0, 1.0], ..h };
        queue.write_buffer(&self.cfg_h, 0, bytemuck::bytes_of(&h));
        queue.write_buffer(&self.cfg_v, 0, bytemuck::bytes_of(&v));
        self.targets(device, size);
        let (pipeline, Some(t)) = (&self.pipeline, &self.targets) else { return };
        enc.copy_texture_to_texture(
            frame.as_image_copy(),
            t.a.as_image_copy(),
            wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
        );
        for pair in 0..3 {
            for (bind, out) in [(&t.read_a, &t.b_view), (&t.read_b, if pair == 2 { frame_view } else { &t.a_view })] {
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("blur"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: out,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                    })],
                    timestamp_writes: timer.and_then(|t| t.pass("blur", crate::gpu_time::Role::Inner)),
                    ..Default::default()
                });
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, bind, &[]);
                pass.draw(0..3, 0..1);
            }
        }
    }
}
