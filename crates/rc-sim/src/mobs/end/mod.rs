//! Ender Dragon, the End fight and shulker (family F15, `server-mobs.md` § 4.F15).
//! Info sheets, own mob state (dragon, shulker), world state (crystals, breath clouds, dragon
//! fireballs, shulker bullets, fight) and hooks called by the common engine (`mobs/`). The
//! dragon has its own tick (`dragon.rs`, without common AI or movement), its phases
//! (`phases.rs`) and its flight graph (`graph.rs`); the fight (`fight.rs`) places the exit
//! portal, the egg and the gateways and handles the respawn with the four crystals.

pub(crate) mod breath;
pub(crate) mod bullet;
pub(crate) mod crystal;
pub(crate) mod dragon;
pub(crate) mod fight;
pub(crate) mod goals;
pub(crate) mod graph;
pub(crate) mod phases;
pub(crate) mod shulker;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod kb_world_tests;
#[cfg(test)]
mod world_tests;

use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use super::mob::Mob;
use super::species::{Category, Info, Kind, info as base_info};
use crate::nbtio::{self, int};
use crate::server::Server;

pub use goals::EGoal;

// ---------- Info sheets ----------

/// Dragon: 16 × 8 (eyes at 6.8), HP 200, client tracking 10, volume 5, fire-immune, no own
/// experience (the death experience is given by the fight).
static ENDER_DRAGON: Info = {
    let mut i = base_info("minecraft:ender_dragon", Category::Monster, 16.0, 8.0, 8.0 * 0.85);
    i.health = 200.0;
    i.tracking = 10;
    // Allowed in peaceful (neither removed nor refused on summon), like the game.
    i.ambient = Some("minecraft:entity.ender_dragon.ambient");
    i.hurt = "minecraft:entity.ender_dragon.hurt";
    i.volume = 5.0;
    i.fire_immune = true;
    i
};

/// Shulker: 1 × 1 (eyes at 0.5), HP 30, 5 experience, fire-immune.
static SHULKER: Info = {
    let mut i = base_info("minecraft:shulker", Category::Monster, 1.0, 1.0, 0.5);
    i.health = 30.0;
    i.xp = 5;
    i.tracking = 10;
    // Allowed in peaceful: a shulker is not removed when the difficulty goes there.
    i.ambient = Some("minecraft:entity.shulker.ambient");
    i.hurt = "minecraft:entity.shulker.hurt";
    i.death = "minecraft:entity.shulker.death";
    i.fire_immune = true;
    // `#fall_damage_immune`.
    i.fall_immune = true;
    i
};

pub fn info(k: Kind) -> &'static Info {
    if k == Kind::EnderDragon { &ENDER_DRAGON } else { &SHULKER }
}

/// Species of the family.
pub fn handles(k: Kind) -> bool {
    matches!(k, Kind::EnderDragon | Kind::Shulker)
}

// ---------- Own state ----------

/// State of the family (`end` field of the mob).
#[derive(Debug, Clone)]
pub struct EndMob {
    /// Dragon: phases, flight history, parts, linked fight.
    pub dragon: Option<Box<dragon::Dragon>>,
    /// Shulker: attach face (0 down, 1 up, 2 north, 3 south, 4 west, 5 east), synchronized
    /// opening (0..100), current and previous opening (0..1), color (16: none).
    pub attach: i32,
    pub peek: i32,
    pub peek_amount: f32,
    pub peek_o: f32,
    pub color: i8,
}

impl Default for EndMob {
    fn default() -> EndMob {
        EndMob { dragon: None, attach: 0, peek: 0, peek_amount: 0.0, peek_o: 0.0, color: 16 }
    }
}

/// World state (`end` field of the mobs, exchanged with the dimension).
#[derive(Debug, Default)]
pub struct EndWorld {
    /// Crystals (the clouds, dragon fireballs and bullets live in the projectile engine).
    pub crystals: Vec<crystal::Crystal>,
    /// Dragon fight (dimension with a fight: the End), read at the first tick.
    pub fight: Option<Box<fight::Fight>>,
    pub fight_checked: bool,
    /// Part of the dragon hit by the current blow (default: the body).
    pub part: Option<usize>,
    /// "Real" damage in progress: the dragon's hook is skipped.
    pub really: bool,
    /// Player hit on a dragon part (the weapon's knockback goes to the part).
    pub part_attack: bool,
    /// Up-to-date compounds of the family's entities (selectors and commands), rebuilt every tick.
    pub snapshots: Vec<(i32, Nbt)>,
    /// Direct entity of the current damage (arrow, wind charge: nothing in the sitting phase;
    /// shulker bullet: teleportation and duplication).
    pub direct: Option<&'static str>,
}

