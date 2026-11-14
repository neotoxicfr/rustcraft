//! Combat (`server-gameplay.md` § 2, `server-mobs.md` § 3.8): damage taken by a mob
//! (invulnerability window, armor, knockback, sounds), death, loot and experience; mob hits on
//! the player; player hits (attack strength, crit, sweep).

use rc_client::{EntityEvent, Event};
use rc_proto_v777::entity::{LevelParticles, Particle};
use rc_proto_v777::play::{SoundPacket, SoundRef};

use super::mob::{Ent, Hit, Mob};
use super::mth::{DEG, cos, sin};
use super::species::Kind;
use crate::player::Stack;
use crate::server::Server;

/// Knockback direction of an injury: given as is (projectile), otherwise
/// source minus position, otherwise zero.
pub(crate) fn hurt_knockback_dir(dir: Option<(f64, f64)>, src: Option<[f64; 3]>, pos: [f64; 3]) -> (f64, f64) {
    match (dir, src) {
        (Some(d), _) => d,
        (None, Some(s)) => (s[0] - pos[0], s[2] - pos[2]),
        (None, None) => (0.0, 0.0),
    }
}

/// Path of an identifier written without a namespace (`is_fire`) or with an empty one
/// (`:is_fire`): the game reads both in `minecraft`. `None` for a complete identifier.
fn default_namespace_path(written: &str) -> Option<&str> {
    match written.split_once(':') {
        None => Some(written),
        Some(("", path)) => Some(path),
        Some(_) => None,
    }
}

/// Does the complete identifier `full` (`minecraft:is_fire`) name what `written` names, once
/// `written` is read like the game reads an identifier (missing namespace = `minecraft`)?
pub(crate) fn same_identifier(full: &str, written: &str) -> bool {
    match default_namespace_path(written) {
        Some(path) => full.strip_prefix("minecraft:") == Some(path),
        None => full == written,
    }
}

impl Server {
    /// Damage type in a tag (`tags/damage_type`). The tag may be written short (`is_fire`, from
    /// a `damage_resistant` or `blocks_attacks` component): it is then in `minecraft`, like the game.
    pub(crate) fn dmg_tag(&self, ty: &str, tag: &str) -> bool {
        let Some(idx) = self.regs.entries("minecraft:damage_type").iter().position(|e| e == ty) else { return false };
        self.regs
            .tags
            .iter()
            .find(|(r, _)| r == "minecraft:damage_type")
            .and_then(|(_, t)| t.iter().find(|(n, _)| same_identifier(n, tag)))
            .is_some_and(|(_, ids)| ids.contains(&(idx as i32)))
    }

    /// Is damage type `ty` in one entry of a set of damage types: a tag (`#is_fire`) or a single
    /// damage type (`in_fire`), each with an optional namespace like the game.
    pub(crate) fn damage_set_entry_has(&self, entry: &str, ty: &str) -> bool {
        match entry.strip_prefix('#') {
            Some(tag) => self.dmg_tag(ty, tag),
            None => same_identifier(ty, entry),
        }
    }

    pub(crate) fn sound_id(&self, name: &str) -> Option<i32> {
        self.data
            .registry("minecraft:sound_event")
            .and_then(|r| r.entries.iter().position(|e| e == name))
            .map(|i| i as i32)
    }

    /// Sound played by a mob: nothing if it is silent (`Silent`); the callers' pitch
    /// draws are still made. The seed comes from the sound-only generator, like the official game: a
    /// sound never shifts the world's random source (spawns, finalizations).
    pub(crate) fn mob_sound(&mut self, m: &Mob, name: &str, volume: f32, pitch: f32) {
        if m.x.silent {
            return;
        }
        self.mob_sound_unmuted(m, name, volume, pitch);
    }

    /// Sound played by the world at a mob's position: heard even if it is silent.
    pub(crate) fn mob_sound_unmuted(&mut self, m: &Mob, name: &str, volume: f32, pitch: f32) {
        let seed = crate::vitals::sound_seed();
        let Some(id) = self.sound_id(name) else { return };
        let category = Self::mob_sound_source(m);
        let _ = self.send(Event::Sound(SoundPacket {
            sound: SoundRef::Id(id),
            category,
            pos: Some(m.pos()),
            entity: None,
            volume,
            pitch,
            seed,
        }));
    }

    /// Game sound of the sound variant (pig, cow, chicken, wolf, cat): "baby" or
    /// "adult" set when the variant has two, `key` = `ambient_sound`, `hurt_sound`…
    pub(crate) fn variant_sound(&self, m: &Mob, key: &str) -> Option<String> {
        let reg = match m.kind {
            Kind::Pig => "pig",
            Kind::Cow => "cow",
            Kind::Chicken => "chicken",
            Kind::Wolf => "wolf",
            Kind::Cat => "cat",
            _ => return None,
        };
        let dir = format!("{reg}_sound_variant");
        let entries = rc_data::ids::with_key(&["minecraft:", &dir], |k| self.regs.entries(k));
        let id = entries.get(usize::try_from(m.sound_variant).ok()?)?;
        let v = self.life.pack.get(&dir, id)?;
        let set = if m.is_baby() { "baby_sounds" } else { "adult_sounds" };
        let sounds = v.get(set).unwrap_or_else(|| v.as_ref());
        sounds.get(key)?.as_str().map(str::to_owned)
    }

    /// `shear` event on the sheared mob, attributed to the player.
    pub(crate) fn sheared_by_player(&mut self, m: &Mob) {
        let src = Some(super::recent::vibration::Src::Living(Ent::Player));
        self.game_event_src("minecraft:shear", m.pos(), src, None);
    }

    /// Sound category of a mob.
    pub(crate) fn mob_sound_source(m: &Mob) -> i32 {
        super::surface::sound_source(m)
            .or_else(|| super::mounts::sound_source(m))
            .or_else(|| super::sealife::sound_source(m))
            .or_else(|| super::recent::sound_source(m))
            .unwrap_or_else(|| m.sound_category())
    }

    /// Sound attached to an entity: the client makes it follow
    /// the entity (`sound_entity` packet).
    pub(crate) fn entity_sound(&mut self, id: i32, name: &str, category: i32, volume: f32, pitch: f32) {
        let seed = crate::vitals::sound_seed();
        let Some(sid) = self.sound_id(name) else { return };
        let _ = self.send(Event::Sound(SoundPacket {
            sound: SoundRef::Id(sid),
            category,
            pos: None,
            entity: Some(id),
            volume,
            pitch,
            seed,
        }));
    }

    pub(crate) fn world_sound(&mut self, name: &str, category: i32, pos: [f64; 3], volume: f32, pitch: f32) {
        let seed = crate::vitals::sound_seed();
        let Some(id) = self.sound_id(name) else { return };
        let _ = self.send(Event::Sound(SoundPacket {
            sound: SoundRef::Id(id),
            category,
            pos: Some(pos),
            entity: None,
            volume,
            pitch,
            seed,
        }));
    }

    /// Voice pitch.
    pub(crate) fn voice_pitch(m: &mut Mob) -> f32 {
        // F14: happy ghast, fixed pitch without a draw.
        if let Some(p) = super::recent::fixed_voice_pitch(m) {
            return p;
        }
        // Baby zombie villager: 2.0 (F01).
        let baby = if m.kind == Kind::ZombieVillager { 2.0 } else { 1.5 };
        let base = if m.is_baby() { baby } else { 1.0 };
        let p = (m.rng.next_float() - m.rng.next_float()) * 0.2 + base;
        super::surface::voice_pitch(m, p)
    }

    /// Volume of the species, voice pitch.
    pub(crate) fn make_sound(&mut self, m: &mut Mob, name: &str) {
        let v = m.sound_volume();
        let p = Self::voice_pitch(m);
        self.mob_sound(m, name, v, p);
    }

    pub(crate) fn mob_event(&self, m: &Mob, ev: i8) {
        let _ = self.send(Event::Entity(EntityEvent::Event { id: m.id, event: ev }));
    }

    pub(crate) fn mob_swing(&self, m: &mut Mob) {
        m.swing_time = 6;
        let _ = self.send(Event::Entity(EntityEvent::Swing { id: m.id, hand: 0, kind: 1, duration: 6 }));
    }

    pub(crate) fn particles(&self, name: &str, pos: [f64; 3], delta: [f32; 3], speed: f32, count: i32) {
        self.particles_full(name, 0, pos, delta, [speed; 3], count, 0);
    }

    /// Crumbs of an item (`item` particle).
    pub(crate) fn item_particles(&self, item: &str, pos: [f64; 3], delta: [f32; 3], speed: f32, count: i32) {
        let index = |reg: &str, name: &str| self.data.registry(reg)?.entries.iter().position(|e| e == name);
        let (Some(kind), Some(item)) = (index("minecraft:particle_type", "minecraft:item"), index("minecraft:item", item))
        else {
            return;
        };
        let p = LevelParticles {
            particle: Particle {
                kind: kind as i32,
                color: 0,
                state: 0,
                item: item as i32,
                scale: 1.0,
                color2: 0,
                vib: None,
                trail: None,
            },
            force: false,
            always_visible: false,
            pos,
            delta,
            speed: [speed; 3],
            count,
            mode: 0,
        };
        let _ = self.send(Event::Particles(Box::new(p)));
    }

