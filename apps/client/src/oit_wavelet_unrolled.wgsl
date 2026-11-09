// Improved transparency, wavelet of the transmittance unrolled (`RUSTCRAFT_OPT_OIT_UNROLLED_WAVELET`):
// the three levels of the rank-2 tree are computed side by side in vectors (x: finest level, whose
// nodes span 2 bins; y: middle level; z: root) and the coefficients they touch are picked with
// selects. The loop form indexed a local array of eight coefficients by the fragment's bin, which
// most GPUs keep in local memory for every translucent fragment. Same floating-point operations,
// in the same order, as `oit_wavelet_loops.wgsl`.

// Width of a node's span and Haar height of each level (x: finest, y: middle, z: root).
const OIT_LEVEL_WIDTH: vec3<f32> = vec3<f32>(0.25, 0.5, 1.0);
const OIT_LEVEL_HEIGHT: vec3<f32> = vec3<f32>(2.0, exp2(0.5), 1.0);

// Tree node holding bin `bin` at each level: the leaves are numbered from OIT_COEFFS, each
// level up halves the number.
fn oit_tree_path(bin: i32) -> vec3<i32> {
    let leaf = bin + OIT_COEFFS;
    return vec3<i32>(leaf, leaf >> 1u, leaf >> 2u);
}

// +1 for a left child, −1 for a right one, at each level of the path `node`.
fn oit_level_sides(node: vec3<i32>) -> vec3<f32> {
    return vec3<f32>(vec3<i32>(1) - ((node & vec3<i32>(1)) << vec3<u32>(1u)));
}

// Haar edge of `depth` in each level's node of the path `node`.
fn oit_level_edges(node: vec3<i32>, depth: f32) -> vec3<f32> {
    let parent = node >> vec3<u32>(1u);
    // Index of the parent within its level.
    let index = vec3<f32>(parent & vec3<i32>(3, 1, 0));
    let right = vec3<f32>(node & vec3<i32>(1));
    let offset = vec3<f32>(depth) - OIT_LEVEL_WIDTH * index;
    return right * OIT_LEVEL_WIDTH + oit_level_sides(node) * offset;
}

// Coefficients of the parents met on a path (`parent` per level): c[4..7], c[2..3], then c[1];
// `low` and `high` hold coefficients 0-3 and 4-7.
fn oit_path_coefficients(low: vec4<f32>, high: vec4<f32>, parent: vec3<i32>) -> vec3<f32> {
    let finest = select(select(high.x, high.y, parent.x == 5), select(high.z, high.w, parent.x == 7), parent.x >= 6);
    let middle = select(low.z, low.w, parent.y == 3);
    return vec3<f32>(finest, middle, low.y);
}

// "Transmittance" stage: coefficients of the fragment's transmittance `t`.
fn oit_add_transmittance(frag: vec4<f32>, t: f32) -> OitCoeffs {
    let absorbance = oit_absorbance(t);
    let original = oit_normalize(frag);
    let depth = original * f32(OIT_BINS - 1) / f32(OIT_BINS);
    let mean = absorbance * (1.0 - depth);
    let node = oit_tree_path(i32(floor(original * f32(OIT_BINS - 1))));
    let parent = node >> vec3<u32>(1u);
    let value = oit_level_edges(node, depth) * OIT_LEVEL_HEIGHT * absorbance;
    // The root's coefficient is c[1]; the middle level writes c[2] or c[3], the finest c[4..7].
    let middle_left = select(0.0, value.y, parent.y == 2);
    let middle_right = select(0.0, value.y, parent.y == 3);
    let finest = select(vec4<f32>(0.0), vec4<f32>(value.x), vec4<i32>(4, 5, 6, 7) == vec4<i32>(parent.x));
    return OitCoeffs(vec4<f32>(mean, value.z, middle_left, middle_right), finest);
}

// Absorbance accumulated in front of the normalised depth `original`, the fragment's own
// contribution (`current`) removed; interpolated between the fragment's bin (path b) and the
// previous one (path a). `low` and `high`: coefficients 0-3 and 4-7.
fn oit_sample_absorbance(low: vec4<f32>, high: vec4<f32>, original: f32, current: f32) -> f32 {
    var average = low.x;
    if average == 0.0 {
        return 0.0;
    }
    let bins = original * f32(OIT_BINS - 1);
    let depth = bins / f32(OIT_BINS);
    average -= current * (1.0 - depth);
    let bin_b = clamp(i32(floor(bins)), 0, OIT_BINS - 1);
    let sample_a = bin_b >= 1;
    let node_b = oit_tree_path(bin_b);
    let node_a = oit_tree_path(select(bin_b, bin_b - 1, sample_a));
    let parent_b = node_b >> vec3<u32>(1u);
    let parent_a = node_a >> vec3<u32>(1u);
    let own = oit_level_edges(node_b, depth) * OIT_LEVEL_HEIGHT * current;
    let db = oit_path_coefficients(low, high, parent_b) - own;
    // A parent shared by both paths is read with the fragment's contribution removed.
    let da = select(oit_path_coefficients(low, high, parent_a), db, parent_a == parent_b);
    let step_b = OIT_LEVEL_HEIGHT * db * oit_level_sides(node_b);
    let step_a = OIT_LEVEL_HEIGHT * da * oit_level_sides(node_a);
    // Subtracted one level after the other, finest first, as the loop did.
    let sb = ((average - step_b.x) - step_b.y) - step_b.z;
    let sa = select(0.0, ((average - step_a.x) - step_a.y) - step_a.z, sample_a);
    let lerp = select(fract(bins), 1.0, bins >= f32(OIT_BINS));
    return mix(sa, sb, lerp);
}
