//! Tick of a mob in the game's order (`server-mobs.md` § 1.2, § 3.1): common
//! tick, base tick (fire, air, death, forgetting, ambient sound), the AI step (thresholds, server AI,
//! jump, movement, push), body rotation; then the tick of the world's mobs.

use super::goals::max_head_y;
use super::mob::{Mob, MoveOp};
use super::mth::{DEG, degrees_difference, pitch_to, rotate_if_necessary, rotate_towards, rotlerp_360, yaw_to};
use super::species::Kind;
use crate::server::Server;

impl Server {
    /// Tick of the mobs (after the player's life, before the block broadcast).
    /// Parts of the tick specific to the current player (host, or guest set up by `as_guest`).
    pub(crate) fn mobs_player_tick(&mut self) {
        self.watch_main_hand();
        self.player_memory_tick();
        super::surface::parrot::player_tick(self);
        self.sealife_player_tick(); // F09: player swimming
        self.player_stingers_tick(); // F11: embedded stingers
    }

    /// Contacts of the current player after the mob tick: mount, push, cubes,
    /// pufferfish.
    pub(crate) fn mobs_player_touch(&mut self) {
        self.riding_player_tick(); // F04
        self.mount_menu_tick(); // F04
        self.player_pushes_mobs();
        self.cubes_player_touch();
        self.pufferfish_player_touch(); // F09
    }

    pub(crate) fn mobs_tick(&mut self) {
        self.mobs_player_tick();
        self.rec_world_tick(); // F14: shrieker warnings
        // Frozen (`tick freeze`): spawns, raids, fight and mobs wait, except the
        // player's mount.
        let runs = self.cmd.ticking.runs();
        if runs {
            self.natural_spawn_tick();
            // Special spawners: only if mobs spawn (rule `spawn_mobs`).
            if self.rule("minecraft:spawn_mobs", true) {
                self.phantom_spawner_tick();
                self.patrol_spawner_tick(); // F07
                self.cat_spawner_tick();
                self.village_siege_tick();
                self.trader_spawner_tick(); // F05
            }
            self.raids_tick(); // F07: raids before the entities
            self.end_fight_tick(); // F15: dragon fight (before the entities)
        }
        if runs {
            self.bells_tick(); // rung bells: resonance, raiders glowing
        }
        let player_vehicle = self.mobs.mounts.riding.vehicle;
        // Snapshot: mobs added during this tick wait for the next.
        let n = self.mobs.list.len();
        for i in 0..n {
            let Some(mut m) = self.mobs.list[i].take() else { continue };
            if m.removed || (!runs && player_vehicle != Some(m.id)) {
                self.mobs.list[i] = Some(m);
                continue;
            }
            if self.check_despawn(&mut m) {
                m.removed = true;
                self.mobs.list[i] = Some(m);
                continue;
            }
            // F04: a passenger is ticked by its vehicle.
            if self.ticked_by_vehicle(&mut m) {
                self.mobs.list[i] = Some(m);
                continue;
            }
            let c = ((m.pos[0].floor() as i32) >> 4, (m.pos[2].floor() as i32) >> 4);
            let ticking = (self.mob_chunk_ticking(c) && self.chunks.get(&c).is_some_and(Option::is_some))
                || self.fight_forces_ticking(&m); // F15: arena ticket
            if ticking {
                self.tick_mob(&mut m);
            }
            let (id, carries) = (m.id, !m.mount.passengers.is_empty());
            // G6-F10: on the ground on a sculk sensor or a shrieker (effect block 0.2 under the feet).
            let step = (ticking && m.on_ground && !m.removed && !self.mobs.rec.vib_blocks.is_empty())
                .then(|| (m.support, m.pos, m.kind == Kind::Warden));
            self.mobs.list[i] = Some(m);
            if let Some((support, pos, warden)) = step {
                let src = super::recent::vibration::Src::Living(super::mob::Ent::Mob(id));
                self.sensor_entity_on(pos, support, src, warden);
            }
            if ticking && carries {
                self.tick_passengers(id);
            }
        }
        self.riding_player_tick(); // F04
        self.mount_menu_tick(); // F04
        if runs {
            // Arrows and bolts of mobs: common projectile engine (S8, `projectiles_tick`).
            self.fangs_tick(); // F07
            // F07: witch potions ticked with the projectiles (S8, `projectiles_tick`).
            self.spits_tick(); // F04
            self.bolts_tick(); // F04
            // F06: snowballs of golems ticked with the projectiles (S8, `projectiles_tick`).
            // F08: drowned tridents ticked with the projectiles (S8, `projectiles_tick`).
            // F12: fireballs ticked with the projectiles (S8, `projectiles_tick`).
            // F14: wind charges ticked with the projectiles (S8, `projectiles_tick`).
            self.end_world_tick(); // F15: crystals, breath, bullets
            self.spawner_blocks_tick(); // W9: spawners, trials, vaults (block entities, after the entities)
            self.player_pushes_mobs(); // player push: also stopped when frozen
        }
        self.cubes_player_touch();
        self.pufferfish_player_touch(); // F09
        // Removals, then network tracking.
        let mut gone = Vec::new();
        let mut bars = Vec::new(); // F16: bars of removed withers
        let mut uuids = Vec::new(); // scoreboard: permanently removed entities
        let mut ears = Vec::new(); // vibration listeners (warden, allay) to forget
        let scored = self.cmd.scores.has_entries();
        for slot in self.mobs.list.iter_mut() {
            if slot.as_ref().is_some_and(|m| m.removed) {
                let m = slot.take().expect("mob");
                if matches!(m.kind, Kind::Warden | Kind::Allay) {
                    ears.push(m.id);
                }
                if m.track.seen {
                    gone.push(m.id);
                }
                if m.wither.bar_shown {
                    bars.push(m.wither.bar);
                }
                if scored {
                    uuids.push(m.uuid);
                }
            }
        }
        for id in ears {
            self.listener_remove(super::recent::vibration::Who::Mob(id));
        }
        for b in bars {
            super::wither::boss::bar_removed(self, b);
        }
        let before = self.mobs.list.len();
        for u in uuids {
            self.entity_scores_removed(u);
        }
        self.mobs.list.retain(Option::is_some);
        let shifted = self.mobs.list.len() != before;
        self.mobs.index.refresh(&self.mobs.list, shifted);
        if !gone.is_empty() {
            let _ = self.send(rc_client::Event::Entity(rc_client::EntityEvent::Remove(gone)));
        }
        for i in 0..self.mobs.list.len() {
            let Some(mut m) = self.mobs.list[i].take() else { continue };
            self.send_changes(&mut m);
            self.mobs.list[i] = Some(m);
        }
    }

