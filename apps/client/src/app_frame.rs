//! The per-frame work of the application: free flight, the frame itself and the texture animation.

use std::time::Instant;

use crate::app_entity_sets::{fill_ghost_set, fill_glow_colors, name_tags, refresh_player_sets};
use crate::grid_world::draws_local_player;
use crate::{
    App, FIRST_DRAWN, alloc_trace, camera, capseq, debug_render, debug_switcher, entity_render, environ, first_frame,
    front, gui, hand, hud, hud_more, ime, inv_effects, item_special, keys, leash, light_engine, links, menus, misc,
    nametag, op_screens, options, panorama, particles, pause, perf_log, pick, profiler, progress_app, recipe_book,
    remesh, special, spectator, sprite_anim, stack, subtitles, tablist, tech_blocks, ticking, totem_anim, widgets,
    win_screen, world_list,
};

#[cfg(feature = "memstats")]
use crate::memstats;

impl App {
    fn fly(&mut self, dt: f64) {
        let speed = if self.binds.down(keys::SPRINT) { 60.0 } else { 15.0 } * dt;
        let (s, c) = (f64::from(self.yaw.to_radians()).sin(), f64::from(self.yaw.to_radians()).cos());
        let (fwd, right) = ([-s, c], [-c, -s]);
        let mut d = [0.0f64; 3];
        let pressed = |i: usize| self.binds.down(i);
        if pressed(keys::FORWARD) {
            d[0] += fwd[0];
            d[2] += fwd[1];
        }
        if pressed(keys::BACK) {
            d[0] -= fwd[0];
            d[2] -= fwd[1];
        }
        if pressed(keys::RIGHT) {
            d[0] += right[0];
            d[2] += right[1];
        }
        if pressed(keys::LEFT) {
            d[0] -= right[0];
            d[2] -= right[1];
        }
        if pressed(keys::JUMP) {
            d[1] += 1.0;
        }
        if pressed(keys::SNEAK) {
            d[1] -= 1.0;
        }
        for (e, d) in self.eye.iter_mut().zip(d) {
            *e += d * speed;
        }
    }

    pub(crate) fn frame(&mut self) {
        // Only what arrived before the frame: a flood of chunks does not delay it.
        profiler::push("scheduledPacketProcessing");
        let (t_recv, n_recv) = (Instant::now(), self.session.events.len());
        // Event held back while the renderer was being built: first, the order stays the same.
        if let Some(ev) = self.held_event.take() {
            self.deliver(ev);
        }
        for _ in 0..n_recv {
            let Ok(ev) = self.session.events.try_recv() else { break };
            self.deliver(ev);
        }
        perf_log::received(t_recv.elapsed(), n_recv);
        profiler::pop_push("scheduledExecutables");
        // Out of the world: title screen, servers, connection (`flow.rs`).
        if !self.front.in_world {
            self.menu_frame();
            return;
        }
        // Slot left by the mouse in a container or creative screen (`bundle.rs`).
        self.bundle_hover();
        self.relight_then_schedule_meshing();
        profiler::pop_push("mouse");
        self.mouse_frame();
        profiler::pop();
        self.frame_after_meshing();
    }

    /// Light recomputed after the packets received, then the meshing of what changed (each frame,
    /// and between the phases of the first renderer's build).
    pub(crate) fn relight_then_schedule_meshing(&mut self) {
        // Light recomputed after the frame's packets, before the meshing (like the game).
        if !self.relight.is_empty() {
            let changes = std::mem::take(&mut self.relight);
            let sky = std::mem::take(&mut self.relight_sky);
            let (grid, height, has_sky) = (&mut self.world.grid, self.world.height, self.env.has_sky);
            let touched = if light_engine::opts().sky_bit {
                light_engine::relight_masked(grid, height, &self.light, has_sky, &changes, &sky)
            } else {
                light_engine::relight_sections(grid, height, &self.light, has_sky, &changes)
            };
            for ((x, sy, z), edges) in touched {
                if let Some(s) = &mut self.light_shadow {
                    s.touch(x, z);
                }
                self.world.touch_around(x, sy, z, edges, true);
            }
            self.weather.rev += 1;
        }
        self.light_recv.clear();
        if let Some(s) = &mut self.light_shadow
            && self.solo.is_some()
        {
            for ((x, z), sky, block) in s.check(&self.world.grid) {
                eprintln!("singleplayer light ({x}, {z}): differences with the server sky {sky}, block {block}");
            }
        }
        self.schedule_meshing();
    }

