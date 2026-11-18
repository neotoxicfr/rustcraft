//! Sign text (`docs/specs/block-entities.md` § 3): lines laid out by the interface
//! (same glyphs and styles), placed in the frame of each face. One matrix per face; the
//! layout is kept per face as long as the sign's data does not change. Back faces are culled
//! by the GPU, like the game's polygon-offset text pipeline.
//!
//! **Glyph pool** (`RUSTCRAFT_OPT_SIGN_TEXT_GLYPH_POOL`, default 1). Old path (`=0`): one
//! 52-byte instance per glyph (face, light, glyph), copied and sent on every frame. New path:
//! the glyphs of each face layout live in the shared glyph pool (`glyph_pool.rs`), sent once;
//! per frame, a 16-byte reference per face and per pass (matrix, light, first glyph, count),
//! outline references first, text after, each in the order of the faces (farthest first).
//!
//! **Back faces left out** (`RUSTCRAFT_OPT_SIGN_BACKFACE_SKIP`, default 1): a face whose text
//! surely turns its back to the camera is not sent (`BackFaceTest`); its layout is kept.
//!
//! **Unchanged writes skipped** (`RUSTCRAFT_OPT_TEXT_SKIP_UNCHANGED_WRITES`, default 1, new
//! path): matrices and references identical to those already on the GPU are not sent again.

use hashbrown::HashMap;
use hashbrown::hash_map::Entry;

use glam::Mat4;

use crate::block_entities::{BlockEntities, Data, SignFace};
use crate::glyph_pool::{
    BackFaceTest, BufferPlan, DrawSplitter, FrameBuffer, GlyphPool, PoolUpload, RefDraw, draw_refs, glyph_radius,
    skip_unchanged_writes_on, write_growing,
};
use crate::gui::Gui;
use crate::nametag::{Glyph, glyphs};
use crate::render::upload::Uploader;
use crate::small_layers::SmallLayerOptions;

/// Sign face visible this frame (prepared by the block-entity renderer).
pub struct SignDraw {
    pub pos: [i32; 3],
    /// Matrices of the front and back faces (font pixels → camera frame).
    pub faces: [Mat4; 2],
    /// Block light (block × 16 | sky × 16 << 8).
    pub light: u32,
    pub hanging: bool,
    /// Distance² between the camera and the block center.
    pub d2: f64,
}

/// Old path: one instance per glyph.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct TextInstance {
    face: u32,
    light: u32,
    glyph: Glyph,
}

/// New path: a face in a pass, `count` glyphs of the pool from `first`, placed by matrix `face`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct FaceRef {
    face: u32,
    light: u32,
    first: u32,
    count: u32,
}

/// Passes, drawn in this order: outline (normal mode), text (polygon offset).
const OUTLINE: usize = 0;
const TEXT: usize = 1;

/// Light of glowing text: full block and sky light.
const FULL_BRIGHT: u32 = 0xf0f0;

/// Glyphs kept in the shared pool (`RUSTCRAFT_OPT_SIGN_TEXT_GLYPH_POOL`, default 1), read once.
fn glyph_pool_on() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_SIGN_TEXT_GLYPH_POOL")
}

/// Faces turned away left out (`RUSTCRAFT_OPT_SIGN_BACKFACE_SKIP`, default 1), read once.
fn backface_skip_on() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_SIGN_BACKFACE_SKIP")
}

/// Text color per dye (§ 3.2).
pub(crate) const TEXT_COLORS: [u32; 16] = [
    0xFFFFFF, 0xFF681F, 0xFF00FF, 0x9AC0CD, 0xFFFF00, 0xBFFF00, 0xFF69B4, 0x808080, 0xD3D3D3, 0x00FFFF, 0xA020F0,
    0x0000FF, 0x8B4513, 0x00FF00, 0xFF0000, 0x000000,
];

