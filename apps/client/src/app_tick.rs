//! The fixed game tick (20 per second) and the client tick of the block entities.

use rc_physics::{Input, World as _};

use crate::grid_world::GridWorld;
use crate::{
    App, block_entities, capseq, entity_motion, entity_render, environ, hud_more, keys, particles, profiler,
    projectile_sim, stack, ticking, weather, world_list, world_sounds,
};

impl App {
    /// Client tick of the block entities (`block-entities.md` § 1.5).
    fn tick_block_entities(&mut self) {
        let view = environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
        let tables = &self.tables;
        let top = self.world.min_y + self.world.height;
        let surface = |x: i32, z: i32| {
            (self.world.min_y..top)
                .rev()
                .find(|&y| !tables.is_air(view.state(x, y, z)))
                .map_or(self.world.min_y, |y| y + 1)
        };
        // Both lists from one walk, in the same order.
        let by_facts = crate::entities::kind_id_filters_on();
        let is_player = |e: &&crate::entities::Entity| if by_facts { e.facts.player } else { e.kind == "player" };
        let (mut players, mut named): (Vec<_>, Vec<_>) =
            self.entities.map.values().filter(is_player).map(|e| (e.pos, (e.uuid, e.pos))).unzip();
        if self.positioned && self.env.game_mode != 3 {
            players.push(self.player.pos);
            named.push((self.local_uuid, self.player.pos));
        }
        let w = block_entities::TickWorld {
            state: &|x, y, z| view.state(x, y, z),
            surface: &surface,
            players: &players,
            named: &named,
            game_time: self.env.env.clocks.game_time,
            min_y: self.world.min_y,
        };
        let mut emitted = Vec::new();
        self.block_entities.tick(&w, &mut emitted);
        let opts = particles::Options::default();
        for (kind, pos, vel) in emitted {
            self.particles.spawn(kind, &opts, pos, vel);
        }
        let sounds = std::mem::take(&mut self.block_entities.sounds);
        if let Some(a) = self.audio.get() {
            for (name, pos, volume, pitch) in sounds {
                a.play(name, rc_audio::Category::Block, Some(pos), volume, pitch, None);
            }
        }
        self.pistons_due = true;
    }

