//! Illager spells: fangs, vex summoning,
//! "wololo" (blue sheep → red), mirror image (invisibility 1,200 ticks) and blindness
//! (400 ticks) of the illusioner. Fangs: delay, hit from 6 to −8, event 4, life 22 ticks.

use rc_client::{EntityEvent, Event};
use rc_proto_v777::entity::AddEntity;

use super::super::mob::{Ent, Mob};
use super::super::mth::{block_of, cos, dist2, sin};
use super::super::species::Kind;
use super::Spell;
use crate::server::Server;

/// Evoker fangs in flight.
#[derive(Debug, Clone)]
pub struct Fangs {
    pub id: i32,
    pub pos: [f64; 3],
    pub yaw: f32,
    pub warmup: i32,
    pub life: i32,
    pub sent_spike: bool,
    pub owner: Option<i32>,
    pub removed: bool,
    /// Rank in the entity walk (`walk::next_rank`).
    pub rank: u64,
    /// Announced to the client (L9-TICK: false again once the player has left their world).
    pub seen: bool,
}

impl Server {
    /// Start condition of a spell (common: living target, no casting, interval elapsed).
    pub(crate) fn spell_can_use(&mut self, m: &mut Mob, spell: Spell, next_tick: i32, last_target: i32) -> bool {
        if spell == Spell::Wololo {
            return self.wololo_can_use(m, next_tick);
        }
        if !self.ent_valid(m.target) || m.ill.spell_ticks > 0 || m.tick_count < next_tick {
            return false;
        }
        match spell {
            Spell::SummonVex => {
                let b = m.bbox();
                let area = rc_physics::Aabb { min: b.min.map(|v| v - 16.0), max: b.max.map(|v| v + 16.0) };
                let vexes = self
                    .mobs
                    .of_kind(Kind::Vex)
                    .filter(|o| o.alive() && o.touches(&area) && dist2(m.pos(), o.pos()) <= 256.0)
                    .count() as i32;
                m.rng.next_int_bound(8) + 1 > vexes
            }
            Spell::Disappear => !m.x.effects.iter().any(|e| e.id == "minecraft:invisibility"),
            Spell::Blindness => {
                let tid = match m.target {
                    Some(Ent::Mob(id)) => id,
                    Some(Ent::Player) => self.player_id,
                    None => return false,
                };
                if tid == last_target {
                    return false;
                }
                let (eff, _) = self.local_difficulty(m.block_pos());
                eff > 2.0
            }
            _ => true,
        }
    }

    /// Start condition of the evoker's "wololo" spell: no target, rule `mob_griefing`, blue sheep within 16 blocks
    /// (sight required) drawn at random.
    fn wololo_can_use(&mut self, m: &mut Mob, next_tick: i32) -> bool {
        if m.target.is_some() || m.ill.spell_ticks > 0 || m.tick_count < next_tick {
            return false;
        }
        if !self.rule("minecraft:mob_griefing", true) {
            return false;
        }
        let b = m.bbox();
        let area = rc_physics::Aabb {
            min: [b.min[0] - 16.0, b.min[1] - 4.0, b.min[2] - 16.0],
            max: [b.max[0] + 16.0, b.max[1] + 4.0, b.max[2] + 16.0],
        };
        // Blue sheep through the per-species index; the sight checks only read the server.
        let sheep: Vec<i32> = self
            .mobs
            .of_kind(Kind::Sheep)
            .filter(|o| o.alive() && (o.wool & 15) == 11 && o.touches(&area))
            .map(|o| o.id)
            .filter(|&id| self.targeting_ok(m, Ent::Mob(id), false, 16.0, true, true))
            .collect();
        if sheep.is_empty() {
            return false;
        }
        let i = m.rng.next_int_bound(sheep.len() as i32) as usize;
        m.ill.wololo = Some(sheep[i]);
        true
    }

