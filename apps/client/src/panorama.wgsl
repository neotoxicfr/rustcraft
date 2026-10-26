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

// 6 faces × 2 triangles; the winding order does not matter (no culling).
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

@vertex
fn vs_cube(@builtin(vertex_index) i: u32) -> CubeOut {
    let p = CORNERS[INDICES[i]];
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

@vertex
fn vs_overlay(@builtin(vertex_index) i: u32) -> OverlayOut {
    let c = array<vec2<f32>, 6>(vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(1.0, 1.0), vec2(0.0, 0.0), vec2(1.0, 1.0), vec2(0.0, 1.0));
    let uv = c[i];
    var out: OverlayOut;
    out.pos = vec4(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    out.uv = uv;
    return out;
}

@fragment
fn fs_overlay(in: OverlayOut) -> @location(0) vec4<f32> {
    return textureSample(overlay, nearest, in.uv);
}
