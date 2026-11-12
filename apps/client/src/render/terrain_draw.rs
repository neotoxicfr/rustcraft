//! Terrain draw commands: the batches of a frame encoded into the world pass, the translucent terrain of
//! improved transparency, the solids that follow the opaque terrain, and the indexed form of the
//! instanced commands (`RUSTCRAFT_OPT_FACEIDX`).
//!
//! `RUSTCRAFT_OPT_TERRAIN_BIND_STATE` (read once, on by default): the batch loop does not set again a
//! binding the pass already holds (region group, vertex slot 0, face indices).

use super::regions::RegionGpu;
use super::{Batch, DrawArgs, DrawIndexedArgs, INDEXED_FACES, Renderer, TRANS_KEY};

/// Indexed commands equivalent to the instanced commands `args` (block faces, 6 vertices
/// per instance): same faces in the same order, an instance whose attribute (identity buffer,
/// `first_instance`) gives the first face, split at [`INDEXED_FACES`] faces.
pub(super) fn indexed_args(args: &[DrawArgs], out: &mut Vec<DrawIndexedArgs>) {
    for a in args {
        let mut start = a.first_instance;
        let end = a.first_instance + a.instance_count;
        while start < end {
            let n = (end - start).min(INDEXED_FACES);
            out.push(DrawIndexedArgs {
                index_count: n * 6,
                instance_count: 1,
                first_index: 0,
                base_vertex: 0,
                first_instance: start,
            });
            start += n;
        }
    }
}

/// What the terrain loop asks of the pass it encodes into: a pass of the frame, or a recorder in
/// the tests.
pub(super) trait TerrainCommands {
    /// What is drawn between the cutout and the translucent terrain (cracks, entities, texts,
    /// effects, opaque particles, debug lines): it leaves every binding of the pass unknown.
    fn features(&mut self);
    fn pipeline(&mut self, layer: usize, fluid: bool);
    /// Group 1: the buffers of region `region`.
    fn region_group(&mut self, region: (i32, i32));
    fn vertex_slot(&mut self, source: VertexSlot);
    /// Index buffer: the face indices shared by every indexed command.
    fn face_index(&mut self);
    fn draw_indexed(&mut self, batch: usize);
    fn draw_instanced(&mut self, batch: usize, first: u32, count: u32);
}

/// What vertex slot 0 holds for a terrain draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum VertexSlot {
    /// Identity buffer of the indexed faces, shared by every region.
    FaceIds,
    /// A buffer of a region: its block faces, its fluid quads, its sorted translucent order.
    Faces((i32, i32)),
    Fluids((i32, i32)),
    SortedOrder((i32, i32)),
}

impl VertexSlot {
    /// Slot 0 of an instanced batch of `region`.
    fn instanced(region: (i32, i32), layer: usize, fluid: bool) -> Self {
        match (fluid, layer) {
            (true, 2) => VertexSlot::SortedOrder(region),
            (true, _) => VertexSlot::Fluids(region),
            _ => VertexSlot::Faces(region),
        }
    }
}

/// Batches a terrain loop draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TerrainLoop {
    /// The world pass: opaque and cutout terrain, the features, then the translucent terrain unless
    /// improved transparency draws it.
    WorldPass { improved_transparency: bool },
    /// The translucent layer alone, in a stage of improved transparency.
    TranslucentStage,
}

/// Choices of the terrain loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TerrainLoopPaths {
    /// Block faces drawn as indexed commands (`RUSTCRAFT_OPT_FACEIDX`).
    pub indexed_faces: bool,
    /// `RUSTCRAFT_OPT_TERRAIN_BIND_STATE`: a binding equal to the one in place is not set again.
    pub keep_bindings: bool,
    #[cfg(test)]
    pub sabotage: Option<BindingSabotage>,
}

/// Faults the tests put into the terrain loop to see their proof fail.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BindingSabotage {
    /// A region buffer set in slot 0 (fluid batch) is not noted: the face identity looks still bound.
    RegionSlotNotNoted,
    /// The bindings noted before the features are kept after them.
    KeptAcrossFeatures,
}

