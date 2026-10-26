//! Creeper explosion (§ 4.1): rays on the surface of a 16³ cube,
//! block resistance, damage and knockback of entities (visible part), blocks destroyed with their
//! loot (decay according to the rule), explosion packet to the player.

use rc_client::{EntityEvent, Event};
use rc_proto_v777::entity::Particle;
use rc_proto_v777::play::SoundRef;

use super::env::Fluid;
use super::mob::{Ent, Mob};
use super::mth::dist2;
use crate::server::Server;

#[cfg(test)]
thread_local! {
    /// Tests: primed TNT explosions of this thread, (dimension, passed through a portal).
    pub(crate) static TNT_BLASTS: std::cell::RefCell<Vec<(String, bool)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Interaction of an explosion with blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Blast {
    /// Mob: according to `mob_griefing`, decay according to `mob_explosion_drop_decay`.
    Mob,
    /// Block (crystal, bed…): always, decay according to `block_explosion_drop_decay`.
    Block,
    /// Primed TNT: always, decay according to `tnt_explosion_drop_decay`; true if it went
    /// through a portal.
    Tnt(bool),
    /// G5-F06: TNT minecart ("TNT" interaction); primed, it spares the rails and the block
    /// that carries them (zero resistance, never destroyed).
    TntCart { primed: bool },
    /// None.
    None,
    /// Bed or respawn anchor ("block"); anchor in water: its cell resists like water,
    /// even empty (own calculator for the explosion of a respawn anchor).
    Respawn(Option<[i32; 3]>),
}

impl Server {
    fn block_resistance(&self, p: [i32; 3]) -> Option<f32> {
        let s = self.st(p);
        if self.is_air(p) && self.mob_fluid(p) == Fluid::None {
            return None;
        }
        // Registry resistance (extracted from the game, per state), and 100 for water and lava.
        let fluid = match self.mob_fluid(p) {
            Fluid::None => 0.0,
            _ => 100.0,
        };
        let block = self.data.blast.get(s as usize).copied().unwrap_or(0.0);
        Some(block.max(fluid))
    }

    /// Visible part of a box from the centre.
    pub(crate) fn seen_percent(&self, center: [f64; 3], b: &rc_physics::Aabb) -> f32 {
        let xs = 1.0 / ((b.max[0] - b.min[0]) * 2.0 + 1.0);
        let ys = 1.0 / ((b.max[1] - b.min[1]) * 2.0 + 1.0);
        let zs = 1.0 / ((b.max[2] - b.min[2]) * 2.0 + 1.0);
        let xo = (1.0 - (1.0 / xs).floor() * xs) / 2.0;
        let zo = (1.0 - (1.0 / zs).floor() * zs) / 2.0;
        let (mut hits, mut count) = (0, 0);
        let mut xx = 0.0;
        while xx <= 1.0 {
            let mut yy = 0.0;
            while yy <= 1.0 {
                let mut zz = 0.0;
                while zz <= 1.0 {
                    let x = b.min[0] + xx * (b.max[0] - b.min[0]);
                    let y = b.min[1] + yy * (b.max[1] - b.min[1]);
                    let z = b.min[2] + zz * (b.max[2] - b.min[2]);
                    if !self.clip_blocks([x + xo, y, z + zo], center) {
                        hits += 1;
                    }
                    count += 1;
                    zz += zs;
                }
                yy += ys;
            }
            xx += xs;
        }
        if count == 0 { 0.0 } else { hits as f32 / count as f32 }
    }

    /// Explosion of a mob ("mob" interaction); `source` is excluded from the damage.
    pub(crate) fn mob_explode(&mut self, source: &Mob, center: [f64; 3], radius: f32) {
        // Direct source = cause = the creeper: player_explosion (message "blown up by").
        self.explode_core(
            Some(source),
            Some(Ent::Mob(source.id)),
            Some(source.id),
            center,
            radius,
            false,
            "minecraft:player_explosion",
        );
    }

    /// Explosion ("mob" interaction): author (mob held out of the list, or entity),
    /// mob excluded, fire placed (F12: ghast fireball), damage type.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn explode_core(
        &mut self,
        source: Option<&Mob>,
        by: Option<Ent>,
        exclude: Option<i32>,
        center: [f64; 3],
        radius: f32,
        fire: bool,
        ty: &str,
    ) {
        self.explode_with(source, by, exclude, center, radius, fire, ty, Blast::Mob);
    }

    /// Explosion of a primed TNT (`tnt`: its identifier, outside the blast), author of the priming if any.
    /// `used_portal`: TNT that went through a portal (the game's own calculator: the Nether portal
    /// does not resist and does not break). `wet`: TNT in water (direct source of the decorations).
    pub(crate) fn tnt_explode(
        &mut self,
        tnt: i32,
        center: [f64; 3],
        radius: f32,
        by: Option<Ent>,
        wet: bool,
        used_portal: bool,
    ) {
        self.deco.blast_wet = wet;
        #[cfg(test)]
        TNT_BLASTS.with_borrow_mut(|v| v.push((self.dim.name.clone(), used_portal)));
        self.explode_with(None, by, Some(tnt), center, radius, false, "minecraft:explosion", Blast::Tnt(used_portal));
    }

    /// Explosion according to its interaction with blocks (F15: crystal "block", dragon respawn
    /// "none").
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn explode_with(
        &mut self,
        source: Option<&Mob>,
        by: Option<Ent>,
        exclude: Option<i32>,
        center: [f64; 3],
        radius: f32,
        fire: bool,
        ty: &str,
        blast: Blast,
    ) {
        // G5-F06: projectile direct source (taken before any chain explosion).
        let direct = self.carts.blast_direct.take();
        // Direct source in water: the exploding mob, otherwise the value
        // set by the caller (TNT, projectile), consumed here.
        let direct_wet = source.map_or(self.deco.blast_wet, |m| m.in_water);
        self.deco.blast_wet = false;
        // `explode` event at the centre; creature source here, the other sources (projectile,
        // crystal, none) emit it before the call.
        if let Some(m) = source {
            let src = Some(super::recent::vibration::Src::Living(Ent::Mob(m.id)));
            self.game_event_src("minecraft:explode", center, src, None);
        }
        let (griefing, decay) = match blast {
            Blast::Mob => {
                (self.rule("minecraft:mob_griefing", true), self.rule("minecraft:mob_explosion_drop_decay", true))
            }
            Blast::Block | Blast::Respawn(_) => (true, self.rule("minecraft:block_explosion_drop_decay", true)),
            Blast::Tnt(_) | Blast::TntCart { .. } => (true, self.rule("minecraft:tnt_explosion_drop_decay", false)),
            Blast::None => (false, false),
        };
        // Dropped items, orbs, falling blocks: touched if the mob rule allows it
        // or if the explosion destroys blocks.
        let blocklike =
            self.rule("minecraft:mob_griefing", true) || matches!(blast, Blast::Block | Blast::Tnt(_) | Blast::TntCart { .. });
        let rail_safe = matches!(blast, Blast::TntCart { primed: true });
        let portal_immune = blast == Blast::Tnt(true);
        let nether_portal = self.tb().block_id("nether_portal");
        // 1. Blocks hit.
        let mut toblow: Vec<[i32; 3]> = Vec::new();
        let h = self.dim.height;
        for xx in 0..16 {
            for yy in 0..16 {
                for zz in 0..16 {
                    if !(xx == 0 || xx == 15 || yy == 0 || yy == 15 || zz == 0 || zz == 15) {
                        continue;
                    }
                    let mut xd = f64::from(xx as f32 / 15.0 * 2.0 - 1.0);
                    let mut yd = f64::from(yy as f32 / 15.0 * 2.0 - 1.0);
                    let mut zd = f64::from(zz as f32 / 15.0 * 2.0 - 1.0);
                    let d = (xd * xd + yd * yd + zd * zd).sqrt();
                    xd /= d;
                    yd /= d;
                    zd /= d;
                    let mut power = radius * (0.7 + self.mobs.rng.next_float() * 0.6);
                    let (mut xp, mut yp, mut zp) = (center[0], center[1], center[2]);
                    while power > 0.0 {
                        let p = [xp.floor() as i32, yp.floor() as i32, zp.floor() as i32];
                        if p[1] < h.min_y || p[1] >= h.min_y + h.sections as i32 * 16 {
                            break;
                        }
                        let water = match blast {
                            Blast::Respawn(Some(w)) if w == p => Some(100.0),
                            _ => None,
                        };
                        // TNT that went through a portal: the Nether portal without resistance or
                        // destruction.
                        let immune = portal_immune && self.v().block(self.st(p)) == nether_portal;
                        // G5-F06: primed TNT minecart, rails and their support spared.
                        let spared = rail_safe
                            && (self.btag_id(p, crate::tag_bits::TagId::Rails) || self.btag_id([p[0], p[1] + 1, p[2]], crate::tag_bits::TagId::Rails));
                        if !immune && let Some(r) = water.or_else(|| self.block_resistance(p)) {
                            let r = if spared { 0.0 } else { self.wither_blast_resistance(p, r) }; // F16: blue skull
                            power -= (r + 0.3) * 0.3;
                        }
                        if power > 0.0 && !immune && !spared && !toblow.contains(&p) {
                            toblow.push(p);
                        }
                        xp += xd * f64::from(0.3f32);
                        yp += yd * f64::from(0.3f32);
                        zp += zd * f64::from(0.3f32);
                        power -= 0.225_000_01;
                    }
                }
            }
        }
        // Order of a hash set of the game: the order only matters for the next shuffle.
        let count = toblow.len() as i32;
        // 2. Entities. The author held out of the list (creeper, wither being ticked) stays
        // findable by identifier for the time of the damage (warden anger, gossip…).
        if let Some(s) = source {
            self.ghost_push(s);
        }
        let double = radius * 2.0;
        let mut player_kb: Option<[f64; 3]> = None;
        let ids: Vec<i32> = self.mobs.list.iter().flatten().filter(|o| Some(o.id) != exclude).map(|o| o.id).collect();
        let mut targets: Vec<Ent> = ids.into_iter().map(Ent::Mob).collect();
        targets.push(Ent::Player);
        for t in targets {
            let (Some(pos), Some(eye), Some(bb)) = (self.ent_pos(t), self.ent_eye(t), self.ent_bbox(t)) else {
                continue;
            };
            if t == Ent::Player && self.game_mode == 3 {
                continue;
            }
            let dist = dist2(pos, center).sqrt() / f64::from(double);
            if dist > 1.0 {
                continue;
            }
            let mut dir = [pos[0] - center[0], eye - center[1], pos[2] - center[2]];
            let l = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
            dir = if l < 1.0e-4 { [0.0; 3] } else { dir.map(|v| v / l) };
            let exposure = self.seen_percent(center, &bb);
            let pow = (1.0 - dist) * f64::from(exposure);
            let dmg = ((pow * pow + pow) / 2.0 * 7.0 * f64::from(double) + 1.0) as f32;
            let kb = dir.map(|v| v * (1.0 - dist) * f64::from(exposure));
            if rc_data::env_flag!("RUSTCRAFT_S4LOG") {
                eprintln!("explosion: {t:?} at {pos:?}, distance {dist:.3}, seen part {exposure}, damage {dmg}");
            }
            match t {
                Ent::Player => {
                    // W8c: point of impact and direct cause (`fall_after_explosion`).
                    let cause = match (source, blast) {
                        (Some(s), _) => Some(s.kind.info().name.to_owned()),
                        (None, Blast::Tnt(_)) => Some("minecraft:tnt".to_owned()),
                        _ => self.prog.expl_cause_next.map(str::to_owned),
                    };
                    self.prog.expl_impact = Some((self.pos, cause));
                    // F12: author out of the list (creeper) or found again (ghast), otherwise anonymous.
                    let owner = match (source, by) {
                        (Some(s), _) => Some(s.clone()),
                        (None, Some(Ent::Mob(id))) => self.mob_ref(id).cloned(),
                        _ => None,
                    };
                    // W8c: source = the exploding entity (creeper: alive) or the centre.
                    self.prog.dmg_src = crate::item_use::DmgSrc {
                        pos: Some(center),
                        direct_living: source.is_some(),
                        attacker_pos: source.map(|s| s.pos),
                        ..Default::default()
                    };
                    match owner {
                        Some(o) => {
                            self.player_hurt_by_mob(&o, ty, dmg, Some((center[0] - pos[0], center[2] - pos[2])));
                        }
                        None => {
                            self.hurt(ty, dmg);
                        }
                    }
                    if self.game_mode != 3 && !(self.game_mode == 1 && self.flying) {
                        // W8a: explosion knockback resistance (Blast Protection).
                        // Power computed from one block then applied directionally, like the game.
                        let r = self.player_attribute("minecraft:explosion_knockback_resistance");
                        let power = (1.0 - dist) * f64::from(exposure) * (1.0 - r);
                        player_kb = Some(dir.map(|v| v * power));
                    }
                }
                Ent::Mob(id) => {
                    let Some(i) = self.mob_index(id) else { continue };
                    let mut o = self.mobs.list[i].take().expect("mob");
                    // Ghast hit by the explosion of a fireball sent back by the player: 1,000.
                    let dmg = if super::nether::reflected_fireball(&o, self.mobs.fireballs.direct_large, by) {
                        1000.0
                    } else {
                        dmg
                    };
                    self.mob_hurt(&mut o, ty, dmg, by, Some(center));
                    o.vel = [o.vel[0] + kb[0], o.vel[1] + kb[1], o.vel[2] + kb[2]];
                    o.needs_sync = true;
                    self.mobs.list[i] = Some(o);
                }
            }
        }
        // Non-living entities of blocks and loot: primed TNT, falling block, item, orb.
        self.explosion_hits_objects(center, radius, exclude, ty, by, direct_wet, blocklike);
        // G5-F06: minecarts (direct source: exploding projectile, otherwise none).
        self.with_direct(direct, |s| s.carts_explosion_hit(center, radius, exclude, ty, by));
        // Fireballs and wind charges: pushed, the author becomes their owner.
        let owner = self.blast_owner_uuid(by, source);
        self.explosion_redirect(center, radius, owner, 1.0);
        // F15: crystals (destroyed without a new explosion) and shulker bullets in the radius.
        self.end_explosion_hits(center, radius, by, ty, exclude);
        if source.is_some() {
            self.ghost_pop();
        }
        // 3. Blocks destroyed (shuffled, loot grouped by 16).
        if griefing {
            // Beehives: emergency exit if the direct source is a TNT, a creeper, a wither skull,
            // the wither or a TNT minecart; bees angered after the breaking.
            let hive_release = matches!(blast, Blast::Tnt(_) | Blast::TntCart { .. })
                || source.is_some_and(|m| matches!(m.kind, super::species::Kind::Creeper | super::species::Kind::Wither))
                || self.prog.expl_cause_next == Some("minecraft:wither_skull");
            self.mobs.rng.shuffle(&mut toblow);
            let mut stacks: Vec<([i32; 3], crate::player::Stack)> = Vec::new();
            for p in toblow.iter().copied() {
                if self.is_air(p) {
                    continue;
                }
                let s = self.st(p);
                let name = self.state_name(s).to_owned();
                self.heart_explosion_hit(p, s, ty, by);
                // TNT: no loot, a primed TNT with a short fuse takes its place.
                if name == "minecraft:tnt" {
                    let air = self.default_state("minecraft:air");
                    self.set_block(p, air, crate::blocks::flag::ALL);
                    self.tnt_was_exploded(p);
                    continue;
                }
                let hive = self.v().kind(s) == crate::blocks::Kind::Beehive;
                if hive {
                    self.hive_blown(p, hive_release);
                }
                let drops = self.explosion_drops(p, s, &name, if decay { Some(radius) } else { None });
                for d in drops {
                    let mut d = d;
                    for (_, st) in stacks.iter_mut() {
                        if crate::items::same(st, &d) {
                            let room = (16 - st.count).max(0).min(d.count);
                            st.count += room;
                            d.count -= room;
                            if d.count <= 0 {
                                break;
                            }
                        }
                    }
                    if d.count > 0 {
                        stacks.push((p, d));
                    }
                }
                let air = self.default_state("minecraft:air");
                self.container_broken(p);
                self.set_block(p, air, crate::blocks::flag::ALL);
                if hive {
                    crate::mobs::wild::hive::anger_nearby(self, p);
                }
            }
            for (p, s) in stacks {
                self.pop_resource(p, s);
            }
        }
        // F12: fire: one cell in three, empty, above a full block.
        if fire {
            for p in toblow.iter().copied() {
                if self.mobs.rng.next_int_bound(3) == 0 && self.is_air(p) && self.solid_render([p[0], p[1] - 1, p[2]]) {
                    let f = self.fire_state_for(p);
                    self.set_block(p, f, crate::blocks::flag::ALL);
                }
            }
        }
        // 4. Packet to the player.
        if dist2(self.pos, center) < 4096.0 {
            let small = radius < 2.0 || !griefing;
            let pt = |s: &Server, n: &str| Particle {
                kind: s
                    .data
                    .registry("minecraft:particle_type")
                    .and_then(|r| r.entries.iter().position(|e| e == n))
                    .unwrap_or(0) as i32,
                color: 0,
                state: 0,
                item: -1,
                scale: 1.0,
                color2: 0,
                vib: None,
            };
            let ex = rc_client::extra::Explosion {
                center,
                radius,
                blocks: count,
                knockback: player_kb,
                particle: pt(self, if small { "minecraft:explosion" } else { "minecraft:explosion_emitter" }),
                sound: SoundRef::Id(
                    self.data
                        .registry("minecraft:sound_event")
                        .and_then(|r| r.entries.iter().position(|e| e == "minecraft:entity.generic.explode"))
                        .unwrap_or(0) as i32,
                ),
                block_particles: vec![
                    (pt(self, "minecraft:poof"), 0.5, 1.0, 1),
                    (pt(self, "minecraft:smoke"), 1.0, 1.0, 1),
                ],
                play_sound: true,
            };
            let _ = self.send(Event::Explode(Box::new(ex)));
        }
        let _ = EntityEvent::Remove(Vec::new());
    }

    /// Loot of a blown-up block (block table, explosion context).
    fn explosion_drops(
        &mut self,
        p: [i32; 3],
        state: u32,
        name: &str,
        radius: Option<f32>,
    ) -> Vec<crate::player::Stack> {
        if !self.rule("minecraft:block_drops", true) {
            return Vec::new();
        }
        let props: Vec<(String, String)> =
            self.data.state_properties(state).map(|(k, v)| (k.to_owned(), v.to_owned())).collect();
        let be = self.block_entity(p).cloned();
        let table = format!("minecraft:blocks/{}", name.trim_start_matches("minecraft:"));
        let tags = &self.block_tags;
        let data = &self.data;
        let tag_fn = |tag: &str, n: &str| {
            let id = data.blocks.iter().position(|b| b.name == n).map(|i| i as u32);
            id.is_some_and(|id| tags.get(tag).is_some_and(|v| v.binary_search(&id).is_ok()))
        };
        let loot = crate::loot::Loot { pack: &self.life.pack, items: &self.life.items, block_tag: &tag_fn };
        let empty = None;
        let ctx = crate::loot::Ctx {
            block: Some((name, props)),
            tool: Some(empty),
            explosion: radius,
            block_entity: be.as_ref(),
            ..Default::default()
        };
        let seq =
            self.life.pack.get("loot_table", &table).and_then(|t| t["random_sequence"].as_str().map(str::to_owned));
        match seq {
            Some(s) => {
                let rng = self.life.seqs.get(&s);
                loot.drops(&table, &ctx, rng)
            }
            None => {
                let mut rng = self.life.rng;
                let d = loot.drops(&table, &ctx, &mut rng);
                self.life.rng = rng;
                d
            }
        }
    }

    pub(crate) fn default_state(&self, name: &str) -> u32 {
        self.data.blocks.iter().find(|b| b.name == name).map_or(0, |b| b.default_state)
    }
}
