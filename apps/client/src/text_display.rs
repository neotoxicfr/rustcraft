//! Text displays (`docs/specs/entities-objects.md` § 13.8): component split to the line width,
//! aligned lines (left, right, center), solid background, drop shadow, opacity. One matrix
//! per layer (background, shadow, text); layout kept per entity as long as the text, the width
//! and the flags do not change (except obfuscated text).
//!
//! Game modes: "polygon offset" (depth tested and written, lightmap, fog)
//! or "see-through" (flag 2: no depth, color only), back faces culled.
//!
//! **Glyph pool** (`RUSTCRAFT_OPT_DISPLAY_TEXT_GLYPH_POOL`, default 1). Old path (`=0`): one
//! 52-byte instance per glyph, its alpha scaled by the opacity of the frame on the CPU, copied
//! and sent on every frame. New path: the glyphs of each layout (and its background fill) live
//! in the shared glyph pool (`glyph_pool.rs`), sent once; per frame, a 24-byte reference per
//! layer (matrix, light, first glyph, count, opacity, background colour), the shader scaling the
//! alpha with the same integer rounding. In improved transparency, the opaque glyphs come from
//! the solids-first copy of a layer (only when the layer is fully opaque this frame).
//! Matrices and references identical to those already on the GPU are not sent again
//! (`RUSTCRAFT_OPT_TEXT_SKIP_UNCHANGED_WRITES`).
//!
//! **Back faces left out** (`RUSTCRAFT_OPT_DISPLAY_BACKFACE_SKIP`, default 1): a display (not a
//! held map name) that surely turns its back to the camera is not sent; the GPU would cull it.

use hashbrown::HashMap;

use glam::{Mat4, Vec3};

use crate::entities::Entities;
use crate::entity_render::{DisplaySub, TextDraw};
use crate::glyph_pool::{
    BackFaceTest, BufferPlan, DrawSplitter, FrameBuffer, GlyphPool, PoolUpload, PooledRun, RefDraw, draw_refs,
    skip_unchanged_writes_on, write_growing,
};
use crate::gui::Gui;
use crate::nametag::{Glyph, glyphs};
use crate::render::upload::Uploader;
use crate::small_layers::SmallLayerOptions;

/// Old path: one instance per glyph.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct TextInstance {
    face: u32,
    light: u32,
    glyph: Glyph,
}

/// New path: a layer in a pass, `count` glyphs of the pool from `first`, placed by matrix
/// `layer`; glyph alpha × `opacity` / 255 (bytes, integer division); `fill` ≠ 0 replaces the
/// glyph colour (background, chosen per frame).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct LayerRef {
    layer: u32,
    light: u32,
    first: u32,
    count: u32,
    opacity: u32,
    fill: u32,
}

/// Polygon-offset layer of the new path, with its solids-first copy (`sorted`, `solids`
/// opaque glyphs this frame) for improved transparency.
#[derive(Clone, Copy)]
struct OffsetLayer {
    layer: LayerRef,
    sorted: u32,
    solids: u32,
}

/// Passes of the new path: polygon offset, see-through, held map names; in improved
/// transparency the polygon-offset one is cut into opaque glyphs (with the solids) and the rest.
const OFFSET: usize = 0;
const SEE: usize = 1;
const HAND: usize = 2;
const SOLID: usize = 3;
const REST: usize = 4;

/// Light marked "see-through" for the shader (neither lightmap nor fog).
const SEE_THROUGH: u32 = 0x8000_0000;

/// Depths of the layers in font pixels (toward the front of the text): background, shadow, text
/// above a shadow.
const BACKGROUND_DEPTH: f32 = -0.01;
const SHADOW_FRONT: f32 = 0.03;

/// Glyphs kept in the shared pool (`RUSTCRAFT_OPT_DISPLAY_TEXT_GLYPH_POOL`, default 1), read once.
fn glyph_pool_on() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_DISPLAY_TEXT_GLYPH_POOL")
}

/// Displays turned away left out (`RUSTCRAFT_OPT_DISPLAY_BACKFACE_SKIP`, default 1), read once.
fn backface_skip_on() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_DISPLAY_BACKFACE_SKIP")
}

/// Key of an entity's kept layout: text, line width, flags.
type LayoutKey = (Option<rc_proto::nbt::Nbt>, i32, i8);

