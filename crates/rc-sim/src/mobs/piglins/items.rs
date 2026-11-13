//! Piglin gold: loved items, admiring, bartering (table `gameplay/piglin_bartering`),
//! pickup, 8-slot inventory, equipment, item throws and
//! piglin anger when a container is opened or a guarded block is broken.

use super::super::brainy::brain::Brain;
use super::super::ground_items::in_pickup_reach;
use super::super::mob::{Ent, Mob};
use super::super::species::Kind;
use super::INVENTORY_SIZE;
use crate::player::Stack;
use crate::server::Server;

/// Loved item (`#piglin_loved`).
pub fn is_loved(s: &Server, st: &Stack) -> bool {
    s.item_tag(&st.id, "minecraft:piglin_loved")
}

fn is_currency(st: &Stack) -> bool {
    st.id == "minecraft:gold_ingot"
}

fn is_food(s: &Server, st: &Stack) -> bool {
    s.item_tag(&st.id, "minecraft:piglin_food")
}

/// Item held in either hand.
pub fn holds(m: &Mob, id: &str) -> bool {
    m.equipment[0].as_ref().is_some_and(|s| s.id == id) || m.equipment[1].as_ref().is_some_and(|s| s.id == id)
}

/// `!the "no loved item in off-hand" test`.
pub fn loved_in_offhand(s: &Server, m: &Mob) -> bool {
    m.equipment[1].as_ref().is_some_and(|st| is_loved(s, st))
}

/// The add-to-container test.
fn can_add(s: &Server, m: &Mob, st: &Stack) -> bool {
    if m.pig.inventory.len() < INVENTORY_SIZE {
        return true;
    }
    let max = s.life.items.max_stack(st);
    m.pig.inventory.iter().any(|c| crate::items::same(c, st) && c.count < max)
}

/// Completes identical stacks then empty slots; remainder returned.
fn add_to_inventory(s: &Server, m: &mut Mob, mut st: Stack) -> Option<Stack> {
    let max = s.life.items.max_stack(&st);
    for c in m.pig.inventory.iter_mut() {
        if st.count <= 0 {
            break;
        }
        if crate::items::same(c, &st) && c.count < max {
            let n = (max - c.count).min(st.count);
            c.count += n;
            st.count -= n;
        }
    }
    while st.count > 0 && m.pig.inventory.len() < INVENTORY_SIZE {
        let n = st.count.min(max);
        m.pig.inventory.push(Stack { count: n, ..st.clone() });
        st.count -= n;
    }
    (st.count > 0).then_some(st)
}

/// The piglin's pickup wish (mobGriefing rule and pickup checked by the caller).
pub fn wants(s: &Server, m: &Mob, b: Option<&Brain>, st: &Stack) -> bool {
    if m.kind == Kind::PiglinBrute {
        return st.id == "minecraft:golden_axe";
    }
    if m.is_baby() && s.item_tag(&st.id, "minecraft:ignored_by_piglin_babies") {
        return false;
    }
    if s.item_tag(&st.id, "minecraft:piglin_repellents") {
        return false;
    }
    let disabled = b.is_some_and(|b| b.mem.pig.admiring_disabled.has());
    if disabled && b.is_some_and(|b| b.mem.attack.has()) {
        return false;
    }
    if is_currency(st) {
        return !loved_in_offhand(s, m);
    }
    let space = can_add(s, m, st);
    if st.id == "minecraft:gold_nugget" {
        return space;
    }
    if is_food(s, st) {
        return !b.is_some_and(|b| b.mem.pig.ate_recently.has()) && space;
    }
    if !is_loved(s, st) {
        let slot = s.slot_item_is_worn_in(m, st);
        return s.replaces_worn_item(m, st, m.equipment[slot].as_ref(), slot);
    }
    !loved_in_offhand(s, m) && space
}

/// Closest wanted item (32 × 16, seen).
pub fn nearest_wanted(s: &Server, m: &Mob, b: &Brain) -> Option<i32> {
    if !s.rule("minecraft:mob_griefing", true) || !m.can_pickup {
        return None;
    }
    s.nearest_wanted_item(m, |st| wants(s, m, Some(b), st))
}

impl Server {
    /// Off-hand gesture.
    fn pig_swing_offhand(&self, m: &mut Mob) {
        m.swing_time = 6;
        let _ = self.send(rc_client::Event::Entity(rc_client::EntityEvent::Swing {
            id: m.id,
            hand: 1,
            kind: 1,
            duration: 6,
        }));
    }

