//! 3D overlays of the debug screen (`debug-f3.md` § 8): entity boxes, chunk borders,
//! tracers of the F3+F6 screen (collision shapes, sturdy faces, water levels, light
//! levels, heightmaps, carrier blocks, light sections), 3D crosshair.
//!
//! Primitives: segments as quads of fixed pixel thickness (points: squares), filled faces
//! sorted from farthest to nearest, floating text facing the camera. Like the game, opaque
//! segments write depth and translucent ones follow, tested without
//! writing it; all before the translucent terrain. Nothing is sent or drawn while the
//! lists are empty (no active entry: no cost).

use hashbrown::HashMap;
use std::time::{Duration, Instant};

use rc_physics::Aabb;

use crate::entities::{Entity, Kind};
use crate::nametag::Glyph;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Segment {
    pub a: [f32; 3],
    pub b: [f32; 3],
    /// RGBA colour (bytes, gamma space).
    pub color: [u8; 4],
    pub width: f32,
}

/// Filled face (four corners in outline order).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Face {
    p: [[f32; 3]; 4],
    color: [u8; 4],
}

/// Floating text glyph: anchor, block per font pixel.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct TextInst {
    anchor: [f32; 3],
    scale: f32,
    glyph: Glyph,
}

/// List to fill: `Depth` depth-tested, `Front` always visible, `Crosshair` after the hand.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    Depth,
    Front,
    Crosshair,
}

/// Segment lists: opaque depth, translucent depth, opaque front, translucent
/// front, crosshair.
const OPAQUE: usize = 0;
const TRANSLUCENT: usize = 1;
const FRONT_OPAQUE: usize = 2;
const FRONT_TRANSLUCENT: usize = 3;
const CROSSHAIR: usize = 4;

/// Floating text: default scale (block per font pixel = scale / 16).
const TEXT_SCALE: f32 = 0.32;

/// Tracer data refreshed at an interval (like the game) rather than every frame.
#[derive(Default)]
struct Caches {
    /// Collision shapes around the camera entity (100 ms).
    collisions: Option<(Instant, Vec<Aabb>)>,
    /// Entities around the camera entity for the carrier blocks (100 ms).
    supported: Option<(Instant, Vec<i32>)>,
    /// Sky light sections: edges and faces (500 ms).
    sections: Option<(Instant, SectionShapes)>,
    /// Glyphs laid out per text (white), and generation of the sheet.
    glyphs: HashMap<String, Vec<Glyph>>,
    glyph_gen: u32,
}

pub struct DebugLines {
    /// Segments: opaque (write depth), translucent, always in front.
    line_pipelines: [wgpu::RenderPipeline; 3],
    face_pipeline: wgpu::RenderPipeline,
    text_pipeline: wgpu::RenderPipeline,
    front_text_pipeline: wgpu::RenderPipeline,
    /// "Always in front" glyphs (sent after `texts`, in the same buffer).
    front_texts: Vec<TextInst>,
    front_text_count: u32,
    bind: wgpu::BindGroup,
    style: wgpu::Buffer,
    text_layout: wgpu::BindGroupLayout,
    text_sampler: wgpu::Sampler,
    axes: wgpu::Buffer,
    text_bind: Option<(wgpu::BindGroup, u32)>,
    seg_buf: Option<(wgpu::Buffer, u64)>,
    face_buf: Option<(wgpu::Buffer, u64)>,
    text_buf: Option<(wgpu::Buffer, u64)>,
    lists: [Vec<Segment>; 5],
    faces: Vec<Face>,
    texts: Vec<TextInst>,
    /// An opaque face or text exists (draw order, see `draw`).
    opaque_faces: bool,
    opaque_texts: bool,
    /// Camera (relative eye, direction) for segment clipping.
    clip: Option<([f32; 3], [f32; 3])>,
    /// Axes of the text panel (right, up).
    right_up: [[f32; 4]; 2],
    /// Instance ranges sent, per list; number of faces and glyphs sent.
    ranges: [std::ops::Range<u32>; 5],
    face_count: u32,
    text_count: u32,
    cache: Caches,
    /// Layouts (camera, line style) and pipelines of the improved transparency
    /// (segments, faces, texts), made on demand.
    layouts: (wgpu::BindGroupLayout, wgpu::BindGroupLayout),
    oit: Option<[crate::oit::Set; 3]>,
}

/// ARGB colour (`0xAARRGGBB`) as RGBA bytes.
pub fn rgba(c: u32) -> [u8; 4] {
    [(c >> 16) as u8, (c >> 8) as u8, c as u8, (c >> 24) as u8]
}

/// Float channel as 8 bits (truncation, like the game).
fn channel(v: f32) -> u32 {
    (v * 255.0).floor() as u32
}

/// ARGB colour from floats (alpha, red, green, blue).
fn argb(a: f32, r: f32, g: f32, b: f32) -> u32 {
    channel(a) << 24 | channel(r) << 16 | channel(g) << 8 | channel(b)
}

/// Channel-by-channel blend in 8 bits (`a + ⌊t × (b − a)⌋`).
fn lerp_argb(t: f32, a: u32, b: u32) -> u32 {
    let ch = |c: u32, s: u32| ((c >> s) & 0xff) as i32;
    let mix = |s: u32| (ch(a, s) + (t * (ch(b, s) - ch(a, s)) as f32).floor() as i32) as u32 & 0xff;
    mix(24) << 24 | mix(16) << 16 | mix(8) << 8 | mix(0)
}

