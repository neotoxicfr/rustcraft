//! Loot containers still sealed read by a comparator or a hopper (S8): the game draws the
//! loot as soon as the contents are consulted (without
//! player luck), before computing the signal or moving an item.

use crate::server::Server;

impl Server {
    /// Loot drawn for each of the positions (double chest: both halves).
    pub(crate) fn unseal_all(&mut self, parts: &[[i32; 3]]) {
        for p in parts {
            if self.block_entity(*p).is_some_and(|b| b.get("LootTable").is_some()) {
                self.unpack_loot(*p, false);
            }
        }
    }

    /// Block read by a comparator at `p` (same geometry as its input): loot drawn.
    pub(crate) fn comparator_unseal(&mut self, p: [i32; 3], s: u32) {
        let d = self.v().facing(s);
        let tp = crate::blocks::rel(p, d);
        let ts = self.get(tp);
        let target = if self.v().has_analog(ts) {
            Some(tp)
        } else if self.v().conductor(ts) {
            let t2 = crate::blocks::rel(tp, d);
            self.v().has_analog(self.get(t2)).then_some(t2)
        } else {
            None
        };
        if let Some(t) = target
            && let Some(parts) = self.container_at(t)
        {
            self.unseal_all(&parts);
        }
    }
}
