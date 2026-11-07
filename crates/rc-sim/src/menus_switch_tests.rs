//! Equivalence of the menu switches (`MenuSwitches`, `docs/perf.md`, menus) at the menu level: the
//! same clicks (every mode, drags, gathers, bundles, crafting, trading, mount equipment, client
//! copies announced right or wrong) on fresh menus under each of the 16 combinations of the
//! switches. The trace (every packet the menus queue, the slots, the cursor, the dropped
//! items) must be the same for every combination and match a stored fingerprint. Version data and
//! the player's jar mandatory.

use super::*;
use crate::menus::merchant::Trade;
use crate::menus::{FORCED_SWITCHES, MenuSwitches, MountInfo};
use crate::mobs::trades::{Cost, Offer};

/// Fingerprint of the trace (the former code, every switch off); a change of the menu rules
/// moves it on purpose, a switch never.
const MENU_TRACE_FINGERPRINT: u64 = 0x3025_06a4_786b_f994;

/// `TestEnv` with a client fingerprint per stack (item index and count), so that a copy the
/// client announces can match what the server holds.
struct HashingEnv<'a> {
    inner: TestEnv<'a>,
}

impl Env for HashingEnv<'_> {
    fn items(&self) -> &Items {
        self.inner.items()
    }
    fn creative(&self) -> bool {
        self.inner.creative
    }
    fn selected(&self) -> usize {
        0
    }
    fn drop_item(&mut self, s: Stack, thrown: bool) {
        self.inner.drop_item(s, thrown);
    }
    fn craft(&mut self, grid: &[Option<Stack>], w: usize) -> Option<Stack> {
        self.inner.craft(grid, w)
    }
    fn remaining(&mut self, grid: &[Option<Stack>], w: usize) -> Vec<Option<Stack>> {
        self.inner.remaining(grid, w)
    }
    fn crafted(&mut self, result: &Stack, count: i32) {
        self.inner.crafted(result, count);
    }
    fn smelted(&mut self, _count: i32) {}
    fn is_fuel(&self, s: &Stack) -> bool {
        self.inner.is_fuel(s)
    }
    fn can_smelt(&self, s: &Stack, kind: u8) -> bool {
        self.inner.can_smelt(s, kind)
    }
    fn hashed(&self, s: Option<&Stack>) -> Option<HashedStack> {
        s.map(|s| HashedStack {
            item: self.inner.w.items.id(&s.id).unwrap_or(-1),
            count: s.count,
            added: Vec::new(),
            removed: Vec::new(),
        })
    }
}

/// Every combination of the four switches.
fn all_combinations() -> Vec<MenuSwitches> {
    (0..16u8)
        .map(|b| MenuSwitches {
            broadcast_in_place: b & 1 != 0,
            remote_copy_reuse: b & 2 != 0,
            pull_without_decoding: b & 4 != 0,
            fewer_allocations: b & 8 != 0,
        })
        .collect()
}

fn fnv(h: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(h, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3))
}

fn digest(lines: &[String]) -> u64 {
    lines.iter().fold(0xcbf2_9ce4_8422_2325, |h, l| fnv(fnv(h, l.as_bytes()), b"\n"))
}

/// A menu under test with the player's slots and the trace of what it did.
struct Bench<'a> {
    env: HashingEnv<'a>,
    inv: Vec<Option<Stack>>,
    trace: Vec<String>,
}

impl Bench<'_> {
    /// One click, then the client's announced copies, then the broadcast; all recorded.
    fn click(&mut self, m: &mut Menu, slot: i16, button: i8, mode: i32) {
        m.click(&mut self.env, &mut self.inv, slot, button, mode);
        self.sync(m);
    }

    /// The broadcast and a record of everything visible.
    fn sync(&mut self, m: &mut Menu) {
        m.broadcast(&self.env, &self.inv);
        self.record(m);
    }

    fn record(&mut self, m: &mut Menu) {
        self.trace.push(format!("out {:?}", std::mem::take(&mut m.out)));
        self.trace.push(format!("own {:?} carried {:?} state {}", m.own, m.carried, m.state));
        self.trace.push(format!("inv {:?}", self.inv));
        self.trace.push(format!("dropped {:?} crafted {}", self.env.inner.dropped, self.env.inner.crafted));
    }

    /// The client announces slot `slot` as `held` (right or wrong) and its cursor as `cursor`.
    fn announce(&mut self, m: &mut Menu, slot: i16, held: Option<&Stack>, cursor: Option<&Stack>) {
        m.set_remote(slot, self.env.hashed(held));
        m.set_remote_carried(self.env.hashed(cursor));
        self.sync(m);
    }
}