/// Construction: the dragon enters hover.
pub fn on_new(m: &mut Mob) {
    if m.kind == Kind::EnderDragon {
        let mut d = Box::new(dragon::Dragon::default());
        d.set_phase_initial();
        m.end.dragon = Some(d);
        // Without physics: never on the ground.
        m.on_ground = false;
    }
}

pub fn define_data(m: &mut Mob) {
    match m.kind {
        // Dragon phase data (hover at construction).
        Kind::EnderDragon => m.define(16, Meta::Int(phases::Phase::Hover as i32)),
        _ => {
            // Shulker data: attach face, opening, color.
            m.define(16, Meta::Direction(0));
            m.define(17, Meta::Byte(0));
            m.define(18, Meta::Byte(16));
        }
    }
}

pub fn sync_data(m: &mut Mob) {
    match m.kind {
        Kind::EnderDragon => {
            let p = m.end.dragon.as_ref().map_or(phases::Phase::Hover, |d| d.phase);
            m.set(16, Meta::Int(p as i32));
        }
        _ => {
            m.set(16, Meta::Direction(m.end.attach));
            m.set(17, Meta::Byte(m.end.peek as i8));
            m.set(18, Meta::Byte(m.end.color));
        }
    }
}

/// Own posture: the dragon gets "dying" on the fatal blow (even if it still flies).
pub fn pose(m: &Mob) -> Option<i32> {
    (m.kind == Kind::EnderDragon && m.end.dragon.as_ref().is_some_and(|d| d.dying_pose)).then_some(7)
}

pub fn load(m: &mut Mob, e: &Nbt) {
    match m.kind {
        Kind::EnderDragon => {
            let Some(d) = m.end.dragon.as_mut() else { return };
            if let Some(p) = int(e, "DragonPhase") {
                d.set_phase(phases::Phase::from_id(p as i32));
            }
            d.death_time = int(e, "DragonDeathTime").unwrap_or(0) as i32;
            d.sitting_damage = nbtio::float(e, "sitting_damage_received").unwrap_or(0.0);
        }
        _ => {
            m.end.attach = int(e, "AttachFace").map_or(0, |v| v as i32).clamp(0, 5);
            m.end.peek = int(e, "Peek").map_or(0, |v| i32::from(v as i8));
            m.end.color = int(e, "Color").map_or(16, |v| v as i8);
            // "Covered" modifier read back with the attributes (armor + 20).
            let covered = nbtio::list(e.get("attributes")).iter().any(|a| {
                a.get("id").and_then(Nbt::as_str) == Some("minecraft:armor")
                    && nbtio::list(a.get("modifiers"))
                        .iter()
                        .any(|md| md.get("id").and_then(Nbt::as_str) == Some("minecraft:covered"))
            });
            if covered {
                m.attrs.armor.set_perm_mod("minecraft:covered", 20.0, 0);
            }
        }
    }
}

pub fn save(m: &Mob, e: &mut Nbt) {
    match m.kind {
        Kind::EnderDragon => {
            let Some(d) = m.end.dragon.as_ref() else { return };
            nbtio::set(e, "DragonPhase", Nbt::Int(d.phase as i32));
            nbtio::set(e, "DragonDeathTime", Nbt::Int(d.death_time));
            nbtio::set(e, "sitting_damage_received", Nbt::Float(d.sitting_damage));
        }
        _ => {
            nbtio::set(e, "AttachFace", Nbt::Byte(m.end.attach as i8));
            nbtio::set(e, "Peek", Nbt::Byte(m.end.peek as i8));
            nbtio::set(e, "Color", Nbt::Byte(m.end.color));
            // Armor and its "covered" modifier (recorded when it is placed).
            if let Some(Nbt::List(list)) = nbtio::get_mut(e, "attributes") {
                list.retain(|a| a.get("id").and_then(Nbt::as_str) != Some("minecraft:armor"));
                let mut armor =
                    vec![("id".into(), Nbt::String("minecraft:armor".into())), ("base".into(), Nbt::Double(0.0))];
                if m.attrs.armor.mods.iter().any(|md| md.0 == "minecraft:covered") {
                    armor.push((
                        "modifiers".into(),
                        Nbt::List(vec![Nbt::Compound(vec![
                            ("id".into(), Nbt::String("minecraft:covered".into())),
                            ("amount".into(), Nbt::Double(20.0)),
                            ("operation".into(), Nbt::String("add_value".into())),
                        ])]),
                    ));
                }
                list.push(Nbt::Compound(armor));
            }
        }
    }
}

