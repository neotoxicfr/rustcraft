//! G5-F04: attribute modifiers of a mob's equipment (armor, toughness, knockback resistance,
//! weapon damage, enchantment attribute effects such as the burning time of Fire Protection),
//! tracked tick after tick like the game: each slot (main hand → saddle) is compared with the
//! stack seen on the previous tick; for each changed slot the old stack's modifiers are removed,
//! then those of the new stack (if it is not broken) replace any modifier with the same
//! identifier. Transient modifiers: never saved. Only modeled attributes receive them.

use super::mob::{Mob, SLOT_NAMES, intern};
use crate::player::Stack;
use crate::server::Server;

impl Server {
    /// Equipment changes since the previous tick: modifiers updated (after the item in use,
    /// before the AI step, like the game).
    pub(crate) fn mob_gear_tick(&mut self, m: &mut Mob) {
        // Nothing worn and nothing seen: the case of almost every mob, without any work.
        if m.gear_seen.is_none() && m.equipment.iter().all(Option::is_none) {
            return;
        }
        let mut seen = m.gear_seen.take().unwrap_or_default();
        let mut changed = [false; 8];
        for (k, flag) in changed.iter_mut().enumerate() {
            if crate::items::matches(seen[k].as_ref(), m.equipment[k].as_ref()) {
                continue;
            }
            *flag = true;
            if let Some(old) = seen[k].take() {
                self.mob_remove_stack_mods(m, &old, k);
            }
        }
        for k in (0..8).filter(|&k| changed[k]) {
            let cur = m.equipment[k].clone();
            if let Some(s) = &cur
                && !self.stack_broken(s)
            {
                for md in self.stack_mods(s, SLOT_NAMES[k]) {
                    if let Some(a) = m.attrs.by_name_mut(&md.attr) {
                        let id = intern(&md.id);
                        a.remove_mod(id);
                        a.set_mod(id, md.amount, md.op);
                    }
                }
            }
            seen[k] = cur;
        }
        if seen.iter().any(Option::is_some) {
            m.gear_seen = Some(seen);
        }
    }

    /// Modifiers of the stack `s` worn in the slot `k` removed (change of item, break).
    pub(crate) fn mob_remove_stack_mods(&self, m: &mut Mob, s: &Stack, k: usize) {
        for md in self.stack_mods(s, SLOT_NAMES[k]) {
            if let Some(a) = m.attrs.by_name_mut(&md.attr) {
                a.remove_mod(intern(&md.id));
            }
        }
    }

    /// Wear of the item worn by a mob in the slot `k` (Unbreaking: formula of the item's kind,
    /// world random); broken: stack shrunk, its modifiers removed at once, break event of the
    /// slot, equipment sent back.
    pub(crate) fn mob_hurt_slot(&mut self, m: &mut Mob, k: usize, n: i32) {
        let Some(s) = m.equipment[k].clone() else { return };
        if !self.life.items.damageable(&s) {
            return;
        }
        let armor = self.life.items.has_tag("minecraft:enchantable/armor", &s.id);
        let n = self.unbreaking_kept(&s, n, armor);
        if n <= 0 {
            return;
        }
        let dmg = self.life.items.damage(&s) + n;
        let mut s = s;
        let max = self.life.items.max_damage(&s);
        if dmg >= max {
            self.mob_remove_stack_mods(m, &s, k);
            s.count -= 1;
            // The rest of the stack keeps the wear, capped at the durability (broken too).
            crate::items::set_component(&mut s, "minecraft:damage", rc_proto::nbt::Nbt::Int(max));
            m.equipment[k] = (s.count > 0).then_some(s);
            let event = BREAK_EVENTS[k];
            let _ = self.send(rc_client::Event::Entity(rc_client::EntityEvent::Event { id: m.id, event }));
        } else {
            crate::items::set_component(&mut s, "minecraft:damage", rc_proto::nbt::Nbt::Int(dmg));
            m.equipment[k] = Some(s);
        }
        self.send_mob_equipment(m, &[k]);
    }
}

impl Server {
    /// Writes equipment slot `k` of a mob the way every living entity of the game does, and
    /// returns the stack it held. After the mob's first tick, a new item (or the same item with
    /// other components) plays the equip sound of the new stack when that stack is worn in this
    /// very slot, unless the mob is silent, and gives off the `equip` game event (a wearable
    /// stack) or `unequip` (anything else, an emptied slot included). A skeleton then chooses its
    /// weapon goal again, whatever the slot. The caller sends the equipment to the clients as
    /// before.
    pub(crate) fn mob_set_item_slot(&mut self, m: &mut Mob, k: usize, new: Option<Stack>) -> Option<Stack> {
        let old = std::mem::replace(&mut m.equipment[k], new);
        self.mob_equip_reaction(m, k, old.as_ref());
        if m.kind.skeleton_like() {
            self.reassess_skeleton_weapon(m);
        }
        old
    }

