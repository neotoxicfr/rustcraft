// Improved transparency (option "improvedTransparency"): common functions, appended to the
// shader of a translucent family for its three stages (docs/specs/oit.md).
// Group 3: 0 = projection coefficients, 1 = depth bounds (RGBA32F),
// 2 and 3 = transmittance coefficients (RGBA16F).
//
// "Bounds" stage: (−lin, lin, depth, depth if opaque) blended with MAX.
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

// "Bounds" stage: almost transparent fragments are rejected.
fn oit_depth_bounds(frag: vec4<f32>, alpha: f32) -> vec4<f32> {
    if alpha < 0.01 {
        discard;
    }
    let lin = oit_linear(frag.z);
    let opaque = select(0.0, frag.z, alpha > OIT_OPAQUE_ALPHA);
    return vec4<f32>(-lin, lin, frag.z, opaque);
}

// Fragment depth brought into [0, 1] between the pixel's bounds; beyond a 13.3-block
// range, the first 6 bins cover the first 10 blocks.
fn oit_normalize(frag: vec4<f32>) -> f32 {
    let b = textureLoad(oit_bounds, vec2<i32>(frag.xy), 0);
    let near = -b.r;
    let far = b.g;
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

// "Transmittance" stage: coefficients of the fragment's transmittance `t`.
fn oit_add_transmittance(frag: vec4<f32>, t: f32) -> OitCoeffs {
    var c: array<f32, 8>;
    let absorbance = oit_absorbance(t);
    let original = oit_normalize(frag);
    let depth = original * f32(OIT_BINS - 1) / f32(OIT_BINS);
    c[0] = absorbance * (1.0 - depth);
    // The tree starts at coefficient 1.
    var tree = i32(floor(original * f32(OIT_BINS - 1))) + OIT_COEFFS;
    for (var level = 0; level <= OIT_RANK; level++) {
        let power = OIT_RANK - level;
        let parent = tree >> 1u;
        let index = f32(parent & ((1 << u32(power)) - 1));
        let right = tree & 1;
        let sgn = 1 - (right << 1u);
        let width = exp2(-f32(power));
        let height = exp2(f32(power) * 0.5);
        let offset = depth - width * index;
        let edge = f32(right) * width + f32(sgn) * offset;
        c[parent] = edge * height * absorbance;
        tree = parent;
    }
    return OitCoeffs(vec4<f32>(c[0], c[1], c[2], c[3]), vec4<f32>(c[4], c[5], c[6], c[7]));
}

// Absorbance accumulated in front of the normalised depth `original`, the fragment's own
// contribution (`current`) removed; interpolated between the fragment's bin and the previous one.
fn oit_sample_absorbance(coefficients: array<f32, 8>, original: f32, current: f32) -> f32 {
    var c = coefficients;
    var average = c[0];
    if average == 0.0 {
        return 0.0;
    }
    let bins = original * f32(OIT_BINS - 1);
    let depth = bins / f32(OIT_BINS);
    average -= current * (1.0 - depth);
    var tb = clamp(i32(floor(bins)), 0, OIT_BINS - 1);
    let sample_a = tb >= 1;
    var ta = select(tb, tb - 1, sample_a);
    tb += OIT_COEFFS;
    ta += OIT_COEFFS;
    var sb = average;
    var sa = select(0.0, average, sample_a);
    for (var level = 0; level <= OIT_RANK; level++) {
        let power = OIT_RANK - level;
        let width = exp2(-f32(power));
        let height = exp2(f32(power) * 0.5);
        let pb = tb >> 1u;
        let rb = tb & 1;
        let sgn_b = 1 - (rb << 1u);
        let index = f32(pb & ((1 << u32(power)) - 1));
        let offset = depth - width * index;
        let own = (f32(rb) * width + f32(sgn_b) * offset) * height * current;
        var db = c[pb];
        db -= own;
        sb -= height * db * f32(sgn_b);
        tb = pb;
        if sample_a {
            let pa = ta >> 1u;
            let da = select(c[pa], db, pa == pb);
            let ra = ta & 1;
            let sgn_a = 1 - (ra << 1u);
            sa -= height * da * f32(sgn_a);
            ta = pa;
        }
    }
    let lerp = select(fract(bins), 1.0, bins >= f32(OIT_BINS));
    return mix(sa, sb, lerp);
}

fn oit_sample_transmittance(pos: vec2<i32>, depth: f32, current: f32) -> f32 {
    let a = textureLoad(oit_coeff0, pos, 0);
    let b = textureLoad(oit_coeff1, pos, 0);
    let c = array<f32, 8>(a.x, a.y, a.z, a.w, b.x, b.y, b.z, b.w);
    return oit_transmittance_of(oit_sample_absorbance(c, depth, oit_absorbance(current)));
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
