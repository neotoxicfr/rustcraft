// Clouds, improved transparency (continuation of sky.wgsl and oit.wgsl): bounds without the fade by
// distance; neither fog nor fade added to the accumulation (like classic rendering).

fn cloud_color(i: V) -> vec4<f32> {
    return vec4<f32>(i.color.rgb, i.color.a * (1.0 - ramp(i.dist, 0.0, u.extra.w)));
}

@fragment
fn fs_cloud_bounds(i: V) -> @location(0) vec4<f32> {
    return oit_depth_bounds(i.pos, i.color.a);
}

@fragment
fn fs_cloud_trans(i: V) -> OitCoeffs {
    return oit_add_transmittance(i.pos, 1.0 - cloud_color(i).a);
}

@fragment
fn fs_cloud_accum(i: V) -> @location(0) vec4<f32> {
    return oit_accumulate(i.pos, cloud_color(i), false);
}
