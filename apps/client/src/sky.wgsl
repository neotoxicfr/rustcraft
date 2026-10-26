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

// Vertex `v` (0 to 5) of face `code` (direction | inner << 3 | top shade << 4) of
// cell (i, k): position relative to the mesh origin, shading factor in w.
fn cloud_corner(i: i32, k: i32, code: u32, v: u32) -> vec4<f32> {
    var corners = CLOUD_CORNERS;
    var shades = CLOUD_SHADE;
    var quad = CLOUD_QUAD;
    let dir = code & 7u;
    var q = quad[v];
    if (code & 8u) != 0u {
        q = 3u - q;
    }
    let c = corners[dir * 4u + q];
    var shade = shades[dir];
    if (code & 16u) != 0u {
        shade = shades[1];
    }
    return vec4<f32>(c.x * 12.0 + 12.0 * f32(i), c.y * 4.0, c.z * 12.0 + 12.0 * f32(k), shade);
}

// One face per instance: i (bits 0-9, signed), k (10-19), code (20-24).
fn cloud_face(w: u32, v: u32) -> vec4<f32> {
    return cloud_corner(bitcast<i32>(w << 22u) >> 22u, bitcast<i32>(w << 12u) >> 22u, (w >> 20u) & 31u, v);
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
