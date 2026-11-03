//! Equivalence proofs of the show-air cells (`RUSTCRAFT_OPT_STRUCTURE_*`): the list of (cell, colour,
//! growth) of the cells equals the old outline list (names compared per cell, x, y, z order) on random
//! 48³ areas, then after 100 block and column changes (cache keyed by the column stamps); on the
//! real GPU, the instanced cubes draw the same image as the twelve CPU segments per cell, camera
//! outside and inside the area, inside a cube and with edges behind the eye.
//!
//! Micro-benches (loaded machine, ignored): `cargo test --release -p rc-app bench_show_air -- --ignored
//! --nocapture`.

use std::sync::Arc;
use std::time::Instant;

use rc_world::{ChunkGrid, Column, Container, Section};

use super::cells::{Area, CellInstance, CellOptions, Classifier, ShowAirCells, marker_kinds, marker_style, walk_area};
use super::*;

const MIN_Y: i32 = -64;
const HEIGHT: i32 = 384;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Deterministic xorshift (no clock in a proof).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// States drawn in the test world: the marked blocks (light at every level), and visible ones.
struct Palette {
    marked: Vec<u32>,
    visible: Vec<u32>,
    direct_bits: u8,
}

fn palette(d: &rc_data::VersionData) -> Palette {
    let states_of = |name: &str| {
        let b = d.blocks.iter().position(|b| b.name == name).expect(name);
        (0..d.states.len() as u32).filter(move |&s| usize::from(d.states[s as usize].block) == b)
    };
    let mut marked: Vec<u32> =
        ["minecraft:air", "minecraft:cave_air", "minecraft:void_air"].iter().flat_map(|n| states_of(n)).collect();
    marked.extend(states_of("minecraft:structure_void"));
    marked.extend(states_of("minecraft:barrier"));
    marked.extend(states_of("minecraft:light"));
    let visible = ["minecraft:stone", "minecraft:glass", "minecraft:water", "minecraft:oak_stairs", "minecraft:chest"]
        .iter()
        .flat_map(|n| states_of(n).take(6))
        .collect();
    Palette { marked, visible, direct_bits: (usize::BITS - d.states.len().leading_zeros()) as u8 }
}

impl Palette {
    /// Half marked (air first), half visible.
    fn draw(&self, rng: &mut Rng) -> u32 {
        match rng.below(4) {
            0 => 0,
            1 => self.marked[rng.below(self.marked.len())],
            _ => self.visible[rng.below(self.visible.len())],
        }
    }
}

/// Column with random blocks between y = 40 and 140, stone below, air above.
fn column(p: &Palette, rng: &mut Rng, cx: i32, cz: i32) -> Column {
    let stone = p.visible[0];
    let sections: Vec<Section> = (0..HEIGHT / 16)
        .map(|si| {
            let y0 = MIN_Y + si * 16;
            let values: Vec<u32> = (0..4096)
                .map(|i| {
                    let y = y0 + (i >> 8);
                    if y < 40 {
                        stone
                    } else if y < 140 {
                        p.draw(rng)
                    } else {
                        0
                    }
                })
                .collect();
            let non_empty = values.iter().filter(|&&v| v != 0).count() as i16;
            Section { non_empty, blocks: Container::pack(&values, 4, 8, p.direct_bits), biomes: Container::Single(0) }
        })
        .collect();
    let n = sections.len() + 2;
    Column {
        x: cx,
        z: cz,
        sections: sections.into_boxed_slice(),
        sky_light: vec![None; n].into_boxed_slice(),
        block_light: vec![Some(Arc::new([0x77; rc_world::LIGHT_BYTES])); n].into_boxed_slice(),
        heightmaps: Vec::new(),
    }
}

/// Columns -1..=4 on both axes (a corner left empty: state 0 there).
fn world(p: &Palette, rng: &mut Rng) -> ChunkGrid {
    let mut grid = ChunkGrid::new(8);
    for cx in -1..=4 {
        for cz in -1..=4 {
            if (cx, cz) != (4, 4) {
                grid.insert(column(p, rng, cx, cz));
            }
        }
    }
    grid
}