    /// Tick of a mob taken out of the list; marked "ticking" for the duration of the tick.
    pub(crate) fn tick_mob(&mut self, m: &mut Mob) {
        self.mobs.ticking.push((m.uuid, m.id));
        self.tick_mob_body(m);
        self.mobs.ticking.pop();
    }

    fn tick_mob_body(&mut self, m: &mut Mob) {
        // F15: the dragon has its own tick (no common AI or movement).
        if self.end_tick_mob(m) {
            return;
        }
        // Creeper: fuse before the common tick.
        if m.kind == Kind::Creeper && self.creeper_tick(m) {
            return;
        }
        // F01: skeleton freezing, zombie villager cure, pests' body.
        if self.variant_pre_tick(m) {
            return;
        }
        super::surface::pre_tick(m);
        super::sealife::pre_tick(self, m); // F09: pufferfish inflation
        super::brainy::pre_tick(self, m); // F10: sniffer seed
        super::nether::pre_tick(self, m); // F12: strider sounds and cold
        super::recent::pre_tick(self, m); // F14: creaking heart, warden vibrations, breeze random draw
        // Cubes: sulfur cube fuse, crushing; then common tick and landing.
        if m.kind.is_cube() {
            if self.cube_pre_tick(m) {
                return;
            }
            self.tick_living(m);
            self.cube_post_tick(m);
            return;
        }
        self.tick_living(m);
    }

