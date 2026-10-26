// Debug lines (docs/specs/debug-f3.md § 8).
// Segments: one per instance, widened into a quadrilateral of constant thickness in pixels, own
// color, clipped at the near plane, world fog. Zero-length segment = point: square
// of `width` pixels. Faces: one solid quadrilateral per instance, no fog.

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

struct Style {
    // Screen size (px).
    screen: vec4<f32>,
};

@group(0) @binding(0) var<uniform> cam: Camera;
@group(1) @binding(0) var<uniform> style: Style;

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) fogd: vec2<f32>,
};

// Layer offset (1 − 1/4096) then line shrink (1 − 1/256), towards the camera:
// a line lying on a face is not hidden by it.
const LINE_SHRINK: f32 = (1.0 - 1.0 / 256.0) * (1.0 - 1.0 / 4096.0);
const FACE_SHRINK: f32 = 1.0 - 1.0 / 4096.0;

fn fog_dist(p: vec3<f32>) -> vec2<f32> {
    let d = p - cam.eye.xyz;
    return vec2<f32>(length(d), max(length(d.xz), abs(d.y)));
}

@vertex
fn vs(
    @builtin(vertex_index) vi: u32,
    @location(0) a: vec3<f32>,
    @location(1) b: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) width: f32,
) -> Out {
    let k = array<u32, 6>(0u, 1u, 2u, 2u, 1u, 3u)[vi];
    let e = cam.eye.xyz;
    var o: Out;
    o.color = color;
    let size = style.screen.xy;
    if all(a == b) {
        // Point: square of `width` pixels, without layer offset.
        let c = cam.view_proj * vec4<f32>(a, 1.0);
        let sx = select(-1.0, 1.0, k % 2u == 1u);
        let sy = select(-1.0, 1.0, k >= 2u);
        let off = vec2<f32>(sx, sy) * width / size;
        o.pos = vec4<f32>(c.xy + off * c.w, c.z, c.w);
        o.fogd = fog_dist(a);
        return o;
    }
    var ca = cam.view_proj * vec4<f32>(e + (a - e) * LINE_SHRINK, 1.0);
    var cb = cam.view_proj * vec4<f32>(e + (b - e) * LINE_SHRINK, 1.0);
    o.fogd = fog_dist(select(a, b, k >= 2u));
    // Reversed depth [0, 1]: near plane z = w (da, db < 0 behind it).
    let da = ca.w - ca.z;
    let db = cb.w - cb.z;
    if da < 0.0 && db < 0.0 {
        o.pos = vec4<f32>(2.0, 2.0, 2.0, 1.0);
        return o;
    }
    if da < 0.0 {
        ca = mix(ca, cb, da / (da - db) + 1e-5);
    } else if db < 0.0 {
        cb = mix(cb, ca, db / (db - da) + 1e-5);
    }
    let c0 = select(ca, cb, k >= 2u);
    let n0 = ca.xy / ca.w;
    let n1 = cb.xy / cb.w;
    var d = (n1 - n0) * size;
    if dot(d, d) < 1e-12 {
        d = vec2<f32>(1.0, 0.0);
    }
    let sd = normalize(d);
    var off = vec2<f32>(-sd.y, sd.x) * width / size;
    if k % 2u == 1u {
        off = -off;
    }
    o.pos = vec4<f32>(c0.xy + off * c0.w, c0.z, c0.w);
    return o;
}

fn linear_fog(d: f32, s: f32, e: f32) -> f32 {
    if d <= s { return 0.0; }
    if d >= e { return 1.0; }
    return (d - s) / (e - s);
}

@fragment
fn fs(i: Out) -> @location(0) vec4<f32> {
    let v = max(linear_fog(i.fogd.x, cam.fog.x, cam.fog.y), linear_fog(i.fogd.y, cam.fog.z, cam.fog.w));
    return vec4<f32>(mix(i.color.rgb, cam.fog_color.rgb, v * cam.fog_color.a), i.color.a);
}

struct FaceOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_face(
    @builtin(vertex_index) vi: u32,
    @location(0) p0: vec3<f32>,
    @location(1) p1: vec3<f32>,
    @location(2) p2: vec3<f32>,
    @location(3) p3: vec3<f32>,
    @location(4) color: vec4<f32>,
) -> FaceOut {
    let k = array<u32, 6>(0u, 1u, 2u, 0u, 2u, 3u)[vi];
    var p = p0;
    if k == 1u {
        p = p1;
    } else if k == 2u {
        p = p2;
    } else if k == 3u {
        p = p3;
    }
    let e = cam.eye.xyz;
    var o: FaceOut;
    o.pos = cam.view_proj * vec4<f32>(e + (p - e) * FACE_SHRINK, 1.0);
    o.color = color;
    return o;
}

@fragment
fn fs_face(i: FaceOut) -> @location(0) vec4<f32> {
    if i.color.a == 0.0 {
        discard;
    }
    return i.color;
}