/// Dark color: cream for glowing black, otherwise 0.4 × the color (per channel, integer).
pub(crate) fn dark_color(dye: u8, glowing: bool) -> u32 {
    if dye == 15 && glowing {
        return 0xF0EBCC;
    }
    let color = TEXT_COLORS[usize::from(dye & 15)];
    let channel = |shift: u32| (((color >> shift) & 0xff) as f64 * 0.4) as u32;
    channel(16) << 16 | channel(8) << 8 | channel(0)
}

/// Kept layout of a face.
struct FaceLayout {
    rev: u32,
    /// Text, outline (8 offset copies), already placed.
    text: Vec<Glyph>,
    outline: Vec<Glyph>,
    /// Distance from the face origin to the farthest glyph corner (font pixels).
    radius: f32,
    /// First glyphs in the pool (new path).
    text_first: u32,
    outline_first: u32,
    frame: u64,
}

impl FaceLayout {
    fn new(rev: u32, (text, outline): (Vec<Glyph>, Vec<Glyph>)) -> Self {
        let radius = glyph_radius(&text).max(glyph_radius(&outline));
        FaceLayout { rev, text, outline, radius, text_first: 0, outline_first: 0, frame: 0 }
    }

    fn add_to(&mut self, pool: &mut GlyphPool) {
        self.text_first = pool.push(&self.text);
        self.outline_first = pool.push(&self.outline);
    }

    fn pooled(&self) -> usize {
        self.text.len() + self.outline.len()
    }
}

/// A face to draw this frame: cache key (position, side), sign revision, matrix, light,
/// outline drawn.
struct FaceDraw {
    key: ([i32; 3], u8),
    rev: u32,
    matrix: Mat4,
    light: u32,
    outline: bool,
}

/// What to send at the end of the frame (new path).
struct SignUpload {
    pool: PoolUpload,
    faces: BufferPlan,
    refs: BufferPlan,
}

/// CPU state of the frame (no GPU, for the proofs).
struct SignLists {
    use_pool: bool,
    skip_back_faces: bool,
    skip_unchanged: bool,
    cache: HashMap<([i32; 3], u8), FaceLayout>,
    frame: u64,
    faces: Vec<[[f32; 4]; 4]>,
    /// Old path: instances of the frame.
    outline_inst: Vec<TextInstance>,
    text_inst: Vec<TextInstance>,
    /// New path: references of each pass, then all of them as sent (outline then text),
    /// their draws and the range of draws of each pass.
    pool: GlyphPool,
    pass_refs: [Vec<FaceRef>; 2],
    refs: Vec<FaceRef>,
    draws: Vec<RefDraw>,
    passes: [(u32, u32); 2],
    face_buffer: FrameBuffer,
    ref_buffer: FrameBuffer,
    /// Test sabotage: back-face test reversed (faces turned toward the camera left out).
    #[cfg(test)]
    reversed_backface_test: bool,
}

impl SignLists {
    fn new(use_pool: bool, skip_back_faces: bool, skip_unchanged: bool) -> Self {
        SignLists {
            use_pool,
            skip_back_faces,
            skip_unchanged,
            cache: HashMap::new(),
            frame: 0,
            faces: Vec::new(),
            outline_inst: Vec::new(),
            text_inst: Vec::new(),
            pool: GlyphPool::default(),
            pass_refs: [Vec::new(), Vec::new()],
            refs: Vec::new(),
            draws: Vec::new(),
            passes: [(0, 0); 2],
            face_buffer: FrameBuffer::default(),
            ref_buffer: FrameBuffer::default(),
            #[cfg(test)]
            reversed_backface_test: false,
        }
    }

    fn begin(&mut self) {
        self.frame += 1;
        self.faces.clear();
        self.outline_inst.clear();
        self.text_inst.clear();
        self.pass_refs.iter_mut().for_each(Vec::clear);
        // No reference of the frame exists yet: the pool can be rebuilt.
        if self.use_pool && self.pool.compact_if_worth() {
            for c in self.cache.values_mut() {
                c.add_to(&mut self.pool);
            }
        }
    }

