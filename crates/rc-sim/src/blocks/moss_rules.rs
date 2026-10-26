//! G6-F06: families with their own shapes — pale moss carpet (base, low or tall sides along
//! walls, top layer placed at random), pale hanging moss (hangs from a full underside
//! or from itself, tip at the bottom), giant mushroom blocks (faces off against the same
//! block). Rules of game 26.3.

use super::{Dir, P, above, below, rel};
use crate::server::Server;

/// Sides of the carpet, in the game's order.
const SIDES: [Dir; 4] = [Dir::North, Dir::East, Dir::South, Dir::West];

impl Server {
    // --- Pale moss carpet. ---

    /// Sides recomputed: a side exists against a full face; new (`create`, or base) it is
    /// low, otherwise it keeps its value; low under a hanging carpet on the same side → tall; without base,
    /// erased above a carpet that does not have this side.
    pub(crate) fn moss_carpet_sides(&self, s: u32, p: P, create: bool) -> u32 {
        let v = self.v();
        let carpet = v.block(s);
        let base = v.get_bool(s, "bottom");
        let create = create || base;
        let (mut up, mut down) = (None, None);
        let mut out = s;
        for d in SIDES {
            let mut side = if self.multiface_can_attach(p, d) {
                if create { "low" } else { v.get(s, d.name()).unwrap_or("none") }
            } else {
                "none"
            };
            if side == "low" {
                let a = *up.get_or_insert_with(|| self.get(above(p)));
                if v.block(a) == carpet && v.get(a, d.name()) != Some("none") && !v.get_bool(a, "bottom") {
                    side = "tall";
                }
                if !base {
                    let b = *down.get_or_insert_with(|| self.get(below(p)));
                    if v.block(b) == carpet && v.get(b, d.name()) == Some("none") {
                        side = "none";
                    }
                }
            }
            out = v.set(out, d.name(), side);
        }
        out
    }

    fn moss_carpet_has_faces(&self, s: u32) -> bool {
        let v = self.v();
        v.get_bool(s, "bottom") || SIDES.iter().any(|d| v.get(s, d.name()) != Some("none"))
    }

    /// Base: anything but air below; hanging layer: a carpet with a base below.
    pub(crate) fn moss_carpet_survives(&self, s: u32, p: P) -> bool {
        let v = self.v();
        let b = self.get(below(p));
        if v.get_bool(s, "bottom") {
            !v.is_air(b)
        } else {
            v.block(b) == v.block(s) && v.get_bool(b, "bottom")
        }
    }

    pub(crate) fn moss_carpet_update_shape(&self, s: u32, p: P) -> u32 {
        if !self.moss_carpet_survives(s, p) {
            return self.air();
        }
        let n = self.moss_carpet_sides(s, p, false);
        if self.moss_carpet_has_faces(n) { n } else { self.air() }
    }

    /// Top layer of a carpet placed at `p`: without base, new sides, each side kept if
    /// `keep` (random draw) accepts it; air if nothing holds or nothing changes.
    pub(crate) fn moss_carpet_topper(&mut self, carpet_state: u32, p: P, mut keep: impl FnMut(&mut Self) -> bool) -> u32 {
        let v = self.v();
        let carpet = v.block(carpet_state);
        let up = above(p);
        let prev = self.get(up);
        let is_carpet = v.block(prev) == carpet;
        if (is_carpet && v.get_bool(prev, "bottom")) || (!is_carpet && !v.replaceable(prev)) {
            return self.air();
        }
        let mut top = self.moss_carpet_sides(v.set_bool(v.default_state(carpet), "bottom", false), up, true);
        for d in SIDES {
            if v.get(top, d.name()) != Some("none") && !keep(self) {
                top = v.set(top, d.name(), "none");
            }
        }
        if self.moss_carpet_has_faces(top) && top != prev { top } else { self.air() }
    }

    /// Carpet placed by a player: top layer drawn at random (one boolean per side).
    pub(crate) fn moss_carpet_placed(&mut self, s: u32, p: P) {
        let top = self.moss_carpet_topper(s, p, |sv| sv.bs.rng.next_bool());
        if !self.v().is_air(top) {
            self.set_update(above(p), top);
        }
    }

    // --- Pale hanging moss. ---

    pub(crate) fn hanging_moss_survives(&self, s: u32, p: P) -> bool {
        let a = self.get(above(p));
        self.multiface_can_attach(p, Dir::Up) || self.v().block(a) == self.v().block(s)
    }

    /// No longer holds: tick to the next (broken with loot); tip if nothing similar below.
    pub(crate) fn hanging_moss_update_shape(&mut self, s: u32, p: P) -> u32 {
        let v = self.v();
        if !self.hanging_moss_survives(s, p) {
            self.schedule_tick(p, v.block(s), 1);
        }
        let tip = v.block(self.get(below(p))) != v.block(s);
        v.set_bool(s, "tip", tip)
    }

    pub(crate) fn hanging_moss_tick(&mut self, s: u32, p: P) {
        if !self.hanging_moss_survives(s, p) {
            self.destroy_block(p, true, 512);
        }
    }

    // --- Giant mushroom blocks. ---

    /// Placement: each visible face except against the same block.
    pub(crate) fn huge_mushroom_placement(&self, def: u32, p: P) -> u32 {
        let v = self.v();
        let me = v.block(def);
        let mut s = def;
        for d in Dir::ALL {
            s = v.set_bool(s, d.name(), v.block(self.get(rel(p, d))) != me);
        }
        s
    }
}
