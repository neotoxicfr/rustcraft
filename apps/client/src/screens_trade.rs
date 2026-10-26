//! Trading screen (`docs/specs/special-screens.md` § 1): offers, adjusted price, prediction
//! of the result, payment filling and `select_trade`, experience bar, scrolling.

use rc_proto_v777::item::ItemStack;
use rc_proto_v777::special::{MerchantOffers, Offer};

use crate::inventory::Container;
use crate::menu::{Kind, Menu, Rules};
use crate::menu_special::Sp;
use crate::special::{Ctx, TITLE_COLOR, inside};

/// Experience thresholds of the merchant levels.
const XP_LEVELS: [i32; 5] = [0, 10, 70, 150, 250];

#[derive(Default)]
pub struct Trade {
    pub offers: Vec<Offer>,
    pub level: i32,
    pub xp: i32,
    pub show_progress: bool,
    pub can_restock: bool,
    /// Offer chosen on screen (and menu selection index).
    pub selected: usize,
    pub scroll: usize,
    dragging: bool,
    /// Experience of the predicted offer.
    pub future_xp: i32,
    /// Payment slots at the last computation.
    last: (Option<ItemStack>, Option<ItemStack>),
}

impl Trade {
    pub fn set_offers(&mut self, o: MerchantOffers) {
        self.offers = o.offers;
        self.level = o.level;
        self.xp = o.xp;
        self.show_progress = o.show_progress;
        self.can_restock = o.can_restock;
        self.scroll = self.scroll.min(self.offers.len().saturating_sub(7));
    }
}

fn out_of_stock(o: &Offer) -> bool {
    o.uses >= o.max_uses
}

/// Adjusted price A (§ 1.1).
pub fn adjusted_a(o: &Offer, rules: &dyn Rules) -> ItemStack {
    let b = o.cost_a.count;
    let d = ((b as f32 * o.demand as f32 * o.price_multiplier).floor() as i32).max(0);
    let max = rules.max_stack(&o.cost_a);
    ItemStack { count: (b + d + o.special_diff).clamp(1, max.max(1)), ..o.cost_a.clone() }
}

/// The stack carries the cost item and its required components.
fn matches_cost(cost: &ItemStack, s: &ItemStack) -> bool {
    s.item == cost.item && cost.added.iter().all(|c| s.added.iter().any(|a| a.0 == c.0 && a.1 == c.1))
}

fn satisfied(o: &Offer, a: &ItemStack, b: Option<&ItemStack>, rules: &dyn Rules) -> bool {
    if !matches_cost(&o.cost_a, a) || a.count < adjusted_a(o, rules).count {
        return false;
    }
    match (&o.cost_b, b) {
        (None, None) => true,
        (None, Some(_)) | (Some(_), None) => false,
        (Some(c), Some(b)) => matches_cost(c, b) && b.count >= c.count,
    }
}

/// Predicted result (§ 1.2): offer found and not sold out.
pub fn predict(t: &Trade, s0: Option<&ItemStack>, s1: Option<&ItemStack>, rules: &dyn Rules) -> Option<usize> {
    let (a, b) = if s0.is_none() { (s1, None) } else { (s0, s1) };
    let a = a?;
    let n = t.offers.len();
    let find = |a: &ItemStack, b: Option<&ItemStack>| -> Option<usize> {
        let h = t.selected;
        if h > 0 && h < n {
            return satisfied(&t.offers[h], a, b, rules).then_some(h);
        }
        (0..n).find(|&i| satisfied(&t.offers[i], a, b, rules))
    };
    let first = find(a, b).filter(|&i| !out_of_stock(&t.offers[i]));
    first.or_else(|| b.and_then(|b| find(b, Some(a))).filter(|&i| !out_of_stock(&t.offers[i])))
}