impl TerrainLoopPaths {
    #[cfg(test)]
    fn sabotaged(&self, s: BindingSabotage) -> bool {
        self.sabotage == Some(s)
    }
}

/// Bindings the terrain loop set in the pass (`None` / `false`: unknown).
#[derive(Default)]
struct PassBindings {
    pipeline: Option<(usize, bool)>,
    region_group: Option<(i32, i32)>,
    vertex_slot: Option<VertexSlot>,
    face_index: bool,
}

impl PassBindings {
    /// What the features left: nothing known (the pipeline is set again in every path).
    fn forget(&mut self, paths: TerrainLoopPaths) {
        #[cfg(test)]
        if paths.sabotaged(BindingSabotage::KeptAcrossFeatures) {
            self.pipeline = None;
            return;
        }
        let _ = paths;
        *self = PassBindings::default();
    }

    fn set_pipeline(&mut self, out: &mut impl TerrainCommands, layer: usize, fluid: bool) {
        if self.pipeline != Some((layer, fluid)) {
            out.pipeline(layer, fluid);
            self.pipeline = Some((layer, fluid));
        }
    }

    fn set_region_group(&mut self, out: &mut impl TerrainCommands, region: (i32, i32), paths: TerrainLoopPaths) {
        if !(paths.keep_bindings && self.region_group == Some(region)) {
            out.region_group(region);
            self.region_group = Some(region);
        }
    }

    fn set_vertex_slot(&mut self, out: &mut impl TerrainCommands, source: VertexSlot, paths: TerrainLoopPaths) {
        if paths.keep_bindings && self.vertex_slot == Some(source) {
            return;
        }
        out.vertex_slot(source);
        #[cfg(test)]
        if paths.sabotaged(BindingSabotage::RegionSlotNotNoted) && source != VertexSlot::FaceIds {
            return;
        }
        self.vertex_slot = Some(source);
    }

    fn set_face_index(&mut self, out: &mut impl TerrainCommands, paths: TerrainLoopPaths) {
        if !(paths.keep_bindings && self.face_index) {
            out.face_index();
            self.face_index = true;
        }
    }
}

/// Encodes the terrain batches of `which` into `out`, layer by layer; the batches of a region
/// `has_region` does not know are left out. Returns whether the features were drawn.
pub(super) fn encode_terrain_loop(
    batches: &[Batch],
    which: TerrainLoop,
    paths: TerrainLoopPaths,
    has_region: impl Fn((i32, i32)) -> bool,
    out: &mut impl TerrainCommands,
) -> bool {
    let mut bound = PassBindings::default();
    let mut features_drawn = false;
    for (b, &(layer, fluid, region, first, count)) in batches.iter().enumerate() {
        match which {
            TerrainLoop::TranslucentStage if layer != 2 => continue,
            // Improved transparency: the world pass only keeps the solids; the translucent (the
            // last commands) follows in its stages (`draw_oit`).
            TerrainLoop::WorldPass { improved_transparency: true } if layer == 2 => break,
            // Entities: after the opaque and cutout terrain, before the translucent (part 2 § 3.11).
            TerrainLoop::WorldPass { .. } if layer == 2 && !features_drawn => {
                features_drawn = true;
                out.features();
                bound.forget(paths);
            }
            _ => {}
        }
        if !has_region(region) {
            continue;
        }
        bound.set_pipeline(out, layer, fluid);
        bound.set_region_group(out, region, paths);
        if !fluid && paths.indexed_faces {
            bound.set_vertex_slot(out, VertexSlot::FaceIds, paths);
            bound.set_face_index(out, paths);
            out.draw_indexed(b);
        } else {
            bound.set_vertex_slot(out, VertexSlot::instanced(region, layer, fluid), paths);
            out.draw_instanced(b, first, count);
        }
    }
    features_drawn
}

/// A pass of the frame as the terrain loop's target: the world pass, or a stage of improved
/// transparency (its stage and the two translucent pipeline sets: block faces, sorted fluids).
struct TerrainPass<'a, 'e> {
    renderer: &'a Renderer,
    pass: &'a mut wgpu::RenderPass<'e>,
    stage: Option<(crate::oit::Frame<'a>, &'a [crate::oit::Set; 2])>,
    /// Bindings set (perf log).
    bindings_set: u64,
}

