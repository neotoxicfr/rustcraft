// Full-screen overlays (`overlays.rs`): screen effects of the hand pass and camera overlays
// of the interface. Vertices already in clip coordinates.

struct VsIn {
    @location(0) pos: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    // x: source (0 misc nearest, 1 misc smoothed, 2 blocks, 3 big blocks), y: layer.
    @location(3) tex: vec2<u32>,
}

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) tex: vec2<u32>,
}

@group(0) @binding(0) var misc: texture_2d_array<f32>;
@group(0) @binding(1) var blocks: texture_2d_array<f32>;
@group(0) @binding(2) var big: texture_2d_array<f32>;
@group(0) @binding(3) var nearest: sampler;
@group(0) @binding(4) var linear: sampler;

@vertex
fn vs(v: VsIn) -> VsOut {
    var o: VsOut;
    o.pos = v.pos;
    o.uv = v.uv;
    o.color = v.color;
    o.tex = v.tex;
    return o;
}

@fragment
fn fs(i: VsOut) -> @location(0) vec4<f32> {
    var t: vec4<f32>;
    let layer = i32(i.tex.y);
    switch i.tex.x {
        case 0u: { t = textureSampleLevel(misc, nearest, i.uv, layer, 0.0); }
        case 1u: { t = textureSampleLevel(misc, linear, i.uv, layer, 0.0); }
        case 2u: { t = textureSampleLevel(blocks, nearest, i.uv, layer, 0.0); }
        // Flat fill (black bands of the spyglass).
        case 4u: { t = vec4<f32>(1.0); }
        default: { t = textureSampleLevel(big, nearest, i.uv, layer, 0.0); }
    }
    let c = t * i.color;
    if c.a == 0.0 {
        discard;
    }
    return c;
}