impl DebugLines {
    pub fn new(device: &wgpu::Device, camera_layout: &wgpu::BindGroupLayout, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("debug lines"),
            source: wgpu::ShaderSource::Wgsl(include_str!("debug_lines.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("debug lines"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let style = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("debug lines"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("debug lines"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: style.as_entire_binding() }],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("debug lines"),
            bind_group_layouts: &[Some(camera_layout), Some(&layout)],
            immediate_size: 0,
        });
        let depth = |compare: wgpu::CompareFunction, write: bool| {
            Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(write),
                depth_compare: Some(compare),
                stencil: Default::default(),
                bias: Default::default(),
            })
        };
        let target = |blend: Option<wgpu::BlendState>| {
            [Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })]
        };
        let seg_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Unorm8x4, 3 => Float32];
        let line = |compare: wgpu::CompareFunction, write: bool, blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("debug lines"),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Segment>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &seg_attrs,
                    })],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &target(blend),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: depth(compare, write),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let alpha = Some(wgpu::BlendState::ALPHA_BLENDING);
        let face_attrs = wgpu::vertex_attr_array![
            0 => Float32x3, 1 => Float32x3, 2 => Float32x3, 3 => Float32x3, 4 => Unorm8x4
        ];
        let face_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("debug faces"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_face"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Face>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &face_attrs,
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_face"),
                compilation_options: Default::default(),
                targets: &target(alpha),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: depth(crate::render::DEPTH_NEARER, false),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        // Text: interface glyph sheet, panel axes.
        let text_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("debug text"),
            source: wgpu::ShaderSource::Wgsl(include_str!("debug_text.wgsl").into()),
        });
        let text_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("debug text"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let text_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("debug text"),
            bind_group_layouts: &[Some(camera_layout), Some(&text_layout)],
            immediate_size: 0,
        });
        let text_attrs = wgpu::vertex_attr_array![
            0 => Float32x3, 1 => Float32, 2 => Float32x4, 3 => Float32,
            4 => Float32x4, 5 => Unorm8x4, 6 => Uint32
        ];
        let text_pipe = |compare: wgpu::CompareFunction, write: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("debug text"),
                layout: Some(&text_pl),
                vertex: wgpu::VertexState {
                    module: &text_shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<TextInst>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &text_attrs,
                    })],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &text_shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &target(alpha),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: depth(compare, write),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let text_pipeline = text_pipe(crate::render::DEPTH_NEARER, true);
        // "Always in front" texts: no depth test, drawn from farthest to
        // nearest (the game tests them against each other on a cleared depth).
        let front_text_pipeline = text_pipe(wgpu::CompareFunction::Always, false);
        let text_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let axes = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("debug text (axes)"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        DebugLines {
            line_pipelines: [
                line(crate::render::DEPTH_NEARER, true, None),
                line(crate::render::DEPTH_NEARER, false, alpha),
                line(wgpu::CompareFunction::Always, false, alpha),
            ],
            face_pipeline,
            text_pipeline,
            front_text_pipeline,
            front_texts: Vec::new(),
            front_text_count: 0,
            bind,
            style,
            text_layout,
            text_sampler,
            axes,
            text_bind: None,
            seg_buf: None,
            face_buf: None,
            text_buf: None,
            lists: Default::default(),
            faces: Vec::new(),
            texts: Vec::new(),
            opaque_faces: false,
            opaque_texts: false,
            clip: None,
            right_up: [[0.0; 4]; 2],
            ranges: Default::default(),
            face_count: 0,
            text_count: 0,
            cache: Caches::default(),
            layouts: (camera_layout.clone(), layout),
            oit: None,
        }
    }

    /// Improved transparency pipelines (once): two faces, like the game's overlays.
    pub fn ensure_oit(&mut self, device: &wgpu::Device, layouts: &crate::oit::Layouts) {
        if self.oit.is_some() {
            return;
        }
        let lines = crate::oit::module(
            device,
            "debug (transparency)",
            include_str!("debug_lines.wgsl"),
            include_str!("debug_lines_oit.wgsl"),
        );
        let text = crate::oit::module(
            device,
            "debug text (transparency)",
            include_str!("debug_text.wgsl"),
            include_str!("debug_text_oit.wgsl"),
        );
        let groups = [Some(&self.layouts.0), Some(&self.layouts.1)];
        let text_groups = [Some(&self.layouts.0), Some(&self.text_layout)];
        let seg = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Segment>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Unorm8x4, 3 => Float32],
        })];
        let face = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Face>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![
                0 => Float32x3, 1 => Float32x3, 2 => Float32x3, 3 => Float32x3, 4 => Unorm8x4
            ],
        })];
        let glyph = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<TextInst>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![
                0 => Float32x3, 1 => Float32, 2 => Float32x4, 3 => Float32,
                4 => Float32x4, 5 => Unorm8x4, 6 => Uint32
            ],
        })];
        use crate::oit::Spec;
        self.oit = Some([
            layouts.set(device, &Spec::new("debug (transparency)", &groups, &lines, "vs", &seg, "fs")),
            layouts.set(device, &Spec::new("debug faces (transparency)", &groups, &lines, "vs_face", &face, "fs_face")),
            layouts.set(device, &Spec::new("debug text (transparency)", &text_groups, &text, "vs", &glyph, "fs")),
        ]);
    }

    /// Improved transparency: opaque segments of the "depth" layer, with the solids.
    pub fn draw_solid(&self, pass: &mut wgpu::RenderPass<'_>) {
        self.draw_segments(pass, OPAQUE, 0);
    }

    /// Improved transparency, stage `f`: faces, translucent segments and texts of the
    /// "depth" layer (opaque faces and texts included).
    pub fn draw_oit(&self, pass: &mut wgpu::RenderPass<'_>, f: crate::oit::Frame<'_>) {
        let Some([seg, face, text]) = &self.oit else { return };
        if let Some((buf, _)) = self.face_buf.as_ref().filter(|_| self.face_count > 0) {
            f.set(pass, face);
            pass.set_bind_group(1, &self.bind, &[]);
            pass.set_vertex_buffer(0, buf.slice(..));
            pass.draw(0..6, 0..self.face_count);
        }
        let r = self.ranges[TRANSLUCENT].clone();
        if let Some((buf, _)) = self.seg_buf.as_ref().filter(|_| !r.is_empty()) {
            f.set(pass, seg);
            pass.set_bind_group(1, &self.bind, &[]);
            pass.set_vertex_buffer(0, buf.slice(..));
            pass.draw(0..6, r);
        }
        if let (Some((buf, _)), Some((bind, _))) = (&self.text_buf, &self.text_bind)
            && self.text_count > 0
        {
            f.set(pass, text);
            pass.set_bind_group(1, bind, &[]);
            pass.set_vertex_buffer(0, buf.slice(..));
            pass.draw(0..6, 0..self.text_count);
        }
    }

    /// Clears the lists (every frame, before filling them).
    pub fn clear(&mut self) {
        for l in &mut self.lists {
            l.clear();
        }
        self.faces.clear();
        self.texts.clear();
        self.front_texts.clear();
        self.opaque_faces = false;
        self.opaque_texts = false;
    }

    pub fn is_empty(&self) -> bool {
        self.lists.iter().all(Vec::is_empty)
            && self.faces.is_empty()
            && self.texts.is_empty()
            && self.front_texts.is_empty()
    }

    /// Segment from `a` to `b` (frame relative to the camera's integer origin), filed according to its
    /// opacity.
    pub fn line(&mut self, layer: Layer, a: [f32; 3], b: [f32; 3], color: u32, width: f32) {
        // Clipping at the plane in front of the camera (0.1 block): an end behind the eye would corrupt the
        // projection of the quad.
        let (mut a, mut b) = (a, b);
        if let Some((eye, f)) = self.clip {
            let d = |p: [f32; 3]| (p[0] - eye[0]) * f[0] + (p[1] - eye[1]) * f[1] + (p[2] - eye[2]) * f[2] - 0.1;
            let (da, db) = (d(a), d(b));
            if da < 0.0 && db < 0.0 {
                return;
            }
            let cut = |p: [f32; 3], q: [f32; 3], dp: f32, dq: f32| {
                let t = dp / (dp - dq);
                std::array::from_fn(|i| p[i] + (q[i] - p[i]) * t)
            };
            if da < 0.0 {
                a = cut(a, b, da, db);
            } else if db < 0.0 {
                b = cut(b, a, db, da);
            }
        }
        if a == b {
            return;
        }
        self.push(layer, Segment { a, b, color: rgba(color), width });
    }

    fn push(&mut self, layer: Layer, s: Segment) {
        let opaque = s.color[3] == 255;
        let list = match layer {
            Layer::Depth if opaque => OPAQUE,
            Layer::Depth => TRANSLUCENT,
            Layer::Front if opaque => FRONT_OPAQUE,
            Layer::Front => FRONT_TRANSLUCENT,
            Layer::Crosshair => CROSSHAIR,
        };
        self.lists[list].push(s);
    }

    /// Point: square of `size` pixels on screen.
    pub fn point(&mut self, layer: Layer, p: [f32; 3], color: u32, size: f32) {
        if let Some((eye, f)) = self.clip
            && (p[0] - eye[0]) * f[0] + (p[1] - eye[1]) * f[1] + (p[2] - eye[2]) * f[2] < 0.05
        {
            return;
        }
        self.push(layer, Segment { a: p, b: p, color: rgba(color), width: size });
    }

    /// Outline of a box (12 edges).
    pub fn cuboid(&mut self, layer: Layer, min: [f32; 3], max: [f32; 3], color: u32, width: f32) {
        let p = |i: u8| {
            [
                if i & 1 != 0 { max[0] } else { min[0] },
                if i & 2 != 0 { max[1] } else { min[1] },
                if i & 4 != 0 { max[2] } else { min[2] },
            ]
        };
        for (a, b) in [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)] {
            self.line(layer, p(a), p(b), color, width);
        }
    }

    /// Filled face (corners in outline order).
    fn face(&mut self, p: [[f32; 3]; 4], color: u32) {
        let color = rgba(color);
        self.opaque_faces |= color[3] == 255;
        self.faces.push(Face { p, color });
    }

    /// Face `dir` (the game's 3D order) of the box `min`..`max`, filled.
    fn box_face(&mut self, min: [f32; 3], max: [f32; 3], dir: usize, color: u32) {
        let (a, b) = (min, max);
        let p = match dir {
            0 => [[a[0], a[1], a[2]], [b[0], a[1], a[2]], [b[0], a[1], b[2]], [a[0], a[1], b[2]]],
            1 => [[a[0], b[1], a[2]], [a[0], b[1], b[2]], [b[0], b[1], b[2]], [b[0], b[1], a[2]]],
            2 => [[a[0], a[1], a[2]], [a[0], b[1], a[2]], [b[0], b[1], a[2]], [b[0], a[1], a[2]]],
            3 => [[a[0], a[1], b[2]], [b[0], a[1], b[2]], [b[0], b[1], b[2]], [a[0], b[1], b[2]]],
            4 => [[a[0], a[1], a[2]], [a[0], a[1], b[2]], [a[0], b[1], b[2]], [a[0], b[1], a[2]]],
            _ => [[b[0], a[1], a[2]], [b[0], b[1], a[2]], [b[0], b[1], b[2]], [b[0], a[1], b[2]]],
        };
        self.face(p, color);
    }

    /// Filled box (six faces).
    pub(crate) fn filled_cuboid(&mut self, min: [f32; 3], max: [f32; 3], color: u32) {
        for d in 0..6 {
            self.box_face(min, max, d, color);
        }
    }

    /// Text centred on `pos`, growing downwards from it, `scale` = style scale (0.32 by
    /// default: 0.02 block per font pixel).
    fn text(&mut self, gui: &mut crate::gui::Gui, queue: &wgpu::Queue, pos: [f32; 3], s: &str, color: u32, scale: f32) {
        let c = rgba(color);
        self.opaque_texts |= c[3] == 255;
        self.texts.extend(text_glyphs(&mut self.cache, gui, queue, pos, s, c, scale));
    }

    /// "Always in front" text (style of the game's bring-to-foreground pass), same rules as `text`.
    pub(crate) fn front_text(
        &mut self,
        gui: &mut crate::gui::Gui,
        queue: &wgpu::Queue,
        pos: [f32; 3],
        s: &str,
        color: u32,
        scale: f32,
    ) {
        if self.cache.glyph_gen != gui.glyph_gen {
            self.cache.glyphs.clear();
            self.cache.glyph_gen = gui.glyph_gen;
        }
        self.front_texts.extend(text_glyphs(&mut self.cache, gui, queue, pos, s, rgba(color), scale));
    }

    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, size: (u32, u32), gui: &crate::gui::Gui) {
        if self.is_empty() {
            self.ranges = Default::default();
            (self.face_count, self.text_count, self.front_text_count) = (0, 0, 0);
            return;
        }
        let style: [f32; 4] = [size.0 as f32, size.1.max(1) as f32, 0.0, 0.0];
        queue.write_buffer(&self.style, 0, bytemuck::cast_slice(&style));
        let total: usize = self.lists.iter().map(Vec::len).sum();
        let mut at = 0u32;
        if total > 0 {
            let buf = ensure(device, &mut self.seg_buf, (total * std::mem::size_of::<Segment>()) as u64, "debug lines");
            for (i, l) in self.lists.iter().enumerate() {
                if !l.is_empty() {
                    let off = u64::from(at) * std::mem::size_of::<Segment>() as u64;
                    queue.write_buffer(buf, off, bytemuck::cast_slice(l));
                }
                self.ranges[i] = at..at + l.len() as u32;
                at += l.len() as u32;
            }
        } else {
            self.ranges = Default::default();
        }
        // Translucent faces: from farthest to nearest (centre of the four corners).
        self.face_count = self.faces.len() as u32;
        if !self.faces.is_empty() {
            let eye = self.clip.map_or([0.0; 3], |c| c.0);
            let d2 = |f: &Face| {
                (0..3).map(|i| ((f.p[0][i] + f.p[1][i] + f.p[2][i] + f.p[3][i]) / 4.0 - eye[i]).powi(2)).sum::<f32>()
            };
            self.faces.sort_by(|a, b| d2(b).total_cmp(&d2(a)));
            let bytes = (self.faces.len() * std::mem::size_of::<Face>()) as u64;
            let buf = ensure(device, &mut self.face_buf, bytes, "debug faces");
            queue.write_buffer(buf, 0, bytemuck::cast_slice(&self.faces));
        }
        self.text_count = self.texts.len() as u32;
        self.front_text_count = self.front_texts.len() as u32;
        if !self.front_texts.is_empty() {
            // From farthest to nearest (stable sort: the glyphs of a text stay grouped).
            let eye = self.clip.map_or([0.0; 3], |c| c.0);
            let d2 = |t: &TextInst| (0..3).map(|i| (t.anchor[i] - eye[i]).powi(2)).sum::<f32>();
            self.front_texts.sort_by(|a, b| d2(b).total_cmp(&d2(a)));
        }
        if !self.texts.is_empty() || !self.front_texts.is_empty() {
            if self.text_bind.as_ref().is_none_or(|b| b.1 != gui.glyph_gen) {
                let view = gui.glyph_view();
                self.text_bind = Some((
                    device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("debug text"),
                        layout: &self.text_layout,
                        entries: &[
                            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::Sampler(&self.text_sampler),
                            },
                            wgpu::BindGroupEntry { binding: 2, resource: self.axes.as_entire_binding() },
                        ],
                    }),
                    gui.glyph_gen,
                ));
            }
            queue.write_buffer(&self.axes, 0, bytemuck::cast_slice(&self.right_up));
            let size = std::mem::size_of::<TextInst>();
            let bytes = ((self.texts.len() + self.front_texts.len()) * size) as u64;
            let buf = ensure(device, &mut self.text_buf, bytes, "debug text");
            if !self.texts.is_empty() {
                queue.write_buffer(buf, 0, bytemuck::cast_slice(&self.texts));
            }
            if !self.front_texts.is_empty() {
                queue.write_buffer(buf, (self.texts.len() * size) as u64, bytemuck::cast_slice(&self.front_texts));
            }
        }
    }

    fn draw_segments(&self, pass: &mut wgpu::RenderPass<'_>, list: usize, pipeline: usize) {
        let r = self.ranges[list].clone();
        let Some((buf, _)) = &self.seg_buf else { return };
        if r.is_empty() {
            return;
        }
        pass.set_pipeline(&self.line_pipelines[pipeline]);
        pass.set_bind_group(1, &self.bind, &[]);
        pass.set_vertex_buffer(0, buf.slice(..));
        pass.draw(0..6, r);
    }

    fn draw_faces(&self, pass: &mut wgpu::RenderPass<'_>) {
        let Some((buf, _)) = &self.face_buf else { return };
        if self.face_count == 0 {
            return;
        }
        pass.set_pipeline(&self.face_pipeline);
        pass.set_bind_group(1, &self.bind, &[]);
        pass.set_vertex_buffer(0, buf.slice(..));
        pass.draw(0..6, 0..self.face_count);
    }

    fn draw_texts(&self, pass: &mut wgpu::RenderPass<'_>) {
        let (Some((buf, _)), Some((bind, _))) = (&self.text_buf, &self.text_bind) else { return };
        if self.text_count == 0 {
            return;
        }
        pass.set_pipeline(&self.text_pipeline);
        pass.set_bind_group(1, bind, &[]);
        pass.set_vertex_buffer(0, buf.slice(..));
        pass.draw(0..6, 0..self.text_count);
    }

    fn draw_front_texts(&self, pass: &mut wgpu::RenderPass<'_>) {
        let (Some((buf, _)), Some((bind, _))) = (&self.text_buf, &self.text_bind) else { return };
        if self.front_text_count == 0 {
            return;
        }
        pass.set_pipeline(&self.front_text_pipeline);
        pass.set_bind_group(1, bind, &[]);
        pass.set_vertex_buffer(0, buf.slice(..));
        pass.draw(0..6, self.text_count..self.text_count + self.front_text_count);
    }

    /// Drawing of a list; group 0 (world camera) must be bound. `Depth` follows the game's
    /// order: each kind of overlay is drawn at its first appearance in the sequence "opaque
    /// group (faces, lines, texts), translucent group (faces, lines, texts)", the kinds
    /// already seen being grouped with their first appearance.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, layer: Layer) {
        match layer {
            Layer::Depth => {
                if self.opaque_faces {
                    self.draw_faces(pass);
                }
                self.draw_segments(pass, OPAQUE, 0);
                if self.opaque_texts {
                    self.draw_texts(pass);
                }
                if !self.opaque_faces {
                    self.draw_faces(pass);
                }
                self.draw_segments(pass, TRANSLUCENT, 1);
                if !self.opaque_texts {
                    self.draw_texts(pass);
                }
            }
            Layer::Front => {
                self.draw_segments(pass, FRONT_OPAQUE, 2);
                self.draw_segments(pass, FRONT_TRANSLUCENT, 2);
                self.draw_front_texts(pass);
            }
            Layer::Crosshair => self.draw_segments(pass, CROSSHAIR, 2),
        }
    }

    pub fn has(&self, layer: Layer) -> bool {
        match layer {
            Layer::Depth => {
                !self.ranges[OPAQUE].is_empty()
                    || !self.ranges[TRANSLUCENT].is_empty()
                    || self.face_count > 0
                    || self.text_count > 0
            }
            Layer::Front => {
                !self.ranges[FRONT_OPAQUE].is_empty()
                    || !self.ranges[FRONT_TRANSLUCENT].is_empty()
                    || self.front_text_count > 0
            }
            Layer::Crosshair => !self.ranges[CROSSHAIR].is_empty(),
        }
    }
}

