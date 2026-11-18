//! One frame of the renderer: camera uniforms, the uploads of every layer, the passes in order (world,
//! improved transparency, glowing outlines, screen effects, hand and interface, menu blur), submission
//! and presentation.

use std::sync::Arc;
use std::time::Instant;

use rc_mesh::visibility::Graph;

use super::capture::surface_state;
use super::column_upload::walk_connectivity;
use super::compiled_sections::ColumnSquare;
use super::fog_cull::{self, WalkInputs};
use super::frustum::{self, MeshingView, frustum_planes};
use super::regions::identity_buffer;
use super::terrain_draw::indexed_args;
use super::{
    CAM_GUI, CAM_HAND, CAM_TOTEM, CAM_WORLD, CameraUniform, DEPTH_CLEAR, DrawArgs, DrawIndexedArgs, DrawKey, FrameEnv,
    Renderer, VisSnapshot, WalkKey, WalkResult, lists, perspective,
};
use crate::gpu_time::Role;
use crate::perf_log::ListStage;
use crate::present_watch::Stage;
use crate::sky::SkyOrder;

impl Renderer {
    /// Draws and presents a frame; `false` if the surface provided nothing (hidden window,
    /// timeout, reconfiguration): the caller then counts no displayed frame.
    pub fn render(&mut self, eye: [f64; 3], yaw: f32, pitch: f32, min_y: i32, height: i32, env: &FrameEnv) -> bool {
        if let Some(t) = &mut self.timer {
            t.collect();
        }
        self.collect_world_icon();
        let frame = match self.surface.image_for_this_frame() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => {
                if std::mem::take(&mut self.warned_surface) {
                    eprintln!("presentation resumed: the surface provides frames again");
                }
                f
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.reconfigure_surface();
                return false;
            }
            other => {
                if !self.warned_surface {
                    self.warned_surface = true;
                    eprintln!("presentation blocked: the surface provides no frame ({})", surface_state(&other));
                }
                return false;
            }
        };
        // CPU work of the frame: from the acquired frame to the upload to the GPU (excluding the wait for display).
        let t_cpu = Instant::now();
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Camera frame: integer origin (x, z) in the shader, fractional part here.
        // Game convention: yaw 0 = towards +z, angles increasing towards −x; positive pitch = downwards.
        let (origin, local, view_m) = crate::camera::view_matrix(eye, yaw, pitch);
        let sky_view = crate::camera::view_rotation(yaw, pitch);
        let aspect = self.config.width as f32 / self.config.height.max(1) as f32;
        let perspective = |fov: f32| perspective(fov, aspect, 0.05, env.far);
        // World projection = P × shake × sway (view space, `hand-camera.md` § 1.1).
        let proj = perspective(self.view.fov) * self.view.tilt * self.distort;
        let view_proj = proj * view_m;
        // Section sorting: same view without shake, field max(fov, option) (§ 1.1).
        let cull_view_proj = perspective(self.view.cull_fov) * view_m;
        // Instant of the frame for the appearance fade (same instant CPU- and GPU-side).
        let frame_now = Instant::now();
        let uniform = CameraUniform {
            view_proj: view_proj.to_cols_array_2d(),
            origin: [origin[0], min_y, origin[1], i32::from(self.rgss)],
            shade: self.shade,
            fade_now: self.fade_ring.clock(frame_now),
            fade_len: (self.fade_secs * 1e6) as f32,
            eye: [local.x, local.y, local.z, 0.0],
            fog_color: env.fog.color,
            fog: [env.fog.env_start, env.fog.env_end, env.fog.render_start, env.fog.render_end],
            fog_sky: [env.fog.sky_end, env.fog.cloud_end],
            atlas_texel: self.atlas_texel,
            sky_color: env.sky,
            sky_view_proj: (proj * sky_view).to_cols_array_2d(),
        };
        self.cameras.set(CAM_WORLD, bytemuck::bytes_of(&uniform));
        // Hand: own projection (hand field, without shake: it is in the hand's stack),
        // same frame, fog and lights as the world.
        if self.view.hand {
            let hand =
                CameraUniform { view_proj: (perspective(self.view.hud_fov) * view_m).to_cols_array_2d(), ..uniform };
            self.cameras.set(CAM_HAND, bytemuck::bytes_of(&hand));
            // Totem: same projection, without the view rotation (pose in the view frame), without fog.
            let totem = uniform.overlay(perspective(self.view.hud_fov).to_cols_array_2d(), [0.0; 4]);
            self.cameras.set(CAM_TOTEM, bytemuck::bytes_of(&totem));
        }
        // Interface models: x to the right and y upwards in interface units (y = −row),
        // z towards the screen, (reversed) depth in [0.5, 1] (always in front of the hand), without fog.
        if self.gui.has_models() {
            // Same projected size as the 2D interface (unrounded, like the game).
            let (w, h) = (self.gui.proj[0].max(1.0), self.gui.proj[1].max(1.0));
            let vp = glam::Mat4::from_cols(
                glam::Vec4::new(2.0 / w, 0.0, 0.0, 0.0),
                glam::Vec4::new(0.0, 2.0 / h, 0.0, 0.0),
                glam::Vec4::new(0.0, 0.0, 0.001, 0.0),
                glam::Vec4::new(-1.0, 1.0, 0.75, 1.0),
            );
            let g = uniform.overlay(vp.to_cols_array_2d(), [0.0, 0.0, 1000.0, 0.0]);
            self.cameras.set(CAM_GUI, bytemuck::bytes_of(&g));
        }
        self.cameras.flush(&mut self.up, &self.queue);
        let sky_vp = proj * sky_view;
        self.sky.prepare(&self.device, &self.queue, &mut self.up, sky_vp, &env.bodies, env.dome, eye);
        if let Some(p) = self.panorama.as_mut().filter(|_| self.show_panorama) {
            p.prepare(&self.queue, &mut self.up, (self.config.width, self.config.height));
        }
        // Improved transparency: texts stored (solids or stages) as soon as the frame is uploaded.
        let oit_on = self.improved_transparency && self.oit_ok;
        self.tags.oit = oit_on;
        self.text_displays.oit = oit_on;
        self.gui.upload(&self.device, &self.queue, &mut self.up);
        if let Some(e) = &mut self.entities {
            e.upload(&self.device, &self.queue, &mut self.up);
        }
        let gui = &self.gui;
        self.tags.upload(&self.device, &self.queue, &mut self.up, gui.glyph_gen, || gui.glyph_view());
        self.block_text.upload(&self.device, &self.queue, &mut self.up, gui.glyph_gen, || gui.glyph_view());
        self.text_displays.upload(&self.device, &self.queue, &mut self.up, gui.glyph_gen, || gui.glyph_view());
        if let Some(e) = &self.entities {
            self.block_fx.upload(&self.queue, &mut self.up, &e.blocks.fx);
        }
        let size = (self.config.width, self.config.height);
        self.outline.upload(&self.queue, &mut self.up, size);
        self.cracks.upload(&self.queue, &mut self.up);
        self.leash.upload(&self.queue, &mut self.up);
        self.debug_lines.upload(&self.device, &self.queue, &mut self.up, size, &self.gui);
        self.particles.upload(&self.device, &self.queue, &mut self.up);
        self.weather.upload(&self.device, &self.queue, &mut self.up);
        // Appearance fade: visibilities of the recent columns (0 → 1 over `fade_secs`), or
        // only births and ends of fade on the GPU (`RUSTCRAFT_OPT_FADEGPU`).
        match self.fade_ring.frame(frame_now, self.fade_secs) {
            crate::terrain_shader::FadeWrite::None => {}
            crate::terrain_shader::FadeWrite::All(ring) => {
                self.up.write(&self.queue, &self.fade, 0, bytemuck::cast_slice(ring));
            }
            crate::terrain_shader::FadeWrite::Slots { slots, ring } => {
                for &s in slots {
                    let s = s as usize;
                    self.up.write(&self.queue, &self.fade, s as u64 * 4, bytemuck::cast_slice(&ring[s..s + 1]));
                }
            }
        }
        let planes = frustum_planes(cull_view_proj);
        // Kept for the meshing, which only remeshes the dirty sections in view (frustum moved back
        // as the game's: the sections right behind the camera are rebuilt too).
        let forward = crate::camera::axes(yaw, pitch).2;
        let meshing_view = MeshingView::new(planes, origin, eye, forward, min_y, height / 16);
        if self.meshing_view != Some(meshing_view) {
            self.meshing_view_rev += 1;
        }
        self.meshing_view = Some(meshing_view);
        // Improved transparency: targets and pipelines prepared as needed, targets freed otherwise;
        // first the depth target, with the usage the option needs.
        self.keep_depth_usage(oit_on);
        if oit_on {
            self.prepare_oit(proj);
        } else if let Some(o) = &mut self.oit {
            o.release();
        }
        // Work targets of the menu blur and of the image effects: freed once idle for a few seconds.
        self.blur.release_idle_targets(frame_now);
        self.post_fx.release_idle_targets(frame_now);
        // Storage of the next frame's objects (solids or stages).
        if let Some(e) = &mut self.entities {
            e.oit = oit_on;
        }

        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        if let Some(t) = &mut self.timer {
            t.begin(&mut enc);
        }
        // Last pass of the frame: that of the interface after the blur if there is one.
        let blur_radius = self.gui.blur_radius();
        let last_pass = blur_radius.is_none();
        self.gui.encode_items(&self.queue, &mut self.up, &mut enc, self.timer.as_ref());
        // Screen effects of the frame, and the pass that draws the hand and the interface.
        let effects = std::mem::take(&mut self.post_effects);
        let post = !effects.is_empty() && self.can_capture;
        let glow = self.entities.as_ref().is_some_and(|e| e.has_glow());
        // A world icon is copied between the hand and the interface: their own passes then.
        let world_icon = self.world_icon_request.take();
        let hand_pass = if world_icon.is_some() {
            HandPass::Own
        } else {
            HandPass::of_frame(PassMerges::from_switches(), oit_on, glow, post)
        };
        let full_viewport = PassViewport::full((self.config.width, self.config.height));
        let drawn: (usize, u64);
        {
            let hand_in_world_pass = hand_pass == HandPass::InWorldPass;
            let role = if hand_in_world_pass && last_pass { Role::WorldAndLast } else { Role::World };
            let timestamp_writes = self.stamp("world", role);
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("world"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // Background: fog colour, like the game (§ 7.4).
                        load: wgpu::LoadOp::Clear({
                            let c = env.fog.color.map(f64::from);
                            wgpu::Color { r: c[0], g: c[1], b: c[2], a: 0.0 }
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(DEPTH_CLEAR),
                        store: self.depth_paths.world_pass_store(oit_on),
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes,
                ..Default::default()
            });
            // Diagnostic: fragments and triangles of the world pass (real overdraw).
            if let Some(t) = &self.timer {
                t.stats_begin(&mut pass);
            }
            pass.set_bind_group(0, &self.bind, &[]);
            let world_viewport =
                if hand_in_world_pass { full_viewport.with_depth(0.0, WORLD_DEPTH_SHARE) } else { full_viewport };
            if hand_in_world_pass {
                world_viewport.set(&mut pass);
            }
            let sky = WorldSky { parts: SkyParts::of_frame(env), world_viewport };
            let sky_after_opaque = self.sky.order() == SkyOrder::AfterOpaque;
            if !sky_after_opaque {
                self.draw_sky(&mut pass, sky);
            }
            // Sections reached from the camera (occlusion sorting, docs/specs/visibility.md),
            // kept if their box (one block of margin: a model may overflow) is in the field.
            let t_vis = Instant::now();
            let sections = height / 16;
            let cam_col = (eye[0].floor() as i32 >> 4, eye[2].floor() as i32 >> 4);
            // The game's walk keeps to the render distance, whatever columns are held beyond it.
            let radius = env.render_distance;
            self.reposition_compiled_sections(ColumnSquare { center: cam_col, radius });
            crate::perf_log::list_stage(ListStage::Radius, t_vis.elapsed());
            let t_walk = Instant::now();
            // Fog of the frame (`fog_cull`): the sections it hides whole are not drawn, and under
            // `RUSTCRAFT_OPT_FOG_CULL_CUTS_WALK` the walk itself stops at a sight limit.
            let fog_paths = fog_cull::FogCullPaths::from_switches();
            let frame_draws = fog_cull::FrameDraws {
                farthest_fog_piercing_draw: self.farthest_fog_piercing_draw(local),
                improved_transparency: oit_on,
            };
            let fog_reach =
                if self.fog_cull_suspended { None } else { fog_cull::FogReach::of_frame(env, frame_draws, fog_paths) };
            let sight_limit = fog_cull::walk_sight_limit(fog_reach, fog_paths);
            // Walk redone only if the camera or the columns changed, in a background task
            // like the game: we draw with the last result to arrive (one frame of lag at
            // most); the very first is computed in place. Camera counted in cells of 8 blocks
            // like the game (not at each step: the walk and its copies cost every frame
            // while walking).
            let key: WalkKey =
                (fog_cull::cell_of(eye), self.vis_rev, self.occlusion, min_y, sections, sight_limit.to_bits(), radius);
            while let Ok(walk) = self.graph_rx.try_recv() {
                self.apply_walk(walk);
            }
            // The fog shows farther than the walk in use looked (the camera left water, an effect
            // ended): the sections it left out must be drawn on this very frame, so the walk is
            // redone on the spot, without waiting for the buffers of a walk still running.
            let widen_now = self.reached_sight_limit < sight_limit;
            let free_graph = if self.reached_key != Some(key) || widen_now {
                self.graph.take().or_else(|| widen_now.then(Graph::default))
            } else {
                None
            };
            if let Some(graph) = free_graph {
                self.reached_key = Some(key);
                let snapshot = self.vis_snapshot.current(self.vis_rev).unwrap_or_else(|| self.remake_vis_snapshot());
                let inputs = WalkInputs { eye, min_y, sections, radius, occlusion: self.occlusion, sight_limit };
                self.graph_args = Some(inputs);
                self.walk_launches += 1;
                let launch = self.walk_launches;
                let job = move || walk_task(graph, &snapshot, &inputs, launch);
                if self.reached.is_empty() || widen_now {
                    self.apply_walk(job());
                } else {
                    let tx = self.graph_tx.clone();
                    rayon::spawn(move || {
                        let _ = tx.send(job());
                    });
                }
            }
            crate::perf_log::list_stage(ListStage::WalkLaunch, t_walk.elapsed());
            // Sections reached as a set, for the entity visibility test.
            let t_set = Instant::now();
            if self.reached_set.0 != self.reached_gen {
                let reach = match self.reached_grid.take() {
                    Some(g) => lists::Reach::Grid(g),
                    None => lists::Reach::Set(self.reached.iter().copied().collect()),
                };
                self.reached_set = (self.reached_gen, min_y, sections, Arc::new(reach));
            }
            crate::perf_log::list_stage(ListStage::ReachedSet, t_set.elapsed());
            // Sections the terrain draws: the reached ones, without those the fog hides whole.
            let t_fog = Instant::now();
            self.drawn_sections.refresh(&self.reached, self.reached_gen, eye, min_y, fog_reach);
            let in_sight = self.drawn_sections.sections(&self.reached).len();
            crate::perf_log::stage_time("us_fog_cull_per_frame", t_fog.elapsed());
            crate::perf_log::counter("terrain_sections_reached_per_frame", self.reached.len() as u64);
            crate::perf_log::counter("terrain_sections_in_sight_per_frame", in_sight as u64);
            // Runs of consecutive visible sections per column (the sections are sorted by column
            // then height), in a reused buffer; each column points a range into it.
            let size = (self.config.width, self.config.height);
            let draw_key: DrawKey = (
                eye.map(f64::to_bits),
                yaw.to_bits(),
                u64::from(pitch.to_bits()) | u64::from(self.view.cull_fov.to_bits()) << 32,
                self.columns_rev,
                self.drawn_sections.generation(),
                size,
                min_y,
                sections,
            );
            let lists_changed = self.draw_key != Some(draw_key);
            let rebuilt = draw_list_stages(
                self,
                lists_changed,
                |r| r.resort_translucent_columns(eye, min_y, cam_col, oit_on),
                |r| r.rebuild_draw_lists(&planes, origin, eye, min_y, sections, t_vis),
                |resort_time| crate::perf_log::list_stage(ListStage::Resort, resort_time),
            );
            if let Some(d) = rebuilt {
                (self.drawn_cache, self.draw_key) = (d, Some(draw_key));
            }
            drawn = self.drawn_cache;
            self.count_terrain_draws();
            // Sky after the opaque terrain: drawn by the loop with the features, at the head of the
            // translucent layer, or here when the loop did not reach that layer.
            let entities_done = self.encode_terrain_batches(&mut pass, oit_on, sky_after_opaque.then_some(sky));
            if sky_after_opaque && !entities_done {
                self.draw_sky(&mut pass, sky);
            }
            if oit_on {
                self.draw_solid_features(&mut pass);
            } else {
                if !entities_done {
                    self.draw_opaque_features(&mut pass);
                }
                // Translucent particles after the translucent terrain.
                pass.set_bind_group(0, &self.bind, &[]);
                self.particles.draw_translucent(&mut pass);
                self.cracks.draw(&mut pass, true);
                self.outline.draw(&mut pass, true);
                // Debug: "always in front" traces (cube of the current section).
                self.debug_lines.draw(&mut pass, crate::debug_render::Layer::Front);
                // "See-through" labels after the translucent (and the outlines).
                self.tags.draw_see_through(&mut pass);
                self.text_displays.draw_see_through(&mut pass);
                // Clouds after the translucent terrain, then the interface.
                self.sky.draw_clouds(&mut pass);
                // Weather after the clouds (`sky.md` § 1).
                pass.set_bind_group(0, &self.bind, &[]);
                self.weather.draw(&mut pass);
                self.weather.wall.draw(&mut pass);
                // Panorama of the out-of-world screens: covers everything, under the interface.
                if let Some(p) = self.panorama.as_ref().filter(|_| self.show_panorama) {
                    p.draw(&mut pass);
                }
            }
            // Pipeline statistics of the world alone: closed before the hand.
            if let Some(t) = &self.timer {
                t.stats_end(&mut pass);
            }
            if hand_in_world_pass {
                // The hand and the interface in the depth share in front of the world's.
                full_viewport.with_depth(WORLD_DEPTH_SHARE, 1.0).set(&mut pass);
                self.encode_hand_and_interface(&mut pass, post);
            }
        }
        if oit_on {
            self.draw_oit(&mut enc, &view);
            // After the composition: see-through labels, then "always in front" traces. None of
            // them tests or writes the depth, so a pass that also holds the hand clears it (the hand
            // is in front of everything) instead of loading it.
            let holds_hand = hand_pass == HandPass::InAfterTransparencyPass;
            let (label, role) = match (holds_hand, last_pass) {
                (true, true) => ("after transparency, hand and interface", Role::Last),
                (true, false) => ("after transparency, hand and interface", Role::Inner),
                (false, _) => ("after transparency", Role::Inner),
            };
            let load = if holds_hand { wgpu::LoadOp::Clear(DEPTH_CLEAR) } else { wgpu::LoadOp::Load };
            let depth = wgpu::Operations { load, store: self.depth_paths.after_transparency_store() };
            let mut pass = self.begin_frame_pass(&mut enc, &view, label, depth, role);
            pass.set_bind_group(0, &self.bind, &[]);
            self.draw_after_oit(&mut pass);
            if let Some(p) = self.panorama.as_ref().filter(|_| self.show_panorama) {
                p.draw(&mut pass);
            }
            if holds_hand {
                self.encode_hand_and_interface(&mut pass, post);
            }
        }
        // Outline of the glowing entities: coloured silhouettes, edge and blur, then blend over
        // the world image (visible through blocks), before the hand and the interface.
        if let Some(e) = self.entities.as_ref().filter(|e| e.has_glow()) {
            self.entity_outline.ensure(&self.device, (self.config.width, self.config.height));
            if let Some(target) = self.entity_outline.silhouettes() {
                {
                    let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("glowing entity silhouettes"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: target,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        timestamp_writes: self.stamp("glowing entity silhouettes", Role::Inner),
                        ..Default::default()
                    });
                    pass.set_bind_group(0, &self.bind, &[]);
                    e.draw_glow(&mut pass);
                }
                self.entity_outline.process(&mut enc, self.timer.as_ref());
                self.entity_outline.composite(&mut enc, &view, self.timer.as_ref());
            }
        }
        // Screen effects (`miscellaneous.md` § 6.2): after the world and the hand, before the
        // screen overlays and the interface; the hand then has its own pass.
        let cleared_depth = wgpu::Operations { load: wgpu::LoadOp::Clear(DEPTH_CLEAR), store: wgpu::StoreOp::Discard };
        if post {
            if self.view.hand
                && let Some(e) = &self.entities
            {
                let mut pass = self.begin_frame_pass(&mut enc, &view, "main", cleared_depth, Role::Inner);
                pass.set_bind_group(0, &self.hand_bind, &[]);
                e.draw_hand(&mut pass);
                // Names of the held map's decorations, same camera.
                self.text_displays.draw_hand(&mut pass);
            }
            let blur = &mut self.blur;
            self.post_fx.run(
                &self.device,
                &self.queue,
                &mut self.up,
                &mut enc,
                &frame.texture,
                &view,
                &effects,
                blur,
                self.post_blur,
                self.timer.as_ref(),
            );
        }
        let world_icon_copied = world_icon.is_some();
        if hand_pass == HandPass::Own {
            // First-person hand (`hand-camera.md` § 1.2): depth cleared (the hand is
            // always in front of the world), own projection; then the interface.
            let role = if last_pass { Role::Last } else { Role::Inner };
            match world_icon {
                None => {
                    let mut pass = self.begin_frame_pass(&mut enc, &view, "hand and interface", cleared_depth, role);
                    self.encode_hand_and_interface(&mut pass, post);
                }
                Some(file) => self.encode_hand_icon_and_interface(&mut enc, &view, &frame.texture, file, post, role),
            }
        }
        // Menu background: blur of everything that precedes, then the rest of the interface (depth cleared).
        if let Some(radius) = blur_radius {
            // No image copy possible: interface without blur.
            if self.can_capture {
                let timer = self.timer.as_ref();
                self.blur.run(&self.device, &self.queue, &mut self.up, &mut enc, &frame.texture, &view, radius, timer);
            }
            let mut pass =
                self.begin_frame_pass(&mut enc, &view, "interface after the blur", cleared_depth, Role::Last);
            self.gui.draw_blur_part(&mut pass, true, |_| {});
        }
        self.drawn = drawn;
        // End of the measurement; the capture copy that follows is not part of it (intended).
        if let Some(t) = &mut self.timer {
            t.end(&mut enc);
        }
        let readback = self.encode_capture_copy(&mut enc, &frame.texture);
        crate::present_watch::during(Stage::Submit, || self.up.submit(&self.queue, enc.finish()));
        if world_icon_copied {
            self.map_world_icon();
        }
        let s = self.up.take_stats();
        crate::perf_log::gpu_writes(s.writes, s.transits, s.bytes);
        crate::perf_log::counter("gpu_copies_per_frame", s.copies);
        crate::perf_log::counter("gpu_command_buffers_per_frame", s.command_buffers);
        self.cpu_ms_sum += t_cpu.elapsed().as_secs_f64() * 1e3;
        self.cpu_frames += 1;
        if let Some(t) = &mut self.timer {
            t.after_submit();
        }
        // Read callbacks handled at the next submit (wgpu picks up finished work there).
        if !self.no_poll {
            // Watched as a GPU wait: the poll also frees the finished work's resources, a spike suspect.
            let _ = crate::present_watch::during(Stage::GpuWait, || self.device.poll(wgpu::PollType::Poll));
        }
        if let Some(pending) = readback {
            self.capture_readback(pending);
        }
        self.window.pre_present_notify();
        let acquire_next = rc_data::opt_switch!("RUSTCRAFT_OPT_ACQUIRE_BEFORE_INPUT");
        self.surface.present(&self.queue, frame, acquire_next);
        true
    }

    /// Pass over the frame image as drawn so far and the depth target (`depth`), timed as `label`.
    fn begin_frame_pass<'e>(
        &self,
        enc: &'e mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        label: &'static str,
        depth: wgpu::Operations<f32>,
        role: Role,
    ) -> wgpu::RenderPass<'e> {
        enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth,
                depth_ops: Some(depth),
                stencil_ops: None,
            }),
            timestamp_writes: self.stamp(label, role),
            ..Default::default()
        })
    }

    /// First-person hand (`hand-camera.md` § 1.2) with its own projection, unless the screen effects
    /// (`post`) drew it before them; screen effects and camera overlays, totem animation, 3D
    /// crosshair, then the interface. The depth holds nothing of the world in front of them:
    /// cleared, or the world kept in its own share behind theirs ([`HandPass::InWorldPass`]).
    fn encode_hand_and_interface(&self, pass: &mut wgpu::RenderPass<'_>, post: bool) {
        self.encode_hand_and_screen_effects(pass, post);
        self.encode_interface(pass);
    }

    /// The frame of a world icon: [`Self::encode_hand_and_interface`] cut in two passes around the
    /// copy of the image, which the game takes after the world and the hand, before the interface.
    /// The depth the hand leaves is kept for the interface (its models test it as in one pass).
    fn encode_hand_icon_and_interface(
        &mut self,
        enc: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        frame_texture: &wgpu::Texture,
        file: std::path::PathBuf,
        post: bool,
        role: Role,
    ) {
        let kept_depth = wgpu::Operations { load: wgpu::LoadOp::Clear(DEPTH_CLEAR), store: wgpu::StoreOp::Store };
        {
            let mut pass = self.begin_frame_pass(enc, view, "hand", kept_depth, Role::Inner);
            self.encode_hand_and_screen_effects(&mut pass, post);
        }
        self.encode_world_icon_copy(enc, frame_texture, file);
        let loaded_depth = wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Discard };
        let mut pass = self.begin_frame_pass(enc, view, "interface", loaded_depth, role);
        self.encode_interface(&mut pass);
    }

    /// The part of [`Self::encode_hand_and_interface`] the world image holds: hand, screen effects,
    /// totem animation, 3D crosshair.
    fn encode_hand_and_screen_effects(&self, pass: &mut wgpu::RenderPass<'_>, post: bool) {
        if self.view.hand
            && !post
            && let Some(e) = &self.entities
        {
            pass.set_bind_group(0, &self.hand_bind, &[]);
            e.draw_hand(pass);
            // Names of the held map's decorations, same camera.
            self.text_displays.draw_hand(pass);
        }
        // Screen effects over the hand (the camera overlays, between vignette and crosshair, come
        // with the interface, under the menu blur like the rest of the HUD).
        if let Some(o) = self.overlays.as_ref() {
            o.draw_screen(pass);
        }
        // Totem animation: over the screen effects (fire, water, block), fixed lights.
        if self.view.hand
            && let Some(e) = &self.entities
        {
            pass.set_bind_group(0, &self.totem_bind, &[]);
            e.draw_totem(pass);
        }
        // 3D crosshair (F3): hand camera (hand field, without sway or
        // shake: it stays centred and only turns with the camera), above the hand,
        // under the interface.
        if self.debug_lines.has(crate::debug_render::Layer::Crosshair) {
            pass.set_bind_group(0, if self.view.hand { &self.hand_bind } else { &self.bind }, &[]);
            self.debug_lines.draw(pass, crate::debug_render::Layer::Crosshair);
        }
    }

    /// The interface of [`Self::encode_hand_and_interface`]: camera overlays, then the screens and
    /// the HUD with their models.
    fn encode_interface(&self, pass: &mut wgpu::RenderPass<'_>) {
        let overlays = self.overlays.as_ref();
        let (entities, gui_bind) = (self.entities.as_ref(), &self.gui_bind);
        self.gui.draw_blur_part_models(
            pass,
            false,
            |p| {
                if let Some(o) = overlays {
                    o.draw_gui(p);
                }
            },
            |p| {
                if let Some(e) = entities {
                    p.set_bind_group(0, gui_bind, &[]);
                    e.draw_gui(p);
                }
            },
        );
    }

    /// Sky of the frame at its place in the world pass ([`encode_sky`], [`encode_sky_behind_terrain`]);
    /// group 0 holds the world camera before and after.
    pub(super) fn draw_sky(&self, pass: &mut wgpu::RenderPass<'_>, sky: WorldSky) {
        match self.sky.order() {
            SkyOrder::BeforeTerrain => encode_sky(pass, &self.sky_pipeline, &self.sky, &self.bind, sky.parts),
            SkyOrder::AfterOpaque => encode_sky_behind_terrain(pass, &self.sky_pipeline, &self.sky, &self.bind, sky),
        }
    }

    /// Distance from `eye` (frame of the world camera), in blocks, of the farthest fog-piercing
    /// draw the frame uploaded (`fog_cull`): 0 without one. The world border wall and the filled
    /// debug faces are not measured: infinite as soon as one is drawn.
    fn farthest_fog_piercing_draw(&self, eye: glam::Vec3) -> f64 {
        if !self.weather.wall.order.is_empty() || self.debug_lines.draws_filled_faces() {
            return f64::INFINITY;
        }
        let additive_entities = self.entities.as_ref().map_or(0.0, |entities| entities.farthest_additive_draw());
        f64::from(additive_entities.max(self.cracks.farthest_vertex(eye)))
    }

    /// Rebuilds the draw lists of the frame (visible columns from the farthest to the nearest, runs
    /// of sections in the field, batches and commands, `lists`) and sends the indirect commands;
    /// returns the columns and faces drawn. `t_vis`: start of the frame's visibility work.
    fn rebuild_draw_lists(
        &mut self,
        planes: &[glam::Vec4; 6],
        origin: [i32; 2],
        eye: [f64; 3],
        min_y: i32,
        sections: i32,
        t_vis: Instant,
    ) -> (usize, u64) {
        let t_lists = Instant::now();
        let input = lists::ListIn {
            columns: &self.columns,
            reached: self.drawn_sections.sections(&self.reached),
            planes,
            origin,
            eye,
            min_y,
            sections,
            face_vertices: self.shader_opts.face_vertices(),
            split_draws: self.split_draws,
            sections_generation: self.drawn_sections.generation(),
            columns_rev: self.columns_rev,
        };
        let vis_ms = &mut self.vis_ms;
        let ordered = || *vis_ms = t_vis.elapsed().as_secs_f64() * 1e3;
        // On this thread: only the slices of the parallel path go to the list pool, this thread
        // taking slices too.
        let drawn = lists::build(&input, &mut self.lists, lists::ListPaths::from_switches(), ordered);
        let (reused, rebuilt) = self.lists.column_command_reuse();
        crate::perf_log::counter("terrain_column_commands_reused_per_frame", reused);
        crate::perf_log::counter("terrain_column_commands_rebuilt_per_frame", rebuilt);
        let (batches, args) = (&self.lists.batches, &self.lists.args);
        crate::perf_log::list_stage(ListStage::Lists, t_lists.elapsed());
        let t_send = Instant::now();
        // Indexed faces: indexed commands of each batch of block faces, split;
        // only the instanced commands of the fluids remain, grouped.
        let (mut indexed_draw_args, mut fluid_draw_args) =
            (std::mem::take(&mut self.indexed_draw_args), std::mem::take(&mut self.fluid_draw_args));
        indexed_draw_args.clear();
        fluid_draw_args.clear();
        self.indexed_batch_ranges.clear();
        if self.shader_opts.faceidx {
            for &(_, fluid, _, first, count) in batches {
                let cmds = &args[first as usize..(first + count) as usize];
                if fluid {
                    self.indexed_batch_ranges.push((fluid_draw_args.len() as u32, count));
                    fluid_draw_args.extend_from_slice(cmds);
                } else {
                    let at = indexed_draw_args.len() as u32;
                    indexed_args(cmds, &mut indexed_draw_args);
                    self.indexed_batch_ranges.push((at, indexed_draw_args.len() as u32 - at));
                }
            }
        }
        // Commands sent: with `RUSTCRAFT_OPT_ARGSKIP`, only the range that differs from what the
        // buffer already holds (unless a new buffer).
        let skip = lists::args_skip();
        let instanced: &[DrawArgs] = if self.shader_opts.faceidx { &fluid_draw_args } else { args };
        if self.indirect_ok && !instanced.is_empty() {
            let bytes = std::mem::size_of_val(instanced) as u64;
            let mut fresh = !skip;
            if self.indirect.size() < bytes {
                self.indirect = self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("indirect commands"),
                    size: bytes.next_power_of_two(),
                    usage: wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                fresh = true;
            }
            let range =
                if fresh { Some(0..instanced.len()) } else { lists::changed_range(&self.sent_args, instanced) };
            if let Some(r) = range {
                let at = (r.start * std::mem::size_of::<DrawArgs>()) as u64;
                self.up.write(&self.queue, &self.indirect, at, bytemuck::cast_slice(&instanced[r]));
            }
            if skip {
                self.sent_args.clear();
                self.sent_args.extend_from_slice(instanced);
            }
        }
        if self.shader_opts.faceidx {
            let need = indexed_draw_args.iter().map(|a| a.first_instance + 1).max().unwrap_or(0);
            if need > self.face_ids.1 {
                let n = need.next_power_of_two();
                self.face_ids = (identity_buffer(&self.device, n), n);
            }
            if self.indirect_ok && !indexed_draw_args.is_empty() {
                let bytes = (indexed_draw_args.len() * std::mem::size_of::<DrawIndexedArgs>()) as u64;
                let mut fresh = !skip;
                if self.indirect_idx.size() < bytes {
                    self.indirect_idx = self.device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("indirect indexed commands"),
                        size: bytes.next_power_of_two(),
                        usage: wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                    fresh = true;
                }
                let range = if fresh {
                    Some(0..indexed_draw_args.len())
                } else {
                    lists::changed_range(&self.sent_indexed_draw_args, &indexed_draw_args)
                };
                if let Some(r) = range {
                    let at = (r.start * std::mem::size_of::<DrawIndexedArgs>()) as u64;
                    self.up.write(
                        &self.queue,
                        &self.indirect_idx,
                        at,
                        bytemuck::cast_slice(&indexed_draw_args[r]),
                    );
                }
                if skip {
                    self.sent_indexed_draw_args.clear();
                    self.sent_indexed_draw_args.extend_from_slice(&indexed_draw_args);
                }
            }
        }
        (self.indexed_draw_args, self.fluid_draw_args) = (indexed_draw_args, fluid_draw_args);
        crate::perf_log::list_stage(ListStage::CommandSubmit, t_send.elapsed());
        drawn
    }

    /// More than `n` sections of the last walk, in sight, hold something to draw: the game's count
    /// of drawn sections before it takes a world icon (here without the field-of-view test). Stops
    /// at the first section past `n`; read at most once a second until the icon exists.
    pub fn drawn_sections_exceed(&self, n: usize) -> bool {
        let mut count = 0;
        let mut column: Option<((i32, i32), rc_mesh::wire::Sections)> = None;
        // Sorted by column then height: each column's drawable sections are read once.
        for &(x, sy, z) in self.drawn_sections.sections(&self.reached) {
            let drawable = match column {
                Some((key, d)) if key == (x, z) => d,
                _ => {
                    let d = self.columns.get(&(x, z)).map(lists::drawable_sections).unwrap_or_default();
                    column = Some(((x, z), d));
                    d
                }
            };
            if usize::try_from(sy).is_ok_and(|s| drawable.contains(s)) {
                count += 1;
                if count > n {
                    return true;
                }
            }
        }
        false
    }

    /// Terrain batches and indirect commands drawn by this frame (perf log).
    fn count_terrain_draws(&self) {
        let commands = if self.shader_opts.faceidx {
            self.indexed_draw_args.len() + self.fluid_draw_args.len()
        } else {
            self.lists.args.len()
        };
        crate::perf_log::counter("terrain_batches_per_frame", self.lists.batches.len() as u64);
        crate::perf_log::counter("terrain_commands_per_frame", commands as u64);
    }

    /// Connectivity snapshot of the current columns for a walk task (`walk_inputs::VisSnapshotStore`).
    fn remake_vis_snapshot(&mut self) -> VisSnapshot {
        let started = Instant::now();
        let (columns, empty, unmeshed) = (&self.columns, &self.empty_vis, &self.unmeshed);
        let connectivity = |k| walk_connectivity(columns, empty, unmeshed, k).cloned();
        // Every loaded column once, by the same precedence as `walk_connectivity`: a mesh first.
        let all = || {
            let mut all: hashbrown::HashMap<_, _> = unmeshed.iter().map(|(&k, v)| (k, v.clone())).collect();
            all.extend(empty.iter().map(|(&k, v)| (k, v.clone())));
            all.extend(columns.iter().map(|(&k, c)| (k, c.vis.clone())));
            all
        };
        let snapshot = self.vis_snapshot.remake(self.vis_rev, connectivity, all);
        crate::perf_log::stage_time("us_visibility_snapshot_per_frame", started.elapsed());
        if crate::remesh::checking() && *snapshot != all() {
            eprintln!("[meshcheck] visibility snapshot patched by the uploads differs from one made again");
        }
        snapshot
    }

    /// Takes the result of a walk into use, unless a later walk already is: a walk redone on the
    /// spot for a longer sight limit overtakes the one still running, which then only gives its
    /// buffers back.
    fn apply_walk(&mut self, walk: WalkResult) {
        if walk.launch < self.applied_walk_launch {
            self.graph.get_or_insert(walk.graph);
            return;
        }
        self.applied_walk_launch = walk.launch;
        self.graph = Some(walk.graph);
        self.reached = walk.reached;
        self.mark_reached_sections_compiled(walk.sections);
        self.reached_grid = walk.grid;
        self.reached_columns = walk.columns;
        self.reached_sight_limit = walk.sight_limit;
        self.reached_gen += 1;
        self.graph_ms = walk.ms;
        self.keep_occ_nodes();
    }

    /// The square of compiled sections follows the camera, as the game moves it at the start of its
    /// drawing (`compiled_sections.rs`); the shared history is only written when the square moved.
    /// The terrain's fade of the columns reset is forgotten with them, as the game's.
    fn reposition_compiled_sections(&mut self, square: ColumnSquare) {
        if self.compiled_sections.square() != Some(square) {
            let fade_ring = &mut self.fade_ring;
            Arc::make_mut(&mut self.compiled_sections).reposition(square, |key| fade_ring.forget(key));
        }
    }

    /// The sections the walk in use reached become compiled, as the game's walk makes them
    /// (`compiled_sections.rs`); columns `sections` high. The meshed columns reached start their
    /// terrain fade then if they have none.
    fn mark_reached_sections_compiled(&mut self, sections: i32) {
        let started = Instant::now();
        let (columns, empty) = (&self.columns, &self.empty_vis);
        let meshed = |k| columns.contains_key(&k) || empty.contains_key(&k);
        let compiled = Arc::make_mut(&mut self.compiled_sections);
        let fade_ring = &mut self.fade_ring;
        let unmeshed = self.unmeshed.as_map();
        let birth = |key| fade_ring.birth(key, started);
        compiled.mark_reached(&self.reached, sections.max(0) as usize, meshed, unmeshed, started, birth);
        crate::perf_log::stage_time("us_compiled_sections_per_frame", started.elapsed());
    }
}

