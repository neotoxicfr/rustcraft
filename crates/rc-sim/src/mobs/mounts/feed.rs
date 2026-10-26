//! Player interactions with a mount: food (healing, growth, anger, love), taming, saddle,
//! armor, carpet, chest, mounting; golden dandelion (age lock); breeding (babies, inherited
//! attributes, coat, llama strength).

use rc_proto::nbt::Nbt;

use super::super::mob::Mob;
use super::super::species::Kind;
use super::{
    BODY, FLAG_EATING, SADDLE, age_up, can_age_up, chested, create_inventory, is_camel, is_llama, is_tamed, max_temper,
    modify_temper,
};
use crate::player::Stack;
use crate::server::Server;

/// `equippable` component of a stack (or the item's default): slot, allowed entities,
/// right-click placement, placement sound.
pub(crate) struct Equippable {
    pub(crate) slot: usize,
    pub(crate) allowed: Option<Vec<String>>,
    pub(crate) on_interact: bool,
    pub(crate) sound: Option<String>,
}

/// Engine slot from the component name.
fn slot_of(name: &str) -> Option<usize> {
    super::super::mob::slot_index(name)
}

impl Server {
    pub(crate) fn equippable(&self, s: &Stack) -> Option<Equippable> {
        if let Some(e) = crate::items::component(s, "minecraft:equippable") {
            let slot = slot_of(e.get("slot").and_then(Nbt::as_str)?)?;
            let allowed = match e.get("allowed_entities") {
                Some(Nbt::String(t)) => Some(vec![t.clone()]),
                Some(Nbt::List(l)) => Some(l.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()),
                _ => None,
            };
            let on_interact = crate::nbtio::int(e, "equip_on_interact").is_some_and(|v| v != 0);
            let sound = e.get("equip_sound").and_then(Nbt::as_str).map(str::to_owned);
            return Some(Equippable { slot, allowed, on_interact, sound });
        }
        let v = self.life.items.default_component(s, "minecraft:equippable")?;
        let slot = slot_of(v["slot"].as_str()?)?;
        let allowed = match &v["allowed_entities"] {
            serde_json::Value::String(t) => Some(vec![t.clone()]),
            serde_json::Value::Array(a) => Some(a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()),
            _ => None,
        };
        let on_interact = v["equip_on_interact"].as_bool().unwrap_or(false);
        let sound = match &v["equip_sound"] {
            serde_json::Value::String(t) => Some(t.clone()),
            o => o["sound_id"].as_str().map(str::to_owned),
        };
        Some(Equippable { slot, allowed, on_interact, sound })
    }

    /// Entity type in a tag (`tags/entity_type`) or a list.
    pub(crate) fn entity_in(&self, kind: Kind, list: &[String]) -> bool {
        let name = kind.info().name;
        list.iter().any(|e| match e.strip_prefix('#') {
            Some(tag) => {
                let Some(idx) = self.entity_kind_index(name) else {
                    return false;
                };
                self.regs
                    .tags
                    .iter()
                    .find(|(r, _)| r == "minecraft:entity_type")
                    .and_then(|(_, t)| t.iter().find(|(n, _)| n == tag))
                    .is_some_and(|(_, ids)| ids.contains(&(idx as i32)))
            }
            None => e == name,
        })
    }

    /// The saddle of donkeys, mules and camels requires a tamed adult animal.
    fn mount_can_use_slot(m: &Mob, slot: usize) -> bool {
        match m.kind {
            Kind::Horse | Kind::Llama | Kind::TraderLlama | Kind::SkeletonHorse | Kind::ZombieHorse => true,
            _ => slot != SADDLE || (m.alive() && !m.is_baby() && is_tamed(m)),
        }
    }

    /// The possible-equipment test.
    pub(crate) fn mount_equippable_in(&self, m: &Mob, s: &Stack, slot: usize) -> bool {
        match self.equippable(s) {
            None => slot == 0,
            Some(e) => {
                e.slot == slot
                    && Self::mount_can_use_slot(m, slot)
                    && e.allowed.as_ref().is_none_or(|l| self.entity_in(m.kind, l))
            }
        }
    }