impl<'a> TerrainPass<'a, '_> {
    fn region(&self, key: (i32, i32)) -> &'a RegionGpu {
        self.renderer.terrain_region(key).expect("the loop only draws the regions it knows")
    }
}

impl TerrainCommands for TerrainPass<'_, '_> {
    fn features(&mut self) {
        let (r, pass) = (self.renderer, &mut *self.pass);
        // Cracks of the opaque blocks: after the opaque and cutout terrain.
        r.cracks.draw(pass, false);
        if let Some(e) = &r.entities {
            e.draw(pass);
            pass.set_bind_group(0, &r.bind, &[]);
            r.leash.draw(pass);
            r.tags.draw_normal(pass);
            r.block_text.draw(pass);
            r.text_displays.draw_offset(pass);
            r.block_fx.draw(pass);
            r.outline.draw(pass, false);
        }
        // Opaque particles with the entities (`particles.md` § 2.5).
        pass.set_bind_group(0, &r.bind, &[]);
        r.particles.draw_opaque(pass);
        // Debug (F3+B, F3+G, F3+F6 tracers): with the outlines, before the translucent
        // terrain (the water in front tints the lines).
        r.debug_lines.draw(pass, crate::debug_render::Layer::Depth);
    }

    fn pipeline(&mut self, layer: usize, fluid: bool) {
        let r = self.renderer;
        match self.stage {
            Some((f, [blocks, sorted])) => f.set(self.pass, if fluid { sorted } else { blocks }),
            None => self.pass.set_pipeline(match (fluid, layer) {
                (true, 2) => &r.sorted_pipeline,
                (true, _) => &r.fluid_pipelines[layer],
                _ => &r.pipelines[layer],
            }),
        }
    }

    fn region_group(&mut self, region: (i32, i32)) {
        self.bindings_set += 1;
        let bind = &self.region(region).bind;
        self.pass.set_bind_group(1, bind, &[]);
    }

    fn vertex_slot(&mut self, source: VertexSlot) {
        self.bindings_set += 1;
        let slice = match source {
            VertexSlot::FaceIds => self.renderer.face_ids.0.slice(..),
            VertexSlot::Faces(k) => self.region(k).faces.slice(..),
            VertexSlot::Fluids(k) => self.region(k).fluids.slice(..),
            VertexSlot::SortedOrder(k) => self.region(k).order.slice(..),
        };
        self.pass.set_vertex_buffer(0, slice);
    }

    fn face_index(&mut self) {
        self.bindings_set += 1;
        self.pass.set_index_buffer(self.renderer.face_index.slice(..), wgpu::IndexFormat::Uint16);
    }

    fn draw_indexed(&mut self, batch: usize) {
        self.renderer.draw_indexed_faces(self.pass, batch);
    }

    fn draw_instanced(&mut self, batch: usize, first: u32, count: u32) {
        self.renderer.draw_instanced(self.pass, batch, first, count);
    }
}

impl Renderer {
    /// Region of a terrain batch: the translucent fluids' shared one, or a region of columns.
    fn terrain_region(&self, key: (i32, i32)) -> Option<&RegionGpu> {
        if key == TRANS_KEY { Some(&self.trans) } else { self.regions.get(&key) }
    }

    fn terrain_loop_paths(&self) -> TerrainLoopPaths {
        TerrainLoopPaths {
            indexed_faces: self.shader_opts.faceidx,
            keep_bindings: rc_data::opt_switch!("RUSTCRAFT_OPT_TERRAIN_BIND_STATE"),
            #[cfg(test)]
            sabotage: None,
        }
    }

