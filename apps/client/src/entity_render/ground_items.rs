//! Stacks drawn in the `ground` context (ground items, ominous item spawner). What a drawing
//! derives from the stack alone (model layers, model box, copy offsets) does not change from
//! one frame to the next: `GROUND_ITEM_LAYOUT_CACHE` keeps it per stack content, as long as
//! the stack's model reads nothing from its surroundings (a compass needle, a clock, a use in
//! progress are evaluated again every frame).

use std::sync::Arc;

use rc_proto_v777::item::ItemStack;

use super::*;

/// The part of a ground stack's drawing that depends on the stack alone.
pub(super) struct GroundLayout {
    layers: Vec<rc_assets::item::Layer>,
    /// Per layer: `ground` display transformation, then the layer's own.
    grounds: Vec<Mat4>,
    /// Lift of the bottom of the model's box plus 1/16, added to the bobbing.
    rise: f32,
    /// Per copy (by the stack's count): translation applied after the base matrix, `None` for
    /// the base itself.
    copies: Vec<Option<Mat4>>,
}

/// A kept layout, or why there is none.
enum KeptLayout {
    Draw(Arc<GroundLayout>),
    /// The stack draws nothing (no model, no layer, empty model).
    Nothing,
    /// The model reads the stack's surroundings: evaluated every frame.
    ReadsSurroundings,
}

/// Layouts kept per stack content, with the frame of their last use.
#[derive(Default)]
pub(super) struct GroundLayouts {
    by_stack: HashMap<ItemStack, (KeptLayout, u32)>,
    /// Generation of the item models the layouts come from (a resource reload drops them).
    generation: u64,
    frame: u32,
}

/// Bound of the kept layouts: stacks that each carry their own components (named, enchanted)
/// could otherwise pile up. Once full, a new stack is drawn without keeping anything (the old
/// path) rather than emptying the table: emptying it would rebuild it every frame while more
/// distinct stacks than this are in view.
pub(super) const MAX_KEPT_LAYOUTS: usize = 4096;

/// Frames after which a layout no longer used (its items picked up or despawned) is dropped.
pub(super) const LAYOUT_KEPT_FRAMES: u32 = 256;

/// Largest component data (bytes) of a stack whose layout is kept: the key is a full copy of
/// the stack, and a written book or a filled Shulker box carries tens of KiB. Such stacks are
/// drawn without keeping anything; the table then stays within a few MiB.
const MAX_KEPT_COMPONENT_BYTES: usize = 1024;

/// Bytes of the components a stack carries (its key's heap share): each added component's entry
/// and value, each removed type.
fn component_bytes(stack: &ItemStack) -> usize {
    let entry = std::mem::size_of::<(u16, Box<[u8]>)>();
    stack.added.iter().map(|(_, value)| entry + value.len()).sum::<usize>() + stack.removed.len() * 2
}

impl EntityRenderer {
    /// New frame for the kept ground layouts: those unused for a while are dropped.
    pub(super) fn begin_ground_layouts_frame(&mut self) {
        let kept = &mut self.ground_layouts;
        kept.frame = kept.frame.wrapping_add(1);
        if kept.frame.is_multiple_of(LAYOUT_KEPT_FRAMES) && !kept.by_stack.is_empty() {
            let now = kept.frame;
            kept.by_stack.retain(|_, (_, seen)| now.wrapping_sub(*seen) < LAYOUT_KEPT_FRAMES);
        }
    }
}

