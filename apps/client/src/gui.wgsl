// 2D interface (docs/specs/hud.md, text.md § 6): sprites of the interface atlas, glyphs
// (256 × 256 pages) and filled rectangles, in GUI coordinates (origin at the top left).

struct Screen {
    // Interface size in GUI pixels.
    size: vec4<f32>,
    // Rectangles (x, y, width, height, atlas fractions) of the End sky and the portal.
    sky: vec4<f32>,
    portal: vec4<f32>,
    // Rectangle (atlas fractions) of the smoothed sprite (loading logo).
    smooth_rect: vec4<f32>,
};

// End portal (loading background): same colors and same layers as the game's shader.
const END_COLORS = array<vec3<f32>, 16>(
    vec3<f32>(0.022087, 0.098399, 0.110818),
    vec3<f32>(0.011892, 0.095924, 0.089485),
    vec3<f32>(0.027636, 0.101689, 0.100326),
    vec3<f32>(0.046564, 0.109883, 0.114838),
    vec3<f32>(0.064901, 0.117696, 0.097189),
    vec3<f32>(0.063761, 0.086895, 0.123646),
    vec3<f32>(0.084817, 0.111994, 0.166380),
    vec3<f32>(0.097489, 0.154120, 0.091064),
    vec3<f32>(0.106152, 0.131144, 0.195191),
    vec3<f32>(0.097721, 0.110188, 0.187229),
    vec3<f32>(0.133516, 0.138278, 0.148582),
    vec3<f32>(0.070006, 0.243332, 0.235792),
    vec3<f32>(0.196766, 0.142899, 0.214696),
    vec3<f32>(0.047281, 0.315338, 0.321970),
    vec3<f32>(0.204675, 0.390010, 0.302066),
    vec3<f32>(0.080955, 0.314821, 0.661491),
);

// Layer `layer` (game time zero during loading: no vertical scrolling).
fn end_layer(layer: f32) -> mat4x4<f32> {
    let translate = mat4x4<f32>(
        1.0, 0.0, 0.0, 17.0 / layer,
        0.0, 1.0, 0.0, 0.0,
        0.0, 0.0, 1.0, 0.0,
        0.0, 0.0, 0.0, 1.0,
    );
    let r = radians((layer * layer * 4321.0 + layer * 9.0) * 2.0);
    let s = (4.5 - layer / 4.0) * 2.0;
    let rot = mat2x2<f32>(cos(r), -sin(r), sin(r), cos(r)) * mat2x2<f32>(s, 0.0, 0.0, s);
    let m = mat4x4<f32>(
        vec4<f32>(rot[0], 0.0, 0.0),
        vec4<f32>(rot[1], 0.0, 0.0),
        vec4<f32>(0.0, 0.0, 1.0, 0.0),
        vec4<f32>(0.0, 0.0, 0.0, 1.0),
    );
    let st = mat4x4<f32>(
        0.5, 0.0, 0.0, 0.25,
        0.0, 0.5, 0.0, 0.25,
        0.0, 0.0, 1.0, 0.0,
        0.0, 0.0, 0.0, 1.0,
    );
    return m * translate * st;
}

@group(0) @binding(0) var<uniform> screen: Screen;
@group(0) @binding(1) var sprites: texture_2d<f32>;
@group(0) @binding(2) var glyphs: texture_2d_array<f32>;
@group(0) @binding(3) var nearest: sampler;
@group(0) @binding(4) var vignette: texture_2d<f32>;
@group(0) @binding(5) var linear: sampler;
@group(1) @binding(0) var items: texture_2d<f32>;

struct V {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    // 0 sprite, 1 glyph, 2 filled, 7 repeated sprite; glyph layer.
    @location(2) @interpolate(flat) mode: vec2<u32>,
};

fn place(p: vec2<f32>, uv: vec2<f32>, color: vec4<f32>, mode: vec2<u32>) -> V {
    var o: V;
    o.pos = vec4<f32>(p.x / screen.size.x * 2.0 - 1.0, 1.0 - p.y / screen.size.y * 2.0, 0.0, 1.0);
    o.uv = uv;
    o.color = color;
    o.mode = mode;
    return o;
}

