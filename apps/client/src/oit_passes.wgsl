// Improved transparency: full-screen passes (appended to oit.wgsl).
// Final composition, bounds "pruned" by the opaque fragments, depth copies.

@group(3) @binding(4) var oit_accum: texture_2d<f32>;
@group(3) @binding(5) var oit_depth_in: texture_depth_2d;

// Triangle covering the screen.
@vertex
fn vs_screen(@builtin(vertex_index) v: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((v << 1u) & 2u), f32(v & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

struct Composite {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
};

// Coverage = 1 − total transmittance (nearly opaque: 1); accumulated color renormalized
// by the coverage; depth = nearest translucent surface.
@fragment
fn fs_composite(@builtin(position) frag: vec4<f32>) -> Composite {
    let p = vec2<i32>(frag.xy);
    let accumulated = textureLoad(oit_accum, p, 0);
    let t = oit_sample_transmittance(p, 100000.0, 1.0);
    let coverage = 1.0 - select(t, 0.0, t < 0.02);
    // Additive surfaces: light without coverage; nothing at all: pixel left as is.
    if coverage < 0.00001 && dot(accumulated.rgb, vec3<f32>(1.0)) < 0.00001 {
        discard;
    }
    let norm = select(1.0, coverage / accumulated.a, accumulated.a > 0.00001);
    return Composite(vec4<f32>(accumulated.rgb * norm, coverage), textureLoad(oit_bounds, p, 0).b);
}

struct Culled {
    @location(0) bounds: vec4<f32>,
    @builtin(frag_depth) depth: f32,
};

// Bounds copied as is.
@fragment
fn fs_copy_bounds(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    return textureLoad(oit_bounds, vec2<i32>(frag.xy), 0);
}

// Nearest opaque translucent fragment: written into the depth (test ≥), far bound
// brought in front of it.
@fragment
fn fs_cull_bounds(@builtin(position) frag: vec4<f32>) -> Culled {
    let b = textureLoad(oit_bounds, vec2<i32>(frag.xy), 0);
    let opaque = oit_linear(b.a);
    return Culled(vec4<f32>(b.r, min(b.g, opaque), b.b, b.a), b.a);
}

@fragment
fn fs_copy_depth(@builtin(position) frag: vec4<f32>) -> @builtin(frag_depth) f32 {
    return textureLoad(oit_depth_in, vec2<i32>(frag.xy), 0);
}