    /// One game tick in the official order: player physics (with the abilities packet
    /// if it toggles flight), then synchronisation (inputs if they change, sprint, position),
    /// then end of tick. "Player loaded" is only signalled once its chunk is received.
    pub(crate) fn game_tick(&mut self) {
        profiler::push("Keybindings");
        self.debug_tick();
        self.keys_tick();
        profiler::pop_push("gui");
        if self.pause.on {
            self.paused_tick();
            return;
        }
        // Frozen world (`/tick freeze`): entities, textures, particles, weather and clocks stopped.
        let runs = self.exact.rate.begin_tick();
        self.chat.tick();
        if self.positioned {
            self.cooldowns.tick();
        }
        self.commands_tick();
        self.hud_extra.tick();
        self.front.loading_tick(); // loading screen: smoothed progress
        world_list::tick_autosave(self.solo.as_ref().is_some_and(rc_sim::Handle::saving));
        // Music then ambiences (sounds without a packet), before the sound engine (`sounds.md` § 3.8).
        if let Some(a) = self.audio.get() {
            let view =
                environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
            let ctx = world_sounds::TickCtx {
                view: &view,
                env: &mut self.env,
                player: self.positioned.then_some(&self.player),
                camera: self.eye,
                dimension: &self.dimension,
                boss_flags: self.hud_extra.boss_flags(),
                // Connection in progress (before the first position): world loading screen.
                loading: !self.positioned,
                paused: false,
            };
            self.world_sounds.tick_end_flash(a, self.shot.pos);
            self.world_sounds.tick(a, ctx);
        }
        self.tick_audio();
        entity_render::set_game_time(self.env.env.clocks.game_time);
        // World thunderstorm: world with weather, thunder > 0.9.
        let weather_ok = self.env.has_sky && !self.dimension.ends_with("the_end");
        entity_render::set_thundering(weather_ok && self.env.env.weather.thunder() > 0.9);
        profiler::pop_push("entities");
        {
            let view =
                environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
            entity_render::water_track(&mut self.entities, &self.data, &self.data.entity_dims, &|x, y, z| {
                view.state(x, y, z)
            });
            // Start of the parrots' tick (frozen world: no tick, nothing forgotten).
            if runs {
                let data = &self.data;
                entity_render::party_track(&mut self.entities, &|j| {
                    data.block_of(view.state(j[0], j[1], j[2])).is_some_and(|b| b.name == "minecraft:jukebox")
                });
            }
        }
        ticking::tick_entities(&mut self.entities, runs);
        // Empty unless a body rests on a redstone ore (no allocation otherwise).
        let mut bodies_on_redstone_ore: Vec<[i32; 3]> = Vec::new();
        if runs {
            // Projectiles, items, orbs, falling blocks, TNT: also simulated client-side between
            // two server resyncs (`projectile_sim.rs`, `entity_motion.rs`).
            let zones = entity_motion::shulker_peeks(&mut self.entities);
            let pushable = self.positioned && self.env.game_mode != 3 && self.moves_freely();
            let colliders = entity_motion::colliders(&self.entities, None, None);
            let world = GridWorld {
                grid: &self.world.grid,
                min_y: self.world.min_y,
                height: self.world.height,
                t: &self.phys,
                border: &self.weather.border,
                colliders: &colliders,
                movers: Some(&self.block_entities),
                noclip: None,
                palette_prefilter: crate::grid_world::palette_prefilter(),
            };
            if !zones.is_empty() {
                // Opening shulkers push the local player (their own collision world, as for
                // their movement) and the bodies simulated here.
                let mine = entity_motion::colliders(&self.entities, None, Some(self.player.pos[1]));
                let pw = GridWorld { colliders: &mine, ..world };
                let me = pushable.then_some((&mut self.player, &pw as &dyn rc_physics::World));
                entity_motion::shulker_push(&mut self.entities, &zones, me, &world, &self.phys);
            }
            let me = rc_physics::body::Follow {
                id: self.player_id,
                pos: self.player.pos,
                eye: self.player.eye_height(),
                spectator: !self.positioned || self.player.spectator,
                dead: self.hud.dead(),
            };
            let players = &self.players;
            let spectator = |u: u128| players.map.get(&u).is_some_and(|p| p.game_mode == 3);
            let (time, lava) = (self.env.env.clocks.game_time, self.player.fast_lava);
            let ps = &mut self.particles;
            projectile_sim::tick(&mut self.entities, &self.phys, &world, me, time, lava, &self.misc.power, ps);
            // Bodies resting on a redstone ore: its reaction runs on this client too, after them.
            let (data, ores) = (&self.data, self.act.redstone_ore_blocks);
            let mut on_ore = |p: [i32; 3], s: u32| {
                if data.state(s).is_some_and(|i| ores.contains(&i.block)) {
                    bodies_on_redstone_ore.push(p);
                }
            };
            entity_motion::tick(&mut self.entities, &world, &self.phys, me, lava, &spectator, ps, &mut on_ore);
        }
        for p in bodies_on_redstone_ore {
            self.block_stepped_on(p);
        }
        capseq::log_tick(&self.entities);
        // Mob effect swirls: in a frozen world, players and their mounts still tick.
        self.tick_mob_effect_particles(runs);
        // Block entities also stopped when the world is frozen.
        profiler::pop_push("blockEntities");
        if runs {
            self.tick_block_entities();
            self.tick_entity_particles();
        }
        self.exact_tick();
        self.misc_tick(runs);
        // Without an open screen, a dead player (connection while dead) sees the death screen without cause.
        if self.death.is_none() && self.chat.input.is_none() && self.hud.dead() {
            self.open_death(None);
        }
        self.inventory.tick();
        self.creative_tick();
        self.book_tick();
        self.invtest_tick();
        self.special_tick();
        self.bed_tick();
        self.win_tick();
        self.menus_tick();
        self.solo_pause();
        let held = self.inventory.hotbar(self.hud.selected).map(|s| {
            let view = stack::StackRef {
                stack: s,
                info: usize::try_from(s.item).ok().and_then(|i| self.data.items.get(i)),
                ids: &self.component_ids,
                data: &self.data,
                hold: stack::Hold::MAIN,
            };
            (s.item, view.display_name())
        });
        self.hud.tick_held(held);
        if let Some(d) = &mut self.death {
            d.tick();
            // Mouse-free test: click on "Respawn" after 60 ticks.
            if rc_data::env_flag!("RUSTCRAFT_DEATHTEST") && d.ticks() == 60 {
                let _ = self.session.link.send_client_command(0);
            }
        }
        // Following emitters, music notification (`world_fx.rs`).
        if runs {
            self.fx_tick();
            self.entity_events_tick();
        }
        self.bundle_sound();
        self.social_toast_tick();
        self.tutorial_tick();
        // Animation of the blocks around the player (`block_anim.rs`), before the particles tick.
        profiler::pop_push("animateTick");
        if runs && self.positioned {
            self.tick_block_anim();
        }
        let view = environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
        profiler::pop_push("level");
        self.env.frozen = !runs;
        self.env.boss_flags = self.hud_extra.boss_flags();
        self.env.hide_flashes = self.options.b("hideLightningFlashes");
        self.env.tick(&view, self.player.eyes_in_water);
        // End lightning started: delayed sound, except in front of the end screen.
        if let Some(angles) = self.env.end_flash_started()
            && self.win.is_none()
        {
            self.world_sounds.queue_end_flash(angles);
        }
        self.weather.ticks += 1;
        if self.weather.ticks.is_multiple_of(20) && rc_data::env_flag!("RUSTCRAFT_WEATHERLOG") {
            let (t, rain) = (self.env.env.clocks.game_time, self.env.env.weather.rain);
            eprintln!("{}, time {t}, rain {rain}", self.weather.debug_line(&view, self.eye));
        }
        // Particles: test emissions, then engine tick (current world and camera).
        profiler::pop_push("particles");
        if self.positioned {
            self.particles.test_emit(self.eye, self.yaw);
        }
        self.particles.set_biomes(&self.palette, self.video.mesh.blend, self.env.plains());
        // Vibration signals: target followed before the tick (fields borrowed one by one).
        let (ents, me) = (&self.entities, (self.player_id, self.player.pos));
        self.particles.track_vibrations(|id| if id == me.0 { Some(me.1) } else { ents.map.get(&id).map(|e| e.pos) });
        let pw = particles::TickWorld {
            grid: &self.world.grid,
            min_y: self.world.min_y,
            height: self.world.height,
            phys: &self.phys,
            player: self.positioned.then_some((self.player.pos, self.player.vel)),
            scope: (self.positioned && self.scoping_first_person()).then_some(self.eye),
        };
        if runs {
            self.particles.tick(&pw, self.eye);
            // Particle sounds (drips, fireworks).
            for s in self.particles.take_sounds() {
                if let Some(a) = self.audio.get() {
                    s.play(a, self.eye);
                }
            }
        }
        // Weather: drops on the ground, rain sound, lightning (`weather-border.md` § 2-3).
        if runs {
            let view =
                environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
            let ctx = weather::TickCtx {
                view: &view,
                data: &self.data,
                phys: &self.phys,
                particles: &mut self.particles,
                audio: self.audio.get(),
                entities: &self.entities,
                eye: self.shot.pos,
                rain: self.env.env.weather.rain,
                game_time: self.env.env.clocks.game_time,
                radius: self.options.i("weatherRadius"),
                particle_level: self.options.i("particles"),
            };
            for id in self.weather.tick(ctx) {
                self.entities.map.remove(&id);
            }
            self.env.sky_flash = self.weather.sky_flash > 0 && !self.options.b("hideLightningFlashes");
            self.hud.border_warn = self.weather.border.warning(self.player.pos[0], self.player.pos[2]);
        }
        let mut eye = self.player.pos;
        eye[1] += self.player.eye_height();
        self.hud.tick(self.env.eye_brightness(&view, eye));
        self.video.darkness.tick();
        self.hud.more.tick(&hud_more::TickIn {
            air: self.entities.me.as_ref().and_then(|e| e.meta_int(1)),
            eyes_in_water: self.player.eyes_in_water,
            vehicle: hud_more::vehicle_of(&self.entities, self.player_vehicle(), &self.attr_names),
            frozen: self.entities.me.as_ref().and_then(|e| e.meta_int(7)),
            in_portal: self.positioned
                && self.env.game_mode != 3
                && hud_more::in_portal(&self.data, |x, y, z| view.state(x, y, z), self.player.pos),
            // Keyboard-free test: RUSTCRAFT_JUMPTEST holds the mount jump charge.
            jump: (self.grabbed && self.binds.down(keys::JUMP))
                || rc_data::env_flag!("RUSTCRAFT_JUMPTEST")
                || self.riding.script_jump(),
        });
        self.riding.prune(&self.entities);
        if let Some(power) = self.hud.more.take_jump() {
            let _ = self.session.link.send_player_command_data(rc_client::link::action::START_RIDING_JUMP, power);
            self.riding.jump = Some(power);
        }
        self.sun_angle = self.env.sun_angle();
        let link = self.session.link.clone();
        if !self.positioned {
            self.tick_pistons();
            return;
        }
        profiler::pop_push("playerSync");
        // Carried slot: sent at the tick following the change (`hud.md` § 6.2).
        if let Some(slot) = self.hud.take_carried() {
            let _ = link.send_carried_item(slot);
        }
        if !link.is_loaded() {
            // End of the world loading screen (`flow.rs`).
            if !self.world_loaded() {
                self.tick_pistons();
                return;
            }
            let _ = link.send_player_loaded();
            // Keyboard-free test: a message and a command on entering the game.
            // `RUSTCRAFT_CHATTEST=<command>`: this command instead of the time query.
            if let Some(v) = rc_data::env_os!("RUSTCRAFT_CHATTEST") {
                let _ = link.send_chat("Hello from Rustcraft");
                let cmd = v.to_str().filter(|s| s.len() > 1).unwrap_or("time query daytime");
                let _ = link.send_command(cmd);
            }
        }
        // Clicks, mining, use and action keys (`interactions.md`), before the physics.
        self.act_tick();
        let k = |i: usize| self.grabbed && self.binds.down(i);
        let input = Input {
            forward: k(keys::FORWARD),
            backward: k(keys::BACK),
            left: k(keys::LEFT),
            right: k(keys::RIGHT),
            jump: k(keys::JUMP),
            shift: k(keys::SNEAK),
            sprint: k(keys::SPRINT),
        };
        let mut input = if self.free_cam { Input::default() } else { input };
        // Keyboard-free test: RUSTCRAFT_AUTOWALK=<seconds> walks forward (sprint, jump every 2 s).
        if let Some(secs) = self.autowalk
            && self.started.elapsed().as_secs_f64() < secs
        {
            self.autowalk_ticks += 1;
            input.forward = true;
            input.sprint = true;
            input.jump = self.autowalk_ticks % 40 < 2;
            // Turns slowly to meet some terrain.
            self.yaw += 0.5;
        }
        if !self.free_cam {
            self.player.yaw = self.yaw;
            self.player.pitch = self.pitch;
        }
        self.prev_pos = self.player.pos;
        // Piloted vehicle, passengers on their seat; camera elsewhere: motionless (`riding.rs`).
        let free = self.moves_freely() && self.sleeping_pos().is_none();
        self.ride_tick(&mut input);
        // Equipment (elytra, boots), attached rockets, pushing neighbours (`player_env.rs`).
        self.player_env_tick();
        // Boats, shulkers, happy ghasts: solid for the player too.
        let colliders = entity_motion::colliders(&self.entities, None, Some(self.player.pos[1]));
        let world = GridWorld {
            grid: &self.world.grid,
            min_y: self.world.min_y,
            height: self.world.height,
            t: &self.phys,
            border: &self.weather.border,
            colliders: &colliders,
            movers: Some(&self.block_entities),
            noclip: None,
            palette_prefilter: crate::grid_world::palette_prefilter(),
        };
        self.player.spectator = self.env.game_mode == 3;
        let sig = if free { self.player.tick(input, &world, &self.phys) } else { Default::default() };
        // Auto jump: the forced jump also goes in the inputs packet.
        input.jump |= sig.auto_jump;
        if let Some(a) = self.audio.get() {
            let moved = std::array::from_fn(|i| self.player.pos[i] - self.prev_pos[i]);
            self.world_sounds.steps(a, &self.data, &self.phys, &world, &self.player, moved);
            self.world_sounds.tick_elytra(a, &self.player);
            let landed = sig.landed_on.map(|(q, fall)| (world.state(q[0], q[1], q[2]).unwrap_or(0), fall));
            self.world_sounds.landing_and_slides(a, &self.phys, landed, sig.honey_slides, self.player.pos);
        }
        // Block walked on without sneaking: its step effect runs on this client too.
        if let Some(under) = sig.stepped_on {
            self.block_stepped_on(under);
        }
        // Moving blocks after the player's own movement, before sending its
        // position (the game: entities, block entities, then player synchronisation).
        self.tick_pistons();
        self.tick_camera();
        self.hud.detached = self.camera.view.detached();
        if let Some(t) = &mut self.trace {
            use std::io::Write;
            let (p, v) = (self.player.pos, self.player.vel);
            let _ = writeln!(
                t,
                "{:.4} {:.4} {:.4} {:.4} {:.4} {:.4} {}",
                p[0],
                p[1],
                p[2],
                v[0],
                v[1],
                v[2],
                u8::from(self.player.on_ground)
            );
        }
        if let Some(f) = sig.flying {
            let _ = link.send_abilities(f);
        }
        if sig.start_fall_flying {
            let _ = link.send_player_command(rc_client::link::action::START_FALL_FLYING);
        }
        if self.last_input != Some(input.bits()) {
            let _ = link.send_input(input.bits());
            self.last_input = Some(input.bits());
        }
        if let Some(on) = sig.sprinting {
            use rc_client::link::action;
            let _ = link.send_player_command(if on { action::START_SPRINTING } else { action::STOP_SPRINTING });
        }
        let p = &self.player;
        if !free {
            // Passenger or camera on another entity (`riding.rs`).
            self.send_moves_exact();
        } else if let Some(m) = self.sync.tick(p.pos, p.yaw, p.pitch, p.on_ground, p.horizontal_collision) {
            let _ = link.send_move(m, p.on_ground, p.horizontal_collision);
        }
        let _ = link.send_tick_end();
    }
}
