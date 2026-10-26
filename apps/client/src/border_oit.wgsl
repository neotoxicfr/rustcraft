// World border, improved transparency (continuation of border.wgsl and oit.wgsl): additive
// surface (bounds on the texture's alpha alone, nothing in the transmittance).

fn border_texel(i: Out) -> vec4<f32> {
    let t = textureSample(tex, tex_sampler, i.uv);
    if t.a == 0.0 {
        discard;
    }
    return t;
}

@fragment
fn fs_bounds(i: Out) -> @location(0) vec4<f32> {
    return oit_depth_bounds(i.pos, border_texel(i).a);
}

@fragment
fn fs_trans(i: Out) -> OitCoeffs {
    discard;
    return OitCoeffs(vec4<f32>(0.0), vec4<f32>(0.0));
}

@fragment
fn fs_accum(i: Out) -> @location(0) vec4<f32> {
    return oit_accumulate(i.pos, border_texel(i) * wall.color, true);
}
