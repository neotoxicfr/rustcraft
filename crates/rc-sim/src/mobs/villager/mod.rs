//! Full villager and wandering trader (family F05, `docs/specs/server-mobs.md`
//! § 4.4): trader state (offers, 8-slot inventory, unhappiness), reputation
//! (gossip), food and breeding, restocking, sleep, schedule brain
//! (`brain.rs`, `behave.rs`), NBT reading and writing, opening of trading.

pub mod behave;
pub mod brain;
mod rpos;
pub mod trader;

#[cfg(test)]
mod tests;

use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use super::jrand::JRand;
use super::mob::{Ent, Mob};
use super::species::Kind;
use super::trades::Offer;
use crate::nbtio;
use crate::player::Stack;
use crate::server::Server;
use brain::Brain;

pub const PROF_NONE: i32 = 0;
pub const PROF_FARMER: i32 = 5;
pub const PROF_NITWIT: i32 = 11;

/// Experience thresholds to move to the next level (levels 1 to 4).
const XP_LEVELS: [i32; 5] = [0, 10, 70, 150, 250];

/// Gossip: types (weight, maximum, loss per day, loss on transfer).
pub const GOSSIP_TYPES: [(&str, i32, i32, i32, i32); 5] = [
    ("major_negative", -5, 100, 10, 10),
    ("minor_negative", -1, 200, 20, 20),
    ("minor_positive", 1, 25, 1, 5),
    ("major_positive", 5, 20, 0, 20),
    ("trading", 1, 25, 2, 20),
];
pub const G_MAJOR_NEG: usize = 0;
pub const G_MINOR_NEG: usize = 1;
pub const G_MINOR_POS: usize = 2;
pub const G_MAJOR_POS: usize = 3;
pub const G_TRADING: usize = 4;

/// Reputation: subject → values by type (0 = absent), arrival order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Gossips(pub Vec<(u128, [i32; 5])>);

impl Gossips {
    fn entry(&mut self, who: u128) -> &mut [i32; 5] {
        if let Some(i) = self.0.iter().position(|e| e.0 == who) {
            return &mut self.0[i].1;
        }
        self.0.push((who, [0; 5]));
        &mut self.0.last_mut().expect("entry").1
    }

    fn clean(&mut self) {
        self.0.retain(|e| e.1.iter().any(|&v| v != 0));
    }

    /// `add`: sum clamped to the maximum (otherwise keeps the old one if higher), < 2 removed.
    pub fn add(&mut self, who: u128, ty: usize, amount: i32) {
        let max = GOSSIP_TYPES[ty].2;
        let e = self.entry(who);
        let old = e[ty];
        let sum = old + amount;
        let mut v = if old == 0 {
            amount
        } else if sum > max {
            max.max(old)
        } else {
            sum
        };
        if v > max {
            v = max;
        }
        e[ty] = if v < 2 { 0 } else { v };
        self.clean();
    }

    pub fn reputation(&self, who: u128) -> i32 {
        self.0.iter().find(|e| e.0 == who).map_or(0, |e| (0..5).map(|t| e.1[t] * GOSSIP_TYPES[t].1).sum())
    }

    pub fn decay(&mut self) {
        for e in &mut self.0 {
            for (v, ty) in e.1.iter_mut().zip(GOSSIP_TYPES.iter()) {
                if *v != 0 {
                    let d = *v - ty.3;
                    *v = if d < 2 { 0 } else { d };
                }
            }
        }
        self.clean();
    }

    fn unpack(&self) -> Vec<(u128, usize, i32)> {
        let mut out = Vec::new();
        for e in &self.0 {
            for t in 0..5 {
                if e.1[t] != 0 {
                    out.push((e.0, t, e.1[t]));
                }
            }
        }
        out
    }

    /// 10 draws weighted by |value × weight| (no duplicates), loss on
    /// transfer, merge at the maximum; returns the number of gossips drawn.
    pub fn transfer_from(&mut self, src: &Gossips, rng: &mut JRand, max: usize) -> usize {
        let entries = src.unpack();
        if entries.is_empty() {
            return 0;
        }
        let mut ranges = Vec::with_capacity(entries.len());
        let mut end = 0;
        for e in &entries {
            end += (e.2 * GOSSIP_TYPES[e.1].1).abs();
            ranges.push(end - 1);
        }
        let mut picked: Vec<usize> = Vec::new();
        for _ in 0..max {
            let c = rng.next_int_bound(end);
            let i = match ranges.binary_search(&c) {
                Ok(i) => i,
                Err(i) => i,
            };
            if !picked.contains(&i) {
                picked.push(i);
            }
        }
        for &i in &picked {
            let (who, ty, v) = entries[i];
            let d = v - GOSSIP_TYPES[ty].4;
            if d >= 2 {
                let e = self.entry(who);
                e[ty] = e[ty].max(d);
            }
        }
        picked.len()
    }

    pub fn to_nbt(&self) -> Nbt {
        Nbt::List(
            self.unpack()
                .into_iter()
                .map(|(who, ty, v)| {
                    Nbt::Compound(vec![
                        ("Target".into(), nbtio::uuid_nbt(who)),
                        ("Type".into(), Nbt::String(GOSSIP_TYPES[ty].0.into())),
                        ("Value".into(), Nbt::Int(v)),
                    ])
                })
                .collect(),
        )
    }

    pub fn from_nbt(v: Option<&Nbt>) -> Gossips {
        let mut g = Gossips::default();
        for e in nbtio::list(v) {
            let Some(who) = nbtio::uuid(e.get("Target")) else { continue };
            let Some(ty) = e.get("Type").and_then(Nbt::as_str).and_then(|t| GOSSIP_TYPES.iter().position(|g| g.0 == t))
            else {
                continue;
            };
            let val = nbtio::int(e, "Value").unwrap_or(0) as i32;
            if val > 0 {
                g.entry(who)[ty] = val;
            }
        }
        g.clean();
        g
    }
}

/// State of a trader (villager or wandering trader).
#[derive(Debug, Clone, Default)]
pub struct Npc {
    /// Offers (generated at the first consultation).
    pub offers: Option<Vec<Offer>>,
    /// Player currently trading.
    pub trading: bool,
    pub inv: Vec<Option<Stack>>,
    pub unhappy: i32,
    // Villager.
    pub food: i32,
    pub gossips: Gossips,
    pub last_gossip: i64,
    pub last_gossip_decay: i64,
    pub last_restock: i64,
    pub restocks: i32,
    pub restock_day: i64,
    pub last_traded: bool,
    pub brain: Option<Box<Brain>>,
    /// Occupied bed (head).
    pub sleeping: Option<[i32; 3]>,
    pub was_baby: bool,
    // Wandering trader.
    pub despawn_delay: i32,
    pub wander: Option<[i32; 3]>,
    pub home: Option<([i32; 3], i32)>,
    /// "Drink" goal in progress (item in hand, ticks left).
    pub drinking: i32,
}

/// World state: wandering trader spawner.
#[derive(Debug, Clone)]
pub struct NpcWorld {
    pub tick_delay: i32,
    pub spawn_delay: i32,
    pub spawn_chance: i32,
    pub loaded: bool,
    pub rng: JRand,
}

impl Default for NpcWorld {
    fn default() -> NpcWorld {
        NpcWorld {
            tick_delay: 1200,
            spawn_delay: 24000,
            spawn_chance: 25,
            loaded: false,
            rng: JRand::from_time(0x5452_4144),
        }
    }
}

pub fn is_npc(k: Kind) -> bool {
    matches!(k, Kind::Villager | Kind::WanderingTrader)
}

/// Ambient sound of a trader: none when asleep (villager), "trade" during a trade.
pub fn ambient_sound(m: &Mob) -> Option<String> {
    let n = m.npc.as_ref();
    let base = if m.kind == Kind::Villager { "villager" } else { "wandering_trader" };
    if m.kind == Kind::Villager && n.is_some_and(|n| n.sleeping.is_some()) {
        return None;
    }
    let what = if n.is_some_and(|n| n.trading) { "trade" } else { "ambient" };
    Some(format!("minecraft:entity.{base}.{what}"))
}

