//! The one chain of part matrices. Every layer drawn from a baked model (entity layers, armor,
//! cape, ears, wings, posed layers, the first-person arm, objects and block entities) places its
//! parts here, parent before child: local matrix of the pose, frame of the parent times it, then
//! the matrix and normal matrix the shader reads.

use glam::{Affine3A, Mat4, Vec3A};

use super::{BakedPart, EntityRenderer, KNOWN, PartMat, Pose, animation_paths, part_matrix_with};

/// How the parts of a layer hang on the entity.
#[derive(Clone, Copy)]
pub(super) struct PartChain {
    /// Frame of the root parts.
    pub root: Mat4,
    /// Layer offset (towards the camera) applied to the drawn matrices only, never to the frames
    /// the children and the held items read: armor, equipment, wings.
    pub drawn_offset: Option<Mat4>,
    /// Matrix composed after the local matrix of the parts that carry cubes (the cape's swing).
    pub geometry_turn: Option<Mat4>,
    /// Parts hung on `root` whatever their parent, every other root part left out (the
    /// first-person arm and its sleeve).
    pub hung_on_root: Option<[Option<usize>; 2]>,
}

impl PartChain {
    /// Root parts under `root`, nothing else.
    pub fn under(root: Mat4) -> PartChain {
        PartChain { root, drawn_offset: None, geometry_turn: None, hung_on_root: None }
    }

    /// The same chain, its drawn matrices shifted by `offset`.
    pub fn with_drawn_offset(self, offset: Mat4) -> PartChain {
        PartChain { drawn_offset: Some(offset), ..self }
    }
}

/// Frames of the parts of the last layer placed (none: hidden, or under a hidden parent), kept in
/// the matrix type the chain ran in.
#[derive(Default)]
pub(super) struct PartFrames {
    full: Vec<Option<Mat4>>,
    affine: Vec<Option<Affine3A>>,
    affine_in_use: bool,
}

impl PartFrames {
    /// Frame of part `i` of the last layer placed.
    pub fn get(&self, i: usize) -> Option<Mat4> {
        if self.affine_in_use { self.affine[i].map(Mat4::from) } else { self.full[i] }
    }

    /// The frames of the last layer placed written into `to` (the main layer's frames, read after
    /// the layers by the armor, cape and held items): by swapping buffers when `by_swap` and the
    /// chain ran in full matrices, otherwise by a copy. The frames read afterwards are not those.
    pub fn hand_over(&mut self, to: &mut Vec<Option<Mat4>>, by_swap: bool) {
        to.clear();
        if self.affine_in_use {
            to.extend(self.affine.iter().map(|f| f.map(Mat4::from)));
        } else if by_swap {
            std::mem::swap(to, &mut self.full);
        } else {
            to.extend_from_slice(&self.full);
        }
    }
}

/// Work buffers of a layer's pose and part frames, lent from one layer to the next.
#[derive(Default)]
pub(super) struct PoseScratch {
    pub parts: Vec<[f32; 9]>,
    pub visible: Vec<bool>,
    pub frames: PartFrames,
}

/// Matrix type the chain multiplies in: the full 4 × 4 (old path) or the affine 3 × 4.
trait ChainMatrix: Copy + std::ops::Mul<Output = Self> {
    fn from_mat4(m: &Mat4) -> Self;
    fn drawn(&self) -> PartMat;
}

impl ChainMatrix for Mat4 {
    fn from_mat4(m: &Mat4) -> Self {
        *m
    }

    fn drawn(&self) -> PartMat {
        PartMat::new(self)
    }
}

impl ChainMatrix for Affine3A {
    fn from_mat4(m: &Mat4) -> Self {
        // Checked in every test build too (the tests run optimised, without debug assertions):
        // each root the scene tests place passes here.
        if cfg!(any(test, debug_assertions)) {
            assert!(is_affine(m), "a part chain matrix is not affine: {m:?}");
        }
        Affine3A::from_mat4(*m)
    }

    fn drawn(&self) -> PartMat {
        PartMat::from_affine(self)
    }
}

/// Last row 0, 0, 0, 1 (either sign of zero): what dropping it to an affine matrix leaves unchanged.
pub(super) fn is_affine(m: &Mat4) -> bool {
    m.row(3) == glam::Vec4::W
}

impl PartMat {
    /// [`PartMat::new`] of an affine frame: the same columns and cofactors (the products and the
    /// determinant's sign in the same order, so the same bits), the last row written exactly.
    pub(super) fn from_affine(a: &Affine3A) -> Self {
        let (x, y, z) = (a.matrix3.x_axis, a.matrix3.y_axis, a.matrix3.z_axis);
        let (cx, cy, cz) = (y.cross(z), z.cross(x), x.cross(y));
        let sign = if x.dot(cx) < 0.0 { -1.0 } else { 1.0 };
        let normal = |v: Vec3A| column(v * sign, 0.0);
        PartMat {
            m: [column(x, 0.0), column(y, 0.0), column(z, 0.0), column(a.translation, 1.0)],
            n: [normal(cx), normal(cy), normal(cz)],
        }
    }
}

