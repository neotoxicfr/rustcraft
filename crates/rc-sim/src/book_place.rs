//! W8c: placing a recipe from the book (`place_recipe`), like game 26.3: check of
//! the room to empty the grid, matching ingredients ↔ items by augmenting paths,
//! one more batch at each re-click, stack cap, centring in the grid, taking from
//! the hotbar then the inventory. In our own words.

use std::collections::BTreeMap;

use crate::items::Items;
use crate::menus::{Env, Menu, Src};
use crate::player::Stack;
use crate::recipes::Ingredient;

/// Item usable for crafting: not damaged, not enchanted, not renamed.
pub(crate) fn usable(items: &Items, s: &Stack) -> bool {
    items.damage(s) == 0
        && crate::items::component(s, "minecraft:enchantments")
            .is_none_or(|e| matches!(e, rc_proto::nbt::Nbt::Compound(f) if f.is_empty()))
        && crate::items::component(s, "minecraft:custom_name").is_none()
}

/// Quantities available per item (item identifier → count), stable order.
pub(crate) type Amounts = BTreeMap<i32, i32>;

fn accepts(ing: &Ingredient, item: i32) -> bool {
    ing.binary_search(&item).is_ok()
}

/// Matching of a recipe: each ingredient gets a distinct item
/// per slot, `capacity` copies per retained item.
struct Picker<'a> {
    ings: &'a [Ingredient],
    items: Vec<i32>,
    conn: Vec<bool>,
    assigned: Vec<bool>,
    satisfied: Vec<bool>,
    vis_ing: Vec<bool>,
    vis_item: Vec<bool>,
}

