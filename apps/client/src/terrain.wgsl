// Textured terrain. One instance per face: block position (x 4 bits, z 4 bits, y 12 bits,
// relative to the bottom of the world) + baked face index. The geometry (corners, UV, texture,
// tint, shading) is read from the baked-face array.

struct Camera {
    view_proj: mat4x4<f32>,
    // x, z: integer origin of the camera in blocks; y: bottom of the world.
    origin: vec4<i32>,
    // Shading per direction: down, up, north, south (x..w), west, east (x, y).
    shade_a: vec4<f32>,
    shade_b: vec2<f32>,
    // GPU appear fade (`FADEGPU`): frame instant (µs modulo 2^32), duration (µs).
    fade_now: u32,
    fade_len: f32,
    // Eye in the vertex frame (x, z relative to the integer origin, y absolute).
    eye: vec4<f32>,
    // Fog (docs/specs/environment.md § 7): color; environmental start and end,
    // render-distance start and end; end of the sky fog.
    fog_color: vec4<f32>,
    fog: vec4<f32>,
    fog_sky: vec4<f32>,
    // Sky color and view projection without translation (sky dome).
    sky_color: vec4<f32>,
    sky_view_proj: mat4x4<f32>,
};

struct Quad {
    // xyz: corner as a block fraction; w: u.
    p: array<vec4<f32>, 4>,
    // v of each corner.
    v: vec4<f32>,
    // x: texture layer; y: tint (0xffffffff = none); z: direction | shading << 3.
    info: vec4<u32>,
};

struct Column {
    pos: vec4<i32>,
};

@group(0) @binding(0) var<uniform> cam: Camera;
@group(0) @binding(1) var<storage, read> quads: array<Quad>;
@group(0) @binding(2) var atlas: texture_2d_array<f32>;
@group(0) @binding(3) var atlas_sampler: sampler;
@group(0) @binding(4) var<storage, read> anim: array<vec2<u32>>;
@group(0) @binding(5) var atlas_big: texture_2d_array<f32>;
// Appear fade per column, ring of 128 × 128 (x, z modulo 128).
//#if FADEGPU
// Birth instant of the column (µs modulo 2^32); 0xffffffff: fade finished.
@group(0) @binding(6) var<storage, read> fade: array<u32>;
//#else
// Visibility of the column, rewritten by the CPU every frame.
@group(0) @binding(6) var<storage, read> fade: array<f32>;
//#endif
// Lightmap: column = block level, row = sky level; read per vertex bilinearly.
@group(0) @binding(7) var lightmap: texture_2d<f32>;
@group(0) @binding(8) var lightmap_sampler: sampler;
// Offsets of the compact-face models: [class − 1][axis][nibble] (rc_mesh offset_table).
@group(0) @binding(9) var<storage, read> offsets: array<f32>;
@group(1) @binding(0) var<uniform> col: Column;

// Fluid or translucent quad in the old format (128 bytes), read by index for the sorted order.
struct FluidQuad {
    p: array<vec4<f32>, 4>,
    v: vec4<f32>,
    info: vec4<u32>,
    colors: vec4<u32>,
    light: vec4<u32>,
};

// Lightmap color for a packed light (block × 16 | sky × 16 << 8).
fn light_color(l: u32) -> vec3<f32> {
    let uv = clamp(vec2<f32>(f32(l & 0xffu), f32(l >> 8u)) / 256.0 + 1.0 / 32.0, vec2<f32>(1.0 / 32.0), vec2<f32>(31.0 / 32.0));
    return textureSampleLevel(lightmap, lightmap_sampler, uv, 0.0).rgb;
}

// Light of corner `corner` in two words (corners 0-1, 2-3, 16 bits each).
fn corner_light(l: vec2<u32>, corner: u32) -> u32 {
    let w = select(l.x, l.y, corner >= 2u);
    return (w >> ((corner & 1u) * 16u)) & 0xffffu;
}
@group(1) @binding(1) var<storage, read> region_quads: array<FluidQuad>;

