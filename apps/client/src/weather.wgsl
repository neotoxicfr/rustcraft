// Rain and snow columns (docs/specs/weather-border.md § 1.5-1.8): one quad per
// instance, texture scrolling and alpha computed here every frame; white color ×
// lightmap, discard below 0.1 alpha, terrain fog.

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

struct Frame {
    // t & 131071, t & 511.
    ticks: vec4<u32>,
    // Tick fraction, rain level, radius².
    f: vec4<f32>,
};

@group(0) @binding(0) var<uniform> cam: Camera;
@group(0) @binding(7) var lightmap: texture_2d<f32>;
@group(0) @binding(8) var lightmap_sampler: sampler;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var tex_sampler: sampler;
@group(1) @binding(2) var<uniform> frame: Frame;

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) fogd: vec2<f32>,
};

@vertex
fn vs(
    @builtin(vertex_index) vi: u32,
    // Center x, center z, bottom, top.
    @location(0) col: vec4<f32>,
    @location(1) half: vec2<f32>,
    @location(2) p: vec4<f32>,
    @location(3) light: u32,
    @location(4) kind: u32,
) -> Out {
    // Vertices in the game's order: (X0, top) (X1, top) (X1, bottom) (X0, bottom).
    let k = array<u32, 6>(0u, 1u, 2u, 0u, 2u, 3u)[vi];
    let side = select(-1.0, 1.0, k == 1u || k == 2u);
    let top = k < 2u;
    let pt = frame.f.x;
    var u_off = 0.0;
    var v_off = 0.0;
    var a_max = 1.0;
    if kind == 0u {
        // Rain: integer offset added before the conversion, then the tick fraction.
        let o = -((f32(frame.ticks.x + u32(p.y)) + pt) / 32.0 * p.x);
        v_off = o - 32.0 * trunc(o / 32.0);
    } else {
        let elapsed = f32(frame.ticks.x) + pt;
        u_off = p.x + elapsed * 0.01 * p.y;
        v_off = -(f32(frame.ticks.y) + pt) / 512.0 + (p.z + elapsed * p.w * 0.001);
        a_max = 0.8;
    }
    let x = col.x + half.x * side;
    let z = col.y + half.y * side;
    let y = select(col.z, col.w, top);
    let pos = vec3<f32>(x, y, z);
    // Alpha: opaque at the center, half at the edge of the radius, × rain level, quantized to a byte.
    let d = col.xy - cam.eye.xz;
    let t = min(dot(d, d) / frame.f.z, 1.0);
    let alpha = floor((a_max + t * (0.5 - a_max)) * frame.f.y * 255.0) / 255.0;
    var o: Out;
    o.pos = cam.view_proj * vec4<f32>(pos, 1.0);
    // Like the game: v0 (bottom × 0.25) at the top vertices, v1 (top × 0.25) at the bottom ones.
    o.uv = vec2<f32>(u_off + select(0.0, 1.0, side > 0.0), select(col.w, col.z, top) * 0.25 + v_off);
    let luv = clamp(vec2<f32>(f32(light & 0xffu), f32((light >> 8u) & 0xffu)) / 256.0 + 1.0 / 32.0, vec2<f32>(1.0 / 32.0), vec2<f32>(31.0 / 32.0));
    o.color = vec4<f32>(textureSampleLevel(lightmap, lightmap_sampler, luv, 0.0).rgb, alpha);
    let e = pos - cam.eye.xyz;
    o.fogd = vec2<f32>(length(e), max(length(e.xz), abs(e.y)));
    return o;
}

fn linear_fog(d: f32, s: f32, e: f32) -> f32 {
    if d <= s { return 0.0; }
    if d >= e { return 1.0; }
    return (d - s) / (e - s);
}

@fragment
fn fs(i: Out) -> @location(0) vec4<f32> {
    let c = textureSample(tex, tex_sampler, i.uv) * i.color;
    if c.a < 0.1 {
        discard;
    }
    let v = max(linear_fog(i.fogd.x, cam.fog.x, cam.fog.y), linear_fog(i.fogd.y, cam.fog.z, cam.fog.w));
    return vec4<f32>(mix(c.rgb, cam.fog_color.rgb, v * cam.fog_color.a), c.a);
}
