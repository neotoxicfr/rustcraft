//! Entities under improved transparency (`crate::oit`): solids pass, water mask in the
//! terrain depth, then translucents, eyes, lightning, swirl, rays and shadows in the
//! stages.

use super::{
    EntityRenderer, FREE_BEAM, FREE_GBEAM, FREE_RAYS, FreeVertex, Instance, OIT_SOLID_PASSES, PASS_ITEM_TRANSLUCENT,
    PASS_TRANSLUCENT_CULL, ShadowTile, StaticVertex,
};
use crate::oit::{Frame, Layouts, Set, Spec};

/// Pipelines of the stages per render type of the game.
pub(super) struct EntityOit {
    /// Translucents (two faces), culled translucents, eyes, lightning, swirl, rays, shadows.
    translucent: Set,
    translucent_cull: Set,
    eyes: Set,
    bolt: Set,
    swirl: Set,
    rays: Set,
    shadow: Set,
    /// Water mask: depth only, no colour target.
    water_mask: wgpu::RenderPipeline,
}

impl EntityRenderer {
    /// Pipelines of the improved transparency (once).
    pub fn ensure_oit(&mut self, device: &wgpu::Device, layouts: &Layouts) {
        if self.oit_sets.is_some() {
            return;
        }
        let module = crate::oit::module(
            device,
            "entities (transparency)",
            include_str!("../entity.wgsl"),
            include_str!("../entity_oit.wgsl"),
        );
        let groups = [Some(&self.camera_layout), Some(&self.layout)];
        let model = [
            Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<StaticVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x3, 3 => Uint32],
            }),
            Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<Instance>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &wgpu::vertex_attr_array![4 => Uint32, 5 => Float32x4, 6 => Unorm8x4, 7 => Uint32, 8 => Uint32],
            }),
        ];
        let free = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<FreeVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![
                0 => Float32x3, 1 => Float32x2, 2 => Float32x4, 3 => Unorm8x4, 4 => Uint32, 5 => Float32x3
            ],
        })];
        let shadow = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<ShadowTile>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32, 3 => Float32],
        })];
        let set = |label, vs, buffers: &[Option<wgpu::VertexBufferLayout<'_>>], fs, cull| {
            layouts.set(device, &Spec::new(label, &groups, &module, vs, buffers, fs).cull(cull))
        };
        // Face culling like the game's pipelines: ordinary translucent models without,
        // culled variant, eyes, lightning, rays and shadows with (default setting).
        let sets = EntityOit {
            translucent: set("translucent entities (transparency)", "vs", &model, "fs_translucent", false),
            translucent_cull: set(
                "culled translucent entities (transparency)",
                "vs",
                &model,
                "fs_translucent",
                true,
            ),
            eyes: set("entity eyes (transparency)", "vs", &model, "fs_eyes", true),
            bolt: set("lightning (transparency)", "vs", &model, "fs_bolt", true),
            swirl: set("energy swirl (transparency)", "vs", &model, "fs_swirl", false),
            rays: set("entity rays (transparency)", "vs_free", &free, "fs_rays", true),
            shadow: set("entity shadows (transparency)", "vs_shadow", &shadow, "fs_shadow", true),
            water_mask: {
                let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("water mask (transparency)"),
                    bind_group_layouts: &groups,
                    immediate_size: 0,
                });
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("water mask (transparency)"),
                    layout: Some(&layout),
                    vertex: wgpu::VertexState {
                        module: &module,
                        entry_point: Some("vs"),
                        compilation_options: Default::default(),
                        buffers: &model,
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &module,
                        entry_point: Some("fs_water_mask"),
                        compilation_options: Default::default(),
                        targets: &[],
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
                })
            },
        };
        self.oit_sets = Some(sets);
    }

    /// World batches (excluding hand, totem and interface).
    fn world_batches(&self) -> &[(usize, u8, std::ops::Range<u32>, u32, u32)] {
        let split = self.hand_batch.min(self.batches.len());
        &self.batches[..split]
    }

    /// At least one boat masks the water in the frame.
    pub fn has_water_mask(&self) -> bool {
        self.world_batches().iter().any(|b| b.0 == 3)
    }

    fn bind_model(&self, pass: &mut wgpu::RenderPass<'_>, src: u8, bound: &mut u8) -> bool {
        if src != *bound {
            match src {
                0 => pass.set_vertex_buffer(0, self.statics.slice(..)),
                _ => match &self.item_buf {
                    Some((b, _)) => pass.set_vertex_buffer(0, b.slice(..)),
                    None => return false,
                },
            }
            *bound = src;
        }
        true
    }

    /// Improved transparency, solids pass: cutouts, opaques, dissolve, special models,
    /// reflections in a separate pass, beams (group 0 = camera already bound).
    pub fn draw_solid(&self, pass: &mut wgpu::RenderPass<'_>) {
        let (Some(bind), Some((inst, _))) = (&self.bind, &self.inst_buf) else { return };
        pass.set_bind_group(1, bind, &[]);
        pass.set_vertex_buffer(1, inst.slice(..));
        let (mut current, mut bound) = (usize::MAX, u8::MAX);
        for (p, src, verts, first, count) in self.world_batches() {
            if !OIT_SOLID_PASSES.contains(p) {
                continue;
            }
            if *p != current {
                pass.set_pipeline(
                    [
                        &self.cutout,
                        &self.translucent,
                        &self.eyes,
                        &self.water_mask,
                        &self.additive,
                        &self.dissolve,
                        &self.glint_pass,
                        &self.swirl,
                        &self.cutout_cull,
                        &self.solid,
                        &self.translucent_cull,
                        &self.translucent,
                        &self.translucent_cull,
                    ][*p],
                );
                current = *p;
            }
            if self.bind_model(pass, *src, &mut bound) {
                pass.draw(verts.clone(), *first..*first + *count);
            }
        }
        let beams = self.free_verts[FREE_BEAM].len() as u32;
        let rays = self.free_verts[FREE_RAYS].len() as u32;
        let gbeams = self.free_verts[FREE_GBEAM].len() as u32;
        if let Some((fb, _)) = self.free_buf.as_ref().filter(|_| beams + gbeams > 0) {
            pass.set_vertex_buffer(0, fb.slice(..));
            pass.set_pipeline(&self.free_beam);
            pass.draw(0..beams, 0..1);
            pass.set_pipeline(&self.free_gbeam);
            pass.draw(beams + rays..beams + rays + gbeams, 0..1);
        }
    }

    /// Water mask of the boats in the terrain depth (group 0 = camera already bound).
    pub fn draw_water_mask(&self, pass: &mut wgpu::RenderPass<'_>) {
        let (Some(bind), Some((inst, _)), Some(sets)) = (&self.bind, &self.inst_buf, &self.oit_sets) else { return };
        pass.set_bind_group(1, bind, &[]);
        pass.set_vertex_buffer(1, inst.slice(..));
        pass.set_pipeline(&sets.water_mask);
        let mut bound = u8::MAX;
        for (p, src, verts, first, count) in self.world_batches() {
            if *p == 3 && self.bind_model(pass, *src, &mut bound) {
                pass.draw(verts.clone(), *first..*first + *count);
            }
        }
    }

    /// Stage `f`: shadows, translucents, eyes, lightning, swirl, then rays (group 0 bound).
    pub fn draw_oit(&self, pass: &mut wgpu::RenderPass<'_>, f: Frame<'_>) {
        let (Some(bind), Some((inst, _)), Some(sets)) = (&self.bind, &self.inst_buf, &self.oit_sets) else { return };
        pass.set_bind_group(1, bind, &[]);
        if self.shadow_count > 0
            && let Some((sb, _)) = &self.shadow_buf
        {
            f.set(pass, &sets.shadow);
            pass.set_vertex_buffer(0, sb.slice(..));
            pass.draw(0..6, 0..self.shadow_count);
        }
        pass.set_vertex_buffer(1, inst.slice(..));
        let (mut current, mut bound) = (usize::MAX, u8::MAX);
        for (p, src, verts, first, count) in self.world_batches() {
            let set = match *p {
                1 => &sets.translucent,
                // Invisible entity ghost, translucent items and moving blocks: back faces
                // culled.
                PASS_TRANSLUCENT_CULL | PASS_ITEM_TRANSLUCENT => &sets.translucent_cull,
                2 => &sets.eyes,
                4 => &sets.bolt,
                7 => &sets.swirl,
                _ => continue,
            };
            if *p != current {
                f.set(pass, set);
                current = *p;
            }
            if self.bind_model(pass, *src, &mut bound) {
                pass.draw(verts.clone(), *first..*first + *count);
            }
        }
        let beams = self.free_verts[FREE_BEAM].len() as u32;
        let rays = self.free_verts[FREE_RAYS].len() as u32;
        if let Some((fb, _)) = self.free_buf.as_ref().filter(|_| rays > 0) {
            f.set(pass, &sets.rays);
            pass.set_vertex_buffer(0, fb.slice(..));
            pass.draw(beams..beams + rays, 0..1);
        }
    }
}
