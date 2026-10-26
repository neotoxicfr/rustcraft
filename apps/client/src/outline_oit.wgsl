// Normal outline of the targeted block, improved transparency (continuation of outline.wgsl and oit.wgsl).

@fragment
fn fs_bounds(i: Out) -> @location(0) vec4<f32> {
    return oit_depth_bounds(i.pos, style.color.a);
}

@fragment
fn fs_trans(i: Out) -> OitCoeffs {
    return oit_add_transmittance(i.pos, 1.0 - style.color.a);
}

@fragment
fn fs_accum(i: Out) -> @location(0) vec4<f32> {
    let a = oit_accumulate(i.pos, style.color, false);
    let v = max(linear_fog(i.fogd.x, cam.fog.x, cam.fog.y), linear_fog(i.fogd.y, cam.fog.z, cam.fog.w));
    return vec4<f32>(mix(a.rgb, cam.fog_color.rgb * a.a, v * cam.fog_color.a), a.a);
}
