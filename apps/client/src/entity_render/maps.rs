//! Drawing of a map (`docs/specs/maps.md` § 6) in the entity engine: pixels, then
//! decorations, in a 0..128 frame (y towards the bottom of the map); background of the map in hand
//! (`hand-camera.md` § 2.9).

use super::*;

/// Neutral overlay (neither red nor white).
const NEUTRAL: u32 = 10 << 16;

/// Unit square (0,0)-(1,1) with +Y normal, UV = position, in model pixels (the shader divides
/// by 16).
pub(super) fn unit_quad() -> Vec<StaticVertex> {
    let v =
        |x: f32, y: f32| StaticVertex { pos: [x * 16.0, y * 16.0, 0.0], uv: [x, y], normal: [0.0, 1.0, 0.0], part: 0 };
    let q = [v(0.0, 0.0), v(1.0, 0.0), v(1.0, 1.0), v(0.0, 1.0)];
    [0, 1, 2, 0, 2, 3].iter().map(|&i| q[i]).collect()
}

fn t(x: f32, y: f32, z: f32) -> Mat4 {
    Mat4::from_translation(Vec3::new(x, y, z))
}

fn s(x: f32, y: f32, z: f32) -> Mat4 {
    Mat4::from_scale(Vec3::new(x, y, z))
}

/// Frame of a decoration's name before centring: point (x, y + 4) of the map, 0.025 in front.
pub(super) fn map_label_anchor(m: Mat4, px: f32, py: f32) -> Mat4 {
    m * t(px, py + 4.0, -0.025)
}

/// Final frame of the name (font pixels, y downwards): centring `width × scale / 2`, scale
/// `clamp(25 / width, 0, 6/9)` (reversed depth), then 0.1 forward.
pub(crate) fn map_label_matrix(anchor: Mat4, width: f32) -> Mat4 {
    let scale = (25.0 / width).clamp(0.0, 6.0 / 9.0);
    anchor * t(-width * scale / 2.0, 0.0, 0.0) * s(scale, scale, -1.0) * t(0.0, 0.0, 0.1)
}

impl EntityRenderer {
    /// Name `text` of a map decoration at frame `anchor`: white, no shadow, half-transparent
    /// black background, drawn by the text renderers (hand camera if `hand`).
    fn push_label(&mut self, text: rc_proto::nbt::Nbt, anchor: Mat4, light: u32, hand: bool) {
        let label = self.objects.labels.len() as u32;
        self.objects.labels.push(text);
        self.objects.texts.push(TextDraw {
            id: -1,
            m: anchor,
            light,
            opacity: 255,
            background: 0x8000_0000,
            flags: 0,
            label,
            hand,
        });
    }

    /// Flat quad without directional shading ("text" render type): the part's normal is
    /// replaced by the first light of the world, which brings the diffusion to 1 (front face).
    fn push_flat(&mut self, m: Mat4, rect: [f32; 4], light: u32, back: bool) {
        let l = world_lights(false)[0];
        let k = if back { -1.0 } else { 1.0 };
        let base = self.mats.len() as u32;
        let mut pm = PartMat::new(&m);
        pm.n = [[0.0; 4], [l[0] * k, l[1] * k, l[2] * k, 0.0], [0.0; 4]];
        self.mats.push(pm);
        let first = self.map_tex.quad_first;
        self.instances[0].push((1, first, 6, Instance { base, rect, color: 0xffff_ffff, light, overlay: NEUTRAL }));
    }

    /// Id of the map carried by `stack`, if its data is known to the client.
    pub(super) fn known_map(&self, stack: &rc_proto_v777::item::ItemStack) -> Option<i32> {
        let id = crate::maps::map_id(&self.ids, stack)?;
        crate::maps::store().get(id).map(|_| id)
    }

