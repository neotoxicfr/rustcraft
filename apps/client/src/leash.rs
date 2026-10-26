//! Leashes (`docs/specs/miscellaneous.md` § 1): rope ends (one, or four between
//! a happy ghast and a mount), geometry in two crossed ribbons of 24 steps, and draw pass
//! (color × lightmap, fog, opaque, no back-face culling).


use crate::entities::{Entity, rot_lerp};

/// Rope vertex: position relative to the integer origin of the camera (x, z), absolute y;
/// RGBA color; light (block × 16, sky × 16 << 8).
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pos: [f32; 3],
    color: u32,
    light: u32,
}

/// A rope: start, end (world), sag.
pub struct Rope {
    pub start: [f64; 3],
    pub end: [f64; 3],
    pub sag: bool,
}

/// What the end-point calculation reads from the renderer.
pub struct Ctx<'a> {
    pub pt: f32,
    /// Dimensions per type (width, height, eyes, label).
    pub table: &'a [[f32; 4]],
    /// Id of the local player and its main arm (option `mainHand`).
    pub local: i32,
    pub local_left: bool,
    /// Subjective view (camera option): the local player's rope starts in front of its eyes.
    pub first_person: bool,
}

/// Current width, height, eye height.
pub fn size(e: &Entity, table: &[[f32; 4]]) -> (f64, f64, f64) {
    let s = e.size(table);
    (f64::from(s.w), f64::from(s.h), f64::from(s.eye))
}

/// Rotates `v` by the yaw `deg` (degrees): game convention, (0, 0, 1) = forward.
fn yaw_rot(v: [f64; 3], deg: f64) -> [f64; 3] {
    let (s, c) = (-deg.to_radians()).sin_cos();
    [v[0] * c + v[2] * s, v[1], v[2] * c - v[0] * s]
}

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn is_boat(k: &str) -> bool {
    k.ends_with("_boat") || k.ends_with("_raft")
}

/// Attachment square (a, b, c, h) of a held entity that accepts leashes in a square.
fn square_params(k: &str) -> Option<[f64; 4]> {
    Some(match k {
        "horse" | "skeleton_horse" => [0.04, 0.52, 0.23, 0.87],
        "donkey" | "mule" | "zombie_horse" => [0.04, 0.41, 0.18, 0.73],
        "camel" | "camel_husk" => [0.02, 0.48, 0.25, 0.82],
        "sniffer" => [-0.01, 0.63, 0.38, 1.15],
        _ if is_boat(k) => [0.0, 0.64, 0.382, 0.88],
        _ => return None,
    })
}

/// Points of the square, in the game's order.
fn square(p: [f64; 4], w: f64, h: f64) -> [[f64; 3]; 4] {
    let (a, b, c, y) = (p[0] * w, p[1] * w, p[2] * w, p[3] * h);
    [[-c, y, b + a], [-c, y, -b + a], [c, y, -b + a], [c, y, b + a]]
}

/// Attachment offset `D` of a single rope (§ 1.4).
fn attach(e: &Entity, w: f64, h: f64, eye: f64) -> [f64; 3] {
    let k = e.kind.as_str();
    let (fy, fz) = match k {
        "allay" => (0.6, 0.1),
        "bee" => (0.5, 0.2),
        "ocelot" | "parrot" => (0.5, 0.4),
        "fox" => (0.55, 0.4),
        "pig" | "rabbit" | "wolf" | "strider" => (0.6, 0.4),
        "snow_golem" => (0.75, 0.4),
        "iron_golem" => (0.875, 0.4),
        "copper_golem" => (0.75, 0.0),
        "llama" | "trader_llama" => (0.75, 0.5),
        // Camel rider anchor: approximated by 0.79 × H (not animated here).
        "camel" | "camel_husk" => return [0.0, h * 0.79 - 0.2 * if e.baby() { 0.5 } else { 1.0 }, 0.56 * w],
        "happy_ghast" => return [0.0; 3],
        "sulfur_cube" => return [0.0, h / 2.0, 0.0],
        _ if is_boat(k) => return [0.0, 0.88 * h, 0.64 * w],
        _ => (1.0, 0.4),
    };
    [0.0, fy * eye, fz * w]
}

/// Left main arm of a player (metadata 15: 0 left, 1 right).
fn left_handed(e: &Entity) -> bool {
    // Main arm = humanoid arm serializer (Meta::Variant); 0 = left.
    e.meta_int(15) == Some(0)
}

