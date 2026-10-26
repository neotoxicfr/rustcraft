//! Shelves and chiseled bookshelf of the integrated server: targeted slot on the front face
//! (columns and rows), item swap, lateral chain of powered shelves (up to
//! 3, swap of the whole bar), comparator outputs.
//!
//! Block entity data (the game's keys): `Items`; shelf: `align_items_to_bottom`;
//! bookshelf: `last_interacted_slot`.

use super::use_block::ItemOn;
use super::{Dir, Kind, P, rel_n};
use crate::containers::{read_items, write_items};
use crate::mobs::mob::Ent;
use crate::player::Stack;
use crate::server::Server;

/// Maximum length of a chain of shelves.
const MAX_CHAIN: i32 = 3;

/// Targeted slot: only on the front face; position of the touched point re-read
/// like the game (absolute point minus the corner of the block neighbouring the face, in doubles).
pub(crate) fn hit_slot(p: P, face: Dir, at: [f32; 3], facing: Dir, rows: i32, cols: i32) -> Option<usize> {
    if face != facing || face.axis() == 1 {
        return None;
    }
    let n = super::rel(p, face);
    let rel = |a: usize| (f64::from(p[a]) + f64::from(at[a])) - f64::from(n[a]);
    let (x, y) = match face {
        Dir::North => (1.0 - rel(0), rel(1)),
        Dir::South => (rel(0), rel(1)),
        Dir::West => (rel(2), rel(1)),
        _ => (1.0 - rel(2), rel(1)),
    };
    let section = |c: f32, max: i32| {
        let pixel = c * 16.0;
        let size = 16.0 / max as f32;
        ((pixel / size).floor() as i32).clamp(0, max - 1)
    };
    let row = section(1.0 - y as f32, rows);
    let col = section(x as f32, cols);
    Some((col + row * cols) as usize)
}

/// Chain part: 0 alone, 1 right, 2 centre, 3 left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Unconnected,
    Right,
    Center,
    Left,
}

impl Part {
    fn parse(s: Option<&str>) -> Part {
        match s {
            Some("right") => Part::Right,
            Some("center") => Part::Center,
            Some("left") => Part::Left,
            _ => Part::Unconnected,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Part::Unconnected => "unconnected",
            Part::Right => "right",
            Part::Center => "center",
            Part::Left => "left",
        }
    }

    fn connected(self) -> bool {
        self != Part::Unconnected
    }

    fn towards(self, end: Part) -> bool {
        self == Part::Center || self == end
    }

    fn chain_end(self) -> bool {
        self != Part::Center
    }

    fn with_right(self) -> Part {
        match self {
            Part::Unconnected | Part::Left => Part::Left,
            _ => Part::Center,
        }
    }

    fn with_left(self) -> Part {
        match self {
            Part::Unconnected | Part::Right => Part::Right,
            _ => Part::Center,
        }
    }

    fn without_right(self) -> Part {
        match self {
            Part::Unconnected | Part::Left => Part::Unconnected,
            _ => Part::Right,
        }
    }

    fn without_left(self) -> Part {
        match self {
            Part::Unconnected | Part::Right => Part::Unconnected,
            _ => Part::Left,
        }
    }
}

impl Server {
    fn shelf_part(&self, s: u32) -> Part {
        Part::parse(self.v().get(s, "side_chain"))
    }

    /// Shelf that can chain: tag `wooden_shelves`, powered.
    fn shelf_connectable(&self, s: u32) -> bool {
        self.v().get_bool(s, "powered") && self.btag_state(s, "minecraft:wooden_shelves")
    }

    fn btag_state(&self, s: u32, tag: &str) -> bool {
        let b = u32::from(self.v().block(s));
        self.block_tags.get(tag).is_some_and(|t| t.binary_search(&b).is_ok())
    }

    /// Chain neighbour at `steps` cells (left: clockwise relative to the facing): its part if it
    /// chains to this block (same facing), otherwise `None`.
    fn shelf_neighbor(&self, center: P, facing: Dir, left: bool, steps: i32) -> (P, Option<Part>) {
        let d = if left { facing.clockwise() } else { facing.counter_clockwise() };
        let q = rel_n(center, d, steps);
        let s = self.get(q);
        let ok = self.v().kind(s) == Kind::Shelf && self.shelf_connectable(s) && self.v().facing(s) == facing;
        (q, ok.then(|| self.shelf_part(s)))
    }

