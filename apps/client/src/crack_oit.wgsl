// Mining cracks of TRANSLUCENT blocks with improved transparency (continuation of crack.wgsl and
// oit.wgsl): accumulated like a translucent surface (no more multiplicative blend), without
// depth offset, with the `destroy_oit_stage_<stage>` set (layers 10 to 19).

fn crack_texel(i: Out) -> vec4<f32> {
    let c = textureSample(stages, samp, i.uv, i32(i.stage) + 10);
    if c.a < 0.1 {
        discard;
    }
    return c;
}

@fragment
fn fs_bounds(i: Out) -> @location(0) vec4<f32> {
    return oit_depth_bounds(i.pos, crack_texel(i).a);
}

@fragment
fn fs_trans(i: Out) -> OitCoeffs {
    return oit_add_transmittance(i.pos, 1.0 - crack_texel(i).a);
}

@fragment
fn fs_accum(i: Out) -> @location(0) vec4<f32> {
    let a = oit_accumulate(i.pos, crack_texel(i), false);
    let v = max(linear_fog(i.fogd.x, cam.fog.x, cam.fog.y), linear_fog(i.fogd.y, cam.fog.z, cam.fog.w));
    return vec4<f32>(mix(a.rgb, cam.fog_color.rgb * a.a, v * cam.fog_color.a), a.a);
}
