// Entities (docs/specs/entities.md part 2 § 3): static vertices of the baked layers, one
// matrix per part and per instance; texture × cardinal diffuse light (per face;
// back-face culling depending on the pass), damage overlay, entity lightmap, enchantment
// glint, terrain fog.

struct Camera {
    view_proj: mat4x4<f32>,
    origin: vec4<i32>,
    shade_a: vec4<f32>,
    shade_b: vec4<f32>,
    eye: vec4<f32>,
    fog_color: vec4<f32>,
    fog: vec4<f32>,
    fog_sky: vec4<f32>,
    sky_color: vec4<f32>,
    sky_view_proj: mat4x4<f32>,
};

struct Lights {
    l0: vec4<f32>,
    l1: vec4<f32>,
    // Rectangle of the shadow texture in the sheet.
    shadow: vec4<f32>,
    // Rectangle of the item enchantment glint in the sheet; scrolling (u and
    // v offsets, strength, 0); rectangle of the armor glint.
    glint: vec4<f32>,
    glint_t: vec4<f32>,
    glint_armor: vec4<f32>,
};

// Full pose of a part (frame relative to the integer origin of the camera) and normal
// matrix.
struct PartMat {
    m: mat4x4<f32>,
    n0: vec4<f32>,
    n1: vec4<f32>,
    n2: vec4<f32>,
};

@group(0) @binding(0) var<uniform> cam: Camera;
@group(0) @binding(7) var lightmap: texture_2d<f32>;
@group(0) @binding(8) var lightmap_sampler: sampler;
@group(1) @binding(0) var sheet: texture_2d<f32>;
@group(1) @binding(1) var nearest: sampler;
@group(1) @binding(2) var<uniform> lights: Lights;
@group(1) @binding(3) var<storage, read> parts: array<PartMat>;

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) light: vec3<f32>,
    @location(3) @interpolate(flat) overlay: u32,
    @location(4) normal: vec3<f32>,
    @location(5) fogd: vec2<f32>,
    @location(6) @interpolate(flat) rect: vec4<f32>,
    // Glint mode (bits 16-17 of the light word): 0 none, 1 item, 2 entity, 3 armor.
    @location(7) @interpolate(flat) glint: u32,
    // Flags of the light word (bits 18-23): 1 emissive (no lightmap), 2 no cardinal
    // light, 4 scrolling texture (offset in the overlay word, no overlay), 8 special
    // glint (compasses, clock: `guv` coordinates), 16 first person (scale 0.75), 32 interface (scale 0.5).
    @location(8) @interpolate(flat) flags: u32,
    // Special glint coordinates before the scrolling matrix.
    @location(9) guv: vec2<f32>,
};

// Special glint: vertex position in the model (fraction of a block) divided by the context
// scale, projected onto the quad's face (model normal, exact axis), / 128. Same table as
// `special_glint_uv` (item_atlas.rs).
fn special_glint_uv(pos: vec3<f32>, n: vec3<f32>, k: f32) -> vec2<f32> {
    let p = pos / 16.0 / k;
    var uv: vec2<f32>;
    if n.y > 0.5 {
        uv = vec2<f32>(p.x, p.z);
    } else if n.y < -0.5 {
        uv = vec2<f32>(p.x, -p.z);
    } else if n.z < -0.5 {
        uv = vec2<f32>(-p.x, -p.y);
    } else if n.z > 0.5 {
        uv = vec2<f32>(p.x, -p.y);
    } else if n.x < -0.5 {
        uv = vec2<f32>(-p.z, -p.y);
    } else {
        uv = vec2<f32>(p.z, -p.y);
    }
    return uv / 128.0;
}

// Context scale for the special glint: interface 0.5, first person 0.75, otherwise 1.
fn glint_scale(flags: u32) -> f32 {
    if (flags & 32u) != 0u { return 0.5; }
    if (flags & 16u) != 0u { return 0.75; }
    return 1.0;
}

