//! Handling of the events the session delivers: one `App::apply` call per event received from the server.

use rc_client::Event;
use rc_mesh::blocks::{self, Tables};
use rc_world::Column;
use std::sync::Arc;

use crate::grid_world::level_grid;
use crate::{
    App, environ, hud_more, interact, item_regs, light_engine, maps, menu, menu_special, particles, player_env,
    player_position_values, resolve_teleport, scoreboard, screens_work, skins, tooltips, tooltips_items, weather,
    widgets,
};

/// Attributes of another entity (`update_attributes`, `names`: the `minecraft:attribute`
/// registry) kept by the entity: every value (mounts, mount hearts), and the render scale and
/// name tag distances in their own fields.
pub(crate) fn store_attributes(
    e: &mut crate::entities::Entity,
    attributes: &[rc_proto_v777::play::Attribute],
    names: &[String],
) {
    for a in attributes {
        let Some(name) = names.get(a.id as usize) else { continue };
        let Ok(id) = u16::try_from(a.id) else { continue };
        let value = hud_more::attribute_value(name, a);
        if rc_data::env_flag!("RUSTCRAFT_RIDELOG") {
            eprintln!("attribute {} {name} = {value}", e.id);
        }
        e.set_synced_attribute(id, value);
        match name.as_str() {
            "minecraft:scale" => e.scale = value as f32,
            "minecraft:name_tag_distance" => e.tag_dist = value,
            "minecraft:below_name_distance" => e.below_dist = value,
            _ => {}
        }
    }
}

