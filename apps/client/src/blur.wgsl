// Separable box blur of the menu background (`docs/specs/option-menus.md` § 1.12): one pass
// along `dir`, bilinear samples every 2 texels from −r + 0.5 to r, plus the sample
// at +r weighted ½, all divided by r + 0.5 (2r + 1 texels of equal weight).

struct Cfg {
    inv_size: vec2<f32>,
    dir: vec2<f32>,
    radius: f32,
    _p0: f32,
    _p1: f32,
    _p2: f32,
};

@group(0) @binding(0) var<uniform> cfg: Cfg;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

struct V {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// Triangle covering the screen.
@vertex
fn vs(@builtin(vertex_index) i: u32) -> V {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    var o: V;
    o.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    o.uv = vec2<f32>(x, y);
    return o;
}

@fragment
fn fs(v: V) -> @location(0) vec4<f32> {
    let step = cfg.inv_size * cfg.dir;
    let r = cfg.radius;
    var acc = vec4<f32>(0.0);
    var a = -r + 0.5;
    loop {
        if a > r {
            break;
        }
        acc += textureSampleLevel(src, samp, v.uv + step * a, 0.0);
        a += 2.0;
    }
    acc += textureSampleLevel(src, samp, v.uv + step * r, 0.0) * 0.5;
    return acc / (r + 0.5);
}
