//! Glowing outline of entities ("glowing" effect). The colored silhouettes of the glowing
//! entities are drawn into a target (`EntityRenderer::draw_glow`), then the game's chain
//! turns them into an outline: edge detection on the alpha, horizontal and vertical box
//! blur (radius 2), blend onto the frame before the hand and the interface.

use wgpu::util::DeviceExt;

pub const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Target of a pass: texture, view and the binding that reads it.
struct Target {
    view: wgpu::TextureView,
    _texture: wgpu::Texture,
}

pub struct EntityOutline {
    sobel: wgpu::RenderPipeline,
    blur: wgpu::RenderPipeline,
    blit: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    nearest: wgpu::Sampler,
    linear: wgpu::Sampler,
    size: (u32, u32),
    /// Silhouettes (A) and intermediate target (B).
    a: Option<Target>,
    b: Option<Target>,
    /// Bindings: sobel reads A, horizontal blur reads B, vertical blur reads A, blend reads B.
    binds: Vec<wgpu::BindGroup>,
}

impl EntityOutline {
    pub fn new(device: &wgpu::Device, surface: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::include_wgsl!("entity_outline.wgsl"));
        let entry = |binding, visibility| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("entity outline"),
            entries: &[
                entry(0, wgpu::ShaderStages::FRAGMENT),
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
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("entity outline"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |fs: &str, format: wgpu::TextureFormat, blend: Option<wgpu::BlendState>| {
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
                    targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let sampler = |filter| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: filter,
                min_filter: filter,
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                ..Default::default()
            })
        };
        EntityOutline {
            sobel: pipeline("fs_sobel", TARGET_FORMAT, None),
            blur: pipeline("fs_blur", TARGET_FORMAT, None),
            blit: pipeline("fs_blit", surface, Some(wgpu::BlendState::ALPHA_BLENDING)),
            layout,
            nearest: sampler(wgpu::FilterMode::Nearest),
            linear: sampler(wgpu::FilterMode::Linear),
            size: (0, 0),
            a: None,
            b: None,
            binds: Vec::new(),
        }
    }

    fn target(device: &wgpu::Device, (w, h): (u32, u32), label: &str) -> Target {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TARGET_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        Target { view: texture.create_view(&Default::default()), _texture: texture }
    }

    /// Targets at the window size (recreated when it changes).
    pub fn ensure(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        if self.size == size && self.a.is_some() {
            return;
        }
        let size = (size.0.max(1), size.1.max(1));
        let (a, b) = (Self::target(device, size, "silhouettes"), Self::target(device, size, "outline"));
        let cfg = |dir: [f32; 2]| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("outline"),
                contents: bytemuck::cast_slice(&[size.0 as f32, size.1 as f32, dir[0], dir[1]]),
                usage: wgpu::BufferUsages::UNIFORM,
            })
        };
        let bind = |c: &wgpu::Buffer, view: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("outline"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: c.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(view) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.nearest) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.linear) },
                ],
            })
        };
        let (c0, ch, cv) = (cfg([0.0, 0.0]), cfg([1.0, 0.0]), cfg([0.0, 1.0]));
        self.binds = vec![bind(&c0, &a.view), bind(&ch, &b.view), bind(&cv, &a.view), bind(&c0, &b.view)];
        (self.a, self.b, self.size) = (Some(a), Some(b), size);
    }

    /// Target of the colored silhouettes (to clear to transparent, then fill).
    pub fn silhouettes(&self) -> Option<&wgpu::TextureView> {
        self.a.as_ref().map(|t| &t.view)
    }

    fn pass(
        &self,
        enc: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        pl: &wgpu::RenderPipeline,
        bind: usize,
        clear: bool,
        timer: Option<&crate::gpu_time::GpuTimer>,
    ) {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("entity outline"),
            timestamp_writes: timer.and_then(|t| t.pass("entity outline", crate::gpu_time::Role::Inner)),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: if clear { wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT) } else { wgpu::LoadOp::Load },
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(pl);
        pass.set_bind_group(0, &self.binds[bind], &[]);
        pass.draw(0..3, 0..1);
    }

    /// Edge, horizontal blur, vertical blur: the result is in B.
    pub fn process(&self, enc: &mut wgpu::CommandEncoder, timer: Option<&crate::gpu_time::GpuTimer>) {
        let (Some(a), Some(b)) = (&self.a, &self.b) else { return };
        self.pass(enc, &b.view, &self.sobel, 0, true, timer);
        self.pass(enc, &a.view, &self.blur, 1, true, timer);
        self.pass(enc, &b.view, &self.blur, 2, true, timer);
    }

    /// Blend of the outline onto the frame.
    pub fn composite(
        &self,
        enc: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        timer: Option<&crate::gpu_time::GpuTimer>,
    ) {
        self.pass(enc, target, &self.blit, 3, false, timer);
    }
}
