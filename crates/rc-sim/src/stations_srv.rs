//! Server-side stations (S7): opening from the block, validity, buttons
//! (`container_button_click`: enchanting, stonecutter recipe), anvil name
//! (`rename_item`), effects on the world (levels, anvil wear, orbs, sounds), recipe
//! sets sent to the client.

use rc_proto::Reader;
use rc_proto::varint::read_varint;

use crate::menus::{Kind, Menu};
use crate::player::Stack;
use crate::server::Server;
use crate::stations::{self, Ev, St};

/// Station of a block (short name).
pub fn station_of(short: &str) -> Option<St> {
    Some(match short {
        "brewing_stand" => St::Brewing,
        "enchanting_table" => St::Enchanting,
        "anvil" | "chipped_anvil" | "damaged_anvil" => St::Anvil,
        "grindstone" => St::Grindstone,
        "smithing_table" => St::Smithing,
        "stonecutter" => St::Stonecutter,
        "loom" => St::Loom,
        "cartography_table" => St::Cartography, // S8
        _ => return None,
    })
}

impl Server {
    /// Menu of a station opened by right click; `None`: not a station.
    pub(crate) fn station_menu(
        &mut self,
        short: &str,
        pos: [i32; 3],
        window: i32,
    ) -> Option<(Menu, Option<rc_text::Component>)> {
        let st = station_of(short)?;
        let title = |k: &str| Some(rc_text::Component::translate(k, Vec::new()));
        let m = match st {
            St::Brewing => {
                self.ensure_brewer(pos);
                let b = self.life.brewers.get(&pos).cloned().unwrap_or_default();
                let mut m = stations::open(window, st, b.items.to_vec());
                m.data = b.data().to_vec();
                (m, title("container.brewing"))
            }
            St::Enchanting => {
                let mut m = stations::open(window, st, Vec::new());
                m.st.seed = self.life.vit.xp_seed;
                m.data[3] = m.st.seed;
                m.st.bookcases = self.bookcases(pos);
                (m, title("container.enchant"))
            }
            St::Anvil => (stations::open(window, st, Vec::new()), title("container.repair")),
            St::Grindstone => (stations::open(window, st, Vec::new()), title("container.grindstone_title")),
            St::Smithing => (stations::open(window, st, Vec::new()), title("container.upgrade")),
            St::Stonecutter => (stations::open(window, st, Vec::new()), title("container.stonecutter")),
            St::Loom => (stations::open(window, st, Vec::new()), title("container.loom")),
            St::Cartography => (stations::open(window, st, Vec::new()), title("container.cartography_table")),
        };
        Some(m)
    }

    /// Bookshelves around the table: ring of 5 × 5 over two heights, each with
    /// the intermediate slot free (`enchantment_power_transmitter`).
    pub(crate) fn bookcases(&self, pos: [i32; 3]) -> i32 {
        let mut n = 0;
        for dx in -2i32..=2 {
            for dy in 0..=1 {
                for dz in -2i32..=2 {
                    if dx.abs() != 2 && dz.abs() != 2 {
                        continue;
                    }
                    let p = [pos[0] + dx, pos[1] + dy, pos[2] + dz];
                    let mid = [pos[0] + dx / 2, pos[1] + dy, pos[2] + dz / 2];
                    if self.btag_id(p, crate::tag_bits::TagId::EnchantmentPowerProvider)
                        && self.btag_id(mid, crate::tag_bits::TagId::EnchantmentPowerTransmitter)
                    {
                        n += 1;
                    }
                }
            }
        }
        n
    }

    /// Block still valid for the open station.
    pub(crate) fn station_block_ok(&self, st: St, p: [i32; 3]) -> bool {
        let name = self.block_name_s7(p).unwrap_or_default();
        let short = name.trim_start_matches("minecraft:");
        station_of(short) == Some(st) && (st != St::Brewing || self.block_entity(p).is_some())
    }

    /// Packets specific to stations; true if handled.
    pub(crate) fn station_packet(&mut self, id: i32, r: &mut Reader<'_>) -> bool {
        let ids = self.peer.ids();
        if id == ids.container_button_click {
            let (Ok(window), Ok(button)) = (read_varint(r), read_varint(r)) else { return true };
            self.station_button(window, button);
            true
        } else if id == ids.rename_item {
            let Ok(name) = rc_proto::read_str(r, 32767).map(str::to_owned) else { return true };
            self.station_rename(&name);
            true
        } else if id == ids.set_beacon {
            self.beacon_packet(r); // G5-F11
            true
        } else if id == ids.container_slot_state_changed {
            // G5-F11: crafter slot opened or closed (slot, window, open).
            use rc_proto::Decode;
            if let (Ok(slot), Ok(window), Ok(on)) = (read_varint(r), read_varint(r), bool::decode(r)) {
                self.crafter_slot_state(slot, window, on);
            }
            true
        } else {
            false
        }
    }