/// Background, titles, offer buttons, list, scrolling, experience bar.
pub fn draw_back(cx: &mut Ctx<'_, '_>, c: &Container) {
    let (gx, gy) = (cx.gx, cx.gy);
    let f = |v: i32| v as f32;
    let t = &c.extra.trade;
    let g = &mut *cx.gui;
    g.sprite_region("container_bg/villager", 512.0, 256.0, 0.0, 0.0, f(gx), f(gy), 276.0, 166.0);
    let n = t.offers.len();
    // Chosen offer sold out.
    if let Some(o) = t.offers.get(t.selected)
        && out_of_stock(o)
    {
        g.sprite("container/villager/out_of_stock", f(gx + 182), f(gy + 35), 28.0, 21.0, 0xffff_ffff);
    }
    // Experience bar.
    if t.show_progress && (1..5).contains(&t.level) {
        g.sprite("container/villager/experience_bar_background", f(gx + 136), f(gy + 16), 102.0, 5.0, 0xffff_ffff);
        let lv = t.level as usize;
        let (min, max) = (XP_LEVELS[lv - 1], XP_LEVELS[lv]);
        let m = 102.0 / (max - min) as f32;
        let mut w = 0;
        if t.xp >= min {
            w = ((m * (t.xp - min) as f32).floor() as i32).min(102);
            if w > 0 {
                let s = "container/villager/experience_bar_current";
                g.sprite_region(s, 102.0, 5.0, 0.0, 0.0, f(gx + 136), f(gy + 16), f(w), 5.0);
            }
        }
        if t.future_xp > 0 {
            let wf = ((t.future_xp as f32 * m).floor() as i32).min(102 - w);
            if wf > 0 {
                let s = "container/villager/experience_bar_result";
                g.sprite_region(s, 102.0, 5.0, f(w), 0.0, f(gx + 136 + w), f(gy + 16), f(wf), 5.0);
            }
        }
    }
    // Titles.
    let title = c.title.clone().unwrap_or_else(|| rc_text::Component::text(""));
    let title = if (1..=5).contains(&t.level) && t.show_progress {
        rc_text::Component::translate(
            "merchant.title",
            vec![
                rc_text::Arg::Component(title),
                rc_text::Arg::Component(rc_text::Component::translate(format!("merchant.level.{}", t.level), vec![])),
            ],
        )
    } else {
        title
    };
    let title = crate::special::chars(g, &title);
    let tw = rc_text::width(g, &title);
    g.text(cx.queue, &title, f(gx + 187 - tw / 2), f(gy + 6), TITLE_COLOR, false, 0);
    let trades = crate::special::tr(g, "merchant.trades");
    let ttw = rc_text::width(g, &trades);
    g.text(cx.queue, &trades, f(gx + 53 - ttw / 2), f(gy + 6), TITLE_COLOR, false, 0);
    let inv = crate::special::tr(g, "container.inventory");
    g.text(cx.queue, &inv, f(gx + 107), f(gy + 72), TITLE_COLOR, false, 0);
    // Offer buttons.
    for i in 0..7.min(n) as i32 {
        let (bx, by) = (gx + 5, gy + 18 + 20 * i);
        let hl = inside(cx.mouse, bx, by, 88, 20);
        let s = if hl { "widget/button_highlighted" } else { "widget/button" };
        g.sprite(s, f(bx), f(by), 88.0, 20.0, 0xffff_ffff);
    }
    // Scrolling.
    let steps = n as i32 - 6;
    if steps > 1 {
        let rest = 139 - (27 + (steps - 1) * 139 / steps);
        let step = 1 + rest / steps + 139 / steps;
        let mut dy = (t.scroll as i32 * step).min(113);
        if t.scroll as i32 == steps - 1 {
            dy = 113;
        }
        g.sprite("container/villager/scroller", f(gx + 94), f(gy + 18 + dy), 6.0, 27.0, 0xffff_ffff);
    } else {
        g.sprite("container/villager/scroller_disabled", f(gx + 94), f(gy + 18), 6.0, 27.0, 0xffff_ffff);
    }
    // List (over the buttons).
    let rules = crate::screens::ItemRules { infos: cx.it.infos, names: cx.it.names, ids: cx.it.ids, data: cx.it.data };
    let first = if n <= 7 { 0 } else { t.scroll };
    for (k, o) in t.offers.iter().enumerate().skip(first).take(7) {
        let y = gy + 19 + 20 * (k - first) as i32;
        let adj = adjusted_a(o, &rules);
        crate::special::item(cx.gui, cx.it, &adj, f(gx + 10), f(y));
        if adj.count == o.cost_a.count {
            crate::special::decorations(cx.gui, cx.queue, cx.it, &adj, f(gx + 10), f(y), false);
        } else {
            crate::special::decorations(cx.gui, cx.queue, cx.it, &o.cost_a, f(gx + 10), f(y), true);
            crate::special::decorations(cx.gui, cx.queue, cx.it, &adj, f(gx + 24), f(y), true);
            cx.gui.next_layer();
            let s = "container/villager/discount_strikethrough";
            cx.gui.sprite(s, f(gx + 17), f(y + 12), 9.0, 2.0, 0xffff_ffff);
        }
        if let Some(b) = &o.cost_b {
            crate::special::stack(cx.gui, cx.queue, cx.it, b, gx + 40, y);
        }
        let arrow = if out_of_stock(o) {
            "container/villager/trade_arrow_out_of_stock"
        } else {
            "container/villager/trade_arrow"
        };
        cx.gui.sprite(arrow, f(gx + 60), f(y + 3), 10.0, 9.0, 0xffff_ffff);
        if let Some(r) = &o.result {
            crate::special::stack(cx.gui, cx.queue, cx.it, r, gx + 73, y);
        }
    }
    cx.gui.next_layer();
}

