// Mining cracks (docs/specs/interactions.md § 2.2): faces of the block model, projected UVs
// computed on the CPU, texture `destroy_stage_<stage>`, multiplicative blend (2 · src · dst).

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

@group(0) @binding(0) var<uniform> cam: Camera;
@group(1) @binding(0) var stages: texture_2d_array<f32>;
@group(1) @binding(1) var samp: sampler;

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) stage: u32,
    @location(2) fogd: vec2<f32>,
};

@vertex
fn vs(@location(0) p: vec3<f32>, @location(1) uv: vec2<f32>, @location(2) stage: u32) -> Out {
    var o: Out;
    o.pos = cam.view_proj * vec4<f32>(p, 1.0);
    o.uv = uv;
    o.stage = stage;
    let d = p - cam.eye.xyz;
    o.fogd = vec2<f32>(length(d), max(length(d.xz), abs(d.y)));
    return o;
}

fn linear_fog(d: f32, s: f32, e: f32) -> f32 {
    if d <= s { return 0.0; }
    if d >= e { return 1.0; }
    return (d - s) / (e - s);
}

@fragment
fn fs(i: Out) -> @location(0) vec4<f32> {
    let c = textureSample(stages, samp, i.uv, i32(i.stage));
    if c.a < 0.1 {
        discard;
    }
    let v = max(linear_fog(i.fogd.x, cam.fog.x, cam.fog.y), linear_fog(i.fogd.y, cam.fog.z, cam.fog.w));
    return vec4<f32>(mix(c.rgb, cam.fog_color.rgb, v * cam.fog_color.a), c.a);
}