impl Server {
    // ---------- Access ----------

    /// Another villager of the list (taken out for the duration of the call) with its brain.
    pub(crate) fn with_villager<R>(
        &mut self,
        id: i32,
        f: impl FnOnce(&mut Server, &mut Mob, Option<&mut Brain>) -> R,
    ) -> Option<R> {
        let i = self.mob_index(id)?;
        let mut o = self.mobs.list[i].take()?;
        let mut b = o.npc.as_mut().and_then(|n| n.brain.take());
        let r = f(self, &mut o, b.as_deref_mut());
        if let (Some(n), Some(b)) = (o.npc.as_mut(), b) {
            n.brain = Some(b);
        }
        if let Some(slot) = self.mobs.list.get_mut(i) {
            *slot = Some(o);
        } else {
            self.mobs.push(o);
        }
        Some(r)
    }

    pub(crate) fn npc_trading(&self, m: &Mob) -> bool {
        m.npc.as_ref().is_some_and(|n| n.trading)
    }

    pub(crate) fn npc_sleeping(&self, m: &Mob) -> Option<[i32; 3]> {
        m.npc.as_ref().and_then(|n| n.sleeping)
    }

    pub(crate) fn npc_count(&self, m: &Mob, item: &str) -> i32 {
        m.npc.as_ref().map_or(0, |n| n.inv.iter().flatten().filter(|s| s.id == item).map(|s| s.count).sum())
    }

    /// Removes up to `n` copies, from the start of the inventory.
    pub(crate) fn npc_remove(&self, m: &mut Mob, item: &str, mut n: i32) {
        let Some(npc) = m.npc.as_mut() else { return };
        for s in npc.inv.iter_mut() {
            if n <= 0 {
                break;
            }
            if let Some(st) = s
                && st.id == item
            {
                let k = st.count.min(n);
                st.count -= k;
                n -= k;
                if st.count <= 0 {
                    *s = None;
                }
            }
        }
    }

    /// Completes identical stacks then empty slots; remainder.
    pub(crate) fn npc_add(&self, m: &mut Mob, mut s: Stack) -> Option<Stack> {
        let max = self.life.items.max_stack(&s);
        let npc = m.npc.as_mut()?;
        for slot in npc.inv.iter_mut() {
            if s.count <= 0 {
                break;
            }
            if let Some(c) = slot
                && crate::items::same(c, &s)
            {
                let k = (max - c.count).min(s.count).max(0);
                c.count += k;
                s.count -= k;
            }
        }
        for slot in npc.inv.iter_mut() {
            if s.count <= 0 {
                break;
            }
            if slot.is_none() {
                *slot = Some(s.clone());
                s.count = 0;
            }
        }
        (s.count > 0).then_some(s)
    }

    fn npc_can_add(&self, m: &Mob, s: &Stack) -> bool {
        let max = self.life.items.max_stack(s);
        m.npc.as_ref().is_some_and(|n| {
            n.inv.iter().any(|c| match c {
                None => true,
                Some(c) => crate::items::same(c, s) && c.count < max,
            })
        })
    }

    /// Nutritional value for a villager (component `villager_food`).
    pub(crate) fn villager_food(&self, s: &Stack) -> Option<i32> {
        let v = self.life.items.default_component(s, "minecraft:villager_food")?;
        Some(v["nutrition"].as_i64().or_else(|| v.as_i64()).unwrap_or(0) as i32)
    }

    pub(crate) fn npc_food_points(&self, m: &Mob) -> i32 {
        m.npc
            .as_ref()
            .map_or(0, |n| n.inv.iter().flatten().map(|s| self.villager_food(s).map_or(0, |v| v * s.count)).sum())
    }

    pub(crate) fn npc_has_plantable(&self, m: &Mob) -> bool {
        m.npc.as_ref().is_some_and(|n| {
            n.inv.iter().flatten().any(|s| self.life.items.has_tag("minecraft:villager_plantable_seeds", &s.id))
        })
    }

    /// The villager's pickup wish.
    pub(crate) fn villager_wants(&self, m: &Mob, s: &Stack) -> bool {
        let items = &self.life.items;
        (items.has_tag("minecraft:villager_picks_up", &s.id)
            || self.villager_food(s).is_some()
            || behave::requested(m.villager[1]).contains(&s.id.as_str()))
            && self.npc_can_add(m, s)
    }

    /// Internal food + inventory ≥ 12, not asleep, age 0.
    pub(crate) fn can_breed(&self, m: &Mob) -> bool {
        let Some(n) = &m.npc else { return false };
        n.food + self.npc_food_points(m) >= 12 && n.sleeping.is_none() && m.age == 0
    }

    /// Hurt shake in progress (10 ticks after any full damage, whatever its source).
    pub(crate) fn hurt_recently(&self, m: &Mob) -> bool {
        m.hurt_time > 0
    }

    pub(crate) fn player_has_effect(&self, name: &str) -> bool {
        self.fx().has(name)
    }

    pub(crate) fn held_main_hand(&self) -> Option<Stack> {
        self.slots.get(36 + self.held as usize).cloned().flatten()
    }

    /// Item shown in hand (display as a held item / emptying the held item).
    pub(crate) fn display_held(&self, m: &mut Mob, s: Option<Stack>) {
        m.drop_chance[0] = if s.is_some() { 0.0 } else { 0.085 };
        if m.equipment[0] != s {
            m.equipment[0] = s;
            self.send_main_hand(m);
        }
    }

    /// Item in hand sent to the clients that see the mob.
    pub(crate) fn send_main_hand(&self, m: &Mob) {
        if m.track.seen {
            let net = self.net(m.equipment[0].as_ref());
            let slots: rc_proto_v777::entity::Equipment = vec![(0, net)];
            let _ = self.send(rc_client::Event::Entity(rc_client::EntityEvent::Equipment { id: m.id, slots }));
        }
    }

    pub(crate) fn send_level_event(&self, kind: i32, p: [i32; 3], data: i32) {
        let _ = self.send(rc_client::Event::LevelEvent { kind, pos: p, data, global: false });
    }

    // ---------- Villager data ----------

    /// The choice of villager data with profession change: offers forgotten.
    pub(crate) fn set_profession(&self, m: &mut Mob, prof: i32) {
        if m.villager[1] != prof
            && let Some(n) = m.npc.as_mut()
        {
            n.offers = None;
        }
        m.villager[1] = prof;
    }