impl<'a> Picker<'a> {
    fn new(ings: &'a [Ingredient], amounts: &Amounts) -> Picker<'a> {
        let items: Vec<i32> =
            amounts.iter().filter(|(i, n)| **n > 0 && ings.iter().any(|g| accepts(g, **i))).map(|(i, _)| *i).collect();
        let (ni, nt) = (ings.len(), items.len());
        let mut conn = vec![false; ni * nt];
        for (g, ing) in ings.iter().enumerate() {
            for (t, &it) in items.iter().enumerate() {
                if accepts(ing, it) {
                    conn[t * ni + g] = true;
                }
            }
        }
        Picker {
            ings,
            items,
            conn,
            assigned: vec![false; ni * nt],
            satisfied: vec![false; ni],
            vis_ing: vec![false; ni],
            vis_item: vec![false; nt],
        }
    }

    fn idx(&self, item: usize, ing: usize) -> usize {
        item * self.ings.len() + ing
    }

    fn try_pick(&mut self, amounts: &mut Amounts, cap: i32, mut out: Option<&mut Vec<i32>>) -> bool {
        if cap <= 0 {
            return true;
        }
        let (ni, nt) = (self.ings.len(), self.items.len());
        let mut sat_count = 0;
        loop {
            let Some(path) = self.try_assign(amounts, cap) else {
                let valid = sat_count == ni;
                self.vis_ing.fill(false);
                self.vis_item.fill(false);
                self.satisfied.fill(false);
                for g in 0..ni {
                    for t in 0..nt {
                        let k = self.idx(t, g);
                        if self.assigned[k] {
                            self.assigned[k] = false;
                            *amounts.entry(self.items[t]).or_insert(0) += cap;
                            if valid && let Some(o) = out.as_deref_mut() {
                                o.push(self.items[t]);
                            }
                            break;
                        }
                    }
                }
                return valid;
            };
            let first = path[0];
            *amounts.entry(self.items[first]).or_insert(0) -= cap;
            let last = path[path.len() - 1];
            self.satisfied[last] = true;
            sat_count += 1;
            for i in 0..path.len() - 1 {
                if i & 1 == 0 {
                    let k = self.idx(path[i], path[i + 1]);
                    self.assigned[k] = true;
                } else {
                    let k = self.idx(path[i + 1], path[i]);
                    self.assigned[k] = false;
                }
            }
        }
    }

    fn try_assign(&mut self, amounts: &Amounts, cap: i32) -> Option<Vec<usize>> {
        self.vis_ing.fill(false);
        self.vis_item.fill(false);
        for t in 0..self.items.len() {
            if amounts.get(&self.items[t]).copied().unwrap_or(0) >= cap
                && let Some(p) = self.find_path(t)
            {
                return Some(p);
            }
        }
        None
    }

    fn find_path(&mut self, start: usize) -> Option<Vec<usize>> {
        let (ni, nt) = (self.ings.len(), self.items.len());
        let mut path = vec![start];
        self.vis_item[start] = true;
        while !path.is_empty() {
            let len = path.len();
            if (len - 1) & 1 == 0 {
                let item = path[len - 1];
                for g in 0..ni {
                    if !self.vis_ing[g] && self.conn[self.idx(item, g)] && !self.assigned[self.idx(item, g)] {
                        self.vis_ing[g] = true;
                        path.push(g);
                        break;
                    }
                }
            } else {
                let g = path[len - 1];
                if !self.satisfied[g] {
                    return Some(path);
                }
                for t in 0..nt {
                    if !self.vis_item[t] && self.assigned[self.idx(t, g)] {
                        self.vis_item[t] = true;
                        path.push(t);
                        break;
                    }
                }
            }
            if path.len() == len {
                path.pop();
            }
        }
        None
    }

    /// Largest batch (binary search bounded by the most numerous item of
    /// each ingredient).
    fn try_pick_all(&mut self, amounts: &mut Amounts, out: Option<&mut Vec<i32>>) -> i32 {
        let mut upper = i32::MAX;
        for ing in self.ings {
            let best = amounts.iter().filter(|(i, _)| accepts(ing, **i)).map(|(_, n)| *n).max().unwrap_or(0);
            upper = upper.min(best);
            if upper == 0 {
                break;
            }
        }
        let (mut min, mut max) = (0, upper.saturating_add(1));
        loop {
            let mid = (min + max) / 2;
            if self.try_pick(amounts, mid, None) {
                if max - min <= 1 {
                    if mid > 0 {
                        self.try_pick(amounts, mid, out);
                    }
                    return mid;
                }
                min = mid;
            } else {
                max = mid;
            }
        }
    }
}

/// Is the recipe doable `amount` times? Items retained per ingredient.
fn can_craft(ings: &[Ingredient], amounts: &Amounts, amount: i32, out: Option<&mut Vec<i32>>) -> bool {
    if ings.is_empty() {
        return false;
    }
    let mut a = amounts.clone();
    Picker::new(ings, &a).try_pick(&mut a, amount, out)
}

/// Slots the recipe goes to: for each filled slot of the grid,
/// the entry of next rank; narrow recipe centred (width < half the grid),
/// low recipe shifted down one row (height < half).
pub(crate) fn placement(gw: usize, gh: usize, rw: usize, rh: usize, entries: &[i32]) -> Vec<(i32, usize)> {
    let mut out = Vec::new();
    let mut it = entries.iter();
    let mut index = 0usize;
    let mut y = 0;
    while y < gh {
        let center_v = (rh as f32) < gh as f32 / 2.0;
        let start_v = (gh as f32 / 2.0 - rh as f32 / 2.0).floor() as i32;
        if center_v && start_v > y as i32 {
            index += gw;
            y += 1;
        }
        let mut x = 0;
        while x < gw {
            if it.len() == 0 {
                return out;
            }
            let center_h = (rw as f32) < gw as f32 / 2.0;
            let start_h = (gw as f32 / 2.0 - rw as f32 / 2.0).floor() as usize;
            let mut total = rw;
            let mut add = x < rw;
            if center_h {
                total = start_h + rw;
                add = start_h <= x && x < start_h + rw;
            }
            if add {
                let Some(&e) = it.next() else { return out };
                out.push((e, index));
            } else if total == x {
                index += gw - x;
                break;
            }
            index += 1;
            x += 1;
        }
        y += 1;
    }
    out
}

/// Recipe to place, as seen by the placement.
pub(crate) struct Plan {
    /// Ingredients and input per recipe slot (-1: empty).
    pub ings: Vec<Ingredient>,
    pub entries: Vec<i32>,
    pub rw: usize,
    pub rh: usize,
    /// Grid slots (menu), side, slots to empty, slots counted in the available items
    /// (furnace: all, including fuel and result), "simple" counting.
    pub grid: Vec<usize>,
    pub side: usize,
    pub clear: Vec<usize>,
    pub counted: Vec<usize>,
    pub counted_simple: bool,
    /// The grid already matches the recipe.
    pub matches: bool,
    pub creative: bool,
}

/// Outcome of the placement.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Nothing,
    Ghost,
}

fn src_get<'a>(m: &'a Menu, inv: &'a [Option<Stack>], i: usize) -> Option<&'a Stack> {
    m.get(inv, i)
}

fn src_set(m: &mut Menu, inv: &mut [Option<Stack>], i: usize, s: Option<Stack>) {
    match m.slots[i].src {
        Src::Inv(k) => inv[k] = s,
        Src::Own(k) => {
            if let Some(c) = m.own.get_mut(k) {
                *c = s;
            }
        }
    }
}

/// Main inventory slots (menu 0), hotbar first (search for a matching slot).
const MAIN: [usize; 36] = {
    let mut a = [0usize; 36];
    let mut i = 0;
    while i < 36 {
        a[i] = if i < 9 { 36 + i } else { i };
        i += 1;
    }
    a
};