/// Kept layout of a display.
struct DisplayLayout {
    key: LayoutKey,
    /// Shadow and text glyphs, already placed (lines, alignment), width and height.
    shadow: Vec<Glyph>,
    text: Vec<Glyph>,
    width: f32,
    height: f32,
    frame: u64,
    /// New path: background fill (one glyph, colour given per frame), shadow and text in the pool.
    background: u32,
    shadow_run: PooledRun,
    text_run: PooledRun,
}

impl DisplayLayout {
    fn new(key: LayoutKey, shadow: Vec<Glyph>, text: Vec<Glyph>, width: f32, height: f32) -> Self {
        let none = PooledRun::default();
        DisplayLayout { key, shadow, text, width, height, frame: 0, background: 0, shadow_run: none, text_run: none }
    }

    fn add_to(&mut self, pool: &mut GlyphPool) {
        let fill = Glyph { rect: [-1.0, self.width, -1.0, self.height], skew: 0.0, uv: [0.0; 4], color: 0, mode: 0 };
        self.background = pool.push([&fill]);
        self.shadow_run = pool.push_run(&self.shadow);
        self.text_run = pool.push_run(&self.text);
    }

    fn pooled(&self) -> usize {
        1 + self.shadow_run.pooled() + self.text_run.pooled()
    }
}

/// Separates the shadow from the text: `all` (with shadow) contains `main` (without shadow) in order; the
/// rest is the shadow.
fn split_shadow(all: Vec<Glyph>, main: &[Glyph]) -> Vec<Glyph> {
    let same = |a: &Glyph, b: &Glyph| bytemuck::bytes_of(a) == bytemuck::bytes_of(b);
    let mut matched = 0;
    let mut shadow = Vec::new();
    for g in all {
        if matched < main.len() && same(&g, &main[matched]) {
            matched += 1;
        } else {
            shadow.push(g);
        }
    }
    if matched < main.len() { Vec::new() } else { shadow }
}

/// ARGB color as RGBA bytes of the glyphs.
fn rgba(argb: u32) -> u32 {
    let (a, r, g, b) = (argb >> 24, (argb >> 16) & 0xff, (argb >> 8) & 0xff, argb & 0xff);
    r | g << 8 | b << 16 | a << 24
}

/// Glyph alpha multiplied by the text opacity (byte).
fn with_opacity(g: Glyph, opacity: u32) -> Glyph {
    let a = (g.color >> 24) * opacity / 255;
    Glyph { color: (g.color & 0x00ff_ffff) | a << 24, ..g }
}

/// Horizontal offset of a line of width `lw` in a block of width `w` (flags 8:
/// left, 16: right, otherwise centered).
fn align(flags: i8, w: i32, lw: i32) -> f32 {
    if flags & 8 != 0 {
        0.0
    } else if flags & 16 != 0 {
        (w - lw) as f32
    } else {
        w as f32 / 2.0 - lw as f32 / 2.0
    }
}

/// Instances sent by the old path, per pass.
#[derive(Clone, Copy, Default)]
struct SentInstances {
    offset: u32,
    see: u32,
    hand: u32,
}

/// Layers and instances (or references) of the frame.
struct FrameOut {
    use_pool: bool,
    /// Camera of the frame when the displays turned away are left out.
    back_faces: Option<BackFaceTest>,
    /// Test sabotage: back-face test reversed.
    #[cfg(test)]
    reversed_backface_test: bool,
    layers: Vec<[[f32; 4]; 4]>,
    /// Old path.
    offset_inst: Vec<TextInstance>,
    see_inst: Vec<TextInstance>,
    /// Held map names: polygon offset, hand camera.
    hand_inst: Vec<TextInstance>,
    /// New path.
    offset_refs: Vec<OffsetLayer>,
    see_refs: Vec<LayerRef>,
    hand_refs: Vec<LayerRef>,
}

impl FrameOut {
    fn clear(&mut self) {
        self.layers.clear();
        self.offset_inst.clear();
        self.see_inst.clear();
        self.hand_inst.clear();
        self.offset_refs.clear();
        self.see_refs.clear();
        self.hand_refs.clear();
    }

