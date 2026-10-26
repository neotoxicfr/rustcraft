//! Singleplayer mobs (mission S4, `docs/specs/server-mobs.md`): twelve common species
//! simulated on the integrated server side like the official 26.3 server — physics, goals and
//! navigation, combat, spawning and despawning, network tracking, save in `entities/`.

pub(crate) mod aquatic;
pub(crate) mod arrows;
mod bell;
mod body;
pub(crate) mod fireballs;
pub(crate) mod nether;
pub(crate) mod brains;
pub(crate) mod brainy;
pub(crate) mod combat;
pub(crate) mod cubes;
pub(crate) mod effects;
pub(crate) mod env;
pub(crate) mod end;
pub(crate) mod explode;
pub(crate) mod goals;
pub(crate) mod golems;
pub(crate) mod illagers;
pub(crate) mod index;
pub(crate) mod interact; // G5-F03: name tag, egg, dyeing, cat and wolf
pub(crate) mod jrand;
pub(crate) mod mob;
pub(crate) mod mounts;
pub(crate) mod mth;
pub(crate) mod nav;
mod pests;
pub(crate) mod piglins; // F13: piglins and hoglins
mod phantom;
pub(crate) mod sealife;
pub(crate) mod recent; // F14: warden, breeze, creaking, happy ghast
pub(crate) mod spawn;
pub(crate) mod species;
mod structure_spawn; // F07: spawn lists of structures
pub(crate) mod surface;
mod tick;
pub(crate) mod track;
pub(crate) mod trades;
pub(crate) mod trial; // W9: cages, trial spawners, vaults
pub(crate) mod variants;
pub(crate) mod villager;
pub(crate) mod wild;
pub(crate) mod wither; // F16

#[cfg(test)]
mod tag_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_f01;
#[cfg(test)]
mod tests_index;
#[cfg(test)]
mod tests_g4f02;
#[cfg(test)]
mod tests_integ;
#[cfg(test)]
mod tests_loot_rng;
#[cfg(test)]
mod tests_patterns;
#[cfg(test)]
mod tests_respawn_blast;
#[cfg(test)]
mod tests_w10;
#[cfg(test)]
mod tests_g4f03;
#[cfg(test)]
mod tests_g5f03;
#[cfg(test)]
mod tests_g5f03b;

use rc_proto::Reader;
use rc_proto::nbt::Nbt;
use rc_proto::varint::read_varint;

use mob::{Ent, Mob};
use species::Kind;

use crate::player::Stack;
use crate::server::Server;