    /// Equipment placement: placement sound (seed drawn from the entity random).
    pub(crate) fn mount_set_slot(&mut self, m: &mut Mob, slot: usize, s: Option<Stack>) {
        let changed = !crate::items::matches(m.equipment[slot].as_ref(), s.as_ref());
        m.equipment[slot] = s.clone();
        // Equipment changed: `set_equipment` to the players following the animal.
        if changed && m.track.seen {
            let slots = vec![(slot as u8, self.net(s.as_ref()))];
            let _ = self.send(rc_client::Event::Entity(rc_client::EntityEvent::Equipment { id: m.id, slots }));
        }
        let Some(s) = s else { return };
        if !changed {
            return;
        }
        let Some(e) = self.equippable(&s) else { return };
        if e.slot != slot {
            return;
        }
        let sound = if slot == SADDLE {
            match m.kind {
                Kind::Camel => Some("minecraft:entity.camel.saddle".to_owned()),
                Kind::CamelHusk => Some("minecraft:entity.camel_husk.saddle".to_owned()),
                // F12: strider.
                Kind::Strider => Some("minecraft:entity.strider.saddle".to_owned()),
                Kind::Nautilus | Kind::ZombieNautilus => Some(
                    if super::super::sealife::under_water(m) {
                        "minecraft:item.nautilus_saddle_underwater_equip"
                    } else {
                        "minecraft:item.nautilus_saddle_equip"
                    }
                    .to_owned(),
                ),
                _ => Some("minecraft:entity.horse.saddle".to_owned()),
            }
        } else {
            e.sound
        };
        let seed = m.rng.next_long();
        if let Some(name) = sound {
            self.seeded_sound(m, &name, 1.0, 1.0, seed);
        }
    }

    fn seeded_sound(&self, m: &Mob, name: &str, volume: f32, pitch: f32, seed: i64) {
        use rc_proto_v777::play::{SoundPacket, SoundRef};
        let Some(id) =
            self.data.registry("minecraft:sound_event").and_then(|r| r.entries.iter().position(|e| e == name))
        else {
            return;
        };
        let _ = self.send(rc_client::Event::Sound(SoundPacket {
            sound: SoundRef::Id(id as i32),
            category: 6,
            pos: Some(m.pos),
            entity: None,
            volume,
            pitch,
            seed,
        }));
    }

    /// One unit of the held stack removed (`consume`: nothing in creative).
    fn mount_consume(&mut self, slot: usize) {
        self.consume_held(slot);
    }

    fn holding_item(&self, id: &str) -> bool {
        let main = self.slots[36 + self.held as usize].as_ref().is_some_and(|s| s.id == id);
        let off = self.slots[45].as_ref().is_some_and(|s| s.id == id);
        main || off
    }

    /// Right click on a mount; true if the action is consumed.
    pub(crate) fn mount_interact(&mut self, m: &mut Mob, slot: usize) -> bool {
        // Zombie horse, camel husk: persistent as soon as a player interacts.
        if matches!(m.kind, Kind::ZombieHorse | Kind::CamelHusk) {
            m.persistent = true;
        }
        if is_camel(m.kind) {
            return self.camel_interact(m, slot);
        }
        if m.kind == Kind::SkeletonHorse {
            return is_tamed(m) && self.horse_interact(m, slot);
        }
        let held = self.slots[slot].clone();
        let should_open = !m.is_baby() && is_tamed(m) && self.sneaking;
        let dandelion = m.kind != Kind::ZombieHorse && m.is_baby() && self.holding_item("minecraft:golden_dandelion");
        if m.mount.passengers.is_empty()
            && !should_open
            && !dandelion
            && let Some(h) = &held
        {
            if self.item_tag(&h.id, super::food_tag(m.kind)) {
                return self.fed_food(m, slot);
            }
            if !is_tamed(m) {
                self.mount_make_mad(m);
                return true;
            }
            if chested(m.kind) && !m.mount.chest && h.id == "minecraft:chest" {
                m.mount.chest = true;
                let p = (m.rng.next_float() - m.rng.next_float()) * 0.2 + 1.0;
                let s = match m.kind {
                    Kind::Mule => "minecraft:entity.mule.chest",
                    Kind::Llama | Kind::TraderLlama => "minecraft:entity.llama.chest",
                    _ => "minecraft:entity.donkey.chest",
                };
                self.mount_sound(m, s, 1.0, p);
                self.mount_consume(slot);
                create_inventory(m);
                return true;
            }
        }
        self.horse_interact(m, slot)
    }