/// Glyphs of a white text laid out once (cache), tinted with `c`, centred on `pos`.
fn text_glyphs<'a>(
    cache: &'a mut Caches,
    gui: &mut crate::gui::Gui,
    queue: &wgpu::Queue,
    pos: [f32; 3],
    s: &str,
    c: [u8; 4],
    scale: f32,
) -> impl Iterator<Item = TextInst> + use<'a> {
    if !cache.glyphs.contains_key(s) {
        let chars = rc_text::styled_chars(&rc_text::flatten(&rc_text::Component::text(s), &gui.lang));
        let g = crate::nametag::glyphs(&gui.text_quads(queue, &chars, 0xffff_ffff, 0));
        cache.glyphs.insert(s.to_owned(), g);
    }
    let packed = u32::from_le_bytes(c);
    let k = scale / 16.0;
    cache.glyphs[s].iter().map(move |&g| TextInst { anchor: pos, scale: k, glyph: Glyph { color: packed, ..g } })
}

/// Vertex buffer of at least `bytes` bytes (grown to a power of two).
fn ensure<'a>(
    device: &wgpu::Device,
    slot: &'a mut Option<(wgpu::Buffer, u64)>,
    bytes: u64,
    label: &str,
) -> &'a wgpu::Buffer {
    if slot.as_ref().is_none_or(|(_, cap)| *cap < bytes) {
        let cap = bytes.next_power_of_two().max(4096);
        *slot = Some((
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: cap,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            cap,
        ));
    }
    &slot.as_ref().expect("created above").0
}

/// What the frame's overlays need.
pub struct Scene<'a> {
    pub debug: &'a crate::debug_screen::DebugScreen,
    pub entities: &'a crate::entities::Entities,
    pub eye: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub pt: f32,
    /// Entity followed by the camera (`set_camera`); `None`: the local player.
    pub camera: Option<i32>,
    /// First-person view: the camera entity has no box.
    pub first_person: bool,
    /// Free focus camera: the local player is never drawn.
    pub free_cam: bool,
    pub player: &'a rc_physics::Player,
    pub grid: &'a rc_world::ChunkGrid,
    pub min_y: i32,
    pub height: i32,
    pub phys: &'a rc_physics::Tables,
    pub border: &'a rc_physics::WorldBorder,
    pub data: &'a rc_data::VersionData,
    pub ids: &'a crate::stack::ComponentIds,
    pub crosshair: bool,
    pub gui_scale: u32,
}

