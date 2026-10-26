//! Container menus on the client side (`docs/specs/inventories.md` § 2, § 5): slot
//! layout, acceptance rules and local prediction of clicks (same logic as the server).

use rc_proto_v777::item::ItemStack;

/// Menu type (`menu` registry, § 1.1) reduced to the handled screens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Player inventory (window 0).
    Player,
    /// Chest of 1 to 6 rows.
    Chest(u8),
    /// Dispenser / dropper.
    Dispenser,
    Hopper,
    Crafting,
    /// Furnace (0), blast furnace (1), smoker (2).
    Furnace(u8),
    Brewing,
    /// "Slots only" screen: container slots before the 36 player slots.
    Generic,
    /// Special screens (`menu_special.rs`).
    Special(crate::menu_special::Sp),
}

impl Kind {
    /// `menu` registry type → screen.
    pub fn from_registry(id: i32) -> Kind {
        if let Some(s) = crate::menu_special::from_registry(id) {
            return Kind::Special(s);
        }
        match id {
            0..=5 => Kind::Chest(id as u8 + 1),
            6 => Kind::Dispenser,
            10 => Kind::Furnace(1),
            11 => Kind::Brewing,
            12 => Kind::Crafting,
            14 => Kind::Furnace(0),
            16 => Kind::Hopper,
            22 => Kind::Furnace(2),
            _ => Kind::Generic,
        }
    }

    /// Number of slots specific to the container (before the inventory), `None` if unknown (generic).
    pub fn container_size(self) -> Option<usize> {
        Some(match self {
            Kind::Player => 9,
            Kind::Chest(n) => 9 * n as usize,
            Kind::Dispenser => 9,
            Kind::Hopper => 5,
            Kind::Crafting => 10,
            Kind::Furnace(_) => 3,
            Kind::Brewing => 5,
            Kind::Generic => return None,
            Kind::Special(s) => crate::menu_special::container_size(s),
        })
    }

    /// Total number of slots of a new menu (0 if unknown).
    pub fn total_size(self) -> usize {
        match self {
            Kind::Special(s) => crate::menu_special::total(s),
            _ => self.container_size().map_or(0, |c| c + 36),
        }
    }
}

/// What a slot accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accept {
    Any,
    /// Result slot: receives nothing.
    Nothing,
    /// Armor slot: equipment slot (`ItemInfo::equip_slot`).
    Armor(u8),
    Fuel,
    Potion,
    BrewFuel,
    /// Rule specific to a special screen (`menu_special::accept`).
    Special(u8),
}

#[derive(Debug, Clone, Copy)]
pub struct SlotDef {
    pub x: i32,
    pub y: i32,
    pub max: i32,
    pub accept: Accept,
    /// Crafting result slot: taking it consumes the grid.
    pub craft_result: bool,
    pub empty_sprite: Option<&'static str>,
}

impl SlotDef {
    const fn at(x: i32, y: i32) -> SlotDef {
        SlotDef { x, y, max: 64, accept: Accept::Any, craft_result: false, empty_sprite: None }
    }
}

/// Standard inventory (§ 5): slot `p` (0..36) of the player part, top `t`.
fn standard(p: usize, t: i32) -> SlotDef {
    let p = p as i32;
    if p < 27 { SlotDef::at(8 + 18 * (p % 9), t + 18 * (p / 9)) } else { SlotDef::at(8 + 18 * (p - 27), t + 58) }
}

/// Size of the screen's image.
pub fn image_size(kind: Kind, n: usize) -> (i32, i32) {
    match kind {
        Kind::Chest(r) => (176, 114 + 18 * i32::from(r)),
        Kind::Hopper => (176, 133),
        Kind::Generic => (176, 114 + 18 * generic_rows(n)),
        Kind::Special(s) => crate::menu_special::image_size(s),
        _ => (176, 166),
    }
}

/// Rows displayed by the generic screen (6 at most).
pub fn generic_rows(n: usize) -> i32 {
    (n.saturating_sub(36).div_ceil(9)).clamp(1, 6) as i32
}