    /// Contents of map `id` (§ 6): `m` = 0..128 frame; `hand`: map in hand (all
    /// decorations, names drawn with the hand camera), otherwise in a frame (only the
    /// decorations visible in a frame). `back`: visible face of reverse direction.
    pub(super) fn push_map_contents(&mut self, m: Mat4, id: i32, light: u32, hand: bool, back: bool) -> bool {
        let mut store = crate::maps::store();
        let Some(rect) = self.map_tex.texture(&mut self.atlas, &mut store, id) else { return false };
        self.push_flat(m * t(0.0, 0.0, -0.01) * s(128.0, 128.0, 1.0), rect, light, back);
        let Some(data) = store.get(id) else { return true };
        let decos: Vec<_> = data.decorations.iter().map(|d| (d.kind, d.x, d.y, d.rot, d.name.clone())).collect();
        drop(store);
        let mut n = 0;
        for (kind, x, y, rot, name) in decos {
            // Type outside the registry: ignored.
            let Some(&(sprite, in_frame)) = usize::try_from(kind).ok().and_then(|k| crate::maps::DECORATIONS.get(k))
            else {
                continue;
            };
            if !hand && !in_frame {
                continue;
            }
            let (px, py) = (f32::from(x) / 2.0 + 64.0, f32::from(y) / 2.0 + 64.0);
            // Sprite missing: nothing in its place, but the name and the depth rank remain.
            if let Some(r) = self.atlas.rect(&format!("map/decorations/{sprite}")) {
                let d = m
                    * t(px, py, -0.02)
                    * Mat4::from_rotation_z((f32::from(rot) * 22.5).to_radians())
                    * s(4.0, 4.0, 3.0)
                    * t(-0.125, 0.125, 0.0)
                    * t(-1.0, 1.0, -0.001 * n as f32)
                    * s(2.0, -2.0, 1.0);
                self.push_flat(d, r, light, !back);
            }
            // Name under the marker: centred and scaled at text drawing (font width),
            // 4 map pixels lower, in front of the map.
            if let Some(name) = name {
                self.push_label(name, map_label_anchor(m, px, py), light, hand);
            }
            n += 1;
        }
        true
    }

    /// Map in hand (`hand-camera.md` § 2.9): `m` = 0..128 frame after C(…); checkered background
    /// if the data is known, otherwise plain background; then the contents, all decorations.
    pub fn push_hand_map(&mut self, stack: &rc_proto_v777::item::ItemStack, m: Mat4, light: u32) {
        let id = self.known_map(stack);
        let bg = if id.is_some() { "map/map_background_checkerboard" } else { "map/map_background" };
        if let Some(r) = self.atlas.rect(bg) {
            self.push_flat(m * t(-7.0, -7.0, 0.0) * s(142.0, 142.0, 1.0), r, light, true);
        }
        if let Some(id) = id {
            self.push_map_contents(m, id, light, true, true);
        }
    }

    /// Names of the map decorations placed in this frame (`TextDraw::label` indices).
    pub fn text_labels(&self) -> &[rc_proto::nbt::Nbt] {
        &self.objects.labels
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decoration name: scale 25 / width bounded to 6/9, centred on the marker, 4 pixels
    /// below it, at 0.025 then 0.1 (reversed depth) in front of the map.
    #[test]
    fn label_matrix_follows_the_game() {
        let m = Mat4::from_translation(Vec3::new(3.0, 2.0, 1.0));
        let a = map_label_anchor(m, 70.0, 50.0);
        // Short name (width 20): scale capped at 6/9.
        let k = 6.0 / 9.0;
        let want = m
            * Mat4::from_translation(Vec3::new(70.0 - 20.0 * k / 2.0, 54.0, -0.025))
            * Mat4::from_scale(Vec3::new(k, k, -1.0))
            * Mat4::from_translation(Vec3::new(0.0, 0.0, 0.1));
        assert!(map_label_matrix(a, 20.0).abs_diff_eq(want, 1e-5));
        // Long name (width 100): scale 0.25, centre always at the marker.
        let got = map_label_matrix(a, 100.0);
        let mid = got.transform_point3(Vec3::new(50.0, 0.0, 0.0));
        assert!((mid - Vec3::new(73.0, 56.0, 1.0 - 0.025 - 0.1)).length() < 1e-4, "{mid:?}");
        assert!((got.transform_vector3(Vec3::X).x - 0.25).abs() < 1e-6);
    }
}