/// Holder's grab point (§ 1.5).
fn grip(holder: &Entity, ctx: &Ctx<'_>, id: i32) -> [f64; 3] {
    let pt = ctx.pt;
    let p = holder.render_pos(pt);
    let (_, h, eye) = size(holder, ctx.table);
    let body = f64::from(rot_lerp(pt, holder.body_o, holder.body));
    match holder.kind.as_str() {
        // Node position recomputed by the client: block center, bottom + 0.375.
        "leash_knot" => [holder.pos[0].floor() + 0.5, holder.pos[1].floor() + 0.375 + 0.2, holder.pos[2].floor() + 0.5],
        "villager" | "wandering_trader" => add(p, yaw_rot([0.0, h - 1.0, 0.2], body)),
        // Local player in subjective view: fixed point in front of the eyes, on the side of the main arm
        // (0.39 aside, 0.6 lower, 0.3 forward), rotated by the pitch then the yaw. Both
        // angles are read like the game: from the current to the old one, at half fraction.
        "player" if id == ctx.local && ctx.first_person => {
            let half = |cur: f32, old: f32| f64::from(cur + pt * 0.5 * (old - cur));
            let side = if ctx.local_left { 1.0 } else { -1.0 };
            let o = pitch_rot([0.39 * side, -0.6, 0.3], -half(holder.pitch, holder.pitch_o).to_radians());
            add([p[0], p[1] + eye, p[2]], yaw_rot(o, half(holder.yaw, holder.yaw_o)))
        }
        "player" | "mannequin" => {
            let left = if id == ctx.local { ctx.local_left } else { left_handed(holder) };
            let x0 = if left { 0.22 } else { -0.22 };
            // Pitch read as the game does (inverted bounds, half fraction).
            let pitch = f64::from(holder.pitch + pt * 0.5 * (holder.pitch_o - holder.pitch)).to_radians();
            let pose = holder.pose();
            if pose == 1 || pose == 4 {
                // Gliding or spin attack: roll, pitch, yaw.
                let v = holder.velocity;
                let yaw = f64::from(holder.yaw).to_radians();
                let r = [-yaw.sin(), yaw.cos()];
                let (lv, lr) = (v[0].hypot(v[2]), 1.0);
                let roll = if lv > 0.0 {
                    let cos = ((v[0] * r[0] + v[2] * r[1]) / (lv * lr)).clamp(-1.0, 1.0);
                    cos.acos() * (v[0] * r[1] - v[2] * r[0]).signum()
                } else {
                    0.0
                };
                let (sr, cr) = roll.sin_cos();
                let o = [x0, -0.11, 0.85];
                let o = [o[0] * cr - o[1] * sr, o[0] * sr + o[1] * cr, o[2]];
                let o = pitch_rot(o, pitch);
                add(p, yaw_rot(o, body))
            } else if pose == 3 {
                add(p, yaw_rot(pitch_rot([x0, 0.2, -0.15], pitch), body))
            } else {
                let z = if pose == 5 { -0.2 } else { 0.07 };
                add(p, yaw_rot([x0, h - 1.0, z], body))
            }
        }
        _ => [p[0], p[1] + 0.7 * eye, p[2]],
    }
}

/// Rotation around X (pitch, radians, positive = downward).
fn pitch_rot(v: [f64; 3], a: f64) -> [f64; 3] {
    let (s, c) = a.sin_cos();
    [v[0], v[1] * c + v[2] * s, v[2] * c - v[1] * s]
}

/// Ropes between the held entity and its holder.
pub fn ropes(held: &Entity, holder: &Entity, holder_id: i32, ctx: &Ctx<'_>) -> Vec<Rope> {
    let pt = ctx.pt;
    let p = held.render_pos(pt);
    let (w, h, eye) = size(held, ctx.table);
    let body = f64::from(rot_lerp(pt, held.body_o, held.body));
    if holder.kind == "happy_ghast"
        && let Some(sq) = square_params(&held.kind)
    {
        let (hw, hh, _) = size(holder, ctx.table);
        let hp = holder.render_pos(pt);
        let hbody = f64::from(rot_lerp(pt, holder.body_o, holder.body));
        let from = square(sq, w, h);
        let to = square([-0.03125, 0.4375, 0.46875, 0.03125], hw, hh);
        return (0..4)
            .map(|i| Rope { start: add(p, yaw_rot(from[i], body)), end: add(hp, yaw_rot(to[i], hbody)), sag: false })
            .collect();
    }
    let start = add(p, yaw_rot(attach(held, w, h, eye), body));
    vec![Rope { start, end: grip(holder, ctx, holder_id), sag: true }]
}