    /// The player's interaction with a mount.
    fn horse_interact(&mut self, m: &mut Mob, slot: usize) -> bool {
        if !m.mount.passengers.is_empty() || m.is_baby() {
            return self.animal_interact(m, slot);
        }
        if is_tamed(m) && self.sneaking {
            self.open_mount_menu(m);
            return true;
        }
        if let Some(h) = self.slots[slot].clone() {
            if self.equip_on_interact(m, slot, &h) {
                return true;
            }
            if self.mount_equippable_in(m, &h, BODY) && m.equipment[BODY].is_none() {
                // The creative stack stays whole.
                let one = Stack { count: 1, ..h.clone() };
                self.mount_consume(slot);
                m.drop_chance[BODY] = 2.0;
                self.mount_set_slot(m, BODY, Some(one));
                return true;
            }
        }
        self.do_player_ride(m);
        true
    }

    /// Placement through the `equippable` component (saddle); true if placed.
    fn equip_on_interact(&mut self, m: &mut Mob, slot: usize, h: &Stack) -> bool {
        let Some(e) = self.equippable(h) else { return false };
        if !e.on_interact || !self.mount_equippable_in(m, h, e.slot) || m.equipment[e.slot].is_some() || !m.alive() {
            return false;
        }
        // Placement by the item: in creative, the game works on a copy of the stack (nothing removed).
        let one = Stack { count: 1, ..h.clone() };
        self.mount_consume(slot);
        m.drop_chance[e.slot] = 2.0;
        self.mount_set_slot(m, e.slot, Some(one));
        true
    }

    /// The player's interaction with an animal (baby or occupied mount) then golden dandelion.
    fn animal_interact(&mut self, m: &mut Mob, slot: usize) -> bool {
        let Some(h) = self.slots[slot].clone() else { return false };
        if self.item_tag(&h.id, super::food_tag(m.kind)) {
            if m.age == 0 && can_fall_in_love(m) {
                self.mount_consume(slot);
                self.set_in_love(m);
                return true;
            }
            if can_age_up(m) {
                self.mount_consume(slot);
                let secs = super::feeding_secs(-m.age);
                age_up(m, secs, true);
                return true;
            }
        }
        self.dandelion_interact(m, slot, &h)
    }

    /// Golden dandelion on a baby (age lock toggled).
    fn dandelion_interact(&mut self, m: &mut Mob, slot: usize, h: &Stack) -> bool {
        let locked_out = matches!(m.kind, Kind::ZombieHorse | Kind::SkeletonHorse);
        if h.id != "minecraft:golden_dandelion" || !m.is_baby() || locked_out || m.bm.lock_timer != 0 {
            return false;
        }
        m.mount.age_locked = !m.mount.age_locked;
        m.age = -24000;
        m.bm.lock_timer = 40;
        self.mount_consume(slot);
        if m.mount.age_locked {
            m.persistent = true;
        }
        let s = if m.mount.age_locked {
            "minecraft:item.golden_dandelion.use"
        } else {
            "minecraft:item.golden_dandelion.unuse"
        };
        let p = m.block_pos().map(|v| f64::from(v) + 0.5);
        self.world_sound(s, 7, p, 1.0, 1.0);
        true
    }

    /// 600 ticks, event 18.
    fn set_in_love(&mut self, m: &mut Mob) {
        m.in_love = 600;
        m.love_cause = true;
        self.mob_event(m, 18);
    }

    /// Item consumed if it was used.
    fn fed_food(&mut self, m: &mut Mob, slot: usize) -> bool {
        let Some(h) = self.slots[slot].clone() else { return false };
        let ate = if is_llama(m.kind) { self.llama_eating(m, &h.id) } else { self.horse_eating(m, &h.id) };
        if ate {
            self.mount_consume(slot);
        }
        ate
    }

    /// The server's "particle" argument: three draws from the entity random, no effect.
    fn particle_draws(m: &mut Mob) {
        let _ = (m.rng.next_double(), m.rng.next_double(), m.rng.next_double());
    }