impl Scene<'_> {
    fn view(&self) -> crate::environ::WorldView<'_> {
        crate::environ::WorldView { grid: self.grid, min_y: self.min_y, height: self.height }
    }

    fn state(&self, x: i32, y: i32, z: i32) -> u32 {
        self.view().state(x, y, z)
    }

    /// Physics world; `colliders`: boxes of the solid entities.
    fn world<'b>(&'b self, colliders: &'b [Aabb]) -> crate::GridWorld<'b> {
        crate::GridWorld {
            grid: self.grid,
            min_y: self.min_y,
            height: self.height,
            t: self.phys,
            border: self.border,
            colliders,
            movers: None,
            noclip: None,
        }
    }
}

/// Frame relative to the camera's integer origin.
struct Rel {
    origin: [i32; 2],
}

impl Rel {
    fn p(&self, p: [f64; 3]) -> [f32; 3] {
        [(p[0] - f64::from(self.origin[0])) as f32, p[1] as f32, (p[2] - f64::from(self.origin[1])) as f32]
    }

    fn b(&self, b: &Aabb) -> ([f32; 3], [f32; 3]) {
        (self.p(b.min), self.p(b.max))
    }
}

/// Fills the frame's lists (§ 8); empty if no rendering entry is active.
pub fn fill(r: &mut crate::render::Renderer, s: &Scene<'_>, in_view: &dyn Fn([f32; 3], [f32; 3]) -> bool) {
    let queue = r.queue_handle();
    // Paths and visibility of the sections: nodes of the last occlusion walk.
    let (occ_paths, occ_vis) = (s.debug.is_on("chunk_section_paths"), s.debug.is_on("chunk_section_visibility"));
    r.occ_debug = false;
    let occ = (occ_paths || occ_vis).then(|| r.occlusion_debug());
    let (lines, gui, er) = (&mut r.debug_lines, &mut r.gui, r.entities.as_ref());
    lines.clear();
    if lines.cache.glyph_gen != gui.glyph_gen {
        lines.cache.glyphs.clear();
        lines.cache.glyph_gen = gui.glyph_gen;
    }
    let now = Instant::now();
    let rel = Rel { origin: [s.eye[0].floor() as i32, s.eye[2].floor() as i32] };
    let (right, up, f) = crate::camera::axes(s.yaw, s.pitch);
    lines.clip = Some((rel.p(s.eye), f.to_array()));
    lines.right_up = [right.extend(0.0).to_array(), up.extend(0.0).to_array()];
    let table = er.map_or(&[][..], |e| e.dims.as_slice());
    let painting = |v: i32| er.and_then(|e| e.painting_size(v));
    let on = |k: &str| s.debug.is_on(k);
    // Camera entity: position (feet) and box.
    let cam_entity = s.camera.and_then(|id| s.entities.map.get(&id));
    let (cam_pos, cam_box) = match cam_entity {
        Some(e) => e.bounding_box(table, &painting, false),
        None => (s.player.pos, s.player.bounding_box()),
    };
    let cam_block = cam_pos.map(|v| v.floor() as i32);
    let eye_block = s.eye.map(|v| v.floor() as i32);
    let player_block = s.player.pos.map(|v| v.floor() as i32);
    if on("chunk_borders") {
        chunk_borders(lines, s, &rel, cam_block);
    }
    if on("visualize_water_levels") {
        water_levels(lines, gui, &queue, s, &rel, player_block);
    }
    if on("visualize_heightmap") {
        heightmaps(lines, s, &rel, eye_block);
    }
    if on("visualize_collision_boxes") {
        let stale =
            lines.cache.collisions.as_ref().is_none_or(|c| now.duration_since(c.0) > Duration::from_millis(100));
        if stale {
            let boxes = collisions(s, cam_entity, &cam_box);
            lines.cache.collisions = Some((now, boxes));
        }
        let boxes = lines.cache.collisions.take().expect("filled above");
        for b in &boxes.1 {
            let (a, c) = rel.b(b);
            lines.cuboid(Layer::Depth, a, c, 0xFFFF_FFFF, 2.5);
        }
        lines.cache.collisions = Some(boxes);
    } else {
        lines.cache.collisions = None;
    }
    if on("visualize_entity_supporting_blocks") {
        supporting_blocks(lines, s, &rel, cam_entity, &cam_box, table, now);
    } else {
        lines.cache.supported = None;
    }
    let (block_light, sky_light) = (on("visualize_block_light_levels"), on("visualize_sky_light_levels"));
    if block_light || sky_light {
        light_levels(lines, gui, &queue, s, &rel, eye_block, block_light, sky_light);
    }
    if on("visualize_solid_faces") {
        solid_faces(lines, s, &rel, eye_block);
    }
    if on("visualize_sky_light_sections") {
        let stale = lines.cache.sections.as_ref().is_none_or(|c| now.duration_since(c.0) > Duration::from_millis(500));
        if stale {
            let center = player_block.map(|v| v >> 4);
            lines.cache.sections = Some((now, SectionShapes::new(s, center)));
        }
        let shapes = lines.cache.sections.take().expect("filled above");
        shapes.1.emit(lines, &rel);
        lines.cache.sections = Some(shapes);
    } else {
        lines.cache.sections = None;
    }
    if on("entity_hitboxes") {
        // The local player is drawn unless they are the camera entity seen from the inside.
        let me_drawn = !s.free_cam && (s.camera.is_some() || !s.first_person);
        let me = s.entities.me.as_ref().filter(|_| me_drawn);
        for (id, e) in s.entities.map.iter().map(|(id, e)| (Some(*id), e)).chain(me.map(|e| (None, e))) {
            if e.flags() & 0x20 != 0 {
                continue;
            }
            if id.is_some() && id == s.camera && s.first_person {
                continue;
            }
            hitbox(lines, s, e, table, er, &painting, &rel, in_view);
        }
    }
    if let Some(nodes) = &occ {
        section_graph(lines, s, &rel, in_view, nodes, occ_paths, occ_vis);
    }
    if s.crosshair && on("3d_crosshair") {
        let c = rel.p(s.eye);
        let c = [c[0] + f.x, c[1] + f.y, c[2] + f.z];
        let k = 0.01 * s.gui_scale as f32;
        let axes = [([k, 0.0, 0.0], 0xFFFF_0000), ([0.0, k, 0.0], 0xFF00_FF00), ([0.0, 0.0, k], 0xFF7F_7FFF)];
        for (pass, width) in [(0, 4.0), (1, 2.0)] {
            for (d, color) in axes {
                let end = [c[0] + d[0], c[1] + d[1], c[2] + d[2]];
                lines.line(Layer::Crosshair, c, end, if pass == 0 { 0xFF00_0000 } else { color }, width);
            }
        }
    }
}

/// The frame carries a map (the box then covers the whole block).
pub(crate) fn framed_map(e: &Entity, ids: &crate::stack::ComponentIds) -> bool {
    matches!(e.kind.as_str(), "item_frame" | "glow_item_frame")
        && matches!(e.meta.get(&9), Some(rc_proto_v777::entity::Meta::Item(Some(st)))
            if st.count > 0 && crate::maps::map_id(ids, st).is_some())
}

/// Arrow: segment and tip of four strokes set back and spread by
/// `l = clamp(0.1 × length, 0.1, 1)` at 45° (frame rotated from +X towards the direction).
pub(crate) fn arrow(lines: &mut DebugLines, a: [f32; 3], b: [f32; 3], color: u32, width: f32) {
    lines.line(Layer::Depth, a, b, color, width);
    for t in arrow_tips(a, b) {
        lines.line(Layer::Depth, t, b, color, width);
    }
}

fn arrow_tips(a: [f32; 3], b: [f32; 3]) -> [[f32; 3]; 4] {
    let d = glam::Vec3::from(b) - glam::Vec3::from(a);
    let len = d.length();
    let rot = glam::Quat::from_rotation_arc(glam::Vec3::X, d / len.max(f32::MIN_POSITIVE));
    let l = (len * 0.1).clamp(0.1, 1.0);
    [[-l, l, 0.0], [-l, 0.0, l], [-l, -l, 0.0], [-l, 0.0, -l]]
        .map(|t| (glam::Vec3::from(b) + rot * glam::Vec3::from(t)).to_array())
}

/// The entity renderer would draw this entity (so the game draws its box).
fn shown_by_renderer(
    s: &Scene<'_>,
    e: &Entity,
    er: Option<&crate::entity_render::EntityRenderer>,
    bb: &Aabb,
    rel: &Rel,
    in_view: &dyn Fn([f32; 3], [f32; 3]) -> bool,
) -> bool {
    if er.is_some_and(|er| !er.within_view_distance(e, s.pt, s.eye)) {
        return false;
    }
    let (lo, hi) = rel.b(&Aabb { min: bb.min.map(|v| v - 0.5), max: bb.max.map(|v| v + 0.5) });
    if !in_view(lo, hi) {
        return false;
    }
    let p = e.render_pos(s.pt);
    s.grid.get((p[0].floor() as i32) >> 4, (p[2].floor() as i32) >> 4).is_some()
}

