//! Dropped items as the fox, the panda and the dolphin look at them: the reach of a pickup and the
//! box an item search tests, written once so the three species test them the same way.

use rc_physics::Aabb;

use crate::drops::ItemEnt;

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
