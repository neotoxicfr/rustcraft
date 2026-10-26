//! Name tags (`docs/specs/entities.md` part 2 § 1.8): text laid out by the
//! interface (same glyphs and styles), camera-facing panel at 0.025 block per pixel. Normal
//! pass with the entities; "see-through" pass (no depth test) after the translucent
//! terrain.
//!
//! The layout is kept per entity as long as the name and the posture do not change (except
//! obfuscated text).
//!
//! **Kept glyphs** (`RUSTCRAFT_OPT_TAGPERSIST`, default 1). Old path (`=0`): one instance
//! per glyph (anchor, light and glyph copied on every frame, 60 bytes). New path: the
//! glyphs of each layout live in a pool (storage buffer) sent back only
//! for new layouts; per frame, a 24-byte reference per tag and per
//! pass (anchor, light, first glyph, count), drawn as instances of `6 × n` vertices (`n`
//! = longest name of the draw; a pass is cut into contiguous draws whose vertices stay
//! under twice those of their glyphs). The normal pass keeps the glyph order; in
//! improved transparency, the opaque glyphs come from a copy sorted solids first
//! (kept only when the original order is not already so). Forgotten glyphs are counted
//! and the pool is compacted at the start of a frame when they exceed the live ones and 4096 (pool
//! at most live + max(live, 4096); the GPU buffer does not shrink). All writes
//! go through the common transit.

use hashbrown::HashMap;

use glam::Vec3;
use rc_text::StyledChar;

use crate::gui::Gui;
use crate::render::upload::Uploader;

/// Glyph or flat fill in font pixels: top-left and top-right edges (x), top and bottom (y),
/// bottom offset (italic), corner UVs, 8-bit RGBA colour, mode (0 flat, page + 1).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct Glyph {
    pub rect: [f32; 4],
    pub skew: f32,
    pub uv: [f32; 4],
    pub color: u32,
    pub mode: u32,
}

impl Glyph {
    /// Opaque glyph or stroke: drawn with the solids in improved transparency, like an opaque
    /// game text without background (the translucent backgrounds follow the stages).
    pub fn solid(&self) -> bool {
        self.color >> 24 == 255
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct TagInstance {
    anchor: [f32; 3],
    light: u32,
    glyph: Glyph,
}

/// Glyph of the pool (same fields as [`Glyph`], aligned to 16 bytes for storage).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Stored {
    rect: [f32; 4],
    uv: [f32; 4],
    skew: f32,
    color: u32,
    mode: u32,
    pad: u32,
}

impl Stored {
    fn new(g: &Glyph) -> Self {
        Stored { rect: g.rect, uv: g.uv, skew: g.skew, color: g.color, mode: g.mode, pad: 0 }
    }
}

/// A tag in a pass: `count` glyphs of the pool starting at `first`.
#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct TagRef {
    anchor: [f32; 3],
    light: u32,
    first: u32,
    count: u32,
}

/// Passes of the new path: normal, see-through, and in improved transparency the normal one
/// cut into opaque glyphs (with the solids) and the rest (stages).
const NORMAL: usize = 0;
const SEE: usize = 1;
const SOLID: usize = 2;
const REST: usize = 3;

/// Forgotten glyphs tolerated before compaction (beyond, if they also exceed the live ones).
const COMPACT_MIN: usize = 4096;

/// Glyphs kept on the GPU (`RUSTCRAFT_OPT_TAGPERSIST`, default 1), read once.
fn persist_on() -> bool {
    rc_data::env_str!("RUSTCRAFT_OPT_TAGPERSIST") != Some("0")
}

const GLYPH_ATTRS: [wgpu::VertexAttribute; 7] = wgpu::vertex_attr_array![
    0 => Float32x3, 1 => Uint32, 2 => Float32x4, 3 => Float32,
    4 => Float32x4, 5 => Unorm8x4, 6 => Uint32
];
const REF_ATTRS: [wgpu::VertexAttribute; 4] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Uint32, 2 => Uint32, 3 => Uint32];