// Compact translucent quad (108 bytes, `RUSTCRAFT_OPT_TRANSCOMPACT`), same buffer read differently:
// corners (x, y, z, u) × 4 then v × 4; texture (15 bits) | double face << 15 | slot of the
// column << 16; color of each corner; light of corners 0-1 and 2-3.
struct TransQuad {
    c: array<f32, 20>,
    info: u32,
    colors: array<u32, 4>,
    light: array<u32, 2>,
};
@group(1) @binding(2) var<storage, read> region_trans: array<TransQuad>;
// Absolute column of each slot of the compact translucent quads.
@group(1) @binding(3) var<storage, read> trans_columns: array<vec2<i32>>;

// Block face of the region (24 bytes, same content as the instance attribute of `vs`), read by
// index in the indexed draws (`vs_idx`).
struct Face {
    packed: u32,
    quad_index: u32,
    tint: u32,
    light0: u32,
    light1: u32,
    gray: u32,
};
@group(1) @binding(4) var<storage, read> region_faces: array<Face>;

// Outputs of a vertex, as the fragments read them.
struct Frag {
    pos: vec4<f32>,
    uv: vec2<f32>,
    layer: u32,
    tint: vec3<f32>,
    // Animation: next layer and share of it (0 = no fade).
    next: u32,
    fade: f32,
    // Distances at the vertex for the fog: spherical, cylindrical.
    fogd: vec2<f32>,
    // Appear visibility of the section (1 = fully visible).
    vis: f32,
};

//#if VARYINGS
// Grouped outputs: uv and fog, tint and visibility, animation (layer | next << 16,
// share as f32 bits), 3 slots instead of 7.
struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv_fog: vec4<f32>,
    @location(1) tint_vis: vec4<f32>,
    @location(2) @interpolate(flat) anim: vec2<u32>,
};

fn pack(f: Frag) -> Out {
    var o: Out;
    o.pos = f.pos;
    o.uv_fog = vec4<f32>(f.uv, f.fogd);
    o.tint_vis = vec4<f32>(f.tint, f.vis);
    o.anim = vec2<u32>(f.layer | (f.next << 16u), bitcast<u32>(f.fade));
    return probe(o);
}

fn frag(i: Out) -> Frag {
    var f: Frag;
    f.pos = i.pos;
    f.uv = i.uv_fog.xy;
    f.fogd = i.uv_fog.zw;
    f.tint = i.tint_vis.xyz;
    f.vis = i.tint_vis.w;
    f.layer = i.anim.x & 0xffffu;
    f.next = i.anim.x >> 16u;
    f.fade = bitcast<f32>(i.anim.y);
    return f;
}
//#else
struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) layer: u32,
    @location(2) tint: vec3<f32>,
    @location(3) @interpolate(flat) next: u32,
    @location(4) @interpolate(flat) fade: f32,
    @location(5) fogd: vec2<f32>,
    @location(6) vis: f32,
};

fn pack(f: Frag) -> Out {
    var o: Out;
    o.pos = f.pos;
    o.uv = f.uv;
    o.layer = f.layer;
    o.tint = f.tint;
    o.next = f.next;
    o.fade = f.fade;
    o.fogd = f.fogd;
    o.vis = f.vis;
    return probe(o);
}

fn frag(i: Out) -> Frag {
    var f: Frag;
    f.pos = i.pos;
    f.uv = i.uv;
    f.layer = i.layer;
    f.tint = i.tint;
    f.next = i.next;
    f.fade = i.fade;
    f.fogd = i.fogd;
    f.vis = i.vis;
    return f;
}
//#endif

// Diagnostic `RUSTCRAFT_GPUPROBE=vs`: every vertex outside the view (x > w), computation guarded.
fn probe(o: Out) -> Out {
    var p = o;
//#if PROBE_VS
    p.pos = vec4<f32>(abs(o.pos.x) + 2.0 * abs(o.pos.w) + 1.0, o.pos.y, o.pos.z, o.pos.w);
//#endif
    return p;
}