    /// The chain from left to right (empty if it cannot chain).
    fn shelf_chain(&self, p: P) -> Vec<P> {
        let s = self.get(p);
        if self.v().kind(s) != Kind::Shelf || !self.shelf_connectable(s) {
            return Vec::new();
        }
        let facing = self.v().facing(s);
        let mut out = std::collections::VecDeque::from([p]);
        for (left, end) in [(true, Part::Left), (false, Part::Right)] {
            for steps in 1..MAX_CHAIN {
                let (q, part) = self.shelf_neighbor(p, facing, left, steps);
                if part.is_some_and(|x| x.towards(end)) {
                    if left {
                        out.push_front(q);
                    } else {
                        out.push_back(q);
                    }
                }
                if part.is_none_or(Part::chain_end) {
                    break;
                }
            }
        }
        out.into()
    }

    /// Part changed only if it differs.
    fn shelf_set_part(&mut self, q: P, part: Part) {
        let s = self.get(q);
        if self.shelf_part(s) != part {
            let ns = self.v().set(s, "side_chain", part.name());
            self.set_update(q, ns);
        }
    }

    /// The reaction to placing a shelf block: chain formed if powered, otherwise neighbours detached.
    pub(crate) fn shelf_on_place(&mut self, p: P, s: u32, old: u32) {
        if self.v().get_bool(s, "powered") {
            self.shelf_powering_up(p, s, old);
        } else {
            self.shelf_powering_down(p, s);
        }
    }

    /// Left neighbour detached from its right, right neighbour
    /// detached from its left.
    pub(crate) fn shelf_powering_down(&mut self, p: P, s: u32) {
        let facing = self.v().facing(s);
        let (lq, lp) = self.shelf_neighbor(p, facing, true, 1);
        let (rq, rp) = self.shelf_neighbor(p, facing, false, 1);
        if let Some(x) = lp {
            self.shelf_set_part(lq, x.without_right());
        }
        if let Some(x) = rp {
            self.shelf_set_part(rq, x.without_left());
        }
    }

    /// Neighbouring chains joined if the total
    /// length stays ≤ 3 (left first), parts set.
    fn shelf_powering_up(&mut self, p: P, s: u32, old: u32) {
        if !self.shelf_connectable(s) {
            return;
        }
        let old_connected =
            self.v().kind(old) == Kind::Shelf && self.shelf_connectable(old) && self.shelf_part(old).connected();
        if self.shelf_part(s).connected() || old_connected {
            return;
        }
        let facing = self.v().facing(s);
        let (lq, lp) = self.shelf_neighbor(p, facing, true, 1);
        let (rq, rp) = self.shelf_neighbor(p, facing, false, 1);
        let left_len = if lp.is_some() { self.shelf_chain(lq).len() as i32 } else { 0 };
        let right_len = if rp.is_some() { self.shelf_chain(rq).len() as i32 } else { 0 };
        let can = |n: i32, cur: i32| n > 0 && cur + n <= MAX_CHAIN;
        let mut me = Part::Unconnected;
        let mut cur = 1;
        if can(left_len, cur) {
            me = me.with_left();
            if let Some(x) = lp {
                self.shelf_set_part(lq, x.with_right());
            }
            cur += left_len;
        }
        if can(right_len, cur) {
            me = me.with_right();
            if let Some(x) = rp {
                self.shelf_set_part(rq, x.with_left());
            }
        }
        self.shelf_set_part(p, me);
    }