/// Vertices of an instance: glyph (old path) or tag reference.
fn vertex_layout(persist: bool) -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: if persist { size_of::<TagRef>() } else { size_of::<TagInstance>() } as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: if persist { &REF_ATTRS } else { &GLYPH_ATTRS },
    }
}

/// Colours of a tag for background opacity `a` (0.25 by default, the text background
/// opacity when the option sets it to "everywhere"): white "see-through" text of alpha
/// max((a + 0.75) / 2, 0.5), black background of alpha a; 8-bit channels by truncation.
pub fn tag_colors(a: f32) -> (u32, u32) {
    let ch = |v: f32| (v * 255.0).floor().clamp(0.0, 255.0) as u32;
    (ch(((a + 0.75) * 0.5).max(0.5)) << 24 | 0xff_ffff, ch(a) << 24)
}

/// Default background opacity of the tags.
pub const DEFAULT_OPACITY: f32 = 0.25;

/// Origin of an entity's name: plain text (player), component (custom name), kind
/// (translated name of the type).
#[derive(Debug, Clone, Copy)]
pub enum NameRef<'a> {
    Text(&'a str),
    Nbt(&'a rc_proto::nbt::Nbt),
    Type(&'a str),
}

/// Same origin, owned (cache key).
#[derive(Debug, Clone, PartialEq)]
enum NameSource {
    Text(String),
    Nbt(rc_proto::nbt::Nbt),
    Type(String),
}

impl NameSource {
    fn new(r: NameRef<'_>) -> Self {
        match r {
            NameRef::Text(s) => NameSource::Text(s.to_owned()),
            NameRef::Nbt(n) => NameSource::Nbt(n.clone()),
            NameRef::Type(s) => NameSource::Type(s.to_owned()),
        }
    }

    fn is(&self, r: NameRef<'_>) -> bool {
        match (self, r) {
            (NameSource::Text(a), NameRef::Text(b)) | (NameSource::Type(a), NameRef::Type(b)) => a == b,
            (NameSource::Nbt(a), NameRef::Nbt(b)) => a == b,
            _ => false,
        }
    }
}

/// Starts of the ranges of a layout in the pool: normal pass, see-through, normal
/// sorted solids first (equal to `normal` if the original order already is), and number of solids.
#[derive(Clone, Copy, Default)]
struct Spans {
    normal: u32,
    see: u32,
    sorted: u32,
    solid: u32,
}

struct Cached {
    source: NameSource,
    discrete: bool,
    /// Normal pass, "see-through" pass.
    normal: Vec<Glyph>,
    see: Vec<Glyph>,
    frame: u64,
    spans: Spans,
}

impl Cached {
    /// Glyphs occupied in the pool.
    fn stored(&self) -> usize {
        let copy = if self.spans.sorted != self.spans.normal { self.normal.len() } else { 0 };
        self.normal.len() + self.see.len() + copy
    }

    /// References of the tag in each pass.
    fn refs(&self, anchor: [f32; 3], normal_light: u32, light: u32) -> [TagRef; 4] {
        let s = self.spans;
        let n = self.normal.len() as u32;
        let r = |light, first, count| TagRef { anchor, light, first, count };
        [
            r(normal_light, s.normal, n),
            r(light, s.see, self.see.len() as u32),
            r(normal_light, s.sorted, s.solid),
            r(normal_light, s.sorted + s.solid, n - s.solid),
        ]
    }
}

/// Glyph pool (CPU copy of the storage buffer).
#[derive(Default)]
struct Store {
    glyphs: Vec<Stored>,
    /// First glyph not yet sent.
    dirty: usize,
    /// Glyphs no longer referenced by any kept layout.
    dead: usize,
    /// Capacity of the GPU buffer in glyphs (0 = not yet created).
    cap: usize,
    /// Test sabotage: additions never sent.
    #[cfg(test)]
    sabotage: bool,
    #[cfg(test)]
    compactions: u32,
}

impl Store {
    fn add(&mut self, normal: &[Glyph], see: &[Glyph]) -> Spans {
        // Append at the tail: always beyond `dirty`, hence sent at the end of the frame.
        let at = self.glyphs.len();
        self.glyphs.extend(normal.iter().map(Stored::new));
        let see_at = self.glyphs.len();
        self.glyphs.extend(see.iter().map(Stored::new));
        let solid = normal.iter().filter(|g| g.solid()).count();
        let sorted = if normal[..solid].iter().all(Glyph::solid) {
            at
        } else {
            let s = self.glyphs.len();
            self.glyphs.extend(normal.iter().filter(|g| g.solid()).map(Stored::new));
            self.glyphs.extend(normal.iter().filter(|g| !g.solid()).map(Stored::new));
            s
        };
        Spans { normal: at as u32, see: see_at as u32, sorted: sorted as u32, solid: solid as u32 }
    }
}

/// Upload of the pool for the frame: buffer recreated at `grow` glyphs, glyphs `from..` to write.
struct StoreSend {
    grow: Option<usize>,
    from: usize,
}

/// Lists of the frame where a tag is added.
struct Out<'a> {
    persist: bool,
    normal: &'a mut Vec<TagInstance>,
    see: &'a mut Vec<TagInstance>,
    tags: &'a mut Vec<[TagRef; 4]>,
}

impl Out<'_> {
    /// Instances of a tag: normal pass (light at least 2 if the entity is not
    /// sneaking), "see-through" pass.
    fn emit(self, c: &Cached, anchor: Vec3, discrete: bool, light: (u8, u8)) {
        let pack = |b: u8, s: u8| (u32::from(b) * 16) | ((u32::from(s) * 16) << 8);
        let anchor = anchor.to_array();
        let base = pack(light.0, light.1);
        let normal_light = if discrete { base } else { pack(light.0.max(2), light.1.max(2)) };
        if self.persist {
            self.tags.push(c.refs(anchor, normal_light, base));
        } else {
            self.normal.extend(c.normal.iter().map(|&glyph| TagInstance { anchor, light: normal_light, glyph }));
            self.see.extend(c.see.iter().map(|&glyph| TagInstance { anchor, light: base, glyph }));
        }
    }
}

