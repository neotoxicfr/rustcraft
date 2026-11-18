// Improved transparency, depth bounds in one RGBA32F target (`RUSTCRAFT_OPT_OIT_COMPACT_BOUNDS=0`,
// the layout before): (−distance, distance, depth, depth if opaque), blended with MAX.

struct OitBounds {
    @location(0) bounds: vec4<f32>,
};

// Bounds as the culled target keeps them.
alias OitBoundsValue = vec4<f32>;

// "Bounds" stage: almost transparent fragments are rejected.
fn oit_depth_bounds(frag: vec4<f32>, alpha: f32) -> OitBounds {
    if alpha < 0.01 {
        discard;
    }
    let lin = oit_linear(frag.z);
    let opaque = select(0.0, frag.z, alpha > OIT_OPAQUE_ALPHA);
    return OitBounds(vec4<f32>(-lin, lin, frag.z, opaque));
}

// Distances (blocks) of the pixel's nearest and farthest translucent fragments.
fn oit_near_far(pos: vec2<i32>) -> vec2<f32> {
    let b = textureLoad(oit_bounds, pos, 0);
    return vec2<f32>(-b.r, b.g);
}

// Device depth of the pixel's nearest translucent fragment.
fn oit_nearest_depth(pos: vec2<i32>) -> f32 {
    return textureLoad(oit_bounds, pos, 0).b;
}

fn oit_load_bounds(pos: vec2<i32>) -> OitBoundsValue {
    return textureLoad(oit_bounds, pos, 0);
}

// Device depth of the pixel's nearest nearly opaque translucent fragment (0: none).
fn oit_opaque_depth(pos: vec2<i32>) -> f32 {
    return textureLoad(oit_bounds, pos, 0).a;
}

// Bounds `b` with the far bound brought in front of the nearly opaque depth `opaque`.
fn oit_cull_bounds(b: OitBoundsValue, opaque: f32) -> OitBoundsValue {
    return vec4<f32>(b.r, min(b.g, oit_linear(opaque)), b.b, b.a);
}