/// Tooltips of the offer buttons and of the sold-out offer.
pub fn draw_front(cx: &mut Ctx<'_, '_>, c: &Container) {
    let Some(m) = cx.mouse else { return };
    if cx.inv.carried.is_some() {
        return;
    }
    let (gx, gy) = (cx.gx, cx.gy);
    let t = &c.extra.trade;
    let n = t.offers.len();
    let first = if n <= 7 { 0 } else { t.scroll };
    let rules = crate::screens::ItemRules { infos: cx.it.infos, names: cx.it.names, ids: cx.it.ids, data: cx.it.data };
    for i in 0..7.min(n) {
        let (bx, by) = (gx + 5, gy + 18 + 20 * i as i32);
        if !inside(Some(m), bx, by, 88, 20) {
            continue;
        }
        let Some(o) = t.offers.get(first + i) else { continue };
        let x = m.0;
        let st = if x < (bx + 20) as f32 {
            Some(adjusted_a(o, &rules))
        } else if x > (bx + 30) as f32 && x < (bx + 50) as f32 {
            o.cost_b.clone()
        } else if x > (bx + 65) as f32 {
            o.result.clone()
        } else {
            None
        };
        if let Some(st) = st {
            crate::special::item_tooltip(cx.gui, cx.queue, cx.it, cx.enchantments, &st, m);
        }
        return;
    }
    if t.can_restock && t.offers.get(t.selected).is_some_and(out_of_stock) && inside(Some(m), gx + 186, gy + 35, 22, 21)
    {
        // A single line, never cut (plain text, no component tooltip with width).
        let line = crate::special::tr(cx.gui, "merchant.deprecated");
        crate::special::text_tooltip(cx.gui, cx.queue, &[line], m);
    }
}

impl crate::App {
    fn trade_origin(&self) -> Option<(i32, i32)> {
        let r = self.renderer.as_ref()?;
        Some(((r.gui.width as i32 - 276) / 2, (r.gui.height as i32 - 166) / 2))
    }

