//! Mount inventory: saddle, armor or carpet, chest; opened by
//! `mount_screen_open` (window, columns, entity), copied onto the animal at every change,
//! closed if the animal dies, moves away or changes chest.

use rc_client::Event;

use super::super::mob::{Ent, Mob};
use super::super::species::Kind;
use super::{BODY, SADDLE, inventory_columns, inventory_size, is_camel, is_llama, is_tamed};
use crate::menus::{Kind as MenuKind, Menu, MountInfo};
use crate::server::Server;

impl Server {
    /// Rank of the entity type in the built-in registry (tag indices).
    pub(crate) fn entity_kind_index(&self, name: &str) -> Option<usize> {
        self.data.registry("minecraft:entity_type")?.entries.iter().position(|n| n == name)
    }

    /// Entity type tags that contain this species.
    fn entity_tags_of(&self, kind: Kind) -> Vec<String> {
        self.entity_tags_named(kind.info().name)
    }

    /// Entity type tags that contain this type (`minecraft:player` included).
    pub(crate) fn entity_tags_named(&self, name: &str) -> Vec<String> {
        let Some(idx) = self.entity_kind_index(name) else {
            return Vec::new();
        };
        self.regs
            .tags
            .iter()
            .find(|(r, _)| r == "minecraft:entity_type")
            .map(|(_, t)| t.iter().filter(|(_, ids)| ids.contains(&(idx as i32))).map(|(n, _)| n.clone()).collect())
            .unwrap_or_default()
    }

    /// Tamed animal (camel: always), free or ridden by the
    /// player; the old menu is closed, then `mount_screen_open` and the full contents.
    pub(crate) fn open_mount_menu(&mut self, m: &mut Mob) {
        let free = m.mount.passengers.is_empty() || m.mount.passengers.contains(&Ent::Player);
        if !is_camel(m.kind) && (!free || !is_tamed(m)) {
            return;
        }
        if self.life.menu.is_some() {
            self.close_menu(true);
        }
        self.life.counter = self.life.counter % 100 + 1;
        let window = self.life.counter;
        let tags = self.entity_tags_of(m.kind);
        let saddle_ok = tags.iter().any(|t| t == "minecraft:can_equip_saddle") && Self::saddle_slot_usable(m);
        let body_ok = tags.iter().any(|t| t == "minecraft:can_wear_horse_armor")
            || is_llama(m.kind)
            // F09: nautilus armor (tamed living adult).
            || (tags.iter().any(|t| t == "minecraft:can_wear_nautilus_armor") && Self::saddle_slot_usable(m));
        let info = MountInfo { name: m.kind.info().name.to_owned(), tags, active: [saddle_ok, body_ok] };
        let mut own = vec![m.equipment[SADDLE].clone(), m.equipment[BODY].clone()];
        own.extend(m.mount.inventory.iter().cloned());
        let menu = Menu::mount(window, m.id, info, own);
        let _ = self.send(Event::MountScreen { window, columns: inventory_columns(m), entity: m.id });
        self.life.menu = Some(menu);
        if let Some(mm) = &mut self.life.menu {
            mm.send_all(&self.slots);
        }
        self.flush_menus();
    }

    /// The usage test of the saddle slot (donkey, mule, camel: alive, adult, tamed).
    fn saddle_slot_usable(m: &Mob) -> bool {
        match m.kind {
            Kind::Horse | Kind::Llama | Kind::TraderLlama | Kind::SkeletonHorse | Kind::ZombieHorse => true,
            _ => m.alive() && !m.is_baby() && is_tamed(m),
        }
    }

    /// Menu contents copied onto the mount (saddle and armor placement sounds included).
    pub(crate) fn mount_menu_back(&mut self, entity: i32, own: &[Option<crate::player::Stack>]) {
        let Some(i) = self.mob_index(entity) else { return };
        let Some(mut m) = self.mobs.list[i].take() else { return };
        if let Some(s) = own.first() {
            if s.is_some() && m.equipment[SADDLE].is_none() {
                m.drop_chance[SADDLE] = 2.0;
            }
            self.mount_set_slot(&mut m, SADDLE, s.clone());
        }
        if let Some(s) = own.get(1) {
            if s.is_some() && m.equipment[BODY].is_none() {
                m.drop_chance[BODY] = 2.0;
            }
            self.mount_set_slot(&mut m, BODY, s.clone());
        }
        let n = inventory_size(&m);
        for k in 0..n {
            m.mount.inventory[k] = own.get(2 + k).cloned().flatten();
        }
        self.mobs.list[i] = Some(m);
    }

    /// Validity of the mount menu: animal alive, within interaction range + 4,
    /// chest unchanged; otherwise forced close.
    pub(crate) fn mount_menu_tick(&mut self) {
        let Some(MenuKind::Mount { entity, size }) = self.life.menu.as_ref().map(|m| m.kind) else { return };
        let ok = self.mob_ref(entity).is_some_and(|m| {
            let bb = m.bbox();
            let eye = [self.pos[0], self.player_eye_y(), self.pos[2]];
            let d2: f64 = (0..3)
                .map(|a| {
                    let c = eye[a].clamp(bb.min[a], bb.max[a]);
                    (eye[a] - c) * (eye[a] - c)
                })
                .sum();
            let range = if self.game_mode == 1 { 5.0 } else { 3.0 } + 4.0;
            m.alive() && !m.removed && inventory_size(m) == size && d2 < range * range
        });
        if !ok {
            self.close_menu(true);
        }
    }
}
