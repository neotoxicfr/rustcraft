//! Primed TNT (§ 4.1): the TNT block swelling over its last 10 ticks and blinking white every
//! 5 ticks. A heap set off throws hundreds at once, so the per-entity work is cut down:
//!
//! - `PRIMED_TNT_SHARED_POSE`: a TNT that is not swelling has the same block pose as every other
//!   one up to its translation. The product `T(rel) · T(0, ½, 0) · S(1) · Ry(−90°) ·
//!   T(−½, −½, ½) · Ry(90°)` taken left to right leaves the rotation columns of the constant tail
//!   untouched and its translation column equal to the tail's own plus `(rel.x, rel.y + ½,
//!   rel.z)`, added last: so the pose is built once (normal matrix included) and each TNT adds
//!   its translation, the same floats up to the sign of zeros. A swelling TNT keeps the product,
//!   its two quarter turns built once.

use super::*;

/// The two quarter turns of the TNT pose, built once: a rotation by a constant angle is the
/// same matrix every frame.
fn quarter_turns() -> (Mat4, Mat4) {
    static TURNS: std::sync::OnceLock<(Mat4, Mat4)> = std::sync::OnceLock::new();
    *TURNS.get_or_init(|| (Mat4::from_rotation_y(deg(-90.0)), Mat4::from_rotation_y(deg(90.0))))
}

/// Pose of a TNT that is not swelling, for a state drawn as its block mesh alone.
struct SharedPose {
    state: u32,
    /// Part matrix of the pose's constant tail: rotation and normal matrix of every such TNT,
    /// translation column to which each one adds its own.
    tail: PartMat,
    /// Instances of the block mesh: (pass, source, first vertex, vertex count, colour).
    groups: Vec<(usize, u8, u32, u32, u32)>,
}

#[derive(Default)]
pub(in crate::entity_render) struct TntDrawState {
    shared: Option<SharedPose>,
    /// Default state of the TNT block (sent by no server unless changed).
    default_state: Option<u32>,
}

impl EntityRenderer {
    /// Primed TNT `e` at `rel` in the frame, entity light `light`.
    pub(in crate::entity_render) fn push_primed_tnt(&mut self, e: &Entity, pt: f32, rel: [f32; 3], light: u32) -> bool {
        let f = e.meta_int(8).unwrap_or(80) as f32 - pt + 1.0;
        let (g, on) = tnt_swell(f);
        let overlay = if on { WHITE } else { NEUTRAL };
        let shared = self.opt.primed_tnt_shared_pose;
        let state = match meta_state(e, 9) {
            Some(s) => s,
            None if shared => self.tnt_default_state(),
            None => self.default_state("minecraft:tnt"),
        };
        self.place_primed_tnt(state, rel, g, light, overlay)
    }

    fn tnt_default_state(&mut self) -> u32 {
        if let Some(s) = self.objects.primed_tnt.default_state {
            return s;
        }
        let s = self.default_state("minecraft:tnt");
        self.objects.primed_tnt.default_state = Some(s);
        s
    }

    /// Places the TNT block: shared pose when not swelling (`PRIMED_TNT_SHARED_POSE`), otherwise
    /// the full product.
    fn place_primed_tnt(&mut self, state: u32, rel: [f32; 3], g: f32, light: u32, overlay: u32) -> bool {
        let shared = self.opt.primed_tnt_shared_pose;
        if shared && g == 0.0 && self.ensure_shared_pose(state) {
            self.push_shared_pose(rel, self.block_as_entity_light(state, light), overlay);
            return true;
        }
        let (neg, pos) = if shared {
            quarter_turns()
        } else {
            (Mat4::from_rotation_y(deg(-90.0)), Mat4::from_rotation_y(deg(90.0)))
        };
        let m = Mat4::from_translation(Vec3::from(rel))
            * Mat4::from_translation(Vec3::new(0.0, 0.5, 0.0))
            * Mat4::from_scale(Vec3::splat(1.0 + g))
            * neg
            * Mat4::from_translation(Vec3::new(-0.5, -0.5, 0.5))
            * pos;
        self.push_block(state, m, light, overlay)
    }

    /// Builds the shared pose of `state` if it is not the kept one; false for a state that
    /// needs the full path (special model such as a chest, no mesh).
    fn ensure_shared_pose(&mut self, state: u32) -> bool {
        if self.objects.primed_tnt.shared.as_ref().is_some_and(|p| p.state == state) {
            return true;
        }
        if self.special_of(state).is_some() {
            return false;
        }
        let Some(mesh) = self.block_mesh(state) else { return false };
        let (neg, pos) = quarter_turns();
        let tail = neg * Mat4::from_translation(Vec3::new(-0.5, -0.5, 0.5)) * pos;
        let (pass, src) = self.mesh_pass(mesh);
        let groups = self.objects.meshes[mesh]
            .iter()
            .map(|&(t, first, count)| {
                (pass, src, first, count, argb_to_instance_rgba(self.block_tint(state, t, BlockTint::Entity)))
            })
            .collect();
        self.objects.primed_tnt.shared = Some(SharedPose { state, tail: PartMat::new(&tail), groups });
        true
    }

    /// The kept shared pose moved to `rel`: its translation column plus `(rel.x, rel.y + ½,
    /// rel.z)`, added in the order of the full product (module comment).
    fn push_shared_pose(&mut self, rel: [f32; 3], light: u32, overlay: u32) {
        let Some(pose) = &self.objects.primed_tnt.shared else { return };
        let base = self.mats.len() as u32;
        let mut part = pose.tail;
        let lift = [rel[0], rel[1] + 0.5, rel[2]];
        for (c, r) in part.m[3].iter_mut().zip(lift) {
            *c += r;
        }
        self.mats.push(part);
        for &(pass, src, first, count, color) in &pose.groups {
            let inst = Instance { base, rect: [0.0, 0.0, 1.0, 1.0], color, light, overlay };
            self.instances[pass].push((src, first, count, inst));
        }
    }

    /// Drops the shared pose: a proof that changes the switches of a
    /// renderer must not replay what another setting kept (proofs only).
    #[cfg(test)]
    pub(in crate::entity_render) fn forget_primed_tnt(&mut self) {
        self.objects.primed_tnt = TntDrawState::default();
    }

    /// Whether the shared pose was built (proofs only).
    #[cfg(test)]
    pub(in crate::entity_render) fn shared_tnt_pose_built(&self) -> bool {
        self.objects.primed_tnt.shared.is_some()
    }
}
