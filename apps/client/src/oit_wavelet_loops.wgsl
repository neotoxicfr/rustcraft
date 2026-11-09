// Improved transparency, wavelet of the transmittance with a loop over the tree levels and a local
// array of the eight coefficients (`RUSTCRAFT_OPT_OIT_UNROLLED_WAVELET=0`, the code before).

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
// `low` and `high`: coefficients 0-3 and 4-7.
fn oit_sample_absorbance(low: vec4<f32>, high: vec4<f32>, original: f32, current: f32) -> f32 {
    var c = array<f32, 8>(low.x, low.y, low.z, low.w, high.x, high.y, high.z, high.w);
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