fn set(grid: &mut ChunkGrid, p: &Palette, at: [i32; 3], state: u32) {
    let Some(c) = grid.get_mut(at[0] >> 4, at[2] >> 4) else { return };
    let ly = at[1] - MIN_Y;
    let i = rc_world::block_index((at[0] & 15) as usize, (ly & 15) as usize, (at[2] & 15) as usize) as u16;
    c.sections[(ly >> 4) as usize].set_blocks(&[(i, state)], p.direct_bits, |v| v == 0);
}

/// The old outline list, written as before the cells (state per cell, block name compared).
fn old_outline_list(world: &WorldView<'_>, data: &rc_data::VersionData, areas: &[Area]) -> Vec<([i32; 3], u32, f32)> {
    let mut out = Vec::new();
    for a in areas {
        for x in 0..a.size[0] {
            for y in 0..a.size[1] {
                for z in 0..a.size[2] {
                    let (bx, by, bz) = (a.start[0] + x, a.start[1] + y, a.start[2] + z);
                    let Some(b) = data.block_of(world.state(bx, by, bz)) else { continue };
                    let (color, scale) = match b.name.as_str() {
                        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air" => (0xFF7F_7FFF, 0.05),
                        "minecraft:structure_void" => (0xFFFF_BFBF, 0.0),
                        "minecraft:barrier" => (0xFFFF_0000, 0.0),
                        "minecraft:light" => (0xFFFF_FF00, 0.0),
                        _ => continue,
                    };
                    out.push(([bx, by, bz], color, scale));
                }
            }
        }
    }
    out
}

fn styled(cells: &[CellInstance]) -> Vec<([i32; 3], u32, f32)> {
    cells
        .iter()
        .map(|c| {
            let (color, growth) = marker_style(c.kind as u8);
            (c.block, color, growth)
        })
        .collect()
}

/// Areas of the proofs: unaligned 48³ in the random layer, one reaching the empty corner column
/// and one crossing the top of the world.
const AREAS: [Area; 3] = [
    Area { start: [5, 61, 7], size: [48, 48, 48] },
    Area { start: [40, 90, 41], size: [33, 20, 37] },
    Area { start: [-9, 300, -3], size: [12, 30, 9] },
];

fn frame(cells: &mut ShowAirCells, world: &WorldView<'_>, classify: Classifier<'_>, areas: &[Area]) {
    cells.begin_frame();
    for a in areas {
        cells.add_area(a.start, a.size, world);
    }
    cells.finish_frame(world, classify, [0, 0]);
}

#[test]
fn cell_list_matches_the_old_outline_list() {
    let Some(data) = rc_testdata::version_data() else { return };
    let p = palette(&data);
    let kinds = marker_kinds(&data);
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let grid = world(&p, &mut rng);
    let world = WorldView { grid: &grid, min_y: MIN_Y, height: HEIGHT };
    let expected = old_outline_list(&world, &data, &AREAS);
    let count = |color: u32| expected.iter().filter(|e| e.1 == color).count();
    for color in [0xFF7F_7FFF, 0xFFFF_BFBF, 0xFFFF_0000, 0xFFFF_FF00] {
        assert!(count(color) > 500, "colour {color:08X}: only {} cells, the test proves nothing", count(color));
    }
    for table in [false, true] {
        for parallel in [false, true] {
            let options = CellOptions { table, parallel, ..CellOptions::NEW };
            let mut cells = ShowAirCells::with_options(FORMAT, options);
            let classify = if table { Classifier::Table(&kinds) } else { Classifier::Names(&data) };
            frame(&mut cells, &world, classify, &AREAS);
            assert_eq!(styled(cells.cells()), expected, "table={table} parallel={parallel}");
        }
    }
}