fn plain(id: &str, count: i32) -> Option<Stack> {
    st(id, count)
}

fn damaged(id: &str, damage: i32) -> Option<Stack> {
    let components = rc_proto::nbt::Nbt::Compound(vec![("minecraft:damage".into(), rc_proto::nbt::Nbt::Int(damage))]);
    Some(Stack { id: format!("minecraft:{id}"), count: 1, components: Some(components) })
}

/// Inventory menu: every click mode, a drag, a gather, the 2×2 grid, bundles, client copies.
fn inventory_clicks(b: &mut Bench<'_>) {
    let mut m = Menu::inventory();
    b.inv[9] = plain("cobblestone", 33);
    b.inv[10] = plain("oak_planks", 5);
    b.inv[11] = damaged("iron_sword", 7);
    b.inv[12] = plain("bundle", 1);
    b.inv[36] = plain("cobblestone", 50);
    m.send_all(&b.inv);
    b.record(&mut m);
    for (slot, button, mode) in [(9, 1, mode::PICKUP), (13, 1, mode::PICKUP), (9, 0, mode::PICKUP), (36, 0, mode::QUICK_MOVE)] {
        b.click(&mut m, slot, button, mode);
    }
    // Client copies: right for slot 9, wrong for slot 10, cursor wrong.
    let right = b.inv[9].clone();
    b.announce(&mut m, 9, right.as_ref(), None);
    b.announce(&mut m, 10, plain("dirt", 2).as_ref(), plain("stone", 1).as_ref());
    // Planks into the 2×2 grid, then the result taken by a shift-click.
    for (slot, button, mode) in [(10, 0, mode::PICKUP), (1, 1, mode::PICKUP), (2, 1, mode::PICKUP), (10, 0, mode::PICKUP)] {
        b.click(&mut m, slot, button, mode);
    }
    b.click(&mut m, 0, 0, mode::QUICK_MOVE);
    // Swap with the hotbar and the off hand, throw one then the whole stack.
    for (slot, button, mode) in [(11, 2, mode::SWAP), (38, 40, mode::SWAP), (9, 0, mode::THROW), (9, 1, mode::THROW)] {
        b.click(&mut m, slot, button, mode);
    }
    // Drag over three slots, then a gather back onto the cursor.
    b.click(&mut m, 36, 0, mode::PICKUP);
    for (slot, button) in [(-999, 0), (20, 1), (21, 1), (22, 1), (-999, 2)] {
        b.click(&mut m, slot, button, mode::QUICK_CRAFT);
    }
    b.click(&mut m, 20, 0, mode::PICKUP_ALL);
    b.click(&mut m, 23, 0, mode::PICKUP);
    // Bundle: carried onto a stack, then a stack carried onto the placed bundle.
    for (slot, button) in [(12, 0), (13, 0), (24, 1), (12, 0), (24, 0), (12, 0), (12, 1)] {
        b.click(&mut m, slot, button, mode::PICKUP);
    }
}

/// Chest menu: quick moves both ways, a gather, a clone in creative, a resync.
fn container_clicks(b: &mut Bench<'_>) {
    let mut contents = vec![None; 27];
    contents[0] = plain("dirt", 64);
    contents[1] = plain("dirt", 10);
    contents[5] = damaged("diamond_pickaxe", 3);
    contents[26] = plain("stick", 20);
    let mut m = Menu::container(1, 2, contents, false);
    m.send_all(&b.inv);
    b.record(&mut m);
    for (slot, button, mode) in [(0, 0, mode::QUICK_MOVE), (5, 0, mode::QUICK_MOVE), (27, 0, mode::QUICK_MOVE), (1, 0, mode::PICKUP)] {
        b.click(&mut m, slot, button, mode);
    }
    b.click(&mut m, 30, 1, mode::PICKUP_ALL);
    b.click(&mut m, 2, 0, mode::PICKUP);
    b.env.inner.creative = true;
    b.click(&mut m, 26, 2, mode::CLONE);
    b.click(&mut m, -999, 0, mode::PICKUP);
    b.env.inner.creative = false;
    m.send_all(&b.inv);
    b.record(&mut m);
}