@vertex
fn vs(@location(0) p: vec2<f32>, @location(1) uv: vec2<f32>, @location(2) color: vec4<f32>, @location(3) mode: vec2<u32>) -> V {
    return place(p, uv, color, mode);
}

// One instance per quad: corners (top-left, bottom-left, bottom-right, top-right), uv
// rectangle, top and bottom color; vertices 0..6 = corners 0, 1, 2, 2, 3, 0 (same triangles and same
// provoking vertex as 6 vertices placed on the processor).
@vertex
fn vs_inst(
    @builtin(vertex_index) vi: u32,
    @location(0) p01: vec4<f32>,
    @location(1) p23: vec4<f32>,
    @location(2) uvr: vec4<f32>,
    @location(3) top: vec4<f32>,
    @location(4) bottom: vec4<f32>,
    @location(5) mode: vec2<u32>,
) -> V {
    var c = vi;
    if vi == 3u {
        c = 2u;
    } else if vi == 4u {
        c = 3u;
    } else if vi == 5u {
        c = 0u;
    }
    if c == 0u {
        return place(p01.xy, uvr.xy, top, mode);
    }
    if c == 1u {
        return place(p01.zw, uvr.xw, bottom, mode);
    }
    if c == 2u {
        return place(p23.xy, uvr.zw, bottom, mode);
    }
    return place(p23.zw, uvr.zy, top, mode);
}

fn end_sample(rect: vec4<f32>, p: vec4<f32>) -> vec3<f32> {
    let uv = fract(vec2<f32>(p.x / p.w, 1.0 - p.y / p.w));
    return textureSampleLevel(sprites, nearest, rect.xy + uv * rect.zw, 0.0).rgb;
}

@fragment
fn fs(i: V) -> @location(0) vec4<f32> {
    if i.mode.x == 0u {
        let c = textureSampleLevel(sprites, nearest, i.uv, 0.0) * i.color;
        if c.a == 0.0 {
            discard;
        }
        return c;
    }
    if i.mode.x == 1u {
        let c = textureSampleLevel(glyphs, nearest, i.uv, i.mode.y, 0.0) * i.color;
        if c.a < 0.1 {
            discard;
        }
        return c;
    }
    if i.mode.x == 7u {
        // Repeated sprite: uv in tiles from the pattern origin, rectangle (atlas fractions)
        // in the color; fractional part like a repeating texture. Interpolation
        // can return n - 1 ulp on a seam (fract almost 1): the sample is clamped to half
        // a texel from the sprite edge, otherwise it would read the neighboring sprite of the atlas.
        let half = 0.5 / vec2<f32>(textureDimensions(sprites));
        let t = clamp(fract(i.uv) * i.color.zw, half, i.color.zw - half);
        let c = textureSampleLevel(sprites, nearest, i.color.xy + t, 0.0);
        if c.a == 0.0 {
            discard;
        }
        return c;
    }
    if i.mode.x == 5u {
        // Coordinates projected onto the screen (origin at the bottom); texture repeated in the atlas.
        let p = vec4<f32>(i.uv.x, 1.0 - i.uv.y, 0.0, 1.0);
        var color = end_sample(screen.sky, p) * END_COLORS[0];
        for (var k = 0u; k < 15u; k++) {
            color += end_sample(screen.portal, p * end_layer(f32(k + 1u))) * END_COLORS[k];
        }
        return vec4<f32>(color, 1.0);
    }
    if i.mode.x == 6u {
        // Smoothed sprite (linear filter, edges clamped in its atlas rectangle).
        let half = 0.5 / vec2<f32>(textureDimensions(sprites));
        let uv = clamp(i.uv, screen.smooth_rect.xy + half, screen.smooth_rect.xy + screen.smooth_rect.zw - half);
        let c = textureSampleLevel(sprites, linear, uv, 0.0) * i.color;
        if c.a == 0.0 {
            discard;
        }
        return c;
    }
    if i.mode.x == 4u {
        // Object atlas slot: colors already premultiplied.
        return textureSampleLevel(items, nearest, i.uv, 0.0) * i.color;
    }
    if i.mode.x == 3u {
        // Vignette: bilinear texture × color, blend that darkens.
        return textureSampleLevel(vignette, linear, i.uv, 0.0) * i.color;
    }
    return i.color;
}