    /// The frame in a world after the packets, the light and the meshing.
    fn frame_after_meshing(&mut self) {
        let now = Instant::now();
        self.debug.on_frame(now, &self.session.link.stats);
        let elapsed = now.duration_since(self.last).as_secs_f64();
        let dt = elapsed.min(0.1);
        self.last = now;
        if self.free_cam {
            self.fly(dt);
        }
        // Singleplayer pause from the top screen of the previous frame (`pause.rs`).
        self.update_pause();
        // All the elapsed time counts: a hitch is caught up (at most 10 ticks per frame).
        let due = ticking::due_ticks(&mut self.tick_acc, elapsed, self.exact.rate.tick_len());
        if due > 0 {
            self.animate_textures();
        }
        profiler::push("tick");
        let depth = profiler::depth();
        for _ in 0..due {
            profiler::increment("clientTick");
            self.game_tick();
            profiler::unwind(depth);
        }
        profiler::pop();
        if !self.free_cam && self.positioned {
            // Player's eyes, interpolated between the last two ticks.
            let alpha = self.partial();
            let eye = self.player.eye_height();
            for a in 0..3 {
                self.eye[a] = self.prev_pos[a] + (self.player.pos[a] - self.prev_pos[a]) * alpha;
            }
            self.eye[1] += eye;
        }
        let shot = self.place_camera(self.partial() as f32);
        self.shot = shot;
        let aspect = self.renderer.as_ref().map_or(16.0 / 9.0, |r| r.aspect());
        self.env.eye_effects = self.video.darkness.fog(self.partial() as f32);
        self.env.effects = environ::CameraEffects::read(&self.hud_extra.effects, &self.hud_extra.effect_names);
        let view = environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
        let frame_env = self.env.frame(
            &view,
            &self.phys,
            &self.data,
            shot.pos,
            shot.yaw,
            shot.pitch,
            aspect,
            self.partial() as f32,
            dt,
            self.options.i("renderDistance"),
            self.player.eyes_in_water,
        );
        // Crosshair aiming and outline, before borrowing the renderer.
        // Listener: eyes of the camera and its orientation (`sounds.md` § 3.4).
        profiler::push("sound");
        // Without joining: kept while the engine is being built.
        let (_, up, fwd) = camera::axes(shot.yaw, shot.pitch);
        self.audio.set_listener(shot.pos, fwd.to_array(), up.to_array());
        profiler::pop_push("frame");
        profiler::push("update");
        let hit = self.pick();
        let cracks = self.crack_vertices();
        self.hud.attack = Some(self.attack_indicator(hit));
        self.hud.attack_mode = self.options.i("attackIndicator");
        self.hud.main_left = self.options.s("mainHand") == "left";
        self.hud_frame(hit);
        let outline = self.outline_target(hit);
        let screen_mouse = self.gui_mouse();
        // F3 screen: lines collected as soon as an entry is active (text or not).
        let debug_text = self.debug_collect(hit);
        self.hud.crosshair_3d = self.debug.is_on("3d_crosshair") && !self.camera.view.detached();
        // Resource reload ready: applied before drawing (`reload.rs`).
        self.reload_tick();
        let (m5_hide_hand, assets) = (self.m5_hide_hand(), self.assets.clone());
        // Top screen (deferred subtitles or not, `pause.rs`).
        let (top, partial) = (self.pause_top(), self.partial() as f32);
        // Hands of the local player for the frame's item models (cast rod, selected slot).
        self.publish_local_hands();
        // Control of the remeshing (`remesh.rs`) at the time of the final capture.
        if remesh::checking()
            && self.capture.as_ref().is_some_and(|(_, at)| self.started.elapsed().as_secs_f64() >= *at)
        {
            self.mesh_check_report();
        }
        let Some(r) = &mut self.renderer else { return };
        let t_up = Instant::now();
        let o = remesh::opts();
        let mut batch = std::mem::take(&mut self.mesh_pending);
        batch.extend(self.meshes_rx.try_iter());
        if let Some(budget) = o.budget {
            let eye = ((self.eye[0] / 16.0).floor() as i32, (self.eye[2] / 16.0).floor() as i32);
            self.mesh_pending = remesh::split_budget(&mut batch, eye, budget);
        }
        let (mut t_done, mut t_gpu, mut sent) = (std::time::Duration::ZERO, std::time::Duration::ZERO, 0);
        for d in batch {
            self.in_flight -= 1;
            perf_log::meshed(d.mesh.x, d.mesh.z);
            remesh::note_received();
            let t = Instant::now();
            let done = self.world.mesh_done(d);
            let t_send = Instant::now();
            t_done += t_send - t;
            if let Some(remesh::Done { mesh, prep, in_place, .. }) = done {
                self.misc.column_meshed(mesh.x, mesh.z);
                r.upload(&mesh, prep, in_place);
                // Large arrays freed on the pool, not on the frame thread.
                if o.pack == remesh::PackAt::Pool {
                    rayon::spawn(move || drop(mesh));
                }
                sent += 1;
                t_gpu += t_send.elapsed();
            }
        }
        perf_log::uploaded(t_up.elapsed(), t_done, t_gpu, sent);
        remesh::reclaim_pool();
        if capseq::frame(r, self.started) {
            self.quit_after_capture = true;
        }
        if let Some((path, at)) = &self.capture
            && self.started.elapsed().as_secs_f64() >= *at
        {
            r.request_capture(path.clone());
            self.capture = None;
            self.quit_after_capture = true;
        } else if self.quit_after_capture {
            // Light engine check: RUSTCRAFT_LIGHTDUMP=<file> writes the light of the
            // columns around the player (radius 2), to compare two runs.
            if let Some(path) = rc_data::env_os!("RUSTCRAFT_LIGHTDUMP") {
                let (cx, cz) = ((self.player.pos[0].floor() as i32) >> 4, (self.player.pos[2].floor() as i32) >> 4);
                let mut out = Vec::new();
                for x in cx - 2..=cx + 2 {
                    for z in cz - 2..=cz + 2 {
                        let Some(col) = self.world.grid.get(x, z) else { continue };
                        for layers in [&col.block_light, &col.sky_light] {
                            for l in layers.iter() {
                                match l {
                                    Some(l) => out.extend_from_slice(&l[..]),
                                    None => out.extend_from_slice(&[0xff; rc_world::LIGHT_BYTES]),
                                }
                            }
                        }
                    }
                }
                let _ = std::fs::write(path, out);
            }
            rc_assets::cache::wait_writer();
            self.close_solo();
            std::process::exit(0);
        }
        let (ww, wh) = r.size();
        profiler::pop_push("extract");
        r.gui.begin(ww, wh);
        if rc_data::env_flag!("RUSTCRAFT_GUITEST") {
            let q = r.queue_handle();
            let g = &mut r.gui;
            g.sprite("hud/hotbar", g.width / 2.0 - 91.0, g.height - 22.0, 182.0, 22.0, 0xffff_ffff);
            let line = gui::plain("Rustcraft §cText §lbold §r§oitalic§r §nunderlined§r é 中 한");
            g.text(&q, &line, 2.0, 2.0, 0xffff_ffff, true, 0);
            let tr = rc_text::flatten(
                &rc_text::Component::translate(
                    "chat.type.text",
                    vec![rc_text::Arg::Text("Bench".into()), rc_text::Arg::Text("hello".into())],
                ),
                &g.lang,
            );
            g.text(&q, &rc_text::styled_chars(&tr), 2.0, 12.0, 0xffff_ffff, true, 0x8000_0000);
        }
        // Items of the local player (bar, menus, first-person hand). Fields taken one by one
        // (the renderer is borrowed): like `hud_env`.
        let env = stack::HudEnv {
            dimension: self.dimension.clone(),
            tick: (self.started.elapsed().as_secs_f64() * 20.0) as i64,
            game_time: self.env.env.clocks.game_time,
            sun_angle: self.sun_angle,
            pos: self.player.pos,
            // Body rotation not yet tracked: that of the head.
            body_yaw: self.yaw,
            spawn: self.spawn.clone(),
            last_death: self.last_death.clone(),
            keys: self.binds.down_mask(),
            camera_elsewhere: self.exact.camera.is_some(),
            ticking: self.exact.rate.runs,
            team: self.scores.team_rgb(&self.local_name),
        };
        {
            let q = r.queue_handle();
            let max_health = self.player.attribute("minecraft:max_health").unwrap_or(20.0);
            let armor = self.player.attribute("minecraft:armor").unwrap_or(0.0);
            let items = hud::HotbarItems {
                inv: &self.inventory,
                ids: &self.component_ids,
                data: &self.data,
                infos: &self.data.items,
                names: &self.item_names,
                env: &env,
                pt: partial,
            };
            // Location bar: markers seen from the camera (eyes of the known tracked entities).
            let pt = partial;
            let dims = r.entities.as_ref().map_or(&[][..], |e| e.dims.as_slice());
            let entity = |uuid: u128| {
                let e = self.entities.map.values().find(|e| e.uuid == uuid)?;
                let feet = e.render_pos(pt);
                Some((feet, [feet[0], feet[1] + f64::from(e.dims(dims)[1]), feet[2]]))
            };
            self.hud.more.locate(&hud_more::LocateIn {
                cam: shot.pos,
                yaw: shot.yaw,
                pitch: shot.pitch,
                fov: r.view.fov,
                me: self.player.pos,
                me_uuid: self.local_uuid,
                entity: &entity,
            });
            self.hud.draw(&mut r.gui, &q, self.env.game_mode, max_health, armor, Some(&items));
            for s in self.hud.more.take_sounds() {
                if let Some(a) = self.audio.get() {
                    let (cat, pos) = if s.ambient {
                        (rc_audio::Category::Ambient, None)
                    } else {
                        (rc_audio::Category::Player, Some(self.player.pos))
                    };
                    a.play(s.name, cat, pos, s.volume, s.pitch, None);
                }
            }
            // Spectator menu in place of the item bar (`spectator.rs`).
            if self.env.game_mode == 3 && !self.hud.hidden {
                let backdrop = if self.options.b("backgroundForChatOnly") {
                    0
                } else {
                    ((self.options.f("textBackgroundOpacity") * 255.0) as u32) << 24
                };
                let options = &self.options;
                self.spectator.draw(&mut r.gui, &q, backdrop, &|u| spectator::default_face(&assets, u), &|s| {
                    options.s(spectator::HOTBAR_KEY_OPTIONS[s])
                });
            }
            // HUD effects hidden when the inventory screen shows them (`inv_effects.rs`).
            if !self.hud.hidden {
                if inv_effects::screen_column(&r.gui, self.creative_ui.open, &self.screens, &self.inventory).is_none() {
                    self.hud_extra.draw_effects(&mut r.gui);
                }
                self.hud_extra.draw_bosses(&mut r.gui, &q);
            }
            // Sleep fade (even with the HUD hidden), under the sidebar and the titles.
            self.hud.more.draw_sleep(&mut r.gui);
            if !self.hud.hidden {
                self.scores.draw_sidebar(&mut r.gui, &q, &self.local_name, self.hud.text_background);
                // Action bar then title (the title covers it), text background outside the chat.
                let bg = hud::background_color(self.hud.text_background, 0);
                self.chat.draw_action_bar(&mut r.gui, &q, pt, bg);
                self.hud_extra.draw_title(&mut r.gui, &q, pt, bg);
            }
            // Menus over the chat (restrictions screen): chat drawn closed. Closed chat
            // hidden with the HUD (F1); open, it is a screen and stays drawn.
            self.chat.covered = self.menus.is_open();
            if pause::chat_drawn(self.hud.hidden, self.chat.is_open()) {
                self.chat.draw(&mut r.gui, &q, partial, screen_mouse);
            }
            if self.chat.is_open() && !self.chat.covered {
                self.cmd.draw(&mut r.gui, &q, &mut self.chat, screen_mouse);
            }
            // Chat in bed: "Leave Bed" button (`bed.rs`).
            self.bed.draw(&mut r.gui, &q, screen_mouse, self.chat.is_open() && !self.chat.covered);
            // Player list while Tab is held (game keys inactive during input).
            let tab = self.binds.down(keys::PLAYERLIST) || rc_data::env_flag!("RUSTCRAFT_TABTEST");
            let tab = tab && tablist::visible(self.solo.is_some(), &self.players, &self.scores);
            self.tab_list.set_visible(tab);
            if tab && !self.chat.is_open() && !self.hud.hidden {
                r.gui.next_layer();
                let ents = &self.entities;
                let ctx = tablist::Ctx {
                    online: self.online_mode,
                    tick: i64::from(self.hud.gui_ticks()),
                    background: hud::background_color(self.hud.text_background, 0x20ff_ffff),
                    in_world: &|u| {
                        ents.me.as_ref().is_some_and(|e| e.uuid == u)
                            || ents.map.values().any(|e| e.uuid == u && e.kind == "player")
                    },
                    default_skin: &|u| tablist::default_skin(&assets, u),
                };
                self.tab_list.draw(&mut r.gui, &q, &self.players, &ctx, &self.scores);
            }
            // Subtitles (last element of the HUD) and system notification (`misc.rs`).
            let subs = self.options.b("showSubtitles");
            if let Some(a) = self.audio.get() {
                a.set_subtitles(subs);
                let now = std::time::Instant::now();
                for s in a.take_subtitles() {
                    self.misc.subtitles.notify(s, now);
                }
            }
            // Lines never erased (rank of first appearance kept for the whole session). Without a
            // screen, drawn last (above F3); under an in-game screen, on its veil.
            let subs_at = if subs { pause::subtitles_at(self.hud.hidden, top) } else { pause::Subs::Off };
            let opacity =
                if self.options.b("backgroundForChatOnly") { 0.8 } else { self.options.f("textBackgroundOpacity") };
            let subs_style = subtitles::Style {
                duration: self.options.f("notificationDisplayTime"),
                background: ((opacity * 255.0) as u32) << 24,
            };
            if subs_at == pause::Subs::Before {
                self.misc.subtitles.draw(&mut r.gui, &q, shot.pos, shot.yaw, shot.pitch, &subs_style);
            }
            self.misc.draw_toast(&mut r.gui, &q, self.options.f("notificationDisplayTime"));
            if self.screens.open {
                let me = self.entities.local;
                self.screens.player_model = self.entities.me.is_some().then_some(me);
                self.screens.draw(&mut r.gui, &q, &self.inventory, &items, screen_mouse);
                let ctx = recipe_book::Ctx { data: &self.data, rec: &self.creative_rec, item_names: &self.item_names };
                let ench = &self.screens.hasher.enchantments;
                let link = &self.session.link;
                self.recipe_book.draw(&mut r.gui, &q, &self.inventory, &items, ench, &ctx, screen_mouse, link);
            }
            if self.creative_ui.open {
                let me = self.entities.local;
                self.creative_ui.player_model = self.entities.me.is_some().then_some(me);
                let t = self.started.elapsed().as_secs_f32();
                let ench = &self.screens.hasher.enchantments;
                self.creative_ui.draw(&mut r.gui, &q, &self.inventory, &items, screen_mouse, ench, t);
            }
            if let Some((col, top)) =
                inv_effects::screen_column(&r.gui, self.creative_ui.open, &self.screens, &self.inventory)
            {
                let (fx, names, rate) = (&self.hud_extra.effects, &self.hud_extra.effect_names, self.exact.rate.rate);
                inv_effects::draw(&mut r.gui, &q, fx, names, rate, col, top, screen_mouse);
            }
            self.special.draw(&mut r.gui, &q, &items, screen_mouse, self.started.elapsed().as_secs_f64());
            op_screens::draw_suggestions(&mut self.special, &mut self.cmd, &mut r.gui, &q, screen_mouse);
            if let Some(d) = &mut self.death {
                let s = f64::from(r.gui.scale);
                let mouse = self.cursor.map(|(x, y)| ((x / s) as f32, (y / s) as f32));
                d.draw(&mut r.gui, &q, mouse);
            }
            // Server dialog (`dialog.rs`).
            if let Some(d) = &mut self.exact.dialog {
                let s = f64::from(r.gui.scale);
                let mouse = self.cursor.map(|(x, y)| ((x / s) as f32, (y / s) as f32));
                d.draw(&mut r.gui, &q, mouse, Some(&items), &self.screens.hasher.enchantments);
            }
            if self.menus.is_open() && self.packs.screen.is_none() && self.progress.screen.is_none() {
                self.menus.draw(&mut r.gui, &q, &self.options, screen_mouse, self.started.elapsed().as_secs_f64());
                let audible = self.options.f("soundCategory_master") * self.options.f("soundCategory_music") > 0.0;
                if self.menus.top() == Some(menus::Id::Pause) && self.options.s("musicToast") != "never" && audible {
                    self.fx.music.draw_pause(&mut r.gui, &q, self.started.elapsed().as_secs_f64() * 1000.0);
                }
            }
            // Advancements and statistics screens (`progress_app.rs`).
            let ctx = progress_app::DrawCtx {
                assets: &self.assets,
                blur: self.options.i("menuBackgroundBlurriness"),
                time: self.started.elapsed().as_secs_f64(),
            };
            self.progress.draw(&mut r.gui, &q, &items, &ctx, screen_mouse);
            // Autosave indicator (singleplayer, `world_list.rs`): above the open screen,
            // even with the HUD hidden, smoothed by the frame duration (in ticks), like the game.
            let bd = if self.options.b("backgroundForChatOnly") {
                0
            } else {
                ((self.options.f("textBackgroundOpacity") * 255.0) as u32) << 24
            };
            let real = ticking::realtime_delta(elapsed);
            world_list::draw_autosave(&mut r.gui, &q, real, self.options.b("showAutosaveIndicator"), bd);
            // Notifications above the screens (manager drawn after the open screen); updated
            // even with the HUD hidden, only the drawing is skipped.
            let (now, mult) =
                (self.started.elapsed().as_secs_f64() * 1000.0, self.options.f("notificationDisplayTime"));
            self.toasts.periodic_tick(&r.gui, now);
            self.toasts.update(now, mult);
            self.fx.music.update(now);
            for s in self.toasts.sounds.drain(..) {
                if let Some(a) = self.audio.get() {
                    a.play(s, rc_audio::Category::Ui, None, 1.0, 1.0, None);
                }
            }
            if !self.hud.hidden {
                self.toasts.draw(&mut r.gui, &q, &items, now, mult);
                if self.options.s("musicToast") == "pause_and_toast" && self.menus.top() != Some(menus::Id::Pause) {
                    self.fx.music.draw_toast(&mut r.gui, &q);
                }
            }
            // Hover tooltips and link confirmation screen (`links.rs`).
            let (book, death) = (links::book_of(&mut self.special.overlay), self.death.as_ref());
            let advanced = self.options.b("advancedItemTooltips");
            links::draw_overlay(&mut r.gui, &q, &mut self.chat, book, death, screen_mouse, &self.data, advanced);
            if !self.menus.is_open()
                && let Some(m) = self.chat.test_mouse.or(screen_mouse)
                && links::clickable_under(
                    &r.gui,
                    &mut self.chat,
                    links::book_of(&mut self.special.overlay),
                    self.death.as_ref(),
                    m,
                    self.mods.shift_key(),
                )
            {
                widgets::request_cursor(widgets::Cursor::Hand);
            }
            // End screen (`win_screen.rs`): alone, on the End portal background.
            if let Some(w) = &mut self.win {
                let (ww, wh) = r.size();
                r.gui.begin(ww, wh);
                w.draw(&mut r.gui, &q, self.exact.rate.tick_len());
            }
            // World loading screen (`flow.rs`): alone, on the panorama.
            if self.front.is_open() {
                let (ww, wh) = r.size();
                r.gui.begin(ww, wh);
                self.front.draw(&mut r.gui, &q, &self.options, screen_mouse, self.started.elapsed().as_secs_f64());
            }
            // Server pack prompt, resource loading screen (`reload.rs`).
            self.packs.draw(&mut r.gui, &q, &self.options, screen_mouse);
            if debug_text {
                self.debug.draw(&mut r.gui, &q, (ww, wh), self.options.i("debugGuiScale"), self.options.i("maxFps"));
            }
            // Under an in-game screen, interleaved between its veil and its panel.
            let st = &mut self.misc.subtitles;
            match subs_at {
                pause::Subs::Deferred => st.draw(&mut r.gui, &q, shot.pos, shot.yaw, shot.pitch, &subs_style),
                pause::Subs::InScreen => {
                    r.gui.draw_after_veil(|g| st.draw(g, &q, shot.pos, shot.yaw, shot.pitch, &subs_style));
                }
                pause::Subs::Off | pause::Subs::Before => {}
            }
            // Game mode switcher (F3+F4).
            if let Some(s) = &mut self.debug.switcher {
                s.prepare(&debug_switcher::Icons {
                    models: r.gui.items.as_ref().map(|a| &a.models),
                    names: &self.item_names,
                    infos: &self.data.items,
                    ids: &self.component_ids,
                    data: &self.data,
                    env: &env,
                });
                let f4 = options::key_label(&r.gui.lang, self.options.s("key_key.debug.switchGameMode"));
                s.draw(&mut r.gui, &q, screen_mouse, &f4);
            }
            let (scale, time) = (self.options.i("debugGuiScale"), self.started.elapsed().as_secs_f64());
            self.debug.menu_blur = self.options.i("menuBackgroundBlurriness");
            self.debug.draw_options(&mut r.gui, &q, screen_mouse, scale, time);
            // IME composition of the focused field: over the interface, under the tooltips (`ime.rs`).
            ime::finish_frame(&mut r.gui, &q);
        }
        let pt = partial;
        self.env.darkness = self.video.darkness.light(pt, self.options.f("darknessEffectScale") as f32);
        if let Some(lm) = self.env.lightmap(pt) {
            r.set_lightmap(&lm);
            self.debug.logs.lightmap = Some(lm); // F3+4 (`debug_charts.rs`)
        }
        // `RUSTCRAFT_NOENT`: entities not drawn (measuring their cost).
        // Projection of the frame: field of view, damage shake, sway, hand.
        {
            let pt = partial;
            let hurt =
                self.entities.me.as_ref().filter(|e| e.hurt_time > 0).map(|e| (e.hurt_time as f32 - pt, e.hurt_dir));
            let mut view = self.camera.render_view(pt, frame_env.fluid, hurt);
            view.hand &= !self.free_cam && !self.hud.hidden && self.env.game_mode != 3 && !m5_hide_hand;
            // Hand pass also for the totem animation (even in third person).
            self.ev.totem.show_hand = view.hand;
            view.hand |= self.ev.totem.active() && !self.hud.hidden;
            r.view = view;
            // Screen effects, camera overlays, portal and nausea distortion.
            let wv = environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
            let state_at = |x, y, z| wv.state(x, y, z);
            let mut eye = self.player.pos;
            eye[1] += self.player.eye_height();
            let (fx, distort) = self.hud.more.scene(&hud_more::SceneIn {
                data: &self.data,
                state_at: &state_at,
                me: self.entities.me.as_ref(),
                equippable: self.component_ids.equippable,
                feet: self.player.pos,
                eye,
                yaw: self.yaw,
                pitch: self.pitch,
                shot: (shot.yaw, shot.pitch),
                view: r.view,
                aspect,
                first_person: !self.camera.view.detached() && !self.free_cam,
                spectator: self.env.game_mode == 3,
                hud_hidden: self.hud.hidden,
                screen_effect_scale: self.options.f("screenEffectScale") as f32,
                eye_brightness: self.env.eye_brightness(&wv, eye),
                pt,
                tick_len: self.exact.rate.tick_len() as f32,
            });
            r.distort = distort;
            r.prepare_overlays(&fx);
        }
        let in_view = r.visibility(shot.pos, shot.yaw, shot.pitch, frame_env.far);
        r.leash.verts.clear();
        // Screen effects (§ 6.2): server list then observed entity, only with a player.
        let cam_kind = self.exact.camera.and_then(|id| self.entities.map.get(&id)).map(|e| e.kind.as_str());
        r.post_effects = self.misc.effects(self.exact.camera, cam_kind, self.camera.view.detached());
        r.post_blur = self.options.i("menuBackgroundBlurriness");
        let win_size = r.size();
        if let Some(er) = r.entities.as_mut().filter(|_| !rc_data::env_flag!("RUSTCRAFT_NOENT")) {
            let view =
                environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
            let origin = [shot.pos[0].floor() as i32, shot.pos[2].floor() as i32];
            let pt = partial;
            let (darken, has_sky, ambient) = self.env.shadow_params();
            if rc_data::env_flag!("RUSTCRAFT_ENTCHECK") && self.fps_frames == 10 {
                let names = self.data.registry("minecraft:entity_type").map(|r| r.entries.clone()).unwrap_or_default();
                eprintln!("kinds without render: {}", er.unresolved(&names).join(" "));
            }
            for (name, entries) in self.pending_registries.drain(..) {
                er.set_variants(&name, &entries);
            }
            er.set_camera(shot.yaw, shot.pitch);
            // Skins of the placed player heads.
            let mut patched = Vec::new();
            for (key, value) in self
                .block_entities
                .heads
                .drain(..)
                .chain(er.pending_heads.drain(..))
                .chain(item_special::take_wanted())
                .chain(r.gui.objects.wanted.drain(..))
            {
                // Texture imposed by the profile's skin patch: game resource, read here.
                if value.starts_with("asset:") {
                    patched.push((key, value));
                } else {
                    self.skins.request_head(key, value);
                }
            }
            for (key, value) in patched {
                let id = &value["asset:".len()..];
                let (ns, path) = id.split_once(':').unwrap_or(("minecraft", id));
                if let Some(img) = self.assets.image(&format!("assets/{ns}/textures/{path}.png")) {
                    if let Some(items) = r.gui.items.as_mut() {
                        items.set_head_skin(&r.queue, key, &img);
                    }
                    er.set_skin(key, Some((img, false, value)), None, None);
                }
            }
            for (key, skin) in item_special::take_default_heads() {
                if let (Some(items), Some(img)) = (r.gui.items.as_mut(), item_special::default_skin_image(&self.assets, skin)) {
                    items.set_head_skin(&r.queue, key, &img);
                }
            }
            for (uuid, s) in self.skins.poll() {
                // Without skin (cape or elytra alone): the icons keep the default skin.
                if let Some((image, _, _)) = &s.skin {
                    // Interface icons of the player heads: skin of the head's identity.
                    if let Some(items) = r.gui.items.as_mut() {
                        items.set_head_skin(&r.queue, uuid, image);
                    }
                    self.spectator.skin(uuid, image);
                    self.tab_list.skin(uuid, image);
                    r.gui.objects.skin(uuid, image);
                }
                er.set_skin(uuid, s.skin, s.cape, s.elytra);
            }
            let items = r.gui.items.as_ref().map(|a| &a.models);
            // Local player only in third person (no name above oneself).
            // Camera on another entity: the local player is not drawn, nor the followed
            // entity in first person (`set_camera`).
            let shown = draws_local_player(
                self.camera.view.detached(),
                // Lying: bed position (value 14), like `sleeping_pos` (render borrowed here).
                self.entities.me.as_ref().is_some_and(|m| {
                    matches!(m.meta.get(&14), Some(rc_proto_v777::entity::Meta::BlockPos(Some(_))))
                }),
                self.free_cam,
                self.exact.camera.is_some(),
            );
            let me = self.entities.me.as_ref().filter(|_| shown);
            let hidden = self.exact.camera.filter(|_| !self.camera.view.detached());
            // Flying copies of the picked-up items, drawn like their entity (`misc.rs`).
            let pickups = self.misc.pickup_copies(pt);
            let local_spectator = self.env.game_mode == 3;
            fill_glow_colors(&mut er.glow, &self.entities, &self.players, &self.scores);
            fill_ghost_set(&mut er.ghosts, &self.entities, &self.players, (&self.scores, local_spectator));
            er.world.clone_from(&env);
            er.camera = self.exact.camera;
            refresh_player_sets(
                [&mut er.spectators, &mut er.upside_down, &mut er.extra_ears],
                &mut er.player_sets_stamp,
                &self.entities,
                &self.players,
                local_spectator,
            );
            // Bobbers and fishing lines, text displays, guardian beams (`objects.rs`).
            let obj_frame = entity_render::ObjectsFrame {
                pt,
                origin,
                cam: shot.pos,
                local: self.player_id,
                local_left: self.options.s("mainHand") == "left",
                first_person: !self.camera.view.detached(),
                fov: self.options.fov(),
                yaw: shot.yaw,
                pitch: shot.pitch,
                aspect: win_size.0 as f32 / win_size.1.max(1) as f32,
            };
            er.objects_prepass(&self.entities, &obj_frame);
            // Biome colours of a block (falling blocks, pushed by a piston, transient).
            let (palette, plains) = (&self.palette, self.env.plains());
            let biome_tints = |x, y, z| view.biome_tints(palette, x, y, z, plains);
            er.prepare(
                self.entities
                    .map
                    .iter()
                    .filter(|(id, _)| Some(**id) != hidden)
                    .map(|(_, e)| e)
                    .chain(me)
                    .chain(&pickups),
                pt,
                origin,
                shot.pos,
                &|x, y, z| view.light_pair(x, y, z),
                &in_view,
                items,
                &entity_render::ShadowWorld {
                    state: &|x, y, z| view.state(x, y, z),
                    brightness: &|x, y, z| view.raw_brightness(x, y, z, has_sky, darken),
                    ambient,
                    biome_tints: &biome_tints,
                },
            );
            er.push_guardian_beams(&self.entities, &obj_frame);
            er.push_block_entities(
                &self.block_entities,
                &entity_render::BlockFrame {
                    pt,
                    origin,
                    cam: shot.pos,
                    game_time: self.env.env.clocks.game_time,
                    min_y: self.world.min_y,
                    light: &|x, y, z| view.light_pair(x, y, z),
                    visible: &in_view,
                    items,
                    render_distance: self.options.i("renderDistance").min(self.env.server_view.max(2)),
                    top: self.world.min_y + self.world.height,
                    state: &|x, y, z| view.state(x, y, z),
                    scoping: self.hud.more.scoping,
                    biome_tints: &biome_tints,
                },
            );
            // Transient blocks and leash ropes (`misc.rs`).
            let light = |x, y, z| view.light_pair(x, y, z);
            er.push_transient(self.misc.transient.iter().map(|t| (t.pos, t.state)), origin, &light, &biome_tints);
            let ctx = leash::Ctx {
                pt,
                table: &er.dims,
                local: self.player_id,
                local_left: self.options.s("mainHand") == "left",
                first_person: !self.camera.view.detached(),
            };
            r.leash.verts =
                misc::leash_vertices(&self.misc, &self.entities, &ctx, origin, shot.pos, &light, &in_view);
            // First-person hand: stack = inverse view × shake and sway × tilt.
            if r.view.hand
                && self.ev.totem.show_hand
                && let (Some(me), Some(models), Some(hs)) = (&self.entities.me, items, &self.camera.hand)
            {
                let base = camera::view_matrix(shot.pos, shot.yaw, shot.pitch).2.inverse()
                    * r.view.tilt
                    * self.camera.hand_sway(pt, shot.yaw, shot.pitch);
                let p = me.render_pos(pt);
                let eye_y = p[1] + self.camera.eye_height(pt);
                let (b, s) = view.light_pair(p[0].floor() as i32, eye_y.floor() as i32, p[2].floor() as i32);
                let light = (u32::from(b) * 16) | ((u32::from(s) * 16) << 8);
                let extra = hand::HandExtra {
                    enchantments: &self.screens.hasher.enchantments,
                    spin: self.player.spin_attack,
                    kinetic_ticks: self.ev.kinetic_since(self.player_id, self.env.env.clocks.game_time, pt),
                    env: &env,
                };
                hand::push(er, models, hs, me, pt, base, light, &self.component_ids, &self.data, &extra);
            }
            // Totem activated: full-screen item in front of the camera, drawn after the screen effects,
            // in the view frame with the items' fixed lights (`totem_anim.rs`).
            if r.view.hand
                && !self.hud.hidden
                && let (Some(models), Some((stack, m))) = (items, self.ev.totem.current(pt, aspect))
            {
                er.begin_totem();
                er.push_stack(models, stack, Some(totem_anim::FIXED), None, m, totem_anim::FULL_LIGHT);
            }
            // 3D interface models requested when drawing the screens (`special.rs`).
            special::push_models(er, &mut self.entities, items, pt);
        }
        // Text of the visible signs (faces collected by the block-entity render) and displays;
        // the camera lets them leave out the faces turned away.
        if let Some(er) = &r.entities {
            let q = r.queue_handle();
            let camera = crate::glyph_pool::BackFaceTest::for_camera(shot.pos, r.view.fov, r.size().1);
            r.block_text.prepare(&mut r.gui, &q, &er.blocks.texts, &self.block_entities, &camera);
            let bg = self.hud.text_background.unwrap_or(63 << 24);
            r.text_displays.prepare(&mut r.gui, &q, er.text_draws(), er.text_labels(), &self.entities, bg, &camera);
        }
        // End screen: End portal background in front of the camera (`win_screen.rs`).
        if let Some(er) = r.entities.as_mut()
            && self.win.as_ref().is_some_and(|w| w.portal_background())
        {
            win_screen::push_backdrop(&mut er.blocks.fx, &shot);
        }
        let pt = partial;
        let pw = particles::TickWorld {
            grid: &self.world.grid,
            min_y: self.world.min_y,
            height: self.world.height,
            phys: &self.phys,
            player: None,
            scope: None,
        };
        // The game tests a particle against the frustum moved back three blocks, without the
        // occlusion of the entities (`RUSTCRAFT_OPT_PARTICLE_GAME_FRUSTUM=0`: the entities' test).
        let particle_frustum = rc_data::opt_switch!("RUSTCRAFT_OPT_PARTICLE_GAME_FRUSTUM")
            .then(|| r.particle_frustum(shot.pos, shot.yaw, shot.pitch, frame_env.far));
        let cull = match &particle_frustum {
            Some(frustum) => particles::ParticleCull::GameFrustum(frustum),
            None => particles::ParticleCull::LikeEntities(&in_view),
        };
        self.particles.prepare(&mut r.particles, &pw, shot.pos, shot.yaw, shot.pitch, pt, cull);
        {
            let view =
                environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
            let (rain, t) = (self.env.env.weather.rain, self.env.env.clocks.game_time);
            let radius = self.options.i("weatherRadius");
            self.weather.prepare(&mut r.weather, &view, &self.data, shot.pos, rain, t, pt, radius);
            let opt = self.options.i("renderDistance");
            let chunks = if self.env.server_view > 0 { opt.min(self.env.server_view) } else { opt };
            self.weather.prepare_wall(&mut r.weather.wall, shot.pos, pt, f64::from(chunks * 16), frame_env.far);
        }
        r.outline.set(outline.as_ref().map(|(shape, boxes, corner, tr)| (*shape, boxes.as_slice(), *corner, *tr)));
        if !r.cracks.loaded() {
            let images: Vec<_> = (0..20)
                .map(|i| {
                    let name = if i < 10 { format!("destroy_stage_{i}") } else { format!("destroy_oit_stage_{}", i - 10) };
                    rc_assets::texture::load(&self.assets, &format!("minecraft:block/{name}"))
                        .unwrap_or_else(|| rc_assets::texture::Image::missing(16))
                })
                .collect();
            r.cracks.load(&r.queue_handle(), &images);
        }
        r.cracks.set(cracks.0, cracks.1);
        // F3 traces (boxes, chunk borders, 3D crosshair): nothing if no entry is active.
        let scene = debug_render::Scene {
            debug: &self.debug,
            entities: &self.entities,
            eye: shot.pos,
            yaw: shot.yaw,
            pitch: shot.pitch,
            pt,
            camera: self.exact.camera.filter(|id| self.entities.map.contains_key(id)),
            first_person: !self.camera.view.detached(),
            free_cam: self.free_cam,
            player: &self.player,
            grid: &self.world.grid,
            min_y: self.world.min_y,
            height: self.world.height,
            phys: &self.phys,
            border: &self.weather.border,
            data: &self.data,
            ids: &self.component_ids,
            crosshair: !self.camera.view.detached() && !self.hud.hidden && !self.free_cam,
            gui_scale: r.gui.scale,
        };
        debug_render::fill(r, &scene, &in_view);
        let win_w = r.size().0;
        if let Some(er) = &r.entities {
            er.push_fishing_lines(&mut r.debug_lines, win_w);
        }
        {
            let view =
                environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
            let visible = self.env.game_mode == 3 || (self.env.game_mode == 1 && self.debug.op_level >= 2);
            let rd = self.options.i("renderDistance").min(self.env.server_view.max(2));
            tech_blocks::draw_boxes(&mut r.debug_lines, &self.tech, shot.pos, visible, rd, &view, &self.data);
            let q = r.queue_handle();
            let now = std::time::Instant::now();
            tech_blocks::draw_markers(&mut r.debug_lines, &mut r.gui, &q, &mut self.tech, shot.pos, rd, now);
        }
        let target = match hit {
            pick::Hit::Entity { id, .. } => Some(id),
            _ => None,
        };
        // Background of the labels: 0.25, or the text background opacity if the option applies it everywhere.
        r.tags.set_background(if self.options.b("backgroundForChatOnly") {
            nametag::DEFAULT_OPACITY
        } else {
            self.options.f("textBackgroundOpacity") as f32
        });
        name_tags(
            r,
            &self.entities,
            &self.players,
            &self.world,
            shot.pos,
            (shot.yaw, shot.pitch),
            pt,
            self.hud.hidden,
            target,
            (&self.scores, &self.local_name, self.env.game_mode == 3),
            &self.component_ids,
            &mut self.below_line_keys,
        );
        r.show_panorama = self.front.is_open();
        if r.show_panorama && r.panorama.is_none() {
            if let Some(p) = panorama::PanoramaAssets::load(&self.assets) {
                r.enable_panorama(&p);
            }
        } else if !r.show_panorama && r.panorama.is_some() {
            // In a game the panorama is not used: its VRAM (24 MB) is given back.
            r.panorama = None;
        }
        profiler::pop_push("render");
        let drawn = r.render(shot.pos, shot.yaw, shot.pitch, self.world.min_y, self.world.height, &frame_env);
        profiler::pop_push("fpsUpdate");
        if drawn {
            self.fps_frames += 1;
        } else {
            // Surface without a frame (hidden window): no tight loop.
            std::thread::sleep(std::time::Duration::from_millis(4));
        }
        first_frame(self.started);
        if drawn {
            FIRST_DRAWN.store(true, std::sync::atomic::Ordering::Relaxed);
            let busy = self.in_flight > 0 || !self.world.dirty.is_empty();
            // Columns to mesh: the loaded ones, minus those waiting for a neighbour.
            let cols = self.world.grid.len().saturating_sub(self.world.meshes.waiting_len());
            let in_game = self.positioned && !self.front.is_open();
            perf_log::frame(self.started, Some(perf_log::WorldFrame { busy, in_game, columns: cols }));
        }
        self.report_each_second(true);
    }

