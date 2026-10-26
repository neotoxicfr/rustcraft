// Floating text of the debug tracers (docs/specs/debug-f3.md § 8): interface glyphs
// on a camera-facing panel, `scale` blocks per font pixel, full light
// (block 15, sky 15), world fog, no background.

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
@group(1) @binding(0) var glyphs: texture_2d_array<f32>;
@group(1) @binding(1) var nearest: sampler;

struct Axes {
    right: vec4<f32>,
    up: vec4<f32>,
};
@group(1) @binding(2) var<uniform> axes: Axes;

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    // 0 = flat fill, otherwise glyph page + 1.
    @location(2) @interpolate(flat) mode: u32,
    @location(3) fogd: vec2<f32>,
};

@vertex
fn vs(
    @builtin(vertex_index) vi: u32,
    @location(0) anchor: vec3<f32>,
    @location(1) scale: f32,
    @location(2) rect: vec4<f32>,
    @location(3) skew: f32,
    @location(4) uvr: vec4<f32>,
    @location(5) color: vec4<f32>,
    @location(6) mode: u32,
) -> Out {
    let k = array<u32, 6>(0u, 1u, 2u, 0u, 2u, 3u)[vi];
    let bottom = k == 1u || k == 2u;
    let right = k >= 2u;
    let x = select(rect.x, rect.y, right) + select(0.0, skew, bottom);
    let y = select(rect.z, rect.w, bottom);
    let pos = anchor + axes.right.xyz * (scale * x) - axes.up.xyz * (scale * y);
    var o: Out;
    o.pos = cam.view_proj * vec4<f32>(pos, 1.0);
    o.uv = vec2<f32>(select(uvr.x, uvr.z, right), select(uvr.y, uvr.w, bottom));
    let light = textureSampleLevel(lightmap, lightmap_sampler, vec2<f32>(31.0 / 32.0), 0.0).rgb;
    o.color = vec4<f32>(color.rgb * light, color.a);
    o.mode = mode;
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
    var c = i.color;
    if i.mode != 0u {
        c = textureSampleLevel(glyphs, nearest, i.uv, i.mode - 1u, 0.0) * c;
    }
    if c.a < 0.1 {
        discard;
    }
    let v = max(linear_fog(i.fogd.x, cam.fog.x, cam.fog.y), linear_fog(i.fogd.y, cam.fog.z, cam.fog.w));
    return vec4<f32>(mix(c.rgb, cam.fog_color.rgb, v * cam.fog_color.a), c.a);
}
