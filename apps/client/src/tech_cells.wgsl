// Invisible blocks of a structure block's save area with "show air" on (continuation of
// debug_lines.wgsl). One instance per marked cell: its absolute block and its kind. The 72 vertices
// of an instance are the 12 edges of the cell's small cube, each widened by `segment_vertex` exactly
// like a debug segment: same corners, colours, 2.5 px thickness and cut at the plane 0.1 block in
// front of the eye as the segments the CPU used to build (tech_blocks/cells.rs).

struct CellFrame {
    // x, z: integer origin (blocks) of the frame of the debug lines.
    origin: vec4<i32>,
    // Plane that cuts the segments: eye in the frame (w = 1: cut on), forward axis.
    clip_eye: vec4<f32>,
    clip_forward: vec4<f32>,
    // Per marked kind 1..4: RGBA colour (packed bytes), low and high corner of the cube in its cell.
    colors: vec4<u32>,
    low: vec4<f32>,
    high: vec4<f32>,
};

@group(2) @binding(0) var<uniform> cells: CellFrame;

const CELL_LINE_WIDTH: f32 = 2.5;

// Distance of `p` beyond the cutting plane (negative behind it).
fn clip_distance(p: vec3<f32>) -> f32 {
    let e = cells.clip_eye.xyz;
    let f = cells.clip_forward.xyz;
    return (p.x - e.x) * f.x + (p.y - e.y) * f.y + (p.z - e.z) * f.z - 0.1;
}

// Point of `p`-`q` on the cutting plane.
fn cut(p: vec3<f32>, q: vec3<f32>, dp: f32, dq: f32) -> vec3<f32> {
    let t = dp / (dp - dq);
    return p + (q - p) * t;
}

// Corner of the cube: bit 0 high x, bit 1 high y, bit 2 high z.
fn cube_corner(cell: vec3<f32>, low: f32, high: f32, corner: u32) -> vec3<f32> {
    return cell + vec3<f32>(
        select(low, high, (corner & 1u) != 0u),
        select(low, high, (corner & 2u) != 0u),
        select(low, high, (corner & 4u) != 0u),
    );
}

fn hidden_vertex() -> Out {
    var o: Out;
    o.pos = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    return o;
}

@vertex
fn vs_cell(@builtin(vertex_index) vi: u32, @location(0) marked: vec4<i32>) -> Out {
    // Edges in the order of the CPU outline (corner pairs).
    let edge = vi / 6u;
    let corner_a = array<u32, 12>(0u, 2u, 4u, 6u, 0u, 1u, 4u, 5u, 0u, 1u, 2u, 3u)[edge];
    let corner_b = array<u32, 12>(1u, 3u, 5u, 7u, 2u, 3u, 6u, 7u, 4u, 5u, 6u, 7u)[edge];
    let kind = u32(marked.w) - 1u;
    // Integer offset from the origin first: exact, then a single rounding when the corner is added
    // (same value as the CPU's double-precision sum cast to f32).
    let cell = vec3<f32>(vec3<i32>(marked.x - cells.origin.x, marked.y, marked.z - cells.origin.z));
    var a = cube_corner(cell, cells.low[kind], cells.high[kind], corner_a);
    var b = cube_corner(cell, cells.low[kind], cells.high[kind], corner_b);
    if cells.clip_eye.w != 0.0 {
        let da = clip_distance(a);
        let db = clip_distance(b);
        if da < 0.0 && db < 0.0 {
            return hidden_vertex();
        }
        if da < 0.0 {
            a = cut(a, b, da, db);
        } else if db < 0.0 {
            b = cut(b, a, db, da);
        }
    }
    // The CPU dropped a segment cut down to a point (`segment_vertex` would draw a square).
    if all(a == b) {
        return hidden_vertex();
    }
    let k = array<u32, 6>(0u, 1u, 2u, 2u, 1u, 3u)[vi % 6u];
    return segment_vertex(k, a, b, unpack4x8unorm(cells.colors[kind]), CELL_LINE_WIDTH);
}