    /// The skeleton's weapon goals rebuilt after an equipment change, like the game: the running
    /// one is stopped (a drawn bow is let go), both are removed, then the goal the held item calls
    /// for is added at the end of the list.
    fn reassess_skeleton_weapon(&mut self, m: &mut Mob) {
        use super::goals::Goal;
        let mut goals = std::mem::take(&mut m.goals);
        for w in goals.goals.iter_mut().filter(|w| w.running && matches!(w.goal, Goal::Bow { .. } | Goal::Melee { .. })) {
            w.running = false;
            self.goal_stop(&mut w.goal, m);
        }
        m.goals = goals;
        super::brains::reassess_weapon(self, m);
    }

    fn mob_equip_reaction(&mut self, m: &mut Mob, k: usize, old: Option<&Stack>) {
        // Not ticked yet (just spawned, converted or loaded): the game equips in silence then.
        if m.tick_count == 0 {
            return;
        }
        let unchanged = match (old, m.equipment[k].as_ref()) {
            (None, None) => true,
            (Some(a), Some(b)) => crate::items::same(a, b),
            _ => false,
        };
        if unchanged {
            return;
        }
        let wearable = m.equipment[k].as_ref().and_then(|s| self.equippable(s));
        if let Some(e) = &wearable
            && e.slot == k
            && !m.x.silent
        {
            let sound = equip_sound(m, k, e);
            let seed = m.rng.next_long();
            self.mob_sound_with_seed(m, &sound, seed);
        }
        let event = if wearable.is_some() { "minecraft:equip" } else { "minecraft:unequip" };
        self.mob_game_event(m, event);
    }

    /// The game's common routine for a mob letting go of its preserved equipment (a copper golem
    /// turning into a statue, an ominous trial spawner's mob): every stack with a guaranteed drop
    /// leaves its slot through the equipment reaction (an `unequip` vibration each), then falls at
    /// the mob's feet.
    pub(crate) fn drop_preserved_equipment(&mut self, m: &mut Mob) {
        self.drop_preserved_equipment_unless(m, |_, _| false);
    }

    /// [`Server::drop_preserved_equipment`], except the stacks `held_back` picks: those stay in
    /// their slot whatever their drop chance, and their slots are returned in slot order.
    pub(crate) fn drop_preserved_equipment_unless(
        &mut self,
        m: &mut Mob,
        held_back: impl Fn(&Self, &Stack) -> bool,
    ) -> Vec<usize> {
        let mut kept = Vec::new();
        for i in 0..m.equipment.len() {
            let Some(stack) = &m.equipment[i] else { continue };
            if held_back(self, stack) {
                kept.push(i);
            } else if m.drop_chance[i] > 1.0
                && let Some(dropped) = self.mob_set_item_slot(m, i, None)
            {
                self.drop_at_height(m, dropped, 0.0);
            }
        }
        kept
    }

    /// Sound at a mob's position, in its sound source, with a seed drawn by the caller.
    fn mob_sound_with_seed(&self, m: &Mob, name: &str, seed: i64) {
        use rc_proto_v777::play::{SoundPacket, SoundRef};
        let Some(id) = self.sound_id(name) else { return };
        let _ = self.send(rc_client::Event::Sound(SoundPacket {
            sound: SoundRef::Id(id),
            category: Self::mob_sound_source(m),
            pos: Some(m.pos()),
            entity: None,
            volume: 1.0,
            pitch: 1.0,
            seed,
        }));
    }
}

/// Equip sound of stack `e` put in slot `k` of `m`: the saddle has the species' own sound, every
/// other slot the stack's (the generic armor sound when the component names none).
fn equip_sound(m: &Mob, k: usize, e: &super::mounts::feed::Equippable) -> String {
    use super::species::Kind;
    let saddle = match m.kind {
        _ if k != super::mounts::SADDLE => None,
        Kind::Camel => Some("minecraft:entity.camel.saddle"),
        Kind::CamelHusk => Some("minecraft:entity.camel_husk.saddle"),
        Kind::Strider => Some("minecraft:entity.strider.saddle"),
        Kind::Pig => Some("minecraft:entity.pig.saddle"),
        Kind::Nautilus | Kind::ZombieNautilus if super::sealife::under_water(m) => {
            Some("minecraft:item.nautilus_saddle_underwater_equip")
        }
        Kind::Nautilus | Kind::ZombieNautilus => Some("minecraft:item.nautilus_saddle_equip"),
        kind if super::mounts::handles(kind) => Some("minecraft:entity.horse.saddle"),
        _ => None,
    };
    match saddle {
        Some(name) => name.to_owned(),
        None => e.sound.clone().unwrap_or_else(|| "minecraft:item.armor.equip_generic".to_owned()),
    }
}

/// Break event of each slot (main hand, off hand, feet, legs, chest, head, body, saddle).
pub(crate) const BREAK_EVENTS: [i8; 8] = [47, 48, 52, 51, 50, 49, 65, 68];