    fn villager_type_name(m: &Mob) -> &'static str {
        super::variants::VILLAGER_TYPES.get(m.villager[0] as usize).copied().unwrap_or("plains")
    }

    /// Generated at the first consultation (current profession and level).
    pub(crate) fn npc_offers(&mut self, m: &mut Mob) -> Vec<Offer> {
        if m.npc.as_ref().is_some_and(|n| n.offers.is_none()) {
            let mut offers = Vec::new();
            self.update_trades(m, &mut offers);
            if let Some(n) = m.npc.as_mut() {
                n.offers = Some(offers);
            }
        }
        m.npc.as_ref().and_then(|n| n.offers.clone()).unwrap_or_default()
    }

    /// Trade set of the profession at the current level (or the trader's three sets).
    fn update_trades(&mut self, m: &mut Mob, offers: &mut Vec<Offer>) {
        let ty = Self::villager_type_name(m);
        if m.kind == Kind::WanderingTrader {
            for set in ["wandering_trader/buying", "wandering_trader/uncommon", "wandering_trader/common"] {
                self.add_trade_set(set, ty, offers);
            }
            return;
        }
        let prof = super::variants::PROFESSIONS.get(m.villager[1] as usize).copied().unwrap_or("none");
        if matches!(prof, "none" | "nitwit") {
            return;
        }
        let set = format!("{prof}/level_{}", m.villager[2]);
        self.add_trade_set(&set, ty, offers);
        self.resolve_offer_maps(offers, m.block_pos()); // S8: exploration maps
    }

    // ---------- Trading ----------

    /// Click on a villager or a trader; `None`: generic behavior.
    pub(crate) fn npc_interact(&mut self, m: &mut Mob, item: &str) -> Option<bool> {
        // Both traders let the VILLAGER spawn egg through (the wandering trader too).
        if item == "minecraft:villager_spawn_egg" || !m.alive() || self.npc_trading(m) || self.npc_sleeping(m).is_some()
        {
            return None;
        }
        if m.is_baby() {
            // Baby villager: unhappy; baby wandering trader: generic behavior.
            if m.kind != Kind::Villager {
                return None;
            }
            self.set_unhappy(m);
            return Some(true);
        }
        // Main-hand click on an adult: counted even without an offer.
        self.award_custom("talked_to_villager", 1);
        let offers = self.npc_offers(m);
        if offers.is_empty() {
            // Villager: unhappy; wandering trader: nothing.
            if m.kind == Kind::Villager {
                self.set_unhappy(m);
            }
            return Some(true);
        }
        if m.kind == Kind::Villager {
            self.update_special_prices(m);
        }
        self.start_trading(m);
        Some(true)
    }

    pub(crate) fn set_unhappy(&mut self, m: &mut Mob) {
        if let Some(n) = m.npc.as_mut() {
            n.unhappy = 40;
        }
        self.make_sound(m, "minecraft:entity.villager.no");
    }

    /// Difference reset to zero, reputation, hero of the village.
    fn update_special_prices(&mut self, m: &mut Mob) {
        let uuid = self.uuid;
        let Some(n) = m.npc.as_mut() else { return };
        let rep = n.gossips.reputation(uuid);
        let hero = self.fx().amp("minecraft:hero_of_the_village");
        let Some(offers) = n.offers.as_mut() else { return };
        for o in offers.iter_mut() {
            o.special = 0;
            if rep != 0 {
                o.special += -((rep as f32 * o.mult).floor() as i32);
            }
            if let Some(a) = hero {
                let modf = f64::from(0.3f32 + 0.0625f32 * a as f32);
                let red = (modf * f64::from(o.cost_a.count)).floor() as i32;
                o.special += -red.max(1);
            }
        }
        if n.trading {
            self.merchant_resend(m);
        }
    }

    /// Opens the trading screen and sends the offers.
    fn start_trading(&mut self, m: &mut Mob) {
        if let Some(n) = m.npc.as_mut() {
            n.trading = true;
        }
        let window = {
            self.life.counter = self.life.counter % 100 + 1;
            self.life.counter
        };
        let offers = m.npc.as_ref().and_then(|n| n.offers.clone()).unwrap_or_default();
        let trade = crate::menus::merchant::Trade { mob: m.id, offers, ..Default::default() };
        let menu = crate::menus::Menu::merchant(window, trade);
        let title = match &m.custom_name {
            Some(n) => rc_text::from_nbt(n),
            None if m.kind == Kind::Villager => {
                let prof = super::variants::PROFESSIONS.get(m.villager[1] as usize).copied().unwrap_or("none");
                Some(rc_text::Component::translate(format!("entity.minecraft.villager.{prof}"), Vec::new()))
            }
            None => Some(rc_text::Component::translate("entity.minecraft.wandering_trader", Vec::new())),
        };
        self.open_menu(menu, title);
        self.merchant_resend(m);
    }

    /// Sending the offers (non-empty offers only).
    pub(crate) fn merchant_resend(&mut self, m: &Mob) {
        let Some(n) = &m.npc else { return };
        let Some(offers) = &n.offers else { return };
        if offers.is_empty() {
            return;
        }
        let Some(window) = self
            .life
            .menu
            .as_ref()
            .filter(|mm| mm.merchant.as_ref().is_some_and(|t| t.mob == m.id))
            .map(|mm| mm.window)
        else {
            return;
        };
        if let Some(t) = self.life.menu.as_mut().and_then(|mm| mm.merchant.as_mut()) {
            t.offers.clone_from(offers);
        }
        let items = &self.life.items;
        let net_offers: Vec<rc_proto_v777::special::Offer> = offers
            .iter()
            .map(|o| {
                let cost = |c: &super::trades::Cost| {
                    let s = c.stack(c.count);
                    let mut i = self.net(Some(&s)).unwrap_or_else(|| rc_proto_v777::item::ItemStack {
                        item: items.id(&c.id).unwrap_or(0),
                        count: c.count,
                        added: Vec::new(),
                        removed: Vec::new(),
                    });
                    i.count = c.count;
                    i.removed.clear();
                    i
                };
                rc_proto_v777::special::Offer {
                    cost_a: cost(&o.cost_a),
                    result: self.net(Some(&o.result)),
                    cost_b: o.cost_b.as_ref().map(cost),
                    out_of_stock: o.out_of_stock(),
                    uses: o.uses,
                    max_uses: o.max_uses,
                    xp: o.xp,
                    special_diff: o.special,
                    price_multiplier: o.mult,
                    demand: o.demand,
                }
            })
            .collect();
        let villager = m.kind == Kind::Villager;
        let mo = rc_proto_v777::special::MerchantOffers {
            window,
            offers: net_offers,
            level: if villager { m.villager[2] } else { 1 },
            xp: if villager { m.x.villager_xp } else { 0 },
            show_progress: villager,
            can_restock: villager,
        };
        let _ = self.send(rc_client::Event::MerchantOffers(Box::new(mo)));
        if let Some(mm) = self.life.menu.as_mut() {
            let env_items = &self.life.items;
            let _ = env_items;
            mm.own_dirty = true;
        }
    }

    /// After an action on the trading menu: trades made applied to the trader.
    pub(crate) fn merchant_sync(&mut self) {
        let Some(t) = self.life.menu.as_mut().and_then(|mm| mm.merchant.as_mut()) else { return };
        let id = t.mob;
        let traded = std::mem::take(&mut t.traded);
        let updated = std::mem::take(&mut t.updated);
        let sound = std::mem::replace(&mut t.trade_sound, false);
        let offers = t.offers.clone();
        let Some(i) = self.mob_index(id) else { return };
        let Some(mut m) = self.mobs.list[i].take() else { return };
        for valid in updated {
            // Yes/no sound, at most once per interval.
            let interval = m.kind.info().ambient_interval;
            if m.ambient_time > -interval + 20 {
                m.ambient_time = -interval;
                let s = if valid { "minecraft:entity.villager.yes" } else { "minecraft:entity.villager.no" };
                let s = if m.kind == Kind::WanderingTrader {
                    s.replace("villager", "wandering_trader")
                } else {
                    s.to_owned()
                };
                self.make_sound(&mut m, &s);
            }
        }
        if sound {
            // Client-local sound: not sent by the server.
        }
        for k in traded {
            if let Some(n) = m.npc.as_mut()
                && let Some(off) = n.offers.as_mut().and_then(|o| o.get_mut(k))
            {
                // Use, ambient timer, reward.
                *off = offers[k].clone();
            }
            let interval = m.kind.info().ambient_interval;
            m.ambient_time = -interval;
            let off = offers[k].clone();
            self.reward_trade(&mut m, &off);
            self.s9_traded(&m, &off.result); // S9: statistic and trading trigger
        }
        self.mobs.list[i] = Some(m);
    }

    /// The experience reward of a trade (villager) or simple reward (wandering trader).
    fn reward_trade(&mut self, m: &mut Mob, off: &Offer) {
        let mut pop = 3 + m.rng.next_int_bound(4);
        if m.kind == Kind::Villager {
            m.x.villager_xp += off.xp;
            if let Some(n) = m.npc.as_mut() {
                n.last_traded = true;
            }
            let lvl = m.villager[2];
            if (1..5).contains(&lvl) && m.x.villager_xp >= XP_LEVELS[lvl as usize] {
                m.villager[2] = lvl + 1;
                let mut offers = m.npc.as_ref().and_then(|n| n.offers.clone()).unwrap_or_default();
                self.update_trades(m, &mut offers);
                if let Some(n) = m.npc.as_mut() {
                    n.offers = Some(offers);
                }
                self.update_special_prices(m);
                self.give_mob_effect(m, "minecraft:regeneration", 200, 0);
                pop += 5;
            }
        }
        if off.reward_exp {
            // A single orb of the drawn value (neither splitting nor merging).
            self.add_orb([m.pos[0], m.pos[1] + 0.5, m.pos[2]], pop);
        }
    }

    /// `select_trade` received.
    pub(crate) fn merchant_select(&mut self, index: i32) {
        let selected = self.held as usize;
        let creative = self.game_mode == 1;
        let _ = (selected, creative);
        let open = self.life.menu.as_ref().is_some_and(|m| m.merchant.is_some());
        if !open || !self.merchant_valid() {
            return;
        }
        self.with_menu(true, |m, inv, env| m.select_trade(env, inv, index));
        self.merchant_sync();
        self.broadcast_menus();
    }

    /// Menu still valid: trader alive, trading with the player, in range.
    pub(crate) fn merchant_valid(&self) -> bool {
        let Some(t) = self.life.menu.as_ref().and_then(|m| m.merchant.as_ref()) else { return true };
        // Interaction range (3) + 4 up to the box.
        self.mob_ref(t.mob).is_some_and(|o| {
            let b = o.bbox();
            let eye = [self.pos[0], self.player_eye_y(), self.pos[2]];
            let d2: f64 = (0..3).map(|a| (eye[a] - eye[a].clamp(b.min[a], b.max[a])).powi(2)).sum();
            o.alive() && self.npc_trading(o) && d2 < 7.0 * 7.0
        })
    }

    /// Trade closing: the trader no longer trades.
    pub(crate) fn merchant_closed(&mut self, id: i32) {
        if let Some(i) = self.mob_index(id)
            && let Some(m) = self.mobs.list[i].as_mut()
            && let Some(n) = m.npc.as_mut()
        {
            n.trading = false;
            if let Some(o) = &mut n.offers {
                for x in o.iter_mut() {
                    x.special = 0;
                }
            }
        }
    }

    // ---------- Reputation ----------

    /// Reputation event received by a villager.
    pub(crate) fn reputation_event(&mut self, m: &mut Mob, ev: &str, source: u128) {
        let Some(n) = m.npc.as_mut() else { return };
        match ev {
            "cured" => {
                n.gossips.add(source, G_MAJOR_POS, 20);
                n.gossips.add(source, G_MINOR_POS, 25);
            }
            "trade" => n.gossips.add(source, G_TRADING, 2),
            "hurt" => n.gossips.add(source, G_MINOR_NEG, 25),
            "killed" => n.gossips.add(source, G_MAJOR_NEG, 25),
            _ => {}
        }
        if source == self.uuid && self.npc_trading(m) {
            self.update_special_prices(m);
        }
    }

    fn npc_ent_uuid(&self, e: Ent) -> Option<u128> {
        match e {
            Ent::Player => Some(self.uuid),
            Ent::Mob(id) => self.mob_ref(id).map(|o| o.uuid),
        }
    }

    /// `gossip`: gossip exchange (once per 1200 ticks for each), golem attempt.
    pub(crate) fn gossip_with(&mut self, m: &mut Mob, other: i32, t: i64) {
        let ok = |last: i64| t < last || t >= last + 1200;
        let mine = m.npc.as_ref().map_or(0, |n| n.last_gossip);
        let theirs = self.mob_ref(other).and_then(|o| o.npc.as_ref()).map_or(0, |n| n.last_gossip);
        if !(ok(mine) && ok(theirs)) {
            return;
        }
        let src = self.mob_ref(other).and_then(|o| o.npc.as_ref()).map(|n| n.gossips.clone()).unwrap_or_default();
        let mut rng = m.rng;
        let got = m.npc.as_mut().map_or(0, |n| n.gossips.transfer_from(&src, &mut rng, 10));
        m.rng = rng;
        if let Some(n) = m.npc.as_mut() {
            n.last_gossip = t;
        }
        self.with_villager(other, |_, o, _| {
            if let Some(n) = o.npc.as_mut() {
                n.last_gossip = t;
            }
        });
        let mut b = m.npc.as_mut().and_then(|n| n.brain.take());
        if let Some(b) = b.as_deref_mut() {
            self.spawn_golem_if_needed(m, b, t, 5);
        }
        if let (Some(n), Some(b)) = (m.npc.as_mut(), b) {
            n.brain = Some(b);
        }
        if got > 0 && self.npc_trading(m) {
            self.update_special_prices(m);
        }
    }

    /// Villager wanting a golem within ±10; iron golem not simulated here
    /// (family F06), the attempt stops at the countdown.
    pub(crate) fn spawn_golem_if_needed(&mut self, m: &mut Mob, b: &mut Brain, t: i64, needed: usize) {
        let wants = |mem: &brain::Mem| mem.last_slept.is_some_and(|s| t - s < 24000) && mem.golem.is_none();
        if !wants(&b.mem) {
            return;
        }
        let bb = m.bbox();
        let mut count = 0;
        for o in self.mobs.list.iter().flatten() {
            if o.kind != Kind::Villager {
                continue;
            }
            let ob = o.bbox();
            let inside = ob.max[0] > bb.min[0] - 10.0
                && ob.min[0] < bb.max[0] + 10.0
                && ob.max[1] > bb.min[1] - 10.0
                && ob.min[1] < bb.max[1] + 10.0
                && ob.max[2] > bb.min[2] - 10.0
                && ob.min[2] < bb.max[2] + 10.0;
            if inside && o.npc.as_ref().and_then(|n| n.brain.as_ref()).is_some_and(|ob| wants(&ob.mem)) && count < 5 {
                count += 1;
            }
        }
        // The current villager is out of the list: it counts for itself.
        if count < 5 {
            count += 1;
        }
        if count >= needed && self.summon_golem_for_villager(m.block_pos()) {
            // Golem born (F06): all villagers in the enlarged box of 10 have "seen" it.
            b.mem.golem = Some(599);
            for o in self.mobs.list.iter_mut().flatten() {
                if o.kind != Kind::Villager {
                    continue;
                }
                let ob = o.bbox();
                let inside = ob.max[0] > bb.min[0] - 10.0
                    && ob.min[0] < bb.max[0] + 10.0
                    && ob.max[1] > bb.min[1] - 10.0
                    && ob.min[1] < bb.max[1] + 10.0
                    && ob.max[2] > bb.min[2] - 10.0
                    && ob.min[2] < bb.max[2] + 10.0;
                if inside && let Some(ob) = o.npc.as_mut().and_then(|n| n.brain.as_mut()) {
                    ob.mem.golem = Some(599);
                }
            }
        }
    }

    // ---------- Food and breeding ----------

    /// Eats up to 12 (inventory), then loses 12.
    pub(crate) fn eat_and_digest(&self, m: &mut Mob) {
        let foods: Vec<(usize, i32, i32)> = m
            .npc
            .as_ref()
            .map(|n| {
                n.inv
                    .iter()
                    .enumerate()
                    .filter_map(|(i, s)| {
                        let s = s.as_ref()?;
                        Some((i, self.villager_food(s)?, s.count))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let Some(n) = m.npc.as_mut() else { return };
        if n.food < 12 {
            'outer: for (slot, nut, count) in foods {
                let mut remove = 0;
                for _ in 0..count {
                    n.food += nut;
                    remove += 1;
                    if n.food >= 12 {
                        take(&mut n.inv[slot], remove);
                        break 'outer;
                    }
                }
                take(&mut n.inv[slot], remove);
            }
        }
        n.food -= 12;
    }

    /// Free reachable bed (ticket taken), otherwise anger.
    pub(crate) fn try_give_birth(&mut self, m: &mut Mob, other: i32) {
        let center = m.block_pos();
        let list = self.poi_in_range(|t| t == crate::poi::PoiType::Home, center, 48, crate::poi::Occupancy::HasSpace);
        let mut bed = None;
        for r in list {
            let ok = self.create_path(m, r.pos, r.ty.valid_range()).is_some_and(|p| p.reached);
            if ok {
                self.poi_acquire(r.pos);
                bed = Some(r.pos);
                break;
            }
        }
        let Some(bed) = bed else {
            if let Some(o) = self.mob_ref(other) {
                self.mob_event(o, 13);
            }
            self.mob_event(m, 13);
            return;
        };
        // Baby: type 50% biome, 25% each parent.
        let roll = m.rng.next_double();
        let ty = if roll < 0.5 {
            self.villager_type(m.block_pos())
        } else if roll < 0.75 {
            m.villager[0]
        } else {
            self.mob_ref(other).map_or(m.villager[0], |o| o.villager[0])
        };
        m.age = 6000;
        self.with_villager(other, |_, o, _| o.age = 6000);
        let mut baby = self.new_mob(Kind::Villager, m.pos, 0.0);
        baby.villager = [ty, PROF_NONE, 1];
        baby.x.vdata_final = true;
        baby.age = -24000;
        baby.yaw = 0.0;
        baby.pitch = 0.0;
        // The baby is not "finalized": neither follow bonus, nor left-handed, no world draw.
        self.npc_init(&mut baby);
        let home = self.gpos_here(bed);
        if let Some(b) = baby.npc.as_mut().and_then(|n| n.brain.as_mut()) {
            b.mem.home = Some(home);
        }
        self.mob_event(&baby, 12);
        self.add_mob(baby);
    }

    // ---------- Work ----------

    pub(crate) fn play_work_sound(&mut self, m: &mut Mob) {
        let prof = super::variants::PROFESSIONS.get(m.villager[1] as usize).copied().unwrap_or("none");
        if !matches!(prof, "none" | "nitwit") {
            self.make_sound(m, &format!("minecraft:entity.villager.work_{prof}"));
        }
    }

    /// Up to 3 breads (3 wheat each) if ≤ 36 breads; surplus dropped.
    pub(crate) fn make_bread(&mut self, m: &mut Mob) {
        if self.npc_count(m, "minecraft:bread") > 36 {
            return;
        }
        let n = (self.npc_count(m, "minecraft:wheat") / 3).min(3);
        if n == 0 {
            return;
        }
        self.npc_remove(m, "minecraft:wheat", n * 3);
        if let Some(rest) = self.npc_add(m, Stack { id: "minecraft:bread".into(), count: n, components: None }) {
            self.spawn_at_location(m, rest, 0.5);
        }
    }

    /// Item at the position (+ vertical offset), random velocity.
    pub(crate) fn spawn_at_location(&mut self, m: &Mob, s: Stack, dy: f64) {
        let r = &mut self.mobs.rng;
        let v = [r.next_double() * 0.2 - 0.1, 0.2, r.next_double() * 0.2 - 0.1];
        self.spawn_item([m.pos[0], m.pos[1] + dy, m.pos[2]], v, s, 10);
    }

    /// Composter: bone meal recovered at level 8, then up to 20 seeds (≥ 10 kept).
    pub(crate) fn compost(&mut self, m: &mut Mob, p: [i32; 3]) {
        let s0 = self.st(p);
        let mut level = self.v().get_int(s0, "level");
        if level == 8 {
            self.pop_resource_up(p, Stack { id: "minecraft:bone_meal".into(), count: 1, components: None });
            level = 0;
            let ns = self.v().set_int(s0, "level", 0);
            self.set_block(p, ns, 3);
            let c = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
            self.world_sound("minecraft:block.composter.empty", 4, c, 1.0, 1.0);
        }
        let start = level;
        let mut total = 20;
        let mut seen = [0i32; 2];
        let items = ["minecraft:wheat_seeds", "minecraft:beetroot_seeds"];
        let inv: Vec<Option<Stack>> = m.npc.as_ref().map(|n| n.inv.clone()).unwrap_or_default();
        'outer: for i in (0..inv.len()).rev() {
            if total <= 0 {
                break;
            }
            let Some(st) = &inv[i] else { continue };
            let Some(k) = items.iter().position(|x| *x == st.id) else { continue };
            seen[k] += st.count;
            let use_n = (seen[k] - 10).min(total).min(st.count);
            if use_n <= 0 {
                continue;
            }
            total -= use_n;
            for _ in 0..use_n {
                // Layers of the `compostable` component (seeds: "weak").
                if level < 7 {
                    let layers = self.compost_layers(&st.id, level);
                    if layers > 0 {
                        level = (level + layers).clamp(0, 7);
                        let s = self.st(p);
                        let ns = self.v().set_int(s, "level", level);
                        self.set_block(p, ns, 3);
                        if level == 7 {
                            self.schedule_block_tick(p, 20);
                        }
                    }
                    if let Some(n) = m.npc.as_mut()
                        && let Some(slot) = n.inv[i].as_mut()
                    {
                        slot.count -= 1;
                        if slot.count <= 0 {
                            n.inv[i] = None;
                        }
                    }
                }
                if level == 7 {
                    self.send_level_event(1500, p, i32::from(level != start));
                    return;
                }
            }
            let _ = &mut seen;
            continue 'outer;
        }
        self.send_level_event(1500, p, i32::from(level != start));
    }

    /// Layers added by a compostable item (the game's provider: 1 on an empty composter,
    /// otherwise weighted list on the world random).
    pub(crate) fn compost_layers(&mut self, item: &str, level: i32) -> i32 {
        let comp = self.life.items.defaults(item)["minecraft:compostable"]["layers"].clone();
        let prov = match &comp {
            serde_json::Value::String(id) => self.life.pack.get("context_int_provider", id).map(|v| (*v).clone()),
            v if v.is_object() => Some(v.clone()),
            _ => None,
        };
        let Some(prov) = prov else { return 0 };
        self.eval_compost(&prov, level)
    }

    fn eval_compost(&mut self, v: &serde_json::Value, level: i32) -> i32 {
        if let Some(n) = v.as_i64() {
            return n as i32;
        }
        match v["type"].as_str().unwrap_or("").trim_start_matches("minecraft:") {
            "number_dispatcher" => {
                for c in v["cases"].as_array().into_iter().flatten() {
                    let want = c["condition"]["state"]["level"].as_str().and_then(|s| s.parse::<i32>().ok());
                    if want == Some(level) {
                        return self.eval_compost(&c["value"], level);
                    }
                }
                self.eval_compost(&v["default"], level)
            }
            "weighted_list" => {
                let list = v["distribution"].as_array().cloned().unwrap_or_default();
                let total: i32 = list.iter().map(|e| e["weight"].as_i64().unwrap_or(0) as i32).sum();
                if total <= 0 {
                    return 0;
                }
                let mut r = self.mobs.rng.next_int_bound(total);
                for e in &list {
                    r -= e["weight"].as_i64().unwrap_or(0) as i32;
                    if r < 0 {
                        return self.eval_compost(&e["data"], level);
                    }
                }
                0
            }
            "constant" => v["value"].as_i64().unwrap_or(0) as i32,
            _ => 0,
        }
    }

    /// Bone meal above the composter.
    pub(crate) fn pop_resource_up(&mut self, p: [i32; 3], s: Stack) {
        let r = &mut self.mobs.rng;
        let ox = f64::from((r.next_float() - 0.5) * 0.7);
        let oz = f64::from((r.next_float() - 0.5) * 0.7);
        let pos = [f64::from(p[0]) + 0.5 + ox, f64::from(p[1]) + 1.01, f64::from(p[2]) + 0.5 + oz];
        let v = [r.next_double() * 0.2 - 0.1, 0.2, r.next_double() * 0.2 - 0.1];
        self.spawn_item(pos, v, s, 10);
    }

    pub(crate) fn schedule_block_tick(&mut self, p: [i32; 3], delay: i32) {
        let s = self.st(p);
        let b = self.v().block(s);
        self.schedule_tick(p, b, delay);
    }

    /// New day (catch-up), then right and need.
    pub(crate) fn should_restock(&mut self, m: &mut Mob) -> bool {
        let gt = self.time;
        let day =
            self.clocks.iter().find(|(n, _)| n == "minecraft:overworld").map_or(self.time, |c| c.1).div_euclid(24000);
        let Some(n) = m.npc.as_mut() else { return false };
        let mut new_day = gt > n.last_restock + 12000;
        new_day |= n.restock_day > 0 && day > n.restock_day;
        n.restock_day = day;
        if new_day {
            n.last_restock = gt;
            // The demand catch-up then counter reset to zero.
            let missed = 2 - n.restocks;
            if let Some(o) = n.offers.as_mut() {
                if missed > 0 {
                    for x in o.iter_mut() {
                        x.uses = 0;
                    }
                }
                for _ in 0..missed.max(0) {
                    for x in o.iter_mut() {
                        x.update_demand();
                    }
                }
            }
            n.restocks = 0;
            if n.trading {
                self.update_special_prices(m);
            }
        }
        let n = m.npc.as_ref().expect("trader");
        let allowed = n.restocks == 0 || (n.restocks < 2 && gt > n.last_restock + 2400);
        let needs = n.offers.as_ref().is_some_and(|o| o.iter().any(Offer::needs_restock));
        allowed && needs
    }

    pub(crate) fn restock(&mut self, m: &mut Mob) {
        let gt = self.time;
        if let Some(n) = m.npc.as_mut() {
            if let Some(o) = n.offers.as_mut() {
                for x in o.iter_mut() {
                    x.update_demand();
                }
                for x in o.iter_mut() {
                    x.uses = 0;
                }
            }
            n.last_restock = gt;
            n.restocks += 1;
        }
        if self.npc_trading(m) {
            self.update_special_prices(m);
        }
    }

    /// Crop growth with bone meal for a crop: age + 2..5 (beetroot: ÷ 3), world random.
    pub(crate) fn grow_crop(&mut self, p: [i32; 3]) -> bool {
        let s = self.st(p);
        let Some((age, max)) = self.crop_max_age(s) else { return false };
        if age >= max {
            return false;
        }
        let torch = self.state_name(s) == "minecraft:torchflower_crop";
        let beet = self.state_name(s) == "minecraft:beetroots";
        // Torchflower sprout: +1 without a draw; beetroot: draw divided by 3.
        let inc = if torch {
            1
        } else {
            let i = 2 + self.mobs.rng.next_int_bound(4);
            if beet { i / 3 } else { i }
        };
        let age = (age + inc).min(max);
        let ns = if torch && age == 2 {
            self.v().default_state(self.bs.t.block_id("torchflower"))
        } else {
            self.v().set_int(s, "age", age)
        };
        self.set_block(p, ns, 2);
        true
    }

    pub(crate) fn villager_destroy_block(&mut self, m: &Mob, p: [i32; 3]) {
        // Loot with an empty tool, nothing from the player.
        let by = crate::mobs::recent::vibration::Src::Living(crate::mobs::mob::Ent::Mob(m.id));
        self.destroy_block_by(p, true, 512, Some(by));
    }

    pub(crate) fn plant_seed(&mut self, m: &mut Mob, p: [i32; 3]) {
        let Some(n) = m.npc.as_ref() else { return };
        let idx = n.inv.iter().position(|s| {
            s.as_ref().is_some_and(|s| self.life.items.has_tag("minecraft:villager_plantable_seeds", &s.id))
        });
        let Some(i) = idx else { return };
        let seed = n.inv[i].as_ref().map(|s| s.id.clone()).unwrap_or_default();
        let block = match seed.as_str() {
            "minecraft:wheat_seeds" => "minecraft:wheat",
            "minecraft:beetroot_seeds" => "minecraft:beetroots",
            "minecraft:carrot" => "minecraft:carrots",
            "minecraft:potato" => "minecraft:potatoes",
            "minecraft:torchflower_seeds" => "minecraft:torchflower_crop",
            "minecraft:pitcher_pod" => "minecraft:pitcher_crop",
            _ => return,
        };
        let Some(state) = self.data.blocks.iter().find(|b| b.name == block).map(|b| b.default_state) else { return };
        self.set_block(p, state, 3);
        self.game_event_block("minecraft:block_place", p, Some(crate::mobs::mob::Ent::Mob(m.id)), Some(state));
        let c = [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])];
        self.world_sound("minecraft:item.crop.plant", 4, c, 1.0, 1.0);
        if let Some(n) = m.npc.as_mut()
            && let Some(s) = n.inv[i].as_mut()
        {
            s.count -= 1;
            if s.count <= 0 {
                n.inv[i] = None;
            }
        }
    }

    /// Throws half a stack toward a target (an item throw toward a target).
    pub(crate) fn throw_half_stack(&mut self, m: &mut Mob, target: [f64; 3], pred: impl Fn(&Server, &Stack) -> bool) {
        let len = m.npc.as_ref().map_or(0, |n| n.inv.len());
        let mut thrown = None;
        for i in 0..len {
            let Some(s) = m.npc.as_ref().and_then(|n| n.inv[i].clone()) else { continue };
            if !pred(self, &s) {
                continue;
            }
            let max = self.life.items.max_stack(&s);
            let count = if s.count > max / 2 {
                s.count / 2
            } else if s.count > 24 {
                s.count - 24
            } else {
                continue;
            };
            if let Some(n) = m.npc.as_mut()
                && let Some(st) = n.inv[i].as_mut()
            {
                st.count -= count;
                if st.count <= 0 {
                    n.inv[i] = None;
                }
            }
            thrown = Some(Stack { count, ..s });
            break;
        }
        if let Some(s) = thrown {
            self.throw_item(m, s, target);
        }
    }

    pub(crate) fn throw_item(&mut self, m: &Mob, s: Stack, target: [f64; 3]) {
        let pos = [m.pos[0], m.eye_y() - 0.3, m.pos[2]];
        let d = [target[0] - m.pos[0], target[1] - m.pos[1], target[2] - m.pos[2]];
        let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let v = if l < 1.0e-4 { [0.0; 3] } else { [d[0] / l * 0.3, d[1] / l * 0.3, d[2] / l * 0.3] };
        let v = v.map(|x| x * f64::from(0.3f32) / 0.3);
        self.spawn_item(pos, v, s, 10);
    }

    /// Gift to the hero: profession table (baby, no profession).
    pub(crate) fn throw_gift(&mut self, m: &mut Mob) {
        let prof = super::variants::PROFESSIONS.get(m.villager[1] as usize).copied().unwrap_or("none");
        let table = if m.is_baby() {
            "gameplay/hero_of_the_village/baby_gift".to_owned()
        } else if matches!(prof, "none" | "nitwit") {
            "gameplay/hero_of_the_village/unemployed_gift".to_owned()
        } else {
            format!("gameplay/hero_of_the_village/{prof}_gift")
        };
        let drops = self.simple_loot(&format!("minecraft:{table}"), None);
        let target = self.pos;
        for d in drops {
            self.throw_item(m, d, target);
        }
    }

    // ---------- Sleep ----------

    /// Position on the bed, occupied bed, lying pose.
    pub(crate) fn start_sleeping(&mut self, m: &mut Mob, bed: [i32; 3]) -> bool {
        let s = self.st(bed);
        let Some(h) = self.bed_sleep_height(bed) else { return false };
        let ns = self.v().set_bool(s, "occupied", true);
        self.set_block(bed, ns, 3);
        m.pos = [f64::from(bed[0]) + 0.5, f64::from(bed[1]) + h + 0.125, f64::from(bed[2]) + 0.5];
        m.vel = [0.0; 3];
        m.needs_sync = true;
        if let Some(n) = m.npc.as_mut() {
            n.sleeping = Some(bed);
        }
        true
    }

    /// Waking: bed freed, stands up next to it like the player (the game's search for a
    /// 0.6 × 1.95 villager box, side chosen by its yaw), turned toward the bed, pitch 0;
    /// `last_woken`.
    pub(crate) fn stop_sleeping(&mut self, m: &mut Mob, b: &mut Brain) {
        let Some(bed) = m.npc.as_mut().and_then(|n| n.sleeping.take()) else { return };
        if let Some((stand, yaw)) = self.leave_bed(bed, m.yaw, (0.6, 1.95), false) {
            m.pos = stand;
            m.yaw = yaw;
            m.pitch = 0.0;
        }
        m.needs_sync = true;
        b.mem.last_woken = Some(self.time);
    }

    // ---------- Life cycle ----------

    /// Trader state of a new or read-back mob, villager brain.
    pub(crate) fn npc_init(&self, m: &mut Mob) {
        if !is_npc(m.kind) {
            return;
        }
        if m.npc.is_none() {
            m.npc = Some(Box::new(Npc { inv: vec![None; 8], ..Npc::default() }));
        }
        m.can_pickup = m.kind == Kind::Villager || m.can_pickup;
        m.nav.can_open_doors = m.kind == Kind::Villager;
        m.nav.can_float = true;
        if m.kind == Kind::Villager {
            m.nav.required_length = 48.0;
            m.nav.max_visited = (f64::from(m.nav.required_length).max(m.follow_range()) * 16.0).floor() as i32;
            let mem =
                m.npc.as_mut().and_then(|n| n.brain.take()).map_or_else(brain::Mem::default, |b| brain::pack(&b.mem));
            let b = self.make_brain(m, mem);
            let baby = m.is_baby();
            if let Some(n) = m.npc.as_mut() {
                n.brain = Some(Box::new(b));
                n.was_baby = baby;
            }
        }
    }

    /// Brain, reputation after a trade, raid, end of trade.
    pub(crate) fn npc_custom_ai(&mut self, m: &mut Mob) {
        if m.kind != Kind::Villager {
            return;
        }
        // Passage to adulthood: brain rebuilt.
        let baby = m.is_baby();
        if m.npc.as_ref().is_some_and(|n| n.was_baby != baby) {
            if let Some(b) = m.npc.as_mut().and_then(|n| n.brain.take()) {
                let nb = self.refresh_brain(m, *b);
                if let Some(n) = m.npc.as_mut() {
                    n.brain = Some(Box::new(nb));
                }
            }
            if let Some(n) = m.npc.as_mut() {
                n.was_baby = baby;
            }
        }
        if let Some(mut b) = m.npc.as_mut().and_then(|n| n.brain.take()) {
            self.brain_tick(m, &mut b);
            if b.refresh {
                b.refresh = false;
                b = Box::new(self.refresh_brain(m, *b));
            }
            if let Some(n) = m.npc.as_mut() {
                n.brain = Some(b);
            }
        }
        if m.npc.as_ref().is_some_and(|n| n.last_traded) {
            let uuid = self.uuid;
            self.reputation_event(m, "trade", uuid);
            self.mob_event(m, 14);
            if let Some(n) = m.npc.as_mut() {
                n.last_traded = false;
            }
        }
        if !m.no_ai && m.rng.next_int_bound(100) == 0 {
            // Raid active and not finished here: fear drops (event 42).
            let raid = self.raid_at(m.block_pos()).and_then(|id| self.raid_ref(id));
            if raid.is_some_and(|r| r.active && !r.is_over()) {
                self.mob_event(m, 42);
            }
        }
        let empty = m.npc.as_ref().is_some_and(|n| n.offers.as_ref().is_none_or(Vec::is_empty));
        if empty && self.npc_trading(m) {
            self.npc_stop_trading(m);
        }
    }

    pub(crate) fn npc_stop_trading(&mut self, m: &mut Mob) {
        if let Some(n) = m.npc.as_mut() {
            n.trading = false;
        }
        if self.life.menu.as_ref().and_then(|mm| mm.merchant.as_ref()).is_some_and(|t| t.mob == m.id) {
            self.close_menu(true);
        }
    }

    /// End of the entity tick: unhappiness, gossip decay (villager), trader delay.
    pub(crate) fn npc_post_tick(&mut self, m: &mut Mob) {
        if !is_npc(m.kind) {
            return;
        }
        let t = self.time;
        if let Some(n) = m.npc.as_mut() {
            if n.unhappy > 0 {
                n.unhappy -= 1;
            }
            if m.kind == Kind::Villager {
                if n.last_gossip_decay == 0 {
                    n.last_gossip_decay = t;
                } else if t >= n.last_gossip_decay + 24000 {
                    n.gossips.decay();
                    n.last_gossip_decay = t;
                }
            }
        }
        if m.kind == Kind::WanderingTrader {
            self.trader_post_tick(m);
        }
    }

    /// Pickup of wanted items (range 1, 0, 1).
    pub(crate) fn npc_pickup(&mut self, m: &mut Mob) {
        if m.kind != Kind::Villager || !m.can_pickup || !m.alive() || !self.rule("minecraft:mob_griefing", true) {
            return;
        }
        let bb = m.bbox();
        let ids: Vec<i32> = self
            .life
            .drops
            .iter()
            .filter(|d| {
                !d.is_removed()
                    && d.pickup_delay <= 0
                    && d.pos[0] + 0.125 > bb.min[0] - 1.0
                    && d.pos[0] - 0.125 < bb.max[0] + 1.0
                    && d.pos[1] + 0.25 > bb.min[1]
                    && d.pos[1] < bb.max[1]
                    && d.pos[2] + 0.125 > bb.min[2] - 1.0
                    && d.pos[2] - 0.125 < bb.max[2] + 1.0
            })
            .map(|d| d.id)
            .collect();
        for id in ids {
            let Some(s) = self.life.drops.iter().find(|d| d.id == id).map(|d| d.stack.clone()) else { continue };
            if !self.villager_wants(m, &s) {
                continue;
            }
            let rest = self.npc_add(m, s.clone());
            let taken = s.count - rest.as_ref().map_or(0, |r| r.count);
            if taken > 0 {
                self.mob_take(id, m.id, taken);
            }
        }
    }

    /// Hurt by an entity: reputation, visible anger if player.
    pub(crate) fn npc_hurt_by(&mut self, m: &mut Mob, by: Ent) {
        if m.kind != Kind::Villager {
            return;
        }
        if let Some(u) = self.npc_ent_uuid(by) {
            self.reputation_event(m, "hurt", u);
        }
        if m.alive() && by == Ent::Player {
            self.mob_event(m, 13);
        }
    }

    /// Death: witnesses notified, POIs released, trade stopped.
    pub(crate) fn npc_died(&mut self, m: &mut Mob, by: Option<Ent>) {
        if !is_npc(m.kind) {
            return;
        }
        if m.kind == Kind::Villager {
            if let Some(src) = by.and_then(|e| self.npc_ent_uuid(e)) {
                // All villagers of the list, visibility evaluated for each (not only
                // those already in cache).
                let mut witnesses: Vec<i32> = Vec::new();
                if let Some(mut b) = m.npc.as_mut().and_then(|n| n.brain.take()) {
                    let list: Vec<i32> = b
                        .mem
                        .visible
                        .as_ref()
                        .map(|v| {
                            v.iter()
                                .filter_map(|x| if let Ent::Mob(id) = x.0 { Some(id) } else { None })
                                .filter(|&id| self.mob_ref(id).is_some_and(|o| o.kind == Kind::Villager))
                                .collect()
                        })
                        .unwrap_or_default();
                    for id in list {
                        if self.visible(m, &mut b, Ent::Mob(id)) {
                            witnesses.push(id);
                        }
                    }
                    if let Some(n) = m.npc.as_mut() {
                        n.brain = Some(b);
                    }
                }
                for w in witnesses {
                    self.with_villager(w, |s, o, _| {
                        if o.kind == Kind::Villager {
                            s.reputation_event(o, "killed", src);
                        }
                    });
                }
            }
            self.release_all_pois(m);
        }
        self.npc_stop_trading(m);
    }

    pub(crate) fn release_all_pois(&mut self, m: &mut Mob) {
        let Some(b) = m.npc.as_mut().and_then(|n| n.brain.take()) else { return };
        for mem in [behave::PoiMem::Home, behave::PoiMem::JobSite, behave::PoiMem::Potential, behave::PoiMem::Meeting] {
            self.release_poi_mem(m, &b, mem);
        }
        if let Some(n) = m.npc.as_mut() {
            n.brain = Some(b);
        }
    }

    /// Villager zombie cured by the player: reputation (type "cured zombie villager").
    pub(crate) fn villager_cured(&mut self, v: &mut Mob, starter: Option<u128>) {
        if let Some(u) = starter.filter(|&u| u == self.uuid) {
            self.npc_init(v);
            self.reputation_event(v, "cured", u);
        }
    }
}