    /// Common tick of a living entity.
    fn tick_living(&mut self, m: &mut Mob) {
        // Common tick.
        m.old = m.pos;
        super::piglins::spear::compute_speed(m); // known speed
        m.pitch_o = m.pitch;
        m.head_yaw_o = m.head_yaw;
        m.tick_count += 1;
        // Base tick of the entity (F06: iron golem run particles first).
        self.golem_base_tick(m);
        m.was_eyes_in_water = m.eyes_in_water;
        let (was_in_water, vel) = (m.in_water, m.vel);
        let air_before = m.air; // F09: air before the base tick
        self.mob_fluids(m);
        // Splash on entering water, except on the first tick (F08).
        if m.in_water && !was_in_water && m.tick_count > 1 {
            self.mob_splash(m, vel);
        }
        super::aquatic::update_swimming(self, m); // F08: drowned swimming
        let fire_immune = m.kind.info().fire_immune || super::recent::fire_immune(m);
        if m.fire > 0 && fire_immune {
            m.fire = 0;
        } else if m.fire > 0 {
            if m.fire % 20 == 0 && m.lava_height <= 0.0 {
                self.mob_hurt(m, "minecraft:on_fire", 1.0, None, None);
            }
            m.fire -= 1;
        }
        if m.lava_height > 0.0 && fire_immune {
            m.fall_distance *= 0.5;
        } else if m.lava_height > 0.0 {
            m.fall_distance *= 0.5;
            if m.fire < 300 {
                m.fire = 300;
            }
            self.mob_hurt(m, "minecraft:lava", 4.0, None, None);
        }
        if m.pos[1] < f64::from(self.dim.height.min_y - 64) {
            m.removed = true;
            return;
        }
        // Base tick of the living entity.
        if m.alive() {
            let eye = super::mth::block_of([m.pos[0], m.eye_y(), m.pos[2]]);
            // F07: the vex passes through blocks (no physics: never "in a wall"). Only
            // suffocating blocks count: a parrot against a pane does not suffocate.
            if !super::illagers::no_physics(m) && self.in_wall([m.pos[0], m.eye_y(), m.pos[2]], m.width()) {
                self.mob_hurt(m, "minecraft:in_wall", 1.0, None, None);
            }
            if m.eyes_in_water && self.bname(eye) != "minecraft:bubble_column" {
                // F06: the iron golem never loses air (neutral).
                // S7: water breathing, conduit, nautilus shell breath.
                let fx = |id: &str| m.x.effects.iter().any(|e| e.id == id);
                let (wb, cp, bn) = (
                    fx("minecraft:water_breathing"),
                    fx("minecraft:conduit_power"),
                    fx("minecraft:breath_of_the_nautilus"),
                );
                if m.kind == Kind::IronGolem {
                } else if !m.can_breathe_underwater() && !(wb || cp || bn) {
                    m.air -= 1;
                    if m.air <= -20 {
                        m.air = 0;
                        self.mob_event(m, 67);
                        self.mob_hurt(m, "minecraft:drown", 2.0, None, None);
                    }
                } else if m.air < super::brainy::max_air(m).unwrap_or_else(|| super::sealife::max_air(m))
                    && (!bn || wb || cp)
                {
                    // Breathes underwater: air rises, except with the nautilus shell breath alone
                    // (F09: own maximum air, dolphin filled at once).
                    m.air = super::brainy::max_air(m)
                        .map_or_else(|| super::sealife::increase_air(m), |x| (m.air + 4).min(x));
                }
                self.mob_underwater_dismount(m); // F04
            } else if m.air < super::brainy::max_air(m).unwrap_or_else(|| super::sealife::max_air(m)) {
                // F09: own maximum air and regain (dolphin: 4800 at once).
                m.air =
                    super::brainy::max_air(m).map_or_else(|| super::sealife::increase_air(m), |x| (m.air + 4).min(x));
            }
        }
        if m.hurt_time > 0 {
            m.hurt_time -= 1;
        }
        if m.damage_cooldown > 0 {
            m.damage_cooldown -= 1;
        }
        // F14: creaking being undone (45 ticks, dislocation).
        if (m.dead || m.health <= 0.0) && super::recent::tick_death(self, m) {
            if m.removed {
                return;
            }
        } else if m.dead || m.health <= 0.0 {
            m.death_time += 1;
            if m.death_time >= 20 {
                self.mob_event(m, 60);
                if m.kind.is_cube() {
                    self.cube_split(m);
                }
                self.mob_removed_effects(m); // gust, cobwebs, slimes
                m.removed = true;
                return;
            }
        }
        if m.last_hurt_by_player > 0 {
            m.last_hurt_by_player -= 1;
        }
        if let Some((e, ts)) = m.last_hurt_by_mob
            && (!self.ent_alive(e) || m.tick_count - ts > 100)
        {
            m.last_hurt_by_mob = None;
        }
        self.mob_effects_tick(m);
        // Ambient sound (entity's random draw, every tick).
        if m.alive() {
            let r = m.rng.next_int_bound(1000);
            let t = m.ambient_time;
            m.ambient_time += 1;
            if r < t {
                m.ambient_time = -self.ambient_interval(m);
                if let Some(s) = self.ambient_sound(m) {
                    // F11: fox cry at volume 2.
                    if let Some(v) = super::wild::ambient_volume(m, &s) {
                        let p = Self::voice_pitch(m);
                        self.mob_sound(m, &s, v, p);
                    } else {
                        // The fox recalls the base ambient sound: second draw (cry possible,
                        // at normal volume).
                        let s =
                            if m.kind == super::species::Kind::Fox { self.ambient_sound(m).unwrap_or(s) } else { s };
                        self.make_sound(m, &s);
                    }
                }
            }
        }
        // F09: air of aquatic mobs out of water (after the base tick).
        super::sealife::handle_air(self, m, air_before);
        super::brainy::handle_air(self, m, air_before); // F10: axolotl, tadpole
        if m.swing_time > 0 {
            m.swing_time -= 1;
        }
        // Tick of the living entity: item in use, then the AI step.
        if m.use_ticks >= 0 {
            // F07: crossbow sounds and loading.
            if m.kind == Kind::Pillager || m.kind == Kind::Piglin {
                self.crossbow_use_tick(m);
            }
            // F13: spear.
            if matches!(m.kind, Kind::Piglin | Kind::ZombifiedPiglin) {
                self.spear_use_tick(m);
            }
            m.use_ticks += 1;
        }
        if m.kind == Kind::Chicken {
            self.chicken_tick(m);
        }
        if m.kind == Kind::SulfurCube {
            self.sulfur_equipment_check(m);
            self.sulfur_age_tick(m);
        } else if m.kind.ageable() {
            self.ageable_tick(m);
        }
        if m.kind == Kind::Wolf {
            self.anger_tick(m);
        }
        self.ai_step(m);
        if m.kind == Kind::Wolf {
            self.wolf_shake_start(m);
        }
        // Body.
        self.body_rotation(m);
        if m.kind.base() == Kind::Spider {
            m.climbing = m.horizontal_collision;
        }
        // Control flags every 5 ticks (no passenger here: everything allowed).
        if m.tick_count % 5 == 0 {
            m.goals.disabled = Self::control_flags(m); // F04: steering rider
            // F07: a pillager who steers a ravager leaves it its goals.
            if m.kind == Kind::Ravager
                && Self::mob_controller(m)
                    .and_then(|id| self.mob_ref(id))
                    .is_some_and(|r| super::illagers::is_raider(r.kind))
            {
                m.goals.disabled = 0;
            }
            m.targets.disabled = 0;
        }
        // F01: drowning of zombies, lifetime of the endermite.
        self.variant_post_tick(m);
        self.npc_post_tick(m); // F05
        super::surface::post_tick(self, m);
        super::mounts::post_tick(self, m);
        self.golem_post_tick(m);
        self.illager_post_tick(m); // F07: limited life of the vex
        super::sealife::post_tick(self, m); // F09
        super::brainy::post_tick(self, m); // F10: armadillo, allay
        self.wild_post_tick(m); // F11
        super::nether::post_tick(self, m); // F12: strider standing on lava
        super::piglins::post_tick(self, m); // F13
        super::recent::post_tick(self, m); // F14: happy ghast
        self.end_post_tick(m); // F15: shulker attachment and opening
        self.cat_post_tick(m); // cat: beg, purr
        if m.kind == Kind::Wolf {
            self.wolf_shake_tick(m);
        }
    }

