//! "Trap" skeleton horse: spawning during a thunderstorm (draw per chunk), triggered
//! when a player comes within 10 blocks (visual lightning, four horses ridden by helmeted
//! skeletons); mounts of monsters on natural spawning (zombie horse and its spear
//! zombie, camel husk, husk and parched).

use rc_client::{EntityEvent, Event};
use rc_proto_v777::entity::AddEntity;

use super::super::jrand::JRand;
use super::super::mob::{Ent, Mob};
use super::super::spawn::{Group, Reason};
use super::super::species::Kind;
use super::handles;
use crate::player::Stack;
use crate::server::Server;

/// Lightning in progress (the client draws it and plays its sounds): id, life, flashes; a
/// "visual" lightning (skeleton horse trap) neither burns nor strikes anything.
#[derive(Debug, Clone)]
pub struct Bolt {
    pub id: i32,
    pub uuid: u128,
    life: i32,
    flashes: i32,
    rng: JRand,
    pos: [f64; 3],
    visual: bool,
    /// Thrown by the player (channeling): `channeled_lightning` progress on every strike.
    cause: bool,
    /// Cells set on fire and entities struck.
    fires: i32,
    hit: Vec<Ent>,
}

impl Server {
    /// Rain in the world (rain level above 0.2).
    pub(crate) fn world_raining(&self) -> bool {
        self.bs_raining()
    }

    /// Thunder in the world (effective thunder, rain included, above 0.9).
    pub(crate) fn world_thundering(&self) -> bool {
        self.can_have_weather() && self.life.weather.thunder_eff() > 0.9
    }

    /// Thunderstorm on a spawn chunk: 1 chance in 100,000 per tick;
    /// lightning target; trap if the rain falls there, spawns are allowed and
    /// the draw passes under the effective difficulty × 0.01 (no lightning rod below); otherwise
    /// a real lightning (fire, damage, conversions, lightning rod).
    pub(crate) fn thunder_chunk_tick(&mut self, cx: i32, cz: i32) {
        if !(self.world_raining() && self.world_thundering()) || self.bs.rng.next_int_bound(100_000) != 0 {
            return;
        }
        self.bs.lcg = self.bs.lcg.wrapping_mul(3).wrapping_add(1_013_904_223);
        let v = self.bs.lcg >> 2;
        let (x, z) = ((cx << 4) + (v & 15), (cz << 4) + ((v >> 8) & 15));
        let p = self.lightning_target_around(x, z);
        if !self.is_raining_at(p) {
            return;
        }
        let (eff, _) = self.local_difficulty(p);
        let trap = self.rule("minecraft:spawn_mobs", true)
            && self.bs.rng.next_double() < f64::from(eff) * 0.01
            && !self.bname([p[0], p[1] - 1, p[2]]).ends_with("lightning_rod");
        if trap {
            let mut h = self.new_mob(Kind::SkeletonHorse, p.map(f64::from), 0.0);
            h.mount.trap = true;
            Self::add_trap_goal(&mut h);
            h.age = 0;
            self.add_mob(h);
        }
        let at = [f64::from(p[0]) + 0.5, f64::from(p[1]), f64::from(p[2]) + 0.5];
        self.add_bolt(at, trap, false);
    }

    /// Closest lightning rod (128 blocks, top of its
    /// column) otherwise a living entity under the sky in the column (±3), spectators included,
    /// otherwise the "motion blocking" surface.
    pub(crate) fn lightning_target_around(&mut self, x: i32, z: i32) -> [i32; 3] {
        let mut center = [x, self.surface_y(x, z, 4), z];
        if let Some(rod) = self.lightning_rod_near(center) {
            return [rod[0], rod[1] + 1, rod[2]];
        }
        let max_y = self.dim.height.min_y + self.dim.height.sections as i32 * 16 - 1;
        let col = rc_physics::Aabb {
            min: [f64::from(x) - 3.0, f64::from(center[1]) - 3.0, f64::from(z) - 3.0],
            max: [f64::from(x) + 4.0, f64::from(max_y) + 5.0, f64::from(z) + 4.0],
        };
        let mut targets: Vec<[i32; 3]> = self
            .mobs
            .list
            .iter()
            .flatten()
            .filter(|m| m.alive() && m.touches(&col))
            .map(|m| m.block_pos())
            .filter(|b| self.can_see_sky(*b))
            .collect();
        if !self.life.vit.dead && self.player_aabb().intersects(&col) {
            let b = super::super::mth::block_of(self.pos);
            if self.can_see_sky(b) {
                targets.push(b);
            }
        }
        // The other LAN players are candidates too.
        let others: Vec<[i32; 3]> = self
            .lan_others()
            .filter(|c| !c.vit.dead && crate::play::player_box(c.pos, c.sneaking).intersects(&col))
            .map(|c| super::super::mth::block_of(c.pos))
            .collect();
        targets.extend(others.into_iter().filter(|b| self.can_see_sky(*b)));
        if !targets.is_empty() {
            let i = self.bs.rng.next_int_bound(targets.len() as i32) as usize;
            return targets[i];
        }
        if center[1] == self.dim.height.min_y - 1 {
            center[1] += 2;
        }
        center
    }

    /// Closest "lightning rod" POI within 128 blocks whose block is the
    /// top of its column ("world surface" map − 1).
    fn lightning_rod_near(&mut self, center: [i32; 3]) -> Option<[i32; 3]> {
        use crate::poi::{Occupancy, PoiType};
        let list = self.poi_in_range(|t| t == PoiType::LightningRod, center, 128, Occupancy::Any);
        let mut best: Option<(i64, [i32; 3])> = None;
        for r in list {
            if r.pos[1] != self.surface_y(r.pos[0], r.pos[2], 1) - 1 {
                continue;
            }
            let d = crate::poi::dist2i(r.pos, center);
            if best.is_none_or(|b| d < b.0) {
                best = Some((d, r.pos));
            }
        }
        best.map(|b| b.1)
    }

    /// Visual lightning (skeleton horse trap).
    pub(crate) fn spawn_bolt(&mut self, pos: [f64; 3]) {
        self.add_bolt(pos, true, false);
    }