/// Depth share of the world when the hand and the interface follow in the same pass
/// ([`HandPass::InWorldPass`]): the world in [0, 0.25], the hand and the interface in [0.25, 1]. A
/// power of two: the world's depths scaled by it keep every bit, and so do their tests and biases.
const WORLD_DEPTH_SHARE: f32 = 0.25;

/// Passes merged into the pass before them (switches read once).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PassMerges {
    /// `RUSTCRAFT_OPT_DEPTH_RANGE_PASS_MERGE`: without improved transparency, the hand and the
    /// interface at the end of the world pass.
    hand_into_world: bool,
    /// `RUSTCRAFT_OPT_AFTER_TRANSPARENCY_PASS_MERGE`: with improved transparency, the hand and the
    /// interface at the end of the pass after the transparency.
    hand_into_after_transparency: bool,
}

impl PassMerges {
    fn from_switches() -> Self {
        PassMerges {
            hand_into_world: rc_data::opt_switch!("RUSTCRAFT_OPT_DEPTH_RANGE_PASS_MERGE"),
            hand_into_after_transparency: rc_data::opt_switch!("RUSTCRAFT_OPT_AFTER_TRANSPARENCY_PASS_MERGE"),
        }
    }
}

/// The pass that draws the hand and the interface in a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HandPass {
    /// Their own pass, on a cleared depth.
    Own,
    /// The end of the world pass: the world drawn in the depth share [0, `WORLD_DEPTH_SHARE`], the
    /// hand and the interface in the share in front of it, the same relation as a cleared depth
    /// (one transformation for both, so the hand still hides the interface models where it did).
    InWorldPass,
    /// The end of the pass after the transparency, which then clears the depth (what it draws
    /// before them neither tests nor writes it).
    InAfterTransparencyPass,
}

