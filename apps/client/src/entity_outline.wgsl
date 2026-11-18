// Glowing outline of entities ("glowing" effect, docs/specs/entities.md): the game's
// post-processing chain on the target of colored silhouettes — edge detection on the alpha
// (simplified Sobel), horizontal then vertical box blur (radius 2), blend onto the frame.

struct Cfg {
    size: vec2<f32>,
    dir: vec2<f32>,
};

@group(0) @binding(0) var<uniform> cfg: Cfg;
@group(0) @binding(1) var in_tex: texture_2d<f32>;
@group(0) @binding(2) var nearest_s: sampler;
@group(0) @binding(3) var linear_s: sampler;

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) t: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) vi: u32) -> Out {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var o: Out;
    o.pos = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    o.t = vec2<f32>(uv.x, 1.0 - uv.y);
    return o;
}

@fragment
fn fs_sobel(i: Out) -> @location(0) vec4<f32> {
    let one = 1.0 / cfg.size;
    let c = textureSampleLevel(in_tex, nearest_s, i.t, 0.0);
    let l = textureSampleLevel(in_tex, nearest_s, i.t - vec2<f32>(one.x, 0.0), 0.0);
    let r = textureSampleLevel(in_tex, nearest_s, i.t + vec2<f32>(one.x, 0.0), 0.0);
    let u = textureSampleLevel(in_tex, nearest_s, i.t - vec2<f32>(0.0, one.y), 0.0);
    let d = textureSampleLevel(in_tex, nearest_s, i.t + vec2<f32>(0.0, one.y), 0.0);
    let total = clamp(abs(c.a - l.a) + abs(c.a - r.a) + abs(c.a - u.a) + abs(c.a - d.a), 0.0, 1.0);
    let rgb = c.rgb * c.a + l.rgb * l.a + r.rgb * r.a + u.rgb * u.a + d.rgb * d.a;
    return vec4<f32>(rgb * 0.2, total);
}

@fragment
fn fs_blur(i: Out) -> @location(0) vec4<f32> {
    let step = (1.0 / cfg.size) * cfg.dir;
    let radius = 2.0;
    var sum = vec4<f32>(0.0);
    var a = -radius + 0.5;
    loop {
        if a > radius { break; }
        sum += textureSampleLevel(in_tex, linear_s, i.t + step * a, 0.0);
        a += 2.0;
    }
    sum += textureSampleLevel(in_tex, linear_s, i.t + step * radius, 0.0) / 2.0;
    return vec4<f32>((sum / (radius + 0.5)).rgb, sum.a);
}

@fragment
fn fs_blit(i: Out) -> @location(0) vec4<f32> {
    return textureSampleLevel(in_tex, nearest_s, i.t, 0.0);
}