fn take(s: &mut Option<Stack>, n: i32) {
    if let Some(x) = s {
        x.count -= n;
        if x.count <= 0 {
            *s = None;
        }
    }
}

// ---------- Metadata and saving ----------

/// Villager ↔ zombie villager conversion: live offers and gossip written into the compound
/// of the new form (kept for saving), then read back if it is a trader.
pub fn copy_trade(from: &Mob, to: &mut Mob) {
    if let Some(n) = &from.npc {
        let dst = to.nbt.get_or_insert_with(|| Nbt::Compound(Vec::new()));
        if let Some(o) = &n.offers {
            nbtio::set(dst, "Offers", super::trades::offers_to_nbt(o));
        }
        nbtio::set(dst, "Gossips", n.gossips.to_nbt());
    }
    // Non-trader form (zombie villager): offers and gossip in its compound.
    let src = if from.npc.is_some() { None } else { from.nbt.clone() };
    if let Some(n) = to.npc.as_mut() {
        let get = |k: &str| src.as_ref().and_then(|s| s.get(k).cloned());
        if let Some(o) = get("Offers") {
            n.offers = Some(super::trades::offers_from_nbt(&o));
        }
        if let Some(g) = get("Gossips") {
            n.gossips = Gossips::from_nbt(Some(&g));
        }
    }
}

