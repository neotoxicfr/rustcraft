//! W8c: bundle (`bundle_contents`) in menus — click on a slot with a bundle carried by the
//! cursor or placed in the slot (insertion, removal of a stack), weight in exact fractions,
//! emptying by use in hand. Rules of game 26.3, in our own words.

use rc_proto::nbt::Nbt;

use super::{Env, Menu};
use crate::items::{Items, component, remove_component, same, set_component};
use crate::player::Stack;

/// Positive exact fraction (weight of a bundle: 1 = full).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Frac {
    n: i128,
    d: i128,
}

fn gcd(mut a: i128, mut b: i128) -> i128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.abs().max(1)
}

impl Frac {
    pub(crate) const ZERO: Frac = Frac { n: 0, d: 1 };
    pub(crate) const ONE: Frac = Frac { n: 1, d: 1 };

    pub(crate) fn new(n: i128, d: i128) -> Frac {
        let g = gcd(n, d);
        Frac { n: n / g, d: d / g }
    }
    fn add(self, o: Frac) -> Option<Frac> {
        Some(Frac::new(self.n.checked_mul(o.d)?.checked_add(o.n.checked_mul(self.d)?)?, self.d.checked_mul(o.d)?))
    }
    fn sub(self, o: Frac) -> Frac {
        Frac::new(self.n * o.d - o.n * self.d, self.d * o.d)
    }
    fn times(self, k: i32) -> Option<Frac> {
        Some(Frac::new(self.n.checked_mul(i128::from(k))?, self.d))
    }
    /// Integer part of `self / o` (division of one fraction by another, then integer part).
    fn div_floor(self, o: Frac) -> i128 {
        (self.n * o.d) / (self.d * o.n)
    }
}

/// Contents of a bundle: stacks of the component (set on the stack or by default on the item).
pub(crate) fn contents(items: &Items, s: &Stack) -> Option<Vec<Stack>> {
    match component(s, "minecraft:bundle_contents") {
        Some(Nbt::List(l)) => Some(l.iter().filter_map(Stack::from_nbt).collect()),
        Some(_) => Some(Vec::new()),
        None => items.default_component(s, "minecraft:bundle_contents").map(|_| Vec::new()),
    }
}

/// Weight of a unit stack: nested bundle = its weight + 1/16; occupied hive =
/// 1; otherwise 1 / maximum size. `None`: impossible weight (overflow).
pub(crate) fn weight(items: &Items, s: &Stack) -> Option<Frac> {
    if let Some(inner) = contents(items, s) {
        return contents_weight(items, &inner)?.add(Frac::new(1, 16));
    }
    if matches!(component(s, "minecraft:bees"), Some(Nbt::List(l)) if !l.is_empty()) {
        return Some(Frac::ONE);
    }
    Some(Frac::new(1, i128::from(items.max_stack(s))))
}

fn contents_weight(items: &Items, list: &[Stack]) -> Option<Frac> {
    let mut w = Frac::ZERO;
    for s in list {
        w = w.add(weight(items, s)?.times(s.count)?)?;
    }
    Some(w)
}

/// Can go into a bundle: everything except shulker boxes.
fn fits(s: &Stack) -> bool {
    s.count > 0 && !s.id.ends_with("shulker_box")
}

/// Stackable: size > 1 and not damaged.
fn stackable(items: &Items, s: &Stack) -> bool {
    items.max_stack(s) > 1 && items.damage(s) == 0
}

/// Editable content.
pub(crate) struct Bag {
    pub items: Vec<Stack>,
    weight: Frac,
}

impl Bag {
    /// An impossible weight gives an empty bundle.
    pub(crate) fn of(items: &Items, list: Vec<Stack>) -> Bag {
        match contents_weight(items, &list) {
            Some(weight) => Bag { items: list, weight },
            None => Bag { items: Vec::new(), weight: Frac::ZERO },
        }
    }

    fn max_to_add(&self, w: Frac) -> i32 {
        Frac::ONE.sub(self.weight).div_floor(w).clamp(0, i128::from(i32::MAX)) as i32
    }

    /// Stack `s` loses what goes in; number inserted (at the head of the list).
    pub(crate) fn try_insert(&mut self, items: &Items, s: &mut Stack) -> i32 {
        if !fits(s) {
            return 0;
        }
        let Some(w) = weight(items, s) else { return 0 };
        let n = s.count.min(self.max_to_add(w));
        if n == 0 {
            return 0;
        }
        let Some(nw) = w.times(n).and_then(|x| self.weight.add(x)) else { return 0 };
        self.weight = nw;
        let found = if stackable(items, s) { self.items.iter().position(|t| same(t, s)) } else { None };
        match found {
            Some(k) => {
                let mut merged = self.items.remove(k);
                merged.count += n;
                self.items.insert(0, merged);
            }
            None => self.items.insert(0, Stack { count: n, ..s.clone() }),
        }
        s.count -= n;
        n
    }

