//! Commands on entities: `summon`, `effect`, `enchant`, `damage`, `rotate`, `ride`,
//! `attribute`, `execute on` links; dispatch of the remaining commands.

use rc_client::{EntityEvent, Event};
use rc_proto::nbt::Nbt;
use rc_text::Component;

use super::cmds_basic::Track;
use super::ents::{E, Effect};
use super::src::{Fail, Src};
use super::text::{jf, lit};
use super::{Parsed, R, Seg, args};
use crate::mobs::mob::Ent;
use crate::mobs::species::Kind;
use crate::server::Server;
use crate::tr;

/// Instant effects (duration in raw ticks).
fn instant(id: &str) -> bool {
    crate::effects::def(id).is_some_and(|d| d.instant)
}

impl Server {
    /// Commands not handled by `dispatch`.
    pub(crate) fn dispatch_more(&mut self, s: &Src, p: &Parsed, seg: &Seg, _path: &str) -> R {
        if let Some(r) = self.dispatch_s9(s, p, seg) {
            return r; // S9: commands added or rewritten
        }
        let head = seg.path.first().map(String::as_str).unwrap_or("");
        match head {
            "summon" => {
                let ty = args::id(&p.arg(seg, "entity").unwrap_or_default());
                let pos = p.arg(seg, "pos").map_or(s.pos, |t| args::vec3(self, s, &t));
                let nbt = match p.arg(seg, "nbt") {
                    Some(t) => Some(super::snbt::parse(&t).map_err(|_| Fail(tr!("argument.nbt.expected.compound")))?),
                    None => None,
                };
                let e = self.summon_entity(s, &ty, pos, nbt)?;
                self.ok(s, tr!("commands.summon.success", self.ent_display(e)));
                Ok(1)
            }
            "effect" => self.cmd_effect(s, p, seg),
            "enchant" => self.cmd_enchant(s, p, seg),
            "damage" => self.cmd_damage(s, p, seg),
            "rotate" => {
                let e = self.arg_entity(s, &p.arg(seg, "target").unwrap_or_default(), false)?;
                if let Some(r) = p.arg(seg, "rotation") {
                    // W11: rotation resolved against the source, rotation only (no teleport).
                    self.command_rotate(e, args::rotation(s, &r), args::rotation_relative(&r));
                } else if let Some(fe) = p.arg(seg, "facingEntity") {
                    let f = self.arg_entity(s, &fe, false)?;
                    let eyes = p.arg(seg, "facingAnchor").as_deref() == Some("eyes");
                    let mut t = self.e_pos(f);
                    if eyes {
                        t[1] += self.ent_eye_height(f);
                    }
                    self.command_look_at(e, s.eyes, t, Some((f, eyes)));
                } else {
                    let t = args::vec3(self, s, &p.arg(seg, "facingLocation").unwrap_or_default());
                    self.command_look_at(e, s.eyes, t, None);
                }
                self.ok(s, tr!("commands.rotate.success", self.ent_display(e)));
                Ok(1)
            }
            "attribute" => self.cmd_attribute(s, p, seg),
            "scoreboard" => self.cmd_scoreboard(s, p, seg),
            "team" => self.cmd_team(s, p, seg),
            "trigger" => self.cmd_trigger(s, p, seg),
            "data" => self.cmd_data(s, p, seg),
            "setblock" => self.cmd_setblock(s, p, seg),
            "fill" => self.cmd_fill(s, p, seg),
            "clone" => self.cmd_clone(s, p, seg),
            "forceload" => self.cmd_forceload(s, p, seg),
            "worldborder" => self.cmd_worldborder(s, p, seg),
            "tick" => self.cmd_tick(s, p, seg),
            "debug" => self.cmd_debug(s, p, seg),
            "jfr" => self.cmd_jfr(s, p, seg),
            "locate" => self.cmd_locate(s, p, seg),
            "recipe" => self.cmd_recipe(s, p, seg),
            "bossbar" => self.cmd_bossbar(s, p, seg),
            "swing" => {
                let targets = match p.arg(seg, "targets") {
                    Some(t) => self.arg_entities(s, &t, false)?,
                    None => vec![self.source_entity(s)?],
                };
                // W11: only living entities swing the arm; animation type and
                // duration given (default: "whack", 6 ticks); game messages.
                let hand = i32::from(seg.path.iter().any(|x| x == "offhand"));
                let kind = match p.arg(seg, "animation").as_deref() {
                    Some("none") => 0,
                    Some("stab") => 2,
                    _ => 1,
                };
                let duration = p.arg(seg, "duration").map_or(6, |d| super::cmds_basic::time_ticks(&d));
                let mut t: Track<rc_text::Component> = Default::default();
                for e in &targets {
                    if !self.ent_is_living(*e) {
                        continue;
                    }
                    let id = self.ent_id(*e);
                    let _ = self.send(Event::Entity(EntityEvent::Swing { id, hand, kind, duration }));
                    t.track(self.ent_display(*e), 1);
                }
                self.track_feedback(
                    s,
                    &t,
                    true,
                    Some(Fail(tr!("commands.swing.failed.notliving"))),
                    |_, n| tr!("commands.swing.success.single", n.clone()),
                    |c| tr!("commands.swing.success.multiple", c),
                )
            }
            _ => Err(Fail(tr!("command.failed"))),
        }
    }

