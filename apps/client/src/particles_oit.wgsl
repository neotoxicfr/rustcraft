// Translucent particles, improved transparency (continuation of particles.wgsl and oit.wgsl).

fn particle_texel(i: Out) -> vec4<f32> {
    let lo = vec2<i32>(floor(i.rect.xy));
    let hi = max(vec2<i32>(ceil(i.rect.zw)) - 1, lo);
    let p = clamp(vec2<i32>(floor(i.uv)), lo, hi);
    var t: vec4<f32>;
    if i.tex == 0u {
        t = textureLoad(atlas, p, 0);
    } else {
        t = textureLoad(blocks, p, i32(i.tex - 1u), 0);
    }
    let c = t * i.color;
    if c.a < 0.1 {
        discard;
    }
    return c;
}

@fragment
fn fs_bounds(i: Out) -> @location(0) vec4<f32> {
    return oit_depth_bounds(i.pos, particle_texel(i).a);
}

@fragment
fn fs_trans(i: Out) -> OitCoeffs {
    return oit_add_transmittance(i.pos, 1.0 - particle_texel(i).a);
}

@fragment
fn fs_accum(i: Out) -> @location(0) vec4<f32> {
    let a = oit_accumulate(i.pos, particle_texel(i), false);
    let v = max(linear_fog(i.fogd.x, cam.fog.x, cam.fog.y), linear_fog(i.fogd.y, cam.fog.z, cam.fog.w));
    return vec4<f32>(mix(a.rgb, cam.fog_color.rgb * a.a, v * cam.fog_color.a), a.a);
}