    /// Once a second, in a world and outside (title screen, menus): statistics line read by the
    /// bench, allocator totals, window title.
    pub(crate) fn report_each_second(&mut self, in_world: bool) {
        let since = self.fps_since.elapsed().as_secs_f64();
        if since < 1.0 {
            return;
        }
        let Some(r) = &mut self.renderer else { return };
        let fps = f64::from(self.fps_frames) / since;
        let stats = perf_log::SecondStats::read(r, fps, self.world.grid.len(), self.in_flight, self.block_entities.len());
        if in_world {
            self.debug.set_gpu(stats.gpu_ms);
        }
        eprintln!("[{:>6.1} s] {}", self.started.elapsed().as_secs_f64(), stats.line());
        alloc_trace::per_second(since, self.fps_frames);
        let mo = |b: u64| b as f64 / 1_048_576.0;
        // Total of the allocator (Vulkan): everything included (textures, buffers, targets).
        if let Some(rep) = r.allocator_report() {
            eprintln!(
                "GPU allocator: {:.1} MB allocated, {:.1} MB reserved",
                mo(rep.total_allocated_bytes),
                mo(rep.total_reserved_bytes)
            );
            if rc_data::env_flag!("RUSTCRAFT_VRAMDUMP") {
                let allocations: Vec<_> = rep.allocations.iter().map(|x| (x.size, x.name.as_str())).collect();
                let blocks: Vec<_> = rep.blocks.iter().map(|b| (b.size, b.allocations.clone())).collect();
                for line in vram_dump_lines(&allocations, &blocks) {
                    eprintln!("{line}");
                }
            }
        }
        #[cfg(feature = "memstats")]
        eprintln!(
            "Rust heap: {:.1} MB (world {:.1} MB, render translucents {:.1} MB)",
            memstats::LIVE.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1_048_576.0,
            self.world.grid.iter().map(rc_world::Column::heap_bytes).sum::<usize>() as f64 / 1_048_576.0,
            r.trans_bytes() as f64 / 1_048_576.0
        );
        // Title like the official one ("<name> <version>", plus " - <mode>" in a world), name
        // Rustcraft (decided on 12/10/2026); the readings stay in the log above.
        let title = if in_world {
            let mode = front::title_mode_key(self.solo.is_some(), self.menus.world.published.is_some());
            let mode = r.gui.lang.map.get(mode).map_or(mode, String::as_str);
            format!("Rustcraft {} - {mode}", front::VERSION_NAME)
        } else {
            format!("Rustcraft {}", front::VERSION_NAME)
        };
        self.window_title.set(&r.window, &title);
        self.fps_frames = 0;
        self.fps_since = Instant::now();
    }