    /// Layout of `face`, made by `layout` (text, outline) only if it is not kept.
    fn kept_layout(&mut self, face: &FaceDraw, layout: impl FnOnce() -> (Vec<Glyph>, Vec<Glyph>)) -> &mut FaceLayout {
        let entry = match self.cache.entry(face.key) {
            Entry::Occupied(e) if e.get().rev == face.rev => return e.into_mut(),
            entry => entry,
        };
        let mut fresh = FaceLayout::new(face.rev, layout());
        if self.use_pool {
            fresh.add_to(&mut self.pool);
        }
        match entry {
            Entry::Occupied(mut e) => {
                self.pool.forget(e.get().pooled());
                e.insert(fresh);
                e.into_mut()
            }
            Entry::Vacant(e) => e.insert(fresh),
        }
    }

    /// A face of the frame; `view`: camera for the back-face test.
    fn push(&mut self, face: &FaceDraw, view: &BackFaceTest, layout: impl FnOnce() -> (Vec<Glyph>, Vec<Glyph>)) {
        let frame = self.frame;
        let c = self.kept_layout(face, layout);
        c.frame = frame;
        if c.text.is_empty() {
            return;
        }
        let (text, outline) = (FaceRef::of(&c.text, c.text_first), FaceRef::of(&c.outline, c.outline_first));
        let radius = c.radius;
        if self.turned_away(&face.matrix, radius, view) {
            return;
        }
        let index = self.faces.len() as u32;
        self.faces.push(face.matrix.to_cols_array_2d());
        if self.use_pool {
            let place = |r: FaceRef| FaceRef { face: index, light: face.light, ..r };
            if face.outline {
                self.pass_refs[OUTLINE].push(place(outline));
            }
            self.pass_refs[TEXT].push(place(text));
            return;
        }
        let c = &self.cache[&face.key];
        let instance = |&glyph| TextInstance { face: index, light: face.light, glyph };
        // Outline: black, or camera within 16 blocks.
        if face.outline {
            self.outline_inst.extend(c.outline.iter().map(instance));
        }
        self.text_inst.extend(c.text.iter().map(instance));
    }

    /// True when back faces are left out and the face `matrix` (glyphs within `radius` font
    /// pixels) surely turns its back to `view`; its layout stays kept.
    fn turned_away(&self, matrix: &Mat4, radius: f32, view: &BackFaceTest) -> bool {
        if !self.skip_back_faces {
            return false;
        }
        // Test sabotage: the mirrored font frame turns the test around.
        #[cfg(test)]
        if self.reversed_backface_test {
            return view.faces_away(&(*matrix * Mat4::from_scale(glam::Vec3::new(-1.0, 1.0, 1.0))), radius);
        }
        view.faces_away(matrix, radius)
    }

    /// End of the frame: unseen layouts forgotten; new path: references and draws of the
    /// passes, and what to send (nothing if nothing is drawn).
    fn finish(&mut self) -> Option<SignUpload> {
        let (frame, pool) = (self.frame, &mut self.pool);
        self.cache.retain(|_, c| {
            let keep = c.frame == frame;
            if !keep {
                pool.forget(c.pooled());
            }
            keep
        });
        if !self.use_pool {
            return None;
        }
        self.refs.clear();
        self.draws.clear();
        for p in [OUTLINE, TEXT] {
            let at = self.draws.len() as u32;
            let mut draws = DrawSplitter::new(self.refs.len() as u32, true);
            for r in self.pass_refs[p].iter().filter(|r| r.count > 0) {
                draws.add(r.count, &mut self.draws);
                self.refs.push(*r);
            }
            draws.finish(&mut self.draws);
            self.passes[p] = (at, self.draws.len() as u32);
        }
        if self.refs.is_empty() {
            return None;
        }
        Some(SignUpload {
            pool: self.pool.plan_upload(),
            faces: self.face_buffer.plan(bytemuck::cast_slice(&self.faces), self.skip_unchanged),
            refs: self.ref_buffer.plan(bytemuck::cast_slice(&self.refs), self.skip_unchanged),
        })
    }

    fn pass_draws(&self, pass: usize) -> &[RefDraw] {
        let (a, b) = self.passes[pass];
        &self.draws[a as usize..b as usize]
    }
}

