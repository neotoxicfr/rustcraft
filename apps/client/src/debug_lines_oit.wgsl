// Translucent debug lines, improved transparency (continuation of debug_lines.wgsl and
// oit.wgsl): segments (fog), faces (no fog).

@fragment
fn fs_bounds(i: Out) -> @location(0) vec4<f32> {
    return oit_depth_bounds(i.pos, i.color.a);
}

@fragment
fn fs_trans(i: Out) -> OitCoeffs {
    return oit_add_transmittance(i.pos, 1.0 - i.color.a);
}

@fragment
fn fs_accum(i: Out) -> @location(0) vec4<f32> {
    let a = oit_accumulate(i.pos, i.color, false);
    let v = max(linear_fog(i.fogd.x, cam.fog.x, cam.fog.y), linear_fog(i.fogd.y, cam.fog.z, cam.fog.w));
    return vec4<f32>(mix(a.rgb, cam.fog_color.rgb * a.a, v * cam.fog_color.a), a.a);
}

fn face_color(i: FaceOut) -> vec4<f32> {
    if i.color.a == 0.0 {
        discard;
    }
    return i.color;
}

@fragment
fn fs_face_bounds(i: FaceOut) -> @location(0) vec4<f32> {
    return oit_depth_bounds(i.pos, face_color(i).a);
}

@fragment
fn fs_face_trans(i: FaceOut) -> OitCoeffs {
    return oit_add_transmittance(i.pos, 1.0 - face_color(i).a);
}

@fragment
fn fs_face_accum(i: FaceOut) -> @location(0) vec4<f32> {
    return oit_accumulate(i.pos, face_color(i), false);
}