/// Silent entity (`Silent`).
pub fn silent(m: &Mob) -> bool {
    m.x.silent
}

/// Goals and targets (the dragon has none).
pub fn register_goals(m: &mut Mob) {
    if m.kind == Kind::Shulker {
        shulker::register_goals(m);
    }
}

// ---------- Engine hooks ----------

/// Own box: shulker (opening, attach face).
pub fn bbox(m: &Mob) -> Option<rc_physics::Aabb> {
    (m.kind == Kind::Shulker).then(|| shulker::bbox(m))
}

/// Never removed (dragon, void included).
pub fn never_despawn(m: &Mob) -> bool {
    m.kind == Kind::EnderDragon
}

/// Despawning far from players: never for the shulker (a golem in the game).
pub fn remove_when_far_away(m: &Mob) -> Option<bool> {
    (m.kind == Kind::Shulker).then_some(false)
}

/// The dragon refuses any effect (always false).
pub fn refuses_effects(k: Kind) -> bool {
    k == Kind::EnderDragon
}

/// No knockback for the sitting dragon.
pub fn no_knockback(m: &Mob) -> bool {
    m.kind == Kind::EnderDragon && m.end.dragon.as_ref().is_some_and(|d| d.phase.sitting())
}

/// The shulker never moves (speed always zero) nor turns its body.
pub fn immobile(m: &Mob) -> bool {
    m.kind == Kind::Shulker
}

/// Ambient sound: the closed shulker is quiet.
pub fn ambient_sound(m: &Mob) -> Option<String> {
    match m.kind {
        Kind::Shulker if m.end.peek == 0 => None,
        k => info(k).ambient.map(Into::into),
    }
}

/// Own hurt sound (closed shulker).
pub fn hurt_sound(m: &Mob, death: bool) -> Option<&'static str> {
    (m.kind == Kind::Shulker && !death && m.end.peek == 0).then_some("minecraft:entity.shulker.hurt_closed")
}

/// Head bounds (the head yaw rotation limit) of the shulker: 180.
/// Dragon in a sitting phase (arrows and tridents without effect, set on fire).
pub fn sitting_dragon(s: &Server, t: super::mob::Ent) -> bool {
    match t {
        super::mob::Ent::Mob(id) => s
            .mob_ref(id)
            .and_then(|m| m.end.dragon.as_ref())
            .is_some_and(|d| d.phase.sitting_class()),
        super::mob::Ent::Player => false,
    }
}

pub fn head_limits(k: Kind) -> Option<(f32, f32)> {
    (k == Kind::Shulker).then_some((180.0, 180.0))
}

impl Server {
    /// Own tick of a mob of the family; true if it replaces the common tick (dragon).
    pub(crate) fn end_tick_mob(&mut self, m: &mut Mob) -> bool {
        if m.kind == Kind::EnderDragon {
            self.dragon_tick(m);
            return true;
        }
        false
    }

    /// End of the common tick (a shulker's tick after `super.tick`): attach, opening.
    pub(crate) fn end_post_tick(&mut self, m: &mut Mob) {
        if m.kind == Kind::Shulker {
            self.shulker_post_tick(m);
        }
    }

    /// Own gaze control (shulker); true if it replaces the common one.
    pub(crate) fn end_look_control(&self, m: &mut Mob) -> bool {
        if m.kind == Kind::Shulker {
            shulker::look_control(m);
            return true;
        }
        false
    }

    /// Damage received (hook at the head of `mob_hurt`); `Some`: own handling. `dir` = direction
    /// of the imposed knockback (projectile), passed as is to the dragon's real hit.
    pub(crate) fn end_before_hurt(
        &mut self,
        m: &mut Mob,
        ty: &str,
        amount: f32,
        by: Option<super::mob::Ent>,
        src: Option<[f64; 3]>,
        dir: Option<(f64, f64)>,
    ) -> Option<bool> {
        match m.kind {
            Kind::EnderDragon if !self.mobs.end.really => Some(self.dragon_hurt(m, ty, amount, by, src, dir)),
            Kind::Shulker => self.shulker_before_hurt(m, ty),
            _ => None,
        }
    }

