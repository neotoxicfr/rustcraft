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

/// Offers matched by the payment slots (§ 1.2): (active offer, offer whose result is shown). A
/// sold-out first match stays the active offer, without a result, when the swapped attempt finds
/// no offer still in stock.
fn lookup(
    t: &Trade,
    s0: Option<&ItemStack>,
    s1: Option<&ItemStack>,
    rules: &dyn Rules,
) -> (Option<usize>, Option<usize>) {
    let (a, b) = if s0.is_none() { (s1, None) } else { (s0, s1) };
    let Some(a) = a else { return (None, None) };
    let n = t.offers.len();
    let find = |a: &ItemStack, b: Option<&ItemStack>| -> Option<usize> {
        let h = t.selected;
        if h > 0 && h < n {
            return satisfied(&t.offers[h], a, b, rules).then_some(h);
        }
        (0..n).find(|&i| satisfied(&t.offers[i], a, b, rules))
    };
    let first = find(a, b);
    if first.is_some_and(|i| !out_of_stock(&t.offers[i])) {
        return (first, first);
    }
    match b.and_then(|b| find(b, Some(a))) {
        Some(j) if !out_of_stock(&t.offers[j]) => (Some(j), Some(j)),
        _ => (first, None),
    }
}

/// Predicted result (§ 1.2): offer found and not sold out.
pub fn predict(t: &Trade, s0: Option<&ItemStack>, s1: Option<&ItemStack>, rules: &dyn Rules) -> Option<usize> {
    lookup(t, s0, s1, rules).1
}

/// Result slot and predicted experience recomputed from the payment slots. With payment but no
/// offer received yet, the result slot is left as it is.
fn refresh(t: &mut Trade, slots: &mut [Option<ItemStack>], rules: &dyn Rules) {
    t.last = (slots[0].clone(), slots[1].clone());
    if (slots[0].is_some() || slots[1].is_some()) && t.offers.is_empty() {
        return;
    }
    match predict(t, slots[0].as_ref(), slots[1].as_ref(), rules) {
        Some(i) => {
            slots[2] = t.offers[i].result.clone();
            t.future_xp = t.offers[i].xp;
        }
        None => {
            slots[2] = None;
            t.future_xp = 0;
        }
    }
}

/// Result just taken out of slot 2 (§ 1.4): the active offer consumes its payment (in either slot
/// order) and counts one use, the merchant's experience grows by the offer's in any case, then the
/// result is predicted again from what is left. Without this the client never sees an offer sell
/// out before the screen is reopened (the server does not resend the offers after a trade).
pub(crate) fn take(t: &mut Trade, slots: &mut [Option<ItemStack>], rules: &dyn Rules) {
    if let (Some(i), _) = lookup(t, slots[0].as_ref(), slots[1].as_ref(), rules) {
        let o = &mut t.offers[i];
        if pay(o, slots, 0, 1, rules) || pay(o, slots, 1, 0, rules) {
            o.uses = o.uses.wrapping_add(1);
        }
        t.xp = t.xp.wrapping_add(o.xp);
    }
    refresh(t, slots, rules);
}

/// Payment of `o` with slot `a` as first cost and slot `b` as second; nothing taken when unmet.
fn pay(o: &Offer, slots: &mut [Option<ItemStack>], a: usize, b: usize, rules: &dyn Rules) -> bool {
    let Some(sa) = &slots[a] else { return false };
    if !satisfied(o, sa, slots[b].as_ref(), rules) {
        return false;
    }
    let shrink = |s: &mut Option<ItemStack>, n: i32| {
        if let Some(st) = s {
            st.count -= n;
            if st.count <= 0 {
                *s = None;
            }
        }
    };
    shrink(&mut slots[a], adjusted_a(o, rules).count);
    if let Some(c) = &o.cost_b {
        shrink(&mut slots[b], c.count);
    }
    true
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
        refresh(&mut c.extra.trade, &mut c.slots, &rules);
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
        // Scrollbar: held, the list follows the mouse once it moves (not at the press).
        if n > 7
            && m.0 > (gx + 94) as f32
            && m.0 < (gx + 100) as f32
            && m.1 > (gy + 18) as f32
            && m.1 <= (gy + 158) as f32
        {
            t.dragging = true;
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
        let mut m = Menu { kind: c.kind, slots: &mut c.slots, carried: &mut carried, rules: &rules, creative, trade: None };
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
        t.scroll = wheel_offset(t.scroll, n, dy);
        true
    }
}