    /// Runs the terrain loop into `pass`; returns whether the features were drawn.
    fn encode_into(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        which: TerrainLoop,
        stage: Option<(crate::oit::Frame<'_>, &[crate::oit::Set; 2])>,
    ) -> bool {
        let mut target = TerrainPass { renderer: self, pass, stage, bindings_set: 0 };
        let has_region = |k| self.terrain_region(k).is_some();
        let drawn = encode_terrain_loop(&self.lists.batches, which, self.terrain_loop_paths(), has_region, &mut target);
        crate::perf_log::counter("terrain_bindings_set_per_frame", target.bindings_set);
        drawn
    }

    /// Terrain batches of the world pass in draw order, layer by layer. Between the cutout and the
    /// translucent layers come the entities and the effects drawn with them (cracks, leashes, labels,
    /// particles, debug traces); in improved transparency the pass stops before the translucent layer
    /// (`draw_oit` takes over). Returns whether those entities were drawn here.
    pub(super) fn encode_terrain_batches(&self, pass: &mut wgpu::RenderPass<'_>, oit_on: bool) -> bool {
        self.encode_into(pass, TerrainLoop::WorldPass { improved_transparency: oit_on }, None)
    }

    /// Improved transparency, solids pass: what follows the opaque and cutout terrain
    /// (cracks, entities, texts and effects of opaque blocks, opaque particles, debug).
    pub(super) fn draw_solid_features(&self, pass: &mut wgpu::RenderPass<'_>) {
        // Cracks of the opaque blocks: stay in the solids pass (classic blend).
        pass.set_bind_group(0, &self.bind, &[]);
        self.cracks.draw(pass, false);
        if let Some(e) = &self.entities {
            e.draw_solid(pass);
            pass.set_bind_group(0, &self.bind, &[]);
            self.leash.draw(pass);
            self.tags.draw_solid(pass);
            // Sign text: always opaque and without background, therefore solid in the game.
            self.block_text.draw(pass);
            self.text_displays.draw_solid(pass);
            self.block_fx.draw_solid(pass);
        }
        self.outline.draw_solid(pass);
        pass.set_bind_group(0, &self.bind, &[]);
        self.particles.draw_opaque(pass);
        self.debug_lines.draw_solid(pass);
    }

    /// Translucent commands of the terrain (layer 2) with the pipelines of stage `f`.
    pub(super) fn draw_translucent_terrain(&self, pass: &mut wgpu::RenderPass<'_>, f: crate::oit::Frame<'_>) {
        let Some(sets) = &self.terrain_oit else { return };
        self.encode_into(pass, TerrainLoop::TranslucentStage, Some((f, sets)));
    }

    /// Instanced commands of batch `b` (`first..first + count` of its commands; with
    /// `RUSTCRAFT_OPT_FACEIDX`, place in the fluid list); fallback: one per series without
    /// indirect draw.
    fn draw_instanced(&self, pass: &mut wgpu::RenderPass<'_>, b: usize, first: u32, count: u32) {
        let (list, first, count) = if self.shader_opts.faceidx {
            (&self.fluid_draw_args, self.indexed_batch_ranges[b].0, self.indexed_batch_ranges[b].1)
        } else {
            (&self.lists.args, first, count)
        };
        if self.indirect_ok {
            pass.multi_draw_indirect(&self.indirect, u64::from(first) * 16, count);
        } else {
            for a in &list[first as usize..(first + count) as usize] {
                pass.draw(0..a.vertex_count, a.first_instance..a.first_instance + a.instance_count);
            }
        }
    }

    /// Indexed commands of batch `b` (block faces, `RUSTCRAFT_OPT_FACEIDX`); the identity buffer and
    /// the face indices are bound by the loop.
    fn draw_indexed_faces(&self, pass: &mut wgpu::RenderPass<'_>, b: usize) {
        let (first, count) = self.indexed_batch_ranges[b];
        if self.indirect_ok {
            pass.multi_draw_indexed_indirect(&self.indirect_idx, u64::from(first) * 20, count);
        } else {
            for a in &self.indexed_draw_args[first as usize..(first + count) as usize] {
                pass.draw_indexed(0..a.index_count, 0, a.first_instance..a.first_instance + 1);
            }
        }
    }
}

#[cfg(test)]
#[path = "terrain_draw_test.rs"]
mod tests;
