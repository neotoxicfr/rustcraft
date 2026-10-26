// Leash ropes (docs/specs/miscellaneous.md § 1.7): untextured, vertex color ×
// lightmap (sampled at the vertex, interpolated between steps), terrain fog, opaque,
// no back-face culling.

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
@group(0) @binding(7) var lightmap: texture_2d<f32>;
@group(0) @binding(8) var lightmap_sampler: sampler;

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) fogd: vec2<f32>,
};

@vertex
fn vs(@location(0) pos: vec3<f32>, @location(1) color: vec4<f32>, @location(2) light: u32) -> Out {
    var o: Out;
    o.pos = cam.view_proj * vec4<f32>(pos, 1.0);
    let l = vec2<f32>(f32(light & 0xffu), f32((light >> 8u) & 0xffu));
    let uv = clamp(l / 256.0 + 1.0 / 32.0, vec2<f32>(1.0 / 32.0), vec2<f32>(31.0 / 32.0));
    o.color = color.rgb * textureSampleLevel(lightmap, lightmap_sampler, uv, 0.0).rgb;
    let d = pos - cam.eye.xyz;
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
    let v = max(linear_fog(i.fogd.x, cam.fog.x, cam.fog.y), linear_fog(i.fogd.y, cam.fog.z, cam.fog.w));
    return vec4<f32>(mix(i.color, cam.fog_color.rgb, v * cam.fog_color.a), 1.0);
}