    /// Real lightning (channeling: `cause` = thrown by the player).
    pub(crate) fn spawn_lightning(&mut self, pos: [f64; 3], cause: bool) {
        self.add_bolt(pos, false, cause);
    }

    /// Lightning entity sent to the client, ticked by `bolts_tick`, removed at end of life.
    fn add_bolt(&mut self, pos: [f64; 3], visual: bool, cause: bool) {
        self.next_entity += 1;
        let id = self.next_entity;
        let mut rng = JRand::from_time(i64::from(id));
        let _seed = rng.next_long();
        let flashes = rng.next_int_bound(3) + 1;
        let uuid = (id as u128) << 64 | 0x4200_0000_0000_0000;
        let add = AddEntity {
            id,
            uuid,
            kind: self.entity_kind_id("minecraft:lightning_bolt"),
            pos,
            velocity: [0.0; 3],
            pitch: 0.0,
            yaw: 0.0,
            head_yaw: 0.0,
            data: 0,
        };
        let _ = self.send(Event::Entity(EntityEvent::Add(Box::new(add))));
        self.mobs.mounts.bolts.push(Bolt { id, uuid, life: 2, flashes, rng, pos, visual, cause, fires: 0, hit: Vec::new() });
    }

    /// Life of the lightning bolts: at the first tick fire (normal and hard),
    /// lightning rod, scraped copper, `lightning_strike` event; while life is positive,
    /// all entities in the 6×12×6 box are struck; removed once the flashes are used up.
    pub(crate) fn bolts_tick(&mut self) {
        let mut bolts = std::mem::take(&mut self.mobs.mounts.bolts);
        let mut gone = Vec::new();
        for b in &mut bolts {
            if b.life == 2 {
                if self.level.difficulty_id() >= 2 {
                    self.bolt_fire(b, 4);
                }
                let strike = [b.pos[0].floor() as i32, (b.pos[1] - 1.0e-6).floor() as i32, b.pos[2].floor() as i32];
                self.power_lightning_rod(strike);
                self.clear_copper_on_strike(strike);
                let src = crate::mobs::recent::vibration::Src::Other;
                self.game_event_src("minecraft:lightning_strike", b.pos, Some(src), None);
            }
            b.life -= 1;
            if b.life < 0 {
                if b.flashes == 0 {
                    gone.push(b.id);
                    self.bolt_vanish_trigger(b);
                } else if b.life < -b.rng.next_int_bound(10) {
                    b.flashes -= 1;
                    b.life = 1;
                    let _seed = b.rng.next_long();
                    self.bolt_fire(b, 0);
                }
            }
            if b.life >= 0 && !b.visual {
                self.bolt_strike_entities(b);
            }
        }
        bolts.append(&mut self.mobs.mounts.bolts);
        self.mobs.mounts.bolts = bolts;
        if !gone.is_empty() {
            self.mobs.mounts.bolts.retain(|b| !gone.contains(&b.id));
            let _ = self.send(Event::Entity(EntityEvent::Remove(gone)));
        }
    }

    /// Fire on the lightning's cell then `extra` neighboring cells drawn (±1), if
    /// air is there and fire holds there (spread radius rule around the player).
    fn bolt_fire(&mut self, b: &mut Bolt, extra: i32) {
        if b.visual {
            return;
        }
        let p = b.pos.map(|v| v.floor() as i32);
        if !self.can_spread_fire_around(p) {
            return;
        }
        let place = |s: &mut Server, q: [i32; 3]| {
            let fire = s.fire_state_for(q);
            if s.is_air(q) && s.can_survive_fire(fire, q) {
                s.set_update(q, fire);
                return 1;
            }
            0
        };
        b.fires += place(self, p);
        for _ in 0..extra {
            let d = [b.rng.next_int_bound(3) - 1, b.rng.next_int_bound(3) - 1, b.rng.next_int_bound(3) - 1];
            b.fires += place(self, [p[0] + d[0], p[1] + d[1], p[2] + d[2]]);
        }
    }

    /// Lightning disappearance: `lightning_strike` criterion of the player within 256 blocks, with
    /// as witnesses the living entities of the ±15 box (height +6) that were not struck.
    fn bolt_vanish_trigger(&mut self, b: &Bolt) {
        let area = rc_physics::Aabb {
            min: [b.pos[0] - 15.0, b.pos[1] - 15.0, b.pos[2] - 15.0],
            max: [b.pos[0] + 15.0, b.pos[1] + 21.0, b.pos[2] + 15.0],
        };
        let mut viewers: Vec<Ent> = self
            .mobs
            .list
            .iter()
            .flatten()
            .filter(|m| m.alive() && m.touches(&area) && !b.hit.contains(&Ent::Mob(m.id)))
            .map(|m| Ent::Mob(m.id))
            .collect();
        if !self.life.vit.dead && self.player_aabb().intersects(&area) && !b.hit.contains(&Ent::Player) {
            viewers.push(Ent::Player);
        }
        self.trig_lightning_strike(b.pos, b.fires, &b.hit, &viewers);
    }

    /// Lightning rod struck: powered, neighbors notified, extinguished 8 ticks later,
    /// sparks (event 3002, rod axis).
    fn power_lightning_rod(&mut self, p: [i32; 3]) {
        let s = self.get(p);
        if !self.bname(p).ends_with("lightning_rod") {
            return;
        }
        let ns = self.v().set_bool(s, "powered", true);
        self.set_update(p, ns);
        let facing = self.v().facing(s);
        let block = self.v().block(s);
        self.update_neighbors_at(crate::blocks::rel(p, facing.opposite()), block);
        self.schedule_tick(p, block, 8);
        self.level_event(3002, p, i32::from(facing.axis()));
    }