/// First offer shown after a wheel motion: the offset minus the motion, truncated toward zero
/// (a small motion upwards still moves by one, a small one downwards does nothing).
fn wheel_offset(scroll: usize, offers: usize, dy: f64) -> usize {
    ((scroll as f64 - dy) as i32).clamp(0, offers as i32 - 7) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wheel over 10 offers (3 hidden): offset minus the motion, truncated toward zero.
    #[test]
    fn merchant_wheel_truncates_the_motion() {
        assert_eq!(wheel_offset(2, 10, 1.0), 1);
        assert_eq!(wheel_offset(2, 10, 0.3), 1, "small motion up: one offer");
        assert_eq!(wheel_offset(2, 10, -0.3), 2, "small motion down: nothing");
        assert_eq!(wheel_offset(1, 10, -2.0), 3, "two notches at once");
        assert_eq!(wheel_offset(3, 10, -1.0), 3, "at the end");
        assert_eq!(wheel_offset(0, 10, 0.5), 0);
    }

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

    /// Click on the merchant menu (payment 0-1, result 2, inventory 3..39); returns the carried stack.
    fn click(t: &mut Trade, slots: &mut [Option<ItemStack>], slot: i16, mode: i32) -> Option<ItemStack> {
        let mut carried = None;
        let kind = Kind::Special(Sp::Merchant);
        let mut m = Menu { kind, slots, carried: &mut carried, rules: &R, creative: false, trade: Some(t) };
        m.click(slot, 0, mode, &mut |_| None);
        carried
    }

    /// Taking the result counts a use, consumes the adjusted payment and raises the merchant's
    /// experience; the result comes back while the offer is in stock and disappears once sold out
    /// (the server never resends the offers after a trade).
    #[test]
    fn taking_the_result_counts_a_use_until_sold_out() {
        let mut t = Trade { offers: vec![offer(st(1, 3), None, 9)], xp: 10, ..Default::default() };
        t.offers[0].uses = 10;
        let mut slots = vec![None; 39];
        slots[0] = Some(st(1, 7));
        refresh(&mut t, &mut slots, &R);
        assert_eq!(slots[2], Some(st(9, 1)));
        assert_eq!(click(&mut t, &mut slots, 2, 0), Some(st(9, 1)));
        assert_eq!((t.offers[0].uses, t.xp), (11, 15));
        assert_eq!(slots[0], Some(st(1, 4)), "payment consumed");
        assert_eq!(slots[2], Some(st(9, 1)), "still in stock: result predicted again");
        // Shift-click: trades repeated while the result comes back, here once (12 uses = sold out).
        assert_eq!(click(&mut t, &mut slots, 2, 1), None);
        assert_eq!((t.offers[0].uses, t.xp), (12, 20));
        assert_eq!(slots[0], Some(st(1, 1)));
        assert_eq!(slots[2], None, "sold out: no result");
        assert_eq!(slots[38], Some(st(9, 1)), "result moved to the end of the inventory");
    }

    /// Payment placed in the swapped order (cost B in slot 0, cost A in slot 1): both consumed.
    #[test]
    fn taking_the_result_pays_in_either_slot_order() {
        let mut t = Trade { offers: vec![offer(st(1, 3), Some(st(2, 2)), 9)], ..Default::default() };
        let mut slots = vec![None; 39];
        slots[0] = Some(st(2, 2));
        slots[1] = Some(st(1, 5));
        refresh(&mut t, &mut slots, &R);
        assert_eq!(click(&mut t, &mut slots, 2, 0), Some(st(9, 1)));
        assert_eq!(t.offers[0].uses, 1);
        assert_eq!((slots[0].clone(), slots[1].clone(), slots[2].clone()), (None, Some(st(1, 2)), None));
    }
}
