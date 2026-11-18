//! The dust's second power evaluator, used when the world has the redstone experiments.
//!
//! Instead of recomputing every dust around a change again and again, it works out the new
//! powers first: the dusts that may lose power go into a first queue and are switched off, those
//! that may gain it go into a second queue and are switched on; the walk around each dust follows
//! its orientation (`orientation.rs`), each neighbour reached in the direction it lies in. Then
//! every dust whose power changed is written once (in the order they were first met), and the
//! neighbour updates follow, oriented too.

use std::collections::VecDeque;

use hashbrown::{HashMap, HashSet};
use rc_data::PropKey;

use super::orientation::{Orientation, SideBias};
use super::rules::SignalRules;
use super::{Dir, P, flag, rel};
use crate::server::Server;

/// Powers worked out so far, with the orientation each dust was first reached with, in the order
/// the dusts were first met (the order they are written in).
#[derive(Debug, Default)]
struct PendingPowers {
    order: Vec<(P, Orientation, i32)>,
    index: HashMap<P, usize>,
}

impl PendingPowers {
    fn get(&self, p: P) -> Option<(Orientation, i32)> {
        self.index.get(&p).map(|&i| (self.order[i].1, self.order[i].2))
    }

    /// New power of `p`; a dust met for the first time keeps `orientation` from then on.
    fn set(&mut self, p: P, power: i32, orientation: Orientation) {
        match self.index.get(&p) {
            Some(&i) => self.order[i].2 = power,
            None => {
                self.index.insert(p, self.order.len());
                self.order.push((p, orientation, power));
            }
        }
    }
}

/// A queue of dusts that holds each one once at most.
#[derive(Debug, Default)]
struct DustQueue {
    order: VecDeque<P>,
    members: HashSet<P>,
}

impl DustQueue {
    fn contains(&self, p: P) -> bool {
        self.members.contains(&p)
    }

    fn push(&mut self, p: P) {
        if self.members.insert(p) {
            self.order.push_back(p);
        }
    }

    fn pop(&mut self) -> Option<P> {
        let p = self.order.pop_front()?;
        self.members.remove(&p);
        Some(p)
    }
}

/// Working state of one evaluation, kept between evaluations so its buffers are reused.
#[derive(Debug, Default)]
pub(crate) struct OrientedWireWork {
    pending: PendingPowers,
    turn_off: DustQueue,
    turn_on: DustQueue,
}

impl OrientedWireWork {
    fn clear(&mut self) {
        self.pending.order.clear();
        self.pending.index.clear();
        self.turn_off.order.clear();
        self.turn_off.members.clear();
        self.turn_on.order.clear();
        self.turn_on.members.clear();
    }
}

impl Server {
    /// Power update of the dust network around `start` (a dust, or the place of a removed one).
    /// `incoming`: the orientation of the update that triggered it (drawn when there is none);
    /// `shape_update_at_start`: the first dust written lets its neighbour dusts update their shape.
    pub(crate) fn oriented_wire_update(&mut self, start: P, incoming: Option<Orientation>, shape_update_at_start: bool) {
        let orientation = match incoming {
            Some(o) => o,
            None => Orientation::random(&mut self.bs.rng),
        }
        .with_up(Dir::Up)
        .with_side_bias(SideBias::Left);
        // Taken out for the evaluation (a nested evaluation starts from empty buffers).
        let mut work = std::mem::take(&mut self.bs.oriented_wire);
        work.clear();
        self.oriented_wire_changes(&mut work, start, orientation);
        self.oriented_wire_write(&mut work, shape_update_at_start);
        self.oriented_wire_notify(&work);
        self.bs.oriented_wire = work;
    }

    /// Power a dust shows to this evaluation: the one worked out, otherwise its own.
    fn oriented_wire_power(&self, work: &OrientedWireWork, p: P, s: u32) -> i32 {
        match work.pending.get(p) {
            Some((_, power)) => power,
            None if self.view().block(s) == self.bs.t.id.wire => self.view().get_int(s, PropKey::Power),
            None => 0,
        }
    }

    /// The two queues: dusts that may lose power first (switched off), then those that may gain it.
    fn oriented_wire_changes(&mut self, work: &mut OrientedWireWork, start: P, orientation: Orientation) {
        let wire = self.bs.t.id.wire;
        let s = self.get(start);
        if self.view().block(s) == wire {
            work.pending.set(start, self.view().get_int(s, PropKey::Power), orientation);
            work.turn_off.push(start);
        } else {
            self.oriented_wire_spread(work, start, 0, orientation, true);
        }
        while let Some(p) = work.turn_off.pop() {
            let (o, old) = work.pending.get(p).expect("queued dusts have a power");
            let from_blocks = self.wire_block_signal(p);
            let new = from_blocks.max(self.oriented_wire_incoming(work, p));
            let power = if new < old {
                // Powered by a block: switched on again once everything is off.
                if from_blocks > 0 && !work.turn_on.contains(p) {
                    work.turn_on.push(p);
                }
                0
            } else {
                new
            };
            if power != old {
                work.pending.set(p, power, o);
            }
            self.oriented_wire_spread(work, p, power, o, old > new);
        }
        while let Some(p) = work.turn_on.pop() {
            let (o, old) = work.pending.get(p).expect("queued dusts have a power");
            let new = self.wire_block_signal(p).max(self.oriented_wire_incoming(work, p));
            // A dust is never switched off while switching on (the game would stop on it).
            if new > old {
                work.pending.set(p, new, o);
            }
            self.oriented_wire_spread(work, p, new.max(old), o, false);
        }
    }

