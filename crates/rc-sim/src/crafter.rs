//! Crafter (G5-F11): block menu (3×3 grid of the block entity, shown result, disabled
//! slots and received signal as data) and packet `container_slot_state_changed`.
//!
//! Block entity data (the game's keys): `Items`, `disabled_slots` (disabled slots, read
//! only on an empty slot), `triggered` (signal received, without marking the chunk for rewrite).

use rc_proto::nbt::Nbt;

use crate::blocks::P;
use crate::containers::read_items;
use crate::menus::{Kind, Menu};
use crate::server::Server;

/// Disabled slots of a block entity (a full slot never is).
fn disabled(be: &Nbt) -> [bool; 9] {
    let items = read_items(be, 9);
    let mut d = [false; 9];
    if let Some(Nbt::IntArray(v)) = be.get("disabled_slots") {
        for &k in v {
            if let Some(i) = usize::try_from(k).ok().filter(|&i| i < 9 && items[i].is_none()) {
                d[i] = true;
            }
        }
    }
    d
}

impl Server {
    /// The 10 menu data: disabled slots (1), then the received signal.
    pub(crate) fn crafter_data(&self, p: P) -> Vec<i32> {
        let Some(be) = self.be_get(p) else { return vec![0; 10] };
        let mut d: Vec<i32> = disabled(be).iter().map(|&x| i32::from(x)).collect();
        d.push(crate::nbtio::int(be, "triggered").unwrap_or(0) as i32);
        d
    }

    /// Result shown for a crafter grid.
    pub(crate) fn crafter_result(&self, grid: &[Option<crate::player::Stack>]) -> Option<crate::player::Stack> {
        let maps = &self.maps;
        self.life.recipes.craft(&self.life.items, grid, 3, &|id| maps.scale_of(id)).map(|r| r.1)
    }

    /// Crafter menu opened at `p` (the block entity must exist).
    pub(crate) fn crafter_menu(&self, p: P, window: i32) -> Option<Menu> {
        let be = self.be_get(p)?;
        let mut m = Menu::crafter(window, read_items(be, 9));
        m.own[9] = self.crafter_result(&m.own[..9]);
        m.data = self.crafter_data(p);
        m.block = Some(p);
        Some(m)
    }

    /// Received signal kept in the block entity.
    pub(crate) fn crafter_set_triggered(&mut self, p: P, on: bool) {
        let Some(i) = self.be_index(p) else { return };
        if let Some(Some(c)) = self.chunks.get_mut(&(p[0] >> 4, p[2] >> 4))
            && let Some(be) = c.block_entities.get_mut(i)
        {
            crate::nbtio::set(be, "triggered", Nbt::Int(i32::from(on)));
        }
    }

    /// `container_slot_state_changed`: grid slot enabled or disabled by the player of the
    /// open menu; only an empty slot changes.
    pub(crate) fn crafter_slot_state(&mut self, slot: i32, window: i32, enabled: bool) {
        if self.game_mode == 3 || window != self.current_window() {
            return;
        }
        let Some(p) = self.life.menu.as_ref().filter(|m| m.kind == Kind::Crafter).and_then(|m| m.block) else {
            return;
        };
        let Some(k) = usize::try_from(slot).ok().filter(|&k| k < 9) else { return };
        let Some(be) = self.be_get(p) else { return };
        if read_items(be, 9)[k].is_some() {
            return;
        }
        let mut d = disabled(be);
        d[k] = !enabled;
        let list: Vec<i32> = (0..9).filter(|&i| d[i]).map(|i| i as i32).collect();
        if let Some(be) = self.be_get_mut(p) {
            crate::nbtio::set(be, "disabled_slots", Nbt::IntArray(list));
        }
        self.container_changed(p);
    }
}
