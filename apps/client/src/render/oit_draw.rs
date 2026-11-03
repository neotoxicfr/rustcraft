//! Improved transparency: preparation of its targets and pipelines, the staged passes of the translucent
//! elements, and the composition over the solids pass.

use glam::Mat4;

use super::Renderer;
use super::pipelines::instance_layout;
use crate::gpu_time::Role;

impl Renderer {
    /// Improved transparency: full-screen passes, targets at the frame size, pipelines of the
    /// families (made once), frame projection.
    pub(super) fn prepare_oit(&mut self, proj: Mat4) {
        let oit = self.oit.get_or_insert_with(|| {
            eprintln!("improved transparency: passes prepared");
            crate::oit::Oit::new(&self.device, self.config.format)
        });
        oit.ensure(&self.device, (self.config.width, self.config.height), &self.depth);
        oit.set_projection(&self.queue, proj);
        if self.terrain_oit.is_none() {
            self.terrain_oit = Some(terrain_oit_sets(
                &self.device,
                &oit.layouts,
                &self.terrain_res.layout,
                &self.column_layout,
                self.formats.entries().0,
                self.shader_opts,
            ));
        }
        self.sky.ensure_oit(&self.device, &oit.layouts);
        self.particles.ensure_oit(&self.device, &oit.layouts);
        self.weather.ensure_oit(&self.device, &oit.layouts);
        if let Some(e) = &mut self.entities {
            e.ensure_oit(&self.device, &oit.layouts);
        }
        self.tags.ensure_oit(&self.device, &oit.layouts);
        self.text_displays.ensure_oit(&self.device, &oit.layouts);
        self.outline.ensure_oit(&self.device, &oit.layouts);
        self.block_fx.ensure_oit(&self.device, &oit.layouts);
        self.cracks.ensure_oit(&self.device, &oit.layouts);
        self.debug_lines.ensure_oit(&self.device, &oit.layouts);
    }

    /// Improved transparency, after the composition.
    pub(super) fn draw_after_oit(&self, pass: &mut wgpu::RenderPass<'_>) {
        // Like the game: see-through texts, then "always in front" traces.
        self.tags.draw_see_through(pass);
        self.text_displays.draw_see_through(pass);
        self.debug_lines.draw(pass, crate::debug_render::Layer::Front);
    }

    /// Stages of the improved transparency then composition (after the solids pass).
    /// Terrain in the terrain depth (boats' water mask included), elements in the
    /// main depth, clouds in theirs; bounds culling after the first stage.
    pub(super) fn draw_oit(&self, enc: &mut wgpu::CommandEncoder, view: &wgpu::TextureView) {
        use crate::oit::Stage;
        let Some(oit) = &self.oit else { return };
        let mask = self.entities.as_ref().is_some_and(|e| e.has_water_mask());
        let ts = |name| self.stamp(name, Role::Inner);
        let water_mask = |enc: &mut wgpu::CommandEncoder| {
            let mut pass = oit.begin_water_mask(enc, ts("transparency: water mask"));
            pass.set_bind_group(0, &self.bind, &[]);
            if let Some(e) = &self.entities {
                e.draw_water_mask(&mut pass);
            }
        };
        if mask {
            water_mask(enc);
        }
        let terrain_depth = if mask { oit.terrain_depth() } else { &self.depth };
        let clouds = self.sky.has_clouds();
        for stage in Stage::ALL {
            let f = oit.frame(stage);
            let [terrain, features, cloud_pass] = stage.pass_names();
            {
                let mut pass = oit.begin(enc, stage, terrain_depth, true, terrain, ts(terrain));
                pass.set_bind_group(0, &self.bind, &[]);
                self.draw_translucent_terrain(&mut pass, f);
            }
            {
                let mut pass = oit.begin(enc, stage, &self.depth, false, features, ts(features));
                pass.set_bind_group(0, &self.bind, &[]);
                self.draw_translucent_features(&mut pass, f);
            }
            if stage == Stage::Bounds {
                oit.cull(enc, &self.depth, ts("transparency: bounds culling"));
            }
            if clouds {
                let mut pass = oit.begin_clouds(enc, stage, ts(cloud_pass));
                self.sky.draw_clouds_oit(&mut pass, f);
            }
            // Main depth changed by the culling: water mask redone on it.
            if stage == Stage::Bounds && mask {
                water_mask(enc);
            }
        }
        oit.composite(enc, view, &self.depth, ts("transparency: composition"));
    }

    /// Translucent elements (stage `f`): shadows, models, labels, texts, outlines, items,
    /// cracks, particles, debug, then border and weather; group 0 (camera) bound.
    fn draw_translucent_features(&self, pass: &mut wgpu::RenderPass<'_>, f: crate::oit::Frame<'_>) {
        if let Some(e) = &self.entities {
            e.draw_oit(pass, f);
            pass.set_bind_group(0, &self.bind, &[]);
        }
        self.tags.draw_oit(pass, f);
        self.text_displays.draw_oit(pass, f);
        self.outline.draw_oit(pass, f);
        self.block_fx.draw_oit(pass, f);
        self.cracks.draw_oit(pass, f);
        self.particles.draw_translucent_oit(pass, f);
        self.debug_lines.draw_oit(pass, f);
        pass.set_bind_group(0, &self.bind, &[]);
        self.weather.draw_oit(pass, f);
    }
}

/// Pipelines of the translucent terrain in improved transparency: block faces (`vs`) and sorted
/// fluids (`vs_sorted`), back faces culled like the classic terrain.
fn terrain_oit_sets(
    device: &wgpu::Device,
    layouts: &crate::oit::Layouts,
    camera: &wgpu::BindGroupLayout,
    column: &wgpu::BindGroupLayout,
    vs_sorted: &str,
    opts: crate::terrain_shader::ShaderOpts,
) -> [crate::oit::Set; 2] {
    let module = crate::oit::module(
        device,
        "terrain (transparency)",
        &crate::terrain_shader::terrain(opts),
        include_str!("../terrain_oit.wgsl"),
    );
    let groups = [Some(camera), Some(column)];
    let face_vs = opts.face_entry();
    let faces = [Some(instance_layout(face_vs))];
    let sorted = [Some(instance_layout(vs_sorted))];
    let mut blocks = crate::oit::Spec::new(
        "translucent terrain (transparency)",
        &groups,
        &module,
        face_vs,
        &faces,
        "fs_translucent",
    )
    .cull(true);
    if face_vs == "vs" {
        blocks.topology = opts.topology();
    }
    [
        layouts.set(device, &blocks),
        layouts.set(
            device,
            &crate::oit::Spec::new(
                "sorted fluids (transparency)",
                &groups,
                &module,
                vs_sorted,
                &sorted,
                "fs_translucent",
            )
            .cull(true),
        ),
    ]
}