// Appear visibility of the column (0 to 1).
fn column_fade(column: vec2<i32>) -> f32 {
    let slot = u32(((column.x & 127) << 7u) | (column.y & 127));
//#if FADEGPU
    // Same rule as the old CPU computation: elapsed ≥ duration (zero duration included) → 1, otherwise
    // elapsed / duration; exact integer difference, then float.
    let born = fade[slot];
    let e = f32(cam.fade_now - born);
    return select(e / cam.fade_len, 1.0, born == 0xffffffffu || e >= cam.fade_len);
//#else
    return fade[slot];
//#endif
}

// Appear fade: zero within 16 blocks of the eye, full beyond 32.
fn appear(column: vec2<i32>, dist: f32) -> f32 {
    let f = column_fade(column);
    return mix(1.0, f, clamp((dist - 16.0) / 16.0, 0.0, 1.0));
}

// Spherical and cylindrical distances of a position relative to the eye.
fn fog_distances(p: vec3<f32>) -> vec2<f32> {
    return vec2<f32>(length(p), max(length(p.xz), abs(p.y)));
}

fn linear_fog(d: f32, s: f32, e: f32) -> f32 {
    if d <= s { return 0.0; }
    if d >= e { return 1.0; }
    return (d - s) / (e - s);
}

// Terrain fog: the stronger of the two (environment, render distance).
fn apply_fog(c: vec3<f32>, d: vec2<f32>) -> vec3<f32> {
    let v = max(linear_fog(d.x, cam.fog.x, cam.fog.y), linear_fog(d.y, cam.fog.z, cam.fog.w));
    return mix(c, cam.fog_color.rgb, v * cam.fog_color.a);
}

// Layer displayed by texture `t` this tick (animated textures), in `o`.
fn animate(o: ptr<function, Frag>, t: u32) {
    let a = anim[t];
    (*o).layer = a.x & 0xffffu;
    (*o).next = a.x >> 16u;
    (*o).fade = f32(a.y) / 1000.0;
}

// Texture color, blended toward the next frame for interpolated animations.
fn texel(i: Frag) -> vec4<f32> {
    let dx = dpdxFine(i.uv);
    let dy = dpdyFine(i.uv);
    let c = sample_layer(i.uv, i.layer, dx, dy);
    if i.fade > 0.0 {
        return mix(c, sample_layer(i.uv, i.next, dx, dy), i.fade);
    }
    return c;
}

// Layer of one of the two arrays (bit 15: large textures), sampled like the game.
fn sample_layer(uv: vec2<f32>, layer: u32, dx: vec2<f32>, dy: vec2<f32>) -> vec4<f32> {
    let big = (layer & 0x8000u) != 0u;
    var size = vec2<f32>(textureDimensions(atlas));
    if big {
        size = vec2<f32>(textureDimensions(atlas_big));
    }
    let near = fetch_grad(sharpen(uv, size, dx, dy), layer, dx, dy);
    // RGSS filtering (`textureFiltering` setting = 1, `cam.origin.w`): four samples in a
    // rotated grid on two mip levels, mixed with the sharp render according to the texel size.
    if cam.origin.w == 0 {
        return near;
    }
    let pixel = 1.0 / size;
    let screen = sqrt(dx * dx + dy * dy);
    let min_pixel = min(pixel.x, pixel.y);
    let blend = smoothstep(min_pixel, min_pixel * 2.0, max(screen.x, screen.y));
    let effective = sqrt(min(length(dx), length(dy)) * max(length(dx), length(dy)));
    let lod = max(0.0, log2(effective / min_pixel));
    let lo = floor(lod);
    let offsets = array<vec2<f32>, 4>(vec2<f32>(0.125, 0.375), vec2<f32>(-0.125, -0.375), vec2<f32>(0.375, -0.125), vec2<f32>(-0.375, 0.125));
    var a = vec4<f32>(0.0);
    var b = vec4<f32>(0.0);
    for (var k = 0; k < 4; k++) {
        let s = uv + offsets[k] * pixel;
        a += fetch_lod(s, layer, lo);
        b += fetch_lod(s, layer, lo + 1.0);
    }
    return mix(near, mix(a * 0.25, b * 0.25, fract(lod)), blend);
}