    /// Creation of an entity: simulated living species,
    /// other frozen types (NBT only, visible).
    pub fn summon_entity(&mut self, s: &Src, ty: &str, pos: [f64; 3], nbt: Option<Nbt>) -> Result<E, Fail> {
        if pos[0].abs() >= 30_000_000.0 || pos[2].abs() >= 30_000_000.0 || pos[1].abs() >= 20_000_000.0 {
            return Err(Fail(tr!("commands.summon.invalidPosition")));
        }
        let finalize = nbt.is_none();
        if let Some(kind) = Kind::from_name(ty) {
            if kind.info().not_in_peaceful && self.level.difficulty_id() == 0 {
                return Err(Fail(tr!("commands.summon.failed.peaceful")));
            }
            let mut m = self.new_mob(kind, pos, 0.0);
            let yaw = m.rng.next_float() * std::f32::consts::TAU;
            m.yaw = yaw;
            m.head_yaw = yaw;
            m.body_yaw = yaw;
            if let Some(n) = &nbt {
                let mut full = n.clone();
                crate::nbtio::set(&mut full, "id", Nbt::String(ty.to_owned()));
                crate::nbtio::set(&mut full, "Pos", Nbt::List(pos.iter().map(|&x| Nbt::Double(x)).collect()));
                self.load_mob(&mut m, &full);
                m.pos = pos;
                m.old = pos;
                m.sync_data();
            }
            if finalize {
                // F11: reread of the `{id}` compound done by the game before finalization.
                crate::mobs::wild::summon_loaded(&mut m);
                let mut g = crate::mobs::spawn::Group::default();
                self.finalize_spawn(&mut m, crate::mobs::spawn::Reason::SpawnEgg, &mut g);
            }
            let id = m.id;
            self.add_mob(m);
            let _ = s;
            return Ok(E::Mob(id));
        }
        if ty == "minecraft:player" {
            return Err(Fail(tr!("commands.summon.failed")));
        }
        // Item on the ground with its stack: simulated (pickup, merging, damage) like a dropped item.
        if ty == "minecraft:item"
            && let Some(n) = &nbt
            && let Some(stack) = n.get("Item").and_then(crate::player::Stack::from_nbt)
        {
            let vel = crate::nbtio::motion(n.get("Motion")).unwrap_or_default();
            let delay = crate::nbtio::int(n, "PickupDelay").unwrap_or(0) as i32;
            self.spawn_item(pos, vel, stack, delay);
            if let Some(d) = self.life.drops.last_mut() {
                d.age = crate::nbtio::int(n, "Age").unwrap_or(0) as i32;
                d.health = crate::nbtio::int(n, "Health").unwrap_or(5) as i32;
                return Ok(E::Item(d.id));
            }
        }
        // F15: crystal, cloud, dragon fireball, shulker bullet simulated.
        if let Some(id) = self.end_summon(ty, pos, nbt.as_ref()) {
            return Ok(E::Other(id));
        }
        // G5-F06: minecarts simulated.
        if let Some(id) = self.cart_summon(ty, pos, nbt.as_ref()) {
            return Ok(E::Other(id));
        }
        if ty == "minecraft:item" {
            return self.summon_item(pos, nbt.as_ref());
        }
        // Primed TNT, falling block, orb, projectiles: simulated, movement NBT included
        // (`Motion`, fuse, state, value…), as the game reads them at summon time.
        if let Some(id) = self.summon_sim_entity(ty, pos, nbt.as_ref()) {
            return Ok(E::Other(id));
        }
        if ty == "minecraft:experience_orb" {
            return Ok(E::Orb(self.summon_orb(pos, nbt.as_ref())));
        }
        if crate::projectiles::PKind::from_name(ty).is_some() {
            let mut full = nbt.clone().unwrap_or(Nbt::Compound(Vec::new()));
            crate::nbtio::set(&mut full, "id", Nbt::String(ty.to_owned()));
            crate::nbtio::set(&mut full, "Pos", Nbt::List(pos.iter().map(|&x| Nbt::Double(x)).collect()));
            if let Some(p) = self.proj_from_nbt(&full) {
                let id = p.id;
                self.proj_add(p);
                return Ok(E::Other(id));
            }
        }
        // Non-simulated entity: compound kept, spawn announced to the client.
        self.next_entity += 1;
        let id = self.next_entity;
        let uuid = (self.life.rng.next_long() as u128) << 64 | self.life.rng.next_long() as u64 as u128;
        let mut n = nbt.unwrap_or(Nbt::Compound(Vec::new()));
        crate::nbtio::set(&mut n, "id", Nbt::String(ty.to_owned()));
        crate::nbtio::set(&mut n, "Pos", Nbt::List(pos.iter().map(|&x| Nbt::Double(x)).collect()));
        if n.get("UUID").is_none() {
            crate::nbtio::set(&mut n, "UUID", crate::nbtio::uuid_nbt(uuid));
        }
        let chunk = ((pos[0].floor() as i32) >> 4, (pos[2].floor() as i32) >> 4);
        // Spawn and metadata; paintings, frames, supports, cushions simulated.
        self.deco_announce(id, &n);
        self.deco_register(id, &n);
        self.entities.entry(chunk).or_default().push((id, n));
        self.life.drop_chunks.insert(chunk);
        Ok(E::Other(id))
    }

    /// Summoned item on the ground: a real simulated item entity (fall, merging, pickup by the
    /// player and mobs, sensors), read as at load time (`Item`, `Motion`, `Age`,
    /// `PickupDelay` at 0 by default). Without an item, the entity is removed as soon as it is read and
    /// the add fails (duplicate id message, like the game).
    fn summon_item(&mut self, pos: [f64; 3], nbt: Option<&Nbt>) -> Result<E, Fail> {
        use crate::nbtio::int;
        let Some(stack) = nbt.and_then(|n| n.get("Item")).and_then(crate::player::Stack::from_nbt) else {
            return Err(Fail(tr!("commands.summon.failed.uuid")));
        };
        let vel = nbt.and_then(|n| crate::nbtio::motion(n.get("Motion"))).unwrap_or_default();
        let delay = nbt.and_then(|n| int(n, "PickupDelay")).unwrap_or(0) as i32;
        let age = nbt.and_then(|n| int(n, "Age")).unwrap_or(0) as i32;
        self.spawn_item(pos, vel, stack, delay);
        let d = self.life.drops.last_mut().ok_or_else(|| Fail(tr!("commands.summon.failed")))?;
        d.age = age;
        Ok(E::Item(d.id))
    }

    /// `execute on attacker|target|owner`.
    pub fn relation_attacker(&self, e: E) -> Option<E> {
        match e {
            E::Mob(i) => self.mobs.mob_ref_any(i).and_then(|m| m.last_hurt_by_mob.map(|x| x.0)).map(to_e),
            E::Player => self.mobs.player_hurt_by.map(|x| to_e(x.0)),
            _ => None,
        }
    }

    pub fn relation_target(&self, e: E) -> Option<E> {
        match e {
            E::Mob(i) => self.mobs.mob_ref_any(i).and_then(|m| m.target).map(to_e),
            _ => None,
        }
    }

    pub fn relation_owner(&self, e: E) -> Option<E> {
        match e {
            E::Mob(i) => self.mobs.mob_ref_any(i).and_then(|m| m.owner).and_then(|u| self.ent_by_uuid(u)),
            _ => None,
        }
    }

    fn effect_net_id(&self, id: &str) -> i32 {
        self.data
            .registry("minecraft:mob_effect")
            .and_then(|r| r.entries.iter().position(|e| e == id))
            .map_or(0, |i| i as i32)
    }

