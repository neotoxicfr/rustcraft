// Entities, improved transparency (continuation of entity.wgsl and oit.wgsl): translucent (models,
// items, orbs), eyes, lightning, energy swirls, beams, shadows. Color without fog for
// the steps; fog applied after accumulation towards its premultiplied color.

// Fog without the color's alpha factor (lightning and beams: color × (1 − value)).
fn fog_raw(d: vec2<f32>) -> f32 {
    return max(linear_fog(d.x, cam.fog.x, cam.fog.y), linear_fog(d.y, cam.fog.z, cam.fog.w));
}

fn fog_premultiplied(a: vec4<f32>, d: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(mix(a.rgb, cam.fog_color.rgb * a.a, fog_value(d)), a.a);
}

// Like `shade_texel`, without the fog.
fn shade_plain(i: Out, t: vec4<f32>, front: bool) -> vec4<f32> {
    let lit = select(diffuse(normalize(i.normal), front), 1.0, (i.flags & 2u) != 0u);
    var c = t * vec4<f32>(i.color.rgb * lit, i.color.a);
    var rgb = select(apply_overlay(c.rgb, i.overlay), c.rgb, (i.flags & 4u) != 0u) * i.light;
    if i.glint != 0u {
        let g = glint_color(i, i.glint);
        rgb += g * g;
        c.a = max(c.a, lights.glint_t.z);
    }
    return vec4<f32>(rgb, c.a);
}

fn translucent_color(i: Out, front: bool) -> vec4<f32> {
    let t = base_texel(i);
    if t.a < 0.1 {
        discard;
    }
    return shade_plain(i, t, front);
}

@fragment
fn fs_translucent_bounds(i: Out, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    return oit_depth_bounds(i.pos, translucent_color(i, front).a);
}

@fragment
fn fs_translucent_trans(i: Out, @builtin(front_facing) front: bool) -> OitCoeffs {
    return oit_add_transmittance(i.pos, 1.0 - translucent_color(i, front).a);
}

@fragment
fn fs_translucent_accum(i: Out, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    return fog_premultiplied(oit_accumulate(i.pos, translucent_color(i, front), false), i.fogd);
}

// Eyes: 0.1 cutout on the texture (like all the entities of this family), without light.
fn eyes_color(i: Out) -> vec4<f32> {
    let t = textureSampleLevel(sheet, nearest, i.uv, 0.0);
    if t.a < 0.1 {
        discard;
    }
    return t * i.color;
}

@fragment
fn fs_eyes_bounds(i: Out) -> @location(0) vec4<f32> {
    return oit_depth_bounds(i.pos, eyes_color(i).a);
}

@fragment
fn fs_eyes_trans(i: Out) -> OitCoeffs {
    return oit_add_transmittance(i.pos, 1.0 - eyes_color(i).a);
}

@fragment
fn fs_eyes_accum(i: Out) -> @location(0) vec4<f32> {
    return fog_premultiplied(oit_accumulate(i.pos, eyes_color(i), false), i.fogd);
}

// Lightning: color erased by the fog, additive surface.
fn bolt_color(i: Out) -> vec4<f32> {
    return textureSampleLevel(sheet, nearest, i.uv, 0.0) * i.color * (1.0 - fog_raw(i.fogd));
}

@fragment
fn fs_bolt_bounds(i: Out) -> @location(0) vec4<f32> {
    return oit_depth_bounds(i.pos, bolt_color(i).a);
}

@fragment
fn fs_bolt_trans(i: Out) -> OitCoeffs {
    discard;
    return OitCoeffs(vec4<f32>(0.0), vec4<f32>(0.0));
}

@fragment
fn fs_bolt_accum(i: Out) -> @location(0) vec4<f32> {
    return oit_accumulate(i.pos, bolt_color(i), true);
}

// Energy swirl: additive, alpha capped at 0.99 before the cutout.
fn swirl_color(i: Out) -> vec4<f32> {
    let sz = vec2<f32>(textureDimensions(sheet));
    let off = vec2<f32>(f32(i.overlay & 0xffffu), f32(i.overlay >> 16u)) / 65535.0;
    let local = fract((i.uv * sz - i.rect.xy) / i.rect.zw + off);
    let px = clamp(local * i.rect.zw, vec2<f32>(0.001), i.rect.zw - 0.001);
    let t = textureSampleLevel(sheet, nearest, (i.rect.xy + px) / sz, 0.0);
    let a = min(0.99, t.a);
    if a < 0.1 {
        discard;
    }
    return vec4<f32>(t.rgb * i.color.rgb, a * i.color.a);
}

@fragment
fn fs_swirl_bounds(i: Out) -> @location(0) vec4<f32> {
    return oit_depth_bounds(i.pos, swirl_color(i).a);
}

@fragment
fn fs_swirl_trans(i: Out) -> OitCoeffs {
    discard;
    return OitCoeffs(vec4<f32>(0.0), vec4<f32>(0.0));
}

@fragment
fn fs_swirl_accum(i: Out) -> @location(0) vec4<f32> {
    return fog_premultiplied(oit_accumulate(i.pos, swirl_color(i), true), i.fogd);
}

// Beams of the dying dragon: vertex color erased by the fog, additive.
fn rays_color(i: FreeOut) -> vec4<f32> {
    return i.color * (1.0 - fog_raw(i.fogd));
}

@fragment
fn fs_rays_bounds(i: FreeOut) -> @location(0) vec4<f32> {
    return oit_depth_bounds(i.pos, rays_color(i).a);
}

@fragment
fn fs_rays_trans(i: FreeOut) -> OitCoeffs {
    discard;
    return OitCoeffs(vec4<f32>(0.0), vec4<f32>(0.0));
}

@fragment
fn fs_rays_accum(i: FreeOut) -> @location(0) vec4<f32> {
    return oit_accumulate(i.pos, rays_color(i), true);
}

// Ground shadows.
fn shadow_color(i: ShadowOut) -> vec4<f32> {
    let px = clamp(i.uv * lights.shadow.zw, vec2<f32>(0.5), lights.shadow.zw - 0.5);
    let uv = (lights.shadow.xy + px) / vec2<f32>(textureDimensions(sheet));
    let t = textureSampleLevel(sheet, nearest, uv, 0.0);
    return vec4<f32>(t.rgb, t.a * i.alpha);
}

@fragment
fn fs_shadow_bounds(i: ShadowOut) -> @location(0) vec4<f32> {
    return oit_depth_bounds(i.pos, shadow_color(i).a);
}

@fragment
fn fs_shadow_trans(i: ShadowOut) -> OitCoeffs {
    return oit_add_transmittance(i.pos, 1.0 - shadow_color(i).a);
}

@fragment
fn fs_shadow_accum(i: ShadowOut) -> @location(0) vec4<f32> {
    return fog_premultiplied(oit_accumulate(i.pos, shadow_color(i), false), i.fogd);
}

// Water mask (depth only, no color).
@fragment
fn fs_water_mask(i: Out) {
}
