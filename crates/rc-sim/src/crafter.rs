//! Crafter (G5-F11): block menu (3×3 grid of the block entity, shown result, disabled
//! slots and received signal as data) and packet `container_slot_state_changed`; the craft
//! itself on its scheduled tick, its `crafting` countdown and its comparator output.
//!
//! Block entity data (the game's keys): `Items`, `disabled_slots` (disabled slots, read
//! only on an empty slot), `triggered` (signal received, without marking the chunk for rewrite),
//! `crafting_ticks_remaining` (ticks left showing the `crafting` face).

use rc_data::PropKey;
use rc_proto::nbt::Nbt;

use crate::blocks::level::BeTick;
use crate::blocks::{Dir, Kind as BlockKind, P, flag, rel};
use crate::containers::read_items;
use crate::menus::{Kind, Menu};
use crate::player::Stack;
use crate::server::Server;

/// Ticks a crafter shows its `crafting` face after a craft.
const CRAFTING_TICKS: i32 = 6;
/// Side of the cube centred on the crafter whose players are credited with its craft.
const CRAFT_CREDIT_SIDE: f64 = 17.0;
/// Level events of a craft: no recipe for the grid; an item thrown out; its smoke (data: the
/// direction it was thrown towards).
const EVENT_NO_RECIPE: i32 = 1050;
const EVENT_ITEM_THROWN: i32 = 1049;
const EVENT_THROW_SMOKE: i32 = 2010;

/// Disabled slots of a block entity (a full slot never is).
fn disabled(be: &Nbt) -> [bool; 9] {
    disabled_among(be, &read_items(be, 9))
}

/// Disabled slots given the block entity's items already read.
fn disabled_among(be: &Nbt, items: &[Option<Stack>]) -> [bool; 9] {
    let mut d = [false; 9];
    if let Some(Nbt::IntArray(v)) = be.get("disabled_slots") {
        for &k in v {
            if let Some(i) = usize::try_from(k).ok().filter(|&i| i < 9 && items[i].is_none()) {
                d[i] = true;
            }
        }
    }
    d
}

/// Crafter data as the game keeps it after a load: it reads the items first, then disables
/// only the listed slots 0-8 that are empty, so an occupied or out-of-range index is dropped
/// (and the next save writes the cleaned, sorted list).
pub(crate) fn keep_loadable_disabled_slots(be: &mut Nbt) {
    let d = disabled(be);
    let kept = (0..9).filter(|&i| d[i]).map(|i| i as i32).collect();
    crate::nbtio::set(be, "disabled_slots", Nbt::IntArray(kept));
}

impl Server {
    /// The 10 menu data: disabled slots (1), then the received signal.
    pub(crate) fn crafter_data(&self, p: P) -> Vec<i32> {
        let Some(be) = self.be_get(p) else { return vec![0; 10] };
        let mut d: Vec<i32> = disabled(be).iter().map(|&x| i32::from(x)).collect();
        d.push(crate::nbtio::int(be, "triggered").unwrap_or(0) as i32);
        d
    }

    /// Result shown for a crafter grid.
    pub(crate) fn crafter_result(&self, grid: &[Option<crate::player::Stack>]) -> Option<crate::player::Stack> {
        let maps = &self.maps;
        self.life.recipes.craft(&self.life.items, grid, 3, &|id| maps.scale_of(id)).map(|r| r.1)
    }

    /// Crafter menu opened at `p` (the block entity must exist).
    pub(crate) fn crafter_menu(&self, p: P, window: i32) -> Option<Menu> {
        let be = self.be_get(p)?;
        let mut m = Menu::crafter(window, read_items(be, 9));
        m.own[9] = self.crafter_result(&m.own[..9]);
        m.data = self.crafter_data(p);
        m.block = Some(p);
        Some(m)
    }

    /// Received signal kept in the block entity.
    pub(crate) fn crafter_set_triggered(&mut self, p: P, on: bool) {
        let Some(i) = self.be_index(p) else { return };
        if let Some(Some(c)) = self.chunks.get_mut(&(p[0] >> 4, p[2] >> 4))
            && let Some(be) = c.block_entities.get_mut(i)
        {
            crate::nbtio::set(be, "triggered", Nbt::Int(i32::from(on)));
        }
    }

