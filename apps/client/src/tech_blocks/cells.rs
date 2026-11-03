//! Invisible blocks of a structure block's save area with "show air" on (air, structure void,
//! barrier, light): a small cube outline per cell. Up to 48³ cells, so instead of twelve debug
//! segments per cell built and sent every frame (1.33 M segments, 42 MB for a 48³ area of air), each
//! marked cell is one 16-byte GPU instance (absolute block, kind) and `tech_cells.wgsl` widens the
//! twelve edges of its cube exactly like the segments. The instances stay on the GPU as long as the
//! areas and the stamps of the columns they cover are unchanged (`rc_world::ChunkGrid::stamp`).
//!
//! Switches (read once): `RUSTCRAFT_OPT_STRUCTURE_CELL_INSTANCES=0` restores the segments built on the
//! CPU, `RUSTCRAFT_OPT_STRUCTURE_CELL_CACHE=0` walks and uploads the cells every frame,
//! `RUSTCRAFT_OPT_STRUCTURE_MARKER_TABLE=0` compares block names per cell instead of reading a byte
//! per state, `RUSTCRAFT_OPT_STRUCTURE_PARALLEL_WALK=1` walks the x slices on the thread pool.

use rayon::prelude::*;

use crate::environ::WorldView;

/// Kind of invisible block marked by a cube (0: block not marked).
pub(crate) const NOT_MARKED: u8 = 0;
const AIR: u8 = 1;
const STRUCTURE_VOID: u8 = 2;
const BARRIER: u8 = 3;
const LIGHT: u8 = 4;
const MARKED_KINDS: [u8; 4] = [AIR, STRUCTURE_VOID, BARRIER, LIGHT];

/// Outline thickness of a marker cube, in pixels.
pub(crate) const MARKER_LINE_WIDTH: f32 = 2.5;

/// Marker kind of a block, by its registry name.
pub(crate) fn marker_kind(name: &str) -> u8 {
    match name {
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air" => AIR,
        "minecraft:structure_void" => STRUCTURE_VOID,
        "minecraft:barrier" => BARRIER,
        "minecraft:light" => LIGHT,
        _ => NOT_MARKED,
    }
}

/// ARGB colour of the cube of a marked kind and its growth around the 0.45..0.55 core of the cell.
pub(crate) fn marker_style(kind: u8) -> (u32, f32) {
    match kind {
        AIR => (0xFF7F_7FFF, 0.05),
        STRUCTURE_VOID => (0xFFFF_BFBF, 0.0),
        BARRIER => (0xFFFF_0000, 0.0),
        _ => (0xFFFF_FF00, 0.0),
    }
}

/// Low and high corner of the cube inside its cell, as single-precision sums like the game's floats.
/// The shader receives these values instead of computing them: a WGSL constant `0.45 - 0.05` folds
/// to exactly 0.4, one unit in the last place away from `0.45f32 - 0.05f32`.
pub(crate) fn marker_corners(growth: f32) -> (f32, f32) {
    (0.45 - growth, 0.55 + growth)
}

/// Marker kind of every block state: one byte read per cell instead of a name comparison.
pub(crate) fn marker_kinds(data: &rc_data::VersionData) -> Vec<u8> {
    let by_block: Vec<u8> = data.blocks.iter().map(|b| marker_kind(&b.name)).collect();
    data.states.iter().map(|s| by_block.get(usize::from(s.block)).copied().unwrap_or(NOT_MARKED)).collect()
}

/// How the marker kind of a state is found.
#[derive(Clone, Copy)]
pub(crate) enum Classifier<'a> {
    /// Byte per state (`marker_kinds`).
    Table(&'a [u8]),
    /// Name of the state's block, compared per cell (old path).
    Names(&'a rc_data::VersionData),
}

impl Classifier<'_> {
    #[inline]
    pub(crate) fn kind(&self, state: u32) -> u8 {
        match self {
            Classifier::Table(t) => t.get(state as usize).copied().unwrap_or(NOT_MARKED),
            Classifier::Names(d) => d.block_of(state).map_or(NOT_MARKED, |b| marker_kind(&b.name)),
        }
    }
}

