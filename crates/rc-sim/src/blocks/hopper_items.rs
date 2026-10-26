//! Hopper and dropped items: suction from above when no container is above, and
//! immediate entry of an item falling into the hopper.
//!
//! Suction zone: the whole width of the block, from 11/16 above its bottom up to two blocks
//! high. A full cube above (outside the tag `does_not_block_hoppers`) blocks the suction. An
//! item is taken only if it fits entirely; otherwise, what fitted leaves anyway and the rest
//! stays on the ground, with no transfer delay (as in the game).

use rc_client::{EntityEvent, Event};
use rc_proto::nbt::Nbt;

use super::{Kind, P, above};
use crate::drops::ItemEnt;
use crate::server::Server;

/// Half-width and height of a dropped item's box.
const HALF: f64 = 0.125;
const HEIGHT: f64 = 0.25;

/// Does the box of the item at `pos` touch the suction zone of hopper `p`?
fn in_suck_zone(p: P, pos: [f64; 3]) -> bool {
    let (x, y, z) = (f64::from(p[0]), f64::from(p[1]), f64::from(p[2]));
    pos[0] - HALF < x + 1.0
        && pos[0] + HALF > x
        && pos[1] < y + 2.0
        && pos[1] + HEIGHT > y + 11.0 / 16.0
        && pos[2] - HALF < z + 1.0
        && pos[2] + HALF > z
}

impl Server {
    /// Dropped items above hopper `p`: the first that fits entirely stops the
    /// search.
    pub(crate) fn hopper_suck_items(&mut self, p: P) -> bool {
        let up = self.get(above(p));
        let block = self.v().block(up);
        if self.v().full_collision(up) && !self.tag_id(crate::tag_bits::TagId::DoesNotBlockHoppers, block) {
            return false;
        }
        if !self.life.drops.iter().any(|d| !d.is_removed() && in_suck_zone(p, d.pos)) {
            return false;
        }
        let mut drops = std::mem::take(&mut self.life.drops);
        let mut taken = false;
        for d in drops.iter_mut() {
            if !d.is_removed() && in_suck_zone(p, d.pos) && self.hopper_add_item(p, d) {
                taken = true;
                break;
            }
        }
        // Items that appeared in the meantime (none here, to be safe) kept afterwards.
        drops.append(&mut self.life.drops);
        self.life.drops = drops;
        taken
    }

    /// Pours the dropped item `d` into hopper `p`: true if it went in entirely (it
    /// disappears); otherwise what did not fit stays on the ground.
    pub(crate) fn hopper_add_item(&mut self, p: P, d: &mut ItemEnt) -> bool {
        if d.stack.count <= 0 {
            d.mark_removed();
            return true;
        }
        let item = d.stack.to_nbt(None);
        let max = self.max_stack(&d.stack.id);
        let mut left = d.stack.count;
        let mut slots = self.slots(p);
        let mut moved = false;
        for slot in slots.iter_mut() {
            if left <= 0 {
                break;
            }
            match slot {
                None => {
                    let mut it = item.clone();
                    crate::nbtio::set(&mut it, "count", Nbt::Int(left));
                    *slot = Some(it);
                    left = 0;
                    moved = true;
                }
                Some(cur) if Self::same_item(cur, &item) => {
                    let n = left.min(max - Self::item_count(cur)).max(0);
                    if n > 0 {
                        crate::nbtio::set(cur, "count", Nbt::Int(Self::item_count(cur) + n));
                        left -= n;
                        moved = true;
                    }
                }
                Some(_) => {}
            }
        }
        if moved {
            self.store_slots(p, &slots);
        }
        if left <= 0 {
            d.stack.count = 0;
            d.mark_removed();
            true
        } else {
            if left != d.stack.count {
                d.stack.count = left;
                let net = self.net(Some(&d.stack));
                let _ = self.send(Event::Entity(EntityEvent::Data {
                    id: d.id,
                    values: vec![(8, rc_proto_v777::entity::Meta::Item(net))],
                }));
            }
            false
        }
    }

    /// Dropped item that just moved from `old` to its position: each hopper whose block it crosses
    /// immediately tries to take it.
    pub(crate) fn hopper_item_inside(&mut self, old: [f64; 3], d: &mut ItemEnt) {
        if d.is_removed() || d.stack.count <= 0 {
            return;
        }
        let eps = 1.0e-5;
        let lo = |a: usize| (old[a].min(d.pos[a]) - if a == 1 { 0.0 } else { HALF } + eps).floor() as i32;
        let hi = |a: usize| (old[a].max(d.pos[a]) + if a == 1 { HEIGHT } else { HALF } - eps).floor() as i32;
        let (x0, x1, y0, y1, z0, z1) = (lo(0), hi(0), lo(1), hi(1), lo(2), hi(2));
        for x in x0..=x1 {
            for y in y0..=y1 {
                for z in z0..=z1 {
                    let p = [x, y, z];
                    if self.v().kind(self.get(p)) != Kind::Hopper || !in_suck_zone(p, d.pos) {
                        continue;
                    }
                    self.hopper_try_move(p, Some(d));
                    if d.is_removed() {
                        return;
                    }
                }
            }
        }
    }
}
