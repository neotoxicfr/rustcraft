//! Ground items and experience orbs pushed by a moving block. Each push is followed, like in the
//! game, by the effects of the blocks the push alone crossed (the block stepped on, then the
//! blocks it is in: hopper, fire, lava, plates, tripwire), at the push itself and not at the
//! entity's next tick.

use rc_physics::Aabb;
use rc_physics::body::{Body, Heft};
use rc_physics::piston::PistonTarget;

use super::{ITEM_SIZE, ItemEnt, ORB_SIZE, Orb, dampens_vibrations};
use crate::inside_blocks::Who;
use crate::mobs::recent::vibration::Src;
use crate::server::Server;

/// Item `i`, out of its slot while it is pushed (a placeholder holds the slot, the plates count it
/// through `contact.ticking_item`, as during its tick).
struct PushedItem<'a> {
    s: &'a mut Server,
    d: &'a mut ItemEnt,
    i: usize,
}

impl PistonTarget for PushedItem<'_> {
    fn bbox(&self) -> Aabb {
        self.d.body.bounding_box()
    }
    fn position(&self) -> [f64; 3] {
        self.d.pos
    }
    fn grounded(&self) -> bool {
        self.d.on_ground
    }
    fn supported_by(&self, p: [i32; 3]) -> bool {
        self.d.body.support() == Some(p)
    }
    fn launch(&mut self, axis: usize, v: f64) {
        self.d.vel[axis] = v;
    }
    fn piston_push(&mut self, delta: [f64; 3]) {
        let d = &mut *self.d;
        let from = d.pos;
        (d.body.pos, d.body.vel, d.body.on_ground) = (d.pos, d.vel, d.on_ground);
        self.s.piston_move_body(&mut d.body, delta, Heft::ITEM);
        (d.pos, d.vel, d.on_ground) = (d.body.pos, d.body.vel, d.body.on_ground);
        self.s.item_block_effects_after_push(d, self.i, from);
    }
}

/// Orb `i`, out of its slot while it is pushed (counted through `contact.ticking_orb`).
struct PushedOrb<'a> {
    s: &'a mut Server,
    o: &'a mut Orb,
    i: usize,
}

impl PistonTarget for PushedOrb<'_> {
    fn bbox(&self) -> Aabb {
        self.o.body.bounding_box()
    }
    fn position(&self) -> [f64; 3] {
        self.o.pos
    }
    fn grounded(&self) -> bool {
        self.o.on_ground
    }
    fn supported_by(&self, p: [i32; 3]) -> bool {
        self.o.body.support() == Some(p)
    }
    fn launch(&mut self, axis: usize, v: f64) {
        self.o.vel[axis] = v;
    }
    fn piston_push(&mut self, delta: [f64; 3]) {
        let o = &mut *self.o;
        let from = o.pos;
        (o.body.pos, o.body.vel, o.body.on_ground) = (o.pos, o.vel, o.on_ground);
        self.s.piston_move_body(&mut o.body, delta, Heft::ORB);
        (o.pos, o.vel, o.on_ground) = (o.body.pos, o.body.vel, o.body.on_ground);
        self.s.orb_block_effects_after_push(o, self.i, from);
    }
}

impl Server {
    /// The items and orbs whose box touches `area`, each handed to `apply` (push, drag) out of
    /// its slot.
    pub(crate) fn push_items_and_orbs(&mut self, area: &Aabb, apply: &dyn Fn(&mut dyn PistonTarget)) {
        for i in 0..self.life.drops.len() {
            let d = &self.life.drops[i];
            if d.is_removed() || !d.body.bounding_box().intersects(area) {
                continue;
            }
            let spare = self.walk.spare_item.take().unwrap_or_else(ItemEnt::placeholder);
            let mut d = std::mem::replace(&mut self.life.drops[i], spare);
            (d.body.pos, d.body.vel, d.body.on_ground) = (d.pos, d.vel, d.on_ground);
            apply(&mut PushedItem { s: self, d: &mut d, i });
            self.walk.spare_item = Some(std::mem::replace(&mut self.life.drops[i], d));
        }
        for i in 0..self.life.orbs.len() {
            let o = &self.life.orbs[i];
            if o.is_removed() || !o.body.bounding_box().intersects(area) {
                continue;
            }
            let spare = self.walk.spare_orb.take().unwrap_or_else(Orb::placeholder);
            let mut o = std::mem::replace(&mut self.life.orbs[i], spare);
            (o.body.pos, o.body.vel, o.body.on_ground) = (o.pos, o.vel, o.on_ground);
            apply(&mut PushedOrb { s: self, o: &mut o, i });
            self.walk.spare_orb = Some(std::mem::replace(&mut self.life.orbs[i], o));
        }
    }

    /// The piston movement of a body (cap, collisions, its physics' own block pass), in the
    /// world as the pushing block leaves it (`bs.noclip`).
    pub(crate) fn piston_move_body(&self, b: &mut Body, delta: [f64; 3], k: Heft) {
        let w = self.world();
        b.piston_move(delta, self.time, &w, &self.phys, k);
    }

    /// Puts the push's straight segment `from`..`to` in place of the movements of `b`'s tick
    /// (the game's pass after a push sees that segment alone); called again, puts them back.
    fn swap_push_segment(&mut self, b: &mut Body, from: [f64; 3], to: [f64; 3], entering: bool) {
        let mut moves = std::mem::take(&mut self.contact.push_moves);
        if entering {
            moves.clear();
            moves.push((from, to, None));
        }
        b.swap_effect_moves(&mut moves);
        self.contact.push_moves = moves;
    }

    /// The blocks crossed by the push of item `d` (slot `i`) from `from`, in the order of its own
    /// tick: stepped on, hopper, the blocks it is in, plates and tripwire.
    fn item_block_effects_after_push(&mut self, d: &mut ItemEnt, i: usize, from: [f64; 3]) {
        if d.is_removed() || d.body.no_physics {
            return;
        }
        self.contact.ticking_item = Some((d.pos, false));
        let src = Src::Item { owner: d.thrower, dampens: dampens_vibrations(&d.stack.id) };
        if let Some(under) = d.body.stepped_on {
            self.object_step_on(under, d.pos, || src);
        }
        self.hopper_item_inside(from, d);
        if !d.removed {
            self.swap_push_segment(&mut d.body, from, d.pos, true);
            self.inside_blocks(&mut Who::Item(d));
            self.swap_push_segment(&mut d.body, from, d.pos, false);
        }
        self.item_seen(i, d.pos, d.removed);
        if !d.removed {
            self.contact_triggers(from, d.pos, ITEM_SIZE / 2.0, ITEM_SIZE, src);
        }
        self.contact.ticking_item = None;
    }

    /// Same for orb `o` (slot `i`): stepped on, the blocks it is in, plates and tripwire.
    fn orb_block_effects_after_push(&mut self, o: &mut Orb, i: usize, from: [f64; 3]) {
        if o.is_removed() || o.body.no_physics {
            return;
        }
        self.contact.ticking_orb = Some((o.pos, false));
        if let Some(under) = o.body.stepped_on {
            self.object_step_on(under, o.pos, || Src::Other);
        }
        self.swap_push_segment(&mut o.body, from, o.pos, true);
        self.inside_blocks(&mut Who::Orb(o));
        self.swap_push_segment(&mut o.body, from, o.pos, false);
        self.orb_seen(i, o.pos, o.removed);
        if !o.removed {
            self.contact_triggers(from, o.pos, ORB_SIZE / 2.0, ORB_SIZE, Src::Other);
        }
        self.contact.ticking_orb = None;
    }
}
