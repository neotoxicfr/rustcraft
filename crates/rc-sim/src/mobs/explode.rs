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

/// Player behind an explosion's damage, read before any other player is installed.
pub(crate) struct BlastAuthor {
    name: String,
    display: rc_text::Component,
    named_item: Option<rc_text::Component>,
}

/// What an explosion with a player author did to one player: the hurt report (past the
/// invulnerability window) and that player's name if it died of it.
type AuthorHit = (Option<super::combat::HurtParts>, Option<String>);

#[cfg(test)]
thread_local! {
    /// Tests: primed TNT explosions of this thread, (dimension, passed through a portal).
    pub(crate) static TNT_BLASTS: std::cell::RefCell<Vec<(String, bool)>> = const { std::cell::RefCell::new(Vec::new()) };
    /// Tests: cells hit by each explosion of this thread.
    pub(crate) static BLAST_CELLS: std::cell::RefCell<Vec<BlastCells>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Tests: centre of an explosion and the cells it hit.
#[cfg(test)]
pub(crate) type BlastCells = ([f64; 3], Vec<[i32; 3]>);

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
    /// None; true with the calculator of an entity that went through a portal (its portal
    /// delay runs): the Nether portal neither resists nor counts as hit.
    None(bool),
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

    /// Explosion of a primed TNT (`tnt`: its identifier, outside the blast), author of the priming if any
    /// (then of the "player" type, like any explosion with an author and a direct entity).
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
        let ty = if by.is_some() { "minecraft:player_explosion" } else { "minecraft:explosion" };
        self.explode_with(None, by, Some(tnt), center, radius, false, ty, Blast::Tnt(used_portal));
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
        // L10-COMBAT: the projectile being resolved is the direct entity of its own explosion
        // only (fireball, skull); another exploding entity (crystal set off by an arrow, TNT,
        // minecart) is the direct entity of the damage it deals and hides it. Restored after.
        let outer_hit = if direct.is_none() { self.proj.direct_hit.take() } else { None };
        let me = self.proj.blast_next.take();
        let outer_ent = std::mem::replace(&mut self.proj.blast_ent, me);
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
            Blast::None(_) => (false, false),
        };
        // Dropped items, orbs, falling blocks: touched if the mob rule allows it
        // or if the explosion destroys blocks.
        let blocklike =
            self.rule("minecraft:mob_griefing", true) || matches!(blast, Blast::Block | Blast::Tnt(_) | Blast::TntCart { .. });
        let rail_safe = matches!(blast, Blast::TntCart { primed: true });
        let portal_immune = matches!(blast, Blast::Tnt(true) | Blast::None(true));
        let nether_portal = self.table().block_id("nether_portal");
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
                        let immune = portal_immune && self.view().block(self.st(p)) == nether_portal;
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
        #[cfg(test)]
        BLAST_CELLS.with_borrow_mut(|v| v.push((center, toblow.clone())));
        // 2. Entities. The author held out of the list (creeper, wither being ticked) stays
        // findable by identifier for the time of the damage (warden anger, gossip…).
        if let Some(s) = source {
            self.ghost_push(s);
        }
        let double = radius * 2.0;
        // A digging or emerging warden ignores explosions (neither hurt nor pushed).
        let ids: Vec<i32> = self
            .mobs
            .list
            .iter()
            .filter(|o| Some(o.id) != exclude && !super::recent::ignores_explosion(o))
            .map(|o| o.id)
            .collect();
        for id in ids {
            let t = Ent::Mob(id);
            let (Some(pos), Some(eye), Some(bb)) = (self.ent_pos(t), self.ent_eye(t), self.ent_bbox(t)) else {
                continue;
            };
            let dist = dist2(pos, center).sqrt() / f64::from(double);
            if dist > 1.0 {
                continue;
            }
            let dir = super::mth::normalize([pos[0] - center[0], eye - center[1], pos[2] - center[2]]);
            let exposure = self.seen_percent(center, &bb);
            let pow = (1.0 - dist) * f64::from(exposure);
            let dmg = ((pow * pow + pow) / 2.0 * 7.0 * f64::from(double) + 1.0) as f32;
            if rc_data::env_flag!("RUSTCRAFT_S4LOG") {
                eprintln!("explosion: {t:?} at {pos:?}, distance {dist:.3}, seen part {exposure}, damage {dmg}");
            }
            let Some(i) = self.mob_index(id) else { continue };
            let mut o = self.mobs.list.take(i).expect("mob");
            // Ghast hit by the explosion of a fireball sent back by the player: 1,000.
            let dmg = if super::nether::reflected_fireball(&o, self.mobs.fireballs.direct_large, by) { 1000.0 } else { dmg };
            self.mob_hurt(&mut o, ty, dmg, by, Some(center));
            // Knockback power, then along the direction; reduced by the explosion
            // knockback resistance (Blast Protection), like the game.
            let kp = (1.0 - dist) * f64::from(exposure) * (1.0 - o.attrs.explosion_kb_resistance.value());
            o.vel = [o.vel[0] + dir[0] * kp, o.vel[1] + dir[1] * kp, o.vel[2] + dir[2] * kp];
            o.needs_sync = true;
            self.mobs.list.put(i, o);
        }
        // The current player, then the other players of the local network: hurt and pushed like
        // the host (W8c: the direct cause is the explosion's, whoever is installed).
        let cause = match (source, blast) {
            (Some(s), _) => Some(s.kind.info().name),
            (None, Blast::Tnt(_)) => Some("minecraft:tnt"),
            _ => self.prog.expl_cause_next,
        };
        // A player author (TNT primer, crystal breaker…) read now: once another player is
        // installed, `Ent::Player` designates that one.
        let author = (by == Some(Ent::Player)).then(|| self.blast_author());
        let (player_kb, own) = self.explosion_player(source, by, ty, center, double, cause, author.as_ref());
        // The author hurt by its own explosion: `player_hurt_entity` on itself; no kill credit
        // (the game never credits a player with its own death).
        if let Some((original, taken, blocked)) = own.0 {
            let me = Some(crate::progress::pred::Who::E(crate::commands::ents::E::Player));
            self.s9_player_hurt_living(me, ty, original, taken, blocked);
        }
        let mut peers: Vec<(Option<usize>, Option<[f64; 3]>)> = Vec::new();
        for o in self.lan_peers().into_iter().filter(|o| o.game_mode != 3) {
            let (kb, hit) = self.as_player(o.who, |sv| sv.explosion_player(source, by, ty, center, double, cause, author.as_ref()));
            // Credited to the author (the current player again): hurt, then the kill.
            if let Some((original, taken, blocked)) = hit.0 {
                self.s9_player_hurt_living(None, ty, original, taken, blocked);
            }
            if let Some(victim) = hit.1 {
                self.s9_player_killed_player(&victim, ty);
            }
            peers.push((o.who, kb));
        }
        // Non-living entities of blocks and loot: primed TNT, falling block, item, orb.
        self.explosion_hits_objects(center, radius, exclude, ty, by, direct_wet, blocklike);
        // G5-F06: minecarts (direct source: exploding projectile, otherwise none).
        self.with_direct(direct, |s| s.carts_explosion_hit(center, radius, exclude, ty, by));
        // Projectiles hurt and pushed (fireballs and wind charges: the author becomes their
        // owner); displays, markers, interactions and clouds pushed.
        let owner = self.blast_owner_uuid(by, source);
        self.explosion_projectiles(center, radius, owner, 1.0, true);
        self.blast_push_still(center, radius, 1.0);
        // F15: crystals (destroyed without a new explosion) and shulker bullets in the radius.
        self.end_explosion_hits(center, radius, by, ty, exclude);
        if source.is_some() {
            self.ghost_pop();
        }
        self.proj.blast_ent = outer_ent;
        if direct.is_none() {
            self.proj.direct_hit = outer_hit;
        }
        // 3. Blocks destroyed (shuffled, loot grouped by 16).
        if griefing {
            // Beehives: emergency exit if the direct source is a TNT, a creeper, a wither skull,
            // the wither or a TNT minecart; bees angered after the breaking.
            // (A TNT interaction from a mob, the sulfur cube, does not count.)
            let hive_release = (source.is_none() && matches!(blast, Blast::Tnt(_) | Blast::TntCart { .. }))
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
                let hive = self.view().kind(s) == crate::blocks::Kind::Beehive;
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
                // Contents thrown by the removal itself (`container_broken` in `set_block`).
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
        // 4. Packet to each player within 64 blocks, carrying that player's own knockback, also
        //    given to its server velocity.
        {
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
                trail: None,
            };
            let ex = rc_client::extra::Explosion {
                center,
                radius,
                blocks: count,
                knockback: None,
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
            self.send_explosion(ex, player_kb, &peers);
        }
        let _ = EntityEvent::Remove(Vec::new());
    }

    /// The explosion reaching the installed player (spectators excluded): impact point and cause
    /// (`fall_after_explosion`), impulse grace cancelled, damage (from its author when a mob is
    /// behind it), then its knockback added to its server velocity; returned for its packet
    /// unless it flies in creative mode. `double`: twice the radius. A player `author` is refused
    /// by the `pvp` rule or a common team without friendly fire (the push stays), otherwise named
    /// as the attacker (death message, `killed_by`); then also returned: the hurt report if the
    /// hit went past the invulnerability window, and the player's name if it died of it.
    #[allow(clippy::too_many_arguments)]
    fn explosion_player(
        &mut self,
        source: Option<&Mob>,
        by: Option<Ent>,
        ty: &str,
        center: [f64; 3],
        double: f32,
        cause: Option<&'static str>,
        author: Option<&BlastAuthor>,
    ) -> (Option<[f64; 3]>, AuthorHit) {
        let none = (None, (None, None));
        if self.game_mode == 3 {
            return none;
        }
        let t = Ent::Player;
        let (Some(pos), Some(eye), Some(bb)) = (self.ent_pos(t), self.ent_eye(t), self.ent_bbox(t)) else { return none };
        let dist = dist2(pos, center).sqrt() / f64::from(double);
        if dist > 1.0 {
            return none;
        }
        let dir = super::mth::normalize([pos[0] - center[0], eye - center[1], pos[2] - center[2]]);
        let exposure = self.seen_percent(center, &bb);
        let pow = (1.0 - dist) * f64::from(exposure);
        let dmg = ((pow * pow + pow) / 2.0 * 7.0 * f64::from(double) + 1.0) as f32;
        if rc_data::env_flag!("RUSTCRAFT_S4LOG") {
            eprintln!("explosion: {t:?} at {pos:?}, distance {dist:.3}, seen part {exposure}, damage {dmg}");
        }
        self.prog.expl_impact = Some((self.pos, cause.map(str::to_owned)));
        // Ordinary explosion: the grace of an impulse in progress is cancelled.
        let at = self.pos;
        self.life.vit.impulse_from_explosion(false, at);
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
        let mut hit = (None, None);
        match (owner, author) {
            (Some(o), _) => {
                self.player_hurt_by_mob(&o, ty, dmg, Some((center[0] - pos[0], center[2] - pos[2])));
            }
            (None, Some(a)) => {
                if !self.refuses_player_damage(&a.name) {
                    self.prog.attacker_player = Some((a.display.clone(), a.named_item.clone()));
                    self.prog.hurt_parts = None;
                    let alive = !self.life.vit.dead;
                    self.hurt(ty, dmg);
                    self.prog.attacker_player = None;
                    hit = (self.prog.hurt_parts.take(), (alive && self.life.vit.dead).then(|| self.name.clone()));
                }
            }
            (None, None) => {
                self.hurt(ty, dmg);
            }
        }
        if self.game_mode == 3 {
            return (None, hit);
        }
        // W8a: explosion knockback resistance (Blast Protection). Power computed from one block
        // then applied directionally, like the game.
        let r = self.player_attribute("minecraft:explosion_knockback_resistance");
        let power = (1.0 - dist) * f64::from(exposure) * (1.0 - r);
        let kb = dir.map(|v| v * power);
        // Flying in creative: pushed on the server, nothing in its packet.
        self.add_player_vel(kb);
        ((!(self.game_mode == 1 && self.flying)).then_some(kb), hit)
    }

    /// The current player as the author of an explosion's damage: score name, displayed name, and
    /// its main-hand item when it bears a custom name (death message "… using …").
    fn blast_author(&self) -> BlastAuthor {
        let held = self.slots.get(36 + self.held as usize).and_then(Option::as_ref);
        BlastAuthor {
            name: self.name.clone(),
            display: self.player_display_name(),
            named_item: held.filter(|s| crate::items::component(s, "minecraft:custom_name").is_some()).map(|s| self.stack_display(s)),
        }
    }

    /// Explosion packet to each player within 64 blocks of its centre, carrying that player's
    /// own knockback (`own`: the current player's; `peers`: those of other players by seat,
    /// none for the others), like the game sends one per player.
    pub(crate) fn send_explosion(
        &mut self,
        mut ex: rc_client::extra::Explosion,
        own: Option<[f64; 3]>,
        peers: &[(Option<usize>, Option<[f64; 3]>)],
    ) {
        if self.muted {
            return;
        }
        let center = ex.center;
        if self.lan.is_none() {
            if dist2(self.pos, center) < 4096.0 {
                ex.knockback = own;
                let _ = self.send(Event::Explode(Box::new(ex)));
            }
            return;
        }
        if dist2(self.pos, center) < 4096.0 {
            let mut mine = ex.clone();
            mine.knockback = own;
            let _ = self.send_to_current(Event::Explode(Box::new(mine)));
        }
        for (who, _) in self.lan_other_players() {
            let mut theirs = ex.clone();
            theirs.knockback = peers.iter().find(|p| p.0 == who).and_then(|p| p.1);
            self.as_player(who, |sv| {
                if dist2(sv.pos, center) < 4096.0 {
                    let _ = sv.send_to_current(Event::Explode(Box::new(theirs)));
                }
            });
        }
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
