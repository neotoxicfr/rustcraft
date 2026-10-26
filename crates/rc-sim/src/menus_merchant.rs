//! Trading menu (F05): two payment slots, one result slot, player inventory
//! (the trade result slot). The offers are a copy of
//! the merchant's, copied back to it after each action (uses, trades made).

use super::{Env, Kind, Menu, Role, SlotDef, Src, place_back};
use crate::mobs::trades::{Offer, recipe_for};
use crate::player::Stack;

/// Trading state carried by the menu.
#[derive(Debug, Clone, Default)]
pub struct Trade {
    /// Merchant mob.
    pub mob: i32,
    pub offers: Vec<Offer>,
    pub hint: i32,
    pub active: Option<usize>,
    pub future_xp: i32,
    /// Offers traded since the last copy-back (in order).
    pub traded: Vec<usize>,
    /// Result updates: non-empty or empty result.
    pub updated: Vec<bool>,
    /// Trade sound played by quick move.
    pub trade_sound: bool,
}

impl Menu {
    /// Trading menu: payment 0 and 1, result 2, then bag and hotbar.
    pub fn merchant(window: i32, trade: Trade) -> Menu {
        let mut slots = vec![
            SlotDef { src: Src::Own(0), role: Role::Plain },
            SlotDef { src: Src::Own(1), role: Role::Plain },
            SlotDef { src: Src::Own(2), role: Role::MerchantResult },
        ];
        Self::player_slots(&mut slots);
        let mut m = Menu::base(window, Kind::Merchant, slots, 3, 0);
        m.merchant = Some(Box::new(trade));
        m
    }

    fn payments(&self) -> (Option<Stack>, Option<Stack>) {
        (self.own.first().cloned().flatten(), self.own.get(1).cloned().flatten())
    }

    /// Active offer from the payments (A then B, otherwise B then A).
    pub fn update_sell_item(&mut self, env: &dyn Env) {
        let Some(t) = self.merchant.as_mut() else { return };
        t.active = None;
        let (p0, p1) = (self.own[0].clone(), self.own[1].clone());
        let (a, b) = if p0.is_none() { (p1, None) } else { (p0, p1) };
        if a.is_none() {
            self.own[2] = None;
            t.future_xp = 0;
            self.own_dirty = true;
            return;
        }
        if !t.offers.is_empty() {
            let items = env.items();
            let mut off = recipe_for(items, &t.offers, a.as_ref(), b.as_ref(), t.hint);
            if off.is_none_or(|i| t.offers[i].out_of_stock()) {
                t.active = off;
                off = recipe_for(items, &t.offers, b.as_ref(), a.as_ref(), t.hint);
            }
            match off.filter(|&i| !t.offers[i].out_of_stock()) {
                Some(i) => {
                    t.active = Some(i);
                    self.own[2] = Some(t.offers[i].result.clone());
                    t.future_xp = t.offers[i].xp;
                }
                None => {
                    self.own[2] = None;
                    t.future_xp = 0;
                }
            }
        }
        t.updated.push(self.own[2].is_some());
        self.own_dirty = true;
    }

    /// The removal condition of a trade result.
    pub(super) fn merchant_may_pickup(&self, env: &dyn Env) -> bool {
        let Some(t) = &self.merchant else { return false };
        let Some(i) = t.active else { return false };
        let (a, b) = self.payments();
        let o = &t.offers[i];
        o.satisfied_by(env.items(), a.as_ref(), b.as_ref()) || o.satisfied_by(env.items(), b.as_ref(), a.as_ref())
    }

    /// Payment removed, trade noted, result recomputed.
    pub(super) fn merchant_on_take(&mut self, env: &dyn Env) {
        let Some(t) = self.merchant.as_mut() else { return };
        let Some(i) = t.active else { return };
        let (mut a, mut b) = (self.own[0].take(), self.own[1].take());
        let items = env.items();
        let o = t.offers[i].clone();
        if o.take(items, &mut a, &mut b) || o.take(items, &mut b, &mut a) {
            t.offers[i].uses += 1;
            t.traded.push(i);
        }
        self.own[0] = a;
        self.own[1] = b;
        self.own_dirty = true;
        self.update_sell_item(env);
    }

    /// The selection index then the item move (`select_trade`).
    pub fn select_trade(&mut self, env: &mut dyn Env, inv: &mut [Option<Stack>], index: i32) {
        let Some(t) = self.merchant.as_mut() else { return };
        t.hint = index;
        self.update_sell_item(env);
        let n = self.merchant.as_ref().map_or(0, |t| t.offers.len());
        if index < 0 || index as usize >= n {
            return;
        }
        for slot in 0..2 {
            if let Some(mut s) = self.own[slot].clone() {
                if !self.move_to(env, inv, &mut s, 3, 39, true) {
                    return;
                }
                self.own[slot] = (s.count > 0).then_some(s);
                self.own_dirty = true;
                self.update_sell_item(env);
            }
        }
        if self.own[0].is_none() && self.own[1].is_none() {
            let o = self.merchant.as_ref().map(|t| t.offers[index as usize].clone()).expect("offer");
            self.move_from_inventory(env, inv, 0, &o.cost_a);
            if let Some(b) = &o.cost_b {
                self.move_from_inventory(env, inv, 1, b);
            }
        }
    }

    /// The transfer from the inventory to the payment slot.
    fn move_from_inventory(
        &mut self,
        env: &dyn Env,
        inv: &mut [Option<Stack>],
        pay: usize,
        cost: &crate::mobs::trades::Cost,
    ) {
        let items = env.items();
        for i in 3..39 {
            let Some(it) = self.get(inv, i).cloned() else { continue };
            if !cost.test(Some(&it)) {
                continue;
            }
            let cur = self.own[pay].clone();
            if cur.as_ref().is_none_or(|c| crate::items::same(&it, c)) {
                let max = items.max_stack(&it);
                let have = cur.as_ref().map_or(0, |c| c.count);
                let mv = (max - have).min(it.count);
                let total = have + mv;
                self.set(inv, i, Some(Stack { count: it.count - mv, ..it.clone() }));
                self.own[pay] = Some(Stack { count: total, ..it });
                self.own_dirty = true;
                self.update_sell_item(env);
                if total >= max {
                    break;
                }
            }
        }
    }

    /// Closing: payments returned to the inventory.
    pub(super) fn merchant_close(&mut self, env: &mut dyn Env, inv: &mut [Option<Stack>]) {
        for slot in 0..2 {
            if let Some(s) = self.own[slot].take() {
                place_back(inv, env, s);
            }
        }
        self.own[2] = None;
        self.own_dirty = true;
    }
}
