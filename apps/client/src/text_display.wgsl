// Text displays (docs/specs/entities-objects.md § 13.8): interface glyphs and flat background
// fills placed in the frame of each display (matrix per layer). "Polygon offset"
// mode: lightmap and terrain fog; "see through" mode (bit 31 of the
// light): color only, no fog.

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
@group(1) @binding(2) var<storage, read> faces: array<mat4x4<f32>>;

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    // 0 = flat fill, otherwise glyph page + 1.
    @location(2) @interpolate(flat) mode: u32,
    @location(3) fogd: vec2<f32>,
    @location(4) @interpolate(flat) see: u32,
};

const SEE_THROUGH: u32 = 0x80000000u;

// One instance per glyph: corners (top-left, bottom-left, bottom-right, top-right) in font pixels
// in the frame of the layer (y downwards, scale included in the matrix).
@vertex
fn vs(
    @builtin(vertex_index) vi: u32,
    @location(0) face: u32,
    @location(1) light: u32,
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
    let pos = (faces[face] * vec4<f32>(x, y, 0.0, 1.0)).xyz;
    var o: Out;
    o.pos = cam.view_proj * vec4<f32>(pos, 1.0);
    o.uv = vec2<f32>(select(uvr.x, uvr.z, right), select(uvr.y, uvr.w, bottom));
    o.see = select(0u, 1u, (light & SEE_THROUGH) != 0u);
    if o.see == 1u {
        o.color = color;
    } else {
        let luv = clamp(vec2<f32>(f32(light & 0xffu), f32((light >> 8u) & 0xffu)) / 256.0 + 1.0 / 32.0, vec2<f32>(1.0 / 32.0), vec2<f32>(31.0 / 32.0));
        o.color = vec4<f32>(color.rgb * textureSampleLevel(lightmap, lightmap_sampler, luv, 0.0).rgb, color.a);
    }
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
    if i.see == 1u {
        return c;
    }
    let v = max(linear_fog(i.fogd.x, cam.fog.x, cam.fog.y), linear_fog(i.fogd.y, cam.fog.z, cam.fog.w));
    return vec4<f32>(mix(c.rgb, cam.fog_color.rgb, v * cam.fog_color.a), c.a);
}
