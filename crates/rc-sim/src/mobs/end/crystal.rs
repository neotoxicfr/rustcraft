//! End crystals (F15): fire maintained under the crystal during a fight, destruction at the
//! slightest hit (except from the dragon, or invulnerable ones outside a creative player) with a
//! power-6 explosion (unless destroyed by an explosion), fight notified; synchronized beam and
//! base; placing by the item (obsidian or bedrock, free space) which attempts the dragon
//! respawn.

use rc_client::{EntityEvent, Event};
use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::{AddEntity, Meta};

use super::super::mob::Ent;
use crate::nbtio::{self, int};
use crate::server::Server;

#[derive(Debug, Clone)]
pub struct Crystal {
    pub id: i32,
    pub uuid: u128,
    pub pos: [f64; 3],
    /// Velocity (`Motion`): a crystal never moves, but a push (wind burst) is kept and saved.
    pub vel: [f64; 3],
    /// Velocity changed by a push: sent with the next tracking.
    vel_dirty: bool,
    pub yaw: f32,
    /// Beam target (`beam_target`).
    pub beam: Option<[i32; 3]>,
    pub show_bottom: bool,
    /// Permanent invulnerability (`Invulnerable`, respawn crystals).
    pub invulnerable: bool,
    pub time: i32,
    pub removed: bool,
    pub seen: bool,
    meta_dirty: bool,
    nbt: Nbt,
    /// Rank in the entity walk (`walk::next_rank`), stamped when it enters the list.
    pub(crate) rank: u64,
}

impl Crystal {
    pub fn new(id: i32, uuid: u128, pos: [f64; 3], yaw: f32) -> Crystal {
        Crystal {
            id,
            uuid,
            pos,
            vel: [0.0; 3],
            vel_dirty: false,
            yaw,
            beam: None,
            show_bottom: true,
            invulnerable: false,
            time: 0,
            removed: false,
            seen: false,
            meta_dirty: false,
            nbt: Nbt::Compound(Vec::new()),
            rank: crate::walk::next_rank(),
        }
    }

    pub fn load(id: i32, e: &Nbt) -> Crystal {
        let pos = nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
        let [yaw, _] = nbtio::f32s::<2>(e.get("Rotation")).unwrap_or_default();
        let mut c = Crystal::new(id, nbtio::uuid(e.get("UUID")).unwrap_or(0), pos, yaw);
        // Each component beyond 10 in absolute value is read as 0, like any entity.
        c.vel = nbtio::motion(e.get("Motion")).unwrap_or_default();
        c.beam = match e.get("beam_target") {
            Some(Nbt::IntArray(a)) if a.len() == 3 => Some([a[0], a[1], a[2]]),
            _ => None,
        };
        c.show_bottom = int(e, "ShowBottom").is_none_or(|v| v != 0);
        c.invulnerable = int(e, "Invulnerable").is_some_and(|v| v != 0);
        c.nbt = e.clone();
        c
    }

    pub fn save(&self) -> Nbt {
        let mut e = self.nbt.clone();
        let dbl = |v: [f64; 3]| Nbt::List(v.iter().map(|&x| Nbt::Double(x)).collect());
        nbtio::set(&mut e, "id", Nbt::String("minecraft:end_crystal".into()));
        nbtio::set(&mut e, "Pos", dbl(self.pos));
        nbtio::set(&mut e, "Motion", dbl(self.vel));
        nbtio::set(&mut e, "Rotation", Nbt::List(vec![Nbt::Float(self.yaw), Nbt::Float(0.0)]));
        nbtio::set(&mut e, "UUID", nbtio::uuid_nbt(self.uuid));
        nbtio::set(&mut e, "Invulnerable", Nbt::Byte(i8::from(self.invulnerable)));
        match self.beam {
            Some(b) => nbtio::set(&mut e, "beam_target", Nbt::IntArray(b.to_vec())),
            None => nbtio::remove(&mut e, "beam_target"),
        }
        nbtio::set(&mut e, "ShowBottom", Nbt::Byte(i8::from(self.show_bottom)));
        e
    }

    /// 2 × 2 box.
    pub fn bbox(&self) -> rc_physics::Aabb {
        let p = self.pos;
        rc_physics::Aabb { min: [p[0] - 1.0, p[1], p[2] - 1.0], max: [p[0] + 1.0, p[1] + 2.0, p[2] + 1.0] }
    }

    pub fn block_pos(&self) -> [i32; 3] {
        super::super::mth::block_of(self.pos)
    }

    pub fn set_beam(&mut self, b: Option<[i32; 3]>) {
        if self.beam != b {
            self.beam = b;
            self.meta_dirty = true;
        }
    }
}