/// Step of a rope.
const STEPS: usize = 24;

/// Vertices of a rope (triangles), lights (block, sky) at both ends.
pub fn geometry(rope: &Rope, light: [(u8, u8); 2], origin: [i32; 2], out: &mut Vec<Vertex>) {
    let d = [rope.end[0] - rope.start[0], rope.end[1] - rope.start[1], rope.end[2] - rope.start[2]];
    // Exactly vertical rope (zero horizontal difference in single precision): the game divides by
    // zero, its vertices are NaN and the rope does not appear (quirk reproduced).
    let (fx, fz) = (d[0] as f32, d[2] as f32);
    if fx * fx + fz * fz == 0.0 {
        return;
    }
    let len = d[0].hypot(d[2]);
    let (ox, oz) = (d[2] * 0.025 / len, d[0] * 0.025 / len);
    let base = [rope.start[0] - f64::from(origin[0]), rope.start[1], rope.start[2] - f64::from(origin[1])];
    let lerp = |p: f64, a: u8, b: u8| (f64::from(a) + p * (f64::from(b) - f64::from(a))) as u32;
    let step = |k: usize| {
        let p = k as f64 / STEPS as f64;
        let y = if !rope.sag {
            d[1] * p
        } else if d[1] > 0.0 {
            d[1] * p * p
        } else {
            d[1] - d[1] * (1.0 - p) * (1.0 - p)
        };
        let light = (lerp(p, light[0].0, light[1].0) * 16) | ((lerp(p, light[0].1, light[1].1) * 16) << 8);
        ([base[0] + d[0] * p, base[1] + y, base[2] + d[2] * p], light)
    };
    let color = |dark: bool| {
        let f = if dark { 0.7 } else { 1.0 };
        let c = |v: f64| ((v * f * 255.0) as u32).min(255);
        c(0.5) | c(0.4) << 8 | c(0.3) << 16 | 0xff << 24
    };
    let v = |p: [f64; 3], light: u32, color: u32| Vertex { pos: [p[0] as f32, p[1] as f32, p[2] as f32], color, light };
    // Ribbon A: (x − ox, y + 0.05, z + oz), (x + ox, y, z − oz), dark at even steps;
    // ribbon B: (x − ox, y, z + oz), (x + ox, y + 0.05, z − oz), dark at odd steps.
    for ribbon in 0..2 {
        let pair = |k: usize| {
            let (c, l) = step(k);
            let col = color(k.is_multiple_of(2) == (ribbon == 0));
            let (ya, yb) = if ribbon == 0 { (0.05, 0.0) } else { (0.0, 0.05) };
            (v([c[0] - ox, c[1] + ya, c[2] + oz], l, col), v([c[0] + ox, c[1] + yb, c[2] - oz], l, col))
        };
        let mut prev = pair(0);
        for k in 1..=STEPS {
            let cur = pair(k);
            out.extend_from_slice(&[prev.0, prev.1, cur.0, prev.1, cur.1, cur.0]);
            prev = cur;
        }
    }
}

/// Draw pass of the frame's ropes.
pub struct LeashRenderer {
    pipeline: wgpu::RenderPipeline,
    buf: Option<(wgpu::Buffer, u64)>,
    pub verts: Vec<Vertex>,
    count: u32,
}