/// Switches of the show-air rendering (all on by default except the parallel walk).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CellOptions {
    /// One GPU instance per cell (otherwise twelve segments per cell built on the CPU).
    pub instances: bool,
    /// Cells kept while the areas and their columns are unchanged.
    pub cache: bool,
    /// Byte per state instead of a name comparison.
    pub table: bool,
    /// Walk split into x slices on the thread pool. Off by default: with the cache the walk only
    /// runs after a change in the covered columns, and the pool is the meshing one, whose queued
    /// jobs the frame would wait behind.
    pub parallel: bool,
}

impl CellOptions {
    /// Defaults of `from_env` (the proofs pin them whatever the environment).
    #[cfg(test)]
    pub(crate) const NEW: CellOptions = CellOptions { instances: true, cache: true, table: true, parallel: false };

    pub(crate) fn from_env() -> CellOptions {
        let on = |v: Option<&str>| v.is_none_or(|v| v != "0");
        CellOptions {
            instances: on(rc_data::env_str!("RUSTCRAFT_OPT_STRUCTURE_CELL_INSTANCES")),
            cache: on(rc_data::env_str!("RUSTCRAFT_OPT_STRUCTURE_CELL_CACHE")),
            table: on(rc_data::env_str!("RUSTCRAFT_OPT_STRUCTURE_MARKER_TABLE")),
            parallel: rc_data::env_str!("RUSTCRAFT_OPT_STRUCTURE_PARALLEL_WALK").is_some_and(|v| v != "0"),
        }
    }
}

/// A marked cell as sent to the GPU: absolute block (the buffer stays valid while the camera
/// moves) and marker kind.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct CellInstance {
    pub block: [i32; 3],
    pub kind: u32,
}

/// Save area drawn with its invisible blocks: first block and size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Area {
    pub start: [i32; 3],
    pub size: [i32; 3],
}

/// Marked cells of the x slice `x` of an area, in y then z order.
fn walk_slice(area: Area, x: i32, world: &WorldView<'_>, classify: Classifier<'_>, out: &mut Vec<CellInstance>) {
    let bx = area.start[0] + x;
    for y in 0..area.size[1] {
        let by = area.start[1] + y;
        for z in 0..area.size[2] {
            let bz = area.start[2] + z;
            let kind = classify.kind(world.state(bx, by, bz));
            if kind != NOT_MARKED {
                out.push(CellInstance { block: [bx, by, bz], kind: u32::from(kind) });
            }
        }
    }
}

/// Marked cells of an area in x, y, z order (the order of the old outlines).
pub(super) fn walk_area(
    area: Area,
    world: &WorldView<'_>,
    classify: Classifier<'_>,
    parallel: bool,
    out: &mut Vec<CellInstance>,
) {
    if !parallel {
        for x in 0..area.size[0] {
            walk_slice(area, x, world, classify, out);
        }
        return;
    }
    // One slice per x on the pool, concatenated in x order: same list as the single-threaded walk.
    let slices: Vec<Vec<CellInstance>> = (0..area.size[0])
        .into_par_iter()
        .map(|x| {
            let mut slice = Vec::new();
            walk_slice(area, x, world, classify, &mut slice);
            slice
        })
        .collect();
    for slice in slices {
        out.extend_from_slice(&slice);
    }
}

/// Key of an area: its bounds, the world's height range and the stamp of every column it covers.
/// Equal keys from one frame to the next: no block of the area can have changed.
fn push_area_key(key: &mut Vec<u64>, area: Area, world: &WorldView<'_>) {
    let as_key = |v: i32| u64::from(v as u32);
    key.extend(area.start.iter().chain(&area.size).map(|&v| as_key(v)));
    key.push(as_key(world.min_y));
    key.push(as_key(world.height));
    let (x0, x1) = (area.start[0] >> 4, (area.start[0] + area.size[0] - 1) >> 4);
    let (z0, z1) = (area.start[2] >> 4, (area.start[2] + area.size[2] - 1) >> 4);
    for cx in x0..=x1 {
        for cz in z0..=z1 {
            key.push(world.grid.stamp(cx, cz));
        }
    }
}