    /// Strongest power coming from the neighbour dusts (side, above a side block when nothing
    /// covers this dust, below a side gap), less one: the shared rule read through this
    /// evaluation's powers.
    fn oriented_wire_incoming(&self, work: &OrientedWireWork, p: P) -> i32 {
        self.wire_incoming_read(p, |q, s| self.oriented_wire_power(work, q, s))
    }

    /// The neighbours of `p` that may change after its power became `power`: the side dusts,
    /// those above a side when nothing solid covers `p`, those below a side gap.
    fn oriented_wire_spread(&mut self, work: &mut OrientedWireWork, p: P, power: i32, o: Orientation, may_turn_off: bool) {
        for d in o.horizontal() {
            self.oriented_wire_enqueue(work, rel(p, d), power, o.with_front(d), may_turn_off);
        }
        for v in o.vertical() {
            let vp = rel(p, v);
            let covered = self.view().conductor(self.get(vp));
            for d in o.horizontal() {
                let reach = match v {
                    Dir::Up => !covered,
                    _ => !self.view().conductor(self.get(rel(p, d))),
                };
                if reach {
                    self.oriented_wire_enqueue(work, rel(vp, d), power, o.with_front(d), may_turn_off);
                }
            }
        }
    }

    /// A dust next to one now at `from_power`: queued to switch on if it is two or more below,
    /// to switch off if it is above (when allowed).
    fn oriented_wire_enqueue(&mut self, work: &mut OrientedWireWork, p: P, from_power: i32, o: Orientation, may_turn_off: bool) {
        let s = self.get(p);
        if self.view().block(s) != self.bs.t.id.wire {
            return;
        }
        let power = self.oriented_wire_power(work, p, s);
        if power < from_power - 1 && !work.turn_on.contains(p) {
            work.turn_on.push(p);
            work.pending.set(p, power, o);
        }
        if may_turn_off && power > from_power && !work.turn_off.contains(p) {
            work.turn_off.push(p);
            work.pending.set(p, power, o);
        }
    }

    /// Each dust whose power changed written once, in the order they were met (to the clients
    /// only; the neighbour dusts' shapes are left alone but around the first one when asked);
    /// the others leave the list.
    fn oriented_wire_write(&mut self, work: &mut OrientedWireWork, shape_update_at_start: bool) {
        let wire = self.bs.t.id.wire;
        let mut first = true;
        let mut kept = 0;
        for i in 0..work.pending.order.len() {
            let (p, o, power) = work.pending.order[i];
            let s = self.get(p);
            let changed = self.view().block(s) == wire && self.view().get_int(s, PropKey::Power) != power;
            if changed {
                let flags = if shape_update_at_start && first { flag::CLIENTS } else { flag::CLIENTS | flag::SKIP_WIRE_SHAPE };
                let ns = self.view().set_int(s, PropKey::Power, power);
                self.set_block(p, ns, flags);
                work.pending.order[kept] = (p, o, power);
                kept += 1;
            }
            first = false;
        }
        work.pending.order.truncate(kept);
    }

    /// Neighbour updates of the written dusts, each walking its connected sides (and below) in its
    /// orientation's order; a solid block reached passes the update on to its own neighbours.
    fn oriented_wire_notify(&mut self, work: &OrientedWireWork) {
        let wire = self.bs.t.id.wire;
        for &(p, o, _) in &work.pending.order {
            let s = self.get(p);
            for d in o.neighbours() {
                if !self.oriented_wire_connected(s, d) {
                    continue;
                }
                let np = rel(p, d);
                let ns = self.get(np);
                let no = o.with_front_keeping_up(d);
                self.neighbor_changed_full_oriented(ns, np, wire, false, Some(no));
                if self.view().conductor(ns) {
                    for d2 in no.neighbours().into_iter().filter(|&d2| d2 != d.opposite()) {
                        self.neighbor_changed_oriented(rel(np, d2), wire, Some(no.with_front_keeping_up(d2)));
                    }
                }
            }
        }
    }

    /// A dust reaches a neighbour: through a connected side, always below, never above.
    fn oriented_wire_connected(&self, s: u32, d: Dir) -> bool {
        match d {
            Dir::Down => true,
            Dir::Up => false,
            _ => self.view().block(s) == self.bs.t.id.wire && self.wire_get(s, d).connected(),
        }
    }
}