impl HandPass {
    /// The glowing outlines and the screen effects come between the world and the hand: their own
    /// pass then; improved transparency reads the world's depth as it is, so its frames merge with
    /// the pass after the transparency only.
    fn of_frame(merges: PassMerges, improved_transparency: bool, glow: bool, screen_effects: bool) -> HandPass {
        if glow || screen_effects {
            return HandPass::Own;
        }
        match improved_transparency {
            false if merges.hand_into_world => HandPass::InWorldPass,
            true if merges.hand_into_after_transparency => HandPass::InAfterTransparencyPass,
            _ => HandPass::Own,
        }
    }
}

/// Parts of the sky a frame draws.
#[derive(Clone, Copy, Debug)]
pub(super) struct SkyParts {
    dome: bool,
    dark_disc: bool,
    end: bool,
}

impl SkyParts {
    fn of_frame(env: &FrameEnv) -> Self {
        SkyParts { dome: env.dome, dark_disc: env.disc, end: env.bodies.end.is_some() }
    }
}

/// Viewport of a pass over its whole target, with a depth range.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PassViewport {
    size: (u32, u32),
    depth: (f32, f32),
}

impl PassViewport {
    /// The whole target and the whole depth range: the viewport a pass starts with.
    pub(super) fn full(size: (u32, u32)) -> Self {
        PassViewport { size, depth: (0.0, 1.0) }
    }