/// Box of an entity (§ 8.1): white collision, dot at the feet, yellow seat plate,
/// eyes in red (living), dragon parts, blue gaze arrow (all entities).
#[allow(clippy::too_many_arguments)]
fn hitbox(
    lines: &mut DebugLines,
    s: &Scene<'_>,
    e: &Entity,
    table: &[[f32; 4]],
    er: Option<&crate::entity_render::EntityRenderer>,
    painting: &dyn Fn(i32) -> Option<(u8, u8)>,
    rel: &Rel,
    in_view: &dyn Fn([f32; 3], [f32; 3]) -> bool,
) {
    let (latest, bb) = e.bounding_box(table, painting, framed_map(e, s.ids));
    // Interpolation offset: rendered position minus tick position (zero for attached,
    // motionless entities).
    let rp = e.render_pos(s.pt);
    let off: [f64; 3] = std::array::from_fn(|i| rp[i] - e.pos[i]);
    let current: [f64; 3] = std::array::from_fn(|i| latest[i] + off[i]);
    let bb = bb.offset(off[0], off[1], off[2]);
    let (min, max) = rel.b(&bb);
    // Like the entity renderer: display distance, box inflated by 0.5 in the field of view,
    // column of the entity's block present (section built).
    if !shown_by_renderer(s, e, er, &bb, rel, in_view) {
        return;
    }
    lines.cuboid(Layer::Depth, min, max, 0xFFFF_FFFF, 2.5);
    lines.point(Layer::Depth, rel.p(current), 0xFFFF_FFFF, 2.0);
    let size = e.size(table);
    if let Some(v) = e.vehicle.and_then(|id| s.entities.map.get(&id)) {
        // Seat: vehicle attachment point = passenger position + its "vehicle" attachment.
        let w = f64::from(v.size(table).w.min(size.w) / 2.0);
        let y = rp[1] + crate::riding::rider_attach(e, f64::from(e.attribute_scale()));
        let (a, b) = rel.b(&Aabb { min: [rp[0] - w, y, rp[2] - w], max: [rp[0] + w, y + 0.0625, rp[2] + w] });
        lines.cuboid(Layer::Depth, a, b, 0xFFFF_FF00, 2.5);
    }
    if e.class != Kind::Other {
        let eye = bb.min[1] + f64::from(size.eye);
        let pad = f64::from(0.01f32);
        let (a, b) = rel.b(&Aabb { min: [bb.min[0], eye - pad, bb.min[2]], max: [bb.max[0], eye + pad, bb.max[2]] });
        lines.cuboid(Layer::Depth, a, b, 0xFFFF_0000, 2.5);
    }
    if e.kind == "ender_dragon" {
        for (p, w, h) in dragon_parts(e, s.pt) {
            let r = f64::from(w) / 2.0;
            let (a, b) =
                rel.b(&Aabb { min: [p[0] - r, p[1], p[2] - r], max: [p[0] + r, p[1] + f64::from(h), p[2] + r] });
            lines.cuboid(Layer::Depth, a, b, argb(1.0, 0.25, 1.0, 0.0), 2.5);
        }
    }
    let (yaw, pitch) = e.view_rotation(s.pt);
    let v = rc_physics::view_vector(pitch, yaw);
    let eye = [current[0], current[1] + f64::from(size.eye), current[2]];
    let end = [eye[0] + v[0] * 2.0, eye[1] + v[1] * 2.0, eye[2] + v[2] * 2.0];
    arrow(lines, rel.p(eye), rel.p(end), 0xFF00_00FF, 2.5);
}

/// Ring of the dragon's flight history (`entity_render::dragon`).
const RING: usize = 64;

/// Dragon parts (head, neck, body, three tails, two wings): position interpolated between
/// the previous and the current tick, width, height. Placed as the game's client places them on
/// each tick from the flight history; never placed (no AI from the spawn): at
/// the world origin.
fn dragon_parts(e: &Entity, pt: f32) -> [([f64; 3], f32, f32); 8] {
    const SIZES: [(f32, f32); 8] =
        [(1.0, 1.0), (3.0, 3.0), (5.0, 3.0), (2.0, 2.0), (2.0, 2.0), (2.0, 2.0), (4.0, 2.0), (4.0, 2.0)];
    if e.history.len() < RING {
        return SIZES.map(|(w, h)| ([0.0; 3], w, h));
    }
    let head = e.extra[0] as usize;
    let sitting = matches!(e.meta_int(16), Some(5 | 6 | 7 | 10));
    let now = place_parts(e, head, 0, e.pos, sitting);
    let old = place_parts(e, head, 1, e.pos_o, sitting);
    let t = f64::from(pt);
    std::array::from_fn(|i| (std::array::from_fn(|a| old[i][a] + (now[i][a] - old[i][a]) * t), SIZES[i].0, SIZES[i].1))
}

/// Positions of the parts at the tick `shift` ticks before the last (dragon feet `pos`).
fn place_parts(e: &Entity, head: usize, shift: usize, pos: [f64; 3], sitting: bool) -> [[f64; 3]; 8] {
    use rc_physics::{cos_t, sin_t};
    const DEG: f32 = std::f32::consts::PI / 180.0;
    let get = |k: usize| e.history[(head + RING - ((k + shift) & (RING - 1))) & (RING - 1)];
    let yaw = get(0)[1] as f32;
    let tilt = ((get(5)[0] - get(10)[0]) as f32) * 10.0 * DEG;
    let (cc_tilt, ss_tilt) = (cos_t(tilt), sin_t(tilt));
    let rot1 = yaw * DEG;
    let (ss1, cc1) = (sin_t(rot1), cos_t(rot1));
    let at = |x: f32, y: f64, z: f32| [pos[0] + f64::from(x), pos[1] + y, pos[2] + f64::from(z)];
    let y_off = if sitting { -1.0 } else { (get(5)[0] - get(0)[0]) as f32 };
    let mut out = [[0.0; 3]; 8];
    out[2] = at(ss1 * 0.5, 0.0, -cc1 * 0.5);
    out[6] = at(cc1 * 4.5, 2.0, ss1 * 4.5);
    out[7] = at(cc1 * -4.5, 2.0, ss1 * -4.5);
    out[0] = at(ss1 * 6.5 * cc_tilt, f64::from(y_off + ss_tilt * 6.5), -cc1 * 6.5 * cc_tilt);
    out[1] = at(ss1 * 5.5 * cc_tilt, f64::from(y_off + ss_tilt * 5.5), -cc1 * 5.5 * cc_tilt);
    let p1 = get(5);
    for i in 0..3 {
        let p0 = get(12 + i * 2);
        let rot = rot1 + wrap_degrees(p0[1] - p1[1]) * DEG;
        let (ss, cc) = (sin_t(rot), cos_t(rot));
        let dd = (i + 1) as f32 * 2.0;
        out[3 + i] = at(
            -(ss1 * 1.5 + ss * dd) * cc_tilt,
            p0[0] - p1[0] - f64::from((dd + 1.5) * ss_tilt) + 1.5,
            (cc1 * 1.5 + cc * dd) * cc_tilt,
        );
    }
    out
}

/// Angle brought into [−180, 180[ (degrees).
fn wrap_degrees(a: f64) -> f32 {
    let mut v = a % 360.0;
    if v >= 180.0 {
        v -= 360.0;
    }
    if v < -180.0 {
        v += 360.0;
    }
    v as f32
}

/// Borders of the camera entity's chunk (§ 8.2).
fn chunk_borders(lines: &mut DebugLines, s: &Scene<'_>, rel: &Rel, cam: [i32; 3]) {
    let x0 = ((cam[0] >> 4) * 16 - rel.origin[0]) as f32;
    let z0 = ((cam[2] >> 4) * 16 - rel.origin[1]) as f32;
    let (y0, y1) = (s.min_y as f32, (s.min_y + s.height) as f32);
    let v = |l: &mut DebugLines, x: f32, z: f32, color: u32, width: f32| {
        l.line(Layer::Depth, [x, y0, z], [x, y1, z], color, width);
    };
    for dx in [-16.0, 0.0, 16.0, 32.0] {
        for dz in [-16.0, 0.0, 16.0, 32.0] {
            v(lines, x0 + dx, z0 + dz, argb(0.5, 1.0, 0.0, 0.0), 4.0);
        }
    }
    for i in (2..16).step_by(2) {
        let o = i as f32;
        let color = if i % 4 == 0 { 0xFF00_9B9B } else { 0xFFFF_FF00 };
        v(lines, x0 + o, z0, color, 1.0);
        v(lines, x0 + o, z0 + 16.0, color, 1.0);
    }
    for i in (2..16).step_by(2) {
        let o = i as f32;
        let color = if i % 4 == 0 { 0xFF00_9B9B } else { 0xFFFF_FF00 };
        v(lines, x0, z0 + o, color, 1.0);
        v(lines, x0 + 16.0, z0 + o, color, 1.0);
    }
    let square = |l: &mut DebugLines, y: f32, color: u32, width: f32| {
        let c = [[x0, y, z0], [x0, y, z0 + 16.0], [x0 + 16.0, y, z0 + 16.0], [x0 + 16.0, y, z0]];
        for i in 0..4 {
            l.line(Layer::Depth, c[i], c[(i + 1) % 4], color, width);
        }
    };
    let mut y = s.min_y;
    while y <= s.min_y + s.height {
        let color = if y % 8 == 0 { 0xFF00_9B9B } else { 0xFFFF_FF00 };
        square(lines, y as f32, color, 1.0);
        y += 2;
    }
    let major = argb(1.0, 0.25, 0.25, 1.0);
    for (dx, dz) in [(0.0, 0.0), (0.0, 16.0), (16.0, 0.0), (16.0, 16.0)] {
        v(lines, x0 + dx, z0 + dz, major, 4.0);
    }
    // Cube of the current section, in front of everything.
    let sy = (cam[1] >> 4) as f32 * 16.0;
    lines.cuboid(Layer::Front, [x0, sy, z0], [x0 + 16.0, sy + 16.0, z0 + 16.0], major, 1.0);
    let mut y = s.min_y;
    while y <= s.min_y + s.height {
        square(lines, y as f32, major, 4.0);
        y += 16;
    }
}