    /// Particles with color (ARGB; `effect` opaque white = 0xFFFF_FFFF), speed per axis and
    /// dispersion mode (0 default, 1 alternate, 2 alternate with speed).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn particles_full(
        &self,
        name: &str,
        color: u32,
        pos: [f64; 3],
        delta: [f32; 3],
        speed: [f32; 3],
        count: i32,
        mode: i32,
    ) {
        let Some(kind) =
            self.data.registry("minecraft:particle_type").and_then(|r| r.entries.iter().position(|e| e == name))
        else {
            return;
        };
        let p = LevelParticles {
            particle: Particle { kind: kind as i32, color, state: 0, item: -1, scale: 1.0, color2: 0, vib: None, trail: None },
            force: false,
            always_visible: false,
            pos,
            delta,
            speed,
            count,
            mode,
        };
        let _ = self.send(Event::Particles(Box::new(p)));
    }

    /// One `trail` particle from `pos` towards `target`, reached in `duration` ticks, RGB `color`;
    /// `forced`: beyond the usual distance and always shown (creaking heart).
    pub(crate) fn trail_particle(&self, pos: [f64; 3], target: [f64; 3], color: u32, duration: i32, forced: bool) {
        let Some(kind) = self
            .data
            .registry("minecraft:particle_type")
            .and_then(|r| r.entries.iter().position(|e| e == "minecraft:trail"))
        else {
            return;
        };
        let mut particle = Particle::plain(kind as i32);
        particle.color = color;
        particle.trail = Some((target, duration));
        let p = LevelParticles {
            particle,
            force: forced,
            always_visible: forced,
            pos,
            delta: [0.0; 3],
            speed: [0.0; 3],
            count: 1,
            mode: 0,
        };
        let _ = self.send(Event::Particles(Box::new(p)));
    }

    /// Damage taken by a mob; true if it landed.
    pub(crate) fn mob_hurt(
        &mut self,
        m: &mut Mob,
        ty: &str,
        amount: f32,
        by: Option<Ent>,
        src: Option<[f64; 3]>,
    ) -> bool {
        self.mob_hurt_dir(m, ty, amount, by, src, None)
    }

    /// `mob_hurt` by the mob `by`, taken out of the list during its own tick: its frozen
    /// copy stays findable by id for the duration of the hit (anger of the warden, of the nautilus,
    /// villager gossip, aimed knockback of the sulfur cube).
    pub(crate) fn mob_hurt_by(&mut self, m: &mut Mob, by: &Mob, ty: &str, amount: f32) -> bool {
        self.ghost_push(by);
        let r = self.mob_hurt(m, ty, amount, Some(Ent::Mob(by.id)), Some(by.pos()));
        self.ghost_pop();
        r
    }

    /// Like `mob_hurt`, with the knockback direction given as is (`dir` = the game's `xd`, `zd`) instead
    /// of deduced from the source: projectiles, whose knockback follows their velocity
    /// (the computation of the horizontal knockback direction) without going through a position (roundings).
    pub(crate) fn mob_hurt_dir(
        &mut self,
        m: &mut Mob,
        ty: &str,
        amount: f32,
        by: Option<Ent>,
        src: Option<[f64; 3]>,
        dir: Option<(f64, f64)>,
    ) -> bool {
        // `Invulnerable` flag or temporary invulnerability: only `bypasses_invulnerability` and a
        // creative player get through (not through a ghast's: the game's ghast has its own rule).
        if m.is_invulnerable()
            && !self.dmg_tag(ty, "minecraft:bypasses_invulnerability")
            && !(by == Some(Ent::Player) && self.game_mode == 1 && m.kind != Kind::Ghast)
        {
            return false;
        }
        // Tag `fall_damage_immune`: the game computes no fall damage, hence no hit
        // (nor species hook: the wither does not arm block destruction).
        if ty == "minecraft:fall" && m.kind.info().fall_immune {
            return false;
        }
        // G5-F02e: Frost Walker, immunity to hot-floor damage.
        if self.frost_walker_immune(m.equipment[2].as_ref(), ty) {
            return false;
        }
        // F14: linked creaking (damage diverted to its heart), buried warden, breeze among themselves.
        if let Some(r) = super::recent::before_hurt(self, m, ty, by) {
            return r;
        }
        let hurt = self.mob_hurt_common(m, ty, amount, by, src, dir);
        // F14: warden anger, whether or not the damage landed.
        super::recent::after_hurt(self, m, ty, by);
        hurt
    }

    fn mob_hurt_common(
        &mut self,
        m: &mut Mob,
        ty: &str,
        amount: f32,
        by: Option<Ent>,
        src: Option<[f64; 3]>,
        dir: Option<(f64, f64)>,
    ) -> bool {
        if m.removed || m.health <= 0.0 || m.dead {
            return false;
        }
        // F08: thorns of the motionless guardian, restarted wandering (before everything else).
        super::aquatic::before_hurt(self, m, ty, by);
        // F15: dragon (part hit, phase), closed shulker insensitive to arrows.
        if let Some(r) = self.end_before_hurt(m, ty, amount, by, src, dir) {
            return r;
        }
        // F16: wither (summoning, armor against arrows, undead allies).
        if let Some(r) = super::wither::boss::before_hurt(self, m, ty, by) {
            return r;
        }
        // F10: armadillo rolled up (reduced damage), axolotl (playing dead), allay (beloved player).
        let mut amount = amount;
        if let Some(r) = super::brainy::before_hurt(self, m, ty, &mut amount, by) {
            return r;
        }
        // F01: the silverfish hit by an entity (or poison/magic) wakes its fellows.
        if m.kind == Kind::Silverfish && (by.is_some() || self.dmg_tag(ty, "minecraft:always_triggers_silverfish")) {
            super::pests::notify_hurt(m);
        }
        // F03: magma insensitive to fire; full sulfur cube (immunities, priming).
        // F07: fire resistance effect (witch's potion).
        // F12: the big ball returned by the player overrides the ghast's insensitivity.
        if (m.kind.info().fire_immune || m.x.effects.iter().any(|e| e.id == "minecraft:fire_resistance"))
            && self.dmg_tag(ty, "minecraft:is_fire")
            && !super::nether::reflected_fireball(m, self.mobs.fireballs.direct_large, by)
        {
            return false;
        }
        // S7: fire resistance.
        if self.dmg_tag(ty, "minecraft:is_fire") && m.x.effects.iter().any(|e| e.id == "minecraft:fire_resistance") {
            return false;
        }
        if m.kind == Kind::SulfurCube
            && let Some(r) = self.sulfur_hurt(m, ty, amount, by, src)
        {
            return r;
        }
        if (ty == "minecraft:in_wall" || ty == "minecraft:cramming") && m.no_ai {
            // Mobs without AI stay sensitive like the others.
        }
        // Non-passenger enderman: projectiles dodged by teleporting; thrown potion: only
        // water hurts (normal hit), then teleport in all cases.
        // Ordinary hit without an author: teleport 9 times out of 10, whether or not the hit landed.
        if m.kind == Kind::Enderman {
            let free = m.mount.vehicle.is_none();
            if free && self.dmg_tag(ty, "minecraft:is_projectile") {
                self.enderman_teleport_repeat(m);
                return false;
            }
            if let Some(water) = self.proj.direct_potion {
                let hurt = water && self.mob_hurt_body(m, ty, amount, by, src, dir);
                self.enderman_teleport_repeat(m);
                return hurt;
            }
            let hurt = self.mob_hurt_body(m, ty, amount, by, src, dir);
            // Draw always consumed; dead or passenger, the teleport is then refused.
            if by.is_none() && m.rng.next_int_bound(10) != 0 {
                self.enderman_teleport_random(m);
            }
            return hurt;
        }
        self.mob_hurt_body(m, ty, amount, by, src, dir)
    }

    /// Up to 64 random teleport attempts (stops at the first success), never as a passenger.
    fn enderman_teleport_repeat(&mut self, m: &mut Mob) {
        if m.mount.vehicle.is_some() {
            return;
        }
        for _ in 0..64 {
            if self.enderman_teleport_random(m) {
                break;
            }
        }
    }

    /// The hit itself (after the species hooks): invulnerability delay, damage, anger,
    /// knockback, sounds, death, own reactions.
    fn mob_hurt_body(
        &mut self,
        m: &mut Mob,
        ty: &str,
        amount: f32,
        by: Option<Ent>,
        src: Option<[f64; 3]>,
        dir: Option<(f64, f64)>,
    ) -> bool {
        let crack_before = (m.kind == Kind::IronGolem).then(|| super::golems::iron::crackiness(m));
        super::surface::before_hurt(m);
        self.wild_before_hurt(m); // F11: foraging interrupted, panda standing
        // F07: raider of an active raid: boss bar updated (before the damage).
        if let Some(id) = m.ill.raid.filter(|&id| super::illagers::is_raider(m.kind) && self.raid_is_active(id)) {
            self.raid_update_bossbar(id);
        }
        m.no_action_time = 0;
        let mut damage = amount.max(0.0);
        // Damage as received, before the extra freeze damage and the helmet (`player_hurt_entity`).
        let original = damage;
        // `#freeze_hurts_extra_types`: freeze damage ×5 (strider, blaze, magma cube).
        if m.kind.freeze_hurts_extra() && self.dmg_tag(ty, "minecraft:is_freezing") {
            damage *= 5.0;
        }
        // G5-F04: a hit that damages helmets (falling block) loses a quarter on a worn head.
        if m.equipment[5].is_some() && self.dmg_tag(ty, "minecraft:damages_helmet") {
            damage *= 0.75;
        }
        if !damage.is_finite() {
            damage = f32::MAX;
        }
        let bypass_cd = self.dmg_tag(ty, "minecraft:bypasses_cooldown");
        let full = if m.damage_cooldown as f32 > 10.0 && !bypass_cd {
            if damage <= m.last_hurt {
                return false;
            }
            self.mob_actually_hurt(m, ty, damage - m.last_hurt, by);
            m.last_hurt = damage;
            false
        } else {
            m.last_hurt = damage;
            m.damage_cooldown = 20;
            self.mob_actually_hurt(m, ty, damage, by);
            m.hurt_time = 10;
            true
        };
        if let Some(b) = by
            && !self.dmg_tag(ty, "minecraft:no_anger")
            // `#no_anger_from_wind_charge`: the wind charge does not designate the thrower.
            && !(ty == "minecraft:wind_charge" && m.kind.no_anger_from_wind_charge())
        {
            m.last_hurt_by_mob = Some((b, m.tick_count));
            self.npc_hurt_by(m, b); // F05: reputation
        }
        match by {
            Some(Ent::Player) => {
                m.last_hurt_by_player = 100;
                // Its attacker, its anger and the credit of its death name this player.
                self.bind_mob_to_installed_player(m);
            }
            // G5-F04: a tamed wolf credits its owner (tamed without an owner: memory forgotten).
            Some(Ent::Mob(id)) => {
                if let Some(w) = self.mob_ref(id).filter(|w| w.kind == Kind::Wolf && w.tame) {
                    m.last_hurt_by_player = if w.owner.is_some() { 100 } else { 0 };
                }
            }
            None => {}
        }
        if full {
            let _ = self.send(Event::Entity(EntityEvent::Hurt { id: m.id, yaw: 0.0 }));
            if !self.dmg_tag(ty, "minecraft:no_impact") {
                m.track.sync_velocity = true;
            }
            if !self.dmg_tag(ty, "minecraft:no_knockback") {
                let (xd, zd) = hurt_knockback_dir(dir, src, m.pos());
                self.mob_knockback(m, 0.4, xd, zd, by, damage, false);
            }
        }
        if m.health <= 0.0 {
            if full {
                let d = super::surface::hurt_sound(m, true)
                    .or_else(|| super::mounts::hurt_sound(m, true))
                    .or_else(|| super::golems::hurt_sound(m, true))
                    .or_else(|| super::aquatic::hurt_sound(m, true))
                    .or_else(|| super::sealife::hurt_sound(m, true))
                    .or_else(|| super::brainy::hurt_sound(m, true))
                    .or_else(|| super::recent::hurt_sound(m, true))
                    .or_else(|| super::end::hurt_sound(m, true))
                    .map(str::to_owned)
                    .or_else(|| self.variant_sound(m, "death_sound"))
                    .unwrap_or_else(|| m.death_sound().to_owned());
                self.make_sound(m, &d);
                self.thorns_hit_sound(ty, 5, m.pos());
            }
            self.mob_die(m, ty, by);
        } else if full {
            m.ambient_time = -m.kind.info().ambient_interval;
            let h = super::surface::hurt_sound(m, false)
                .or_else(|| super::mounts::hurt_sound(m, false))
                .or_else(|| super::golems::hurt_sound(m, false))
                .or_else(|| super::aquatic::hurt_sound(m, false))
                .or_else(|| super::sealife::hurt_sound(m, false))
                .or_else(|| super::brainy::hurt_sound(m, false))
                .or_else(|| super::recent::hurt_sound(m, false))
                .or_else(|| super::end::hurt_sound(m, false))
                .or_else(|| self.wolf_armor_absorbs(m, ty).then_some("minecraft:item.wolf_armor.damage"))
                .map(str::to_owned)
                .or_else(|| self.variant_sound(m, "hurt_sound"))
                .unwrap_or_else(|| m.hurt_sound().to_owned());
            self.make_sound(m, &h);
            self.thorns_hit_sound(ty, 5, m.pos());
        }
        m.last_hit = Some(Hit { ty: ty.to_owned(), by, tick: self.time as u64 });
        self.mob_hurt_effects(m); // infestation
        // Any hit whose source is the player (melee, sweep, smash, thorns, projectile, blast of
        // its TNT…), once the hurt is through, like the game.
        if by == Some(Ent::Player) {
            self.s9_player_hurt_mob(m, ty, original, damage);
        }
        // Own reactions.
        match m.kind {
            Kind::Pig | Kind::Cow | Kind::Sheep | Kind::Chicken | Kind::Wolf | Kind::Cat => {
                m.reset_love();
                if m.kind == Kind::Wolf || m.kind == Kind::Cat {
                    m.sitting = false;
                }
            }
            k if super::surface::handles(k) => super::surface::on_hurt(self, m, by),
            k if super::mounts::handles(k) => super::mounts::on_hurt(m, true),
            // F09: ink and darkening of the squids, anger of the nautiluses.
            k if super::sealife::handles(k) => super::sealife::on_hurt(self, m, by),
            // F10: forgotten love, armadillo (danger, ball).
            k if super::brainy::handles(k) => super::brainy::after_hurt(self, m, ty, by),
            // F13: anger, retreat and retaliation of piglins and hoglins.
            k if super::piglins::handles(k) => super::piglins::after_hurt(self, m, by),
            // F15: shulker teleport and duplication.
            k if super::end::handles(k) => self.end_after_hurt(m, ty),
            _ => {}
        }
        // Any hurt animal forgets its love and its cause (fox, bee, panda, strider,
        // hoglin, happy ghast included).
        if m.kind.is_animal() {
            m.reset_love();
        }
        if m.kind == Kind::Wolf && by == Some(Ent::Player) && !m.tame {
            m.anger_target = Some(Ent::Player);
            m.anger_end = self.time + i64::from(m.rng.int_between(400, 780));
        }
        // F06: iron golem, sound of the cracks when their level changes.
        self.golem_after_hurt(m, crack_before);
        // Zombie family: reinforcements, even if the hit killed.
        if m.kind.zombie_like() {
            self.zombie_reinforcements(m, by);
        }
        true
    }

    /// G5-F04: enchantment protection of a mob's equipment against a hit (`damage_protection`
    /// effects of the pack: none for `bypasses_invulnerability` damage, like their conditions).
    fn mob_protection(&mut self, m: &mut Mob, ty: &str, by: Option<Ent>) -> f32 {
        // Nothing worn (almost every mob): no context to build.
        if m.equipment.iter().all(Option::is_none) {
            return 0.0;
        }
        let (mut world, mut own) = (self.mobs.rng, m.rng);
        let p = {
            let gear: [(usize, Option<&Stack>); 8] = std::array::from_fn(|k| (k, m.equipment[k].as_ref()));
            let ctx = self.hurt_ctx(ty, crate::progress::pred::Who::Mob(m), m.pos(), by);
            self.ench_protection(&gear, &ctx, &mut world, &mut own)
        };
        (self.mobs.rng, m.rng) = (world, own);
        p
    }

    /// G5-F04: armor absorption (toughness of the attributes), the armor fraction modified by the
    /// weapon of a direct attacker (Breach).
    fn mob_armor_absorb(&mut self, m: &mut Mob, ty: &str, dmg: f32, by: Option<Ent>) -> f32 {
        let armor = (m.attrs.armor.value() as f32).floor();
        if armor <= 0.0 {
            return dmg;
        }
        let t = 2.0 + m.attrs.toughness.value() as f32 / 4.0;
        let a = (armor - dmg / t).clamp(armor * 0.2, 20.0);
        let mut frac = a / 25.0;
        if let Some(w) = self.source_weapon(ty, by) {
            let mut own = m.rng;
            frac = {
                let ctx = self.hurt_ctx(ty, crate::progress::pred::Who::Mob(m), m.pos(), by);
                self.armor_fraction(frac, Some(&w), &ctx, &mut own)
            };
            m.rng = own;
        }
        dmg * (1.0 - frac)
    }

    /// G5-F04: a horse's body armor wears down with the hits (⌊d/4⌋, at least 1), if it is worn
    /// down on hurt and does not resist the type; the other mobs never wear their pieces.
    fn horse_armor_wear(&mut self, m: &mut Mob, ty: &str, dmg: f32) {
        if dmg <= 0.0 || m.kind != Kind::Horse {
            return;
        }
        let n = (dmg / 4.0).max(1.0) as i32;
        if m.equipment[6].as_ref().is_some_and(|s| self.life.items.damageable(s) && self.armor_wears(s, ty)) {
            self.mob_hurt_slot(m, 6, n);
        }
    }

    fn mob_actually_hurt(&mut self, m: &mut Mob, ty: &str, mut dmg: f32, by: Option<Ent>) {
        super::mounts::before_damage(self, m); // F04: the camel stands up
        // Wolf armor: it takes everything, health does not move.
        if self.wolf_armor_absorb(m, ty, dmg) {
            return;
        }
        if !self.dmg_tag(ty, "minecraft:bypasses_armor") {
            self.horse_armor_wear(m, ty, dmg);
            dmg = self.mob_armor_absorb(m, ty, dmg, by);
        }
        // S7: Resistance effect, then enchantment protections of the armor (feet → body).
        if !self.dmg_tag(ty, "minecraft:bypasses_effects") {
            if !self.dmg_tag(ty, "minecraft:bypasses_resistance")
                && let Some(a) = m.x.effects.iter().find(|e| e.id == "minecraft:resistance").map(|e| e.amp)
            {
                let before = dmg;
                dmg = (dmg * (25 - (a + 1) * 5) as f32 / 25.0).max(0.0);
                // `damage_dealt_resisted` statistic of the author player (tenths of a point).
                let resisted = before - dmg;
                if by == Some(Ent::Player) && resisted > 0.0 && resisted < 3.402_823_5e37 {
                    self.award_custom("damage_dealt_resisted", (resisted * 10.0).round() as i32);
                }
            }
            if dmg > 0.0 && !self.dmg_tag(ty, "minecraft:bypasses_enchantments") {
                let p = self.mob_protection(m, ty, by);
                if p > 0.0 {
                    dmg *= 1.0 - p.clamp(0.0, 20.0) / 25.0;
                }
            }
        }
        // F07: the witch only takes 15% of magic damage (`witch_resistant_to`), after
        // the resistance and the protections.
        if m.kind == Kind::Witch && self.dmg_tag(ty, "minecraft:witch_resistant_to") {
            dmg *= 0.15;
        }
        if dmg != 0.0 {
            // G5-F04: the hits of an owned tamed animal are kept for its death message.
            if m.owner.is_some() && super::species::tamable(m.kind) {
                self.mob_record_damage(m, ty, dmg, by);
            }
            m.health = (m.health - dmg).clamp(0.0, m.max_health());
            self.mob_game_event(m, "minecraft:entity_damage");
        }
        // F06: the hurt copper golem goes back to rest.
        if m.kind == Kind::CopperGolem {
            m.golem.state = 0;
        }
    }

    /// Knockback (`knockback`).
    pub(crate) fn knockback(m: &mut Mob, power: f64, mut xd: f64, mut zd: f64) {
        let power = power * (1.0 - m.attrs.knockback_resistance.value());
        if power <= 0.0 {
            return;
        }
        m.needs_sync = true;
        while xd * xd + zd * zd < f64::from(1.0e-5f32) {
            xd = (m.rng.next_double() - m.rng.next_double()) * 0.01;
            zd = (m.rng.next_double() - m.rng.next_double()) * 0.01;
        }
        let l = (xd * xd + zd * zd).sqrt();
        let (dx, dz) = (xd / l * power, zd / l * power);
        let v = m.vel;
        m.vel = [v[0] / 2.0 - dx, if m.on_ground { (v[1] / 2.0 + power).min(0.4) } else { v[1] }, v[2] / 2.0 - dz];
    }

    /// Death (`die`): loot, experience, event 3, "dying" pose.
    pub(crate) fn mob_die(&mut self, m: &mut Mob, ty: &str, by: Option<Ent>) {
        if m.removed || m.dead {
            return;
        }
        // F15: dragon's fatal hit (never "dead", death phase).
        if self.end_die(m, ty, by) {
            return;
        }
        // G5-F04: the owner of a tamed animal receives its death message (before the rest).
        if let Some(owner) = m.owner.filter(|_| super::species::tamable(m.kind))
            && self.rule("minecraft:show_death_messages", true)
        {
            self.pet_death_message(m, owner);
        }
        // Like the game, a sleeper is woken before the rest of the death, conversion by a zombie
        // included: bed freed, stood up beside it.
        self.npc_wake(m);
        m.dead = true;
        m.use_ticks = -1;
        // Wolf: no longer wet or shaking off.
        (m.x.wet, m.x.shaking, m.x.shake) = (false, false, [0.0; 2]);
        self.s9_mob_died(m, ty, by); // S9: player credit
        // F01: a zombie that kills a villager may convert it (neither loot nor experience).
        if self.killed_by_zombie(m, by) {
            self.mob_event(m, 3);
            m.needs_sync = true;
            return;
        }
        self.charged_creeper_skull(m, ty, by); // head dropped (charged creeper)
        self.npc_died(m, by); // F05: witnesses, POI, trading
        self.raider_died(m, by); // F07: raid (leader, hero, removal)
        self.mob_game_event(m, "minecraft:entity_die");
        let player_killed = m.last_hurt_by_player > 0;
        // Monsters drop loot and experience even as babies (baby zombie).
        let monster = m.kind.is_monster() && m.kind != Kind::Phantom && !super::mounts::handles(m.kind);
        // F13: the hoglin (animal) only drops its loot as an adult, but always its experience.
        let loot_ok = super::piglins::drops_loot(m).unwrap_or_else(|| monster || !m.is_baby());
        self.wild_death_loot(m); // F11: fox's mouth item
        if loot_ok && self.rule("minecraft:mob_drops", true) {
            self.drop_loot(m, ty, by, player_killed);
            self.drop_equipment(m, by, player_killed);
            self.wither_death_loot(m); // F16: nether star
        }
        // F04: chest and contents of the mount, even small.
        self.mount_drop_inventory(m);
        super::brainy::on_death(self, m); // F10: sniffer at rest, allay's reserve
        super::piglins::on_death(self, m); // F13: piglin inventory
        let xp_ok = player_killed && (monster || !m.is_baby()) && self.rule("minecraft:mob_drops", true);
        if xp_ok {
            let xp = self.mob_xp(m);
            if xp > 0 {
                self.spawn_orbs(m.pos(), xp);
            }
        }
        self.wither_rose_mob(m); // F16: victim of a wither
        self.mob_event(m, 3);
        if let Some(c) = m.carried.take() {
            let name = self.state_name(c).to_owned();
            self.drop_mob_item(m, Stack { id: name, count: 1, components: None });
        }
        m.needs_sync = true;
    }

    /// G5-F04: death message of a tamed animal sent to its owner alone, if connected (the host or
    /// a local network guest).
    fn pet_death_message(&mut self, m: &Mob, owner: u128) {
        let seat = match &self.lan {
            Some(l) if l.shared.host.1 == owner => Some(None),
            Some(l) => l.guests.iter().position(|g| g.uuid == owner).map(Some),
            None => (owner == self.uuid).then_some(None),
        };
        let Some(seat) = seat else { return };
        let empty = crate::vitals::Combat::default();
        let c = m.combat.as_deref().unwrap_or(&empty);
        let msg = self.death_message_of(c, self.mob_display(m), self.mob_kill_credit(m));
        self.as_player(seat, |sv| sv.send_personal(Event::Chat { component: Box::new(msg), tag: rc_client::ChatTag::System }));
    }

    fn mob_xp(&self, m: &mut Mob) -> i32 {
        if let Some(x) = self.cube_xp(m) {
            return x;
        }
        // F09: 1 to 3, without equipment bonus (animals and aquatic mobs).
        if let Some(x) = super::piglins::xp(m) {
            return x;
        }
        if let Some(x) = super::brainy::xp(m) {
            return x;
        }
        if let Some(x) = super::sealife::xp(m) {
            return x;
        }
        // Chicken jockey: 10, without a draw.
        if m.kind == Kind::Chicken && m.chicken_jockey {
            return 10;
        }
        let mut base = if m.kind.is_animal() {
            1 + m.rng.next_int_bound(3)
        } else if m.kind.is_cube() {
            m.cube.size
        } else {
            m.kind.info().xp
        };
        // Baby zombie: experience × 2.5 (truncated).
        if m.kind.zombie_like() && m.baby {
            base = (base as f32 * 2.5) as i32;
        }
        if base <= 0 {
            return base;
        }
        let mut r = base;
        for (i, s) in m.equipment.iter().enumerate() {
            if s.is_some() && i != 7 && m.drop_chance[i] <= 1.0 {
                r += 1 + m.rng.next_int_bound(3);
            }
        }
        r
    }

    fn drop_mob_item(&mut self, m: &Mob, s: Stack) {
        // At the position, raised by 0, random velocity.
        let pos = [m.pos()[0], m.pos()[1], m.pos()[2]];
        let r = &mut self.mobs.rng;
        let v = [r.next_double() * 0.2 - 0.1, 0.2, r.next_double() * 0.2 - 0.1];
        self.spawn_item(pos, v, s, 10);
    }

    fn drop_loot(&mut self, m: &mut Mob, ty: &str, by: Option<Ent>, player_killed: bool) {
        // Imposed table (`DeathLootTable`, forgotten once drawn), otherwise the species'.
        let table = m.death_loot.take().unwrap_or_else(|| m.kind.loot_table());
        for d in self.entity_loot(m, ty, by, player_killed, &table) {
            self.drop_mob_item(m, d);
        }
    }

    /// Charged creeper author of the fatal hit (before the victim's loot): the
    /// `charged_creeper/root` table is drawn for the victim (its head); the first item out
    /// exhausts the creeper's right for the whole explosion.
    fn charged_creeper_skull(&mut self, m: &mut Mob, ty: &str, by: Option<Ent>) {
        let Some((cid, false)) = self.mobs.charged_skull else { return };
        if by != Some(Ent::Mob(cid)) || !self.rule("minecraft:mob_drops", true) {
            return;
        }
        for d in self.entity_loot(m, ty, by, false, "minecraft:charged_creeper/root") {
            self.drop_mob_item(m, d);
            self.mobs.charged_skull = Some((cid, true));
        }
    }

    /// Draw of a loot table for a dead mob: victim's context; fixed seed
    /// (`DeathLootTableSeed`), otherwise sequence declared by the table, otherwise the level's random source.
    pub(crate) fn entity_loot(
        &mut self,
        m: &Mob,
        ty: &str,
        by: Option<Ent>,
        player_killed: bool,
        table: &str,
    ) -> Vec<Stack> {
        let table = table.to_owned();
        let weapon = if by == Some(Ent::Player) { self.slots[36 + self.held as usize].clone() } else { None };
        let looting = weapon.as_ref().map_or(0, |w| self.life.items.enchantment(w, "minecraft:looting"));
        let data = &self.data;
        let tags = &self.block_tags;
        let tag_fn = |tag: &str, name: &str| {
            let id = data.blocks.iter().position(|b| b.name == name).map(|i| i as u32);
            id.is_some_and(|id| tags.get(tag).is_some_and(|v| v.binary_search(&id).is_ok()))
        };
        // W8c: the world lent to the conditions (direct attacker and its equipment, tags of the
        // source, vehicle…); the non-living direct entity is only known by its type.
        use crate::progress::pred::{Dmg, Who};
        let seq =
            self.life.pack.get("loot_table", &table).and_then(|t| t["random_sequence"].as_str().map(str::to_owned));
        // Sequence generator taken out for the duration of the draw (the world stays lent).
        let seq_rng = match &seq {
            Some(s) if m.death_loot_seed == 0 => Some(*self.life.seqs.get(s)),
            _ => None,
        };
        let who = |e: Option<Ent>| -> Option<Who<'_>> {
            match e? {
                Ent::Player => Some(Who::E(crate::commands::ents::E::Player)),
                Ent::Mob(id) => self.mob_ref(id).map(Who::Mob),
            }
        };
        let source = who(by);
        let direct = crate::loot::direct_of(ty);
        let direct_who = match direct {
            Some(None) => source,
            _ => None,
        };
        let world = self.loot_world(crate::progress::pred::Ctx {
            this: Some(Who::Mob(m)),
            origin: Some(m.pos()),
            attacker: source,
            direct_attacker: direct_who,
            attacking_player: player_killed.then_some(Who::E(crate::commands::ents::E::Player)),
            damage: Some(Dmg { ty: ty.to_owned(), source, direct: direct_who, pos: None }),
            ..Default::default()
        });
        let (recipes, items) = (&self.life.recipes, &self.life.items);
        let smelt =
            |s: &Stack| recipes.cooking(items, crate::recipes::CookKind::Smelting, s).map(|(_, c)| c.result.stack());
        let loot = crate::loot::Loot { pack: &self.life.pack, items: &self.life.items, block_tag: &tag_fn };
        let ctx = crate::loot::Ctx {
            world: Some(&world),
            smelt: Some(&smelt),
            direct_type: direct.flatten().map(str::to_owned),
            killed_by_player: player_killed,
            on_fire: m.on_fire(),
            baby: m.is_baby(),
            looting,
            damage_type: Some(ty.to_owned()),
            sheep: (m.kind == Kind::Sheep).then(|| (DYES[(m.wool & 15) as usize].to_owned(), m.sheared)),
            cube_size: m.kind.is_cube().then_some(m.cube.size),
            raider: super::illagers::is_raider(m.kind)
                .then(|| (self.raider_has_raid(m), super::illagers::is_captain(m))),
            // F10: source entity (frog and its variant).
            source_entity: self.bz_loot_source(by),
            this_type: Some(m.kind.info().name.to_owned()),
            // Author of the damage (disc of the creeper killed by a skeleton).
            attacker: self.bz_loot_source(by).map(|(n, _)| {
                let tags = self.entity_tags_named(&n);
                (n, tags)
            }),
            ..Default::default()
        };
        // Fixed seed (`DeathLootTableSeed`): Java generator seeded with it.
        if m.death_loot_seed != 0 {
            let mut rng = super::jrand::JRand::new(m.death_loot_seed);
            return loot.drops(&table, &ctx, &mut rng);
        }
        let mut rng = seq_rng.unwrap_or(self.life.rng);
        let d = loot.drops(&table, &ctx, &mut rng);
        drop(ctx);
        drop(world);
        match &seq {
            Some(s) => *self.life.seqs.get(s) = rng,
            None => self.life.rng = rng,
        }
        d
    }

    /// Equipment dropped on death: slot skipped if its chance is
    /// zero; chance modified by the `equipment_drops` effects when the author is a living
    /// entity; draw only for an item without a vanishing curse, killed by a
    /// player or with a guaranteed chance.
    fn drop_equipment(&mut self, m: &mut Mob, by: Option<Ent>, player_killed: bool) {
        for i in 0..8 {
            let mut chance = m.drop_chance[i];
            if chance == 0.0 {
                continue;
            }
            let guaranteed = chance > 1.0;
            if let Some(by) = by {
                chance = self.equipment_drop_chance(m, by, chance);
            }
            let Some(s) = m.equipment[i].clone() else { continue };
            if self.ench_has(&s, "minecraft:prevent_equipment_drop") {
                continue;
            }
            if (player_killed || guaranteed) && m.rng.next_float() < chance {
                let mut s = s;
                if !guaranteed && self.life.items.damageable(&s) {
                    let max = self.life.items.max_damage(&s);
                    let inner = m.rng.next_int_bound((max - 3).max(1));
                    let dmg = max - m.rng.next_int_bound(1 + inner);
                    crate::items::set_component(&mut s, "minecraft:damage", rc_proto::nbt::Nbt::Int(dmg.max(0)));
                }
                self.drop_mob_item(m, s);
                // Emptied through the equipment reaction, like the game: an `unequip` vibration.
                self.mob_set_item_slot(m, i, None);
            }
        }
    }

    /// Effects of the victim's equipment, then of that of
    /// the damage author (the victim's random source for random effects).
    fn equipment_drop_chance(&self, m: &mut Mob, by: Ent, chance: f32) -> f32 {
        use super::mob::SLOT_NAMES;
        let (who, gear): (Option<&str>, Vec<(&str, Option<&Stack>)>) = match by {
            Ent::Player => {
                let sl = |i: usize| self.slots[i].as_ref();
                let g = vec![
                    ("mainhand", sl(36 + self.held as usize)),
                    ("offhand", sl(45)),
                    ("feet", sl(8)),
                    ("legs", sl(7)),
                    ("chest", sl(6)),
                    ("head", sl(5)),
                ];
                (Some("minecraft:player"), g)
            }
            Ent::Mob(id) => match self.mobs.mob_ref_any(id) {
                Some(a) => (
                    Some(a.kind.info().name),
                    SLOT_NAMES.iter().copied().zip(a.equipment.iter().map(Option::as_ref)).collect(),
                ),
                None => (None, Vec::new()),
            },
        };
        let victim: Vec<(&str, Option<&Stack>)> =
            SLOT_NAMES.iter().copied().zip(m.equipment.iter().map(Option::as_ref)).collect();
        let ctx = crate::projectiles::ench::Ctx { this: Some(m.kind.info().name), attacker: who, ..Default::default() };
        self.ench_equipment_drop_chance(&victim, &gear, &ctx, chance, &mut m.rng)
    }

    /// Melee hit of the mob.
    pub(crate) fn mob_attack(&mut self, m: &mut Mob, t: Ent) -> bool {
        let hurt = self.mob_attack_core(m, t);
        // The base hit memorizes the victim; the bee, the iron golem and the creeper
        // have their own, which does not memorize it.
        if hurt && !matches!(m.kind, Kind::Bee | Kind::IronGolem | Kind::Creeper) {
            m.last_hurt_mob = Some(t);
        }
        hurt
    }

    fn mob_attack_core(&mut self, m: &mut Mob, t: Ent) -> bool {
        // F06: iron golem's own hit (vertical throw).
        if m.kind == Kind::IronGolem {
            return self.iron_golem_attack(m, t);
        }
        // F11: bee sting; panda bite (non-aggressive: "bit").
        if let Some(r) = self.wild_attack(m, t) {
            return r;
        }
        self.illager_before_attack(m); // F07: ravager's hit
        super::recent::before_melee(self, m); // F14: warden impact, creaking's arms
        // Attack attribute (G5-F04: the held weapon's modifiers are in it, vindicator's axe, vex's
        // sword…), then enchantments.
        let mut dmg = m.attrs.attack.value() as f32;
        // S7: weapon enchantments (sharpness, smite, bane; knockback).
        let weapon = m.equipment[0].clone();
        let wl = |s: &Self, e: &str| weapon.as_ref().map_or(0, |w| s.life.items.enchantment(w, e)) as f32;
        let sharp = wl(self, "minecraft:sharpness");
        if sharp > 0.0 {
            dmg += 0.5 * sharp + 0.5;
        }
        if let Ent::Mob(id) = t
            && let Some(o) = self.mob_ref(id)
            && let Some(w) = &weapon
        {
            dmg += self.kind_ench_bonus(w, o.kind);
        }
        let kb_ench = wl(self, "minecraft:knockback");
        // G5-F04: damage source of the weapon (its type, the mace's smash), then the mace's bonus
        // by fallen block (mobs never glide).
        let smash = weapon.as_ref().is_some_and(|w| w.id == crate::mace::MACE)
            && crate::mace::can_smash(m.fall_distance, false);
        let ty = self.weapon_damage_type(weapon.as_ref(), smash, "minecraft:mob_attack");
        let ty = ty.as_str();
        if smash {
            // Value effects drawn on the victim's random (the player's, or the mob's, written back).
            let (mut own_p, mut own_m) = (self.life.rng, t_rng(self, t));
            let bonus = {
                let victim = match t {
                    Ent::Player => Some(crate::progress::pred::Who::E(crate::commands::ents::E::Player)),
                    Ent::Mob(id) => self.mob_ref(id).map(crate::progress::pred::Who::Mob),
                };
                let origin = self.ent_pos(t).unwrap_or_else(|| m.pos());
                let own: &mut dyn crate::randsrc::Rand = if t == Ent::Player { &mut own_p } else { &mut own_m };
                match victim {
                    Some(v) => {
                        let ctx = self.damage_ctx(ty, v, origin, Some(crate::progress::pred::Who::Mob(m)), true);
                        self.mace_bonus(weapon.as_ref(), m.fall_distance, true, &ctx, own)
                    }
                    None => 0.0,
                }
            };
            match t {
                Ent::Player => self.life.rng = own_p,
                Ent::Mob(id) => {
                    if let Some(o) = self.mobs.get_mut(id) {
                        o.rng = own_m;
                    }
                }
            }
            dmg += bonus;
        }
        let hurt = match t {
            Ent::Player => {
                let kb = (m.pos()[0] - self.pos[0], m.pos()[2] - self.pos[2]);
                let r = self.player_hurt_by_mob(m, ty, dmg, Some(kb));
                self.take_riposte(m);
                r
            }
            Ent::Mob(id) => {
                let Some(i) = self.mob_index(id) else { return false };
                let mut o = self.mobs.list.take(i).expect("mob");
                // F01: attacker's random source lent to the reaction to a kill (villager conversion).
                self.mobs.killer = Some((m.rng, m.kind, m.id, m.block_pos()));
                let r = self.mob_hurt_by(&mut o, m, ty, dmg);
                self.aqua_flush_thorns(m); // F08: struck guardian's thorns
                if let Some((rng, _, _, _)) = self.mobs.killer.take() {
                    m.rng = rng;
                }
                self.mobs.list.put(i, o);
                r
            }
        };
        if hurt {
            let kb = (m.attrs.attack_knockback.value() as f32 + kb_ench) / 2.0;
            if kb > 0.0 {
                let (s, c) = (sin(m.yaw * DEG), cos(m.yaw * DEG));
                match t {
                    Ent::Player => self.player_knockback(f64::from(kb), f64::from(s), f64::from(-c)),
                    Ent::Mob(id) => {
                        // Victim's own knockback (full sulfur cube:
                        // attacker's aim), coming from an effect, with the hit's damage.
                        if let Some(i) = self.mob_index(id) {
                            let mut o = self.mobs.list.take(i).expect("mob");
                            self.ghost_push(m);
                            let by = Some(Ent::Mob(m.id));
                            self.mob_knockback(&mut o, f64::from(kb), f64::from(s), f64::from(-c), by, dmg, true);
                            self.ghost_pop();
                            self.mobs.list.put(i, o);
                        }
                    }
                }
                m.vel[0] *= 0.6;
                m.vel[2] *= 0.6;
            }
            // G5-F04: the weapon's own effect (mace smash), then the after-hit effects: the
            // victim's equipment (thorns), then the weapon's (fire aspect, bane, wind burst).
            if smash
                && let Some(st) = self.struck(t)
            {
                self.mob_smash(m, &st);
            }
            self.melee_post_attack(m, t, ty, weapon.as_ref());
            // F01: burning zombie (ignited target), husk hunger, cave spider poison.
            self.on_mob_hit(m, t);
            // F12: wither effect of the wither skeleton.
            super::nether::on_hit(self, m, t);
            // F02: killer bunny.
            if m.kind == Kind::Rabbit && m.variant == super::surface::rabbit::EVIL {
                let p = (m.rng.next_float() - m.rng.next_float()) * 0.2 + 1.0;
                self.mob_sound(m, "minecraft:entity.rabbit.attack", 1.0, p);
            }
            // F09: dolphin attack sound.
            if m.kind == Kind::Dolphin {
                self.mob_sound(m, "minecraft:entity.dolphin.attack", 1.0, 1.0);
            }
            // The creaking's attack sound (its own volume and voice pitch).
            if m.kind == Kind::Creaking {
                self.make_sound(m, "minecraft:entity.creaking.attack");
            }
        }
        self.wild_after_attack(m, hurt); // F11: panda bite sound
        hurt
    }

    /// Damage of a mob to the player: difficulty scaling, knockback, memory of the attacker.
    pub(crate) fn player_hurt_by_mob(&mut self, m: &Mob, ty: &str, amount: f32, kb: Option<(f64, f64)>) -> bool {
        let Some(d) = self.mob_damage_on_player(ty, amount) else { return false };
        self.player_hurt_by_mob_scaled(Box::new(m.clone()), ty, d, kb)
    }

    /// `player_hurt_by_mob` for a caller that already made its own copy of the mob: that copy
    /// becomes the damage source, instead of a second copy.
    pub(crate) fn player_hurt_by_mob_copy(
        &mut self,
        m: Box<Mob>,
        ty: &str,
        amount: f32,
        kb: Option<(f64, f64)>,
    ) -> bool {
        let Some(d) = self.mob_damage_on_player(ty, amount) else { return false };
        self.player_hurt_by_mob_scaled(m, ty, d, kb)
    }

    /// Damage of a mob's hit on the player after the difficulty; `None` if the difficulty cancels
    /// it (the shoulder parrots still get off).
    fn mob_damage_on_player(&mut self, ty: &str, amount: f32) -> Option<f32> {
        self.prog.riposte = None;
        let scaling = self
            .life
            .pack
            .get("damage_type", ty)
            .and_then(|v| v["scaling"].as_str().map(str::to_owned))
            .unwrap_or_default();
        let mut d = amount;
        if scaling == "when_caused_by_living_non_player" {
            d = match self.level.difficulty_id() {
                0 => 0.0,
                1 => (d / 2.0 + 1.0).min(d),
                3 => d * 1.5,
                _ => d,
            };
            if d == 0.0 {
                self.prog.dmg_src = Default::default(); // W8c
                // Damage cancelled by the difficulty: the parrots get off anyway.
                if !self.hurt_refused(ty) {
                    super::surface::parrot::remove_from_shoulders(self);
                }
                return None;
            }
        }
        Some(d)
    }

    /// The rest of `player_hurt_by_mob`, damage `d` already scaled; `m`: copy of the mob, kept as
    /// the damage source for the time of the hit.
    fn player_hurt_by_mob_scaled(&mut self, m: Box<Mob>, ty: &str, d: f32, kb: Option<(f64, f64)>) -> bool {
        let full = self.life.vit.cooldown <= 10;
        // W8c: without a source set by the caller (projectile, explosion), the mob hits
        // itself: position, weapon that disables the shield.
        if self.prog.dmg_src.pos.is_none() {
            self.prog.dmg_src = self.melee_src(&m, ty);
        }
        let id = m.id;
        // Direct attacker, named in the death message ("killed by a husk").
        self.mobs.hit_by = Some(m.kind);
        self.prog.attacker = Some(m); // S9: damage source
        let ok = self.hurt(ty, d);
        self.prog.attacker = None; // S9
        self.mobs.hit_by = None;
        if !ok {
            return false;
        }
        self.mobs.player_hurt_by = Some((Ent::Mob(id), self.mobs.player_ticks));
        if full && !self.dmg_tag(ty, "minecraft:no_knockback") {
            // Without an origin, the game still pushes, in a randomly drawn direction.
            let (xd, zd) = kb.unwrap_or((0.0, 0.0));
            self.player_knockback(0.4, xd, zd);
        }
        true
    }

    /// Damage without a responsible mob on the player, with the game's default knockback: direction
    /// `kb` (`xd`, `zd`: origin minus player, or opposite of a projectile's velocity), drawn
    /// at random without an origin; nothing for `no_knockback` types.
    pub(crate) fn player_hurt_kb(&mut self, ty: &str, amount: f32, kb: Option<(f64, f64)>) -> bool {
        let full = self.life.vit.cooldown <= 10;
        let ok = self.hurt(ty, amount);
        if ok && full && !self.dmg_tag(ty, "minecraft:no_knockback") {
            let (xd, zd) = kb.unwrap_or((0.0, 0.0));
            self.player_knockback(0.4, xd, zd);
        }
        ok
    }

    /// Takes back onto the mob `m` what the retaliation of a parried hit changed on it.
    pub(crate) fn take_riposte(&mut self, m: &mut Mob) {
        if let Some((id, stun, rng)) = self.prog.riposte.take()
            && id == m.id
        {
            m.ill.stun_tick = stun;
            m.rng = rng;
        }
    }

    /// Adds `v` to the player's velocity (`push(x, y, z)` then velocity resynced).
    pub(crate) fn player_add_velocity(&mut self, v: [f64; 3]) {
        let pv = self.mobs.player_vel;
        let nv = [pv[0] + v[0], pv[1] + v[1], pv[2] + v[2]];
        self.mobs.player_vel = nv;
        let _ = self
            .send(Event::Entity(EntityEvent::Motion { id: self.player_id, velocity: super::track::lp_quantize(nv) }));
    }

    /// Entity push of the player against the mob `a`: moved apart from each other by
    /// 0.05 at most (the player on one side, the mob on the other).
    pub(crate) fn player_push_from(&mut self, a: &mut Mob) {
        let Some((xa, za)) = Self::entity_push_amount(self.pos, a.pos()) else { return };
        self.player_add_velocity([-xa, 0.0, -za]);
        a.vel = [a.vel[0] + xa, a.vel[1], a.vel[2] + za];
        a.needs_sync = true;
    }

    /// Player knockback: server velocity, sent back to the client.
    pub(crate) fn player_knockback(&mut self, power: f64, xd: f64, zd: f64) {
        if self.push_player_velocity(power, xd, zd) {
            self.send_player_velocity();
        }
    }

    /// The player's server velocity sent to its client.
    pub(crate) fn send_player_velocity(&self) {
        let velocity = super::track::lp_quantize(self.mobs.player_vel);
        let _ = self.send(Event::Entity(EntityEvent::Motion { id: self.player_id, velocity }));
    }

    /// Player knockback on the server velocity only, not sent; false when the knockback
    /// resistance cancels it.
    pub(crate) fn push_player_velocity(&mut self, power: f64, mut xd: f64, mut zd: f64) -> bool {
        // W8a: knockback resistance (netherite, `/attribute`) like a mob's knockback.
        let power = power * (1.0 - self.player_attribute("minecraft:knockback_resistance"));
        if power <= 0.0 {
            return false;
        }
        while xd * xd + zd * zd < f64::from(1.0e-5f32) {
            xd = (self.mobs.rng.next_double() - self.mobs.rng.next_double()) * 0.01;
            zd = (self.mobs.rng.next_double() - self.mobs.rng.next_double()) * 0.01;
        }
        let l = (xd * xd + zd * zd).sqrt();
        let v = self.mobs.player_vel;
        let nv = [
            v[0] / 2.0 - xd / l * power,
            if self.on_ground { (v[1] / 2.0 + power).min(0.4) } else { v[1] },
            v[2] / 2.0 - zd / l * power,
        ];
        self.mobs.player_vel = nv;
        true
    }

    /// Value of a player attribute with the modifiers of the held item.
    fn player_attr(&self, name: &str) -> f64 {
        // Full value (W8a, `attrs.rs`): base, `/attribute`, effects and modifiers of
        // all the tracked equipment (modified stack included), capped like the game.
        self.player_attribute(name)
    }

    /// Weapon of a player hit: the item of the spin attack in progress, otherwise the held one.
    pub(crate) fn player_weapon(&self) -> Option<Stack> {
        if self.proj_player.spin > 0
            && let Some(s) = &self.proj_player.spin_item
        {
            return Some(s.clone());
        }
        self.slots[36 + self.held as usize].clone()
    }

    /// Base damage of a player hit: that of the spin attack in progress, otherwise the attribute.
    pub(crate) fn player_base_damage(&self) -> f32 {
        if self.proj_player.spin > 0 {
            return self.proj_player.spin_damage;
        }
        self.player_attr("minecraft:attack_damage") as f32
    }

    /// Player hit on a mob.
    pub(crate) fn player_attack(&mut self, id: i32) {
        if self.game_mode == 3 || self.life.vit.dead {
            return;
        }
        let Some(i) = self.mob_index(id) else {
            // Local network: another player.
            if let Some(p) = self.lan_player(id) {
                self.player_attack_player(id, p);
            }
            return;
        };
        let mut m = self.mobs.list.take(i).expect("mob");
        let reach_ok = {
            let b = m.bbox();
            let eye = [self.pos[0], self.player_eye_y(), self.pos[2]];
            let c =
                [eye[0].clamp(b.min[0], b.max[0]), eye[1].clamp(b.min[1], b.max[1]), eye[2].clamp(b.min[2], b.max[2])];
            super::mth::dist2(eye, c) < (3.0 + 3.0) * (3.0 + 3.0)
        };
        if reach_ok && m.alive() {
            self.player_attack_mob(&mut m);
        }
        self.mobs.list.put(i, m);
    }

    pub(crate) fn player_attack_mob(&mut self, m: &mut Mob) {
        let weapon = self.player_weapon();
        let mut base = self.player_base_damage();
        let speed = self.player_attr("minecraft:attack_speed");
        let delay = (1.0 / speed * 20.0) as f32;
        let scale = ((self.mobs.attack_ticker as f32 + 0.5) / delay).clamp(0.0, 1.0);
        let ench = weapon.as_ref().map_or(0.0, |w| self.enchanted_bonus(w, m));
        let magic = scale * ench;
        base *= 0.2 + scale * scale * 0.8;
        self.mobs.attack_ticker = 0;
        if base <= 0.0 && magic <= 0.0 {
            return;
        }
        let full = scale > 0.9;
        let sprint_kb = self.life.vit.sprinting && full;
        if sprint_kb {
            self.world_sound("minecraft:entity.player.attack.knockback", 7, self.pos, 1.0, 1.0);
        }
        // G5-F04: damage source of the weapon (its type, the mace's smash) and the mace's bonus
        // by fallen block, added after the charge scaling, before the critical hit.
        let fall = self.life.vit.fall;
        let smash = weapon.as_ref().is_some_and(|w| w.id == crate::mace::MACE)
            && crate::mace::can_smash(fall, self.proj_player.fall_flying);
        let ty = self.weapon_damage_type(weapon.as_ref(), smash, "minecraft:player_attack");
        let ty = ty.as_str();
        if smash {
            let mut own = m.rng;
            let bonus = {
                let ctx = self.damage_ctx(
                    ty,
                    crate::progress::pred::Who::Mob(m),
                    m.pos(),
                    Some(crate::progress::pred::Who::E(crate::commands::ents::E::Player)),
                    true,
                );
                self.mace_bonus(weapon.as_ref(), fall, true, &ctx, &mut own)
            };
            m.rng = own;
            base += bonus;
        }
        let crit = full
            && self.life.vit.fall > 0.0
            && !self.on_ground
            && !self.life.vit.sprinting
            && !self.phys.climbable(self.st(super::mth::block_of(self.pos)));
        if crit {
            base *= 1.5;
        }
        let total = base + magic;
        let sweep = self.sweep_allowed(full, crit, sprint_kb, weapon.as_ref());
        let old_h = m.health;
        let hurt = self.mob_hurt(m, ty, total, Some(Ent::Player), Some(self.pos));
        if hurt {
            let kb_ench = weapon.as_ref().map_or(0, |w| self.life.items.enchantment(w, "minecraft:knockback")) as f32;
            let kb = kb_ench / 2.0 + if sprint_kb { 0.5 } else { 0.0 };
            if kb > 0.0 {
                let (s, c) = (sin(self.yaw * DEG), cos(self.yaw * DEG));
                self.mob_knockback(m, f64::from(kb), f64::from(s), f64::from(-c), Some(Ent::Player), total, true);
                self.life.vit.sprinting = false;
            }
            if sweep {
                self.world_sound("minecraft:entity.player.attack.sweep", 7, self.pos, 1.0, 1.0);
                self.sweep_attack(&m.bbox(), Some(m.id), base, scale, ty, weapon.as_ref());
            }
            if crit {
                self.world_sound("minecraft:entity.player.attack.crit", 7, self.pos, 1.0, 1.0);
                let _ = self.send(Event::Entity(EntityEvent::Animate { id: m.id, action: 1 }));
            }
            if !crit && !sweep {
                let s =
                    if full { "minecraft:entity.player.attack.strong" } else { "minecraft:entity.player.attack.weak" };
                self.world_sound(s, 7, self.pos, 1.0, 1.0);
            }
            if magic > 0.0 {
                let _ = self.send(Event::Entity(EntityEvent::Animate { id: m.id, action: 2 }));
            }
            // Hit dealt to a dragon part: non-living target, the memory is cleared.
            self.set_last_hurt_mob(Some(Ent::Player), (!self.mobs.end.part_attack).then_some(Ent::Mob(m.id)));
            // G5-F04: the weapon's own effect (mace smash), the after-hit effects (the victim's
            // equipment: thorns; then the weapon's: fire aspect, bane, wind burst), then the
            // smash's fall reset.
            if smash {
                let st = crate::mace::Struck::mob(self, m);
                self.player_smash(&st);
            }
            {
                let mut hit = crate::ench_fx::Hit {
                    ty,
                    victim: crate::ench_fx::Side::Mob(m),
                    attacker: Some(crate::ench_fx::Side::Player),
                    direct: true,
                };
                self.post_attack(&mut hit, weapon.as_ref());
            }
            if smash && crate::mace::can_smash(self.life.vit.fall, self.proj_player.fall_flying) {
                self.life.vit.fall = 0.0;
            }
            // Weapon wear (`weapon` component).
            if let Some(w) = &weapon {
                let per = self.weapon_wear(w);
                if per > 0 {
                    self.hurt_item(36 + self.held as usize, per);
                }
            }
            // A dragon part is not a living entity: no damage statistic nor damage particles.
            let dealt = if self.mobs.end.part_attack { 0.0 } else { old_h - m.health };
            self.s9_player_hit(dealt, weapon.as_ref()); // S9
            if dealt > 2.0 {
                let n = (dealt * 0.5) as i32;
                self.particles(
                    "minecraft:damage_indicator",
                    [m.pos()[0], m.pos()[1] + f64::from(m.height()) * 0.5, m.pos()[2]],
                    [0.1, 0.0, 0.1],
                    0.2,
                    n,
                );
            }
            let invul = self.abilities().invulnerable;
            self.life.vit.exhaust(0.1, invul);
        } else {
            self.world_sound("minecraft:entity.player.attack.nodamage", 7, self.pos, 1.0, 1.0);
        }
    }

    /// Hit from the current player on another local-network player (`who`: their seat, then their
    /// position, the box of their pose, dead, mode): same computation as on a mob (charge, crit,
    /// sharpness, sprint knockback, sweep), damage, knockback and fire applied on the victim's side;
    /// refused (hit without damage) if the `pvp` rule or the victim's team forbids it.
    fn player_attack_player(&mut self, id: i32, target: crate::lan::OtherPlayer) {
        let (who, pos, b, dead, mode) = target;
        if dead || mode == 3 {
            return;
        }
        let eye = [self.pos[0], self.player_eye_y(), self.pos[2]];
        let c = [eye[0].clamp(b.min[0], b.max[0]), eye[1].clamp(b.min[1], b.max[1]), eye[2].clamp(b.min[2], b.max[2])];
        if super::mth::dist2(eye, c) >= (3.0 + 3.0) * (3.0 + 3.0) {
            return;
        }
        let weapon = self.player_weapon();
        let mut base = self.player_base_damage();
        let speed = self.player_attr("minecraft:attack_speed");
        let delay = (1.0 / speed * 20.0) as f32;
        let scale = ((self.mobs.attack_ticker as f32 + 0.5) / delay).clamp(0.0, 1.0);
        // A player is neither undead, nor arthropod, nor aquatic: sharpness only.
        let magic = scale * weapon.as_ref().map_or(0.0, |w| self.sharpness_bonus(w));
        base *= 0.2 + scale * scale * 0.8;
        self.mobs.attack_ticker = 0;
        if base <= 0.0 && magic <= 0.0 {
            return;
        }
        let full = scale > 0.9;
        let sprint_kb = self.life.vit.sprinting && full;
        if sprint_kb {
            self.world_sound("minecraft:entity.player.attack.knockback", 7, self.pos, 1.0, 1.0);
        }
        // G5-F04: weapon damage source and mace bonus, as on a mob.
        let fall = self.life.vit.fall;
        let smash = weapon.as_ref().is_some_and(|w| w.id == crate::mace::MACE)
            && crate::mace::can_smash(fall, self.proj_player.fall_flying);
        let ty = self.weapon_damage_type(weapon.as_ref(), smash, "minecraft:player_attack");
        if smash {
            let me = crate::progress::pred::Who::E(crate::commands::ents::E::Player);
            let mut own = self.life.rng;
            let ctx = self.damage_ctx(&ty, me, pos, Some(me), true);
            base += self.mace_bonus(weapon.as_ref(), fall, true, &ctx, &mut own);
        }
        let crit = full
            && self.life.vit.fall > 0.0
            && !self.on_ground
            && !self.life.vit.sprinting
            && !self.phys.climbable(self.st(super::mth::block_of(self.pos)));
        if crit {
            base *= 1.5;
        }
        let total = base + magic;
        let sweep = self.sweep_allowed(full, crit, sprint_kb, weapon.as_ref());
        let kb_ench = weapon.as_ref().map_or(0, |w| self.life.items.enchantment(w, "minecraft:knockback")) as f32;
        let hit = self.pvp_hit(&ty, weapon.as_ref(), total, kb_ench / 2.0 + if sprint_kb { 0.5 } else { 0.0 });
        let (parts, landed) = self.as_player(who, |sv| {
            let (parts, health) = sv.pvp_receive(&hit);
            (parts, health.map(|(o, n)| (o, n, sv.life.vit.dead.then(|| sv.name.clone()))))
        });
        // The victim's hurt went past its invulnerability window (even fully parried).
        if let Some((original, taken, blocked)) = parts {
            self.s9_player_hurt_player(&ty, original, taken, blocked);
        }
        let Some((old_h, new_h, killed)) = landed else {
            self.world_sound("minecraft:entity.player.attack.nodamage", 7, self.pos, 1.0, 1.0);
            return;
        };
        if hit.knockback > 0.0 {
            self.life.vit.sprinting = false;
        }
        if sweep {
            self.world_sound("minecraft:entity.player.attack.sweep", 7, self.pos, 1.0, 1.0);
            self.sweep_attack(&b, Some(id), base, scale, &ty, weapon.as_ref());
        }
        if crit {
            self.world_sound("minecraft:entity.player.attack.crit", 7, self.pos, 1.0, 1.0);
            let _ = self.send(Event::Entity(EntityEvent::Animate { id, action: 1 }));
        }
        if !crit && !sweep {
            let s = if full { "minecraft:entity.player.attack.strong" } else { "minecraft:entity.player.attack.weak" };
            self.world_sound(s, 7, self.pos, 1.0, 1.0);
        }
        if magic > 0.0 {
            let _ = self.send(Event::Entity(EntityEvent::Animate { id, action: 2 }));
        }
        // G5-F04: the mace's smash on the other player (on the ground or not), the weapon's
        // after-hit effects (Wind Burst; the victim's equipment is not read), then the fall reset.
        if smash {
            let (on_ground, uuid) = self.as_player(who, |sv| (sv.on_ground, sv.uuid));
            let st = crate::mace::Struck {
                ent: Ent::Mob(id),
                uuid,
                pos,
                bbox: b,
                on_ground,
                on_pos: self.on_pos(pos, None, 1.0e-5),
            };
            self.player_smash(&st);
        }
        self.player_weapon_post_attack(crate::commands::ents::E::Other(id), pos, &ty, weapon.as_ref());
        if smash && crate::mace::can_smash(self.life.vit.fall, self.proj_player.fall_flying) {
            self.life.vit.fall = 0.0;
        }
        if let Some(w) = &weapon {
            let per = self.weapon_wear(w);
            if per > 0 {
                self.hurt_item(36 + self.held as usize, per);
            }
        }
        let dealt = old_h - new_h;
        // Like on a mob: damage dealt, weapon used; then the kill credit if the victim dies of it.
        self.s9_player_hit(dealt, weapon.as_ref());
        if let Some(victim) = killed {
            self.s9_player_killed_player(&victim, &ty);
        }
        if dealt > 2.0 {
            // Half the height of the victim's box (its pose).
            let h = b.max[1] - b.min[1];
            self.particles("minecraft:damage_indicator", [pos[0], pos[1] + h * 0.5, pos[2]], [0.1, 0.0, 0.1], 0.2, (dealt * 0.5) as i32);
        }
        let invul = self.abilities().invulnerable;
        self.life.vit.exhaust(0.1, invul);
    }

    /// Hit from another player received by the current player: `pvp` rule and team (allies without
    /// friendly fire: refused), then "player hit" damage (shield, armor…), base knockback
    /// from the attacker, extra knockback in their gaze, fire. Returns the hurt report (damage
    /// received, after the shield, parried) if the hit went past the invulnerability window, and
    /// the health before and after the hit if it landed.
    fn pvp_receive(&mut self, hit: &PvpHit) -> (Option<HurtParts>, Option<(f32, f32)>) {
        if self.refuses_player_damage(&hit.name) {
            return (None, None);
        }
        let ty = hit.ty.as_str();
        let full = self.life.vit.cooldown <= 10;
        let old = self.life.vit.health;
        let (parts, ok) = self.pvp_hurt(hit);
        if !ok {
            return (parts, None);
        }
        if full && !self.dmg_tag(ty, "minecraft:no_knockback") {
            self.player_knockback(0.4, hit.pos[0] - self.pos[0], hit.pos[2] - self.pos[2]);
        }
        if hit.knockback > 0.0 {
            let (s, c) = (sin(hit.yaw * DEG), cos(hit.yaw * DEG));
            self.player_knockback(f64::from(hit.knockback), f64::from(s), f64::from(-c));
        }
        if hit.fire > 0 && self.life.vit.fire < hit.fire {
            self.life.vit.fire = hit.fire;
        }
        (parts, Some((old, self.life.vit.health)))
    }

    /// The hurt of a hit from another player received by the current player: "player hit" damage
    /// (shield, armor…) with its author known to the death message and the advancements. Returns
    /// the hurt report and whether the hit landed.
    fn pvp_hurt(&mut self, hit: &PvpHit) -> (Option<HurtParts>, bool) {
        self.prog.dmg_src = crate::item_use::DmgSrc {
            pos: Some(hit.pos),
            direct_living: true,
            piercing_arrow: false,
            disable_secs: hit.disable_secs,
            attacker_pos: Some(hit.pos),
        };
        self.prog.attacker_player = Some((hit.display.clone(), hit.named_item.clone()));
        self.prog.attacker_weapon = hit.weapon.clone();
        self.prog.hurt_parts = None;
        let ok = self.hurt(&hit.ty, hit.total);
        let parts = self.prog.hurt_parts.take();
        self.prog.attacker_player = None;
        self.prog.attacker_weapon = None;
        (parts, ok)
    }

    /// Hit from the current player on another player: `total` damage of type `ty` with `weapon`,
    /// `knockback` along the attacker's gaze, the weapon's fire and shield disabling.
    fn pvp_hit(&self, ty: &str, weapon: Option<&Stack>, total: f32, knockback: f32) -> PvpHit {
        PvpHit {
            ty: ty.to_owned(),
            weapon: weapon.cloned(),
            name: self.name.clone(),
            display: self.player_display_name(),
            named_item: weapon
                .filter(|s| crate::items::component(s, "minecraft:custom_name").is_some())
                .map(|s| self.stack_display(s)),
            pos: self.pos,
            yaw: self.yaw,
            total,
            knockback,
            fire: weapon.map_or(0, |w| 80 * self.life.items.enchantment(w, "minecraft:fire_aspect")),
            disable_secs: weapon
                .and_then(|w| self.comp_json(w, "minecraft:weapon"))
                .and_then(|w| w["disable_blocking_for_seconds"].as_f64())
                .unwrap_or(0.0) as f32,
        }
    }

    /// Stab (or spear charge) of the weapon of `hand` from the current player on another
    /// local-network player (entity `id`), refused like a hit by the `pvp` rule or the team: the
    /// damage, the two knockbacks and the dismount applied on the victim's side; then here, if any
    /// of them happened, the enchantment spark, the weapon's after-hit effects of a hurt, its wear,
    /// the statistics, the damage particles and the exhaustion.
    pub(crate) fn player_stab_other_player(&mut self, id: i32, hand: i32, fx: crate::player_spear::StabEffects) -> bool {
        let Some((who, pos, b, dead, mode)) = self.lan_player(id) else { return false };
        if dead || mode == 3 {
            return false;
        }
        let slot = self.hand_slot(hand);
        let weapon = self.slots[slot].clone();
        // A player is neither undead, nor arthropod, nor aquatic: sharpness only, read on the main hand.
        let bonus = self.slots[36 + self.held as usize].as_ref().map_or(0.0, |w| self.sharpness_bonus(w));
        let (base, magic) = self.stab_damage(hand, fx.damage, bonus);
        let ty = self.weapon_damage_type(weapon.as_ref(), false, "minecraft:player_attack");
        let total = if fx.deals_damage { base + magic } else { 0.0 };
        let hit = self.pvp_hit(&ty, weapon.as_ref(), total, self.stab_weapon_knockback());
        let (parts, received) = self.as_player(who, |sv| sv.pvp_receive_stab(&hit, fx));
        if let Some((original, taken, blocked)) = parts {
            self.s9_player_hurt_player(&ty, original, taken, blocked);
        }
        let Some(r) = received else { return false };
        if fx.deals_knockback {
            self.life.vit.sprinting = false;
        }
        if !r.hurt && !fx.deals_knockback && !r.dismounted {
            return false;
        }
        if magic > 0.0 {
            let _ = self.send(Event::Entity(EntityEvent::Animate { id, action: 2 }));
        }
        if r.hurt {
            self.player_weapon_post_attack(crate::commands::ents::E::Other(id), pos, &ty, weapon.as_ref());
        }
        self.stab_weapon_wear(slot, weapon.as_ref());
        let dealt = r.health_before - r.health_after;
        self.s9_player_hit(dealt, weapon.as_ref());
        if let Some(victim) = r.killed {
            self.s9_player_killed_player(&victim, &ty);
        }
        if dealt > 2.0 {
            let h = b.max[1] - b.min[1];
            self.particles("minecraft:damage_indicator", [pos[0], pos[1] + h * 0.5, pos[2]], [0.1, 0.0, 0.1], 0.2, (dealt * 0.5) as i32);
        }
        let invul = self.abilities().invulnerable;
        self.life.vit.exhaust(0.1, invul);
        true
    }

    /// A stab from another player received by the current player: its damage (with the default
    /// knockback of a hurt unless its type has none), then its two knockbacks along the attacker's
    /// yaw (0.4, then the weapon's) whether it hurt or not, then the dismount from a mob vehicle,
    /// then the fire of a hurt. `None`: refused (`pvp`, team).
    fn pvp_receive_stab(
        &mut self,
        hit: &PvpHit,
        fx: crate::player_spear::StabEffects,
    ) -> (Option<HurtParts>, Option<StabReceived>) {
        if self.refuses_player_damage(&hit.name) {
            return (None, None);
        }
        let health_before = self.life.vit.health;
        let velocity_before = self.mobs.player_vel;
        let full = self.life.vit.cooldown <= 10;
        let (parts, hurt) = if fx.deals_damage { self.pvp_hurt(hit) } else { (None, false) };
        let ty = hit.ty.as_str();
        // A full hurt marks the velocity to send (drowning aside); the game sends it at the first
        // knockback below and then puts back the velocity from before the stab.
        let mut velocity_to_send = hurt && full && !self.dmg_tag(ty, "minecraft:no_impact");
        let default_knockback = hurt && full && !self.dmg_tag(ty, "minecraft:no_knockback");
        if default_knockback {
            self.push_player_velocity(0.4, hit.pos[0] - self.pos[0], hit.pos[2] - self.pos[2]);
        }
        if fx.deals_knockback {
            let (s, c) = (f64::from(sin(hit.yaw * DEG)), f64::from(cos(hit.yaw * DEG)));
            for power in [0.4, hit.knockback] {
                if power > 0.0 {
                    self.push_player_velocity(f64::from(power), s, -c);
                }
                // Only the first knockback reaches the client; the second one stays on the server.
                if velocity_to_send {
                    self.send_player_velocity();
                    self.mobs.player_vel = velocity_before;
                    velocity_to_send = false;
                }
            }
        }
        // Without a stab knockback, the hurt's own velocity (the default knockback) goes out
        // like after any hurt that pushes.
        if velocity_to_send && default_knockback {
            self.send_player_velocity();
        }
        let dismounted = fx.dismounts && self.mobs.mounts.riding.vehicle.is_some();
        if dismounted {
            self.player_stop_riding();
        }
        if hurt && hit.fire > 0 && self.life.vit.fire < hit.fire {
            self.life.vit.fire = hit.fire;
        }
        let killed = (hurt && self.life.vit.dead).then(|| self.name.clone());
        let received = StabReceived { hurt, dismounted, health_before, health_after: self.life.vit.health, killed };
        (parts, Some(received))
    }

    /// The current player refuses any damage whose author is the player named `attacker` (itself
    /// included): `pvp` rule off, or a common team without friendly fire.
    pub(crate) fn refuses_player_damage(&self, attacker: &str) -> bool {
        self.player_refuses_damage_from(&self.name, attacker)
    }

    /// Whether player `victim` refuses the damage of player `attacker` (`pvp` rule off, or a
    /// common team without friendly fire), read without installing either.
    pub(crate) fn player_refuses_damage_from(&self, victim: &str, attacker: &str) -> bool {
        let team_spares =
            |t: &crate::commands::scores::Team| !t.friendly_fire && t.players.iter().any(|p| p == attacker);
        !self.rule("minecraft:pvp", true) || self.cmd.scores.team_of(victim).is_some_and(team_spares)
    }

    /// G5-F04: second hurt sound of a full hit of thorns, at the hurt entity, category players (7)
    /// for the player and hostile (5) for any mob, whatever it is.
    pub(crate) fn thorns_hit_sound(&mut self, ty: &str, category: i32, pos: [f64; 3]) {
        if ty == "minecraft:thorns" {
            self.world_sound("minecraft:enchant.thorns.hit", category, pos, 1.0, 1.0);
        }
    }

    /// G5-F04: wear of a hit with a weapon: its `weapon` component (the stack's or the item's),
    /// 1 point when the field is left out (the default value, omitted by the extracted data); no
    /// component: none.
    pub(crate) fn weapon_wear(&self, w: &Stack) -> i32 {
        self.comp_json(w, "minecraft:weapon").map_or(0, |v| v["item_damage_per_attack"].as_i64().unwrap_or(1) as i32)
    }

    /// Sharpness bonus of the weapon (any target).
    pub(crate) fn sharpness_bonus(&self, w: &Stack) -> f32 {
        let sharp = self.life.items.enchantment(w, "minecraft:sharpness") as f32;
        if sharp > 0.0 { 0.5 * sharp + 0.5 } else { 0.0 }
    }

    /// Enchantment bonus of the weapon against a target (sharpness, smite, bane).
    pub(crate) fn enchanted_bonus(&self, w: &Stack, m: &Mob) -> f32 {
        self.sharpness_bonus(w) + self.kind_ench_bonus(w, m.kind)
    }

    /// Enchantment bonus filtered by victim type: smite (`#sensitive_to_smite`),
    /// bane (`#sensitive_to_bane_of_arthropods`), impaling (`#sensitive_to_impaling`, in
    /// melee too).
    pub(crate) fn kind_ench_bonus(&self, w: &Stack, k: Kind) -> f32 {
        let lvl = |e: &str| self.life.items.enchantment(w, e) as f32;
        let mut b = 0.0;
        if k.undead() {
            b += 2.5 * lvl("minecraft:smite");
        }
        if k.arthropod() {
            b += 2.5 * lvl("minecraft:bane_of_arthropods");
        }
        if k.aquatic() {
            b += 2.5 * lvl("minecraft:impaling");
        }
        b
    }

    /// Sweep hit allowed: full charge, neither crit nor sprint knockback, on the ground, movement
    /// known to the player under (speed × 2.5)², sword in hand.
    pub(crate) fn sweep_allowed(&self, full: bool, crit: bool, sprint_kb: bool, weapon: Option<&crate::player::Stack>) -> bool {
        if !full || crit || sprint_kb || !self.on_ground {
            return false;
        }
        let k = self.proj_player.known_move;
        let max = f64::from(self.player_attribute("minecraft:movement_speed") as f32) * 2.5;
        k[0] * k[0] + k[2] * k[2] < max * max && weapon.is_some_and(|w| self.item_tag(&w.id, "minecraft:swords"))
    }

    /// Sweep around the box `tb` of the struck target (mob or other player `target`, or
    /// non-living entity such as a minecart): every living entity within 3 blocks of the player
    /// that is not its ally (mobs, armor stands but markers, mannequins, the other players),
    /// damage (with the weapon's enchantment bonus against each of them), then, if it landed,
    /// knockback along the player's yaw and after-hit effects.
    pub(crate) fn sweep_attack(
        &mut self,
        tb: &rc_physics::Aabb,
        target: Option<i32>,
        base: f32,
        scale: f32,
        ty: &str,
        weapon: Option<&Stack>,
    ) {
        // W8a: ratio read from the attribute (Sweeping Edge via the tracked equipment, `/attribute`).
        let ratio = self.player_attribute("minecraft:sweeping_damage_ratio") as f32;
        let sweep = 1.0 + ratio * base;
        let area = rc_physics::Aabb {
            min: [tb.min[0] - 1.0, tb.min[1] - 0.25, tb.min[2] - 1.0],
            max: [tb.max[0] + 1.0, tb.max[1] + 0.25, tb.max[2] + 1.0],
        };
        let me = self.name.clone();
        // Allies are spared: the player's team looked up once, each entity's team found by its
        // UUID (never formatted).
        let own_team = self.cmd.scores.team_of(&me).map(|t| t.name.clone());
        let ally = |s: &Self, uuid: u128| own_team.as_ref().is_some_and(|own| s.cmd.scores.uuid_in_team(uuid, own));
        let ids: Vec<i32> = self
            .mobs
            .list
            .iter()
            .filter(|o| Some(o.id) != target && o.touches(&area) && super::mth::dist2(o.pos(), self.pos) < 9.0)
            .filter(|o| !ally(self, o.uuid))
            .map(|o| o.id)
            .collect();
        let (s, c) = (sin(self.yaw * DEG), cos(self.yaw * DEG));
        for id in ids {
            let Some(i) = self.mob_index(id) else { continue };
            let mut o = self.mobs.list.take(i).expect("mob");
            let dmg = (sweep + weapon.map_or(0.0, |w| self.enchanted_bonus(w, &o))) * scale;
            if self.mob_hurt(&mut o, ty, dmg, Some(Ent::Player), Some(self.pos)) {
                self.mob_knockback(&mut o, 0.4, f64::from(s), f64::from(-c), Some(Ent::Player), dmg, false);
                let mut hit = crate::ench_fx::Hit {
                    ty,
                    victim: crate::ench_fx::Side::Mob(o.as_mut()),
                    attacker: Some(crate::ench_fx::Side::Player),
                    direct: true,
                };
                self.post_attack(&mut hit, weapon);
            }
            self.mobs.list.put(i, o);
        }
        // Armor stands (markers left out) and mannequins: the stand takes the hit as a stand
        // (wobble or broken), the mannequin as a living entity.
        let bonus = weapon.map_or(0.0, |w| self.sharpness_bonus(w));
        for d in self.deco_living() {
            if d.marker || !d.bbox.intersects(&area) || super::mth::dist2(d.pos, self.pos) >= 9.0 || ally(self, d.uuid) {
                continue;
            }
            let Some(e) = self.deco_nbt(d.id).cloned() else { continue };
            let dmg = (sweep + bonus) * scale;
            let landed = if crate::decoration::kind_of(&e) == Some(crate::decoration::DKind::Stand) {
                self.stand_hurt(d.id, &e, ty)
            } else {
                self.mannequin_hurt(d.id, ty, dmg, Some(Ent::Player), Some(self.pos))
            };
            if landed && self.deco_nbt(d.id).is_some() {
                self.deco_knockback(d.id, 0.4, f64::from(s), f64::from(-c));
                self.player_weapon_post_attack(crate::commands::ents::E::Other(d.id), d.pos, ty, weapon);
            }
        }
        // The other players of the local network (the struck one left out).
        for o in self.lan_peers() {
            if Some(o.id) == target || o.game_mode == 3 || !o.bbox.intersects(&area) || super::mth::dist2(o.pos, self.pos) >= 9.0 {
                continue;
            }
            let name = self.as_player(o.who, |sv| sv.name.clone());
            if self.same_team(&me, &name) {
                continue;
            }
            let hit = self.pvp_hit(ty, weapon, (sweep + bonus) * scale, 0.4);
            let (parts, landed) = self.as_player(o.who, |sv| sv.pvp_receive(&hit));
            if let Some((original, taken, blocked)) = parts {
                self.s9_player_hurt_player(ty, original, taken, blocked);
            }
            if landed.is_some() {
                self.player_weapon_post_attack(crate::commands::ents::E::Other(o.id), o.pos, ty, weapon);
            }
        }
        let (dx, dz) = (-f64::from(s), f64::from(c));
        self.particles(
            "minecraft:sweep_attack",
            [self.pos[0] + dx, self.pos[1] + 0.9, self.pos[2] + dz],
            [dx as f32, 0.0, dz as f32],
            0.0,
            0,
        );
    }
}