    fn with_depth(self, min: f32, max: f32) -> Self {
        PassViewport { depth: (min, max), ..self }
    }

    fn set(self, pass: &mut wgpu::RenderPass<'_>) {
        let (w, h) = (self.size.0 as f32, self.size.1 as f32);
        pass.set_viewport(0.0, 0.0, w, h, self.depth.0, self.depth.1);
    }
}

/// The sky of a frame and the viewport of the world pass around it.
#[derive(Clone, Copy, Debug)]
pub(super) struct WorldSky {
    parts: SkyParts,
    world_viewport: PassViewport,
}

/// Sky drawn after the opaque terrain ([`SkyOrder::AfterOpaque`]). Through a viewport whose depth
/// range is [0, 0], every sky fragment gets the cleared depth, the farthest: the depth test keeps
/// the sky only where no terrain was drawn and drops the rest before it is shaded. The clipping
/// still uses the vertices' own depth, so the far plane of a short render distance cuts the sky as
/// before and every pixel comes out as when the sky was drawn first. (Vertices put on the far
/// plane by the shader, with the cut redone per fragment, moved the clipping: 61 pixels of the End
/// view of the proof changed.) The world pass's viewport comes back after.
fn encode_sky_behind_terrain(
    pass: &mut wgpu::RenderPass<'_>,
    plain_sky: &wgpu::RenderPipeline,
    sky: &crate::sky::Sky,
    camera: &wgpu::BindGroup,
    frame_sky: WorldSky,
) {
    frame_sky.world_viewport.with_depth(DEPTH_CLEAR, DEPTH_CLEAR).set(pass);
    encode_sky(pass, plain_sky, sky, camera, frame_sky.parts);
    frame_sky.world_viewport.set(pass);
}

