// Improved transparency, compact depth bounds (`RUSTCRAFT_OPT_OIT_COMPACT_BOUNDS`): (depth, −depth)
// in an RG32F target and the depth of the nearly opaque fragments in an R32F one, all blended
// with MAX. 12 bytes per pixel instead of 16 (the culled copy: 8 instead of 16), and no division
// in the bounds stage. The distances are computed from the depths when they are read: the distance
// shrinks as the reversed depth grows and its rounding is monotone, so the distance of the largest
// depth is exactly the smallest of the fragments' distances, which the RGBA32F layout stored.

@group(3) @binding(6) var oit_bounds_opaque: texture_2d<f32>;

struct OitBounds {
    @location(0) depths: vec2<f32>,
    @location(1) opaque: f32,
};

// Bounds as the culled target keeps them: (nearest depth, minus the farthest depth).
alias OitBoundsValue = vec2<f32>;

// "Bounds" stage: almost transparent fragments are rejected.
fn oit_depth_bounds(frag: vec4<f32>, alpha: f32) -> OitBounds {
    if alpha < 0.01 {
        discard;
    }
    return OitBounds(vec2<f32>(frag.z, -frag.z), select(0.0, frag.z, alpha > OIT_OPAQUE_ALPHA));
}

// Distances (blocks) of the pixel's nearest and farthest translucent fragments.
fn oit_near_far(pos: vec2<i32>) -> vec2<f32> {
    let b = textureLoad(oit_bounds, pos, 0);
    return vec2<f32>(oit_linear(b.r), oit_linear(-b.g));
}

// Device depth of the pixel's nearest translucent fragment.
fn oit_nearest_depth(pos: vec2<i32>) -> f32 {
    return textureLoad(oit_bounds, pos, 0).r;
}

fn oit_load_bounds(pos: vec2<i32>) -> OitBoundsValue {
    return textureLoad(oit_bounds, pos, 0).rg;
}

// Device depth of the pixel's nearest nearly opaque translucent fragment (0: none).
fn oit_opaque_depth(pos: vec2<i32>) -> f32 {
    return textureLoad(oit_bounds_opaque, pos, 0).r;
}

// Bounds `b` with the far bound brought in front of the nearly opaque depth `opaque`: the
// farthest depth becomes the larger of the two, stored negated.
fn oit_cull_bounds(b: OitBoundsValue, opaque: f32) -> OitBoundsValue {
    return vec2<f32>(b.x, min(b.y, -opaque));
}