    fn ambient_interval(&self, m: &Mob) -> i32 {
        super::recent::ambient_interval(m).unwrap_or_else(|| m.kind.info().ambient_interval)
    }

    fn ambient_sound(&mut self, m: &mut Mob) -> Option<String> {
        Some(match m.kind {
            Kind::Enderman => {
                if m.creepy { "minecraft:entity.enderman.scream" } else { "minecraft:entity.enderman.ambient" }.into()
            }
            Kind::Wolf => {
                if m.anger_end >= 0 {
                    self.variant_sound(m, "growl_sound").unwrap_or_else(|| "minecraft:entity.wolf.growl".into())
                } else if m.rng.next_int_bound(3) == 0 {
                    if m.tame && m.health < 20.0 {
                        self.variant_sound(m, "whine_sound").unwrap_or_else(|| "minecraft:entity.wolf.whine".into())
                    } else {
                        self.variant_sound(m, "pant_sound").unwrap_or_else(|| "minecraft:entity.wolf.pant".into())
                    }
                } else {
                    self.variant_sound(m, "ambient_sound").unwrap_or_else(|| "minecraft:entity.wolf.ambient".into())
                }
            }
            Kind::Cat => {
                if m.tame {
                    if m.is_in_love() {
                        self.variant_sound(m, "purr_sound").unwrap_or_else(|| "minecraft:entity.cat.purr".into())
                    } else if m.rng.next_int_bound(4) == 0 {
                        self.variant_sound(m, "purreow_sound").unwrap_or_else(|| "minecraft:entity.cat.purreow".into())
                    } else {
                        self.variant_sound(m, "ambient_sound").unwrap_or_else(|| "minecraft:entity.cat.ambient".into())
                    }
                } else {
                    self.variant_sound(m, "stray_ambient_sound")
                        .unwrap_or_else(|| "minecraft:entity.cat.stray_ambient".into())
                }
            }
            k if super::surface::handles(k) => return super::surface::ambient_sound(self, m),
            k if super::mounts::handles(k) => return super::mounts::ambient_sound(m),
            // F05: sleeping trader silent, in a trade: trade sound.
            k if super::villager::is_npc(k) => return super::villager::ambient_sound(m),
            k if super::golems::handles(k) => return super::golems::ambient_sound(m),
            k if super::aquatic::handles(k) => return super::aquatic::ambient_sound(m),
            k if super::sealife::handles(k) => return super::sealife::ambient_sound(m),
            k if super::brainy::handles(k) => return super::brainy::ambient_sound(m),
            k if super::wild::handles(k) => return super::wild::ambient_sound(self, m),
            k if super::nether::handles(k) => return super::nether::ambient_sound(m),
            k if super::piglins::handles(k) => return super::piglins::ambient_sound(self, m),
            k if super::recent::handles(k) => return super::recent::ambient_sound(m),
            k if super::end::handles(k) => return super::end::ambient_sound(m),
            _ => self.variant_sound(m, "ambient_sound").or_else(|| m.kind.info().ambient.map(str::to_owned))?,
        })
    }

