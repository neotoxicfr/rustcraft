// Full-screen image effects (docs/specs/miscellaneous.md § 6.3): passes with one triangle
// covering the screen. Coordinates `t` in the game's convention (v = 0 at the bottom), flipped when read.

struct Cfg {
    size: vec2<f32>,
    dir: vec2<f32>,
    radius: f32,
    rotation: f32,
    pad: vec2<f32>,
    scale: vec2<f32>,
    offset: vec2<f32>,
    scissor: vec4<f32>,
    vignette: vec4<f32>,
    modulate: vec4<f32>,
};

@group(0) @binding(0) var<uniform> cfg: Cfg;
@group(0) @binding(1) var in_tex: texture_2d<f32>;
@group(0) @binding(2) var blur_tex: texture_2d<f32>;
@group(0) @binding(3) var linear_s: sampler;
@group(0) @binding(4) var nearest_s: sampler;

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) t: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) vi: u32) -> Out {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var o: Out;
    o.pos = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    o.t = uv;
    return o;
}

fn flip(t: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(t.x, 1.0 - t.y);
}

fn sat(c: vec3<f32>, s: f32) -> vec3<f32> {
    let l = dot(c, vec3<f32>(0.3, 0.59, 0.11));
    return l + (c - l) * s;
}

// The game's box blur: bilinear reads between two pixels, last pixel at half weight.
@fragment
fn fs_blur(i: Out) -> @location(0) vec4<f32> {
    let step = cfg.dir / cfg.size;
    let r = cfg.radius;
    var acc = vec4<f32>(0.0);
    var a = -r + 0.5;
    loop {
        if a > r { break; }
        acc += textureSampleLevel(in_tex, linear_s, flip(i.t + step * a), 0.0);
        a += 2.0;
    }
    acc += textureSampleLevel(in_tex, linear_s, flip(i.t + step * r), 0.0) / 2.0;
    return acc / (r + 0.5);
}

// 80% negative (enderman).
@fragment
fn fs_invert(i: Out) -> @location(0) vec4<f32> {
    let c = textureSampleLevel(in_tex, nearest_s, flip(i.t), 0.0).rgb;
    return vec4<f32>(mix(c, 1.0 - c, 0.8), 1.0);
}

// Creeper 1: green channel = luminance, saturation 1.8.
@fragment
fn fs_color(i: Out) -> @location(0) vec4<f32> {
    let c = textureSampleLevel(in_tex, nearest_s, flip(i.t), 0.0).rgb;
    let g = dot(c, vec3<f32>(0.3, 0.59, 0.11));
    return vec4<f32>(sat(vec3<f32>(0.0, g, 0.0), 1.8), 1.0);
}

// Creeper 2: 4 × 4 mosaic, 16 levels, saturation 1.5.
@fragment
fn fs_bits(i: Out) -> @location(0) vec4<f32> {
    let mosaic = cfg.size / 4.0;
    let fract_pix = fract(i.t * mosaic) / mosaic;
    let c = textureSampleLevel(in_tex, nearest_s, flip(i.t - fract_pix), 0.0).rgb;
    let q = c - fract(c * 16.0) / 16.0;
    return vec4<f32>(sat(q, 1.5), 1.0);
}

// Spider: vignette rotated and scaled over a background.
@fragment
fn fs_spider(i: Out) -> @location(0) vec4<f32> {
    let a = radians(cfg.rotation);
    let ca = cos(a);
    let sa = sin(a);
    let s = vec2<f32>(i.t.x * ca - i.t.y * sa, i.t.y * ca + i.t.x * sa) * cfg.scale + cfg.offset;
    let scaled = textureSampleLevel(in_tex, nearest_s, flip(s), 0.0);
    let back = textureSampleLevel(blur_tex, nearest_s, flip(i.t), 0.0);
    var o = scaled;
    let sc = cfg.scissor;
    let vg = cfg.vignette;
    if s.x < sc.x || s.y < sc.y || s.x > sc.z || s.y > sc.w {
        o = back;
    }
    if s.x < vg.x { o = mix(back, o, (sc.x - s.x) / (sc.x - vg.x)); }
    if s.y < vg.y { o = mix(back, o, (sc.y - s.y) / (sc.y - vg.y)); }
    if s.x > vg.z { o = mix(back, o, (sc.z - s.x) / (sc.z - vg.z)); }
    if s.y > vg.w { o = mix(back, o, (sc.w - s.y) / (sc.w - vg.w)); }
    return vec4<f32>(o.rgb, 1.0);
}

// Modulated copy.
@fragment
fn fs_blit(i: Out) -> @location(0) vec4<f32> {
    let c = textureSampleLevel(in_tex, nearest_s, flip(i.t), 0.0);
    return vec4<f32>(c.rgb * cfg.modulate.rgb, 1.0);
}
