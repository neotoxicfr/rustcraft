//! GPU side of the weather (`docs/specs/weather-border.md` § 1.7-1.8): instanced rain and
//! snow columns. The instances depend only on the camera block and the world; texture
//! scrolling and alpha (exact distance to the camera, rain level) are
//! computed in the shader on every frame.

use rc_assets::texture::Image;

/// Column: center (relative to the camera's integer origin in x, z), bottom and top, half-widths,
/// type parameters (rain: speed, tick offset; snow: double, Gaussian, double,
/// Gaussian), packed light, type (0 rain, 1 snow).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Inst {
    pub pos: [f32; 4],
    pub half: [f32; 2],
    pub p: [f32; 4],
    pub light: u32,
    pub kind: u32,
}

/// Frame uniform: (t & 131071, t & 511), then tick fraction, rain level, radius².
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Frame {
    pub ticks: [u32; 4],
    pub f: [f32; 4],
}

/// Weather images, loaded at startup.
pub struct WeatherImages {
    pub rain: Option<Image>,
    pub snow: Option<Image>,
    pub forcefield: Option<Image>,
}

/// Wall vertex: position relative to the wall origin (absolute y), texture coordinates.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable, Default)]
pub struct WallVert {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
}

/// Wall uniform: (x, z) offset toward the camera origin, scrolling; tint and alpha.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct WallFrame {
    pub off: [f32; 4],
    pub color: [f32; 4],
}

/// Border wall: 4 quads (south, west, north, east), drawn in `order`.
pub struct Wall {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    bind: Option<wgpu::BindGroup>,
    buf: wgpu::Buffer,
    pub verts: [WallVert; 24],
    pub dirty: bool,
    pub order: Vec<u32>,
    pub frame: WallFrame,
}

pub struct WeatherRenderer {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    pending: Option<WeatherImages>,
    /// Rain and snow bindings.
    binds: Option<[wgpu::BindGroup; 2]>,
    buf: Option<(wgpu::Buffer, u64)>,
    /// Instances to send (set by the preparation when they change).
    pub insts: Vec<Inst>,
    pub dirty: bool,
    /// Rain columns (the first ones), then snow.
    pub counts: (u32, u32),
    pub frame: Frame,
    pub wall: Wall,
    /// Camera layout (group 0), for the pipelines made on demand.
    camera_layout: wgpu::BindGroupLayout,
    /// Weather (two faces) and border (additive), improved transparency.
    oit: Option<[crate::oit::Set; 2]>,
}