    /// The AI step of a mob.
    fn ai_step(&mut self, m: &mut Mob) {
        self.illager_pre_ai_step(m); // F07: witch, raid pillager
        super::aquatic::pre_ai_step(m); // F08: guardians
        super::surface::pre_ai_step(self, m);
        super::mounts::pre_ai_step(m);
        super::sealife::pre_ai_step(self, m); // F09: fish jump out of water
        super::brainy::pre_ai_step(self, m); // F10: tadpole wriggling
        self.wild_pre_ai_step(m); // F11: fox
        super::nether::pre_ai_step(m); // F12: blaze slow fall
        super::piglins::pre_ai_step(m); // F13: hoglin attack animation
        super::recent::pre_ai_step(self, m); // F14: frozen creaking, happy ghast
        super::wither::boss::pre_ai_step(self, m); // F16: push towards the target
        // Cubes: the game's "age" class, not "monster" (no wear from light).
        if m.kind.is_monster()
            && m.kind != Kind::Phantom
            && !m.kind.is_cube()
            && !super::mounts::handles(m.kind)
            && !super::sealife::handles(m.kind)
            && !super::nether::is_ghast(m.kind)
            && m.kind != Kind::Hoglin
            && (super::illagers::always_idle_wear(m.kind)
                || super::nether::always_idle_wear(m.kind)
                || self.magic_light(m.block_pos()) > 0.5)
        {
            m.no_action_time += 2;
        }
        if m.kind == Kind::Enderman {
            m.jumping = false;
        }
        if m.no_jump_delay > 0 {
            m.no_jump_delay -= 1;
        }
        for a in [0, 2] {
            if m.vel[a].abs() < 0.003 {
                m.vel[a] = 0.0;
            }
        }
        if m.vel[1].abs() < 0.003 {
            m.vel[1] = 0.0;
        }
        m.xxa *= 0.98;
        m.zza *= 0.98;
        let immobile = m.dead || m.health <= 0.0 || super::mounts::immobile(m) || super::illagers::immobile(m);
        // F04: mount piloted by the player: neither AI nor server movement (client is master).
        let ridden = Self::player_controls(m) && m.alive();
        if immobile {
            m.jumping = false;
            m.xxa = 0.0;
            m.zza = 0.0;
        } else if !m.no_ai && !ridden {
            self.server_ai_step(m);
        }
        if ridden {
            self.tick_ridden(m);
        } else if !m.no_ai {
            if m.kind == Kind::Phantom {
                self.phantom_travel(m);
            } else if m.kind == Kind::Vex {
                self.vex_travel(m); // F07
            } else if super::end::immobile(m) {
                // F15: the shulker never moves (speed always zero).
            } else {
                self.mob_jump(m);
                self.mob_travel(m);
            }
        }
        // Block effects after the movement: without current fire, the counter returns to 0
        // (a negative fire re-read lasts only one tick; only the player keeps −20).
        if m.fire < 0 && !m.removed && !super::illagers::no_physics(m) {
            m.fire = 0;
        }
        self.mob_freezing(m); // freezing in powder snow
        self.mob_push_entities(m);
        if m.kind.info().water_sensitive && self.mob_in_water_or_rain(m) && m.alive() {
            self.mob_hurt(m, "minecraft:drown", 1.0, None, None);
        }
        // Sun, pickup.
        self.sun_burn(m);
        super::surface::post_ai_step(self, m);
        super::sealife::post_ai_step(self, m); // F09
        super::brainy::post_ai_step(self, m); // F10: tadpole, allay
        super::mounts::post_ai_step(self, m);
        self.npc_pickup(m); // F05
        if m.kind == Kind::SulfurCube {
            self.sulfur_pickup(m);
        }
        self.golem_post_ai_step(m);
        self.illager_post_ai_step(m); // F07: ravager
        self.wild_post_ai_step(m); // F11
        self.piglin_pickup(m); // F13: piglin pickup
        super::recent::post_ai_step(self, m); // F14: happy ghast healing
        super::wither::boss::post_ai_step(self, m); // F16: heads, particle draws
    }