    /// Recompute of the predicted result from the payment slots.
    pub(crate) fn trade_predict(&mut self) {
        let rules = crate::screens::ItemRules {
            infos: &self.data.items,
            names: &self.item_names,
            ids: &self.component_ids,
            data: &self.data,
        };
        let Some(c) = &mut self.inventory.container else { return };
        if c.kind != Kind::Special(Sp::Merchant) || c.slots.len() < 3 {
            return;
        }
        let t = &mut c.extra.trade;
        t.last = (c.slots[0].clone(), c.slots[1].clone());
        match predict(t, c.slots[0].as_ref(), c.slots[1].as_ref(), &rules) {
            Some(i) => {
                c.slots[2] = t.offers[i].result.clone();
                t.future_xp = t.offers[i].xp;
            }
            None => {
                c.slots[2] = None;
                t.future_xp = 0;
            }
        }
    }

    pub(crate) fn trade_tick(&mut self) {
        let changed = self
            .inventory
            .container
            .as_ref()
            .is_some_and(|c| c.slots.len() >= 2 && (c.slots[0].clone(), c.slots[1].clone()) != c.extra.trade.last);
        if changed {
            self.trade_predict();
        }
    }

    pub(crate) fn trade_press(&mut self, m: (f32, f32), b: u8) -> bool {
        let Some((gx, gy)) = self.trade_origin() else { return false };
        let Some(c) = &mut self.inventory.container else { return false };
        let t = &mut c.extra.trade;
        let n = t.offers.len();
        if b != 0 {
            return false;
        }
        // Scrollbar.
        if n > 7
            && m.0 > (gx + 94) as f32
            && m.0 < (gx + 100) as f32
            && m.1 > (gy + 18) as f32
            && m.1 <= (gy + 158) as f32
        {
            t.dragging = true;
            self.trade_motion(m);
            return true;
        }
        for i in 0..7.min(n) {
            if inside(Some(m), gx + 5, gy + 18 + 20 * i as i32, 88, 20) {
                let first = if n <= 7 { 0 } else { t.scroll };
                let index = i + first;
                self.play_ui(crate::widgets::CLICK_SOUND, 1.0);
                self.trade_select(index);
                return true;
            }
        }
        false
    }

    /// Choice of an offer: selection, predicted payment filling, `select_trade` (§ 1.3).
    fn trade_select(&mut self, index: usize) {
        let creative = self.env.game_mode == 1;
        let rules = crate::screens::ItemRules {
            infos: &self.data.items,
            names: &self.item_names,
            ids: &self.component_ids,
            data: &self.data,
        };
        let Some(c) = &mut self.inventory.container else { return };
        c.extra.trade.selected = index;
        let offer = c.extra.trade.offers.get(index).cloned();
        let n = c.slots.len();
        let mut carried = None;
        let mut m = Menu { kind: c.kind, slots: &mut c.slots, carried: &mut carried, rules: &rules, creative };
        let mut emptied = true;
        for s in 0..2 {
            if let Some(mut st) = m.slots[s].clone() {
                m.move_to(&mut st, 3, n, true);
                m.slots[s] = (st.count > 0).then_some(st.clone());
                if st.count > 0 {
                    emptied = false;
                    break;
                }
            }
        }
        if emptied && let Some(o) = &offer {
            let a = adjusted_a(o, &rules);
            for (target, cost) in [(0usize, Some(a)), (1, o.cost_b.clone())] {
                let Some(cost) = cost else { continue };
                for j in 3..n {
                    let Some(s) = m.slots[j].clone() else { continue };
                    if !matches_cost(&cost, &s) {
                        continue;
                    }
                    if let Some(cur) = &m.slots[target]
                        && !crate::menu::same(cur, &s)
                    {
                        continue;
                    }
                    let have = m.slots[target].as_ref().map_or(0, |x| x.count);
                    let room = rules.max_stack(&s) - have;
                    if room <= 0 {
                        break;
                    }
                    let take = room.min(s.count);
                    m.slots[target] = Some(ItemStack { count: have + take, ..s.clone() });
                    m.slots[j] = (s.count > take).then(|| ItemStack { count: s.count - take, ..s });
                }
            }
        }
        self.inventory.sync_from_container();
        self.trade_predict();
        let link = &self.session.link;
        let _ = link.send_raw(link.ids.select_trade, |w| rc_proto_v777::special::write_select_trade(w, index as i32));
    }

