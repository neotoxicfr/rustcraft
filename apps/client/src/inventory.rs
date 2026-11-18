//! Player inventory on the client side (`docs/specs/hud.md` § 6): menu 0 (46 slots), cursor
//! stack, "pop" counter of the hotbar slots; open container (`inventories.md` § 1).

use rc_proto_v777::item::{Inventory, ItemStack};

use crate::menu::Kind;

/// Slots of the inventory menu: 0 result, 1..4 crafting, 5..8 armor (head → feet),
/// 9..35 main inventory, 36..44 hotbar, 45 off-hand.
pub const MENU_SLOTS: usize = 46;

/// Container menu opened by the server.
pub struct Container {
    pub window: i32,
    pub kind: Kind,
    pub title: Option<rc_text::Component>,
    pub slots: Vec<Option<ItemStack>>,
    pub state: i32,
    /// Properties (`container_set_data`).
    pub props: [i32; 10],
    /// State of the special screens (offers, choices, scrolling…).
    pub extra: crate::special::Extra,
}

impl Container {
    /// Start of the player part (36 slots, at the end of the menu except for special screens).
    pub fn player_start(&self) -> usize {
        match self.kind {
            Kind::Special(s) => crate::menu_special::player_start(s, self.slots.len()),
            _ => self.slots.len().saturating_sub(36),
        }
    }

    /// Player part, if present (36 slots).
    fn player_part(&self) -> Option<std::ops::Range<usize>> {
        let p = self.player_start();
        (p + 36 <= self.slots.len()).then_some(p..p + 36)
    }
}

pub struct PlayerInventory {
    pub menu: Vec<Option<ItemStack>>,
    pub carried: Option<ItemStack>,
    pub state: i32,
    /// Pickup animation per hotbar slot (ticks left, § 3.2).
    pub pop: [i32; 9],
    pub container: Option<Container>,
}

impl Default for PlayerInventory {
    fn default() -> Self {
        PlayerInventory { menu: vec![None; MENU_SLOTS], carried: None, state: 0, pop: [0; 9], container: None }
    }
}

/// Inventory index (`set_player_inventory` packet) → menu slot.
fn menu_slot(inventory: i32) -> Option<usize> {
    Some(match inventory {
        0..=8 => 36 + inventory as usize,
        9..=35 => inventory as usize,
        36..=39 => (44 - inventory) as usize,
        40 => 45,
        _ => return None,
    })
}

impl PlayerInventory {
    pub fn apply(&mut self, inv: Inventory) {
        let open = self.container.as_ref().map(|c| c.window);
        match inv {
            Inventory::Content { container: 0, state, mut slots, carried } => {
                slots.resize(MENU_SLOTS, None);
                self.menu = slots;
                self.state = state;
                if open.is_none() {
                    self.carried = carried;
                }
                self.sync_to_container();
            }
            Inventory::Slot { container: 0, state, slot, stack } => {
                self.state = state;
                if let Some(i) = usize::try_from(slot).ok().filter(|&i| i < MENU_SLOTS) {
                    if (36..45).contains(&i)
                        && let Some(s) = &stack
                        && self.menu[i].as_ref().is_none_or(|old| s.count > old.count)
                    {
                        self.pop[i - 36] = 5;
                    }
                    self.menu[i] = stack;
                }
                self.sync_to_container();
            }
            Inventory::PlayerSlot { slot, stack } => {
                if let Some(i) = menu_slot(slot) {
                    self.menu[i] = stack;
                }
                self.sync_to_container();
            }
            Inventory::Cursor(stack) => self.carried = stack,
            Inventory::Content { container, state, slots, carried } if Some(container) == open => {
                if let Some(c) = &mut self.container {
                    c.slots = slots;
                    c.state = state;
                }
                self.carried = carried;
                self.sync_from_container();
            }
            Inventory::Slot { container, state, slot, stack } if Some(container) == open => {
                if let Some(c) = &mut self.container {
                    c.state = state;
                    if let Some(s) = usize::try_from(slot).ok().and_then(|i| c.slots.get_mut(i)) {
                        *s = stack;
                    }
                }
                self.sync_from_container();
            }
            Inventory::Content { .. } | Inventory::Slot { .. } => {}
        }
    }

    /// Player part of menu 0 → open container (they share the inventory).
    pub fn sync_to_container(&mut self) {
        if let Some(c) = &mut self.container
            && let Some(r) = c.player_part()
        {
            c.slots[r].clone_from_slice(&self.menu[9..45]);
        }
    }

    /// Player part of the open container → menu 0.
    pub fn sync_from_container(&mut self) {
        if let Some(c) = &self.container
            && let Some(r) = c.player_part()
        {
            self.menu[9..45].clone_from_slice(&c.slots[r]);
        }
    }

    /// `open_screen`: empty menu of the type, while waiting for the content.
    pub fn open(&mut self, window: i32, kind: Kind, title: Option<rc_text::Component>) {
        let n = kind.total_size();
        let extra = Default::default();
        self.container = Some(Container { window, kind, title, slots: vec![None; n], state: 0, props: [0; 10], extra });
        self.carried = None;
        self.sync_to_container();
    }

    pub fn set_data(&mut self, window: i32, prop: i16, value: i16) {
        if let Some(c) = &mut self.container
            && c.window == window
            && let Some(p) = c.props.get_mut(prop as usize)
        {
            *p = i32::from(value);
        }
    }

    pub fn hotbar(&self, i: usize) -> Option<&ItemStack> {
        self.menu.get(36 + i)?.as_ref()
    }

    pub fn offhand(&self) -> Option<&ItemStack> {
        self.menu[45].as_ref()
    }

    pub fn tick(&mut self) {
        for p in &mut self.pop {
            if *p > 0 {
                *p -= 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(item: i32) -> Option<ItemStack> {
        Some(ItemStack { item, count: 1, added: vec![], removed: vec![] })
    }

    #[test]
    fn container_and_player_menu_share_inventory() {
        let mut inv = PlayerInventory::default();
        inv.menu[36] = st(4);
        inv.open(3, Kind::Chest(3), None);
        assert_eq!(inv.container.as_ref().unwrap().slots[27 + 27], st(4));
        inv.apply(Inventory::Slot { container: 3, state: 7, slot: 27, stack: st(9) });
        assert_eq!(inv.menu[9], st(9));
        assert_eq!(inv.container.as_ref().unwrap().state, 7);
        // Another window is ignored.
        inv.apply(Inventory::Slot { container: 2, state: 1, slot: 0, stack: st(1) });
        assert_eq!(inv.container.as_ref().unwrap().slots[0], None);
    }
}
