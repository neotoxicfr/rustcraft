//! Server-side menus (`server-gameplay.md` § 4.1-4.2, `inventories.md` § 2): slots (player inventory
//! or the menu's own contents), the game's click algorithm (take, quick move,
//! swap, clone, throw, drag, gather), remote copy per slot and state number.
//! The menu only knows the rest of the server through `Env` (recipes, thrown items, experience).

use rc_proto_v777::container::HashedStack;

use crate::items::{Items, matches, same};
use crate::player::Stack;

/// Where the contents of a slot come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Src {
    /// Slot of menu 0 (player inventory, layout 0..46 of `Server::slots`).
    Inv(usize),
    /// Contents of the menu itself (chest, crafting grid, furnace…).
    Own(usize),
}

/// Particular rules of a slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Plain,
    /// Crafting result: receives nothing, empties by a block, consumes the grid.
    CraftResult,
    /// Armor: equipment slot (1 feet … 4 head), size 1.
    Armor(u8),
    /// Furnace fuel (or empty bucket, alone).
    Fuel,
    /// Furnace result: receives nothing, gives back experience.
    FurnaceResult,
    /// Shulker box: refuses Shulker boxes.
    Shulker,
    /// S7: input slot of a station (brewing stand, enchanting table, anvil…), local index.
    Station(u8),
    /// S7: result of a station (anvil, grindstone, smithing table, stonecutter).
    StationResult,
    /// F04: mount equipment (7 saddle, 6 body), size 1.
    MountEquip(u8),
    /// Result of a trade (F05): receives nothing, empties by a block, consumes the payment.
    MerchantResult,
    /// G6-F01: lectern book, which can be taken but receives nothing.
    TakeOnly,
    /// G5-F11: beacon payment (`beacon_payment_items` items, a single one).
    BeaconPayment,
    /// G5-F11: slot `k` of the crafter grid, disabled when data `k` is 1.
    CrafterGrid(u8),
    /// G5-F11: result shown by the crafter: neither taken nor placed.
    NoInteract,
}