    /// Layers of a display: background, shadow, text.
    fn emit(&mut self, d: &TextDraw, c: &DisplayLayout, default_background: u32) {
        let see = d.flags & 2 != 0;
        let light = if see { SEE_THROUGH } else { d.light };
        let base = if d.label != u32::MAX {
            crate::entity_render::map_label_matrix(d.m, c.width)
        } else {
            d.m * Mat4::from_translation(Vec3::new(1.0 - c.width / 2.0, -c.height, 0.0))
        };
        if !d.hand && self.turned_away(&base, c) {
            return;
        }
        let background = if d.flags & 4 != 0 { default_background } else { d.background };
        let opacity = u32::from(d.opacity);
        let shadowed = d.flags & 1 != 0;
        let pass = if see {
            SEE
        } else if d.hand {
            HAND
        } else {
            OFFSET
        };
        if background != 0 {
            let f = self.push_layer(base, BACKGROUND_DEPTH);
            self.add_background(pass, f, light, c, rgba(background));
        }
        if !c.shadow.is_empty() {
            let f = self.push_layer(base, 0.0);
            self.add_glyphs(pass, f, light, &c.shadow, c.shadow_run, opacity);
        }
        let f = self.push_layer(base, if shadowed { SHADOW_FRONT } else { 0.0 });
        self.add_glyphs(pass, f, light, &c.text, c.text_run, opacity);
    }

    /// True when the display (frame `base`) surely turns its back to the camera: the GPU would
    /// cull all its layers (a fraction of a font pixel apart, well within the test's slack).
    fn turned_away(&self, base: &Mat4, c: &DisplayLayout) -> bool {
        let Some(view) = &self.back_faces else { return false };
        // Glyphs and background lie within [-1, width + 1] × [-1, height + 1] font pixels (shadow included).
        let radius = (c.width + 1.0).hypot(c.height + 1.0) + 1.0;
        #[cfg(test)]
        if self.reversed_backface_test {
            return view.faces_away(&(*base * Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0))), radius);
        }
        view.faces_away(base, radius)
    }

    /// Matrix of a layer `z` font pixels in front of `base`; returns its index.
    fn push_layer(&mut self, base: Mat4, z: f32) -> u32 {
        self.layers.push((base * Mat4::from_translation(Vec3::new(0.0, 0.0, z))).to_cols_array_2d());
        (self.layers.len() - 1) as u32
    }

    fn add_background(&mut self, pass: usize, layer: u32, light: u32, c: &DisplayLayout, fill: u32) {
        if self.use_pool {
            let r = LayerRef { layer, light, first: c.background, count: 1, opacity: 255, fill };
            let opaque = u32::from(fill >> 24 == 255);
            self.push_ref(pass, OffsetLayer { layer: r, sorted: c.background, solids: opaque });
            return;
        }
        let glyph = Glyph { rect: [-1.0, c.width, -1.0, c.height], skew: 0.0, uv: [0.0; 4], color: fill, mode: 0 };
        self.instances(pass).push(TextInstance { face: layer, light, glyph });
    }

    fn add_glyphs(&mut self, pass: usize, layer: u32, light: u32, glyphs: &[Glyph], run: PooledRun, opacity: u32) {
        if self.use_pool {
            let r = LayerRef { layer, light, first: run.first, count: run.count, opacity, fill: 0 };
            // Below full opacity no glyph stays opaque: all of them go with the translucent ones.
            let (sorted, solids) = if opacity == 255 { (run.sorted, run.solids) } else { (run.first, 0) };
            self.push_ref(pass, OffsetLayer { layer: r, sorted, solids });
            return;
        }
        let out = self.instances(pass);
        out.extend(glyphs.iter().map(|&g| TextInstance { face: layer, light, glyph: with_opacity(g, opacity) }));
    }

    fn instances(&mut self, pass: usize) -> &mut Vec<TextInstance> {
        match pass {
            SEE => &mut self.see_inst,
            HAND => &mut self.hand_inst,
            _ => &mut self.offset_inst,
        }
    }

    fn push_ref(&mut self, pass: usize, r: OffsetLayer) {
        match pass {
            SEE => self.see_refs.push(r.layer),
            HAND => self.hand_refs.push(r.layer),
            _ => self.offset_refs.push(r),
        }
    }

    /// References of `pass` with at least one glyph, appended to `out`.
    fn collect_pass(&self, pass: usize, out: &mut Vec<LayerRef>) {
        let at = out.len();
        match pass {
            OFFSET => out.extend(self.offset_refs.iter().map(|o| o.layer)),
            SEE => out.extend_from_slice(&self.see_refs),
            HAND => out.extend_from_slice(&self.hand_refs),
            SOLID => {
                out.extend(self.offset_refs.iter().map(|o| LayerRef { first: o.sorted, count: o.solids, ..o.layer }))
            }
            _ => out.extend(self.offset_refs.iter().map(|o| LayerRef {
                first: o.sorted + o.solids,
                count: o.layer.count - o.solids,
                ..o.layer
            })),
        }
        let mut kept = at;
        for i in at..out.len() {
            if out[i].count > 0 {
                out[kept] = out[i];
                kept += 1;
            }
        }
        out.truncate(kept);
    }
}