/// 100 frames: block changes inside and outside the areas, columns replaced and removed, frames
/// without change. The cached list always equals the list rebuilt from scratch, and a frame without
/// any change in the covered columns rebuilds nothing.
#[test]
fn cached_cells_follow_100_changes() {
    let Some(data) = rc_testdata::version_data() else { return };
    let p = palette(&data);
    let kinds = marker_kinds(&data);
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    let mut grid = world(&p, &mut rng);
    let mut cells = ShowAirCells::with_options(FORMAT, CellOptions::NEW);
    let areas = &AREAS[..2];
    let (mut rebuilds, mut reused) = (0, 0);
    for step in 0..100 {
        let changed = match step % 10 {
            // Nothing.
            0 | 5 => false,
            // Column far from the areas: no rebuild expected.
            3 => {
                let mut col = column(&p, &mut rng, -1, -1);
                col.x = -1;
                grid.insert(col);
                false
            }
            // Covered column removed, then put back.
            7 => {
                grid.remove(1, 2);
                true
            }
            8 => {
                grid.insert(column(&p, &mut rng, 1, 2));
                true
            }
            // Blocks set inside an area (marked or visible).
            _ => {
                for _ in 0..1 + rng.below(4) {
                    let a = areas[rng.below(areas.len())];
                    let at: [i32; 3] = std::array::from_fn(|i| a.start[i] + rng.below(a.size[i] as usize) as i32);
                    set(&mut grid, &p, at, p.draw(&mut rng));
                }
                true
            }
        };
        let world = WorldView { grid: &grid, min_y: MIN_Y, height: HEIGHT };
        let before = cells.generation();
        frame(&mut cells, &world, Classifier::Table(&kinds), areas);
        let rebuilt = cells.generation() != before;
        if step > 0 {
            assert_eq!(rebuilt, changed, "step {step}: rebuild {rebuilt}, change {changed}");
        }
        if rebuilt {
            rebuilds += 1
        } else {
            reused += 1
        }
        assert_eq!(styled(cells.cells()), old_outline_list(&world, &data, areas), "step {step}");
    }
    assert!(rebuilds > 50 && reused > 20, "{rebuilds} rebuilds, {reused} reuses");
    // No area this frame: nothing drawn, cells kept for the next frame with the same key.
    let world = WorldView { grid: &grid, min_y: MIN_Y, height: HEIGHT };
    let before = cells.generation();
    frame(&mut cells, &world, Classifier::Table(&kinds), &[]);
    assert!(cells.cells().is_empty());
    frame(&mut cells, &world, Classifier::Table(&kinds), areas);
    assert_eq!(cells.generation(), before);
    assert_eq!(styled(cells.cells()), old_outline_list(&world, &data, areas));
}

/// One byte per state, same kind as the name comparison for every state of the version.
#[test]
fn marker_table_matches_the_names() {
    let Some(data) = rc_testdata::version_data() else { return };
    let kinds = marker_kinds(&data);
    assert_eq!(kinds.len(), data.states.len());
    let marked = (0..data.states.len() as u32)
        .filter(|&s| {
            let by_name = Classifier::Names(&data).kind(s);
            assert_eq!(Classifier::Table(&kinds).kind(s), by_name, "state {s}");
            by_name != cells::NOT_MARKED
        })
        .count();
    // air, cave_air, void_air, structure_void, barrier (2 states), light (32 states).
    assert_eq!(marked, 38);
    assert_eq!(Classifier::Table(&kinds).kind(u32::MAX), cells::NOT_MARKED);
}

/// Sizes the shader reads: 16 bytes per cell (against 12 segments of 32 bytes), 96 bytes per frame.
#[test]
fn gpu_formats_keep_their_size() {
    assert_eq!(std::mem::size_of::<CellInstance>(), 16);
    assert_eq!(std::mem::size_of::<crate::debug_render::Segment>(), 32);
    assert_eq!(cells::CELL_FRAME_BYTES, 96);
}