/// F04: mount of a mount inventory menu: name, entity type tags, active
/// saddle and body slots.
#[derive(Debug, Clone, Default)]
pub struct MountInfo {
    pub name: String,
    pub tags: Vec<String>,
    pub active: [bool; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotDef {
    pub src: Src,
    pub role: Role,
}

/// Menu type (`menu` registry).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Player inventory (window 0).
    Inventory,
    /// 3×3 crafting table.
    Crafting,
    /// Chests and similar: network type, number of container slots.
    Container { menu_type: i32, size: usize },
    /// Furnace (0), blast furnace (1), smoker (2).
    Furnace(u8),
    /// S7: stations (`stations.rs`).
    Station(crate::stations::St),
    /// F04: mount inventory (entity, chest slots), opened by `mount_screen_open`.
    Mount { entity: i32, size: usize },
    /// Trading (F05, `menus_merchant.rs`).
    Merchant,
    /// G5-F11: beacon (payment, levels and effects as data).
    Beacon,
    /// G5-F11: crafter (block entity grid, disabled slots and signal as data).
    Crafter,
}

/// What the client is supposed to have in a slot.
#[derive(Debug, Clone, PartialEq)]
pub enum Remote {
    Exact(Option<Stack>),
    Hashed(Option<HashedStack>),
}

/// Menu packet to send (stacks in disk form, translated by the server).
#[derive(Debug, Clone, PartialEq)]
pub enum Out {
    /// `container_set_slot`.
    Slot { state: i32, slot: i16, stack: Option<Stack> },
    /// `container_set_content`.
    Content { state: i32, slots: Vec<Option<Stack>>, carried: Option<Stack> },
    /// `set_cursor_item`.
    Cursor(Option<Stack>),
    /// `container_set_data`.
    Data { prop: i16, value: i16 },
}

/// What the menu asks of the server.
pub trait Env {
    fn items(&self) -> &Items;
    /// Infinite materials (creative).
    fn creative(&self) -> bool;
    /// Held slot of the hotbar (0..9).
    fn selected(&self) -> usize;
    /// Item thrown by the player (`thrown`: thrown in front of them).
    fn drop_item(&mut self, s: Stack, thrown: bool);
    /// Result of the square grid (`w`×`w`), `None` without a recipe.
    fn craft(&mut self, grid: &[Option<Stack>], w: usize) -> Option<Stack>;
    /// Leftovers of the recipe per grid slot.
    fn remaining(&mut self, grid: &[Option<Stack>], w: usize) -> Vec<Option<Stack>>;
    /// Crafting result taken (statistics, recipe learned).
    fn crafted(&mut self, result: &Stack, count: i32);
    /// Furnace result taken: experience of the cooked recipes.
    fn smelted(&mut self, count: i32);
    /// S9: result taken ("crafted" statistic).
    fn made(&mut self, _s: &Stack, _n: i32) {}
    /// S9: grid at the time of crafting (ingredients of the criteria).
    fn craft_grid(&mut self, _g: &[Option<Stack>]) {}
    /// S9: bottle taken from a brewing stand (`brewed_potion` trigger).
    fn brewed(&mut self, _s: &Stack) {}
    fn is_fuel(&self, s: &Stack) -> bool;
    fn can_smelt(&self, s: &Stack, kind: u8) -> bool;
    /// Network fingerprint of a stack.
    fn hashed(&self, s: Option<&Stack>) -> Option<HashedStack>;
    /// S7: station data (brewing, smithing, stonecutter recipes; enchantments).
    fn st_data(&self) -> Option<&crate::stations::Data> {
        None
    }
    /// S7: experience level of the player.
    fn xp_level(&self) -> i32 {
        0
    }
    /// S7: effect of a station to be applied by the server (levels, sound, anvil wear…).
    fn station_event(&mut self, _ev: crate::stations::Ev) {}
    /// S7: integer `[0, bound)` drawn from the level's random draw.
    fn level_int(&mut self, _bound: i32) -> i32 {
        0
    }
    /// S8: known map (locked, scale) for the cartography table.
    fn map_info(&self, _id: i32) -> Option<(bool, u8)> {
        None
    }
    /// G5-F11: result shown by the crafter for its 3×3 grid (without the book rule).
    fn crafter_result(&mut self, _grid: &[Option<Stack>]) -> Option<Stack> {
        None
    }
}

pub struct Menu {
    pub window: i32,
    pub kind: Kind,
    pub slots: Vec<SlotDef>,
    pub own: Vec<Option<Stack>>,
    pub carried: Option<Stack>,
    pub state: i32,
    remote: Vec<Remote>,
    remote_carried: Remote,
    /// Menu properties (`container_set_data`).
    pub data: Vec<i32>,
    remote_data: Vec<i32>,
    qc_status: i32,
    qc_type: i32,
    qc_slots: Vec<usize>,
    /// Stack chosen with the wheel in a bundle: (menu slot, index).
    pub(crate) bundle_sel: Option<(usize, i32)>,
    /// Crafting grid: first slot and side; slot 0 is the result.
    pub grid: Option<(usize, usize)>,
    remove_count: i32,
    /// Open block (validity of the menu); second half of a double chest.
    pub block: Option<[i32; 3]>,
    pub block2: Option<[i32; 3]>,
    /// Ender chest (contents in the player file).
    pub ender: bool,
    /// Packets to send, in order.
    pub out: Vec<Out>,
    /// Own contents modified since the last copy back to their owner.
    pub own_dirty: bool,
    /// S7: own state of a station (enchantment seed, anvil name…).
    pub st: crate::stations::State,
    /// F04: mount of the mount inventory menu.
    pub mount: Option<MountInfo>,
    /// Trading: offers and trade state (F05).
    pub merchant: Option<Box<merchant::Trade>>,
    /// G5-F06: open minecart with chest or hopper (network identifier).
    pub cart: Option<i32>,
}

/// Player inventory (inventory indices: 0..36 main inventory and hotbar, 36..40 armor from feet
/// to head, 40 off hand) → slot of menu 0.
pub fn inv_to_menu0(i: usize) -> usize {
    match i {
        0..=8 => 36 + i,
        9..=35 => i,
        36..=39 => 8 - (i - 36),
        _ => 45,
    }
}

/// Inventory slot (0..36, 40) whose stack can still receive `s`
/// (held hand, off hand, then 0..36).
pub(crate) fn slot_with_space(inv: &[Option<Stack>], items: &Items, selected: usize, s: &Stack) -> Option<usize> {
    let room = |i: usize| {
        inv[inv_to_menu0(i)].as_ref().is_some_and(|t| {
            same(t, s) && items.max_stack(t) > 1 && items.damage(t) == 0 && t.count < items.max_stack(t)
        })
    };
    if room(selected) {
        return Some(selected);
    }
    if room(40) {
        return Some(40);
    }
    (0..36).find(|&i| room(i))
}

fn free_slot(inv: &[Option<Stack>]) -> Option<usize> {
    (0..36).find(|&i| inv[inv_to_menu0(i)].is_none())
}

/// Returns what did not fit.
fn add_resource(inv: &mut [Option<Stack>], items: &Items, slot: usize, s: &Stack) -> i32 {
    let k = inv_to_menu0(slot);
    let cur = inv[k].get_or_insert_with(|| Stack { count: 0, ..s.clone() });
    let add = s.count.min(items.max_stack(cur) - cur.count).max(0);
    cur.count += add;
    if cur.count <= 0 {
        inv[k] = None;
    }
    s.count - add
}

/// Storing in the inventory. Returns the stack only if NOTHING could be
/// stored (as in the game, a partial remainder is lost for the caller, which only throws in that case).
pub fn inventory_add(
    inv: &mut [Option<Stack>],
    items: &Items,
    selected: usize,
    mut s: Stack,
    creative: bool,
) -> Option<Stack> {
    if s.count <= 0 {
        return None;
    }
    if items.damage(&s) > 0 {
        return match free_slot(inv) {
            Some(i) => {
                inv[inv_to_menu0(i)] = Some(s);
                None
            }
            None if creative => None,
            None => Some(s),
        };
    }
    let first = s.count;
    loop {
        let last = s.count;
        let rest = match slot_with_space(inv, items, selected, &s).or_else(|| free_slot(inv)) {
            Some(slot) => add_resource(inv, items, slot, &s),
            None => s.count,
        };
        s.count = rest;
        if s.count <= 0 || s.count >= last {
            break;
        }
    }
    if s.count == first && !creative { Some(s) } else { None }
}

/// Given back to the player: stored, otherwise thrown.
pub fn place_back(inv: &mut [Option<Stack>], env: &mut dyn Env, mut s: Stack) {
    let selected = env.selected();
    while s.count > 0 {
        let items = env.items();
        let Some(slot) = slot_with_space(inv, items, selected, &s).or_else(|| free_slot(inv)) else {
            env.drop_item(s, false);
            return;
        };
        let k = inv_to_menu0(slot);
        let space = items.max_stack(&s) - inv[k].as_ref().map_or(0, |t| t.count);
        let n = space.min(s.count);
        if n <= 0 {
            env.drop_item(s, false);
            return;
        }
        let part = Stack { count: n, ..s.clone() };
        s.count -= n;
        let rest = add_resource(inv, items, slot, &part);
        s.count += rest;
        if rest == n {
            env.drop_item(s, false);
            return;
        }
    }
}

impl Menu {
    pub(crate) fn base(window: i32, kind: Kind, slots: Vec<SlotDef>, own: usize, data: usize) -> Menu {
        let n = slots.len();
        Menu {
            window,
            kind,
            slots,
            own: vec![None; own],
            carried: None,
            state: 0,
            remote: vec![Remote::Exact(None); n],
            remote_carried: Remote::Exact(None),
            data: vec![0; data],
            remote_data: vec![0; data],
            qc_status: 0,
            qc_type: -1,
            qc_slots: Vec::new(),
            bundle_sel: None,
            grid: None,
            remove_count: 0,
            block: None,
            block2: None,
            ender: false,
            out: Vec::new(),
            own_dirty: false,
            st: crate::stations::State::default(),
            mount: None,
            merchant: None,
            cart: None,
        }
    }

    /// F04: mount inventory: saddle, body, chest (3 × columns), player.
    pub fn mount(window: i32, entity: i32, info: MountInfo, own: Vec<Option<Stack>>) -> Menu {
        let size = own.len().saturating_sub(2);
        let mut slots = vec![
            SlotDef { src: Src::Own(0), role: Role::MountEquip(7) },
            SlotDef { src: Src::Own(1), role: Role::MountEquip(6) },
        ];
        slots.extend((2..2 + size).map(|k| SlotDef { src: Src::Own(k), role: Role::Plain }));
        Self::player_slots(&mut slots);
        let mut m = Menu::base(window, Kind::Mount { entity, size }, slots, size + 2, 0);
        m.own = own;
        m.mount = Some(info);
        m
    }