impl LeashRenderer {
    pub fn new(device: &wgpu::Device, camera_layout: &wgpu::BindGroupLayout, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("leashes"),
            source: wgpu::ShaderSource::Wgsl(include_str!("leash.wgsl").into()),
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("leashes"),
            bind_group_layouts: &[Some(camera_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("leashes"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Unorm8x4, 2 => Uint32],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(crate::render::DEPTH_NEARER),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        LeashRenderer { pipeline, buf: None, verts: Vec::new(), count: 0 }
    }

    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.count = self.verts.len() as u32;
        if self.verts.is_empty() {
            return;
        }
        let bytes: &[u8] = bytemuck::cast_slice(&self.verts);
        if self.buf.as_ref().is_none_or(|(_, cap)| *cap < bytes.len() as u64) {
            let cap = (bytes.len() as u64).next_power_of_two().max(4096);
            self.buf = Some((
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("leashes"),
                    size: cap,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                cap,
            ));
        }
        queue.write_buffer(&self.buf.as_ref().expect("created above").0, 0, bytes);
    }

    /// Drawing (group 0 = world camera already bound).
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        let Some((buf, _)) = &self.buf else { return };
        if self.count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, buf.slice(..));
        pass.draw(0..self.count, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sagging_rope_has_low_point_at_lower_end() {
        let rope = Rope { start: [0.0, 2.0, 0.0], end: [4.0, 0.0, 0.0], sag: true };
        let mut v = Vec::new();
        geometry(&rope, [(15, 15), (0, 15)], [0, 0], &mut v);
        assert_eq!(v.len(), 2 * STEPS * 6);
        // Lower arrival: y = dy − dy·(1 − p)²; halfway −2 + 2·0.25 = −1.5 → 0.5 absolute.
        let mid = v.iter().filter(|x| (x.pos[0] - 2.0).abs() < 1e-4).map(|x| x.pos[1]).fold(f32::MAX, f32::min);
        assert!((mid - 0.5).abs() < 1e-4, "{mid}");
        // Interpolated and truncated block light: 15 at the start, 7 halfway.
        assert!(v.iter().any(|x| x.light & 0xff == 15 * 16));
        assert!(v.iter().any(|x| (x.pos[0] - 2.0).abs() < 1e-4 && x.light & 0xff == 7 * 16));
    }

    /// Exactly vertical rope: nothing is drawn (the game divides by zero); a tiny horizontal
    /// difference is enough to draw it.
    #[test]
    fn vertical_rope_is_not_drawn() {
        let mut v = Vec::new();
        geometry(&Rope { start: [3.5, 70.0, -2.5], end: [3.5, 64.0, -2.5], sag: true }, [(15, 15); 2], [0, 0], &mut v);
        assert!(v.is_empty());
        geometry(&Rope { start: [3.5, 70.0, -2.5], end: [3.5, 64.0, -2.49], sag: true }, [(15, 15); 2], [0, 0], &mut v);
        assert_eq!(v.len(), 2 * STEPS * 6);
    }

    /// Local player in subjective view: rope held in front of the eyes, on the main arm's side,
    /// angles read from the current to the old one at half fraction; in third person, the
    /// model's hand.
    #[test]
    fn first_person_grip() {
        let mut me = crate::debug_render::tests::test_entity("player");
        me.pos = [10.0, 64.0, -3.0];
        me.pos_o = me.pos;
        me.yaw = 90.0;
        me.yaw_o = 0.0;
        let table = [[0.6, 1.8, 1.62, 0.0]];
        let eye = size(&me, &table).2;
        let mut ctx = Ctx { pt: 1.0, table: &table, local: 7, local_left: false, first_person: true };
        // Seen yaw: 90 + 0.5 × (0 − 90) = 45°; offset (−0.39; −0.6; 0.3) rotated by 45°.
        let g = grip(&me, &ctx, 7);
        let (s, c) = (-45f64).to_radians().sin_cos();
        let want = [10.0 + -0.39 * c + 0.3 * s, 64.0 + eye - 0.6, -3.0 + 0.3 * c + 0.39 * s];
        for i in 0..3 {
            assert!((g[i] - want[i]).abs() < 1e-9, "{g:?} against {want:?}");
        }
        // Left-handed: opposite side.
        ctx.local_left = true;
        let l = grip(&me, &ctx, 7);
        assert!((l[0] - (10.0 + 0.39 * c + 0.3 * s)).abs() < 1e-9 && (l[2] - (-3.0 + 0.3 * c - 0.39 * s)).abs() < 1e-9);
        // Third person, or another player: model's hand, below the eyes.
        ctx.first_person = false;
        assert_ne!(grip(&me, &ctx, 7), l);
        ctx.first_person = true;
        assert_ne!(grip(&me, &ctx, 8), l);
    }

    #[test]
    fn yaw_rotation_matches_game_convention() {
        // Yaw 90°: forward (0, 0, 1) points toward −x.
        let r = yaw_rot([0.0, 0.0, 1.0], 90.0);
        assert!((r[0] + 1.0).abs() < 1e-9 && r[2].abs() < 1e-9);
    }
}