/// What to send at the end of the frame (new path).
struct DisplayUpload {
    pool: PoolUpload,
    layers: BufferPlan,
    refs: BufferPlan,
}

/// CPU state of the displays (no GPU, for the proofs).
struct DisplayLists {
    use_pool: bool,
    skip_unchanged: bool,
    skip_back_faces: bool,
    cache: HashMap<i32, DisplayLayout>,
    /// Layout of map decoration names, per text.
    labels: Vec<(rc_proto::nbt::Nbt, DisplayLayout)>,
    frame: u64,
    out: FrameOut,
    /// Old path: instances sent, opaque polygon-offset ones at the head in improved transparency.
    counts: SentInstances,
    solid: u32,
    /// New path: pool, references sent (passes one after another), their draws and the range
    /// of draws of each pass.
    pool: GlyphPool,
    refs: Vec<LayerRef>,
    draws: Vec<RefDraw>,
    passes: [(u32, u32); 5],
    layer_buffer: FrameBuffer,
    ref_buffer: FrameBuffer,
}

impl DisplayLists {
    fn new(use_pool: bool, skip_back_faces: bool, skip_unchanged: bool) -> Self {
        DisplayLists {
            use_pool,
            skip_unchanged,
            cache: HashMap::new(),
            labels: Vec::new(),
            frame: 0,
            skip_back_faces,
            out: FrameOut {
                use_pool,
                back_faces: None,
                #[cfg(test)]
                reversed_backface_test: false,
                layers: Vec::new(),
                offset_inst: Vec::new(),
                see_inst: Vec::new(),
                hand_inst: Vec::new(),
                offset_refs: Vec::new(),
                see_refs: Vec::new(),
                hand_refs: Vec::new(),
            },
            counts: SentInstances::default(),
            solid: 0,
            pool: GlyphPool::default(),
            refs: Vec::new(),
            draws: Vec::new(),
            passes: [(0, 0); 5],
            layer_buffer: FrameBuffer::default(),
            ref_buffer: FrameBuffer::default(),
        }
    }

    fn begin(&mut self) {
        self.frame += 1;
        self.out.clear();
        // No reference of the frame exists yet: the pool can be rebuilt.
        if self.use_pool && self.pool.compact_if_worth() {
            for c in self.cache.values_mut().chain(self.labels.iter_mut().map(|(_, c)| c)) {
                c.add_to(&mut self.pool);
            }
        }
    }

    /// A fresh layout (`layout` → layout, obfuscated) drawn this frame; returned if it is kept.
    fn fresh(&mut self, d: &TextDraw, default_background: u32, layout: (DisplayLayout, bool)) -> Option<DisplayLayout> {
        let (mut c, obfuscated) = layout;
        if self.use_pool {
            c.add_to(&mut self.pool);
        }
        c.frame = self.frame;
        self.out.emit(d, &c, default_background);
        // Obfuscated text: glyphs drawn on every frame, no cache.
        if obfuscated {
            self.pool.forget(c.pooled());
            return None;
        }
        Some(c)
    }

    /// Text display of entity `d.id` with text `key`; `layout` is called only if the layout
    /// must be (re)done.
    fn push_entity(
        &mut self,
        d: &TextDraw,
        key: (&Option<rc_proto::nbt::Nbt>, i32, i8),
        default_background: u32,
        layout: impl FnOnce() -> (DisplayLayout, bool),
    ) {
        let frame = self.frame;
        let same = |c: &DisplayLayout| c.key.0 == *key.0 && c.key.1 == key.1 && c.key.2 == key.2;
        if let Some(c) = self.cache.get_mut(&d.id).filter(|c| same(c)) {
            c.frame = frame;
            self.out.emit(d, c, default_background);
            return;
        }
        let replaced = match self.fresh(d, default_background, layout()) {
            Some(c) => self.cache.insert(d.id, c),
            None => self.cache.remove(&d.id),
        };
        if let Some(old) = replaced {
            self.pool.forget(old.pooled());
        }
    }