    /// F04: does the stack go into the mount's equipment slot?
    fn mount_accepts(&self, env: &dyn Env, slot: u8, s: &Stack) -> bool {
        let Some(info) = &self.mount else { return false };
        if !info.active[usize::from(slot == 6)] || env.items().equip_slot(s) != Some(slot) {
            return false;
        }
        let allowed: Vec<String> = match crate::items::component(s, "minecraft:equippable") {
            Some(e) => match e.get("allowed_entities") {
                Some(rc_proto::nbt::Nbt::String(t)) => vec![t.clone()],
                Some(rc_proto::nbt::Nbt::List(l)) => l.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect(),
                _ => return true,
            },
            None => {
                match env.items().default_component(s, "minecraft:equippable").map(|v| v["allowed_entities"].clone()) {
                    Some(serde_json::Value::String(t)) => vec![t],
                    Some(serde_json::Value::Array(a)) => {
                        a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()
                    }
                    _ => return true,
                }
            }
        };
        allowed.iter().any(|a| match a.strip_prefix('#') {
            Some(t) => info.tags.iter().any(|x| x == t),
            None => *a == info.name,
        })
    }

    pub(crate) fn player_slots(slots: &mut Vec<SlotDef>) {
        slots.extend((9..36).map(|k| SlotDef { src: Src::Inv(k), role: Role::Plain }));
        slots.extend((36..45).map(|k| SlotDef { src: Src::Inv(k), role: Role::Plain }));
    }

    /// Player inventory: result, 2×2 grid, armor (head → feet), main inventory, hotbar, off
    /// hand; everything is in `Server::slots`.
    pub fn inventory() -> Menu {
        let mut slots = vec![SlotDef { src: Src::Inv(0), role: Role::CraftResult }];
        slots.extend((1..5).map(|k| SlotDef { src: Src::Inv(k), role: Role::Plain }));
        slots.extend((5..9).map(|k| SlotDef { src: Src::Inv(k), role: Role::Armor((9 - k) as u8) }));
        Self::player_slots(&mut slots);
        slots.push(SlotDef { src: Src::Inv(45), role: Role::Plain });
        let mut m = Menu::base(0, Kind::Inventory, slots, 0, 0);
        m.grid = Some((1, 2));
        m
    }

    pub fn crafting(window: i32) -> Menu {
        let mut slots = vec![SlotDef { src: Src::Own(0), role: Role::CraftResult }];
        slots.extend((1..10).map(|k| SlotDef { src: Src::Own(k), role: Role::Plain }));
        Self::player_slots(&mut slots);
        let mut m = Menu::base(window, Kind::Crafting, slots, 10, 0);
        m.grid = Some((1, 3));
        m
    }

    /// Simple container (chest, barrel, dispenser, hopper, Shulker box).
    pub fn container(window: i32, menu_type: i32, contents: Vec<Option<Stack>>, shulker: bool) -> Menu {
        let size = contents.len();
        let role = if shulker { Role::Shulker } else { Role::Plain };
        let mut slots: Vec<SlotDef> = (0..size).map(|k| SlotDef { src: Src::Own(k), role }).collect();
        Self::player_slots(&mut slots);
        let mut m = Menu::base(window, Kind::Container { menu_type, size }, slots, size, 0);
        m.own = contents;
        m
    }

    pub fn furnace(window: i32, kind: u8, contents: Vec<Option<Stack>>) -> Menu {
        let mut slots = vec![
            SlotDef { src: Src::Own(0), role: Role::Plain },
            SlotDef { src: Src::Own(1), role: Role::Fuel },
            SlotDef { src: Src::Own(2), role: Role::FurnaceResult },
        ];
        Self::player_slots(&mut slots);
        let mut m = Menu::base(window, Kind::Furnace(kind), slots, 3, 4);
        m.own = contents;
        m.own.resize(3, None);
        m
    }

    /// G5-F11: beacon: payment slot then player; 3 data (levels, primary and
    /// secondary effects).
    pub fn beacon(window: i32) -> Menu {
        let mut slots = vec![SlotDef { src: Src::Own(0), role: Role::BeaconPayment }];
        Self::player_slots(&mut slots);
        Menu::base(window, Kind::Beacon, slots, 1, 3)
    }

    /// G5-F11: crafter: 3×3 grid, player, then the shown result (own contents 9);
    /// 10 data (disabled slots, signal received).
    pub fn crafter(window: i32, grid: Vec<Option<Stack>>) -> Menu {
        let mut slots: Vec<SlotDef> =
            (0..9).map(|k| SlotDef { src: Src::Own(k), role: Role::CrafterGrid(k as u8) }).collect();
        Self::player_slots(&mut slots);
        slots.push(SlotDef { src: Src::Own(9), role: Role::NoInteract });
        let mut m = Menu::base(window, Kind::Crafter, slots, 10, 10);
        for (o, g) in m.own.iter_mut().zip(grid) {
            *o = g;
        }
        m
    }

    /// G5-F11: crafter result recomputed from its grid.
    pub(crate) fn crafter_refresh(&mut self, env: &mut dyn Env) {
        if self.kind == Kind::Crafter && self.own.len() == 10 {
            self.own[9] = env.crafter_result(&self.own[..9]);
        }
    }