    /// Gesture, then each stack thrown toward the target + 1 in height; an
    /// empty stack (leftover of a full storage) is "thrown" without creating anything.
    fn pig_throw_items(&mut self, m: &mut Mob, items: Vec<Option<Stack>>, target: [f64; 3]) {
        if items.is_empty() {
            return;
        }
        self.pig_swing_offhand(m);
        let to = [target[0], target[1] + 1.0, target[2]];
        for st in items.into_iter().flatten() {
            self.throw_item(m, st, to);
        }
    }

    /// Random position (4 × 2), otherwise its own.
    fn pig_throw_random(&mut self, m: &mut Mob, items: Vec<Option<Stack>>) {
        let p = self.land_random_pos(m, 4, 2).unwrap_or_else(|| m.pos());
        self.pig_throw_items(m, items, p);
    }

    /// Toward the seen player, otherwise at random.
    fn pig_throw(&mut self, m: &mut Mob, b: Option<&Brain>, items: Vec<Option<Stack>>) {
        let player = b.and_then(|b| b.mem.visible_player.get());
        match player.and_then(|p| self.ent_pos(p)) {
            Some(pp) => self.pig_throw_items(m, items, pp),
            None => self.pig_throw_random(m, items),
        }
    }

    /// The remainder (or an empty stack) is thrown at random.
    fn pig_put_in_inventory(&mut self, m: &mut Mob, st: Stack) {
        let rest = add_to_inventory(self, m, st);
        self.pig_throw_random(m, vec![rest]);
    }

    /// Equipping a picked-up item (the common rule of every mob); true if the item was equipped.
    fn pig_equip(&mut self, m: &mut Mob, st: &Stack) -> bool {
        self.equip_if_wanted(m, st).is_some()
    }

    /// The held item drops first; persistence except for a gold ingot.
    fn pig_hold_offhand(&mut self, m: &mut Mob, st: Stack) {
        if let Some(old) = m.equipment[1].clone() {
            self.drop_at_height(m, old, 0.0);
        }
        let gold = is_currency(&st);
        self.mob_set_item_slot(m, 1, Some(st));
        m.drop_chance[1] = 2.0;
        if !gold {
            m.persistent = true;
        }
        self.send_mob_equipment(m, &[1]);
    }

    /// Bartering (adult's gold ingot), storing, or the baby's hand.
    /// An adult that admired an ingot without bartering (hurt) loses it.
    pub(crate) fn piglin_stop_holding_offhand(&mut self, m: &mut Mob, barter: bool, b: Option<&Brain>) {
        let Some(st) = self.mob_set_item_slot(m, 1, None) else { return };
        self.send_mob_equipment(m, &[1]);
        if !m.is_baby() {
            let currency = is_currency(&st);
            if barter && currency {
                let items = self.simple_loot("minecraft:gameplay/piglin_bartering", None);
                self.pig_throw(m, b, items.into_iter().map(Some).collect());
            } else if !currency && !self.pig_equip(m, &st) {
                self.pig_put_in_inventory(m, st);
            }
        } else if !self.pig_equip(m, &st) {
            let main = m.equipment[0].clone();
            match main {
                Some(mh) if is_loved(self, &mh) => self.pig_put_in_inventory(m, mh),
                mh => self.pig_throw(m, b, vec![mh]),
            }
            self.mob_set_item_slot(m, 0, Some(st));
            m.drop_chance[0] = 2.0;
            m.persistent = true;
            self.send_mob_equipment(m, &[0]);
        }
    }

    /// The admired item drops (conversion).
    pub(crate) fn piglin_cancel_admiring(&mut self, m: &mut Mob) {
        let admiring = m.bm.brain.as_ref().is_some_and(|b| b.mem.pig.admiring_item.has());
        if admiring && let Some(st) = m.equipment[1].clone() {
            self.drop_at_height(m, st, 0.0);
            self.mob_set_item_slot(m, 1, None);
            self.send_mob_equipment(m, &[1]);
        }
    }

    /// Pickup (the mobs' common AI step then the pickup of an item): box enlarged by 1 / 0 / 1.
    pub(crate) fn piglin_pickup(&mut self, m: &mut Mob) {
        if !super::is_abstract_piglin(m.kind)
            || !m.can_pickup
            || !m.alive()
            || m.dead
            || !self.rule("minecraft:mob_griefing", true)
        {
            return;
        }
        // The game goes through ALL items in range in the same tick (the mob's box enlarged by
        // 1 / 0 / 1, list taken once), each rechecked at its turn (desire changes after each
        // pickup). Positions do not move during the loop and an item born in it (a thrown rest) is
        // appended after the `listed` first ones, so walking those by index is that list, with no
        // list built and no stack copied before the piglin wants it.
        let bb = m.bbox();
        let listed = self.life.drops.len();
        let mut b = m.bm.brain.take();
        for i in 0..listed {
            let Some(d) = self.life.drops.get(i) else { break };
            if !in_pickup_reach(d, &bb) || !wants(self, m, b.as_deref(), &d.stack) {
                continue;
            }
            if m.kind == Kind::PiglinBrute {
                // The brute keeps the common pickup of every mob.
                self.mob_pick_up_and_equip(m, i);
            } else {
                let (id, st) = (d.id, d.stack.clone());
                self.thrown_pickup_by_mob(id, m); // W8c: the reaction to an item's pickup
                self.piglin_pick_up_item(m, b.as_deref_mut(), id, st);
            }
        }
        m.bm.brain = b;
    }

