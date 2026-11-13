//! Which item a mob would rather wear or hold, as every mob of the game decides it: the slot an
//! item belongs to, whether the mob may use that slot, the comparison of a new item with the one
//! already there (armor then toughness; weapon kind the species prefers, then damage; then more
//! enchantments, less wear, a name) and the equipping itself. Shared by the common pickup of
//! ground items, the piglins' pickup and admiring, and the mounts' armor and saddles.

use super::mob::{Mob, SLOT_NAMES};
use super::mounts::{BODY, SADDLE, is_tamed};
use super::species::Kind;
use crate::player::Stack;
use crate::server::Server;

pub(crate) const MAIN_HAND: usize = 0;

/// Humanoid armor (feet, legs, chest, head) and the body armor of animals.
fn is_armor_slot(slot: usize) -> bool {
    (2..=BODY).contains(&slot)
}

/// Most items a slot holds: no limit in the hands, one piece everywhere else.
fn slot_count_limit(slot: usize, count: i32) -> i32 {
    if slot <= 1 { count } else { count.min(1) }
}

/// The species' own rule on its slots: a saddle or body armor only on a living adult (tamed for
/// donkeys, mules, camels and nautiluses); every other slot is always usable.
pub(crate) fn species_has_slot(m: &Mob, slot: usize) -> bool {
    let adult = || m.alive() && !m.is_baby();
    match m.kind {
        Kind::Horse | Kind::Llama | Kind::TraderLlama | Kind::SkeletonHorse | Kind::ZombieHorse => true,
        Kind::Donkey | Kind::Mule | Kind::Camel | Kind::CamelHusk => slot != SADDLE || (adult() && is_tamed(m)),
        Kind::Nautilus | Kind::ZombieNautilus => (slot != SADDLE && slot != BODY) || (adult() && is_tamed(m)),
        Kind::Pig | Kind::Strider => slot != SADDLE || adult(),
        Kind::HappyGhast | Kind::SulfurCube => slot != BODY || adult(),
        _ => true,
    }
}

/// Item tag of the weapons the species prefers over any other in its main hand.
fn preferred_weapon_tag(m: &Mob) -> Option<&'static str> {
    match m.kind {
        Kind::Piglin if !m.is_baby() => Some("minecraft:piglin_preferred_weapons"),
        Kind::Drowned => Some("minecraft:drowned_preferred_weapons"),
        Kind::Pillager => Some("minecraft:pillager_preferred_weapons"),
        Kind::WitherSkeleton => None,
        k if k.skeleton_like() => Some("minecraft:skeleton_preferred_weapons"),
        _ => None,
    }
}

/// Number of enchantments the stack carries.
fn enchantment_count(st: &Stack) -> usize {
    use rc_proto::nbt::Nbt;
    match crate::items::component(st, "minecraft:enchantments") {
        // 26.3 form `{id: level}`; older form `{levels: {id: level}}`.
        Some(Nbt::Compound(fields)) => match fields.iter().find(|(k, _)| k == "levels") {
            Some((_, Nbt::Compound(levels))) => levels.len(),
            _ => fields.len(),
        },
        _ => 0,
    }
}

/// Same attribute, written with or without its namespace.
fn same_attribute(a: &str, b: &str) -> bool {
    a.trim_start_matches("minecraft:") == b.trim_start_matches("minecraft:")
}

impl Server {
    /// The engine slot the item's `equippable` component names, `None` without the component.
    /// Read through the item table's slot code rather than the whole component (which allocates
    /// its lists): the piglins ask it for every item they look at. Shared by the mobs and `/item`
    /// on the other living entities (armor stands, mannequins).
    pub(crate) fn slot_named_by_item(&self, st: &Stack) -> Option<usize> {
        // Slot codes of the item table (main hand, feet, legs, chest, head, off hand, body,
        // saddle) to the engine's slots.
        const ENGINE_SLOT: [usize; 8] = [0, 2, 3, 4, 5, 1, 6, 7];
        self.life.items.equip_slot(st).and_then(|code| ENGINE_SLOT.get(usize::from(code)).copied())
    }