    /// Network type of the menu (`menu` registry).
    pub fn menu_type(&self) -> i32 {
        match self.kind {
            Kind::Inventory => -1,
            Kind::Crafting => 12,
            Kind::Container { menu_type, .. } => menu_type,
            Kind::Furnace(0) => 14,
            Kind::Furnace(1) => 10,
            Kind::Furnace(_) => 22,
            Kind::Station(st) => crate::stations::menu_type(st),
            Kind::Mount { .. } => -1,
            Kind::Merchant => 19,
            Kind::Beacon => crate::beacon::MENU_TYPE,
            Kind::Crafter => 7, // `crafter_3x3`
        }
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    pub fn get<'a>(&'a self, inv: &'a [Option<Stack>], i: usize) -> Option<&'a Stack> {
        match self.slots[i].src {
            Src::Inv(k) => inv[k].as_ref(),
            Src::Own(k) => self.own[k].as_ref(),
        }
    }

    fn get_mut<'a>(&'a mut self, inv: &'a mut [Option<Stack>], i: usize) -> &'a mut Option<Stack> {
        match self.slots[i].src {
            Src::Inv(k) => &mut inv[k],
            Src::Own(k) => {
                self.own_dirty = true;
                &mut self.own[k]
            }
        }
    }

    pub(crate) fn set(&mut self, inv: &mut [Option<Stack>], i: usize, s: Option<Stack>) {
        *self.get_mut(inv, i) = s.filter(|s| s.count > 0);
    }

    fn max_slot(&self, items: &Items, i: usize, s: &Stack) -> i32 {
        let m = items.max_stack(s);
        match self.slots[i].role {
            Role::Armor(_) | Role::MountEquip(_) | Role::BeaconPayment => 1,
            Role::Fuel if s.id == "minecraft:bucket" => 1,
            Role::Station(k) => crate::stations::max_slot(self, k).unwrap_or_else(|| m.min(99)),
            _ => m.min(99),
        }
    }

    pub(crate) fn may_place(&self, env: &dyn Env, i: usize, s: &Stack) -> bool {
        match self.slots[i].role {
            Role::CraftResult | Role::FurnaceResult | Role::MerchantResult | Role::TakeOnly | Role::NoInteract => false,
            Role::BeaconPayment => env.items().has_tag("minecraft:beacon_payment_items", &s.id),
            Role::CrafterGrid(k) => self.data.get(usize::from(k)) != Some(&1),
            Role::Armor(slot) => env.items().equip_slot(s) == Some(slot),
            Role::Fuel => env.is_fuel(s) || s.id == "minecraft:bucket",
            Role::Shulker => !s.id.ends_with("shulker_box"),
            Role::Station(k) => crate::stations::may_place(self, env, k, s),
            Role::StationResult => false,
            Role::Plain => true,
            Role::MountEquip(slot) => self.mount_accepts(env, slot, s),
        }
    }

    fn may_pickup(&self, env: &dyn Env, inv: &[Option<Stack>], i: usize) -> bool {
        if self.slots[i].role == Role::NoInteract {
            return false;
        }
        if self.slots[i].role == Role::MerchantResult {
            return self.merchant_may_pickup(env);
        }
        if let Role::Armor(_) | Role::MountEquip(_) = self.slots[i].role
            && let Some(s) = self.get(inv, i)
            && !env.creative()
            && env.items().enchantment(s, "minecraft:binding_curse") > 0
        {
            return false;
        }
        if self.slots[i].role == Role::StationResult {
            return crate::stations::may_pickup(self, env);
        }
        true
    }

    /// Raw removal from a slot: the crafting result slot empties by a block.
    fn remove(&mut self, inv: &mut [Option<Stack>], i: usize, amount: i32) -> Option<Stack> {
        let role = self.slots[i].role;
        let have = self.get(inv, i)?.count;
        let n = if matches!(role, Role::CraftResult | Role::MerchantResult) { have } else { amount.min(have) };
        if matches!(role, Role::CraftResult | Role::FurnaceResult | Role::MerchantResult) {
            self.remove_count += amount.min(have);
        }
        if n <= 0 {
            return None;
        }
        let slot = self.get_mut(inv, i);
        let cur = slot.as_mut()?;
        let out = Stack { count: n, ..cur.clone() };
        cur.count -= n;
        if cur.count <= 0 {
            *slot = None;
        }
        Some(out)
    }

    /// Refused if the slot cannot be taken, or is not modifiable and the ceiling is too low.
    fn try_remove(
        &mut self,
        env: &mut dyn Env,
        inv: &mut [Option<Stack>],
        i: usize,
        amount: i32,
        max: i32,
    ) -> Option<Stack> {
        if !self.may_pickup(env, inv, i) {
            return None;
        }
        let count = self.get(inv, i).map_or(0, |s| s.count);
        let modifiable = self.get(inv, i).is_none_or(|s| self.may_place(env, i, s));
        if !modifiable && max < count {
            return None;
        }
        self.remove(inv, i, amount.min(max))
    }

    fn safe_take(
        &mut self,
        env: &mut dyn Env,
        inv: &mut [Option<Stack>],
        i: usize,
        amount: i32,
        max: i32,
    ) -> Option<Stack> {
        let r = self.try_remove(env, inv, i, amount, max)?;
        self.on_take(env, inv, i, &r);
        Some(r)
    }

    /// Returns the rest of the stack.
    fn safe_insert(
        &mut self,
        env: &mut dyn Env,
        inv: &mut [Option<Stack>],
        i: usize,
        mut s: Stack,
        amount: i32,
    ) -> Option<Stack> {
        if s.count <= 0 || !self.may_place(env, i, &s) {
            return Some(s).filter(|s| s.count > 0);
        }
        let cur = self.get(inv, i).map_or(0, |c| c.count);
        let n = amount.min(s.count).min(self.max_slot(env.items(), i, &s) - cur);
        if n <= 0 {
            return Some(s);
        }
        match self.get_mut(inv, i) {
            slot @ None => {
                *slot = Some(Stack { count: n, ..s.clone() });
                s.count -= n;
            }
            Some(c) if same(c, &s) => {
                c.count += n;
                s.count -= n;
            }
            _ => {}
        }
        Some(s).filter(|s| s.count > 0)
    }

    /// Crafting result (grid consumed, leftovers) or furnace result (experience).
    fn on_take(&mut self, env: &mut dyn Env, inv: &mut [Option<Stack>], i: usize, taken: &Stack) {
        match self.slots[i].role {
            Role::CraftResult => {
                self.check_take(env, taken);
                let Some((first, w)) = self.grid else { return };
                let grid: Vec<Option<Stack>> = (first..first + w * w).map(|j| self.get(inv, j).cloned()).collect();
                env.craft_grid(&grid); // S9
                let remaining = env.remaining(&grid, w);
                for (n, rem) in remaining.into_iter().enumerate() {
                    let j = first + n;
                    if self.get(inv, j).is_some() {
                        let _ = self.remove(inv, j, 1);
                    }
                    let Some(mut rem) = rem else { continue };
                    let cur = self.get(inv, j).cloned();
                    match cur {
                        None => self.set(inv, j, Some(rem)),
                        Some(c) if same(&c, &rem) => {
                            rem.count += c.count;
                            self.set(inv, j, Some(rem));
                        }
                        Some(_) => {
                            let creative = env.creative();
                            let selected = env.selected();
                            if let Some(rest) = inventory_add(inv, env.items(), selected, rem, creative) {
                                env.drop_item(rest, false);
                            }
                        }
                    }
                }
                self.update_result(env, inv);
            }
            Role::FurnaceResult => self.check_take(env, taken),
            Role::StationResult => crate::stations::on_take(self, env, inv, taken),
            Role::Station(0..=2) if self.kind == Kind::Station(crate::stations::St::Brewing) => env.brewed(taken), // S9
            Role::MerchantResult => {
                self.check_take(env, taken);
                self.merchant_on_take(env);
            }
            _ => {}
        }
    }

    fn check_take(&mut self, env: &mut dyn Env, taken: &Stack) {
        if self.remove_count > 0 {
            env.made(taken, self.remove_count); // S9
        }
        match self.kind {
            Kind::Furnace(_) => env.smelted(self.remove_count),
            Kind::Merchant => {}
            _ if self.remove_count > 0 => env.crafted(taken, self.remove_count),
            _ => {}
        }
        self.remove_count = 0;
    }

    /// Items passed by the quick move from a result.
    fn on_quick_craft(&mut self, env: &mut dyn Env, i: usize, picked: &Stack, before: i32) {
        let n = before - picked.count;
        if n > 0 && matches!(self.slots[i].role, Role::CraftResult | Role::FurnaceResult | Role::MerchantResult) {
            self.remove_count += n;
            let taken = Stack { count: n, ..picked.clone() };
            self.check_take(env, &taken);
        }
    }

    /// Result recomputed from the grid: placed and sent.
    pub fn update_result(&mut self, env: &mut dyn Env, inv: &mut [Option<Stack>]) {
        let Some((first, w)) = self.grid else { return };
        let grid: Vec<Option<Stack>> = (first..first + w * w).map(|j| self.get(inv, j).cloned()).collect();
        let result = env.craft(&grid, w);
        self.set(inv, 0, result.clone());
        self.remote[0] = Remote::Exact(result.clone());
        let state = self.next_state();
        self.out.push(Out::Slot { state, slot: 0, stack: result });
    }

    fn next_state(&mut self) -> i32 {
        self.state = (self.state + 1) & 0x7FFF;
        self.state
    }
}

// --- Clicks ---

/// Modes of `container_click`.
pub mod mode {
    pub const PICKUP: i32 = 0;
    pub const QUICK_MOVE: i32 = 1;
    pub const SWAP: i32 = 2;
    pub const CLONE: i32 = 3;
    pub const THROW: i32 = 4;
    pub const QUICK_CRAFT: i32 = 5;
    pub const PICKUP_ALL: i32 = 6;
}

/// Slot able to receive the stack while dragging.
fn quick_replace(items: &Items, cur: Option<&Stack>, s: &Stack, ignore_size: bool) -> bool {
    match cur {
        None => true,
        Some(c) if same(c, s) => c.count + if ignore_size { 0 } else { s.count } <= items.max_stack(s),
        Some(_) => false,
    }
}

impl Menu {
    fn reset_quick_craft(&mut self) {
        self.qc_status = 0;
        self.qc_slots.clear();
    }