    /// `container_slot_state_changed`: grid slot enabled or disabled by the player of the
    /// open menu; only an empty slot changes.
    pub(crate) fn crafter_slot_state(&mut self, slot: i32, window: i32, enabled: bool) {
        if self.game_mode == 3 || window != self.current_window() {
            return;
        }
        let Some(p) = self.life.menu.as_ref().filter(|m| m.kind == Kind::Crafter).and_then(|m| m.block) else {
            return;
        };
        let Some(k) = usize::try_from(slot).ok().filter(|&k| k < 9) else { return };
        let Some(be) = self.be_get(p) else { return };
        if read_items(be, 9)[k].is_some() {
            return;
        }
        let mut d = disabled(be);
        d[k] = !enabled;
        let list: Vec<i32> = (0..9).filter(|&i| d[i]).map(|i| i as i32).collect();
        if let Some(be) = self.be_get_mut(p) {
            crate::nbtio::set(be, "disabled_slots", Nbt::IntArray(list));
        }
        self.container_changed(p);
    }

    /// Comparator output: the slots that hold an item or are disabled (0 to 9), not a fill ratio.
    pub(crate) fn crafter_comparator(&self, p: P) -> i32 {
        let Some(be) = self.be_get(p) else { return 0 };
        let items = read_items(be, 9);
        let off = disabled_among(be, &items);
        (0..9).filter(|&i| items[i].is_some() || off[i]).count() as i32
    }
}

/// Direction the crafter faces (first half of its `orientation`, e.g. `north_up`).
fn crafter_front(s: u32, server: &Server) -> Dir {
    let v = server.view();
    v.get(s, PropKey::Orientation).and_then(|o| o.split('_').next()).and_then(Dir::parse).unwrap_or(Dir::North)
}

/// `stack` minus one item, nothing when that was the last.
fn minus_one(stack: Stack) -> Option<Stack> {
    let rest = Stack { count: stack.count - 1, ..stack };
    (rest.count > 0).then_some(rest)
}

impl Server {
    /// Scheduled tick of a crafter that received a signal: crafts its grid's recipe once. No
    /// recipe: the refusal event. Otherwise the `crafting` face for 6 ticks, the result (a
    /// crafted map enlarged or locked first) then each remainder go out through the front, and
    /// one item of every filled slot is used.
    pub(crate) fn crafter_craft(&mut self, s: u32, p: P) {
        let Some(be) = self.be_get(p) else { return };
        let grid = read_items(be, 9);
        let maps = &self.maps;
        let found = self.life.recipes.craft(&self.life.items, &grid, 3, &|id| maps.scale_of(id));
        let Some((recipe, mut result)) = found.filter(|(_, result)| result.count > 0) else {
            self.level_event(EVENT_NO_RECIPE, p, 0);
            return;
        };
        self.crafter_start_countdown(p);
        let crafting = self.view().set_bool(s, PropKey::Crafting, true);
        self.set_block(p, crafting, flag::CLIENTS);
        self.map_post_process_stack(&mut result);
        let remainders = self.life.recipes.remaining_of(recipe, &self.life.items, &grid);
        let front = crafter_front(s, self);
        self.crafter_output(p, front, result, recipe, &grid);
        for rest in remainders.into_iter().flatten().filter(|r| r.count > 0) {
            self.crafter_output(p, front, rest, recipe, &grid);
        }
        self.crafter_use_one_of_each(p);
    }

    /// One item used from every filled slot, then the block entity's change mark (comparators).
    fn crafter_use_one_of_each(&mut self, p: P) {
        let Some(be) = self.be_get_mut(p) else { return };
        let items: Vec<Option<Stack>> = read_items(be, 9).into_iter().map(|slot| slot.and_then(minus_one)).collect();
        crate::containers::write_items(be, &items);
        self.container_changed(p);
    }

    /// One stack out of the crafter through its front: into the container there; what is left
    /// is thrown 0.7 block ahead with the craft credited, the click and the smoke (nothing of
    /// that when the container took everything).
    fn crafter_output(&mut self, p: P, front: Dir, stack: Stack, recipe: usize, grid: &[Option<Stack>]) {
        let Some(rest) = self.crafter_insert_in_front(p, front, stack) else { return };
        self.dispense_spawn_item(p, front, rest);
        self.crafter_credit_players(p, recipe, grid);
        self.level_event(EVENT_ITEM_THROWN, p, 0);
        self.level_event(EVENT_THROW_SMOKE, p, front as i32);
    }

    /// The crafter's stack offered to the container in front: the composter there, a container
    /// block, otherwise a container minecart drawn among those in the cell. Returns what is left.
    fn crafter_insert_in_front(&mut self, p: P, front: Dir, stack: Stack) -> Option<Stack> {
        let target = rel(p, front);
        let face = front.opposite();
        // The composter takes one compostable item from above, never more.
        let one = Stack { count: 1, ..stack.clone() }.to_nbt(None);
        if let Some(taken) = self.composter_put(target, &one, face) {
            return if taken { minus_one(stack) } else { Some(stack) };
        }
        if let Some(dest) = self.container_at(target) {
            self.unseal_all(&dest); // S8: sealed loot drawn by the container read
            return self.crafter_fill_container(&dest, target, stack, face);
        }
        match self.cart_container_at(target) {
            Some(cart) => self.crafter_fill_cart(cart, stack),
            None => Some(stack),
        }
    }