/// Random of a mob target of the list (a fresh one for the player or a missing mob: unused then).
fn t_rng(s: &Server, t: Ent) -> super::jrand::JRand {
    match t {
        Ent::Mob(id) => s.mob_ref(id).map_or_else(|| super::jrand::JRand::new(0), |o| o.rng),
        Ent::Player => super::jrand::JRand::new(0),
    }
}

/// Hurt report of a player hit by another: damage received, after the shield, parried.
pub(crate) type HurtParts = (f32, f32, bool);

/// Hit from one player on another, computed on the attacker's side, applied on the victim's side.
struct PvpHit {
    /// G5-F04: damage type of the weapon (mace smash…) and the weapon (armor effectiveness).
    ty: String,
    weapon: Option<Stack>,
    /// Attacker's name (team) and displayed name (death messages).
    name: String,
    display: rc_text::Component,
    named_item: Option<rc_text::Component>,
    pos: [f64; 3],
    yaw: f32,
    total: f32,
    /// Extra knockback (enchantment, sprint).
    knockback: f32,
    /// Fire ticks (fire aspect).
    fire: i32,
    /// Shield disabled (seconds, `weapon` component of the weapon).
    disable_secs: f32,
}

/// What a stab did to the player who received it (`pvp_receive_stab`).
struct StabReceived {
    hurt: bool,
    dismounted: bool,
    health_before: f32,
    health_after: f32,
    /// Name of the victim killed by the stab.
    killed: Option<String>,
}

/// Dye colors in id order.
pub const DYES: [&str; 16] = [
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "light_gray",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
];