    /// After damage dealt: teleportation, duplication.
    pub(crate) fn end_after_hurt(&mut self, m: &mut Mob, ty: &str) {
        if m.kind == Kind::Shulker {
            self.shulker_after_hurt(m, ty);
        }
    }

    /// Compounds of the family's entities (id, NBT up to date as of the last tick).
    pub(crate) fn end_snapshot(&mut self) {
        let mut out: Vec<(i32, Nbt)> = Vec::new();
        out.extend(self.mobs.end.crystals.iter().filter(|c| !c.removed).map(|c| (c.id, c.save())));
        self.mobs.end.snapshots = out;
    }

    /// Compound of an entity of the family (commands).
    pub(crate) fn end_other(&self, id: i32) -> Option<&Nbt> {
        self.mobs.end.snapshots.iter().find(|(i, _)| *i == id).map(|(_, n)| n)
    }

    /// `kill` of an entity of the family that is not a mob; true if found.
    /// The crystal notifies the fight (without explosion).
    pub(crate) fn end_kill_other(&mut self, id: i32) -> bool {
        if let Some(c) = self.mobs.end.crystals.iter_mut().find(|c| c.id == id && !c.removed) {
            c.removed = true;
            self.fight_crystal_destroyed(id, None);
            return true;
        }
        false
    }

    /// `kill` of the dragon: removed on the spot, fight notified (victory).
    pub(crate) fn end_kill_mob(&mut self, id: i32) -> bool {
        let Some(i) = self.mob_index(id) else { return false };
        let Some(m) = self.mobs.list[i].as_mut() else { return false };
        if m.kind != Kind::EnderDragon {
            return false;
        }
        m.removed = true;
        let fight = m.end.dragon.as_ref().is_some_and(|d| d.fight);
        let m = (**m).clone();
        if fight {
            self.fight_update_dragon(&m);
            self.fight_dragon_killed(m.uuid);
        }
        true
    }

    /// Own death (`die` of the dragon); true if handled.
    pub(crate) fn end_die(&mut self, m: &mut Mob, ty: &str, by: Option<super::mob::Ent>) -> bool {
        if m.kind == Kind::EnderDragon {
            self.dragon_die(m, ty, by);
            return true;
        }
        false
    }

    /// Tick of the family's entities that are not mobs (after the mobs):
    /// fight, crystals, dragon fireballs, clouds, shulker bullets; then network tracking.
    pub(crate) fn end_world_tick(&mut self) {
        self.crystals_tick();
        self.track_crystals();
        self.end_snapshot();
    }

    /// Explosion: crystals and shulker bullets within the radius (twice the power) hit,
    /// except the explosion's source entity (exclude).
    pub(crate) fn end_explosion_hits(
        &mut self,
        center: [f64; 3],
        radius: f32,
        by: Option<super::mob::Ent>,
        ty: &str,
        exclude: Option<i32>,
    ) {
        let reach = f64::from(radius * 2.0);
        let ids: Vec<i32> = self
            .mobs
            .end
            .crystals
            .iter()
            .filter(|c| !c.removed && Some(c.id) != exclude && super::mth::dist2(c.pos, center).sqrt() / reach <= 1.0)
            .map(|c| c.id)
            .collect();
        for id in ids {
            self.crystal_hurt(id, ty, by, false);
        }
        let bullets: Vec<i32> = self
            .proj
            .list
            .iter()
            .filter(|p| {
                !p.removed
                    && p.kind == crate::projectiles::PKind::ShulkerBullet
                    && super::mth::dist2(p.pos, center).sqrt() / reach <= 1.0
            })
            .map(|p| p.id)
            .collect();
        for id in bullets {
            self.bullet_destroy(id);
        }
    }

    /// Dragon fight (before the entities).
    pub(crate) fn end_fight_tick(&mut self) {
        self.fight_tick();
    }

    /// Id of a read mob: the dragon reserves the next 8 for its parts.
    pub(crate) fn end_adopt_id(&mut self, kind: Kind, id: i32) -> i32 {
        if kind != Kind::EnderDragon {
            return id;
        }
        self.next_entity += 1;
        let nid = self.next_entity;
        self.next_entity += 8;
        nid
    }

    /// Reserves the ids of the parts of a new dragon (`new_mob`).
    pub(crate) fn end_reserve_parts(&mut self, kind: Kind) {
        if kind == Kind::EnderDragon {
            self.next_entity += 8;
        }
    }