    /// Map decoration name: a line with no splitting or flags, layout kept per text (except
    /// obfuscated text).
    fn push_label(
        &mut self,
        d: &TextDraw,
        text: &rc_proto::nbt::Nbt,
        default_background: u32,
        layout: impl FnOnce() -> (DisplayLayout, bool),
    ) {
        let frame = self.frame;
        if let Some((_, c)) = self.labels.iter_mut().find(|(t, _)| t == text) {
            c.frame = frame;
            self.out.emit(d, c, default_background);
            return;
        }
        if let Some(c) = self.fresh(d, default_background, layout()) {
            self.labels.push((text.clone(), c));
        }
    }

    /// End of the frame: unseen layouts forgotten; old path: opaque polygon-offset instances
    /// first in improved transparency; new path: references and draws of the passes, and what
    /// to send (nothing if nothing is drawn).
    fn finish(&mut self, oit: bool) -> Option<DisplayUpload> {
        let (frame, pool) = (self.frame, &mut self.pool);
        let mut unseen = |c: &DisplayLayout| {
            let keep = c.frame == frame;
            if !keep {
                pool.forget(c.pooled());
            }
            keep
        };
        self.cache.retain(|_, c| unseen(c));
        self.labels.retain(|(_, c)| unseen(c));
        let o = &mut self.out;
        if !self.use_pool {
            if oit {
                o.offset_inst.sort_unstable_by_key(|t| !t.glyph.solid());
                self.solid = o.offset_inst.iter().take_while(|t| t.glyph.solid()).count() as u32;
            }
            let count = |v: &Vec<TextInstance>| v.len() as u32;
            self.counts =
                SentInstances { offset: count(&o.offset_inst), see: count(&o.see_inst), hand: count(&o.hand_inst) };
            return None;
        }
        self.refs.clear();
        self.draws.clear();
        self.passes = [(0, 0); 5];
        let order: &[usize] = if oit { &[SOLID, REST, SEE, HAND] } else { &[OFFSET, SEE, HAND] };
        for &p in order {
            let (at, first) = (self.draws.len() as u32, self.refs.len());
            self.out.collect_pass(p, &mut self.refs);
            let mut draws = DrawSplitter::new(first as u32, true);
            for r in &self.refs[first..] {
                draws.add(r.count, &mut self.draws);
            }
            draws.finish(&mut self.draws);
            self.passes[p] = (at, self.draws.len() as u32);
        }
        if self.refs.is_empty() {
            return None;
        }
        Some(DisplayUpload {
            pool: self.pool.plan_upload(),
            layers: self.layer_buffer.plan(bytemuck::cast_slice(&self.out.layers), self.skip_unchanged),
            refs: self.ref_buffer.plan(bytemuck::cast_slice(&self.refs), self.skip_unchanged),
        })
    }

    fn pass_draws(&self, pass: usize) -> &[RefDraw] {
        let (a, b) = self.passes[pass];
        &self.draws[a as usize..b as usize]
    }
}

pub struct TextDisplays {
    /// Polygon offset (depth tested and written), see-through (no depth).
    offset: wgpu::RenderPipeline,
    see_through: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    bind: Option<wgpu::BindGroup>,
    /// Glyph sheet generation and layer buffer generation bound.
    bound: (u32, u64),
    face_gen: u64,
    /// Old path: layer matrices and instances (buffer, capacity).
    face_buf: Option<(wgpu::Buffer, u64)>,
    inst_buf: Option<(wgpu::Buffer, u64)>,
    /// New path: glyph pool, layer matrices, references.
    pool_buf: Option<wgpu::Buffer>,
    layer_matrices: Option<wgpu::Buffer>,
    ref_buf: Option<wgpu::Buffer>,
    lists: DisplayLists,
    /// Improved transparency (set by the renderer): opaque polygon-offset glyphs with the
    /// solids, backgrounds and translucent text in the stages.
    pub oit: bool,
    camera_layout: wgpu::BindGroupLayout,
    oit_set: Option<crate::oit::Set>,
    /// Form of the vertex shader (also that of the improved-transparency pipelines).
    shader_options: SmallLayerOptions,
}

const INSTANCE_ATTRS: [wgpu::VertexAttribute; 7] = wgpu::vertex_attr_array![
    0 => Uint32, 1 => Uint32, 2 => Float32x4, 3 => Float32,
    4 => Float32x4, 5 => Unorm8x4, 6 => Uint32
];
const REF_ATTRS: [wgpu::VertexAttribute; 6] =
    wgpu::vertex_attr_array![0 => Uint32, 1 => Uint32, 2 => Uint32, 3 => Uint32, 4 => Uint32, 5 => Uint32];