    fn station_button(&mut self, window: i32, button: i32) {
        if self.game_mode == 3 || window != self.current_window() || window == 0 || !self.menu_valid() {
            return;
        }
        // G6-F01: lectern buttons (pages, book taken).
        if self.life.menu.as_ref().is_some_and(|m| {
            m.kind == Kind::Container { menu_type: crate::blocks::lectern::MENU_TYPE, size: 1 }
        }) {
            self.lectern_button(button);
            return;
        }
        let Some(st) = self.life.menu.as_ref().and_then(|m| match m.kind {
            Kind::Station(s) => Some(s),
            _ => None,
        }) else {
            return;
        };
        match st {
            St::Enchanting => self.enchant_button(button),
            St::Stonecutter => {
                self.with_menu_s7(|m, inv, env| {
                    let _ = inv;
                    stations::stonecutter_select(m, env, button);
                });
            }
            St::Loom => {
                self.with_menu_s7(|m, inv, env| {
                    let _ = inv;
                    stations::loom_select(m, env, button);
                });
            }
            _ => {}
        }
        self.after_menu_action();
    }

    fn enchant_button(&mut self, button: i32) {
        if !(0..3).contains(&button) {
            return;
        }
        let creative = self.game_mode == 1;
        let Some(m) = self.life.menu.as_ref() else { return };
        let b = button as usize;
        let need = button + 1;
        let (item, lapis) = (m.own[0].clone(), m.own[1].clone());
        let cost = m.data[b];
        let level = self.life.vit.xp_level;
        if lapis.as_ref().is_none_or(|l| l.count < need) && !creative {
            return;
        }
        let Some(item) = item else { return };
        if cost <= 0 || ((level < need || level < cost) && !creative) {
            return;
        }
        let seed = m.st.seed;
        let list = self.life.stations.ench.table_list(&self.life.items, seed, button, cost, &item);
        if list.is_empty() {
            return;
        }
        // Levels removed, new player seed.
        let v = &mut self.life.vit;
        v.xp_level -= need;
        if v.xp_level < 0 {
            v.xp_level = 0;
            v.xp_progress = 0.0;
            v.xp_total = 0;
        }
        self.life.vit.xp_seed = self.life.rng.next_int();
        let mut out = item;
        if out.id == "minecraft:book" {
            out.id = "minecraft:enchanted_book".into();
        }
        for (e, l) in &list {
            let id = self.life.stations.ench.list[*e].id.clone();
            crate::enchanting::upgrade(&self.life.items, &mut out, &id, *l);
        }
        self.s9_enchanted(&out, need); // S9
        let new_seed = self.life.vit.xp_seed;
        let pos = self.life.menu.as_ref().and_then(|m| m.block);
        self.with_menu_s7(|m, inv, env| {
            m.own[0] = Some(out);
            if !creative && let Some(l) = &mut m.own[1] {
                l.count -= need;
                if l.count <= 0 {
                    m.own[1] = None;
                }
            }
            m.st.seed = new_seed;
            stations::slots_changed(m, env, inv);
        });
        if let Some(p) = pos {
            let pitch = self.life.rng.next_float() * 0.1 + 0.9;
            let at = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
            self.play_sound("minecraft:block.enchantment_table.use", 4, at, 1.0, pitch);
        }
        self.life.vit.force_resend(); // level changed without changing the total
        self.send_health();
    }

    fn station_rename(&mut self, name: &str) {
        let Some(m) = &self.life.menu else { return };
        if m.kind != Kind::Station(St::Anvil) || !self.menu_valid() {
            return;
        }
        let name = name.to_owned();
        self.with_menu_s7(|m, inv, env| {
            stations::anvil_rename(m, env, inv, &name);
        });
        self.after_menu_action();
    }