fn fetch_grad(uv: vec2<f32>, layer: u32, dx: vec2<f32>, dy: vec2<f32>) -> vec4<f32> {
    if (layer & 0x8000u) != 0u {
        return textureSampleGrad(atlas_big, atlas_sampler, uv, layer & 0x7fffu, dx, dy);
    }
    return textureSampleGrad(atlas, atlas_sampler, uv, layer, dx, dy);
}

fn fetch_lod(uv: vec2<f32>, layer: u32, lod: f32) -> vec4<f32> {
    if (layer & 0x8000u) != 0u {
        return textureSampleLevel(atlas_big, atlas_sampler, uv, layer & 0x7fffu, lod);
    }
    return textureSampleLevel(atlas, atlas_sampler, uv, layer, lod);
}

// The game's sampling (default setting) with a linear filter: the coordinate is
// brought toward the texel center according to the texel size on screen, which keeps pixels
// sharp up close with softened edges, and a smooth image from afar.
fn sharpen(uv: vec2<f32>, size: vec2<f32>, dx: vec2<f32>, dy: vec2<f32>) -> vec2<f32> {
    let pixel = 1.0 / size;
    let screen = sqrt(dx * dx + dy * dy);
    let coords = uv / pixel;
    let center = round(coords) - 0.5;
    let offset = clamp((coords - center - 0.5) * pixel / screen + 0.5, vec2<f32>(0.0), vec2<f32>(1.0));
    return (center + offset) * pixel;
}

// Corner of vertex v (0 to 5) of a quad drawn as two triangles 0-1-2 then 0-2-3.
//#if QUADBITS
fn quad_corner(v: u32) -> u32 {
    // One nibble per vertex, from vertex 0 (low bits) to vertex 5.
    return (0x320210u >> (v * 4u)) & 0xfu;
}
//#else
const QUAD = array<u32, 6>(0u, 1u, 2u, 0u, 2u, 3u);
fn quad_corner(v: u32) -> u32 {
    return QUAD[v];
}
//#endif
// Directional shading: down, up, north, south, west, east.
fn shade_of(d: u32) -> f32 {
    if d < 4u { return cam.shade_a[d]; }
    return cam.shade_b[d - 4u];
}

@vertex
fn vs(
    @builtin(vertex_index) v: u32,
    @location(0) packed: u32,
    @location(1) quad_index: u32,
    @location(2) tint_rgb: u32,
    @location(3) light: vec2<u32>,
    @location(4) gray: u32,
) -> Out {
//#if STRIP
    // Strip of 4 vertices at corners 1, 2, 0, 3: triangles 1-2-0 and 0-2-3, same triangles and
    // same orientation as the list.
    let corner = (0x3021u >> (v * 4u)) & 0xfu;
//#else
    let corner = quad_corner(v);
//#endif
    return pack(block_vertex(corner, packed, quad_index, tint_rgb, light, gray));
}

// Indexed block faces (`RUSTCRAFT_OPT_FACEIDX`): one instance per command, its attribute
// gives the first face (identity buffer: no implicit offset of vertices or instances,
// wrong in DX12 without validation of the indirect commands); 4 vertices per face, shared indices.
@vertex
fn vs_idx(@builtin(vertex_index) v: u32, @location(0) first: u32) -> Out {
    let f = region_faces[first + (v >> 2u)];
    return pack(block_vertex(v & 3u, f.packed, f.quad_index, f.tint, vec2<u32>(f.light0, f.light1), f.gray));
}