    /// Scheduled tick of a lightning rod: extinguishing. True if the block was one.
    pub(crate) fn lightning_rod_tick(&mut self, s: u32, p: [i32; 3]) -> bool {
        if !self.state_name(s).ends_with("lightning_rod") {
            return false;
        }
        let ns = self.v().set_bool(s, "powered", false);
        self.set_update(p, ns);
        let facing = self.v().facing(s);
        let block = self.v().block(s);
        self.update_neighbors_at(crate::blocks::rel(p, facing.opposite()), block);
        true
    }

    /// Lightning rod placed (block other than before) already powered, without a scheduled extinguish for
    /// it: extinguishing in 8 ticks.
    pub(crate) fn lightning_rod_on_place(&mut self, s: u32, p: [i32; 3], old: u32) {
        let block = self.v().block(s);
        if block == self.v().block(old) || !self.state_name(s).ends_with("lightning_rod") {
            return;
        }
        if self.v().get_bool(s, "powered") && !self.has_scheduled_tick(p, block) {
            self.schedule_tick(p, block, 8);
        }
    }

    /// Powered lightning rod removed: its neighbors are notified. True if it was one.
    pub(crate) fn lightning_rod_removed(&mut self, old: u32, p: [i32; 3]) -> bool {
        if !self.state_name(old).ends_with("lightning_rod") {
            return false;
        }
        if self.v().get_bool(old, "powered") {
            let facing = self.v().facing(old);
            let block = self.v().block(old);
            self.update_neighbors_at(crate::blocks::rel(p, facing.opposite()), block);
        }
        true
    }

    /// Copper struck: a block that oxidizes goes back to new, a waxed block is only the starting
    /// point; then 3 to 5 random walks (1 to 8 steps) remove one oxidation stage
    /// from neighboring copper blocks (sparks 3002 at each step).
    fn clear_copper_on_strike(&mut self, p: [i32; 3]) {
        let name = self.bname(p).trim_start_matches("minecraft:").to_owned();
        let weathering = copper_family(&name).is_some() && !name.starts_with("waxed_");
        let waxed = name.strip_prefix("waxed_").is_some_and(|n| copper_family(n).is_some());
        if !weathering && !waxed {
            return;
        }
        if weathering {
            let s = self.get(p);
            if let Some(first) = self.copper_variant(s, 0) {
                self.set_update(p, first);
            }
        }
        let strikes = self.bs.rng.next_int_bound(3) + 3;
        for _ in 0..strikes {
            let steps = self.bs.rng.next_int_bound(8) + 1;
            let mut at = p;
            for _ in 0..steps {
                let mut found = None;
                for _ in 0..10 {
                    let c = [
                        at[0] - 1 + self.bs.rng.next_int_bound(3),
                        at[1] - 1 + self.bs.rng.next_int_bound(3),
                        at[2] - 1 + self.bs.rng.next_int_bound(3),
                    ];
                    let n = self.bname(c).trim_start_matches("minecraft:").to_owned();
                    if let Some((_, stage)) = copper_family(&n).filter(|_| !n.starts_with("waxed_")) {
                        if stage > 0 {
                            let s = self.get(c);
                            if let Some(prev) = self.copper_variant(s, stage - 1) {
                                self.set_update(c, prev);
                            }
                        }
                        self.level_event(3002, c, -1);
                        found = Some(c);
                        break;
                    }
                }
                let Some(c) = found else { break };
                at = c;
            }
        }
    }

    /// Same copper block at oxidation stage `stage` (0 new … 3 oxidized), properties kept.
    fn copper_variant(&self, s: u32, stage: u8) -> Option<u32> {
        let name = self.state_name(s).trim_start_matches("minecraft:").to_owned();
        let (base, _) = copper_family(&name)?;
        let target = match (stage, base.as_str()) {
            (0, "copper") => "copper_block".to_owned(),
            (0, b) => b.to_owned(),
            (k, b) => format!("{}_{b}", ["", "exposed", "weathered", "oxidized"][k as usize]),
        };
        let id = self.bs.t.block_id(&target);
        if id == u16::MAX {
            return None;
        }
        let first = self.data.blocks[self.v().block(s) as usize].first_state;
        Some(self.data.blocks[id as usize].first_state + (s - first))
    }

    /// Lightning strike on all living entities of its box.
    fn bolt_strike_entities(&mut self, b: &mut Bolt) {
        let area = rc_physics::Aabb {
            min: [b.pos[0] - 3.0, b.pos[1] - 3.0, b.pos[2] - 3.0],
            max: [b.pos[0] + 3.0, b.pos[1] + 9.0, b.pos[2] + 3.0],
        };
        let mut hit: Vec<Ent> = Vec::new();
        let n = self.mobs.list.len();
        for i in 0..n {
            let inside = self.mobs.list[i].as_ref().is_some_and(|m| m.alive() && m.touches(&area));
            if !inside {
                continue;
            }
            let Some(mut m) = self.mobs.list[i].take() else { continue };
            hit.push(Ent::Mob(m.id));
            super::super::surface::thunder_hit(self, &mut m, b.uuid);
            self.mobs.list[i] = Some(m);
        }
        if !self.life.vit.dead && self.player_aabb().intersects(&area) {
            hit.push(Ent::Player);
            self.player_thunder_hit();
        }
        let items: Vec<i32> = self
            .life
            .drops
            .iter()
            .filter(|d| d.health > 0 && Mob::bbox_at(d.pos, 0.25, 0.25).intersects(&area))
            .map(|d| d.id)
            .collect();
        for id in items {
            self.item_hurt(id, "minecraft:lightning_bolt", 5.0, false);
        }
        let orbs: Vec<i32> = self
            .life
            .orbs
            .iter()
            .filter(|o| o.health > 0 && Mob::bbox_at(o.pos, 0.5, 0.5).intersects(&area))
            .map(|o| o.id)
            .collect();
        for id in orbs {
            self.orb_hurt(id, 5.0);
        }
        for e in &hit {
            if !b.hit.contains(e) {
                b.hit.push(*e);
            }
        }
        if b.cause {
            self.s8_channeled(&hit);
        }
    }