/// CPU state of the tags (without GPU, for the proofs).
struct Lists {
    persist: bool,
    normal_inst: Vec<TagInstance>,
    see_inst: Vec<TagInstance>,
    /// Instances (normal, see-through) sent.
    counts: (u32, u32),
    /// Improved transparency, old path: opaque glyphs of the normal pass first.
    solid: u32,
    cache: HashMap<i32, Cached>,
    frame: u64,
    /// Version of the teams of the kept layout.
    version: u64,
    right_up: [[f32; 4]; 2],
    /// Colours (see-through text, background) of the current background opacity (`tag_colors`).
    colors: (u32, u32),
    store: Store,
    /// New path: tags of the frame, one reference per pass.
    tags: Vec<[TagRef; 4]>,
    /// References sent, passes one after another (new path).
    refs: Vec<TagRef>,
    /// Draws of the frame: (first instance, instances, glyphs at most per instance); a
    /// pass is a contiguous range of them (`passes`, start..end), drawn in that order.
    segs: Vec<(u32, u32, u32)>,
    passes: [(u32, u32); 4],
    /// Test sabotage: a pass in a single draw.
    #[cfg(test)]
    unsplit: bool,
}

impl Lists {
    fn new(persist: bool) -> Self {
        Lists {
            persist,
            normal_inst: Vec::new(),
            see_inst: Vec::new(),
            counts: (0, 0),
            solid: 0,
            cache: HashMap::new(),
            frame: 0,
            version: 0,
            right_up: [[0.0; 4]; 2],
            colors: tag_colors(DEFAULT_OPACITY),
            store: Store::default(),
            tags: Vec::new(),
            refs: Vec::new(),
            segs: Vec::new(),
            passes: [(0, 0); 4],
            #[cfg(test)]
            unsplit: false,
        }
    }

    /// All layouts forgotten.
    fn forget(&mut self) {
        self.store.dead += self.cache.values().map(Cached::stored).sum::<usize>();
        self.cache.clear();
    }

    fn sync(&mut self, version: u64) {
        if self.version != version {
            self.version = version;
            self.forget();
        }
    }

