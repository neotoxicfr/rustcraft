// World border wall (docs/specs/weather-border.md § 4.3): force field texture
// scrolling diagonally (real time), uniform tint, weighted additive blend, no
// fog or lightmap.

struct Camera {
    view_proj: mat4x4<f32>,
    origin: vec4<i32>,
    shade_a: vec4<f32>,
    shade_b: vec4<f32>,
    eye: vec4<f32>,
    fog_color: vec4<f32>,
    fog: vec4<f32>,
    fog_sky: vec4<f32>,
    sky_color: vec4<f32>,
    sky_view_proj: mat4x4<f32>,
};

struct Wall {
    // Wall origin minus the camera's integer origin (x, z), scrolling.
    off: vec4<f32>,
    color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> cam: Camera;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var tex_sampler: sampler;
@group(1) @binding(2) var<uniform> wall: Wall;

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@location(0) pos: vec3<f32>, @location(1) uv: vec2<f32>) -> Out {
    var o: Out;
    o.pos = cam.view_proj * vec4<f32>(pos.x + wall.off.x, pos.y, pos.z + wall.off.y, 1.0);
    o.uv = uv + vec2<f32>(wall.off.z);
    return o;
}

@fragment
fn fs(i: Out) -> @location(0) vec4<f32> {
    let t = textureSample(tex, tex_sampler, i.uv);
    if t.a == 0.0 {
        discard;
    }
    return t * wall.color;
}