/// Slot `i` of a menu of `n` slots.
pub fn slot_def(kind: Kind, i: usize, n: usize) -> SlotDef {
    let c = kind.container_size().unwrap_or_else(|| n.saturating_sub(36));
    match kind {
        Kind::Player => match i {
            0 => SlotDef { accept: Accept::Nothing, craft_result: true, ..SlotDef::at(154, 28) },
            1..=4 => SlotDef::at(98 + 18 * ((i as i32 - 1) % 2), 18 + 18 * ((i as i32 - 1) / 2)),
            5..=8 => {
                const SPRITES: [&str; 4] = [
                    "container/slot/helmet",
                    "container/slot/chestplate",
                    "container/slot/leggings",
                    "container/slot/boots",
                ];
                SlotDef {
                    max: 1,
                    accept: Accept::Armor(10 - i as u8),
                    empty_sprite: Some(SPRITES[i - 5]),
                    ..SlotDef::at(8, 8 + 18 * (i as i32 - 5))
                }
            }
            45 => SlotDef { empty_sprite: Some("container/slot/shield"), ..SlotDef::at(77, 62) },
            _ => standard(i - 9, 84),
        },
        Kind::Special(s) => crate::menu_special::slot_def(s, i, n),
        _ if i >= c => {
            let top = match kind {
                Kind::Chest(r) => 31 + 18 * i32::from(r),
                Kind::Generic => 31 + 18 * generic_rows(n),
                Kind::Hopper => 51,
                _ => 84,
            };
            standard(i - c, top)
        }
        Kind::Chest(_) | Kind::Generic => SlotDef::at(8 + 18 * (i as i32 % 9), 18 + 18 * (i as i32 / 9)),
        Kind::Dispenser => SlotDef::at(62 + 18 * (i as i32 % 3), 17 + 18 * (i as i32 / 3)),
        Kind::Hopper => SlotDef::at(44 + 18 * i as i32, 20),
        Kind::Crafting => match i {
            0 => SlotDef { accept: Accept::Nothing, craft_result: true, ..SlotDef::at(124, 35) },
            _ => SlotDef::at(30 + 18 * ((i as i32 - 1) % 3), 17 + 18 * ((i as i32 - 1) / 3)),
        },
        Kind::Furnace(_) => match i {
            0 => SlotDef::at(56, 17),
            1 => SlotDef { accept: Accept::Fuel, ..SlotDef::at(56, 53) },
            _ => SlotDef { accept: Accept::Nothing, ..SlotDef::at(116, 35) },
        },
        Kind::Brewing => match i {
            0..=2 => SlotDef {
                max: 1,
                accept: Accept::Potion,
                empty_sprite: Some("container/slot/potion"),
                ..SlotDef::at([56, 79, 102][i], [51, 58, 51][i])
            },
            3 => SlotDef::at(79, 17),
            _ => SlotDef {
                accept: Accept::BrewFuel,
                empty_sprite: Some("container/slot/brewing_fuel"),
                ..SlotDef::at(17, 17)
            },
        },
    }
}

/// What the prediction must know about items.
pub trait Rules {
    fn max_stack(&self, s: &ItemStack) -> i32;
    /// Equipment slot (`ItemInfo::equip_slot`), 0 if none.
    fn equip_slot(&self, s: &ItemStack) -> u8;
    fn is_fuel(&self, s: &ItemStack) -> bool;
    fn smeltable(&self, s: &ItemStack) -> bool;
    fn is_potion(&self, s: &ItemStack) -> bool;
    fn brew_fuel(&self, s: &ItemStack) -> bool;
    fn brew_ingredient(&self, s: &ItemStack) -> bool;
    /// Item name without namespace (special screens).
    fn item_name(&self, _s: &ItemStack) -> &str {
        ""
    }
    /// Maximum durability (0: none).
    fn max_damage(&self, _s: &ItemStack) -> u32 {
        0
    }
    /// Network id of the `enchantments` component.
    fn enchantments_component(&self) -> Option<u16> {
        None
    }
    /// Network id of the `map_id` component.
    fn map_id_component(&self) -> Option<u16> {
        None
    }
    /// Bundle data (click prediction, `bundle.rs`); `None`: no bundles.
    fn bundles(&self) -> Option<crate::bundle::Ctx<'_>> {
        None
    }
}

/// Same stack: same item and same components.
pub fn same(a: &ItemStack, b: &ItemStack) -> bool {
    a.item == b.item && a.added == b.added && a.removed == b.removed
}

fn with_count(s: &ItemStack, n: i32) -> ItemStack {
    ItemStack { count: n, ..s.clone() }
}

/// State of a menu for the prediction: slots and carried item.
pub struct Menu<'a> {
    pub kind: Kind,
    pub slots: &'a mut [Option<ItemStack>],
    pub carried: &'a mut Option<ItemStack>,
    pub rules: &'a dyn Rules,
    pub creative: bool,
}

