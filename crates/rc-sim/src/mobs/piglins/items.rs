//! Piglin gold: loved items, admiring, bartering (table `gameplay/piglin_bartering`),
//! pickup, 8-slot inventory, equipment, item throws and
//! piglin anger when a container is opened or a guarded block is broken.

use super::super::brainy::brain::Brain;
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

/// Mob slot for an item.
fn slot_for(s: &Server, st: &Stack) -> usize {
    match s.life.items.equip_slot(st) {
        Some(1) => 2,
        Some(2) => 3,
        Some(3) => 4,
        Some(4) => 5,
        Some(5) => 1,
        Some(6) => 6,
        Some(7) => 7,
        _ => 0,
    }
}

fn slot_name(i: usize) -> &'static str {
    ["mainhand", "offhand", "feet", "legs", "chest", "head", "body", "saddle"][i]
}

/// Base of the attribute plus the "value" modifiers of
/// the item for this slot.
fn approx_attr(s: &Server, m: &Mob, st: &Stack, attr: &str, slot: usize) -> f64 {
    let base = match attr {
        "minecraft:armor" => m.attrs.armor.base,
        "minecraft:attack_damage" => m.attrs.attack.base,
        _ => 0.0,
    };
    let Some(mods) = s.life.items.default_component(st, "minecraft:attribute_modifiers") else { return base };
    let list = mods.as_array().cloned().or_else(|| mods["modifiers"].as_array().cloned()).unwrap_or_default();
    let name = slot_name(slot);
    let mut v = base;
    let mut mult = 0.0;
    let mut total = 1.0;
    for x in &list {
        if x["type"].as_str() != Some(attr) {
            continue;
        }
        let group = x["slot"].as_str().unwrap_or("any");
        let ok = match group {
            "any" => true,
            "hand" => slot <= 1,
            "armor" => (2..=6).contains(&slot),
            g => g == name,
        };
        if !ok {
            continue;
        }
        let amount = x["amount"].as_f64().unwrap_or(0.0);
        match x["operation"].as_str().unwrap_or("add_value") {
            "add_multiplied_base" => mult += amount,
            "add_multiplied_total" => total *= 1.0 + amount,
            _ => v += amount,
        }
    }
    (v + base * mult) * total
}

fn enchant_count(st: &Stack) -> usize {
    match crate::items::component(st, "minecraft:enchantments") {
        Some(rc_proto::nbt::Nbt::Compound(f)) => {
            if let Some(rc_proto::nbt::Nbt::Compound(l)) =
                st.components.as_ref().and_then(|c| c.get("minecraft:enchantments")).and_then(|e| e.get("levels"))
            {
                l.len()
            } else {
                f.len()
            }
        }
        _ => 0,
    }
}

/// More enchantments, less worn, then named.
fn replace_equal(s: &Server, new: &Stack, cur: &Stack) -> bool {
    let (ne, ce) = (enchant_count(new), enchant_count(cur));
    if ne != ce {
        return ne > ce;
    }
    let (nd, cd) = (s.life.items.damage(new), s.life.items.damage(cur));
    if nd != cd {
        return nd < cd;
    }
    crate::items::component(new, "minecraft:custom_name").is_some()
        && crate::items::component(cur, "minecraft:custom_name").is_none()
}

/// Preferred weapon: `#piglin_preferred_weapons`, except the baby.
fn preferred(s: &Server, m: &Mob, st: &Stack) -> bool {
    m.kind == Kind::Piglin && !m.is_baby() && s.item_tag(&st.id, "minecraft:piglin_preferred_weapons")
}