    /// Adding an effect: player through the real effects (S7, `effects.rs`); other living
    /// entities: kept and sent (not simulated).
    fn give_effect(&mut self, e: E, ef: Effect) -> bool {
        if e == E::Player {
            let mut inst = crate::effects::Inst::new(&ef.id, ef.duration, ef.amplifier);
            inst.ambient = ef.ambient;
            inst.visible = ef.particles;
            inst.icon = ef.icon;
            return self.add_effect(inst);
        }
        // Simulated mobs: real effects (F01 + S7, `mobs/effects.rs`).
        if let E::Mob(i) = e {
            let mut inst = crate::effects::Inst::new(&ef.id, ef.duration, ef.amplifier);
            inst.ambient = ef.ambient;
            inst.visible = ef.particles;
            inst.icon = ef.icon;
            let Some(k) = self.mob_index(i) else { return false };
            let Some(mut m) = self.mobs.list[k].take() else { return false };
            let r = self.add_mob_effect(&mut m, inst);
            self.mobs.list[k] = Some(m);
            return r;
        }
        let id = self.ent_id(e);
        let list = self.cmd.mob_effects.entry(id).or_default();
        let changed = match list.iter_mut().find(|x| x.id == ef.id) {
            Some(old) => {
                let longer = ef.duration == -1 || (old.duration != -1 && ef.duration > old.duration);
                if ef.amplifier > old.amplifier
                    || (ef.amplifier == old.amplifier && longer)
                    || ef.ambient != old.ambient
                {
                    *old = ef.clone();
                    true
                } else {
                    false
                }
            }
            None => {
                list.push(ef.clone());
                true
            }
        };
        if changed {
            let flags = i8::from(ef.ambient) | if ef.particles { 2 } else { 0 } | if ef.icon { 4 } else { 0 };
            let effect = self.effect_net_id(&ef.id);
            let _ = self.send(Event::Entity(EntityEvent::Effect {
                id,
                effect,
                amplifier: ef.amplifier,
                duration: ef.duration,
                flags,
            }));
        }
        changed
    }

    fn take_effect(&mut self, e: E, which: Option<&str>) -> bool {
        if e == E::Player {
            return match which {
                Some(w) => self.remove_effect(w),
                None => self.clear_effects(),
            };
        }
        if let E::Mob(i) = e {
            let Some(k) = self.mob_index(i) else { return false };
            let Some(mut m) = self.mobs.list[k].take() else { return false };
            let r = match which {
                Some(w) => self.remove_mob_effect(&mut m, w),
                None => self.clear_mob_effects(&mut m),
            };
            self.mobs.list[k] = Some(m);
            return r;
        }
        let id = self.ent_id(e);
        let list = self.cmd.mob_effects.entry(id).or_default();
        let gone: Vec<String> = list.iter().filter(|x| which.is_none_or(|w| x.id == w)).map(|x| x.id.clone()).collect();
        list.retain(|x| which.is_some_and(|w| x.id != w));
        for g in &gone {
            let effect = self.effect_net_id(g);
            let _ = self.send(Event::Entity(EntityEvent::RemoveEffect { id, effect }));
        }
        !gone.is_empty()
    }

    fn cmd_effect(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let w: Vec<&str> = seg.path.iter().map(String::as_str).collect();
        let targets = match p.arg(seg, "targets") {
            Some(t) => self.arg_entities(s, &t, false)?,
            None => vec![self.source_entity(s)?],
        };
        let mut t: Track<Component> = Track::default();
        if w.get(1) == Some(&"clear") {
            match p.arg(seg, "effect") {
                None => {
                    for e in targets {
                        if self.ent_is_living(e) {
                            let r = self.take_effect(e, None);
                            t.track(self.ent_display(e), i32::from(r));
                        }
                    }
                    self.track_feedback(
                        s,
                        &t,
                        false,
                        Some(Fail(tr!("commands.effect.clear.everything.failed"))),
                        |_, n| tr!("commands.effect.clear.everything.success.single", n.clone()),
                        |c| tr!("commands.effect.clear.everything.success.multiple", c),
                    )
                }
                Some(ef) => {
                    let id = args::id(&ef);
                    for e in targets {
                        if self.ent_is_living(e) {
                            let r = self.take_effect(e, Some(&id));
                            t.track(self.ent_display(e), i32::from(r));
                        }
                    }
                    let name = effect_name(&id);
                    self.track_feedback(
                        s,
                        &t,
                        false,
                        Some(Fail(tr!("commands.effect.clear.specific.failed"))),
                        |_, n| tr!("commands.effect.clear.specific.success.single", name.clone(), n.clone()),
                        |c| tr!("commands.effect.clear.specific.success.multiple", name.clone(), c),
                    )
                }
            }
        } else {
            let id = args::id(&p.arg(seg, "effect").unwrap_or_default());
            let infinite = w.contains(&"infinite");
            let secs: Option<i32> =
                if infinite { Some(-1) } else { p.arg(seg, "seconds").and_then(|v| v.parse().ok()) };
            let duration = match secs {
                Some(v) if instant(&id) => v,
                Some(-1) => -1,
                Some(v) => v * 20,
                None => {
                    if instant(&id) {
                        1
                    } else {
                        600
                    }
                }
            };
            let amplifier: i32 = p.arg(seg, "amplifier").and_then(|v| v.parse().ok()).unwrap_or(0);
            let hide = p.arg(seg, "hideParticles").as_deref() == Some("true");
            for e in targets {
                if !self.ent_is_living(e) {
                    continue;
                }
                let ef = Effect { id: id.clone(), amplifier, duration, ambient: false, particles: !hide, icon: true };
                let r = self.give_effect(e, ef);
                t.track(self.ent_display(e), i32::from(r));
            }
            let name = effect_name(&id);
            let d = duration / 20;
            self.track_feedback(
                s,
                &t,
                false,
                Some(Fail(tr!("commands.effect.give.failed"))),
                |_, n| tr!("commands.effect.give.success.single", name.clone(), n.clone(), d),
                |c| tr!("commands.effect.give.success.multiple", name.clone(), c, d),
            )
        }
    }

