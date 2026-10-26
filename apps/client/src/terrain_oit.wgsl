// Translucent terrain, improved transparency (continuation of terrain.wgsl and oit.wgsl). The
// alpha-only steps ignore the appearance fade, like the game.

fn translucent_texel(i: Frag) -> vec4<f32> {
    let c = texel(i);
    if c.a < 0.1 {
        discard;
    }
    return c;
}

@fragment
fn fs_translucent_bounds(o: Out) -> @location(0) vec4<f32> {
    let i = frag(o);
    return oit_depth_bounds(i.pos, translucent_texel(i).a);
}

@fragment
fn fs_translucent_trans(o: Out) -> OitCoeffs {
    let i = frag(o);
    return oit_add_transmittance(i.pos, 1.0 - translucent_texel(i).a);
}

@fragment
fn fs_translucent_accum(o: Out) -> @location(0) vec4<f32> {
    let i = frag(o);
    let c = translucent_texel(i);
    let a = oit_accumulate(i.pos, vec4<f32>(appeared(c.rgb * i.tint, i.vis), c.a), false);
    // Fog towards its color premultiplied by the accumulated alpha.
    let v = max(linear_fog(i.fogd.x, cam.fog.x, cam.fog.y), linear_fog(i.fogd.y, cam.fog.z, cam.fog.w));
    return vec4<f32>(mix(a.rgb, cam.fog_color.rgb * a.a, v * cam.fog_color.a), a.a);
}