    /// Attack packet targeting an entity of the family that is not a mob (dragon part,
    /// crystal, bullet); true if handled.
    pub(crate) fn end_attacked(&mut self, id: i32) -> bool {
        if self.dragon_part_attacked(id) {
            return true;
        }
        if self.crystal_attacked(id) {
            return true;
        }
        self.bullet_attacked(id)
    }

    /// Entities of the family in the entity chunks to write.
    pub(crate) fn end_into_chunks(&self, by_chunk: &mut std::collections::BTreeMap<(i32, i32), Vec<Nbt>>) {
        let chunk = |p: [f64; 3]| ((p[0].floor() as i32) >> 4, (p[2].floor() as i32) >> 4);
        for c in self.mobs.end.crystals.iter().filter(|c| !c.removed) {
            by_chunk.entry(chunk(c.pos)).or_default().push(c.save());
        }
    }

    /// Entities of the family read with a chunk (removed from the frozen entities).
    pub(crate) fn end_adopt(&mut self, loaded: &[(i32, i32)]) {
        for p in loaded {
            let Some(list) = self.entities.get_mut(p) else { continue };
            type Ents = Vec<(i32, Nbt)>;
            let (mine, rest): (Ents, Ents) = std::mem::take(list)
                .into_iter()
                .partition(|(_, e)| e.get("id").and_then(Nbt::as_str) == Some("minecraft:end_crystal"));
            *list = rest;
            if mine.is_empty() {
                continue;
            }
            // The chunk will be rewritten (entities removed in the meantime).
            self.life.drop_chunks.insert(*p);
            for (id, e) in mine {
                self.mobs.end.crystals.push(crystal::Crystal::load(id, &e));
            }
        }
    }

    /// Creation by command (`summon`) of an entity of the family that is not a mob.
    pub(crate) fn end_summon(&mut self, ty: &str, pos: [f64; 3], nbt: Option<&Nbt>) -> Option<i32> {
        let mut e = nbt.cloned().unwrap_or(Nbt::Compound(Vec::new()));
        nbtio::set(&mut e, "id", Nbt::String(ty.to_owned()));
        nbtio::set(&mut e, "Pos", Nbt::List(pos.iter().map(|&x| Nbt::Double(x)).collect()));
        // Persistent effect cloud: simulated (radius, wait, particle, effects), announced with its
        // metadata (8 radius, 9 wait, 10 particle).
        if ty == "minecraft:area_effect_cloud" {
            let id = self.proj_new_id();
            self.cloud_add(crate::projectiles::cloud::Cloud::from_nbt(id, &e));
            return Some(id);
        }
        if ty != "minecraft:end_crystal" {
            return None;
        }
        self.next_entity += 1;
        let id = self.next_entity;
        let mut c = crystal::Crystal::load(id, &e);
        if e.get("UUID").is_none() {
            c.uuid = self.end_uuid();
        }
        self.mobs.end.crystals.push(c);
        Some(id)
    }

    /// Version 4 UUID drawn from the new entity's own random (seeded from the clock), without touching
    /// the world random.
    pub(crate) fn end_uuid(&mut self) -> u128 {
        let mut r = super::jrand::JRand::from_time(i64::from(self.next_entity) ^ 0x4615);
        let u = (r.next_long() as u128) << 64 | r.next_long() as u64 as u128;
        (u & !(0xF000u128 << 64) | (0x4000u128 << 64)) & !(0xC000_0000_0000_0000u128) | 0x8000_0000_0000_0000u128
    }

    /// Own save (fight of the active dimension's dragon).
    pub(crate) fn end_save_jobs(&mut self, jobs: &mut Vec<crate::save::Job>) {
        self.fight_save_jobs(jobs);
    }

    /// Item used on a block (End crystal); true if handled.
    pub(crate) fn end_use_item_on(&mut self, slot: usize, item: &str, clicked: [i32; 3]) -> bool {
        if item != "minecraft:end_crystal" {
            return false;
        }
        if self.crystal_item_use(clicked) {
            self.award_stat("minecraft:used", item, 1);
            if self.game_mode != 1 {
                if let Some(s) = &mut self.slots[slot] {
                    s.count -= 1;
                    if s.count <= 0 {
                        self.slots[slot] = None;
                    }
                }
                self.broadcast_menus();
            }
        }
        true
    }
}