    fn cmd_enchant(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let targets = self.arg_entities(s, &p.arg(seg, "targets").unwrap_or_default(), false)?;
        let ench = args::id(&p.arg(seg, "enchantment").unwrap_or_default());
        let level: i32 = p.arg(seg, "level").and_then(|v| v.parse().ok()).unwrap_or(1);
        let defs = crate::enchanting::of(&self.life.pack);
        let Some(def) = defs.get(&ench) else {
            return Err(Fail(tr!("argument.resource.not_found", ench, "minecraft:enchantment")));
        };
        let max = def.max_level;
        if level > max {
            return Err(Fail(tr!("commands.enchant.failed.level", level, max)));
        }
        let single = targets.len() == 1;
        let mut t: Track<Component> = Track::default();
        for e in &targets {
            if *e != E::Player {
                if single {
                    let name = plain(&self.ent_display(*e));
                    return Err(Fail(if self.ent_is_living(*e) {
                        tr!("commands.enchant.failed.itemless", name)
                    } else {
                        tr!("commands.enchant.failed.entity", name)
                    }));
                }
                continue;
            }
            let slot = 36 + self.held as usize;
            let Some(mut st) = self.slots[slot].clone() else {
                if single {
                    return Err(Fail(tr!("commands.enchant.failed.itemless", plain(&self.player_display_name()))));
                }
                continue;
            };
            // The possible-enchantment test (supported items) and compatibility with each enchantment present.
            let items = &self.life.items;
            let have = crate::enchanting::enchants_of(items, &st);
            let ok = defs.supports(items, def, &st.id) && have.iter().all(|(h, _)| defs.compatible(h, &ench));
            if !ok {
                if single {
                    return Err(Fail(tr!("commands.enchant.failed.incompatible", plain(&self.stack_name(&st)))));
                }
                continue;
            }
            crate::enchanting::upgrade(items, &mut st, &ench, level);
            self.slots[slot] = Some(st);
            t.track(self.ent_display(*e), 1);
        }
        self.broadcast_menus();
        let full = enchantment_name(&ench, level, max);
        self.track_feedback(
            s,
            &t,
            false,
            Some(Fail(tr!("commands.enchant.failed"))),
            |_, n| tr!("commands.enchant.success.single", full.clone(), n.clone()),
            |c| tr!("commands.enchant.success.multiple", full.clone(), c),
        )
    }

    fn cmd_damage(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let e = self.arg_entity(s, &p.arg(seg, "target").unwrap_or_default(), false)?;
        let amount: f32 = p.arg(seg, "amount").and_then(|v| v.parse().ok()).unwrap_or(0.0);
        let ty = p.arg(seg, "damageType").map_or_else(|| "minecraft:generic".to_owned(), |t| args::id(&t));
        // Source: `at <position>` alone, `by <entity>` (direct and author), `from <author>`.
        let at = p.arg(seg, "location").map(|t| args::vec3(self, s, &t));
        let direct = match p.arg(seg, "entity") {
            Some(t) => Some(self.arg_entity(s, &t, false)?),
            None => None,
        };
        let causing = match p.arg(seg, "cause") {
            Some(t) => Some(self.arg_entity(s, &t, false)?),
            None => direct,
        };
        let ok = self.damage_with_source(e, &ty, amount, direct, causing, at);
        if !ok {
            return Err(Fail(tr!("commands.damage.invulnerable")));
        }
        self.ok(s, tr!("commands.damage.success", jf(amount), self.ent_display(e)));
        Ok(1)
    }

    /// The server-side damage computation of an entity with a full source: direct entity, author, position.
    fn damage_with_source(
        &mut self,
        e: E,
        ty: &str,
        amount: f32,
        direct: Option<E>,
        causing: Option<E>,
        at: Option<[f64; 3]>,
    ) -> bool {
        // Position of the source: given, otherwise that of the direct entity.
        let src = at.or_else(|| direct.map(|d| self.e_pos(d)));
        let by = match causing {
            Some(E::Player) => Some(Ent::Player),
            Some(E::Mob(i)) => Some(Ent::Mob(i)),
            _ => None,
        };
        let take = |s: &mut Self, i: i32| {
            let k = s.mobs.index_of(i)?;
            s.mobs.list[k].take().map(|m| (k, m))
        };
        match e {
            E::Player => {
                let dir = src.map_or((0.0, 0.0), |p| (p[0] - self.pos[0], p[2] - self.pos[2]));
                if let Some(Ent::Mob(i)) = by
                    && let Some((k, m)) = take(self, i)
                {
                    let r = self.player_hurt_by_mob(&m, ty, amount, Some(dir));
                    self.mobs.list[k] = Some(m);
                    return r;
                }
                let full = self.life.vit.cooldown <= 10 || self.dmg_tag(ty, "minecraft:bypasses_cooldown");
                let r = self.hurt(ty, amount);
                if r && full && !self.dmg_tag(ty, "minecraft:no_knockback") {
                    self.player_knockback(0.4, dir.0, dir.1);
                }
                r
            }
            E::Mob(i) => {
                let Some((k, mut m)) = take(self, i) else { return false };
                self.mobs.rec.direct_source = by.map(|_| direct == causing);
                let r = self.mob_hurt(&mut m, ty, amount, by, src);
                self.mobs.rec.direct_source = None;
                self.mobs.list[k] = Some(m);
                r
            }
            E::Item(i) => self.item_hurt(i, ty, amount, matches!(causing, Some(E::Mob(_)))),
            E::Orb(i) => self.orb_hurt(i, amount),
            // G5-F06: minecart.
            E::Other(i) if self.cart_ref(i).is_some() => match self.cart_take(i) {
                Some((k, mut c)) => {
                    let r = self.cart_hurt(&mut c, ty, amount, by);
                    self.cart_put(k, c);
                    r
                }
                None => false,
            },
            // F15: End crystal; the other frozen entities do not hold hit points.
            E::Other(i) => self.crystal_hurt(i, ty, by, by == Some(Ent::Player) && self.game_mode == 1),
        }
    }

    fn cmd_attribute(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let e = self.arg_entity(s, &p.arg(seg, "target").unwrap_or_default(), false)?;
        let attr = args::id(&p.arg(seg, "attribute").unwrap_or_default());
        let desc = tr!(&format!("attribute.name.{}", attr.trim_start_matches("minecraft:")));
        let w: Vec<&str> = seg.path.iter().map(String::as_str).collect();
        let Some(base) = self.attribute_base(e, &attr) else {
            return Err(Fail(tr!("commands.attribute.failed.no_attribute", self.ent_display(e), desc)));
        };
        let scale: f64 = p.arg(seg, "scale").and_then(|v| v.parse().ok()).unwrap_or(1.0);
        match (w.get(3).copied(), w.get(4).copied()) {
            (Some("get"), _) => {
                let r = (base * scale).floor() as i32;
                self.ok(
                    s,
                    tr!("commands.attribute.value.get.success", desc, self.ent_display(e), super::text::jd(base)),
                );
                Ok(r)
            }
            (Some("base"), Some("get")) => {
                let r = (base * scale).floor() as i32;
                self.ok(
                    s,
                    tr!("commands.attribute.base_value.get.success", desc, self.ent_display(e), super::text::jd(base)),
                );
                Ok(r)
            }
            (Some("base"), Some("set")) => {
                let v: f64 = p.arg(seg, "value").and_then(|x| x.parse().ok()).unwrap_or(0.0);
                self.set_attribute_base(e, &attr, v);
                self.ok(
                    s,
                    tr!("commands.attribute.base_value.set.success", desc, self.ent_display(e), super::text::jd(v)),
                );
                Ok(1)
            }
            _ => Err(Fail(tr!("commands.attribute.failed.entity", self.ent_display(e)))),
        }
    }

