// Beacon and gateway beams, End portal (docs/specs/block-entities.md § 4.9 to
// 4.11): vertices in the camera frame (blocks), terrain fog.

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

struct Params {
    // Portal time: (game time mod 24000 + fraction) / 24000.
    time: vec4<f32>,
};

@group(0) @binding(0) var<uniform> cam: Camera;
@group(1) @binding(0) var beacon_tex: texture_2d<f32>;
@group(1) @binding(1) var gateway_tex: texture_2d<f32>;
@group(1) @binding(2) var sky_tex: texture_2d<f32>;
@group(1) @binding(3) var portal_tex: texture_2d<f32>;
@group(1) @binding(4) var repeat: sampler;
@group(1) @binding(5) var<uniform> params: Params;

fn linear_fog(d: f32, s: f32, e: f32) -> f32 {
    if d <= s { return 0.0; }
    if d >= e { return 1.0; }
    return (d - s) / (e - s);
}

fn apply_fog(c: vec3<f32>, d: vec2<f32>) -> vec3<f32> {
    let v = max(linear_fog(d.x, cam.fog.x, cam.fog.y), linear_fog(d.y, cam.fog.z, cam.fog.w));
    return mix(c, cam.fog_color.rgb, v * cam.fog_color.a);
}

fn fog_dist(pos: vec3<f32>) -> vec2<f32> {
    let d = pos - cam.eye.xyz;
    return vec2<f32>(length(d), max(length(d.xz), abs(d.y)));
}

struct BeamOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) tex: u32,
    // Interpolated position: fog distance computed per fragment (very tall beams).
    @location(3) wpos: vec3<f32>,
};

@vertex
fn vs_beam(
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) tex: u32,
) -> BeamOut {
    var o: BeamOut;
    o.pos = cam.view_proj * vec4<f32>(pos, 1.0);
    o.uv = uv;
    o.color = color;
    o.tex = tex;
    o.wpos = pos;
    return o;
}

fn beam_color(i: BeamOut) -> vec4<f32> {
    let a = textureSample(beacon_tex, repeat, i.uv);
    let b = textureSample(gateway_tex, repeat, i.uv);
    return select(a, b, i.tex == 1u) * i.color;
}

// Core: opaque, depth written.
@fragment
fn fs_beam(i: BeamOut) -> @location(0) vec4<f32> {
    let c = beam_color(i);
    return vec4<f32>(apply_fog(c.rgb, fog_dist(i.wpos)), 1.0);
}

// Halo: classic blend, no depth write.
@fragment
fn fs_halo(i: BeamOut) -> @location(0) vec4<f32> {
    let c = beam_color(i);
    return vec4<f32>(apply_fog(c.rgb, fog_dist(i.wpos)), c.a);
}

struct PortalOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) proj: vec4<f32>,
    @location(1) @interpolate(flat) layers: u32,
    @location(2) fogd: vec2<f32>,
};

@vertex
fn vs_portal(@location(0) pos: vec3<f32>, @location(1) layers: u32) -> PortalOut {
    var o: PortalOut;
    let clip = cam.view_proj * vec4<f32>(pos, 1.0);
    o.pos = clip;
    // Coordinates projected onto the screen (the texture does not follow the surface).
    let h = clip * 0.5;
    o.proj = vec4<f32>(h.x + h.w, h.y + h.w, clip.z, clip.w);
    o.layers = layers;
    o.fogd = fog_dist(pos);
    return o;
}

const COLORS = array<vec3<f32>, 16>(
    vec3<f32>(0.022087, 0.098399, 0.110818),
    vec3<f32>(0.011892, 0.095924, 0.089485),
    vec3<f32>(0.027636, 0.101689, 0.100326),
    vec3<f32>(0.046564, 0.109883, 0.114838),
    vec3<f32>(0.064901, 0.117696, 0.097189),
    vec3<f32>(0.063761, 0.086895, 0.123646),
    vec3<f32>(0.084817, 0.111994, 0.166380),
    vec3<f32>(0.097489, 0.154120, 0.091064),
    vec3<f32>(0.106152, 0.131144, 0.195191),
    vec3<f32>(0.097721, 0.110188, 0.187229),
    vec3<f32>(0.133516, 0.138278, 0.148582),
    vec3<f32>(0.070006, 0.243332, 0.235792),
    vec3<f32>(0.196766, 0.142899, 0.214696),
    vec3<f32>(0.047281, 0.315338, 0.321970),
    vec3<f32>(0.204675, 0.390010, 0.302066),
    vec3<f32>(0.080955, 0.314821, 0.661491),
);

// Layer `layer`: in-plane scale and rotation, scrolling translation, then scale 0.5
// and offset 0.25 of the projected frame (products to the right of a row vector).
fn layer_matrix(layer: f32) -> mat4x4<f32> {
    let g = params.time.x;
    let translate = mat4x4<f32>(
        1.0, 0.0, 0.0, 17.0 / layer,
        0.0, 1.0, 0.0, (2.0 + layer / 1.5) * (g * 1.5),
        0.0, 0.0, 1.0, 0.0,
        0.0, 0.0, 0.0, 1.0,
    );
    let r = radians((layer * layer * 4321.0 + layer * 9.0) * 2.0);
    let s = (4.5 - layer / 4.0) * 2.0;
    let rot = mat2x2<f32>(cos(r), -sin(r), sin(r), cos(r)) * mat2x2<f32>(s, 0.0, 0.0, s);
    let m = mat4x4<f32>(
        vec4<f32>(rot[0], 0.0, 0.0),
        vec4<f32>(rot[1], 0.0, 0.0),
        vec4<f32>(0.0, 0.0, 1.0, 0.0),
        vec4<f32>(0.0, 0.0, 0.0, 1.0),
    );
    let st = mat4x4<f32>(
        0.5, 0.0, 0.0, 0.25,
        0.0, 0.5, 0.0, 0.25,
        0.0, 0.0, 1.0, 0.0,
        0.0, 0.0, 0.0, 1.0,
    );
    return m * translate * st;
}

fn sample_proj(t: texture_2d<f32>, p: vec4<f32>) -> vec3<f32> {
    // Texture origin at the top (the game's projected frame: at the bottom).
    let uv = p.xy / p.w;
    return textureSampleLevel(t, repeat, vec2<f32>(uv.x, 1.0 - uv.y), 0.0).rgb;
}

@fragment
fn fs_portal(i: PortalOut) -> @location(0) vec4<f32> {
    var color = sample_proj(sky_tex, i.proj) * COLORS[0];
    for (var k = 0u; k < i.layers; k++) {
        color += sample_proj(portal_tex, i.proj * layer_matrix(f32(k + 1u))) * COLORS[k];
    }
    return vec4<f32>(apply_fog(color, i.fogd), 1.0);
}