    /// A mount taking food.
    fn horse_eating(&mut self, m: &mut Mob, item: &str) -> bool {
        let mut used = false;
        let (heal, age, temper) = match item {
            "minecraft:wheat" => (2.0, 20, 3),
            "minecraft:sugar" => (1.0, 30, 3),
            "minecraft:hay_block" => (20.0, 180, 0),
            "minecraft:apple" => (3.0, 60, 3),
            "minecraft:red_mushroom" => (3.0, 0, 3),
            "minecraft:carrot" => (3.0, 60, 3),
            "minecraft:golden_carrot" => (4.0, 60, 5),
            "minecraft:golden_apple" | "minecraft:enchanted_golden_apple" => (10.0, 240, 10),
            _ => (0.0, 0, 0),
        };
        let love_food =
            matches!(item, "minecraft:golden_carrot" | "minecraft:golden_apple" | "minecraft:enchanted_golden_apple");
        if love_food && is_tamed(m) && m.age == 0 && m.in_love <= 0 {
            used = true;
            self.set_in_love(m);
        }
        if m.health < m.max_health() && heal > 0.0 {
            m.health = (m.health + heal).min(m.max_health());
            used = true;
        }
        if m.is_baby() && age > 0 && !m.mount.age_locked {
            Self::particle_draws(m);
            age_up(m, age, false);
            used = true;
        }
        if temper > 0 && (used || !is_tamed(m)) && m.mount.temper < max_temper(m.kind) {
            modify_temper(m, temper);
            used = true;
        }
        if used {
            self.mount_eating(m);
            self.mob_game_event(m, "minecraft:eat");
        }
        used
    }

    /// Without an open mouth.
    fn llama_eating(&mut self, m: &mut Mob, item: &str) -> bool {
        let mut used = false;
        let (age, temper, heal) = match item {
            "minecraft:wheat" => (10, 3, 2.0),
            "minecraft:hay_block" => (90, 6, 10.0),
            _ => (0, 0, 0.0),
        };
        if item == "minecraft:hay_block" && is_tamed(m) && m.age == 0 && can_fall_in_love(m) {
            used = true;
            self.set_in_love(m);
        }
        if m.health < m.max_health() && heal > 0.0 {
            m.health = (m.health + heal).min(m.max_health());
            used = true;
        }
        if m.is_baby() && age > 0 && !m.mount.age_locked {
            Self::particle_draws(m);
            age_up(m, age, false);
            used = true;
        }
        if temper > 0 && (used || !is_tamed(m)) && m.mount.temper < max_temper(m.kind) {
            modify_temper(m, temper);
            used = true;
        }
        if used {
            let p = 1.0 + (m.rng.next_float() - m.rng.next_float()) * 0.2;
            self.mount_sound(m, "minecraft:entity.llama.eat", 1.0, p);
        }
        used
    }

    /// The player's interaction with a camel.
    fn camel_interact(&mut self, m: &mut Mob, slot: usize) -> bool {
        if self.sneaking && !m.is_baby() {
            self.open_mount_menu(m);
            return true;
        }
        let held = self.slots[slot].clone();
        if let Some(h) = &held {
            if self.equip_on_interact(m, slot, h) {
                return true;
            }
            if self.item_tag(&h.id, super::food_tag(m.kind)) {
                let ate = self.camel_eating(m);
                if ate {
                    self.mount_consume(slot);
                }
                return ate;
            }
        }
        if m.mount.passengers.len() < 2 && !m.is_baby() {
            self.do_player_ride(m);
            return true;
        }
        if m.is_baby() && self.holding_item("minecraft:golden_dandelion") {
            return self.horse_interact(m, slot);
        }
        false
    }

    /// A camel taking food (food already checked).
    fn camel_eating(&mut self, m: &mut Mob) -> bool {
        let heal = m.health < m.max_health();
        if heal {
            m.health = (m.health + 2.0).min(m.max_health());
        }
        let love = m.age == 0 && can_fall_in_love(m);
        if love {
            self.set_in_love(m);
        }
        let grow = can_age_up(m);
        if grow {
            Self::particle_draws(m);
            age_up(m, 10, false);
        }
        if !heal && !love && !grow {
            return false;
        }
        if let Some(s) = super::eating_sound(m) {
            let p = 1.0 + (m.rng.next_float() - m.rng.next_float()) * 0.2;
            self.mount_sound(m, s, 1.0, p);
        }
        self.mob_game_event(m, "minecraft:eat");
        true
    }

    /// Neither meal nor rearing, the player takes the animal's rotation and mounts.
    fn do_player_ride(&mut self, m: &mut Mob) {
        // Trader llama held by the trader: refusal (leash not simulated, never the case).
        m.mount.set_flag(FLAG_EATING, false);
        super::clear_standing(m);
        self.yaw = m.yaw;
        self.pitch = m.pitch;
        self.player_start_riding(m);
    }

    // ---------- Breeding ----------

