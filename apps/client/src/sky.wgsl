// Full sky (docs/specs/sky.md): sunrise/sunset glow, sun and moon, stars, clouds.
// Everything is relative to the camera; `mvp` contains the projection, the view without translation and the
// element's own transform.

struct Sky {
    mvp: mat4x4<f32>,
    color: vec4<f32>,
    // Celestial bodies: texture layer in x. Clouds: mesh origin (xyz), end of the fog (w).
    extra: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Sky;
@group(0) @binding(1) var celestial: texture_2d_array<f32>;
@group(0) @binding(2) var nearest: sampler;
@group(0) @binding(3) var end_sky: texture_2d<f32>;
@group(0) @binding(4) var repeat: sampler;

struct V {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) dist: f32,
    @location(3) vertex_alpha: f32,
};

// Glow: vertex color (white, alpha 1 at the center, 0 on the ring) × uniform color.
@vertex
fn vs_fan(@location(0) p: vec4<f32>) -> V {
    var o: V;
    o.pos = u.mvp * vec4<f32>(p.xyz, 1.0);
    o.color = vec4<f32>(1.0, 1.0, 1.0, p.w) * u.color;
    o.vertex_alpha = p.w;
    return o;
}

@fragment
fn fs_fan(i: V) -> @location(0) vec4<f32> {
    if i.vertex_alpha == 0.0 {
        discard;
    }
    return i.color;
}

// Sun and moon: texture (nearest neighbor) × uniform color, discard if the texel is transparent.
@vertex
fn vs_body(@location(0) p: vec3<f32>, @location(1) uv: vec2<f32>) -> V {
    var o: V;
    o.pos = u.mvp * vec4<f32>(p, 1.0);
    o.uv = uv;
    o.color = u.color;
    return o;
}

@fragment
fn fs_body(i: V) -> @location(0) vec4<f32> {
    let t = textureSample(celestial, nearest, i.uv, i32(u.extra.x));
    if t.a == 0.0 {
        discard;
    }
    return t * i.color;
}

// End box: repeated texture × uniform color, discard if the texel is transparent.
@fragment
fn fs_end(i: V) -> @location(0) vec4<f32> {
    let t = textureSample(end_sky, repeat, i.uv);
    if t.a == 0.0 {
        discard;
    }
    return t * i.color;
}

// Stars: uniform color only.
@vertex
fn vs_star(@location(0) p: vec3<f32>) -> V {
    var o: V;
    o.pos = u.mvp * vec4<f32>(p, 1.0);
    o.color = u.color;
    return o;
}

@fragment
fn fs_star(i: V) -> @location(0) vec4<f32> {
    return i.color;
}

// Clouds: face factor × color, alpha faded by the spherical distance (no shift
// towards the fog color).
fn cloud_vertex(p: vec4<f32>) -> V {
    let rel = p.xyz + u.extra.xyz;
    var o: V;
    o.pos = u.mvp * vec4<f32>(rel, 1.0);
    o.color = vec4<f32>(p.w, p.w, p.w, 1.0) * u.color;
    o.dist = length(rel);
    return o;
}

@vertex
fn vs_cloud(@location(0) p: vec4<f32>) -> V {
    return cloud_vertex(p);
}

// Faces of the clouds read by vertex number (`RUSTCRAFT_OPT_CLOUD_INDEXED_FACES`), one word per face.
@group(1) @binding(0) var<storage, read> cloud_face_words: array<u32>;

//#if PACKED_SHADER_CONSTANTS
// Corners of the unit cube per face (down, up, north, south, west, east, like sky_mesh.rs) as one
// bit per axis, bit `direction * 4 + corner`: a constant array indexed by a per-vertex value is
// copied into local memory for every vertex, a shift of a constant is not.
const CLOUD_CORNER_X: u32 = 0xf03cc3u;
const CLOUD_CORNER_Y: u32 = 0x6666f0u;
const CLOUD_CORNER_Z: u32 = 0xc3f066u;
// Quad corner (2 bits each) of the 6 vertices of the two triangles 0-1-2 and 2-3-0.
const CLOUD_QUAD_CORNERS: u32 = 0x3a4u;

fn cloud_quad_corner(v: u32) -> u32 {
    return (CLOUD_QUAD_CORNERS >> (2u * v)) & 3u;
}

fn cloud_cube_corner(dir: u32, corner: u32) -> vec3<f32> {
    let bit = dir * 4u + corner;
    return vec3<f32>((vec3<u32>(CLOUD_CORNER_X, CLOUD_CORNER_Y, CLOUD_CORNER_Z) >> vec3<u32>(bit)) & vec3<u32>(1u));
}