/// Collision shapes that touch the camera entity's box inflated by 6: blocks (whole shape
/// as soon as one of its boxes touches the zone; those of the local player depending on their state),
/// then solid entities.
fn collisions(s: &Scene<'_>, cam: Option<&Entity>, cam_box: &Aabb) -> Vec<Aabb> {
    let area = Aabb { min: cam_box.min.map(|v| v - 6.0), max: cam_box.max.map(|v| v + 6.0) };
    let exclude = s.camera;
    let colliders = crate::entity_motion::colliders(s.entities, exclude, Some(cam_box.min[1]));
    let world = s.world(&colliders);
    let player_view;
    let w: &dyn rc_physics::World = if cam.is_none() {
        player_view = s.player.collision_view(&world, s.phys);
        &player_view
    } else {
        &world
    };
    let mut out = Vec::new();
    let lo = area.min.map(|v| (v - 1e-7).floor() as i32 - 1);
    let hi = area.max.map(|v| (v + 1e-7).floor() as i32 + 1);
    let mut cell = Vec::new();
    for x in lo[0]..=hi[0] {
        for y in lo[1]..=hi[1] {
            for z in lo[2]..=hi[2] {
                cell.clear();
                w.collision(x, y, z, &mut cell);
                let start = out.len();
                out.extend(cell.iter().map(|b| b.offset(f64::from(x), f64::from(y), f64::from(z))));
                if !out[start..].iter().any(|b| b.intersects(&area)) {
                    out.truncate(start);
                }
            }
        }
    }
    w.entity_colliders(&area, &mut out);
    out
}

/// Offset specific to each entity (keeps their outlines from merging): drawn from the hash of the
/// decimal text of the id + 0.132453657.
fn support_bias(id: i32) -> f64 {
    let text = format!("{}", f64::from(id) + 0.132_453_657);
    let hash = text.encode_utf16().fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(i32::from(c as i16)));
    0.02 * f64::from(hash % 1000) / 1000.0
}

/// Carrier blocks: local player in red, other entities in green (all those within 16 blocks of
/// the camera entity's box), effect block in cyan if it differs.
fn supporting_blocks(
    lines: &mut DebugLines,
    s: &Scene<'_>,
    rel: &Rel,
    cam: Option<&Entity>,
    cam_box: &Aabb,
    table: &[[f32; 4]],
    now: Instant,
) {
    let colliders = [];
    let world = s.world(&colliders);
    let highlight = |lines: &mut DebugLines, pos: [i32; 3], offset: f64, color: u32| {
        let from = pos.map(|v| f64::from(v) - 2.0 * offset);
        let b = Aabb { min: from, max: from.map(|v| v + 1.0 + 4.0 * offset) };
        let (a, c) = rel.b(&b);
        lines.cuboid(Layer::Depth, a, c, (102 << 24) | (color & 0xff_ffff), 2.5);
        let mut shape = Vec::new();
        s.phys.collision(s.state(pos[0], pos[1], pos[2]), &mut shape);
        for b in shape {
            let (a, c) = rel.b(&b.offset(f64::from(pos[0]), f64::from(pos[1]), f64::from(pos[2])));
            lines.cuboid(Layer::Depth, a, c, color, 2.5);
        }
    };
    let draw = |lines: &mut DebugLines, support: [i32; 3], effect: [i32; 3], bias: f64, color: u32| {
        highlight(lines, support, 0.02 + bias, color);
        if effect != support {
            highlight(lines, effect, 0.04 + bias, 0xFF00_FFFF);
        }
    };
    if let Some(support) = s.player.supporting_block() {
        draw(lines, support, s.player.on_pos(s.phys, &world, 0.2), 0.0, 0xFFFF_0000);
    }
    let stale = lines.cache.supported.as_ref().is_none_or(|c| now.duration_since(c.0) > Duration::from_millis(100));
    if stale {
        let area = Aabb { min: cam_box.min.map(|v| v - 16.0), max: cam_box.max.map(|v| v + 16.0) };
        let cam_id = cam.and(s.camera);
        let list = s
            .entities
            .map
            .iter()
            .filter(|(id, e)| Some(**id) != cam_id && e.bounding_box(table, &|_| None, false).1.intersects(&area))
            .map(|(id, _)| *id)
            .collect();
        lines.cache.supported = Some((now, list));
    }
    let list = lines.cache.supported.take().expect("filled above");
    for id in &list.1 {
        let Some(e) = s.entities.map.get(id) else { continue };
        if e.flags() & 0x20 != 0 || !e.on_ground {
            continue;
        }
        let bb = e.bounding_box(table, &|_| None, false).1;
        let Some(support) = rc_physics::support_of(&world, &bb, e.pos) else { continue };
        let effect = rc_physics::below_of(Some(support), e.pos, s.phys, &world, 0.2);
        draw(lines, support, effect, support_bias(*id), 0xFF00_FF00);
    }
    lines.cache.supported = Some(list);
}

/// Water levels within 10 blocks of the player: green shape at 15% up to the fluid height, then
/// the amount in black at the top.
fn water_levels(
    lines: &mut DebugLines,
    gui: &mut crate::gui::Gui,
    queue: &wgpu::Queue,
    s: &Scene<'_>,
    rel: &Rel,
    center: [i32; 3],
) {
    let fluids = &s.phys.fluids;
    let water = |x: i32, y: i32, z: i32| {
        let st = s.state(x, y, z);
        (fluids.fluid(st) == rc_physics::Fluid::Water).then(|| {
            let h = f64::from(fluids.height_at(st, s.state(x, y + 1, z)));
            (h, s.data.state(st).map_or(0, |i| i.fluid_amount))
        })
    };
    let around = |f: &mut dyn FnMut(i32, i32, i32)| {
        for x in center[0] - 10..=center[0] + 10 {
            for y in center[1] - 10..=center[1] + 10 {
                for z in center[2] - 10..=center[2] + 10 {
                    f(x, y, z);
                }
            }
        }
    };
    let k = f64::from(0.01f32);
    around(&mut |x, y, z| {
        if let Some((h, _)) = water(x, y, z) {
            let (fx, fy, fz) = (f64::from(x), f64::from(y), f64::from(z));
            let (a, b) = rel.b(&Aabb { min: [fx + k, fy + k, fz + k], max: [fx + 0.99, fy + h, fz + 0.99] });
            lines.filled_cuboid(a, b, argb(0.15, 0.0, 1.0, 0.0));
        }
    });
    around(&mut |x, y, z| {
        if let Some((h, amount)) = water(x, y, z) {
            let p = rel.p([f64::from(x) + 0.5, f64::from(y) + h, f64::from(z) + 0.5]);
            lines.text(gui, queue, p, &amount.to_string(), 0xFF00_0000, TEXT_SCALE);
        }
    });
}

/// Height of a received heightmap at the local point (x, z): first free block.
fn heightmap_at(longs: &[i64], x: i32, z: i32, height: i32, min_y: i32) -> Option<i32> {
    let bits = (32 - (height as u32 + 1).saturating_sub(1).leading_zeros()).max(1);
    let per = 64 / bits;
    let i = (x.rem_euclid(16) + z.rem_euclid(16) * 16) as u32;
    let long = *longs.get((i / per) as usize)? as u64;
    let v = (long >> ((i % per) * bits)) & ((1u64 << bits) - 1);
    Some(v as i32 + min_y)
}