impl App {
    pub(crate) fn apply(&mut self, ev: Event) {
        let air = |s: u32, t: &Tables| t.is_air(s);
        match ev {
            Event::Joined { login, height, biomes, dimension, flat } => {
                self.player_id = login.player_id;
                self.debug.logs.subscribed = false; // F3+2 subscriptions sent back to the server
                self.unsecure_toast(login.enforces_secure_chat);
                self.cmd.data.dimensions = login.dimensions.clone();
                self.entities.local = login.player_id;
                self.entities.clear();
                self.reset_riding_world();
                self.misc.reset_world();
                self.progress.join();
                // Local player, drawn in third person (`hand-camera.md` § 3.3).
                let kind =
                    self.entities.type_names.iter().position(|n| n == "minecraft:player").map_or(-1, |i| i as i32);
                self.entities.spawn_me(self.local_uuid, kind, self.player.pos, self.yaw);
                self.particles.clear();
                self.ev.totem.reset();
                self.ev.reset_world();
                self.act.reset_world();
                self.palette = Arc::new(blocks::biome_palette(&self.data, &self.colormaps, &biomes));
                self.env.join(&self.data, &dimension, &biomes, flat);
                crate::environ::set_biome_zoom_seed(login.spawn.hashed_seed);
                self.world_sounds.join(self.audio.get());
                self.dimension = dimension.clone();
                self.env.server_view = login.view_distance;
                self.env.game_mode = login.spawn.game_mode;
                self.last_death = login.spawn.last_death.as_ref().map(|(d, p)| (d.clone(), [p.x, p.y, p.z]));
                // Quick play log (`--quickPlayPath`), at each entry into the game.
                let lang = self.renderer.as_ref().map(|r| &r.gui.lang);
                self.quick_log.log(login.spawn.game_mode, || {
                    lang.map_or_else(|| "Minecraft Server".into(), |l| widgets::tr(l, "selectServer.defaultName", &[]))
                });
                self.hud.hardcore = login.hardcore;
                self.online_mode = login.online_mode;
                self.debug.joined(login.simulation_distance, login.reduced_debug_info);
                self.hud_extra.reset();
                self.player.effects = Default::default();
                self.hud.more.clear_waypoints();
                maps::store().clear();
                self.scores = scoreboard::Scoreboard::default();
                if self.hud_extra.effect_names.is_empty() {
                    self.hud_extra.effect_names =
                        self.data.registry("minecraft:mob_effect").map(|r| r.entries.clone()).unwrap_or_default();
                    self.hud_extra.effect_colors = self.data.effect_colors.clone();
                }
                self.show_death_screen = login.show_death_screen;
                self.death = None;
                (self.win, self.world_sounds.screen_music) = (None, None);
                self.block_anim.set_biomes(&biomes);
                self.biome_names = biomes;
                self.world.grid = level_grid(login.view_distance);
                self.block_entities.clear();
                self.tech.clear();
                self.world.min_y = height.min_y;
                self.world.height = height.height;
                let sea = Some(login.spawn.sea_level);
                self.weather.join(&self.data, &self.biome_names, sea, height.min_y, height.height);
                self.set_shade(height.nether_light);
                if let Some(r) = &self.renderer
                    && let Some(e) = &r.entities
                {
                    e.set_dimension(&r.queue_handle(), height.nether_light);
                }
                self.world.versions.clear();
                self.world.meshes.clear();
                if let Some(r) = &mut self.renderer {
                    r.clear();
                }
            }
            Event::Respawn { height, dimension, flat, keep, last_death } => {
                self.last_death = last_death;
                self.hud.respawn(keep & 2 != 0);
                // New player: no effect until the server sends some back.
                self.player.effects = Default::default();
                // New player: player-specific states reset to zero except retention of the entity
                // data (flag 2).
                if keep & 2 == 0 {
                    self.player.reset_entity_state();
                }
                self.entities.clear();
                self.reset_riding_world();
                self.entities.respawn_me(keep & 2 != 0, self.player.pos, self.yaw);
                self.misc.reset_world();
                self.particles.clear();
                self.ev.totem.reset();
                self.ev.reset_world();
                self.act.reset_world();
                if self.death.take().is_some() && !self.front.is_open() {
                    self.grab(true);
                }
                let names = std::mem::take(&mut self.biome_names);
                self.env.join(&self.data, &dimension, &names, flat);
                self.biome_names = names;
                self.world_sounds.respawn(self.audio.get(), dimension != self.dimension);
                // Current dimension (music, compass, thunderstorm): remained that of the connection.
                self.dimension = dimension;
                // New client world at the server's view radius (that of the connection or of the
                // last `set_chunk_cache_radius`), like the game: a fixed radius of 16 made the columns
                // of a wider server view overlap.
                self.world.grid = level_grid(self.env.server_view);
                self.block_entities.clear();
                self.tech.clear();
                self.world.min_y = height.min_y;
                self.world.height = height.height;
                self.weather.join(&self.data, &self.biome_names, None, height.min_y, height.height);
                self.set_shade(height.nether_light);
                if let Some(r) = &self.renderer
                    && let Some(e) = &r.entities
                {
                    e.set_dimension(&r.queue_handle(), height.nether_light);
                }
                self.world.versions.clear();
                self.world.meshes.clear();
                if let Some(r) = &mut self.renderer {
                    r.clear();
                }
            }
            Event::ViewDistance(d) if d > 0 => {
                self.world.grid.set_radius(d as u32);
                self.env.server_view = d;
            }
            Event::Chat { component, tag } => {
                self.menus.world.chat_line();
                self.chat.add(*component, tag);
            }
            Event::ChatSignature(sig) => self.chat.attach_signature(sig),
            Event::ChatOriginal(text) => self.chat.attach_original(text),
            Event::ChatSender(uuid) => {
                let name = self.players.map.get(&uuid).map(|p| p.name.clone()).unwrap_or_default();
                self.menus.world.chat_sender(uuid, name);
            }
            Event::DeleteChat(sig) => self.chat.delete(sig),
            Event::Commands(c) => self.cmd.set_tree(&c),
            Event::CommandSuggestions(s) => self.op_suggestions(s),
            Event::ChatCompletions { action, entries } => self.cmd.set_completions(action, entries),
            Event::PlayerInfo(u) => {
                self.players.apply(u);
                // `RUSTCRAFT_SKINTEST=<name>`: public signed profile of that name applied to the
                // other players without a skin (test of the download and of the signature).
                if let Some(name) = rc_data::env_str!("RUSTCRAFT_SKINTEST") {
                    skins::apply_test_profile(&mut self.players, name, self.local_uuid);
                }
                self.skins.update(&self.players, self.local_uuid);
            }
            Event::PlayersRemoved(u) => self.players.remove(&u),
            Event::TabList { header, footer } => (self.tab_list.header, self.tab_list.footer) = (header, footer),
            Event::Health { health, food, saturation } => self.hud.set_health(health, food, saturation),
            Event::Cooldown { group, ticks } => self.cooldowns.apply(&group, ticks),
            Event::Experience { progress, level, .. } => self.hud.set_experience(progress, level),
            Event::HeldSlot(s) => self.hud.held_slot(s),
            Event::SpawnPosition { dimension, pos } => self.spawn = Some((dimension, pos)),
            // Applied to the render at the next frame (it may not exist yet).
            Event::Registry { name, entries } => {
                if name == "minecraft:test_instance" {
                    self.set_test_registry(&entries);
                }
                if name == "minecraft:damage_type" {
                    self.ev_damage_registry(&entries);
                }
                if name == "minecraft:instrument" {
                    self.act.instruments = interact::instrument::table(&entries, |p| self.assets.read(p));
                }
                self.cmd.registry(&name, &entries);
                self.creative_rec.registry(&name, &entries);
                tooltips_items::set_registry(&name, &entries);
                item_regs::set_registry(&name, &entries);
                if name == "minecraft:enchantment" {
                    self.screens.hasher.enchantments = entries.into_iter().map(|e| e.0).collect();
                    // Tooltips name the enchantments of a stack from this list.
                    tooltips::world_inputs_changed();
                } else if name == "minecraft:banner_pattern" {
                    screens_work::set_patterns(&entries);
                } else if name == "minecraft:jukebox_song" {
                    self.fx.set_songs(&entries);
                } else {
                    self.packs.remember_registry(&name, &entries);
                    self.pending_registries.push((name, entries));
                }
            }
            Event::OpenScreen { window, kind, title } => {
                self.inventory.open(window, menu::Kind::from_registry(kind), title.map(|t| *t));
                self.screens.show();
                self.keys.clear();
                self.grab(false);
            }
            Event::ContainerClosed => {
                self.inventory.container = None;
                if self.screens.open {
                    self.screens.hide();
                    self.grab(true);
                }
            }
            ev @ (Event::MerchantOffers(_)
            | Event::UpdateRecipes(_)
            | Event::MountScreen { .. }
            | Event::OpenBook(_)
            | Event::OpenSignEditor { .. }) => self.special_apply(ev),
            Event::TagQuery { id, nbt } => self.tag_query(id, nbt),
            Event::TestInstanceStatus { status, size } => self.op_test_status(status.map(|s| *s), size),
            Event::GameTestHighlight { absolute, relative } => {
                self.tech.highlight(absolute, relative, std::time::Instant::now());
            }
            Event::ContainerData { window, prop, value } => {
                if rc_data::env_flag!("RUSTCRAFT_INVLOG") {
                    eprintln!("property: window {window}, {prop} = {value}");
                }
                self.inventory.set_data(window, prop, value)
            }
            Event::Score(s) => self.scores.apply(*s),
            Event::Sound(s) => self.play_sound(s),
            Event::Particles(p) => self.level_particles(&p),
            Event::LevelEvent { kind, pos, data, global } => self.on_level_event(kind, pos, data, global),
            Event::StopSound { category, name } => {
                if let Some(a) = self.audio.get() {
                    a.stop(category.and_then(rc_audio::Category::from_id), name.as_deref());
                }
            }
            Event::Boss(b) => self.hud_extra.boss_event(b),
            Event::Title(t) => self.hud_extra.set_title(t),
            Event::Subtitle(t) => self.hud_extra.set_subtitle(t),
            Event::TitleTimes(t) => self.hud_extra.set_times(t),
            Event::ClearTitles(reset) => self.hud_extra.clear_titles(reset),
            Event::Entity(e) => {
                if rc_data::env_flag!("RUSTCRAFT_ENTLOG") {
                    eprintln!("entity: {e:?}");
                }
                self.bed_entity_event(&e);
                self.m6_entity_event(&e);
                self.entity_event_fx(&e);
                // Effects of the local player: HUD (`hud-advanced.md` § 2).
                match e {
                    rc_client::EntityEvent::Effect { id, effect, amplifier, duration, flags }
                        if id == self.player_id =>
                    {
                        self.hud_extra.effect(effect, amplifier, duration, flags);
                        let name = self.hud_extra.effect_names.get(effect as usize).map(String::as_str);
                        self.video.darkness.effect(name, duration, flags);
                        self.hud.more.effect(name, duration, flags);
                        self.player.effects.set(name.unwrap_or(""), Some(amplifier));
                    }
                    rc_client::EntityEvent::RemoveEffect { id, effect } if id == self.player_id => {
                        self.hud_extra.remove_effect(effect);
                        self.video.darkness.remove(self.hud_extra.effect_names.get(effect as usize).map(String::as_str));
                        self.hud
                            .more
                            .remove_effect(self.hud_extra.effect_names.get(effect as usize).map(String::as_str));
                        if let Some(n) = self.hud_extra.effect_names.get(effect as usize) {
                            self.player.effects.set(n, None);
                        }
                    }
                    // Reduced info (22, 23) and permission level (24 to 28): F3 screen.
                    rc_client::EntityEvent::Event { id, event } if id == self.player_id => {
                        self.debug.player_event(i32::from(event));
                        if (24..=28).contains(&event) {
                            self.cmd.data.op_level = i32::from(event) - 24;
                        }
                    }
                    // Knockback (hits, arrows, wind charges…): velocity of the local player replaced.
                    rc_client::EntityEvent::Motion { id, velocity } if id == self.player_id => {
                        self.player.set_motion(velocity);
                    }
                    // Glide and spin attack as the server sends them back.
                    rc_client::EntityEvent::Data { id, ref values } if id == self.player_id => {
                        player_env::local_flags(values, &mut self.player);
                    }
                    // Player entity added while listed: kept by the social interactions screen.
                    rc_client::EntityEvent::Add(ref a)
                        if self.entities.type_names.get(a.kind as usize).is_some_and(|n| n == "minecraft:player") =>
                    {
                        if let Some(p) = self.players.map.get(&a.uuid) {
                            self.menus.world.spotted(a.uuid, p.name.clone());
                        }
                    }
                    _ => {}
                }
                // Mount removed from under the player: its teleport applies to the player (without acknowledgement).
                if let Some(t) = self.entities.track_player_vehicle(&e, self.player_id) {
                    let p = rc_proto_v777::play::PlayerPosition {
                        id: t.id,
                        pos: t.pos,
                        delta: t.velocity,
                        yaw: t.yaw,
                        pitch: t.pitch,
                        relative: t.relative,
                    };
                    let (pos, vel, yaw, pitch) =
                        resolve_teleport(&p, self.player.pos, self.player.vel, self.yaw, self.pitch);
                    (self.player.pos, self.player.vel, self.prev_pos) = (pos, vel, pos);
                    (self.yaw, self.pitch) = (yaw, pitch);
                }
                // Locally piloted vehicle: only `move_vehicle` moves it (`riding.rs`).
                if self.piloted_move(&e) {
                    return;
                }
                let added = if let rc_client::EntityEvent::Add(a) = &e { Some(a.id) } else { None };
                self.entities.apply(e);
                if !self.entities.pending_sounds.is_empty() {
                    let sounds = std::mem::take(&mut self.entities.pending_sounds);
                    self.play_fx_sounds(&sounds);
                }
                // Sustained sounds created at spawn (minecarts, bees: `entity_loops.rs`).
                if let Some(id) = added {
                    self.loop_sound_added(id);
                }
            }
            Event::Inventory(inv) => {
                if rc_data::env_flag!("RUSTCRAFT_INVLOG") {
                    eprintln!("inventory: {inv:?}");
                }
                self.inventory.apply(inv);
            }
            Event::Died { entity, message } if entity == self.player_id => self.open_death(message.map(|m| *m)),
            Event::ActionBar(c) => self.chat.action_bar(*c),
            Event::LightUpdate { x, z, light } => {
                // Check of the client engine: differences between our light and the server's.
                if rc_data::env_flag!("RUSTCRAFT_LIGHTCHECK")
                    && let Some(col) = self.world.grid.get(x, z)
                {
                    let diff = |ours: &[rc_world::LightLayer], theirs: &[rc_world::LightLayer]| {
                        ours.iter()
                            .zip(theirs)
                            .filter_map(|(a, b)| Some((a.as_ref()?, b.as_ref()?)))
                            .map(|(a, b)| {
                                (0..rc_world::LIGHT_BYTES * 2)
                                    .filter(|&i| (a[i >> 1] >> ((i & 1) * 4)) & 15 != (b[i >> 1] >> ((i & 1) * 4)) & 15)
                                    .count()
                            })
                            .sum::<usize>()
                    };
                    eprintln!(
                        "light ({x}, {z}): differences block {}, sky {}",
                        diff(&col.block_light, &light.block),
                        diff(&col.sky_light, &light.sky)
                    );
                }
                let o = light_engine::opts();
                if o.solo_cut && self.solo.is_some() {
                    self.light_recv.note(x, z, &light.sky, &light.block);
                }
                if let Some(s) = &mut self.light_shadow {
                    s.update(x, z, &light.sky, &light.block);
                }
                // Sections where a light cell changes (compared before the replacement).
                // Nothing changed (`LIGHTNOCOPY`): the column, perhaps held by a meshing task,
                // is not copied to put the same layers back there.
                let changed = self.world.touch_light(x, z, &light.sky, &light.block, o.no_copy);
                if (changed || !o.no_copy)
                    && let Some(col) = self.world.grid.get_mut_by(x, z, rc_world::ColumnWriter::LightUpdate)
                {
                    for (dst, src) in [(&mut col.sky_light, light.sky), (&mut col.block_light, light.block)] {
                        for (d, s) in dst.iter_mut().zip(src) {
                            if s.is_some() {
                                *d = s;
                            }
                        }
                    }
                    self.weather.rev += 1;
                }
            }
            Event::BlocksAheadOfLight { section } => {
                self.light_recv.forget(section[0], section[1] - (self.world.min_y >> 4), section[2]);
            }
            Event::Border(p) => self.weather.on_border(p),
            Event::Time { game_time, clocks } => self.env.env.clocks.set_time(game_time, &clocks),
            Event::GameEvent { kind, param } => {
                self.env.env.weather.event(kind, param);
                // Game mode change.
                if kind == 3 {
                    self.env.game_mode = param as i32;
                }
                // Rule "immediate respawn".
                if kind == 11 {
                    self.show_death_screen = param == 0.0;
                }
                self.m5_game_event(kind, param);
            }
            Event::Chunk(c) => {
                let c = *c;
                let (x, z) = (c.x, c.z);
                let mut col = Column {
                    x,
                    z,
                    sections: c.sections.into_boxed_slice(),
                    sky_light: c.light.sky.into_boxed_slice(),
                    block_light: c.light.block.into_boxed_slice(),
                    heightmaps: c.heightmaps,
                };
                if let Some(old) = self.world.grid.get(x, z) {
                    col.inherit_heightmaps(old);
                }
                col.complete_heightmaps(&self.world.hm_rules, self.world.height as usize);
                if let Some(s) = &mut self.light_shadow {
                    s.chunk(x, z, &col.sky_light, &col.block_light);
                }
                self.world.grid.insert(col);
                crate::perf_log::chunk_received(self.started);
                let view =
                    environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
                self.block_entities.load_chunk(x, z, &c.block_entities, |x, y, z| view.state(x, y, z));
                self.tech.load_chunk(x, z, &c.block_entities);
                self.world.touch_with_neighbors(x, z);
                if let Some(col) = self.world.grid.get(x, z) {
                    let b = weather::Blocking { data: &self.data, phys: &self.phys };
                    self.weather.on_chunk(col, &b);
                    if let Some(r) = &mut self.renderer {
                        r.note_loaded_column(x, z, &col.sections);
                    }
                }
            }
            Event::BlockEvent { pos, b0, b1, .. } => {
                self.piston_event(pos, b0, b1);
                self.block_entities.on_event(pos, b0, b1, self.env.env.clocks.game_time);
                let view =
                    environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
                if let Some((kind, at, vel)) = self.block_entities.note_particle(view.state(pos[0], pos[1], pos[2]), pos) {
                    self.particles.spawn(kind, &particles::Options::default(), at, vel);
                }
            }
            Event::BlockEntityData { pos, kind, nbt } => {
                self.op_block_data(pos, nbt.as_ref());
                self.tech.on_data(pos, kind, nbt.as_ref());
                self.block_entities.on_data(pos, kind, nbt.as_ref())
            }
            Event::ForgetChunk { x, z } => {
                self.block_entities.forget_chunk(x, z);
                self.tech.forget_chunk(x, z);
                self.world.grid.remove(x, z);
                if let Some(s) = &mut self.light_shadow {
                    s.forget(x, z);
                }
                self.weather.forget(x, z);
                self.world.versions.remove(&(x, z));
                self.world.meshes.forget((x, z));
                if let Some(r) = &mut self.renderer {
                    r.remove(x, z);
                }
            }
            Event::BlockChangedAck(seq) => self.act_ack(seq),
            Event::BlockDestruction { id, pos, stage } => self.act.crack(id, pos, i32::from(stage)),
            Event::BlockUpdate { pos, state } => {
                // Blocks awaiting a prediction: the received state is only stored (§ 0.1).
                if self.act.server_block([pos.x, pos.y, pos.z], state) {
                    return;
                }
                let (cx, cz) = (pos.x >> 4, pos.z >> 4);
                let sy = (pos.y - self.world.min_y) >> 4;
                let idx = rc_world::block_index((pos.x & 15) as usize, (pos.y & 15) as usize, (pos.z & 15) as usize);
                let origin = self.server_origin(cx, sy, cz);
                self.update_section_lit(cx, cz, sy, &[(idx as u16, state)], air, origin);
            }
            Event::SectionUpdate { section, changes } => {
                let sy = section[1] - (self.world.min_y >> 4);
                let changes = self.act.filter_section(section, changes);
                let origin = self.server_origin(section[0], sy, section[2]);
                self.update_section_lit(section[0], section[2], sy, &changes, air, origin);
            }
            Event::Teleport(p) => {
                // Passenger: position ignored, acknowledged with the current position (like the game).
                let passenger = self.player_vehicle().is_some();
                let current = (self.player.pos, self.player.vel, self.yaw, self.pitch);
                let (pos, vel, yaw, pitch) = player_position_values(&p, passenger, current);
                if let Some(t) = &mut self.trace {
                    use std::io::Write;
                    let _ = writeln!(t, "tp {:.4} {:.4} {:.4}", pos[0], pos[1], pos[2]);
                }
                if !passenger {
                    self.player.pos = pos;
                    self.player.vel = vel;
                    self.prev_pos = pos;
                }
                let _ = self.session.link.send_accept_teleport(p.id, pos, yaw, pitch);
                self.act.teleported();
                self.stop_destroy();
                self.positioned = true;
                (self.yaw, self.pitch) = (yaw, pitch);
                if !self.placed {
                    self.eye = [pos[0], pos[1] + self.player.eye_height(), pos[2]];
                    self.placed = true;
                }
            }
            Event::Abilities(a) => {
                self.player.may_fly = a.may_fly;
                self.player.flying = a.flying;
                self.player.fly_speed = a.flying_speed;
                self.world_sounds.instabuild = a.instabuild;
                self.act.invulnerable = a.invulnerable;
                self.camera.walking_speed = a.walking_speed;
            }
            Event::Attributes { entity, attributes } if entity == self.player_id => {
                for a in attributes {
                    if let Some(name) = self.attr_names.get(a.id as usize) {
                        let mods = a.modifiers.into_iter().map(|m| (m.id, m.amount, m.operation)).collect();
                        self.player.set_attribute(name, a.base, mods);
                    }
                }
            }
            Event::Attributes { entity, attributes } => {
                // Kept by the entity (mounts, mount hearts), like the game: an unknown entity's
                // attributes are dropped. Render scale of living entities, distances of the name tags.
                if let Some(e) = self.entities.map.get_mut(&entity) {
                    store_attributes(e, &attributes, &self.attr_names);
                }
            }
            Event::Waypoint(w) => self.hud.more.waypoint(*w),
            Event::MapData(m) => maps::store().apply(m),
            Event::Tags(tags) => {
                self.menus.world.set_tags(&tags);
                self.debug.set_tags(&tags);
                self.binds.set_tags(&tags);
                if let Some((_, list)) = tags.iter().find(|(r, _)| r == "minecraft:dialog" || r == "dialog") {
                    self.exact.dialog_tags = list.clone();
                }
                self.cmd.tags(&tags);
                self.creative_rec.tags(&tags);
                menu_special::set_tags(&tags);
                tooltips::set_tags(&tags);
                tooltips_items::set_block_tags(&tags);
                if let Some((_, block_tags)) = tags.iter().find(|(r, _)| r == "minecraft:block") {
                    self.phys.set_block_tags(block_tags);
                    self.tables.fluid.flow.set_block_tags(block_tags);
                    self.world_sounds.set_block_tags(self.data.blocks.len(), block_tags);
                    self.weather.heights.set_block_tags(self.data.blocks.len(), block_tags);
                    let mut by_name: hashbrown::HashMap<String, Vec<u32>> = block_tags
                        .iter()
                        .map(|(n, ids)| (n.clone(), ids.iter().filter_map(|&i| u32::try_from(i).ok()).collect()))
                        .collect();
                    by_name.values_mut().for_each(|v| v.sort_unstable());
                    self.world.hm_rules = rc_sim::chunk::heightmap_rules(&self.data, &by_name);
                    self.act.tags.set(block_tags);
                    self.block_entities.set_block_tags(block_tags);
                    self.block_anim.set_block_tags(block_tags);
                }
            }
            Event::Disconnected(reason) => {
                eprintln!("disconnected: {reason}");
                self.status = format!("disconnected: {reason}");
            }
            Event::Misc(d) => self.apply_misc(d),
            Event::Progress(p) => self.on_progress(p),
            Event::Recipes(e) => self.on_recipes(e),
            ev => self.apply_exact(ev),
        }
    }
}