    fn attribute_base(&self, e: E, attr: &str) -> Option<f64> {
        match e {
            E::Mob(i) => {
                let m = self.mobs.mob_ref_any(i)?;
                Some(match attr {
                    "minecraft:max_health" => m.attrs.max_health.base,
                    "minecraft:movement_speed" => m.attrs.speed.base,
                    "minecraft:attack_damage" => m.attrs.attack.base,
                    "minecraft:follow_range" => m.attrs.follow.base,
                    "minecraft:armor" => m.attrs.armor.base,
                    "minecraft:step_height" => m.attrs.step.base,
                    "minecraft:gravity" => m.attrs.gravity.base,
                    "minecraft:jump_strength" => m.attrs.jump.base,
                    _ => return None,
                })
            }
            E::Player => {
                let saved = crate::nbtio::list(self.pdata.root.get("attributes"))
                    .iter()
                    .find(|a| a.get("id").and_then(Nbt::as_str) == Some(attr))
                    .and_then(|a| a.get("base").and_then(crate::mobs::mob::f64_of));
                saved.or(match attr {
                    "minecraft:max_health" => Some(20.0),
                    "minecraft:movement_speed" => Some(0.1),
                    "minecraft:attack_damage" => Some(1.0),
                    "minecraft:attack_speed" => Some(4.0),
                    "minecraft:armor" | "minecraft:armor_toughness" | "minecraft:luck" => Some(0.0),
                    "minecraft:block_interaction_range" => Some(4.5),
                    "minecraft:entity_interaction_range" => Some(3.0),
                    "minecraft:gravity" => Some(0.08),
                    "minecraft:jump_strength" => Some(0.42),
                    "minecraft:step_height" => Some(0.6),
                    "minecraft:scale" => Some(1.0),
                    "minecraft:safe_fall_distance" => Some(3.0),
                    "minecraft:fall_damage_multiplier" => Some(1.0),
                    _ => None,
                })
            }
            _ => None,
        }
    }

    fn set_attribute_base(&mut self, e: E, attr: &str, v: f64) {
        match e {
            E::Mob(i) => {
                if let Some(m) = self.mobs.get_mut(i) {
                    let slot = match attr {
                        "minecraft:max_health" => &mut m.attrs.max_health,
                        "minecraft:movement_speed" => &mut m.attrs.speed,
                        "minecraft:attack_damage" => &mut m.attrs.attack,
                        "minecraft:follow_range" => &mut m.attrs.follow,
                        "minecraft:armor" => &mut m.attrs.armor,
                        "minecraft:step_height" => &mut m.attrs.step,
                        "minecraft:gravity" => &mut m.attrs.gravity,
                        _ => &mut m.attrs.jump,
                    };
                    slot.base = v;
                }
            }
            E::Player => {
                let mut list: Vec<Nbt> = crate::nbtio::list(self.pdata.root.get("attributes")).to_vec();
                list.retain(|a| a.get("id").and_then(Nbt::as_str) != Some(attr));
                list.push(Nbt::Compound(vec![
                    ("id".into(), Nbt::String(attr.into())),
                    ("base".into(), Nbt::Double(v)),
                ]));
                crate::nbtio::set(&mut self.pdata.root, "attributes", Nbt::List(list));
                self.send_attributes();
            }
            _ => {}
        }
    }

    /// `tick query|rate|freeze|unfreeze|step|sprint`.
    fn cmd_tick(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let w: Vec<&str> = seg.path.iter().map(String::as_str).collect();
        let ms = |n: u64| format!("{:.1}", n as f32 / 1_000_000.0);
        match w.get(1).copied() {
            Some("query") => {
                let t = &self.cmd.ticking;
                let (rate, busy, target) = (format!("{:.1}", t.rate()), t.average_tick_nanos(), t.nanos_per_tick());
                let (p50, p95, p99, n) = t.percentiles();
                if t.sprinting() {
                    self.ok(s, tr!("commands.tick.status.sprinting"));
                    self.ok(s, tr!("commands.tick.query.rate.sprinting", rate.clone(), ms(busy)));
                } else {
                    let status = if t.frozen() {
                        "commands.tick.status.frozen"
                    } else if target < busy {
                        "commands.tick.status.lagging"
                    } else {
                        "commands.tick.status.running"
                    };
                    self.ok(s, tr!(status));
                    self.ok(s, tr!("commands.tick.query.rate.running", rate.clone(), ms(busy), ms(target)));
                }
                self.ok(s, tr!("commands.tick.query.percentiles", ms(p50), ms(p95), ms(p99), n as i32));
                Ok(self.cmd.ticking.rate() as i32)
            }
            Some("rate") => {
                let r: f32 = p.arg(seg, "rate").and_then(|v| v.parse().ok()).unwrap_or(20.0);
                self.cmd.ticking.set_rate(r);
                self.on_tick_rate_changed();
                self.send_ticking_state();
                self.ok(s, tr!("commands.tick.rate.success", format!("{r:.1}")));
                Ok(r as i32)
            }
            Some(f @ ("freeze" | "unfreeze")) => {
                let freeze = f == "freeze";
                if freeze {
                    if let Some(rep) = self.cmd.ticking.stop_sprinting() {
                        self.sprint_finished(&rep);
                    }
                    if self.cmd.ticking.stop_stepping() {
                        self.send_ticking_step();
                    }
                }
                self.cmd.ticking.set_frozen(freeze);
                self.send_ticking_state();
                self.ok(s, tr!(if freeze { "commands.tick.status.frozen" } else { "commands.tick.status.running" }));
                Ok(i32::from(freeze))
            }
            Some("step") => {
                if w.get(2) == Some(&"stop") {
                    if !self.cmd.ticking.stop_stepping() {
                        return Err(Fail(tr!("commands.tick.step.stop.fail")));
                    }
                    self.send_ticking_step();
                    self.ok(s, tr!("commands.tick.step.stop.success"));
                    return Ok(1);
                }
                let n = p.arg(seg, "time").map_or(1, |t| super::cmds_basic::time_ticks(&t));
                if !self.cmd.ticking.step_if_frozen(n) {
                    return Err(Fail(tr!("commands.tick.step.fail")));
                }
                self.send_ticking_step();
                self.ok(s, tr!("commands.tick.step.success", n));
                Ok(1)
            }
            Some("sprint") => {
                if w.get(2) == Some(&"stop") {
                    let Some(rep) = self.cmd.ticking.stop_sprinting() else {
                        return Err(Fail(tr!("commands.tick.sprint.stop.fail")));
                    };
                    self.sprint_finished(&rep);
                    self.ok(s, tr!("commands.tick.sprint.stop.success"));
                    return Ok(1);
                }
                let n = p.arg(seg, "time").map_or(1, |t| super::cmds_basic::time_ticks(&t));
                let interrupted = self.cmd.ticking.request_sprint(n.max(0) as u64);
                self.send_ticking_state();
                if interrupted {
                    self.ok(s, tr!("commands.tick.sprint.stop.success"));
                }
                self.ok(s, tr!("commands.tick.status.sprinting"));
                Ok(1)
            }
            _ => Err(Fail(tr!("command.unknown.command"))),
        }
    }