    /// Into a container block: one item at a time into another crafter (its slots fill evenly)
    /// or when the stack is larger than a slot holds (nothing beyond a slot is lost); otherwise
    /// the whole stack, pass after pass while some of it goes in. An empty hopper that receives
    /// something starts its transfer cooldown.
    fn crafter_fill_container(&mut self, dest: &[P], target: P, mut stack: Stack, face: Dir) -> Option<Stack> {
        let was_empty_hopper = self.hopper_is_empty(target);
        let kind = self.view().kind(self.get(target));
        let slot_size = if kind == BlockKind::Jukebox { 1 } else { self.life.items.max_stack(&stack) };
        let rest = if kind == BlockKind::Crafter || stack.count > slot_size {
            let one = Stack { count: 1, ..stack.clone() }.to_nbt(None);
            while stack.count > 0 && self.insert_one(dest, &one, Some(face)) {
                stack.count -= 1;
            }
            (stack.count > 0).then_some(stack)
        } else {
            let mut rest = Some(stack);
            while let Some(left) = rest.take() {
                let before = left.count;
                rest = self.insert_stack(dest, left, Some(face));
                if rest.as_ref().is_none_or(|r| r.count == before) {
                    break;
                }
            }
            rest
        };
        if was_empty_hopper && !self.hopper_is_empty(target) {
            self.hopper_filled(target, None);
        }
        rest
    }

    /// Into a container minecart: one item at a time when the stack is larger than a slot holds,
    /// otherwise the whole stack in one pass (a second pass would place nothing more).
    fn crafter_fill_cart(&mut self, cart: i32, mut stack: Stack) -> Option<Stack> {
        let Some((at, mut c)) = self.cart_take(cart) else { return Some(stack) };
        let rest = if stack.count > self.life.items.max_stack(&stack) {
            while stack.count > 0 && self.cart_insert(&mut c, Stack { count: 1, ..stack.clone() }).is_none() {
                stack.count -= 1;
            }
            (stack.count > 0).then_some(stack)
        } else {
            self.cart_insert(&mut c, stack)
        };
        self.cart_put(at, c);
        rest
    }

    /// The craft credited to the player near the crafter (box of 17 blocks centred on it,
    /// spectators left out), with the grid's items as the ingredients.
    fn crafter_credit_players(&mut self, p: P, recipe: usize, grid: &[Option<Stack>]) {
        let half = CRAFT_CREDIT_SIDE / 2.0;
        let centre = p.map(|c| f64::from(c) + 0.5);
        let area = rc_physics::Aabb { min: centre.map(|c| c - half), max: centre.map(|c| c + half) };
        if self.game_mode == 3 || !self.player_aabb().intersects(&area) {
            return;
        }
        let Some(id) = self.life.recipes.list.get(recipe).map(|r| r.id.clone()) else { return };
        let used: Vec<Stack> = grid.iter().flatten().cloned().collect();
        self.trig_recipe_crafted("minecraft:crafter_recipe_crafted", &id, &used);
    }

    /// Six ticks of the `crafting` face counted down by the block entity (saved with it). Its
    /// ticker runs only while it counts: an idle crafter's tick does nothing in the game.
    fn crafter_start_countdown(&mut self, p: P) {
        self.be_set_int(p, "crafting_ticks_remaining", CRAFTING_TICKS);
        self.ticker_register(p, BeTick::Crafter);
    }

    /// Block entity tick of a crafting crafter: one tick less; at zero, the `crafting` face goes
    /// away (neighbours told) and the ticker leaves.
    pub(crate) fn crafter_countdown_tick(&mut self, p: P) {
        let left = self.be_int(p, "crafting_ticks_remaining") - 1;
        if left < 0 {
            self.bs.be_ticks.remove(&p);
            return;
        }
        self.be_set_int(p, "crafting_ticks_remaining", left);
        if left == 0 {
            let s = self.get(p);
            let idle = self.view().set_bool(s, PropKey::Crafting, false);
            self.set_block(p, idle, flag::ALL);
            self.bs.be_ticks.remove(&p);
        }
    }

    /// Ticker of a crafter whose block entity was loaded or rewritten: present while its
    /// countdown runs.
    pub(crate) fn crafter_ticker_sync(&mut self, p: P) {
        let counting = self.be_int(p, "crafting_ticks_remaining") > 0;
        let registered = matches!(self.bs.be_ticks.get(&p), Some((_, BeTick::Crafter)));
        if counting && !registered {
            self.ticker_register(p, BeTick::Crafter);
        } else if !counting && registered {
            self.bs.be_ticks.remove(&p);
        }
    }
}