fn light_color(l: u32) -> vec3<f32> {
    let uv = clamp(vec2<f32>(f32(l & 0xffu), f32((l >> 8u) & 0xffu)) / 256.0 + 1.0 / 32.0, vec2<f32>(1.0 / 32.0), vec2<f32>(31.0 / 32.0));
    return textureSampleLevel(lightmap, lightmap_sampler, uv, 0.0).rgb;
}

@vertex
fn vs(
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) normal: vec3<f32>,
    @location(3) part: u32,
    @location(4) base: u32,
    @location(5) rect: vec4<f32>,
    @location(6) color: vec4<f32>,
    @location(7) light: u32,
    @location(8) overlay: u32,
) -> Out {
    let p = parts[base + part];
    let world = (p.m * vec4<f32>(pos / 16.0, 1.0)).xyz;
    var o: Out;
    o.pos = cam.view_proj * vec4<f32>(world, 1.0);
    // Rectangle in pixels of the sheet (loaded on demand, it can grow).
    o.uv = (rect.xy + uv * rect.zw) / vec2<f32>(textureDimensions(sheet));
    o.color = color;
    o.rect = rect;
    // Constant light per entity: sampled at the vertex (emissive: full, no lightmap).
    o.flags = (light >> 18u) & 63u;
    o.guv = vec2<f32>(0.0);
    if (o.flags & 8u) != 0u {
        o.guv = special_glint_uv(pos, normal, glint_scale(o.flags));
    }
    o.light = select(light_color(light), vec3<f32>(1.0), (o.flags & 1u) != 0u);
    o.glint = (light >> 16u) & 3u;
    o.overlay = overlay;
    o.normal = mat3x3<f32>(p.n0.xyz, p.n1.xyz, p.n2.xyz) * normal;
    let d = world - cam.eye.xyz;
    o.fogd = vec2<f32>(length(d), max(length(d.xz), abs(d.y)));
    return o;
}

fn linear_fog(d: f32, s: f32, e: f32) -> f32 {
    if d <= s { return 0.0; }
    if d >= e { return 1.0; }
    return (d - s) / (e - s);
}

fn fog_value(d: vec2<f32>) -> f32 {
    return max(linear_fog(d.x, cam.fog.x, cam.fog.y), linear_fog(d.y, cam.fog.z, cam.fog.w)) * cam.fog_color.a;
}

fn apply_fog(c: vec3<f32>, d: vec2<f32>) -> vec3<f32> {
    return mix(c, cam.fog_color.rgb, fog_value(d));
}

// Texel (integer c, possibly outside the rectangle) of the sheet's rectangle `r`, repeated.
fn texel_repeat(r: vec4<f32>, c: vec2<f32>) -> vec4<f32> {
    let w = c - r.zw * floor(c / r.zw);
    return textureLoad(sheet, vec2<i32>(r.xy + w), 0);
}

// Repeated linear read (no mip) of a texture stored in the rectangle `r`: like a separate
// texture with linear filtering and repeat addressing.
fn sample_repeat(r: vec4<f32>, uv: vec2<f32>) -> vec4<f32> {
    let x = uv * r.zw - 0.5;
    let b = floor(x);
    let f = x - b;
    let top = mix(texel_repeat(r, b), texel_repeat(r, b + vec2<f32>(1.0, 0.0)), f.x);
    let bottom = mix(texel_repeat(r, b + vec2<f32>(0.0, 1.0)), texel_repeat(r, b + vec2<f32>(1.0, 1.0)), f.x);
    return mix(top, bottom, f.y);
}

// Enchantment glint texel (mode 1 item, 2 entity, 3 armor): texture matrix
// T(−o0, o1) · Rz(10°) · S(s) applied to the geometry's UVs (UVs of the model texture, or of
// the sheet in pixels / 1024 for item meshes), glint texture read linear
// repeat.
fn glint_texel(i: Out, mode: u32) -> vec4<f32> {
    let sz = vec2<f32>(textureDimensions(sheet));
    var uv: vec2<f32>;
    var s: f32;
    if mode == 1u {
        uv = select(i.uv * sz / 1024.0, i.guv, (i.flags & 8u) != 0u);
        s = 8.0;
    } else {
        uv = (i.uv * sz - i.rect.xy) / i.rect.zw;
        s = select(0.5, 0.16, mode == 3u);
    }
    let c = 0.98480775;
    let sn = 0.17364818;
    let q = uv * s;
    let p = vec2<f32>(q.x * c - q.y * sn, q.x * sn + q.y * c) + vec2<f32>(-lights.glint_t.x, lights.glint_t.y);
    let r = select(lights.glint, lights.glint_armor, mode == 3u);
    return sample_repeat(r, p);
}