impl WeatherRenderer {
    /// Weather and border pipelines for improved transparency (once).
    pub fn ensure_oit(&mut self, device: &wgpu::Device, layouts: &crate::oit::Layouts) {
        if self.oit.is_some() {
            return;
        }
        let groups = [Some(&self.camera_layout), Some(&self.layout)];
        let weather = crate::oit::module(
            device,
            "weather (transparency)",
            include_str!("weather.wgsl"),
            include_str!("weather_oit.wgsl"),
        );
        let insts = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Inst>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x2, 2 => Float32x4, 3 => Uint32, 4 => Uint32],
        })];
        let border = crate::oit::module(
            device,
            "border (transparency)",
            include_str!("border.wgsl"),
            include_str!("border_oit.wgsl"),
        );
        let verts = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<WallVert>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2],
        })];
        // Like the game: neither culls faces; border without depth offset.
        self.oit = Some([
            layouts.set(device, &crate::oit::Spec::new("weather (transparency)", &groups, &weather, "vs", &insts, "fs")),
            layouts
                .set(device, &crate::oit::Spec::new("border (transparency)", &groups, &border, "vs", &verts, "fs")),
        ]);
    }

    /// Border then weather, stage `f` of improved transparency; group 0 must be bound.
    pub fn draw_oit(&self, pass: &mut wgpu::RenderPass<'_>, f: crate::oit::Frame<'_>) {
        let Some([weather, border]) = &self.oit else { return };
        if let Some(bind) = &self.wall.bind
            && !self.wall.order.is_empty()
        {
            f.set(pass, border);
            pass.set_bind_group(1, bind, &[]);
            pass.set_vertex_buffer(0, self.wall.buf.slice(..));
            for &q in &self.wall.order {
                pass.draw(q * 6..q * 6 + 6, 0..1);
            }
        }
        let (Some(binds), Some((buf, _))) = (&self.binds, &self.buf) else { return };
        let (rain, snow) = self.counts;
        if rain + snow == 0 {
            return;
        }
        f.set(pass, weather);
        pass.set_vertex_buffer(0, buf.slice(..));
        for (bind, range) in binds.iter().zip([0..rain, rain..rain + snow]) {
            if !range.is_empty() {
                pass.set_bind_group(1, bind, &[]);
                pass.draw(0..6, range);
            }
        }
    }

    pub fn new(device: &wgpu::Device, camera_layout: &wgpu::BindGroupLayout, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("weather"),
            source: wgpu::ShaderSource::Wgsl(include_str!("weather.wgsl").into()),
        });
        let entry = |binding, visibility, ty| wgpu::BindGroupLayoutEntry { binding, visibility, ty, count: None };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("weather"),
            entries: &[
                entry(
                    0,
                    wgpu::ShaderStages::FRAGMENT,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                entry(1, wgpu::ShaderStages::FRAGMENT, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
                entry(
                    2,
                    wgpu::ShaderStages::VERTEX_FRAGMENT,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("weather"),
            bind_group_layouts: &[Some(camera_layout), Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("weather"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Inst>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x4, 1 => Float32x2, 2 => Float32x4, 3 => Uint32, 4 => Uint32
                    ],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::SrcAlpha,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(false),
                depth_compare: Some(crate::render::DEPTH_NEARER),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("weather (image)"),
            size: std::mem::size_of::<Frame>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        WeatherRenderer {
            pipeline,
            layout: layout.clone(),
            uniform,
            pending: None,
            binds: None,
            buf: None,
            insts: Vec::new(),
            dirty: false,
            counts: (0, 0),
            frame: Frame { ticks: [0; 4], f: [0.0; 4] },
            wall: Wall::new(device, camera_layout, &layout, format),
            camera_layout: camera_layout.clone(),
            oit: None,
        }
    }

    pub fn set_images(&mut self, images: WeatherImages) {
        self.pending = Some(images);
    }

    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if let Some(img) = self.pending.take() {
            // Nearest neighbor, repeat, no mipmap (§ 1.8).
            let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("weather"),
                address_mode_u: wgpu::AddressMode::Repeat,
                address_mode_v: wgpu::AddressMode::Repeat,
                ..Default::default()
            });
            let view = |i: &Option<Image>| {
                let fallback = Image { width: 1, height: 1, rgba: vec![0; 4] };
                upload_texture(device, queue, i.as_ref().unwrap_or(&fallback))
            };
            let (rain, snow) = (view(&img.rain), view(&img.snow));
            let bind = |v: &wgpu::TextureView| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("weather"),
                    layout: &self.layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(v) },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
                        wgpu::BindGroupEntry { binding: 2, resource: self.uniform.as_entire_binding() },
                    ],
                })
            };
            self.binds = Some([bind(&rain), bind(&snow)]);
            if let Some(f) = &img.forcefield {
                self.wall.set_texture(device, queue, f);
            }
        }
        self.wall.upload(queue);
        if self.counts == (0, 0) {
            return;
        }
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&self.frame));
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let need = (self.insts.len() * std::mem::size_of::<Inst>()) as u64;
        if self.buf.as_ref().is_none_or(|(_, cap)| *cap < need) {
            let cap = need.next_power_of_two().max(16 * 1024);
            let b = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("weather (columns)"),
                size: cap,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.buf = Some((b, cap));
        }
        if let Some((b, _)) = &self.buf {
            queue.write_buffer(b, 0, bytemuck::cast_slice(&self.insts));
        }
    }

    /// Rain then snow; group 0 (camera) must be bound.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        let (Some(binds), Some((buf, _))) = (&self.binds, &self.buf) else { return };
        let (rain, snow) = self.counts;
        if rain + snow == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, buf.slice(..));
        for (bind, range) in binds.iter().zip([0..rain, rain..rain + snow]) {
            if !range.is_empty() {
                pass.set_bind_group(1, bind, &[]);
                pass.draw(0..6, range);
            }
        }
    }
}