    fn with_menu_s7(&mut self, f: impl FnOnce(&mut Menu, &mut [Option<Stack>], &mut crate::life::MenuEnv<'_>)) {
        self.with_menu(true, f);
    }

    /// Effects passed up by the stations after a click.
    pub(crate) fn station_events(&mut self, evs: Vec<Ev>) {
        let pos = self.life.menu.as_ref().and_then(|m| m.block);
        let center = pos.map(|p| [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5]);
        for ev in evs {
            match ev {
                Ev::SpendLevels(n) => {
                    let v = &mut self.life.vit;
                    v.xp_level = v.xp_level.saturating_sub(n);
                    if v.xp_level < 0 {
                        v.xp_level = 0;
                        v.xp_progress = 0.0;
                        v.xp_total = 0;
                    }
                    self.life.vit.force_resend();
                    self.send_health();
                }
                Ev::AnvilUsed => {
                    let Some(p) = pos else { continue };
                    let creative = self.game_mode == 1;
                    let name = self.block_name_s7(p).unwrap_or_default();
                    let is_anvil = name.ends_with("anvil");
                    if !creative && is_anvil && self.life.rng.next_float() < 0.12 {
                        let next = match name.as_str() {
                            "minecraft:anvil" => Some("minecraft:chipped_anvil"),
                            "minecraft:chipped_anvil" => Some("minecraft:damaged_anvil"),
                            _ => None,
                        };
                        match next.and_then(|n| self.same_props_state(p, n)) {
                            Some(s) => {
                                self.set_block(p, s, 2);
                                self.level_event(1030, p, 0);
                            }
                            None => {
                                self.set_block(p, 0, 3);
                                self.level_event(1029, p, 0);
                            }
                        }
                    } else {
                        self.level_event(1030, p, 0);
                    }
                }
                Ev::Grindstone(xp) => {
                    if let (Some(c), Some(p)) = (center, pos) {
                        if xp > 0 {
                            self.spawn_orbs(c, xp);
                        }
                        self.level_event(1042, p, 0);
                    }
                }
                Ev::Smithed(recipe) => {
                    if let Some(r) = recipe {
                        self.award_recipes(&[r]);
                    }
                    if let Some(p) = pos {
                        self.level_event(1044, p, 0);
                    }
                }
                Ev::StoneCut(recipe) => {
                    if let Some(r) = recipe {
                        self.award_recipes(&[r]);
                    }
                    let tick = self.tick;
                    if let Some(c) = center
                        && let Some(m) = self.life.menu.as_mut()
                        && m.st.last_sound != tick
                    {
                        m.st.last_sound = tick;
                        self.play_sound("minecraft:ui.stonecutter.take_result", 4, c, 1.0, 1.0);
                    }
                }
                Ev::LoomTake => {
                    let tick = self.tick;
                    if let Some(c) = center
                        && let Some(m) = self.life.menu.as_mut()
                        && m.st.last_sound != tick
                    {
                        m.st.last_sound = tick;
                        self.play_sound("minecraft:ui.loom.take_result", 4, c, 1.0, 1.0);
                    }
                }
                // S8: cartography table.
                Ev::CartographyTake => {
                    let tick = self.tick;
                    if let Some(c) = center
                        && let Some(m) = self.life.menu.as_mut()
                        && m.st.last_sound != tick
                    {
                        m.st.last_sound = tick;
                        self.play_sound("minecraft:ui.cartography_table.take_result", 4, c, 1.0, 1.0);
                    }
                }
            }
        }
    }

    /// State of block `name` with the common properties of the present state (orientation).
    fn same_props_state(&self, p: [i32; 3], name: &str) -> Option<u32> {
        let s = self.state_at(p)?;
        let props: Vec<(String, rc_proto::nbt::Nbt)> = self
            .data
            .state_properties(s)
            .map(|(k, v)| (k.to_owned(), rc_proto::nbt::Nbt::String(v.to_owned())))
            .collect();
        let entry = rc_proto::nbt::Nbt::Compound(vec![
            ("id".into(), rc_proto::nbt::Nbt::String(name.to_owned())),
            ("properties".into(), rc_proto::nbt::Nbt::Compound(props)),
        ]);
        self.states.state_of(&self.data, &entry)
    }

    /// Station sets added to `update_recipes`: brewing stand, smithing, stonecutter.
    pub(crate) fn station_recipe_sets(&self, rec: &mut rc_proto_v777::special::Recipes) {
        let d = &self.life.stations;
        rec.sets.push(("minecraft:brewing_input".into(), d.brew_inputs.clone()));
        rec.sets.push(("minecraft:brewing_reagent".into(), d.brew_reagents.clone()));
        rec.sets.push(("minecraft:smithing_template".into(), d.smith_sets[0].clone()));
        rec.sets.push(("minecraft:smithing_base".into(), d.smith_sets[1].clone()));
        rec.sets.push(("minecraft:smithing_addition".into(), d.smith_sets[2].clone()));
        for (ing, out, _) in &d.stonecut {
            rec.stonecutter.push((rc_proto_v777::special::Holders::Items(ing.clone()), self.net(Some(out))));
        }
    }
}
