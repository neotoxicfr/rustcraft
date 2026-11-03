//! Terrain draw commands: the batches of a frame encoded into the world pass, the translucent terrain of
//! improved transparency, the solids that follow the opaque terrain, and the indexed form of the
//! instanced commands (`RUSTCRAFT_OPT_FACEIDX`).

use super::{DrawArgs, DrawIndexedArgs, INDEXED_FACES, Renderer, TRANS_KEY};

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

impl Renderer {
    /// Terrain batches of the world pass in draw order, layer by layer. Between the cutout and the
    /// translucent layers come the entities and the effects drawn with them (cracks, leashes, labels,
    /// particles, debug traces); in improved transparency the pass stops before the translucent layer
    /// (`draw_oit` takes over). Returns whether those entities were drawn here.
    pub(super) fn encode_terrain_batches(&self, pass: &mut wgpu::RenderPass<'_>, oit_on: bool) -> bool {
        let mut current: Option<(usize, bool)> = None;
        // Entities: after the opaque and cutout terrain, before the translucent (part 2 § 3.11).
        let mut entities_done = false;
        for (b, &(layer, fluid, key, first, count)) in self.lists.batches.iter().enumerate() {
            // Improved transparency: the pass only keeps the solids; the translucent (the
            // last commands) follows in its stages (`draw_oit`).
            if layer == 2 && oit_on {
                break;
            }
            if layer == 2 && !entities_done {
                entities_done = true;
                // Cracks of the opaque blocks: after the opaque and cutout terrain.
                self.cracks.draw(pass, false);
                if let Some(e) = &self.entities {
                    e.draw(pass);
                    pass.set_bind_group(0, &self.bind, &[]);
                    self.leash.draw(pass);
                    self.tags.draw_normal(pass);
                    self.block_text.draw(pass);
                    self.text_displays.draw_offset(pass);
                    self.block_fx.draw(pass);
                    self.outline.draw(pass, false);
                }
                // Opaque particles with the entities (`particles.md` § 2.5).
                pass.set_bind_group(0, &self.bind, &[]);
                self.particles.draw_opaque(pass);
                // Debug (F3+B, F3+G, F3+F6 tracers): with the outlines, before the translucent
                // terrain (the water in front tints the lines).
                self.debug_lines.draw(pass, crate::debug_render::Layer::Depth);
                current = None;
            }
            let region = if key == TRANS_KEY {
                &self.trans
            } else if let Some(r) = self.regions.get(&key) {
                r
            } else {
                continue;
            };
            if current != Some((layer, fluid)) {
                pass.set_pipeline(match (fluid, layer) {
                    (true, 2) => &self.sorted_pipeline,
                    (true, _) => &self.fluid_pipelines[layer],
                    _ => &self.pipelines[layer],
                });
                current = Some((layer, fluid));
            }
            pass.set_bind_group(1, &region.bind, &[]);
            if !fluid && self.shader_opts.faceidx {
                self.draw_indexed_faces(pass, b);
                continue;
            }
            pass.set_vertex_buffer(
                0,
                match (fluid, layer) {
                    (true, 2) => region.order.slice(..),
                    (true, _) => region.fluids.slice(..),
                    _ => region.faces.slice(..),
                },
            );
            self.draw_instanced(pass, b, first, count);
        }
        entities_done
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
        let Some([blocks, sorted]) = &self.terrain_oit else { return };
        let mut current = None;
        for (b, &(layer, fluid, key, first, count)) in self.lists.batches.iter().enumerate() {
            if layer != 2 {
                continue;
            }
            let region = if key == TRANS_KEY {
                &self.trans
            } else if let Some(r) = self.regions.get(&key) {
                r
            } else {
                continue;
            };
            if current != Some(fluid) {
                f.set(pass, if fluid { sorted } else { blocks });
                current = Some(fluid);
            }
            pass.set_bind_group(1, &region.bind, &[]);
            if !fluid && self.shader_opts.faceidx {
                self.draw_indexed_faces(pass, b);
                continue;
            }
            pass.set_vertex_buffer(0, if fluid { region.order.slice(..) } else { region.faces.slice(..) });
            self.draw_instanced(pass, b, first, count);
        }
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

    /// Indexed commands of batch `b` (block faces, `RUSTCRAFT_OPT_FACEIDX`).
    fn draw_indexed_faces(&self, pass: &mut wgpu::RenderPass<'_>, b: usize) {
        let (first, count) = self.indexed_batch_ranges[b];
        pass.set_vertex_buffer(0, self.face_ids.0.slice(..));
        pass.set_index_buffer(self.face_index.slice(..), wgpu::IndexFormat::Uint16);
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
mod tests {
    use super::*;
    use crate::render::face_indices;

    /// Indexed commands (`RUSTCRAFT_OPT_FACEIDX`): same faces, in the same order, as the
    /// instanced commands, including beyond an index slice; one instance, no
    /// vertex offset (false on DX12 without indirect validation).
    #[test]
    fn indexed_args_draw_the_same_faces_in_order() {
        let a = |n, first| DrawArgs { vertex_count: 6, instance_count: n, first_vertex: 0, first_instance: first };
        let args = [a(40_000, 7), a(1, 100_000), a(INDEXED_FACES, 3), a(INDEXED_FACES + 1, 9)];
        let mut out = Vec::new();
        indexed_args(&args, &mut out);
        let index = face_indices(INDEXED_FACES);
        let drawn: Vec<u32> = out
            .iter()
            .flat_map(|c| {
                assert!(c.index_count as usize <= index.len() && c.index_count % 6 == 0, "{c:?}");
                assert_eq!((c.instance_count, c.first_index, c.base_vertex), (1, 0, 0), "{c:?}");
                index[..c.index_count as usize].chunks(6).map(move |t| c.first_instance + u32::from(t[0] >> 2))
            })
            .collect();
        let expected: Vec<u32> =
            args.iter().flat_map(|a| a.first_instance..a.first_instance + a.instance_count).collect();
        assert_eq!(drawn, expected);
        assert_eq!(out.len(), 3 + 1 + 1 + 2);
    }
}