fn block_vertex(corner: u32, packed: u32, quad_index: u32, tint_rgb: u32, light: vec2<u32>, gray: u32) -> Frag {
    // Compact offset face (class in bits 28-31 of the position): index on 20 bits, nibbles
    // of the offset above.
    let kind = packed >> 28u;
    var qi = quad_index;
    if kind != 0u {
        qi = quad_index & 0xfffffu;
    }
//#if VSREAD
    // Only the corner, its v and the information are read.
    let p = quads[qi].p[corner];
    let pv = quads[qi].v[corner];
    let info = quads[qi].info;
//#else
    let q = quads[qi];
    let p = q.p[corner];
    let pv = q.v[corner];
    let info = q.info;
//#endif
    let block = vec3<i32>(i32(packed & 15u), i32((packed >> 8u) & 4095u), i32((packed >> 4u) & 15u));
    // Camera-relative integers, then block fraction: precision intact far from the origin.
    // Region (uniform) + column in the region (bits 20-27 of the face).
    let column = col.pos.xy + vec2<i32>(i32((packed >> 20u) & 15u), i32((packed >> 24u) & 15u));
    var world: vec3<f32>;
    if kind == 0u {
        let base = vec3<i32>(column.x * 16, cam.origin.y, column.y * 16) + block - vec3<i32>(cam.origin.x, 0, cam.origin.z);
        world = vec3<f32>(base) + p.xyz;
    } else {
        // Same computation as the old explicit quad: column + ((block + offset) + corner), in f32.
        let t = (kind - 1u) * 48u;
        let off = vec3<f32>(
            offsets[t + ((quad_index >> 20u) & 15u)],
            offsets[t + 16u + ((quad_index >> 24u) & 15u)],
            offsets[t + 32u + (quad_index >> 28u)],
        );
        let base = vec3<i32>(column.x * 16, cam.origin.y, column.y * 16) - vec3<i32>(cam.origin.x, 0, cam.origin.z);
        world = vec3<f32>(base) + ((vec3<f32>(block) + off) + p.xyz);
    }
    var o: Frag;
    o.pos = cam.view_proj * vec4<f32>(world, 1.0);
    o.fogd = fog_distances(world - cam.eye.xyz);
    o.vis = appear(column, o.fogd.x);
    o.uv = vec2<f32>(p.w, pv);
    animate(&o, info.x);
    // Vertex color like the game, in truncated integers: occlusion gray × shading of the
    // direction chosen at baking, then × tint / 255 (0xffffffff = none); then lightmap.
    let shade = shade_of(info.z & 7u);
    let g = min(255u, u32(f32((gray >> (corner * 8u)) & 0xffu) * shade));
    var rgb = vec3<u32>(g);
    if tint_rgb != 0xffffffffu {
        rgb = g * vec3<u32>((tint_rgb >> 16u) & 0xffu, (tint_rgb >> 8u) & 0xffu, tint_rgb & 0xffu) / 255u;
    }
    if kind != 0u {
        // Color scaled like that of the old explicit quad (`fluid_vertex`).
        let c = 0xff000000u | (rgb.x << 16u) | (rgb.y << 8u) | rgb.z;
        o.tint = unpack4x8unorm(c).zyx * light_color(corner_light(light, corner));
        return o;
    }
    o.tint = vec3<f32>(rgb) / 255.0 * light_color(corner_light(light, corner));
    return o;
}

@fragment
fn fs_solid(o: Out) -> @location(0) vec4<f32> {
//#if PROBE_FS
    return vec4<f32>(cam.fog_color.rgb, 1.0);
//#else
    let i = frag(o);
    let c = texel(i);
    return vec4<f32>(apply_fog(appeared(c.rgb * i.tint, i.vis), i.fogd), 1.0);
//#endif
}

// Color blended from the fog color during the appearance of the section.
fn appeared(c: vec3<f32>, vis: f32) -> vec3<f32> {
    // Blend always done, like the game (roundings included when the visibility is 1).
    return mix(cam.fog_color.rgb, c, vis);
}