/// State of the mobs of the open world.
pub struct Mobs {
    /// Mobs in order of addition (list of ticking entities).
    pub list: Vec<Option<Box<Mob>>>,
    /// Hint id/UUID → slot of `list` (checked at every read, see `index`).
    pub index: index::MobIndex,
    /// World random for mobs (spawns, finalisations, sounds).
    pub rng: jrand::JRand,
    /// Player: last attacker (mob, timestamp), last mob hit.
    pub player_hurt_by: Option<(Ent, i32)>,
    pub player_hit: Option<(Ent, i32)>,
    pub player_ticks: i32,
    /// Attack strength of the player.
    pub attack_ticker: i32,
    pub last_main_hand: Option<(String, i32)>,
    /// Speed of the player as seen by the server (knockback).
    pub player_vel: [f64; 3],
    /// Entity chunks whose mobs have already been read.
    pub adopted: hashbrown::HashSet<(i32, i32)>,
    /// Charged creeper exploding (identifier, head already dropped): only one victim
    /// drops its head (table `charged_creeper/root`).
    pub charged_skull: Option<(i32, bool)>,
    /// F01: attacker whose hit is being applied (random lent for the reaction to a kill), species, identifier.
    pub killer: Option<(jrand::JRand, Kind, i32, [i32; 3])>,
    /// F01: phantom spawner (next attempt).
    pub phantom_next: i32,
    pub rest_dead: bool,
    /// Mob whose hit is being applied to the player (death message).
    pub hit_by: Option<Kind>,
    /// Parrots on the player's shoulders (left, right) and time of the last placement (F02).
    pub shoulder: [Option<Nbt>; 2],
    pub shoulder_time: i64,
    pub shoulder_loaded: bool,
    /// F03: seed (slime chunks), known speed of the player, sulfur cube archetypes.
    pub cubes: cubes::CubesWorld,
    /// F04: mounted player, llama spits, trap lightning.
    pub mounts: mounts::World,
    /// Special generators of the cats and of the zombie siege (Overworld).
    pub spawners: spawn::CustomSpawners,
    /// F05: registry of points of interest, world state of villagers and traders.
    pub poi: crate::poi::Pois,
    pub npc: villager::NpcWorld,
    /// F07: patrols, fangs, thrown potions, raids.
    pub ill: illagers::IllWorld,
    /// F08: deferred thorns.
    pub aqua: aquatic::AquaWorld,
    /// F09: player swimming (dolphins).
    pub sea: sealife::World,
    /// F10: current killer (components read by the loot).
    pub brainy: brainy::World,
    /// F11: ticking beehives, stingers stuck in the player.
    pub wild: wild::hive::HiveWorld,
    /// F12: blaze and ghast fireballs.
    pub fireballs: fireballs::FireballsWorld,
    /// Mobs being ticked (taken out of `list` for the duration of their tick, passengers
    /// included): (UUID, identifier). A projectile shot during this tick thus finds its
    /// shooter (spawn data, author).
    pub ticking: Vec<(u128, i32)>,
    /// Frozen copies of the attackers being ticked (out of `list`), placed for the duration of a
    /// hit: the victim finds its aggressor by identifier (`mob_ref`; anger, gossip,
    /// aimed knockback). Never modified: `mob_index` does not see them.
    pub ghosts: Vec<Mob>,
    /// F13: shared range of the brain sensors.
    pub pig: piglins::PigWorld,
    /// F14: creaking hearts, vibrations, shriekers, wind charges.
    pub rec: recent::RecWorld,
    /// F15: crystals, dragon breath, shulker bullets, dragon fight.
    pub end: end::EndWorld,
    /// F16: blue skull explosion, wither being ticked.
    pub wither: wither::WitherWorld,
    /// Bells rung (resonance and glowing of pillagers).
    pub bells: Vec<bell::Bell>,
    /// Imposed Halloween day (tests, bench: `Server::halloween_override`); `None`: system
    /// local date, like the game.
    pub halloween: Option<bool>,
}

impl Default for Mobs {
    fn default() -> Mobs {
        Mobs {
            list: Vec::new(),
            index: index::MobIndex::default(),
            rng: jrand::JRand::from_time(0x5345_4153),
            player_hurt_by: None,
            player_hit: None,
            player_ticks: 0,
            attack_ticker: 0,
            last_main_hand: None,
            player_vel: [0.0; 3],
            adopted: hashbrown::HashSet::new(),
            charged_skull: None,
            killer: None,
            phantom_next: 0,
            rest_dead: false,
            hit_by: None,
            shoulder: [None, None],
            shoulder_time: i64::MIN / 2,
            shoulder_loaded: false,
            cubes: cubes::CubesWorld::default(),
            mounts: mounts::World::default(),
            spawners: spawn::CustomSpawners::default(),
            poi: crate::poi::Pois::default(),
            npc: villager::NpcWorld::default(),
            ill: illagers::IllWorld::default(),
            aqua: aquatic::AquaWorld::default(),
            sea: sealife::World::default(),
            brainy: brainy::World::default(),
            wild: wild::hive::HiveWorld::default(),
            fireballs: fireballs::FireballsWorld::default(),
            ticking: Vec::new(),
            ghosts: Vec::new(),
            pig: piglins::PigWorld::default(),
            rec: recent::RecWorld::default(),
            end: end::EndWorld::default(),
            wither: wither::WitherWorld::default(),
            bells: Vec::new(),
            halloween: Server::halloween_override(),
        }
    }
}

impl Mobs {
    /// Added at the end of the list (index kept up to date).
    pub(crate) fn push(&mut self, m: Box<Mob>) {
        self.index.pushed(&m, &self.list);
        self.list.push(Some(m));
    }

    /// Mob of the list with identifier `id`, mutable.
    #[inline]
    pub(crate) fn get_mut(&mut self, id: i32) -> Option<&mut Mob> {
        let i = self.index_of(id)?;
        self.list[i].as_deref_mut()
    }