    /// State of the cadence (rate, freeze) to the client.
    pub(crate) fn send_ticking_state(&self) {
        let t = &self.cmd.ticking;
        let _ = self.send(Event::TickingState { rate: t.rate(), frozen: t.frozen() });
    }

    /// Remaining steps to the client.
    pub(crate) fn send_ticking_step(&self) {
        let _ = self.send(Event::TickingStep(self.cmd.ticking.step()));
    }

    /// End of a sprint: report to the operators (source "Server", grey italic), previous freeze
    /// restored and announced.
    pub(crate) fn sprint_finished(&mut self, rep: &crate::tickrate::SprintReport) {
        if self.rule("minecraft:send_command_feedback", true) && self.cmd.op >= 2 {
            let msg = tr!("commands.tick.sprint.report", rep.ticks_per_second, rep.ms_per_tick.clone());
            let mut c = tr!("chat.type.admin", lit("Server"), msg);
            c.style.color = Some(super::text::GRAY);
            c.style.italic = Some(true);
            self.system(c);
        }
        self.on_tick_rate_changed();
        self.send_ticking_state();
    }
}

fn to_e(e: Ent) -> E {
    match e {
        Ent::Player => E::Player,
        Ent::Mob(i) => E::Mob(i),
    }
}

/// Translated name of an effect.
pub fn effect_name(id: &str) -> Component {
    tr!(&format!("effect.{}", id.replace(':', ".")))
}

/// Full name of an enchantment: level in Roman numerals
/// if the maximum level exceeds 1; curses in red, otherwise grey.
pub fn enchantment_name(id: &str, level: i32, max: i32) -> Component {
    let mut c = Component::text("");
    c.extra.push(tr!(&format!("enchantment.{}", id.replace(':', "."))));
    if level != 1 || max != 1 {
        c.extra.push(lit(" "));
        c.extra.push(tr!(&format!("enchantment.level.{level}")));
    }
    let curse = id.ends_with("_curse");
    c.style.color = Some(if curse { 0xFF5555 } else { 0xAAAAAA });
    c
}

/// Plain text of a component (error parameters passed as a string by the game).
/// Server language for the flattened texts (initialized at the first command).
pub(crate) static SERVER_LANG: std::sync::OnceLock<&'static rc_text::Lang> = std::sync::OnceLock::new();

pub fn plain(c: &Component) -> String {
    let fallback = rc_text::Lang::default();
    let lang = SERVER_LANG.get().copied().unwrap_or(&fallback);
    rc_text::plain_string(c, lang)
}

// ---------------------------------------------------------------------------------------------
// `/debug start|stop|function` and `/jfr` (visible profiling, `integrated-server.md`).

/// Tick profiler of `/debug start`: requested (starts at the beginning of the next tick, like the
/// game), then running since (instant, tick number).
#[derive(Clone, Copy)]
enum TickProfiler {
    Off,
    Pending,
    Running(std::time::Instant, u64),
}

/// Trace of `/debug function`: file text, indent of the last line, expected result
/// on the current line.
struct Tracer {
    out: String,
    last_indent: usize,
    waiting: bool,
}

thread_local! {
    // One integrated server per thread: state specific to each server thread (parallel tests).
    static TICK_PROFILER: std::cell::Cell<TickProfiler> = const { std::cell::Cell::new(TickProfiler::Off) };
    static TRACE: std::cell::RefCell<Option<Tracer>> = const { std::cell::RefCell::new(None) };
}

impl Tracer {
    fn indent(&mut self, depth: usize) {
        for _ in 0..depth + 1 {
            self.out.push_str("    ");
        }
    }

    fn new_line(&mut self) {
        if self.waiting {
            self.out.push('\n');
            self.waiting = false;
        }
    }
}

/// A trace of `/debug function` is in progress.
pub(crate) fn tracing() -> bool {
    TRACE.with(|t| t.borrow().is_some())
}

fn with_trace(f: impl FnOnce(&mut Tracer)) -> bool {
    TRACE.with(|t| match t.borrow_mut().as_mut() {
        Some(tr) => {
            f(tr);
            true
        }
        None => false,
    })
}

/// Call of a traced function (`[F] id size=n`).
pub(crate) fn trace_call(depth: usize, id: &str, size: usize) {
    with_trace(|t| {
        t.new_line();
        t.indent(depth);
        t.last_indent = depth;
        t.out.push_str(&format!("[F] {id} size={size}\n"));
    });
}

/// Start of a traced command (`[C] command`, expected result on the same line).
pub(crate) fn trace_command(depth: usize, command: &str) {
    with_trace(|t| {
        t.new_line();
        t.indent(depth);
        t.last_indent = depth;
        t.out.push_str("[C] ");
        t.out.push_str(command);
        t.waiting = true;
    });
}

/// Result of a successful command (` -> v`, or `[R = v] command` on a separate line).
pub(crate) fn trace_return(depth: usize, command: &str, result: i32) {
    with_trace(|t| {
        if t.waiting {
            t.out.push_str(&format!(" -> {result}\n"));
            t.waiting = false;
        } else {
            t.indent(depth);
            t.last_indent = depth;
            t.out.push_str(&format!("[R = {result}] {command}\n"));
        }
    });
}