    /// Validated click (`container_click` already filtered by window and bounds).
    pub fn click(&mut self, env: &mut dyn Env, inv: &mut [Option<Stack>], slot: i16, button: i8, mode: i32) {
        let n = self.len();
        let idx = usize::try_from(slot).ok().filter(|&i| i < n);
        let grid_before = self.grid_snapshot(inv);
        let own_before = matches!(self.kind, Kind::Station(_)).then(|| self.own.clone());
        let pay_before = (self.own.first().cloned().flatten(), self.own.get(1).cloned().flatten());
        let crafter_before = (self.kind == Kind::Crafter).then(|| self.own.clone());
        self.do_click(env, inv, slot, idx, i32::from(button), mode);
        // G5-F11: crafter grid changed → shown result recomputed.
        if crafter_before.is_some_and(|b| b[..9] != self.own[..9]) {
            self.crafter_refresh(env);
        }
        if self.kind == Kind::Merchant && (self.own[0] != pay_before.0 || self.own[1] != pay_before.1) {
            self.update_sell_item(env);
        }
        if self.grid.is_some() && self.grid_snapshot(inv) != grid_before {
            self.update_result(env, inv);
        }
        // S7: inputs of a station changed → result recomputed.
        if let Some(before) = own_before {
            crate::stations::after_click(self, env, inv, &before);
        }
    }

    fn grid_snapshot(&self, inv: &[Option<Stack>]) -> Vec<Option<Stack>> {
        match self.grid {
            Some((first, w)) => (first..first + w * w).map(|j| self.get(inv, j).cloned()).collect(),
            None => Vec::new(),
        }
    }