    /// First mob of the list with UUID `u` that satisfies `pred` (same result as
    /// `list.iter().flatten().find(|o| o.uuid == u && pred(o))`).
    pub(crate) fn find_uuid_where(&self, u: u128, pred: impl Fn(&Mob) -> bool) -> Option<&Mob> {
        let i = self.index_of_uuid(u)?;
        // No slot before `i` carries `u`: the scan resumes at `i`.
        self.list[i..].iter().flatten().find(|o| o.uuid == u && pred(o)).map(|b| &**b)
    }

    /// First slot of the list carrying the identifier `id`.
    #[inline]
    pub(crate) fn index_of(&self, id: i32) -> Option<usize> {
        self.index.find(&self.list, id)
    }

    /// Mob of the list with identifier `id` (frozen copies excluded).
    #[inline]
    pub(crate) fn get(&self, id: i32) -> Option<&Mob> {
        self.index_of(id).and_then(|i| self.list[i].as_deref())
    }

    /// First slot of the list carrying the UUID `u`.
    #[inline]
    pub(crate) fn index_of_uuid(&self, u: u128) -> Option<usize> {
        self.index.find_uuid(&self.list, u)
    }

    /// Mob of the list with UUID `u`.
    #[inline]
    pub(crate) fn get_uuid(&self, u: u128) -> Option<&Mob> {
        self.index_of_uuid(u).and_then(|i| self.list[i].as_deref())
    }

    /// Mobs of species `k` in the list's order (empty slots skipped): same sequence as
    /// `list.iter().flatten().filter(|o| o.kind == k)`, through the per-species index when it is reliable.
    pub(crate) fn of_kind(&self, k: Kind) -> impl Iterator<Item = &Mob> {
        let slots = self.index.kind_slots(&self.list, k);
        #[cfg(test)]
        if index::verify()
            && let Some(s) = slots
        {
            let fast: Vec<i32> = s.iter().filter_map(|&i| self.list[i as usize].as_deref()).map(|o| o.id).collect();
            let scan: Vec<i32> = self.list.iter().flatten().filter(|o| o.kind == k).map(|o| o.id).collect();
            assert_eq!(fast, scan, "per-species index: {k:?} wrong");
        }
        // Only one of the two sequences is non-empty: indexed slots, or the whole list.
        let (s, all) = match slots {
            Some(s) => (s, &self.list[..0]),
            None => (&[][..], &self.list[..]),
        };
        s.iter()
            .filter_map(|&i| self.list[i as usize].as_deref())
            .chain(all.iter().flatten().map(|b| &**b))
            .filter(move |o| o.kind == k)
    }
}

impl Server {
    /// Mobs read with an entity chunk: those of the simulated species come alive.
    pub(crate) fn adopt_mobs(&mut self, loaded: &[(i32, i32)]) {
        for p in loaded {
            if !self.mobs.adopted.insert(*p) {
                continue;
            }
            let Some(list) = self.entities.get_mut(p) else { continue };
            type Ents = Vec<(i32, Nbt)>;
            let (mine, rest): (Ents, Ents) = std::mem::take(list)
                .into_iter()
                .partition(|(_, e)| e.get("id").and_then(Nbt::as_str).is_some_and(|n| Kind::from_name(n).is_some()));
            *list = rest;
            for (id, e) in mine {
                // F04: passengers recorded in their vehicle (`Passengers`).
                self.adopt_with_passengers(id, &e, None);
            }
        }
        self.end_adopt(loaded); // F15: crystals, clouds, dragon balls, bullets
    }

    /// Mobs to write into the entity chunks (with the dropped items of S2).
    pub(crate) fn mobs_into_chunks(&self, by_chunk: &mut std::collections::BTreeMap<(i32, i32), Vec<Nbt>>) {
        for m in self.mobs.list.iter().flatten() {
            // F04: a passenger is written in its vehicle.
            if m.removed
                || m.dead
                || m.mount.vehicle.is_some_and(|v| self.mob_ref(v).is_some() || self.cart_ref(v).is_some())
                || self.leaves_with_player(m)
            {
                continue;
            }
            let c = ((m.pos[0].floor() as i32) >> 4, (m.pos[2].floor() as i32) >> 4);
            by_chunk.entry(c).or_default().push(self.save_with_passengers(m));
        }
        self.end_into_chunks(by_chunk); // F15
    }