impl FaceRef {
    /// Glyphs of a layout in the pool (face and light set when placed).
    fn of(glyphs: &[Glyph], first: u32) -> Self {
        FaceRef { face: 0, light: 0, first, count: glyphs.len() as u32 }
    }
}

pub struct BlockText {
    /// Text (polygon offset toward the camera) and outline (normal mode, drawn first).
    pipeline: wgpu::RenderPipeline,
    outline_pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    bind: Option<wgpu::BindGroup>,
    /// Glyph sheet generation and face buffer generation bound.
    bound: (u32, u64),
    face_gen: u64,
    /// Old path: face matrices and instances (buffer, capacity), instances sent (outline, text).
    face_buf: Option<(wgpu::Buffer, u64)>,
    inst_buf: Option<(wgpu::Buffer, u64)>,
    counts: (u32, u32),
    /// New path: glyph pool, face matrices, references.
    pool_buf: Option<wgpu::Buffer>,
    face_matrices: Option<wgpu::Buffer>,
    ref_buf: Option<wgpu::Buffer>,
    lists: SignLists,
}

const INSTANCE_ATTRS: [wgpu::VertexAttribute; 7] = wgpu::vertex_attr_array![
    0 => Uint32, 1 => Uint32, 2 => Float32x4, 3 => Float32,
    4 => Float32x4, 5 => Unorm8x4, 6 => Uint32
];
const REF_ATTRS: [wgpu::VertexAttribute; 4] =
    wgpu::vertex_attr_array![0 => Uint32, 1 => Uint32, 2 => Uint32, 3 => Uint32];

impl BlockText {
    pub fn new(device: &wgpu::Device, camera_layout: &wgpu::BindGroupLayout, format: wgpu::TextureFormat) -> Self {
        Self::with_mode(
            device,
            camera_layout,
            format,
            SignLists::new(glyph_pool_on(), backface_skip_on(), skip_unchanged_writes_on()),
            SmallLayerOptions::from_switches(),
        )
    }