/// Trader metadata: unhappiness (18), data (19, 20), sleep (pose, bed).
pub fn sync_data(m: &mut Mob) {
    let Some(n) = &m.npc else { return };
    let (unhappy, sleeping) = (n.unhappy, n.sleeping);
    m.set(18, Meta::Int(unhappy));
    if m.kind == Kind::Villager {
        m.set(20, Meta::Bool(m.x.vdata_final));
    }
    if m.kind == Kind::WanderingTrader {
        let drinking = m.npc.as_ref().is_some_and(|n| n.drinking > 0);
        m.set(8, Meta::Byte(i8::from(drinking)));
    }
    if m.x.effects.iter().any(|e| e.id == "minecraft:invisibility")
        && let Some(Meta::Byte(f)) = m.data.iter().find(|s| s.index == 0).map(|s| s.value.clone())
    {
        m.set(0, Meta::Byte(f | 0x20));
    }
    if let Some(bed) = sleeping {
        m.set(6, Meta::Pose(2));
        m.set(14, Meta::BlockPos(Some(bed)));
    } else {
        m.set(14, Meta::BlockPos(None));
    }
}

/// Reading the trader fields.
pub fn load(m: &mut Mob, e: &Nbt) {
    if !is_npc(m.kind) {
        return;
    }
    let mut n = Npc { inv: vec![None; 8], ..Npc::default() };
    n.offers = e.get("Offers").map(super::trades::offers_from_nbt);
    for it in nbtio::list(e.get("Inventory")) {
        if let Some(s) = Stack::from_nbt(it) {
            let _ = n.inv.iter_mut().find(|x| x.is_none()).map(|x| *x = Some(s));
        }
    }
    if m.kind == Kind::Villager {
        n.food = nbtio::int(e, "FoodLevel").unwrap_or(0) as i32;
        n.gossips = Gossips::from_nbt(e.get("Gossips"));
        n.last_restock = nbtio::int(e, "LastRestock").unwrap_or(0);
        n.last_gossip_decay = nbtio::int(e, "LastGossipDecay").unwrap_or(0);
        n.restocks = nbtio::int(e, "RestocksToday").unwrap_or(0) as i32;
        let mem = brain::mem_from_nbt(e.get("Brain"));
        n.brain = Some(Box::new(Brain {
            mem,
            active: brain::Act::Idle,
            default: brain::Act::Idle,
            available: Vec::new(),
            baby: false,
            last_schedule: -9999,
            sensors: [0; 9],
            bed_cache: Vec::new(),
            bed_tried: 0,
            bed_last: 0,
            refresh: false,
            nodes: Vec::new(),
        }));
        if let Some(Nbt::IntArray(p)) = e.get("sleeping_pos")
            && p.len() == 3
        {
            n.sleeping = Some([p[0], p[1], p[2]]);
        }
    }
    if m.kind == Kind::WanderingTrader {
        n.despawn_delay = nbtio::int(e, "DespawnDelay").unwrap_or(0) as i32;
        n.wander = match e.get("wander_target") {
            Some(Nbt::IntArray(p)) if p.len() == 3 => Some([p[0], p[1], p[2]]),
            _ => None,
        };
        // Residence area read back as is (independent of the stroll destination).
        let radius = nbtio::int(e, "home_radius").map_or(-1, |r| r as i32);
        n.home = (radius >= 0).then(|| {
            let p = match e.get("home_pos") {
                Some(Nbt::IntArray(p)) if p.len() == 3 => [p[0], p[1], p[2]],
                _ => [0, 0, 0],
            };
            (p, radius)
        });
    }
    m.npc = Some(Box::new(n));
}