/// Message sent to the source of a traced command (success or direct failure): the trace
/// replaces the output, line `[M] text`. True if the message was taken by the trace.
pub(crate) fn trace_message(msg: &Component) -> bool {
    if TRACE.with(|t| t.borrow().is_none()) {
        return false;
    }
    let text = plain(msg);
    with_trace(|t| {
        t.new_line();
        let d = t.last_indent + 1;
        t.indent(d);
        t.out.push_str("[M] ");
        t.out.push_str(&text);
        t.out.push('\n');
    })
}

/// Command exception: `[E] text` WITHOUT a line end (like the game: the next line sticks to
/// it) and indentation of the rest raised by one level. Also written in forked mode.
pub(crate) fn trace_error(msg: &Component) {
    let text = plain(msg);
    with_trace(|t| {
        t.new_line();
        let d = t.last_indent + 1;
        t.indent(d);
        t.last_indent = d;
        t.out.push_str("[E] ");
        t.out.push_str(&text);
    });
}

/// Start of a server tick: a requested profiling starts here.
pub(crate) fn debug_profiler_tick_start(tick: u64) {
    TICK_PROFILER.with(|p| {
        if matches!(p.get(), TickProfiler::Pending) {
            p.set(TickProfiler::Running(std::time::Instant::now(), tick));
        }
    });
}

/// Timestamp of the game's file names (`yyyy-MM-dd_HH.mm.ss`, local time).
fn file_stamp() -> String {
    let (y, mo, d, h, mi, s) = crate::storage::local_datetime(crate::progress::now_ms());
    format!("{y:04}-{mo:02}-{d:02}_{h:02}.{mi:02}.{s:02}")
}

impl Server {
    /// The game's folder (folder of the integrated server): parent of `saves`.
    fn game_dir(&self) -> std::path::PathBuf {
        self.world.parent().and_then(|p| p.parent()).map_or_else(|| ".".into(), std::path::Path::to_path_buf)
    }

    pub(crate) fn cmd_debug(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        match seg.path.get(1).map(String::as_str) {
            Some("start") => {
                if !matches!(TICK_PROFILER.with(std::cell::Cell::get), TickProfiler::Off) {
                    return Err(Fail(tr!("commands.debug.alreadyRunning")));
                }
                TICK_PROFILER.with(|t| t.set(TickProfiler::Pending));
                self.ok(s, tr!("commands.debug.started"));
                Ok(0)
            }
            Some("stop") => {
                let state = TICK_PROFILER.with(std::cell::Cell::get);
                // Stop before the first tick: empty results (0 s, 0 tick, NaN per second) and,
                // as in game 26.3, the deferred start is NOT cancelled: profiling
                // still starts on the next tick (a new `start` answers "already started").
                let (secs, ticks) = match state {
                    TickProfiler::Off => return Err(Fail(tr!("commands.debug.notRunning"))),
                    TickProfiler::Pending => (0.0, 0),
                    TickProfiler::Running(t0, k0) => {
                        TICK_PROFILER.with(|t| t.set(TickProfiler::Off));
                        (t0.elapsed().as_secs_f64(), self.tick.saturating_sub(k0) as i32)
                    }
                };
                let tps = f64::from(ticks) / secs;
                self.ok(s, tr!("commands.debug.stopped", java_2f(secs), ticks, java_2f(tps)));
                Ok(if tps.is_nan() { 0 } else { tps as i32 })
            }
            Some("function") => self.cmd_debug_function(s, p, seg),
            _ => Err(Fail(tr!("command.unknown.command"))),
        }
    }

    fn cmd_debug_function(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        if TRACE.with(|t| t.borrow().is_some()) {
            return Err(Fail(tr!("commands.debug.function.noRecursion")));
        }
        let name = p.arg(seg, "name").unwrap_or_default();
        let ids: Vec<String> = if let Some(t) = name.strip_prefix('#') {
            let t = args::id(t);
            match self.cmd.function_tags.get(&t) {
                Some(v) => v.clone(),
                None => return Err(Fail(tr!("arguments.function.tag.unknown", t))),
            }
        } else {
            let id = args::id(&name);
            if !self.cmd.functions.contains_key(&id) {
                return Err(Fail(tr!("arguments.function.unknown", id)));
            }
            vec![id]
        };
        let file = format!("debug-trace-{}.txt", file_stamp());
        let dir = self.game_dir().join("debug");
        TRACE.with(|t| *t.borrow_mut() = Some(Tracer { out: String::new(), last_indent: 0, waiting: false }));
        let mut total = 0usize;
        let mut functions = 0usize;
        let mut single = String::new();
        for id in &ids {
            let Some(lines) = self.cmd.functions.get(id) else { continue };
            total += lines.iter().filter(|l| !l.trim().is_empty() && !l.trim().starts_with('#')).count();
            functions += 1;
            single.clone_from(id);
            with_trace(|t| {
                t.new_line();
                t.out.push_str(id);
                t.out.push('\n');
            });
            let mut fs = s.clone();
            fs.silent = false;
            let _ = self.run_functions(&fs, id, false);
        }
        let trace = TRACE.with(|t| t.borrow_mut().take()).map(|t| t.out).unwrap_or_default();
        let written = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(dir.join(&file), trace));
        if written.is_err() {
            self.fail(s, tr!("commands.debug.function.traceFailed"));
        }
        let msg = if functions == 1 {
            tr!("commands.debug.function.success.single", total, lit(single), file)
        } else {
            tr!("commands.debug.function.success.multiple", total, functions, file)
        };
        self.ok(s, msg);
        Ok(total as i32)
    }

    /// `/jfr`: the Java virtual machine's recorder does not exist here; the game's replies
    /// when it does not start (`start`) and when no recording is in progress (`stop`).
    pub(crate) fn cmd_jfr(&mut self, _s: &Src, _p: &Parsed, seg: &Seg) -> R {
        match seg.path.get(1).map(String::as_str) {
            Some("start") => Err(Fail(tr!("commands.jfr.start.failed"))),
            Some("stop") => Err(Fail(tr!("commands.jfr.dump.failed", "Not currently profiling"))),
            _ => Err(Fail(tr!("command.unknown.command"))),
        }
    }
}

/// Language-independent `%.2f` formatting: NaN and infinities written like Java.
fn java_2f(v: f64) -> String {
    if v.is_nan() {
        "NaN".into()
    } else if v.is_infinite() {
        if v > 0.0 { "Infinity".into() } else { "-Infinity".into() }
    } else {
        format!("{v:.2}")
    }
}

