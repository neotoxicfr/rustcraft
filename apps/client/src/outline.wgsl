// Outline of the targeted block (docs/specs/targeting.md § 4): one segment per instance, widened into a
// quadrilateral of constant thickness in pixels; view shrunk by 1/256 towards the camera.

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
    color: vec4<f32>,
    // Screen size (px), thickness (px).
    screen: vec4<f32>,
};

@group(0) @binding(0) var<uniform> cam: Camera;
@group(1) @binding(0) var<uniform> style: Style;

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) fogd: vec2<f32>,
};

// Layer offset of the game: view shrunk by 1/4096 towards the camera (reversed depth).
const VIEW_SHRINK: f32 = 1.0 - 1.0 / 4096.0;

fn project(p: vec3<f32>) -> vec4<f32> {
    let e = cam.eye.xyz;
    return cam.view_proj * vec4<f32>(e + (p - e) * VIEW_SHRINK, 1.0);
}

@vertex
fn vs(@builtin(vertex_index) vi: u32, @location(0) a: vec3<f32>, @location(1) b: vec3<f32>) -> Out {
    // Quadrilateral (a+, a−, b+, b−) as two triangles.
    let k = array<u32, 6>(0u, 1u, 2u, 2u, 1u, 3u)[vi];
    let p = select(a, b, k >= 2u);
    let dir = normalize(b - a);
    let c0 = project(p);
    let c1 = project(p + dir);
    let n0 = c0.xyz / c0.w;
    let n1 = c1.xyz / c1.w;
    let size = style.screen.xy;
    let sd = normalize((n1.xy - n0.xy) * size);
    var off = vec2<f32>(-sd.y, sd.x) * style.screen.z / size;
    if off.x < 0.0 {
        off = -off;
    }
    if k % 2u == 1u {
        off = -off;
    }
    var o: Out;
    o.pos = vec4<f32>((n0 + vec3<f32>(off, 0.0)) * c0.w, c0.w);
    let d = p - cam.eye.xyz;
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
    let v = max(linear_fog(i.fogd.x, cam.fog.x, cam.fog.y), linear_fog(i.fogd.y, cam.fog.z, cam.fog.w));
    return vec4<f32>(mix(style.color.rgb, cam.fog_color.rgb, v * cam.fog_color.a), style.color.a);
}