    /// An item's pickup by a piglin.
    fn piglin_pick_up_item(&mut self, m: &mut Mob, mut b: Option<&mut Brain>, id: i32, st: Stack) {
        // The walk stop.
        if let Some(b) = b.as_deref_mut() {
            b.mem.walk.erase();
        }
        m.nav.stop();
        let taken = if st.id == "minecraft:gold_nugget" {
            self.mob_take(id, m.id, st.count);
            st
        } else {
            self.mob_take(id, m.id, 1);
            Stack { count: 1, ..st }
        };
        if is_loved(self, &taken) {
            if let Some(b) = b.as_deref_mut() {
                b.mem.pig.time_reaching_item.erase();
            }
            self.pig_hold_offhand(m, taken);
            if let Some(b) = b.as_deref_mut() {
                b.mem.pig.admiring_item.set_exp(true, 119);
            }
        } else if is_food(self, &taken) && !b.as_deref().is_some_and(|b| b.mem.pig.ate_recently.has()) {
            if let Some(b) = b {
                b.mem.pig.ate_recently.set_exp(true, 200);
            }
        } else if !self.pig_equip(m, &taken) {
            self.pig_put_in_inventory(m, taken);
        }
    }

    /// An adult that admires nothing takes a gold ingot.
    pub(crate) fn piglin_interact(&mut self, m: &mut Mob, slot: usize, item: &str) -> Option<bool> {
        let mut b = m.bm.brain.take()?;
        let can = !b.mem.pig.admiring_disabled.has()
            && !b.mem.pig.admiring_item.has()
            && !m.is_baby()
            && item == "minecraft:gold_ingot";
        if can {
            let taken = Stack {
                id: item.into(),
                count: 1,
                components: self.slots[slot].as_ref().and_then(|s| s.components.clone()),
            };
            self.consume_held(slot);
            self.pig_hold_offhand(m, taken);
            b.mem.pig.admiring_item.set_exp(true, 119);
            b.mem.walk.erase();
            m.nav.stop();
        }
        m.bm.brain = Some(b);
        can.then_some(true)
    }

    /// Idle piglins within 16 blocks of the player (seen, if requested).
    pub(crate) fn anger_nearby_piglins(&mut self, if_seen: bool) {
        let pb = self.player_aabb();
        let area = rc_physics::Aabb { min: pb.min.map(|v| v - 16.0), max: pb.max.map(|v| v + 16.0) };
        let ids: Vec<i32> = self
            .mobs
            .list
            .iter()
            .filter(|o| o.kind == Kind::Piglin && o.touches(&area))
            .map(|o| o.id)
            .collect();
        let universal = self.rule("minecraft:universal_anger", false);
        for id in ids {
            self.pig_with(id, |s, o, b| {
                if b.act != super::super::brainy::brain::Act::Idle {
                    return;
                }
                if if_seen && !(b.mem.visible.has() && s.bz_visible_contains(o, b, Ent::Player)) {
                    return;
                }
                if universal {
                    let t = b.mem.attackable_player.get().unwrap_or(Ent::Player);
                    s.pig_set_anger(o, b, t);
                } else {
                    s.pig_set_anger(o, b, Ent::Player);
                }
            });
        }
    }

    /// Container opened by the player (chest, trapped chest, copper chests, barrel, ender chest,
    /// shulker box).
    pub(crate) fn piglin_container_opened(&mut self, block: &str, opened: bool) {
        if !opened {
            return;
        }
        let b = block.trim_start_matches("minecraft:");
        let guarded = b == "chest"
            || b == "trapped_chest"
            || b == "barrel"
            || b == "ender_chest"
            || b.ends_with("copper_chest")
            || b.ends_with("shulker_box");
        if guarded {
            self.anger_nearby_piglins(true);
        }
    }

    /// Guarded block (`#guarded_by_piglins`) broken by the player.
    pub(crate) fn piglin_block_destroyed(&mut self, p: [i32; 3]) {
        if self.btag_id(p, crate::tag_bits::TagId::GuardedByPiglins) {
            self.anger_nearby_piglins(false);
        }
    }
}