    /// The server AI step (§ 3.1).
    fn server_ai_step(&mut self, m: &mut Mob) {
        m.no_action_time += 1;
        m.sight.clear();
        let had = m.target;
        let odd = (m.tick_count + m.id) % 2 != 0 && m.tick_count > 1;
        // F04: the steering rider acts on the navigation of their mount.
        let lent = self.borrow_vehicle_nav(m);
        if odd {
            self.selector_tick_running(m, true);
            self.selector_tick_running(m, false);
        } else {
            self.selector_tick(m, true);
            self.selector_tick(m, false);
        }
        self.return_vehicle_nav(m, lent);
        Self::enderman_target_changed(m, had);
        // F14: the frozen creaking does not follow its navigation.
        if !super::recent::frozen(m) {
            self.nav_tick(m);
        }
        super::surface::custom_ai_step(self, m);
        if super::mounts::is_camel(m.kind) {
            self.camel_brain_tick(m); // F04: camel brain
            super::mounts::camel::move_control_pre(self, m);
        }
        self.npc_custom_ai(m); // F05: villager brain
        self.golem_custom_ai_step(m);
        self.illager_custom_ai(m); // F07: spells, vindicator doors
        super::aquatic::custom_ai_step(self, m); // F08: elder guardian
        super::sealife::custom_ai_step(self, m); // F09: nautilus brain
        super::brainy::custom_ai_step(self, m); // F10: animals with a brain
        self.wild_custom_ai_step(m); // F11: bee
        super::nether::custom_ai_step(self, m); // F12: blaze height
        super::piglins::custom_ai_step(self, m); // F13: brains, zombification, anger
        super::recent::custom_ai_step(self, m); // F14: brains
        super::wither::boss::custom_ai_step(self, m); // F16: summoning, heads, blocks
        // Own mechanics.
        if m.kind == Kind::Enderman && self.bright_outside() && m.tick_count >= m.target_change_time + 600 {
            let br = self.magic_light(m.block_pos());
            if br > 0.5 && self.can_see_sky(m.block_pos()) && m.rng.next_float() * 30.0 < (br - 0.4) * 2.0 {
                let had = m.target;
                m.target = None;
                Self::enderman_target_changed(m, had);
                self.enderman_teleport_random(m);
            }
        }
        if m.kind == Kind::SulfurCube {
            self.cube_custom_ai(m);
        }
        if m.kind == Kind::Phantom {
            // Own flight control; look without effect.
            self.phantom_move_control(m);
        } else {
            if m.kind.is_cube() {
                self.cube_move_control(m);
            } else if m.kind == Kind::Vex {
                self.vex_move_control(m); // F07
            } else if super::wild::handles(m.kind) {
                if !self.wild_move_control(m) {
                    self.move_control(m); // F11
                }
            } else if !super::surface::move_control(self, m)
                && !super::aquatic::move_control(self, m)
                && !super::sealife::move_control(m)
                && !super::brainy::move_control(m)
                && !super::nether::move_control(self, m)
                && !super::recent::move_control(self, m)
                && !super::wither::move_control(m)
            {
                self.move_control(m);
            }
            // F04: the steered camel does not turn its head alone; head bounded to 30°.
            let camel = super::mounts::is_camel(m.kind);
            if (camel && Self::mob_controller(m).is_some())
                || super::sealife::look_control(m)
                || super::brainy::look_control(self, m)
                || self.wild_look_control(m)
                || super::recent::look_control(self, m)
                || self.end_look_control(m)
            {
            } else if !self.sulfur_look_control(m) {
                self.look_control(m);
                if camel && !m.nav.is_done() {
                    m.head_yaw = super::mth::rotate_if_necessary(m.head_yaw, m.body_yaw, 30.0);
                }
            }
        }
        if !super::surface::jump_control(self, m) {
            m.jumping = m.jump_ctl;
            m.jump_ctl = false;
        }
        // F14: jump cut for the frozen creaking.
        if super::recent::frozen(m) {
            m.jumping = false;
        }
    }

    /// The movement control.
    fn move_control(&self, m: &mut Mob) {
        match m.move_ctl.op {
            MoveOp::Strafe => {
                let speed = m.movement_speed() as f32;
                let modified = m.move_ctl.speed_mod as f32 * speed;
                let (mut xa, mut za) = (m.move_ctl.strafe_fwd, m.move_ctl.strafe_right);
                let mut dist = (xa * xa + za * za).sqrt();
                if dist < 1.0 {
                    dist = 1.0;
                }
                dist = modified / dist;
                xa *= dist;
                za *= dist;
                let (s, c) = (super::mth::sin(m.yaw * DEG), super::mth::cos(m.yaw * DEG));
                let dx = xa * c - za * s;
                let dz = za * c + xa * s;
                let p = [
                    (m.pos[0] + f64::from(dx)).floor() as i32,
                    m.block_pos()[1],
                    (m.pos[2] + f64::from(dz)).floor() as i32,
                ];
                let view = self.mob_view(m);
                if super::nav::static_path_type(self, &view, p) != super::nav::PathType::Walkable {
                    m.move_ctl.strafe_fwd = 1.0;
                    m.move_ctl.strafe_right = 0.0;
                }
                m.speed = modified;
                m.zza = modified;
                m.zza = m.move_ctl.strafe_fwd;
                m.xxa = m.move_ctl.strafe_right;
                m.move_ctl.op = MoveOp::Wait;
            }
            MoveOp::MoveTo => {
                m.move_ctl.op = MoveOp::Wait;
                let w = m.move_ctl.wanted;
                let (xd, yd, zd) = (w[0] - m.pos[0], w[1] - m.pos[1], w[2] - m.pos[2]);
                let dd = xd * xd + yd * yd + zd * zd;
                if dd < f64::from(2.500_000_3e-7f32) {
                    m.zza = 0.0;
                    return;
                }
                let yr = yaw_to(xd, zd);
                m.yaw = rotlerp_360(m.yaw, yr, 90.0);
                let sp = (m.move_ctl.speed_mod * m.movement_speed()) as f32;
                m.speed = sp;
                m.zza = sp;
                let bp = m.block_pos();
                let top = self.collision_top(bp);
                let n = self.bname(bp);
                let door_or_fence = self.btag_id(bp, crate::tag_bits::TagId::Doors) || self.btag_id(bp, crate::tag_bits::TagId::Fences);
                let _ = n;
                if (yd > m.step_height() && xd * xd + zd * zd < f64::from(m.width().max(1.0)))
                    || (top.is_some_and(|t| m.pos[1] < t + f64::from(bp[1])) && !door_or_fence)
                {
                    m.jump_ctl = true;
                    m.move_ctl.op = MoveOp::Jumping;
                }
            }
            MoveOp::Jumping => {
                m.speed = (m.move_ctl.speed_mod * m.movement_speed()) as f32;
                m.zza = m.speed;
                if m.on_ground || m.in_water || m.lava_height > 0.0 {
                    m.move_ctl.op = MoveOp::Wait;
                }
            }
            MoveOp::Wait => m.zza = 0.0,
        }
    }

