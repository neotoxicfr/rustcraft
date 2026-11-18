// GUI item atlas: vertices in atlas pixels (y downwards, z towards the viewer).

struct Atlas {
    // x: atlas side, y: glint intensity.
    size: vec4<f32>,
    // Glint matrix: rows u and v applied to (u, v, 1).
    glint_u: vec4<f32>,
    glint_v: vec4<f32>,
}

@group(0) @binding(0) var<uniform> atlas: Atlas;
@group(0) @binding(1) var sprites: texture_2d<f32>;
@group(0) @binding(2) var nearest: sampler;
@group(0) @binding(3) var glint_tex: texture_2d<f32>;
@group(0) @binding(4) var repeat: sampler;

struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    // 0 no glint, 1 standard glint (sprite UVs), 2 special glint (decal UVs).
    @location(2) glint: f32,
    @location(3) guv: vec2<f32>,
}

@vertex
fn vs(
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) glint: f32,
    @location(4) guv: vec2<f32>,
) -> Out {
    var o: Out;
    o.guv = guv;
    let s = atlas.size.x;
    // Reversed depth: z = +1000 → 1, z = −1000 → 0.
    o.clip = vec4<f32>(pos.x / s * 2.0 - 1.0, 1.0 - pos.y / s * 2.0, clamp(pos.z / 2000.0 + 0.5, 0.0, 1.0), 1.0);
    o.uv = uv;
    o.color = color;
    o.glint = glint;
    return o;
}

@fragment
fn fs(i: Out) -> @location(0) vec4<f32> {
    let t = textureSampleLevel(sprites, nearest, i.uv, 0.0);
    if t.a < 0.1 {
        discard;
    }
    var c = t * i.color;
    if i.glint > 0.5 {
        // Glint: alpha raised to the intensity, then the square of the glint texture is added.
        let a = atlas.size.y;
        // Compasses and clock: projected per-face coordinates instead of those of the sprite.
        let p = vec3<f32>(select(i.uv, i.guv, i.glint > 1.5), 1.0);
        let guv = vec2<f32>(dot(atlas.glint_u.xyz, p), dot(atlas.glint_v.xyz, p));
        let g = a * textureSampleLevel(glint_tex, repeat, guv, 0.0);
        c = vec4<f32>(c.rgb + g.rgb * g.rgb, max(c.a, a));
    }
    return c;
}

@fragment
fn fs_clear(i: Out) -> @location(0) vec4<f32> {
    return vec4<f32>(0.0);
}