/// The three lanes of `v` then `w`. On x86-64 the whole register is stored and its fourth lane
/// overwritten: one store per column instead of three lanes pulled out one by one.
#[inline]
fn column(v: Vec3A, w: f32) -> [f32; 4] {
    #[cfg(target_arch = "x86_64")]
    {
        glam::Vec4::from(std::arch::x86_64::__m128::from(v)).with_w(w).to_array()
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        [v.x, v.y, v.z, w]
    }
}

/// Places the parts of `pose`'s layer under `chain`: their matrices appended to `mats` (zero
/// matrix for a hidden part), their frames kept in `frames`; returns the index of the first.
pub(super) fn place_part_chain(
    pose: &Pose<'_>,
    chain: &PartChain,
    frames: &mut PartFrames,
    mats: &mut Vec<PartMat>,
) -> u32 {
    let paths = animation_paths();
    let first = mats.len();
    mats.resize(first + pose.layer.parts.len(), PartMat::zeroed());
    let locals = LocalMatrices { cached: paths.cached_static_parts, closed_form: paths.closed_form_rotation };
    frames.affine_in_use = paths.affine_part_chain;
    let out = &mut mats[first..];
    if paths.affine_part_chain {
        chain_frames(pose, chain, locals, &mut frames.affine, out);
    } else {
        chain_frames(pose, chain, locals, &mut frames.full, out);
    }
    first as u32
}

/// The chain in matrix type `M`, written into `out` (one entry per part, zeroed beforehand).
fn chain_frames<M: ChainMatrix>(
    pose: &Pose<'_>,
    chain: &PartChain,
    locals: LocalMatrices,
    frames: &mut Vec<Option<M>>,
    out: &mut [PartMat],
) {
    let parts = &pose.layer.parts;
    frames.clear();
    frames.resize(parts.len(), None);
    let root = M::from_mat4(&chain.root);
    let offset = chain.drawn_offset.map(|o| M::from_mat4(&o));
    for (i, part) in parts.iter().enumerate() {
        let is_root = part.parent == u32::MAX;
        let parent = match chain.hung_on_root {
            Some(hung) if hung.contains(&Some(i)) => Some(root),
            Some(_) if is_root => None,
            _ if is_root => Some(root),
            _ => frames[part.parent as usize],
        };
        let Some(parent) = parent.filter(|_| pose.visible[i]) else { continue };
        let mut local = locals.of(part, &pose.parts[i]);
        if let Some(turn) = chain.geometry_turn
            && part.has_geometry
        {
            local *= turn;
        }
        let frame = parent * M::from_mat4(&local);
        frames[i] = Some(frame);
        out[i] = match offset {
            Some(offset) => (offset * frame).drawn(),
            None => frame.drawn(),
        };
    }
}

/// Which local matrix a part takes.
#[derive(Clone, Copy)]
struct LocalMatrices {
    /// `CACHED_STATIC_PART_MATRICES`: the baked rest matrix for a part at its definition pose.
    cached: bool,
    /// `CLOSED_FORM_PART_ROTATION`: the rotation formula.
    closed_form: bool,
}

impl LocalMatrices {
    fn of(self, part: &BakedPart, pose: &[f32; 9]) -> Mat4 {
        // Compared bit for bit: an animation that wrote −0.0 over 0.0 rebuilds its matrix, whose
        // zeros then carry its own sign.
        if self.cached && bytemuck::cast::<_, [u32; 9]>(*pose) == bytemuck::cast::<_, [u32; 9]>(part.pose) {
            return part.rest_local[usize::from(self.closed_form)];
        }
        part_matrix_with(pose, self.closed_form)
    }
}

/// The parts both layers know by name take the main layer's pose (`entities.md` part 3: armor,
/// cape and ears are posed like the body, not animated again).
pub(super) fn copy_body_pose(pose: &mut Pose<'_>, body_known: &[u16; KNOWN], body: &(Vec<[f32; 9]>, Vec<bool>)) {
    let layer = pose.layer;
    for (k, &own) in layer.known.iter().enumerate() {
        let from = body_known[k];
        if own != u16::MAX && from != u16::MAX {
            pose.parts[usize::from(own)] = body.0[usize::from(from)];
            pose.visible[usize::from(own)] = body.1[usize::from(from)];
        }
    }
}

impl EntityRenderer {
    /// One layer posed (definition pose; the main layer's pose copied onto the parts both share
    /// when `copy_body_of` names that layer; then `anim`) and placed under `chain`; returns the
    /// index of its first matrix.
    pub(super) fn push_layer_parts(
        &mut self,
        layer: usize,
        copy_body_of: Option<usize>,
        chain: &PartChain,
        anim: impl FnOnce(&mut Pose<'_>),
    ) -> u32 {
        let mut scratch = std::mem::take(&mut self.scratch);
        let mut pose = Pose::reset(&self.layers[layer].baked, scratch.parts, scratch.visible);
        if let Some(body) = copy_body_of {
            copy_body_pose(&mut pose, &self.layers[body].baked.known, &self.base_pose);
        }
        anim(&mut pose);
        let first = place_part_chain(&pose, chain, &mut scratch.frames, &mut self.mats);
        (scratch.parts, scratch.visible) = (pose.parts, pose.visible);
        self.scratch = scratch;
        first
    }
}
