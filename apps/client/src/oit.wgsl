// Improved transparency (option "improvedTransparency"): common functions, appended to the
// shader of a translucent family for its three stages (docs/specs/oit.md). `oit.rs` follows them
// with one layout of the depth bounds (`oit_bounds_compact.wgsl` or `oit_bounds_rgba.wgsl`) and one
// form of the wavelet (`oit_wavelet_unrolled.wgsl` or `oit_wavelet_loops.wgsl`), chosen by switches.
// Group 3: 0 = projection coefficients, 1 = depth bounds, 2 and 3 = transmittance
// coefficients (RGBA16F).
//
// "Bounds" stage: nearest and farthest depth, nearest opaque depth, blended with MAX.
// "Transmittance" stage: Haar wavelet absorbance (rank 2, 8 coefficients) over the
// depth normalised between the bounds, added into two targets.
// "Accumulation" stage: premultiplied colour × transmittance in front of the fragment.

struct OitParams {
    // x = P[2][2], y = P[3][2] of the world projection (reversed depth [0, 1]).
    depth: vec4<f32>,
};

@group(3) @binding(0) var<uniform> oit_params: OitParams;
@group(3) @binding(1) var oit_bounds: texture_2d<f32>;
@group(3) @binding(2) var oit_coeff0: texture_2d<f32>;
@group(3) @binding(3) var oit_coeff1: texture_2d<f32>;

const OIT_COEFFS: i32 = 8;
const OIT_RANK: i32 = 2;
// As many depth bins as coefficients.
const OIT_BINS: i32 = 8;
const OIT_OPAQUE_ALPHA: f32 = 0.99;
const OIT_PRECISE_DEPTH: f32 = 10.0;
const OIT_COARSE_BINS: i32 = 2;

struct OitCoeffs {
    @location(0) c0: vec4<f32>,
    @location(1) c1: vec4<f32>,
};

// Distance (blocks) of a device depth.
fn oit_linear(d: f32) -> f32 {
    return oit_params.depth.y / (d + oit_params.depth.x);
}

fn oit_absorbance(t: f32) -> f32 {
    return clamp(-log(max(t, 0.0001)), 0.0, 4.0);
}

fn oit_transmittance_of(a: f32) -> f32 {
    return clamp(exp(-a), 0.0001, 1.0);
}

// Fragment depth brought into [0, 1] between the pixel's bounds; beyond a 13.3-block
// range, the first 6 bins cover the first 10 blocks.
fn oit_normalize(frag: vec4<f32>) -> f32 {
    let near_far = oit_near_far(vec2<i32>(frag.xy));
    let near = near_far.x;
    let far = near_far.y;
    let lin = oit_linear(frag.z);
    let range = max(far - near, 0.0001);
    let within = clamp(lin - near, 0.0, range);
    let fine = f32(OIT_BINS - OIT_COARSE_BINS) / f32(OIT_BINS);
    let fine_range = fine * range;
    if fine_range <= OIT_PRECISE_DEPTH {
        return within / range;
    } else if within <= OIT_PRECISE_DEPTH {
        return (within / OIT_PRECISE_DEPTH) * fine;
    }
    return fine + ((within - OIT_PRECISE_DEPTH) / (range - OIT_PRECISE_DEPTH)) * (1.0 - fine);
}

fn oit_sample_transmittance(pos: vec2<i32>, depth: f32, current: f32) -> f32 {
    let low = textureLoad(oit_coeff0, pos, 0);
    let high = textureLoad(oit_coeff1, pos, 0);
    return oit_transmittance_of(oit_sample_absorbance(low, high, depth, oit_absorbance(current)));
}

// "Accumulation" stage: premultiplied colour attenuated by what is in front; additive
// surfaces occlude nothing (own transmittance 1, accumulated alpha zero).
fn oit_accumulate(frag: vec4<f32>, color: vec4<f32>, additive: bool) -> vec4<f32> {
    var t = 1.0 - color.a;
    var accum_alpha = color.a;
    if additive {
        t = 1.0;
        accum_alpha = 0.0;
    }
    let s = oit_sample_transmittance(vec2<i32>(frag.xy), oit_normalize(frag), t);
    return vec4<f32>(color.rgb * color.a, accum_alpha) * s;
}