    /// Effect of the spell at the end of the casting delay.
    pub(crate) fn perform_spell(&mut self, m: &mut Mob, spell: Spell) {
        match spell {
            Spell::Fangs => self.cast_fangs(m),
            Spell::SummonVex => self.summon_vexes(m),
            Spell::Wololo => {
                if let Some(id) = m.ill.wololo
                    && let Some(i) = self.mob_index(id)
                    && let Some(o) = self.mobs.list.get_mut(i)
                    && o.alive()
                {
                    // Red (14).
                    o.wool = (o.wool & !15) | 14;
                }
            }
            Spell::Disappear => {
                self.give_mob_effect(m, "minecraft:invisibility", 1200, 0);
            }
            Spell::Blindness => {
                if let Some(t) = m.target {
                    match t {
                        Ent::Player => {
                            self.give_player_effect_from("minecraft:blindness", 400, 0, Some(&*m));
                        }
                        Ent::Mob(id) => {
                            if let Some(i) = self.mob_index(id) {
                                let mut o = self.mobs.list.take(i).expect("mob");
                                self.give_mob_effect(&mut o, "minecraft:blindness", 400, 0);
                                self.mobs.list.put(i, o);
                            }
                        }
                    }
                }
            }
            Spell::None => {}
        }
    }

    /// Launch of the evoker's attack spell: circles within 3 blocks, otherwise a line of
    /// 16 fangs toward the target.
    fn cast_fangs(&mut self, m: &mut Mob) {
        let Some(t) = m.target else { return };
        let Some(tp) = self.ent_pos(t) else { return };
        let min_y = tp[1].min(m.pos()[1]);
        let max_y = tp[1].max(m.pos()[1]) + 1.0;
        let angle = super::super::mth::atan2(tp[2] - m.pos()[2], tp[0] - m.pos()[0]) as f32;
        if dist2(m.pos(), tp) < 9.0 {
            for i in 0..5 {
                let a = angle + i as f32 * std::f32::consts::PI * 0.4;
                let x = m.pos()[0] + f64::from(cos(a)) * 1.5;
                let z = m.pos()[2] + f64::from(sin(a)) * 1.5;
                self.create_fangs(m, x, z, min_y, max_y, a, 0);
            }
            for i in 0..8 {
                let a = angle + i as f32 * std::f32::consts::PI * 2.0 / 8.0 + (std::f64::consts::PI * 2.0 / 5.0) as f32;
                let x = m.pos()[0] + f64::from(cos(a)) * 2.5;
                let z = m.pos()[2] + f64::from(sin(a)) * 2.5;
                self.create_fangs(m, x, z, min_y, max_y, a, 3);
            }
        } else {
            for i in 0..16 {
                let reach = 1.25 * f64::from(i + 1);
                let x = m.pos()[0] + f64::from(cos(angle)) * reach;
                let z = m.pos()[2] + f64::from(sin(angle)) * reach;
                self.create_fangs(m, x, z, min_y, max_y, angle, i);
            }
        }
    }