/// Sections reached by the occlusion walk and in the field of view. Paths: from the centre of the
/// section towards each neighbour the walk came from, tint according to the step (step/50, black at
/// the start). Visibility (sections with geometry): red segment between the middles of two faces
/// that do not communicate, and translucent yellow box (inset of 0.5) if there are any.
fn section_graph(
    lines: &mut DebugLines,
    s: &Scene<'_>,
    rel: &Rel,
    in_view: &dyn Fn([f32; 3], [f32; 3]) -> bool,
    nodes: &[crate::render::OccNode],
    paths: bool,
    visibility: bool,
) {
    const DIRS: [[f32; 3]; 6] =
        [[0.0, -1.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0], [0.0, 0.0, 1.0], [-1.0, 0.0, 0.0], [1.0, 0.0, 0.0]];
    for &(p, sources, step, vis, geometry) in nodes {
        let o = [f64::from(p.0 * 16), f64::from(s.min_y + p.1 * 16), f64::from(p.2 * 16)];
        let (lo, hi) = (rel.p(o), rel.p([o[0] + 16.0, o[1] + 16.0, o[2] + 16.0]));
        if !in_view(lo, hi) {
            continue;
        }
        let at = |d: [f32; 3], k: f32| std::array::from_fn(|i| lo[i] + 8.0 + k * d[i]);
        if paths {
            let rgb = if step == 0 { 0 } else { crate::world_fx::hsv_rgb(f32::from(step) / 50.0, 0.9, 0.9) };
            for (d, dir) in DIRS.iter().enumerate() {
                if sources >> d & 1 == 1 {
                    lines.line(Layer::Depth, at(*dir, 0.0), at(*dir, -16.0), 0xFF00_0000 | rgb, 3.0);
                }
            }
        }
        if visibility && geometry {
            let mut closed = false;
            for (a, da) in DIRS.iter().enumerate() {
                for (b, db) in DIRS.iter().enumerate() {
                    if !rc_mesh::visibility::faces_see(vis, a, b) {
                        closed = true;
                        lines.line(Layer::Depth, at(*da, 8.0), at(*db, 8.0), 0xFFFF_0000, 3.0);
                    }
                }
            }
            if closed {
                lines.filled_cuboid(lo.map(|v| v + 0.5), hi.map(|v| v - 0.5), argb(0.2, 0.9, 0.9, 0.0));
            }
        }
    }
}

/// Heightmaps of the 5 × 5 chunks around the camera: one plaque per column and per
/// known map, offset by 3/32 per map rank.
fn heightmaps(lines: &mut DebugLines, s: &Scene<'_>, rel: &Rel, cam: [i32; 3]) {
    const BOX: f32 = 0.093_75;
    let color = |kind: i32| match kind {
        0 => argb(1.0, 1.0, 1.0, 0.0),
        1 => argb(1.0, 0.0, 0.7, 0.0),
        2 => argb(1.0, 1.0, 0.0, 1.0),
        3 => argb(1.0, 0.0, 0.0, 0.5),
        4 => argb(1.0, 0.0, 0.3, 0.3),
        _ => argb(1.0, 0.0, 0.5, 0.5),
    };
    for cx in -2..=2 {
        for cz in -2..=2 {
            let (bx, bz) = (cam[0] + cx * 16, cam[2] + cz * 16);
            let Some(col) = s.grid.get(bx >> 4, bz >> 4) else { continue };
            let mut maps: Vec<&(i32, Box<[i64]>)> = col.heightmaps.iter().filter(|m| (0..6).contains(&m.0)).collect();
            maps.sort_by_key(|m| m.0);
            for (kind, longs) in maps {
                for rx in 0..16 {
                    for rz in 0..16 {
                        let (xx, zz) = (col.x * 16 + rx, col.z * 16 + rz);
                        let Some(h) = heightmap_at(longs, xx, zz, s.height, s.min_y) else { continue };
                        let y = h as f32 + *kind as f32 * BOX;
                        let (x, z) = (f64::from(xx), f64::from(zz));
                        let a = rel.p([x + 0.25, f64::from(y), z + 0.25]);
                        let b = rel.p([x + 0.75, f64::from(y + BOX), z + 0.75]);
                        lines.filled_cuboid(a, b, color(*kind));
                    }
                }
            }
        }
    }
}

/// Light levels within 10 blocks of the camera: sky light section type large and
/// in red at the centre of the sections, sky (≠ 15) in blue, blocks (≠ 0) in yellow.
#[allow(clippy::too_many_arguments)]
fn light_levels(
    lines: &mut DebugLines,
    gui: &mut crate::gui::Gui,
    queue: &wgpu::Queue,
    s: &Scene<'_>,
    rel: &Rel,
    cam: [i32; 3],
    block_light: bool,
    sky_light: bool,
) {
    let view = s.view();
    let mut seen = hashbrown::HashSet::new();
    for x in cam[0] - 10..=cam[0] + 10 {
        for y in cam[1] - 10..=cam[1] + 10 {
            for z in cam[2] - 10..=cam[2] + 10 {
                let (block, sky) = view.light_pair(x, y, z);
                let sec = [x >> 4, y >> 4, z >> 4];
                if seen.insert(sec) {
                    let c = rel.p(sec.map(|v| f64::from(v * 16 + 8)));
                    let kind = section_type(s, sec);
                    lines.text(gui, queue, c, kind, 0xFFFF_0000, 4.8);
                }
                if sky != 15 && sky_light {
                    let color = lerp_argb(f32::from(sky) / 15.0, 0xFF00_00FF, 0xFF00_FFFF);
                    let p = rel.p([f64::from(x) + 0.5, f64::from(y) + 0.25, f64::from(z) + 0.5]);
                    lines.text(gui, queue, p, &sky.to_string(), color, TEXT_SCALE);
                }
                if block_light && block != 0 {
                    let color = lerp_argb(f32::from(block) / 15.0, 0xFFAA_0000, 0xFFFF_FF00);
                    let p = rel.p([f64::from(x) + 0.5, f64::from(y) + 0.5, f64::from(z) + 0.5]);
                    lines.text(gui, queue, p, &block.to_string(), color, TEXT_SCALE);
                }
            }
        }
    }
}

/// Loaded section that contains blocks (the light engine holds data there).
fn section_has_blocks(s: &Scene<'_>, sec: [i32; 3]) -> bool {
    let Some(col) = s.grid.get(sec[0], sec[2]) else { return false };
    let i = sec[1] - (s.min_y >> 4);
    usize::try_from(i).ok().and_then(|i| col.sections.get(i)).is_some_and(|x| !x.is_empty())
}