// --- Image on the real GPU ----------------------------------------------------------------------

/// Hardware device on one of `backends` (Vulkan, the default, and DirectX 12, the fallback).
fn gpu_on(backends: wgpu::Backends) -> Option<(wgpu::Device, wgpu::Queue)> {
    crate::render::upload::tests::gpu_serial();
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .ok();
    // A software adapter (WARP, llvmpipe) is not the real GPU these proofs are about.
    let hardware = adapter.filter(|a| a.get_info().device_type != wgpu::DeviceType::Cpu);
    let adapter = rc_testdata::need(hardware, "hardware GPU adapter")?;
    eprintln!("GPU: {} ({:?})", adapter.get_info().name, adapter.get_info().backend);
    Some(pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("device"))
}

fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    gpu_on(wgpu::Backends::PRIMARY)
}

const SIZE: u32 = 512;

/// Camera of a view: eye, yaw, pitch.
#[derive(Clone, Copy)]
struct View {
    name: &'static str,
    eye: [f64; 3],
    yaw: f32,
    pitch: f32,
}

/// Views: outside looking at the area, inside it, eye in a marker cube (its edges cross the cut
/// plane), looking along an edge row with cubes behind the eye, eye 0.04 block beside an edge of
/// its cube that crosses the cut plane on screen.
const VIEWS: [View; 5] = [
    View { name: "outside", eye: [29.3, 120.7, -30.2], yaw: 0.0, pitch: 25.0 },
    View { name: "inside", eye: [28.4, 84.6, 30.1], yaw: 37.0, pitch: 12.0 },
    View { name: "in a cube", eye: [20.52, 80.47, 22.5], yaw: 90.0, pitch: 0.0 },
    View { name: "edges behind", eye: [12.5, 70.43, 30.6], yaw: -90.0, pitch: -3.0 },
    View { name: "edge at the eye", eye: [20.5, 80.38, 22.38], yaw: 90.0, pitch: 0.0 },
];

/// GPU objects shared by the captures: camera layout (uniform, light map read by the debug texts),
/// light map and target.
struct Target {
    cam_layout: wgpu::BindGroupLayout,
    lightmap: wgpu::TextureView,
    sampler: wgpu::Sampler,
    color: wgpu::Texture,
    color_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
}

fn target(device: &wgpu::Device) -> Target {
    let entry = |binding, visibility, ty| wgpu::BindGroupLayoutEntry { binding, visibility, ty, count: None };
    let uniform = wgpu::BindingType::Buffer {
        ty: wgpu::BufferBindingType::Uniform,
        has_dynamic_offset: false,
        min_binding_size: None,
    };
    let map = wgpu::BindingType::Texture {
        sample_type: wgpu::TextureSampleType::Float { filterable: true },
        view_dimension: wgpu::TextureViewDimension::D2,
        multisampled: false,
    };
    let cam_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[
            entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT, uniform),
            entry(7, wgpu::ShaderStages::VERTEX_FRAGMENT, map),
            entry(
                8,
                wgpu::ShaderStages::VERTEX_FRAGMENT,
                wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            ),
        ],
    });
    let texture = |format, usage| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    };
    let color = texture(FORMAT, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
    let depth = texture(wgpu::TextureFormat::Depth32Float, wgpu::TextureUsages::RENDER_ATTACHMENT);
    let (color_view, depth_view) = (color.create_view(&Default::default()), depth.create_view(&Default::default()));
    let lightmap = texture(FORMAT, wgpu::TextureUsages::TEXTURE_BINDING).create_view(&Default::default());
    let sampler = device.create_sampler(&Default::default());
    Target { cam_layout, lightmap, sampler, color, color_view, depth_view }
}

