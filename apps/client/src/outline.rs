//! Outline of the targeted block (`docs/specs/targeting.md` § 4): edges of the merged outline shape,
//! black lines at 40%, of fixed thickness in pixels.

use hashbrown::HashMap;

use rc_data::Aabb;

/// Segment (ends relative to the block corner).
pub type Edge = [[f32; 3]; 2];

/// Edges of the union of the boxes: on the grid of their distinct coordinates, an edge
/// borders one or three full cells out of four, or two diagonally; consecutive aligned
/// edges merged.
pub fn shape_edges(boxes: &[Aabb]) -> Vec<Edge> {
    let coords = |a: usize| {
        let mut v: Vec<f64> = boxes.iter().flat_map(|b| [b.min[a], b.max[a]]).collect();
        v.sort_by(f64::total_cmp);
        v.dedup();
        v
    };
    let g = [coords(0), coords(1), coords(2)];
    let n = [g[0].len().saturating_sub(1), g[1].len().saturating_sub(1), g[2].len().saturating_sub(1)];
    let full_cell = |c: [usize; 3]| {
        let p: [f64; 3] = std::array::from_fn(|a| (g[a][c[a]] + g[a][c[a] + 1]) / 2.0);
        boxes.iter().any(|b| (0..3).all(|a| p[a] > b.min[a] && p[a] < b.max[a]))
    };
    let mut filled = vec![false; n[0] * n[1] * n[2]];
    for x in 0..n[0] {
        for y in 0..n[1] {
            for z in 0..n[2] {
                filled[(x * n[1] + y) * n[2] + z] = full_cell([x, y, z]);
            }
        }
    }
    let at = |c: [isize; 3]| {
        (0..3).all(|a| c[a] >= 0 && (c[a] as usize) < n[a])
            && filled[(c[0] as usize * n[1] + c[1] as usize) * n[2] + c[2] as usize]
    };
    let mut out = Vec::new();
    for axis in 0..3 {
        let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
        for j in 0..=n[u] {
            for k in 0..=n[v] {
                let mut run: Option<usize> = None;
                for i in 0..=n[axis] {
                    let edge = i < n[axis] && {
                        let cell = |du: isize, dv: isize| {
                            let mut c = [0isize; 3];
                            c[axis] = i as isize;
                            c[u] = j as isize + du;
                            c[v] = k as isize + dv;
                            at(c)
                        };
                        let (a, b, c, d) = (cell(-1, -1), cell(0, -1), cell(-1, 0), cell(0, 0));
                        let count = [a, b, c, d].iter().filter(|&&f| f).count();
                        count == 1 || count == 3 || (count == 2 && a == d)
                    };
                    match (edge, run) {
                        (true, None) => run = Some(i),
                        (false, Some(start)) => {
                            let point = |t: usize| {
                                let mut p = [0.0f32; 3];
                                p[axis] = g[axis][t] as f32;
                                p[u] = g[u][j] as f32;
                                p[v] = g[v][k] as f32;
                                p
                            };
                            out.push([point(start), point(i)]);
                            run = None;
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    out
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Segment {
    a: [f32; 3],
    b: [f32; 3],
}

pub struct Outline {
    pipeline: wgpu::RenderPipeline,
    /// High-contrast outline: pipelines of the black backdrop and the colored outline, binding and backdrop style.
    contrast: (wgpu::RenderPipeline, wgpu::RenderPipeline, wgpu::BindGroup, wgpu::Buffer),
    /// `highContrastBlockOutline` setting.
    pub high_contrast: bool,
    bind: wgpu::BindGroup,
    style: wgpu::Buffer,
    buf: Option<(wgpu::Buffer, u64)>,
    segments: Vec<Segment>,
    count: u32,
    /// Edges per shape index.
    cache: HashMap<u16, Vec<Edge>>,
    /// Translucent block: drawn after the translucent terrain.
    pub after_translucent: bool,
    /// Layouts (camera, style) and normal outline in improved transparency (on demand).
    layouts: (wgpu::BindGroupLayout, wgpu::BindGroupLayout),
    oit: Option<crate::oit::Set>,
}

impl Outline {
    /// Pipelines of the normal outline in improved transparency (once): two faces.
    pub fn ensure_oit(&mut self, device: &wgpu::Device, layouts: &crate::oit::Layouts) {
        if self.oit.is_some() {
            return;
        }
        let module = crate::oit::module(
            device,
            "outline (transparency)",
            include_str!("outline.wgsl"),
            include_str!("outline_oit.wgsl"),
        );
        let groups = [Some(&self.layouts.0), Some(&self.layouts.1)];
        let buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Segment>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3],
        })];
        let spec = crate::oit::Spec::new("outline (transparency)", &groups, &module, "vs", &buffers, "fs");
        self.oit = Some(layouts.set(device, &spec));
    }

    /// Improved transparency: high-contrast outline (opaque) with the solids, before or after the
    /// translucent terrain.
    pub fn draw_solid(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.high_contrast {
            self.draw(pass, self.after_translucent);
        }
    }

    /// Improved transparency: normal outline (translucent), stage `f`.
    pub fn draw_oit(&self, pass: &mut wgpu::RenderPass<'_>, f: crate::oit::Frame<'_>) {
        let (Some((buf, _)), Some(set)) = (&self.buf, &self.oit) else { return };
        if self.count == 0 || self.high_contrast {
            return;
        }
        f.set(pass, set);
        pass.set_bind_group(1, &self.bind, &[]);
        pass.set_vertex_buffer(0, buf.slice(..));
        pass.draw(0..6, 0..self.count);
    }

    pub fn new(device: &wgpu::Device, camera_layout: &wgpu::BindGroupLayout, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("targeted block outline"),
            source: wgpu::ShaderSource::Wgsl(include_str!("outline.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("targeted block outline"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let style = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("targeted block outline"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("targeted block outline"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: style.as_entire_binding() }],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("targeted block outline"),
            bind_group_layouts: &[Some(camera_layout), Some(&layout)],
            immediate_size: 0,
        });
        // Normal outline: translucent with no depth write. High contrast: opaque black outline,
        // then opaque colored outline pulled toward the camera (depth offset).
        let make = |blend: Option<wgpu::BlendState>, write: bool, bias: wgpu::DepthBiasState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("targeted block outline"),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Segment>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3],
                    })],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(write),
                    depth_compare: Some(crate::render::DEPTH_NEARER),
                    stencil: Default::default(),
                    bias,
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        // Normal outline (classic path): translucent, depth written (the translucents
        // drawn afterwards are masked by the line).
        let pipeline = make(Some(wgpu::BlendState::ALPHA_BLENDING), true, Default::default());
        let contrast_back = make(None, true, Default::default());
        let contrast_front = make(None, true, crate::render::depth_bias(1, 1.0));
        let back_style = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("high-contrast outline (backdrop)"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let back_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("high-contrast outline (backdrop)"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: back_style.as_entire_binding() }],
        });
        Outline {
            pipeline,
            contrast: (contrast_back, contrast_front, back_bind, back_style),
            high_contrast: false,
            bind,
            style,
            buf: None,
            segments: Vec::new(),
            count: 0,
            cache: HashMap::new(),
            after_translucent: false,
            layouts: (camera_layout.clone(), layout),
            oit: None,
        }
    }

    /// Targeted block: shape (index and boxes), block corner relative to the camera origin;
    /// `None`: no outline.
    pub fn set(&mut self, target: Option<(u16, &[Aabb], [f32; 3], bool)>) {
        self.segments.clear();
        let Some((shape, boxes, corner, translucent)) = target else { return };
        self.after_translucent = translucent;
        let edges = self.cache.entry(shape).or_insert_with(|| shape_edges(boxes));
        let add = |p: [f32; 3]| [p[0] + corner[0], p[1] + corner[1], p[2] + corner[2]];
        self.segments.extend(edges.iter().map(|e| Segment { a: add(e[0]), b: add(e[1]) }));
    }

    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, size: (u32, u32)) {
        self.count = self.segments.len() as u32;
        if self.segments.is_empty() {
            return;
        }
        let (w, h) = (size.0 as f32, size.1.max(1) as f32);
        let width = (w / 1920.0 * 2.5).max(2.5);
        // High contrast: opaque 0x57FFE1 outline on a black 7 px backdrop.
        let color = if self.high_contrast {
            [0x57 as f32 / 255.0, 1.0, 0xe1 as f32 / 255.0, 1.0]
        } else {
            [0.0, 0.0, 0.0, 102.0 / 255.0]
        };
        let style: [f32; 8] = [color[0], color[1], color[2], color[3], w, h, width, 0.0];
        queue.write_buffer(&self.style, 0, bytemuck::cast_slice(&style));
        if self.high_contrast {
            let back: [f32; 8] = [0.0, 0.0, 0.0, 1.0, w, h, 7.0, 0.0];
            queue.write_buffer(&self.contrast.3, 0, bytemuck::cast_slice(&back));
        }
        let bytes: &[u8] = bytemuck::cast_slice(&self.segments);
        if self.buf.as_ref().is_none_or(|(_, cap)| *cap < bytes.len() as u64) {
            let cap = (bytes.len() as u64).next_power_of_two().max(1024);
            self.buf = Some((
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("targeted block outline"),
                    size: cap,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                cap,
            ));
        }
        queue.write_buffer(&self.buf.as_ref().expect("created above").0, 0, bytes);
    }

    /// Drawing if `after_translucent` equals `late`; group 0 (camera) is already bound.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, late: bool) {
        let Some((buf, _)) = &self.buf else { return };
        if self.count == 0 || self.after_translucent != late {
            return;
        }
        pass.set_vertex_buffer(0, buf.slice(..));
        if self.high_contrast {
            pass.set_pipeline(&self.contrast.0);
            pass.set_bind_group(1, &self.contrast.2, &[]);
            pass.draw(0..6, 0..self.count);
            pass.set_pipeline(&self.contrast.1);
        } else {
            pass.set_pipeline(&self.pipeline);
        }
        pass.set_bind_group(1, &self.bind, &[]);
        pass.draw(0..6, 0..self.count);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_has_twelve_edges() {
        assert_eq!(shape_edges(&[Aabb { min: [0.0; 3], max: [1.0; 3] }]).len(), 12);
    }

    #[test]
    fn stairs_outline_is_merged() {
        // Step: low slab + upper quarter. Union: 18 edges (not 24 = 2 × 12).
        let stairs =
            [Aabb { min: [0.0, 0.0, 0.0], max: [1.0, 0.5, 1.0] }, Aabb { min: [0.0, 0.5, 0.5], max: [1.0, 1.0, 1.0] }];
        assert_eq!(shape_edges(&stairs).len(), 18);
    }

    #[test]
    fn two_stacked_slabs_are_one_cube() {
        let s = [Aabb { min: [0.0; 3], max: [1.0, 0.5, 1.0] }, Aabb { min: [0.0, 0.5, 0.0], max: [1.0; 3] }];
        assert_eq!(shape_edges(&s).len(), 12);
    }
}