/// Writing the trader fields.
pub fn save(m: &Mob, e: &mut Nbt) {
    let Some(n) = &m.npc else { return };
    let set = |e: &mut Nbt, k: &str, v: Nbt| nbtio::set(e, k, v);
    match &n.offers {
        Some(o) => set(e, "Offers", super::trades::offers_to_nbt(o)),
        None => nbtio::remove(e, "Offers"),
    }
    let inv: Vec<Nbt> = n.inv.iter().flatten().map(|s| s.to_nbt(None)).collect();
    if inv.is_empty() {
        nbtio::remove(e, "Inventory");
    } else {
        set(e, "Inventory", Nbt::List(inv));
    }
    if m.kind == Kind::Villager {
        set(e, "VillagerDataFinalized", Nbt::Byte(i8::from(m.x.vdata_final)));
        set(e, "FoodLevel", Nbt::Byte(n.food as i8));
        set(e, "Gossips", n.gossips.to_nbt());
        set(e, "LastRestock", Nbt::Long(n.last_restock));
        set(e, "LastGossipDecay", Nbt::Long(n.last_gossip_decay));
        set(e, "RestocksToday", Nbt::Int(n.restocks));
        if let Some(b) = &n.brain {
            set(e, "Brain", brain::mem_to_nbt(&b.mem));
        }
        match n.sleeping {
            Some(p) => set(e, "sleeping_pos", Nbt::IntArray(p.to_vec())),
            None => nbtio::remove(e, "sleeping_pos"),
        }
    }
    if m.kind == Kind::WanderingTrader {
        set(e, "DespawnDelay", Nbt::Int(n.despawn_delay));
        match n.wander {
            Some(p) => set(e, "wander_target", Nbt::IntArray(p.to_vec())),
            None => nbtio::remove(e, "wander_target"),
        }
        match n.home {
            Some((p, r)) => {
                set(e, "home_radius", Nbt::Int(r));
                set(e, "home_pos", Nbt::IntArray(p.to_vec()));
            }
            None => {
                nbtio::remove(e, "home_radius");
                nbtio::remove(e, "home_pos");
            }
        }
    }
}
