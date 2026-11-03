//! One frame of the renderer: camera uniforms, the uploads of every layer, the passes in order (world,
//! improved transparency, glowing outlines, screen effects, hand and interface, menu blur), submission
//! and presentation.

use std::sync::Arc;
use std::time::Instant;

use super::capture::surface_state;
use super::frustum::frustum_planes;
use super::regions::identity_buffer;
use super::terrain_draw::indexed_args;
use super::{
    CAM_GUI, CAM_HAND, CAM_TOTEM, CAM_WORLD, CameraUniform, DEPTH_CLEAR, DrawArgs, DrawIndexedArgs, DrawKey, FrameEnv,
    Renderer, VIEW_LIMIT, VisSnapshot, lists, perspective,
};
use crate::gpu_time::Role;
use crate::present_watch::Stage;

impl Renderer {
    /// Draws and presents a frame; `false` if the surface provided nothing (hidden window,
    /// timeout, reconfiguration): the caller then counts no displayed frame.
    pub fn render(&mut self, eye: [f64; 3], yaw: f32, pitch: f32, min_y: i32, height: i32, env: &FrameEnv) -> bool {
        if let Some(t) = &mut self.timer {
            t.collect();
        }
        let frame = match crate::present_watch::during(Stage::Acquire, || self.surface.get_current_texture()) {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => {
                if std::mem::take(&mut self.warned_surface) {
                    eprintln!("presentation resumed: the surface provides frames again");
                }
                f
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                crate::present_watch::during(Stage::Configure, || self.surface.configure(&self.device, &self.config));
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
            fog_sky: [env.fog.sky_end, env.fog.cloud_end, 0.0, 0.0],
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
            p.prepare(&self.queue, (self.config.width, self.config.height));
        }
        // Improved transparency: texts stored (solids or stages) as soon as the frame is uploaded.
        let oit_on = self.improved_transparency && self.oit_ok;
        self.tags.oit = oit_on;
        self.text_displays.oit = oit_on;
        self.gui.upload(&self.device, &self.queue, &mut self.up);
        if let Some(e) = &mut self.entities {
            e.upload(&self.device, &self.queue);
        }
        let gui = &self.gui;
        self.tags.upload(&self.device, &self.queue, &mut self.up, gui.glyph_gen, || gui.glyph_view());
        self.block_text.upload(&self.device, &self.queue, &self.gui);
        self.text_displays.upload(&self.device, &self.queue, &self.gui);
        if let Some(e) = &self.entities {
            self.block_fx.upload(&self.device, &self.queue, &e.blocks.fx);
        }
        self.outline.upload(&self.device, &self.queue, (self.config.width, self.config.height));
        self.cracks.upload(&self.device, &self.queue);
        self.leash.upload(&self.device, &self.queue);
        self.debug_lines.upload(&self.device, &self.queue, (self.config.width, self.config.height), &self.gui);
        self.particles.upload(&self.device, &self.queue);
        self.weather.upload(&self.device, &self.queue);
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
        // Improved transparency: targets and pipelines prepared as needed, targets freed otherwise.
        if oit_on {
            self.prepare_oit(proj);
        } else if let Some(o) = &mut self.oit {
            o.release();
        }
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
        self.gui.encode_items(&self.queue, &mut enc, self.timer.as_ref());
        let drawn: (usize, u64);
        {
            let timestamp_writes = self.stamp("world", Role::World);
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
                        store: wgpu::StoreOp::Store,
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
            // Sky before the terrain: dome, glow, sun, moon, stars, then dark disc
            // below the horizon (docs/specs/sky.md § 1).
            if env.dome {
                pass.set_pipeline(&self.sky_pipeline);
                pass.draw(0..24, 0..1);
                self.sky.draw_bodies(&mut pass);
                pass.set_bind_group(0, &self.bind, &[]);
            }
            if env.disc {
                pass.set_pipeline(&self.sky_pipeline);
                pass.draw(0..24, 1..2);
            }
            if env.bodies.end.is_some() {
                self.sky.draw_end(&mut pass);
                pass.set_bind_group(0, &self.bind, &[]);
            }
            // Sections reached from the camera (occlusion sorting, docs/specs/visibility.md),
            // kept if their box (one block of margin: a model may overflow) is in the field.
            let t_vis = Instant::now();
            let sections = height / 16;
            let cam_col = (eye[0].floor() as i32 >> 4, eye[2].floor() as i32 >> 4);
            // Radius: farthest loaded column (Chebyshev); only changes with the columns
            // or the camera's column (`RUSTCRAFT_OPT_VISORDER`: kept in cache).
            let rkey = (self.columns_rev, cam_col);
            let radius = match self.radius_cache {
                Some((k, r)) if k == rkey && lists::vis_order() => r,
                _ => {
                    let r = self
                        .columns
                        .keys()
                        .chain(self.empty_vis.keys())
                        .map(|&(x, z)| (x - cam_col.0).abs().max((z - cam_col.1).abs()))
                        .max()
                        .unwrap_or(0)
                        .min(VIEW_LIMIT);
                    self.radius_cache = Some((rkey, r));
                    r
                }
            };
            crate::perf_log::list_stage(0, t_vis.elapsed());
            // Walk redone only if the camera or the columns changed, in a background task
            // like the game: we draw with the last result to arrive (one frame of lag at
            // most); the very first is computed in place. Camera counted in cells of 8 blocks
            // like the game (not at each step: the walk and its copies cost every frame
            // while walking).
            let key = (eye.map(|v| (v / 8.0).floor() as i64), self.vis_rev, self.occlusion, min_y, sections);
            while let Ok((graph, reached, ms, grid)) = self.graph_rx.try_recv() {
                self.graph = Some(graph);
                self.reached = reached;
                self.reached_grid = grid;
                self.reached_gen += 1;
                self.graph_ms = ms;
                self.keep_occ_nodes();
            }
            if self.reached_key != Some(key)
                && let Some(mut graph) = self.graph.take()
            {
                self.reached_key = Some(key);
                let snapshot = match &self.vis_snapshot {
                    Some((rev, s)) if *rev == self.vis_rev => s.clone(),
                    _ => {
                        let cols = self.columns.iter().map(|(&k, c)| (k, c.vis.clone()));
                        let s: VisSnapshot =
                            Arc::new(cols.chain(self.empty_vis.iter().map(|(&k, v)| (k, v.clone()))).collect());
                        self.vis_snapshot = Some((self.vis_rev, s.clone()));
                        s
                    }
                };
                let occlusion = self.occlusion;
                self.graph_args = Some((eye, min_y, sections, radius, occlusion));
                let job = move || {
                    let t = Instant::now();
                    let mut r = graph
                        .run(|x, z| snapshot.get(&(x, z)).map(|v| &v[..]), eye, min_y, sections, radius, occlusion)
                        .to_vec();
                    // Sorted by column then height: the frame draws its series from them in one pass.
                    r.sort_unstable_by_key(|&(x, sy, z)| (x, z, sy));
                    // Grid of the sections reached for the entity visibility, made here
                    // rather than on the main thread.
                    let grid = lists::reach_grid().then(|| lists::ReachGrid::new(&r));
                    crate::perf_log::graph_work(t.elapsed());
                    (graph, r, t.elapsed().as_secs_f64() * 1e3, grid)
                };
                if self.reached.is_empty() {
                    let (g, r, ms, grid) = job();
                    (self.graph, self.reached, self.graph_ms, self.reached_grid) = (Some(g), r, ms, grid);
                    self.reached_gen += 1;
                    self.keep_occ_nodes();
                } else {
                    let tx = self.graph_tx.clone();
                    rayon::spawn(move || {
                        let _ = tx.send(job());
                    });
                }
            }
            // Sections reached as a set, for the entity visibility test.
            let t_set = Instant::now();
            if self.reached_set.0 != self.reached_gen {
                let reach = match self.reached_grid.take() {
                    Some(g) => lists::Reach::Grid(g),
                    None => lists::Reach::Set(self.reached.iter().copied().collect()),
                };
                self.reached_set = (self.reached_gen, min_y, sections, Arc::new(reach));
            }
            crate::perf_log::list_stage(1, t_set.elapsed());
            let t_sort = Instant::now();
            self.resort_translucent_columns(eye, min_y, cam_col, oit_on);
            // Runs of consecutive visible sections per column (`reached` is sorted by column
            // then height), in a reused buffer; each column points a range into it.
            let size = (self.config.width, self.config.height);
            let dkey: DrawKey = (
                eye.map(f64::to_bits),
                yaw.to_bits(),
                u64::from(pitch.to_bits()) | u64::from(self.view.cull_fov.to_bits()) << 32,
                self.columns_rev,
                self.reached_gen,
                size,
                min_y,
                sections,
            );
            if self.draw_key != Some(dkey) {
                crate::perf_log::list_stage(2, t_sort.elapsed());
                let t_lists = Instant::now();
                // Visible columns from the farthest to the nearest, runs of sections in the
                // field, batches and commands (`lists`).
                let inp = lists::ListIn {
                    columns: &self.columns,
                    reached: &self.reached,
                    planes: &planes,
                    origin,
                    eye,
                    min_y,
                    sections,
                    face_vertices: self.shader_opts.face_vertices(),
                    split_draws: self.split_draws,
                    reached_gen: self.reached_gen,
                    columns_rev: self.columns_rev,
                };
                let vis_ms = &mut self.vis_ms;
                drawn = lists::build(&inp, &mut self.lists, lists::vis_order(), || {
                    *vis_ms = t_vis.elapsed().as_secs_f64() * 1e3;
                });
                let (batches, args) = (&self.lists.batches, &self.lists.args);
                crate::perf_log::list_stage(3, t_lists.elapsed());
                let t_send = Instant::now();
                // Indexed faces: indexed commands of each batch of block faces, split;
                // only the instanced commands of the fluids remain, grouped.
                let (mut iargs, mut fargs) = (std::mem::take(&mut self.iargs), std::mem::take(&mut self.fargs));
                iargs.clear();
                fargs.clear();
                self.ibatches.clear();
                if self.shader_opts.faceidx {
                    for &(_, fluid, _, first, count) in batches {
                        let cmds = &args[first as usize..(first + count) as usize];
                        if fluid {
                            self.ibatches.push((fargs.len() as u32, count));
                            fargs.extend_from_slice(cmds);
                        } else {
                            let at = iargs.len() as u32;
                            indexed_args(cmds, &mut iargs);
                            self.ibatches.push((at, iargs.len() as u32 - at));
                        }
                    }
                }
                // Commands sent: with `RUSTCRAFT_OPT_ARGSKIP`, only the range that differs from what the
                // buffer already holds (unless a new buffer).
                let skip = lists::args_skip();
                let instanced: &[DrawArgs] = if self.shader_opts.faceidx { &fargs } else { args };
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
                    let need = iargs.iter().map(|a| a.first_instance + 1).max().unwrap_or(0);
                    if need > self.face_ids.1 {
                        let n = need.next_power_of_two();
                        self.face_ids = (identity_buffer(&self.device, n), n);
                    }
                    if self.indirect_ok && !iargs.is_empty() {
                        let bytes = (iargs.len() * std::mem::size_of::<DrawIndexedArgs>()) as u64;
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
                        let range = if fresh { Some(0..iargs.len()) } else { lists::changed_range(&self.sent_iargs, &iargs) };
                        if let Some(r) = range {
                            let at = (r.start * std::mem::size_of::<DrawIndexedArgs>()) as u64;
                            self.up.write(&self.queue, &self.indirect_idx, at, bytemuck::cast_slice(&iargs[r]));
                        }
                        if skip {
                            self.sent_iargs.clear();
                            self.sent_iargs.extend_from_slice(&iargs);
                        }
                    }
                }
                (self.iargs, self.fargs) = (iargs, fargs);
                crate::perf_log::list_stage(4, t_send.elapsed());
                (self.drawn_cache, self.draw_key) = (drawn, Some(dkey));
            } else {
                drawn = self.drawn_cache;
            }
            let entities_done = self.encode_terrain_batches(&mut pass, oit_on);
            if oit_on {
                self.draw_solid_features(&mut pass);
            } else {
                if !entities_done {
                    self.cracks.draw(&mut pass, false);
                }
                if !entities_done && let Some(e) = &self.entities {
                    e.draw(&mut pass);
                    pass.set_bind_group(0, &self.bind, &[]);
                    self.leash.draw(&mut pass);
                    self.tags.draw_normal(&mut pass);
                    self.block_text.draw(&mut pass);
                    self.text_displays.draw_offset(&mut pass);
                    self.block_fx.draw(&mut pass);
                }
                if !entities_done {
                    self.outline.draw(&mut pass, false);
                    pass.set_bind_group(0, &self.bind, &[]);
                    self.particles.draw_opaque(&mut pass);
                    self.debug_lines.draw(&mut pass, crate::debug_render::Layer::Depth);
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
            if let Some(t) = &self.timer {
                t.stats_end(&mut pass);
            }
        }
        if oit_on {
            self.draw_oit(&mut enc, &view);
            // After the composition: see-through labels, then "always in front" traces.
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("after transparency"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: self.stamp("after transparency", Role::Inner),
                ..Default::default()
            });
            pass.set_bind_group(0, &self.bind, &[]);
            self.draw_after_oit(&mut pass);
            if let Some(p) = self.panorama.as_ref().filter(|_| self.show_panorama) {
                p.draw(&mut pass);
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
        let effects = std::mem::take(&mut self.post_effects);
        let post = !effects.is_empty() && self.can_capture;
        if post {
            if self.view.hand
                && let Some(e) = &self.entities
            {
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("main"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &self.depth,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(DEPTH_CLEAR),
                            store: wgpu::StoreOp::Discard,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: self.stamp("main", Role::Inner),
                    ..Default::default()
                });
                pass.set_bind_group(0, &self.hand_bind, &[]);
                e.draw_hand(&mut pass);
                // Names of the held map's decorations, same camera.
                self.text_displays.draw_hand(&mut pass);
            }
            let blur = &mut self.blur;
            self.post_fx.run(
                &self.device,
                &self.queue,
                &mut enc,
                &frame.texture,
                &view,
                &effects,
                blur,
                self.post_blur,
                self.timer.as_ref(),
            );
        }
        {
            // First-person hand (`hand-camera.md` § 1.2): depth cleared (the hand is
            // always in front of the world), own projection; then the interface.
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hand and interface"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(DEPTH_CLEAR),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: self.stamp("hand and interface", if blur_radius.is_none() { Role::Last } else { Role::Inner }),
                ..Default::default()
            });
            if self.view.hand
                && !post
                && let Some(e) = &self.entities
            {
                pass.set_bind_group(0, &self.hand_bind, &[]);
                e.draw_hand(&mut pass);
                // Names of the held map's decorations, same camera.
                self.text_displays.draw_hand(&mut pass);
            }
            // Screen effects over the hand, camera overlays between vignette and crosshair
            // (under the menu blur, like the rest of the HUD).
            let overlays = self.overlays.as_ref();
            if let Some(o) = overlays {
                o.draw_screen(&mut pass);
            }
            // Totem animation: over the screen effects (fire, water, block), fixed lights.
            if self.view.hand
                && let Some(e) = &self.entities
            {
                pass.set_bind_group(0, &self.totem_bind, &[]);
                e.draw_totem(&mut pass);
            }
            // 3D crosshair (F3): hand camera (hand field, without sway or
            // shake: it stays centred and only turns with the camera), above the hand,
            // under the interface.
            if self.debug_lines.has(crate::debug_render::Layer::Crosshair) {
                pass.set_bind_group(0, if self.view.hand { &self.hand_bind } else { &self.bind }, &[]);
                self.debug_lines.draw(&mut pass, crate::debug_render::Layer::Crosshair);
            }
            let (entities, gui_bind) = (self.entities.as_ref(), &self.gui_bind);
            self.gui.draw_blur_part_models(
                &mut pass,
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
        // Menu background: blur of everything that precedes, then the rest of the interface (depth cleared).
        if let Some(radius) = blur_radius {
            // No image copy possible: interface without blur.
            if self.can_capture {
                let timer = self.timer.as_ref();
                self.blur.run(&self.device, &self.queue, &mut enc, &frame.texture, &view, radius, timer);
            }
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("interface after the blur"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(DEPTH_CLEAR),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: self.stamp("interface after the blur", Role::Last),
                ..Default::default()
            });
            self.gui.draw_blur_part(&mut pass, true, |_| {});
        }
        self.drawn = drawn;
        // End of the measurement; the capture copy that follows is not part of it (intended).
        if let Some(t) = &mut self.timer {
            t.end(&mut enc);
        }
        let readback = self.encode_capture_copy(&mut enc, &frame.texture);
        crate::present_watch::during(Stage::Submit, || self.up.submit(&self.queue, enc.finish()));
        let s = self.up.take_stats();
        crate::perf_log::gpu_writes(s.writes, s.transits, s.bytes);
        self.cpu_ms_sum += t_cpu.elapsed().as_secs_f64() * 1e3;
        self.cpu_frames += 1;
        if let Some(t) = &mut self.timer {
            t.after_submit();
        }
        // Read callbacks handled at the next submit (wgpu picks up finished work there).
        if !self.no_poll {
            let _ = self.device.poll(wgpu::PollType::Poll);
        }
        if let Some(pending) = readback {
            self.capture_readback(pending);
        }
        self.window.pre_present_notify();
        crate::present_watch::during(Stage::Present, || self.queue.present(frame));
        true
    }
}
