//! Dropped items as mobs look at them: the reach of a pickup and the box an item search tests,
//! written once so every species tests them the same way, and the pickup every mob allowed to
//! pick up loot runs at the end of its AI step (equip the item if it beats what it wears).

use rc_physics::Aabb;

use super::mob::Mob;
use super::species::Kind;
use crate::drops::ItemEnt;
use crate::player::Stack;
use crate::server::Server;

/// Pickups and item searches stop at the first match (default): the cheap position test runs
/// before the item tag, only the chosen stack is copied, and a search returns its first item
/// instead of a list. `RUSTCRAFT_OPT_PICKUPFILTERFIRST=0`: old path (tag before position, every
/// stack in reach copied, the whole list of matches built to read its first entry).
pub(crate) fn filter_first() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_PICKUPFILTERFIRST")
}

/// The item can be picked up now (alive, not empty, no pickup delay) and its box is within the
/// pickup reach of the mob box `bb`: one block around horizontally, the box's own height.
pub(crate) fn in_pickup_reach(d: &ItemEnt, bb: &Aabb) -> bool {
    !d.is_removed()
        && d.stack.count > 0
        && d.pickup_delay <= 0
        && d.pos[0] + 0.125 > bb.min[0] - 1.0
        && d.pos[0] - 0.125 < bb.max[0] + 1.0
        && d.pos[1] + 0.25 > bb.min[1]
        && d.pos[1] < bb.max[1]
        && d.pos[2] + 0.125 > bb.min[2] - 1.0
        && d.pos[2] - 0.125 < bb.max[2] + 1.0
}

/// The item is alive, has no pickup delay and its box touches `area`.
pub(crate) fn ready_item_touches(d: &ItemEnt, area: &Aabb) -> bool {
    let item_box = Aabb {
        min: [d.pos[0] - 0.125, d.pos[1], d.pos[2] - 0.125],
        max: [d.pos[0] + 0.125, d.pos[1] + 0.25, d.pos[2] + 0.125],
    };
    !d.is_removed() && d.pickup_delay <= 0 && item_box.intersects(area)
}

/// First entry of `items`, by stopping at it (`filter_first`) or, old path, by collecting every
/// entry and reading the first one.
pub(crate) fn first_of<T>(mut items: impl Iterator<Item = T>, filter_first: bool) -> Option<T> {
    if filter_first { items.next() } else { items.collect::<Vec<T>>().into_iter().next() }
}

/// Species that run their own pickup elsewhere in their AI step (inventory, mouth, offhand,
/// absorbed block), and the ender dragon, whose AI step skips the common one.
fn has_own_pickup(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::Villager
            | Kind::Piglin
            | Kind::PiglinBrute
            | Kind::Fox
            | Kind::Panda
            | Kind::Dolphin
            | Kind::Allay
            | Kind::SulfurCube
            | Kind::EnderDragon
    )
}

/// The species wants the item on the ground: zombies leave glow ink sacs (not the zombified
/// piglin), skeletons and drowned leave spears; then the item must be one the mob may hold.
fn wants_to_pick_up(s: &Server, m: &Mob, st: &Stack) -> bool {
    let refused = match m.kind {
        Kind::ZombifiedPiglin => false,
        Kind::Drowned => st.id == "minecraft:glow_ink_sac" || s.item_tag(&st.id, "minecraft:spears"),
        k if k.zombie_like() => st.id == "minecraft:glow_ink_sac",
        k if k.skeleton_like() => s.item_tag(&st.id, "minecraft:spears"),
        _ => false,
    };
    !refused && s.species_may_hold(m, st)
}

/// A banner item (the pillager takes nothing else).
fn is_banner(st: &Stack) -> bool {
    st.id.ends_with("_banner")
}

impl Server {
    /// The pickup at the end of every mob's AI step: a living mob allowed to pick up loot, under
    /// the `mob_griefing` rule, goes through the items in its reach (its box widened by one block
    /// horizontally), each one checked again at its turn, and takes those its species wants.
    pub(crate) fn mob_pick_up_loot(&mut self, m: &mut Mob) {
        if !m.can_pickup || has_own_pickup(m.kind) || !m.alive() || m.dead || self.life.drops.is_empty() {
            return;
        }
        if !self.rule("minecraft:mob_griefing", true) {
            return;
        }
        let bb = m.bbox();
        // The game lists the items in reach once: an item born during the loop (a replaced piece
        // falling) is not in it. Positions do not move during the loop and nothing removes an
        // item from the list before the items' own tick, so walking the first `listed` entries
        // gives the same list without building it.
        let listed = self.life.drops.len();
        for i in 0..listed {
            let Some(d) = self.life.drops.get(i) else { break };
            // The position test first: it rejects nearly every item, the wish reads item tags.
            if in_pickup_reach(d, &bb) && wants_to_pick_up(self, m, &d.stack) {
                self.mob_pick_up_item(m, i);
            }
        }
    }

    /// The item at `index` of the ground items taken by a mob of the common pickup (also the raid
    /// banner goal's last step): a raider's ominous banner makes it the leader of its wave; a
    /// pillager takes no other item than a banner; every other case goes through the common
    /// equipping. Items are passed by index, never looked up by id: a mob standing by a heap it
    /// keeps refusing tests each item once per tick.
    pub(crate) fn mob_pick_up_item(&mut self, m: &mut Mob, index: usize) {
        if super::illagers::is_raider(m.kind) {
            if m.kind == Kind::Pillager && !self.life.drops.get(index).is_some_and(|d| is_banner(&d.stack)) {
                return;
            }
            if self.raider_pick_up_banner(m, index) {
                return;
            }
        }
        self.mob_pick_up_and_equip(m, index);
    }

    /// The common pickup of the item at `index`: equipped if possible, then the "thrown item picked
    /// up" trigger, the pickup packet and the stack shrunk by what was equipped. The decision reads
    /// the stack in place; it is copied only once the mob takes it.
    pub(crate) fn mob_pick_up_and_equip(&mut self, m: &mut Mob, index: usize) {
        let Some(d) = self.life.drops.get(index).filter(|d| !d.is_removed()) else { return };
        let Some(slot) = self.slot_to_equip(m, &d.stack) else { return };
        let (item, stack) = (d.id, d.stack.clone());
        let equipped = self.equip_in_slot(m, stack, slot);
        self.thrown_pickup_by_mob(item, m);
        self.mob_take(item, m.id, equipped);
    }
}