    /// The slot an item goes to on this mob: the one its `equippable` component names when the
    /// mob may use it, the main hand otherwise.
    pub(crate) fn slot_item_is_worn_in(&self, m: &Mob, st: &Stack) -> usize {
        match self.slot_named_by_item(st) {
            Some(slot) if species_has_slot(m, slot) => slot,
            _ => MAIN_HAND,
        }
    }

    /// The item fits this slot of this mob: the slot its `equippable` component names, usable by
    /// the mob, and the mob among the entities the component admits; without the component, only
    /// the main hand.
    pub(crate) fn item_fits_slot(&self, m: &Mob, st: &Stack, slot: usize) -> bool {
        // Without the component (most items), only the main hand, and nothing to read.
        if self.life.items.equip_slot(st).is_none() {
            return slot == MAIN_HAND;
        }
        match self.equippable(st) {
            None => slot == MAIN_HAND,
            Some(e) => {
                e.slot == slot && species_has_slot(m, slot) && e.allowed.as_ref().is_none_or(|l| self.entity_in(m.kind, l))
            }
        }
    }

    /// The species may hold the item at all: a riding baby zombie no egg, a wither skeleton none
    /// of the weapons it dislikes.
    pub(crate) fn species_may_hold(&self, m: &Mob, st: &Stack) -> bool {
        if m.kind.zombie_like() && m.is_baby() && m.mount.vehicle.is_some() && self.item_tag(&st.id, "minecraft:eggs") {
            return false;
        }
        !(m.kind == Kind::WitherSkeleton && self.item_tag(&st.id, "minecraft:wither_skeleton_disliked_weapons"))
    }

    /// The new item replaces the one in `slot` (none: always). The piglin first weighs loved items
    /// and its preferred weapons, the drowned never gives up a nautilus shell; then armor
    /// compares defense, toughness, then the equal-item rule, and the main hand compares weapons.
    pub(crate) fn replaces_worn_item(&self, m: &Mob, new: &Stack, current: Option<&Stack>, slot: usize) -> bool {
        let Some(current) = current else { return true };
        match m.kind {
            Kind::Piglin => {
                if self.prevents_armor_change(current) {
                    return false;
                }
                let wanted = |st: &Stack| {
                    super::piglins::items::is_loved(self, st)
                        || preferred_weapon_tag(m).is_some_and(|tag| self.item_tag(&st.id, tag))
                };
                match (wanted(new), wanted(current)) {
                    (true, false) => return true,
                    (false, true) => return false,
                    _ => {}
                }
            }
            Kind::Drowned if current.id == "minecraft:nautilus_shell" => return false,
            _ => {}
        }
        if is_armor_slot(slot) {
            self.armor_is_better(m, new, current, slot)
        } else {
            slot == MAIN_HAND && self.weapon_is_better(m, new, current)
        }
    }

    /// Curse of binding (the only enchantment that forbids taking the armor off).
    fn prevents_armor_change(&self, st: &Stack) -> bool {
        self.life.items.enchantment(st, "minecraft:binding_curse") > 0
    }

    fn armor_is_better(&self, m: &Mob, new: &Stack, current: &Stack, slot: usize) -> bool {
        if self.prevents_armor_change(current) {
            return false;
        }
        let new_defense = self.attribute_with_item(m, new, "minecraft:armor", slot);
        let old_defense = self.attribute_with_item(m, current, "minecraft:armor", slot);
        if new_defense != old_defense {
            return new_defense > old_defense;
        }
        let new_toughness = self.attribute_with_item(m, new, "minecraft:armor_toughness", slot);
        let old_toughness = self.attribute_with_item(m, current, "minecraft:armor_toughness", slot);
        if new_toughness != old_toughness {
            return new_toughness > old_toughness;
        }
        self.wins_tie_against(new, current)
    }

    fn weapon_is_better(&self, m: &Mob, new: &Stack, current: &Stack) -> bool {
        if let Some(tag) = preferred_weapon_tag(m) {
            match (self.item_tag(&new.id, tag), self.item_tag(&current.id, tag)) {
                (false, true) => return false,
                (true, false) => return true,
                _ => {}
            }
        }
        let new_damage = self.attribute_with_item(m, new, "minecraft:attack_damage", MAIN_HAND);
        let old_damage = self.attribute_with_item(m, current, "minecraft:attack_damage", MAIN_HAND);
        if new_damage != old_damage {
            return new_damage > old_damage;
        }
        self.wins_tie_against(new, current)
    }

