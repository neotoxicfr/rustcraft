// Title screen panorama (docs/specs/title-screen-servers.md § 1.1): cube of side 2 centered
// on the origin, sampled in the direction of the vertex position; then a full-screen veil.

struct Uniforms {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var cube: texture_cube<f32>;
@group(0) @binding(2) var samp: sampler;
@group(0) @binding(3) var overlay: texture_2d<f32>;
@group(0) @binding(4) var nearest: sampler;

struct CubeOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) dir: vec3<f32>,
};

// Position of vertex `i` (0 to 35) of the cube: 6 faces (+Z, −Z, +X, −X, +Y, −Y) × 2 triangles;
// the winding order does not matter (no culling).
//#if PACKED_SHADER_CONSTANTS
// One bit per axis and per face corner (bit `face * 4 + corner`, set = +1), the corner of a
// vertex read from one nibble per vertex (triangles 0-1-2 then 0-2-3): constant arrays indexed by
// the vertex index may be copied into local memory for every vertex, shifts are not.
const CUBE_CORNER_X: u32 = 0x660f96u;
const CUBE_CORNER_Y: u32 = 0x0fccccu;
const CUBE_CORNER_Z: u32 = 0xc3690fu;

fn cube_vertex(i: u32) -> vec3<f32> {
    let corner = (0x320210u >> ((i % 6u) * 4u)) & 0xfu;
    let bit = (i / 6u) * 4u + corner;
    let positive = (vec3<u32>(CUBE_CORNER_X, CUBE_CORNER_Y, CUBE_CORNER_Z) >> vec3<u32>(bit)) & vec3<u32>(1u);
    return vec3<f32>(positive) * 2.0 - 1.0;
}
//#else
const CORNERS = array<vec3<f32>, 8>(
    vec3(-1.0, -1.0, -1.0), vec3(1.0, -1.0, -1.0), vec3(1.0, 1.0, -1.0), vec3(-1.0, 1.0, -1.0),
    vec3(-1.0, -1.0, 1.0), vec3(1.0, -1.0, 1.0), vec3(1.0, 1.0, 1.0), vec3(-1.0, 1.0, 1.0),
);
const INDICES = array<u32, 36>(
    4u, 5u, 6u, 4u, 6u, 7u, // +Z
    1u, 0u, 3u, 1u, 3u, 2u, // -Z
    5u, 1u, 2u, 5u, 2u, 6u, // +X
    0u, 4u, 7u, 0u, 7u, 3u, // -X
    7u, 6u, 2u, 7u, 2u, 3u, // +Y
    0u, 1u, 5u, 0u, 5u, 4u, // -Y
);

fn cube_vertex(i: u32) -> vec3<f32> {
    return CORNERS[INDICES[i]];
}
//#endif

@vertex
fn vs_cube(@builtin(vertex_index) i: u32) -> CubeOut {
    let p = cube_vertex(i);
    var out: CubeOut;
    out.pos = u.view_proj * vec4(p, 1.0);
    out.dir = p;
    return out;
}

@fragment
fn fs_cube(in: CubeOut) -> @location(0) vec4<f32> {
    return vec4(textureSample(cube, samp, in.dir).rgb, 1.0);
}

struct OverlayOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// Texture coordinates of vertex `i` (0 to 5) of the full-screen veil (two triangles).
//#if PACKED_SHADER_CONSTANTS
fn veil_uv(i: u32) -> vec2<f32> {
    // Corner (0,0) (1,0) (1,1) (0,1) of the vertex from one nibble per vertex, then one bit per
    // corner and per axis (same reason as the cube).
    let corner = (0x320210u >> (i * 4u)) & 0xfu;
    return vec2<f32>((vec2<u32>(0x6u, 0xcu) >> vec2<u32>(corner)) & vec2<u32>(1u));
}
//#else
fn veil_uv(i: u32) -> vec2<f32> {
    let c = array<vec2<f32>, 6>(vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(1.0, 1.0), vec2(0.0, 0.0), vec2(1.0, 1.0), vec2(0.0, 1.0));
    return c[i];
}
//#endif

@vertex
fn vs_overlay(@builtin(vertex_index) i: u32) -> OverlayOut {
    let uv = veil_uv(i);
    var out: OverlayOut;
    out.pos = vec4(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    out.uv = uv;
    return out;
}

@fragment
fn fs_overlay(in: OverlayOut) -> @location(0) vec4<f32> {
    return textureSample(overlay, nearest, in.uv);
}