/// Sky (docs/specs/sky.md § 1): dome, glow, sun, moon, stars, then the dark disc below the horizon,
/// then the End sky. `plain_sky`: pipeline of the dome and the disc; `camera`: group 0 of the world
/// camera, bound before the call and again after it.
fn encode_sky(
    pass: &mut wgpu::RenderPass<'_>,
    plain_sky: &wgpu::RenderPipeline,
    sky: &crate::sky::Sky,
    camera: &wgpu::BindGroup,
    parts: SkyParts,
) {
    if parts.dome {
        pass.set_pipeline(plain_sky);
        pass.draw(0..24, 0..1);
        sky.draw_bodies(pass);
        pass.set_bind_group(0, camera, &[]);
    }
    if parts.dark_disc {
        pass.set_pipeline(plain_sky);
        pass.draw(0..24, 1..2);
    }
    if parts.end {
        sky.draw_end(pass);
        pass.set_bind_group(0, camera, &[]);
    }
}

/// A visibility walk over the columns of `connectivity` and what the frame reads from it: the
/// work of the background task.
fn walk_task(mut graph: Graph, connectivity: &VisSnapshot, inputs: &WalkInputs, launch: u64) -> WalkResult {
    let started = Instant::now();
    let WalkInputs { eye, min_y, sections, radius, occlusion, sight_limit } = *inputs;
    let column = |x, z| connectivity.get(&(x, z)).map(|v| &v[..]);
    let mut reached = graph.run_within(column, eye, min_y, sections, radius, occlusion, sight_limit).to_vec();
    // Sorted by column then height: the frame draws its series from them in one pass.
    reached.sort_unstable_by_key(|&(x, sy, z)| (x, z, sy));
    // Grid of the sections reached for the entity visibility, made here rather than on the
    // main thread.
    let grid = lists::reach_grid().then(|| lists::ReachGrid::new(&reached));
    // Run of each column, for the meshing's questions about single columns.
    let columns = frustum::reached_column_index().then(|| frustum::ReachedColumns::new(&reached));
    crate::perf_log::graph_work(started.elapsed());
    let ms = started.elapsed().as_secs_f64() * 1e3;
    WalkResult { graph, reached, ms, grid, columns, sight_limit, launch, sections }
}