/// World camera uniform of a view (layout of `Camera` in debug_lines.wgsl), no fog.
fn camera(device: &wgpu::Device, t: &Target, v: View) -> wgpu::BindGroup {
    use wgpu::util::DeviceExt;
    let (origin, local, view_m) = crate::camera::view_matrix(v.eye, v.yaw, v.pitch);
    let view_proj = crate::render::perspective(70.0, 1.0, 0.05, 1000.0) * view_m;
    let mut words = [0u32; 64];
    let floats: [f32; 16] = view_proj.to_cols_array();
    for (w, f) in words.iter_mut().zip(floats) {
        *w = f.to_bits();
    }
    words[16..20].copy_from_slice(&[origin[0] as u32, MIN_Y as u32, origin[1] as u32, 0]);
    for (i, f) in [local.x, local.y, local.z, 0.0].into_iter().enumerate() {
        words[28 + i] = f.to_bits();
    }
    for (i, f) in [1e9f32, 2e9, 1e9, 2e9].into_iter().enumerate() {
        words[36 + i] = f.to_bits();
    }
    let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(&words),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &t.cam_layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: buf.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::TextureView(&t.lightmap) },
            wgpu::BindGroupEntry { binding: 8, resource: wgpu::BindingResource::Sampler(&t.sampler) },
        ],
    })
}

/// Structure block in save mode with "show air", whose area starts at `start`.
fn tech_with_area(data: &rc_data::VersionData, start: [i32; 3], size: [i32; 3]) -> TechBlocks {
    let mut tech = TechBlocks::new(data);
    let mut s = Structure::new(0);
    s.show_air = true;
    s.size = size;
    tech.map.insert([start[0], start[1] - 1, start[2]], Tech::Structure(s));
    tech
}

/// Fills the debug lines of a frame as the game does (clear, cut plane, boxes) and returns the
/// bytes sent to the GPU by the upload.
fn fill_frame(lines: &mut DebugLines, tech: &TechBlocks, world: &WorldView<'_>, data: &rc_data::VersionData, v: View) {
    lines.clear();
    let origin = [v.eye[0].floor() as i32, v.eye[2].floor() as i32];
    let eye = [(v.eye[0] - f64::from(origin[0])) as f32, v.eye[1] as f32, (v.eye[2] - f64::from(origin[1])) as f32];
    lines.clip = Some((eye, crate::camera::axes(v.yaw, v.pitch).2.to_array()));
    draw_boxes(lines, tech, v.eye, true, 12, world, data);
}

fn render(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    t: &Target,
    lines: &DebugLines,
    cam: &wgpu::BindGroup,
) -> Vec<u8> {
    let mut enc = device.create_command_encoder(&Default::default());
    {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &t.color_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.1, g: 0.2, b: 0.3, a: 1.0 }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &t.depth_view,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: wgpu::StoreOp::Store }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        pass.set_bind_group(0, cam, &[]);
        lines.draw(&mut pass, Layer::Depth);
    }
    let out = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(SIZE * SIZE * 4),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    enc.copy_texture_to_buffer(
        t.color.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &out,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(SIZE * 4), rows_per_image: None },
        },
        wgpu::Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
    );
    queue.submit([enc.finish()]);
    out.slice(..).map_async(wgpu::MapMode::Read, |r| r.expect("read"));
    device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("GPU wait");
    out.slice(..).get_mapped_range().expect("mapping").to_vec()
}

/// Image of a view with the given options, and the number of CPU segments of the frame.
#[allow(clippy::too_many_arguments)]
fn capture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    t: &Target,
    tech: &TechBlocks,
    world: &WorldView<'_>,
    data: &rc_data::VersionData,
    v: View,
    options: CellOptions,
) -> (Vec<u8>, usize) {
    let mut lines = DebugLines::new(device, &t.cam_layout, FORMAT);
    lines.show_air.options = options;
    fill_frame(&mut lines, tech, world, data, v);
    lines.upload_geometry(device, queue, (SIZE, SIZE));
    (render(device, queue, t, &lines, &camera(device, t, v)), lines.segment_count())
}