    pub(crate) fn trade_release(&mut self, _m: (f32, f32), _b: u8) -> bool {
        let Some(c) = &mut self.inventory.container else { return false };
        std::mem::take(&mut c.extra.trade.dragging)
    }

    pub(crate) fn trade_motion(&mut self, m: (f32, f32)) {
        let Some((_, gy)) = self.trade_origin() else { return };
        let Some(c) = &mut self.inventory.container else { return };
        let t = &mut c.extra.trade;
        let n = t.offers.len();
        if !t.dragging || n <= 7 {
            return;
        }
        let s = (m.1 - (gy + 18) as f32 - 13.5) / 112.0;
        t.scroll = ((s * (n - 7) as f32 + 0.5).floor() as i32).clamp(0, (n - 7) as i32) as usize;
    }

    pub(crate) fn trade_wheel(&mut self, _m: (f32, f32), dy: f64) -> bool {
        let Some(c) = &mut self.inventory.container else { return false };
        let t = &mut c.extra.trade;
        let n = t.offers.len();
        if n <= 7 {
            return false;
        }
        let d = dy.signum() as i32;
        t.scroll = (t.scroll as i32 - d).clamp(0, (n - 7) as i32) as usize;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct R;
    impl Rules for R {
        fn max_stack(&self, _: &ItemStack) -> i32 {
            64
        }
        fn equip_slot(&self, _: &ItemStack) -> u8 {
            0
        }
        fn is_fuel(&self, _: &ItemStack) -> bool {
            false
        }
        fn smeltable(&self, _: &ItemStack) -> bool {
            false
        }
        fn is_potion(&self, _: &ItemStack) -> bool {
            false
        }
        fn brew_fuel(&self, _: &ItemStack) -> bool {
            false
        }
        fn brew_ingredient(&self, _: &ItemStack) -> bool {
            false
        }
    }

    fn st(item: i32, count: i32) -> ItemStack {
        ItemStack { item, count, added: vec![], removed: vec![] }
    }

    fn offer(a: ItemStack, b: Option<ItemStack>, out: i32) -> Offer {
        Offer {
            cost_a: a,
            result: Some(st(out, 1)),
            cost_b: b,
            out_of_stock: false,
            uses: 0,
            max_uses: 12,
            xp: 5,
            special_diff: 0,
            price_multiplier: 0.05,
            demand: 0,
        }
    }

    #[test]
    fn adjusted_price_demand_and_discount() {
        let mut o = offer(st(1, 5), None, 9);
        o.demand = 10;
        assert_eq!(adjusted_a(&o, &R).count, 7);
        o.demand = -4;
        o.special_diff = -6;
        assert_eq!(adjusted_a(&o, &R).count, 1, "clamped to 1");
    }

    #[test]
    fn prediction_tries_swapped_inputs_and_skips_out_of_stock() {
        let mut t =
            Trade { offers: vec![offer(st(1, 3), Some(st(2, 1)), 9), offer(st(3, 1), None, 8)], ..Default::default() };
        // B in slot 0, A in slot 1: reversed attempt.
        assert_eq!(predict(&t, Some(&st(2, 1)), Some(&st(1, 4)), &R), Some(0));
        // Slot 0 empty: A = slot 1.
        assert_eq!(predict(&t, None, Some(&st(3, 1)), &R), Some(1));
        t.offers[1].uses = 12;
        assert_eq!(predict(&t, None, Some(&st(3, 1)), &R), None);
        // Insufficient quantity.
        assert_eq!(predict(&t, Some(&st(1, 2)), Some(&st(2, 1)), &R), None);
    }
}