    fn do_click(
        &mut self,
        env: &mut dyn Env,
        inv: &mut [Option<Stack>],
        slot: i16,
        idx: Option<usize>,
        button: i32,
        mode: i32,
    ) {
        if mode == mode::QUICK_CRAFT {
            let expected = self.qc_status;
            self.qc_status = button & 3;
            if ((expected != 1 || self.qc_status != 2) && expected != self.qc_status) || self.carried.is_none() {
                self.reset_quick_craft();
            } else if self.qc_status == 0 {
                self.qc_type = (button >> 2) & 3;
                if self.qc_type == 0 || self.qc_type == 1 || (self.qc_type == 2 && env.creative()) {
                    self.qc_status = 1;
                    self.qc_slots.clear();
                } else {
                    self.reset_quick_craft();
                }
            } else if self.qc_status == 1 {
                let Some(i) = idx else { return };
                let Some(carried) = self.carried.clone() else { return };
                if quick_replace(env.items(), self.get(inv, i), &carried, true)
                    && self.may_place(env, i, &carried)
                    && (self.qc_type == 2 || carried.count > self.qc_slots.len() as i32)
                    && !self.qc_slots.contains(&i)
                {
                    self.qc_slots.push(i);
                }
            } else if self.qc_status == 2 {
                if !self.qc_slots.is_empty() {
                    if self.qc_slots.len() == 1 {
                        let only = self.qc_slots[0];
                        let ty = self.qc_type;
                        self.reset_quick_craft();
                        self.do_click(env, inv, only as i16, Some(only), ty, mode::PICKUP);
                        return;
                    }
                    let Some(source) = self.carried.clone() else {
                        self.reset_quick_craft();
                        return;
                    };
                    let mut remaining = source.count;
                    let n = self.qc_slots.len() as i32;
                    for i in self.qc_slots.clone() {
                        let Some(carried) = self.carried.clone() else { break };
                        if quick_replace(env.items(), self.get(inv, i), &carried, true)
                            && self.may_place(env, i, &carried)
                            && (self.qc_type == 2 || carried.count >= n)
                        {
                            let carry = self.get(inv, i).map_or(0, |s| s.count);
                            let max = env.items().max_stack(&source).min(self.max_slot(env.items(), i, &source));
                            let place = match self.qc_type {
                                0 => ((source.count as f32) / n as f32).floor() as i32,
                                1 => 1,
                                2 => env.items().max_stack(&source),
                                _ => source.count,
                            };
                            let new = (place + carry).min(max);
                            remaining -= new - carry;
                            self.set(inv, i, Some(Stack { count: new, ..source.clone() }));
                        }
                    }
                    self.carried = Some(Stack { count: remaining, ..source }).filter(|s| s.count > 0);
                }
                self.reset_quick_craft();
            } else {
                self.reset_quick_craft();
            }
            return;
        }
        if self.qc_status != 0 {
            self.reset_quick_craft();
            return;
        }
        match mode {
            mode::PICKUP | mode::QUICK_MOVE if button == 0 || button == 1 => {
                if slot == -999 {
                    if let Some(c) = self.carried.take() {
                        if button == 0 {
                            env.drop_item(c, true);
                        } else {
                            let one = Stack { count: 1, ..c.clone() };
                            self.carried = Some(Stack { count: c.count - 1, ..c }).filter(|s| s.count > 0);
                            env.drop_item(one, true);
                        }
                    }
                } else if mode == mode::QUICK_MOVE {
                    let Some(i) = idx else { return };
                    if !self.may_pickup(env, inv, i) {
                        return;
                    }
                    let mut clicked = self.quick_move(env, inv, i);
                    while let Some(c) = &clicked {
                        if self.get(inv, i).is_none_or(|s| s.id != c.id) {
                            break;
                        }
                        clicked = self.quick_move(env, inv, i);
                    }
                } else {
                    let Some(i) = idx else { return };
                    self.pickup(env, inv, i, button);
                }
            }
            mode::SWAP if (0..9).contains(&button) || button == 40 => {
                let Some(i) = idx else { return };
                let k = inv_to_menu0(button as usize);
                let source = inv[k].clone();
                let target = self.get(inv, i).cloned();
                match (source, target) {
                    (None, None) => {}
                    (None, Some(t)) => {
                        if self.may_pickup(env, inv, i) {
                            inv[k] = Some(t.clone());
                            if self.slots[i].role == Role::CraftResult {
                                self.remove_count += t.count;
                            }
                            self.set(inv, i, None);
                            self.on_take(env, inv, i, &t);
                        }
                    }
                    (Some(mut s), None) => {
                        if self.may_place(env, i, &s) {
                            let max = self.max_slot(env.items(), i, &s);
                            if s.count > max {
                                let part = Stack { count: max, ..s.clone() };
                                s.count -= max;
                                inv[k] = Some(s);
                                self.set(inv, i, Some(part));
                            } else {
                                inv[k] = None;
                                self.set(inv, i, Some(s));
                            }
                        }
                    }
                    (Some(mut s), Some(t)) => {
                        if self.may_pickup(env, inv, i) && self.may_place(env, i, &s) {
                            let max = self.max_slot(env.items(), i, &s);
                            if s.count > max {
                                let part = Stack { count: max, ..s.clone() };
                                s.count -= max;
                                inv[k] = Some(s);
                                self.set(inv, i, Some(part));
                                self.on_take(env, inv, i, &t);
                                let creative = env.creative();
                                let selected = env.selected();
                                if let Some(rest) = inventory_add(inv, env.items(), selected, t, creative) {
                                    env.drop_item(rest, true);
                                }
                            } else {
                                inv[k] = Some(t.clone());
                                self.set(inv, i, Some(s));
                                self.on_take(env, inv, i, &t);
                            }
                        }
                    }
                }
            }
            mode::CLONE if env.creative() && self.carried.is_none() => {
                let Some(i) = idx else { return };
                if let Some(s) = self.get(inv, i).cloned() {
                    let max = env.items().max_stack(&s);
                    self.carried = Some(Stack { count: max, ..s });
                }
            }
            mode::THROW if self.carried.is_none() => {
                let Some(i) = idx else { return };
                let amount = if button == 0 { 1 } else { self.get(inv, i).map_or(0, |s| s.count) };
                let Some(mut taken) = self.safe_take(env, inv, i, amount, i32::MAX) else { return };
                env.drop_item(taken.clone(), true);
                if button == 1 {
                    while self.get(inv, i).is_some_and(|s| s.id == taken.id) {
                        let amount = self.get(inv, i).map_or(0, |s| s.count);
                        match self.safe_take(env, inv, i, amount, i32::MAX) {
                            Some(t) => {
                                env.drop_item(t.clone(), true);
                                taken = t;
                            }
                            None => break,
                        }
                    }
                }
            }
            // The merchant gathers nothing (each slot is refused at gathering).
            mode::PICKUP_ALL if self.kind != Kind::Merchant => {
                let Some(i) = idx else { return };
                let Some(mut carried) = self.carried.clone() else { return };
                if self.get(inv, i).is_some() && self.may_pickup(env, inv, i) {
                    return;
                }
                let n = self.len();
                let max = env.items().max_stack(&carried);
                for pass in 0..2 {
                    let order: Vec<usize> = if button == 0 { (0..n).collect() } else { (0..n).rev().collect() };
                    for t in order {
                        if carried.count >= max {
                            break;
                        }
                        let Some(ts) = self.get(inv, t).cloned() else { continue };
                        if !quick_replace(env.items(), Some(&ts), &carried, true)
                            || !self.may_pickup(env, inv, t)
                            || self.slots[t].role == Role::CraftResult
                            || (self.slots[t].role == Role::StationResult && crate::stations::no_pick_all(self))
                        {
                            continue;
                        }
                        if (pass != 0 || ts.count != env.items().max_stack(&ts))
                            && let Some(r) = self.safe_take(env, inv, t, ts.count, max - carried.count)
                        {
                            carried.count += r.count;
                        }
                    }
                }
                self.carried = Some(carried);
            }
            _ => {}
        }
    }

    /// Mode 0 on a slot (§ 2.2).
    fn pickup(&mut self, env: &mut dyn Env, inv: &mut [Option<Stack>], i: usize, button: i32) {
        // W8c: bundle carried or clicked.
        if self.bundle_click(env, inv, i, button) {
            return;
        }
        let clicked = self.get(inv, i).cloned();
        let carried = self.carried.clone();
        match (clicked, carried) {
            (None, Some(c)) => {
                let amount = if button == 0 { c.count } else { 1 };
                self.carried = self.safe_insert(env, inv, i, c, amount);
            }
            (None, None) => {}
            (Some(clicked), carried) => {
                if !self.may_pickup(env, inv, i) {
                    return;
                }
                match carried {
                    None => {
                        let amount = if button == 0 { clicked.count } else { (clicked.count + 1) / 2 };
                        if let Some(taken) = self.try_remove(env, inv, i, amount, i32::MAX) {
                            self.carried = Some(taken.clone());
                            self.on_take(env, inv, i, &taken);
                        }
                    }
                    Some(c) if self.may_place(env, i, &c) => {
                        if same(&clicked, &c) {
                            let amount = if button == 0 { c.count } else { 1 };
                            self.carried = self.safe_insert(env, inv, i, c, amount);
                        } else if c.count <= self.max_slot(env.items(), i, &c) {
                            self.carried = Some(clicked);
                            self.set(inv, i, Some(c));
                        }
                    }
                    Some(mut c) if same(&clicked, &c) => {
                        let room = env.items().max_stack(&c) - c.count;
                        if let Some(taken) = self.try_remove(env, inv, i, clicked.count, room) {
                            c.count += taken.count;
                            self.carried = Some(c);
                            self.on_take(env, inv, i, &taken);
                        }
                    }
                    Some(_) => {}
                }
            }
        }
    }