    /// Texture animation (blocks, item sprites, entity textures including the flames of
    /// burning entities, interface sprites) like the game: a single step
    /// per frame that owes at least one tick, played before those ticks (not one per tick: under 20 frames
    /// per second, the animations slow down), stopped when the world is frozen (`/tick freeze`,
    /// unless stepping is requested). It continues during pause, like in the game.
    fn animate_textures(&mut self) {
        let rate = &self.exact.rate;
        if rate.frozen && rate.steps <= 0 {
            return;
        }
        for (c, a) in self.clocks.iter_mut().zip(&self.models.animations) {
            c.tick(a);
        }
        sprite_anim::advance_clock();
        if let Some(r) = &mut self.renderer {
            if let Some(er) = r.entities.as_mut() {
                er.tick_textures();
            }
            if let Some(items) = r.gui.items.as_mut() {
                items.tick_textures(&r.queue);
            }
            r.animate(self.clocks.iter().zip(&self.models.animations).map(|(c, a)| {
                let (cur, next, milli) = c.frame(a);
                (a.texture, cur, next, milli)
            }));
        }
    }
}

/// `RUSTCRAFT_VRAMDUMP` lines from the GPU allocator's report: the 15 largest allocations (size,
/// name), then the memory blocks (size, allocation ranges) summed (reserved, used, unused in empty
/// blocks and free in the others) and the 15 with the most unused room, each with its largest
/// allocation. The report does not say whether a block is local to the GPU or visible to the host:
/// the name of its largest allocation tells a staging block from a resource block.
fn vram_dump_lines(allocations: &[(u64, &str)], blocks: &[(u64, std::ops::Range<usize>)]) -> Vec<String> {
    let mo = |b: u64| b as f64 / 1_048_576.0;
    let mut lines = Vec::new();
    let mut largest: Vec<_> = allocations.to_vec();
    largest.sort_unstable_by_key(|x| std::cmp::Reverse(x.0));
    for (size, name) in largest.iter().take(15) {
        lines.push(format!("  {:>8.2} MB  {name}", mo(*size)));
    }
    // Per block: size, bytes used, allocations, name of the largest one.
    let mut uses: Vec<(u64, u64, usize, &str)> = blocks
        .iter()
        .map(|(size, range)| {
            let inside = allocations.get(range.clone()).unwrap_or(&[]);
            let used = inside.iter().map(|a| a.0).sum();
            let name = inside.iter().max_by_key(|a| a.0).map_or("(empty)", |a| a.1);
            (*size, used, inside.len(), name)
        })
        .collect();
    let reserved: u64 = uses.iter().map(|u| u.0).sum();
    let used: u64 = uses.iter().map(|u| u.1).sum();
    let empty: Vec<_> = uses.iter().filter(|u| u.2 == 0).collect();
    let empty_bytes: u64 = empty.iter().map(|u| u.0).sum();
    let free_in_used: u64 = uses.iter().filter(|u| u.2 > 0).map(|u| u.0.saturating_sub(u.1)).sum();
    lines.push(format!(
        "  {} memory blocks: {:.1} MB reserved, {:.1} MB used; unused: {:.1} MB in {} empty blocks, {:.1} MB free in the others",
        uses.len(),
        mo(reserved),
        mo(used),
        mo(empty_bytes),
        empty.len(),
        mo(free_in_used)
    ));
    uses.sort_unstable_by_key(|u| std::cmp::Reverse(u.0.saturating_sub(u.1)));
    for (size, used, count, name) in uses.iter().take(15) {
        lines.push(format!(
            "  block {:>8.2} MB, used {:>8.2} MB by {count} allocations, largest: {name}",
            mo(*size),
            mo(*used)
        ));
    }
    lines
}