/// Crafting table: a 3×3 recipe placed by drags, the result shift-clicked.
fn crafting_clicks(b: &mut Bench<'_>) {
    let mut m = Menu::crafting(2);
    b.inv[30] = plain("oak_planks", 12);
    m.send_all(&b.inv);
    b.record(&mut m);
    b.click(&mut m, 30 - 9 + 10, 0, mode::PICKUP);
    for (slot, button) in [(-999, 0), (1, 1), (2, 1), (4, 1), (5, 1), (-999, 2)] {
        b.click(&mut m, slot, button, mode::QUICK_CRAFT);
    }
    b.click(&mut m, 0, 0, mode::QUICK_MOVE);
    b.click(&mut m, 0, 0, mode::PICKUP);
}

fn wheat_offer() -> Offer {
    Offer {
        cost_a: Cost { id: "minecraft:wheat".into(), count: 20, components: None },
        cost_b: None,
        result: Stack { id: "minecraft:emerald".into(), count: 1, components: None },
        uses: 0,
        max_uses: 3,
        reward_exp: true,
        special: 0,
        demand: 0,
        mult: 0.05,
        xp: 2,
    }
}

/// Trading: payment placed by hand, result taken by a click and by a shift-click, trade selected.
fn merchant_clicks(b: &mut Bench<'_>) {
    let mut m = Menu::merchant(3, Trade { mob: 9, offers: vec![wheat_offer()], ..Trade::default() });
    b.inv[31] = plain("wheat", 64);
    m.send_all(&b.inv);
    b.record(&mut m);
    let wheat_slot = 31 - 9 + 3;
    for (slot, button, mode) in [(wheat_slot, 0, mode::PICKUP), (0, 0, mode::PICKUP), (2, 0, mode::PICKUP), (-999, 0, mode::PICKUP)] {
        b.click(&mut m, slot, button, mode);
    }
    b.click(&mut m, 2, 0, mode::QUICK_MOVE);
    m.select_trade(&mut b.env, &mut b.inv, 0);
    b.sync(&mut m);
    b.trace.push(format!("trade {:?}", m.merchant.as_ref().map(|t| (&t.offers[0].uses, &t.traded, &t.updated, t.active))));
}

/// Mount equipment: a saddle and an armour that fit, a sword that does not.
fn mount_clicks(b: &mut Bench<'_>) {
    let info = MountInfo { name: "minecraft:horse".into(), tags: vec!["minecraft:can_equip_saddle".into()], active: [true, true] };
    let mut m = Menu::mount(4, 77, info, vec![None, None]);
    b.inv[32] = plain("saddle", 1);
    b.inv[33] = plain("diamond_horse_armor", 1);
    b.inv[34] = plain("diamond_sword", 1);
    m.send_all(&b.inv);
    b.record(&mut m);
    for k in [32, 33, 34] {
        let menu_slot = (k - 9 + 2) as i16;
        b.click(&mut m, menu_slot, 0, mode::QUICK_MOVE);
        b.click(&mut m, menu_slot, 0, mode::PICKUP);
        b.click(&mut m, 0, 0, mode::PICKUP);
        b.click(&mut m, 1, 0, mode::PICKUP);
    }
}

/// The whole scenario under `switches`, in this thread.
fn menu_trace(w: &'static World, switches: MenuSwitches) -> Vec<String> {
    FORCED_SWITCHES.with(|f| f.set(Some(switches)));
    let env = HashingEnv { inner: TestEnv { w, creative: false, dropped: Vec::new(), crafted: 0 } };
    let mut b = Bench { env, inv: vec![None; crate::player::MENU_SLOTS], trace: Vec::new() };
    inventory_clicks(&mut b);
    container_clicks(&mut b);
    crafting_clicks(&mut b);
    merchant_clicks(&mut b);
    mount_clicks(&mut b);
    FORCED_SWITCHES.with(|f| f.set(None));
    b.trace
}

#[test]
fn menu_switches_keep_every_packet() {
    let w = world().expect("26.3 version data and the player's jar are needed for this proof");
    let reference = menu_trace(w, MenuSwitches::ALL_OFF);
    assert!(reference.len() > 100, "scenario too short: {} lines", reference.len());
    let fingerprint = digest(&reference);
    for switches in all_combinations() {
        let trace = menu_trace(w, switches);
        let first_difference = reference.iter().zip(&trace).position(|(a, b)| a != b);
        assert!(
            first_difference.is_none() && trace.len() == reference.len(),
            "{switches:?} differs at line {first_difference:?}:\n old {:?}\n new {:?}",
            first_difference.map(|k| &reference[k]),
            first_difference.map(|k| &trace[k]),
        );
    }
    assert_eq!(digest(&menu_trace(w, MenuSwitches::ALL_ON)), fingerprint);
    assert_eq!(fingerprint, MENU_TRACE_FINGERPRINT, "menu trace fingerprint {fingerprint:#018x}");
}