    /// Stack `s` (taken from slot `from`) is poured into `[start, end)`.
    pub(crate) fn move_to(
        &mut self,
        env: &dyn Env,
        inv: &mut [Option<Stack>],
        s: &mut Stack,
        start: usize,
        end: usize,
        back: bool,
    ) -> bool {
        let mut changed = false;
        let items = env.items();
        let order: Vec<usize> = if back { (start..end).rev().collect() } else { (start..end).collect() };
        // Stackable: size > 1 and not damaged.
        if items.max_stack(s) > 1 && items.damage(s) == 0 {
            for &d in &order {
                if s.count <= 0 {
                    break;
                }
                let Some(t) = self.get(inv, d).cloned() else { continue };
                if !same(&t, s) {
                    continue;
                }
                let total = t.count + s.count;
                let max = self.max_slot(items, d, &t);
                if total <= max {
                    s.count = 0;
                    self.set(inv, d, Some(Stack { count: total, ..t }));
                    changed = true;
                } else if t.count < max {
                    s.count -= max - t.count;
                    self.set(inv, d, Some(Stack { count: max, ..t }));
                    changed = true;
                }
            }
        }
        if s.count > 0 {
            for &d in &order {
                if self.get(inv, d).is_none() && self.may_place(env, d, s) {
                    let max = self.max_slot(items, d, s);
                    let n = s.count.min(max);
                    self.set(inv, d, Some(Stack { count: n, ..s.clone() }));
                    s.count -= n;
                    changed = true;
                    break;
                }
            }
        }
        changed
    }

    /// The quick move of a menu stack: returns the original stack (copy) if something moved.
    fn quick_move(&mut self, env: &mut dyn Env, inv: &mut [Option<Stack>], i: usize) -> Option<Stack> {
        let mut stack = self.get(inv, i).cloned()?;
        let clicked = stack.clone();
        let n = self.len();
        let ok = match self.kind {
            Kind::Inventory => {
                let eq = env.items().equip_slot(&clicked);
                if i == 0 {
                    let ok = self.move_to(env, inv, &mut stack, 9, 45, true);
                    if ok {
                        self.on_quick_craft(env, i, &stack, clicked.count);
                    }
                    ok
                } else if (1..9).contains(&i) {
                    self.move_to(env, inv, &mut stack, 9, 45, false)
                } else if let Some(e @ 1..=4) = eq
                    && self.get(inv, 9 - e as usize).is_none()
                {
                    let pos = 9 - e as usize;
                    self.move_to(env, inv, &mut stack, pos, pos + 1, false)
                } else if eq == Some(5) && self.get(inv, 45).is_none() {
                    self.move_to(env, inv, &mut stack, 45, 46, false)
                } else if (9..36).contains(&i) {
                    self.move_to(env, inv, &mut stack, 36, 45, false)
                } else if (36..45).contains(&i) {
                    self.move_to(env, inv, &mut stack, 9, 36, false)
                } else {
                    self.move_to(env, inv, &mut stack, 9, 45, false)
                }
            }
            Kind::Crafting => {
                if i == 0 {
                    let ok = self.move_to(env, inv, &mut stack, 10, 46, true);
                    if ok {
                        self.on_quick_craft(env, i, &stack, clicked.count);
                    }
                    ok
                } else if (10..46).contains(&i) {
                    self.move_to(env, inv, &mut stack, 1, 10, false)
                        || if i < 37 {
                            self.move_to(env, inv, &mut stack, 37, 46, false)
                        } else {
                            self.move_to(env, inv, &mut stack, 10, 37, false)
                        }
                } else {
                    self.move_to(env, inv, &mut stack, 10, 46, false)
                }
            }
            Kind::Container { size, .. } => {
                if i < size {
                    self.move_to(env, inv, &mut stack, size, n, true)
                } else {
                    self.move_to(env, inv, &mut stack, 0, size, false)
                }
            }
            // F04: mount: body, then saddle, then chest; otherwise main inventory ↔ hotbar (nothing returned).
            Kind::Mount { size, .. } => {
                let start = 2 + size;
                if i < start {
                    self.move_to(env, inv, &mut stack, start, n, true)
                } else if self.may_place(env, 1, &stack) && self.get(inv, 1).is_none() {
                    self.move_to(env, inv, &mut stack, 1, 2, false)
                } else if self.may_place(env, 0, &stack) && self.get(inv, 0).is_none() {
                    self.move_to(env, inv, &mut stack, 0, 1, false)
                } else if size == 0 || !self.move_to(env, inv, &mut stack, 2, start, false) {
                    let (bag_end, bar_end) = (start + 27, start + 36);
                    if (bag_end..bar_end).contains(&i) {
                        self.move_to(env, inv, &mut stack, start, bag_end, false);
                    } else if (start..bag_end).contains(&i) {
                        self.move_to(env, inv, &mut stack, bag_end, bar_end, false);
                    } else {
                        self.move_to(env, inv, &mut stack, bag_end, bag_end, false);
                    }
                    let rest = (stack.count > 0).then(|| stack.clone());
                    self.set(inv, i, rest);
                    return None;
                } else {
                    true
                }
            }
            Kind::Merchant => {
                if i == 2 {
                    let ok = self.move_to(env, inv, &mut stack, 3, 39, true);
                    if ok {
                        self.on_quick_craft(env, i, &stack, clicked.count);
                        if let Some(t) = self.merchant.as_mut() {
                            t.trade_sound = true;
                        }
                    }
                    ok
                } else if i != 0 && i != 1 {
                    if (3..30).contains(&i) {
                        self.move_to(env, inv, &mut stack, 30, 39, false)
                    } else if (30..39).contains(&i) {
                        self.move_to(env, inv, &mut stack, 3, 30, false)
                    } else {
                        true
                    }
                } else {
                    self.move_to(env, inv, &mut stack, 3, 39, false)
                }
            }
            Kind::Furnace(kind) => {
                if i == 2 {
                    let ok = self.move_to(env, inv, &mut stack, 3, 39, true);
                    if ok {
                        self.on_quick_craft(env, i, &stack, clicked.count);
                    }
                    ok
                } else if i != 1 && i != 0 {
                    if env.can_smelt(&stack, kind) {
                        self.move_to(env, inv, &mut stack, 0, 1, false)
                    } else if env.is_fuel(&stack) {
                        self.move_to(env, inv, &mut stack, 1, 2, false)
                    } else if (3..30).contains(&i) {
                        self.move_to(env, inv, &mut stack, 30, 39, false)
                    } else if (30..39).contains(&i) {
                        self.move_to(env, inv, &mut stack, 3, 30, false)
                    } else {
                        true
                    }
                } else {
                    self.move_to(env, inv, &mut stack, 3, 39, false)
                }
            }
            Kind::Station(_) => crate::stations::quick_move(self, env, inv, i, &mut stack),
            // G5-F11: beacon: payment ↔ player; a payment item alone goes into the empty
            // slot; otherwise main inventory ↔ hotbar.
            Kind::Beacon => {
                if i == 0 {
                    self.move_to(env, inv, &mut stack, 1, 37, true)
                } else if self.get(inv, 0).is_none() && self.may_place(env, 0, &stack) && stack.count == 1 {
                    self.move_to(env, inv, &mut stack, 0, 1, false)
                } else if (1..28).contains(&i) {
                    self.move_to(env, inv, &mut stack, 28, 37, false)
                } else if (28..37).contains(&i) {
                    self.move_to(env, inv, &mut stack, 1, 28, false)
                } else {
                    self.move_to(env, inv, &mut stack, 1, 37, false)
                }
            }
            // G5-F11: crafter: grid ↔ player.
            Kind::Crafter => {
                if i < 9 {
                    self.move_to(env, inv, &mut stack, 9, 45, true)
                } else {
                    self.move_to(env, inv, &mut stack, 0, 9, false)
                }
            }
        };
        if !ok {
            return None;
        }
        // The origin slot keeps the rest (the crafting result is always emptied).
        let role = self.slots[i].role;
        let rest = (stack.count > 0).then(|| stack.clone());
        if matches!(self.kind, Kind::Container { .. } | Kind::Mount { .. }) {
            self.set(inv, i, rest);
            return Some(clicked);
        }
        if role == Role::CraftResult {
            self.set(inv, i, None);
        } else {
            self.set(inv, i, rest.clone());
        }
        if stack.count == clicked.count {
            return None;
        }
        let moved = Stack { count: clicked.count - stack.count, ..clicked.clone() };
        self.on_take(env, inv, i, &moved);
        if role == Role::CraftResult && stack.count > 0 {
            env.drop_item(stack, false);
        } else if role == Role::StationResult && stack.count > 0 && crate::stations::drops_rest(self) {
            // S7: stonecutter, rest of the quick move thrown.
            env.drop_item(stack, false);
        }
        Some(clicked)
    }