// Glint added in the geometry shader: texel × strength.
fn glint_color(i: Out, mode: u32) -> vec3<f32> {
    return lights.glint_t.z * glint_texel(i, mode).rgb;
}

// Diffuse: two world lights, ambient 0.4; back face lit by the opposite normal.
fn diffuse(n: vec3<f32>, front: bool) -> f32 {
    let m = select(-n, n, front);
    let d = max(0.0, dot(lights.l0.xyz, m)) + max(0.0, dot(lights.l1.xyz, m));
    return min(1.0, d * 0.6 + 0.4);
}

// 16 × 16 overlay: rows 0-7 red with alpha 178, rows 8-15 white with alpha (1 − u/15 × 0.75).
fn apply_overlay(c: vec3<f32>, ov: u32) -> vec3<f32> {
    let u = f32(ov & 0xffffu);
    let v = ov >> 16u;
    if v < 8u {
        return mix(vec3<f32>(1.0, 0.0, 0.0), c, 178.0 / 255.0);
    }
    let a = floor((1.0 - u / 15.0 * 0.75) * 255.0) / 255.0;
    return mix(vec3<f32>(1.0), c, a);
}

// Lit color (diffuse, overlay, light), enchantment glint added squared (alpha
// raised to the strength), then fog over everything.
fn shade_texel(i: Out, t: vec4<f32>, front: bool) -> vec4<f32> {
    let lit = select(diffuse(normalize(i.normal), front), 1.0, (i.flags & 2u) != 0u);
    var c = t * vec4<f32>(i.color.rgb * lit, i.color.a);
    var rgb = select(apply_overlay(c.rgb, i.overlay), c.rgb, (i.flags & 4u) != 0u) * i.light;
    if i.glint != 0u {
        let g = glint_color(i, i.glint);
        rgb += g * g;
        c.a = max(c.a, lights.glint_t.z);
    }
    return vec4<f32>(apply_fog(rgb, i.fogd), c.a);
}

// Layer texel, offset and repeated in its rectangle if the texture scrolls (breeze wind).
fn base_texel(i: Out) -> vec4<f32> {
    if (i.flags & 4u) == 0u {
        return textureSampleLevel(sheet, nearest, i.uv, 0.0);
    }
    let sz = vec2<f32>(textureDimensions(sheet));
    let off = vec2<f32>(f32(i.overlay & 0xffffu), f32(i.overlay >> 16u)) / 65535.0;
    let local = fract((i.uv * sz - i.rect.xy) / i.rect.zw + off);
    let px = clamp(local * i.rect.zw, vec2<f32>(0.001), i.rect.zw - 0.001);
    return textureSampleLevel(sheet, nearest, (i.rect.xy + px) / sz, 0.0);
}

fn shade(i: Out, front: bool) -> vec4<f32> {
    let t = base_texel(i);
    if t.a < 0.1 {
        discard;
    }
    return shade_texel(i, t, front);
}

@fragment
fn fs_cutout(i: Out, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let c = shade(i, front);
    return vec4<f32>(c.rgb, 1.0);
}

// Opaque with no cutout (cape, shield, trident, banner…): the color of transparent texels
// is drawn as is; back faces culled by the pass, hence the lighting by the
// front face normal.
@fragment
fn fs_solid(i: Out) -> @location(0) vec4<f32> {
    let t = textureSampleLevel(sheet, nearest, i.uv, 0.0);
    let c = shade_texel(i, t, true);
    return vec4<f32>(c.rgb, 1.0);
}