@fragment
fn fs_cutout(o: Out) -> @location(0) vec4<f32> {
//#if PROBE_FS
    return vec4<f32>(cam.fog_color.rgb, 1.0);
//#else
    let i = frag(o);
    let c = texel(i);
    if c.a < 0.5 {
        discard;
    }
    return vec4<f32>(apply_fog(appeared(c.rgb * i.tint, i.vis), i.fogd), 1.0);
//#endif
}

@fragment
fn fs_translucent(o: Out) -> @location(0) vec4<f32> {
//#if PROBE_FS
    return vec4<f32>(cam.fog_color.rgb, 0.5);
//#else
    let i = frag(o);
    let c = texel(i);
    // Cutout threshold of the game's translucent layer.
    if c.a < 0.1 {
        discard;
    }
    return vec4<f32>(apply_fog(appeared(c.rgb * i.tint, i.vis), i.fogd), c.a);
//#endif
}

// Plain sky: fans of 10 vertices in a view frame without translation (§ 8). Instance 0: dome
// (y = 16, sky color); instance 1: dark disk (y = −16, opposite x, black, offset by
// +12 in y for display only). Eight triangles (center, a, a + 45°).
struct SkyOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) color: vec3<f32>,
    @location(1) fogd: vec2<f32>,
};

@vertex
fn vs_sky(@builtin(vertex_index) v: u32, @builtin(instance_index) inst: u32) -> SkyOut {
    let tri = v / 3u;
    let k = v % 3u;
    var p = vec3<f32>(0.0, 16.0, 0.0);
    if k != 0u {
        let a = radians(-180.0 + 45.0 * f32(tri + k - 1u));
        p = vec3<f32>(512.0 * cos(a), 16.0, 512.0 * sin(a));
    }
    var color = cam.sky_color.rgb;
    var shown = p;
    if inst == 1u {
        p = vec3<f32>(-p.x, -16.0, p.z);
        color = vec3<f32>(0.0);
        shown = p + vec3<f32>(0.0, 12.0, 0.0);
    }
    var o: SkyOut;
    o.pos = cam.sky_view_proj * vec4<f32>(shown, 1.0);
    o.color = color;
    o.fogd = fog_distances(p);
    return o;
}

@fragment
fn fs_sky(i: SkyOut) -> @location(0) vec4<f32> {
    let e = cam.fog_sky.x;
    let v = max(linear_fog(i.fogd.x, 0.0, e), linear_fog(i.fogd.y, e, e));
    return vec4<f32>(mix(i.color, cam.fog_color.rgb, v), 1.0);
}

// Fluid faces: explicit geometry (variable heights), one instance per face.
@vertex
fn vs_fluid(
    @builtin(vertex_index) v: u32,
    @location(0) p0: vec4<f32>,
    @location(1) p1: vec4<f32>,
    @location(2) p2: vec4<f32>,
    @location(3) p3: vec4<f32>,
    @location(4) vs: vec4<f32>,
    @location(5) info: vec4<u32>,
    @location(6) colors: vec4<u32>,
    @location(7) light: vec4<u32>,
) -> Out {
    let column = col.pos.xy + vec2<i32>(i32((info.x >> 16u) & 15u), i32((info.x >> 20u) & 15u));
    return pack(fluid_vertex(v, p0, p1, p2, p3, vs, info.x, colors, light.xy, column));
}