    fn set_background(&mut self, opacity: f32) {
        let colors = tag_colors(opacity);
        if colors != self.colors {
            self.colors = colors;
            self.forget();
        }
    }

    fn clear(&mut self) {
        self.normal_inst.clear();
        self.see_inst.clear();
        self.tags.clear();
    }

    fn begin(&mut self, right: Vec3, up: Vec3) {
        self.clear();
        self.frame += 1;
        self.right_up = [right.extend(0.0).to_array(), up.extend(0.0).to_array()];
        // No reference of the frame exists yet: the pool can be rebuilt.
        let s = &mut self.store;
        if self.persist && s.dead > (s.glyphs.len() - s.dead).max(COMPACT_MIN) {
            s.glyphs.clear();
            s.dead = 0;
            s.dirty = 0;
            #[cfg(test)]
            {
                s.compactions += 1;
            }
            for c in self.cache.values_mut() {
                c.spans = s.add(&c.normal, &c.see);
            }
        }
    }

    /// A tag; `layout` (background colours → glyphs of the normal and see-through passes,
    /// obfuscated text) is called only if the layout must be redone.
    fn push_with(
        &mut self,
        id: i32,
        source: NameRef<'_>,
        anchor: Vec3,
        discrete: bool,
        light: (u8, u8),
        layout: impl FnOnce((u32, u32)) -> (Vec<Glyph>, Vec<Glyph>, bool),
    ) {
        let frame = self.frame;
        let out =
            Out { persist: self.persist, normal: &mut self.normal_inst, see: &mut self.see_inst, tags: &mut self.tags };
        if let Some(c) = self.cache.get_mut(&id).filter(|c| c.discrete == discrete && c.source.is(source)) {
            c.frame = frame;
            out.emit(c, anchor, discrete, light);
            return;
        }
        let (normal, see, obfuscated) = layout(self.colors);
        let spans = if self.persist { self.store.add(&normal, &see) } else { Spans::default() };
        let c = Cached { source: NameSource::new(source), discrete, normal, see, frame, spans };
        out.emit(&c, anchor, discrete, light);
        // Obfuscated text: glyphs drawn on every frame, no cache.
        if obfuscated {
            self.store.dead += c.stored();
            if let Some(old) = self.cache.remove(&id) {
                self.store.dead += old.stored();
            }
        } else if let Some(old) = self.cache.insert(id, c) {
            self.store.dead += old.stored();
        }
    }

    /// End of frame: unseen layouts forgotten, passes sorted. New path: upload of
    /// the pool (none if nothing is drawn).
    fn finish(&mut self, oit: bool) -> Option<StoreSend> {
        // Entities without a tag this frame: layout forgotten.
        let frame = self.frame;
        let dead = &mut self.store.dead;
        self.cache.retain(|_, c| {
            let keep = c.frame == frame;
            if !keep {
                *dead += c.stored();
            }
            keep
        });
        self.segs.clear();
        self.passes = [(0, 0); 4];
        if !self.persist {
            if oit {
                self.normal_inst.sort_unstable_by_key(|t| !t.glyph.solid());
                self.solid = self.normal_inst.iter().take_while(|t| t.glyph.solid()).count() as u32;
            }
            let (n, s) = (self.normal_inst.len() as u32, self.see_inst.len() as u32);
            self.counts = (n, s);
            let solid = self.solid.min(n);
            // One instance per glyph: one draw per pass.
            for (p, start, count) in [(NORMAL, 0, n), (SEE, n, s), (SOLID, 0, solid), (REST, solid, n - solid)] {
                let at = self.segs.len() as u32;
                if count > 0 {
                    self.segs.push((start, count, 1));
                }
                self.passes[p] = (at, self.segs.len() as u32);
            }
            return None;
        }
        self.refs.clear();
        #[cfg(test)]
        let split = !self.unsplit;
        #[cfg(not(test))]
        let split = true;
        let order: &[usize] = if oit { &[SOLID, REST, SEE] } else { &[NORMAL, SEE] };
        for &p in order {
            let at = self.segs.len() as u32;
            // Draw in progress: first instance, instances, glyphs at most, useful glyphs.
            let mut seg = (self.refs.len() as u32, 0u32, 0u32, 0u64);
            for t in &self.tags {
                let c = t[p].count;
                if c == 0 {
                    continue;
                }
                // All the instances of a draw have the vertices of the longest: a draw
                // is closed before its vertices exceed twice those of its glyphs, so a
                // long name does not make the short ones pay. Contiguous ranges: the order is kept.
                let most = seg.2.max(c);
                if split && seg.1 > 0 && u64::from(most) * u64::from(seg.1 + 1) > 2 * (seg.3 + u64::from(c)) {
                    self.segs.push((seg.0, seg.1, seg.2));
                    seg = (self.refs.len() as u32, 0, 0, 0);
                }
                seg = (seg.0, seg.1 + 1, seg.2.max(c), seg.3 + u64::from(c));
                self.refs.push(t[p]);
            }
            if seg.1 > 0 {
                self.segs.push((seg.0, seg.1, seg.2));
            }
            self.passes[p] = (at, self.segs.len() as u32);
        }
        if self.refs.is_empty() {
            return None;
        }
        let s = &mut self.store;
        let len = s.glyphs.len();
        let grow = (s.cap < len).then(|| {
            s.cap = len.next_power_of_two().max(1024);
            s.dirty = 0;
            s.cap
        });
        #[cfg(test)]
        if s.sabotage && grow.is_none() {
            s.dirty = len;
        }
        let from = s.dirty;
        s.dirty = len;
        Some(StoreSend { grow, from })
    }
}