/// Vertex entry and instance layout of a path.
fn vertex_input(use_pool: bool) -> (&'static str, wgpu::VertexBufferLayout<'static>) {
    let (entry, stride, attributes): (_, usize, &'static [_]) = if use_pool {
        ("vs_ref", size_of::<LayerRef>(), &REF_ATTRS)
    } else {
        ("vs", size_of::<TextInstance>(), &INSTANCE_ATTRS)
    };
    (
        entry,
        wgpu::VertexBufferLayout { array_stride: stride as u64, step_mode: wgpu::VertexStepMode::Instance, attributes },
    )
}

impl TextDisplays {
    pub fn new(device: &wgpu::Device, camera_layout: &wgpu::BindGroupLayout, format: wgpu::TextureFormat) -> Self {
        let lists = DisplayLists::new(glyph_pool_on(), backface_skip_on(), skip_unchanged_writes_on());
        Self::with_mode(device, camera_layout, format, lists, SmallLayerOptions::from_switches())
    }

    fn with_mode(
        device: &wgpu::Device,
        camera_layout: &wgpu::BindGroupLayout,
        format: wgpu::TextureFormat,
        lists: DisplayLists,
        shader_options: SmallLayerOptions,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("text displays"),
            source: wgpu::ShaderSource::Wgsl(shader_options.shader_source(include_str!("text_display.wgsl")).into()),
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
        if lists.use_pool {
            entries.push(storage(3));
        }
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("text displays"),
            entries: &entries,
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("text displays"),
            bind_group_layouts: &[Some(camera_layout), Some(&layout)],
            immediate_size: 0,
        });
        let (entry, buffer) = vertex_input(lists.use_pool);
        let make = |label, depth: wgpu::DepthStencilState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    buffers: &[Some(buffer.clone())],
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
                // Back faces culled: a display is only read from the front.
                primitive: wgpu::PrimitiveState { cull_mode: Some(wgpu::Face::Back), ..Default::default() },
                depth_stencil: Some(depth),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        // Polygon offset toward the camera (slope 1, constant 10), depth written.
        let offset = make(
            "text displays",
            wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(crate::render::DEPTH_NEARER),
                stencil: Default::default(),
                bias: crate::render::depth_bias(10, 1.0),
            },
        );
        let see_through = make(
            "text displays (see-through)",
            wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: Default::default(),
                bias: Default::default(),
            },
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        TextDisplays {
            offset,
            see_through,
            layout,
            sampler,
            bind: None,
            bound: (u32::MAX, u64::MAX),
            face_gen: 0,
            face_buf: None,
            inst_buf: None,
            pool_buf: None,
            layer_matrices: None,
            ref_buf: None,
            lists,
            oit: false,
            camera_layout: camera_layout.clone(),
            oit_set: None,
            shader_options,
        }
    }

    /// Improved-transparency pipelines (once): back faces culled.
    pub fn ensure_oit(&mut self, device: &wgpu::Device, layouts: &crate::oit::Layouts) {
        if self.oit_set.is_some() {
            return;
        }
        let module = layouts.module(
            device,
            "text displays (transparency)",
            &self.shader_options.shader_source(include_str!("text_display.wgsl")),
            include_str!("text_display_oit.wgsl"),
        );
        let groups = [Some(&self.camera_layout), Some(&self.layout)];
        let (entry, buffer) = vertex_input(self.lists.use_pool);
        let buffers = [Some(buffer)];
        let spec =
            crate::oit::Spec::new("text displays (transparency)", &groups, &module, entry, &buffers, "fs").cull(true);
        self.oit_set = Some(layouts.set(device, &spec));
    }

    /// Improved transparency: opaque polygon-offset glyphs, with the solids.
    pub fn draw_solid(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.lists.use_pool {
            self.draw_refs(pass, &self.offset, SOLID);
            return;
        }
        self.draw(pass, &self.offset, 0..self.lists.solid.min(self.lists.counts.offset));
    }

    /// Improved transparency: rest of the polygon-offset mode, stage `f`.
    pub fn draw_oit(&self, pass: &mut wgpu::RenderPass<'_>, f: crate::oit::Frame<'_>) {
        let Some(set) = &self.oit_set else { return };
        if self.lists.use_pool {
            let (Some(bind), Some(buf)) = (&self.bind, &self.ref_buf) else { return };
            let draws = self.lists.pass_draws(REST);
            if draws.is_empty() {
                return;
            }
            f.set(pass, set);
            pass.set_bind_group(1, bind, &[]);
            pass.set_vertex_buffer(0, buf.slice(..));
            draw_refs(pass, draws);
            return;
        }
        let (Some(bind), Some((buf, _))) = (&self.bind, &self.inst_buf) else { return };
        let counts = self.lists.counts;
        let range = self.lists.solid.min(counts.offset)..counts.offset;
        if range.is_empty() {
            return;
        }
        f.set(pass, set);
        pass.set_bind_group(1, bind, &[]);
        pass.set_vertex_buffer(0, buf.slice(..));
        pass.draw(0..6, range);
    }

    /// Frame instances. `labels`: map decoration names (`TextDraw::label`);
    /// `default_background`: default background color (flag 4), text-background opacity of the
    /// options; `view`: camera, for the displays turned away.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        gui: &mut Gui,
        queue: &wgpu::Queue,
        draws: &[TextDraw],
        labels: &[rc_proto::nbt::Nbt],
        ents: &Entities,
        default_background: u32,
        view: &BackFaceTest,
    ) {
        self.lists.begin();
        self.lists.out.back_faces = self.lists.skip_back_faces.then_some(*view);
        for d in draws {
            if d.label != u32::MAX {
                let Some(text) = labels.get(d.label as usize) else { continue };
                let layout = || build(gui, queue, (Some(text.clone()), i32::MAX, 0));
                self.lists.push_label(d, text, default_background, layout);
                continue;
            }
            let Some(e) = ents.map.get(&d.id) else { continue };
            let Some(DisplaySub::Text(t)) = e.display.as_deref().and_then(|ds| ds.render.as_ref()).map(|r| &r.sub)
            else {
                continue;
            };
            let layout = || build(gui, queue, (t.text.clone(), t.line_width, t.flags));
            self.lists.push_entity(d, (&t.text, t.line_width, t.flags), default_background, layout);
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
        let plan = self.lists.finish(self.oit);
        if !self.lists.use_pool {
            if self.upload_instances(device, queue) {
                self.bind_group(device, glyph_gen, view);
            }
            return;
        }
        let Some(plan) = plan else { return };
        let l = &self.lists;
        let pool = l.pool.upload(plan.pool, device, queue, up, &mut self.pool_buf, "text displays (glyphs)");
        let (storage, vertex) = (wgpu::BufferUsages::STORAGE, wgpu::BufferUsages::VERTEX);
        let layers = bytemuck::cast_slice(&l.out.layers);
        let label = "text display layers";
        let matrices =
            FrameBuffer::apply(plan.layers, layers, device, queue, up, &mut self.layer_matrices, storage, label);
        let refs = bytemuck::cast_slice(&l.refs);
        FrameBuffer::apply(plan.refs, refs, device, queue, up, &mut self.ref_buf, vertex, "text display references");
        if pool || matrices {
            self.face_gen += 1;
        }
        self.bind_group(device, glyph_gen, view);
    }

    /// Old path: instances and matrices rewritten; false if nothing is drawn.
    fn upload_instances(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> bool {
        let (o, counts) = (&mut self.lists.out, self.lists.counts);
        if counts.offset + counts.see + counts.hand == 0 {
            return false;
        }
        let mut all = std::mem::take(&mut o.offset_inst);
        all.extend_from_slice(&o.see_inst);
        all.extend_from_slice(&o.hand_inst);
        let layers = bytemuck::cast_slice(&o.layers);
        let storage = wgpu::BufferUsages::STORAGE;
        if write_growing(device, queue, &mut self.face_buf, layers, storage, "text display layers") {
            self.face_gen += 1;
        }
        let vertex = wgpu::BufferUsages::VERTEX;
        write_growing(device, queue, &mut self.inst_buf, bytemuck::cast_slice(&all), vertex, "text displays");
        all.truncate(counts.offset as usize);
        o.offset_inst = all;
        true
    }

    fn bind_group(&mut self, device: &wgpu::Device, glyph_gen: u32, view: impl FnOnce() -> wgpu::TextureView) {
        if self.bind.is_some() && self.bound == (glyph_gen, self.face_gen) {
            return;
        }
        let (layers, pool) = if self.lists.use_pool {
            (self.layer_matrices.as_ref(), self.pool_buf.as_ref())
        } else {
            (self.face_buf.as_ref().map(|(b, _)| b), None)
        };
        let view = view();
        let mut entries = vec![
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: layers.expect("layer matrices uploaded before binding").as_entire_binding(),
            },
        ];
        if let Some(pool) = pool {
            entries.push(wgpu::BindGroupEntry { binding: 3, resource: pool.as_entire_binding() });
        }
        self.bind = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("text displays"),
            layout: &self.layout,
            entries: &entries,
        }));
        self.bound = (glyph_gen, self.face_gen);
    }

    /// "Polygon offset" mode, with the entities (group 0 = camera already bound).
    pub fn draw_offset(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.lists.use_pool {
            self.draw_refs(pass, &self.offset, OFFSET);
            return;
        }
        self.draw(pass, &self.offset, 0..self.lists.counts.offset);
    }

    /// "See-through" mode, after the translucent terrain.
    pub fn draw_see_through(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.lists.use_pool {
            self.draw_refs(pass, &self.see_through, SEE);
            return;
        }
        let c = self.lists.counts;
        self.draw(pass, &self.see_through, c.offset..c.offset + c.see);
    }

    /// Names of the held map, after the hand (group 0 = hand camera already bound).
    pub fn draw_hand(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.lists.use_pool {
            self.draw_refs(pass, &self.offset, HAND);
            return;
        }
        let c = self.lists.counts;
        let start = c.offset + c.see;
        self.draw(pass, &self.offset, start..start + c.hand);
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, pipeline: &wgpu::RenderPipeline, range: std::ops::Range<u32>) {
        let (Some(bind), Some((buf, _))) = (&self.bind, &self.inst_buf) else { return };
        if range.is_empty() {
            return;
        }
        pass.set_pipeline(pipeline);
        pass.set_bind_group(1, bind, &[]);
        pass.set_vertex_buffer(0, buf.slice(..));
        pass.draw(0..6, range);
    }

    fn draw_refs(&self, pass: &mut wgpu::RenderPass<'_>, pipeline: &wgpu::RenderPipeline, which: usize) {
        let (Some(bind), Some(buf)) = (&self.bind, &self.ref_buf) else { return };
        let draws = self.lists.pass_draws(which);
        if draws.is_empty() {
            return;
        }
        pass.set_pipeline(pipeline);
        pass.set_bind_group(1, bind, &[]);
        pass.set_vertex_buffer(0, buf.slice(..));
        draw_refs(pass, draws);
    }
}