// Translucents in sorted order: the instance carries the quad's index in the global buffer,
// the quad carries its absolute column (z, w).
@vertex
fn vs_sorted(@builtin(vertex_index) v: u32, @location(0) index: u32) -> Out {
//#if VSREAD
    // Targeted read: information, then the 4 corners only for a double face, then the
    // vertex's corner, its v, its color and the light.
    let info = region_quads[index].info;
    let column = vec2<i32>(bitcast<i32>(info.z), bitcast<i32>(info.w));
    let base = column_base(column);
    var corner = quad_corner(v);
    if (info.x & 0x80000000u) != 0u {
        corner = fluid_corner(v, info.x, base, region_quads[index].p[0], region_quads[index].p[1], region_quads[index].p[2], region_quads[index].p[3]);
    }
    let q = &region_quads[index];
    return pack(fluid_out(corner, base, (*q).p[corner], (*q).v[corner], info.x, (*q).colors[corner], (*q).light.xy, column));
//#else
    let q = region_quads[index];
    let column = vec2<i32>(bitcast<i32>(q.info.z), bitcast<i32>(q.info.w));
    return pack(fluid_vertex(v, q.p[0], q.p[1], q.p[2], q.p[3], q.v, q.info.x, q.colors, q.light.xy, column));
//#endif
}

fn pick(corner: u32, p0: vec4<f32>, p1: vec4<f32>, p2: vec4<f32>, p3: vec4<f32>) -> vec4<f32> {
    var p = p0;
    if corner == 1u { p = p1; } else if corner == 2u { p = p2; } else if corner == 3u { p = p3; }
    return p;
}

// Fluids in compact format (96 bytes, `RUSTCRAFT_OPT_FLUID96`): one color for the 4 corners.
@vertex
fn vs_fluid_c(
    @builtin(vertex_index) v: u32,
    @location(0) p0: vec4<f32>,
    @location(1) p1: vec4<f32>,
    @location(2) p2: vec4<f32>,
    @location(3) p3: vec4<f32>,
    @location(4) vs: vec4<f32>,
    @location(5) info: u32,
    @location(6) color: u32,
    @location(7) light: vec2<u32>,
) -> Out {
    let column = col.pos.xy + vec2<i32>(i32((info >> 16u) & 15u), i32((info >> 20u) & 15u));
    return pack(fluid_vertex(v, p0, p1, p2, p3, vs, info, vec4<u32>(color), light, column));
}

//#if VSREAD
// Corner `k` (x, y, z, u) of compact translucent `i`, read from the buffer.
fn trans_corner(i: u32, k: u32) -> vec4<f32> {
    let o = k * 4u;
    return vec4<f32>(region_trans[i].c[o], region_trans[i].c[o + 1u], region_trans[i].c[o + 2u], region_trans[i].c[o + 3u]);
}
//#endif

// Compact translucents in sorted order (`TransQuad`): column read through its slot.
@vertex
fn vs_sorted_c(@builtin(vertex_index) v: u32, @location(0) index: u32) -> Out {
//#if VSREAD
    let raw = region_trans[index].info;
    let column = trans_columns[raw >> 16u];
    // Texture on 15 bits, double face in bit 31 like the original format.
    let info = (raw & 0x7fffu) | ((raw & 0x8000u) << 16u);
    let base = column_base(column);
    var corner = quad_corner(v);
    if (info & 0x80000000u) != 0u {
        corner = fluid_corner(v, info, base, trans_corner(index, 0u), trans_corner(index, 1u), trans_corner(index, 2u), trans_corner(index, 3u));
    }
    let light = vec2<u32>(region_trans[index].light[0], region_trans[index].light[1]);
    return pack(fluid_out(corner, base, trans_corner(index, corner), region_trans[index].c[16u + corner], info, region_trans[index].colors[corner], light, column));
//#else
    let q = region_trans[index];
    let column = trans_columns[q.info >> 16u];
    let p0 = vec4<f32>(q.c[0], q.c[1], q.c[2], q.c[3]);
    let p1 = vec4<f32>(q.c[4], q.c[5], q.c[6], q.c[7]);
    let p2 = vec4<f32>(q.c[8], q.c[9], q.c[10], q.c[11]);
    let p3 = vec4<f32>(q.c[12], q.c[13], q.c[14], q.c[15]);
    let vs = vec4<f32>(q.c[16], q.c[17], q.c[18], q.c[19]);
    // Texture on 15 bits, double face in bit 31 like the original format.
    let info = (q.info & 0x7fffu) | ((q.info & 0x8000u) << 16u);
    let colors = vec4<u32>(q.colors[0], q.colors[1], q.colors[2], q.colors[3]);
    return pack(fluid_vertex(v, p0, p1, p2, p3, vs, info, colors, vec2<u32>(q.light[0], q.light[1]), column));
//#endif
}