    /// Birth; true if the species is in the family.
    pub(crate) fn mount_breed(&mut self, m: &mut Mob, partner: i32) -> bool {
        if !super::handles(m.kind) {
            return false;
        }
        let Some(pi) = self.mob_index(partner) else { return true };
        let Some(p) = self.mobs.list[pi].as_ref().map(|p| (**p).clone()) else { return true };
        let child_kind = match (m.kind, p.kind) {
            (Kind::Horse, Kind::Donkey) | (Kind::Donkey, Kind::Horse) => Kind::Mule,
            (k, _) => k,
        };
        let mut baby = self.new_mob(child_kind, m.pos, 0.0);
        match (m.kind, child_kind) {
            (Kind::Horse, Kind::Horse) => {
                let pick = m.rng.next_int_bound(9);
                let coat = if pick < 4 {
                    m.variant & 0xFF
                } else if pick < 8 {
                    p.variant & 0xFF
                } else {
                    m.rng.next_int_bound(7)
                };
                let pick = m.rng.next_int_bound(5);
                let marks = if pick < 2 {
                    (m.variant & 0xFF00) >> 8
                } else if pick < 4 {
                    (p.variant & 0xFF00) >> 8
                } else {
                    m.rng.next_int_bound(5)
                };
                baby.variant = coat & 0xFF | (marks << 8) & 0xFF00;
                offspring_attributes(m, &p, &mut baby);
            }
            (Kind::Llama | Kind::TraderLlama, _) => {
                offspring_attributes(m, &p, &mut baby);
                let mut st = m.rng.next_int_bound(m.mount.strength.max(p.mount.strength)) + 1;
                if m.rng.next_float() < 0.03 {
                    st += 1;
                }
                baby.mount.strength = st.clamp(1, 5);
                baby.variant = if m.rng.next_bool() { m.variant } else { p.variant };
                create_inventory(&mut baby);
                if child_kind == Kind::TraderLlama {
                    baby.persistent = true;
                }
            }
            (Kind::Mule | Kind::Camel | Kind::CamelHusk, _) => {}
            _ => offspring_attributes(m, &p, &mut baby),
        }
        baby.age = -24000;
        baby.pos = m.pos;
        baby.yaw = 0.0;
        baby.pitch = 0.0;
        self.s9_bred(m, partner, Some(&baby)); // S9 (before love is forgotten, which erases its cause)
        m.age = 6000;
        m.reset_love();
        if let Some(o) = self.mobs.list[pi].as_mut() {
            o.age = 6000;
            o.reset_love();
        }
        self.mob_event(m, 18);
        if self.rule("minecraft:mob_drops", true) {
            let xp = m.rng.next_int_bound(7) + 1;
            self.spawn_orbs(m.pos, xp);
        }
        self.add_mob(baby);
        true
    }
}

/// Never for the zombie horse or the camel husk.
pub fn can_fall_in_love(m: &Mob) -> bool {
    !matches!(m.kind, Kind::ZombieHorse | Kind::CamelHusk) && m.in_love <= 0
}

/// Neither mount nor passenger, tamed, adult, at full health, in love.
pub fn can_parent(m: &Mob) -> bool {
    m.mount.passengers.is_empty()
        && m.mount.vehicle.is_none()
        && is_tamed(m)
        && !m.is_baby()
        && m.health >= m.max_health()
        && m.in_love > 0
}

/// The possible-mating test of mounts (partner searched among equines); `None` outside the family.
pub fn can_mate(m: &Mob, o: &Mob) -> Option<bool> {
    if !super::handles(m.kind) {
        return None;
    }
    if !super::handles(o.kind) || o.id == m.id {
        return Some(false);
    }
    Some(match m.kind {
        Kind::Horse | Kind::Donkey => matches!(o.kind, Kind::Horse | Kind::Donkey) && can_parent(m) && can_parent(o),
        Kind::Llama | Kind::TraderLlama => is_llama(o.kind) && can_parent(m) && can_parent(o),
        _ => false,
    })
}

/// HP, jump, speed (random draw of parent `m`).
fn offspring_attributes(m: &mut Mob, p: &Mob, baby: &mut Mob) {
    let (hl, hh) = super::health_range();
    let h = super::offspring_attribute(m.attrs.max_health.base, p.attrs.max_health.base, hl, hh, &mut m.rng);
    super::set_max_health(baby, h);
    let (jl, jh) = super::jump_range();
    baby.attrs.jump.base = super::offspring_attribute(m.attrs.jump.base, p.attrs.jump.base, jl, jh, &mut m.rng);
    let (sl, sh) = super::speed_range();
    baby.attrs.speed.base = super::offspring_attribute(m.attrs.speed.base, p.attrs.speed.base, sl, sh, &mut m.rng);
}