    fn with_mode(
        device: &wgpu::Device,
        camera_layout: &wgpu::BindGroupLayout,
        format: wgpu::TextureFormat,
        lists: SignLists,
        shader_options: SmallLayerOptions,
    ) -> Self {
        let use_pool = lists.use_pool;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sign text"),
            source: wgpu::ShaderSource::Wgsl(shader_options.shader_source(include_str!("block_text.wgsl")).into()),
        });
        let storage = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let mut entries = vec![
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
            storage(2),
        ];
        if use_pool {
            entries.push(storage(3));
        }
        let layout = device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("sign text"), entries: &entries });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sign text"),
            bind_group_layouts: &[Some(camera_layout), Some(&layout)],
            immediate_size: 0,
        });
        let (entry, stride, attributes): (_, usize, &[_]) = if use_pool {
            ("vs_ref", size_of::<FaceRef>(), &REF_ATTRS)
        } else {
            ("vs", size_of::<TextInstance>(), &INSTANCE_ATTRS)
        };
        let make = |label, bias| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: stride as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes,
                    })],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                // Back faces culled, like the game's text pipelines: the text of a face never
                // shows mirrored from behind (past the board's edge, or from inside the block).
                primitive: wgpu::PrimitiveState { cull_mode: Some(wgpu::Face::Back), ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(crate::render::DEPTH_NEARER),
                    stencil: Default::default(),
                    bias,
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        // Polygon offset toward the camera (slope 1, constant 10) for the text only: in
        // the game, the glowing outline is drawn in normal mode, first, then the text in
        // polygon offset mode.
        let bias = crate::render::depth_bias(10, 1.0);
        let pipeline = make("sign text", bias);
        let outline_pipeline = make("sign text outline", wgpu::DepthBiasState::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        BlockText {
            pipeline,
            outline_pipeline,
            layout,
            sampler,
            bind: None,
            bound: (u32::MAX, u64::MAX),
            face_gen: 0,
            face_buf: None,
            inst_buf: None,
            counts: (0, 0),
            pool_buf: None,
            face_matrices: None,
            ref_buf: None,
            lists,
        }
    }

    /// Faces of the frame, text and outline of the glowing text; `view`: camera, for the
    /// faces turned away.
    pub fn prepare(
        &mut self,
        gui: &mut Gui,
        queue: &wgpu::Queue,
        draws: &[SignDraw],
        bes: &BlockEntities,
        view: &BackFaceTest,
    ) {
        self.lists.begin();
        for d in draws {
            let Some(e) = bes.get(d.pos) else { continue };
            let Data::Sign(sides) = &e.data else { continue };
            for (side, face) in sides.iter().enumerate() {
                if face.messages.iter().all(Option::is_none) {
                    continue;
                }
                let draw = FaceDraw {
                    key: (d.pos, side as u8),
                    rev: e.rev,
                    matrix: d.faces[side],
                    light: if face.glowing { FULL_BRIGHT } else { d.light },
                    // Outline: black, or camera within 16 blocks.
                    outline: face.glowing && (face.color == 15 || d.d2 < 256.0),
                };
                self.lists.push(&draw, view, || build(gui, queue, face, d.hanging));
            }
        }
    }

    /// Upload of the frame; `glyph_gen` and `view`: generation and view of the glyph sheet.
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        up: &mut Uploader,
        glyph_gen: u32,
        view: impl FnOnce() -> wgpu::TextureView,
    ) {
        let plan = self.lists.finish();
        if !self.lists.use_pool {
            if self.upload_instances(device, queue) {
                self.bind_group(device, glyph_gen, view);
            }
            return;
        }
        let Some(plan) = plan else { return };
        let l = &self.lists;
        let pool = l.pool.upload(plan.pool, device, queue, up, &mut self.pool_buf, "sign text (glyphs)");
        let storage = wgpu::BufferUsages::STORAGE;
        let faces = bytemuck::cast_slice(&l.faces);
        let matrices =
            FrameBuffer::apply(plan.faces, faces, device, queue, up, &mut self.face_matrices, storage, "sign faces");
        let refs = bytemuck::cast_slice(&l.refs);
        let vertex = wgpu::BufferUsages::VERTEX;
        FrameBuffer::apply(plan.refs, refs, device, queue, up, &mut self.ref_buf, vertex, "sign text references");
        if pool || matrices {
            self.face_gen += 1;
        }
        self.bind_group(device, glyph_gen, view);
    }

    /// Old path: instances and matrices rewritten; false if nothing is drawn.
    fn upload_instances(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> bool {
        let l = &mut self.lists;
        self.counts = (l.outline_inst.len() as u32, l.text_inst.len() as u32);
        if self.counts.1 == 0 {
            return false;
        }
        // Outline at the head of the buffer, text after.
        let mut all = std::mem::take(&mut l.outline_inst);
        all.extend_from_slice(&l.text_inst);
        let faces = bytemuck::cast_slice(&l.faces);
        if write_growing(device, queue, &mut self.face_buf, faces, wgpu::BufferUsages::STORAGE, "sign faces") {
            self.face_gen += 1;
        }
        write_growing(
            device,
            queue,
            &mut self.inst_buf,
            bytemuck::cast_slice(&all),
            wgpu::BufferUsages::VERTEX,
            "sign text",
        );
        all.clear();
        l.outline_inst = all;
        true
    }

    fn bind_group(&mut self, device: &wgpu::Device, glyph_gen: u32, view: impl FnOnce() -> wgpu::TextureView) {
        if self.bind.is_some() && self.bound == (glyph_gen, self.face_gen) {
            return;
        }
        let (faces, pool) = if self.lists.use_pool {
            (self.face_matrices.as_ref(), self.pool_buf.as_ref())
        } else {
            (self.face_buf.as_ref().map(|(b, _)| b), None)
        };
        let view = view();
        let mut entries = vec![
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: faces.expect("face matrices uploaded before binding").as_entire_binding(),
            },
        ];
        if let Some(pool) = pool {
            entries.push(wgpu::BindGroupEntry { binding: 3, resource: pool.as_entire_binding() });
        }
        self.bind = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sign text"),
            layout: &self.layout,
            entries: &entries,
        }));
        self.bound = (glyph_gen, self.face_gen);
    }

    /// Drawing with the entities (group 0 = camera already bound).
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.lists.use_pool {
            self.draw_refs(pass);
            return;
        }
        let (Some(bind), Some((buf, _))) = (&self.bind, &self.inst_buf) else { return };
        let (outline, text) = self.counts;
        if text == 0 {
            return;
        }
        pass.set_bind_group(1, bind, &[]);
        pass.set_vertex_buffer(0, buf.slice(..));
        if outline > 0 {
            pass.set_pipeline(&self.outline_pipeline);
            pass.draw(0..6, 0..outline);
        }
        pass.set_pipeline(&self.pipeline);
        pass.draw(0..6, outline..outline + text);
    }

    fn draw_refs(&self, pass: &mut wgpu::RenderPass<'_>) {
        let (Some(bind), Some(buf)) = (&self.bind, &self.ref_buf) else { return };
        let (outline, text) = (self.lists.pass_draws(OUTLINE), self.lists.pass_draws(TEXT));
        if text.is_empty() {
            return;
        }
        pass.set_bind_group(1, bind, &[]);
        pass.set_vertex_buffer(0, buf.slice(..));
        if !outline.is_empty() {
            pass.set_pipeline(&self.outline_pipeline);
            draw_refs(pass, outline);
        }
        pass.set_pipeline(&self.pipeline);
        draw_refs(pass, text);
    }
}