// Clip position (x, y, w) of a fluid corner, same computation as the vertex position.
fn clip_xyw(base: vec3<i32>, p: vec4<f32>) -> vec3<f32> {
    let c = cam.view_proj * vec4<f32>(vec3<f32>(base) + p.xyz, 1.0);
    return c.xyw;
}

// Column corner relative to the camera (integers).
fn column_base(column: vec2<i32>) -> vec3<i32> {
    return vec3<i32>(column.x * 16, cam.origin.y, column.y * 16) - vec3<i32>(cam.origin.x, 0, cam.origin.z);
}

// Corner of vertex v of an explicit quad.
fn fluid_corner(v: u32, info: u32, base: vec3<i32>, p0: vec4<f32>, p1: vec4<f32>, p2: vec4<f32>, p3: vec4<f32>) -> u32 {
    var corner = quad_corner(v);
    if (info & 0x80000000u) != 0u {
        // Double face. The side seen is decided like the rasterization: sign of the determinant of the
        // clip positions (x, y, w), valid even behind the eye, and not with `cam.eye`, which
        // the view bobbing (translation in `view_proj`) moves away from the true center of
        // projection. Triangle seen from behind: 2nd and 3rd vertices swapped, which gives back exactly
        // the triangle of the old back face (corners 0, 3, 2, 1). The two slots
        // take the order of the old path: front triangles (A then B), then back
        // triangles (B then A); a folded quad can see its two triangles overlap, and the order
        // then matters for blending.
        let c0 = clip_xyw(base, p0);
        let c2 = clip_xyw(base, p2);
        let front_a = dot(c0, cross(clip_xyw(base, p1), c2)) >= 0.0;
        let front_b = dot(c0, cross(c2, clip_xyw(base, p3))) >= 0.0;
        // A seen from behind: B goes into the first slot (front B, or back B before back A).
        let t = (v / 3u) ^ select(1u, 0u, front_a);
        let k = v % 3u;
        let swap = !select(front_b, front_a, t == 0u);
        corner = quad_corner(t * 3u + select(k, 3u - k, swap && k != 0u));
    }
    return corner;
}

// Vertex of an explicit quad at corner `corner`: position `p` (x, y, z, u), `pv` (v), color.
fn fluid_out(corner: u32, base: vec3<i32>, p: vec4<f32>, pv: f32, info: u32, color: u32, light: vec2<u32>, column: vec2<i32>) -> Frag {
    var o: Frag;
    let world = vec3<f32>(base) + p.xyz;
    o.pos = cam.view_proj * vec4<f32>(world, 1.0);
    o.fogd = fog_distances(world - cam.eye.xyz);
    o.vis = appear(column, o.fogd.x);
    o.uv = vec2<f32>(p.w, pv);
    animate(&o, info & 0xffffu);
    o.tint = unpack4x8unorm(color).zyx * light_color(corner_light(light, corner));
    return o;
}

fn fluid_vertex(
    v: u32,
    p0: vec4<f32>,
    p1: vec4<f32>,
    p2: vec4<f32>,
    p3: vec4<f32>,
    vs: vec4<f32>,
    info: u32,
    colors: vec4<u32>,
    light: vec2<u32>,
    column: vec2<i32>,
) -> Frag {
    let base = column_base(column);
    let corner = fluid_corner(v, info, base, p0, p1, p2, p3);
    return fluid_out(corner, base, pick(corner, p0, p1, p2, p3), vs[corner], info, colors[corner], light, column);
}