/// Pixels that differ, and the largest channel difference.
fn diff(a: &[u8], b: &[u8]) -> (usize, u8) {
    let pixels = a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0);
    pixels.fold((0, 0), |(n, max), (p, q)| {
        let d = p.iter().zip(q).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0);
        (n + usize::from(d > 0), max.max(d))
    })
}

const OLD: CellOptions = CellOptions { instances: false, cache: false, table: false, parallel: false };

#[test]
fn instanced_cells_draw_the_segment_image_on_vulkan() {
    segment_image_matches_on(wgpu::Backends::VULKAN);
}

/// Fallback backend: the shader goes through another compiler (HLSL).
#[cfg(windows)]
#[test]
fn instanced_cells_draw_the_segment_image_on_dx12() {
    segment_image_matches_on(wgpu::Backends::DX12);
}

fn segment_image_matches_on(backends: wgpu::Backends) {
    let Some(data) = rc_testdata::version_data() else { return };
    let Some((device, queue)) = gpu_on(backends) else { return };
    let p = palette(&data);
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let mut grid = world(&p, &mut rng);
    // Air around the eye of the last two views: its cube contains the eye, so its edges cross the
    // cut plane (cut) or lie behind it (dropped).
    for v in &VIEWS[2..] {
        set(&mut grid, &p, v.eye.map(|c| c.floor() as i32), 0);
    }
    let area = AREAS[0];
    let tech = tech_with_area(&data, area.start, area.size);
    let t = target(&device);
    let mut differing = Vec::new();
    for v in VIEWS {
        let world = WorldView { grid: &grid, min_y: MIN_Y, height: HEIGHT };
        let (old, segments) = capture(&device, &queue, &t, &tech, &world, &data, v, OLD);
        let (new, _) = capture(&device, &queue, &t, &tech, &world, &data, v, CellOptions::NEW);
        let marked = old_outline_list(&world, &data, &[area]).len();
        let lit = old.as_chunks::<4>().0.iter().filter(|px| px[..] != old[..4]).count();
        let (n, max) = diff(&old, &new);
        eprintln!(
            "{}: {marked} cubes, {} segments dropped behind the eye, {lit} lit pixels, {n} differ (max {max})",
            v.name,
            12 * (marked + 1) - segments
        );
        assert!(lit > 2_000, "{}: cubes missing ({lit} pixels)", v.name);
        if n > 0 {
            differing.push((v.name, n, max));
        }
    }
    assert!(differing.is_empty(), "views whose pixels differ (view, pixels, largest difference): {differing:?}");
    // Sensitivity: the first marked cell in front of the "in a cube" camera (looking towards −x)
    // turned into stone must be seen.
    let v = VIEWS[2];
    let world = WorldView { grid: &grid, min_y: MIN_Y, height: HEIGHT };
    let (old, _) = capture(&device, &queue, &t, &tech, &world, &data, v, OLD);
    let near = (area.start[0]..19)
        .rev()
        .map(|x| [x, 80, 22])
        .find(|&b| Classifier::Names(&data).kind(world.state(b[0], b[1], b[2])) != cells::NOT_MARKED)
        .expect("a marked cell in front of the camera");
    set(&mut grid, &p, near, p.visible[0]);
    let world = WorldView { grid: &grid, min_y: MIN_Y, height: HEIGHT };
    let (new, _) = capture(&device, &queue, &t, &tech, &world, &data, v, CellOptions::NEW);
    let (n, _) = diff(&old, &new);
    assert!(n > 20, "a missing cube must change the image ({n} pixels)");
}

// --- Micro-benches (loaded machine) ------------------------------------------------------------