// Shading factor of a direction (the same literals as the table of the other branch).
fn cloud_shade(dir: u32) -> f32 {
    if dir == 0u {
        return 0.7;
    }
    if dir == 1u {
        return 1.0;
    }
    if dir < 4u {
        return 0.8;
    }
    return 0.9;
}
//#else
// Corners of the unit cube per face (down, up, north, south, west, east), like sky_mesh.rs.
const CLOUD_CORNERS = array<vec3<f32>, 24>(
    vec3(1.0, 0.0, 0.0), vec3(1.0, 0.0, 1.0), vec3(0.0, 0.0, 1.0), vec3(0.0, 0.0, 0.0),
    vec3(0.0, 1.0, 0.0), vec3(0.0, 1.0, 1.0), vec3(1.0, 1.0, 1.0), vec3(1.0, 1.0, 0.0),
    vec3(0.0, 0.0, 0.0), vec3(0.0, 1.0, 0.0), vec3(1.0, 1.0, 0.0), vec3(1.0, 0.0, 0.0),
    vec3(1.0, 0.0, 1.0), vec3(1.0, 1.0, 1.0), vec3(0.0, 1.0, 1.0), vec3(0.0, 0.0, 1.0),
    vec3(0.0, 0.0, 1.0), vec3(0.0, 1.0, 1.0), vec3(0.0, 1.0, 0.0), vec3(0.0, 0.0, 0.0),
    vec3(1.0, 0.0, 0.0), vec3(1.0, 1.0, 0.0), vec3(1.0, 1.0, 1.0), vec3(1.0, 0.0, 1.0),
);
const CLOUD_SHADE = array<f32, 6>(0.7, 1.0, 0.8, 0.8, 0.9, 0.9);
const CLOUD_QUAD = array<u32, 6>(0u, 1u, 2u, 2u, 3u, 0u);

fn cloud_quad_corner(v: u32) -> u32 {
    var quad = CLOUD_QUAD;
    return quad[v];
}

fn cloud_cube_corner(dir: u32, corner: u32) -> vec3<f32> {
    var corners = CLOUD_CORNERS;
    return corners[dir * 4u + corner];
}

fn cloud_shade(dir: u32) -> f32 {
    var shades = CLOUD_SHADE;
    return shades[dir];
}
//#endif

// Corner `corner` (0 to 3) of face `code` (direction | inner << 3 | top shade << 4) of cell
// (i, k): position relative to the mesh origin, shading factor in w. An inner face takes its
// corners in reverse order (seen from inside the cell).
fn cloud_face_corner(i: i32, k: i32, code: u32, corner: u32) -> vec4<f32> {
    let dir = code & 7u;
    var q = corner;
    if (code & 8u) != 0u {
        q = 3u - q;
    }
    let c = cloud_cube_corner(dir, q);
    var shade = cloud_shade(dir);
    if (code & 16u) != 0u {
        shade = cloud_shade(1u);
    }
    return vec4<f32>(c.x * 12.0 + 12.0 * f32(i), c.y * 4.0, c.z * 12.0 + 12.0 * f32(k), shade);
}

// Vertex `v` (0 to 5, two triangles 0-1-2 and 2-3-0) of face `code` of cell (i, k).
fn cloud_corner(i: i32, k: i32, code: u32, v: u32) -> vec4<f32> {
    return cloud_face_corner(i, k, code, cloud_quad_corner(v));
}

// Corner `corner` of the face of word `w`: i (bits 0-9, signed), k (10-19), code (20-24).
fn cloud_word_corner(w: u32, corner: u32) -> vec4<f32> {
    return cloud_face_corner(bitcast<i32>(w << 22u) >> 22u, bitcast<i32>(w << 12u) >> 22u, (w >> 20u) & 31u, corner);
}

// One face per instance, 6 vertices.
fn cloud_face(w: u32, v: u32) -> vec4<f32> {
    return cloud_word_corner(w, cloud_quad_corner(v));
}

// One cell per instance: up to 4 one-byte codes (255 = empty slot, w = -1).
fn cloud_cell(i: i32, k: i32, codes: u32, v: u32) -> vec4<f32> {
    let code = (codes >> (8u * (v / 6u))) & 255u;
    if code == 255u {
        return vec4<f32>(0.0, 0.0, 0.0, -1.0);
    }
    return cloud_corner(i, k, code, v % 6u);
}

@vertex
fn vs_cloud_face(@builtin(vertex_index) v: u32, @location(0) w: u32) -> V {
    return cloud_vertex(cloud_face(w, v));
}

// Indexed faces: 4 vertices per face (the shared index list gives triangles 0-1-2 and 2-3-0),
// so the vertex cache shades each corner once instead of six vertices per face.
@vertex
fn vs_cloud_indexed(@builtin(vertex_index) v: u32) -> V {
    return cloud_vertex(cloud_word_corner(cloud_face_words[v / 4u], v & 3u));
}

@vertex
fn vs_cloud_cell(@builtin(vertex_index) v: u32, @location(0) ik: vec2<i32>, @location(1) codes: u32) -> V {
    let p = cloud_cell(ik.x, ik.y, codes, v);
    if p.w < 0.0 {
        // Empty slot: three coincident vertices outside the view volume, a triangle with no area.
        var o: V;
        o.pos = vec4<f32>(2.0, 2.0, 2.0, 1.0);
        return o;
    }
    return cloud_vertex(p);
}

fn ramp(d: f32, s: f32, e: f32) -> f32 {
    if d <= s { return 0.0; }
    if d >= e { return 1.0; }
    return (d - s) / (e - s);
}

@fragment
fn fs_cloud(i: V) -> @location(0) vec4<f32> {
    return vec4<f32>(i.color.rgb, i.color.a * (1.0 - ramp(i.dist, 0.0, u.extra.w)));
}
