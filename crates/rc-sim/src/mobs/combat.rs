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

impl Server {
    /// Damage type in a tag (`tags/damage_type`).
    pub(crate) fn dmg_tag(&self, ty: &str, tag: &str) -> bool {
        let Some(idx) = self.regs.entries("minecraft:damage_type").iter().position(|e| e == ty) else { return false };
        self.regs
            .tags
            .iter()
            .find(|(r, _)| r == "minecraft:damage_type")
            .and_then(|(_, t)| t.iter().find(|(n, _)| n == tag))
            .is_some_and(|(_, ids)| ids.contains(&(idx as i32)))
    }

    pub(crate) fn sound_id(&self, name: &str) -> Option<i32> {
        self.data
            .registry("minecraft:sound_event")
            .and_then(|r| r.entries.iter().position(|e| e == name))
            .map(|i| i as i32)
    }

    /// Sound played by a mob (seed drawn from the world's random source): nothing, and no seed drawn, if
    /// it is silent (`Silent`); the callers' pitch draws are still made.
    pub(crate) fn mob_sound(&mut self, m: &Mob, name: &str, volume: f32, pitch: f32) {
        if m.x.silent {
            return;
        }
        self.mob_sound_unmuted(m, name, volume, pitch);
    }

    /// Sound played by the world at a mob's position: heard even if it is silent.
    pub(crate) fn mob_sound_unmuted(&mut self, m: &Mob, name: &str, volume: f32, pitch: f32) {
        let seed = self.mobs.rng.next_long();
        let Some(id) = self.sound_id(name) else { return };
        let category = Self::mob_sound_source(m);
        let _ = self.send(Event::Sound(SoundPacket {
            sound: SoundRef::Id(id),
            category,
            pos: Some(m.pos),
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
        self.game_event_src("minecraft:shear", m.pos, src, None);
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
        let seed = self.mobs.rng.next_long();
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
        let seed = self.mobs.rng.next_long();
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
            },
            force: false,
            always_visible: false,
            pos,
            delta,
            speed: [speed; 3],
            count,
            mode: 0,
        };
        let _ = self.send(Event::Particles(p));
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
            particle: Particle { kind: kind as i32, color, state: 0, item: -1, scale: 1.0, color2: 0, vib: None },
            force: false,
            always_visible: false,
            pos,
            delta,
            speed,
            count,
            mode,
        };
        let _ = self.send(Event::Particles(p));
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
        let r = self.mob_hurt(m, ty, amount, Some(Ent::Mob(by.id)), Some(by.pos));
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
        // `Invulnerable` flag: only `bypasses_invulnerability` and a creative player get through.
        if m.invulnerable
            && !self.dmg_tag(ty, "minecraft:bypasses_invulnerability")
            && !(by == Some(Ent::Player) && self.game_mode == 1)
        {
            return false;
        }
        // Tag `fall_damage_immune`: the game computes no fall damage, hence no hit
        // (nor species hook: the wither does not arm block destruction).
        if ty == "minecraft:fall" && m.kind.info().fall_immune {
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
        // `#freeze_hurts_extra_types`: freeze damage ×5 (strider, blaze, magma cube).
        if m.kind.freeze_hurts_extra() && self.dmg_tag(ty, "minecraft:is_freezing") {
            amount *= 5.0;
        }
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
        if by == Some(Ent::Player) {
            m.last_hurt_by_player = 100;
        }
        if full {
            let _ = self.send(Event::Entity(EntityEvent::Hurt { id: m.id, yaw: 0.0 }));
            if !self.dmg_tag(ty, "minecraft:no_impact") {
                m.track.sync_velocity = true;
            }
            if !self.dmg_tag(ty, "minecraft:no_knockback") {
                let (xd, zd) = hurt_knockback_dir(dir, src, m.pos);
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
        }
        m.last_hit = Some(Hit { ty: ty.to_owned(), by, tick: self.time as u64 });
        self.mob_hurt_effects(m); // infestation
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

    /// Enchantment protection of a mob's armor (feet, legs, chest, head, body).
    fn mob_protection(&self, m: &Mob, ty: &str) -> f32 {
        let mut p = 0.0;
        for slot in 2..7 {
            let Some(s) = &m.equipment[slot] else { continue };
            let lvl = |e: &str| self.life.items.enchantment(s, e) as f32;
            p += lvl("minecraft:protection");
            if self.dmg_tag(ty, "minecraft:is_fire") {
                p += 2.0 * lvl("minecraft:fire_protection");
            }
            if self.dmg_tag(ty, "minecraft:is_fall") {
                p += 3.0 * lvl("minecraft:feather_falling");
            }
            if self.dmg_tag(ty, "minecraft:is_explosion") {
                p += 2.0 * lvl("minecraft:blast_protection");
            }
            if self.dmg_tag(ty, "minecraft:is_projectile") {
                p += 2.0 * lvl("minecraft:projectile_protection");
            }
        }
        p
    }

    fn mob_actually_hurt(&mut self, m: &mut Mob, ty: &str, mut dmg: f32, by: Option<Ent>) {
        super::mounts::before_damage(self, m); // F04: the camel stands up
        // Wolf armor: it takes everything, health does not move.
        if self.wolf_armor_absorb(m, ty, dmg) {
            return;
        }
        if !self.dmg_tag(ty, "minecraft:bypasses_armor") {
            let armor = (m.attrs.armor.value() as f32).floor();
            if armor > 0.0 {
                let t = 2.0;
                let a = (armor - dmg / t).clamp(armor * 0.2, 20.0);
                dmg *= 1.0 - a / 25.0;
            }
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
                let p = self.mob_protection(m, ty);
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
                self.spawn_orbs(m.pos, xp);
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
        let pos = [m.pos[0], m.pos[1], m.pos[2]];
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
            origin: Some(m.pos),
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
                m.equipment[i] = None;
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
        // Modifiers of the held weapon (vindicator's axe, vex's sword…), then enchantments.
        let mut dmg = (m.attrs.attack.value() + super::illagers::weapon_attack_bonus(self, m)) as f32;
        // S7: weapon enchantments (sharpness, smite, bane; knockback, fire aspect).
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
        let fire_aspect = wl(self, "minecraft:fire_aspect") as i32;
        let ty = "minecraft:mob_attack";
        let hurt = match t {
            Ent::Player => {
                let kb = (m.pos[0] - self.pos[0], m.pos[2] - self.pos[2]);
                let r = self.player_hurt_by_mob(m, ty, dmg, Some(kb));
                self.take_riposte(m);
                r
            }
            Ent::Mob(id) => {
                let Some(i) = self.mob_index(id) else { return false };
                let mut o = self.mobs.list[i].take().expect("mob");
                // F01: attacker's random source lent to the reaction to a kill (villager conversion).
                self.mobs.killer = Some((m.rng, m.kind, m.id, m.block_pos()));
                let r = self.mob_hurt_by(&mut o, m, ty, dmg);
                self.aqua_flush_thorns(m); // F08: struck guardian's thorns
                if let Some((rng, _, _, _)) = self.mobs.killer.take() {
                    m.rng = rng;
                }
                self.mobs.list[i] = Some(o);
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
                            let mut o = self.mobs.list[i].take().expect("mob");
                            self.ghost_push(m);
                            let by = Some(Ent::Mob(m.id));
                            self.mob_knockback(&mut o, f64::from(kb), f64::from(s), f64::from(-c), by, dmg, true);
                            self.ghost_pop();
                            self.mobs.list[i] = Some(o);
                        }
                    }
                }
                m.vel[0] *= 0.6;
                m.vel[2] *= 0.6;
            }
            // S7: fire aspect (after-hit effect: 4 s per level, without shortening a fire in progress).
            if fire_aspect > 0 {
                let ticks = 80 * fire_aspect;
                match t {
                    Ent::Player => {
                        self.player_ignite(ticks);
                    }
                    Ent::Mob(id) => {
                        if let Some(i) = self.mob_index(id)
                            && let Some(o) = self.mobs.list[i].as_mut()
                            && o.fire < ticks
                        {
                            o.fire = ticks;
                        }
                    }
                }
            }
            // Slowness from Bane of Arthropods (direct hit).
            if let Ent::Mob(id) = t
                && let Some(i) = self.mob_index(id)
            {
                let mut o = self.mobs.list[i].take().expect("mob");
                self.bane_slowness(weapon.as_ref(), &mut o);
                self.mobs.list[i] = Some(o);
            }
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
        }
        self.wild_after_attack(m, hurt); // F11: panda bite sound
        hurt
    }

    /// Damage of a mob to the player: difficulty scaling, knockback, memory of the attacker.
    pub(crate) fn player_hurt_by_mob(&mut self, m: &Mob, ty: &str, amount: f32, kb: Option<(f64, f64)>) -> bool {
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
                return false;
            }
        }
        let full = self.life.vit.cooldown <= 10;
        // W8c: without a source set by the caller (projectile, explosion), the mob hits
        // itself: position, weapon that disables the shield.
        if self.prog.dmg_src.pos.is_none() {
            self.prog.dmg_src = self.melee_src(m, ty);
        }
        // Direct attacker, named in the death message ("killed by a husk").
        self.mobs.hit_by = Some(m.kind);
        self.prog.attacker = Some(Box::new(m.clone())); // S9: damage source
        let ok = self.hurt(ty, d);
        self.prog.attacker = None; // S9
        self.mobs.hit_by = None;
        if !ok {
            return false;
        }
        self.mobs.player_hurt_by = Some((Ent::Mob(m.id), self.mobs.player_ticks));
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
        let (mut xa, mut za) = (a.pos[0] - self.pos[0], a.pos[2] - self.pos[2]);
        let dd = xa.abs().max(za.abs());
        if dd < f64::from(0.01f32) {
            return;
        }
        let dd = dd.sqrt();
        xa /= dd;
        za /= dd;
        let pow = (1.0 / dd).min(1.0);
        let k = pow * f64::from(0.05f32);
        let (xa, za) = (xa * k, za * k);
        self.player_add_velocity([-xa, 0.0, -za]);
        a.vel = [a.vel[0] + xa, a.vel[1], a.vel[2] + za];
        a.needs_sync = true;
    }

    /// Player knockback: server velocity, sent back to the client.
    pub(crate) fn player_knockback(&mut self, power: f64, mut xd: f64, mut zd: f64) {
        // W8a: knockback resistance (netherite, `/attribute`) like a mob's knockback.
        let power = power * (1.0 - self.player_attribute("minecraft:knockback_resistance"));
        if power <= 0.0 {
            return;
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
        let _ = self
            .send(Event::Entity(EntityEvent::Motion { id: self.player_id, velocity: super::track::lp_quantize(nv) }));
    }

    /// Value of a player attribute with the modifiers of the held item.
    fn player_attr(&self, name: &str) -> f64 {
        // Full value (W8a, `attrs.rs`): base, `/attribute`, effects and modifiers of
        // all the tracked equipment (modified stack included), capped like the game.
        self.player_attribute(name)
    }

    fn player_weapon(&self) -> Option<Stack> {
        self.slots[36 + self.held as usize].clone()
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
        let mut m = self.mobs.list[i].take().expect("mob");
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
        self.mobs.list[i] = Some(m);
    }

    pub(crate) fn player_attack_mob(&mut self, m: &mut Mob) {
        let weapon = self.player_weapon();
        let mut base = self.player_attr("minecraft:attack_damage") as f32;
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
        let ty = "minecraft:player_attack";
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
                self.sweep_attack(&m.bbox(), Some(m.id), base, scale);
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
            // After-hit effects of the weapon: fire aspect (4 s per level, without shortening a fire
            // in progress), Bane of Arthropods slowness.
            if let Some(w) = &weapon {
                let ticks = 80 * self.life.items.enchantment(w, "minecraft:fire_aspect");
                if ticks > 0 && m.fire < ticks {
                    m.fire = ticks;
                }
            }
            self.bane_slowness(weapon.as_ref(), m);
            // Weapon wear (`weapon` component).
            if let Some(w) = &weapon {
                let per = self
                    .life
                    .items
                    .default_component(w, "minecraft:weapon")
                    .and_then(|v| v["item_damage_per_attack"].as_i64())
                    .unwrap_or(0) as i32;
                if per > 0 {
                    self.hurt_item(36 + self.held as usize, per);
                }
            }
            let dealt = old_h - m.health;
            self.s9_player_hit_mob(m, total, dealt, weapon.as_ref()); // S9
            if dealt > 2.0 {
                let n = (dealt * 0.5) as i32;
                self.particles(
                    "minecraft:damage_indicator",
                    [m.pos[0], m.pos[1] + f64::from(m.height()) * 0.5, m.pos[2]],
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
    /// position, sneaking, dead, mode): same computation as on a mob (charge, crit,
    /// sharpness, sprint knockback, sweep), damage, knockback and fire applied on the victim's side;
    /// refused (hit without damage) if the `pvp` rule or the victim's team forbids it.
    fn player_attack_player(&mut self, id: i32, target: crate::lan::OtherPlayer) {
        let (who, pos, sneak, dead, mode) = target;
        if dead || mode == 3 {
            return;
        }
        let b = crate::play::player_box(pos, sneak);
        let eye = [self.pos[0], self.player_eye_y(), self.pos[2]];
        let c = [eye[0].clamp(b.min[0], b.max[0]), eye[1].clamp(b.min[1], b.max[1]), eye[2].clamp(b.min[2], b.max[2])];
        if super::mth::dist2(eye, c) >= (3.0 + 3.0) * (3.0 + 3.0) {
            return;
        }
        let weapon = self.player_weapon();
        let mut base = self.player_attr("minecraft:attack_damage") as f32;
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
        let hit = PvpHit {
            name: self.name.clone(),
            display: self.player_display_name(),
            named_item: weapon
                .as_ref()
                .filter(|s| crate::items::component(s, "minecraft:custom_name").is_some())
                .map(|s| self.stack_display(s)),
            pos: self.pos,
            yaw: self.yaw,
            total,
            knockback: kb_ench / 2.0 + if sprint_kb { 0.5 } else { 0.0 },
            fire: weapon.as_ref().map_or(0, |w| 80 * self.life.items.enchantment(w, "minecraft:fire_aspect")),
            disable_secs: weapon
                .as_ref()
                .and_then(|w| self.comp_json(w, "minecraft:weapon"))
                .and_then(|w| w["disable_blocking_for_seconds"].as_f64())
                .unwrap_or(0.0) as f32,
        };
        let Some((old_h, new_h, killed)) = self.as_player(who, |sv| {
            sv.pvp_receive(&hit).map(|(o, n)| (o, n, sv.life.vit.dead.then(|| sv.name.clone())))
        }) else {
            self.world_sound("minecraft:entity.player.attack.nodamage", 7, self.pos, 1.0, 1.0);
            return;
        };
        if hit.knockback > 0.0 {
            self.life.vit.sprinting = false;
        }
        if sweep {
            self.world_sound("minecraft:entity.player.attack.sweep", 7, self.pos, 1.0, 1.0);
            self.sweep_attack(&b, None, base, scale);
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
        if let Some(w) = &weapon {
            let per = self
                .life
                .items
                .default_component(w, "minecraft:weapon")
                .and_then(|v| v["item_damage_per_attack"].as_i64())
                .unwrap_or(0) as i32;
            if per > 0 {
                self.hurt_item(36 + self.held as usize, per);
            }
        }
        let dealt = old_h - new_h;
        // Like on a mob: damage dealt, weapon used, `player_hurt_entity`; then the
        // kill credit if the victim dies of it.
        self.s9_player_hit(None, total, dealt, weapon.as_ref());
        if let Some(victim) = killed {
            self.s9_player_killed_player(&victim);
        }
        if dealt > 2.0 {
            let h = if sneak { 1.5 } else { 1.8 };
            self.particles("minecraft:damage_indicator", [pos[0], pos[1] + h * 0.5, pos[2]], [0.1, 0.0, 0.1], 0.2, (dealt * 0.5) as i32);
        }
        let invul = self.abilities().invulnerable;
        self.life.vit.exhaust(0.1, invul);
    }

    /// Hit from another player received by the current player: `pvp` rule and team (allies without
    /// friendly fire: refused), then "player hit" damage (shield, armor…), base knockback
    /// from the attacker, extra knockback in their gaze, fire. Returns the health before and
    /// after the hit if it landed.
    fn pvp_receive(&mut self, hit: &PvpHit) -> Option<(f32, f32)> {
        if !self.rule("minecraft:pvp", true) {
            return None;
        }
        if let Some(t) = self.cmd.scores.team_of(&self.name)
            && t.players.contains(&hit.name)
            && !t.friendly_fire
        {
            return None;
        }
        let ty = "minecraft:player_attack";
        let full = self.life.vit.cooldown <= 10;
        self.prog.dmg_src = crate::item_use::DmgSrc {
            pos: Some(hit.pos),
            direct_living: true,
            piercing_arrow: false,
            disable_secs: hit.disable_secs,
            attacker_pos: Some(hit.pos),
        };
        self.prog.attacker_player = Some((hit.display.clone(), hit.named_item.clone()));
        let old = self.life.vit.health;
        let ok = self.hurt(ty, hit.total);
        self.prog.attacker_player = None;
        if !ok {
            return None;
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
        Some((old, self.life.vit.health))
    }

    /// Sharpness bonus of the weapon (any target).
    fn sharpness_bonus(&self, w: &Stack) -> f32 {
        let sharp = self.life.items.enchantment(w, "minecraft:sharpness") as f32;
        if sharp > 0.0 { 0.5 * sharp + 0.5 } else { 0.0 }
    }

    /// Enchantment bonus of the weapon against a target (sharpness, smite, bane).
    fn enchanted_bonus(&self, w: &Stack, m: &Mob) -> f32 {
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

    /// After-hit effect of Bane of Arthropods (direct hit on an arthropod): slowness IV for a duration
    /// drawn between 1.5 s and 1.5 s + 0.5 s per level beyond the first (victim's random source:
    /// list element, duration, amplifier).
    pub(crate) fn bane_slowness(&self, w: Option<&Stack>, victim: &mut Mob) {
        let Some(w) = w else { return };
        let level = self.life.items.enchantment(w, "minecraft:bane_of_arthropods");
        if level <= 0 || !victim.kind.arthropod() {
            return;
        }
        let _ = victim.rng.next_int_bound(1);
        let max = 1.5 + 0.5 * (level - 1) as f32;
        let ticks = ((victim.rng.next_float() * (max - 1.5) + 1.5) * 20.0).round() as i32;
        // Amplifier drawn between 3 and 3: the draw still takes place.
        let _ = victim.rng.next_float();
        self.give_mob_effect(victim, "minecraft:slowness", ticks, 3);
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

    /// Sweep around the box `tb` of the struck target (mob `target`, or non-living entity
    /// such as a minecart): mobs within 3 blocks of the player, damage and knockback.
    pub(crate) fn sweep_attack(&mut self, tb: &rc_physics::Aabb, target: Option<i32>, base: f32, scale: f32) {
        // W8a: ratio read from the attribute (Sweeping Edge via the tracked equipment, `/attribute`).
        let ratio = self.player_attribute("minecraft:sweeping_damage_ratio") as f32;
        let dmg = (1.0 + ratio * base) * scale;
        let area = rc_physics::Aabb {
            min: [tb.min[0] - 1.0, tb.min[1] - 0.25, tb.min[2] - 1.0],
            max: [tb.max[0] + 1.0, tb.max[1] + 0.25, tb.max[2] + 1.0],
        };
        let ids: Vec<i32> = self
            .mobs
            .list
            .iter()
            .flatten()
            .filter(|o| Some(o.id) != target && o.touches(&area) && super::mth::dist2(o.pos, self.pos) < 9.0)
            .map(|o| o.id)
            .collect();
        let (s, c) = (sin(self.yaw * DEG), cos(self.yaw * DEG));
        for id in ids {
            let Some(i) = self.mob_index(id) else { continue };
            let mut o = self.mobs.list[i].take().expect("mob");
            if self.mob_hurt(&mut o, "minecraft:player_attack", dmg, Some(Ent::Player), Some(self.pos)) {
                self.mob_knockback(&mut o, 0.4, f64::from(s), f64::from(-c), Some(Ent::Player), dmg, false);
            }
            self.mobs.list[i] = Some(o);
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

/// Hit from one player on another, computed on the attacker's side, applied on the victim's side.
struct PvpHit {
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