    // --- Synchronization (§ 4.2) ---

    /// Full contents.
    pub fn send_all(&mut self, inv: &[Option<Stack>]) {
        let slots: Vec<Option<Stack>> = (0..self.len()).map(|i| self.get(inv, i).cloned()).collect();
        for (r, s) in self.remote.iter_mut().zip(&slots) {
            *r = Remote::Exact(s.clone());
        }
        self.remote_carried = Remote::Exact(self.carried.clone());
        self.remote_data.clone_from(&self.data);
        let state = self.next_state();
        self.out.push(Out::Content { state, slots, carried: self.carried.clone() });
        for (i, v) in self.data.clone().into_iter().enumerate() {
            self.out.push(Out::Data { prop: i as i16, value: v as i16 });
        }
    }

    fn remote_matches(r: &mut Remote, cur: Option<&Stack>, env: &dyn Env) -> bool {
        match r {
            Remote::Exact(s) => matches(s.as_ref(), cur),
            Remote::Hashed(h) => {
                if *h == env.hashed(cur) {
                    *r = Remote::Exact(cur.cloned());
                    true
                } else {
                    false
                }
            }
        }
    }

    /// Differences sent.
    pub fn broadcast(&mut self, env: &dyn Env, inv: &[Option<Stack>]) {
        for i in 0..self.len() {
            let cur = self.get(inv, i).cloned();
            let mut r = std::mem::replace(&mut self.remote[i], Remote::Exact(None));
            if Self::remote_matches(&mut r, cur.as_ref(), env) {
                self.remote[i] = r;
            } else {
                self.remote[i] = Remote::Exact(cur.clone());
                let state = self.next_state();
                self.out.push(Out::Slot { state, slot: i as i16, stack: cur });
            }
        }
        let mut r = std::mem::replace(&mut self.remote_carried, Remote::Exact(None));
        if Self::remote_matches(&mut r, self.carried.as_ref(), env) {
            self.remote_carried = r;
        } else {
            self.remote_carried = Remote::Exact(self.carried.clone());
            self.out.push(Out::Cursor(self.carried.clone()));
        }
        for i in 0..self.data.len() {
            if self.remote_data[i] != self.data[i] {
                self.remote_data[i] = self.data[i];
                self.out.push(Out::Data { prop: i as i16, value: self.data[i] as i16 });
            }
        }
    }

    /// What the client announces after its click (the remote memory of the held item).
    pub fn set_remote(&mut self, slot: i16, h: Option<HashedStack>) {
        if let Some(r) = usize::try_from(slot).ok().and_then(|i| self.remote.get_mut(i)) {
            *r = Remote::Hashed(h);
        }
    }

    pub fn set_remote_carried(&mut self, h: Option<HashedStack>) {
        self.remote_carried = Remote::Hashed(h);
    }

    /// Remote copy of a slot (closing: taken over by menu 0, the state transfer).
    pub fn remote_of(&self, i: usize) -> &Remote {
        &self.remote[i]
    }

    pub fn set_remote_exact(&mut self, i: usize, r: Remote) {
        if let Some(x) = self.remote.get_mut(i) {
            *x = r;
        }
    }

    /// Closing (`removed`): carried item given back, crafting grid emptied into the inventory.
    pub fn close(&mut self, env: &mut dyn Env, inv: &mut [Option<Stack>]) {
        if let Some(c) = self.carried.take() {
            place_back(inv, env, c);
        }
        if let Some((first, w)) = self.grid {
            self.set(inv, 0, None);
            for j in first..first + w * w {
                if let Some(s) = self.get_mut(inv, j).take() {
                    place_back(inv, env, s);
                }
            }
        }
        if let Kind::Station(_) = self.kind {
            crate::stations::close(self, env, inv); // S7: inputs given back to the player
        }
        if self.kind == Kind::Merchant {
            self.merchant_close(env, inv);
        }
        // G5-F11: the payment left in the beacon drops at the player's feet.
        if self.kind == Kind::Beacon
            && let Some(s) = self.own.first_mut().and_then(Option::take)
        {
            self.own_dirty = true;
            env.drop_item(s, false);
        }
        self.reset_quick_craft();
    }
}

#[path = "menus_merchant.rs"]
pub mod merchant;

#[path = "menus_bundle.rs"]
pub(crate) mod bundle;

#[cfg(test)]
#[path = "menus_tests.rs"]
mod tests;
