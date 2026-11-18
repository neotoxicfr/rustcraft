// Particles (docs/specs/particles.md § 2.4-2.5): one camera-facing quad per instance, roll
// around the view axis; texture from the particle atlas or from the block texture array
// (block pieces), read without filtering; color × lightmap, discard below 0.1 alpha, fog.

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

@group(0) @binding(0) var<uniform> cam: Camera;
@group(0) @binding(2) var blocks: texture_2d_array<f32>;
@group(0) @binding(7) var lightmap: texture_2d<f32>;
@group(0) @binding(8) var lightmap_sampler: sampler;
@group(1) @binding(0) var atlas: texture_2d<f32>;

struct Axes {
    right: vec4<f32>,
    up: vec4<f32>,
};
@group(1) @binding(1) var<uniform> axes: Axes;

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) rect: vec4<f32>,
    @location(3) @interpolate(flat) tex: u32,
    @location(4) fogd: vec2<f32>,
};

// Rotation around Y (a), then X (b), then Y (c), applied to the vector v (c first).
fn rot_yxy(a: f32, b: f32, c: f32, v: vec3<f32>) -> vec3<f32> {
    let v1 = vec3<f32>(cos(c) * v.x + sin(c) * v.z, v.y, -sin(c) * v.x + cos(c) * v.z);
    let v2 = vec3<f32>(v1.x, cos(b) * v1.y - sin(b) * v1.z, sin(b) * v1.y + cos(b) * v1.z);
    return vec3<f32>(cos(a) * v2.x + sin(a) * v2.z, v2.y, -sin(a) * v2.x + cos(a) * v2.z);
}

@vertex
fn vs(
    @builtin(vertex_index) vi: u32,
    @location(0) center: vec3<f32>,
    @location(1) size: f32,
    @location(2) uvr: vec4<f32>,
    @location(3) color: vec4<f32>,
    @location(4) light: u32,
    @location(5) roll: f32,
    @location(6) tex: u32,
) -> Out {
    // Corners in the game's order: (+1, −1) (+1, +1) (−1, +1) (−1, −1).
    let k = array<u32, 6>(0u, 1u, 2u, 0u, 2u, 3u)[vi];
    let cx = array<f32, 4>(1.0, 1.0, -1.0, -1.0)[k];
    let cy = array<f32, 4>(-1.0, 1.0, 1.0, -1.0)[k];
    let c = cos(roll);
    let s = sin(roll);
    let rx = cx * c - cy * s;
    let ry = cx * s + cy * c;
    // Orientation in bits 29 to 31 of `tex`: 0 camera-facing; 1 and 2 planes tilted at 60°
    // (shriek of the sculk shrieker, a plane and its mirror image); 3 camera-facing around the vertical axis.
    let mode = tex >> 29u;
    var right = axes.right.xyz;
    var upv = axes.up.xyz;
    if mode == 1u {
        right = vec3<f32>(1.0, 0.0, 0.0);
        upv = vec3<f32>(0.0, 0.5, -0.8660254);
    } else if mode == 2u {
        right = vec3<f32>(-1.0, 0.0, 0.0);
        upv = vec3<f32>(0.0, 0.5, -0.8660254);
    } else if mode == 3u {
        let h = normalize(vec3<f32>(center.x - cam.eye.x, 0.0, center.z - cam.eye.z));
        right = normalize(cross(h, vec3<f32>(0.0, 1.0, 0.0)));
        upv = vec3<f32>(0.0, 1.0, 0.0);
    }
    var qx = rx;
    var qy = ry;
    if mode == 4u || mode == 5u {
        // Vibration signal: heading (roll), pitch and sway on 14 bits; two planes
        // (the second flipped by a half turn and with opposite pitch), without its own roll.
        let pi = 3.14159265;
        let pitch = (f32((tex >> 14u) & 0x3fffu) / 16383.0 - 0.5) * pi + pi * 0.5;
        let sway = (f32(tex & 0x3fffu) / 16383.0 - 0.5) * 4.0;
        var yaw = roll;
        var tilt = -pitch;
        if mode == 5u {
            yaw = roll - pi;
            tilt = pitch;
        }
        right = rot_yxy(yaw, tilt, sway, vec3<f32>(1.0, 0.0, 0.0));
        upv = rot_yxy(yaw, tilt, sway, vec3<f32>(0.0, 1.0, 0.0));
        qx = cx;
        qy = cy;
    }
    let pos = center + (right * qx + upv * qy) * size;
    var o: Out;
    o.pos = cam.view_proj * vec4<f32>(pos, 1.0);
    o.uv = vec2<f32>(select(uvr.x, uvr.z, cx > 0.0), select(uvr.y, uvr.w, cy < 0.0));
    o.rect = vec4<f32>(min(uvr.xy, uvr.zw), max(uvr.xy, uvr.zw));
    o.tex = select(tex & 0x1fffffffu, 0u, mode >= 4u);
    let luv = clamp(vec2<f32>(f32(light & 0xffu), f32((light >> 8u) & 0xffu)) / 256.0 + 1.0 / 32.0, vec2<f32>(1.0 / 32.0), vec2<f32>(31.0 / 32.0));
    o.color = vec4<f32>(color.rgb * textureSampleLevel(lightmap, lightmap_sampler, luv, 0.0).rgb, color.a);
    let d = pos - cam.eye.xyz;
    o.fogd = vec2<f32>(length(d), max(length(d.xz), abs(d.y)));
    return o;
}

fn linear_fog(d: f32, s: f32, e: f32) -> f32 {
    if d <= s { return 0.0; }
    if d >= e { return 1.0; }
    return (d - s) / (e - s);
}

@fragment
fn fs(i: Out) -> @location(0) vec4<f32> {
    let lo = vec2<i32>(floor(i.rect.xy));
    let hi = max(vec2<i32>(ceil(i.rect.zw)) - 1, lo);
    let p = clamp(vec2<i32>(floor(i.uv)), lo, hi);
    var t: vec4<f32>;
    if i.tex == 0u {
        t = textureLoad(atlas, p, 0);
    } else {
        t = textureLoad(blocks, p, i32(i.tex - 1u), 0);
    }
    let c = t * i.color;
    if c.a < 0.1 {
        discard;
    }
    let v = max(linear_fog(i.fogd.x, cam.fog.x, cam.fog.y), linear_fog(i.fogd.y, cam.fog.z, cam.fog.w));
    return vec4<f32>(mix(c.rgb, cam.fog_color.rgb, v * cam.fog_color.a), c.a);
}