    /// The look control.
    fn look_control(&self, m: &mut Mob) {
        m.pitch = 0.0;
        if m.look.cooldown > 0 {
            m.look.cooldown -= 1;
            let w = m.look.wanted;
            let (xd, zd) = (w[0] - m.pos[0], w[2] - m.pos[2]);
            if zd.abs() > f64::from(1.0e-5f32) || xd.abs() > f64::from(1.0e-5f32) {
                m.head_yaw = rotate_towards(m.head_yaw, yaw_to(xd, zd), m.look.y_max);
            }
            let yd = w[1] - m.eye_y();
            let sd = (xd * xd + zd * zd).sqrt();
            if yd.abs() > f64::from(1.0e-5f32) || sd.abs() > f64::from(1.0e-5f32) {
                m.pitch = rotate_towards(m.pitch, pitch_to(yd, sd), m.look.x_max);
            }
        } else {
            m.head_yaw = rotate_towards(m.head_yaw, m.body_yaw, 10.0);
        }
        if !m.nav.is_done() {
            m.head_yaw = rotate_if_necessary(m.head_yaw, m.body_yaw, max_head_y(m.kind));
        }
    }

    /// Client-side body rotation (called by the end-of-tick rotation).
    fn body_rotation(&self, m: &mut Mob) {
        // F10: the curled-up armadillo does not turn its body.
        if super::brainy::freezes_body(m) || super::end::immobile(m) {
            return;
        }
        // F14: frozen creaking; mounted happy ghast (head and body on the yaw).
        if super::recent::body_rotation_pre(m) {
            return;
        }
        if m.kind == Kind::Phantom {
            // Head = previous body, body = yaw.
            m.head_yaw = m.body_yaw;
            m.body_yaw = m.yaw;
            return;
        }
        let (xd, zd) = (m.pos[0] - m.old[0], m.pos[2] - m.old[2]);
        if xd * xd + zd * zd > f64::from(2.500_000_3e-7f32) {
            m.body_yaw = m.yaw;
            m.head_yaw = rotate_if_necessary(m.head_yaw, m.body_yaw, max_head_y(m.kind));
            m.body.last_stable_head = m.head_yaw;
            m.body.head_stable_time = 0;
        } else if (m.head_yaw - m.body.last_stable_head).abs() > 15.0 {
            m.body.head_stable_time = 0;
            m.body.last_stable_head = m.head_yaw;
            m.body_yaw = rotate_if_necessary(m.body_yaw, m.head_yaw, max_head_y(m.kind));
        } else {
            m.body.head_stable_time += 1;
            if m.body.head_stable_time > 10 {
                let t = m.body.head_stable_time - 10;
                let f = (t as f32 / 10.0).clamp(0.0, 1.0);
                m.body_yaw = rotate_if_necessary(m.body_yaw, m.head_yaw, max_head_y(m.kind) * (1.0 - f));
            }
        }
        let _ = degrees_difference(0.0, 0.0);
    }
}

impl Server {
    /// Cat spawner (overworld, every 1200 ticks): a living player drawn at
    /// random, cell 8..31 blocks on each axis at THEIR height, loaded zone (±10), cat placement on the
    /// ground; near a village (2 sections) with more than 4 occupied beds within 48 blocks and
    /// fewer than 5 cats around: village cat; otherwise in a witch hut with no cat within
    /// 16 blocks: persistent cat. Like the game, finalization happens BEFORE placement (at
    /// the world origin: variant drawn there), then the cat is placed at the center of the cell.
    pub(crate) fn cat_spawner_tick(&mut self) {
        if self.dim.name != "minecraft:overworld" {
            return;
        }
        self.mobs.spawners.cat_next -= 1;
        if self.mobs.spawners.cat_next > 0 {
            return;
        }
        self.mobs.spawners.cat_next = 1200;
        if self.life.vit.dead {
            return;
        }
        // Living players: the host alone here (`next_int(1)` draw kept for the random stream).
        let _ = self.mobs.rng.next_int_bound(1);
        let base = super::mth::block_of(self.pos);
        let r = &mut self.mobs.rng;
        let x = (8 + r.next_int_bound(24)) * if r.next_bool() { -1 } else { 1 };
        let z = (8 + r.next_int_bound(24)) * if r.next_bool() { -1 } else { 1 };
        let p = [base[0] + x, base[1], base[2] + z];
        let loaded = ((p[0] - 10) >> 4..=(p[0] + 10) >> 4).all(|cx| {
            ((p[2] - 10) >> 4..=(p[2] + 10) >> 4).all(|cz| self.chunks.get(&(cx, cz)).is_some_and(Option::is_some))
        });
        if !loaded || !self.spawn_placement_ok(Kind::Cat, p) {
            return;
        }
        let cats_in = |s: &Server, h: f64| {
            let area = rc_physics::Aabb {
                min: [f64::from(p[0]) - h, f64::from(p[1]) - 8.0, f64::from(p[2]) - h],
                max: [f64::from(p[0]) + 1.0 + h, f64::from(p[1]) + 9.0, f64::from(p[2]) + 1.0 + h],
            };
            s.mobs.of_kind(Kind::Cat).filter(|m| !m.removed && m.touches(&area)).count()
        };
        if self.sections_to_village(p) <= 2 {
            let homes =
                self.poi_in_range(|t| t == crate::poi::PoiType::Home, p, 48, crate::poi::Occupancy::IsOccupied).len();
            if homes > 4 && cats_in(self, 48.0) < 5 {
                self.spawn_cat_at(p, false);
            }
        } else if self.in_structure_piece(p, "minecraft:swamp_hut") && cats_in(self, 16.0) == 0 {
            self.spawn_cat_at(p, true);
        }
    }