    /// Mob read (and its simulated passengers, recursively); the others stay as NBT.
    pub(crate) fn adopt_with_passengers(&mut self, id: i32, e: &Nbt, vehicle: Option<&mut Mob>) -> Option<i32> {
        let kind = Kind::from_name(e.get("id").and_then(Nbt::as_str).unwrap_or(""))?;
        let id = self.end_adopt_id(kind, id); // F15: dragon parts
        let mut rng = jrand::JRand::from_time(i64::from(id));
        // UUID drawn at construction (kept if the NBT has none, like the game): version 4.
        let uuid = (rng.next_long() as u128) << 64 | rng.next_long() as u64 as u128;
        let uuid = (uuid & !(0xF000u128 << 64) | (0x4000u128 << 64)) & !(0xC000_0000_0000_0000u128)
            | 0x8000_0000_0000_0000u128;
        let mut m = Mob::new(kind, id, uuid, [0.0; 3], 0.0, rng);
        let mut e = e.clone();
        let passengers = mounts::riding::take_passengers(&mut e);
        m.load(&e);
        self.enderman_load_carried(&mut m, &e);
        // UUID already present (vehicle returned with the player): duplicate ignored.
        if m.uuid != 0 && self.mobs.index_of_uuid(m.uuid).is_some() {
            return None;
        }
        self.init_mob(&mut m);
        // Illager placed by structure generation: finalised only once (the marker is never
        // rewritten).
        if e.get("rc_finalize_structure").is_some() {
            let mut g = spawn::Group::default();
            self.finalize_spawn(&mut m, spawn::Reason::Structure, &mut g);
            // Marker removed from the kept compound (never rewritten on save).
            if let Some(n) = &mut m.nbt {
                crate::nbtio::remove(n, "rc_finalize_structure");
            }
        }
        if kind.skeleton_like() {
            brains::reassess_weapon(self, &mut m);
        }
        if let Some(v) = vehicle {
            m.mount.vehicle = Some(v.id);
            v.mount.passengers.push(Ent::Mob(m.id));
            if kind.non_controlling_rider() {
                v.mount.passive_riders.push(m.id);
            }
        }
        for p in passengers {
            if p.get("id").and_then(Nbt::as_str).and_then(Kind::from_name).is_some() {
                self.next_entity += 1;
                let pid = self.next_entity;
                self.adopt_with_passengers(pid, &p, Some(&mut m));
            } else {
                m.mount.foreign_passengers.push(p);
            }
        }
        let mid = m.id;
        self.mobs.push(Box::new(m));
        Some(mid)
    }

    /// NBT of a mob with its passengers (`Passengers`).
    pub(crate) fn save_with_passengers(&self, m: &Mob) -> Nbt {
        let mut e = self.mob_nbt(m);
        self.enderman_save_carried(m, &mut e);
        let mut list: Vec<Nbt> = Vec::new();
        for p in &m.mount.passengers {
            if let Ent::Mob(pid) = p
                && let Some(o) = self.mob_ref(*pid)
                && !o.removed
                && !o.dead
            {
                list.push(self.save_with_passengers(o));
            }
        }
        list.extend(m.mount.foreign_passengers.iter().cloned());
        if list.is_empty() {
            crate::nbtio::remove(&mut e, "Passengers");
        } else {
            crate::nbtio::set(&mut e, "Passengers", Nbt::List(list));
        }
        e
    }

    /// Packets specific to mobs (attack, interaction, void hit, eggs); true if handled.
    pub(crate) fn mobs_packet(&mut self, p: &[u8]) -> bool {
        // F04: packets of the mounted player (vehicle, keys, rotation only).
        if self.mount_packet(p) {
            return true;
        }
        let ids = self.peer.ids();
        let mut r = Reader::new(p);
        let Ok(id) = read_varint(&mut r) else { return false };
        let r = &mut r;
        if id == ids.attack {
            let Ok(e) = read_varint(r) else { return true };
            // F12: hit on a fireball (deflecting the ghast's big fireball).
            // F15: dragon part, crystal, shulker bullet.
            // G5-F06: minecart. Paintings, frames, armor stands, cushions.
            if self.player_ready()
                && !self.fireball_attacked(e)
                && !self.end_attacked(e)
                && !self.cart_attacked(e)
                && !self.deco_attacked(e)
            {
                self.player_attack(e);
            }
            true
        } else if id == ids.interact {
            let (Ok(e), Ok(hand)) = (read_varint(r), read_varint(r)) else { return true };
            // Point touched (relative to the entity) and sneaking.
            let at = rc_proto_v777::entity::read_lp_vec3(r).unwrap_or_default();
            let secondary = { use rc_proto::Decode; bool::decode(r).unwrap_or(false) };
            // G5-F06: minecart; decorations.
            if self.player_ready() && !self.cart_interact(e, hand) && !self.deco_interact(e, hand, at, secondary) {
                self.player_interact(e, hand);
            }
            true
        } else if id == ids.select_trade {
            // F05: choice of an offer in the trade screen.
            let Ok(i) = read_varint(r) else { return true };
            self.merchant_select(i);
            true
        } else if id == ids.punch {
            self.mobs.attack_ticker = 0;
            true
        } else if id == ids.use_item_on {
            self.egg_use_item_on(r)
        } else {
            false
        }
    }