pub struct NameTags {
    normal: wgpu::RenderPipeline,
    see_through: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    axes: wgpu::Buffer,
    /// Binding and generation of the bound glyph sheet.
    bind: Option<(wgpu::BindGroup, u32)>,
    /// Instances (glyphs or references) of the frame.
    buf: Option<(wgpu::Buffer, u64)>,
    /// New path: glyph pool on the GPU.
    store_buf: Option<wgpu::Buffer>,
    lists: Lists,
    /// Improved transparency (set by the renderer): opaque glyphs of the normal pass with the
    /// solids, the rest in the stages.
    pub oit: bool,
    camera_layout: wgpu::BindGroupLayout,
    oit_set: Option<crate::oit::Set>,
}

pub(crate) fn glyphs(verts: &[crate::gui::Vertex]) -> Vec<Glyph> {
    // Triangles (0, 1, 2, 2, 3, 0): corners 0, 1, 2 then 3 at the fifth vertex.
    verts
        .as_chunks::<6>()
        .0
        .iter()
        .map(|q| {
            let (c0, c1, c2, c3) = (q[0], q[1], q[2], q[4]);
            let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
            let k = c0.color;
            Glyph {
                rect: [c0.pos[0], c3.pos[0], c0.pos[1], c1.pos[1]],
                skew: c1.pos[0] - c0.pos[0],
                uv: [c0.uv[0], c0.uv[1], c2.uv[0], c2.uv[1]],
                color: c(k[0]) | c(k[1]) << 8 | c(k[2]) << 16 | c(k[3]) << 24,
                mode: if c0.mode[0] == 1 { c0.mode[1] + 1 } else { 0 },
            }
        })
        .collect()
}

impl NameTags {
    pub fn new(device: &wgpu::Device, camera_layout: &wgpu::BindGroupLayout, format: wgpu::TextureFormat) -> Self {
        Self::with_mode(device, camera_layout, format, persist_on())
    }