/// Layout: split to the line width (empty text: no line), lines of
/// 9 + 1 pixels, alignment, shadow separated from the text. True second if the text is obfuscated.
fn build(gui: &mut Gui, queue: &wgpu::Queue, key: LayoutKey) -> (DisplayLayout, bool) {
    let (nbt, line_width, flags) = &key;
    let chars = nbt
        .as_ref()
        .and_then(rc_text::from_nbt)
        .map(|c| rc_text::styled_chars(&rc_text::flatten(&c, &gui.lang)))
        .unwrap_or_default();
    let lines = if chars.is_empty() { Vec::new() } else { rc_text::wrap(&*gui, &chars, *line_width as f32) };
    let widths: Vec<i32> = lines.iter().map(|l| rc_text::width(&*gui, l)).collect();
    let width = widths.iter().copied().max().unwrap_or(0);
    let height = lines.len() as f32 * 10.0 - 1.0;
    let obfuscated = chars.iter().any(|c| c.style.obfuscated);
    let (mut shadow, mut text) = (Vec::new(), Vec::new());
    for (i, (line, &lw)) in lines.iter().zip(&widths).enumerate() {
        if line.is_empty() {
            continue;
        }
        let dx = align(*flags, width, lw);
        let dy = i as f32 * 10.0;
        let place = |mut g: Glyph| {
            g.rect[0] += dx;
            g.rect[1] += dx;
            g.rect[2] += dy;
            g.rect[3] += dy;
            g
        };
        let main = glyphs(&gui.text_quads_left(queue, line, 0xffff_ffff, false));
        if flags & 1 != 0 {
            let all = glyphs(&gui.text_quads_left(queue, line, 0xffff_ffff, true));
            shadow.extend(split_shadow(all, &main).into_iter().map(place));
        }
        text.extend(main.into_iter().map(place));
    }
    (DisplayLayout::new(key, shadow, text, width as f32, height), obfuscated)
}

#[cfg(test)]
mod tests;