impl Menu<'_> {
    fn def(&self, i: usize) -> SlotDef {
        slot_def(self.kind, i, self.slots.len())
    }

    fn c(&self) -> usize {
        self.kind.container_size().unwrap_or_else(|| self.slots.len().saturating_sub(36))
    }

    fn may_place(&self, i: usize, s: &ItemStack) -> bool {
        match self.def(i).accept {
            Accept::Any => true,
            Accept::Nothing => false,
            Accept::Armor(e) => self.rules.equip_slot(s) == e,
            Accept::Fuel => self.rules.is_fuel(s),
            Accept::Potion => self.rules.is_potion(s),
            Accept::BrewFuel => self.rules.brew_fuel(s),
            Accept::Special(code) => crate::menu_special::accepts(code, self.rules, s),
        }
    }

    fn may_pickup(&self, i: usize) -> bool {
        !matches!(self.kind, Kind::Special(s) if crate::menu_special::locked(s, i))
    }

    /// Max size of the slot for this stack.
    fn max_case(&self, i: usize, s: &ItemStack) -> i32 {
        self.def(i).max.min(self.rules.max_stack(s))
    }

    /// Safe insertion of at most `n` items of `p`; returns the remainder.
    fn safe_insert(&mut self, i: usize, p: ItemStack, n: i32) -> Option<ItemStack> {
        if !self.may_place(i, &p) {
            return Some(p);
        }
        let have = self.slots[i].as_ref().map_or(0, |c| c.count);
        if self.slots[i].as_ref().is_some_and(|c| !same(c, &p)) {
            return Some(p);
        }
        let t = n.min(p.count).min(self.max_case(i, &p) - have);
        if t <= 0 {
            return Some(p);
        }
        self.slots[i] = Some(with_count(&p, have + t));
        (p.count > t).then(|| with_count(&p, p.count - t))
    }

    /// Removal of `a` items, cap `m` (§ 2.1).
    fn try_remove(&mut self, i: usize, a: i32, m: i32) -> Option<ItemStack> {
        if !self.may_pickup(i) {
            return None;
        }
        let c = self.slots[i].clone()?;
        let modifiable = self.def(i).accept != Accept::Nothing;
        if !modifiable && m < c.count {
            return None;
        }
        let t = a.min(m).min(c.count);
        if t <= 0 {
            return None;
        }
        self.slots[i] = (c.count > t).then(|| with_count(&c, c.count - t));
        if self.def(i).craft_result {
            self.consume_grid();
        }
        Some(with_count(&c, t))
    }

    /// Taking the crafting result: one item from each grid slot.
    fn consume_grid(&mut self) {
        let grid = match self.kind {
            Kind::Player => 1..5,
            Kind::Crafting => 1..10,
            _ => return,
        };
        for g in grid {
            if let Some(s) = &mut self.slots[g] {
                s.count -= 1;
                if s.count <= 0 {
                    self.slots[g] = None;
                }
            }
        }
    }

    /// Click of mode `mode` (§ 2), local prediction.
    pub fn click(&mut self, slot: i16, button: i8, mode: i32, hotbar: &mut dyn FnMut(i8) -> Option<usize>) {
        match mode {
            0 => self.pickup(slot, button),
            1 if slot >= 0 => {
                let i = slot as usize;
                if i >= self.slots.len() || !self.may_pickup(i) {
                    return;
                }
                while let Some(before) = self.slots[i].clone() {
                    if !self.quick_move(i) || !self.slots[i].as_ref().is_some_and(|s| s.item == before.item) {
                        break;
                    }
                }
            }
            1 => self.pickup(slot, button),
            2 => {
                if let Some(t) = hotbar(button)
                    && slot >= 0
                {
                    self.swap(slot as usize, t);
                }
            }
            3 => {
                if self.creative
                    && self.carried.is_none()
                    && let Some(s) = usize::try_from(slot).ok().and_then(|i| self.slots.get(i)?.clone())
                {
                    let max = self.rules.max_stack(&s);
                    *self.carried = Some(with_count(&s, max));
                }
            }
            4 if self.carried.is_none() && slot >= 0 => {
                let i = slot as usize;
                if i >= self.slots.len() {
                    return;
                }
                if button == 0 {
                    self.try_remove(i, 1, i32::MAX);
                } else if let Some(first) = self.slots[i].clone() {
                    while let Some(c) = self.slots[i].clone() {
                        if c.item != first.item || self.try_remove(i, c.count, i32::MAX).is_none() {
                            break;
                        }
                    }
                }
            }
            6 => self.pickup_all(slot, button),
            _ => {}
        }
    }

    fn pickup(&mut self, slot: i16, button: i8) {
        if slot == -999 {
            if let Some(p) = self.carried.take()
                && button == 1
                && p.count > 1
            {
                *self.carried = Some(with_count(&p, p.count - 1));
            }
            return;
        }
        let Some(i) = usize::try_from(slot).ok().filter(|&i| i < self.slots.len()) else { return };
        if self.bundle_click(i, button) {
            return;
        }
        let n = if button == 0 { i32::MAX } else { 1 };
        match (self.slots[i].clone(), self.carried.clone()) {
            (None, Some(p)) => *self.carried = self.safe_insert(i, p, n),
            (None, None) => {}
            (Some(c), None) => {
                let a = if button == 0 { c.count } else { (c.count + 1) / 2 };
                *self.carried = self.try_remove(i, a, i32::MAX);
            }
            (Some(c), Some(p)) => {
                if !self.may_pickup(i) {
                    return;
                }
                if self.may_place(i, &p) {
                    if same(&c, &p) {
                        *self.carried = self.safe_insert(i, p, n);
                    } else if p.count <= self.max_case(i, &p) {
                        self.slots[i] = Some(p);
                        *self.carried = Some(c);
                    }
                } else if same(&c, &p) {
                    let room = self.rules.max_stack(&p) - p.count;
                    if let Some(t) = self.try_remove(i, c.count, room) {
                        *self.carried = Some(with_count(&p, p.count + t.count));
                    }
                }
            }
        }
    }

    /// Bundle behaviors before the normal click (carried bundle placed on a slot, then slot
    /// containing a bundle); true if the click is consumed. Sounds to play: `bundle::sound`.
    fn bundle_click(&mut self, i: usize, button: i8) -> bool {
        use crate::bundle::{INSERT, INSERT_FAIL, REMOVE_ONE, sound};
        let Some(b) = self.rules.bundles() else { return false };
        let primary = button == 0;
        // Carried bundle, placed on the slot.
        if let Some(car) = self.carried.clone()
            && let Some(mut items) = b.bundle(&car)
        {
            let other = self.slots[i].clone();
            if primary && let Some(o) = other {
                let room = b.room(b.total(&items), b.weight(&o));
                let taken = if b.fits(&o) && room > 0 { self.try_remove(i, o.count, room) } else { None };
                let moved = taken.map_or(0, |mut t| b.try_insert(&mut items, &mut t));
                sound(if moved > 0 { INSERT } else { INSERT_FAIL }, if moved > 0 { 0.8 } else { 1.0 }, moved > 0);
                *self.carried = Some(b.with_contents(&car, &items));
                return true;
            }
            if !primary && other.is_none() {
                if let Some(st) = b.remove_one(&mut items, -1) {
                    match self.safe_insert(i, st.clone(), st.count) {
                        Some(mut rest) if rest.count > 0 => {
                            b.try_insert(&mut items, &mut rest);
                        }
                        _ => sound(REMOVE_ONE, 0.8, true),
                    }
                }
                *self.carried = Some(b.with_contents(&car, &items));
                return true;
            }
        }
        // Bundle in the slot, carried item (or nothing).
        let Some(cur) = self.slots[i].clone() else { return false };
        let Some(mut items) = b.bundle(&cur) else { return false };
        let allow = self.may_pickup(i) && self.may_place(i, &cur);
        match self.carried.clone() {
            None if primary => {
                crate::bundle::set_selected(i, -1);
                false
            }
            Some(mut o) if primary => {
                if allow && b.try_insert(&mut items, &mut o) > 0 {
                    sound(INSERT, 0.8, true);
                    *self.carried = (o.count > 0).then_some(o);
                } else {
                    sound(INSERT_FAIL, 1.0, false);
                }
                self.slots[i] = Some(b.with_contents(&cur, &items));
                true
            }
            None => {
                if allow && let Some(st) = b.remove_one(&mut items, crate::bundle::selected(i)) {
                    sound(REMOVE_ONE, 0.8, true);
                    *self.carried = Some(st);
                    crate::bundle::set_selected(i, -1);
                }
                self.slots[i] = Some(b.with_contents(&cur, &items));
                true
            }
            Some(_) => {
                crate::bundle::set_selected(i, -1);
                false
            }
        }
    }

    /// Mode 2: swap with slot `t` of the menu (hotbar or offhand).
    fn swap(&mut self, i: usize, t: usize) {
        if i >= self.slots.len() || t >= self.slots.len() {
            return;
        }
        match (self.slots[t].clone(), self.slots[i].clone()) {
            (None, None) => {}
            (None, Some(c)) => {
                if self.may_pickup(i) {
                    self.slots[t] = Some(c);
                    self.slots[i] = None;
                    if self.def(i).craft_result {
                        self.consume_grid();
                    }
                }
            }
            (Some(s), None) => {
                if self.may_place(i, &s) {
                    let m = self.max_case(i, &s);
                    if s.count > m {
                        self.slots[i] = Some(with_count(&s, m));
                        self.slots[t] = Some(with_count(&s, s.count - m));
                    } else {
                        self.slots[i] = Some(s);
                        self.slots[t] = None;
                    }
                }
            }
            (Some(s), Some(c)) => {
                if self.may_pickup(i) && self.may_place(i, &s) {
                    let m = self.max_case(i, &s);
                    if s.count > m {
                        // The slot's stack goes elsewhere in the inventory: left to the server.
                        self.slots[i] = Some(with_count(&s, m));
                        self.slots[t] = Some(with_count(&s, s.count - m));
                    } else {
                        self.slots[i] = Some(s);
                        self.slots[t] = Some(c);
                    }
                }
            }
        }
    }

    /// Move to `[start, end)` (§ 2.3); true if something moved.
    pub(crate) fn move_to(&mut self, stack: &mut ItemStack, start: usize, end: usize, reverse: bool) -> bool {
        let end = end.min(self.slots.len());
        if start >= end {
            return false;
        }
        let mut moved = false;
        let order = |k: usize| if reverse { end - 1 - k } else { start + k };
        if self.rules.max_stack(stack) > 1 {
            for k in 0..end - start {
                if stack.count <= 0 {
                    break;
                }
                let j = order(k);
                let Some(c) = self.slots[j].clone() else { continue };
                if !same(&c, stack) {
                    continue;
                }
                let max = self.max_case(j, stack);
                let total = c.count + stack.count;
                if total <= max {
                    stack.count = 0;
                    self.slots[j] = Some(with_count(&c, total));
                    moved = true;
                } else if c.count < max {
                    stack.count -= max - c.count;
                    self.slots[j] = Some(with_count(&c, max));
                    moved = true;
                }
            }
        }
        if stack.count > 0 {
            for k in 0..end - start {
                let j = order(k);
                if self.slots[j].is_none() && self.may_place(j, stack) {
                    let m = self.max_case(j, stack);
                    let t = stack.count.min(m);
                    self.slots[j] = Some(with_count(stack, t));
                    stack.count -= t;
                    moved = true;
                    break;
                }
            }
        }
        moved
    }

    /// Quick move of a slot according to the menu (§ 5); false if nothing moved.
    fn quick_move(&mut self, i: usize) -> bool {
        let Some(orig) = self.slots[i].clone() else { return false };
        let mut s = orig.clone();
        let n = self.slots.len();
        let c = self.c();
        let r = self.rules;
        let ok = match self.kind {
            Kind::Player => match i {
                0 => self.move_to(&mut s, 9, 45, true),
                1..=8 => self.move_to(&mut s, 9, 45, false),
                _ => {
                    let e = r.equip_slot(&s);
                    if (2..=5).contains(&e) && self.slots[10 - e as usize].is_none() {
                        let a = 10 - e as usize;
                        self.move_to(&mut s, a, a + 1, false)
                    } else if e == 6 && self.slots[45].is_none() {
                        self.move_to(&mut s, 45, 46, false)
                    } else if (9..36).contains(&i) {
                        self.move_to(&mut s, 36, 45, false)
                    } else if (36..45).contains(&i) {
                        self.move_to(&mut s, 9, 36, false)
                    } else {
                        self.move_to(&mut s, 9, 45, false)
                    }
                }
            },
            Kind::Chest(_) | Kind::Generic | Kind::Dispenser | Kind::Hopper => {
                if i < c {
                    self.move_to(&mut s, c, n, true)
                } else {
                    self.move_to(&mut s, 0, c, false)
                }
            }
            Kind::Crafting => match i {
                0 => self.move_to(&mut s, 10, 46, true),
                1..=9 => self.move_to(&mut s, 10, 46, false),
                _ => {
                    self.move_to(&mut s, 1, 10, false)
                        || if i < 37 {
                            self.move_to(&mut s, 37, 46, false)
                        } else {
                            self.move_to(&mut s, 10, 37, false)
                        }
                }
            },
            Kind::Furnace(_) => match i {
                2 => self.move_to(&mut s, 3, 39, true),
                0 | 1 => self.move_to(&mut s, 3, 39, false),
                _ if r.smeltable(&s) => self.move_to(&mut s, 0, 1, false),
                _ if r.is_fuel(&s) => self.move_to(&mut s, 1, 2, false),
                _ if i < 30 => self.move_to(&mut s, 30, 39, false),
                _ => self.move_to(&mut s, 3, 30, false),
            },
            Kind::Brewing => match i {
                0..=4 => self.move_to(&mut s, 5, 41, true),
                _ if r.brew_fuel(&s) => {
                    self.move_to(&mut s, 4, 5, false) || r.brew_ingredient(&s) && self.move_to(&mut s, 3, 4, false)
                }
                _ if r.brew_ingredient(&s) => self.move_to(&mut s, 3, 4, false),
                _ if r.is_potion(&s) => self.move_to(&mut s, 0, 3, false),
                _ if i < 32 => self.move_to(&mut s, 32, 41, false),
                _ => self.move_to(&mut s, 5, 32, false),
            },
            Kind::Special(sp) => crate::menu_special::quick_move(self, sp, i, &mut s),
        };
        if !ok || s.count == orig.count {
            return false;
        }
        self.slots[i] = (s.count > 0).then_some(s);
        if self.def(i).craft_result {
            self.consume_grid();
        }
        true
    }

    /// Mode 6: gathering (§ 2.8).
    fn pickup_all(&mut self, slot: i16, button: i8) {
        let Some(i) = usize::try_from(slot).ok().filter(|&i| i < self.slots.len()) else { return };
        let Some(mut p) = self.carried.clone() else { return };
        if self.slots[i].is_some() && self.may_pickup(i) {
            return;
        }
        let max = self.rules.max_stack(&p);
        let n = self.slots.len();
        for pass in 0..2 {
            for k in 0..n {
                if p.count >= max {
                    break;
                }
                let j = if button == 0 { k } else { n - 1 - k };
                let Some(c) = self.slots[j].clone() else { continue };
                let d = self.def(j);
                let no_gather = matches!(self.kind, Kind::Special(s) if crate::menu_special::no_gather(s, j));
                if !same(&c, &p)
                    || d.craft_result
                    || no_gather
                    || !self.may_pickup(j)
                    || d.accept == Accept::Nothing && self.kind != Kind::Generic
                {
                    continue;
                }
                if pass == 0 && c.count == max {
                    continue;
                }
                let t = c.count.min(max - p.count);
                self.slots[j] = (c.count > t).then(|| with_count(&c, c.count - t));
                p.count += t;
            }
        }
        *self.carried = Some(p);
    }

    /// Slot acceptable for a drag (§ 2.7) with `taken` slots already retained.
    pub fn drag_accepts(&self, i: usize, kind: u8, taken: usize, strict: bool) -> bool {
        let Some(p) = self.carried.as_ref() else { return false };
        let enough = kind == 2 || if strict { p.count as usize > taken } else { p.count as usize >= taken };
        let fits = match &self.slots[i] {
            None => true,
            Some(c) => same(c, p) && c.count < self.max_case(i, p),
        };
        enough && fits && self.may_place(i, p)
    }

    /// Planned quantity of a retained slot and remainder of the carried stack, for `selected` slots.
    pub fn drag_share(&self, i: usize, kind: u8, cases: usize) -> Option<(i32, bool)> {
        let p = self.carried.as_ref()?;
        let before = self.slots[i].as_ref().map_or(0, |c| c.count);
        let max = self.rules.max_stack(p);
        let part = match kind {
            0 => p.count / cases.max(1) as i32,
            1 => 1,
            _ => max,
        };
        let cap = max.min(self.max_case(i, p));
        let want = part + before;
        Some((want.min(cap), want > cap))
    }

    /// End of a drag (phase 2) over the retained slots.
    pub fn drag_end(&mut self, cases: &[usize], kind: u8) {
        if cases.len() == 1 {
            self.pickup(cases[0] as i16, kind as i8);
            return;
        }
        let Some(p) = self.carried.clone() else { return };
        let mut rest = p.count;
        let valid: Vec<usize> =
            cases.iter().copied().filter(|&i| self.drag_accepts(i, kind, cases.len(), false)).collect();
        for &i in &valid {
            let Some((n, _)) = self.drag_share(i, kind, cases.len()) else { break };
            let before = self.slots[i].as_ref().map_or(0, |c| c.count);
            self.slots[i] = Some(with_count(&p, n));
            rest -= n - before;
        }
        *self.carried = (rest > 0).then(|| with_count(&p, rest));
    }
}

