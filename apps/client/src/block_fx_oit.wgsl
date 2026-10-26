// Beam halos, improved transparency (continuation of block_fx.wgsl and oit.wgsl).

@fragment
fn fs_halo_bounds(i: BeamOut) -> @location(0) vec4<f32> {
    return oit_depth_bounds(i.pos, beam_color(i).a);
}

@fragment
fn fs_halo_trans(i: BeamOut) -> OitCoeffs {
    return oit_add_transmittance(i.pos, 1.0 - beam_color(i).a);
}

@fragment
fn fs_halo_accum(i: BeamOut) -> @location(0) vec4<f32> {
    let a = oit_accumulate(i.pos, beam_color(i), false);
    let d = fog_dist(i.wpos);
    let v = max(linear_fog(d.x, cam.fog.x, cam.fog.y), linear_fog(d.y, cam.fog.z, cam.fog.w));
    return vec4<f32>(mix(a.rgb, cam.fog_color.rgb * a.a, v * cam.fog_color.a), a.a);
}