impl Server {
    /// Crystal of slot `i`, at its rank in the entity walk: fire under the crystal during a
    /// fight (air cell).
    pub(crate) fn crystal_visit(&mut self, i: usize) {
        let Some(c) = self.mobs.end.crystals.get(i).filter(|c| !c.removed) else { return };
        // G3-F13: a crystal outside a "ticking entities" chunk waits (age included).
        let (ticking, mut rank) = (self.entity_ticks_at(c.pos), c.rank);
        let meet = self.meet(&mut rank, ticking, crate::walk::Fam::Crystal);
        let c = &mut self.mobs.end.crystals[i];
        c.rank = rank;
        if meet != crate::walk::Meet::Tick {
            return;
        }
        c.time += 1;
        if self.mobs.end.fight.is_none() {
            return;
        }
        // Its chunk is loaded (ticking test above).
        let p = c.block_pos();
        if self.is_air(p) {
            let f = self.fire_state_for(p);
            self.set_block(p, f, crate::blocks::flag::ALL);
        }
    }

    /// Knockback alone of a burst (wind) on the crystals, × `mult`: kept in their velocity (a
    /// crystal never moves, but saves and sends it).
    pub(crate) fn crystals_blast_push(&mut self, center: [f64; 3], radius: f32, mult: f64) {
        let double = f64::from(radius * 2.0);
        for i in 0..self.mobs.end.crystals.len() {
            let c = &self.mobs.end.crystals[i];
            if c.removed {
                continue;
            }
            let dist = super::super::mth::dist2(c.pos, center).sqrt() / double;
            if dist > 1.0 {
                continue;
            }
            let eye = c.pos[1] + f64::from(2.0f32 * 0.85);
            let dir = super::super::mth::normalize([c.pos[0] - center[0], eye - center[1], c.pos[2] - center[2]]);
            let kp = (1.0 - dist) * f64::from(self.seen_percent(center, &c.bbox())) * mult;
            let c = &mut self.mobs.end.crystals[i];
            c.vel = [c.vel[0] + dir[0] * kp, c.vel[1] + dir[1] * kp, c.vel[2] + dir[2] * kp];
            c.vel_dirty = true;
        }
    }

    /// New crystal.
    pub(crate) fn spawn_crystal(&mut self, pos: [f64; 3], yaw: f32, show_bottom: bool) -> i32 {
        self.next_entity += 1;
        let id = self.next_entity;
        let uuid = self.end_uuid();
        let mut c = Crystal::new(id, uuid, pos, yaw);
        c.show_bottom = show_bottom;
        self.mobs.end.crystals.push(c);
        id
    }

    /// Damage to a crystal; true if it lands.
    pub(crate) fn crystal_hurt(&mut self, id: i32, ty: &str, by: Option<Ent>, creative: bool) -> bool {
        let Some(i) = self.mobs.end.crystals.iter().position(|c| c.id == id && !c.removed) else { return false };
        let c = &self.mobs.end.crystals[i];
        // Invulnerable (except creative player or damage that bypasses),
        // fire-immune.
        let bypass = self.dmg_tag(ty, "minecraft:bypasses_invulnerability");
        if c.invulnerable && !bypass && !creative {
            return false;
        }
        if self.dmg_tag(ty, "minecraft:is_fire") {
            return false;
        }
        if let Some(Ent::Mob(mid)) = by
            && self.mob_ref(mid).is_some_and(|o| o.kind == super::super::species::Kind::EnderDragon)
        {
            return false;
        }
        let pos = c.pos;
        self.mobs.end.crystals[i].removed = true;
        if !self.dmg_tag(ty, "minecraft:is_explosion") {
            let ety = if by.is_some() { "minecraft:player_explosion" } else { "minecraft:explosion" };
            self.game_event_src("minecraft:explode", pos, Some(crate::mobs::recent::vibration::Src::Other), None);
            self.prog.expl_cause_next = Some("minecraft:end_crystal"); // W8c
            self.blast_direct_next(by, id, |s| s.mobs.end.crystals[i].save()); // L10-COMBAT
            self.explode_with(None, by, None, pos, 6.0, false, ety, super::super::explode::Blast::Block);
            self.prog.expl_cause_next = None;
        }
        self.fight_crystal_destroyed(id, by);
        true
    }