#[cfg(test)]
mod debug_tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use crate::server::Server;

    /// Test world laid out like the game (`<game>/saves/w`) with a function `t:f`; client
    /// session kept to read the messages.
    fn open(tag: &str) -> Option<(Server, rc_client::Session, std::path::PathBuf)> {
        let (Some(world), Some(data)) = (crate::tests::test_world(), rc_testdata::version_data()) else {
            eprintln!("test world or data missing: test skipped");
            return None;
        };
        let copy = crate::tests::fresh_copy(&world, tag);
        let game = copy.with_extension("game");
        let _ = std::fs::remove_dir_all(&game);
        let dir = game.join("saves").join("w");
        std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
        std::fs::rename(&copy, &dir).unwrap();
        let pack = dir.join("datapacks").join("t");
        std::fs::create_dir_all(pack.join("data/t/function")).unwrap();
        std::fs::write(pack.join("pack.mcmeta"), r#"{"pack":{"pack_format":121,"description":"t"}}"#).unwrap();
        std::fs::write(
            pack.join("data/t/function/f.mcfunction"),
            "# comment\nscoreboard objectives add dbg dummy\nscoreboard players set #a dbg 7\nkill @e[tag=nobody]\nexecute as @e[type=player] run kill @e[tag=nobody]\n",
        )
        .unwrap();
        crate::tests::solo_slot();
        let uuid = rc_client::offline_uuid("S2V5");
        let (session, peer) = rc_client::memory::session(Arc::new(data.clone()), uuid).expect("session");
        let opts = crate::Options {
            world: dir.clone(),
            name: "S2V5".into(),
            uuid,
            info: rc_proto_v777::play::ClientInformation { view_distance: 2, ..Default::default() },
            sim_distance: 12,
        };
        let s = match Server::open(opts, uuid, Arc::new(data), peer, Arc::new(AtomicBool::new(false))) {
            Ok(s) => s,
            Err((_, e)) => panic!("ouverture : {e}"),
        };
        Some((s, session, game))
    }

    fn run(s: &mut Server, c: &str) -> (bool, i32) {
        let mut src = s.player_source();
        src.level = 4;
        s.perform(src, c)
    }

    /// Chat messages received since the last read (key and arguments as text).
    fn chats(session: &rc_client::Session) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(e) = session.events.try_recv() {
            if let rc_client::Event::Chat { component, .. } = e {
                out.push(format!("{component:?}"));
            }
        }
        out
    }

    fn tick(s: &mut Server) {
        super::debug_profiler_tick_start(s.tick);
        s.game_tick();
        s.tick += 1;
    }

    /// `/debug start|stop` and `/jfr`: game messages (before: "unknown command").
    #[test]
    fn debug_start_stop_and_jfr() {
        let Some((mut s, session, game)) = open("s2v5-debug") else { return };
        s.loaded = true;
        chats(&session);
        assert!(!run(&mut s, "debug stop").0);
        assert!(chats(&session).iter().any(|c| c.contains("commands.debug.notRunning")));
        assert!(run(&mut s, "debug start").0);
        assert!(chats(&session).iter().any(|c| c.contains("commands.debug.started")));
        assert!(!run(&mut s, "debug start").0);
        assert!(chats(&session).iter().any(|c| c.contains("commands.debug.alreadyRunning")));
        // Stop before the first tick: empty results, like the game (0.00 s, 0 tick, NaN).
        assert_eq!(run(&mut s, "debug stop"), (true, 0));
        let c = chats(&session);
        let stop = c.iter().find(|c| c.contains("commands.debug.stopped")).expect("stop");
        assert!(stop.contains("Text(\"0.00\"), Text(\"0\"), Text(\"NaN\")"), "{stop}");
        // Quirk of 26.3: the deferred start is not cancelled by the stop, a new `start`
        // is refused and profiling starts anyway on the first tick.
        assert!(!run(&mut s, "debug start").0);
        assert!(chats(&session).iter().any(|c| c.contains("commands.debug.alreadyRunning")));
        // Six ticks: profiling starts at the beginning of the first (the game's deferred start).
        for _ in 0..6 {
            tick(&mut s);
        }
        assert!(run(&mut s, "debug stop").0);
        let c = chats(&session);
        let stop = c.iter().find(|c| c.contains("commands.debug.stopped")).expect("stop 2");
        assert!(stop.contains("Text(\"6\")"), "{stop}");
        assert!(!run(&mut s, "jfr start").0);
        assert!(chats(&session).iter().any(|c| c.contains("commands.jfr.start.failed")));
        assert!(!run(&mut s, "jfr stop").0);
        let c = chats(&session);
        assert!(
            c.iter().any(|c| c.contains("commands.jfr.dump.failed") && c.contains("Not currently profiling")),
            "{c:?}"
        );
        drop(s);
        let _ = std::fs::remove_dir_all(game);
    }

    /// `/debug function`: trace file in `<game>/debug`, game message.
    #[test]
    fn debug_function_trace() {
        let Some((mut s, session, game)) = open("s2v5-trace") else { return };
        s.loaded = true;
        chats(&session);
        assert_eq!(run(&mut s, "debug function t:f"), (true, 4));
        let c = chats(&session);
        let msg = c.iter().find(|c| c.contains("commands.debug.function.success.single")).expect("message");
        assert!(msg.contains("t:f") && msg.contains("debug-trace-"), "{msg}");
        let files: Vec<_> = std::fs::read_dir(game.join("debug")).unwrap().flatten().map(|e| e.path()).collect();
        assert_eq!(files.len(), 1);
        let text = std::fs::read_to_string(&files[0]).unwrap();
        let mut lines = text.lines();
        assert_eq!(lines.next(), Some("t:f"));
        assert_eq!(lines.next(), Some("    [F] t:f size=4"));
        assert_eq!(lines.next(), Some("        [C] scoreboard objectives add dbg dummy"));
        assert_eq!(lines.next(), Some("            [M] Created new objective [dbg]"));
        // Result = number of objectives of the scoreboard (the test world already has some).
        let r = lines.next().unwrap_or_default();
        assert!(r.starts_with("        [R = ") && r.ends_with("] scoreboard objectives add dbg dummy"), "{r}");
        assert!(text.contains("[C] kill @e[tag=nobody]"), "{text}");
        // An exception writes `[E]` then `[M]` (same line); in forked mode, `[E]` alone.
        assert_eq!(text.matches("[E] No entity was found").count(), 2, "{text}");
        assert_eq!(text.matches("[M] No entity was found").count(), 1, "{text}");
        drop(s);
        let _ = std::fs::remove_dir_all(game);
    }
}