    /// Spawn egg of a simulated species placed on a block; otherwise left to normal handling.
    fn egg_use_item_on(&mut self, r: &mut Reader<'_>) -> bool {
        use rc_proto::{BlockPos, Decode};
        // Demo over: the common handling refuses the use.
        if self.demo_world && self.demo.ended {
            return false;
        }
        let Ok(hand) = read_varint(r) else { return false };
        let Ok(bp) = BlockPos::decode(r) else { return false };
        let Ok(face) = read_varint(r) else { return false };
        let _ = (f32::decode(r), f32::decode(r), f32::decode(r), bool::decode(r), bool::decode(r));
        let Ok(seq) = read_varint(r) else { return false };
        let slot = if hand == 0 { 36 + self.held as usize } else { 45 };
        let Some(item) = self.slots[slot].as_ref().map(|s| s.id.clone()) else { return false };
        // F03: sulfur cube bucket emptied on the targeted face.
        if item == "minecraft:sulfur_cube_bucket" && self.game_mode != 3 && self.player_ready() {
            self.acknowledge(seq);
            if self.sulfur_bucket_use(slot, [bp.x, bp.y, bp.z], face) {
                self.broadcast_menus();
            }
            return true;
        }
        // F11: harvesting a full beehive (shears, bottle); sneaking with an item in hand,
        // the block is not used.
        if self.game_mode != 3
            && !self.sneaking
            && self.player_ready()
            && self.hive_use_item_on(slot, [bp.x, bp.y, bp.z], &item)
        {
            self.acknowledge(seq);
            return true;
        }
        // Adventure: crystal and egg only act on a block accepted by `can_place_on`.
        let adventure_refused = self.game_mode == 2
            && !self.adventure_can_place_on(self.slots[slot].as_ref(), self.get([bp.x, bp.y, bp.z]));
        if adventure_refused && (item == "minecraft:end_crystal" || item.ends_with("_spawn_egg")) {
            self.acknowledge(seq);
            return true;
        }
        // F15: End crystal placed on obsidian or bedrock.
        if self.game_mode != 3 && self.player_ready() && item == "minecraft:end_crystal" {
            self.acknowledge(seq);
            self.end_use_item_on(slot, &item, [bp.x, bp.y, bp.z]);
            return true;
        }
        if !item.ends_with("_spawn_egg") {
            return false;
        }
        let name = format!("minecraft:{}", item.trim_start_matches("minecraft:").trim_end_matches("_spawn_egg"));
        if Kind::from_name(&name).is_none() || self.game_mode == 3 || !self.player_ready() {
            return false;
        }
        self.acknowledge(seq);
        let pos = [bp.x, bp.y, bp.z];
        if self.use_spawn_egg(&item, pos, face) && self.game_mode != 1 {
            if let Some(s) = &mut self.slots[slot] {
                s.count -= 1;
                if s.count <= 0 {
                    self.slots[slot] = None;
                }
            }
            self.broadcast_menus();
        }
        true
    }

    /// Resets the attack strength when the item in hand changes.
    pub(crate) fn watch_main_hand(&mut self) {
        let now = self.slots[36 + self.held as usize].as_ref().map(|s: &Stack| (s.id.clone(), s.count));
        if now != self.mobs.last_main_hand {
            self.mobs.last_main_hand = now;
            self.mobs.attack_ticker = 0;
        }
    }
}
