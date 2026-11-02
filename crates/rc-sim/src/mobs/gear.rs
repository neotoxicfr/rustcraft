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

/// Break event of each slot (main hand, off hand, feet, legs, chest, head, body, saddle).
pub(crate) const BREAK_EVENTS: [i8; 8] = [47, 48, 52, 51, 50, 49, 65, 68];