/// Draw-list work of a frame: the translucent re-sort has its own rotation and runs every frame,
/// so its time is handed to `record_resort` on every frame; the lists are only rebuilt (`rebuild`)
/// when they changed. Returns what `rebuild` gave, if it ran. Generic over the renderer for the test.
fn draw_list_stages<R, T>(
    renderer: &mut R,
    lists_changed: bool,
    resort: impl FnOnce(&mut R),
    rebuild: impl FnOnce(&mut R) -> T,
    record_resort: impl FnOnce(std::time::Duration),
) -> Option<T> {
    let started = Instant::now();
    resort(renderer);
    record_resort(started.elapsed());
    lists_changed.then(|| rebuild(renderer))
}

#[cfg(test)]
#[path = "sky_order_test.rs"]
mod sky_order_tests;

#[cfg(test)]
#[path = "hand_pass_test.rs"]
mod hand_pass_tests;

#[cfg(test)]
mod tests {
    use super::*;

    /// The re-sort is timed on a frame that keeps its lists (it used to be recorded only on the
    /// frames that rebuilt them); the rebuild does not run then.
    #[test]
    fn translucent_resort_is_timed_on_frames_that_keep_their_lists() {
        let (mut rebuilt, mut resorted, mut recorded) = (false, 0, 0);
        let out = draw_list_stages(&mut rebuilt, false, |_| resorted += 1, |r| *r = true, |_| recorded += 1);
        assert!(out.is_none() && !rebuilt, "the lists must not be rebuilt when they did not change");
        assert_eq!((resorted, recorded), (1, 1), "re-sort of a frame without rebuild not run or not recorded");
    }