impl Wall {
    fn new(
        device: &wgpu::Device,
        camera_layout: &wgpu::BindGroupLayout,
        layout: &wgpu::BindGroupLayout,
        format: wgpu::TextureFormat,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("border"),
            source: wgpu::ShaderSource::Wgsl(include_str!("border.wgsl").into()),
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("border"),
            bind_group_layouts: &[Some(camera_layout), Some(layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("border"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<WallVert>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                // Weighted additive: rgb = src·a + dst, alpha = src.a.
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::SrcAlpha,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::Zero,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            // Depth written, offset toward the camera (non-reversed depth here).
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(crate::render::DEPTH_NEARER),
                stencil: Default::default(),
                bias: crate::render::depth_bias(3, 3.0),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("border (image)"),
            size: std::mem::size_of::<WallFrame>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("border (vertices)"),
            size: (24 * std::mem::size_of::<WallVert>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Wall {
            pipeline,
            layout: layout.clone(),
            uniform,
            bind: None,
            buf,
            verts: [WallVert::default(); 24],
            dirty: false,
            order: Vec::new(),
            frame: WallFrame { off: [0.0; 4], color: [0.0; 4] },
        }
    }

    /// `misc/forcefield.png` with 4 mipmap levels (2 × 2 average), linear minification,
    /// nearest magnification, repeat.
    fn set_texture(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, img: &Image) {
        let mut levels = vec![img.clone()];
        while levels.len() < 5 {
            let p = levels.last().expect("non-empty");
            if p.width < 2 || p.height < 2 {
                break;
            }
            let (w, h) = (p.width / 2, p.height / 2);
            let mut rgba = vec![0u8; (w * h * 4) as usize];
            for y in 0..h {
                for x in 0..w {
                    for c in 0..4 {
                        let at = |dx: u32, dy: u32| {
                            u32::from(p.rgba[(((2 * y + dy) * p.width + 2 * x + dx) * 4 + c) as usize])
                        };
                        rgba[((y * w + x) * 4 + c) as usize] =
                            ((at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1) + 2) / 4) as u8;
                    }
                }
            }
            levels.push(Image { width: w, height: h, rgba });
        }
        let size = wgpu::Extent3d { width: img.width, height: img.height, depth_or_array_layers: 1 };
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("border"),
            size,
            mip_level_count: levels.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (i, l) in levels.iter().enumerate() {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &tex,
                    mip_level: i as u32,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &l.rgba,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(l.width * 4),
                    rows_per_image: Some(l.height),
                },
                wgpu::Extent3d { width: l.width, height: l.height, depth_or_array_layers: 1 },
            );
        }
        let view = tex.create_view(&Default::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("border"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        self.bind = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("border"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: self.uniform.as_entire_binding() },
            ],
        }));
    }

    fn upload(&mut self, queue: &wgpu::Queue) {
        if self.order.is_empty() {
            return;
        }
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&self.frame));
        if self.dirty {
            self.dirty = false;
            queue.write_buffer(&self.buf, 0, bytemuck::cast_slice(&self.verts));
        }
    }

    /// Visible faces, from the closest to the farthest; group 0 must be bound.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        let Some(bind) = &self.bind else { return };
        if self.order.is_empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(1, bind, &[]);
        pass.set_vertex_buffer(0, self.buf.slice(..));
        for &q in &self.order {
            pass.draw(q * 6..q * 6 + 6, 0..1);
        }
    }
}

/// 8-bit RGBA texture without mipmap.
pub fn upload_texture(device: &wgpu::Device, queue: &wgpu::Queue, img: &Image) -> wgpu::TextureView {
    let size = wgpu::Extent3d { width: img.width, height: img.height, depth_or_array_layers: 1 };
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("weather"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        tex.as_image_copy(),
        &img.rgba,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(img.width * 4), rows_per_image: Some(img.height) },
        size,
    );
    tex.create_view(&Default::default())
}