/// Grid that can be emptied without losing anything.
fn test_clear_grid(m: &Menu, inv: &[Option<Stack>], items: &Items, selected: usize, plan: &Plan) -> bool {
    let free = MAIN.iter().filter(|&&k| inv[k].is_none()).count();
    let mut extra: Vec<Stack> = Vec::new();
    for &i in &plan.grid {
        let Some(s) = src_get(m, inv, i).cloned() else { continue };
        let mut s = s;
        let has_room = crate::menus::slot_with_space(inv, items, selected, &s).is_some();
        if !has_room && extra.len() <= free {
            for e in extra.iter_mut() {
                let max = items.max_stack(e);
                if e.id == s.id && e.count != max && e.count + s.count <= max {
                    e.count += s.count;
                    s.count = 0;
                    break;
                }
            }
            if s.count > 0 {
                if extra.len() >= free {
                    return false;
                }
                extra.push(s);
            }
        } else if !has_room {
            return false;
        }
    }
    true
}

impl Menu {
    /// Empties the slots to empty into the inventory.
    fn book_clear(&mut self, env: &mut dyn Env, inv: &mut [Option<Stack>], plan: &Plan) {
        for &i in &plan.clear {
            if let Some(s) = src_get(self, inv, i).cloned() {
                src_set(self, inv, i, None);
                crate::menus::place_back(inv, env, s);
            }
        }
        for &i in &plan.clear {
            src_set(self, inv, i, None);
        }
    }

    /// Placing a recipe in the grid.
    pub(crate) fn book_place(
        &mut self,
        env: &mut dyn Env,
        inv: &mut [Option<Stack>],
        plan: &Plan,
        max: bool,
    ) -> Outcome {
        let items = env.items();
        if !plan.creative && !test_clear_grid(self, inv, items, env.selected(), plan) {
            return Outcome::Nothing;
        }
        let mut amounts = Amounts::new();
        let mut account = |s: &Stack, simple: bool| {
            if (!simple || usable(items, s))
                && let Some(id) = items.id(&s.id)
            {
                *amounts.entry(id).or_insert(0) += s.count;
            }
        };
        for &k in &MAIN {
            if let Some(s) = &inv[k] {
                account(s, true);
            }
        }
        for &i in &plan.counted {
            if let Some(s) = src_get(self, inv, i) {
                account(s, plan.counted_simple);
            }
        }
        if !can_craft(&plan.ings, &amounts, 1, None) {
            self.book_clear(env, inv, plan);
            return Outcome::Ghost;
        }
        let mut a = amounts.clone();
        let biggest = Picker::new(&plan.ings, &a).try_pick_all(&mut a, None);
        let items = env.items();
        if plan.matches {
            for &i in &plan.grid {
                if let Some(s) = src_get(self, inv, i)
                    && biggest.min(items.max_stack(s)) < s.count + 1
                {
                    return Outcome::Nothing;
                }
            }
        }
        let amount = if max {
            biggest
        } else if plan.matches {
            let smallest = plan.grid.iter().filter_map(|&i| src_get(self, inv, i)).map(|s| s.count).min();
            smallest.map_or(i32::MAX, |n| n + 1)
        } else {
            1
        };
        let mut used = Vec::new();
        if !can_craft(&plan.ings, &amounts, amount, Some(&mut used)) {
            return Outcome::Nothing;
        }
        // Cap: default maximum size of each retained item.
        let mut adjusted = amount;
        for &id in &used {
            let Some(name) = items.name(id) else { continue };
            let probe = Stack { id: name.to_owned(), count: 1, components: None };
            adjusted = adjusted.min(items.max_stack(&probe));
        }
        if adjusted != amount {
            used.clear();
            if !can_craft(&plan.ings, &amounts, adjusted, Some(&mut used)) {
                return Outcome::Nothing;
            }
        }
        self.book_clear(env, inv, plan);
        for (entry, cell) in placement(plan.side, plan.side, plan.rw, plan.rh, &plan.entries) {
            if entry < 0 {
                continue;
            }
            let Some(&item) = used.get(entry as usize) else { continue };
            let target = plan.grid[cell];
            let mut remaining = adjusted;
            while remaining > 0 {
                let items = env.items();
                let existing = src_get(self, inv, target).cloned();
                let found = MAIN.iter().copied().find(|&k| {
                    inv[k].as_ref().is_some_and(|s| {
                        items.id(&s.id) == Some(item)
                            && usable(items, s)
                            && existing.as_ref().is_none_or(|e| crate::items::same(e, s))
                    })
                });
                let Some(k) = found else { break };
                let Some(mut s) = inv[k].take() else { break };
                let n = remaining.min(s.count);
                let taken = Stack { count: n, ..s.clone() };
                s.count -= n;
                inv[k] = (s.count > 0).then_some(s);
                let new = match existing {
                    Some(mut e) => {
                        e.count += n;
                        e
                    }
                    None => taken,
                };
                src_set(self, inv, target, Some(new));
                remaining -= n;
            }
        }
        Outcome::Nothing
    }
}