// Silhouette of a glowing entity: solid color of the instance where the texture is opaque.
@fragment
fn fs_outline(i: Out) -> @location(0) vec4<f32> {
    let t = textureSampleLevel(sheet, nearest, i.uv, 0.0);
    if t.a < 0.1 {
        discard;
    }
    return vec4<f32>(i.color.rgb, 1.0);
}

// Glint in a separate pass (special item with several layers): depth equal to the one already
// drawn, glint texture alone (rejected under alpha 0.1), attenuated by the strength and the
// fog; the blend adds the square of the output color. The mode comes from the light word
// (1 by default).
@fragment
fn fs_glint(i: Out) -> @location(0) vec4<f32> {
    let g = glint_texel(i, max(i.glint, 1u));
    if g.a < 0.1 {
        discard;
    }
    let fade = (1.0 - fog_value(i.fogd)) * lights.glint_t.z;
    return vec4<f32>(g.rgb * fade, 1.0);
}

// Energy swirl (wither armor): layer texture offset by (u, v) (fractions of 1/65536
// encoded in the overlay word) and repeated, gray tint, added with no light or diffuse.
@fragment
fn fs_swirl(i: Out) -> @location(0) vec4<f32> {
    let sz = vec2<f32>(textureDimensions(sheet));
    let off = vec2<f32>(f32(i.overlay & 0xffffu), f32(i.overlay >> 16u)) / 65535.0;
    let local = fract((i.uv * sz - i.rect.xy) / i.rect.zw + off);
    let px = clamp(local * i.rect.zw, vec2<f32>(0.001), i.rect.zw - 0.001);
    let t = textureSampleLevel(sheet, nearest, (i.rect.xy + px) / sz, 0.0);
    if t.a < 0.1 {
        discard;
    }
    return vec4<f32>(apply_fog(t.rgb * i.color.rgb, i.fogd), 1.0);
}

@fragment
fn fs_translucent(i: Out, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    return shade(i, front);
}

@fragment
fn fs_eyes(i: Out) -> @location(0) vec4<f32> {
    let t = textureSampleLevel(sheet, nearest, i.uv, 0.0) * i.color;
    return vec4<f32>(apply_fog(t.rgb, i.fogd), t.a);
}

// Cutout with dissolve (dying dragon): mask at the same UV, offset by (dx, dy) pixels
// encoded as i16 in the overlay word (no overlay for this type); rejected if the alpha of the
// color is below the mask's, then opaque.
@fragment
fn fs_dissolve(i: Out, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let t = textureSampleLevel(sheet, nearest, i.uv, 0.0);
    if t.a < 0.1 {
        discard;
    }
    let off = vec2<f32>(f32(bitcast<i32>(i.overlay << 16u) >> 16u), f32(bitcast<i32>(i.overlay) >> 16u));
    let mask = textureSampleLevel(sheet, nearest, i.uv + off / vec2<f32>(textureDimensions(sheet)), 0.0);
    if i.color.a < mask.a {
        discard;
    }
    let rgb = t.rgb * i.color.rgb * diffuse(normalize(i.normal), front) * i.light;
    return vec4<f32>(apply_fog(rgb, i.fogd), 1.0);
}

// Free geometry (crystal beams, dragon death rays): vertices already in the camera
// frame (blocks), local UVs in the rectangle `rect` (sheet pixels).
struct FreeOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) light: vec3<f32>,
    @location(3) @interpolate(flat) rect: vec4<f32>,
    @location(4) normal: vec3<f32>,
    @location(5) fogd: vec2<f32>,
};

@vertex
fn vs_free(
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) rect: vec4<f32>,
    @location(3) color: vec4<f32>,
    @location(4) light: u32,
    @location(5) normal: vec3<f32>,
) -> FreeOut {
    var o: FreeOut;
    o.pos = cam.view_proj * vec4<f32>(pos, 1.0);
    o.uv = uv;
    o.color = color;
    o.light = light_color(light);
    o.rect = rect;
    o.normal = normal;
    let d = pos - cam.eye.xyz;
    o.fogd = vec2<f32>(length(d), max(length(d.xz), abs(d.y)));
    return o;
}