    fn with_mode(
        device: &wgpu::Device,
        camera_layout: &wgpu::BindGroupLayout,
        format: wgpu::TextureFormat,
        persist: bool,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("name tags"),
            source: wgpu::ShaderSource::Wgsl(include_str!("nametag.wgsl").into()),
        });
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
        ];
        if persist {
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            });
        }
        let layout = device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("name tags"), entries: &entries });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("name tags"),
            bind_group_layouts: &[Some(camera_layout), Some(&layout)],
            immediate_size: 0,
        });
        let vs = if persist { "vs_tag" } else { "vs" };
        let pipeline = |label, compare, write: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &[Some(vertex_layout(persist))],
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
        let normal = pipeline("name tags", crate::render::DEPTH_NEARER, true);
        let see_through = pipeline("name tags (see-through)", wgpu::CompareFunction::Always, false);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let axes = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("name tags (axes)"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        NameTags {
            normal,
            see_through,
            layout,
            sampler,
            axes,
            bind: None,
            buf: None,
            store_buf: None,
            lists: Lists::new(persist),
            oit: false,
            camera_layout: camera_layout.clone(),
            oit_set: None,
        }
    }

    /// Improved transparency pipelines (once).
    pub fn ensure_oit(&mut self, device: &wgpu::Device, layouts: &crate::oit::Layouts) {
        if self.oit_set.is_some() {
            return;
        }
        let module = crate::oit::module(
            device,
            "name tags (transparency)",
            include_str!("nametag.wgsl"),
            include_str!("nametag_oit.wgsl"),
        );
        let groups = [Some(&self.camera_layout), Some(&self.layout)];
        let persist = self.lists.persist;
        let buffers = [Some(vertex_layout(persist))];
        let vs = if persist { "vs_tag" } else { "vs" };
        let spec = crate::oit::Spec::new("name tags (transparency)", &groups, &module, vs, &buffers, "fs");
        self.oit_set = Some(layouts.set(device, &spec));
    }

    /// Draws of a pass: (first instance, instances, glyphs at most per instance).
    fn segments(&self, pass: usize) -> &[(u32, u32, u32)] {
        let (a, b) = self.lists.passes[pass];
        &self.lists.segs[a as usize..b as usize]
    }

    fn draw_segments(&self, pass: &mut wgpu::RenderPass<'_>, which: usize) {
        for &(start, n, most) in self.segments(which) {
            pass.draw(0..6 * most, start..start + n);
        }
    }

    /// Improved transparency: opaque glyphs of the normal pass, with the solids.
    pub fn draw_solid(&self, pass: &mut wgpu::RenderPass<'_>) {
        self.draw(pass, &self.normal, SOLID);
    }

    /// Improved transparency: rest of the normal pass (flat fills, translucent text), stage `f`.
    pub fn draw_oit(&self, pass: &mut wgpu::RenderPass<'_>, f: crate::oit::Frame<'_>) {
        let (Some((bind, _)), Some((buf, _)), Some(set)) = (&self.bind, &self.buf, &self.oit_set) else { return };
        if self.segments(REST).is_empty() {
            return;
        }
        f.set(pass, set);
        pass.set_bind_group(1, bind, &[]);
        pass.set_vertex_buffer(0, buf.slice(..));
        self.draw_segments(pass, REST);
    }

    /// Layouts forgotten if `version` (teams) changed.
    pub fn sync(&mut self, version: u64) {
        self.lists.sync(version);
    }

    /// Background opacity from the options (`backgroundForChatOnly`, `textBackgroundOpacity`); the
    /// kept tags are redone if it changes.
    pub fn set_background(&mut self, opacity: f32) {
        self.lists.set_background(opacity);
    }

    /// No tag this frame.
    pub fn clear(&mut self) {
        self.lists.clear();
    }

    /// Start of frame: camera axes (right, up).
    pub fn begin(&mut self, right: Vec3, up: Vec3) {
        self.lists.begin(right, up);
    }

    /// A tag: entity `id`, name (`source`, `chars` computed only if the layout must be
    /// redone), `anchor` = attachment point + 0.5 in y (frame relative to the camera's
    /// origin), `discrete` = sneaking entity, `light` = (block, sky) of the entity.
    #[allow(clippy::too_many_arguments)]
    pub fn push(
        &mut self,
        gui: &mut Gui,
        queue: &wgpu::Queue,
        id: i32,
        source: NameRef<'_>,
        chars: impl FnOnce(&rc_text::Lang) -> Vec<StyledChar>,
        anchor: Vec3,
        discrete: bool,
        light: (u8, u8),
    ) {
        self.lists.push_with(id, source, anchor, discrete, light, |(text, background)| {
            let chars = chars(&gui.lang);
            let (normal, see) = if discrete {
                (glyphs(&gui.text_quads(queue, &chars, text, background)), Vec::new())
            } else {
                (
                    glyphs(&gui.text_quads(queue, &chars, 0xffff_ffff, 0)),
                    glyphs(&gui.text_quads(queue, &chars, text, background)),
                )
            };
            (normal, see, chars.iter().any(|c| c.style.obfuscated))
        });
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
        let persist = self.lists.persist;
        let send = self.lists.finish(self.oit);
        let mut from = usize::MAX;
        if persist {
            let Some(send) = send else { return };
            from = send.from;
            if let Some(cap) = send.grow {
                self.store_buf = Some(device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("name tags (glyphs)"),
                    size: (cap * size_of::<Stored>()) as u64,
                    usage: wgpu::BufferUsages::STORAGE
                        | wgpu::BufferUsages::COPY_DST
                        | if cfg!(test) { wgpu::BufferUsages::COPY_SRC } else { wgpu::BufferUsages::empty() },
                    mapped_at_creation: false,
                }));
                self.bind = None;
            }
        }
        if self.bind.as_ref().is_none_or(|b| b.1 != glyph_gen) {
            let view = view();
            let mut entries = vec![
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: self.axes.as_entire_binding() },
            ];
            if let Some(store) = &self.store_buf {
                entries.push(wgpu::BindGroupEntry { binding: 3, resource: store.as_entire_binding() });
            }
            self.bind = Some((
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("name tags"),
                    layout: &self.layout,
                    entries: &entries,
                }),
                glyph_gen,
            ));
        }
        let l = &self.lists;
        if persist {
            up.write(queue, &self.axes, 0, bytemuck::cast_slice(&l.right_up));
            if let Some(g) = l.store.glyphs.get(from..).filter(|g| !g.is_empty()) {
                let store = self.store_buf.as_ref().expect("pool created on first upload");
                up.write(queue, store, (from * size_of::<Stored>()) as u64, bytemuck::cast_slice(g));
            }
            let bytes: &[u8] = bytemuck::cast_slice(&l.refs);
            let buf = Self::instances(&mut self.buf, device, bytes.len() as u64);
            up.write(queue, buf, 0, bytes);
            return;
        }
        if l.counts == (0, 0) {
            return;
        }
        queue.write_buffer(&self.axes, 0, bytemuck::cast_slice(&l.right_up));
        let size = size_of::<TagInstance>() as u64;
        let need = (u64::from(l.counts.0) + u64::from(l.counts.1)) * size;
        let buf = Self::instances(&mut self.buf, device, need);
        queue.write_buffer(buf, 0, bytemuck::cast_slice(&l.normal_inst));
        queue.write_buffer(buf, u64::from(l.counts.0) * size, bytemuck::cast_slice(&l.see_inst));
    }

    /// Instance buffer of at least `need` bytes.
    fn instances<'a>(slot: &'a mut Option<(wgpu::Buffer, u64)>, device: &wgpu::Device, need: u64) -> &'a wgpu::Buffer {
        if slot.as_ref().is_none_or(|(_, cap)| *cap < need) {
            let cap = need.next_power_of_two().max(4096);
            *slot = Some((
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("name tags"),
                    size: cap,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                cap,
            ));
        }
        &slot.as_ref().expect("created above").0
    }

    /// Normal pass (with the entities); group 0 (camera) is already bound.
    pub fn draw_normal(&self, pass: &mut wgpu::RenderPass<'_>) {
        self.draw(pass, &self.normal, NORMAL);
    }

    /// "See-through" pass, after the translucent terrain.
    pub fn draw_see_through(&self, pass: &mut wgpu::RenderPass<'_>) {
        self.draw(pass, &self.see_through, SEE);
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, pipeline: &wgpu::RenderPipeline, which: usize) {
        let (Some((bind, _)), Some((buf, _))) = (&self.bind, &self.buf) else { return };
        if self.segments(which).is_empty() {
            return;
        }
        pass.set_pipeline(pipeline);
        pass.set_bind_group(1, bind, &[]);
        pass.set_vertex_buffer(0, buf.slice(..));
        self.draw_segments(pass, which);
    }
}

#[cfg(test)]
mod tests;
