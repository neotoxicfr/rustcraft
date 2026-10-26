// Text displays (translucent backgrounds and text), improved transparency (continuation of
// text_display.wgsl and oit.wgsl): without depth offset, like the game.

fn display_color(i: Out) -> vec4<f32> {
    var c = i.color;
    if i.mode != 0u {
        c = textureSampleLevel(glyphs, nearest, i.uv, i.mode - 1u, 0.0) * c;
    }
    if c.a < 0.1 {
        discard;
    }
    return c;
}

@fragment
fn fs_bounds(i: Out) -> @location(0) vec4<f32> {
    return oit_depth_bounds(i.pos, display_color(i).a);
}

@fragment
fn fs_trans(i: Out) -> OitCoeffs {
    return oit_add_transmittance(i.pos, 1.0 - display_color(i).a);
}

@fragment
fn fs_accum(i: Out) -> @location(0) vec4<f32> {
    let a = oit_accumulate(i.pos, display_color(i), false);
    let v = max(linear_fog(i.fogd.x, cam.fog.x, cam.fog.y), linear_fog(i.fogd.y, cam.fog.z, cam.fog.w));
    return vec4<f32>(mix(a.rgb, cam.fog_color.rgb * a.a, v * cam.fog_color.a), a.a);
}