    /// A frame whose lists changed rebuilds them and returns what the rebuild gave.
    #[test]
    fn changed_lists_are_rebuilt() {
        let (mut rebuilt, mut recorded) = (0, 0);
        let rebuild = |r: &mut i32| {
            *r += 1;
            7
        };
        let out = draw_list_stages(&mut rebuilt, true, |_| {}, rebuild, |_| recorded += 1);
        assert_eq!((out, rebuilt, recorded), (Some(7), 1, 1));
    }

    const MERGE_ALL: PassMerges = PassMerges { hand_into_world: true, hand_into_after_transparency: true };
    const MERGE_NONE: PassMerges = PassMerges { hand_into_world: false, hand_into_after_transparency: false };

    /// The hand and the interface join the pass before them only when nothing comes between: the
    /// world pass without improved transparency, the pass after the transparency with it; their own
    /// pass with glowing outlines or screen effects, and with the switches off.
    #[test]
    fn hand_joins_the_pass_before_it_only_when_nothing_comes_between() {
        use HandPass::*;
        for (oit, glow, effects, merged) in [
            (false, false, false, InWorldPass),
            (true, false, false, InAfterTransparencyPass),
            (false, true, false, Own),
            (false, false, true, Own),
            (true, true, false, Own),
            (true, false, true, Own),
        ] {
            let case = format!("oit {oit}, glow {glow}, effects {effects}");
            assert_eq!(HandPass::of_frame(MERGE_ALL, oit, glow, effects), merged, "{case}");
            assert_eq!(HandPass::of_frame(MERGE_NONE, oit, glow, effects), Own, "{case}, switches off");
        }
        let world_only = PassMerges { hand_into_after_transparency: false, ..MERGE_ALL };
        assert_eq!(HandPass::of_frame(world_only, true, false, false), Own);
        let transparency_only = PassMerges { hand_into_world: false, ..MERGE_ALL };
        assert_eq!(HandPass::of_frame(transparency_only, false, false, false), Own);
    }