    /// Between two items of equal strength: more enchantments, then less wear, then a name the
    /// other lacks.
    fn wins_tie_against(&self, new: &Stack, current: &Stack) -> bool {
        let (new_enchantments, old_enchantments) = (enchantment_count(new), enchantment_count(current));
        if new_enchantments != old_enchantments {
            return new_enchantments > old_enchantments;
        }
        let (new_wear, old_wear) = (self.life.items.damage(new), self.life.items.damage(current));
        if new_wear != old_wear {
            return new_wear < old_wear;
        }
        crate::items::component(new, "minecraft:custom_name").is_some()
            && crate::items::component(current, "minecraft:custom_name").is_none()
    }

    /// The mob's base value of `attr` with only the item's own attribute modifiers for `slot`
    /// applied, in their order (enchantment effects left out, like the game's estimate).
    fn attribute_with_item(&self, m: &Mob, st: &Stack, attr: &str, slot: usize) -> f64 {
        let base = match attr {
            "minecraft:armor" => m.attrs.armor.base,
            "minecraft:armor_toughness" => m.attrs.toughness.base,
            "minecraft:attack_damage" => m.attrs.attack.base,
            _ => 0.0,
        };
        let mut value = base;
        self.for_each_item_modifier(st, SLOT_NAMES[slot], |modifier_attr, _, amount, operation| {
            if same_attribute(modifier_attr, attr) {
                value += match operation {
                    1 => amount * base,
                    2 => amount * value,
                    _ => amount,
                };
            }
        });
        value
    }

    /// How every mob of the game equips an item it takes: see `slot_to_equip` and `equip_in_slot`.
    /// Returns the count equipped (one piece outside the hands), `None` when the mob does not take
    /// the item.
    pub(crate) fn equip_if_wanted(&mut self, m: &mut Mob, item: &Stack) -> Option<i32> {
        let slot = self.slot_to_equip(m, item)?;
        Some(self.equip_in_slot(m, item.clone(), slot))
    }

    /// The decision of the equipping, with no side effect: the slot the item belongs to, or the
    /// main hand when the worn armor beats it and that hand is empty; `None` when the mob does not
    /// take the item. Kept apart so a caller reading the item in place copies it only once taken.
    pub(crate) fn slot_to_equip(&self, m: &Mob, item: &Stack) -> Option<usize> {
        let item_slot = self.slot_item_is_worn_in(m, item);
        let mut slot = item_slot;
        let mut can_replace = self.replaces_worn_item(m, item, m.equipment[slot].as_ref(), slot);
        if is_armor_slot(slot) && !can_replace {
            slot = MAIN_HAND;
            can_replace = m.equipment[MAIN_HAND].is_none();
        }
        // The one test that reads the whole `equippable` component (allocating its lists) comes
        // last: a mob standing by items it keeps refusing never pays for it.
        let taken = can_replace && self.species_may_hold(m, item) && self.item_fits_slot(m, item, item_slot);
        taken.then_some(slot)
    }

    /// Equips `item` in `slot` (chosen by `slot_to_equip`): the item replaced may fall (its drop
    /// chance against a draw lowered by 0.1); the new one, cut to one piece outside the hands, is
    /// set for a sure drop and the mob becomes persistent. Returns the count equipped.
    pub(crate) fn equip_in_slot(&mut self, m: &mut Mob, mut item: Stack, slot: usize) -> i32 {
        let drop_chance = m.drop_chance[slot];
        if let Some(old) = m.equipment[slot].clone()
            && (m.rng.next_float() - 0.1).max(0.0) < drop_chance
        {
            self.drop_at_height(m, old, 0.0);
        }
        item.count = slot_count_limit(slot, item.count);
        let count = item.count;
        self.mob_set_item_slot(m, slot, Some(item));
        m.drop_chance[slot] = 2.0;
        m.persistent = true;
        self.send_mob_equipment(m, &[slot]);
        count
    }
}