impl EntityRenderer {
    /// Group of items of a stack in the `ground` context (ground item, ominous spawner):
    /// matrix `pre`, rise `lift` (bobbing) plus lift of the bottom of the model's box (+1/16),
    /// neither for `None`, then `post`; copies depending on the quantity, offset by the game's
    /// 48-bit random (seed = item + damage). `owner`: id, position and visual rotation of the
    /// carrier (needles).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn push_item_cluster(
        &mut self,
        models: &rc_assets::item::ItemModels,
        stack: &ItemStack,
        owner: (i32, [f64; 3], f32),
        pre: Mat4,
        lift: Option<f32>,
        post: Mat4,
        packed: u32,
    ) -> bool {
        if self.opt.ground_item_layout_cache
            && let Some(drawn) = self.push_kept_ground_layout(models, stack, owner, pre, lift, post, packed)
        {
            return drawn;
        }
        let Some(layout) = self.ground_layout(models, stack, owner, false).0 else { return false };
        self.push_ground_layout(models, &layout, pre, lift, post, packed);
        true
    }

    /// [`Self::push_item_cluster`] from the kept layout of `stack` (made on first sight); `None`
    /// when the stack's model reads its surroundings, or when the stack is not kept (table full,
    /// heavy components).
    #[allow(clippy::too_many_arguments)]
    fn push_kept_ground_layout(
        &mut self,
        models: &rc_assets::item::ItemModels,
        stack: &ItemStack,
        owner: (i32, [f64; 3], f32),
        pre: Mat4,
        lift: Option<f32>,
        post: Mat4,
        packed: u32,
    ) -> Option<bool> {
        let kept = &mut self.ground_layouts;
        if kept.generation != models.generation() {
            kept.by_stack.clear();
            kept.generation = models.generation();
        }
        let (frame, full) = (kept.frame, kept.by_stack.len() >= MAX_KEPT_LAYOUTS);
        let layout = match kept.by_stack.get_mut(stack) {
            Some((entry, seen)) => {
                *seen = frame;
                match entry {
                    KeptLayout::Draw(layout) => layout.clone(),
                    KeptLayout::Nothing => return Some(false),
                    KeptLayout::ReadsSurroundings => return None,
                }
            }
            None if full || component_bytes(stack) > MAX_KEPT_COMPONENT_BYTES => return None,
            None => {
                let (layout, reads_surroundings) = self.ground_layout(models, stack, owner, true);
                let layout = layout.map(Arc::new);
                let entry = match (&layout, reads_surroundings) {
                    (_, true) => KeptLayout::ReadsSurroundings,
                    (Some(l), false) => KeptLayout::Draw(l.clone()),
                    (None, false) => KeptLayout::Nothing,
                };
                self.ground_layouts.by_stack.insert(stack.clone(), (entry, frame));
                // Evaluated once this frame anyway: drawn from it.
                let Some(layout) = layout else { return Some(false) };
                layout
            }
        };
        self.push_ground_layout(models, &layout, pre, lift, post, packed);
        Some(true)
    }

    /// Layout of `stack` on the ground, and whether its model read the surroundings (only
    /// watched when `watched`, otherwise reported as read).
    fn ground_layout(
        &mut self,
        models: &rc_assets::item::ItemModels,
        stack: &ItemStack,
        owner: (i32, [f64; 3], f32),
        watched: bool,
    ) -> (Option<GroundLayout>, bool) {
        let info = usize::try_from(stack.item).ok().and_then(|i| self.source.items.get(i));
        let view =
            crate::stack::StackRef { stack, info, ids: &self.ids, data: &self.source, hold: crate::stack::Hold::NONE };
        let Some(model) = view.model() else { return (None, false) };
        let name =
            usize::try_from(stack.item).ok().and_then(|i| self.item_names()?.get(i)).cloned().unwrap_or_default();
        let env = crate::stack::OwnerEnv::new(&self.world, "ground", owner.0, owner.1, owner.2);
        let (mut layers, mut key) = (Vec::new(), Vec::new());
        let reads_surroundings = if watched {
            models.layers_watched(&name, &model, &view, &env, env.seed(), &mut layers, &mut key)
        } else {
            models.layers(&name, &model, &view, &env, env.seed(), &mut layers, &mut key);
            true
        };
        if layers.is_empty() {
            return (None, reads_surroundings);
        }
        // Box of the model after `ground` display and local transformations.
        let grounds: Vec<Mat4> = layers
            .iter()
            .map(|l| {
                let b = models.baked(l.model).unwrap_or(&models.missing);
                Mat4::from_cols_array_2d(&b.displays[1]).transpose() * Mat4::from_cols_array_2d(&l.local).transpose()
            })
            .collect();
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for (l, m) in layers.iter().zip(&grounds) {
            for q in models.baked(l.model).unwrap_or(&models.missing).quads() {
                for p in q.pos {
                    let v = m.transform_point3(Vec3::from(p));
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
            }
            // Special model: its geometry (the base model is empty) gives the box.
            if l.special != 0 {
                let mut sq = Vec::new();
                self.special.quads(l.special, &l.tints, &mut sq);
                for q in &sq {
                    for p in q.pos {
                        let v = m.transform_point3(Vec3::from(p));
                        lo = lo.min(v);
                        hi = hi.max(v);
                    }
                }
            }
        }
        if lo.x > hi.x {
            return (None, reads_surroundings);
        }
        let damage = rc_assets::item::StackView::damage(&view);
        let copies = copy_offsets(stack, damage, hi.z - lo.z);
        (Some(GroundLayout { layers, grounds, rise: -lo.y + 0.0625, copies }), reads_surroundings)
    }

    /// Drops the kept layouts (proofs that change the switches of a renderer).
    #[cfg(test)]
    pub(super) fn forget_ground_layouts(&mut self) {
        self.ground_layouts = GroundLayouts::default();
    }

    /// Turns every kept layout into a wrong one ("draws nothing") from an older generation of
    /// the item models, as after a resource reload: the next frame must drop them all
    /// (proofs only).
    #[cfg(test)]
    pub(super) fn make_ground_layouts_stale(&mut self) {
        let kept = &mut self.ground_layouts;
        kept.generation = kept.generation.wrapping_add(1);
        for (entry, _) in kept.by_stack.values_mut() {
            *entry = KeptLayout::Nothing;
        }
    }

    /// Kept layouts drawn from, and stacks found to read their surroundings (proofs only).
    #[cfg(test)]
    pub(super) fn ground_layouts_counts(&self) -> (usize, usize) {
        let kept = self.ground_layouts.by_stack.values();
        let drawn = kept.clone().filter(|k| matches!(k.0, KeptLayout::Draw(_))).count();
        (drawn, kept.filter(|k| matches!(k.0, KeptLayout::ReadsSurroundings)).count())
    }

    /// Instances of a ground layout: base matrix `pre`, bobbing `lift` plus the layout's rise,
    /// `post`; then each copy and each layer.
    fn push_ground_layout(
        &mut self,
        models: &rc_assets::item::ItemModels,
        layout: &GroundLayout,
        pre: Mat4,
        lift: Option<f32>,
        post: Mat4,
        packed: u32,
    ) {
        let base = match lift {
            Some(bob) => pre * Mat4::from_translation(Vec3::new(0.0, bob + layout.rise, 0.0)) * post,
            None => pre * post,
        };
        for copy in &layout.copies {
            let c = copy.map_or(base, |offset| base * offset);
            for (l, ground) in layout.layers.iter().zip(&layout.grounds) {
                self.push_item_layer(models, l, c * *ground, packed);
            }
        }
    }
}