/// Show-air areas of the frame, their marked cells (kept between frames) and their GPU copy.
pub struct ShowAirCells {
    pub(crate) options: CellOptions,
    format: wgpu::TextureFormat,
    /// Areas declared this frame and their key.
    areas: Vec<Area>,
    frame_key: Vec<u64>,
    /// Key of the areas `cells` was built from.
    built_key: Vec<u64>,
    cells: Vec<CellInstance>,
    /// Bumped at each rebuild: the GPU copy is rewritten when it lags behind.
    generation: u64,
    /// Integer origin (x, z) of the frame of the debug lines.
    origin: [i32; 2],
    /// At least one area is drawn this frame.
    shown: bool,
    gpu: Option<CellGpu>,
}

impl ShowAirCells {
    pub(crate) fn new(format: wgpu::TextureFormat) -> ShowAirCells {
        ShowAirCells::with_options(format, CellOptions::from_env())
    }

    pub(crate) fn with_options(format: wgpu::TextureFormat, options: CellOptions) -> ShowAirCells {
        ShowAirCells {
            options,
            format,
            areas: Vec::new(),
            frame_key: Vec::new(),
            built_key: Vec::new(),
            cells: Vec::new(),
            generation: 0,
            origin: [0; 2],
            shown: false,
            gpu: None,
        }
    }

    /// Start of a frame: no area until `add_area` (the built cells are kept).
    pub(crate) fn begin_frame(&mut self) {
        self.areas.clear();
        self.frame_key.clear();
        self.shown = false;
    }

    pub(crate) fn add_area(&mut self, start: [i32; 3], size: [i32; 3], world: &WorldView<'_>) {
        let area = Area { start, size };
        self.areas.push(area);
        push_area_key(&mut self.frame_key, area, world);
    }

    /// End of the areas of the frame: cells rebuilt if an area or one of its columns changed.
    pub(crate) fn finish_frame(&mut self, world: &WorldView<'_>, classify: Classifier<'_>, origin: [i32; 2]) {
        self.origin = origin;
        self.shown = !self.areas.is_empty();
        if !self.shown || (self.options.cache && self.frame_key == self.built_key) {
            return;
        }
        self.cells.clear();
        for &area in &self.areas {
            walk_area(area, world, classify, self.options.parallel, &mut self.cells);
        }
        std::mem::swap(&mut self.built_key, &mut self.frame_key);
        self.generation += 1;
    }

    /// Cells drawn this frame (empty if no area is shown).
    pub(crate) fn cells(&self) -> &[CellInstance] {
        if self.shown { &self.cells } else { &[] }
    }

    #[cfg(test)]
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// GPU copy of the frame: the instances only after a rebuild, the frame values (96 bytes) every
    /// frame.
    pub(crate) fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layouts: (&wgpu::BindGroupLayout, &wgpu::BindGroupLayout),
        clip: Option<([f32; 3], [f32; 3])>,
    ) {
        if !self.shown {
            return;
        }
        let format = self.format;
        let gpu = self.gpu.get_or_insert_with(|| CellGpu::new(device, layouts, format));
        if gpu.uploaded != self.generation {
            if !self.cells.is_empty() {
                let bytes: &[u8] = bytemuck::cast_slice(&self.cells);
                let buf = crate::debug_render::ensure(device, &mut gpu.instances, bytes.len() as u64, "show-air cells");
                queue.write_buffer(buf, 0, bytes);
            }
            gpu.count = self.cells.len() as u32;
            gpu.uploaded = self.generation;
        }
        if gpu.count > 0 {
            let frame = CellFrame::new(self.origin, clip);
            queue.write_buffer(&gpu.frame, 0, bytemuck::bytes_of(&frame));
        }
    }

    /// Bytes `upload` sends this frame: the frame values, plus the instances after a rebuild.
    #[cfg(test)]
    pub(crate) fn bytes_to_send(&self) -> u64 {
        let stale = self.gpu.as_ref().is_none_or(|g| g.uploaded != self.generation);
        match self.cells() {
            [] => 0,
            cells if stale => (std::mem::size_of_val(cells) + std::mem::size_of::<CellFrame>()) as u64,
            _ => std::mem::size_of::<CellFrame>() as u64,
        }
    }

    /// Draws the cubes (opaque, depth written, like the opaque debug segments); group 0 = world camera,
    /// `style` = bind group of the debug lines (screen size).
    pub(crate) fn draw(&self, pass: &mut wgpu::RenderPass<'_>, style: &wgpu::BindGroup) {
        let current = |g: &&CellGpu| self.shown && g.uploaded == self.generation && g.count > 0;
        let Some(gpu) = self.gpu.as_ref().filter(current) else { return };
        let Some((buf, _)) = &gpu.instances else { return };
        pass.set_pipeline(&gpu.pipeline);
        pass.set_bind_group(1, style, &[]);
        pass.set_bind_group(2, &gpu.bind, &[]);
        pass.set_vertex_buffer(0, buf.slice(..));
        pass.draw(0..CELL_VERTICES, 0..gpu.count);
    }
}