/// Walk of a 48³ area: names against the byte table, then the parallel walk at 1, 2, 4… threads.
#[test]
#[ignore]
fn bench_show_air_walk() {
    let Some(data) = rc_testdata::version_data() else { return };
    let p = palette(&data);
    let kinds = marker_kinds(&data);
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let grid = world(&p, &mut rng);
    let world = WorldView { grid: &grid, min_y: MIN_Y, height: HEIGHT };
    let area = AREAS[0];
    let time = |f: &dyn Fn() -> usize| {
        let mut runs: Vec<f64> = (0..21)
            .map(|_| {
                let t = Instant::now();
                std::hint::black_box(f());
                t.elapsed().as_secs_f64() * 1e3
            })
            .collect();
        runs.sort_by(f64::total_cmp);
        runs[10]
    };
    for (name, classify) in [("names", Classifier::Names(&data)), ("table", Classifier::Table(&kinds))] {
        let ms = time(&|| {
            let mut out = Vec::new();
            walk_area(area, &world, classify, false, &mut out);
            out.len()
        });
        eprintln!("walk 48³, {name}, 1 thread: {ms:.3} ms");
    }
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut threads = 1;
    while threads <= cores {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().expect("pool");
        let ms = time(&|| {
            pool.install(|| {
                let mut out = Vec::new();
                walk_area(area, &world, Classifier::Table(&kinds), true, &mut out);
                out.len()
            })
        });
        eprintln!("walk 48³, table, parallel on {threads} threads: {ms:.3} ms");
        threads = if threads == cores { cores + 1 } else { (threads * 2).min(cores) };
    }
}

/// Frames of a 48³ area of air seen from outside: CPU of the boxes (walk, segments or key check),
/// bytes sent, GPU time of the pass (submit and wait, minus an empty pass), for the old path, the
/// table alone, the instances without cache and the instances with cache.
#[test]
#[ignore]
fn bench_show_air_frames() {
    let Some(data) = rc_testdata::version_data() else { return };
    let Some((device, queue)) = gpu() else { return };
    let p = Palette { marked: vec![0], visible: vec![0], direct_bits: palette(&data).direct_bits };
    let mut rng = Rng(1);
    let grid = world(&p, &mut rng);
    let world = WorldView { grid: &grid, min_y: MIN_Y, height: HEIGHT };
    let area = AREAS[0];
    let tech = tech_with_area(&data, area.start, area.size);
    let t = target(&device);
    let v = VIEWS[0];
    let cam = camera(&device, &t, v);
    let variants = [
        ("old (segments, names)", OLD),
        ("segments, table", CellOptions { table: true, ..OLD }),
        ("instances, no cache", CellOptions { cache: false, ..CellOptions::NEW }),
        ("instances, cache", CellOptions::NEW),
        ("instances, cache, parallel walk", CellOptions { parallel: true, ..CellOptions::NEW }),
    ];
    let mut empty = DebugLines::new(&device, &t.cam_layout, FORMAT);
    empty.clear();
    let median = |mut v: Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    };
    let gpu_pass = |lines: &DebugLines| {
        let t0 = Instant::now();
        render(&device, &queue, &t, lines, &cam);
        t0.elapsed().as_secs_f64() * 1e3
    };
    let base = median((0..30).map(|_| gpu_pass(&empty)).collect());
    for (name, options) in variants {
        let mut lines = DebugLines::new(&device, &t.cam_layout, FORMAT);
        lines.show_air.options = options;
        let (mut cpu, mut sent, mut pass) = (Vec::new(), 0u64, Vec::new());
        for i in 0..40 {
            let t0 = Instant::now();
            fill_frame(&mut lines, &tech, &world, &data, v);
            if i >= 10 {
                let segments = lines.segment_count() * std::mem::size_of::<crate::debug_render::Segment>();
                sent += lines.show_air.bytes_to_send() + segments as u64;
            }
            lines.upload_geometry(&device, &queue, (SIZE, SIZE));
            cpu.push(t0.elapsed().as_secs_f64() * 1e3);
            pass.push(gpu_pass(&lines) - base);
        }
        eprintln!(
            "{name}: CPU {:.3} ms per frame, {:.2} MB sent per frame, GPU pass {:.3} ms",
            median(cpu[10..].to_vec()),
            sent as f64 / 30.0 / 1e6,
            median(pass[10..].to_vec())
        );
    }
}