    /// The stack chosen with the wheel if the index is valid, otherwise the first.
    pub(crate) fn remove_one(&mut self, items: &Items, selected: i32) -> Option<Stack> {
        if self.items.is_empty() {
            return None;
        }
        let at = usize::try_from(selected).ok().filter(|&k| k < self.items.len()).unwrap_or(0);
        let s = self.items.remove(at);
        if let Some(w) = weight(items, &s).and_then(|w| w.times(s.count)) {
            self.weight = self.weight.sub(w);
        }
        Some(s)
    }

    /// Component rewritten on the bundle stack (removed if it becomes the default again).
    pub(crate) fn store(&self, items: &Items, bag: &mut Stack) {
        if self.items.is_empty() && items.default_component(bag, "minecraft:bundle_contents").is_some() {
            remove_component(bag, "minecraft:bundle_contents");
        } else {
            set_component(
                bag,
                "minecraft:bundle_contents",
                Nbt::List(self.items.iter().map(|s| s.to_nbt(None)).collect()),
            );
        }
    }
}

impl Menu {
    /// `bundle_item_selected` (the choice of the bundle item then the toggle of the selected item): same
    /// index or index outside the bundle = deselection.
    pub(crate) fn select_bundle(&mut self, items: &Items, inv: &[Option<Stack>], slot: i32, index: i32) {
        let Ok(slot) = usize::try_from(slot) else { return };
        if slot >= self.len() {
            return;
        }
        let Some(n) = self.get(inv, slot).and_then(|s| contents(items, s)).map(|l| l.len()) else { return };
        let cur = self.bundle_sel.filter(|&(k, _)| k == slot).map_or(-1, |(_, k)| k);
        let valid = index >= 0 && (index as usize) < n;
        self.bundle_sel = (cur != index && valid).then_some((slot, index));
    }

    /// Simple click (mode 0) diverted by a bundle: bundle carried
    /// onto a slot, then slot containing a bundle
    /// (the replacement of stacking onto itself). True if the click is consumed.
    pub(crate) fn bundle_click(&mut self, env: &mut dyn Env, inv: &mut [Option<Stack>], i: usize, button: i32) -> bool {
        let primary = button == 0;
        let clicked = self.get(inv, i).cloned();
        if let Some(mut bag_stack) = self.carried.clone()
            && let Some(list) = contents(env.items(), &bag_stack)
        {
            let mut bag = Bag::of(env.items(), list);
            if primary && let Some(other) = &clicked {
                // Take limited to what fits, if the item can go in.
                if let Some(w) = weight(env.items(), other)
                    && fits(other)
                {
                    let max = bag.max_to_add(w);
                    if let Some(mut taken) = self.safe_take(env, inv, i, other.count, max) {
                        bag.try_insert(env.items(), &mut taken);
                    }
                }
                bag.store(env.items(), &mut bag_stack);
                self.carried = Some(bag_stack);
                return true;
            }
            if !primary && clicked.is_none() {
                // The carried bundle has no menu slot: no selection (index -1).
                if let Some(s) = bag.remove_one(env.items(), -1) {
                    let n = s.count;
                    if let Some(mut rest) = self.safe_insert(env, inv, i, s, n) {
                        bag.try_insert(env.items(), &mut rest);
                    }
                }
                bag.store(env.items(), &mut bag_stack);
                self.carried = Some(bag_stack);
                return true;
            }
        }
        let Some(mut bag_stack) = clicked else { return false };
        let Some(list) = contents(env.items(), &bag_stack) else { return false };
        let carried = self.carried.clone();
        let allow = self.may_pickup(env, inv, i) && self.may_place(env, i, &bag_stack);
        let sel = self.bundle_sel.filter(|&(k, _)| k == i).map_or(-1, |(_, k)| k);
        let mut bag = Bag::of(env.items(), list);
        match (primary, carried) {
            (true, Some(mut c)) => {
                if allow {
                    bag.try_insert(env.items(), &mut c);
                }
                self.carried = (c.count > 0).then_some(c);
            }
            (false, None) => {
                if allow && let Some(s) = bag.remove_one(env.items(), sel) {
                    self.carried = Some(s);
                    self.bundle_sel = None; // Removing one copy resets the selection to -1
                }
            }
            _ => {
                self.bundle_sel = None; // The toggle of the selected item when the click is not taken
                return false;
            }
        }
        bag.store(env.items(), &mut bag_stack);
        *self.get_mut(inv, i) = Some(bag_stack);
        true
    }
}