/// Layout of a face (§ 3.2): first line of the split of each message, centered,
/// line `i` at `y = i·h − 2h`; outline = 8 unstyled copies, offset by one pixel.
fn build(gui: &mut Gui, queue: &wgpu::Queue, face: &SignFace, hanging: bool) -> (Vec<Glyph>, Vec<Glyph>) {
    let (max, h) = if hanging { (60.0, 9.0) } else { (90.0, 10.0) };
    let base = TEXT_COLORS[usize::from(face.color & 15)];
    let dark = dark_color(face.color, face.glowing);
    let color = 0xff00_0000 | if face.glowing { base } else { dark };
    let (mut text, mut outline) = (Vec::new(), Vec::new());
    for (i, m) in face.messages.iter().enumerate() {
        let Some(n) = m else { continue };
        let Some(comp) = rc_text::from_nbt(n) else { continue };
        let chars = rc_text::styled_chars(&rc_text::flatten(&comp, &gui.lang));
        let Some(line) = rc_text::wrap(&*gui, &chars, max).into_iter().next() else { continue };
        if line.is_empty() {
            continue;
        }
        let w = rc_text::width(&*gui, &line);
        // Centering with integer division (the template centers at half-pixel).
        let dx = (-(w / 2)) as f32 + w as f32 / 2.0;
        let dy = i as f32 * h - 2.0 * h;
        let shift = |g: &mut Glyph, ox: f32, oy: f32| {
            g.rect[0] += dx + ox;
            g.rect[1] += dx + ox;
            g.rect[2] += dy + oy;
            g.rect[3] += dy + oy;
        };
        for mut g in glyphs(&gui.text_quads(queue, &line, color, 0)) {
            shift(&mut g, 0.0, 0.0);
            text.push(g);
        }
        if face.glowing {
            let plain: Vec<rc_text::StyledChar> = line
                .iter()
                .map(|c| {
                    let mut c = c.clone();
                    c.style.color = None;
                    c.style.underlined = false;
                    c.style.strikethrough = false;
                    c
                })
                .collect();
            let quads = glyphs(&gui.text_quads(queue, &plain, 0xff00_0000 | dark, 0));
            for (ox, oy) in
                [(-1.0, -1.0), (0.0, -1.0), (1.0, -1.0), (-1.0, 0.0), (1.0, 0.0), (-1.0, 1.0), (0.0, 1.0), (1.0, 1.0)]
            {
                for &q in &quads {
                    let mut g = q;
                    shift(&mut g, ox, oy);
                    outline.push(g);
                }
            }
        }
    }
    (text, outline)
}

#[cfg(test)]
mod tests;