/// Copies drawn for a stack of `stack.count` items (1 to 5), offset by the game's 48-bit random
/// seeded by item + damage: scattered in a cube for a thick model (`depth` > 1/16), otherwise
/// fanned out along z.
fn copy_offsets(stack: &ItemStack, damage: i32, depth: f32) -> Vec<Option<Mat4>> {
    let n = match stack.count {
        ..=1 => 1,
        2..=16 => 2,
        17..=32 => 3,
        33..=48 => 4,
        _ => 5,
    };
    let mut seed = (i64::from(stack.item) + i64::from(damage)) as u64 ^ 0x5_DEEC_E66D;
    let mut rand = move || {
        seed = seed.wrapping_mul(0x5_DEEC_E66D).wrapping_add(11) & ((1 << 48) - 1);
        (seed >> 24) as f32 / (1u32 << 24) as f32
    };
    let mut copies = Vec::with_capacity(n);
    if depth > 0.0625 {
        copies.push(None);
        for _ in 1..n {
            let o = Vec3::new((rand() * 2.0 - 1.0) * 0.15, (rand() * 2.0 - 1.0) * 0.15, (rand() * 2.0 - 1.0) * 0.15);
            copies.push(Some(Mat4::from_translation(o)));
        }
    } else {
        let step = depth * 1.5;
        let mut z = -step * (n as f32 - 1.0) / 2.0;
        copies.push(Some(Mat4::from_translation(Vec3::new(0.0, 0.0, z))));
        for _ in 1..n {
            z += step;
            let o = Vec3::new((rand() * 2.0 - 1.0) * 0.075, (rand() * 2.0 - 1.0) * 0.075, z);
            copies.push(Some(Mat4::from_translation(o)));
        }
    }
    copies
}