/// Vertices of a cell: 12 edges × 6.
const CELL_VERTICES: u32 = 72;

/// Bytes of the frame values sent every frame while an area is shown.
#[cfg(test)]
pub(super) const CELL_FRAME_BYTES: usize = std::mem::size_of::<CellFrame>();

/// Frame values of `tech_cells.wgsl` (`CellFrame`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct CellFrame {
    origin: [i32; 4],
    clip_eye: [f32; 4],
    clip_forward: [f32; 4],
    colors: [u32; 4],
    low: [f32; 4],
    high: [f32; 4],
}

impl CellFrame {
    fn new(origin: [i32; 2], clip: Option<([f32; 3], [f32; 3])>) -> CellFrame {
        let (eye, forward) = clip.unwrap_or_default();
        let style = MARKED_KINDS.map(marker_style);
        let corners = style.map(|(_, growth)| marker_corners(growth));
        CellFrame {
            origin: [origin[0], 0, origin[1], 0],
            clip_eye: [eye[0], eye[1], eye[2], if clip.is_some() { 1.0 } else { 0.0 }],
            clip_forward: [forward[0], forward[1], forward[2], 0.0],
            colors: style.map(|(color, _)| u32::from_le_bytes(crate::debug_render::rgba(color))),
            low: corners.map(|c| c.0),
            high: corners.map(|c| c.1),
        }
    }
}

/// Pipeline, frame values and instances of the cells, made at the first show-air area.
struct CellGpu {
    pipeline: wgpu::RenderPipeline,
    frame: wgpu::Buffer,
    bind: wgpu::BindGroup,
    instances: Option<(wgpu::Buffer, u64)>,
    /// Generation of the instances on the GPU, and their number.
    uploaded: u64,
    count: u32,
}

impl CellGpu {
    fn new(
        device: &wgpu::Device,
        (camera_layout, style_layout): (&wgpu::BindGroupLayout, &wgpu::BindGroupLayout),
        format: wgpu::TextureFormat,
    ) -> CellGpu {
        let source = format!("{}\n{}", include_str!("../debug_lines.wgsl"), include_str!("../tech_cells.wgsl"));
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("show-air cells"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("show-air cells"),
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
        let frame = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("show-air cells"),
            size: std::mem::size_of::<CellFrame>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("show-air cells"),
            layout: &frame_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: frame.as_entire_binding() }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("show-air cells"),
            bind_group_layouts: &[Some(camera_layout), Some(style_layout), Some(&frame_layout)],
            immediate_size: 0,
        });
        // Same states as the opaque debug segments: depth tested and written, no blending.
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("show-air cells"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_cell"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<CellInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Sint32x4],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(crate::render::DEPTH_NEARER),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        CellGpu { pipeline, frame, bind, instances: None, uploaded: u64::MAX, count: 0 }
    }
}