    /// Fangs placed on the first full top face going down from `max_y`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn create_fangs(&mut self, m: &Mob, x: f64, z: f64, min_y: f64, max_y: f64, angle: f32, delay: i32) {
        let mut p = block_of([x, max_y, z]);
        let mut found = false;
        let mut top = 0.0;
        loop {
            let below = [p[0], p[1] - 1, p[2]];
            if self.face_sturdy_up(below) {
                if !self.is_air(p)
                    && let Some(mx) = self.phys.shape(self.st(p)).iter().map(|b| b.max[1]).reduce(f64::max)
                {
                    top = mx;
                }
                found = true;
                break;
            }
            p = below;
            if p[1] < min_y.floor() as i32 - 1 {
                break;
            }
        }
        if !found {
            return;
        }
        self.next_entity += 1;
        let id = self.next_entity;
        let pos = [x, f64::from(p[1]) + top, z];
        let yaw = angle * (180.0 / std::f32::consts::PI);
        let mut f = Fangs {
            id,
            pos,
            yaw,
            warmup: delay,
            life: 22,
            sent_spike: false,
            owner: Some(m.id),
            removed: false,
            rank: crate::walk::next_rank(),
            seen: false,
        };
        self.fangs_pair(&mut f);
        self.mobs.ill.fangs.push(f);
        let src = Some(crate::mobs::recent::vibration::Src::Living(Ent::Mob(m.id)));
        self.game_event_src("minecraft:entity_place", pos, src, None);
    }

    /// Announces the fangs to the client. Only the entity itself: the bite event is never sent
    /// again, so fangs paired after it stay invisible on the client (game behaviour: the client
    /// draws them only once it has received that event).
    fn fangs_pair(&mut self, f: &mut Fangs) {
        let add = AddEntity {
            id: f.id,
            uuid: (f.id as u128) << 64 | 0x4600_0000_0000_0000,
            kind: self.entity_kind_id("minecraft:evoker_fangs"),
            pos: f.pos,
            velocity: [0.0; 3],
            pitch: 0.0,
            yaw: f.yaw,
            head_yaw: 0.0,
            data: 0,
        };
        let _ = self.send(Event::Entity(EntityEvent::Add(Box::new(add))));
        f.seen = true;
    }

    /// Full top face: approximated by a collision box
    /// covering the whole top.
    fn face_sturdy_up(&self, p: [i32; 3]) -> bool {
        self.phys.shape(self.st(p)).iter().any(|b| {
            b.max[1] >= 1.0 - 1.0e-7
                && b.min[0] <= 1.0e-7
                && b.min[2] <= 1.0e-7
                && b.max[0] >= 1.0 - 1.0e-7
                && b.max[2] >= 1.0 - 1.0e-7
        })
    }

    /// Fangs of slot `i`, at their rank in the entity walk: delay, hit at −8, event 4,
    /// disappearance. The list is taken out during their tick (newcomers appended after).
    pub(crate) fn fang_visit(&mut self, i: usize) {
        let mut list = std::mem::take(&mut self.mobs.ill.fangs);
        if let Some(f) = list.get_mut(i).filter(|f| !f.removed) {
            self.fang_tick(f);
        }
        list.append(&mut self.mobs.ill.fangs);
        self.mobs.ill.fangs = list;
    }

    fn fang_tick(&mut self, f: &mut Fangs) {
        // L9-TICK: back in their world, the client has forgotten them (cleared at respawn).
        if !f.seen {
            self.fangs_pair(f);
        }
        // G3-F13: outside a "ticking entities" chunk, it waits.
        let ticking = self.entity_ticks_at(f.pos);
        if self.meet(&mut f.rank, ticking, crate::walk::Fam::Fang) != crate::walk::Meet::Tick {
            return;
        }
        f.warmup -= 1;
        if f.warmup >= 0 {
            return;
        }
        if f.warmup == -8 {
            // Box 0.5 × 0.8 inflated by 0.2 horizontally.
            let bb = rc_physics::Aabb {
                min: [f.pos[0] - 0.25 - 0.2, f.pos[1], f.pos[2] - 0.25 - 0.2],
                max: [f.pos[0] + 0.25 + 0.2, f.pos[1] + 0.8, f.pos[2] + 0.25 + 0.2],
            };
            self.fangs_damage(f, &bb);
        }
        if !f.sent_spike {
            let _ = self.send(Event::Entity(EntityEvent::Event { id: f.id, event: 4 }));
            f.sent_spike = true;
        }
        f.life -= 1;
        if f.life < 0 {
            f.removed = true;
        }
    }

    /// Removed fangs, after the entity walk.
    pub(crate) fn fangs_post(&mut self) {
        if !self.mobs.ill.fangs.iter().any(|f| f.removed) {
            return;
        }
        let gone: Vec<i32> = self.mobs.ill.fangs.iter().filter(|f| f.removed).map(|f| f.id).collect();
        let _ = self.send(Event::Entity(EntityEvent::Remove(gone)));
        self.mobs.ill.fangs.retain(|f| !f.removed);
    }

    /// 6 magic (indirect if the caster lives), allies spared.
    fn fangs_damage(&mut self, f: &Fangs, bb: &rc_physics::Aabb) {
        let ids: Vec<i32> = self
            .mobs
            .list
            .iter()
            .filter(|o| o.alive() && o.touches(bb) && Some(o.id) != f.owner)
            .map(|o| o.id)
            .collect();
        // The caster is copied as it is before any hit (it stays in the list: thorns and anger
        // reach it there). `RUSTCRAFT_OPT_FANGSLAZY`: the copy is made only when something is hit,
        // which most fangs never do.
        let lazy = crate::poi::new_search_path(rc_data::opt_switch!("RUSTCRAFT_OPT_FANGSLAZY"));
        if lazy && ids.is_empty() && !self.fangs_reach_player(bb) {
            crate::poi::note_search_path("fangs without caster copy");
            return;
        }
        let owner = f.owner.and_then(|id| self.mob_ref(id)).filter(|o| o.alive()).cloned();
        for id in ids {
            let Some(i) = self.mob_index(id) else { continue };
            let mut o = self.mobs.list.take(i).expect("mob");
            match &owner {
                None => {
                    self.mob_hurt(&mut o, "minecraft:magic", 6.0, None, None);
                }
                Some(ow) => {
                    if !super::allied(self, ow, &o)
                        && self.mob_hurt(&mut o, "minecraft:indirect_magic", 6.0, Some(Ent::Mob(ow.id)), Some(f.pos))
                    {
                        // G5-F04: thorns of the struck mob on the caster.
                        self.mobs.list.put(i, o);
                        self.post_attack_by(Ent::Mob(id), Some(Ent::Mob(ow.id)), "minecraft:indirect_magic");
                        continue;
                    }
                }
            }
            self.mobs.list.put(i, o);
        }
        if self.fangs_reach_player(bb) {
            match &owner {
                None => {
                    self.hurt("minecraft:magic", 6.0);
                }
                Some(ow) => {
                    // Direct source: the fangs (knockback from their position).
                    let kb = Some((f.pos[0] - self.pos[0], f.pos[2] - self.pos[2]));
                    if self.player_hurt_by_mob(ow, "minecraft:indirect_magic", 6.0, kb) {
                        self.player_thorns_by(Some(Ent::Mob(ow.id)), "minecraft:indirect_magic"); // G5-F02d: on the caster
                    }
                }
            }
        }
    }

    fn fangs_reach_player(&self, bb: &rc_physics::Aabb) -> bool {
        self.game_mode != 3 && self.ent_alive(Ent::Player) && self.player_aabb().intersects(bb)
    }

    /// Launch of the evoker's summoning spell: 3 vexes at ±2 blocks, one block higher, life of
    /// 30 to 119 s (evoker's random).
    pub(crate) fn summon_vexes(&mut self, m: &mut Mob) {
        let b = m.block_pos();
        for _ in 0..3 {
            let dx = -2 + m.rng.next_int_bound(5);
            let dz = -2 + m.rng.next_int_bound(5);
            let p = [b[0] + dx, b[1] + 1, b[2] + dz];
            let pos = [f64::from(p[0]) + 0.5, f64::from(p[1]), f64::from(p[2]) + 0.5];
            let mut v = self.new_mob(Kind::Vex, pos, 0.0);
            v.head_yaw = 0.0;
            v.body_yaw = 0.0;
            let mut g = super::super::spawn::Group::default();
            self.finalize_spawn(&mut v, super::super::spawn::Reason::MobSummoned, &mut g);
            v.ill.owner = Some(m.uuid);
            v.ill.owner_id = Some(m.id);
            v.ill.bound = Some(p);
            v.ill.life = Some(20 * (30 + m.rng.next_int_bound(90)));
            // The evoker's scoreboard team, if any, before the vex enters the world.
            if let Some(team) = self.cmd.scores.team_of_uuid(m.uuid).map(|t| t.name.clone()) {
                self.cmd.scores.join_team(&crate::nbtio::uuid_text(v.uuid), &team);
            }
            self.add_mob(v);
            self.game_event_block("minecraft:entity_place", p, Some(Ent::Mob(m.id)), None);
        }
    }
}