    /// Common mob strike: one more fire tick, 8 s of fire if it had just
    /// gone out (duration × `burning_time`, freeze cleared), 5 points of "lightning" damage.
    pub(crate) fn mob_thunder_hit_default(&mut self, m: &mut Mob) {
        m.fire += 1;
        if m.fire == 0 {
            let k = m.attrs.by_name("minecraft:burning_time").map_or(1.0, |a| a.value());
            let t = (160.0 * k).ceil() as i32;
            if m.fire < t {
                m.fire = t;
            }
            m.ticks_frozen = 0;
        }
        self.mob_hurt(m, "minecraft:lightning_bolt", 5.0, None, None);
    }

    /// Common strike of the current player.
    fn player_thunder_hit(&mut self) {
        self.life.vit.fire += 1;
        if self.life.vit.fire == 0 {
            self.player_ignite(160);
        }
        self.hurt("minecraft:lightning_bolt", 5.0);
    }

    /// The trap closes on the player.
    pub(crate) fn trap_trigger(&mut self, h: &mut Mob) {
        let p = h.block_pos();
        h.mount.trap = false;
        h.mount.set_flag(super::FLAG_TAME, true);
        h.age = 0;
        self.spawn_bolt(h.pos);
        let mut sk = self.trap_skeleton(h);
        self.mob_start_riding(&mut sk, h, false);
        self.add_mob(sk);
        for _ in 0..3 {
            let mut o = self.new_mob(Kind::SkeletonHorse, h.pos, 0.0);
            let mut g = Group::default();
            self.finalize_spawn(&mut o, Reason::SpawnEgg, &mut g);
            o.pos = h.pos;
            o.damage_cooldown = 60;
            o.persistent = true;
            o.mount.set_flag(super::FLAG_TAME, true);
            o.age = 0;
            let mut s2 = self.trap_skeleton(&o);
            self.mob_start_riding(&mut s2, &mut o, false);
            let push = [h.rng.triangle(0.0, 1.1485), 0.0, h.rng.triangle(0.0, 1.1485)];
            o.vel = [o.vel[0] + push[0], o.vel[1], o.vel[2] + push[2]];
            o.needs_sync = true;
            self.add_mob(o);
            self.add_mob(s2);
        }
        let _ = p;
    }

    /// Trap skeleton: normal equipment, persistent, invulnerable 60 ticks, iron helmet
    /// if it has none; weapon and helmet disenchanted then enchanted by the provider
    /// `mob_spawn_equipment` (horse's local difficulty, skeleton's random).
    fn trap_skeleton(&mut self, horse: &Mob) -> Mob {
        let mut s = self.new_mob(Kind::Skeleton, horse.pos, 0.0);
        let mut g = Group::default();
        self.finalize_spawn(&mut s, Reason::SpawnEgg, &mut g);
        s.pos = horse.pos;
        s.damage_cooldown = 60;
        s.persistent = true;
        if s.equipment[5].is_none() {
            s.equipment[5] = Some(Stack { id: "minecraft:iron_helmet".into(), count: 1, components: None });
        }
        let special = self.local_difficulty(horse.block_pos()).1;
        for slot in [0usize, 5] {
            let Some(mut st) = s.equipment[slot].take() else { continue };
            crate::enchanting::set_enchants(&self.life.items, &mut st, &[]);
            let mut rng = s.rng;
            let st = super::super::spawn::mob_spawn_enchant(&self.life.pack, &self.life.items, st, special, &mut rng);
            s.rng = rng;
            s.equipment[slot] = Some(st);
        }
        s
    }

    /// Finalization specific to mounts, before the spawn finalization of the mobs that grow up.
    pub(crate) fn mount_finalize(&mut self, m: &mut Mob, reason: Reason, group: &mut Group) {
        if !handles(m.kind) {
            return;
        }
        match m.kind {
            Kind::Horse => {
                let coat = match group.mount_variant {
                    Some(v) => v,
                    None => {
                        let v = self.mobs.rng.next_int_bound(7);
                        group.mount_variant = Some(v);
                        group.baby_chance = Some(0.05);
                        v
                    }
                };
                let marks = self.mobs.rng.next_int_bound(5);
                m.variant = coat & 0xFF | (marks << 8) & 0xFF00;
            }
            Kind::Llama | Kind::TraderLlama => {
                let max = if self.mobs.rng.next_float() < 0.04 { 5 } else { 3 };
                m.mount.strength = (1 + self.mobs.rng.next_int_bound(max)).clamp(1, 5);
                let v = match group.mount_variant {
                    Some(v) => v,
                    _ => {
                        let v = self.mobs.rng.next_int_bound(4);
                        group.mount_variant = Some(v);
                        group.baby_chance = Some(0.05);
                        v
                    }
                };
                m.variant = v;
                super::create_inventory(m);
            }
            Kind::ZombieHorse if reason == Reason::Natural => self.zombie_horse_jockey(m),
            Kind::Camel | Kind::CamelHusk => {
                m.mount.last_pose_tick = (self.time - 52 - 1).max(0);
                m.mount.camel_sitting_pose = false;
            }
            _ => {}
        }
        // Default group with 20% babies.
        if group.baby_chance.is_none() {
            group.baby_chance = Some(0.2);
        }
        let mut r = std::mem::replace(&mut self.mobs.rng, JRand::new(0));
        super::randomize_attributes(m, &mut r);
        self.mobs.rng = r;
    }

    /// Natural zombie horse: a zombie (finalized first) with an iron spear rides it.
    fn zombie_horse_jockey(&mut self, h: &mut Mob) {
        let mut z = self.new_mob(Kind::Zombie, h.pos, h.yaw);
        z.pitch = 0.0;
        let mut g = Group::default();
        self.finalize_spawn(&mut z, Reason::Natural, &mut g);
        z.equipment[0] = Some(Stack { id: "minecraft:iron_spear".into(), count: 1, components: None });
        self.mob_start_riding(&mut z, h, false);
        self.add_mob(z);
    }