// Crystal beam: cutout 0.1, per-vertex lighting (no back face), no overlay.
@fragment
fn fs_beam(i: FreeOut) -> @location(0) vec4<f32> {
    // u in [0, 1] (right edge kept inside the rectangle), v repeated (scrolling).
    let local = vec2<f32>(clamp(i.uv.x, 0.0, 0.99999), fract(i.uv.y));
    let uv = (i.rect.xy + local * i.rect.zw) / vec2<f32>(textureDimensions(sheet));
    let t = textureSampleLevel(sheet, nearest, uv, 0.0);
    if t.a < 0.1 {
        discard;
    }
    let rgb = t.rgb * i.color.rgb * diffuse(normalize(i.normal), true) * i.light;
    return vec4<f32>(apply_fog(rgb, i.fogd), 1.0);
}

// Guardian beam: entity cutout with per-face lighting (PER_FACE_LIGHTING), the back face
// is lit by the opposite normal.
@fragment
fn fs_gbeam(i: FreeOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let local = vec2<f32>(clamp(i.uv.x, 0.0, 0.99999), fract(i.uv.y));
    let uv = (i.rect.xy + local * i.rect.zw) / vec2<f32>(textureDimensions(sheet));
    let t = textureSampleLevel(sheet, nearest, uv, 0.0);
    if t.a < 0.1 {
        discard;
    }
    let rgb = t.rgb * i.color.rgb * diffuse(normalize(i.normal), front) * i.light;
    return vec4<f32>(apply_fog(rgb, i.fogd), 1.0);
}

// Rays: vertex color alone (additive blend).
@fragment
fn fs_rays(i: FreeOut) -> @location(0) vec4<f32> {
    return vec4<f32>(apply_fog(i.color.rgb, i.fogd), i.color.a);
}

// Ground shadows: texture × alpha, fog; position moved closer to the camera by 1/4096
// (the game's layer offset) so as not to merge with the top of the block.
struct ShadowOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) alpha: f32,
    @location(2) fogd: vec2<f32>,
};

// One tile per instance: corners (x, z), (x, z + 1), (x + 1, z + 1), (x + 1, z) as two
// triangles; UVs in the shadow texture's space, clamped to [0, 1] at the fragment.
@vertex
fn vs_shadow(
    @builtin(vertex_index) vi: u32,
    @location(0) tile: vec3<f32>,
    @location(1) center: vec2<f32>,
    @location(2) inv: f32,
    @location(3) alpha: f32,
) -> ShadowOut {
    var o: ShadowOut;
    let k = array<u32, 6>(0u, 1u, 2u, 0u, 2u, 3u)[vi];
    let off = vec2<f32>(f32(k >= 2u), f32(k == 1u || k == 2u));
    let c = tile.xz + off;
    let pos = vec3<f32>(c.x, tile.y, c.y);
    let uv = (center - c) * inv + 0.5;
    let rel = pos - cam.eye.xyz;
    let p = cam.eye.xyz + rel * (1.0 - 1.0 / 4096.0);
    o.pos = cam.view_proj * vec4<f32>(p, 1.0);
    o.uv = uv;
    o.alpha = alpha;
    o.fogd = vec2<f32>(length(rel), max(length(rel.xz), abs(rel.y)));
    return o;
}

@fragment
fn fs_shadow(i: ShadowOut) -> @location(0) vec4<f32> {
    // Clamped to the center of the edge texels (stretched edge, without reading the neighboring texture).
    let px = clamp(i.uv * lights.shadow.zw, vec2<f32>(0.5), lights.shadow.zw - 0.5);
    let uv = (lights.shadow.xy + px) / vec2<f32>(textureDimensions(sheet));
    let t = textureSampleLevel(sheet, nearest, uv, 0.0);
    return vec4<f32>(apply_fog(t.rgb, i.fogd), t.a * i.alpha);
}