    /// The pass merges, the sky after the opaque terrain and the depth paths are on unless set to 0.
    #[test]
    fn new_pass_switches_are_on_by_default() {
        let merges = PassMerges::from_switches();
        assert!(merges.hand_into_world || rc_data::env_flag!("RUSTCRAFT_OPT_DEPTH_RANGE_PASS_MERGE"));
        let after_transparency = rc_data::env_flag!("RUSTCRAFT_OPT_AFTER_TRANSPARENCY_PASS_MERGE");
        assert!(merges.hand_into_after_transparency || after_transparency);
        let sky_set = rc_data::env_flag!("RUSTCRAFT_OPT_SKY_AFTER_OPAQUE");
        assert!(SkyOrder::from_switch() == SkyOrder::AfterOpaque || sky_set);
        let depth = super::super::surface::DepthPaths::from_switches();
        assert!(depth.discard_unread || rc_data::env_flag!("RUSTCRAFT_OPT_DEPTH_DISCARD"));
        assert!(depth.sampled_only_for_transparency || rc_data::env_flag!("RUSTCRAFT_OPT_DEPTH_NO_SAMPLING"));
    }

    /// A shader that writes its own depth escapes the viewport's depth range: the hand drawn in the
    /// world pass's share in front of the world, and the sky put at the farthest depth, rely on no
    /// shader of these passes doing so. Only the passes of improved transparency write it.
    #[test]
    fn only_the_transparency_passes_write_their_own_depth() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut writers = Vec::new();
        let mut todo = vec![dir];
        while let Some(dir) = todo.pop() {
            for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
                let path = entry.expect("directory entry").path();
                if path.is_dir() {
                    todo.push(path);
                } else if path.extension().is_some_and(|ext| ext == "wgsl")
                    && std::fs::read_to_string(&path).expect("shader").contains("frag_depth")
                {
                    writers.push(path.file_name().expect("file name").to_string_lossy().into_owned());
                }
            }
        }
        assert_eq!(writers, ["oit_passes.wgsl"], "a shader writes its own depth: judge it against the depth shares");
    }

    /// The world's depth share is a power of two: its depths scale without losing a bit.
    #[test]
    fn world_depth_share_scales_depths_exactly() {
        assert_eq!(WORLD_DEPTH_SHARE.to_bits() & 0x007f_ffff, 0, "not a power of two");
        for depth in [1.0f32, 0.5, 0.123_456_79, 1e-4, 3.3e-7] {
            assert_eq!((depth * WORLD_DEPTH_SHARE) / WORLD_DEPTH_SHARE, depth);
        }
    }
}