    /// Natural husk (10% if the space is free): camel husk ridden, parched at the back.
    pub(crate) fn husk_camel_jockey(&mut self, husk: &mut Mob) {
        let mut c = self.new_mob(Kind::CamelHusk, husk.pos, 0.0);
        let mut g = Group::default();
        self.finalize_spawn(&mut c, Reason::Natural, &mut g);
        self.mob_start_riding(husk, &mut c, true);
        let mut p = self.new_mob(Kind::Parched, husk.pos, husk.yaw);
        p.pitch = 0.0;
        let mut g = Group::default();
        self.finalize_spawn(&mut p, Reason::Natural, &mut g);
        self.mob_start_riding(&mut p, &mut c, false);
        self.add_mob(c);
        self.add_mob(p);
    }
}

/// Copper family that oxidizes (name without oxidation prefix, stage 0..3), if the block has
/// one: the "exposed" entry of the family must exist (ores, raw copper, torches excluded).
fn copper_family(name: &str) -> Option<(String, u8)> {
    let name = name.strip_prefix("waxed_").unwrap_or(name);
    let (base, stage) = if let Some(r) = name.strip_prefix("exposed_") {
        (r, 1)
    } else if let Some(r) = name.strip_prefix("weathered_") {
        (r, 2)
    } else if let Some(r) = name.strip_prefix("oxidized_") {
        (r, 3)
    } else if name == "copper_block" {
        ("copper", 0)
    } else {
        (name, 0)
    };
    COPPER_FAMILIES.contains(&base).then(|| (base.to_owned(), stage))
}

/// Wave W9 on a copy of the test world (`RUSTCRAFT_SOLO_WORLD`, zone 200..231): real
/// lightning, lightning rod, copper, bed (monsters, obstruction, rain), thunder darkening,
/// inhabited time. World absent: tests skipped.
/// Oxidizable copper families of the game (base name without prefix; `copper` = copper block).
const COPPER_FAMILIES: &[&str] = &[
    "copper",
    "cut_copper",
    "cut_copper_stairs",
    "cut_copper_slab",
    "chiseled_copper",
    "copper_door",
    "copper_trapdoor",
    "copper_grate",
    "copper_bulb",
    "copper_chest",
    "copper_golem_statue",
    "copper_bars",
    "copper_chain",
    "copper_lantern",
    "lightning_rod",
];

#[cfg(test)]
mod w9_tests {
    use super::super::super::mob::Ent;
    use super::super::super::species::Kind;
    use crate::blocks::probe::{Probe, state_string};

    fn probe(tag: &str) -> Option<(Probe, std::path::PathBuf)> {
        let (world, data) = (crate::tests::test_world()?, rc_testdata::version_data()?);
        let dir = super::super::super::tests::prep(&world, tag, 0, "normal", &[]);
        let mut p = Probe::open(&dir, std::sync::Arc::new(data));
        p.run("fill 200 -60 200 231 -60 231 minecraft:stone\nfill 200 -59 200 231 -40 231 minecraft:air\nplayer 216.5 -59 202.5 0 0\n");
        if state_string(&p.s, p.s.get([216, -60, 216])) != "minecraft:stone" {
            eprintln!("zone 200..231 missing from the copy");
            return None;
        }
        p.s.game_mode = 0;
        p.s.loaded = true;
        p.s.view = 8;
        Some((p, dir))
    }

    fn mob(p: &mut Probe, k: Kind, pos: [f64; 3]) -> i32 {
        let mut m = p.s.new_mob(k, pos, 0.0);
        m.persistent = true;
        let id = m.id;
        p.s.add_mob(m);
        id
    }