    /// Power followed (chain cut without current), sound, event.
    pub(crate) fn shelf_neighbor_changed(&mut self, p: P, s: u32) {
        let signal = self.has_neighbor_signal(p);
        if self.v().get_bool(s, "powered") == signal {
            return;
        }
        let mut ns = self.v().set_bool(s, "powered", signal);
        if !signal {
            ns = self.v().set(ns, "side_chain", "unconnected");
        }
        self.set_update(p, ns);
        let sound = if signal { "minecraft:block.shelf.activate" } else { "minecraft:block.shelf.deactivate" };
        self.block_sound(p, sound, 1.0, 1.0);
        let ev = if signal { "minecraft:block_activate" } else { "minecraft:block_deactivate" };
        self.game_event_block(ev, p, None, Some(ns));
    }

    /// Shelf block entity modified: comparators notified,
    /// game event if any (block state), contents sent back to the client.
    fn shelf_changed(&mut self, q: P, event: Option<&str>) {
        let s = self.get(q);
        let b = self.v().block(s);
        self.update_neighbour_for_output_signal(q, b);
        if let Some(ev) = event {
            self.game_event_block(ev, q, None, Some(s));
        }
        self.changed.push(q);
    }

    /// Use of an item on a shelf block (main hand).
    pub(crate) fn shelf_use_item(&mut self, p: P, s: u32, face: Dir, at: [f32; 3], slot: usize) -> ItemOn {
        if self.be_get(p).is_none() {
            return ItemOn::Pass;
        }
        let Some(hit) = hit_slot(p, face, at, self.v().facing(s), 1, 3) else { return ItemOn::Pass };
        let held = self.slots[slot].clone();
        if !self.v().get_bool(s, "powered") {
            // The whole held stack against the slot's one.
            let mut items = self.be_get(p).map_or_else(|| vec![None; 3], |b| read_items(b, 3));
            let removed = std::mem::replace(&mut items[hit], held.clone());
            if let Some(be) = self.be_get_mut(p) {
                write_items(be, &items);
            }
            let creative = self.game_mode == 1;
            let back = if creative && removed.is_none() { held.clone() } else { removed.clone() };
            self.slots[slot] = back.clone();
            let quiet = back.as_ref().is_some_and(|b| {
                self.life.items.default_component(b, "minecraft:use_effects").is_some_and(|u| {
                    u["interact_vibrations"].as_bool() == Some(false)
                })
            });
            let event = (!quiet).then_some("minecraft:item_interact_finish");
            self.shelf_changed(p, event);
            if removed.is_some() {
                let sound =
                    if held.is_none() { "minecraft:block.shelf.take_item" } else { "minecraft:block.shelf.single_swap" };
                self.block_sound(p, sound, 1.0, 1.0);
            } else {
                if held.is_none() {
                    self.broadcast_menus();
                    return ItemOn::Pass;
                }
                self.block_sound(p, "minecraft:block.shelf.place_item", 1.0, 1.0);
            }
            self.broadcast_menus();
            return ItemOn::Done;
        }
        // Powered: the bar swapped with the whole chain.
        let chain = self.shelf_chain(p);
        if chain.is_empty() {
            return ItemOn::Done;
        }
        let n = chain.len();
        let mut any = false;
        for (k, &q) in chain.iter().enumerate() {
            if self.be_get(q).is_none() {
                continue;
            }
            let mut items = self.be_get(q).map_or_else(|| vec![None; 3], |b| read_items(b, 3));
            for (i, item) in items.iter_mut().enumerate() {
                // Bar slot: the chain occupies the last slots (3 per shelf).
                let hs = 36 + 9 - (n - k) * 3 + i;
                let placed = self.slots[hs].take();
                let removed = std::mem::replace(item, placed.clone());
                if placed.is_some() || removed.is_some() {
                    self.slots[hs] = removed;
                    any = true;
                }
            }
            if let Some(be) = self.be_get_mut(q) {
                write_items(be, &items);
            }
            self.shelf_changed(q, Some("minecraft:entity_interact"));
        }
        self.broadcast_menus();
        if any {
            self.block_sound(p, "minecraft:block.shelf.multi_swap", 1.0, 1.0);
        }
        ItemOn::Done
    }

    /// Shelf comparator output: only toward the back; one bit per full slot.
    pub(crate) fn shelf_comparator(&self, p: P, s: u32, from: Dir) -> i32 {
        if from != self.v().facing(s).opposite() {
            return 0;
        }
        let Some(be) = self.be_get(p) else { return 0 };
        let items = read_items(be, 3);
        items.iter().enumerate().map(|(i, it)| i32::from(it.is_some()) << i).sum()
    }

