//! Integrated-server campfire: food placed on click (campfire cooking recipe,
//! 4 slots), cooking per tick while lit, cooling by 2 per tick when put out,
//! cooked food dropped at the block; extinguished with a shovel (tag `douses_campfires`).
//!
//! Block entity data (the game's keys): `Items`, `CookingTimes`, `CookingTotalTimes`. The
//! ticker keeps two facts (food present, cooking started) to skip empty fires without
//! re-reading the block entity; the result is identical (a tick with no food or cooking does
//! nothing) as long as those facts follow every write to the block entity: click, tick, chunk
//! load and write by command (`be_ticks_reloaded`).

use rc_proto::nbt::Nbt;

use super::level::BeTick;
use super::use_block::ItemOn;
use super::{Kind, P};
use crate::containers::{read_items, write_items};
use crate::mobs::mob::Ent;
use crate::player::Stack;
use crate::recipes::CookKind;
use crate::server::Server;

const SLOTS: usize = 4;

fn times(be: &Nbt, key: &str) -> [i32; SLOTS] {
    let mut out = [0; SLOTS];
    if let Some(Nbt::IntArray(v)) = be.get(key) {
        for (o, x) in out.iter_mut().zip(v) {
            *o = *x;
        }
    }
    out
}

/// Ticker flags: food present, cooking started.
fn flags(items: &[Option<Stack>], progress: &[i32; SLOTS]) -> BeTick {
    BeTick::Campfire { food: items.iter().any(Option::is_some), warm: progress.iter().any(|&t| t > 0) }
}

impl Server {
    /// Food placed (statistic), or shovel putting out the lit fire.
    pub(crate) fn campfire_use_item(&mut self, p: P, s: u32, slot: usize) -> ItemOn {
        let held = self.slots[slot].clone();
        if self.be_get(p).is_some()
            && let Some(h) = &held
            && self.life.recipes.cooking(&self.life.items, CookKind::Campfire, h).is_some()
        {
            if self.campfire_place_food(p, s, slot) {
                self.award_stat("minecraft:custom", "minecraft:interact_with_campfire", 1);
            }
            return ItemOn::Done;
        }
        let douses = held.as_ref().is_some_and(|h| self.life.items.has_tag("minecraft:douses_campfires", &h.id));
        if douses && self.v().get_bool(s, "lit") {
            self.level_event(1009, p, 0);
            self.game_event_block("minecraft:block_change", p, Some(Ent::Player), None);
            let ns = self.v().set_bool(s, "lit", false);
            self.set_update(p, ns);
            self.hurt_item(slot, 1);
            return ItemOn::Done;
        }
        ItemOn::TryEmptyHand
    }

    /// First empty slot, recipe duration, one unit taken; true if placed.
    fn campfire_place_food(&mut self, p: P, s: u32, slot: usize) -> bool {
        let Some(be) = self.be_get(p) else { return false };
        let mut items = read_items(be, SLOTS);
        let mut progress = times(be, "CookingTimes");
        let mut total = times(be, "CookingTotalTimes");
        let Some(free) = items.iter().position(Option::is_none) else { return false };
        let Some(held) = self.slots[slot].clone() else { return false };
        let Some(time) = self.life.recipes.cooking(&self.life.items, CookKind::Campfire, &held).map(|(_, c)| c.time)
        else {
            return false;
        };
        total[free] = time;
        progress[free] = 0;
        items[free] = Some(Stack { count: 1, ..held });
        self.consume_held(slot);
        self.campfire_store(p, &items, &progress, &total);
        self.game_event_block("minecraft:block_change", p, Some(Ent::Player), Some(s));
        // State and contents sent back to the client (end of tick).
        self.changed.push(p);
        true
    }

    fn campfire_store(&mut self, p: P, items: &[Option<Stack>], progress: &[i32; SLOTS], total: &[i32; SLOTS]) {
        if let Some(be) = self.be_get_mut(p) {
            write_items(be, items);
            crate::nbtio::set(be, "CookingTimes", Nbt::IntArray(progress.to_vec()));
            crate::nbtio::set(be, "CookingTotalTimes", Nbt::IntArray(total.to_vec()));
        }
        if let Some((_, t)) = self.bs.be_ticks.get_mut(&p) {
            *t = flags(items, progress);
        }
    }