/// Slots modified by a prediction (§ 1.6).
pub fn changed(before: &[Option<ItemStack>], after: &[Option<ItemStack>]) -> Vec<usize> {
    before.iter().zip(after).enumerate().filter(|(_, (a, b))| a != b).map(|(i, _)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct R;
    impl Rules for R {
        fn max_stack(&self, s: &ItemStack) -> i32 {
            if s.item == 99 { 1 } else { 64 }
        }
        fn equip_slot(&self, s: &ItemStack) -> u8 {
            if s.item == 50 { 5 } else { 0 }
        }
        fn is_fuel(&self, s: &ItemStack) -> bool {
            s.item == 2
        }
        fn smeltable(&self, s: &ItemStack) -> bool {
            s.item == 3
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
        fn map_id_component(&self) -> Option<u16> {
            Some(7)
        }
    }

    /// Cartography table map slot: any stack that carries `map_id`, whatever its
    /// name; an exploration map still without a number is refused, like the server.
    #[test]
    fn cartography_map_slot_needs_map_id() {
        use crate::menu_special::{accept::MAP, accepts};
        let mut s = st(5, 1).expect("stack");
        assert!(!accepts(MAP, &R, &s), "stack without map number accepted");
        s.added.push((7, Box::new([3])));
        assert!(accepts(MAP, &R, &s), "stack with map number refused");
    }

    fn st(item: i32, count: i32) -> Option<ItemStack> {
        Some(ItemStack { item, count, added: vec![], removed: vec![] })
    }

    fn run(kind: Kind, slots: &mut Vec<Option<ItemStack>>, carried: &mut Option<ItemStack>, s: i16, b: i8, m: i32) {
        let mut menu = Menu { kind, slots, carried, rules: &R, creative: false };
        let mut hot = |b: i8| (0..9).contains(&b).then(|| 36 + b as usize);
        menu.click(s, b, m, &mut hot);
    }

    #[test]
    fn left_and_right_pickup() {
        let mut slots = vec![None; 46];
        slots[10] = st(1, 7);
        let mut carried = None;
        run(Kind::Player, &mut slots, &mut carried, 10, 1, 0);
        assert_eq!((carried.as_ref().unwrap().count, slots[10].as_ref().unwrap().count), (4, 3));
        // Right click on an empty slot: a single item dropped.
        run(Kind::Player, &mut slots, &mut carried, 11, 1, 0);
        assert_eq!((carried.as_ref().unwrap().count, slots[11].as_ref().unwrap().count), (3, 1));
        // Left click on the same stack: everything merges.
        run(Kind::Player, &mut slots, &mut carried, 10, 0, 0);
        assert_eq!((carried.clone(), slots[10].as_ref().unwrap().count), (None, 6));
    }

    #[test]
    fn swap_different_items_and_drop_outside() {
        let mut slots = vec![None; 46];
        slots[9] = st(1, 5);
        let mut carried = st(2, 3);
        run(Kind::Player, &mut slots, &mut carried, 9, 0, 0);
        assert_eq!((slots[9].as_ref().unwrap().item, carried.as_ref().unwrap().item), (2, 1));
        run(Kind::Player, &mut slots, &mut carried, -999, 1, 0);
        assert_eq!(carried.as_ref().unwrap().count, 4);
        run(Kind::Player, &mut slots, &mut carried, -999, 0, 0);
        assert!(carried.is_none());
    }

    #[test]
    fn result_slot_refuses_and_consumes_grid() {
        let mut slots = vec![None; 46];
        slots[0] = st(5, 4);
        slots[1] = st(1, 2);
        slots[2] = st(1, 1);
        let mut carried = st(6, 1);
        run(Kind::Player, &mut slots, &mut carried, 0, 0, 0);
        assert_eq!(slots[0].as_ref().unwrap().item, 5, "result refused: no swap");
        let mut carried = None;
        run(Kind::Player, &mut slots, &mut carried, 0, 0, 0);
        assert_eq!(carried.as_ref().unwrap().count, 4);
        assert_eq!((slots[1].as_ref().unwrap().count, slots[2].clone()), (1, None));
    }

    #[test]
    fn shift_click_player_and_chest() {
        let mut slots = vec![None; 46];
        slots[10] = st(1, 40);
        slots[36] = st(1, 60);
        let mut carried = None;
        run(Kind::Player, &mut slots, &mut carried, 10, 0, 1);
        // Storage → hotbar: completes the stack of 60 then the first empty slot.
        assert_eq!(slots[36].as_ref().unwrap().count, 64);
        assert_eq!(slots[37].as_ref().unwrap().count, 36);
        assert!(slots[10].is_none());
        // Helmet → head armor slot (5).
        slots[12] = st(50, 1);
        run(Kind::Player, &mut slots, &mut carried, 12, 0, 1);
        assert_eq!(slots[5].as_ref().unwrap().item, 50);
        // Chest of 3 rows: chest slot → inventory in reverse order (hotbar first).
        let mut chest = vec![None; 27 + 36];
        chest[0] = st(1, 10);
        run(Kind::Chest(3), &mut chest, &mut carried, 0, 0, 1);
        assert_eq!(chest[62].as_ref().unwrap().count, 10);
        run(Kind::Chest(3), &mut chest, &mut carried, 62, 0, 1);
        assert_eq!(chest[0].as_ref().unwrap().count, 10);
    }

    #[test]
    fn furnace_shift_click_routes_by_kind() {
        let mut slots = vec![None; 39];
        slots[5] = st(3, 8);
        slots[6] = st(2, 8);
        slots[7] = st(1, 8);
        let mut carried = None;
        run(Kind::Furnace(0), &mut slots, &mut carried, 5, 0, 1);
        run(Kind::Furnace(0), &mut slots, &mut carried, 6, 0, 1);
        run(Kind::Furnace(0), &mut slots, &mut carried, 7, 0, 1);
        assert_eq!(slots[0].as_ref().unwrap().item, 3);
        assert_eq!(slots[1].as_ref().unwrap().item, 2);
        assert_eq!(slots[30].as_ref().unwrap().item, 1);
    }

    #[test]
    fn hotbar_swap_throw_and_gather() {
        let mut slots = vec![None; 46];
        slots[20] = st(1, 5);
        let mut carried = None;
        run(Kind::Player, &mut slots, &mut carried, 20, 2, 2);
        assert_eq!((slots[20].clone(), slots[38].as_ref().unwrap().count), (None, 5));
        run(Kind::Player, &mut slots, &mut carried, 38, 0, 4);
        assert_eq!(slots[38].as_ref().unwrap().count, 4);
        run(Kind::Player, &mut slots, &mut carried, 38, 1, 4);
        assert!(slots[38].is_none());
        // Gathering: the carried stack picks up the identical stacks.
        slots[9] = st(1, 64);
        slots[15] = st(1, 3);
        slots[30] = st(1, 10);
        let mut carried = st(1, 2);
        run(Kind::Player, &mut slots, &mut carried, 12, 0, 6);
        // First pass: non-full stacks (3 + 10), then the full one completes up to 64.
        assert_eq!(carried.as_ref().unwrap().count, 64);
        assert_eq!((slots[15].clone(), slots[30].clone()), (None, None));
        assert_eq!(slots[9].as_ref().unwrap().count, 15);
    }

    #[test]
    fn drag_splits_evenly() {
        let mut slots = vec![None; 46];
        let mut carried = st(1, 10);
        let mut m = Menu { kind: Kind::Player, slots: &mut slots, carried: &mut carried, rules: &R, creative: false };
        m.drag_end(&[9, 10, 11], 0);
        assert_eq!(slots[9].as_ref().unwrap().count, 3);
        assert_eq!(slots[11].as_ref().unwrap().count, 3);
        assert_eq!(carried.as_ref().unwrap().count, 1);
    }

    #[test]
    fn changed_lists_differences() {
        let a = vec![st(1, 1), None, st(2, 2)];
        let b = vec![st(1, 1), st(3, 1), st(2, 1)];
        assert_eq!(changed(&a, &b), vec![1, 2]);
    }
}