#[cfg(test)]
mod vram_dump_tests {
    use super::vram_dump_lines;

    const MB: u64 = 1 << 20;

    /// Totals and block lines: an empty block counts in the unused room of the empty blocks, the free
    /// room of a used block in the other total; blocks come out by decreasing unused room.
    #[test]
    fn allocator_blocks_split_the_unused_room() {
        let allocations = [(3 * MB, "region faces"), (MB, "camera"), (8 * MB, "block textures")];
        let blocks = [(8 * MB, 0..2), (16 * MB, 2..2), (8 * MB, 2..3)];
        let lines = vram_dump_lines(&allocations, &blocks);
        assert_eq!(lines[0], "      8.00 MB  block textures");
        assert_eq!(
            lines[3],
            "  3 memory blocks: 32.0 MB reserved, 12.0 MB used; unused: 16.0 MB in 1 empty blocks, 4.0 MB free in the others"
        );
        assert_eq!(lines[4], "  block    16.00 MB, used     0.00 MB by 0 allocations, largest: (empty)");
        assert_eq!(lines[5], "  block     8.00 MB, used     4.00 MB by 2 allocations, largest: region faces");
        assert_eq!(lines[6], "  block     8.00 MB, used     8.00 MB by 1 allocations, largest: block textures");
        assert_eq!(lines.len(), 7);
    }
}