    fn finish(p: Probe, dir: std::path::PathBuf) {
        drop(p);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn step(p: &mut Probe, n: i32) {
        for _ in 0..n {
            p.tick();
            p.s.bolts_tick();
        }
    }

    /// A real lightning: charged creeper, pig turned into a zombified piglin, fire placed, struck copper
    /// restored to new; a visual lightning does none of this.
    #[test]
    fn real_lightning_effects() {
        let Some((mut p, dir)) = probe("w9bolt") else { return };
        p.run("setblock 205 -60 205 minecraft:oxidized_copper\n");
        let creeper = mob(&mut p, Kind::Creeper, [210.5, -59.0, 210.5]);
        let pig = mob(&mut p, Kind::Pig, [211.5, -59.0, 210.5]);
        // Visual lightning next to it: nothing.
        p.s.spawn_bolt([210.5, -59.0, 210.5]);
        step(&mut p, 30);
        assert!(!p.s.mob_ref(creeper).unwrap().powered, "visual lightning: charged creeper");
        assert_eq!(state_string(&p.s, p.s.get([210, -59, 210])), "minecraft:air", "visual lightning: fire");
        p.s.spawn_lightning([210.5, -59.0, 210.5], false);
        step(&mut p, 3);
        assert!(p.s.mob_ref(creeper).is_none_or(|m| m.powered), "charged creeper");
        assert!(p.s.mob_ref(pig).is_none_or(|m| m.removed), "pig converted");
        let zp = p.s.mobs.list.iter().flatten().filter(|m| m.kind == Kind::ZombifiedPiglin).count();
        assert_eq!(zp, 1, "a zombified piglin");
        assert!(state_string(&p.s, p.s.get([210, -59, 210])).starts_with("minecraft:fire"), "fire under the lightning");
        p.s.spawn_lightning([205.5, -59.0, 205.5], false);
        step(&mut p, 2);
        assert_eq!(state_string(&p.s, p.s.get([205, -60, 205])), "minecraft:copper_block", "scraped copper");
        let _ = Ent::Player;
        finish(p, dir);
    }

    /// Lightning rod at the top of its column: thunder target within 128 blocks, powered by
    /// the lightning (signal 15), extinguished 8 ticks later; no fire on the rod.
    #[test]
    fn lightning_rod_target_and_power() {
        let Some((mut p, dir)) = probe("w9rod") else { return };
        p.run("setblock 220 -59 220 minecraft:exposed_lightning_rod\n");
        p.tick();
        assert_eq!(p.s.lightning_target_around(202, 202), [220, -58, 220], "target = top of the lightning rod");
        p.s.spawn_lightning([220.5, -58.0, 220.5], false);
        step(&mut p, 1);
        let s = state_string(&p.s, p.s.get([220, -59, 220]));
        assert!(s.contains("powered=true"), "powered lightning rod: {s}");
        step(&mut p, 10);
        let s = state_string(&p.s, p.s.get([220, -59, 220]));
        assert!(s.contains("powered=false"), "extinguished lightning rod: {s}");
        finish(p, dir);
    }

    /// Bed: a zombie 8 blocks away prevents sleeping, not a departed zombie; a solid block above
    /// obstructs it; in rain one goes to bed earlier (12,200); at noon under the thunderstorm, mobs
    /// see a sky darkened by 5 (not 7).
    #[test]
    fn bed_rules() {
        let Some((mut p, dir)) = probe("w9bed") else { return };
        p.run("setblock 216 -59 210 minecraft:red_bed[facing=south,part=foot]\nsetblock 216 -59 211 minecraft:red_bed[facing=south,part=head]\nplayer 216.5 -59 208.5 0 0\n");
        for c in &mut p.s.clocks {
            if c.0 == "minecraft:overworld" {
                c.1 = 18000;
            }
        }
        let z = mob(&mut p, Kind::Zombie, [222.5, -59.0, 215.5]);
        assert!(p.s.use_bed([216, -59, 210]));
        assert!(p.s.life.vit.sleeping.is_none(), "nearby zombie: no sleep");
        p.s.mobs.list.iter_mut().flatten().filter(|m| m.id == z).for_each(|m| m.removed = true);
        p.run("setblock 216 -58 211 minecraft:stone\n");
        assert!(p.s.use_bed([216, -59, 210]));
        assert!(p.s.life.vit.sleeping.is_none(), "obstructed bed: no sleep");
        p.run("setblock 216 -58 211 minecraft:air\n");
        assert!(p.s.use_bed([216, -59, 210]));
        assert_eq!(p.s.life.vit.sleeping, Some([216, -59, 211]), "night, nothing around: sleep");
        p.s.wake_up();
        p.run("player 216.5 -59 208.5 0 0\n");
        for c in &mut p.s.clocks {
            if c.0 == "minecraft:overworld" {
                c.1 = 12200;
            }
        }
        assert!(!p.s.is_dark_outside(), "12,200 without rain: day");
        p.s.life.weather.rain_level = 1.0;
        assert!(p.s.is_dark_outside(), "12,200 in the rain: dark");
        for c in &mut p.s.clocks {
            if c.0 == "minecraft:overworld" {
                c.1 = 6000;
            }
        }
        p.s.life.weather.thunder_level = 1.0;
        assert_eq!(p.s.mob_sky_darken(), 5, "noon, full thunderstorm");
        finish(p, dir);
    }

    /// New world (seed 42): chunk generation places the biome's animals. With
    /// `RUSTCRAFT_W9_KEEP`, the world is kept (comparison with an official server).
    #[test]
    fn chunk_generation_places_animals() {
        use std::time::{Duration, Instant};
        let Some(data) = rc_testdata::version_data() else { return };
        let data = std::sync::Arc::new(data);
        if crate::registries::client_jar(&data).is_none() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("rc-w9-animals-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut w = crate::newworld::NewWorld::new("animals");
        w.seed = 42;
        crate::newworld::create_world(&dir, &data, &w).expect("creation");
        let opts = crate::Options {
            world: dir.clone(),
            name: "AgentW9".into(),
            uuid: rc_client::offline_uuid("AgentW9"),
            info: rc_proto_v777::play::ClientInformation { view_distance: 6, ..Default::default() },
            sim_distance: 12,
        };
        crate::tests::solo_slot();
        let (session, handle) = crate::start(opts, data.clone()).expect("solo");
        let t = Instant::now();
        let mut adds = 0;
        while t.elapsed() < Duration::from_secs(12) {
            let Ok(e) = session.events.recv_timeout(Duration::from_millis(100)) else { continue };
            match e {
                rc_client::Event::Teleport(p) => {
                    session.link.send_accept_teleport(p.id, p.pos, p.yaw, p.pitch).unwrap();
                    session.link.send_player_loaded().unwrap();
                }
                rc_client::Event::Entity(rc_client::EntityEvent::Add(_)) => adds += 1,
                rc_client::Event::Disconnected(r) => panic!("{r}"),
                _ => {}
            }
        }
        session.link.disconnect();
        handle.wait();
        eprintln!("entities announced: {adds}; world {}", dir.display());
        assert!(adds >= 20, "too few animals: {adds}");
        if !rc_data::env_flag!("RUSTCRAFT_W9_KEEP") {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// Survey (without tick) of the animals placed at the generation of chunks −6..5 of a new world
    /// of seed 42, written to `RUSTCRAFT_W9_DUMP` ("id x y z" per line) for comparison
    /// with a frozen official server (`tick freeze`, `forceload`). Without the variable: skipped.
    #[test]
    fn dump_generation_animals() {
        use std::time::{Duration, Instant};
        let Some(out) = rc_data::env_os!("RUSTCRAFT_W9_DUMP") else { return };
        let Some(data) = rc_testdata::version_data() else { return };
        let data = std::sync::Arc::new(data);
        let dir = std::env::temp_dir().join(format!("rc-w9-dump-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut w = crate::newworld::NewWorld::new("survey");
        w.seed = 42;
        crate::newworld::create_world(&dir, &data, &w).expect("creation");
        let mut p = Probe::open(&dir, data);
        let want: Vec<(i32, i32)> = (-6..=5).flat_map(|x| (-6..=5).map(move |z| (x, z))).collect();
        p.s.ensure_loaded(&want);
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(180) {
            p.s.poll_gen();
            if want.iter().all(|c| p.s.chunks.get(c).is_some_and(Option::is_some)) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let mut lines: Vec<String> =
            p.s.mobs
                .list
                .iter()
                .flatten()
                .map(|m| format!("{} {:.3} {:.3} {:.3}", m.kind.info().name, m.pos[0], m.pos[1], m.pos[2]))
                .collect();
        lines.sort();
        std::fs::write(out, lines.join("\n") + "\n").unwrap();
        drop(p);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Beds placed in a grid (every 16 blocks, ±48 around 216, 202): village all around,
    /// occupied beds (tickets taken).
    fn village(p: &mut Probe) {
        let mut script = String::new();
        let mut heads = Vec::new();
        for gx in -3..=3 {
            for gz in -3..=3 {
                let (x, z) = (216 + gx * 16, 202 + gz * 16);
                script.push_str(&format!(
                    "setblock {x} -60 {z} minecraft:stone\nsetblock {x} -60 {} minecraft:stone\n",
                    z + 1
                ));
                script.push_str(&format!("setblock {x} -59 {z} minecraft:red_bed[facing=south,part=foot]\n"));
                script.push_str(&format!("setblock {x} -59 {} minecraft:red_bed[facing=south,part=head]\n", z + 1));
                heads.push([x, -59, z + 1]);
            }
        }
        p.run(&script);
        p.tick();
        for h in heads {
            p.s.poi_acquire(h);
        }
    }

    /// Village cats: near a village of more than 4 occupied beds, a (non-persistent) cat
    /// ends up spawning at the player's height, in the center of a cell.
    #[test]
    fn village_cat_spawns() {
        let Some((mut p, dir)) = probe("w9cat") else { return };
        let want: Vec<(i32, i32)> = (8..=19).flat_map(|x| (7..=18).map(move |z| (x, z))).collect();
        p.s.ensure_loaded(&want);
        village(&mut p);
        let mut cat = None;
        for _ in 0..200 {
            p.s.mobs.spawners.cat_next = 0;
            p.s.cat_spawner_tick();
            if let Some(m) = p.s.mobs.list.iter().flatten().find(|m| m.kind == Kind::Cat) {
                cat = Some((m.pos, m.persistent));
                break;
            }
        }
        let (pos, persistent) = cat.expect("no village cat");
        assert_eq!(pos[1], -59.0, "player's height");
        assert_eq!((pos[0].fract().abs(), pos[2].fract().abs()), (0.5, 0.5), "cell center");
        assert!(!persistent, "village cat not persistent");
        finish(p, dir);
    }

    /// Zombie siege: at night, in a village, once the siege is decided, zombies
    /// spawn (at most 20), one every 3 ticks.
    #[test]
    fn village_siege_spawns_zombies() {
        let Some((mut p, dir)) = probe("w9siege") else { return };
        let want: Vec<(i32, i32)> = (8..=19).flat_map(|x| (7..=18).map(move |z| (x, z))).collect();
        p.s.ensure_loaded(&want);
        village(&mut p);
        for c in &mut p.s.clocks {
            if c.0 == "minecraft:overworld" {
                c.1 = 18001;
            }
        }
        p.s.mobs.spawners.siege_tonight = true;
        for _ in 0..100 {
            p.s.village_siege_tick();
        }
        let n = p.s.mobs.list.iter().flatten().filter(|m| m.kind == Kind::Zombie).count();
        assert!((1..=20).contains(&n), "siege zombies: {n}");
        // By day, nothing.
        let before = n;
        for c in &mut p.s.clocks {
            if c.0 == "minecraft:overworld" {
                c.1 = 6000;
            }
        }
        p.s.mobs.spawners.siege_tonight = true;
        p.s.mobs.spawners.siege_setup = false;
        for _ in 0..100 {
            p.s.village_siege_tick();
        }
        let after = p.s.mobs.list.iter().flatten().filter(|m| m.kind == Kind::Zombie).count();
        assert_eq!(after, before, "no siege by day");
        finish(p, dir);
    }

    /// Monster spawner: player within 16 blocks, delay elapsed → up to 4 zombies
    /// (spawner's own spawn rules: any light), event 2004, delay reset between 200 and
    /// 799; far from the player, nothing moves.
    #[test]
    fn mob_spawner_spawns() {
        use rc_proto::nbt::Nbt;
        let Some((mut p, dir)) = probe("w9spawner") else { return };
        p.run("setblock 216 -59 212 minecraft:spawner\n");
        let lim = || Nbt::Compound(vec![("min_inclusive".into(), Nbt::Int(0)), ("max_inclusive".into(), Nbt::Int(15))]);
        let data = Nbt::Compound(vec![
            ("entity".into(), Nbt::Compound(vec![("id".into(), Nbt::String("minecraft:zombie".into()))])),
            (
                "custom_spawn_rules".into(),
                Nbt::Compound(vec![("block_light_limit".into(), lim()), ("sky_light_limit".into(), lim())]),
            ),
        ]);
        {
            let be = p.s.be_get_mut([216, -59, 212]).expect("spawner block entity");
            crate::nbtio::set(be, "SpawnData", data);
            crate::nbtio::set(be, "Delay", Nbt::Short(0));
        }
        // Player far away: nothing.
        p.run("player 216.5 -59 240.5 0 0\n");
        p.s.spawner_blocks_tick();
        let zombies = |p: &Probe| p.s.mobs.list.iter().flatten().filter(|m| m.kind == Kind::Zombie).count();
        assert_eq!(zombies(&p), 0, "player at 28 blocks");
        p.run("player 216.5 -59 202.5 0 0\n");
        p.s.spawner_blocks_tick();
        let n = zombies(&p);
        assert!((1..=4).contains(&n), "zombies: {n}");
        let delay = crate::nbtio::int(p.s.be_get([216, -59, 212]).unwrap(), "Delay").unwrap();
        assert!((200..800).contains(&delay), "delay {delay}");
        p.s.spawner_blocks_tick();
        assert_eq!(zombies(&p), n, "no spawn during the delay");
        finish(p, dir);
    }

    /// Border: damage out of bounds (safe zone 5, 0.2 per block, at least 1), none inside;
    /// size transition advanced one step per tick.
    #[test]
    fn world_border_damage_and_lerp() {
        let Some((mut p, dir)) = probe("w9border") else { return };
        p.s.cmd.border.center = [216.0, 202.0];
        p.s.cmd.border.size = 10.0;
        assert_eq!(p.s.border_damage(), None, "inside");
        p.run("player 240.5 -59 202.5 0 0\n");
        // Edge at 221: distance −19.5 + 5 = −14.5 → ⌊2.9⌋ = 2.
        assert_eq!(p.s.border_damage(), Some(2.0));
        p.run("player 222.5 -59 202.5 0 0\n");
        assert_eq!(p.s.border_damage(), None, "inside the safe zone: nothing");
        p.s.cmd.border.size = 100.0;
        p.s.cmd.border.lerp = Some((100.0, 10.0, 20, 20));
        for _ in 0..10 {
            p.s.border_tick();
        }
        assert!((p.s.cmd.border.size - 55.0).abs() < 1e-9, "size {}", p.s.cmd.border.size);
        for _ in 0..10 {
            p.s.border_tick();
        }
        assert_eq!((p.s.cmd.border.size, p.s.cmd.border.lerp.is_none()), (10.0, true));
        finish(p, dir);
    }

    /// Spawn costs: charge 0.7, budget 0.15 (soul sand valley); a charge 4 blocks away
    /// lets it through (0.1225), 3 blocks away refuses (0.163).
    #[test]
    fn spawn_cost_potential() {
        use super::super::super::spawn::potential_ok;
        let pot = vec![([0, 64, 0], 0.7)];
        assert!(potential_ok(0.7, 0.15, [4, 64, 0], &pot));
        assert!(!potential_ok(0.7, 0.15, [3, 64, 0], &pot));
        assert!(!potential_ok(0.7, 0.15, [0, 64, 0], &pot), "same cell: infinite potential");
        assert!(potential_ok(0.0, 0.15, [0, 64, 0], &pot), "zero charge");
    }

    /// Inhabited time: +1 per tick for the player's chunk (even without `spawn_mobs`), nothing
    /// for a spectator; local difficulty reads it.
    #[test]
    fn inhabited_time_grows() {
        let Some((mut p, dir)) = probe("w9inh") else { return };
        let c = (216 >> 4, 202 >> 4);
        let before = p.s.chunks.get(&c).and_then(|c| c.as_ref()).map(|c| c.inhabited_time).unwrap();
        for _ in 0..10 {
            p.s.natural_spawn_tick();
        }
        let after = p.s.chunks.get(&c).and_then(|c| c.as_ref()).map(|c| c.inhabited_time).unwrap();
        assert_eq!(after - before, 10, "inhabited time");
        p.s.game_mode = 3;
        p.s.natural_spawn_tick();
        let spec = p.s.chunks.get(&c).and_then(|c| c.as_ref()).map(|c| c.inhabited_time).unwrap();
        assert_eq!(spec, after, "spectator: nothing");
        p.s.chunks.get_mut(&c).and_then(|c| c.as_mut()).unwrap().inhabited_time = 3_600_000;
        for c in &mut p.s.clocks {
            if c.0 == "minecraft:overworld" {
                c.1 = 0;
            }
        }
        let (eff, _) = p.s.local_difficulty([216, -59, 202]);
        // Normal: 2 × (0.75 + 0.75 + moon bounded by the null global) = 3.
        assert!((eff - 3.0).abs() < 1e-5, "local difficulty {eff}");
        finish(p, dir);
    }

    /// Surge Protector: lightning that disappears without setting fire, with a villager witness
    /// not struck within 30 blocks, gives the advancement; a placed fire (or a struck witness) does not.
    #[test]
    fn lightning_rod_advancement() {
        let Some((mut p, dir)) = probe("w9adv") else { return };
        let Some(&a) = p.s.prog.tree.index.get("minecraft:adventure/lightning_rod_with_villager_no_fire") else {
            finish(p, dir);
            return;
        };
        let v = mob(&mut p, Kind::Villager, [214.5, -59.0, 210.5]);
        let bolt = [210.5, -59.0, 210.5];
        p.s.trig_lightning_strike(bolt, 1, &[], &[Ent::Mob(v)]);
        assert!(!p.s.prog.done(a), "fire placed: no advancement");
        p.s.trig_lightning_strike(bolt, 0, &[], &[]);
        assert!(!p.s.prog.done(a), "no witness: no advancement");
        p.s.trig_lightning_strike(bolt, 0, &[Ent::Mob(v)], &[]);
        assert!(!p.s.prog.done(a), "villager struck: no advancement");
        p.s.trig_lightning_strike(bolt, 0, &[], &[Ent::Mob(v)]);
        assert!(p.s.prog.done(a), "no fire, villager witness");
        finish(p, dir);
    }

    /// Thunder and inhabited time near a guest / host spectator: the chunk of a non-spectator
    /// player always receives inhabited time.
    #[test]
    fn inhabited_time_follows_non_spectators() {
        let Some((mut p, dir)) = probe("w9spec") else { return };
        let c = (216 >> 4, 202 >> 4);
        let get = |p: &Probe| p.s.chunks.get(&c).and_then(|c| c.as_ref()).map(|c| c.inhabited_time).unwrap();
        let before = get(&p);
        p.s.game_mode = 0;
        p.s.natural_spawn_tick();
        assert_eq!(get(&p) - before, 1);
        finish(p, dir);
    }

    /// Spawner: blaze, breeze, zoglin, ghast without a supporting block; drowned and guardian without water.
    #[test]
    fn spawner_rules_skip_support_and_water() {
        use super::super::super::spawn::Reason;
        let Some((mut p, dir)) = probe("w9sprule") else { return };
        let air = [216, -50, 216];
        for k in [Kind::Blaze, Kind::Breeze, Kind::Zoglin] {
            assert!(p.s.spawn_rules_for(k, air, Reason::Spawner), "{k:?} from a spawner");
            assert!(!p.s.spawn_rules_for(k, air, Reason::Natural), "{k:?} natural in the air");
        }
        // Guardian: the spawner is exempt from the water at the position only; the water below the
        // position is still required (game rule), hence refusal in the air.
        assert!(!p.s.spawn_rules_for(Kind::Guardian, air, Reason::Spawner), "guardian: water below required");
        finish(p, dir);
    }
}