    /// Ticker: cooking (lit) or cooling (out). Skipped, without re-reading the
    /// block entity, when there is nothing to cook (lit) or to cool (out).
    pub(crate) fn campfire_tick(&mut self, p: P, food: bool, warm: bool) {
        let s = self.get(p);
        let lit = self.v().get_bool(s, "lit");
        if if lit { !food } else { !warm } {
            return;
        }
        let Some(be) = self.be_get(p) else { return };
        let mut items = read_items(be, SLOTS);
        let mut progress = times(be, "CookingTimes");
        let total = times(be, "CookingTotalTimes");
        let mut changed = false;
        if lit {
            for i in 0..SLOTS {
                let Some(it) = items[i].clone() else { continue };
                changed = true;
                progress[i] += 1;
                if progress[i] >= total[i] {
                    let result = self
                        .life
                        .recipes
                        .cooking(&self.life.items, CookKind::Campfire, &it)
                        .map_or(it, |(_, c)| c.result.stack());
                    self.drop_contents([f64::from(p[0]), f64::from(p[1]), f64::from(p[2])], result);
                    items[i] = None;
                    self.changed.push(p);
                    self.game_event_block("minecraft:block_change", p, None, Some(s));
                }
            }
        } else {
            for i in 0..SLOTS {
                if progress[i] > 0 {
                    changed = true;
                    progress[i] = (progress[i] - 2).clamp(0, total[i].max(0));
                }
            }
        }
        if changed {
            self.campfire_store(p, &items, &progress, &total);
        }
    }

    /// Ticker placed for the campfire at `p` (block entity created or re-read).
    pub(crate) fn campfire_ticker_register(&mut self, p: P) {
        if matches!(self.bs.be_ticks.get(&p), Some((_, BeTick::Campfire { .. }))) {
            return;
        }
        let state = match self.be_get(p) {
            Some(be) => flags(&read_items(be, SLOTS), &times(be, "CookingTimes")),
            None => BeTick::Campfire { food: false, warm: false },
        };
        let id = self.add_ticker(p);
        self.bs.be_ticks.insert(p, (id, state));
    }

    /// Block entity rewritten from outside (`/data`, `/setblock`, `/clone`, `/item`: `old`, the
    /// data from before): state kept next to the re-read NBT, as reading its own fields re-reads it.
    pub(crate) fn be_ticks_reloaded(&mut self, p: P, old: &Nbt) {
        match self.v().kind(self.get(p)) {
            Kind::Campfire => {
                let state = self.be_get(p).map(|be| flags(&read_items(be, SLOTS), &times(be, "CookingTimes")));
                if let (Some(state), Some((_, t))) = (state, self.bs.be_ticks.get_mut(&p))
                    && matches!(t, BeTick::Campfire { .. })
                {
                    *t = state;
                }
            }
            Kind::Jukebox => self.jukebox_reloaded(p, old),
            _ => {}
        }
    }

    /// Ticker still valid (G6-F01); `None`: this ticker is not one of the campfire tickers. A dead
    /// ticker is forgotten.
    pub(crate) fn be_tick_alive(&mut self, id: u64, p: P) -> Option<bool> {
        let &(i, t) = self.bs.be_ticks.get(&p)?;
        if i != id {
            return None;
        }
        let s = self.get(p);
        let alive = match t {
            BeTick::Jukebox(_) => self.v().kind(s) == Kind::Jukebox && self.v().get_bool(s, "has_record"),
            BeTick::Campfire { .. } => self.v().kind(s) == Kind::Campfire,
            BeTick::Gateway => self.state_name(s) == "minecraft:end_gateway",
            BeTick::Beacon(_) => self.v().kind(s) == Kind::Beacon,
            BeTick::Conduit(_) => self.v().block(s) == self.bs.t.id.conduit,
        } && self.be_get(p).is_some();
        if !alive {
            self.bs.be_ticks.remove(&p);
        }
        Some(alive)
    }

    /// Index of the block entity type at `p` (registry `block_entity_type`).
    pub(crate) fn be_type_index(&self, p: P) -> Option<i32> {
        let id = self.be_get(p)?.get("id").and_then(Nbt::as_str)?;
        let r = self.data.registry("minecraft:block_entity_type")?;
        r.entries.iter().position(|e| e == id).map(|i| i as i32)
    }

    /// Network update of the block entity for blocks with displayed contents, sent
    /// along with the state change; `None`: no packet for this block.
    pub(crate) fn be_update_tag(&self, p: P, s: u32) -> Option<Nbt> {
        if self.v().block(s) == self.bs.t.id.conduit {
            return self.conduit_update_tag(p); // G6-F12
        }
        let keys: &[&str] = match self.v().kind(s) {
            Kind::Campfire => &["Items"],
            Kind::Shelf => &["Items", "align_items_to_bottom"],
            Kind::DecoratedPot => &["sherds", "item"],
            _ => return None,
        };
        let be = self.be_get(p)?;
        let mut out = Vec::with_capacity(keys.len());
        for k in keys {
            match be.get(k) {
                Some(v) => out.push(((*k).to_owned(), v.clone())),
                // Lists and flags always written by the game, even when empty.
                None if *k == "Items" => out.push(("Items".to_owned(), Nbt::List(Vec::new()))),
                None if *k == "align_items_to_bottom" => out.push((k.to_string(), Nbt::Byte(0))),
                None => {}
            }
        }
        Some(Nbt::Compound(out))
    }
}
