// Box blur of the menu background as sliding sums (`blur.wgsl` is the fragment form): one pass
// along `dir`. Each invocation walks a run of RUN_TEXELS texels and keeps the sum of the
// 2r + 1 texels around the current one: moving one texel on adds the texel entering the window
// and removes the one leaving it, so a texel costs two reads whatever the radius (the fragment
// form reads r + 1 bilinear samples).
//
// The sums are those of the stored 8-bit values, in integers, and the average is rounded to the
// nearest like the write of the fragment pass into its 8-bit target: the same bytes.

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
@group(0) @binding(2) var dst: texture_storage_2d<rgba8unorm, write>;

// Short enough for every line to be shared between many invocations, long enough for the
// 2r + 1 reads that start a run to stay a small share of its cost.
const RUN_TEXELS: i32 = 64;

// Stored 8-bit values of the texel `along` the pass direction in the line `across`; beyond an
// edge, the edge texel (what the sampler of the fragment form returns there).
fn stored(along: i32, across: i32, axis: vec2<i32>, last: vec2<i32>) -> vec4<u32> {
    let at = clamp(axis * along + (vec2<i32>(1) - axis) * across, vec2<i32>(0), last);
    return vec4<u32>(textureLoad(src, at, 0) * 255.0 + 0.5);
}

// Invocation (run, line): the runs of a line follow each other along the pass direction.
@compute @workgroup_size(4, 16)
fn blur_runs(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = vec2<i32>(textureDimensions(src));
    let axis = vec2<i32>(cfg.dir);
    let across_axis = vec2<i32>(1) - axis;
    let first = i32(id.x) * RUN_TEXELS;
    let across = i32(id.y);
    if first >= dot(size, axis) || across >= dot(size, across_axis) {
        return;
    }
    let r = i32(cfg.radius);
    let last = size - 1;
    var sum = vec4<u32>(0u);
    for (var k = -r; k <= r; k++) {
        sum += stored(first + k, across, axis, last);
    }
    // Nearest integer to sum / (2r + 1) = floor((sum + r + 0.5) / (2r + 1)). The count being odd,
    // that quotient is never closer than 0.5 / (2r + 1) to an integer: the float product below
    // (error under 1e-4) cannot land on the wrong side.
    let rounding = f32(r) + 0.5;
    let inv_count = 1.0 / f32(2 * r + 1);
    let end = min(first + RUN_TEXELS, dot(size, axis));
    for (var along = first; along < end; along++) {
        let average = floor((vec4<f32>(sum) + rounding) * inv_count);
        textureStore(dst, axis * along + across_axis * across, average / 255.0);
        sum = sum + stored(along + r + 1, across, axis, last) - stored(along - r, across, axis, last);
    }
}