/// The held-item replacement test, with the piglin's override (loved items, preferred weapons).
fn can_replace(s: &Server, m: &Mob, new: &Stack, cur: Option<&Stack>, slot: usize) -> bool {
    let Some(cur) = cur else { return true };
    if m.kind == Kind::Piglin {
        if s.life.items.enchantment(cur, "minecraft:binding_curse") > 0 {
            return false;
        }
        let nw = is_loved(s, new) || preferred(s, m, new);
        let cw = is_loved(s, cur) || preferred(s, m, cur);
        if nw && !cw {
            return true;
        }
        if !nw && cw {
            return false;
        }
    }
    if (2..=6).contains(&slot) {
        if s.life.items.enchantment(cur, "minecraft:binding_curse") > 0 {
            return false;
        }
        let (na, oa) =
            (approx_attr(s, m, new, "minecraft:armor", slot), approx_attr(s, m, cur, "minecraft:armor", slot));
        if na != oa {
            return na > oa;
        }
        let nt = approx_attr(s, m, new, "minecraft:armor_toughness", slot);
        let ot = approx_attr(s, m, cur, "minecraft:armor_toughness", slot);
        return if nt != ot { nt > ot } else { replace_equal(s, new, cur) };
    }
    if slot != 0 {
        return false;
    }
    if m.kind == Kind::Piglin && !m.is_baby() {
        let (cp, np) = (preferred(s, m, cur), preferred(s, m, new));
        if cp && !np {
            return false;
        }
        if !cp && np {
            return true;
        }
    }
    let nd = approx_attr(s, m, new, "minecraft:attack_damage", 0);
    let od = approx_attr(s, m, cur, "minecraft:attack_damage", 0);
    if nd != od { nd > od } else { replace_equal(s, new, cur) }
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
        let slot = slot_for(s, st);
        return can_replace(s, m, st, m.equipment[slot].as_ref(), slot);
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
        let p = self.land_random_pos(m, 4, 2).unwrap_or(m.pos);
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

    /// Equipping a picked-up item; true if the item was equipped.
    fn pig_equip(&mut self, m: &mut Mob, st: &Stack) -> bool {
        let mut slot = slot_for(self, st);
        let mut cur = m.equipment[slot].clone();
        let mut ok = can_replace(self, m, st, cur.as_ref(), slot);
        if (2..=5).contains(&slot) && !ok {
            slot = 0;
            cur = m.equipment[0].clone();
            ok = cur.is_none();
        }
        if !ok {
            return false;
        }
        let chance = m.drop_chance[slot];
        if let Some(c) = cur
            && (m.rng.next_float() - 0.1).max(0.0) < chance
        {
            self.pig_spawn_at_location(m, c);
        }
        let n = if slot == 0 || slot == 1 { st.count } else { 1 };
        m.equipment[slot] = Some(Stack { count: n, ..st.clone() });
        m.drop_chance[slot] = 2.0;
        m.persistent = true;
        self.send_mob_equipment(m, &[slot]);
        true
    }

    /// The held item drops first; persistence except for a gold ingot.
    fn pig_hold_offhand(&mut self, m: &mut Mob, st: Stack) {
        if let Some(old) = m.equipment[1].take() {
            self.pig_spawn_at_location(m, old);
        }
        let gold = is_currency(&st);
        m.equipment[1] = Some(st);
        m.drop_chance[1] = 2.0;
        if !gold {
            m.persistent = true;
        }
        self.send_mob_equipment(m, &[1]);
    }

    /// Bartering (adult's gold ingot), storing, or the baby's hand.
    /// An adult that admired an ingot without bartering (hurt) loses it.
    pub(crate) fn piglin_stop_holding_offhand(&mut self, m: &mut Mob, barter: bool, b: Option<&Brain>) {
        let Some(st) = m.equipment[1].take() else { return };
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
            let main = m.equipment[0].take();
            match main {
                Some(mh) if is_loved(self, &mh) => self.pig_put_in_inventory(m, mh),
                mh => self.pig_throw(m, b, vec![mh]),
            }
            m.equipment[0] = Some(st);
            m.drop_chance[0] = 2.0;
            m.persistent = true;
            self.send_mob_equipment(m, &[0]);
        }
    }

    /// The admired item drops (conversion).
    pub(crate) fn piglin_cancel_admiring(&mut self, m: &mut Mob) {
        let admiring = m.bm.brain.as_ref().is_some_and(|b| b.mem.pig.admiring_item.has());
        if admiring && let Some(st) = m.equipment[1].take() {
            self.pig_spawn_at_location(m, st);
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
        // The game goes through ALL items in range in the same tick (the mob's box
        // enlarged by 1 / 0 / 1, list taken once), each rechecked at its turn.
        let bb = m.bbox();
        let near: Vec<i32> = self
            .life
            .drops
            .iter()
            .filter(|d| {
                !d.is_removed()
                    && d.pos[0] + 0.125 > bb.min[0] - 1.0
                    && d.pos[0] - 0.125 < bb.max[0] + 1.0
                    && d.pos[1] + 0.25 > bb.min[1]
                    && d.pos[1] < bb.max[1]
                    && d.pos[2] + 0.125 > bb.min[2] - 1.0
                    && d.pos[2] - 0.125 < bb.max[2] + 1.0
            })
            .map(|d| d.id)
            .collect();
        // Each item is re-evaluated at its turn (desire changes after each pickup).
        let mut b = m.bm.brain.take();
        for id in near {
            let Some(st) = self
                .life
                .drops
                .iter()
                .find(|d| d.id == id && !d.is_removed() && d.stack.count > 0 && d.pickup_delay <= 0)
                .map(|d| d.stack.clone())
            else {
                continue;
            };
            if !wants(self, m, b.as_deref(), &st) {
                continue;
            }
            if m.kind == Kind::PiglinBrute {
                if self.pig_equip(m, &st) {
                    self.thrown_pickup_by_mob(id, m); // W8c
                    self.mob_take(id, m.id, st.count.min(1));
                }
            } else {
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
            .flatten()
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