    /// Player hit on a crystal; true if the id is a crystal.
    pub(crate) fn crystal_attacked(&mut self, id: i32) -> bool {
        let Some(c) = self.mobs.end.crystals.iter().find(|c| c.id == id && !c.removed) else { return false };
        if self.game_mode == 3 || self.life.vit.dead {
            return true;
        }
        let (b, pos) = (c.bbox(), c.pos);
        let eye = [self.pos[0], self.player_eye_y(), self.pos[2]];
        let cl: [f64; 3] = std::array::from_fn(|k| eye[k].clamp(b.min[k], b.max[k]));
        if super::super::mth::dist2(eye, cl) >= 36.0 {
            return true;
        }
        self.mobs.attack_ticker = 0;
        let creative = self.game_mode == 1;
        // G5-F04: damage source of the weapon (its type, the mace's smash).
        let weapon = self.player_weapon();
        let smash = weapon.as_ref().is_some_and(|w| w.id == crate::mace::MACE)
            && crate::mace::can_smash(self.life.vit.fall, self.proj_player.fall_flying);
        let ty = self.weapon_damage_type(weapon.as_ref(), smash, "minecraft:player_attack");
        if self.crystal_hurt(id, &ty, Some(Ent::Player), creative) {
            // Non-living target hit: the player forgets their last victim; the weapon's after-hit
            // effects still run (Wind Burst).
            self.set_last_hurt_mob(Some(Ent::Player), None);
            self.player_weapon_post_attack(crate::commands::ents::E::Other(id), pos, &ty, weapon.as_ref());
        }
        true
    }

    /// "End crystal" item used on a block; true if placed.
    pub(crate) fn crystal_item_use(&mut self, clicked: [i32; 3]) -> bool {
        let name = self.state_name(self.st(clicked));
        if name != "minecraft:obsidian" && name != "minecraft:bedrock" {
            return false;
        }
        let above = [clicked[0], clicked[1] + 1, clicked[2]];
        if !self.is_air(above) {
            return false;
        }
        let (x, y, z) = (f64::from(above[0]), f64::from(above[1]), f64::from(above[2]));
        let area = rc_physics::Aabb { min: [x, y, z], max: [x + 1.0, y + 2.0, z + 1.0] };
        let busy = self.mobs.list.iter().flatten().any(|o| !o.removed && o.touches(&area))
            || (self.game_mode != 3 && !self.life.vit.dead && self.player_aabb().intersects(&area))
            || self.mobs.end.crystals.iter().any(|c| !c.removed && c.bbox().intersects(&area))
            || self.life.drops.iter().any(|d| {
                let b = rc_physics::Aabb {
                    min: [d.pos[0] - 0.125, d.pos[1], d.pos[2] - 0.125],
                    max: [d.pos[0] + 0.125, d.pos[1] + 0.25, d.pos[2] + 0.125],
                };
                b.intersects(&area)
            });
        if busy {
            return false;
        }
        self.spawn_crystal([x + 0.5, y, z + 0.5], 0.0, false);
        self.fight_try_respawn();
        true
    }

    /// Network tracking of a crystal (range 16 chunks, never a position update).
    pub(crate) fn track_crystals(&mut self) {
        let range = (16.0f64 * 16.0).min(f64::from(self.view) * 16.0);
        let kind = self.entity_kind_id("minecraft:end_crystal");
        let mut list = std::mem::take(&mut self.mobs.end.crystals);
        for c in &mut list {
            let chunk = ((c.pos[0].floor() as i32) >> 4, (c.pos[2].floor() as i32) >> 4);
            let (dx, dz) = (self.pos[0] - c.pos[0], self.pos[2] - c.pos[2]);
            let visible = !c.removed && dx * dx + dz * dz <= range * range && self.sent.contains(&chunk);
            if visible && !c.seen {
                c.seen = true;
                c.meta_dirty = false;
                let add = AddEntity {
                    id: c.id,
                    uuid: c.uuid,
                    kind,
                    pos: c.pos,
                    velocity: c.vel,
                    pitch: 0.0,
                    yaw: c.yaw,
                    head_yaw: 0.0,
                    data: 0,
                };
                let _ = self.send(Event::Entity(EntityEvent::Add(Box::new(add))));
                let mut values = Vec::new();
                if c.beam.is_some() {
                    values.push((8, Meta::BlockPos(c.beam)));
                }
                if !c.show_bottom {
                    values.push((9, Meta::Bool(false)));
                }
                if !values.is_empty() {
                    let _ = self.send(Event::Entity(EntityEvent::Data { id: c.id, values }));
                }
            } else if !visible && c.seen {
                c.seen = false;
                let _ = self.send(Event::Entity(EntityEvent::Remove(vec![c.id])));
            } else if c.seen && c.meta_dirty {
                c.meta_dirty = false;
                let values = vec![(8, Meta::BlockPos(c.beam)), (9, Meta::Bool(c.show_bottom))];
                let _ = self.send(Event::Entity(EntityEvent::Data { id: c.id, values }));
            }
            // A push: its velocity goes to the client (the spawn carries it otherwise).
            if std::mem::take(&mut c.vel_dirty) && c.seen && !c.removed {
                let _ = self.send(Event::Entity(EntityEvent::Motion { id: c.id, velocity: crate::mobs::track::lp_quantize(c.vel) }));
            }
        }
        list.retain(|c| !c.removed || c.seen);
        // Removed ones still seen leave on the next pass (already marked invisible).
        list.retain(|c| !c.removed);
        list.append(&mut self.mobs.end.crystals);
        self.mobs.end.crystals = list;
    }
}