    // --- Chiseled bookshelf. ---

    /// Book (tag `bookshelf_books`) stored in the empty targeted slot.
    pub(crate) fn bookshelf_use_item(&mut self, p: P, s: u32, face: Dir, at: [f32; 3], slot: usize) -> ItemOn {
        if self.be_get(p).is_none() {
            return ItemOn::Pass;
        }
        let Some(held) = self.slots[slot].clone() else { return ItemOn::TryEmptyHand };
        if !self.life.items.has_tag("minecraft:bookshelf_books", &held.id) {
            return ItemOn::TryEmptyHand;
        }
        let Some(hit) = hit_slot(p, face, at, self.v().facing(s), 2, 3) else { return ItemOn::Pass };
        if self.v().get_bool(s, &format!("slot_{hit}_occupied")) {
            return ItemOn::TryEmptyHand;
        }
        self.award_stat("minecraft:used", &held.id, 1);
        let sound = if held.id == "minecraft:enchanted_book" {
            "minecraft:block.chiseled_bookshelf.insert.enchanted"
        } else {
            "minecraft:block.chiseled_bookshelf.insert"
        };
        let book = Stack { count: 1, ..held };
        self.consume_held(slot);
        self.bookshelf_set(p, hit, Some(book));
        self.block_sound(p, sound, 1.0, 1.0);
        self.broadcast_menus();
        ItemOn::Done
    }

    /// Book of the targeted slot taken back; empty slot → action kept.
    pub(crate) fn bookshelf_use_without_item(&mut self, p: P, s: u32, face: Dir, at: [f32; 3]) -> bool {
        if self.be_get(p).is_none() {
            return false;
        }
        let Some(hit) = hit_slot(p, face, at, self.v().facing(s), 2, 3) else { return false };
        if !self.v().get_bool(s, &format!("slot_{hit}_occupied")) {
            return true;
        }
        let book = self.be_get(p).and_then(|b| read_items(b, 6).swap_remove(hit));
        self.bookshelf_set(p, hit, None);
        let enchanted = book.as_ref().is_some_and(|b| b.id == "minecraft:enchanted_book");
        let sound = if enchanted {
            "minecraft:block.chiseled_bookshelf.pickup.enchanted"
        } else {
            "minecraft:block.chiseled_bookshelf.pickup"
        };
        self.block_sound(p, sound, 1.0, 1.0);
        if let Some(book) = book {
            let selected = self.held as usize;
            let creative = self.game_mode == 1;
            if let Some(rest) = self.pickup_into_inventory(book, creative, selected) {
                self.drop_from_player(rest, false);
            }
        }
        self.game_event_block("minecraft:block_change", p, Some(Ent::Player), None);
        self.broadcast_menus();
        true
    }

    /// Putting an item in place / removing an item then the state update: slot changed, last slot touched, state
    /// of the 6 slots set, game event.
    fn bookshelf_set(&mut self, p: P, slot: usize, item: Option<Stack>) {
        let Some(be) = self.be_get(p) else { return };
        let mut items = read_items(be, 6);
        items[slot] = item;
        if let Some(be) = self.be_get_mut(p) {
            write_items(be, &items);
            crate::nbtio::set(be, "last_interacted_slot", rc_proto::nbt::Nbt::Int(slot as i32));
        }
        let mut ns = self.get(p);
        for (i, it) in items.iter().enumerate() {
            ns = self.v().set_bool(ns, &format!("slot_{i}_occupied"), it.is_some());
        }
        self.set_update(p, ns);
        self.game_event_block("minecraft:block_change", p, None, Some(ns));
    }

    /// Comparator output: last slot touched + 1 (0 if none).
    pub(crate) fn bookshelf_comparator(&self, p: P) -> i32 {
        self.be_get(p).and_then(|b| crate::nbtio::int(b, "last_interacted_slot")).map_or(-1, |v| v as i32) + 1
    }
}