/// Light section type: "0" light and data (the section has blocks), "1"
/// light only (one of the 26 neighbours has some), "2" empty.
fn section_type(s: &Scene<'_>, sec: [i32; 3]) -> &'static str {
    if section_has_blocks(s, sec) {
        return "0";
    }
    for dx in -1..=1 {
        for dy in -1..=1 {
            for dz in -1..=1 {
                if (dx, dy, dz) != (0, 0, 0) && section_has_blocks(s, [sec[0] + dx, sec[1] + dy, sec[2] + dz]) {
                    return "1";
                }
            }
        }
    }
    "2"
}

/// Sturdy faces within 6 blocks of the camera: faces of the outline boxes (inflated by 0.002) of the
/// sturdy sides, in red at 50%.
fn solid_faces(lines: &mut DebugLines, s: &Scene<'_>, rel: &Rel, cam: [i32; 3]) {
    // Game order: west, south, east, north, down, up (3D ids).
    const ORDER: [usize; 6] = [4, 3, 5, 2, 0, 1];
    for x in cam[0] - 6..=cam[0] + 6 {
        for y in cam[1] - 6..=cam[1] + 6 {
            for z in cam[2] - 6..=cam[2] + 6 {
                let st = s.state(x, y, z);
                let Some(info) = s.data.state(st) else { continue };
                if s.data.blocks.get(usize::from(info.block)).is_some_and(|b| b.name == "minecraft:air") {
                    continue;
                }
                for b in s.data.shape(info.outline) {
                    let p = [f64::from(x), f64::from(y), f64::from(z)];
                    let min = rel.p(std::array::from_fn(|i| p[i] + b.min[i] - 0.002));
                    let max = rel.p(std::array::from_fn(|i| p[i] + b.max[i] + 0.002));
                    for d in ORDER {
                        if info.sturdy & (1 << d) != 0 {
                            lines.box_face(min, max, d, 0x80FF_0000);
                        }
                    }
                }
            }
        }
    }
}

/// Sky light sections around the player (radius 10): those that have blocks (yellow at
/// 25%), and those where light propagates (dark green at 12.5%); edges and faces of the
/// volumes.
struct SectionShapes {
    min: [i32; 3],
    with_blocks: Vec<bool>,
    lit: Vec<bool>,
}

const SECTION_SIZE: i32 = 21;

impl SectionShapes {
    fn new(s: &Scene<'_>, center: [i32; 3]) -> Self {
        let n = SECTION_SIZE as usize;
        let min = center.map(|v| v - 10);
        let (mut with_blocks, mut lit) = (vec![false; n * n * n], vec![false; n * n * n]);
        for z in 0..SECTION_SIZE {
            for y in 0..SECTION_SIZE {
                for x in 0..SECTION_SIZE {
                    let i = Self::index(x, y, z);
                    match section_type(s, [min[0] + x, min[1] + y, min[2] + z]) {
                        "0" => (with_blocks[i], lit[i]) = (true, true),
                        "1" => lit[i] = true,
                        _ => {}
                    }
                }
            }
        }
        SectionShapes { min, with_blocks, lit }
    }

    fn index(x: i32, y: i32, z: i32) -> usize {
        ((z * SECTION_SIZE + y) * SECTION_SIZE + x) as usize
    }

    fn emit(&self, lines: &mut DebugLines, rel: &Rel) {
        let light_and_blocks = argb(0.25, 1.0, 1.0, 0.0);
        let light_only = argb(0.125, 0.25, 0.125, 0.0);
        let corner = |c: [i32; 3]| rel.p(std::array::from_fn(|i| f64::from((self.min[i] + c[i]) * 16)));
        for (shape, color) in [(&self.with_blocks, light_and_blocks), (&self.lit, light_only)] {
            for_all_edges(shape, &mut |a, b| lines.line(Layer::Depth, corner(a), corner(b), color | 0xFF00_0000, 3.0));
        }
        for (shape, color) in [(&self.with_blocks, light_and_blocks), (&self.lit, light_only)] {
            for_all_faces(shape, &mut |d, c| {
                let a = corner(c);
                lines.box_face(a, [a[0] + 16.0, a[1] + 16.0, a[2] + 16.0], d, color);
            });
        }
    }
}

/// Filled cell of a 21³ volume (outside the volume: empty).
fn full(shape: &[bool], c: [i32; 3]) -> bool {
    c.iter().all(|v| (0..SECTION_SIZE).contains(v)) && shape[SectionShapes::index(c[0], c[1], c[2])]
}

/// Edges of a volume of cells, merged along their axis: an edge separates, among the four
/// cells around it, only one or three filled ones, or two diagonal.
fn for_all_edges(shape: &[bool], f: &mut dyn FnMut([i32; 3], [i32; 3])) {
    for axis in 0..3 {
        let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
        for a in 0..=SECTION_SIZE {
            for b in 0..=SECTION_SIZE {
                let mut start = None;
                for c in 0..=SECTION_SIZE {
                    let mut count = 0;
                    let mut odd = 0;
                    for da in 0..=1 {
                        for db in 0..=1 {
                            let mut p = [0; 3];
                            p[u] = a + da - 1;
                            p[v] = b + db - 1;
                            p[axis] = c;
                            if full(shape, p) {
                                count += 1;
                                odd ^= da ^ db;
                            }
                        }
                    }
                    let edge = count == 1 || count == 3 || (count == 2 && odd & 1 == 0);
                    let at = |c: i32| {
                        let mut p = [0; 3];
                        p[u] = a;
                        p[v] = b;
                        p[axis] = c;
                        p
                    };
                    if edge {
                        start.get_or_insert(c);
                    } else if let Some(s0) = start.take() {
                        f(at(s0), at(c));
                    }
                }
            }
        }
    }
}

/// Outer faces of a volume of cells: `f(3D direction, cell)`.
fn for_all_faces(shape: &[bool], f: &mut dyn FnMut(usize, [i32; 3])) {
    // x axis: west 4 / east 5; y: down 0 / up 1; z: north 2 / south 3.
    const DIRS: [(usize, usize); 3] = [(4, 5), (0, 1), (2, 3)];
    for (axis, &(neg, pos)) in DIRS.iter().enumerate() {
        let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
        for a in 0..SECTION_SIZE {
            for b in 0..SECTION_SIZE {
                let mut last = false;
                for c in 0..=SECTION_SIZE {
                    let mut p = [0; 3];
                    p[u] = a;
                    p[v] = b;
                    p[axis] = c;
                    let now = c != SECTION_SIZE && full(shape, p);
                    if !last && now {
                        f(neg, p);
                    }
                    if last && !now {
                        p[axis] = c - 1;
                        f(pos, p);
                    }
                    last = now;
                }
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn arrow_tip_spreads_by_its_recoil() {
        // Gaze arrow of 2 blocks towards +Z: tip set back by 0.2 and spread by 0.2.
        let tips = arrow_tips([0.0, 0.0, 0.0], [0.0, 0.0, 2.0]);
        for t in tips {
            assert!((t[2] - 1.8).abs() < 1e-5, "{t:?}");
            let spread = (t[0] * t[0] + t[1] * t[1]).sqrt();
            assert!((spread - 0.2).abs() < 1e-5, "{t:?}");
        }
        // Short: minimal setback 0.1; long: capped at 1.
        let short = arrow_tips([0.0; 3], [0.5, 0.0, 0.0]);
        assert!((short[0][0] - 0.4).abs() < 1e-5);
        let long = arrow_tips([0.0; 3], [30.0, 0.0, 0.0]);
        assert!((long[0][0] - 29.0).abs() < 1e-4);
    }

    #[test]
    fn colors_like_the_game() {
        assert_eq!(argb(1.0, 0.25, 0.25, 1.0), 0xFF3F_3FFF);
        assert_eq!(argb(0.5, 1.0, 0.0, 0.0), 0x7FFF_0000);
        assert_eq!(argb(1.0, 0.25, 1.0, 0.0), 0xFF3F_FF00);
        assert_eq!(argb(0.15, 0.0, 1.0, 0.0), 0x2600_FF00);
        assert_eq!(lerp_argb(0.0, 0xFF00_00FF, 0xFF00_FFFF), 0xFF00_00FF);
        assert_eq!(lerp_argb(1.0, 0xFFAA_0000, 0xFFFF_FF00), 0xFFFF_FF00);
        // 7/15 between 0xAA and 0xFF: 170 + ⌊0.4667 × 85⌋ = 209.
        assert_eq!(lerp_argb(7.0 / 15.0, 0xFFAA_0000, 0xFFFF_FF00) >> 16 & 0xff, 209);
    }

    #[test]
    fn support_bias_hashes_the_decimal_text() {
        // "5.132453657": Java hash of the characters, remainder of the division by 1000.
        let text = "5.132453657";
        let mut h: i32 = 0;
        for c in text.chars() {
            h = h.wrapping_mul(31).wrapping_add(c as i32);
        }
        assert_eq!(support_bias(5), 0.02 * f64::from(h % 1000) / 1000.0);
        assert_eq!(format!("{}", 5.0f64 + 0.132_453_657), text);
    }

    #[test]
    fn single_cell_has_twelve_edges_and_six_faces() {
        let n = SECTION_SIZE as usize;
        let mut shape = vec![false; n * n * n];
        shape[SectionShapes::index(3, 4, 5)] = true;
        let mut edges = Vec::new();
        for_all_edges(&shape, &mut |a, b| edges.push((a, b)));
        assert_eq!(edges.len(), 12);
        let mut faces = Vec::new();
        for_all_faces(&shape, &mut |d, c| faces.push((d, c)));
        assert_eq!(faces.len(), 6);
        assert!(faces.iter().all(|f| f.1 == [3, 4, 5]));
        // Two aligned cells: edges of the axis merged (12 in total), 10 faces.
        shape[SectionShapes::index(4, 4, 5)] = true;
        edges.clear();
        for_all_edges(&shape, &mut |a, b| edges.push((a, b)));
        assert_eq!(edges.len(), 12);
        faces.clear();
        for_all_faces(&shape, &mut |d, c| faces.push((d, c)));
        assert_eq!(faces.len(), 10);
    }

    #[test]
    fn heightmap_decoding() {
        // Height 384: 9 bits per value, 7 per long; column (1, 0) = 70 → −64 + 70 = 6.
        let longs = [70i64 << 9];
        assert_eq!(heightmap_at(&longs, 1, 0, 384, -64), Some(6));
    }

    #[test]
    fn dragon_parts_at_origin_before_any_tick() {
        let e = test_entity("ender_dragon");
        let parts = dragon_parts(&e, 0.5);
        assert!(parts.iter().all(|p| p.0 == [0.0; 3]));
        assert_eq!(parts[2].1, 5.0);
    }

    #[test]
    fn dragon_parts_follow_the_flight_history() {
        let mut e = test_entity("ender_dragon");
        e.pos = [10.0, 70.0, -4.0];
        e.pos_o = e.pos;
        // Horizontal flight, yaw 0: constant history.
        e.history = vec![[70.0, 0.0, 0.0]; RING];
        e.extra[0] = 0.0;
        let parts = dragon_parts(&e, 1.0);
        // Body: (sin 0 × 0.5; 0; −cos 0 × 0.5); head 6.5 in front (−z), same height.
        assert!((parts[2].0[2] - (-4.5)).abs() < 1e-6, "{:?}", parts[2].0);
        assert!((parts[0].0[2] - (-10.5)).abs() < 1e-6 && (parts[0].0[1] - 70.0).abs() < 1e-6);
        // Wings at ±4.5 in x, 2 higher; first tail behind (+z): 1.5 + 2.
        assert!((parts[6].0[0] - 14.5).abs() < 1e-6 && (parts[6].0[1] - 72.0).abs() < 1e-6);
        assert!((parts[3].0[2] - (-0.5)).abs() < 1e-6 && (parts[3].0[1] - 71.5).abs() < 1e-6);
    }

    /// Test entity of a given kind (type id 0), spawned at the origin.
    pub(crate) fn test_entity(kind: &str) -> Entity {
        let mut all = crate::entities::Entities { type_names: vec![format!("minecraft:{kind}")], ..Default::default() };
        let add = rc_proto_v777::entity::AddEntity {
            id: 1,
            uuid: 1,
            kind: 0,
            pos: [0.0; 3],
            velocity: [0.0; 3],
            pitch: 0.0,
            yaw: 0.0,
            head_yaw: 0.0,
            data: 0,
        };
        all.apply(rc_client::EntityEvent::Add(Box::new(add)));
        all.map.remove(&1).expect("spawned")
    }
}