    fn spawn_cat_at(&mut self, p: [i32; 3], persistent: bool) {
        let mut m = self.new_mob(Kind::Cat, [0.0; 3], 0.0);
        let mut g = super::spawn::Group::default();
        self.finalize_spawn(&mut m, super::spawn::Reason::Natural, &mut g);
        if persistent {
            m.persistent = true;
        }
        let at = [f64::from(p[0]) + 0.5, f64::from(p[1]), f64::from(p[2]) + 0.5];
        m.pos = at;
        m.old = at;
        m.yaw = 0.0;
        m.head_yaw = 0.0;
        m.body_yaw = 0.0;
        m.pitch = 0.0;
        self.add_mob(m);
    }

    /// Zombie siege (overworld, night, monsters allowed): drawn at 1 chance in 10 at the "siege"
    /// marker of the clock (18000); set up around a non-spectator player in a
    /// village (outside biomes without siege): center at 32 blocks; then one zombie every 3 ticks,
    /// 20 at most, on the surface of a dark village cell within ±8 of the center. Finalized at
    /// the world origin before being placed (like the game).
    pub(crate) fn village_siege_tick(&mut self) {
        if self.dim.name != "minecraft:overworld" {
            return;
        }
        let spawn_enemies = self.level.difficulty_id() != 0 && self.rule("minecraft:spawn_monsters", true);
        let bright = self.mob_sky_darken() < 4;
        if bright || !spawn_enemies {
            self.mobs.spawners.siege_tonight = false;
            self.mobs.spawners.siege_setup = false;
            return;
        }
        if self.overworld_clock().rem_euclid(24000) == 18000 {
            self.mobs.spawners.siege_tonight = self.mobs.rng.next_int_bound(10) == 0;
        }
        if !self.mobs.spawners.siege_tonight {
            return;
        }
        if !self.mobs.spawners.siege_setup {
            if !self.setup_siege() {
                return;
            }
            self.mobs.spawners.siege_setup = true;
        }
        let sp = &mut self.mobs.spawners;
        if sp.siege_next > 0 {
            sp.siege_next -= 1;
            return;
        }
        sp.siege_next = 2;
        if sp.siege_left > 0 {
            sp.siege_left -= 1;
            self.siege_spawn();
        } else {
            sp.siege_tonight = false;
        }
    }

    fn setup_siege(&mut self) -> bool {
        if self.game_mode == 3 || self.life.vit.dead {
            return false;
        }
        let center = super::mth::block_of(self.pos);
        if !self.is_village(center) || self.biome_tag(center, "minecraft:without_zombie_sieges") {
            return false;
        }
        for _ in 0..10 {
            let a = self.mobs.rng.next_float() * std::f32::consts::TAU;
            let at = [
                center[0] + (rc_physics::cos_t(a) * 32.0).floor() as i32,
                center[1],
                center[2] + (rc_physics::sin_t(a) * 32.0).floor() as i32,
            ];
            self.mobs.spawners.siege_at = at;
            if self.siege_spawn_pos(at).is_some() {
                self.mobs.spawners.siege_next = 0;
                self.mobs.spawners.siege_left = 20;
                break;
            }
        }
        true
    }

    fn siege_spawn(&mut self) {
        let Some(at) = self.siege_spawn_pos(self.mobs.spawners.siege_at) else { return };
        let mut z = self.new_mob(Kind::Zombie, [0.0; 3], 0.0);
        let mut g = super::spawn::Group::default();
        self.finalize_spawn(&mut z, super::spawn::Reason::Event, &mut g);
        let yaw = self.mobs.rng.next_float() * 360.0;
        z.pos = at;
        z.old = at;
        z.yaw = yaw;
        z.head_yaw = yaw;
        z.body_yaw = yaw;
        z.pitch = 0.0;
        self.add_mob(z);
    }

    /// Surface cell of a village where a zombie can spawn (10 attempts within ±8).
    fn siege_spawn_pos(&mut self, c: [i32; 3]) -> Option<[f64; 3]> {
        for _ in 0..10 {
            let x = c[0] + self.mobs.rng.next_int_bound(16) - 8;
            let z = c[2] + self.mobs.rng.next_int_bound(16) - 8;
            let p = [x, self.surface_y(x, z, 1), z];
            if self.is_village(p) && self.spawn_rules(Kind::Zombie, p) {
                return Some([f64::from(x) + 0.5, f64::from(p[1]), f64::from(z) + 0.5]);
            }
        }
        None
    }
}
